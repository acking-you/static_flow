//! Admin runtime configuration and policy: the full `AdminRuntimeConfig`
//! view, its partial-update counterpart, and billing/cache policy defaults
//! plus billable-token computation.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::{
    DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT, DEFAULT_AUTH_CACHE_TTL_SECONDS,
    DEFAULT_CODEX_CLIENT_VERSION, DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
    DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
    DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS, DEFAULT_CODEX_WEIGHT_FREE,
    DEFAULT_CODEX_WEIGHT_PLUS, DEFAULT_CODEX_WEIGHT_PRO20X, DEFAULT_CODEX_WEIGHT_PRO5X,
    DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB, DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
    DEFAULT_KIRO_COMPACT_TRIGGER_TOKENS, DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
    DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES, DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS,
    DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS, DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS,
    DEFAULT_KIRO_PREFIX_CACHE_MODE, DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
    DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
    DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS, DEFAULT_MAX_REQUEST_BODY_BYTES,
    DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS, DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE,
    DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS, DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    DEFAULT_USAGE_JOURNAL_BLOCK_MAX_EVENTS, DEFAULT_USAGE_JOURNAL_BLOCK_TARGET_UNCOMPRESSED_BYTES,
    DEFAULT_USAGE_JOURNAL_CONSUMER_LEASE_MS, DEFAULT_USAGE_JOURNAL_DELETE_BAD_FILES,
    DEFAULT_USAGE_JOURNAL_ENABLED, DEFAULT_USAGE_JOURNAL_FSYNC_INTERVAL_MS,
    DEFAULT_USAGE_JOURNAL_MAX_FILES, DEFAULT_USAGE_JOURNAL_MAX_FILE_AGE_MS,
    DEFAULT_USAGE_JOURNAL_MAX_FILE_BYTES, DEFAULT_USAGE_JOURNAL_ZSTD_LEVEL,
    DEFAULT_USAGE_QUERY_BASE_URL, DEFAULT_USAGE_QUERY_BIND_ADDR,
};

