//! Shared control-plane record types used by non-SQLite backends.

use llm_access_core::store::{self as core_store, AdminRuntimeConfig};
use serde::{Deserialize, Serialize};

/// Complete key state loaded from the control-plane store.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyBundle {
    /// API key row.
    pub key: KeyRecord,
    /// Route configuration row.
    pub route: KeyRouteConfig,
    /// Accumulated usage rollup row.
    pub rollup: KeyUsageRollup,
}

/// API key current-state row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRecord {
    /// Stable key id.
    pub key_id: String,
    /// Human-readable key name.
    pub name: String,
    /// Plaintext secret retained for source-compatible admin behavior.
    pub secret: String,
    /// SHA-256 hash of the bearer secret.
    pub key_hash: String,
    /// Key status.
    pub status: String,
    /// Provider type.
    pub provider_type: String,
    /// Client protocol family.
    pub protocol_family: String,
    /// Whether this key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// API key route configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteConfig {
    /// Owning key id.
    pub key_id: String,
    /// Account route strategy.
    pub route_strategy: Option<String>,
    /// Fixed account name for fixed routing.
    pub fixed_account_name: Option<String>,
    /// JSON array of account names for auto routing.
    pub auto_account_names_json: Option<String>,
    /// Account group id selected by the key.
    pub account_group_id: Option<String>,
    /// JSON object mapping public model names to upstream model names.
    pub model_name_map_json: Option<String>,
    /// Optional per-key concurrency cap.
    pub request_max_concurrency: Option<i64>,
    /// Optional per-key pacing interval.
    pub request_min_start_interval_ms: Option<i64>,
    /// Whether Codex fast/priority requests are enabled for this key.
    pub codex_fast_enabled: bool,
    /// Whether Kiro public request validation is enabled.
    pub kiro_request_validation_enabled: bool,
    /// Whether Kiro cache estimation is enabled.
    pub kiro_cache_estimation_enabled: bool,
    /// Whether zero-cache diagnostic capture is enabled.
    pub kiro_zero_cache_debug_enabled: bool,
    /// Whether every Kiro request should retain full request payloads.
    pub kiro_full_request_logging_enabled: bool,
    /// Whether URL image/document sources should be fetched server-side.
    pub kiro_remote_media_resolution_enabled: bool,
    /// Whether recent Kiro latency metrics may influence route ordering.
    pub kiro_latency_routing_enabled: bool,
    /// Optional Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<String>,
    /// Optional Kiro billable multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<String>,
}

