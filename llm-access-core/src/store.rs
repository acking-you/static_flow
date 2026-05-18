//! Storage traits consumed by provider runtimes.

use std::collections::BTreeMap;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{provider::RouteStrategy, usage::UsageEvent};

/// Default public auth-cache TTL used when no runtime config row exists yet.
pub const DEFAULT_AUTH_CACHE_TTL_SECONDS: u64 = 60;
/// Default Codex status refresh interval used before runtime config is
/// imported.
pub const DEFAULT_CODEX_STATUS_REFRESH_SECONDS: u64 = 300;
/// Default maximum request body size enforced by provider request handlers.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Default consecutive upstream failure threshold before an account is skipped.
pub const DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 10;
/// Default Codex client version sent to upstream requests.
pub const DEFAULT_CODEX_CLIENT_VERSION: &str = "0.124.0";
/// Default lower bound for randomized Codex status refresh.
pub const DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
/// Default upper bound for randomized Codex status refresh.
pub const DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
/// Default maximum Codex account refresh jitter.
pub const DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
/// Default weighted auto-routing multiplier for Free Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_FREE: u64 = 1;
/// Default weighted auto-routing multiplier for Plus Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PLUS: u64 = 10;
/// Default weighted auto-routing multiplier for Pro 5x Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PRO5X: u64 = 50;
/// Default weighted auto-routing multiplier for Pro 20x Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PRO20X: u64 = 200;
/// Default lower bound for randomized Kiro status refresh.
pub const DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
/// Default upper bound for randomized Kiro status refresh.
pub const DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
/// Default maximum Kiro account refresh jitter.
pub const DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
/// Default usage-event flush batch size.
pub const DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 256;
/// Default usage-event timed flush interval.
pub const DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 15;
/// Default usage-event buffered payload cap.
pub const DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
/// Default DuckDB usage writer memory limit in MiB.
pub const DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB: u64 = 1024;
/// Default DuckDB usage writer WAL checkpoint threshold in MiB.
pub const DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB: u64 = 16;
/// Default retained usage analytics horizon in days.
pub const DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS: u64 = 7;
/// Default usage-journal write toggle.
pub const DEFAULT_USAGE_JOURNAL_ENABLED: bool = true;
/// Default compressed journal file rollover size.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Default journal file age rollover threshold.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILE_AGE_MS: u64 = 300_000;
/// Default maximum journal files kept on disk.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILES: u64 = 128;
/// Default journal block target before compression.
pub const DEFAULT_USAGE_JOURNAL_BLOCK_TARGET_UNCOMPRESSED_BYTES: u64 = 1024 * 1024;
/// Default maximum usage events per journal block.
pub const DEFAULT_USAGE_JOURNAL_BLOCK_MAX_EVENTS: u64 = 1024;
/// Default journal fsync interval.
pub const DEFAULT_USAGE_JOURNAL_FSYNC_INTERVAL_MS: u64 = 250;
/// Default journal zstd compression level.
pub const DEFAULT_USAGE_JOURNAL_ZSTD_LEVEL: i64 = 3;
/// Default worker lease age before a claimed journal is recovered.
pub const DEFAULT_USAGE_JOURNAL_CONSUMER_LEASE_MS: u64 = 300_000;
/// Default corrupt-file policy.
pub const DEFAULT_USAGE_JOURNAL_DELETE_BAD_FILES: bool = false;
/// Default worker query bind address.
pub const DEFAULT_USAGE_QUERY_BIND_ADDR: &str = "127.0.0.1:19081";
/// Default worker query base URL used by the API process.
pub const DEFAULT_USAGE_QUERY_BASE_URL: &str = "http://127.0.0.1:19081";
/// Default usage maintenance toggle.
pub const DEFAULT_USAGE_EVENT_MAINTENANCE_ENABLED: bool = true;
/// Default usage maintenance interval.
pub const DEFAULT_USAGE_EVENT_MAINTENANCE_INTERVAL_SECONDS: u64 = 60 * 60;
/// Default detailed usage retention.
pub const DEFAULT_USAGE_EVENT_DETAIL_RETENTION_DAYS: i64 = 7;
/// Default Kiro prefix cache mode.
pub const DEFAULT_KIRO_PREFIX_CACHE_MODE: &str = "prefix_tree";
/// Alternate Kiro prefix cache mode retained for admin compatibility.
pub const KIRO_PREFIX_CACHE_MODE_FORMULA: &str = "formula";
/// Default Kiro prefix-cache budget.
pub const DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS: u64 = 1_000_000;
/// Default Kiro prefix-cache entry TTL.
pub const DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS: u64 = 2 * 60 * 60;
/// Default Kiro conversation anchor capacity.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES: u64 = 4_096;
/// Default Kiro conversation anchor TTL.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS: u64 = 6 * 60 * 60;
/// Default Kiro account channel concurrency retained in storage.
pub const DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 1;
/// Default Kiro account request pacing interval retained in storage.
pub const DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 0;
/// Pending status used by public token/account contribution requests.
pub const PUBLIC_TOKEN_REQUEST_STATUS_PENDING: &str = "pending";
/// Validated status used by account contribution requests after auth refresh
/// checks.
pub const PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED: &str = "validated";
/// Submitted status used by public sponsor requests before payment email.
pub const PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED: &str = "submitted";
/// Sponsor status used after payment instructions were sent.
pub const PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT: &str = "payment_email_sent";
/// Active managed key status.
pub const KEY_STATUS_ACTIVE: &str = "active";
/// Disabled managed key status.
pub const KEY_STATUS_DISABLED: &str = "disabled";
/// Codex provider string used by current admin key records.
pub const PROVIDER_CODEX: &str = "codex";
/// Kiro provider string used by current admin key records.
pub const PROVIDER_KIRO: &str = "kiro";
/// OpenAI-compatible protocol family.
pub const PROTOCOL_OPENAI: &str = "openai";
/// Anthropic-compatible protocol family.
pub const PROTOCOL_ANTHROPIC: &str = "anthropic";

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

/// Admin-facing projection of one managed API key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKey {
    /// Key id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plaintext secret shown in admin UI.
    pub secret: String,
    /// SHA-256 secret hash.
    pub key_hash: String,
    /// Key status.
    pub status: String,
    /// Provider type.
    pub provider_type: String,
    /// Whether the key is visible on the public access page.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated credit usage.
    pub usage_credit_total: f64,
    /// Number of events missing credit usage.
    pub usage_credit_missing_events: u64,
    /// Remaining billable tokens.
    pub remaining_billable: i64,
    /// Last usage timestamp.
    pub last_used_at: Option<i64>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
    /// Account route strategy.
    pub route_strategy: Option<String>,
    /// Account group id.
    pub account_group_id: Option<String>,
    /// Fixed account name.
    pub fixed_account_name: Option<String>,
    /// Auto account names.
    pub auto_account_names: Option<Vec<String>>,
    /// Model name mapping.
    pub model_name_map: Option<BTreeMap<String, String>>,
    /// Per-key request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-key request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Whether Kiro request validation is enabled.
    pub kiro_request_validation_enabled: bool,
    /// Whether Kiro cache estimation is enabled.
    pub kiro_cache_estimation_enabled: bool,
    /// Whether Kiro zero-cache diagnostics are enabled.
    pub kiro_zero_cache_debug_enabled: bool,
    /// Whether every Kiro request should retain full request payload
    /// diagnostics.
    pub kiro_full_request_logging_enabled: bool,
    /// Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<String>,
    /// Kiro billable multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<String>,
    /// Effective Kiro cache policy JSON.
    pub effective_kiro_cache_policy_json: String,
    /// Whether the effective Kiro cache policy is global.
    pub uses_global_kiro_cache_policy: bool,
    /// Effective Kiro billable multiplier JSON.
    pub effective_kiro_billable_model_multipliers_json: String,
    /// Whether the effective billable multipliers are global.
    pub uses_global_kiro_billable_model_multipliers: bool,
    /// Admin-facing candidate-credit summary for Kiro routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_candidate_credit_summary: Option<AdminKiroKeyCandidateCreditSummary>,
}

/// Admin-facing candidate-credit summary for one Kiro key.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroKeyCandidateCreditSummary {
    /// Number of candidate accounts matched by the key route.
    pub candidate_count: usize,
    /// Number of candidate accounts with a loaded balance snapshot.
    pub loaded_balance_count: usize,
    /// Number of candidate accounts still missing a balance snapshot.
    pub missing_balance_count: usize,
    /// Sum of upstream credit limits across loaded candidate accounts.
    pub total_limit: f64,
    /// Sum of remaining upstream credits across loaded candidate accounts.
    pub total_remaining: f64,
}

/// Offset pagination request shared by admin list endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminPageRequest {
    /// Maximum number of rows to return.
    pub limit: usize,
    /// Number of rows to skip.
    pub offset: usize,
}

impl AdminPageRequest {
    /// Return true when at least one row remains after this page.
    pub fn has_more(self, returned: usize, total: usize) -> bool {
        self.offset.saturating_add(returned) < total
    }
}