/// Runtime config view shared by admin handlers and persistent stores.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminRuntimeConfig {
    /// Auth cache TTL in seconds.
    pub auth_cache_ttl_seconds: u64,
    /// Maximum request body size in bytes.
    pub max_request_body_bytes: u64,
    /// Account failure retry limit.
    pub account_failure_retry_limit: u64,
    /// Default Codex client version.
    pub codex_client_version: String,
    /// Codex minimum status refresh interval.
    pub codex_status_refresh_min_interval_seconds: u64,
    /// Codex maximum status refresh interval.
    pub codex_status_refresh_max_interval_seconds: u64,
    /// Codex per-account refresh jitter.
    pub codex_status_account_jitter_max_seconds: u64,
    /// Weight multiplier applied to Free Codex accounts during auto routing.
    pub codex_weight_free: u64,
    /// Weight multiplier applied to Plus Codex accounts during auto routing.
    pub codex_weight_plus: u64,
    /// Weight multiplier applied to Pro 5x Codex accounts during auto routing.
    pub codex_weight_pro5x: u64,
    /// Weight multiplier applied to Pro 20x Codex accounts during auto routing.
    pub codex_weight_pro20x: u64,
    /// Kiro minimum status refresh interval.
    pub kiro_status_refresh_min_interval_seconds: u64,
    /// Kiro maximum status refresh interval.
    pub kiro_status_refresh_max_interval_seconds: u64,
    /// Kiro per-account refresh jitter.
    pub kiro_status_account_jitter_max_seconds: u64,
    /// Usage event flush batch size.
    pub usage_event_flush_batch_size: u64,
    /// Usage event timed flush interval.
    pub usage_event_flush_interval_seconds: u64,
    /// Usage event buffered payload cap.
    pub usage_event_flush_max_buffer_bytes: u64,
    /// DuckDB usage writer memory limit in MiB.
    pub duckdb_usage_memory_limit_mib: u64,
    /// DuckDB usage writer WAL checkpoint threshold in MiB.
    pub duckdb_usage_checkpoint_threshold_mib: u64,
    /// Number of recent days retained in DuckDB usage analytics.
    pub usage_analytics_retention_days: u64,
    /// Whether API workers write usage events to local journal files.
    pub usage_journal_enabled: bool,
    /// Maximum compressed journal file bytes before sealing.
    pub usage_journal_max_file_bytes: u64,
    /// Maximum journal file age before sealing.
    pub usage_journal_max_file_age_ms: u64,
    /// Maximum journal files retained on disk.
    pub usage_journal_max_files: u64,
    /// Target uncompressed bytes per journal block.
    pub usage_journal_block_target_uncompressed_bytes: u64,
    /// Maximum events per journal block.
    pub usage_journal_block_max_events: u64,
    /// Journal fsync interval in milliseconds.
    pub usage_journal_fsync_interval_ms: u64,
    /// Journal zstd compression level.
    pub usage_journal_zstd_level: i64,
    /// Worker lease age before claimed journals are recovered.
    pub usage_journal_consumer_lease_ms: u64,
    /// Whether corrupt journals are deleted rather than quarantined.
    pub usage_journal_delete_bad_files: bool,
    /// Worker query HTTP bind address.
    pub usage_query_bind_addr: String,
    /// Worker query base URL used by API-side compatibility routes.
    pub usage_query_base_url: String,
    /// Kiro cache k-model coefficients JSON.
    pub kiro_cache_kmodels_json: String,
    /// Kiro billable model multiplier JSON.
    pub kiro_billable_model_multipliers_json: String,
    /// Kiro cache policy JSON.
    pub kiro_cache_policy_json: String,
    /// Minimum request-side input tokens before trusting Kiro contextUsage.
    pub kiro_context_usage_min_request_tokens: u64,
    /// Proactive auto-compaction trigger in counted input tokens; `0` disables.
    pub kiro_compact_trigger_tokens: u64,
    /// Kiro prefix cache mode.
    pub kiro_prefix_cache_mode: String,
    /// Kiro prefix cache token budget.
    pub kiro_prefix_cache_max_tokens: u64,
    /// Kiro prefix cache entry TTL.
    pub kiro_prefix_cache_entry_ttl_seconds: u64,
    /// Kiro conversation anchor max entries.
    pub kiro_conversation_anchor_max_entries: u64,
    /// Kiro conversation anchor TTL.
    pub kiro_conversation_anchor_ttl_seconds: u64,
}

impl Default for AdminRuntimeConfig {
    fn default() -> Self {
        Self {
            auth_cache_ttl_seconds: DEFAULT_AUTH_CACHE_TTL_SECONDS,
            max_request_body_bytes: DEFAULT_MAX_REQUEST_BODY_BYTES,
            account_failure_retry_limit: DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT,
            codex_client_version: DEFAULT_CODEX_CLIENT_VERSION.to_string(),
            codex_status_refresh_min_interval_seconds:
                DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
            codex_status_refresh_max_interval_seconds:
                DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            codex_status_account_jitter_max_seconds:
                DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
            codex_weight_free: DEFAULT_CODEX_WEIGHT_FREE,
            codex_weight_plus: DEFAULT_CODEX_WEIGHT_PLUS,
            codex_weight_pro5x: DEFAULT_CODEX_WEIGHT_PRO5X,
            codex_weight_pro20x: DEFAULT_CODEX_WEIGHT_PRO20X,
            kiro_status_refresh_min_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
            kiro_status_refresh_max_interval_seconds:
                DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            kiro_status_account_jitter_max_seconds: DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
            usage_event_flush_batch_size: DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE,
            usage_event_flush_interval_seconds: DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
            usage_event_flush_max_buffer_bytes: DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
            duckdb_usage_memory_limit_mib: DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
            duckdb_usage_checkpoint_threshold_mib: DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
            usage_analytics_retention_days: DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS,
            usage_journal_enabled: DEFAULT_USAGE_JOURNAL_ENABLED,
            usage_journal_max_file_bytes: DEFAULT_USAGE_JOURNAL_MAX_FILE_BYTES,
            usage_journal_max_file_age_ms: DEFAULT_USAGE_JOURNAL_MAX_FILE_AGE_MS,
            usage_journal_max_files: DEFAULT_USAGE_JOURNAL_MAX_FILES,
            usage_journal_block_target_uncompressed_bytes:
                DEFAULT_USAGE_JOURNAL_BLOCK_TARGET_UNCOMPRESSED_BYTES,
            usage_journal_block_max_events: DEFAULT_USAGE_JOURNAL_BLOCK_MAX_EVENTS,
            usage_journal_fsync_interval_ms: DEFAULT_USAGE_JOURNAL_FSYNC_INTERVAL_MS,
            usage_journal_zstd_level: DEFAULT_USAGE_JOURNAL_ZSTD_LEVEL,
            usage_journal_consumer_lease_ms: DEFAULT_USAGE_JOURNAL_CONSUMER_LEASE_MS,
            usage_journal_delete_bad_files: DEFAULT_USAGE_JOURNAL_DELETE_BAD_FILES,
            usage_query_bind_addr: DEFAULT_USAGE_QUERY_BIND_ADDR.to_string(),
            usage_query_base_url: DEFAULT_USAGE_QUERY_BASE_URL.to_string(),
            kiro_cache_kmodels_json: default_kiro_cache_kmodels_json(),
            kiro_billable_model_multipliers_json: default_kiro_billable_model_multipliers_json(),
            kiro_cache_policy_json: default_kiro_cache_policy_json(),
            kiro_context_usage_min_request_tokens: DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
            kiro_compact_trigger_tokens: DEFAULT_KIRO_COMPACT_TRIGGER_TOKENS,
            kiro_prefix_cache_mode: DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            kiro_prefix_cache_max_tokens: DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS,
            kiro_prefix_cache_entry_ttl_seconds: DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS,
            kiro_conversation_anchor_max_entries: DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES,
            kiro_conversation_anchor_ttl_seconds: DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS,
        }
    }
}