/// API key accumulated usage rollup row.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyUsageRollup {
    /// Owning key id.
    pub key_id: String,
    /// Accumulated uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Accumulated cached input tokens.
    pub input_cached_tokens: i64,
    /// Accumulated output tokens.
    pub output_tokens: i64,
    /// Accumulated billable tokens.
    pub billable_tokens: i64,
    /// Accumulated credit usage.
    pub credit_total: f64,
    /// Number of events missing credit usage.
    pub credit_missing_events: i64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// Runtime configuration row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigRecord {
    /// Singleton id.
    pub id: String,
    /// Auth cache TTL in seconds.
    pub auth_cache_ttl_seconds: i64,
    /// Maximum request body size.
    pub max_request_body_bytes: i64,
    /// Account failure retry limit.
    pub account_failure_retry_limit: i64,
    /// Codex client version.
    pub codex_client_version: String,
    /// Default Kiro per-account concurrency.
    pub kiro_channel_max_concurrency: i64,
    /// Default Kiro per-account pacing interval.
    pub kiro_channel_min_start_interval_ms: i64,
    /// Codex minimum status refresh interval.
    pub codex_status_refresh_min_interval_seconds: i64,
    /// Codex maximum status refresh interval.
    pub codex_status_refresh_max_interval_seconds: i64,
    /// Codex per-account refresh jitter.
    pub codex_status_account_jitter_max_seconds: i64,
    /// Free Codex routing weight.
    pub codex_weight_free: i64,
    /// Plus Codex routing weight.
    pub codex_weight_plus: i64,
    /// Pro 5x Codex routing weight.
    pub codex_weight_pro5x: i64,
    /// Pro 20x Codex routing weight.
    pub codex_weight_pro20x: i64,
    /// Kiro minimum status refresh interval.
    pub kiro_status_refresh_min_interval_seconds: i64,
    /// Kiro maximum status refresh interval.
    pub kiro_status_refresh_max_interval_seconds: i64,
    /// Kiro per-account refresh jitter.
    pub kiro_status_account_jitter_max_seconds: i64,
    /// Usage event flush batch size.
    pub usage_event_flush_batch_size: i64,
    /// Usage event flush interval.
    pub usage_event_flush_interval_seconds: i64,
    /// Usage event flush max buffer bytes.
    pub usage_event_flush_max_buffer_bytes: i64,
    /// DuckDB usage writer memory limit in MiB.
    pub duckdb_usage_memory_limit_mib: i64,
    /// DuckDB usage writer WAL checkpoint threshold in MiB.
    pub duckdb_usage_checkpoint_threshold_mib: i64,
    /// Number of recent days retained in DuckDB usage analytics.
    pub usage_analytics_retention_days: i64,
    /// Whether API workers write usage events to local journal files.
    pub usage_journal_enabled: bool,
    /// Maximum compressed journal file bytes before sealing.
    pub usage_journal_max_file_bytes: i64,
    /// Maximum journal file age before sealing.
    pub usage_journal_max_file_age_ms: i64,
    /// Maximum journal files retained on disk.
    pub usage_journal_max_files: i64,
    /// Target uncompressed bytes per journal block.
    pub usage_journal_block_target_uncompressed_bytes: i64,
    /// Maximum events per journal block.
    pub usage_journal_block_max_events: i64,
    /// Journal fsync interval in milliseconds.
    pub usage_journal_fsync_interval_ms: i64,
    /// Journal zstd compression level.
    pub usage_journal_zstd_level: i64,
    /// Worker lease age before claimed journals are recovered.
    pub usage_journal_consumer_lease_ms: i64,
    /// Whether corrupt journals are deleted rather than quarantined.
    pub usage_journal_delete_bad_files: bool,
    /// Worker query HTTP bind address.
    pub usage_query_bind_addr: String,
    /// Worker query base URL used by API-side compatibility routes.
    pub usage_query_base_url: String,
    /// Whether usage maintenance is enabled.
    pub usage_event_maintenance_enabled: bool,
    /// Usage maintenance interval.
    pub usage_event_maintenance_interval_seconds: i64,
    /// Heavy usage detail retention in days.
    pub usage_event_detail_retention_days: i64,
    /// Kiro cache k-models JSON.
    pub kiro_cache_kmodels_json: String,
    /// Kiro billable model multipliers JSON.
    pub kiro_billable_model_multipliers_json: String,
    /// Kiro cache policy JSON.
    pub kiro_cache_policy_json: String,
    /// Minimum request-side input tokens before trusting Kiro contextUsage.
    #[serde(default = "default_kiro_context_usage_min_request_tokens_i64")]
    pub kiro_context_usage_min_request_tokens: i64,
    /// Kiro prefix cache mode.
    pub kiro_prefix_cache_mode: String,
    /// Kiro prefix cache max tokens.
    pub kiro_prefix_cache_max_tokens: i64,
    /// Kiro prefix cache entry TTL.
    pub kiro_prefix_cache_entry_ttl_seconds: i64,
    /// Kiro conversation anchor max entries.
    pub kiro_conversation_anchor_max_entries: i64,
    /// Kiro conversation anchor TTL.
    pub kiro_conversation_anchor_ttl_seconds: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

/// Codex account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Account email when known.
    pub email: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Persisted settings JSON.
    pub settings_json: String,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Kiro account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id from usage limits when known.
    pub user_id: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Per-account concurrency cap.
    pub max_concurrency: Option<i64>,
    /// Per-account pacing interval.
    pub min_start_interval_ms: Option<i64>,
    /// Optional proxy config id.
    pub proxy_config_id: Option<String>,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn default_kiro_context_usage_min_request_tokens_i64() -> i64 {
    core_store::DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS as i64
}