/// Page of admin-managed API keys.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKeysPage {
    /// Page rows.
    pub keys: Vec<AdminKey>,
    /// Full aggregate over all rows matching this page filter.
    pub summary: AdminKeysSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Full aggregate for admin-managed API keys.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
pub struct AdminKeysSummary {
    /// Total rows matching the provider filter.
    pub total: usize,
    /// Public-visible key count.
    pub public_visible_count: usize,
    /// Active key count.
    pub active_count: usize,
    /// Disabled key count.
    pub disabled_count: usize,
    /// Sum of configured billable quotas.
    pub quota_billable_limit_sum: u64,
    /// Sum of remaining billable quotas.
    pub remaining_billable_sum: i64,
    /// Sum of uncached input tokens.
    pub usage_input_uncached_tokens_sum: u64,
    /// Sum of cached input tokens.
    pub usage_input_cached_tokens_sum: u64,
    /// Sum of output tokens.
    pub usage_output_tokens_sum: u64,
    /// Sum of billable tokens.
    pub usage_billable_tokens_sum: u64,
    /// Sum of recorded credit usage.
    pub usage_credit_total: f64,
    /// Sum of events missing credit usage.
    pub usage_credit_missing_events: u64,
}

/// Admin key list query shared by paginated inventory screens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminKeyPageQuery {
    /// Optional case-insensitive search query.
    pub search: Option<String>,
    /// Whether disabled rows should be excluded.
    pub active_only: bool,
    /// Sort mode applied before pagination.
    pub sort: AdminKeySortMode,
}

/// Supported admin key list sort modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AdminKeySortMode {
    /// Default created-at descending order.
    #[default]
    Newest,
    /// Remaining quota ascending.
    QuotaAsc,
    /// Remaining quota descending.
    QuotaDesc,
    /// Recorded credit usage ascending.
    UsageAsc,
    /// Recorded credit usage descending.
    UsageDesc,
}

fn summarize_admin_keys(keys: &[AdminKey]) -> AdminKeysSummary {
    let mut summary = AdminKeysSummary::default();
    for key in keys {
        summary.total += 1;
        if key.public_visible {
            summary.public_visible_count += 1;
        }
        match key.status.as_str() {
            KEY_STATUS_ACTIVE => summary.active_count += 1,
            KEY_STATUS_DISABLED => summary.disabled_count += 1,
            _ => {},
        }
        summary.quota_billable_limit_sum = summary
            .quota_billable_limit_sum
            .saturating_add(key.quota_billable_limit);
        summary.remaining_billable_sum = summary
            .remaining_billable_sum
            .saturating_add(key.remaining_billable);
        summary.usage_input_uncached_tokens_sum = summary
            .usage_input_uncached_tokens_sum
            .saturating_add(key.usage_input_uncached_tokens);
        summary.usage_input_cached_tokens_sum = summary
            .usage_input_cached_tokens_sum
            .saturating_add(key.usage_input_cached_tokens);
        summary.usage_output_tokens_sum = summary
            .usage_output_tokens_sum
            .saturating_add(key.usage_output_tokens);
        summary.usage_billable_tokens_sum = summary.usage_billable_tokens_sum.saturating_add(
            key.quota_billable_limit
                .saturating_sub(key.remaining_billable.max(0) as u64),
        );
        summary.usage_credit_total += key.usage_credit_total;
        summary.usage_credit_missing_events = summary
            .usage_credit_missing_events
            .saturating_add(key.usage_credit_missing_events);
    }
    summary
}

fn admin_key_matches_query(key: &AdminKey, query: &AdminKeyPageQuery) -> bool {
    if query.active_only && key.status == KEY_STATUS_DISABLED {
        return false;
    }
    let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let search = search.to_ascii_lowercase();
    key.id.to_ascii_lowercase().contains(&search)
        || key.name.to_ascii_lowercase().contains(&search)
        || key.provider_type.to_ascii_lowercase().contains(&search)
        || key.status.to_ascii_lowercase().contains(&search)
}

fn apply_admin_key_query(keys: &mut Vec<AdminKey>, query: &AdminKeyPageQuery) {
    keys.retain(|key| admin_key_matches_query(key, query));
    match query.sort {
        AdminKeySortMode::Newest => keys.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        }),
        AdminKeySortMode::QuotaAsc => keys.sort_by_key(|key| key.remaining_billable),
        AdminKeySortMode::QuotaDesc => {
            keys.sort_by_key(|key| std::cmp::Reverse(key.remaining_billable));
        },
        AdminKeySortMode::UsageAsc => keys.sort_by(|a, b| {
            a.usage_credit_total
                .partial_cmp(&b.usage_credit_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        }),
        AdminKeySortMode::UsageDesc => keys.sort_by(|a, b| {
            b.usage_credit_total
                .partial_cmp(&a.usage_credit_total)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.created_at.cmp(&a.created_at))
        }),
    }
}

/// New admin key row after request validation and secret generation.
#[derive(Debug, Clone, PartialEq)]
pub struct NewAdminKey {
    /// Key id.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Plaintext secret.
    pub secret: String,
    /// SHA-256 secret hash.
    pub key_hash: String,
    /// Provider type.
    pub provider_type: String,
    /// Protocol family.
    pub protocol_family: String,
    /// Whether the key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Per-key request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-key request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Admin key patch after request normalization.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminKeyPatch {
    /// New name.
    pub name: Option<String>,
    /// New status.
    pub status: Option<String>,
    /// New public visibility.
    pub public_visible: Option<bool>,
    /// New quota limit.
    pub quota_billable_limit: Option<u64>,
    /// New route strategy.
    pub route_strategy: Option<Option<String>>,
    /// New account group id.
    pub account_group_id: Option<Option<String>>,
    /// New fixed account name.
    pub fixed_account_name: Option<Option<String>>,
    /// New auto account list.
    pub auto_account_names: Option<Option<Vec<String>>>,
    /// New model name map.
    pub model_name_map: Option<Option<BTreeMap<String, String>>>,
    /// New per-key request concurrency cap.
    pub request_max_concurrency: Option<Option<u64>>,
    /// New per-key request pacing interval.
    pub request_min_start_interval_ms: Option<Option<u64>>,
    /// New Kiro request-validation toggle.
    pub kiro_request_validation_enabled: Option<bool>,
    /// New Kiro cache-estimation toggle.
    pub kiro_cache_estimation_enabled: Option<bool>,
    /// New Kiro zero-cache diagnostic toggle.
    pub kiro_zero_cache_debug_enabled: Option<bool>,
    /// New Kiro full request logging toggle.
    pub kiro_full_request_logging_enabled: Option<bool>,
    /// New Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<Option<String>>,
    /// New Kiro billable model multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<Option<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Admin-facing projection of one reusable account group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroup {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Account names included in the group.
    pub account_names: Vec<String>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
}

/// Page of admin account groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroupsPage {
    /// Page rows.
    pub groups: Vec<AdminAccountGroup>,
    /// Total rows matching the provider before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Lightweight reusable account-group projection for routing selectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroupOption {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Total accounts in this group.
    pub account_count: usize,
    /// The lone account name when this group contains exactly one account.
    pub single_account_name: Option<String>,
}

/// New reusable account group row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminAccountGroup {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Account names included in the group.
    pub account_names: Vec<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one reusable account group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminAccountGroupPatch {
    /// New group name.
    pub name: Option<String>,
    /// Replacement account list.
    pub account_names: Option<Vec<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Admin-facing projection of one reusable upstream proxy config.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProxyConfig {
    /// Proxy config id.
    pub id: String,
    /// Human-readable proxy name.
    pub name: String,
    /// Proxy URL.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
    /// Config status.
    pub status: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
}

/// New reusable proxy config row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminProxyConfig {
    /// Proxy config id.
    pub id: String,
    /// Human-readable proxy name.
    pub name: String,
    /// Proxy URL.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one reusable proxy config.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminProxyConfigPatch {
    /// New proxy name.
    pub name: Option<String>,
    /// New proxy URL.
    pub proxy_url: Option<String>,
    /// New optional proxy username.
    pub proxy_username: Option<Option<String>>,
    /// New optional proxy password.
    pub proxy_password: Option<Option<String>>,
    /// New status.
    pub status: Option<String>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Effective provider-level proxy binding shown in admin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminProxyBinding {
    /// Provider type.
    pub provider_type: String,
    /// Source used to resolve the effective proxy.
    pub effective_source: String,
    /// Explicitly bound proxy config id.
    pub bound_proxy_config_id: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy username.
    pub effective_proxy_username: Option<String>,
    /// Effective proxy password.
    pub effective_proxy_password: Option<String>,
    /// Binding update timestamp.
    pub binding_updated_at: Option<i64>,
    /// Error message for invalid bindings.
    pub error_message: Option<String>,
}

/// Admin-facing Codex account summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminCodexAccount {
    /// Account display name.
    pub name: String,
    /// Runtime status.
    pub status: String,
    /// Upstream account id.
    pub account_id: Option<String>,
    /// Upstream plan type, when known.
    pub plan_type: Option<String>,
    /// Manual routing tier override used for weighted auto routing.
    pub route_weight_tier: String,
    /// Primary rate-limit remaining percentage, when known.
    pub primary_remaining_percent: Option<f64>,
    /// Secondary rate-limit remaining percentage, when known.
    pub secondary_remaining_percent: Option<f64>,
    /// Whether GPT-5.3 Codex is mapped to Spark for this account.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auto_refresh_enabled: bool,
    /// Per-account request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-account request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Proxy selection mode.
    pub proxy_mode: String,
    /// Fixed proxy config id when proxy mode is fixed.
    pub proxy_config_id: Option<String>,
    /// Effective proxy source.
    pub effective_proxy_source: String,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Last auth refresh timestamp.
    pub last_refresh: Option<i64>,
    /// Current access token expiry timestamp in Unix milliseconds.
    pub access_token_expires_at: Option<i64>,
    /// Last auth refresh error, if any.
    pub auth_refresh_error_message: Option<String>,
    /// Last usage refresh attempt timestamp.
    pub last_usage_checked_at: Option<i64>,
    /// Last successful usage refresh timestamp.
    pub last_usage_success_at: Option<i64>,
    /// Last usage refresh error.
    pub usage_error_message: Option<String>,
}