/// Admin request body for patching runtime config.
#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct UpdateAdminRuntimeConfig {
    /// Auth cache TTL in seconds.
    #[serde(default)]
    pub auth_cache_ttl_seconds: Option<u64>,
    /// Maximum request body size in bytes.
    #[serde(default)]
    pub max_request_body_bytes: Option<u64>,
    /// Account failure retry limit.
    #[serde(default)]
    pub account_failure_retry_limit: Option<u64>,
    /// Default Codex client version.
    #[serde(default)]
    pub codex_client_version: Option<String>,
    /// Codex minimum status refresh interval.
    #[serde(default)]
    pub codex_status_refresh_min_interval_seconds: Option<u64>,
    /// Codex maximum status refresh interval.
    #[serde(default)]
    pub codex_status_refresh_max_interval_seconds: Option<u64>,
    /// Codex per-account refresh jitter.
    #[serde(default)]
    pub codex_status_account_jitter_max_seconds: Option<u64>,
    /// Free Codex routing weight.
    #[serde(default)]
    pub codex_weight_free: Option<u64>,
    /// Plus Codex routing weight.
    #[serde(default)]
    pub codex_weight_plus: Option<u64>,
    /// Pro 5x Codex routing weight.
    #[serde(default)]
    pub codex_weight_pro5x: Option<u64>,
    /// Pro 20x Codex routing weight.
    #[serde(default)]
    pub codex_weight_pro20x: Option<u64>,
    /// Kiro minimum status refresh interval.
    #[serde(default)]
    pub kiro_status_refresh_min_interval_seconds: Option<u64>,
    /// Kiro maximum status refresh interval.
    #[serde(default)]
    pub kiro_status_refresh_max_interval_seconds: Option<u64>,
    /// Kiro per-account refresh jitter.
    #[serde(default)]
    pub kiro_status_account_jitter_max_seconds: Option<u64>,
    /// Usage event flush batch size.
    #[serde(default)]
    pub usage_event_flush_batch_size: Option<u64>,
    /// Usage event timed flush interval.
    #[serde(default)]
    pub usage_event_flush_interval_seconds: Option<u64>,
    /// Usage event buffered payload cap.
    #[serde(default)]
    pub usage_event_flush_max_buffer_bytes: Option<u64>,
    /// DuckDB usage writer memory limit in MiB.
    #[serde(default)]
    pub duckdb_usage_memory_limit_mib: Option<u64>,
    /// DuckDB usage writer WAL checkpoint threshold in MiB.
    #[serde(default)]
    pub duckdb_usage_checkpoint_threshold_mib: Option<u64>,
    /// Number of recent days retained in DuckDB usage analytics.
    #[serde(default)]
    pub usage_analytics_retention_days: Option<u64>,
    /// Usage-journal write toggle.
    #[serde(default)]
    pub usage_journal_enabled: Option<bool>,
    /// Maximum compressed journal file bytes before sealing.
    #[serde(default)]
    pub usage_journal_max_file_bytes: Option<u64>,
    /// Maximum journal file age before sealing.
    #[serde(default)]
    pub usage_journal_max_file_age_ms: Option<u64>,
    /// Maximum journal files retained on disk.
    #[serde(default)]
    pub usage_journal_max_files: Option<u64>,
    /// Target uncompressed bytes per journal block.
    #[serde(default)]
    pub usage_journal_block_target_uncompressed_bytes: Option<u64>,
    /// Maximum events per journal block.
    #[serde(default)]
    pub usage_journal_block_max_events: Option<u64>,
    /// Journal fsync interval in milliseconds.
    #[serde(default)]
    pub usage_journal_fsync_interval_ms: Option<u64>,
    /// Journal zstd compression level.
    #[serde(default)]
    pub usage_journal_zstd_level: Option<i64>,
    /// Worker lease age before claimed journals are recovered.
    #[serde(default)]
    pub usage_journal_consumer_lease_ms: Option<u64>,
    /// Whether corrupt journals are deleted rather than quarantined.
    #[serde(default)]
    pub usage_journal_delete_bad_files: Option<bool>,
    /// Worker query HTTP bind address.
    #[serde(default)]
    pub usage_query_bind_addr: Option<String>,
    /// Worker query base URL used by API-side compatibility routes.
    #[serde(default)]
    pub usage_query_base_url: Option<String>,
    /// Kiro cache k-model coefficients JSON.
    #[serde(default)]
    pub kiro_cache_kmodels_json: Option<String>,
    /// Kiro billable model multiplier JSON.
    #[serde(default)]
    pub kiro_billable_model_multipliers_json: Option<String>,
    /// Kiro cache policy JSON.
    #[serde(default)]
    pub kiro_cache_policy_json: Option<String>,
    /// Minimum request-side input tokens before trusting Kiro contextUsage.
    #[serde(default)]
    pub kiro_context_usage_min_request_tokens: Option<u64>,
    /// Proactive auto-compaction trigger in counted input tokens; `0` disables.
    #[serde(default)]
    pub kiro_compact_trigger_tokens: Option<u64>,
    /// Kiro prefix cache mode.
    #[serde(default)]
    pub kiro_prefix_cache_mode: Option<String>,
    /// Kiro prefix cache token budget.
    #[serde(default)]
    pub kiro_prefix_cache_max_tokens: Option<u64>,
    /// Kiro prefix cache entry TTL.
    #[serde(default)]
    pub kiro_prefix_cache_entry_ttl_seconds: Option<u64>,
    /// Kiro conversation anchor max entries.
    #[serde(default)]
    pub kiro_conversation_anchor_max_entries: Option<u64>,
    /// Kiro conversation anchor TTL.
    #[serde(default)]
    pub kiro_conversation_anchor_ttl_seconds: Option<u64>,
}