impl Default for RuntimeConfigRecord {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            auth_cache_ttl_seconds: core_store::DEFAULT_AUTH_CACHE_TTL_SECONDS as i64,
            max_request_body_bytes: core_store::DEFAULT_MAX_REQUEST_BODY_BYTES as i64,
            account_failure_retry_limit: core_store::DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT as i64,
            codex_client_version: core_store::DEFAULT_CODEX_CLIENT_VERSION.to_string(),
            kiro_channel_max_concurrency: core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY as i64,
            kiro_channel_min_start_interval_ms:
                core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS as i64,
            codex_status_refresh_min_interval_seconds:
                core_store::DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS as i64,
            codex_status_refresh_max_interval_seconds:
                core_store::DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS as i64,
            codex_status_account_jitter_max_seconds:
                core_store::DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS as i64,
            codex_weight_free: core_store::DEFAULT_CODEX_WEIGHT_FREE as i64,
            codex_weight_plus: core_store::DEFAULT_CODEX_WEIGHT_PLUS as i64,
            codex_weight_pro5x: core_store::DEFAULT_CODEX_WEIGHT_PRO5X as i64,
            codex_weight_pro20x: core_store::DEFAULT_CODEX_WEIGHT_PRO20X as i64,
            kiro_status_refresh_min_interval_seconds:
                core_store::DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS as i64,
            kiro_status_refresh_max_interval_seconds:
                core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS as i64,
            kiro_status_account_jitter_max_seconds:
                core_store::DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS as i64,
            usage_event_flush_batch_size: core_store::DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE as i64,
            usage_event_flush_interval_seconds:
                core_store::DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS as i64,
            usage_event_flush_max_buffer_bytes:
                core_store::DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES as i64,
            duckdb_usage_memory_limit_mib: core_store::DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB as i64,
            duckdb_usage_checkpoint_threshold_mib:
                core_store::DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB as i64,
            usage_analytics_retention_days: core_store::DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS
                as i64,
            usage_journal_enabled: core_store::DEFAULT_USAGE_JOURNAL_ENABLED,
            usage_journal_max_file_bytes: core_store::DEFAULT_USAGE_JOURNAL_MAX_FILE_BYTES as i64,
            usage_journal_max_file_age_ms: core_store::DEFAULT_USAGE_JOURNAL_MAX_FILE_AGE_MS as i64,
            usage_journal_max_files: core_store::DEFAULT_USAGE_JOURNAL_MAX_FILES as i64,
            usage_journal_block_target_uncompressed_bytes:
                core_store::DEFAULT_USAGE_JOURNAL_BLOCK_TARGET_UNCOMPRESSED_BYTES as i64,
            usage_journal_block_max_events: core_store::DEFAULT_USAGE_JOURNAL_BLOCK_MAX_EVENTS
                as i64,
            usage_journal_fsync_interval_ms: core_store::DEFAULT_USAGE_JOURNAL_FSYNC_INTERVAL_MS
                as i64,
            usage_journal_zstd_level: core_store::DEFAULT_USAGE_JOURNAL_ZSTD_LEVEL,
            usage_journal_consumer_lease_ms: core_store::DEFAULT_USAGE_JOURNAL_CONSUMER_LEASE_MS
                as i64,
            usage_journal_delete_bad_files: core_store::DEFAULT_USAGE_JOURNAL_DELETE_BAD_FILES,
            usage_query_bind_addr: core_store::DEFAULT_USAGE_QUERY_BIND_ADDR.to_string(),
            usage_query_base_url: core_store::DEFAULT_USAGE_QUERY_BASE_URL.to_string(),
            usage_event_maintenance_enabled: core_store::DEFAULT_USAGE_EVENT_MAINTENANCE_ENABLED,
            usage_event_maintenance_interval_seconds:
                core_store::DEFAULT_USAGE_EVENT_MAINTENANCE_INTERVAL_SECONDS as i64,
            usage_event_detail_retention_days:
                core_store::DEFAULT_USAGE_EVENT_DETAIL_RETENTION_DAYS,
            kiro_cache_kmodels_json: core_store::default_kiro_cache_kmodels_json(),
            kiro_billable_model_multipliers_json:
                core_store::default_kiro_billable_model_multipliers_json(),
            kiro_cache_policy_json: core_store::default_kiro_cache_policy_json(),
            kiro_context_usage_min_request_tokens:
                default_kiro_context_usage_min_request_tokens_i64(),
            kiro_prefix_cache_mode: core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE.to_string(),
            kiro_prefix_cache_max_tokens: core_store::DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS as i64,
            kiro_prefix_cache_entry_ttl_seconds:
                core_store::DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS as i64,
            kiro_conversation_anchor_max_entries:
                core_store::DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES as i64,
            kiro_conversation_anchor_ttl_seconds:
                core_store::DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS as i64,
            updated_at_ms: now_ms(),
        }
    }
}

