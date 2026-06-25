//! In-memory strict Codex key/session rejection cache.

use std::{num::NonZeroUsize, sync::Mutex, time::Instant};

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
        if affinity_id.source != CodexAffinitySource::Explicit {
            return None;
        }
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
        if affinity_id.source != CodexAffinitySource::Explicit {
            return;
        }
        let ttl = config.session_ttl;
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

#[cfg(test)]
mod tests {
    use std::{thread, time::Duration};

    use super::*;

    fn affinity_id(key: &str, source: CodexAffinitySource) -> CodexAffinityId {
        CodexAffinityId {
            key: key.to_string(),
            source,
        }
    }

    fn config(max_entries: usize, session_ttl: Duration) -> CodexAffinityRuntimeConfig {
        CodexAffinityRuntimeConfig {
            max_entries,
            session_ttl,
            ..CodexAffinityRuntimeConfig::default()
        }
    }

    #[test]
    fn remembers_and_looks_up_explicit_rejection() {
        let cache = CodexSessionRejection::new(8);
        let config = config(8, Duration::from_secs(60));
        let id = affinity_id("key:session", CodexAffinitySource::Explicit);

        cache.remember(&id, CodexUpstreamErrorClass::CyberPolicy, "blocked", "account-a", &config);

        let entry = cache.lookup(&id, &config).expect("explicit rejection");
        assert_eq!(entry.error_class, CodexUpstreamErrorClass::CyberPolicy);
        assert_eq!(entry.message, "blocked");
        assert_eq!(entry.account_name, "account-a");
    }

    #[test]
    fn ignores_derived_rejection_ids() {
        let cache = CodexSessionRejection::new(8);
        let config = config(8, Duration::from_secs(60));
        let id = affinity_id("key:derived-prefix", CodexAffinitySource::Derived);

        cache.remember(&id, CodexUpstreamErrorClass::CyberPolicy, "blocked", "account-a", &config);

        assert!(cache.lookup(&id, &config).is_none());
    }

    #[test]
    fn zero_max_entries_disables_rejections() {
        let cache = CodexSessionRejection::new(8);
        let config = config(0, Duration::from_secs(60));
        let id = affinity_id("key:session", CodexAffinitySource::Explicit);

        cache.remember(&id, CodexUpstreamErrorClass::CyberPolicy, "blocked", "account-a", &config);

        assert!(cache.lookup(&id, &config).is_none());
    }

    #[test]
    fn expired_rejection_is_removed() {
        let cache = CodexSessionRejection::new(8);
        let config = config(8, Duration::from_millis(1));
        let id = affinity_id("key:session", CodexAffinitySource::Explicit);

        cache.remember(&id, CodexUpstreamErrorClass::CyberPolicy, "blocked", "account-a", &config);
        thread::sleep(Duration::from_millis(5));

        assert!(cache.lookup(&id, &config).is_none());
    }

    #[test]
    fn resize_keeps_existing_entries_when_capacity_allows() {
        let cache = CodexSessionRejection::new(2);
        let small = config(2, Duration::from_secs(60));
        let large = config(4, Duration::from_secs(60));
        let id = affinity_id("key:session", CodexAffinitySource::Explicit);

        cache.remember(&id, CodexUpstreamErrorClass::CyberPolicy, "blocked", "account-a", &small);

        assert!(cache.lookup(&id, &large).is_some());
    }
}