/// Page of admin Codex accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminCodexAccountsPage {
    /// Page rows.
    pub accounts: Vec<AdminCodexAccount>,
    /// Full aggregate over all Codex accounts.
    pub summary: AdminAccountsSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Full aggregate for admin account inventories.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountsSummary {
    /// Total account rows.
    pub total: usize,
    /// Active account count.
    pub active_count: usize,
    /// Disabled account count.
    pub disabled_count: usize,
    /// Unavailable account count.
    pub unavailable_count: usize,
}

/// Minimal Codex account projection used by background status refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStatusRefreshTarget {
    /// Account display name.
    pub name: String,
    /// Runtime status.
    pub status: String,
}

/// New imported Codex account row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexAccount {
    /// Account display name.
    pub name: String,
    /// Upstream account id.
    pub account_id: Option<String>,
    /// Persisted auth JSON.
    pub auth_json: String,
    /// Whether GPT-5.3 Codex is mapped to Spark for this account.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auto_refresh_enabled: bool,
    /// Manual routing tier override used for weighted auto routing.
    pub route_weight_tier: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one Codex account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminCodexAccountPatch {
    /// New runtime status.
    pub status: Option<String>,
    /// New GPT-5.3 Codex Spark mapping toggle.
    pub map_gpt53_codex_to_spark: Option<bool>,
    /// New automatic auth refresh toggle.
    pub auto_refresh_enabled: Option<bool>,
    /// New routing weight tier override.
    pub route_weight_tier: Option<String>,
    /// New proxy selection mode.
    pub proxy_mode: Option<String>,
    /// New proxy config id.
    pub proxy_config_id: Option<Option<String>>,
    /// New per-account request concurrency cap.
    pub request_max_concurrency: Option<Option<u64>>,
    /// New per-account request pacing interval.
    pub request_min_start_interval_ms: Option<Option<u64>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Admin-facing summary for one Codex batch import job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobSummary {
    /// Batch job id.
    pub job_id: String,
    /// Provider type.
    pub provider_type: String,
    /// Import source type.
    pub source_type: String,
    /// Whether refresh validation runs before import.
    pub validate_before_import: bool,
    /// Current batch status.
    pub status: String,
    /// Total queued item count.
    pub total_count: usize,
    /// Number of terminal items.
    pub completed_count: usize,
    /// Number of imported items.
    pub succeeded_count: usize,
    /// Number of skipped items.
    pub skipped_count: usize,
    /// Number of failed/conflict items.
    pub failed_count: usize,
    /// Batch-level failure reason when the worker aborts early.
    pub batch_error_message: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last update timestamp.
    pub updated_at_ms: i64,
    /// Finish timestamp once terminal.
    pub finished_at_ms: Option<i64>,
}

/// Admin-facing result row for one Codex batch import item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobItem {
    /// Zero-based item index within the batch.
    pub item_index: usize,
    /// Requested account name.
    pub requested_name: String,
    /// Requested upstream account id when present.
    pub requested_account_id: Option<String>,
    /// Current item status.
    pub status: String,
    /// Terminal error message when the item fails.
    pub error_message: Option<String>,
    /// Imported account name when successful.
    pub imported_account_name: Option<String>,
    /// Final upstream account id after validation/import.
    pub final_account_id: Option<String>,
    /// Validation timestamp when refresh validation succeeds.
    pub validated_at_ms: Option<i64>,
    /// Import timestamp when the account row is created.
    pub imported_at_ms: Option<i64>,
}

/// Full admin-facing detail for one Codex batch import job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobDetail {
    /// Job summary row.
    pub summary: AdminCodexImportJobSummary,
    /// Per-item states ordered by item index.
    pub items: Vec<AdminCodexImportJobItem>,
}

/// New batch import job persisted before background execution starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexImportJob {
    /// Batch job id.
    pub job_id: String,
    /// Provider type.
    pub provider_type: String,
    /// Import source type.
    pub source_type: String,
    /// Whether refresh validation runs before import.
    pub validate_before_import: bool,
    /// Submitted items.
    pub items: Vec<NewAdminCodexImportJobItem>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// One submitted item persisted as part of a new batch import job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexImportJobItem {
    /// Requested account name.
    pub requested_name: String,
    /// Requested upstream account id when present.
    pub requested_account_id: Option<String>,
    /// Stored raw auth JSON for background processing.
    pub raw_auth_json: String,
}

/// Terminal update written after processing one batch import item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCodexImportJobItemResult {
    /// Zero-based item index within the batch.
    pub item_index: usize,
    /// Terminal item status.
    pub status: String,
    /// Terminal error message when the item fails.
    pub error_message: Option<String>,
    /// Imported account name when successful.
    pub imported_account_name: Option<String>,
    /// Final upstream account id after validation/import.
    pub final_account_id: Option<String>,
    /// Validation timestamp when refresh validation succeeds.
    pub validated_at_ms: Option<i64>,
    /// Import timestamp when the account row is created.
    pub imported_at_ms: Option<i64>,
    /// Completed-item counter increment.
    pub completed_delta: usize,
    /// Imported-item counter increment.
    pub succeeded_delta: usize,
    /// Skipped-item counter increment.
    pub skipped_delta: usize,
    /// Failed/conflict-item counter increment.
    pub failed_delta: usize,
    /// Last update timestamp.
    pub updated_at_ms: i64,
}

/// Admin-facing Kiro account balance snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroBalanceView {
    /// Current upstream credit usage.
    pub current_usage: f64,
    /// Current upstream credit limit.
    pub usage_limit: f64,
    /// Remaining upstream credits.
    pub remaining: f64,
    /// Next reset timestamp in Unix milliseconds.
    pub next_reset_at: Option<i64>,
    /// Upstream subscription title.
    pub subscription_title: Option<String>,
    /// Upstream user id when the status API provides it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

/// Admin-facing Kiro status-cache metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminKiroCacheView {
    /// Cache status label.
    pub status: String,
    /// Expected refresh interval in seconds.
    pub refresh_interval_seconds: u64,
    /// Last status-check attempt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<i64>,
    /// Last successful status-check timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_success_at: Option<i64>,
    /// Last status-check error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
}

impl Default for AdminKiroCacheView {
    fn default() -> Self {
        Self {
            status: "loading".to_string(),
            refresh_interval_seconds: DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
            last_checked_at: None,
            last_success_at: None,
            error_message: None,
        }
    }
}

/// Admin-facing projection of one configured Kiro account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroAccount {
    /// Account display name.
    pub name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Identity provider label.
    pub provider: Option<String>,
    /// Upstream user id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_user_id: Option<String>,
    /// Account email when known.
    pub email: Option<String>,
    /// Access token expiry string.
    pub expires_at: Option<String>,
    /// Kiro profile ARN.
    pub profile_arn: Option<String>,
    /// Whether a refresh token is available.
    pub has_refresh_token: bool,
    /// Whether this account is disabled.
    pub disabled: bool,
    /// Disable/error reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disabled_reason: Option<String>,
    /// Import source label.
    pub source: Option<String>,
    /// Import source DB path.
    pub source_db_path: Option<String>,
    /// Last import timestamp.
    pub last_imported_at: Option<i64>,
    /// Subscription title.
    pub subscription_title: Option<String>,
    /// Default region.
    pub region: Option<String>,
    /// Auth region.
    pub auth_region: Option<String>,
    /// API region.
    pub api_region: Option<String>,
    /// Machine id.
    pub machine_id: Option<String>,
    /// Per-account request concurrency cap.
    pub kiro_channel_max_concurrency: u64,
    /// Per-account request pacing interval.
    pub kiro_channel_min_start_interval_ms: u64,
    /// Cached-credit floor used before blocking the account locally.
    pub minimum_remaining_credits_before_block: f64,
    /// Account proxy mode.
    pub proxy_mode: String,
    /// Fixed proxy config id.
    pub proxy_config_id: Option<String>,
    /// Effective proxy source.
    pub effective_proxy_source: String,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Legacy embedded proxy URL if present.
    pub proxy_url: Option<String>,
    /// Cached balance snapshot.
    pub balance: Option<AdminKiroBalanceView>,
    /// Cached status metadata.
    pub cache: AdminKiroCacheView,
}