impl RuntimeConfigRecord {
    /// Convert the storage row into the admin response view.
    pub fn to_admin_runtime_config(&self) -> AdminRuntimeConfig {
        AdminRuntimeConfig {
            auth_cache_ttl_seconds: self.auth_cache_ttl_seconds as u64,
            max_request_body_bytes: self.max_request_body_bytes as u64,
            account_failure_retry_limit: self.account_failure_retry_limit as u64,
            codex_client_version: self.codex_client_version.clone(),
            codex_status_refresh_min_interval_seconds: self
                .codex_status_refresh_min_interval_seconds
                as u64,
            codex_status_refresh_max_interval_seconds: self
                .codex_status_refresh_max_interval_seconds
                as u64,
            codex_status_account_jitter_max_seconds: self.codex_status_account_jitter_max_seconds
                as u64,
            codex_weight_free: self.codex_weight_free as u64,
            codex_weight_plus: self.codex_weight_plus as u64,
            codex_weight_pro5x: self.codex_weight_pro5x as u64,
            codex_weight_pro20x: self.codex_weight_pro20x as u64,
            kiro_status_refresh_min_interval_seconds: self.kiro_status_refresh_min_interval_seconds
                as u64,
            kiro_status_refresh_max_interval_seconds: self.kiro_status_refresh_max_interval_seconds
                as u64,
            kiro_status_account_jitter_max_seconds: self.kiro_status_account_jitter_max_seconds
                as u64,
            usage_event_flush_batch_size: self.usage_event_flush_batch_size as u64,
            usage_event_flush_interval_seconds: self.usage_event_flush_interval_seconds as u64,
            usage_event_flush_max_buffer_bytes: self.usage_event_flush_max_buffer_bytes as u64,
            duckdb_usage_memory_limit_mib: self.duckdb_usage_memory_limit_mib as u64,
            duckdb_usage_checkpoint_threshold_mib: self.duckdb_usage_checkpoint_threshold_mib
                as u64,
            usage_analytics_retention_days: self.usage_analytics_retention_days as u64,
            usage_journal_enabled: self.usage_journal_enabled,
            usage_journal_max_file_bytes: self.usage_journal_max_file_bytes as u64,
            usage_journal_max_file_age_ms: self.usage_journal_max_file_age_ms as u64,
            usage_journal_max_files: self.usage_journal_max_files as u64,
            usage_journal_block_target_uncompressed_bytes: self
                .usage_journal_block_target_uncompressed_bytes
                as u64,
            usage_journal_block_max_events: self.usage_journal_block_max_events as u64,
            usage_journal_fsync_interval_ms: self.usage_journal_fsync_interval_ms as u64,
            usage_journal_zstd_level: self.usage_journal_zstd_level,
            usage_journal_consumer_lease_ms: self.usage_journal_consumer_lease_ms as u64,
            usage_journal_delete_bad_files: self.usage_journal_delete_bad_files,
            usage_query_bind_addr: self.usage_query_bind_addr.clone(),
            usage_query_base_url: self.usage_query_base_url.clone(),
            kiro_cache_kmodels_json: self.kiro_cache_kmodels_json.clone(),
            kiro_billable_model_multipliers_json: self.kiro_billable_model_multipliers_json.clone(),
            kiro_cache_policy_json: self.kiro_cache_policy_json.clone(),
            kiro_context_usage_min_request_tokens: self.kiro_context_usage_min_request_tokens
                as u64,
            kiro_prefix_cache_mode: self.kiro_prefix_cache_mode.clone(),
            kiro_prefix_cache_max_tokens: self.kiro_prefix_cache_max_tokens as u64,
            kiro_prefix_cache_entry_ttl_seconds: self.kiro_prefix_cache_entry_ttl_seconds as u64,
            kiro_conversation_anchor_max_entries: self.kiro_conversation_anchor_max_entries as u64,
            kiro_conversation_anchor_ttl_seconds: self.kiro_conversation_anchor_ttl_seconds as u64,
        }
    }