/// Return the default Kiro cache k-model JSON.
pub fn default_kiro_cache_kmodels_json() -> String {
    let map = BTreeMap::from([
        ("claude-haiku-4-5-20251001".to_string(), 2.3681034438052206e-06),
        ("claude-opus-4-6".to_string(), 8.061927916785985e-06),
        ("claude-sonnet-4-6".to_string(), 5.055065250835128e-06),
    ]);
    serde_json::to_string(&map).expect("default kmodels should serialize")
}

/// Return the default Kiro billable multiplier JSON.
pub fn default_kiro_billable_model_multipliers_json() -> String {
    let map = BTreeMap::from([
        ("haiku".to_string(), 1.0),
        ("opus".to_string(), 1.0),
        ("sonnet".to_string(), 1.0),
    ]);
    serde_json::to_string(&map).expect("default billable multipliers should serialize")
}

/// Shared billable-token weighting used for gateway accounting.
///
/// Cached input is billed at one tenth of uncached input, while output tokens
/// are billed at five times input cost.
pub fn compute_billable_tokens(
    input_uncached_tokens: u64,
    input_cached_tokens: u64,
    output_tokens: u64,
) -> u64 {
    input_uncached_tokens
        .saturating_add(input_cached_tokens / 10)
        .saturating_add(output_tokens.saturating_mul(5))
}