/// Page of admin Kiro accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminKiroAccountsPage {
    /// Page rows.
    pub accounts: Vec<AdminKiroAccount>,
    /// Full aggregate over all Kiro accounts.
    pub summary: AdminAccountsSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// New persisted Kiro account row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminKiroAccount {
    /// Account display name.
    pub name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id when known.
    pub user_id: Option<String>,
    /// Runtime account status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Per-account request concurrency cap.
    pub max_concurrency: Option<u64>,
    /// Per-account request pacing interval.
    pub min_start_interval_ms: Option<u64>,
    /// Fixed proxy config id when configured.
    pub proxy_config_id: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for mutable Kiro account routing/scheduler settings.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdminKiroAccountPatch {
    /// New runtime status.
    pub status: Option<String>,
    /// New per-account request concurrency cap.
    pub max_concurrency: Option<u64>,
    /// New per-account request pacing interval.
    pub min_start_interval_ms: Option<u64>,
    /// New cached-credit floor.
    pub minimum_remaining_credits_before_block: Option<f64>,
    /// New account proxy mode.
    pub proxy_mode: Option<String>,
    /// New fixed proxy config id.
    pub proxy_config_id: Option<Option<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Cached Kiro account status update produced by a balance refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct AdminKiroStatusCacheUpdate {
    /// Account name.
    pub account_name: String,
    /// Cached balance payload.
    pub balance: Option<AdminKiroBalanceView>,
    /// Cache metadata.
    pub cache: AdminKiroCacheView,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
    /// Expiration timestamp.
    pub expires_at_ms: i64,
    /// Last refresh error.
    pub last_error: Option<String>,
}

/// Minimal Kiro account projection used by background status refresh.
#[derive(Debug, Clone, PartialEq)]
pub struct KiroStatusRefreshTarget {
    /// Account display name.
    pub name: String,
    /// Whether refresh should be skipped and persisted as disabled.
    pub disabled: bool,
    /// Cached status metadata used when preserving disabled state.
    pub cache: AdminKiroCacheView,
}

/// Key state used on the hot request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type as snake_case string.
    pub provider_type: String,
    /// Protocol family as snake_case string.
    pub protocol_family: String,
    /// Key status.
    pub status: String,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Billable usage already consumed.
    pub billable_tokens_used: i64,
}

/// Resolved proxy settings for one upstream provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProxyConfig {
    /// Proxy URL accepted by `reqwest::Proxy`.
    pub proxy_url: String,
    /// Optional proxy username.
    pub proxy_username: Option<String>,
    /// Optional proxy password.
    pub proxy_password: Option<String>,
}

/// Resolved Codex account selected for one provider request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCodexRoute {
    /// Selected account name.
    pub account_name: String,
    /// Account group id from the key route config at resolution time.
    pub account_group_id_at_event: Option<String>,
    /// Effective route strategy from the key route config at resolution time.
    pub route_strategy_at_event: RouteStrategy,
    /// Persisted auth JSON for the selected account.
    pub auth_json: String,
    /// Whether this account maps public gpt-5.3-codex to Spark upstream.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auth_refresh_enabled: bool,
    /// Request concurrency cap configured on this key route.
    pub request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on this key route.
    pub request_min_start_interval_ms: Option<u64>,
    /// Request concurrency cap configured on the selected account.
    pub account_request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on the selected
    /// account.
    pub account_request_min_start_interval_ms: Option<u64>,
    /// Cached account error message captured from the latest status view.
    pub cached_error_message: Option<String>,
    /// Resolved proxy settings for this upstream request.
    pub proxy: Option<ProviderProxyConfig>,
}

/// Refreshed Codex account credential fields persisted by the provider runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderCodexAuthUpdate {
    /// Account name.
    pub account_name: String,
    /// Refreshed auth payload JSON.
    pub auth_json: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Runtime status after refresh.
    pub status: String,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
}

/// Whether a Codex auth error is terminal and should immediately block
/// request routing for that account until credentials are refreshed.
pub fn is_terminal_codex_auth_error(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("refresh_token_invalidated")
        || message.contains("refresh token has been invalidated")
        || message.contains("refresh_token_reused")
        || message.contains("refresh token has already been used")
        || ((message.contains("codex refresh token returned 401")
            || message.contains("codex refresh token returned 403"))
            && message.contains("invalid_request_error"))
        || ((message.contains("codex request returned 401")
            || message.contains("codex request returned 403"))
            && message.contains("after forced refresh"))
        || message.contains("codex auth refresh disabled and current access token expired")
}

/// Decode the JWT `exp` claim from one access token into Unix milliseconds.
pub fn jwt_expiry_unix_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(&decoded).ok()?;
    let exp_seconds = value.get("exp")?.as_i64()?;
    exp_seconds.checked_mul(1000)
}

/// Extract the current Codex access token expiry timestamp from persisted auth
/// JSON when it is available.
pub fn codex_auth_access_token_expires_at_ms(auth_json: &str) -> Option<i64> {
    let value: Value = serde_json::from_str(auth_json).ok()?;
    let access_token = json_string_any(&value, &["access_token", "accessToken"]).or_else(|| {
        value
            .get("tokens")
            .and_then(|tokens| json_string_any(tokens, &["access_token", "accessToken"]))
    })?;
    codex_access_token_expires_at_ms(Some(access_token.as_str()))
}

/// Decode the JWT expiry from a Codex access token string.
pub fn codex_access_token_expires_at_ms(access_token: Option<&str>) -> Option<i64> {
    let access_token = access_token
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    jwt_expiry_unix_ms(access_token)
}

fn json_string_any(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

/// Resolved Kiro account selected for one provider request.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderKiroRoute {
    /// Selected account name.
    pub account_name: String,
    /// Account group id from the key route config at resolution time.
    pub account_group_id_at_event: Option<String>,
    /// Effective route strategy from the key route config at resolution time.
    pub route_strategy_at_event: RouteStrategy,
    /// Persisted auth JSON for the selected account.
    pub auth_json: String,
    /// Profile ARN used by Kiro upstream requests.
    pub profile_arn: Option<String>,
    /// Effective API region used by Kiro upstream requests.
    pub api_region: String,
    /// Whether public request validation is enabled for this key.
    pub request_validation_enabled: bool,
    /// Whether cache estimation is enabled for this key.
    pub cache_estimation_enabled: bool,
    /// Whether zero-cache successes should retain diagnostic request bodies.
    pub zero_cache_debug_enabled: bool,
    /// Whether all successful Kiro requests should retain full request bodies.
    pub full_request_logging_enabled: bool,
    /// JSON object mapping public model names to upstream Kiro model names.
    pub model_name_map_json: String,
    /// Effective Kiro cache k-model JSON for this key.
    pub cache_kmodels_json: String,
    /// Effective Kiro cache policy JSON for this key.
    pub cache_policy_json: String,
    /// Prefix-cache simulation mode.
    pub prefix_cache_mode: String,
    /// Prefix-cache maximum token budget.
    pub prefix_cache_max_tokens: u64,
    /// Prefix-cache entry TTL in seconds.
    pub prefix_cache_entry_ttl_seconds: u64,
    /// Conversation-anchor maximum entries.
    pub conversation_anchor_max_entries: u64,
    /// Conversation-anchor TTL in seconds.
    pub conversation_anchor_ttl_seconds: u64,
    /// Effective Kiro billable multiplier JSON for this key.
    pub billable_model_multipliers_json: String,
    /// Request concurrency cap configured on this key route.
    pub request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on this key route.
    pub request_min_start_interval_ms: Option<u64>,
    /// Request concurrency cap configured on the selected account.
    pub account_request_max_concurrency: Option<u64>,
    /// Minimum interval between request starts configured on the selected
    /// account.
    pub account_request_min_start_interval_ms: Option<u64>,
    /// Resolved proxy settings for this upstream request.
    pub proxy: Option<ProviderProxyConfig>,
    /// Runtime routing identity used for local throttles and cooldowns. This is
    /// the upstream user id when known, otherwise the account name.
    pub routing_identity: String,
    /// Last cached status label for diagnostics and route ordering.
    pub cached_status: Option<String>,
    /// Last cached remaining credits for fairness ordering.
    pub cached_remaining_credits: Option<f64>,
    /// Last cached balance payload for request-time status refreshes.
    pub cached_balance: Option<AdminKiroBalanceView>,
    /// Last cached status metadata for request-time status refreshes.
    pub cached_cache: Option<AdminKiroCacheView>,
    /// Status-refresh interval used when this route updates the cache.
    pub status_refresh_interval_seconds: u64,
    /// Cached-credit floor used before locally blocking this account.
    pub minimum_remaining_credits_before_block: f64,
}