    /// Apply the admin-visible config fields and preserve internal-only fields.
    pub fn apply_admin_runtime_config(&mut self, config: &AdminRuntimeConfig) {
        self.id = "default".to_string();
        self.auth_cache_ttl_seconds = config.auth_cache_ttl_seconds as i64;
        self.max_request_body_bytes = config.max_request_body_bytes as i64;
        self.account_failure_retry_limit = config.account_failure_retry_limit as i64;
        self.codex_client_version = config.codex_client_version.clone();
        self.codex_status_refresh_min_interval_seconds =
            config.codex_status_refresh_min_interval_seconds as i64;
        self.codex_status_refresh_max_interval_seconds =
            config.codex_status_refresh_max_interval_seconds as i64;
        self.codex_status_account_jitter_max_seconds =
            config.codex_status_account_jitter_max_seconds as i64;
        self.codex_weight_free = config.codex_weight_free as i64;
        self.codex_weight_plus = config.codex_weight_plus as i64;
        self.codex_weight_pro5x = config.codex_weight_pro5x as i64;
        self.codex_weight_pro20x = config.codex_weight_pro20x as i64;
        self.kiro_status_refresh_min_interval_seconds =
            config.kiro_status_refresh_min_interval_seconds as i64;
        self.kiro_status_refresh_max_interval_seconds =
            config.kiro_status_refresh_max_interval_seconds as i64;
        self.kiro_status_account_jitter_max_seconds =
            config.kiro_status_account_jitter_max_seconds as i64;
        self.usage_event_flush_batch_size = config.usage_event_flush_batch_size as i64;
        self.usage_event_flush_interval_seconds = config.usage_event_flush_interval_seconds as i64;
        self.usage_event_flush_max_buffer_bytes = config.usage_event_flush_max_buffer_bytes as i64;
        self.duckdb_usage_memory_limit_mib = config.duckdb_usage_memory_limit_mib as i64;
        self.duckdb_usage_checkpoint_threshold_mib =
            config.duckdb_usage_checkpoint_threshold_mib as i64;
        self.usage_analytics_retention_days = config.usage_analytics_retention_days as i64;
        self.usage_journal_enabled = config.usage_journal_enabled;
        self.usage_journal_max_file_bytes = config.usage_journal_max_file_bytes as i64;
        self.usage_journal_max_file_age_ms = config.usage_journal_max_file_age_ms as i64;
        self.usage_journal_max_files = config.usage_journal_max_files as i64;
        self.usage_journal_block_target_uncompressed_bytes =
            config.usage_journal_block_target_uncompressed_bytes as i64;
        self.usage_journal_block_max_events = config.usage_journal_block_max_events as i64;
        self.usage_journal_fsync_interval_ms = config.usage_journal_fsync_interval_ms as i64;
        self.usage_journal_zstd_level = config.usage_journal_zstd_level;
        self.usage_journal_consumer_lease_ms = config.usage_journal_consumer_lease_ms as i64;
        self.usage_journal_delete_bad_files = config.usage_journal_delete_bad_files;
        self.usage_query_bind_addr = config.usage_query_bind_addr.clone();
        self.usage_query_base_url = config.usage_query_base_url.clone();
        self.kiro_cache_kmodels_json = config.kiro_cache_kmodels_json.clone();
        self.kiro_billable_model_multipliers_json =
            config.kiro_billable_model_multipliers_json.clone();
        self.kiro_cache_policy_json = config.kiro_cache_policy_json.clone();
        self.kiro_context_usage_min_request_tokens =
            config.kiro_context_usage_min_request_tokens as i64;
        self.kiro_prefix_cache_mode = config.kiro_prefix_cache_mode.clone();
        self.kiro_prefix_cache_max_tokens = config.kiro_prefix_cache_max_tokens as i64;
        self.kiro_prefix_cache_entry_ttl_seconds =
            config.kiro_prefix_cache_entry_ttl_seconds as i64;
        self.kiro_conversation_anchor_max_entries =
            config.kiro_conversation_anchor_max_entries as i64;
        self.kiro_conversation_anchor_ttl_seconds =
            config.kiro_conversation_anchor_ttl_seconds as i64;
        self.updated_at_ms = now_ms();
    }
}
