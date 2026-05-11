//! Recoverable continuation anchors for Codex responses compatibility.

use std::{
    collections::{HashMap, VecDeque},
    fs,
    path::PathBuf,
    sync::Mutex,
    time::Instant,
};

use anyhow::Context;
use llm_access_codex::continuation::StoredResponseAnchor;
use serde::{Deserialize, Serialize};

pub const DEFAULT_CODEX_RESPONSE_ANCHOR_MAX_ENTRIES: usize = 4_096;

#[derive(Debug, Clone)]
struct AnchorEntry {
    anchor: StoredResponseAnchor,
}

#[derive(Debug, Default)]
struct AnchorIndex {
    entries: HashMap<String, AnchorEntry>,
    lru: VecDeque<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAnchorSnapshot {
    anchors: Vec<StoredResponseAnchor>,
}

/// Recoverable local response anchors keyed by client-visible
/// `previous_response_id`.
pub struct CodexResponseAnchors {
    inner: Mutex<AnchorIndex>,
    persistence_path: Option<PathBuf>,
}

impl Default for CodexResponseAnchors {
    fn default() -> Self {
        Self {
            inner: Mutex::new(AnchorIndex::default()),
            persistence_path: None,
        }
    }
}

impl CodexResponseAnchors {
    pub fn open_persistent(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create codex response anchor directory `{}`", parent.display())
            })?;
        }
        let inner = load_index_from_path(&path).unwrap_or_else(|err| {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to load codex response anchors, starting empty"
            );
            AnchorIndex::default()
        });
        Ok(Self {
            inner: Mutex::new(inner),
            persistence_path: Some(path),
        })
    }

    pub fn get(&self, response_id: &str, _now: Instant) -> Option<StoredResponseAnchor> {
        let mut inner = self.inner.lock().expect("codex anchor mutex poisoned");
        let anchor = inner.entries.get(response_id)?.anchor.clone();
        touch_lru(&mut inner.lru, response_id);
        Some(anchor)
    }

    pub fn insert(&self, anchor: StoredResponseAnchor, _now: Instant) {
        let mut inner = self.inner.lock().expect("codex anchor mutex poisoned");
        let response_id = anchor.client_response_id.clone();
        inner.entries.insert(response_id.clone(), AnchorEntry {
            anchor,
        });
        touch_lru(&mut inner.lru, &response_id);
        while inner.entries.len() > DEFAULT_CODEX_RESPONSE_ANCHOR_MAX_ENTRIES {
            let Some(oldest) = inner.lru.pop_front() else {
                break;
            };
            inner.entries.remove(&oldest);
        }
        if let Err(err) = persist_index(&inner, self.persistence_path.as_ref()) {
            tracing::warn!(error = %err, "failed to persist codex response anchors");
        }
    }
}

fn load_index_from_path(path: &PathBuf) -> anyhow::Result<AnchorIndex> {
    if !path.exists() {
        return Ok(AnchorIndex::default());
    }
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read codex response anchors `{}`", path.display()))?;
    if bytes.is_empty() {
        return Ok(AnchorIndex::default());
    }
    let snapshot: PersistedAnchorSnapshot = serde_json::from_slice(&bytes)
        .with_context(|| format!("failed to decode codex response anchors `{}`", path.display()))?;
    let mut index = AnchorIndex::default();
    for anchor in snapshot.anchors {
        let response_id = anchor.client_response_id.clone();
        if index.entries.contains_key(&response_id) {
            continue;
        }
        index.lru.push_back(response_id.clone());
        index.entries.insert(response_id, AnchorEntry {
            anchor,
        });
    }
    Ok(index)
}

fn persist_index(index: &AnchorIndex, path: Option<&PathBuf>) -> anyhow::Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    let anchors = index
        .lru
        .iter()
        .filter_map(|response_id| index.entries.get(response_id))
        .map(|entry| entry.anchor.clone())
        .collect();
    let snapshot = PersistedAnchorSnapshot {
        anchors,
    };
    let bytes = serde_json::to_vec(&snapshot)
        .with_context(|| format!("failed to encode codex response anchors `{}`", path.display()))?;
    let temp_path = path.with_extension("json.tmp");
    fs::write(&temp_path, bytes).with_context(|| {
        format!("failed to write temporary codex response anchors `{}`", temp_path.display())
    })?;
    fs::rename(&temp_path, path).with_context(|| {
        format!("failed to replace codex response anchors `{}`", path.display())
    })?;
    Ok(())
}

fn touch_lru(lru: &mut VecDeque<String>, key: &str) {
    if let Some(pos) = lru.iter().position(|existing| existing == key) {
        lru.remove(pos);
    }
    lru.push_back(key.to_string());
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use llm_access_codex::continuation::StoredResponseAnchor;
    use serde_json::json;

    use super::{CodexResponseAnchors, DEFAULT_CODEX_RESPONSE_ANCHOR_MAX_ENTRIES};

    #[test]
    fn anchor_store_round_trips_recent_entry() {
        let store = CodexResponseAnchors::default();
        let now = Instant::now();
        store.insert(
            StoredResponseAnchor {
                client_response_id: "resp_test".to_string(),
                history_items: vec![json!({"type":"message","role":"user"})],
            },
            now,
        );

        let recovered = store.get("resp_test", now).expect("anchor exists");
        assert_eq!(recovered.client_response_id, "resp_test");
    }

    #[test]
    fn anchor_store_eviction_keeps_latest_entries() {
        let store = CodexResponseAnchors::default();
        let now = Instant::now();
        for index in 0..=DEFAULT_CODEX_RESPONSE_ANCHOR_MAX_ENTRIES {
            store.insert(
                StoredResponseAnchor {
                    client_response_id: format!("resp_{index}"),
                    history_items: vec![],
                },
                now,
            );
        }

        assert!(store.get("resp_0", now).is_none());
        assert!(store
            .get(&format!("resp_{DEFAULT_CODEX_RESPONSE_ANCHOR_MAX_ENTRIES}"), now)
            .is_some());
    }

    #[test]
    fn anchor_store_recovers_entry_after_reopen() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("codex-response-anchors.json");
        let now = Instant::now();
        {
            let store =
                CodexResponseAnchors::open_persistent(path.clone()).expect("open persistent store");
            store.insert(
                StoredResponseAnchor {
                    client_response_id: "resp_persisted".to_string(),
                    history_items: vec![json!({"type":"message","role":"user"})],
                },
                now,
            );
        }

        let reopened =
            CodexResponseAnchors::open_persistent(path).expect("reopen persistent store");
        let recovered = reopened
            .get("resp_persisted", now)
            .expect("persisted anchor exists");
        assert_eq!(recovered.client_response_id, "resp_persisted");
    }
}