/// Refreshed Kiro account credential fields persisted by the provider runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderKiroAuthUpdate {
    /// Account name.
    pub account_name: String,
    /// Refreshed auth payload JSON.
    pub auth_json: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id when known.
    pub user_id: Option<String>,
    /// Runtime status after refresh.
    pub status: String,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Refresh timestamp.
    pub refreshed_at_ms: i64,
}

impl AuthenticatedKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        self.quota_billable_limit
            .saturating_sub(self.billable_tokens_used)
    }
}

/// Public-safe key summary used by the unauthenticated access endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccessKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Plaintext public key secret.
    pub secret: String,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated billable tokens.
    pub usage_billable_tokens: u64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
}

/// Public usage lookup key and current rollup state.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicUsageLookupKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type.
    pub provider_type: String,
    /// Key status.
    pub status: String,
    /// Whether this key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated billable tokens.
    pub usage_billable_tokens: u64,
    /// Accumulated credit usage.
    pub usage_credit_total: f64,
    /// Number of events missing credit usage.
    pub usage_credit_missing_events: u64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
}

/// Public thank-you card for an approved account contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccountContribution {
    /// Request id.
    pub request_id: String,
    /// Imported account display name.
    pub account_name: String,
    /// Contributor-supplied message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval/issuance timestamp.
    pub processed_at_ms: Option<i64>,
}

/// Public thank-you card for an approved sponsor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSponsor {
    /// Request id.
    pub request_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Sponsor-supplied message.
    pub sponsor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval timestamp.
    pub processed_at_ms: Option<i64>,
}

/// New public token request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicTokenRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Requested billable quota.
    pub requested_quota_billable_limit: u64,
    /// Requester explanation.
    pub request_reason: String,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// New public Codex account contribution request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicAccountContributionRequest {
    /// Stable request id.
    pub request_id: String,
    /// Proposed account display name.
    pub account_name: String,
    /// Optional upstream account id.
    pub account_id: Option<String>,
    /// Upstream id token.
    pub id_token: String,
    /// Upstream access token.
    pub access_token: String,
    /// Upstream refresh token.
    pub refresh_token: String,
    /// Requester email address.
    pub requester_email: String,
    /// Contributor message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Whether this contribution should be shown on the public thank-you wall.
    pub show_on_public_wall: bool,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// New public sponsor request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicSponsorRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Sponsor message.
    pub sponsor_message: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Admin-facing projection of one token request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminTokenRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Requested billable quota.
    pub requested_quota_billable_limit: u64,
    /// Requester explanation.
    pub request_reason: String,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Issued key id when the request has produced a key.
    pub issued_key_id: Option<String>,
    /// Issued key name when the request has produced a key.
    pub issued_key_name: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for token requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminTokenRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminTokenRequest>,
}

/// Admin-facing projection of one account contribution request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountContributionRequest {
    /// Stable request id.
    pub request_id: String,
    /// Proposed account display name.
    pub account_name: String,
    /// Optional upstream account id.
    pub account_id: Option<String>,
    /// Upstream id token.
    pub id_token: String,
    /// Upstream access token.
    pub access_token: String,
    /// Upstream refresh token.
    pub refresh_token: String,
    /// Requester email address.
    pub requester_email: String,
    /// Contributor message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Imported account name after approval.
    pub imported_account_name: Option<String>,
    /// Issued key id after approval.
    pub issued_key_id: Option<String>,
    /// Issued key name after approval.
    pub issued_key_name: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for account contribution requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountContributionRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminAccountContributionRequest>,
}

/// Admin-facing projection of one sponsor request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSponsorRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Sponsor message.
    pub sponsor_message: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Payment email timestamp in Unix milliseconds.
    pub payment_email_sent_at: Option<i64>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for sponsor requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSponsorRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminSponsorRequest>,
}

/// Normalized admin review-queue pagination query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminReviewQueueQuery {
    /// Optional status filter.
    pub status: Option<String>,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
}

/// Admin action metadata applied to one review queue item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminReviewQueueAction {
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// Physical usage-event source queried by admin and public compatibility views.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UsageEventSource {
    /// Query the currently writable active usage store.
    Hot,
    /// Query immutable archived usage segments.
    Archive,
    /// Query both active and archived usage data.
    #[default]
    All,
}

impl UsageEventSource {
    /// Parse a user-facing query value.
    pub fn from_query_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "hot" => Some(Self::Hot),
            "archive" | "archived" => Some(Self::Archive),
            "all" => Some(Self::All),
            _ => None,
        }
    }

    /// Whether active usage rows should be consulted.
    pub fn includes_hot(self) -> bool {
        matches!(self, Self::Hot | Self::All)
    }

    /// Whether archived usage rows should be consulted.
    pub fn includes_archive(self) -> bool {
        matches!(self, Self::Archive | Self::All)
    }
}

/// Paginated usage-event query used by admin and public compatibility views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageEventQuery {
    /// Optional key filter.
    pub key_id: Option<String>,
    /// Optional provider filter.
    pub provider_type: Option<String>,
    /// Optional model filter.
    pub model: Option<String>,
    /// Optional account filter.
    pub account_name: Option<String>,
    /// Optional endpoint filter.
    pub endpoint: Option<String>,
    /// Optional status code filter.
    pub status_code: Option<i32>,
    /// Physical usage event source.
    pub source: UsageEventSource,
    /// Optional inclusive lower creation timestamp bound in Unix milliseconds.
    pub start_ms: Option<i64>,
    /// Optional exclusive upper creation timestamp bound in Unix milliseconds.
    pub end_ms: Option<i64>,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
}

/// Aggregate totals over a filtered usage-event result set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UsageEventTotals {
    /// Count of matching events.
    pub event_count: usize,
    /// Sum of uncached input tokens across all matches.
    pub input_uncached_tokens: u64,
    /// Sum of cached input tokens across all matches.
    pub input_cached_tokens: u64,
    /// Sum of output tokens across all matches.
    pub output_tokens: u64,
    /// Sum of billable tokens across all matches.
    pub billable_tokens: u64,
}

/// Usage-event page returned by the analytics store.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventPage {
    /// Total matching rows.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether more rows remain after this page.
    pub has_more: bool,
    /// Aggregate totals over the full filtered result set.
    pub totals: UsageEventTotals,
    /// Usage events in newest-first order.
    pub events: Vec<UsageEvent>,
}

/// One public usage chart bucket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageChartPoint {
    /// Bucket start timestamp in Unix milliseconds.
    pub bucket_start_ms: i64,
    /// Token total for the bucket.
    pub tokens: u64,
}

/// Result of migrating legacy embedded Kiro proxy fields into shared proxy
/// configs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminLegacyKiroProxyMigration {
    /// Proxy configs created during migration.
    pub created_configs: Vec<AdminProxyConfig>,
    /// Existing proxy configs reused by matching legacy tuples.
    pub reused_configs: Vec<AdminProxyConfig>,
    /// Kiro account names updated by migration.
    pub migrated_account_names: Vec<String>,
}

impl PublicAccessKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        let limit = i64::try_from(self.quota_billable_limit).unwrap_or(i64::MAX);
        let used = i64::try_from(self.usage_billable_tokens).unwrap_or(i64::MAX);
        limit.saturating_sub(used)
    }
}

impl PublicUsageLookupKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        let limit = i64::try_from(self.quota_billable_limit).unwrap_or(i64::MAX);
        let used = i64::try_from(self.usage_billable_tokens).unwrap_or(i64::MAX);
        limit.saturating_sub(used)
    }
}

/// Control-plane queries used by request handlers.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Authenticate a bearer secret by hashing it and loading the key state.
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>>;

    /// Increment usage counters for a key after a usage event is accepted.
    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()>;

    /// Increment usage counters for one owned usage event.
    async fn apply_usage_rollup_owned(&self, event: UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollup(&event).await
    }
}