fn kiro_billable_model_family(model_name: &str) -> Option<&'static str> {
    let normalized = model_name.trim().to_ascii_lowercase();
    if normalized.contains("opus") {
        Some("opus")
    } else if normalized.contains("sonnet") {
        Some("sonnet")
    } else if normalized.contains("haiku") {
        Some("haiku")
    } else {
        None
    }
}

/// Apply the configured Kiro model-family multiplier to billable tokens.
pub fn compute_kiro_billable_tokens(
    model_name: Option<&str>,
    input_uncached_tokens: u64,
    input_cached_tokens: u64,
    output_tokens: u64,
    multipliers: &BTreeMap<String, f64>,
) -> u64 {
    let base = compute_billable_tokens(input_uncached_tokens, input_cached_tokens, output_tokens);
    if base == 0 {
        return 0;
    }

    let multiplier = model_name
        .and_then(kiro_billable_model_family)
        .and_then(|family| multipliers.get(family).copied())
        .unwrap_or(1.0);
    ((base as f64) * multiplier)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

/// Return the default Kiro cache policy JSON.
pub fn default_kiro_cache_policy_json() -> String {
    serde_json::json!({
        "small_input_high_credit_boost": {
            "target_input_tokens": 100000,
            "credit_start": 1.0,
            "credit_end": 1.8
        },
        "prefix_tree_credit_ratio_bands": [
            {
                "credit_start": 0.3,
                "credit_end": 1.0,
                "cache_ratio_start": 0.7,
                "cache_ratio_end": 0.2
            },
            {
                "credit_start": 1.0,
                "credit_end": 2.5,
                "cache_ratio_start": 0.2,
                "cache_ratio_end": 0.0
            }
        ],
        "high_credit_diagnostic_threshold": 2.0,
        "anthropic_cache_creation_input_ratio": 0.0
    })
    .to_string()
}


#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    #[test]
    fn compute_kiro_billable_tokens_applies_family_multiplier() {
        let multipliers = BTreeMap::from([
            ("opus".to_string(), 2.0),
            ("sonnet".to_string(), 1.0),
            ("haiku".to_string(), 1.0),
        ]);

        let base = super::compute_billable_tokens(100, 20, 5);
        let adjusted =
            super::compute_kiro_billable_tokens(Some("claude-opus-4-6"), 100, 20, 5, &multipliers);

        assert_eq!(adjusted, base * 2);
    }

    #[test]
    fn compute_kiro_billable_tokens_defaults_for_unknown_models() {
        let multipliers = BTreeMap::from([
            ("opus".to_string(), 2.0),
            ("sonnet".to_string(), 3.0),
            ("haiku".to_string(), 0.5),
        ]);

        let base = super::compute_billable_tokens(80, 10, 4);
        let adjusted =
            super::compute_kiro_billable_tokens(Some("claude-unknown-1"), 80, 10, 4, &multipliers);

        assert_eq!(adjusted, base);
    }

    #[test]
    fn admin_runtime_config_uses_tightened_kiro_cache_defaults() {
        let config = super::AdminRuntimeConfig::default();

        assert_eq!(config.kiro_prefix_cache_max_tokens, 1_000_000);
        assert_eq!(config.kiro_prefix_cache_entry_ttl_seconds, 2 * 60 * 60);
        assert_eq!(config.kiro_conversation_anchor_max_entries, 4_096);
        assert_eq!(config.kiro_conversation_anchor_ttl_seconds, 6 * 60 * 60);
        assert_eq!(config.kiro_context_usage_min_request_tokens, 15_000);
        assert_eq!(config.kiro_compact_trigger_tokens, 780_000);
    }
}
