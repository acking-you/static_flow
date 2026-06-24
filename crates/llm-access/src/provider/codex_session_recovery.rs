//! In-process Codex synthetic-session recovery keyed by prompt anchors.

use std::{
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use lru::LruCache;

use super::CodexAffinityRuntimeConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexSessionRecoveryConfigState {
    Enabled,
    SessionAffinityDisabled,
    FallbackDisabled,
    EmptyCapacity,
    EmptyTtl,
}

impl CodexSessionRecoveryConfigState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::SessionAffinityDisabled => "session_affinity_disabled",
            Self::FallbackDisabled => "fallback_disabled",
            Self::EmptyCapacity => "empty_capacity",
            Self::EmptyTtl => "empty_ttl",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CodexSessionRecoveryLookup {
    Disabled(CodexSessionRecoveryConfigState),
    InvalidKey,
    Expired,
    Miss,
    Hit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexSessionRecoveryStoreResult {
    Disabled(CodexSessionRecoveryConfigState),
    InvalidKey,
    EmptySession,
    Stored,
}

#[derive(Debug, Clone)]
struct CodexSessionRecoveryEntry {
    session_id: String,
    last_seen: Instant,
}

struct CodexSessionRecoveryInner {
    capacity: usize,
    entries: LruCache<String, CodexSessionRecoveryEntry>,
}

pub(crate) struct CodexSessionRecovery {
    inner: Mutex<CodexSessionRecoveryInner>,
}

impl Default for CodexSessionRecovery {
    fn default() -> Self {
        Self::new(llm_access_core::store::DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize)
    }
}

impl CodexSessionRecovery {
    fn new(max_entries: usize) -> Self {
        let capacity = max_entries.max(1);
        Self {
            inner: Mutex::new(CodexSessionRecoveryInner {
                capacity,
                entries: LruCache::new(NonZeroUsize::new(capacity).expect("capacity is non-zero")),
            }),
        }
    }

    pub(crate) fn recover(
        &self,
        key_id: &str,
        lookup_anchor_hash: &str,
        config: &CodexAffinityRuntimeConfig,
    ) -> CodexSessionRecoveryLookup {
        let config_state = recovery_config_state(config);
        if config_state != CodexSessionRecoveryConfigState::Enabled {
            return CodexSessionRecoveryLookup::Disabled(config_state);
        }
        let Some(key) = recovery_key(key_id, lookup_anchor_hash) else {
            return CodexSessionRecoveryLookup::InvalidKey;
        };
        let mut inner = self.reconfigure(config);
        let Some(entry) = inner.entries.get(&key) else {
            return CodexSessionRecoveryLookup::Miss;
        };
        if entry.last_seen.elapsed() > config.fallback_ttl {
            inner.entries.pop(&key);
            return CodexSessionRecoveryLookup::Expired;
        }
        CodexSessionRecoveryLookup::Hit(entry.session_id.clone())
    }

    pub(crate) fn remember(
        &self,
        key_id: &str,
        anchor_hash: &str,
        session_id: &str,
        config: &CodexAffinityRuntimeConfig,
    ) -> CodexSessionRecoveryStoreResult {
        let config_state = recovery_config_state(config);
        if config_state != CodexSessionRecoveryConfigState::Enabled {
            return CodexSessionRecoveryStoreResult::Disabled(config_state);
        }
        let Some(key) = recovery_key(key_id, anchor_hash) else {
            return CodexSessionRecoveryStoreResult::InvalidKey;
        };
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return CodexSessionRecoveryStoreResult::EmptySession;
        }
        let mut inner = self.reconfigure(config);
        inner.entries.put(key, CodexSessionRecoveryEntry {
            session_id: session_id.to_string(),
            last_seen: Instant::now(),
        });
        CodexSessionRecoveryStoreResult::Stored
    }

    fn reconfigure(
        &self,
        config: &CodexAffinityRuntimeConfig,
    ) -> std::sync::MutexGuard<'_, CodexSessionRecoveryInner> {
        let capacity = config.max_entries.max(1);
        let mut inner = self.inner.lock().expect("codex session recovery mutex");
        if inner.capacity != capacity {
            inner.capacity = capacity;
            inner
                .entries
                .resize(NonZeroUsize::new(capacity).expect("capacity is non-zero"));
        }
        inner
    }
}

fn recovery_config_state(config: &CodexAffinityRuntimeConfig) -> CodexSessionRecoveryConfigState {
    if !config.session_enabled {
        return CodexSessionRecoveryConfigState::SessionAffinityDisabled;
    }
    if !config.fallback_enabled {
        return CodexSessionRecoveryConfigState::FallbackDisabled;
    }
    if config.max_entries == 0 {
        return CodexSessionRecoveryConfigState::EmptyCapacity;
    }
    if config.fallback_ttl <= Duration::ZERO {
        return CodexSessionRecoveryConfigState::EmptyTtl;
    }
    CodexSessionRecoveryConfigState::Enabled
}

fn recovery_key(key_id: &str, anchor_hash: &str) -> Option<String> {
    let key_id = key_id.trim();
    let anchor_hash = anchor_hash.trim();
    if key_id.is_empty() || anchor_hash.is_empty() {
        return None;
    }
    Some(format!("{}:{key_id}{anchor_hash}", key_id.len()))
}