/// Provider route/account resolution used by data-plane dispatch.
#[async_trait]
pub trait ProviderRouteStore: Send + Sync {
    /// Resolve the Codex account to use for an authenticated key.
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Resolve all Codex account candidates for one authenticated key.
    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self.resolve_codex_route(key).await?.into_iter().collect())
    }

    /// Reload one active Codex account route by account name.
    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Resolve the Kiro account to use for an authenticated key.
    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>>;

    /// Resolve all Kiro account candidates for one authenticated key.
    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        Ok(self.resolve_kiro_route(key).await?.into_iter().collect())
    }

    /// Reload one active Kiro account route by account name.
    async fn resolve_kiro_account_route(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    /// Persist a refreshed Kiro credential snapshot.
    async fn save_kiro_auth_update(&self, update: ProviderKiroAuthUpdate) -> anyhow::Result<()>;

    /// Persist a refreshed Codex credential snapshot.
    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()>;

    /// Enable or disable automatic Codex auth refresh for one account.
    async fn set_codex_account_auto_refresh_enabled(
        &self,
        _account_name: &str,
        _enabled: bool,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Persist a hot-path Kiro account quota-exhausted marker.
    async fn mark_kiro_account_quota_exhausted(
        &self,
        _account_name: &str,
        _error_message: &str,
        _checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Persist a Kiro account status-cache update produced on the hot path.
    async fn save_kiro_status_cache_update(
        &self,
        _update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Public read-only queries used by unauthenticated public endpoints.
#[async_trait]
pub trait PublicAccessStore: Send + Sync {
    /// Current auth-cache TTL in seconds.
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64>;

    /// Active, public-visible LLM gateway keys.
    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>>;
}

/// Public read-only community queries used by unauthenticated compatibility
/// endpoints.
#[async_trait]
pub trait PublicCommunityStore: Send + Sync {
    /// Approved account contribution cards.
    async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>>;

    /// Approved sponsor cards.
    async fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>>;
}

/// Public usage lookup queries used by unauthenticated public endpoints.
#[async_trait]
pub trait PublicUsageStore: Send + Sync {
    /// Load one key by its presented plaintext secret for public usage lookup.
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>>;
}

/// Analytics queries over settled usage events.
#[async_trait]
pub trait UsageAnalyticsStore: Send + Sync {
    /// List settled usage events.
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage>;

    /// Load one settled usage event.
    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>>;

    /// Return chart buckets for one key.
    async fn usage_chart_points(
        &self,
        key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>>;
}

/// Public write queries used by unauthenticated public endpoints.
#[async_trait]
pub trait PublicSubmissionStore: Send + Sync {
    /// Persist one public token request.
    async fn create_public_token_request(
        &self,
        request: NewPublicTokenRequest,
    ) -> anyhow::Result<()>;

    /// Persist one public account contribution request.
    async fn create_public_account_contribution_request(
        &self,
        request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()>;

    /// Return whether the proposed account contribution name conflicts with an
    /// existing account or live contribution request.
    async fn public_account_contribution_name_exists(
        &self,
        account_name: &str,
    ) -> anyhow::Result<bool>;

    /// Persist one public sponsor request.
    async fn create_public_sponsor_request(
        &self,
        request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()>;

    /// Persist the payment-email result for one public sponsor request.
    async fn record_public_sponsor_payment_email_result(
        &self,
        request_id: &str,
        sent_at_ms: Option<i64>,
        failure_reason: Option<String>,
    ) -> anyhow::Result<()>;
}

/// Admin runtime config queries used by the standalone frontend surface.
#[async_trait]
pub trait AdminConfigStore: Send + Sync {
    /// Load the current runtime config, or the built-in defaults if no row has
    /// been imported yet.
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig>;

    /// Persist a full runtime config row and return the stored view.
    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig>;
}

/// Admin key management queries used by the current frontend.
#[async_trait]
pub trait AdminKeyStore: Send + Sync {
    /// List all managed keys.
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>>;

    /// Load one managed key by id.
    async fn get_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>>;

    /// List one page of managed keys, optionally scoped to one provider.
    async fn list_admin_keys_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage>;

    /// List one filtered page of managed keys.
    async fn list_admin_keys_filtered_page(
        &self,
        provider_type: Option<&str>,
        query: &AdminKeyPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        let mut keys = self.list_admin_keys().await?;
        if let Some(provider_type) = provider_type {
            keys.retain(|key| key.provider_type == provider_type);
        }
        let summary = summarize_admin_keys(&keys);
        apply_admin_key_query(&mut keys, query);
        let total = keys.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let keys = keys[start..end].to_vec();
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// Find one key that references an account group.
    async fn find_admin_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>>;

    /// Create one managed key.
    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey>;

    /// Patch one managed key by id.
    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>>;

    /// Delete one managed key by id and return the removed row.
    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>>;
}

/// Admin account-group management queries used by the current frontend.
#[async_trait]
pub trait AdminAccountGroupStore: Send + Sync {
    /// List all account groups for one provider.
    async fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>>;

    /// List one page of account groups for one provider.
    async fn list_admin_account_groups_page(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminAccountGroupsPage> {
        let groups = self.list_admin_account_groups(provider_type).await?;
        let total = groups.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let groups = groups[start..end].to_vec();
        Ok(AdminAccountGroupsPage {
            has_more: page.has_more(groups.len(), total),
            groups,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// List lightweight account-group selector options for one provider.
    async fn list_admin_account_group_options(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroupOption>> {
        Ok(self
            .list_admin_account_groups(provider_type)
            .await?
            .into_iter()
            .map(|group| AdminAccountGroupOption {
                account_count: group.account_names.len(),
                single_account_name: (group.account_names.len() == 1)
                    .then(|| group.account_names[0].clone()),
                id: group.id,
                provider_type: group.provider_type,
                name: group.name,
            })
            .collect())
    }

    /// Create one account group.
    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup>;

    /// Patch one account group by id.
    async fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>>;

    /// Delete one account group by id and return the removed row.
    async fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>>;
}

/// Admin reusable proxy configuration queries used by the current frontend.
#[async_trait]
pub trait AdminProxyStore: Send + Sync {
    /// List all proxy configs.
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>>;

    /// Load one proxy config by id.
    async fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// Create one proxy config.
    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig>;

    /// Patch one proxy config by id.
    async fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// Delete one proxy config by id and return the removed row.
    async fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// List effective provider-level proxy bindings.
    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>>;

    /// Update or clear one provider-level proxy binding.
    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding>;

    /// Import legacy embedded Kiro proxy fields into shared proxy configs.
    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration>;
}

/// Admin Codex account management queries used by the current frontend.
#[async_trait]
pub trait AdminCodexAccountStore: Send + Sync {
    /// List all imported Codex accounts.
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>>;

    /// List one page of imported Codex accounts.
    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage>;

    /// List the minimal Codex account fields needed by background status
    /// refresh.
    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>>;

    /// Get one imported Codex account by name.
    async fn get_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Resolve one existing Codex account name by upstream account id.
    async fn find_admin_codex_account_name_by_account_id(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<String>>;

    /// Import one Codex account.
    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount>;

    /// Patch one Codex account.
    async fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Delete one Codex account.
    async fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Mark one Codex account as refreshed and return its latest summary.
    async fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Resolve a single Codex account as a provider route for admin refreshes.
    async fn resolve_admin_codex_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Persist one new Codex batch import job and its queued items.
    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail>;

    /// List recent Codex batch import jobs ordered newest first.
    async fn list_admin_codex_import_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>>;

    /// Load one Codex batch import job with all item states.
    async fn get_admin_codex_import_job(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>>;

    /// Mark one batch job as actively running.
    async fn mark_admin_codex_import_job_running(
        &self,
        job_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()>;

    /// Mark one batch item as actively running.
    async fn mark_admin_codex_import_job_item_running(
        &self,
        job_id: &str,
        item_index: usize,
        updated_at_ms: i64,
    ) -> anyhow::Result<()>;

    /// Complete one batch item and roll up job counters.
    async fn complete_admin_codex_import_job_item(
        &self,
        job_id: &str,
        result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>>;

    /// Mark one batch job as failed before all items could complete.
    async fn fail_admin_codex_import_job(
        &self,
        job_id: &str,
        error_message: &str,
        finished_at_ms: i64,
    ) -> anyhow::Result<()>;
}

/// Admin Kiro account management queries used by the current frontend.
#[async_trait]
pub trait AdminKiroAccountStore: Send + Sync {
    /// List all persisted Kiro accounts with cached status information.
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>>;

    /// List one page of persisted Kiro accounts.
    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage>;

    /// List one page of persisted Kiro accounts, optionally filtered by a
    /// case-insensitive account-name prefix.
    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage>;

    /// List the minimal Kiro account fields needed by background status
    /// refresh.
    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>>;

    /// Create or replace one Kiro account.
    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount>;

    /// Patch one Kiro account.
    async fn patch_admin_kiro_account(
        &self,
        name: &str,
        patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>>;

    /// Delete one Kiro account.
    async fn delete_admin_kiro_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>>;

    /// Return the cached Kiro balance for one account.
    async fn get_admin_kiro_balance(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>>;

    /// Resolve a single Kiro account as a provider route for admin refreshes.
    async fn resolve_admin_kiro_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>>;

    /// Persist one Kiro status-cache update.
    async fn save_admin_kiro_status_cache(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()>;
}

/// Admin review queue queries used by the current frontend.
#[async_trait]
pub trait AdminReviewQueueStore: Send + Sync {
    /// Load one token request.
    async fn get_admin_token_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// List token requests.
    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage>;

    /// Load one account contribution request.
    async fn get_admin_account_contribution_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// List account contribution requests.
    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage>;

    /// Load one sponsor request.
    async fn get_admin_sponsor_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>>;

    /// List sponsor requests.
    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage>;

    /// Issue a token request and create the key when needed.
    async fn issue_admin_token_request(
        &self,
        request_id: &str,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// Reject a token request and disable any partially issued key.
    async fn reject_admin_token_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// Issue an account contribution request and create account, group, and key
    /// records when needed.
    async fn issue_admin_account_contribution_request(
        &self,
        request_id: &str,
        account: Option<NewAdminCodexAccount>,
        account_group: Option<NewAdminAccountGroup>,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Mark an account contribution request as validated after a successful
    /// Codex auth refresh check.
    async fn validate_admin_account_contribution_request(
        &self,
        request_id: &str,
        account_id: Option<String>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Mark an account contribution request as failed after validation rejects
    /// the supplied auth.
    async fn fail_admin_account_contribution_request(
        &self,
        request_id: &str,
        failure_reason: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Reject an account contribution request and disable/remove partial
    /// records.
    async fn reject_admin_account_contribution_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Approve one sponsor request.
    async fn approve_admin_sponsor_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>>;

    /// Delete one sponsor request from admin review/history.
    async fn delete_admin_sponsor_request(&self, request_id: &str) -> anyhow::Result<bool>;
}

/// Empty public-access store used by isolated unit tests.
pub struct EmptyPublicAccessStore;

/// Empty provider route store used by isolated unit tests.
pub struct EmptyProviderRouteStore;

#[async_trait]
impl ProviderRouteStore for EmptyProviderRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_codex_account_route(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(&self, _update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        _account_name: &str,
        _error_message: &str,
        _checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl PublicAccessStore for EmptyPublicAccessStore {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        Ok(DEFAULT_AUTH_CACHE_TTL_SECONDS)
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        Ok(Vec::new())
    }
}

/// Empty community store used by isolated unit tests.
pub struct EmptyPublicCommunityStore;

#[async_trait]
impl PublicCommunityStore for EmptyPublicCommunityStore {
    async fn list_public_account_contributions(
        &self,
        _limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        Ok(Vec::new())
    }

    async fn list_public_sponsors(&self, _limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        Ok(Vec::new())
    }
}

/// Empty public usage store used by isolated unit tests.
pub struct EmptyPublicUsageStore;

#[async_trait]
impl PublicUsageStore for EmptyPublicUsageStore {
    async fn get_public_usage_key_by_secret(
        &self,
        _secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        Ok(None)
    }
}

/// Empty analytics store used by isolated unit tests.
pub struct EmptyUsageAnalyticsStore;

#[async_trait]
impl UsageAnalyticsStore for EmptyUsageAnalyticsStore {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        Ok(UsageEventPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            totals: UsageEventTotals::default(),
            events: Vec::new(),
        })
    }

    async fn get_usage_event(&self, _event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        Ok(None)
    }

    async fn usage_chart_points(
        &self,
        _key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>> {
        Ok((0..bucket_count)
            .map(|index| UsageChartPoint {
                bucket_start_ms: start_ms.saturating_add((index as i64).saturating_mul(bucket_ms)),
                tokens: 0,
            })
            .collect())
    }
}

/// No-op usage sink used by isolated unit tests.
pub struct NoopUsageEventSink;

#[async_trait]
impl UsageEventSink for NoopUsageEventSink {
    async fn append_usage_event(&self, _event: &UsageEvent) -> anyhow::Result<()> {
        Ok(())
    }

    async fn append_usage_events(&self, _events: &[UsageEvent]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty public submission store used by isolated unit tests.
pub struct EmptyPublicSubmissionStore;

#[async_trait]
impl PublicSubmissionStore for EmptyPublicSubmissionStore {
    async fn create_public_token_request(
        &self,
        _request: NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn create_public_account_contribution_request(
        &self,
        _request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn public_account_contribution_name_exists(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn create_public_sponsor_request(
        &self,
        _request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_public_sponsor_payment_email_result(
        &self,
        _request_id: &str,
        _sent_at_ms: Option<i64>,
        _failure_reason: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty admin config store used by isolated unit tests.
pub struct EmptyAdminConfigStore;

#[async_trait]
impl AdminConfigStore for EmptyAdminConfigStore {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(AdminRuntimeConfig::default())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(config)
    }
}

/// Empty admin key store used by isolated unit tests.
pub struct EmptyAdminKeyStore;

#[async_trait]
impl AdminKeyStore for EmptyAdminKeyStore {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        Ok(Vec::new())
    }

    async fn get_admin_key(&self, _key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn list_admin_keys_page(
        &self,
        _provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        Ok(AdminKeysPage {
            keys: Vec::new(),
            summary: AdminKeysSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn find_admin_key_referencing_account_group(
        &self,
        _provider_type: &str,
        _group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        Ok(AdminKey {
            id: key.id,
            name: key.name,
            secret: key.secret,
            key_hash: key.key_hash,
            status: KEY_STATUS_ACTIVE.to_string(),
            provider_type: key.provider_type,
            public_visible: key.public_visible,
            quota_billable_limit: key.quota_billable_limit,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            remaining_billable: key.quota_billable_limit as i64,
            last_used_at: None,
            created_at: key.created_at_ms,
            updated_at: key.created_at_ms,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: key.request_max_concurrency,
            request_min_start_interval_ms: key.request_min_start_interval_ms,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: default_kiro_cache_policy_json(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json:
                default_kiro_billable_model_multipliers_json(),
            uses_global_kiro_billable_model_multipliers: true,
            kiro_candidate_credit_summary: None,
        })
    }

    async fn patch_admin_key(
        &self,
        _key_id: &str,
        _patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn delete_admin_key(&self, _key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }
}

/// Empty admin account-group store used by isolated unit tests.
pub struct EmptyAdminAccountGroupStore;

#[async_trait]
impl AdminAccountGroupStore for EmptyAdminAccountGroupStore {
    async fn list_admin_account_groups(
        &self,
        _provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        Ok(Vec::new())
    }

    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        Ok(AdminAccountGroup {
            id: group.id,
            provider_type: group.provider_type,
            name: group.name,
            account_names: group.account_names,
            created_at: group.created_at_ms,
            updated_at: group.created_at_ms,
        })
    }

    async fn patch_admin_account_group(
        &self,
        _group_id: &str,
        _patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        Ok(None)
    }

    async fn delete_admin_account_group(
        &self,
        _group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        Ok(None)
    }
}

/// Empty admin proxy store used by isolated unit tests.
pub struct EmptyAdminProxyStore;

#[async_trait]
impl AdminProxyStore for EmptyAdminProxyStore {
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        Ok(Vec::new())
    }

    async fn get_admin_proxy_config(
        &self,
        _proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        Ok(AdminProxyConfig {
            id: proxy.id,
            name: proxy.name,
            proxy_url: proxy.proxy_url,
            proxy_username: proxy.proxy_username,
            proxy_password: proxy.proxy_password,
            status: KEY_STATUS_ACTIVE.to_string(),
            created_at: proxy.created_at_ms,
            updated_at: proxy.created_at_ms,
        })
    }

    async fn patch_admin_proxy_config(
        &self,
        _proxy_id: &str,
        _patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn delete_admin_proxy_config(
        &self,
        _proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        Ok(default_proxy_bindings())
    }

    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        _proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        Ok(default_proxy_binding(provider_type))
    }

    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration> {
        Ok(AdminLegacyKiroProxyMigration {
            created_configs: Vec::new(),
            reused_configs: Vec::new(),
            migrated_account_names: Vec::new(),
        })
    }
}

/// Empty admin Codex account store used by isolated unit tests.
pub struct EmptyAdminCodexAccountStore;

/// Empty admin Kiro account store used by isolated unit tests.
pub struct EmptyAdminKiroAccountStore;

#[async_trait]
impl AdminCodexAccountStore for EmptyAdminCodexAccountStore {
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        Ok(Vec::new())
    }

    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        Ok(AdminCodexAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>> {
        Ok(Vec::new())
    }

    async fn get_admin_codex_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn find_admin_codex_account_name_by_account_id(
        &self,
        _account_id: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        Ok(AdminCodexAccount {
            name: account.name,
            status: KEY_STATUS_ACTIVE.to_string(),
            account_id: account.account_id,
            plan_type: None,
            route_weight_tier: account
                .route_weight_tier
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            auto_refresh_enabled: account.auto_refresh_enabled,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "none".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: Some(account.created_at_ms),
            access_token_expires_at: codex_auth_access_token_expires_at_ms(&account.auth_json),
            auth_refresh_error_message: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    async fn patch_admin_codex_account(
        &self,
        _name: &str,
        _patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn delete_admin_codex_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn refresh_admin_codex_account(
        &self,
        _name: &str,
        _refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn resolve_admin_codex_account_route(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail> {
        Ok(AdminCodexImportJobDetail {
            summary: AdminCodexImportJobSummary {
                job_id: job.job_id,
                provider_type: job.provider_type,
                source_type: job.source_type,
                validate_before_import: job.validate_before_import,
                status: "pending".to_string(),
                total_count: job.items.len(),
                completed_count: 0,
                succeeded_count: 0,
                skipped_count: 0,
                failed_count: 0,
                batch_error_message: None,
                created_at_ms: job.created_at_ms,
                updated_at_ms: job.created_at_ms,
                finished_at_ms: None,
            },
            items: job
                .items
                .into_iter()
                .enumerate()
                .map(|(item_index, item)| AdminCodexImportJobItem {
                    item_index,
                    requested_name: item.requested_name,
                    requested_account_id: item.requested_account_id,
                    status: "pending".to_string(),
                    error_message: None,
                    imported_account_name: None,
                    final_account_id: None,
                    validated_at_ms: None,
                    imported_at_ms: None,
                })
                .collect(),
        })
    }

    async fn list_admin_codex_import_jobs(
        &self,
        _limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>> {
        Ok(Vec::new())
    }

    async fn get_admin_codex_import_job(
        &self,
        _job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>> {
        Ok(None)
    }

    async fn mark_admin_codex_import_job_running(
        &self,
        _job_id: &str,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn mark_admin_codex_import_job_item_running(
        &self,
        _job_id: &str,
        _item_index: usize,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn complete_admin_codex_import_job_item(
        &self,
        _job_id: &str,
        _result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        Ok(None)
    }

    async fn fail_admin_codex_import_job(
        &self,
        _job_id: &str,
        _error_message: &str,
        _finished_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl AdminKiroAccountStore for EmptyAdminKiroAccountStore {
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>> {
        Ok(Vec::new())
    }

    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        Ok(AdminKiroAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        _prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        Ok(AdminKiroAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>> {
        Ok(Vec::new())
    }

    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount> {
        Ok(AdminKiroAccount {
            name: account.name,
            auth_method: account.auth_method,
            provider: None,
            upstream_user_id: account.user_id,
            email: None,
            expires_at: None,
            profile_arn: account.profile_arn,
            has_refresh_token: false,
            disabled: account.status != KEY_STATUS_ACTIVE,
            disabled_reason: None,
            source: None,
            source_db_path: None,
            last_imported_at: None,
            subscription_title: None,
            region: None,
            auth_region: None,
            api_region: None,
            machine_id: None,
            kiro_channel_max_concurrency: account.max_concurrency.unwrap_or(1),
            kiro_channel_min_start_interval_ms: account.min_start_interval_ms.unwrap_or(0),
            minimum_remaining_credits_before_block: 0.0,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: account.proxy_config_id,
            effective_proxy_source: "none".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            proxy_url: None,
            balance: None,
            cache: AdminKiroCacheView::default(),
        })
    }

    async fn patch_admin_kiro_account(
        &self,
        _name: &str,
        _patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        Ok(None)
    }

    async fn delete_admin_kiro_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        Ok(None)
    }

    async fn get_admin_kiro_balance(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>> {
        Ok(None)
    }

    async fn resolve_admin_kiro_account_route(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    async fn save_admin_kiro_status_cache(
        &self,
        _update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty admin review queue store used by isolated unit tests.
pub struct EmptyAdminReviewQueueStore;

#[async_trait]
impl AdminReviewQueueStore for EmptyAdminReviewQueueStore {
    async fn get_admin_token_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage> {
        Ok(AdminTokenRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn get_admin_account_contribution_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage> {
        Ok(AdminAccountContributionRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn get_admin_sponsor_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        Ok(None)
    }

    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage> {
        Ok(AdminSponsorRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn issue_admin_token_request(
        &self,
        _request_id: &str,
        _key: Option<NewAdminKey>,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn reject_admin_token_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn issue_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _account: Option<NewAdminCodexAccount>,
        _account_group: Option<NewAdminAccountGroup>,
        _key: Option<NewAdminKey>,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn validate_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _account_id: Option<String>,
        _id_token: String,
        _access_token: String,
        _refresh_token: String,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn fail_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _failure_reason: String,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn reject_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn approve_admin_sponsor_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        Ok(None)
    }

    async fn delete_admin_sponsor_request(&self, _request_id: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Return the default unbound proxy binding views for supported providers.
pub fn default_proxy_bindings() -> Vec<AdminProxyBinding> {
    [PROVIDER_CODEX, PROVIDER_KIRO]
        .into_iter()
        .map(default_proxy_binding)
        .collect()
}

fn default_proxy_binding(provider_type: &str) -> AdminProxyBinding {
    AdminProxyBinding {
        provider_type: provider_type.to_string(),
        effective_source: "none".to_string(),
        bound_proxy_config_id: None,
        effective_proxy_config_name: None,
        effective_proxy_url: None,
        effective_proxy_username: None,
        effective_proxy_password: None,
        binding_updated_at: None,
        error_message: None,
    }
}

/// Public read-only payload for the cached Codex rate-limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitStatus {
    /// Snapshot status label.
    pub status: String,
    /// Suggested client refresh interval in seconds.
    pub refresh_interval_seconds: u64,
    /// Last refresh attempt timestamp in Unix milliseconds.
    pub last_checked_at: Option<i64>,
    /// Last successful refresh timestamp in Unix milliseconds.
    pub last_success_at: Option<i64>,
    /// Upstream source URL used for the status refresh.
    pub source_url: String,
    /// Last refresh error, if any.
    pub error_message: Option<String>,
    /// Per-account public summaries.
    #[serde(default)]
    pub accounts: Vec<CodexPublicAccountStatus>,
    /// Flattened rate-limit buckets across active accounts.
    pub buckets: Vec<CodexRateLimitBucket>,
}

impl CodexRateLimitStatus {
    /// Construct the same empty loading state used before the status cache
    /// warms.
    pub fn loading(refresh_interval_seconds: u64) -> Self {
        Self {
            status: "loading".to_string(),
            refresh_interval_seconds,
            last_checked_at: None,
            last_success_at: None,
            source_url: String::new(),
            error_message: None,
            accounts: Vec::new(),
            buckets: Vec::new(),
        }
    }
}

/// One public Codex account summary rendered on `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexPublicAccountStatus {
    /// Account display name.
    pub name: String,
    /// Runtime status label.
    pub status: String,
    /// Upstream plan type when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// Primary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_remaining_percent: Option<f64>,
    /// Secondary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_remaining_percent: Option<f64>,
    /// Last usage refresh attempt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_checked_at: Option<i64>,
    /// Last successful usage refresh timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_success_at: Option<i64>,
    /// Last usage refresh error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_error_message: Option<String>,
}

/// One limit bucket rendered on the public status surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitBucket {
    /// Upstream limit id.
    pub limit_id: String,
    /// Upstream limit name when available.
    pub limit_name: Option<String>,
    /// Human-readable bucket name.
    pub display_name: String,
    /// Whether this is the primary request bucket.
    pub is_primary: bool,
    /// Plan type attached to this bucket when known.
    pub plan_type: Option<String>,
    /// Primary rolling window.
    pub primary: Option<CodexRateLimitWindow>,
    /// Secondary rolling window.
    pub secondary: Option<CodexRateLimitWindow>,
    /// Credit metadata when upstream provides it.
    pub credits: Option<CodexCredits>,
    /// Account that owns this bucket in multi-account mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

/// One usage window within a rate-limit bucket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitWindow {
    /// Used percentage.
    pub used_percent: f64,
    /// Remaining percentage.
    pub remaining_percent: f64,
    /// Window duration in minutes.
    pub window_duration_mins: Option<i64>,
    /// Reset timestamp in Unix milliseconds.
    pub resets_at: Option<i64>,
}

/// Credit metadata included in upstream usage payloads when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexCredits {
    /// Whether this bucket carries credit data.
    pub has_credits: bool,
    /// Whether the account reports unlimited credits.
    pub unlimited: bool,
    /// Printable balance value.
    pub balance: Option<String>,
}

/// Public read-only queries for compatibility status endpoints.
#[async_trait]
pub trait PublicStatusStore: Send + Sync {
    /// Current cached Codex public rate-limit status.
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus>;

    /// Persist a refreshed Codex public rate-limit snapshot.
    async fn save_codex_rate_limit_status(
        &self,
        _snapshot: CodexRateLimitStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty status store used by isolated unit tests.
pub struct EmptyPublicStatusStore;

#[async_trait]
impl PublicStatusStore for EmptyPublicStatusStore {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        Ok(CodexRateLimitStatus::loading(DEFAULT_CODEX_STATUS_REFRESH_SECONDS))
    }
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist a batch of usage events.
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()>;

    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.append_usage_events(std::slice::from_ref(event)).await
    }

    /// Persist an owned batch of usage events.
    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        self.append_usage_events(&events).await
    }
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
    }
}
