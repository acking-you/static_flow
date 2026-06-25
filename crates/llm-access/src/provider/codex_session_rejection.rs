//! In-memory strict Codex key/session rejection cache.

use std::{
    num::NonZeroUsize,
    sync::Mutex,
    time::{Duration, Instant},
};

use lru::LruCache;

use super::{
    codex_session_affinity::{CodexAffinityId, CodexAffinityRuntimeConfig, CodexAffinitySource},
    codex_upstream_error::CodexUpstreamErrorClass,
};

#[derive(Debug, Clone)]
pub(crate) struct CodexSessionRejectionEntry {
    pub error_class: CodexUpstreamErrorClass,
    pub message: String,
    pub account_name: String,
    pub blocked_at: Instant,
    pub expires_at: Instant,
}

struct CodexSessionRejectionInner {
    capacity: usize,
    entries: LruCache<String, CodexSessionRejectionEntry>,
}

pub(crate) struct CodexSessionRejection {
    inner: Mutex<CodexSessionRejectionInner>,
}

impl Default for CodexSessionRejection {
    fn default() -> Self {
        Self::new(llm_access_core::store::DEFAULT_CODEX_SESSION_AFFINITY_MAX_ENTRIES as usize)
    }
}

impl CodexSessionRejection {
    fn new(max_entries: usize) -> Self {
        let capacity = max_entries.max(1);
        Self {
            inner: Mutex::new(CodexSessionRejectionInner {
                capacity,
                entries: LruCache::new(NonZeroUsize::new(capacity).expect("capacity is non-zero")),
            }),
        }
    }

    pub(crate) fn lookup(
        &self,
        affinity_id: &CodexAffinityId,
        config: &CodexAffinityRuntimeConfig,
    ) -> Option<CodexSessionRejectionEntry> {
        if config.max_entries == 0 {
            return None;
        }
        let mut inner = self.reconfigure(config);
        let entry = inner.entries.get(&affinity_id.key)?;
        if Instant::now() >= entry.expires_at {
            inner.entries.pop(&affinity_id.key);
            return None;
        }
        Some(entry.clone())
    }

    pub(crate) fn remember(
        &self,
        affinity_id: &CodexAffinityId,
        error_class: CodexUpstreamErrorClass,
        message: &str,
        account_name: &str,
        config: &CodexAffinityRuntimeConfig,
    ) {
        let ttl = ttl_for_source(affinity_id.source, config);
        if ttl.is_zero() || config.max_entries == 0 {
            return;
        }
        let now = Instant::now();
        let mut inner = self.reconfigure(config);
        inner
            .entries
            .put(affinity_id.key.clone(), CodexSessionRejectionEntry {
                error_class,
                message: message.to_string(),
                account_name: account_name.to_string(),
                blocked_at: now,
                expires_at: now + ttl,
            });
    }

    fn reconfigure(
        &self,
        config: &CodexAffinityRuntimeConfig,
    ) -> std::sync::MutexGuard<'_, CodexSessionRejectionInner> {
        let capacity = config.max_entries.max(1);
        let mut inner = self.inner.lock().expect("codex session rejection mutex");
        if inner.capacity != capacity {
            inner.capacity = capacity;
            inner
                .entries
                .resize(NonZeroUsize::new(capacity).expect("capacity is non-zero"));
        }
        inner
    }
}

fn ttl_for_source(source: CodexAffinitySource, config: &CodexAffinityRuntimeConfig) -> Duration {
    match source {
        CodexAffinitySource::Explicit => config.session_ttl,
        CodexAffinitySource::Derived => config.fallback_ttl,
    }
}
