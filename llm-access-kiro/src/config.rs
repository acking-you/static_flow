//! Kiro runtime configuration contracts.

use std::{sync::Arc, time::Duration};

use parking_lot::RwLock;

use crate::{
    auth_file::{DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY, DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS},
    cache_sim::{KiroCacheSimulationConfig, KiroCacheSimulationMode},
};

/// Default minimum interval between Kiro account status refresh passes.
pub const DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
/// Default maximum interval between Kiro account status refresh passes.
pub const DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
/// Default per-account status refresh jitter.
pub const DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
/// Default Kiro prefix-cache simulation mode.
pub const DEFAULT_KIRO_PREFIX_CACHE_MODE: &str = "prefix_tree";
/// Default maximum prompt tokens retained by the prefix-cache simulator.
pub const DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS: u64 = 4_000_000;
/// Default TTL for prefix-cache entries.
pub const DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS: u64 = 6 * 60 * 60;
/// Default number of recoverable conversation anchors.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES: u64 = 20_000;
/// Default TTL for recoverable conversation anchors.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS: u64 = 24 * 60 * 60;

/// Runtime settings used by Kiro provider scheduling, status refresh, and
/// cache simulation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroRuntimeConfig {
    /// Default per-account upstream concurrency.
    pub kiro_channel_max_concurrency: u64,
    /// Default per-account upstream request start interval.
    pub kiro_channel_min_start_interval_ms: u64,
    /// Minimum interval between status refresh passes.
    pub kiro_status_refresh_min_interval_seconds: u64,
    /// Maximum interval between status refresh passes.
    pub kiro_status_refresh_max_interval_seconds: u64,
    /// Maximum per-account status refresh jitter.
    pub kiro_status_account_jitter_max_seconds: u64,
    /// Prefix-cache simulation mode.
    pub kiro_prefix_cache_mode: String,
    /// Maximum prompt tokens retained by the prefix-cache simulator.
    pub kiro_prefix_cache_max_tokens: u64,
    /// TTL for prefix-cache entries.
    pub kiro_prefix_cache_entry_ttl_seconds: u64,
    /// Maximum recoverable conversation anchors.
    pub kiro_conversation_anchor_max_entries: u64,
    /// TTL for recoverable conversation anchors.
    pub kiro_conversation_anchor_ttl_seconds: u64,
}

impl Default for KiroRuntimeConfig {
    fn default() -> Self {
        Self {
            kiro_channel_max_concurrency: DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
            kiro_channel_min_start_interval_ms: DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
            kiro_status_refresh_min_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
            kiro_status_refresh_max_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            kiro_status_account_jitter_max_seconds: DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
            kiro_prefix_cache_mode: DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            kiro_prefix_cache_max_tokens: DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS,
            kiro_prefix_cache_entry_ttl_seconds: DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS,
            kiro_conversation_anchor_max_entries: DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES,
            kiro_conversation_anchor_ttl_seconds: DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS,
        }
    }
}

impl KiroRuntimeConfig {
    /// Convert this runtime config into cache-simulation settings.
    pub fn cache_simulation_config(&self) -> KiroCacheSimulationConfig {
        KiroCacheSimulationConfig {
            mode: KiroCacheSimulationMode::from_runtime_value(&self.kiro_prefix_cache_mode),
            prefix_cache_max_tokens: self.kiro_prefix_cache_max_tokens,
            prefix_cache_entry_ttl: Duration::from_secs(self.kiro_prefix_cache_entry_ttl_seconds),
            conversation_anchor_max_entries: usize::try_from(
                self.kiro_conversation_anchor_max_entries,
            )
            .unwrap_or(usize::MAX),
            conversation_anchor_ttl: Duration::from_secs(self.kiro_conversation_anchor_ttl_seconds),
        }
    }
}

/// Source of current Kiro runtime settings.
pub trait KiroRuntimeConfigSource: Send + Sync {
    /// Return a point-in-time config snapshot.
    fn snapshot(&self) -> KiroRuntimeConfig;
}

/// In-memory config source for standalone runtimes and tests.
pub struct SharedKiroRuntimeConfig {
    inner: Arc<RwLock<KiroRuntimeConfig>>,
}

impl SharedKiroRuntimeConfig {
    /// Create a shared config source.
    pub fn new(config: KiroRuntimeConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
        }
    }

    /// Replace the current config.
    pub fn replace(&self, config: KiroRuntimeConfig) {
        *self.inner.write() = config;
    }
}

impl Default for SharedKiroRuntimeConfig {
    fn default() -> Self {
        Self::new(KiroRuntimeConfig::default())
    }
}

impl KiroRuntimeConfigSource for SharedKiroRuntimeConfig {
    fn snapshot(&self) -> KiroRuntimeConfig {
        self.inner.read().clone()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{KiroRuntimeConfig, KiroRuntimeConfigSource, SharedKiroRuntimeConfig};
    use crate::cache_sim::KiroCacheSimulationMode;

    #[test]
    fn converts_runtime_config_to_cache_simulation_config() {
        let config = KiroRuntimeConfig {
            kiro_prefix_cache_mode: "formula".to_string(),
            kiro_prefix_cache_max_tokens: 123,
            kiro_prefix_cache_entry_ttl_seconds: 456,
            kiro_conversation_anchor_max_entries: 789,
            kiro_conversation_anchor_ttl_seconds: 321,
            ..KiroRuntimeConfig::default()
        };

        let cache = config.cache_simulation_config();

        assert_eq!(cache.mode, KiroCacheSimulationMode::Formula);
        assert_eq!(cache.prefix_cache_max_tokens, 123);
        assert_eq!(cache.prefix_cache_entry_ttl, Duration::from_secs(456));
        assert_eq!(cache.conversation_anchor_max_entries, 789);
        assert_eq!(cache.conversation_anchor_ttl, Duration::from_secs(321));
    }

    #[test]
    fn shared_config_source_returns_latest_snapshot() {
        let source = SharedKiroRuntimeConfig::default();
        let updated = KiroRuntimeConfig {
            kiro_status_account_jitter_max_seconds: 42,
            ..KiroRuntimeConfig::default()
        };
        source.replace(updated);

        assert_eq!(source.snapshot().kiro_status_account_jitter_max_seconds, 42);
    }
}
