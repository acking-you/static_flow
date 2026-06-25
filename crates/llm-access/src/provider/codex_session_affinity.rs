//! In-process Codex account affinity keyed by resolved session ids.

use std::{
    collections::HashMap,
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use llm_access_codex::types::CodexResolvedSessionSource;
use llm_access_core::store::{
    AdminRuntimeConfig, DEFAULT_CODEX_FALLBACK_AFFINITY_ENABLED,
    DEFAULT_CODEX_FALLBACK_AFFINITY_MIN_BODY_BYTES, DEFAULT_CODEX_FALLBACK_AFFINITY_PREFIX_BYTES,
    DEFAULT_CODEX_FALLBACK_AFFINITY_TTL_SECONDS, DEFAULT_CODEX_SESSION_AFFINITY_ENABLED,
    DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES, DEFAULT_CODEX_SESSION_AFFINITY_TTL_SECONDS,
};
use lru::LruCache;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAffinityRuntimeConfig {
    pub session_enabled: bool,
    pub max_entries: usize,
    pub session_ttl: Duration,
    pub fallback_enabled: bool,
    pub fallback_ttl: Duration,
    pub fallback_prefix_bytes: usize,
    pub fallback_min_body_bytes: usize,
}

impl Default for CodexAffinityRuntimeConfig {
    fn default() -> Self {
        Self {
            session_enabled: DEFAULT_CODEX_SESSION_AFFINITY_ENABLED,
            max_entries: DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize,
            session_ttl: Duration::from_secs(DEFAULT_CODEX_SESSION_AFFINITY_TTL_SECONDS),
            fallback_enabled: DEFAULT_CODEX_FALLBACK_AFFINITY_ENABLED,
            fallback_ttl: Duration::from_secs(DEFAULT_CODEX_FALLBACK_AFFINITY_TTL_SECONDS),
            fallback_prefix_bytes: DEFAULT_CODEX_FALLBACK_AFFINITY_PREFIX_BYTES as usize,
            fallback_min_body_bytes: DEFAULT_CODEX_FALLBACK_AFFINITY_MIN_BODY_BYTES as usize,
        }
    }
}

impl CodexAffinityRuntimeConfig {
    pub(crate) fn from_admin_config(config: &AdminRuntimeConfig) -> Self {
        Self {
            session_enabled: config.codex_session_affinity_enabled,
            max_entries: usize::try_from(config.codex_session_affinity_max_entries)
                .unwrap_or(usize::MAX),
            session_ttl: Duration::from_secs(config.codex_session_affinity_ttl_seconds),
            fallback_enabled: config.codex_fallback_affinity_enabled,
            fallback_ttl: Duration::from_secs(config.codex_fallback_affinity_ttl_seconds),
            fallback_prefix_bytes: usize::try_from(config.codex_fallback_affinity_prefix_bytes)
                .unwrap_or(usize::MAX),
            fallback_min_body_bytes: usize::try_from(config.codex_fallback_affinity_min_body_bytes)
                .unwrap_or(usize::MAX),
        }
    }

    fn ttl_for_source(&self, source: CodexAffinitySource) -> Duration {
        match source {
            CodexAffinitySource::Explicit => self.session_ttl,
            CodexAffinitySource::Derived => self.fallback_ttl,
        }
    }

    fn can_store_source(&self, source: CodexAffinitySource) -> bool {
        let source_enabled = match source {
            CodexAffinitySource::Explicit => self.session_enabled,
            CodexAffinitySource::Derived => self.session_enabled && self.fallback_enabled,
        };
        source_enabled && self.max_entries > 0 && !self.ttl_for_source(source).is_zero()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodexAffinitySource {
    Explicit,
    Derived,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexAffinityId {
    pub key: String,
    pub source: CodexAffinitySource,
}

#[derive(Debug, Clone)]
struct CodexAffinityEntry {
    account_name: String,
    source: CodexAffinitySource,
    last_seen: Instant,
}

struct CodexSessionAffinityInner {
    capacity: usize,
    entries: LruCache<String, CodexAffinityEntry>,
}

pub(crate) struct CodexSessionAffinity {
    inner: Mutex<CodexSessionAffinityInner>,
}

impl Default for CodexSessionAffinity {
    fn default() -> Self {
        Self::new(DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize)
    }
}

impl CodexSessionAffinity {
    fn new(max_entries: usize) -> Self {
        let capacity = max_entries.max(1);
        Self {
            inner: Mutex::new(CodexSessionAffinityInner {
                capacity,
                entries: LruCache::new(NonZeroUsize::new(capacity).expect("capacity is non-zero")),
            }),
        }
    }

    pub(crate) fn lookup(
        &self,
        affinity_id: &CodexAffinityId,
        config: &CodexAffinityRuntimeConfig,
    ) -> Option<String> {
        if !config.can_store_source(affinity_id.source) {
            return None;
        }
        let mut inner = self.reconfigure(config);
        let entry = inner.entries.get(&affinity_id.key)?;
        if entry.source != affinity_id.source
            || entry.last_seen.elapsed() > config.ttl_for_source(entry.source)
        {
            inner.entries.pop(&affinity_id.key);
            return None;
        }
        Some(entry.account_name.clone())
    }

    pub(crate) fn remember(
        &self,
        affinity_id: &CodexAffinityId,
        account_name: &str,
        config: &CodexAffinityRuntimeConfig,
    ) {
        if !config.can_store_source(affinity_id.source) {
            return;
        }
        let mut inner = self.reconfigure(config);
        inner
            .entries
            .put(affinity_id.key.clone(), CodexAffinityEntry {
                account_name: account_name.to_string(),
                source: affinity_id.source,
                last_seen: Instant::now(),
            });
    }

    pub(crate) fn account_session_counts(
        &self,
        config: &CodexAffinityRuntimeConfig,
    ) -> HashMap<String, usize> {
        let inner = self.reconfigure(config);
        inner
            .entries
            .iter()
            .filter(|(_, entry)| {
                config.can_store_source(entry.source)
                    && entry.last_seen.elapsed() <= config.ttl_for_source(entry.source)
            })
            .fold(HashMap::new(), |mut counts, (_, entry)| {
                if let Some(count) = counts.get_mut(entry.account_name.as_str()) {
                    *count += 1;
                } else {
                    counts.insert(entry.account_name.clone(), 1);
                }
                counts
            })
    }

    fn reconfigure(
        &self,
        config: &CodexAffinityRuntimeConfig,
    ) -> std::sync::MutexGuard<'_, CodexSessionAffinityInner> {
        let capacity = config.max_entries.max(1);
        let mut inner = self.inner.lock().expect("codex session affinity mutex");
        if inner.capacity != capacity {
            inner.capacity = capacity;
            inner
                .entries
                .resize(NonZeroUsize::new(capacity).expect("capacity is non-zero"));
        }
        inner
    }
}

pub(crate) fn build_codex_affinity_id(
    key_id: &str,
    session_id: Option<&str>,
    session_source: Option<CodexResolvedSessionSource>,
) -> Option<CodexAffinityId> {
    let key_id = key_id.trim();
    let session_id = session_id
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    if key_id.is_empty() {
        return None;
    }
    let source = match session_source {
        Some(source) if source.is_derived() => CodexAffinitySource::Derived,
        Some(_) => CodexAffinitySource::Explicit,
        None => return None,
    };
    Some(CodexAffinityId {
        key: affinity_key(key_id, session_id),
        source,
    })
}

fn affinity_key(key_id: &str, session_id: &str) -> String {
    format!("{}:{key_id}{session_id}", key_id.len())
}
