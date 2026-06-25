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

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> CodexAffinityRuntimeConfig {
        CodexAffinityRuntimeConfig {
            session_enabled: true,
            max_entries: 16,
            session_ttl: Duration::from_secs(60),
            fallback_enabled: true,
            fallback_ttl: Duration::from_secs(60),
            fallback_prefix_bytes: 0,
            fallback_min_body_bytes: 0,
        }
    }

    #[test]
    fn remember_then_recover_returns_stored_session() {
        let recovery = CodexSessionRecovery::new(16);
        let config = config();

        assert_eq!(
            recovery.remember("key", "anchor", "session-a", &config),
            CodexSessionRecoveryStoreResult::Stored
        );
        assert_eq!(
            recovery.recover("key", "anchor", &config),
            CodexSessionRecoveryLookup::Hit("session-a".to_string())
        );
        assert_eq!(recovery.recover("key", "missing", &config), CodexSessionRecoveryLookup::Miss);
    }

    #[test]
    fn expired_recovery_entry_is_removed() {
        let recovery = CodexSessionRecovery::new(16);
        let mut config = config();
        config.fallback_ttl = Duration::from_millis(1);

        assert_eq!(
            recovery.remember("key", "anchor", "session-a", &config),
            CodexSessionRecoveryStoreResult::Stored
        );
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(recovery.recover("key", "anchor", &config), CodexSessionRecoveryLookup::Expired);
        assert_eq!(recovery.recover("key", "anchor", &config), CodexSessionRecoveryLookup::Miss);
    }

    #[test]
    fn disabled_config_reports_specific_reason() {
        let recovery = CodexSessionRecovery::new(16);
        let mut cases = Vec::new();

        let mut session_disabled = config();
        session_disabled.session_enabled = false;
        cases.push((session_disabled, CodexSessionRecoveryConfigState::SessionAffinityDisabled));

        let mut fallback_disabled = config();
        fallback_disabled.fallback_enabled = false;
        cases.push((fallback_disabled, CodexSessionRecoveryConfigState::FallbackDisabled));

        let mut empty_capacity = config();
        empty_capacity.max_entries = 0;
        cases.push((empty_capacity, CodexSessionRecoveryConfigState::EmptyCapacity));

        let mut empty_ttl = config();
        empty_ttl.fallback_ttl = Duration::ZERO;
        cases.push((empty_ttl, CodexSessionRecoveryConfigState::EmptyTtl));

        for (config, state) in cases {
            assert_eq!(
                recovery.recover("key", "anchor", &config),
                CodexSessionRecoveryLookup::Disabled(state)
            );
            assert_eq!(
                recovery.remember("key", "anchor", "session-a", &config),
                CodexSessionRecoveryStoreResult::Disabled(state)
            );
        }
    }

    #[test]
    fn remember_rejects_invalid_key_and_empty_session() {
        let recovery = CodexSessionRecovery::new(16);
        let config = config();

        assert_eq!(
            recovery.remember("", "anchor", "session-a", &config),
            CodexSessionRecoveryStoreResult::InvalidKey
        );
        assert_eq!(
            recovery.remember("key", "", "session-a", &config),
            CodexSessionRecoveryStoreResult::InvalidKey
        );
        assert_eq!(
            recovery.remember("key", "anchor", "   ", &config),
            CodexSessionRecoveryStoreResult::EmptySession
        );
        assert_eq!(recovery.recover("", "anchor", &config), CodexSessionRecoveryLookup::InvalidKey);
    }

    #[test]
    fn capacity_evicts_least_recently_used_entry() {
        let recovery = CodexSessionRecovery::new(16);
        let mut config = config();
        config.max_entries = 1;

        assert_eq!(
            recovery.remember("key", "anchor-a", "session-a", &config),
            CodexSessionRecoveryStoreResult::Stored
        );
        assert_eq!(
            recovery.remember("key", "anchor-b", "session-b", &config),
            CodexSessionRecoveryStoreResult::Stored
        );

        assert_eq!(recovery.recover("key", "anchor-a", &config), CodexSessionRecoveryLookup::Miss);
        assert_eq!(
            recovery.recover("key", "anchor-b", &config),
            CodexSessionRecoveryLookup::Hit("session-b".to_string())
        );
    }

    #[test]
    fn recovery_key_length_prefix_disambiguates_parts() {
        assert_ne!(recovery_key("a:1", "bc"), recovery_key("a", "1bc"));
        assert_ne!(recovery_key("12", "3"), recovery_key("1", "23"));
    }
}
