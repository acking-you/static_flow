//! Local admin endpoints for the standalone LLM access service.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    num::NonZeroUsize,
    path::{Path as FsPath, PathBuf},
    time::{Duration, Instant},
};

use anyhow::Context;
use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, Path, Query, State},
    http::{header, HeaderMap, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        self as core_store, AdminAccountContributionRequest, AdminAccountGroupPatch,
        AdminCodexAccountPatch, AdminCodexImportJobItemResult, AdminKeyPatch, AdminPageRequest,
        AdminProxyConfigPatch, AdminReviewQueueAction, AdminRuntimeConfig, NewAdminAccountGroup,
        NewAdminCodexAccount, NewAdminCodexImportJob, NewAdminCodexImportJobItem, NewAdminKey,
        NewAdminKiroAccount, NewAdminProxyConfig, UpdateAdminRuntimeConfig, KEY_STATUS_ACTIVE,
        KEY_STATUS_DISABLED, KIRO_PREFIX_CACHE_MODE_FORMULA, PROTOCOL_ANTHROPIC, PROTOCOL_OPENAI,
        PROVIDER_CODEX, PROVIDER_KIRO,
    },
};
use llm_access_kiro::{
    auth_file::KiroAuthRecord,
    cache_policy::{
        parse_kiro_cache_policy_override_json, resolve_effective_kiro_cache_policy,
        uses_global_kiro_cache_policy, KiroCachePolicy,
    },
    cache_sim::{KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode},
    local_import,
};
use llm_usage_journal::{
    collect_journal_file_lists, JournalFileListsSnapshot, JournalFileSnapshot,
    JournalPreviewReader, JournalPreviewReport, JournalStatusSnapshot, WorkerProgressSnapshot,
};
use lru::LruCache;
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::OwnedSemaphorePermit;

use crate::{
    activity::RequestActivitySnapshot,
    codex_refresh, codex_status, kiro_refresh, kiro_status,
    process_memory::{read_current_process_memory_stats, ProcessMemoryStats},
    provider, HttpState,
};

const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const CODEX_IMPORT_PRINCIPAL_CACHE_MAX_ENTRIES: usize = 4_096;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
const MAX_RUNTIME_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MIN_RUNTIME_REQUEST_BODY_BYTES: u64 = 1024;
const MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 100;
const MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 0;
const MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 240;
const MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 3_600;
const MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS: u64 = 60;
const MAX_RUNTIME_CODEX_AFFINITY_MAX_ENTRIES: u64 = 1_000_000;
const MAX_RUNTIME_CODEX_AFFINITY_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_RUNTIME_CODEX_FALLBACK_PREFIX_BYTES: u64 = 1024 * 1024;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 16_384;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 3_600;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 1_024;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
const MIN_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB: u64 = 512;
const MAX_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB: u64 = 2_048;
const MIN_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB: u64 = 16;
const MAX_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB: u64 = 256;
const MIN_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS: u64 = 1;
const MAX_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS: u64 = 365;
const MIN_RUNTIME_USAGE_JOURNAL_FILE_BYTES: u64 = 1_024;
const MAX_RUNTIME_USAGE_JOURNAL_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS: u64 = 1_000;
const MAX_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_RUNTIME_USAGE_JOURNAL_FILES: u64 = 10_000;
const MIN_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES: u64 = 1_024;
const MAX_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RUNTIME_USAGE_JOURNAL_BLOCK_EVENTS: u64 = 16_384;
const MAX_RUNTIME_USAGE_JOURNAL_FSYNC_INTERVAL_MS: u64 = 60_000;
const MAX_RUNTIME_USAGE_JOURNAL_ZSTD_LEVEL: i64 = 22;
const MIN_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS: u64 = 1_000;
const MAX_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS: u64 = 60 * 60 * 1000;
const MAX_RUNTIME_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS: u64 = 1_000_000;
const MAX_RUNTIME_KIRO_COMPACT_TRIGGER_TOKENS: u64 = 1_000_000;
const MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_INTERVAL_SECONDS: u64 = 24 * 60 * 60;
const MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
// Caps are persisted as Postgres BIGINT (`i64`); bound them to `i64::MAX` so an
// out-of-range value is rejected here instead of silently wrapping negative on
// the `as i64` store conversion and tripping the column CHECK as a 500.
const MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_CAP: u64 = i64::MAX as u64;
const MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY: u64 = 1_024;
const MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS: u64 = 300_000;
const DEFAULT_ADMIN_REVIEW_QUEUE_LIMIT: usize = 50;
const MAX_ADMIN_REVIEW_QUEUE_LIMIT: usize = 200;
const DEFAULT_ADMIN_LIST_LIMIT: usize = 50;
const MAX_ADMIN_LIST_LIMIT: usize = 200;
const DEFAULT_ADMIN_IMPORT_JOB_LIMIT: usize = 20;
const MAX_ADMIN_IMPORT_JOB_LIMIT: usize = 50;
const PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS: u64 = 10;
const PROXY_FULL_CHAIN_CHECK_TIMEOUT_SECONDS: u64 = 120;
const PROXY_FULL_CHAIN_CHECK_MAX_BODY_BYTES: usize = 1024 * 1024;
const PROXY_FULL_CHAIN_CODEX_KEY_NAME: &str = "admin-key";
const PROXY_FULL_CHAIN_KIRO_KEY_NAME: &str = "admin";
const PROXY_FULL_CHAIN_CODEX_MODEL: &str = "gpt-5.5";
const PROXY_FULL_CHAIN_KIRO_MODEL: &str = "claude-sonnet-4-6";
const ADMIN_KIRO_MODEL_PROBE_PROMPT: &str = "Reply with OK only.";
const CODEX_ACCESS_TOKEN_VALIDATION_TIMEOUT_SECONDS: u64 = 20;
const CODEX_WIRE_ORIGINATOR: &str = "codex_cli_rs";
const BAND_CONTIGUITY_TOLERANCE: f64 = 1e-12;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

#[derive(Debug, Serialize)]
struct AdminKeysResponse {
    keys: Vec<core_store::AdminKey>,
    summary: core_store::AdminKeysSummary,
    auth_cache_ttl_seconds: u64,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct DeleteResponse {
    deleted: bool,
    id: String,
}

#[derive(Debug, Serialize)]
struct AdminAccountGroupsResponse {
    groups: Vec<core_store::AdminAccountGroup>,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminAccountGroupOptionsResponse {
    options: Vec<core_store::AdminAccountGroupOption>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminProxyConfigsResponse {
    proxy_config_scope: AdminProxyConfigScopeView,
    proxy_configs: Vec<core_store::AdminProxyConfig>,
    generated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
struct AdminProxyConfigScopeView {
    node_id: String,
    is_core: bool,
    can_edit_slot_metadata: bool,
}

#[derive(Debug, Serialize)]
struct AdminProxyBindingsResponse {
    bindings: Vec<core_store::AdminProxyBinding>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminAccountsResponse {
    accounts: Vec<core_store::AdminCodexAccount>,
    summary: core_store::AdminAccountsSummary,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminCodexModelsProbeResponse {
    ok: bool,
    message: String,
    checked_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminCodexImportJobsResponse {
    jobs: Vec<core_store::AdminCodexImportJobSummary>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminKiroAccountsResponse {
    accounts: Vec<core_store::AdminKiroAccount>,
    summary: core_store::AdminAccountsSummary,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminKiroAccountStatusesResponse {
    accounts: Vec<core_store::AdminKiroAccount>,
    total: usize,
    limit: usize,
    offset: usize,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminKiroCacheStatsResponse {
    #[serde(flatten)]
    stats: KiroCacheRuntimeStats,
    process_memory: ProcessMemoryStats,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminKiroModelProbeResponse {
    ok: bool,
    account_name: String,
    model: String,
    api_region: String,
    proxy_source: String,
    proxy_url: Option<String>,
    upstream_status_code: u16,
    latency_ms: i64,
    checked_at: i64,
    message: String,
}

#[derive(Debug, Serialize)]
struct AdminTokenRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminTokenRequest>,
    generated_at: i64,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AdminListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AdminKeyListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    q: Option<String>,
    active_only: Option<bool>,
    sort: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub(crate) struct AdminCodexAccountListQuery {
    limit: Option<usize>,
    offset: Option<usize>,
    q: Option<String>,
    active_only: Option<bool>,
    unhealthy_only: Option<bool>,
    sort: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminAccountContributionRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminAccountContributionRequest>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminSponsorRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminSponsorRequest>,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminUsageJournalStatusResponse {
    cluster: Option<AdminClusterNodeStatusView>,
    journal_enabled: bool,
    journal_root: String,
    current_rpm: u32,
    current_in_flight: u32,
    active_file_sequence: Option<u64>,
    active_file_bytes: u64,
    sealed_file_count: u64,
    sealed_bytes: u64,
    oldest_sealed_age_ms: Option<i64>,
    dropped_files_total: u64,
    dropped_unconsumed_files_total: u64,
    write_failures_total: u64,
    usage_query_base_url: String,
    producer_current_file: Option<AdminUsageJournalFileView>,
    orphan_active_files: Vec<AdminUsageJournalFileView>,
    current_consuming_file: Option<AdminUsageJournalFileView>,
    orphan_consuming_files: Vec<AdminUsageJournalFileView>,
    active_files: Vec<AdminUsageJournalFileView>,
    sealed_files: Vec<AdminUsageJournalFileView>,
    consuming_files: Vec<AdminUsageJournalFileView>,
    bad_files: Vec<AdminUsageJournalFileView>,
    worker: AdminUsageWorkerProgressView,
    generated_at: i64,
}

#[derive(Debug, Clone, Serialize)]
struct AdminClusterNodeStatusView {
    node_id: String,
    node_class: crate::cluster::NodeClass,
    runtime_role: crate::cluster::NodeRuntimeRole,
    primary_node_id: Option<String>,
    usage_query_mode: crate::cluster::UsageQueryMode,
    primary_worker_base_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct AdminUsageJournalPreviewQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

#[derive(Debug, Serialize)]
struct AdminUsageJournalPreviewResponse {
    journal_root: String,
    producer_current_file: Option<AdminUsageJournalFileView>,
    preview: Option<AdminUsageJournalPreviewFileView>,
    limit: usize,
    offset: usize,
    total: usize,
    has_more: bool,
    generated_at: i64,
}

#[derive(Debug, Serialize)]
struct AdminUsageJournalPreviewFileView {
    path: String,
    file_sequence: u64,
    bytes_scanned: u64,
    complete_blocks: u64,
    truncated_tail: bool,
    total_events: usize,
    events: Vec<AdminUsageJournalPreviewEventView>,
}

#[derive(Debug, Serialize)]
struct AdminUsageJournalPreviewEventView {
    event_id: String,
    created_at_ms: i64,
    provider_type: ProviderType,
    protocol_family: ProtocolFamily,
    key_id: String,
    key_name: String,
    account_name: Option<String>,
    request_method: String,
    endpoint: String,
    model: Option<String>,
    mapped_model: Option<String>,
    status_code: i64,
    input_uncached_tokens: i64,
    input_cached_tokens: i64,
    output_tokens: i64,
    billable_tokens: i64,
    usage_missing: bool,
    credit_usage_missing: bool,
    last_message_content: Option<String>,
    final_event_type: Option<String>,
    stream_completed_cleanly: Option<bool>,
    downstream_disconnect: Option<bool>,
    bytes_streamed: Option<i64>,
    latency_ms: Option<i64>,
    first_sse_write_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct AdminUsageJournalFileView {
    file_name: String,
    path: String,
    sequence: Option<u64>,
    bytes: u64,
    age_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize)]
struct AdminUsageWorkerProgressView {
    state: String,
    current_file_path: Option<String>,
    current_file_sequence: Option<u64>,
    processed_blocks: u64,
    total_blocks: u64,
    processed_events: u64,
    total_events: u64,
    processed_compressed_bytes: u64,
    total_compressed_bytes: u64,
    progress_percent: f64,
    import_rate_events_per_second: f64,
    heartbeat_age_ms: Option<i64>,
    last_successful_file_sequence: Option<u64>,
    last_successful_import_at_ms: Option<i64>,
    last_error: Option<String>,
    last_error_at_ms: Option<i64>,
    process_memory: ProcessMemoryStats,
}

#[derive(Debug, Default)]
struct PartitionedUsageJournalFiles {
    producer_current_file: Option<AdminUsageJournalFileView>,
    orphan_active_files: Vec<AdminUsageJournalFileView>,
    current_consuming_file: Option<AdminUsageJournalFileView>,
    orphan_consuming_files: Vec<AdminUsageJournalFileView>,
}

#[derive(Debug, Serialize)]
struct AdminProxyCheckTargetView {
    target: String,
    url: String,
    reachable: bool,
    status_code: Option<u16>,
    latency_ms: i64,
    error_message: Option<String>,
}

#[derive(Debug, Serialize)]
struct AdminProxyCheckResponse {
    proxy_config_id: String,
    proxy_config_name: String,
    provider_type: String,
    auth_label: String,
    ok: bool,
    targets: Vec<AdminProxyCheckTargetView>,
    checked_at: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub(crate) struct CheckLlmGatewayProxyConfigRequest {
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminProxyCheckMode {
    Connectivity,
    FullChain,
}

#[derive(Debug, Serialize)]
struct AdminLegacyKiroProxyMigrationResponse {
    created_configs: Vec<core_store::AdminProxyConfig>,
    reused_configs: Vec<core_store::AdminProxyConfig>,
    migrated_account_names: Vec<String>,
    generated_at: i64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListKiroAccountStatusesRequest {
    #[serde(default)]
    prefix: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ListReviewQueueRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    offset: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ReviewQueueActionRequest {
    #[serde(default)]
    admin_note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayKeyRequest {
    name: String,
    quota_billable_limit: u64,
    #[serde(default)]
    public_visible: bool,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayKeyRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    public_visible: Option<bool>,
    #[serde(default)]
    quota_billable_limit: Option<u64>,
    #[serde(default)]
    route_strategy: Option<String>,
    #[serde(default)]
    account_group_id: Option<String>,
    #[serde(default)]
    fixed_account_name: Option<String>,
    #[serde(default)]
    auto_account_names: Option<Vec<String>>,
    #[serde(default)]
    preferred_pool_strategy: Option<String>,
    #[serde(default)]
    model_name_map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    request_max_concurrency_unlimited: bool,
    #[serde(default)]
    request_min_start_interval_ms_unlimited: bool,
    #[serde(default)]
    codex_fast_enabled: Option<bool>,
    #[serde(default)]
    kiro_request_validation_enabled: Option<bool>,
    #[serde(default)]
    kiro_cache_estimation_enabled: Option<bool>,
    #[serde(default)]
    kiro_zero_cache_debug_enabled: Option<bool>,
    #[serde(default)]
    kiro_full_request_logging_enabled: Option<bool>,
    #[serde(default)]
    kiro_remote_media_resolution_enabled: Option<bool>,
    #[serde(default)]
    kiro_latency_routing_enabled: Option<bool>,
    #[serde(default)]
    kiro_protected_content_validation_enabled: Option<bool>,
    #[serde(default)]
    kiro_cctest_text_handling_enabled: Option<bool>,
    #[serde(default)]
    kiro_cache_policy_override_json: Option<Option<String>>,
    #[serde(default)]
    kiro_billable_model_multipliers_override_json: Option<Option<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayAccountGroupRequest {
    name: String,
    account_names: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayAccountGroupRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    account_names: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateLlmGatewayProxyConfigRequest {
    name: String,
    proxy_url: String,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayProxyConfigRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct UpdateLlmGatewayProxyBindingRequest {
    #[serde(default)]
    proxy_config_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportLlmGatewayAccountRequest {
    name: String,
    #[serde(default)]
    tokens: Option<ImportLlmGatewayAccountTokens>,
    #[serde(default)]
    auth_json: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportLlmGatewayAccountTokens {
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateCodexBatchImportJobRequest {
    provider_type: String,
    source_type: String,
    #[serde(default)]
    validate_before_import: bool,
    items: Vec<CreateCodexBatchImportJobItemRequest>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateCodexBatchImportJobItemRequest {
    name: String,
    #[serde(default)]
    tokens: Option<ImportLlmGatewayAccountTokens>,
    #[serde(default)]
    auth_json: Option<serde_json::Value>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ListCodexImportJobsRequest {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchLlmGatewayAccountRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    route_weight_tier: Option<String>,
    #[serde(default)]
    proxy_mode: Option<String>,
    #[serde(default)]
    proxy_config_id: Option<String>,
    #[serde(default)]
    map_gpt53_codex_to_spark: Option<bool>,
    #[serde(default)]
    auto_refresh_enabled: Option<bool>,
    #[serde(default)]
    request_max_concurrency: Option<u64>,
    #[serde(default)]
    request_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    request_max_concurrency_unlimited: bool,
    #[serde(default)]
    request_min_start_interval_ms_unlimited: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportLocalKiroAccountRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sqlite_path: Option<String>,
    #[serde(default)]
    kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    kiro_channel_min_start_interval_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct KiroAuthRequestFields {
    name: String,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    profile_arn: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    auth_method: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default)]
    auth_region: Option<String>,
    #[serde(default)]
    api_region: Option<String>,
    #[serde(default)]
    machine_id: Option<String>,
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    subscription_title: Option<String>,
    #[serde(default)]
    kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    kiro_channel_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    minimum_remaining_credits_before_block: Option<f64>,
    #[serde(default)]
    manual_usage_limit: Option<Option<f64>>,
    #[serde(default)]
    pool_strategy: Option<String>,
    #[serde(default)]
    disabled: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CreateManualKiroAccountRequest {
    #[serde(flatten)]
    auth: KiroAuthRequestFields,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ImportParsedKiroAccountRequest {
    #[serde(flatten)]
    auth: KiroAuthRequestFields,
    #[serde(default)]
    source_db_path: Option<String>,
    #[serde(default)]
    last_imported_at: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct PatchKiroAccountRequest {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    kiro_channel_max_concurrency: Option<u64>,
    #[serde(default)]
    kiro_channel_min_start_interval_ms: Option<u64>,
    #[serde(default)]
    minimum_remaining_credits_before_block: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_present_optional_f64")]
    manual_usage_limit: Option<Option<f64>>,
    #[serde(default)]
    pool_strategy: Option<String>,
    #[serde(default)]
    proxy_mode: Option<String>,
    #[serde(default)]
    proxy_config_id: Option<String>,
}

fn deserialize_present_optional_f64<'de, D>(
    deserializer: D,
) -> Result<Option<Option<f64>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<f64>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
pub(crate) struct ProbeKiroAccountModelRequest {
    model: String,
    #[serde(default)]
    proxy_config_id: Option<String>,
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NormalizedProbeKiroAccountModelRequest {
    model: String,
    proxy_config_id: Option<String>,
    inline_proxy: Option<core_store::ProviderProxyConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AdminKiroProbeProxySource {
    Inline,
    ProxyConfig,
    Resolved,
    None,
}

impl AdminKiroProbeProxySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::ProxyConfig => "proxy_config",
            Self::Resolved => "resolved",
            Self::None => "none",
        }
    }
}

#[derive(Debug)]
struct AdminHttpError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for AdminHttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.status.as_u16(),
            }),
        )
            .into_response()
    }
}

pub(crate) async fn get_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to load llm gateway config").into_response(),
    }
}

pub(crate) async fn post_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminRuntimeConfig>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let config = match apply_runtime_config_update(current, request) {
        Ok(config) => config,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_config_store
        .update_admin_runtime_config(config)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to update llm gateway config").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_keys(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminKeyListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(AdminListQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let filter = admin_key_page_query(&query);
    let page = match state
        .admin_key_store
        .list_admin_keys_filtered_page(None, &filter, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list llm gateway keys").into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let keys = match apply_effective_kiro_cache_policies(page.keys, &config) {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to resolve Kiro cache policy").into_response(),
    };
    Json(AdminKeysResponse {
        keys,
        summary: page.summary,
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn create_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    if let Err(response) =
        validate_i64_backed_u64("quota_billable_limit", request.quota_billable_limit)
    {
        return response.into_response();
    }
    if let Err(response) = validate_codex_request_limit_inputs(
        request.request_max_concurrency,
        request.request_min_start_interval_ms,
    ) {
        return response.into_response();
    }
    let secret = generate_secret();
    let key = NewAdminKey {
        id: generate_id("llm-key"),
        name,
        key_hash: sha256_hex(secret.as_bytes()),
        secret,
        provider_type: PROVIDER_CODEX.to_string(),
        protocol_family: PROTOCOL_OPENAI.to_string(),
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        request_max_concurrency: request.request_max_concurrency,
        request_min_start_interval_ms: request.request_min_start_interval_ms,
        created_at_ms: now_ms(),
    };
    match state.admin_key_store.create_admin_key(key).await {
        Ok(key) => Json(key).into_response(),
        Err(_) => internal_error("Failed to create llm gateway key").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match admin_key_provider(&state, &key_id).await {
        Ok(Some(provider_type)) if provider_type == PROVIDER_CODEX => {},
        Ok(Some(_)) => {
            return bad_request("Kiro keys must be managed from /admin/kiro-gateway")
                .into_response();
        },
        Ok(None) => return not_found("LLM gateway key not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway key").into_response(),
    }
    let patch = match normalize_key_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state.admin_key_store.patch_admin_key(&key_id, patch).await {
        Ok(Some(key)) => match resolve_key_effective_kiro_cache_policy(&state, key).await {
            Ok(key) => Json(key).into_response(),
            Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
        },
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway key").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_key_store.delete_admin_key(&key_id).await {
        Ok(Some(key)) => Json(DeleteResponse {
            deleted: true,
            id: key.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway key").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_account_groups(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page = admin_page_request(query);
    match state
        .admin_account_group_store
        .list_admin_account_groups_page(PROVIDER_CODEX, page)
        .await
    {
        Ok(groups) => Json(AdminAccountGroupsResponse {
            groups: groups.groups,
            total: groups.total,
            limit: groups.limit,
            offset: groups.offset,
            has_more: groups.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account groups").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_account_group_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    list_account_group_options_for_provider(state, headers, PROVIDER_CODEX, "llm gateway").await
}

pub(crate) async fn create_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match normalize_account_names(request.account_names) {
        Ok(Some(names)) => names,
        Ok(None) => return bad_request("account_names must not be empty").into_response(),
        Err(response) => return response.into_response(),
    };
    let group = NewAdminAccountGroup {
        id: generate_id("llm-group"),
        provider_type: PROVIDER_CODEX.to_string(),
        name,
        account_names,
        created_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .create_admin_account_group(group)
        .await
    {
        Ok(group) => Json(group).into_response(),
        Err(_) => internal_error("Failed to create llm gateway account group").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match request.name.as_deref().map(normalize_name).transpose() {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match request
        .account_names
        .map(normalize_account_names)
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(response) => return response.into_response(),
    };
    let patch = AdminAccountGroupPatch {
        name,
        account_names,
        updated_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .patch_admin_account_group(&group_id, patch)
        .await
    {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account group").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let key = match state
        .admin_key_store
        .find_admin_key_referencing_account_group(PROVIDER_CODEX, &group_id)
        .await
    {
        Ok(key) => key,
        Err(_) => return internal_error("Failed to inspect llm gateway keys").into_response(),
    };
    if let Some(key) = key {
        return bad_request(&format!("account group is still referenced by key `{}`", key.name))
            .into_response();
    }
    match state
        .admin_account_group_store
        .delete_admin_account_group(&group_id)
        .await
    {
        Ok(Some(group)) => Json(DeleteResponse {
            deleted: true,
            id: group.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account group").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_proxy_configs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let proxy_config_scope = admin_proxy_config_scope_view(&state).await;
    match state.admin_proxy_store.list_admin_proxy_configs().await {
        Ok(proxy_configs) => Json(AdminProxyConfigsResponse {
            proxy_config_scope,
            proxy_configs,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy configs").into_response(),
    }
}

pub(crate) async fn create_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_proxy_config_scope_view(&state)
        .await
        .can_edit_slot_metadata
    {
        return bad_request("proxy slots can only be created on the core node").into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let proxy_url = match normalize_required_proxy_url(&request.proxy_url) {
        Ok(proxy_url) => proxy_url,
        Err(response) => return response.into_response(),
    };
    let proxy = NewAdminProxyConfig {
        id: generate_id("llm-proxy"),
        name,
        proxy_url,
        proxy_username: normalize_optional_string_option(request.proxy_username.as_deref()),
        proxy_password: normalize_optional_string_option(request.proxy_password.as_deref()),
        created_at_ms: now_ms(),
    };
    match state
        .admin_proxy_store
        .create_admin_proxy_config(proxy)
        .await
    {
        Ok(proxy) => Json(proxy).into_response(),
        Err(_) => internal_error("Failed to create llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
    Json(request): Json<PatchLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if request.name.is_some()
        && !admin_proxy_config_scope_view(&state)
            .await
            .can_edit_slot_metadata
    {
        return bad_request("proxy slot names can only be changed on the core node")
            .into_response();
    }
    let patch = match normalize_proxy_config_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_proxy_store
        .patch_admin_proxy_config(&proxy_id, patch)
        .await
    {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_proxy_config_scope_view(&state)
        .await
        .can_edit_slot_metadata
    {
        return bad_request("proxy slots can only be deleted on the core node").into_response();
    }
    let bindings = match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => bindings,
        Err(_) => {
            return internal_error("Failed to inspect llm gateway proxy bindings").into_response()
        },
    };
    if let Some(binding) = bindings
        .iter()
        .find(|binding| binding.bound_proxy_config_id.as_deref() == Some(proxy_id.as_str()))
    {
        return conflict(&format!(
            "proxy config is still bound to provider `{}`",
            binding.provider_type
        ))
        .into_response();
    }
    match state
        .admin_proxy_store
        .delete_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => Json(DeleteResponse {
            deleted: true,
            id: proxy.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway proxy config").into_response(),
    }
}

pub(crate) async fn reset_llm_gateway_proxy_config_override(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_proxy_store
        .reset_admin_proxy_config_override(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => {
            internal_error("Failed to reset llm gateway proxy config override").into_response()
        },
    }
}

pub(crate) async fn list_llm_gateway_proxy_bindings(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => Json(AdminProxyBindingsResponse {
            bindings,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy bindings").into_response(),
    }
}

pub(crate) async fn update_llm_gateway_proxy_binding(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(provider_type): Path<String>,
    Json(request): Json<UpdateLlmGatewayProxyBindingRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let proxy_config_id = normalize_optional_string_option(request.proxy_config_id.as_deref());
    if let Some(proxy_id) = proxy_config_id.as_deref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before binding").into_response();
        }
    }
    match state
        .admin_proxy_store
        .update_admin_proxy_binding(&provider_type, proxy_config_id)
        .await
    {
        Ok(binding) => Json(binding).into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy binding").into_response(),
    }
}

pub(crate) async fn check_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((proxy_id, provider_type)): Path<(String, String)>,
    payload: Option<Json<CheckLlmGatewayProxyConfigRequest>>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let mode = match normalize_proxy_check_mode(payload.as_ref().map(|payload| &payload.0)) {
        Ok(mode) => mode,
        Err(response) => return response.into_response(),
    };
    let proxy = match state
        .admin_proxy_store
        .get_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => proxy,
        Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway proxy config").into_response(),
    };
    let check_result = match mode {
        AdminProxyCheckMode::Connectivity => run_proxy_connectivity_check(&proxy, &provider_type)
            .await
            .map_err(|_| internal_error("Failed to check upstream proxy config")),
        AdminProxyCheckMode::FullChain => {
            run_proxy_full_chain_check(&state, &proxy, &provider_type).await
        },
    };
    match check_result {
        Ok(result) => {
            if let Some(update) = proxy_endpoint_check_update_from_response(&result) {
                match state
                    .admin_proxy_store
                    .record_admin_proxy_endpoint_check(update)
                    .await
                {
                    Ok(Some(_)) => {},
                    Ok(None) => {
                        return not_found("LLM gateway proxy config not found").into_response();
                    },
                    Err(err) => {
                        tracing::warn!(
                            proxy_config_id = %result.proxy_config_id,
                            provider_type = %result.provider_type,
                            "failed to persist upstream proxy check: {err:#}"
                        );
                        return internal_error("Failed to save upstream proxy check")
                            .into_response();
                    },
                }
            }
            Json(result).into_response()
        },
        Err(response) => response.into_response(),
    }
}

pub(crate) async fn import_legacy_kiro_proxy_configs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_proxy_store
        .import_legacy_kiro_proxy_configs()
        .await
    {
        Ok(result) => Json(AdminLegacyKiroProxyMigrationResponse {
            created_configs: result.created_configs,
            reused_configs: result.reused_configs,
            migrated_account_names: result.migrated_account_names,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to import legacy Kiro proxy configs").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_usage_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_list_query(&state, &uri).await
}

pub(crate) async fn get_llm_gateway_usage_filter_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}

pub(crate) async fn get_llm_gateway_usage_metrics(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}

pub(crate) async fn get_llm_gateway_proxy_traffic(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}

pub(crate) async fn get_llm_gateway_usage_event(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(event_id): Path<String>,
) -> Response {
    let _ = event_id;
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}

pub(crate) async fn get_usage_journal_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let mut journal = match producer_journal_status(&state) {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!("failed to load usage journal producer status: {err:#}");
            return internal_error("Failed to load usage journal status").into_response();
        },
    };
    journal.journal_enabled = config.usage_journal_enabled;
    if journal.journal_root.is_empty() {
        journal.journal_root = state
            .usage_journal_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
    let activity = state.request_activity.snapshot(None);
    let now = now_ms();
    let files = match journal_file_lists(&state) {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!("failed to load usage journal file lists: {err:#}");
            return internal_error("Failed to load usage journal file lists").into_response();
        },
    };
    let worker = usage_worker_status(&config.usage_query_base_url, now).await;
    let partitioned = partition_usage_journal_files(&journal, &files, &worker);
    let cluster = match state.cluster_state.as_ref() {
        Some(cluster_state) => {
            let snapshot = cluster_state.snapshot().await;
            Some(AdminClusterNodeStatusView {
                node_id: snapshot.node.node_id,
                node_class: snapshot.node.node_class,
                runtime_role: snapshot.runtime_role,
                primary_node_id: snapshot
                    .primary
                    .as_ref()
                    .map(|primary| primary.node_id.clone()),
                usage_query_mode: snapshot.usage_query_mode,
                primary_worker_base_url: snapshot
                    .primary
                    .as_ref()
                    .and_then(|primary| primary.worker_base_url.clone()),
            })
        },
        None => None,
    };
    Json(AdminUsageJournalStatusResponse {
        cluster,
        journal_enabled: journal.journal_enabled,
        journal_root: journal.journal_root,
        current_rpm: activity.rpm,
        current_in_flight: activity.in_flight,
        active_file_sequence: journal.active_file_sequence,
        active_file_bytes: journal.active_file_bytes,
        sealed_file_count: journal.sealed_file_count,
        sealed_bytes: journal.sealed_bytes,
        oldest_sealed_age_ms: journal.oldest_sealed_age_ms,
        dropped_files_total: journal.dropped_files_total,
        dropped_unconsumed_files_total: journal.dropped_unconsumed_files_total,
        write_failures_total: journal.write_failures_total,
        usage_query_base_url: config.usage_query_base_url,
        producer_current_file: partitioned.producer_current_file,
        orphan_active_files: partitioned.orphan_active_files,
        current_consuming_file: partitioned.current_consuming_file,
        orphan_consuming_files: partitioned.orphan_consuming_files,
        active_files: files.active.into_iter().map(journal_file_view).collect(),
        sealed_files: files.sealed.into_iter().map(journal_file_view).collect(),
        consuming_files: files.consuming.into_iter().map(journal_file_view).collect(),
        bad_files: files.bad.into_iter().map(journal_file_view).collect(),
        worker,
        generated_at: now,
    })
    .into_response()
}

pub(crate) async fn get_usage_journal_preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminUsageJournalPreviewQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let mut journal = match producer_journal_status(&state) {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!("failed to load usage journal producer status: {err:#}");
            return internal_error("Failed to load usage journal status").into_response();
        },
    };
    journal.journal_enabled = config.usage_journal_enabled;
    if journal.journal_root.is_empty() {
        journal.journal_root = state
            .usage_journal_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
    let files = match journal_file_lists(&state) {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!("failed to load usage journal file lists: {err:#}");
            return internal_error("Failed to load usage journal file lists").into_response();
        },
    };
    let producer_current_file = producer_current_journal_file(&journal, &files.active);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0);
    let preview = if let Some(file) = producer_current_file.as_ref() {
        match JournalPreviewReader::open(FsPath::new(&file.path))
            .and_then(|reader| reader.read_recent_events_page(limit, offset))
        {
            Ok(report) => Some(admin_usage_journal_preview_view(report)),
            Err(err) => {
                tracing::warn!(
                    path = %file.path,
                    "failed to preview producer usage journal file: {err:#}"
                );
                return internal_error("Failed to preview usage journal producer file")
                    .into_response();
            },
        }
    } else {
        None
    };
    let total = preview.as_ref().map(|view| view.total_events).unwrap_or(0);
    let has_more = total > offset.saturating_add(limit);
    Json(AdminUsageJournalPreviewResponse {
        journal_root: journal.journal_root,
        producer_current_file,
        preview,
        limit,
        offset,
        total,
        has_more,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn list_llm_gateway_accounts(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminCodexAccountListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(AdminListQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let status = match state.public_status_store.codex_rate_limit_status().await {
        Ok(status) => Some(status),
        Err(_) => {
            return internal_error("Failed to load llm gateway account status").into_response();
        },
    };
    let filter = admin_codex_account_page_query(&query);
    let page = match state
        .admin_codex_account_store
        .list_admin_codex_accounts_filtered_page(&filter, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list llm gateway accounts").into_response(),
    };
    Json(AdminAccountsResponse {
        accounts: apply_cached_codex_status_to_admin_accounts(page.accounts, status),
        summary: page.summary,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}

fn admin_key_page_query(query: &AdminKeyListQuery) -> core_store::AdminKeyPageQuery {
    core_store::AdminKeyPageQuery {
        search: query.q.clone(),
        active_only: query.active_only.unwrap_or(false),
        sort: match query.sort.as_deref() {
            Some("quota_asc") => core_store::AdminKeySortMode::QuotaAsc,
            Some("quota_desc") => core_store::AdminKeySortMode::QuotaDesc,
            Some("usage_asc") => core_store::AdminKeySortMode::UsageAsc,
            Some("usage_desc") => core_store::AdminKeySortMode::UsageDesc,
            _ => core_store::AdminKeySortMode::Newest,
        },
    }
}

fn admin_codex_account_page_query(
    query: &AdminCodexAccountListQuery,
) -> core_store::AdminCodexAccountPageQuery {
    core_store::AdminCodexAccountPageQuery {
        search: query.q.clone(),
        active_only: query.active_only.unwrap_or(false),
        unhealthy_only: query.unhealthy_only.unwrap_or(false),
        sort: match query.sort.as_deref() {
            Some("primary_asc") => core_store::AdminCodexAccountSortMode::PrimaryAsc,
            Some("primary_desc") => core_store::AdminCodexAccountSortMode::PrimaryDesc,
            Some("secondary_asc") => core_store::AdminCodexAccountSortMode::SecondaryAsc,
            Some("secondary_desc") => core_store::AdminCodexAccountSortMode::SecondaryDesc,
            _ => core_store::AdminCodexAccountSortMode::Newest,
        },
    }
}

fn apply_cached_codex_status_to_admin_accounts(
    mut accounts: Vec<core_store::AdminCodexAccount>,
    status: Option<core_store::CodexRateLimitStatus>,
) -> Vec<core_store::AdminCodexAccount> {
    let Some(status) = status else {
        return accounts;
    };
    let mut status_by_name = status
        .accounts
        .into_iter()
        .map(|account| (account.name.clone(), account))
        .collect::<BTreeMap<_, _>>();
    for account in &mut accounts {
        let Some(status_account) = status_by_name.remove(&account.name) else {
            continue;
        };
        apply_codex_public_status_to_admin_account(account, status_account, status.last_checked_at);
    }
    accounts
}

fn apply_codex_public_status_to_admin_account(
    account: &mut core_store::AdminCodexAccount,
    status_account: core_store::CodexPublicAccountStatus,
    _status_last_checked_at: Option<i64>,
) {
    if account.status != KEY_STATUS_ACTIVE || status_account.status != KEY_STATUS_ACTIVE {
        account.plan_type = None;
        account.primary_remaining_percent = None;
        account.secondary_remaining_percent = None;
        account.last_usage_checked_at = None;
        account.last_usage_success_at = None;
        account.usage_error_message = None;
        return;
    }
    account.plan_type = status_account.plan_type;
    account.primary_remaining_percent = status_account.primary_remaining_percent;
    account.secondary_remaining_percent = status_account.secondary_remaining_percent;
    account.last_usage_checked_at = status_account.last_usage_checked_at;
    account.last_usage_success_at = status_account.last_usage_success_at;
    account.usage_error_message = status_account.usage_error_message;
}

pub(crate) async fn import_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let auth = match normalize_imported_codex_auth(request.auth_json, request.tokens) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    let account = NewAdminCodexAccount {
        name,
        account_id: auth.account_id,
        auth_json: auth.auth_json,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        route_weight_tier: None,
        created_at_ms: now_ms(),
    };
    match state
        .admin_codex_account_store
        .create_admin_codex_account(account)
        .await
    {
        Ok(account) => Json(account).into_response(),
        Err(_) => internal_error("Failed to import llm gateway account").into_response(),
    }
}

pub(crate) async fn create_llm_gateway_account_import_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateCodexBatchImportJobRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let request = match normalize_codex_batch_import_request(request) {
        Ok(request) => request,
        Err(response) => return response.into_response(),
    };
    let created_at_ms = now_ms();
    let job_id = generate_id("llm-import");
    let persisted = NewAdminCodexImportJob {
        job_id: job_id.clone(),
        provider_type: request.provider_type.clone(),
        source_type: request.source_type.clone(),
        validate_before_import: request.validate_before_import,
        items: request
            .items
            .iter()
            .map(|item| NewAdminCodexImportJobItem {
                requested_name: item.requested_name.clone(),
                requested_account_id: item.requested_account_id.clone(),
                raw_auth_json: item.raw_auth_json.clone(),
            })
            .collect(),
        created_at_ms,
    };
    let detail = match state
        .admin_codex_account_store
        .create_admin_codex_import_job(persisted)
        .await
    {
        Ok(detail) => detail,
        Err(_) => {
            return internal_error("Failed to create llm gateway account import job")
                .into_response();
        },
    };

    let worker_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) =
            run_codex_batch_import_job(worker_state.clone(), job_id.clone(), request).await
        {
            let _ = worker_state
                .admin_codex_account_store
                .fail_admin_codex_import_job(&job_id, &err.to_string(), now_ms())
                .await;
        }
    });

    Json(detail).into_response()
}

pub(crate) async fn list_llm_gateway_account_import_jobs(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ListCodexImportJobsRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_ADMIN_IMPORT_JOB_LIMIT)
        .clamp(1, MAX_ADMIN_IMPORT_JOB_LIMIT);
    match state
        .admin_codex_account_store
        .list_admin_codex_import_jobs(limit)
        .await
    {
        Ok(jobs) => Json(AdminCodexImportJobsResponse {
            jobs,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account import jobs").into_response(),
    }
}

pub(crate) async fn get_llm_gateway_account_import_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_codex_account_store
        .get_admin_codex_import_job(&job_id)
        .await
    {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => not_found("LLM gateway account import job not found").into_response(),
        Err(_) => internal_error("Failed to load llm gateway account import job").into_response(),
    }
}

pub(crate) async fn patch_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let patch = match normalize_account_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    let refresh_public_status = should_refresh_codex_public_status_after_patch(&patch);
    if let Some(Some(proxy_id)) = patch.proxy_config_id.as_ref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before account binding")
                .into_response();
        }
    }
    match state
        .admin_codex_account_store
        .patch_admin_codex_account(&name, patch)
        .await
    {
        Ok(Some(mut account)) => {
            if refresh_public_status {
                if let Err(err) =
                    refresh_codex_public_status_after_account_update(&state, &mut account).await
                {
                    tracing::warn!(
                        account_name = %account.name,
                        "failed to refresh Codex public status after account update: {err:#}"
                    );
                }
            }
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account").into_response(),
    }
}

fn should_refresh_codex_public_status_after_patch(patch: &AdminCodexAccountPatch) -> bool {
    patch.status.is_some()
        || patch.auto_refresh_enabled.is_some()
        || patch.proxy_mode.is_some()
        || patch.proxy_config_id.is_some()
}

async fn refresh_codex_public_status_after_account_update(
    state: &HttpState,
    account: &mut core_store::AdminCodexAccount,
) -> anyhow::Result<()> {
    let route_store = state.provider_state.route_store();
    let refreshed_status = if account.status == KEY_STATUS_ACTIVE && !account.auto_refresh_enabled {
        codex_status::refresh_single_codex_account_usage_only(
            &state.admin_config_store,
            &state.admin_codex_account_store,
            &route_store,
            &state.public_status_store,
            &account.name,
        )
        .await?
    } else {
        codex_status::prime_single_codex_account_status(
            &state.admin_config_store,
            &state.admin_codex_account_store,
            &route_store,
            &state.public_status_store,
            &account.name,
        )
        .await?
    };
    apply_codex_public_status_to_admin_account(account, refreshed_status, None);
    Ok(())
}

pub(crate) async fn delete_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_codex_account_store
        .delete_admin_codex_account(&name)
        .await
    {
        Ok(Some(account)) => Json(DeleteResponse {
            deleted: true,
            id: account.name,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account").into_response(),
    }
}

pub(crate) async fn refresh_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let route_store = state.provider_state.route_store();
    let refreshed_status = match codex_status::refresh_single_codex_account_status(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        &name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(mut account)) => {
            apply_codex_public_status_to_admin_account(&mut account, refreshed_status, None);
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to refresh llm gateway account").into_response(),
    }
}

pub(crate) async fn refresh_llm_gateway_account_auth(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let route = match state
        .admin_codex_account_store
        .resolve_admin_codex_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("LLM gateway account not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway account").into_response(),
    };
    let refreshed = match codex_refresh::refresh_auth_json_for_route(&route).await {
        Ok(refreshed) => refreshed,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account auth: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    if let Err(err) = state
        .provider_state
        .route_store()
        .save_codex_auth_update(refreshed)
        .await
    {
        return (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to persist llm gateway account auth refresh: {err}"),
                code: StatusCode::BAD_GATEWAY.as_u16(),
            }),
        )
            .into_response();
    }
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(account)) => Json(account).into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to load refreshed llm gateway account").into_response(),
    }
}

pub(crate) async fn refresh_llm_gateway_account_usage(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let route_store = state.provider_state.route_store();
    let refreshed_status = match codex_status::refresh_single_codex_account_usage_only(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        &name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account usage: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(mut account)) => {
            apply_codex_public_status_to_admin_account(&mut account, refreshed_status, None);
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to refresh llm gateway account usage").into_response(),
    }
}

pub(crate) async fn probe_llm_gateway_account_models(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let route = match state
        .admin_codex_account_store
        .resolve_admin_codex_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("LLM gateway account not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway account").into_response(),
    };
    let auth = match normalize_codex_auth_json(&route.auth_json) {
        Ok(auth) => auth,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to parse llm gateway account auth: {}", err.message),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let client_version =
        crate::provider::resolve_codex_client_version(Some(&config.codex_client_version));
    match validate_codex_access_token_for_import_with_client_version(&route, &auth, &client_version)
        .await
    {
        Ok(()) => Json(AdminCodexModelsProbeResponse {
            ok: true,
            message: "Codex models probe succeeded".to_string(),
            checked_at: now_ms(),
        })
        .into_response(),
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to probe llm gateway account models: {err}"),
                code: StatusCode::BAD_GATEWAY.as_u16(),
            }),
        )
            .into_response(),
    }
}

pub(crate) async fn list_admin_kiro_keys(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(query);
    let page = match state
        .admin_key_store
        .list_admin_keys_page(Some(PROVIDER_KIRO), page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list Kiro gateway keys").into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let keys = match apply_effective_kiro_cache_policies(page.keys, &config) {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to resolve Kiro cache policy").into_response(),
    };
    let keys = match attach_kiro_candidate_credit_summaries(&state, keys).await {
        Ok(keys) => keys,
        Err(_) => {
            return internal_error("Failed to compute Kiro candidate credit summary")
                .into_response();
        },
    };
    Json(AdminKeysResponse {
        keys,
        summary: page.summary,
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn create_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    if let Err(response) =
        validate_i64_backed_u64("quota_billable_limit", request.quota_billable_limit)
    {
        return response.into_response();
    }
    let secret = generate_secret();
    let key = NewAdminKey {
        id: generate_id("kiro-key"),
        name,
        key_hash: sha256_hex(secret.as_bytes()),
        secret,
        provider_type: PROVIDER_KIRO.to_string(),
        protocol_family: PROTOCOL_ANTHROPIC.to_string(),
        public_visible: false,
        quota_billable_limit: request.quota_billable_limit,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        created_at_ms: now_ms(),
    };
    match state.admin_key_store.create_admin_key(key).await {
        Ok(key) => match resolve_key_effective_kiro_cache_policy(&state, key).await {
            Ok(key) => Json(key).into_response(),
            Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
        },
        Err(_) => internal_error("Failed to create Kiro gateway key").into_response(),
    }
}

pub(crate) async fn patch_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_key_matches_provider(&state, &key_id, PROVIDER_KIRO).await {
        return not_found("Kiro gateway key not found").into_response();
    }
    let patch = match normalize_kiro_key_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state.admin_key_store.patch_admin_key(&key_id, patch).await {
        Ok(Some(key)) if key.provider_type == PROVIDER_KIRO => {
            match resolve_key_effective_kiro_cache_policy(&state, key).await {
                Ok(key) => Json(key).into_response(),
                Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
            }
        },
        Ok(_) => not_found("Kiro gateway key not found").into_response(),
        Err(_) => internal_error("Failed to update Kiro gateway key").into_response(),
    }
}

pub(crate) async fn delete_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_key_matches_provider(&state, &key_id, PROVIDER_KIRO).await {
        return not_found("Kiro gateway key not found").into_response();
    }
    match state.admin_key_store.delete_admin_key(&key_id).await {
        Ok(Some(key)) => Json(DeleteResponse {
            deleted: true,
            id: key.id,
        })
        .into_response(),
        Ok(None) => not_found("Kiro gateway key not found").into_response(),
        Err(_) => internal_error("Failed to delete Kiro gateway key").into_response(),
    }
}

pub(crate) async fn list_admin_kiro_account_groups(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    list_account_groups_for_provider(state, headers, query, PROVIDER_KIRO, "Kiro gateway").await
}

pub(crate) async fn list_admin_kiro_account_group_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    list_account_group_options_for_provider(state, headers, PROVIDER_KIRO, "Kiro gateway").await
}

pub(crate) async fn create_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayAccountGroupRequest>,
) -> Response {
    create_account_group_for_provider(state, headers, request, PROVIDER_KIRO, "kiro-group").await
}

pub(crate) async fn patch_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountGroupRequest>,
) -> Response {
    patch_account_group_for_provider(state, headers, group_id, request, PROVIDER_KIRO).await
}

pub(crate) async fn delete_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    delete_account_group_for_provider(state, headers, group_id, PROVIDER_KIRO).await
}

pub(crate) async fn list_admin_kiro_usage_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_list_query(&state, &uri).await
}

pub(crate) async fn get_admin_kiro_usage_event(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(event_id): Path<String>,
) -> Response {
    let _ = event_id;
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}

pub(crate) async fn list_admin_kiro_accounts(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(query);
    match state
        .admin_kiro_account_store
        .list_admin_kiro_accounts_page(page_request)
        .await
    {
        Ok(page) => Json(AdminKiroAccountsResponse {
            accounts: page.accounts,
            summary: page.summary,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
            has_more: page.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list Kiro gateway accounts").into_response(),
    }
}

pub(crate) async fn list_admin_kiro_account_statuses(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ListKiroAccountStatusesRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = AdminPageRequest {
        limit: query.limit.unwrap_or(24).clamp(1, 200),
        offset: query.offset.unwrap_or(0),
    };
    let prefix = query
        .prefix
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let page = match state
        .admin_kiro_account_store
        .list_admin_kiro_accounts_filtered_page(prefix, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list Kiro gateway accounts").into_response(),
    };
    Json(AdminKiroAccountStatusesResponse {
        accounts: page.accounts,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) async fn get_admin_kiro_cache_stats(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => {
            return internal_error("Failed to load llm gateway runtime config").into_response()
        },
    };
    Json(AdminKiroCacheStatsResponse {
        stats: state
            .provider_state
            .kiro_cache_stats(kiro_cache_simulation_config_from_admin_config(&config)),
        process_memory: read_current_process_memory_stats(),
        generated_at: now_ms(),
    })
    .into_response()
}

pub(crate) fn kiro_cache_simulation_config_from_admin_config(
    config: &AdminRuntimeConfig,
) -> KiroCacheSimulationConfig {
    KiroCacheSimulationConfig {
        mode: KiroCacheSimulationMode::from_runtime_value(&config.kiro_prefix_cache_mode),
        prefix_cache_max_tokens: config.kiro_prefix_cache_max_tokens,
        prefix_cache_entry_ttl: Duration::from_secs(config.kiro_prefix_cache_entry_ttl_seconds),
        conversation_anchor_max_entries: usize::try_from(
            config.kiro_conversation_anchor_max_entries,
        )
        .unwrap_or(usize::MAX),
        conversation_anchor_ttl: Duration::from_secs(config.kiro_conversation_anchor_ttl_seconds),
    }
}

pub(crate) async fn import_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportLocalKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_kiro_channel_limit_inputs(
        request.kiro_channel_max_concurrency,
        request.kiro_channel_min_start_interval_ms,
    ) {
        return response.into_response();
    }
    let sqlite_path = request
        .sqlite_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(local_import::default_sqlite_path);
    let mut auth =
        match local_import::import_from_sqlite(&sqlite_path, request.name.as_deref()).await {
            Ok(auth) => auth,
            Err(_) => return internal_error("Failed to import local Kiro auth").into_response(),
        };
    if let Some(value) = request.kiro_channel_max_concurrency {
        auth.kiro_channel_max_concurrency = Some(value);
    }
    if let Some(value) = request.kiro_channel_min_start_interval_ms {
        auth.kiro_channel_min_start_interval_ms = Some(value);
    }
    create_or_replace_kiro_account(state, auth.canonicalize()).await
}

pub(crate) async fn import_admin_kiro_auth_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportParsedKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let auth = match kiro_auth_from_import_request(request) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    create_or_replace_kiro_account(state, auth).await
}

pub(crate) async fn create_admin_kiro_manual_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateManualKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let auth = match kiro_auth_from_manual_request(request) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    create_or_replace_kiro_account(state, auth).await
}

pub(crate) async fn patch_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let patch = match normalize_kiro_account_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    let sync_status_cache = patch.status.is_some();
    if let Some(Some(proxy_id)) = patch.proxy_config_id.as_ref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before account binding")
                .into_response();
        }
    }
    match state
        .admin_kiro_account_store
        .patch_admin_kiro_account(&name, patch)
        .await
    {
        Ok(Some(account)) => {
            if sync_status_cache {
                sync_kiro_status_after_account_update(&state, &account).await;
            }
            Json(account).into_response()
        },
        Ok(None) => not_found("Kiro account not found").into_response(),
        Err(_) => internal_error("Failed to update Kiro account").into_response(),
    }
}

pub(crate) async fn delete_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_kiro_account_store
        .delete_admin_kiro_account(&name)
        .await
    {
        Ok(Some(_account)) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Ok(None) => not_found("Kiro account not found").into_response(),
        Err(_) => internal_error("Failed to delete Kiro account").into_response(),
    }
}

pub(crate) async fn get_admin_kiro_account_balance(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_kiro_account_store
        .get_admin_kiro_balance(&name)
        .await
    {
        Ok(Some(balance)) => Json(balance).into_response(),
        Ok(None) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Kiro balance cache is not ready yet".to_string(),
                code: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            }),
        )
            .into_response(),
        Err(_) => internal_error("Failed to load Kiro account balance").into_response(),
    }
}

pub(crate) async fn refresh_admin_kiro_account_balance(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("Kiro account not found").into_response(),
        Err(_) => return internal_error("Failed to load Kiro account").into_response(),
    };
    let now = now_ms();
    let route_store = state.provider_state.route_store();
    match kiro_refresh::fetch_usage_limits_for_route(&route, route_store.as_ref(), true).await {
        Ok(usage) => {
            let balance = admin_kiro_balance_from_usage(&usage)
                .with_manual_usage_limit(route.manual_usage_limit);
            let cache = core_store::AdminKiroCacheView {
                status: "ready".to_string(),
                last_checked_at: Some(now),
                last_success_at: Some(now),
                error_message: None,
                ..core_store::AdminKiroCacheView::default()
            };
            if let Err(err) = state
                .admin_kiro_account_store
                .save_admin_kiro_status_cache(core_store::AdminKiroStatusCacheUpdate {
                    account_name: name.clone(),
                    balance: Some(balance.clone()),
                    refreshed_at_ms: now,
                    expires_at_ms: now
                        + (cache.refresh_interval_seconds.min(i64::MAX as u64 / 1000) as i64
                            * 1000),
                    cache,
                    last_error: None,
                })
                .await
            {
                tracing::warn!(account_name = %name, "failed to persist kiro balance cache: {err:#}");
            }
            Json(balance).into_response()
        },
        Err(err) => {
            let cache = core_store::AdminKiroCacheView {
                status: "error".to_string(),
                last_checked_at: Some(now),
                error_message: Some(err.to_string()),
                ..core_store::AdminKiroCacheView::default()
            };
            let _ = state
                .admin_kiro_account_store
                .save_admin_kiro_status_cache(core_store::AdminKiroStatusCacheUpdate {
                    account_name: name,
                    balance: None,
                    refreshed_at_ms: now,
                    expires_at_ms: now + 60_000,
                    cache,
                    last_error: Some(err.to_string()),
                })
                .await;
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh Kiro account balance: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response()
        },
    }
}

pub(crate) async fn probe_admin_kiro_account_model(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<ProbeKiroAccountModelRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let request = match normalize_probe_kiro_account_model_request(request) {
        Ok(request) => request,
        Err(response) => return response.into_response(),
    };
    let mut route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("Kiro account not found").into_response(),
        Err(_) => return internal_error("Failed to load Kiro account").into_response(),
    };
    let (proxy, proxy_source) = match resolve_admin_kiro_probe_proxy(&state, &route, &request).await
    {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    route.proxy = proxy.clone();
    let upstream_request =
        build_direct_kiro_model_probe_request(&request.model, route.profile_arn.clone());
    let request_body = match serde_json::to_vec(&upstream_request) {
        Ok(body) => body,
        Err(_) => {
            return internal_error("Failed to encode Kiro model probe request").into_response();
        },
    };
    let upstream_url = format!(
        "{}/generateAssistantResponse",
        kiro_refresh::runtime_upstream_base_url(&route.api_region)
    );
    let started_at = Instant::now();
    match provider::call_kiro_generate_for_route(
        &route,
        state.provider_state.route_store().as_ref(),
        upstream_url,
        &request_body,
    )
    .await
    {
        Ok(response) => {
            let upstream_status_code = response.status().as_u16();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(err) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(AdminKiroModelProbeResponse {
                            ok: false,
                            account_name: name,
                            model: request.model,
                            api_region: route.api_region,
                            proxy_source: proxy_source.as_str().to_string(),
                            proxy_url: proxy.map(|value| value.proxy_url),
                            upstream_status_code,
                            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128)
                                as i64,
                            checked_at: now_ms(),
                            message: format!("failed to read Kiro model probe response: {err}"),
                        }),
                    )
                        .into_response();
                },
            };
            let events = match provider::decode_kiro_events_from_bytes(&bytes) {
                Ok(events) => events,
                Err(err) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(AdminKiroModelProbeResponse {
                            ok: false,
                            account_name: name,
                            model: request.model,
                            api_region: route.api_region,
                            proxy_source: proxy_source.as_str().to_string(),
                            proxy_url: proxy.map(|value| value.proxy_url),
                            upstream_status_code,
                            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128)
                                as i64,
                            checked_at: now_ms(),
                            message: err,
                        }),
                    )
                        .into_response();
                },
            };
            if let Some(message) = kiro_probe_eventstream_error_message(&events) {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(AdminKiroModelProbeResponse {
                        ok: false,
                        account_name: name,
                        model: request.model,
                        api_region: route.api_region,
                        proxy_source: proxy_source.as_str().to_string(),
                        proxy_url: proxy.map(|value| value.proxy_url),
                        upstream_status_code,
                        latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                        checked_at: now_ms(),
                        message,
                    }),
                )
                    .into_response();
            }
            Json(AdminKiroModelProbeResponse {
                ok: true,
                account_name: name,
                model: request.model,
                api_region: route.api_region,
                proxy_source: proxy_source.as_str().to_string(),
                proxy_url: proxy.map(|value| value.proxy_url),
                upstream_status_code,
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                checked_at: now_ms(),
                message: "Kiro model probe succeeded".to_string(),
            })
            .into_response()
        },
        Err(err) => (
            err.status(),
            Json(AdminKiroModelProbeResponse {
                ok: false,
                account_name: name,
                model: request.model,
                api_region: route.api_region,
                proxy_source: proxy_source.as_str().to_string(),
                proxy_url: proxy.map(|value| value.proxy_url),
                upstream_status_code: err.status().as_u16(),
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                checked_at: now_ms(),
                message: summarize_upstream_error_body(&err.body_text()),
            }),
        )
            .into_response(),
    }
}

pub(crate) async fn list_llm_gateway_token_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_token_requests(query)
        .await
    {
        Ok(page) => Json(AdminTokenRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway token requests").into_response(),
    }
}

pub(crate) async fn list_llm_gateway_account_contribution_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_account_contribution_requests(query)
        .await
    {
        Ok(page) => Json(AdminAccountContributionRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account contribution requests")
            .into_response(),
    }
}

pub(crate) async fn list_llm_gateway_sponsor_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_sponsor_requests(query)
        .await
    {
        Ok(page) => Json(AdminSponsorRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway sponsor requests").into_response(),
    }
}

pub(crate) async fn approve_and_issue_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway token request is finalized").into_response();
    }
    let Some(notifier) = state.email_notifier.clone() else {
        return internal_error(
            "Failed to send llm gateway token email: email notifier is not configured",
        )
        .into_response();
    };
    if current.issued_key_id.is_some() {
        return conflict("LLM gateway token request already has an issued key").into_response();
    }
    let key = if current.issued_key_id.is_none() {
        let secret = generate_secret();
        Some(NewAdminKey {
            id: generate_id("llm-key"),
            name: normalize_name(&format!("wish-{}", current.request_id))
                .unwrap_or_else(|_| format!("wish-{}", current.request_id)),
            key_hash: sha256_hex(secret.as_bytes()),
            secret,
            provider_type: PROVIDER_CODEX.to_string(),
            protocol_family: PROTOCOL_OPENAI.to_string(),
            public_visible: false,
            quota_billable_limit: current.requested_quota_billable_limit,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            created_at_ms: now_ms(),
        })
    } else {
        None
    };
    let email_key = key.clone();
    match state
        .admin_review_queue_store
        .issue_admin_token_request(&request_id, key, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => {
            let Some(email_key) = email_key.as_ref() else {
                return internal_error("Failed to send llm gateway token email").into_response();
            };
            if notifier
                .send_user_llm_token_issued_notification(&request, email_key)
                .await
                .is_err()
            {
                return internal_error("Failed to send llm gateway token email").into_response();
            }
            Json(request).into_response()
        },
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway token request").into_response(),
    }
}

pub(crate) async fn reject_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway token request cannot be rejected").into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway token request is already rejected").into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_token_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway token request").into_response(),
    }
}

pub(crate) async fn validate_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway account contribution request is finalized").into_response();
    }
    let action = review_queue_action(request);
    let auth = match codex_auth_from_fields(
        current.account_id.as_deref(),
        Some(&current.id_token),
        Some(&current.access_token),
        Some(&current.refresh_token),
    ) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    let validated_auth = match validate_codex_import_auth(&state, &current.account_name, &auth)
        .await
    {
        Ok(auth) => auth,
        Err(err) => {
            let failure_reason = format!("Codex auth validation failed: {err}");
            return match state
                .admin_review_queue_store
                .fail_admin_account_contribution_request(&request_id, failure_reason, action)
                .await
            {
                Ok(Some(request)) => Json(request).into_response(),
                Ok(None) => {
                    not_found("LLM gateway account contribution request not found").into_response()
                },
                Err(_) => internal_error("Failed to fail llm gateway account contribution request")
                    .into_response(),
            };
        },
    };
    let validated_id_token = validated_auth.id_token_or_empty();
    let validated_access_token = validated_auth.access_token_or_empty();
    let validated_refresh_token = validated_auth.refresh_token_or_empty();
    match state
        .admin_review_queue_store
        .validate_admin_account_contribution_request(
            &request_id,
            validated_auth.account_id,
            validated_id_token,
            validated_access_token,
            validated_refresh_token,
            action,
        )
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to validate llm gateway account contribution request")
            .into_response(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountContributionIssueEmailPolicy {
    SkipNoRecipient,
    SkipNoNotifier,
    Send,
}

fn account_contribution_issue_email_policy(
    request: &core_store::AdminAccountContributionRequest,
    notifier_available: bool,
) -> AccountContributionIssueEmailPolicy {
    if request.requester_email.trim().is_empty() {
        return AccountContributionIssueEmailPolicy::SkipNoRecipient;
    }
    if !notifier_available {
        return AccountContributionIssueEmailPolicy::SkipNoNotifier;
    }
    AccountContributionIssueEmailPolicy::Send
}

fn should_issue_account_contribution_access_artifacts(
    request: &core_store::AdminAccountContributionRequest,
) -> bool {
    !request.requester_email.trim().is_empty()
}

pub(crate) async fn approve_and_issue_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway account contribution request is finalized").into_response();
    }
    if current.status != core_store::PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED {
        return conflict("LLM gateway account contribution request must be validated before issue")
            .into_response();
    }
    if current.issued_key_id.is_some() {
        return conflict("LLM gateway account contribution request already has an issued key")
            .into_response();
    }
    let action = review_queue_action(request);
    let imported_account_name = current
        .imported_account_name
        .clone()
        .unwrap_or_else(|| current.account_name.clone());
    let account = if current.imported_account_name.is_none() {
        let auth = match codex_auth_from_fields(
            current.account_id.as_deref(),
            Some(&current.id_token),
            Some(&current.access_token),
            Some(&current.refresh_token),
        ) {
            Ok(auth) => auth,
            Err(response) => return response.into_response(),
        };
        Some(NewAdminCodexAccount {
            name: imported_account_name.clone(),
            account_id: auth.account_id,
            auth_json: auth.auth_json,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            route_weight_tier: None,
            created_at_ms: action.updated_at_ms,
        })
    } else {
        None
    };
    let (account_group, key) = if current.issued_key_id.is_none()
        && should_issue_account_contribution_access_artifacts(&current)
    {
        let group_id = generate_id("llm-group");
        let name = format!("contrib-{}", current.request_id);
        let secret = generate_secret();
        (
            Some(NewAdminAccountGroup {
                id: group_id,
                provider_type: PROVIDER_CODEX.to_string(),
                name: name.clone(),
                account_names: vec![imported_account_name],
                created_at_ms: action.updated_at_ms,
            }),
            Some(NewAdminKey {
                id: generate_id("llm-key"),
                name,
                key_hash: sha256_hex(secret.as_bytes()),
                secret,
                provider_type: PROVIDER_CODEX.to_string(),
                protocol_family: PROTOCOL_OPENAI.to_string(),
                public_visible: false,
                quota_billable_limit: 100_000_000_000,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                created_at_ms: action.updated_at_ms,
            }),
        )
    } else {
        (None, None)
    };
    let email_key = key.clone();
    match state
        .admin_review_queue_store
        .issue_admin_account_contribution_request(&request_id, account, account_group, key, action)
        .await
    {
        Ok(Some(request)) => {
            prime_codex_status_after_account_contribution_issue(&state, &request).await;
            match account_contribution_issue_email_policy(&request, state.email_notifier.is_some())
            {
                AccountContributionIssueEmailPolicy::SkipNoRecipient => {},
                AccountContributionIssueEmailPolicy::SkipNoNotifier => {
                    tracing::warn!(
                        request_id = %request.request_id,
                        account_name = %request.account_name,
                        "skipping issued account contribution email because email notifier is not configured",
                    );
                },
                AccountContributionIssueEmailPolicy::Send => {
                    if let (Some(email_key), Some(notifier)) =
                        (email_key.as_ref(), state.email_notifier.as_ref())
                    {
                        if let Err(err) = notifier
                            .send_user_llm_account_contribution_issued_notification(
                                &request, email_key,
                            )
                            .await
                        {
                            tracing::warn!(
                                request_id = %request.request_id,
                                account_name = %request.account_name,
                                requester_email = %request.requester_email,
                                "failed to send issued account contribution email: {err:#}",
                            );
                        }
                    }
                },
            }
            Json(request).into_response()
        },
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway account contribution request")
            .into_response(),
    }
}

async fn prime_codex_status_after_account_contribution_issue(
    state: &HttpState,
    request: &AdminAccountContributionRequest,
) {
    let account_name = request
        .imported_account_name
        .as_deref()
        .unwrap_or(request.account_name.as_str());
    let route_store = state.provider_state.route_store();
    let refreshed = match codex_status::prime_single_codex_account_status(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        account_name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!(
                request_id = %request.request_id,
                account_name,
                "failed to prime issued Codex account status: {err:#}",
            );
            return;
        },
    };
    tracing::info!(
        request_id = %request.request_id,
        account_name,
        plan_type = refreshed.plan_type.as_deref().unwrap_or("unknown"),
        primary_remaining_percent = refreshed.primary_remaining_percent.unwrap_or_default(),
        secondary_remaining_percent = refreshed.secondary_remaining_percent.unwrap_or_default(),
        "primed issued Codex account status",
    );
}

pub(crate) async fn reject_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway account contribution request cannot be rejected")
            .into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway account contribution request is already rejected")
            .into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_account_contribution_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway account contribution request")
            .into_response(),
    }
}

pub(crate) async fn approve_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_sponsor_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway sponsor request").into_response()
        },
    };
    if current.status == "approved" {
        return conflict("LLM gateway sponsor request is already approved").into_response();
    }
    match state
        .admin_review_queue_store
        .approve_admin_sponsor_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to approve llm gateway sponsor request").into_response(),
    }
}

pub(crate) async fn delete_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_review_queue_store
        .delete_admin_sponsor_request(&request_id)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway sponsor request").into_response(),
    }
}

async fn admin_key_matches_provider(state: &HttpState, key_id: &str, provider_type: &str) -> bool {
    state
        .admin_key_store
        .get_admin_key(key_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|key| key.provider_type == provider_type)
}

async fn admin_key_provider(state: &HttpState, key_id: &str) -> anyhow::Result<Option<String>> {
    Ok(state
        .admin_key_store
        .get_admin_key(key_id)
        .await?
        .map(|key| key.provider_type))
}

async fn list_account_groups_for_provider(
    state: HttpState,
    headers: HeaderMap,
    query: AdminListQuery,
    provider_type: &str,
    label: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page = admin_page_request(query);
    match state
        .admin_account_group_store
        .list_admin_account_groups_page(provider_type, page)
        .await
    {
        Ok(groups) => Json(AdminAccountGroupsResponse {
            groups: groups.groups,
            total: groups.total,
            limit: groups.limit,
            offset: groups.offset,
            has_more: groups.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error(&format!("Failed to list {label} account groups")).into_response(),
    }
}

async fn list_account_group_options_for_provider(
    state: HttpState,
    headers: HeaderMap,
    provider_type: &str,
    label: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_account_group_store
        .list_admin_account_group_options(provider_type)
        .await
    {
        Ok(options) => Json(AdminAccountGroupOptionsResponse {
            options,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => {
            internal_error(&format!("Failed to list {label} account group options")).into_response()
        },
    }
}

async fn create_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    request: CreateLlmGatewayAccountGroupRequest,
    provider_type: &str,
    id_prefix: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match normalize_account_names(request.account_names) {
        Ok(Some(names)) => names,
        Ok(None) => return bad_request("account_names must not be empty").into_response(),
        Err(response) => return response.into_response(),
    };
    let group = NewAdminAccountGroup {
        id: generate_id(id_prefix),
        provider_type: provider_type.to_string(),
        name,
        account_names,
        created_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .create_admin_account_group(group)
        .await
    {
        Ok(group) => Json(group).into_response(),
        Err(_) => internal_error("Failed to create account group").into_response(),
    }
}

async fn patch_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    group_id: String,
    request: PatchLlmGatewayAccountGroupRequest,
    provider_type: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current_groups = match state
        .admin_account_group_store
        .list_admin_account_groups(provider_type)
        .await
    {
        Ok(groups) => groups,
        Err(_) => return internal_error("Failed to inspect account groups").into_response(),
    };
    if !current_groups.iter().any(|group| group.id == group_id) {
        return not_found("Account group not found").into_response();
    }
    let name = match request.name.as_deref().map(normalize_name).transpose() {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match request
        .account_names
        .map(normalize_account_names)
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(response) => return response.into_response(),
    };
    let patch = AdminAccountGroupPatch {
        name,
        account_names,
        updated_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .patch_admin_account_group(&group_id, patch)
        .await
    {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => not_found("Account group not found").into_response(),
        Err(_) => internal_error("Failed to update account group").into_response(),
    }
}

async fn delete_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    group_id: String,
    provider_type: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let key = match state
        .admin_key_store
        .find_admin_key_referencing_account_group(provider_type, &group_id)
        .await
    {
        Ok(key) => key,
        Err(_) => return internal_error("Failed to inspect gateway keys").into_response(),
    };
    if let Some(key) = key {
        return bad_request(&format!("account group is still referenced by key `{}`", key.name))
            .into_response();
    }
    let current_groups = match state
        .admin_account_group_store
        .list_admin_account_groups(provider_type)
        .await
    {
        Ok(groups) => groups,
        Err(_) => return internal_error("Failed to inspect account groups").into_response(),
    };
    if !current_groups.iter().any(|group| group.id == group_id) {
        return not_found("Account group not found").into_response();
    }
    match state
        .admin_account_group_store
        .delete_admin_account_group(&group_id)
        .await
    {
        Ok(Some(group)) => Json(DeleteResponse {
            deleted: true,
            id: group.id,
        })
        .into_response(),
        Ok(None) => not_found("Account group not found").into_response(),
        Err(_) => internal_error("Failed to delete account group").into_response(),
    }
}

fn apply_runtime_config_update(
    current: AdminRuntimeConfig,
    request: UpdateAdminRuntimeConfig,
) -> Result<AdminRuntimeConfig, AdminHttpError> {
    let auth_cache_ttl_seconds = request
        .auth_cache_ttl_seconds
        .unwrap_or(current.auth_cache_ttl_seconds);
    validate_range(
        "auth_cache_ttl_seconds",
        auth_cache_ttl_seconds,
        MIN_RUNTIME_CACHE_TTL_SECONDS,
        MAX_RUNTIME_CACHE_TTL_SECONDS,
    )?;

    let max_request_body_bytes = request
        .max_request_body_bytes
        .unwrap_or(current.max_request_body_bytes);
    validate_range(
        "max_request_body_bytes",
        max_request_body_bytes,
        MIN_RUNTIME_REQUEST_BODY_BYTES,
        MAX_RUNTIME_REQUEST_BODY_BYTES,
    )?;

    let account_failure_retry_limit = request
        .account_failure_retry_limit
        .unwrap_or(current.account_failure_retry_limit);
    validate_range(
        "account_failure_retry_limit",
        account_failure_retry_limit,
        MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
        MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
    )?;

    let codex_client_version = match request.codex_client_version.as_deref() {
        Some(value) => normalize_codex_client_version(value)
            .ok_or_else(|| bad_request("codex_client_version is invalid"))?,
        None => current.codex_client_version,
    };

    let codex_status_refresh_min_interval_seconds = request
        .codex_status_refresh_min_interval_seconds
        .unwrap_or(current.codex_status_refresh_min_interval_seconds);
    let codex_status_refresh_max_interval_seconds = request
        .codex_status_refresh_max_interval_seconds
        .unwrap_or(current.codex_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
    )?;
    let codex_status_account_jitter_max_seconds = request
        .codex_status_account_jitter_max_seconds
        .unwrap_or(current.codex_status_account_jitter_max_seconds);
    validate_max(
        "codex_status_account_jitter_max_seconds",
        codex_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;
    let codex_weight_free = request
        .codex_weight_free
        .unwrap_or(current.codex_weight_free);
    let codex_weight_plus = request
        .codex_weight_plus
        .unwrap_or(current.codex_weight_plus);
    let codex_weight_pro5x = request
        .codex_weight_pro5x
        .unwrap_or(current.codex_weight_pro5x);
    let codex_weight_pro20x = request
        .codex_weight_pro20x
        .unwrap_or(current.codex_weight_pro20x);
    validate_max("codex_weight_free", codex_weight_free, u64::MAX)?;
    validate_max("codex_weight_plus", codex_weight_plus, u64::MAX)?;
    validate_max("codex_weight_pro5x", codex_weight_pro5x, u64::MAX)?;
    validate_max("codex_weight_pro20x", codex_weight_pro20x, u64::MAX)?;

    let codex_session_affinity_enabled = request
        .codex_session_affinity_enabled
        .unwrap_or(current.codex_session_affinity_enabled);
    let codex_session_affinity_max_entries = request
        .codex_session_affinity_max_entries
        .unwrap_or(current.codex_session_affinity_max_entries);
    validate_max(
        "codex_session_affinity_max_entries",
        codex_session_affinity_max_entries,
        MAX_RUNTIME_CODEX_AFFINITY_MAX_ENTRIES,
    )?;
    let codex_session_affinity_ttl_seconds = request
        .codex_session_affinity_ttl_seconds
        .unwrap_or(current.codex_session_affinity_ttl_seconds);
    validate_max(
        "codex_session_affinity_ttl_seconds",
        codex_session_affinity_ttl_seconds,
        MAX_RUNTIME_CODEX_AFFINITY_TTL_SECONDS,
    )?;
    let codex_fallback_affinity_enabled = request
        .codex_fallback_affinity_enabled
        .unwrap_or(current.codex_fallback_affinity_enabled);
    let codex_fallback_affinity_ttl_seconds = request
        .codex_fallback_affinity_ttl_seconds
        .unwrap_or(current.codex_fallback_affinity_ttl_seconds);
    validate_max(
        "codex_fallback_affinity_ttl_seconds",
        codex_fallback_affinity_ttl_seconds,
        MAX_RUNTIME_CODEX_AFFINITY_TTL_SECONDS,
    )?;
    let codex_fallback_affinity_prefix_bytes = request
        .codex_fallback_affinity_prefix_bytes
        .unwrap_or(current.codex_fallback_affinity_prefix_bytes);
    validate_max(
        "codex_fallback_affinity_prefix_bytes",
        codex_fallback_affinity_prefix_bytes,
        MAX_RUNTIME_CODEX_FALLBACK_PREFIX_BYTES,
    )?;
    let codex_fallback_affinity_min_body_bytes = request
        .codex_fallback_affinity_min_body_bytes
        .unwrap_or(current.codex_fallback_affinity_min_body_bytes);
    validate_max(
        "codex_fallback_affinity_min_body_bytes",
        codex_fallback_affinity_min_body_bytes,
        MAX_RUNTIME_CODEX_FALLBACK_PREFIX_BYTES,
    )?;

    let kiro_status_refresh_min_interval_seconds = request
        .kiro_status_refresh_min_interval_seconds
        .unwrap_or(current.kiro_status_refresh_min_interval_seconds);
    let kiro_status_refresh_max_interval_seconds = request
        .kiro_status_refresh_max_interval_seconds
        .unwrap_or(current.kiro_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
    )?;
    let kiro_status_account_jitter_max_seconds = request
        .kiro_status_account_jitter_max_seconds
        .unwrap_or(current.kiro_status_account_jitter_max_seconds);
    validate_max(
        "kiro_status_account_jitter_max_seconds",
        kiro_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let usage_event_flush_batch_size = request
        .usage_event_flush_batch_size
        .unwrap_or(current.usage_event_flush_batch_size);
    validate_range(
        "usage_event_flush_batch_size",
        usage_event_flush_batch_size,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
    )?;
    let usage_event_flush_interval_seconds = request
        .usage_event_flush_interval_seconds
        .unwrap_or(current.usage_event_flush_interval_seconds);
    validate_range(
        "usage_event_flush_interval_seconds",
        usage_event_flush_interval_seconds,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
    )?;
    let usage_event_flush_max_buffer_bytes = request
        .usage_event_flush_max_buffer_bytes
        .unwrap_or(current.usage_event_flush_max_buffer_bytes);
    validate_range(
        "usage_event_flush_max_buffer_bytes",
        usage_event_flush_max_buffer_bytes,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    )?;
    let duckdb_usage_memory_limit_mib = request
        .duckdb_usage_memory_limit_mib
        .unwrap_or(current.duckdb_usage_memory_limit_mib);
    validate_range(
        "duckdb_usage_memory_limit_mib",
        duckdb_usage_memory_limit_mib,
        MIN_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
        MAX_RUNTIME_DUCKDB_USAGE_MEMORY_LIMIT_MIB,
    )?;
    let duckdb_usage_checkpoint_threshold_mib = request
        .duckdb_usage_checkpoint_threshold_mib
        .unwrap_or(current.duckdb_usage_checkpoint_threshold_mib);
    validate_range(
        "duckdb_usage_checkpoint_threshold_mib",
        duckdb_usage_checkpoint_threshold_mib,
        MIN_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
        MAX_RUNTIME_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB,
    )?;
    let usage_analytics_retention_days = request
        .usage_analytics_retention_days
        .unwrap_or(current.usage_analytics_retention_days);
    validate_range(
        "usage_analytics_retention_days",
        usage_analytics_retention_days,
        MIN_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS,
        MAX_RUNTIME_USAGE_ANALYTICS_RETENTION_DAYS,
    )?;

    let usage_journal_enabled = request
        .usage_journal_enabled
        .unwrap_or(current.usage_journal_enabled);
    let usage_journal_max_file_bytes = request
        .usage_journal_max_file_bytes
        .unwrap_or(current.usage_journal_max_file_bytes);
    validate_range(
        "usage_journal_max_file_bytes",
        usage_journal_max_file_bytes,
        MIN_RUNTIME_USAGE_JOURNAL_FILE_BYTES,
        MAX_RUNTIME_USAGE_JOURNAL_FILE_BYTES,
    )?;
    let usage_journal_max_file_age_ms = request
        .usage_journal_max_file_age_ms
        .unwrap_or(current.usage_journal_max_file_age_ms);
    validate_range(
        "usage_journal_max_file_age_ms",
        usage_journal_max_file_age_ms,
        MIN_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS,
        MAX_RUNTIME_USAGE_JOURNAL_FILE_AGE_MS,
    )?;
    let usage_journal_max_files = request
        .usage_journal_max_files
        .unwrap_or(current.usage_journal_max_files);
    validate_range(
        "usage_journal_max_files",
        usage_journal_max_files,
        1,
        MAX_RUNTIME_USAGE_JOURNAL_FILES,
    )?;
    let usage_journal_block_target_uncompressed_bytes = request
        .usage_journal_block_target_uncompressed_bytes
        .unwrap_or(current.usage_journal_block_target_uncompressed_bytes);
    validate_range(
        "usage_journal_block_target_uncompressed_bytes",
        usage_journal_block_target_uncompressed_bytes,
        MIN_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES,
        MAX_RUNTIME_USAGE_JOURNAL_BLOCK_BYTES,
    )?;
    let usage_journal_block_max_events = request
        .usage_journal_block_max_events
        .unwrap_or(current.usage_journal_block_max_events);
    validate_range(
        "usage_journal_block_max_events",
        usage_journal_block_max_events,
        1,
        MAX_RUNTIME_USAGE_JOURNAL_BLOCK_EVENTS,
    )?;
    let usage_journal_fsync_interval_ms = request
        .usage_journal_fsync_interval_ms
        .unwrap_or(current.usage_journal_fsync_interval_ms);
    validate_max(
        "usage_journal_fsync_interval_ms",
        usage_journal_fsync_interval_ms,
        MAX_RUNTIME_USAGE_JOURNAL_FSYNC_INTERVAL_MS,
    )?;
    let usage_journal_zstd_level = request
        .usage_journal_zstd_level
        .unwrap_or(current.usage_journal_zstd_level);
    validate_i64_range(
        "usage_journal_zstd_level",
        usage_journal_zstd_level,
        0,
        MAX_RUNTIME_USAGE_JOURNAL_ZSTD_LEVEL,
    )?;
    let usage_journal_consumer_lease_ms = request
        .usage_journal_consumer_lease_ms
        .unwrap_or(current.usage_journal_consumer_lease_ms);
    validate_range(
        "usage_journal_consumer_lease_ms",
        usage_journal_consumer_lease_ms,
        MIN_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS,
        MAX_RUNTIME_USAGE_JOURNAL_CONSUMER_LEASE_MS,
    )?;
    let usage_journal_delete_bad_files = request
        .usage_journal_delete_bad_files
        .unwrap_or(current.usage_journal_delete_bad_files);
    let usage_query_bind_addr = match request.usage_query_bind_addr.as_deref() {
        Some(value) => normalize_usage_query_bind_addr(value)?,
        None => current.usage_query_bind_addr,
    };
    let usage_query_base_url = match request.usage_query_base_url.as_deref() {
        Some(value) => normalize_usage_query_base_url(value)?,
        None => current.usage_query_base_url,
    };

    let kiro_cache_kmodels_json = request
        .kiro_cache_kmodels_json
        .unwrap_or(current.kiro_cache_kmodels_json);
    parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;

    let kiro_billable_model_multipliers_json = match request.kiro_billable_model_multipliers_json {
        Some(value) => {
            let multipliers = parse_kiro_billable_model_multipliers_json(&value)
                .map_err(|_| bad_request("kiro_billable_model_multipliers_json is invalid"))?;
            serde_json::to_string(&multipliers).map_err(|_| {
                internal_error("Failed to normalize kiro billable multiplier config")
            })?
        },
        None => current.kiro_billable_model_multipliers_json,
    };

    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .unwrap_or(current.kiro_cache_policy_json);
    parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

    let kiro_context_usage_min_request_tokens = request
        .kiro_context_usage_min_request_tokens
        .unwrap_or(current.kiro_context_usage_min_request_tokens);
    validate_range(
        "kiro_context_usage_min_request_tokens",
        kiro_context_usage_min_request_tokens,
        1,
        MAX_RUNTIME_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
    )?;

    // `0` disables the proactive compaction gate, so allow it (validate_max,
    // not validate_positive).
    let kiro_compact_trigger_tokens = request
        .kiro_compact_trigger_tokens
        .unwrap_or(current.kiro_compact_trigger_tokens);
    validate_max(
        "kiro_compact_trigger_tokens",
        kiro_compact_trigger_tokens,
        MAX_RUNTIME_KIRO_COMPACT_TRIGGER_TOKENS,
    )?;

    let kiro_prefix_cache_mode = request
        .kiro_prefix_cache_mode
        .unwrap_or(current.kiro_prefix_cache_mode);
    validate_kiro_prefix_cache_mode(&kiro_prefix_cache_mode)?;

    let kiro_prefix_cache_max_tokens = request
        .kiro_prefix_cache_max_tokens
        .unwrap_or(current.kiro_prefix_cache_max_tokens);
    validate_positive("kiro_prefix_cache_max_tokens", kiro_prefix_cache_max_tokens)?;
    let kiro_prefix_cache_entry_ttl_seconds = request
        .kiro_prefix_cache_entry_ttl_seconds
        .unwrap_or(current.kiro_prefix_cache_entry_ttl_seconds);
    validate_positive("kiro_prefix_cache_entry_ttl_seconds", kiro_prefix_cache_entry_ttl_seconds)?;
    let kiro_conversation_anchor_max_entries = request
        .kiro_conversation_anchor_max_entries
        .unwrap_or(current.kiro_conversation_anchor_max_entries);
    validate_positive(
        "kiro_conversation_anchor_max_entries",
        kiro_conversation_anchor_max_entries,
    )?;
    let kiro_conversation_anchor_ttl_seconds = request
        .kiro_conversation_anchor_ttl_seconds
        .unwrap_or(current.kiro_conversation_anchor_ttl_seconds);
    validate_positive(
        "kiro_conversation_anchor_ttl_seconds",
        kiro_conversation_anchor_ttl_seconds,
    )?;
    let kiro_cache_snapshot_enabled = request
        .kiro_cache_snapshot_enabled
        .unwrap_or(current.kiro_cache_snapshot_enabled);
    let kiro_cache_snapshot_interval_seconds = request
        .kiro_cache_snapshot_interval_seconds
        .unwrap_or(current.kiro_cache_snapshot_interval_seconds);
    validate_range(
        "kiro_cache_snapshot_interval_seconds",
        kiro_cache_snapshot_interval_seconds,
        1,
        MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_INTERVAL_SECONDS,
    )?;
    let kiro_cache_snapshot_ttl_seconds = request
        .kiro_cache_snapshot_ttl_seconds
        .unwrap_or(current.kiro_cache_snapshot_ttl_seconds);
    validate_range(
        "kiro_cache_snapshot_ttl_seconds",
        kiro_cache_snapshot_ttl_seconds,
        1,
        MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_TTL_SECONDS,
    )?;
    let kiro_cache_snapshot_max_tokens = request
        .kiro_cache_snapshot_max_tokens
        .unwrap_or(current.kiro_cache_snapshot_max_tokens);
    validate_max(
        "kiro_cache_snapshot_max_tokens",
        kiro_cache_snapshot_max_tokens,
        MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_CAP,
    )?;
    let kiro_cache_snapshot_max_anchor_entries = request
        .kiro_cache_snapshot_max_anchor_entries
        .unwrap_or(current.kiro_cache_snapshot_max_anchor_entries);
    validate_max(
        "kiro_cache_snapshot_max_anchor_entries",
        kiro_cache_snapshot_max_anchor_entries,
        MAX_RUNTIME_KIRO_CACHE_SNAPSHOT_CAP,
    )?;
    let kiro_cctest_proxy_base_url = match request.kiro_cctest_proxy_base_url.as_deref() {
        Some(value) => normalize_cctest_proxy_base_url(value)?,
        None => current.kiro_cctest_proxy_base_url,
    };
    let kiro_cctest_proxy_api_key = request
        .kiro_cctest_proxy_api_key
        .as_deref()
        .map(normalize_optional_string)
        .unwrap_or(current.kiro_cctest_proxy_api_key);

    Ok(AdminRuntimeConfig {
        auth_cache_ttl_seconds,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version,
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds,
        codex_weight_free,
        codex_weight_plus,
        codex_weight_pro5x,
        codex_weight_pro20x,
        codex_session_affinity_enabled,
        codex_session_affinity_max_entries,
        codex_session_affinity_ttl_seconds,
        codex_fallback_affinity_enabled,
        codex_fallback_affinity_ttl_seconds,
        codex_fallback_affinity_prefix_bytes,
        codex_fallback_affinity_min_body_bytes,
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size,
        usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes,
        duckdb_usage_memory_limit_mib,
        duckdb_usage_checkpoint_threshold_mib,
        usage_analytics_retention_days,
        usage_journal_enabled,
        usage_journal_max_file_bytes,
        usage_journal_max_file_age_ms,
        usage_journal_max_files,
        usage_journal_block_target_uncompressed_bytes,
        usage_journal_block_max_events,
        usage_journal_fsync_interval_ms,
        usage_journal_zstd_level,
        usage_journal_consumer_lease_ms,
        usage_journal_delete_bad_files,
        usage_query_bind_addr,
        usage_query_base_url,
        kiro_cache_kmodels_json,
        kiro_billable_model_multipliers_json,
        kiro_cache_policy_json,
        kiro_context_usage_min_request_tokens,
        kiro_compact_trigger_tokens,
        kiro_prefix_cache_mode,
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
        kiro_cache_snapshot_enabled,
        kiro_cache_snapshot_interval_seconds,
        kiro_cache_snapshot_ttl_seconds,
        kiro_cache_snapshot_max_tokens,
        kiro_cache_snapshot_max_anchor_entries,
        kiro_cctest_proxy_base_url,
        kiro_cctest_proxy_api_key,
    })
}

fn ensure_admin_access(headers: &HeaderMap) -> Result<(), AdminHttpError> {
    if let Some(expected_token) = admin_token() {
        let provided = headers
            .get("x-admin-token")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
        if provided == expected_token {
            return Ok(());
        }
    }

    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        if is_local_host_header(headers) {
            return Ok(());
        }
        return Err(forbidden("Admin endpoint is local-only"));
    }
    let ip = ip
        .parse::<IpAddr>()
        .map_err(|_| forbidden("Admin endpoint is local-only"))?;
    if is_private_or_loopback_ip(ip) {
        Ok(())
    } else {
        Err(forbidden("Admin endpoint is local-only"))
    }
}

fn admin_token() -> Option<String> {
    std::env::var("LLM_ACCESS_ADMIN_TOKEN")
        .ok()
        .or_else(|| std::env::var("ADMIN_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn admin_page_request(query: AdminListQuery) -> core_store::AdminPageRequest {
    core_store::AdminPageRequest {
        limit: query
            .limit
            .unwrap_or(DEFAULT_ADMIN_LIST_LIMIT)
            .clamp(1, MAX_ADMIN_LIST_LIMIT),
        offset: query.offset.unwrap_or(0),
    }
}

fn generate_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}

fn generate_secret() -> String {
    format!("sfk_{}", uuid::Uuid::new_v4().simple())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn normalize_review_queue_query(
    request: ListReviewQueueRequest,
) -> core_store::AdminReviewQueueQuery {
    core_store::AdminReviewQueueQuery {
        status: request
            .status
            .and_then(|status| normalize_optional_string(&status)),
        limit: request
            .limit
            .unwrap_or(DEFAULT_ADMIN_REVIEW_QUEUE_LIMIT)
            .clamp(1, MAX_ADMIN_REVIEW_QUEUE_LIMIT),
        offset: request.offset.unwrap_or(0),
    }
}

fn acquire_admin_usage_query_permit(
    state: &HttpState,
) -> Result<OwnedSemaphorePermit, AdminHttpError> {
    std::sync::Arc::clone(&state.admin_usage_query_gate)
        .try_acquire_owned()
        .map_err(|_| too_many_requests("Another admin usage query is already running"))
}

fn producer_journal_status(state: &HttpState) -> anyhow::Result<JournalStatusSnapshot> {
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    if let Some(sink) = &state.usage_journal_sink {
        return sink.status_snapshot();
    }
    inspect_journal_dir(state.usage_journal_dir.as_deref())
}

fn journal_file_lists(state: &HttpState) -> anyhow::Result<JournalFileListsSnapshot> {
    let Some(root) = state.usage_journal_dir.as_deref() else {
        return Ok(JournalFileListsSnapshot::default());
    };
    collect_journal_file_lists(root)
}

fn inspect_journal_dir(root: Option<&FsPath>) -> anyhow::Result<JournalStatusSnapshot> {
    let Some(root) = root else {
        return Ok(JournalStatusSnapshot::default());
    };
    let active = active_journal_stats(&root.join("active"))?;
    let sealed = sealed_journal_stats(&root.join("sealed"))?;
    Ok(JournalStatusSnapshot {
        journal_enabled: true,
        journal_root: root.display().to_string(),
        active_file_sequence: active.file_sequence,
        active_file_bytes: active.bytes,
        sealed_file_count: sealed.file_count,
        sealed_bytes: sealed.bytes,
        oldest_sealed_age_ms: sealed.oldest_age_ms,
        dropped_files_total: 0,
        dropped_unconsumed_files_total: 0,
        write_failures_total: 0,
    })
}

#[derive(Default)]
struct ActiveJournalStats {
    file_sequence: Option<u64>,
    bytes: u64,
}

#[derive(Default)]
struct JournalDirStats {
    file_count: u64,
    bytes: u64,
    oldest_age_ms: Option<i64>,
}

fn active_journal_stats(dir: &FsPath) -> anyhow::Result<ActiveJournalStats> {
    if !dir.exists() {
        return Ok(ActiveJournalStats::default());
    }
    let mut stats = ActiveJournalStats::default();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read active journal dir `{}`", dir.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let Some(sequence) = journal_file_sequence(&entry.path()) else {
            continue;
        };
        if stats
            .file_sequence
            .is_none_or(|current| sequence >= current)
        {
            stats.file_sequence = Some(sequence);
            stats.bytes = metadata.len();
        }
    }
    Ok(stats)
}

fn sealed_journal_stats(dir: &FsPath) -> anyhow::Result<JournalDirStats> {
    if !dir.exists() {
        return Ok(JournalDirStats::default());
    }
    let mut stats = JournalDirStats::default();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read sealed journal dir `{}`", dir.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        stats.file_count = stats.file_count.saturating_add(1);
        stats.bytes = stats.bytes.saturating_add(metadata.len());
        if let Some(age_ms) = file_age_ms(&metadata) {
            stats.oldest_age_ms = Some(
                stats
                    .oldest_age_ms
                    .map_or(age_ms, |current| current.max(age_ms)),
            );
        }
    }
    Ok(stats)
}

fn journal_file_sequence(path: &FsPath) -> Option<u64> {
    let file_name = path.file_name()?.to_string_lossy();
    let suffix = file_name.strip_prefix("usage-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn journal_file_view(file: JournalFileSnapshot) -> AdminUsageJournalFileView {
    AdminUsageJournalFileView {
        file_name: file.file_name,
        path: file.path,
        sequence: file.sequence,
        bytes: file.bytes,
        age_ms: file.age_ms,
    }
}

fn admin_usage_journal_preview_view(
    report: JournalPreviewReport,
) -> AdminUsageJournalPreviewFileView {
    AdminUsageJournalPreviewFileView {
        path: report.path.display().to_string(),
        file_sequence: report.file_sequence,
        bytes_scanned: report.bytes_scanned,
        complete_blocks: report.complete_blocks,
        truncated_tail: report.truncated_tail,
        total_events: report.total_events,
        events: report
            .events
            .into_iter()
            .map(|event| AdminUsageJournalPreviewEventView {
                event_id: event.event_id,
                created_at_ms: event.created_at_ms,
                provider_type: event.provider_type,
                protocol_family: event.protocol_family,
                key_id: event.key_id,
                key_name: event.key_name,
                account_name: event.account_name,
                request_method: event.request_method,
                endpoint: event.endpoint,
                model: event.model,
                mapped_model: event.mapped_model,
                status_code: event.status_code,
                input_uncached_tokens: event.input_uncached_tokens,
                input_cached_tokens: event.input_cached_tokens,
                output_tokens: event.output_tokens,
                billable_tokens: event.billable_tokens,
                usage_missing: event.usage_missing,
                credit_usage_missing: event.credit_usage_missing,
                last_message_content: event.last_message_content,
                final_event_type: event.stream.final_event_type,
                stream_completed_cleanly: event.stream.stream_completed_cleanly,
                downstream_disconnect: event.stream.downstream_disconnect,
                bytes_streamed: event.stream.bytes_streamed,
                latency_ms: event.timing.latency_ms,
                first_sse_write_ms: event.timing.first_sse_write_ms,
            })
            .collect(),
    }
}

fn partition_usage_journal_files(
    journal: &JournalStatusSnapshot,
    files: &JournalFileListsSnapshot,
    worker: &AdminUsageWorkerProgressView,
) -> PartitionedUsageJournalFiles {
    let producer_current_file = producer_current_journal_file(journal, &files.active);
    let orphan_active_files = files
        .active
        .iter()
        .filter(|file| file.sequence != journal.active_file_sequence)
        .cloned()
        .map(journal_file_view)
        .collect();
    let current_consuming_file = worker_current_journal_file(worker, &files.consuming);
    let orphan_consuming_files = files
        .consuming
        .iter()
        .filter(|file| {
            !matches_worker_current_file(
                file,
                worker.current_file_sequence,
                worker.current_file_path.as_deref(),
            )
        })
        .cloned()
        .map(journal_file_view)
        .collect();
    PartitionedUsageJournalFiles {
        producer_current_file,
        orphan_active_files,
        current_consuming_file,
        orphan_consuming_files,
    }
}

fn producer_current_journal_file(
    journal: &JournalStatusSnapshot,
    active_files: &[JournalFileSnapshot],
) -> Option<AdminUsageJournalFileView> {
    let sequence = journal.active_file_sequence?;
    if let Some(file) = active_files
        .iter()
        .find(|file| file.sequence == Some(sequence))
    {
        return Some(journal_file_view(file.clone()));
    }
    Some(AdminUsageJournalFileView {
        file_name: format!("usage-{sequence:012}.open"),
        path: FsPath::new(&journal.journal_root)
            .join("active")
            .join(format!("usage-{sequence:012}.open"))
            .display()
            .to_string(),
        sequence: Some(sequence),
        bytes: journal.active_file_bytes,
        age_ms: None,
    })
}

fn worker_current_journal_file(
    worker: &AdminUsageWorkerProgressView,
    consuming_files: &[JournalFileSnapshot],
) -> Option<AdminUsageJournalFileView> {
    let matched = consuming_files
        .iter()
        .find(|file| {
            matches_worker_current_file(
                file,
                worker.current_file_sequence,
                worker.current_file_path.as_deref(),
            )
        })
        .cloned();
    match matched {
        Some(file) => Some(journal_file_view(file)),
        None if worker.current_file_sequence.is_some() || worker.current_file_path.is_some() => {
            let sequence = worker.current_file_sequence;
            let file_name = worker
                .current_file_path
                .as_deref()
                .and_then(|path| FsPath::new(path).file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    sequence
                        .map(|seq| format!("usage-{seq:012}.journal"))
                        .unwrap_or_else(|| "current-consuming-file".to_string())
                });
            Some(AdminUsageJournalFileView {
                file_name,
                path: worker.current_file_path.clone().unwrap_or_default(),
                sequence,
                bytes: worker.total_compressed_bytes,
                age_ms: None,
            })
        },
        None => None,
    }
}

fn matches_worker_current_file(
    file: &JournalFileSnapshot,
    current_sequence: Option<u64>,
    current_path: Option<&str>,
) -> bool {
    file.sequence == current_sequence || current_path.is_some_and(|path| file.path == path)
}

fn file_age_ms(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let modified_ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Some(now_ms().saturating_sub(modified_ms))
}

async fn usage_worker_status(base_url: &str, now: i64) -> AdminUsageWorkerProgressView {
    let url = format!("{}/admin/llm-access/usage-worker/status", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(err) => return unreachable_worker_view(now, err.to_string()),
    };
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(err) => return unreachable_worker_view(now, err.to_string()),
    };
    if !response.status().is_success() {
        return unreachable_worker_view(now, format!("worker returned {}", response.status()));
    }
    match response.json::<WorkerStatusSnapshot>().await {
        Ok(status) => worker_progress_view(status.progress, status.process_memory, now),
        Err(err) => unreachable_worker_view(now, err.to_string()),
    }
}

#[derive(Debug, Default, Deserialize)]
struct WorkerStatusSnapshot {
    #[serde(flatten)]
    progress: WorkerProgressSnapshot,
    #[serde(default)]
    process_memory: ProcessMemoryStats,
}

fn worker_progress_view(
    progress: WorkerProgressSnapshot,
    process_memory: ProcessMemoryStats,
    now: i64,
) -> AdminUsageWorkerProgressView {
    AdminUsageWorkerProgressView {
        state: progress.state,
        current_file_path: progress.current_file_path,
        current_file_sequence: progress.current_file_sequence,
        processed_blocks: progress.processed_blocks,
        total_blocks: progress.total_blocks,
        processed_events: progress.processed_events,
        total_events: progress.total_events,
        processed_compressed_bytes: progress.processed_compressed_bytes,
        total_compressed_bytes: progress.total_compressed_bytes,
        progress_percent: progress.progress_percent,
        import_rate_events_per_second: progress.import_rate_events_per_second,
        heartbeat_age_ms: progress
            .heartbeat_at_ms
            .map(|heartbeat| now.saturating_sub(heartbeat)),
        last_successful_file_sequence: progress.last_successful_file_sequence,
        last_successful_import_at_ms: progress.last_successful_import_at_ms,
        last_error: progress.last_error,
        last_error_at_ms: progress.last_error_at_ms,
        process_memory,
    }
}

fn unreachable_worker_view(now: i64, error: String) -> AdminUsageWorkerProgressView {
    worker_progress_view(
        WorkerProgressSnapshot {
            state: "unreachable".to_string(),
            last_error: Some(error),
            last_error_at_ms: Some(now),
            ..WorkerProgressSnapshot::default()
        },
        ProcessMemoryStats::default(),
        now,
    )
}

async fn proxy_usage_list_query(state: &HttpState, uri: &Uri) -> Response {
    let activity_key_id = usage_activity_key_id_from_uri(uri);
    let activity = state.request_activity.snapshot(activity_key_id.as_deref());
    proxy_usage_query_with_activity(state, uri, Some(activity)).await
}

async fn proxy_usage_query(state: &HttpState, uri: &Uri) -> Response {
    proxy_usage_query_with_activity(state, uri, None).await
}

async fn proxy_usage_query_with_activity(
    state: &HttpState,
    uri: &Uri,
    activity: Option<RequestActivitySnapshot>,
) -> Response {
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let base = config.usage_query_base_url.trim_end_matches('/');
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let url = format!("{base}{path_and_query}");
    let response = match reqwest::Client::new().get(&url).send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(url = %url, "usage worker query proxy failed: {err:#}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "Usage worker is unavailable".to_string(),
                    code: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                }),
            )
                .into_response();
        },
    };
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(url = %url, "failed to read usage worker response: {err:#}");
            return internal_error("Failed to read usage worker response").into_response();
        },
    };
    let body = match activity.filter(|_| status.is_success()) {
        Some(activity) => overlay_usage_activity_response_body(bytes.as_ref(), activity)
            .map(Body::from)
            .unwrap_or_else(|| Body::from(bytes)),
        None => Body::from(bytes),
    };
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder
        .body(body)
        .unwrap_or_else(|_| internal_error("Failed to build usage worker response").into_response())
}

fn usage_activity_key_id_from_uri(uri: &Uri) -> Option<String> {
    url::form_urlencoded::parse(uri.query()?.as_bytes()).find_map(|(name, value)| {
        (name == "key_id")
            .then(|| normalize_optional_string(&value))
            .flatten()
    })
}

fn overlay_usage_activity_response_body(
    body: &[u8],
    activity: RequestActivitySnapshot,
) -> Option<Vec<u8>> {
    let mut value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let object = value.as_object_mut()?;
    if !object.contains_key("events") || !object.contains_key("total") {
        return None;
    }
    object.insert("current_rpm".to_string(), serde_json::Value::from(activity.rpm));
    object.insert("current_in_flight".to_string(), serde_json::Value::from(activity.in_flight));
    serde_json::to_vec(&value).ok()
}

fn review_queue_action(request: ReviewQueueActionRequest) -> AdminReviewQueueAction {
    AdminReviewQueueAction {
        admin_note: normalize_optional_string_option(request.admin_note.as_deref()),
        updated_at_ms: now_ms(),
    }
}

async fn required_codex_default_proxy(
    state: &HttpState,
) -> Result<core_store::ProviderProxyConfig, AdminHttpError> {
    let bindings = state
        .admin_proxy_store
        .list_admin_proxy_bindings()
        .await
        .map_err(|_| internal_error("Failed to list llm gateway proxy bindings"))?;
    let binding = bindings
        .into_iter()
        .find(|binding| binding.provider_type == PROVIDER_CODEX)
        .ok_or_else(|| bad_request("default Codex proxy binding is not configured"))?;
    if let Some(message) = binding
        .error_message
        .as_deref()
        .and_then(normalize_optional_string)
    {
        return Err(bad_request(&format!("default Codex proxy binding is invalid: {message}")));
    }
    let proxy_url = binding
        .effective_proxy_url
        .as_deref()
        .and_then(normalize_optional_string)
        .ok_or_else(|| bad_request("default Codex proxy is required for validation"))?;
    Ok(core_store::ProviderProxyConfig {
        proxy_url,
        proxy_username: binding.effective_proxy_username,
        proxy_password: binding.effective_proxy_password,
    })
}

async fn run_codex_batch_import_job(
    state: HttpState,
    job_id: String,
    request: NormalizedCodexBatchImportJobRequest,
) -> anyhow::Result<()> {
    state
        .admin_codex_account_store
        .mark_admin_codex_import_job_running(&job_id, now_ms())
        .await
        .with_context(|| format!("mark codex import job `{job_id}` running"))?;

    let mut principal_lookup_cache = LruCache::new(
        NonZeroUsize::new(CODEX_IMPORT_PRINCIPAL_CACHE_MAX_ENTRIES)
            .expect("codex import principal cache capacity is non-zero"),
    );
    let mut seen_names = HashSet::new();
    for item in request.items {
        let item_updated_at_ms = now_ms();
        state
            .admin_codex_account_store
            .mark_admin_codex_import_job_item_running(&job_id, item.item_index, item_updated_at_ms)
            .await
            .with_context(|| {
                format!("mark codex import job `{job_id}` item {} running", item.item_index)
            })?;

        if !seen_names.insert(item.requested_name.clone()) {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some("account name is duplicated within the batch".to_string()),
                        item.requested_account_id.clone(),
                        None,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete duplicated codex import job `{job_id}` item {}",
                        item.item_index
                    )
                })?;
            continue;
        }

        if state
            .admin_codex_account_store
            .get_admin_codex_account(&item.requested_name)
            .await
            .with_context(|| format!("load codex account `{}`", item.requested_name))?
            .is_some()
        {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some("account name already exists".to_string()),
                        item.requested_account_id.clone(),
                        None,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete existing-name conflict for codex import job `{job_id}` item {}",
                        item.item_index
                    )
                })?;
            continue;
        }

        if let Some(existing_name) = lookup_conflicting_codex_principal_name(
            &state,
            &mut principal_lookup_cache,
            item.auth.principal_id.as_deref(),
            &item.requested_name,
        )
        .await?
        {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some("codex principal already belongs to another account".to_string()),
                        item.requested_account_id.clone(),
                        None,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete principal conflict for codex import job `{job_id}` item {} \
                         against `{existing_name}`",
                        item.item_index
                    )
                })?;
            continue;
        }

        let (auth, validated_at_ms) = if request.validate_before_import {
            match validate_codex_batch_import_auth(&state, &item).await {
                Ok(auth) => (auth, Some(now_ms())),
                Err(err) => {
                    state
                        .admin_codex_account_store
                        .complete_admin_codex_import_job_item(
                            &job_id,
                            codex_import_job_failure_result(
                                item.item_index,
                                "failed",
                                Some(err.to_string()),
                                item.requested_account_id.clone(),
                                None,
                                None,
                            ),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "complete validation failure for codex import job `{job_id}` item \
                                 {}",
                                item.item_index
                            )
                        })?;
                    continue;
                },
            }
        } else {
            (item.auth.clone(), None)
        };

        if let Some(existing_name) = lookup_conflicting_codex_principal_name(
            &state,
            &mut principal_lookup_cache,
            auth.principal_id.as_deref(),
            &item.requested_name,
        )
        .await?
        {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some(
                            "validated codex principal already belongs to another account"
                                .to_string(),
                        ),
                        auth.account_id.clone(),
                        validated_at_ms,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete validated principal conflict for codex import job `{job_id}` \
                         item {} against `{existing_name}`",
                        item.item_index
                    )
                })?;
            continue;
        }

        let imported_at_ms = now_ms();
        match state
            .admin_codex_account_store
            .create_admin_codex_account(NewAdminCodexAccount {
                name: item.requested_name.clone(),
                account_id: auth.account_id.clone(),
                auth_json: auth.auth_json.clone(),
                map_gpt53_codex_to_spark: false,
                auto_refresh_enabled: true,
                route_weight_tier: None,
                created_at_ms: imported_at_ms,
            })
            .await
        {
            Ok(account) => {
                if let Some(principal_id) = auth.principal_id.as_ref() {
                    principal_lookup_cache.put(principal_id.clone(), Some(account.name.clone()));
                }
                state
                    .admin_codex_account_store
                    .complete_admin_codex_import_job_item(
                        &job_id,
                        codex_import_job_success_result(
                            item.item_index,
                            account.name,
                            account.account_id.or(auth.account_id.clone()),
                            validated_at_ms,
                            imported_at_ms,
                        ),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "complete imported codex import job `{job_id}` item {}",
                            item.item_index
                        )
                    })?;
            },
            Err(err) => {
                state
                    .admin_codex_account_store
                    .complete_admin_codex_import_job_item(
                        &job_id,
                        codex_import_job_failure_result(
                            item.item_index,
                            "failed",
                            Some(err.to_string()),
                            auth.account_id.clone(),
                            validated_at_ms,
                            None,
                        ),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "complete create failure for codex import job `{job_id}` item {}",
                            item.item_index
                        )
                    })?;
            },
        }
    }

    Ok(())
}

async fn lookup_conflicting_codex_principal_name(
    state: &HttpState,
    principal_lookup_cache: &mut LruCache<String, Option<String>>,
    principal_id: Option<&str>,
    requested_name: &str,
) -> anyhow::Result<Option<String>> {
    let Some(principal_id) = principal_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let existing_name = match principal_lookup_cache.get(principal_id) {
        Some(cached) => cached.clone(),
        None => {
            let loaded = state
                .admin_codex_account_store
                .find_admin_codex_account_name_by_principal_id(principal_id)
                .await
                .with_context(|| format!("lookup codex principal `{principal_id}`"))?;
            principal_lookup_cache.put(principal_id.to_string(), loaded.clone());
            loaded
        },
    };
    Ok(existing_name.filter(|name| name != requested_name))
}

async fn validate_codex_batch_import_auth(
    state: &HttpState,
    item: &NormalizedCodexBatchImportJobItem,
) -> anyhow::Result<NormalizedCodexAuth> {
    validate_codex_import_auth(state, &item.requested_name, &item.auth)
        .await
        .with_context(|| {
            format!("validate auth for codex batch import account `{}`", item.requested_name)
        })
}

async fn validate_codex_import_auth(
    state: &HttpState,
    account_name: &str,
    auth: &NormalizedCodexAuth,
) -> anyhow::Result<NormalizedCodexAuth> {
    let proxy = required_codex_default_proxy(state)
        .await
        .map_err(|err| anyhow::anyhow!(err.message))?;
    let route = codex_validation_route(account_name, auth, proxy.clone());
    let has_refresh_token = auth
        .refresh_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty());
    if should_validate_codex_access_token_directly(auth) {
        match validate_codex_access_token_for_import(&route, auth).await {
            Ok(()) => return Ok(auth.clone()),
            Err(access_err) if !has_refresh_token => {
                return Err(access_err)
                    .context("validate codex import access token against models");
            },
            Err(_) => {},
        }
    }

    let refreshed = codex_refresh::refresh_auth_json_for_route(&route)
        .await
        .context("refresh auth for codex import account")?;
    let refreshed_auth = normalize_codex_auth_json(&refreshed.auth_json)
        .map_err(|err| anyhow::anyhow!(err.message))?;
    let refreshed_route = codex_validation_route(account_name, &refreshed_auth, proxy);
    validate_codex_access_token_for_import(&refreshed_route, &refreshed_auth)
        .await
        .context("validate refreshed codex import access token against models")?;
    Ok(refreshed_auth)
}

fn codex_validation_route(
    account_name: &str,
    auth: &NormalizedCodexAuth,
    proxy: core_store::ProviderProxyConfig,
) -> core_store::ProviderCodexRoute {
    core_store::ProviderCodexRoute {
        account_name: account_name.to_string(),
        account_group_id_at_event: None,
        route_strategy_at_event: RouteStrategy::Fixed,
        auth_json: auth.auth_json.clone(),
        map_gpt53_codex_to_spark: false,
        auth_refresh_enabled: true,
        codex_fast_enabled: true,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        account_request_max_concurrency: None,
        account_request_min_start_interval_ms: None,
        cached_error_message: None,
        proxy: Some(proxy),
    }
}

fn should_validate_codex_access_token_directly(auth: &NormalizedCodexAuth) -> bool {
    auth.access_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty() && !codex_refresh::access_token_is_expired(token))
}

async fn validate_codex_access_token_for_import(
    route: &core_store::ProviderCodexRoute,
    auth: &NormalizedCodexAuth,
) -> anyhow::Result<()> {
    validate_codex_access_token_for_import_with_client_version(
        route,
        auth,
        core_store::DEFAULT_CODEX_CLIENT_VERSION,
    )
    .await
}

async fn validate_codex_access_token_for_import_with_client_version(
    route: &core_store::ProviderCodexRoute,
    auth: &NormalizedCodexAuth,
    client_version: &str,
) -> anyhow::Result<()> {
    let access_token = auth
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing codex access_token"))?;
    let upstream_url = llm_access_codex::models::append_client_version_query(
        &crate::provider::compute_codex_upstream_url(
            &crate::provider::codex_upstream_base_url(),
            "/v1/models",
        ),
        client_version,
    );
    let client = codex_refresh::provider_client(route.proxy.as_ref())?;
    let mut request = client
        .get(&upstream_url)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            format!("{}/{}", CODEX_WIRE_ORIGINATOR, core_store::DEFAULT_CODEX_CLIENT_VERSION),
        )
        .header(reqwest::header::HeaderName::from_static("originator"), CODEX_WIRE_ORIGINATOR)
        .timeout(Duration::from_secs(CODEX_ACCESS_TOKEN_VALIDATION_TIMEOUT_SECONDS));
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("chatgpt-account-id", account_id);
    }
    if auth
        .id_token
        .as_deref()
        .is_some_and(codex_refresh::id_token_is_fedramp_account)
    {
        request = request.header("x-openai-fedramp", "true");
    }

    let response = request
        .send()
        .await
        .context("request Codex models with access token")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "codex access token validation returned {status}: {}",
            summarize_upstream_error_body(&body)
        );
    }
    let payload = response
        .json::<serde_json::Value>()
        .await
        .context("parse Codex models response")?;
    validate_codex_models_probe_payload(&payload)
}

fn validate_codex_models_probe_payload(payload: &serde_json::Value) -> anyhow::Result<()> {
    let models = payload
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Codex models response is missing models array"))?;
    if models.is_empty() {
        anyhow::bail!("Codex models response has empty models array");
    }
    Ok(())
}

fn codex_import_job_failure_result(
    item_index: usize,
    status: &str,
    error_message: Option<String>,
    final_account_id: Option<String>,
    validated_at_ms: Option<i64>,
    imported_at_ms: Option<i64>,
) -> AdminCodexImportJobItemResult {
    AdminCodexImportJobItemResult {
        item_index,
        status: status.to_string(),
        error_message,
        imported_account_name: None,
        final_account_id,
        validated_at_ms,
        imported_at_ms,
        completed_delta: 1,
        succeeded_delta: 0,
        skipped_delta: 0,
        failed_delta: 1,
        updated_at_ms: now_ms(),
    }
}

fn codex_import_job_success_result(
    item_index: usize,
    imported_account_name: String,
    final_account_id: Option<String>,
    validated_at_ms: Option<i64>,
    imported_at_ms: i64,
) -> AdminCodexImportJobItemResult {
    AdminCodexImportJobItemResult {
        item_index,
        status: "imported".to_string(),
        error_message: None,
        imported_account_name: Some(imported_account_name),
        final_account_id,
        validated_at_ms,
        imported_at_ms: Some(imported_at_ms),
        completed_delta: 1,
        succeeded_delta: 1,
        skipped_delta: 0,
        failed_delta: 0,
        updated_at_ms: now_ms(),
    }
}

async fn run_proxy_connectivity_check(
    proxy: &core_store::AdminProxyConfig,
    provider_type: &str,
) -> anyhow::Result<AdminProxyCheckResponse> {
    let target_url = match provider_type {
        PROVIDER_CODEX => "https://chatgpt.com/backend-api/codex/models".to_string(),
        PROVIDER_KIRO => format!(
            "{}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST",
            crate::kiro_refresh::management_upstream_base_url("us-east-1")
        ),
        _ => unreachable!("provider type must be validated before proxy check"),
    };
    let client = build_proxy_client(proxy)?;
    let started_at = Instant::now();
    let result = client.get(&target_url).send().await;
    let target = match result {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            AdminProxyCheckTargetView {
                target: provider_type.to_string(),
                url: target_url,
                reachable: true,
                status_code: Some(status.as_u16()),
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
            }
        },
        Err(err) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: target_url,
            reachable: false,
            status_code: None,
            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
            error_message: Some(err.to_string()),
        },
    };
    Ok(AdminProxyCheckResponse {
        proxy_config_id: proxy.id.clone(),
        proxy_config_name: proxy.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label: "anonymous connectivity probe".to_string(),
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}

async fn run_proxy_full_chain_check(
    state: &HttpState,
    proxy: &core_store::AdminProxyConfig,
    provider_type: &str,
) -> Result<AdminProxyCheckResponse, AdminHttpError> {
    let spec = proxy_full_chain_probe_spec(provider_type);
    let admin_key = load_full_chain_probe_key(state, spec.key_name, provider_type).await?;
    let authenticated_key = state
        .provider_state
        .authenticate_bearer_secret(&admin_key.secret)
        .await
        .map_err(|_| internal_error("Failed to authenticate real proxy probe key"))?
        .ok_or_else(|| internal_error("Real proxy probe key secret is not accepted"))?;
    if authenticated_key.key_id != admin_key.id
        || authenticated_key.provider_type != provider_type
        || authenticated_key.status != KEY_STATUS_ACTIVE
    {
        return Err(internal_error("Real proxy probe key state is inconsistent"));
    }
    let proxy_config = provider_proxy_config_from_admin(proxy);
    let request = Request::builder()
        .method(Method::POST)
        .uri(spec.path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, "llm-access-admin-proxy-probe")
        .body(Body::from(spec.body))
        .map_err(|_| internal_error("Failed to build real proxy probe request"))?;

    let started_at = Instant::now();
    let result =
        tokio::time::timeout(Duration::from_secs(PROXY_FULL_CHAIN_CHECK_TIMEOUT_SECONDS), async {
            let response = state
                .provider_state
                .dispatch_admin_probe_with_proxy(authenticated_key, request, proxy_config)
                .await;
            let status = response.status();
            let bytes = to_bytes(response.into_body(), PROXY_FULL_CHAIN_CHECK_MAX_BODY_BYTES)
                .await
                .context("read real proxy probe response body")?;
            Ok::<_, anyhow::Error>((status, String::from_utf8_lossy(&bytes).to_string()))
        })
        .await;
    let latency_ms = started_at.elapsed().as_millis().min(i64::MAX as u128) as i64;
    let target = match result {
        Ok(Ok((status, body))) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: status.is_success(),
            status_code: Some(status.as_u16()),
            latency_ms,
            error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
        },
        Ok(Err(err)) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: false,
            status_code: None,
            latency_ms,
            error_message: Some(err.to_string()),
        },
        Err(_) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: false,
            status_code: None,
            latency_ms,
            error_message: Some(format!(
                "real gateway probe timed out after {PROXY_FULL_CHAIN_CHECK_TIMEOUT_SECONDS}s"
            )),
        },
    };
    Ok(AdminProxyCheckResponse {
        proxy_config_id: proxy.id.clone(),
        proxy_config_name: proxy.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label: format!("{} real gateway probe", spec.key_name),
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}

struct ProxyFullChainProbeSpec {
    key_name: &'static str,
    path: &'static str,
    body: Vec<u8>,
}

fn proxy_full_chain_probe_spec(provider_type: &str) -> ProxyFullChainProbeSpec {
    match provider_type {
        PROVIDER_CODEX => ProxyFullChainProbeSpec {
            key_name: PROXY_FULL_CHAIN_CODEX_KEY_NAME,
            path: "/api/codex-gateway/v1/responses",
            body: serde_json::to_vec(&serde_json::json!({
                "model": PROXY_FULL_CHAIN_CODEX_MODEL,
                "input": "ping",
                "instructions": "Reply with one short word.",
                "max_output_tokens": 1,
                "stream": true
            }))
            .expect("static codex probe json should serialize"),
        },
        PROVIDER_KIRO => ProxyFullChainProbeSpec {
            key_name: PROXY_FULL_CHAIN_KIRO_KEY_NAME,
            path: "/api/kiro-gateway/v1/messages",
            body: serde_json::to_vec(&serde_json::json!({
                "model": PROXY_FULL_CHAIN_KIRO_MODEL,
                "max_tokens": 1,
                "stream": true,
                "messages": [{
                    "role": "user",
                    "content": "ping"
                }]
            }))
            .expect("static kiro probe json should serialize"),
        },
        _ => unreachable!("provider type must be validated before proxy check"),
    }
}

async fn load_full_chain_probe_key(
    state: &HttpState,
    key_name: &str,
    provider_type: &str,
) -> Result<core_store::AdminKey, AdminHttpError> {
    let keys = state
        .admin_key_store
        .list_admin_keys()
        .await
        .map_err(|_| internal_error("Failed to load real proxy probe keys"))?;
    keys.into_iter()
        .find(|key| {
            key.name == key_name
                && key.provider_type == provider_type
                && key.status == KEY_STATUS_ACTIVE
        })
        .ok_or_else(|| {
            not_found(&format!(
                "Active {provider_type} real proxy probe key '{key_name}' not found"
            ))
        })
}

fn provider_proxy_config_from_admin(
    proxy: &core_store::AdminProxyConfig,
) -> core_store::ProviderProxyConfig {
    core_store::ProviderProxyConfig {
        proxy_url: proxy.proxy_url.clone(),
        proxy_username: proxy.proxy_username.clone(),
        proxy_password: proxy.proxy_password.clone(),
    }
}

fn normalize_probe_kiro_account_model_request(
    request: ProbeKiroAccountModelRequest,
) -> Result<NormalizedProbeKiroAccountModelRequest, AdminHttpError> {
    let model = normalize_optional_string(&request.model)
        .ok_or_else(|| bad_request("model is required"))?;
    let proxy_url = normalize_optional_string_option(request.proxy_url.as_deref());
    let proxy_username = normalize_optional_string_option(request.proxy_username.as_deref());
    let proxy_password = normalize_optional_string_option(request.proxy_password.as_deref());
    if proxy_url.is_none() && (proxy_username.is_some() || proxy_password.is_some()) {
        return Err(bad_request(
            "proxy_url is required when inline proxy credentials are provided",
        ));
    }
    Ok(NormalizedProbeKiroAccountModelRequest {
        model,
        proxy_config_id: normalize_optional_string_option(request.proxy_config_id.as_deref()),
        inline_proxy: proxy_url.map(|proxy_url| core_store::ProviderProxyConfig {
            proxy_url,
            proxy_username,
            proxy_password,
        }),
    })
}

fn select_admin_kiro_probe_proxy(
    inline_proxy: Option<core_store::ProviderProxyConfig>,
    proxy_config_proxy: Option<core_store::ProviderProxyConfig>,
    resolved_proxy: Option<core_store::ProviderProxyConfig>,
) -> (Option<core_store::ProviderProxyConfig>, AdminKiroProbeProxySource) {
    if let Some(proxy) = inline_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::Inline);
    }
    if let Some(proxy) = proxy_config_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::ProxyConfig);
    }
    if let Some(proxy) = resolved_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::Resolved);
    }
    (None, AdminKiroProbeProxySource::None)
}

async fn resolve_admin_kiro_probe_proxy(
    state: &HttpState,
    route: &core_store::ProviderKiroRoute,
    request: &NormalizedProbeKiroAccountModelRequest,
) -> Result<(Option<core_store::ProviderProxyConfig>, AdminKiroProbeProxySource), AdminHttpError> {
    let proxy_config_proxy = if let Some(proxy_id) = request.proxy_config_id.as_deref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return Err(not_found("LLM gateway proxy config not found")),
            Err(_) => return Err(internal_error("Failed to load llm gateway proxy config")),
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return Err(bad_request("proxy config must be active before probing"));
        }
        Some(provider_proxy_config_from_admin(&proxy))
    } else {
        None
    };
    Ok(select_admin_kiro_probe_proxy(
        request.inline_proxy.clone(),
        proxy_config_proxy,
        route.proxy.clone(),
    ))
}

fn build_direct_kiro_model_probe_request(
    model: &str,
    profile_arn: Option<String>,
) -> llm_access_kiro::wire::KiroRequest {
    let conversation_id = format!("admin-model-probe-{}", uuid::Uuid::new_v4().simple());
    let current_message = llm_access_kiro::wire::CurrentMessage::new(
        llm_access_kiro::wire::UserInputMessage::new(ADMIN_KIRO_MODEL_PROBE_PROMPT, model),
    );
    llm_access_kiro::wire::KiroRequest {
        conversation_state: llm_access_kiro::wire::ConversationState::new(conversation_id)
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message),
        profile_arn,
    }
}

fn kiro_probe_eventstream_error_message(events: &[llm_access_kiro::wire::Event]) -> Option<String> {
    for event in events {
        match event {
            llm_access_kiro::wire::Event::Error {
                error_code,
                error_message,
            } => {
                return Some(format!(
                    "Kiro model probe stream error {error_code}: {}",
                    summarize_upstream_error_body(error_message)
                ));
            },
            llm_access_kiro::wire::Event::Exception {
                exception_type,
                message,
            } => {
                return Some(format!(
                    "Kiro model probe stream exception {exception_type}: {}",
                    summarize_upstream_error_body(message)
                ));
            },
            _ => {},
        }
    }
    None
}

fn normalize_proxy_check_mode(
    request: Option<&CheckLlmGatewayProxyConfigRequest>,
) -> Result<AdminProxyCheckMode, AdminHttpError> {
    let Some(mode) = request
        .and_then(|request| request.mode.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(AdminProxyCheckMode::Connectivity);
    };
    match mode {
        "connectivity" => Ok(AdminProxyCheckMode::Connectivity),
        "full_chain" | "real" | "real_gateway" => Ok(AdminProxyCheckMode::FullChain),
        _ => Err(bad_request("unknown proxy check mode")),
    }
}

fn proxy_endpoint_check_update_from_response(
    response: &AdminProxyCheckResponse,
) -> Option<core_store::AdminProxyEndpointCheckUpdate> {
    let target = response.targets.first()?;
    Some(core_store::AdminProxyEndpointCheckUpdate {
        proxy_config_id: response.proxy_config_id.clone(),
        provider_type: response.provider_type.clone(),
        target_url: target.url.clone(),
        reachable: target.reachable,
        status_code: target.status_code,
        latency_ms: target.latency_ms.max(0),
        error_message: target.error_message.clone(),
        checked_at_ms: response.checked_at,
    })
}

fn build_proxy_client(proxy: &core_store::AdminProxyConfig) -> anyhow::Result<reqwest::Client> {
    let mut proxy_config = reqwest::Proxy::all(&proxy.proxy_url)?;
    if let Some(username) = proxy.proxy_username.as_deref() {
        proxy_config =
            proxy_config.basic_auth(username, proxy.proxy_password.as_deref().unwrap_or(""));
    }
    reqwest::Client::builder()
        .proxy(proxy_config)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS))
        .build()
        .map_err(Into::into)
}

fn summarize_upstream_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

fn validate_range(field: &str, value: u64, min: u64, max: u64) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_i64_range(field: &str, value: i64, min: i64, max: i64) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_max(field: &str, value: u64, max: u64) -> Result<(), AdminHttpError> {
    if value <= max {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_positive(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value > 0 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} must be positive")))
    }
}

fn normalize_usage_query_bind_addr(value: &str) -> Result<String, AdminHttpError> {
    let trimmed = value.trim();
    if trimmed.parse::<SocketAddr>().is_ok() {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("usage_query_bind_addr is invalid"))
    }
}

fn normalize_usage_query_base_url(value: &str) -> Result<String, AdminHttpError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("usage_query_base_url is invalid"))
    }
}

fn normalize_cctest_proxy_base_url(value: &str) -> Result<Option<String>, AdminHttpError> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| bad_request("kiro_cctest_proxy_base_url is invalid"))?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(bad_request("kiro_cctest_proxy_base_url is invalid"));
    }
    if parsed.host_str().is_none() {
        return Err(bad_request("kiro_cctest_proxy_base_url is invalid"));
    }
    Ok(Some(trimmed.to_string()))
}

fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), AdminHttpError> {
    if !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
        .contains(&min_seconds)
        || !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS
            ..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
            .contains(&max_seconds)
    {
        return Err(bad_request("refresh window seconds must be between 240 and 3600"));
    }
    if min_seconds > max_seconds {
        return Err(bad_request("refresh min interval must be less than or equal to max interval"));
    }
    Ok(())
}

fn validate_kiro_prefix_cache_mode(mode: &str) -> Result<(), AdminHttpError> {
    if matches!(mode, KIRO_PREFIX_CACHE_MODE_FORMULA | core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE) {
        Ok(())
    } else {
        Err(bad_request("kiro_prefix_cache_mode is invalid"))
    }
}

async fn resolve_key_effective_kiro_cache_policy(
    state: &HttpState,
    key: core_store::AdminKey,
) -> anyhow::Result<core_store::AdminKey> {
    let config = state.admin_config_store.get_admin_runtime_config().await?;
    let keys = apply_effective_kiro_cache_policies(vec![key], &config)?;
    let keys = attach_kiro_candidate_credit_summaries(state, keys).await?;
    Ok(keys.into_iter().next().expect("single key should remain"))
}

fn apply_effective_kiro_cache_policies(
    mut keys: Vec<core_store::AdminKey>,
    config: &AdminRuntimeConfig,
) -> anyhow::Result<Vec<core_store::AdminKey>> {
    let runtime_policy = parse_kiro_cache_policy_json(&config.kiro_cache_policy_json)?;
    for key in keys
        .iter_mut()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
    {
        let effective = resolve_effective_kiro_cache_policy(
            &runtime_policy,
            key.kiro_cache_policy_override_json.as_deref(),
        )?;
        key.effective_kiro_cache_policy_json = serde_json::to_string(&effective)?;
        key.uses_global_kiro_cache_policy =
            uses_global_kiro_cache_policy(key.kiro_cache_policy_override_json.as_deref());
    }
    Ok(keys)
}

async fn attach_kiro_candidate_credit_summaries(
    state: &HttpState,
    keys: Vec<core_store::AdminKey>,
) -> anyhow::Result<Vec<core_store::AdminKey>> {
    if !keys.iter().any(|key| key.provider_type == PROVIDER_KIRO) {
        return Ok(keys);
    }
    if keys
        .iter()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
        .all(|key| key.kiro_candidate_credit_summary.is_some())
    {
        return Ok(keys);
    }
    let accounts = state
        .admin_kiro_account_store
        .list_admin_kiro_accounts()
        .await?;
    let groups = state
        .admin_account_group_store
        .list_admin_account_groups(PROVIDER_KIRO)
        .await?;
    Ok(apply_kiro_candidate_credit_summaries(keys, &accounts, &groups))
}

fn apply_kiro_candidate_credit_summaries(
    mut keys: Vec<core_store::AdminKey>,
    accounts: &[core_store::AdminKiroAccount],
    groups: &[core_store::AdminAccountGroup],
) -> Vec<core_store::AdminKey> {
    let all_account_names = accounts
        .iter()
        .map(|account| account.name.clone())
        .collect::<Vec<_>>();
    let accounts_by_name = accounts
        .iter()
        .map(|account| (account.name.as_str(), account))
        .collect::<BTreeMap<_, _>>();
    let groups_by_id = groups
        .iter()
        .map(|group| (group.id.as_str(), group))
        .collect::<BTreeMap<_, _>>();
    for key in keys
        .iter_mut()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
    {
        key.kiro_candidate_credit_summary = Some(build_kiro_candidate_credit_summary(
            key,
            &accounts_by_name,
            &groups_by_id,
            &all_account_names,
        ));
    }
    keys
}

fn build_kiro_candidate_credit_summary(
    key: &core_store::AdminKey,
    accounts_by_name: &BTreeMap<&str, &core_store::AdminKiroAccount>,
    groups_by_id: &BTreeMap<&str, &core_store::AdminAccountGroup>,
    all_account_names: &[String],
) -> core_store::AdminKiroKeyCandidateCreditSummary {
    let mut seen = HashSet::<String>::new();
    let mut summary = core_store::AdminKiroKeyCandidateCreditSummary::default();
    let preferred_pool_strategy =
        core_store::normalize_kiro_pool_strategy(&key.preferred_pool_strategy)
            .unwrap_or(core_store::KIRO_POOL_STRATEGY_BALANCED);
    for account_name in select_kiro_candidate_account_names(key, groups_by_id, all_account_names) {
        if !seen.insert(account_name.clone()) {
            continue;
        }
        let Some(account) = accounts_by_name.get(account_name.as_str()) else {
            continue;
        };
        summary.candidate_count += 1;
        let account_pool_strategy =
            core_store::normalize_kiro_pool_strategy(&account.pool_strategy)
                .unwrap_or(core_store::KIRO_POOL_STRATEGY_BALANCED);
        if account_pool_strategy == preferred_pool_strategy {
            summary.preferred_pool_candidate_count += 1;
        }
        if let Some(balance) = account.balance.as_ref() {
            summary.loaded_balance_count += 1;
            summary.total_limit += balance.usage_limit.max(0.0);
            summary.total_remaining += balance.remaining.max(0.0);
        } else {
            summary.missing_balance_count += 1;
        }
    }
    summary
}

fn select_kiro_candidate_account_names(
    key: &core_store::AdminKey,
    groups_by_id: &BTreeMap<&str, &core_store::AdminAccountGroup>,
    all_account_names: &[String],
) -> Vec<String> {
    let route_strategy = key.route_strategy.as_deref().unwrap_or("auto");
    let group_account_names = key
        .account_group_id
        .as_deref()
        .and_then(|group_id| groups_by_id.get(group_id))
        .map(|group| group.account_names.clone());
    match route_strategy {
        "fixed" => {
            if let Some(group_account_names) = group_account_names {
                group_account_names
            } else {
                key.fixed_account_name
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| vec![value.clone()])
                    .unwrap_or_default()
            }
        },
        "auto" => {
            if let Some(group_account_names) = group_account_names {
                group_account_names
            } else if let Some(auto_account_names) = key
                .auto_account_names
                .as_ref()
                .filter(|names| !names.is_empty())
            {
                auto_account_names.clone()
            } else {
                all_account_names.to_vec()
            }
        },
        _ => Vec::new(),
    }
}

fn normalize_key_patch(
    request: PatchLlmGatewayKeyRequest,
) -> Result<AdminKeyPatch, AdminHttpError> {
    let name = match request.name.as_deref() {
        Some(raw) => Some(normalize_name(raw)?),
        None => None,
    };
    let status = match request.status.as_deref() {
        Some(raw) => Some(normalize_status(raw)?),
        None => None,
    };
    if let Some(limit) = request.quota_billable_limit {
        validate_i64_backed_u64("quota_billable_limit", limit)?;
    }
    let route_strategy = match request.route_strategy.as_deref() {
        Some(raw) => Some(normalize_route_strategy_input(raw)?),
        None => None,
    };
    let account_group_id = request
        .account_group_id
        .as_deref()
        .map(normalize_optional_string);
    let fixed_account_name = request
        .fixed_account_name
        .as_deref()
        .map(normalize_optional_string);
    let auto_account_names = request.auto_account_names.map(normalize_auto_account_names);
    let preferred_pool_strategy = request
        .preferred_pool_strategy
        .as_deref()
        .map(normalize_kiro_pool_strategy_input)
        .transpose()?;
    let request_max_concurrency = if request.request_max_concurrency_unlimited {
        Some(None)
    } else {
        request.request_max_concurrency.map(Some)
    };
    let request_min_start_interval_ms = if request.request_min_start_interval_ms_unlimited {
        Some(None)
    } else {
        request.request_min_start_interval_ms.map(Some)
    };
    validate_codex_request_limit_inputs(
        request_max_concurrency.flatten(),
        request_min_start_interval_ms.flatten(),
    )?;
    if let Some(Some(raw)) = request.kiro_cache_policy_override_json.as_ref() {
        parse_kiro_cache_policy_override_json(raw)
            .map_err(|_| bad_request("kiro_cache_policy_override_json is invalid"))?;
    }
    let kiro_billable_model_multipliers_override_json =
        match request.kiro_billable_model_multipliers_override_json {
            Some(Some(raw)) => {
                let normalized = parse_kiro_billable_model_multipliers_json(&raw)
                    .and_then(|value| serde_json::to_string(&value).map_err(Into::into))
                    .map_err(|_| {
                        bad_request("kiro_billable_model_multipliers_override_json is invalid")
                    })?;
                Some(Some(normalized))
            },
            Some(None) => Some(None),
            None => None,
        };
    Ok(AdminKeyPatch {
        name,
        status,
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        route_strategy,
        account_group_id,
        fixed_account_name,
        auto_account_names,
        preferred_pool_strategy,
        model_name_map: request.model_name_map.map(Some),
        request_max_concurrency,
        request_min_start_interval_ms,
        codex_fast_enabled: request.codex_fast_enabled,
        kiro_request_validation_enabled: request.kiro_request_validation_enabled,
        kiro_cache_estimation_enabled: request.kiro_cache_estimation_enabled,
        kiro_zero_cache_debug_enabled: request.kiro_zero_cache_debug_enabled,
        kiro_full_request_logging_enabled: request.kiro_full_request_logging_enabled,
        kiro_remote_media_resolution_enabled: request.kiro_remote_media_resolution_enabled,
        kiro_latency_routing_enabled: request.kiro_latency_routing_enabled,
        kiro_protected_content_validation_enabled: request
            .kiro_protected_content_validation_enabled,
        kiro_cctest_text_handling_enabled: request.kiro_cctest_text_handling_enabled,
        kiro_cache_policy_override_json: request.kiro_cache_policy_override_json,
        kiro_billable_model_multipliers_override_json,
        updated_at_ms: now_ms(),
    })
}

fn normalize_kiro_key_patch(
    mut request: PatchLlmGatewayKeyRequest,
) -> Result<AdminKeyPatch, AdminHttpError> {
    request.public_visible = None;
    request.request_max_concurrency = None;
    request.request_min_start_interval_ms = None;
    request.request_max_concurrency_unlimited = false;
    request.request_min_start_interval_ms_unlimited = false;
    request.codex_fast_enabled = None;
    normalize_key_patch(request)
}

fn normalize_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(bad_request("name is required"))
    } else {
        Ok(trimmed.to_string())
    }
}

fn normalize_status(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if matches!(trimmed, KEY_STATUS_ACTIVE | KEY_STATUS_DISABLED) {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("status must be `active` or `disabled`"))
    }
}

fn normalize_route_strategy_input(raw: &str) -> Result<Option<String>, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}

fn normalize_kiro_pool_strategy_input(raw: &str) -> Result<String, AdminHttpError> {
    core_store::normalize_kiro_pool_strategy(raw)
        .map(str::to_string)
        .ok_or_else(|| bad_request("pool strategy must be `balanced` or `credit_first`"))
}

fn validate_provider_type(provider_type: &str) -> Result<(), AdminHttpError> {
    match provider_type {
        PROVIDER_CODEX | PROVIDER_KIRO => Ok(()),
        _ => Err(bad_request("provider_type must be `codex` or `kiro`")),
    }
}

fn normalize_optional_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_optional_string_option(raw: Option<&str>) -> Option<String> {
    raw.and_then(normalize_optional_string)
}

#[derive(Debug, Clone)]
struct NormalizedCodexAuth {
    auth_json: String,
    account_id: Option<String>,
    principal_id: Option<String>,
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
}

impl NormalizedCodexAuth {
    fn id_token_or_empty(&self) -> String {
        self.id_token.clone().unwrap_or_default()
    }

    fn access_token_or_empty(&self) -> String {
        self.access_token.clone().unwrap_or_default()
    }

    fn refresh_token_or_empty(&self) -> String {
        self.refresh_token.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone)]
struct NormalizedCodexBatchImportJobRequest {
    provider_type: String,
    source_type: String,
    validate_before_import: bool,
    items: Vec<NormalizedCodexBatchImportJobItem>,
}

#[derive(Debug, Clone)]
struct NormalizedCodexBatchImportJobItem {
    item_index: usize,
    requested_name: String,
    requested_account_id: Option<String>,
    raw_auth_json: String,
    auth: NormalizedCodexAuth,
}

fn normalize_codex_batch_import_request(
    request: CreateCodexBatchImportJobRequest,
) -> Result<NormalizedCodexBatchImportJobRequest, AdminHttpError> {
    if request.provider_type.trim() != PROVIDER_CODEX {
        return Err(bad_request("provider_type must be codex"));
    }
    if request.source_type.trim() != "local_json" {
        return Err(bad_request("source_type must be local_json"));
    }
    if request.items.is_empty() {
        return Err(bad_request("items must not be empty"));
    }
    let mut items = Vec::with_capacity(request.items.len());
    for (item_index, item) in request.items.into_iter().enumerate() {
        let requested_name = normalize_account_name(&item.name)?;
        let auth = normalize_imported_codex_auth(item.auth_json, item.tokens)?;
        items.push(NormalizedCodexBatchImportJobItem {
            item_index,
            requested_name,
            requested_account_id: auth.account_id.clone(),
            raw_auth_json: auth.auth_json.clone(),
            auth,
        });
    }
    Ok(NormalizedCodexBatchImportJobRequest {
        provider_type: PROVIDER_CODEX.to_string(),
        source_type: "local_json".to_string(),
        validate_before_import: request.validate_before_import,
        items,
    })
}

fn normalize_imported_codex_auth(
    raw_auth_json: Option<serde_json::Value>,
    tokens: Option<ImportLlmGatewayAccountTokens>,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    if let Some(value) = raw_auth_json {
        return normalize_codex_auth_value(value);
    }
    let Some(tokens) = tokens else {
        return Err(bad_request("auth_json or tokens is required"));
    };
    codex_auth_from_fields(
        tokens.account_id.as_deref(),
        tokens.id_token.as_deref(),
        tokens.access_token.as_deref(),
        tokens.refresh_token.as_deref(),
    )
}

fn codex_auth_from_fields(
    account_id: Option<&str>,
    id_token: Option<&str>,
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    let account_id = normalize_optional_string_option(account_id);
    let id_token = normalize_optional_string_option(id_token);
    let access_token = normalize_optional_string_option(access_token);
    let refresh_token = normalize_optional_string_option(refresh_token);
    if access_token.is_none() && refresh_token.is_none() {
        return Err(bad_request("access_token or refresh_token is required"));
    }
    let mut object = serde_json::Map::new();
    if let Some(value) = id_token.as_ref() {
        object.insert("id_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = access_token.as_ref() {
        object.insert("access_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = refresh_token.as_ref() {
        object.insert("refresh_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = account_id.as_ref() {
        object.insert("account_id".to_string(), serde_json::Value::String(value.clone()));
    }
    normalize_codex_auth_value(serde_json::Value::Object(object))
}

fn normalize_codex_auth_json(raw: &str) -> Result<NormalizedCodexAuth, AdminHttpError> {
    let value = serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|_| bad_request("auth_json must be valid JSON"))?;
    normalize_codex_auth_value(value)
}

fn normalize_codex_auth_value(
    value: serde_json::Value,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    if !value.is_object() {
        return Err(bad_request("auth_json must be a JSON object"));
    }
    let id_token = optional_auth_json_string(&value, &["id_token", "idToken"]);
    let access_token = optional_auth_json_string(&value, &["access_token", "accessToken"]);
    let refresh_token = optional_auth_json_string(&value, &["refresh_token", "refreshToken"]);
    let account_id = optional_auth_json_string(&value, &["account_id", "accountId"]);
    if access_token.is_none() && refresh_token.is_none() {
        return Err(bad_request("auth_json must contain access_token or refresh_token"));
    }
    let auth_json = serde_json::to_string(&value)
        .map_err(|_| internal_error("Failed to encode account auth"))?;
    let principal_id = core_store::codex_auth_principal_id(&auth_json);
    Ok(NormalizedCodexAuth {
        auth_json,
        account_id,
        principal_id,
        id_token,
        access_token,
        refresh_token,
    })
}

fn optional_auth_json_string(value: &serde_json::Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
        .or_else(|| {
            value.get("tokens").and_then(|tokens| {
                fields
                    .iter()
                    .find_map(|field| tokens.get(*field).and_then(serde_json::Value::as_str))
            })
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize_account_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request("account name is required"));
    }
    if trimmed.len() > 64 {
        return Err(bad_request("account name must be 64 characters or fewer"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(bad_request(
            "account name must contain only ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(trimmed.to_string())
}

fn normalize_account_names(values: Vec<String>) -> Result<Option<Vec<String>>, AdminHttpError> {
    let mut names = values
        .into_iter()
        .map(|value| normalize_account_name(&value))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    names.dedup();
    if names.is_empty() {
        Ok(None)
    } else {
        Ok(Some(names))
    }
}

fn normalize_auto_account_names(values: Vec<String>) -> Option<Vec<String>> {
    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}

fn normalize_required_proxy_url(raw: &str) -> Result<String, AdminHttpError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(bad_request("proxy_url is required"));
    }
    let parsed =
        url::Url::parse(value).map_err(|_| bad_request("proxy_url must be a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(bad_request("proxy_url scheme must be http, https, socks5, or socks5h"));
    }
    if parsed.host_str().is_none() {
        return Err(bad_request("proxy_url must include a host"));
    }
    Ok(value.to_string())
}

fn normalize_proxy_config_patch(
    request: PatchLlmGatewayProxyConfigRequest,
) -> Result<AdminProxyConfigPatch, AdminHttpError> {
    let name = request.name.as_deref().map(normalize_name).transpose()?;
    let proxy_url = request
        .proxy_url
        .as_deref()
        .map(normalize_required_proxy_url)
        .transpose()?;
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    Ok(AdminProxyConfigPatch {
        name,
        proxy_url,
        proxy_username: request
            .proxy_username
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        proxy_password: request
            .proxy_password
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        status,
        updated_at_ms: now_ms(),
    })
}

fn normalize_account_patch(
    request: PatchLlmGatewayAccountRequest,
) -> Result<AdminCodexAccountPatch, AdminHttpError> {
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    let proxy_mode = request
        .proxy_mode
        .as_deref()
        .map(normalize_proxy_mode)
        .transpose()?;
    let proxy_config_id = request
        .proxy_config_id
        .as_deref()
        .map(|value| normalize_optional_string_option(Some(value)));
    if matches!(proxy_mode.as_deref(), Some("fixed"))
        && proxy_config_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .is_none()
    {
        return Err(bad_request("fixed proxy_mode requires proxy_config_id"));
    }
    let route_weight_tier = request
        .route_weight_tier
        .as_deref()
        .map(normalize_codex_route_weight_tier)
        .transpose()?;
    let request_max_concurrency = if request.request_max_concurrency_unlimited {
        Some(None)
    } else {
        request.request_max_concurrency.map(Some)
    };
    let request_min_start_interval_ms = if request.request_min_start_interval_ms_unlimited {
        Some(None)
    } else {
        request.request_min_start_interval_ms.map(Some)
    };
    validate_codex_request_limit_inputs(
        request_max_concurrency.flatten(),
        request_min_start_interval_ms.flatten(),
    )?;
    Ok(AdminCodexAccountPatch {
        status,
        map_gpt53_codex_to_spark: request.map_gpt53_codex_to_spark,
        auto_refresh_enabled: request.auto_refresh_enabled,
        route_weight_tier,
        proxy_mode,
        proxy_config_id,
        request_max_concurrency,
        request_min_start_interval_ms,
        updated_at_ms: now_ms(),
    })
}

fn normalize_codex_route_weight_tier(raw: &str) -> Result<String, AdminHttpError> {
    let Some(value) = normalize_optional_string(raw) else {
        return Err(bad_request("route_weight_tier cannot be empty"));
    };
    match value.to_ascii_lowercase().as_str() {
        "auto" | "free" | "plus" | "pro5x" | "pro20x" => Ok(value.to_ascii_lowercase()),
        _ => Err(bad_request("route_weight_tier must be one of auto, free, plus, pro5x, pro20x")),
    }
}

enum KiroAuthSourceConfig {
    Manual,
    Imported { source: &'static str, source_db_path: Option<String>, last_imported_at: Option<i64> },
}

fn kiro_auth_from_manual_request(
    request: CreateManualKiroAccountRequest,
) -> Result<KiroAuthRecord, AdminHttpError> {
    kiro_auth_from_request_fields(request.auth, KiroAuthSourceConfig::Manual)
}

fn kiro_auth_from_import_request(
    request: ImportParsedKiroAccountRequest,
) -> Result<KiroAuthRecord, AdminHttpError> {
    let last_imported_at = match request.last_imported_at {
        Some(value) if value < 0 => return Err(bad_request("last_imported_at must be >= 0")),
        Some(value) => Some(value),
        None => Some(now_ms()),
    };
    kiro_auth_from_request_fields(request.auth, KiroAuthSourceConfig::Imported {
        source: "kiro-cli",
        source_db_path: normalize_optional_string_option(request.source_db_path.as_deref()),
        last_imported_at,
    })
}

fn kiro_auth_from_request_fields(
    request: KiroAuthRequestFields,
    source_config: KiroAuthSourceConfig,
) -> Result<KiroAuthRecord, AdminHttpError> {
    let KiroAuthRequestFields {
        name,
        access_token,
        refresh_token,
        profile_arn,
        expires_at,
        auth_method,
        client_id,
        client_secret,
        region,
        auth_region,
        api_region,
        machine_id,
        provider,
        email,
        subscription_title,
        kiro_channel_max_concurrency,
        kiro_channel_min_start_interval_ms,
        minimum_remaining_credits_before_block,
        manual_usage_limit,
        pool_strategy,
        disabled,
    } = request;
    let name = normalize_account_name(&name)?;
    validate_kiro_channel_limit_inputs(
        kiro_channel_max_concurrency,
        kiro_channel_min_start_interval_ms,
    )?;
    if let Some(value) = minimum_remaining_credits_before_block {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
        }
    }
    if let Some(Some(value)) = manual_usage_limit {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("manual_usage_limit must be >= 0"));
        }
    }
    let pool_strategy = pool_strategy
        .as_deref()
        .map(normalize_kiro_pool_strategy_input)
        .transpose()?;
    let (source, source_db_path, last_imported_at) = match source_config {
        KiroAuthSourceConfig::Manual => (Some("manual".to_string()), None, Some(now_ms())),
        KiroAuthSourceConfig::Imported {
            source,
            source_db_path,
            last_imported_at,
        } => (Some(source.to_string()), source_db_path, last_imported_at),
    };
    Ok(KiroAuthRecord {
        name,
        access_token: normalize_optional_string_option(access_token.as_deref()),
        refresh_token: normalize_optional_string_option(refresh_token.as_deref()),
        profile_arn: normalize_optional_string_option(profile_arn.as_deref()),
        expires_at: normalize_optional_string_option(expires_at.as_deref()),
        auth_method: normalize_optional_string_option(auth_method.as_deref()),
        client_id: normalize_optional_string_option(client_id.as_deref()),
        client_secret: normalize_optional_string_option(client_secret.as_deref()),
        region: normalize_optional_string_option(region.as_deref()),
        auth_region: normalize_optional_string_option(auth_region.as_deref()),
        api_region: normalize_optional_string_option(api_region.as_deref()),
        machine_id: normalize_optional_string_option(machine_id.as_deref()),
        provider: normalize_optional_string_option(provider.as_deref()),
        email: normalize_optional_string_option(email.as_deref()),
        subscription_title: normalize_optional_string_option(subscription_title.as_deref()),
        kiro_channel_max_concurrency,
        kiro_channel_min_start_interval_ms,
        minimum_remaining_credits_before_block,
        manual_usage_limit: manual_usage_limit.flatten(),
        pool_strategy,
        disabled,
        disabled_reason: None,
        source,
        source_db_path,
        last_imported_at,
        ..KiroAuthRecord::default()
    }
    .canonicalize())
}

fn new_admin_kiro_account_from_auth(
    auth: KiroAuthRecord,
    created_at_ms: i64,
) -> Result<NewAdminKiroAccount, AdminHttpError> {
    let name = auth.name.clone();
    let auth_method = auth.auth_method().to_string();
    let profile_arn = auth.profile_arn.clone();
    let max_concurrency = auth.effective_kiro_channel_max_concurrency();
    let min_start_interval_ms = auth.effective_kiro_channel_min_start_interval_ms();
    let proxy_config_id = auth.proxy_selection().proxy_config_id;
    let status = if auth.disabled { KEY_STATUS_DISABLED } else { KEY_STATUS_ACTIVE }.to_string();
    let auth_json = serde_json::to_string(&auth)
        .map_err(|_| internal_error("Failed to encode Kiro account auth"))?;
    Ok(NewAdminKiroAccount {
        name,
        auth_method,
        account_id: None,
        profile_arn,
        user_id: None,
        status,
        auth_json,
        max_concurrency: Some(max_concurrency),
        min_start_interval_ms: Some(min_start_interval_ms),
        proxy_config_id,
        created_at_ms,
    })
}

async fn create_or_replace_kiro_account(state: HttpState, auth: KiroAuthRecord) -> Response {
    let account = match new_admin_kiro_account_from_auth(auth, now_ms()) {
        Ok(account) => account,
        Err(response) => return response.into_response(),
    };
    let account_name = account.name.clone();
    match state
        .admin_kiro_account_store
        .create_admin_kiro_account(account)
        .await
    {
        Ok(account) => {
            tracing::info!(
                account_name = %account.name,
                auth_method = %account.auth_method,
                source = account.source.as_deref().unwrap_or("unknown"),
                region = account.region.as_deref().unwrap_or(""),
                pool_strategy = %account.pool_strategy,
                disabled = account.disabled,
                "saved kiro account"
            );
            sync_kiro_status_after_account_update(&state, &account).await;
            Json(account).into_response()
        },
        Err(err) => {
            tracing::warn!(account_name = %account_name, "failed to save kiro account: {err:#}");
            internal_error("Failed to save Kiro account").into_response()
        },
    }
}

async fn sync_kiro_status_after_account_update(
    state: &HttpState,
    account: &core_store::AdminKiroAccount,
) {
    if account.disabled {
        let now = now_ms();
        let refresh_interval_seconds = account.cache.refresh_interval_seconds;
        let update = core_store::AdminKiroStatusCacheUpdate {
            account_name: account.name.clone(),
            balance: None,
            refreshed_at_ms: now,
            expires_at_ms: now
                .saturating_add((refresh_interval_seconds as i64).saturating_mul(1000)),
            cache: core_store::AdminKiroCacheView {
                status: KEY_STATUS_DISABLED.to_string(),
                refresh_interval_seconds,
                last_checked_at: Some(now),
                last_success_at: account.cache.last_success_at,
                error_message: None,
            },
            last_error: None,
        };
        if let Err(err) = state
            .admin_kiro_account_store
            .save_admin_kiro_status_cache(update)
            .await
        {
            tracing::warn!(
                account_name = %account.name,
                "failed to persist disabled Kiro status after account update: {err:#}"
            );
        }
        return;
    }

    let route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&account.name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(
                account_name = %account.name,
                "failed to resolve Kiro route after account update: {err:#}"
            );
            return;
        },
    };
    let route_store = state.provider_state.route_store();
    if let Err(err) =
        kiro_status::refresh_and_persist_route_status(&route, route_store.as_ref(), false).await
    {
        tracing::warn!(
            account_name = %account.name,
            "failed to refresh cached Kiro status after account update: {err:#}"
        );
    }
}

fn normalize_kiro_account_patch(
    request: PatchKiroAccountRequest,
) -> Result<core_store::AdminKiroAccountPatch, AdminHttpError> {
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    validate_kiro_channel_limit_inputs(
        request.kiro_channel_max_concurrency,
        request.kiro_channel_min_start_interval_ms,
    )?;
    if let Some(value) = request.minimum_remaining_credits_before_block {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
        }
    }
    if let Some(Some(value)) = request.manual_usage_limit {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("manual_usage_limit must be >= 0"));
        }
    }
    let pool_strategy = request
        .pool_strategy
        .as_deref()
        .map(normalize_kiro_pool_strategy_input)
        .transpose()?;
    let proxy_mode = request
        .proxy_mode
        .as_deref()
        .map(normalize_proxy_mode)
        .transpose()?;
    let proxy_config_id = request
        .proxy_config_id
        .as_deref()
        .map(|value| normalize_optional_string_option(Some(value)));
    if matches!(proxy_mode.as_deref(), Some("fixed"))
        && proxy_config_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .is_none()
    {
        return Err(bad_request("fixed proxy_mode requires proxy_config_id"));
    }
    Ok(core_store::AdminKiroAccountPatch {
        status,
        max_concurrency: request.kiro_channel_max_concurrency,
        min_start_interval_ms: request.kiro_channel_min_start_interval_ms,
        minimum_remaining_credits_before_block: request.minimum_remaining_credits_before_block,
        manual_usage_limit: request.manual_usage_limit,
        pool_strategy,
        proxy_mode,
        proxy_config_id,
        updated_at_ms: now_ms(),
    })
}

fn normalize_proxy_mode(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    match trimmed {
        "inherit" | "fixed" | "none" => Ok(trimmed.to_string()),
        _ => Err(bad_request("proxy_mode must be `inherit`, `fixed`, or `none`")),
    }
}

fn validate_kiro_channel_limit_inputs(
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
) -> Result<(), AdminHttpError> {
    if let Some(value) = max_concurrency {
        if value == 0 || value > MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY {
            return Err(bad_request("kiro_channel_max_concurrency is out of range"));
        }
    }
    if let Some(value) = min_start_interval_ms {
        if value > MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS {
            return Err(bad_request("kiro_channel_min_start_interval_ms is out of range"));
        }
    }
    Ok(())
}

fn admin_kiro_balance_from_usage(
    usage: &llm_access_kiro::wire::UsageLimitsResponse,
) -> core_store::AdminKiroBalanceView {
    let usage_limit = usage.usage_limit();
    let current_usage = usage.current_usage();
    core_store::AdminKiroBalanceView {
        current_usage,
        usage_limit,
        remaining: (usage_limit - current_usage).max(0.0),
        upstream_usage_limit: None,
        manual_usage_limit: None,
        next_reset_at: usage
            .usage_breakdown_list
            .first()
            .and_then(|item| item.next_date_reset.or(usage.next_date_reset))
            .map(|value| value as i64),
        subscription_title: usage.subscription_title().map(ToString::to_string),
        user_id: usage.user_id().map(ToString::to_string),
    }
}

fn validate_codex_request_limit_inputs(
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
) -> Result<(), AdminHttpError> {
    if let Some(value) = request_max_concurrency {
        if value == 0 || value > MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY {
            return Err(bad_request("request_max_concurrency is out of range"));
        }
    }
    if let Some(value) = request_min_start_interval_ms {
        if value > MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS {
            return Err(bad_request("request_min_start_interval_ms is out of range"));
        }
    }
    Ok(())
}

fn validate_i64_backed_u64(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value <= i64::MAX as u64 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let map: BTreeMap<String, f64> = serde_json::from_str(value)?;
    anyhow::ensure!(!map.is_empty(), "kmodel map must not be empty");
    for (model, coeff) in &map {
        anyhow::ensure!(!model.trim().is_empty(), "kmodel entry has empty model name");
        anyhow::ensure!(
            coeff.is_finite() && *coeff > 0.0,
            "kmodel entry `{model}` must be a positive finite number"
        );
    }
    Ok(map)
}

fn parse_kiro_billable_model_multipliers_json(
    value: &str,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let overrides: BTreeMap<String, f64> = serde_json::from_str(value)?;
    let mut merged = BTreeMap::from([
        ("haiku".to_string(), 1.0),
        ("opus".to_string(), 1.0),
        ("sonnet".to_string(), 1.0),
    ]);
    for (family, multiplier) in overrides {
        anyhow::ensure!(
            matches!(family.as_str(), "opus" | "sonnet" | "haiku"),
            "billable multiplier family `{family}` must be one of `opus`, `sonnet`, `haiku`"
        );
        anyhow::ensure!(
            multiplier.is_finite() && multiplier > 0.0,
            "billable multiplier `{family}` must be a positive finite number"
        );
        merged.insert(family, multiplier);
    }
    Ok(merged)
}

fn parse_kiro_cache_policy_json(value: &str) -> anyhow::Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}

fn validate_kiro_cache_policy(policy: &KiroCachePolicy) -> anyhow::Result<()> {
    let boost = &policy.small_input_high_credit_boost;
    anyhow::ensure!(
        boost.target_input_tokens > 0,
        "small_input_high_credit_boost.target_input_tokens must be positive"
    );
    anyhow::ensure!(
        boost.credit_start.is_finite()
            && boost.credit_end.is_finite()
            && boost.credit_start < boost.credit_end,
        "small_input_high_credit_boost credit range is invalid"
    );
    anyhow::ensure!(
        policy.high_credit_diagnostic_threshold.is_finite()
            && policy.high_credit_diagnostic_threshold >= 0.0,
        "high_credit_diagnostic_threshold must be finite and >= 0"
    );
    anyhow::ensure!(
        policy.anthropic_cache_creation_input_ratio.is_finite()
            && (0.0..=1.0).contains(&policy.anthropic_cache_creation_input_ratio),
        "anthropic_cache_creation_input_ratio must be finite and between 0 and 1"
    );
    anyhow::ensure!(
        !policy.prefix_tree_credit_ratio_bands.is_empty(),
        "prefix_tree_credit_ratio_bands must contain at least one band"
    );

    let mut previous_credit_end = None;
    let mut previous_ratio_end = None;
    for (index, band) in policy.prefix_tree_credit_ratio_bands.iter().enumerate() {
        anyhow::ensure!(
            band.credit_start.is_finite() && band.credit_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] credit bounds must be finite"
        );
        anyhow::ensure!(
            band.credit_start < band.credit_end,
            "prefix_tree_credit_ratio_bands[{index}] credit_start must be < credit_end"
        );
        anyhow::ensure!(
            band.cache_ratio_start.is_finite() && band.cache_ratio_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be finite"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&band.cache_ratio_start)
                && (0.0..=1.0).contains(&band.cache_ratio_end),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be between 0 and 1"
        );
        anyhow::ensure!(
            band.cache_ratio_start >= band.cache_ratio_end,
            "prefix_tree_credit_ratio_bands[{index}] cache ratio must not increase within the band"
        );
        if let Some(prev_end) = previous_credit_end {
            anyhow::ensure!(
                band.credit_start >= prev_end - BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] overlaps previous band"
            );
            anyhow::ensure!(
                band.credit_start <= prev_end + BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] has a gap after previous band"
            );
        }
        if let Some(prev_ratio) = previous_ratio_end {
            anyhow::ensure!(
                band.cache_ratio_start <= prev_ratio,
                "prefix_tree_credit_ratio_bands[{index}] cache ratio increases between bands"
            );
        }
        previous_credit_end = Some(band.credit_end);
        previous_ratio_end = Some(band.cache_ratio_end);
    }
    Ok(())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',')
        .find_map(|part| normalize_ip_token(part.trim()))
}

fn parse_ip_from_forwarded_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    for segment in raw.split(',') {
        for pair in segment.split(';') {
            let (key, value) = pair.split_once('=')?;
            if key.trim().eq_ignore_ascii_case("for") {
                if let Some(ip) = normalize_ip_token(value.trim().trim_matches('"')) {
                    return Some(ip);
                }
            }
        }
    }
    None
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    if let Some(host) = token
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    if let Some((host, _port)) = token.rsplit_once(':') {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    None
}

fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn is_local_host_header(headers: &HeaderMap) -> bool {
    let Some(raw_host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = raw_host.trim();
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("[::1]") {
        return true;
    }
    if let Some(host_only) = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host_only.parse::<IpAddr>() {
            return is_private_or_loopback_ip(ip);
        }
    }
    let host_only = host
        .split_once(':')
        .map(|parts| parts.0)
        .unwrap_or(host)
        .trim();
    if host_only.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host_only
        .parse::<IpAddr>()
        .map(is_private_or_loopback_ip)
        .unwrap_or(false)
}

async fn admin_proxy_config_scope_view(state: &HttpState) -> AdminProxyConfigScopeView {
    match state.cluster_state.as_ref() {
        Some(cluster_state) => {
            let snapshot = cluster_state.snapshot().await;
            let is_core = snapshot.node.node_class == crate::cluster::NodeClass::Core;
            AdminProxyConfigScopeView {
                node_id: snapshot.node.node_id,
                is_core,
                can_edit_slot_metadata: is_core,
            }
        },
        None => AdminProxyConfigScopeView {
            node_id: "core".to_string(),
            is_core: true,
            can_edit_slot_metadata: true,
        },
    }
}

fn bad_request(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn forbidden(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::FORBIDDEN,
        message: message.to_string(),
    }
}

fn conflict(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::CONFLICT,
        message: message.to_string(),
    }
}

fn too_many_requests(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message: message.to_string(),
    }
}

fn not_found(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::NOT_FOUND,
        message: message.to_string(),
    }
}

fn internal_error(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};

    use super::*;

    fn test_jwt(payload: serde_json::Value) -> String {
        let encoded = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&payload).expect("serialize test jwt payload"));
        format!("header.{encoded}.sig")
    }

    fn sample_account_contribution_request(
        requester_email: &str,
    ) -> core_store::AdminAccountContributionRequest {
        core_store::AdminAccountContributionRequest {
            request_id: "req-1".to_string(),
            account_name: "codex-alpha".to_string(),
            account_id: Some("acct-alpha".to_string()),
            id_token: "id-token".to_string(),
            access_token: "access-token".to_string(),
            refresh_token: "refresh-token".to_string(),
            requester_email: requester_email.to_string(),
            contributor_message: "thanks".to_string(),
            github_id: Some("octocat".to_string()),
            frontend_page_url: Some("https://example.com/llm-access".to_string()),
            status: core_store::PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED.to_string(),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "Local".to_string(),
            admin_note: None,
            failure_reason: None,
            imported_account_name: Some("codex-alpha".to_string()),
            issued_key_id: Some("llm-key-1".to_string()),
            issued_key_name: Some("contrib-req-1".to_string()),
            created_at: 10,
            updated_at: 10,
            processed_at: Some(10),
        }
    }

    fn empty_key_patch_request() -> PatchLlmGatewayKeyRequest {
        PatchLlmGatewayKeyRequest {
            name: None,
            status: None,
            public_visible: None,
            quota_billable_limit: None,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            preferred_pool_strategy: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            request_max_concurrency_unlimited: false,
            request_min_start_interval_ms_unlimited: false,
            codex_fast_enabled: None,
            kiro_request_validation_enabled: None,
            kiro_cache_estimation_enabled: None,
            kiro_zero_cache_debug_enabled: None,
            kiro_full_request_logging_enabled: None,
            kiro_remote_media_resolution_enabled: None,
            kiro_latency_routing_enabled: None,
            kiro_protected_content_validation_enabled: None,
            kiro_cctest_text_handling_enabled: None,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    fn sample_kiro_key(policy_override_json: Option<String>) -> core_store::AdminKey {
        core_store::AdminKey {
            id: "kiro-key-test".to_string(),
            name: "Kiro test".to_string(),
            secret: "sk-test".to_string(),
            key_hash: "hash-test".to_string(),
            status: KEY_STATUS_ACTIVE.to_string(),
            provider_type: PROVIDER_KIRO.to_string(),
            public_visible: true,
            quota_billable_limit: 1_000_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            remaining_billable: 1_000_000,
            last_used_at: None,
            created_at: 10,
            updated_at: 10,
            route_strategy: Some("auto".to_string()),
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            preferred_pool_strategy: core_store::default_kiro_pool_strategy(),
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            codex_fast_enabled: true,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_remote_media_resolution_enabled: false,
            kiro_latency_routing_enabled: true,
            kiro_protected_content_validation_enabled: false,
            kiro_cctest_text_handling_enabled: false,
            kiro_cache_policy_override_json: policy_override_json,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: "{}".to_string(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json:
                core_store::default_kiro_billable_model_multipliers_json(),
            uses_global_kiro_billable_model_multipliers: true,
            kiro_candidate_credit_summary: None,
        }
    }

    fn sample_kiro_account(name: &str, remaining: f64, limit: f64) -> core_store::AdminKiroAccount {
        core_store::AdminKiroAccount {
            name: name.to_string(),
            auth_method: "oauth".to_string(),
            provider: Some("aws".to_string()),
            upstream_user_id: Some(format!("user-{name}")),
            email: None,
            expires_at: None,
            profile_arn: None,
            has_refresh_token: true,
            disabled: false,
            disabled_reason: None,
            source: None,
            source_db_path: None,
            last_imported_at: None,
            subscription_title: Some("Pro".to_string()),
            region: Some("us-east-1".to_string()),
            auth_region: Some("us-east-1".to_string()),
            api_region: Some("us-east-1".to_string()),
            machine_id: None,
            kiro_channel_max_concurrency: 1,
            kiro_channel_min_start_interval_ms: 0,
            minimum_remaining_credits_before_block: 0.0,
            manual_usage_limit: None,
            pool_strategy: core_store::default_kiro_pool_strategy(),
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "inherit".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            proxy_url: None,
            balance: Some(core_store::AdminKiroBalanceView {
                current_usage: (limit - remaining).max(0.0),
                usage_limit: limit,
                remaining,
                upstream_usage_limit: None,
                manual_usage_limit: None,
                next_reset_at: None,
                subscription_title: Some("Pro".to_string()),
                user_id: Some(format!("user-{name}")),
            }),
            cache: core_store::AdminKiroCacheView::default(),
        }
    }

    fn sample_kiro_group(id: &str, account_names: &[&str]) -> core_store::AdminAccountGroup {
        core_store::AdminAccountGroup {
            id: id.to_string(),
            provider_type: PROVIDER_KIRO.to_string(),
            name: id.to_string(),
            account_names: account_names
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            created_at: 1,
            updated_at: 1,
        }
    }

    fn sample_provider_proxy(url: &str) -> core_store::ProviderProxyConfig {
        core_store::ProviderProxyConfig {
            proxy_url: url.to_string(),
            proxy_username: None,
            proxy_password: None,
        }
    }

    #[test]
    fn normalize_probe_kiro_account_model_request_accepts_inline_proxy() {
        let request = ProbeKiroAccountModelRequest {
            model: " claude-opus-4-8 ".to_string(),
            proxy_config_id: Some("  proxy-1 ".to_string()),
            proxy_url: Some(" http://127.0.0.1:7890 ".to_string()),
            proxy_username: Some(" user ".to_string()),
            proxy_password: Some(" pass ".to_string()),
        };

        let normalized =
            normalize_probe_kiro_account_model_request(request).expect("request should normalize");

        assert_eq!(normalized.model, "claude-opus-4-8");
        assert_eq!(normalized.proxy_config_id.as_deref(), Some("proxy-1"));
        assert_eq!(
            normalized.inline_proxy,
            Some(core_store::ProviderProxyConfig {
                proxy_url: "http://127.0.0.1:7890".to_string(),
                proxy_username: Some("user".to_string()),
                proxy_password: Some("pass".to_string()),
            })
        );
    }

    #[test]
    fn normalize_probe_kiro_account_model_request_rejects_proxy_auth_without_url() {
        let err = normalize_probe_kiro_account_model_request(ProbeKiroAccountModelRequest {
            model: "claude-opus-4-8".to_string(),
            proxy_config_id: None,
            proxy_url: None,
            proxy_username: Some("user".to_string()),
            proxy_password: None,
        })
        .expect_err("inline proxy credentials without url must fail");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("proxy_url"));
    }

    #[test]
    fn build_direct_kiro_model_probe_request_preserves_requested_model_id() {
        let request = build_direct_kiro_model_probe_request(
            "claude-opus-4-8",
            Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/test".to_string()),
        );

        assert_eq!(
            request
                .conversation_state
                .current_message
                .user_input_message
                .model_id,
            "claude-opus-4-8"
        );
        assert_eq!(
            request
                .conversation_state
                .current_message
                .user_input_message
                .content,
            ADMIN_KIRO_MODEL_PROBE_PROMPT
        );
        assert_eq!(request.conversation_state.chat_trigger_type.as_deref(), Some("MANUAL"));
        assert!(request.conversation_state.history.is_empty());
        assert_eq!(
            request.profile_arn.as_deref(),
            Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/test")
        );
    }

    #[test]
    fn select_admin_kiro_probe_proxy_prefers_inline_then_proxy_config_then_resolved() {
        let inline_proxy = sample_provider_proxy("http://inline:7890");
        let proxy_config_proxy = sample_provider_proxy("http://config:7890");
        let resolved_proxy = sample_provider_proxy("http://resolved:7890");

        let (selected, source) = select_admin_kiro_probe_proxy(
            Some(inline_proxy.clone()),
            Some(proxy_config_proxy.clone()),
            Some(resolved_proxy.clone()),
        );
        assert_eq!(selected, Some(inline_proxy));
        assert_eq!(source, AdminKiroProbeProxySource::Inline);

        let (selected, source) = select_admin_kiro_probe_proxy(
            None,
            Some(proxy_config_proxy.clone()),
            Some(resolved_proxy.clone()),
        );
        assert_eq!(selected, Some(proxy_config_proxy));
        assert_eq!(source, AdminKiroProbeProxySource::ProxyConfig);

        let (selected, source) =
            select_admin_kiro_probe_proxy(None, None, Some(resolved_proxy.clone()));
        assert_eq!(selected, Some(resolved_proxy));
        assert_eq!(source, AdminKiroProbeProxySource::Resolved);

        let (selected, source) = select_admin_kiro_probe_proxy(None, None, None);
        assert_eq!(selected, None);
        assert_eq!(source, AdminKiroProbeProxySource::None);
    }

    #[test]
    fn kiro_probe_eventstream_error_message_prefers_stream_errors() {
        let message = kiro_probe_eventstream_error_message(&[
            llm_access_kiro::wire::Event::Unknown {},
            llm_access_kiro::wire::Event::Error {
                error_code: "InvalidModel".to_string(),
                error_message: r#"{"message":"model is not supported"}"#.to_string(),
            },
        ])
        .expect("stream error should be surfaced");

        assert!(message.contains("InvalidModel"));
        assert!(message.contains("model is not supported"));
    }

    #[test]
    fn normalize_key_patch_accepts_partial_kiro_cache_policy_override() {
        let mut request = empty_key_patch_request();
        request.kiro_cache_policy_override_json = Some(Some(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":50000}}"#.to_string(),
        ));

        let patch = normalize_key_patch(request).expect("partial override should be accepted");

        assert!(patch
            .kiro_cache_policy_override_json
            .as_ref()
            .and_then(|value| value.as_ref())
            .is_some_and(|json| json.contains("target_input_tokens")));
    }

    #[test]
    fn normalize_key_patch_accepts_kiro_full_request_logging_toggle() {
        let mut request = empty_key_patch_request();
        request.kiro_full_request_logging_enabled = Some(true);

        let patch = normalize_key_patch(request).expect("full request logging toggle");

        assert_eq!(patch.kiro_full_request_logging_enabled, Some(true));
    }

    #[test]
    fn normalize_key_patch_accepts_kiro_remote_media_resolution_toggle() {
        let mut request = empty_key_patch_request();
        request.kiro_remote_media_resolution_enabled = Some(true);

        let patch = normalize_key_patch(request).expect("remote media resolution toggle");

        assert_eq!(patch.kiro_remote_media_resolution_enabled, Some(true));
    }

    #[test]
    fn normalize_key_patch_accepts_kiro_protected_content_validation_toggle() {
        let mut request = empty_key_patch_request();
        request.kiro_protected_content_validation_enabled = Some(true);

        let patch = normalize_key_patch(request).expect("protected content validation toggle");

        assert_eq!(patch.kiro_protected_content_validation_enabled, Some(true));
    }

    #[test]
    fn normalize_account_patch_accepts_auto_refresh_toggle() {
        let patch = normalize_account_patch(PatchLlmGatewayAccountRequest {
            status: None,
            route_weight_tier: None,
            proxy_mode: None,
            proxy_config_id: None,
            map_gpt53_codex_to_spark: None,
            auto_refresh_enabled: Some(false),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            request_max_concurrency_unlimited: false,
            request_min_start_interval_ms_unlimited: false,
        })
        .expect("auto refresh toggle should be accepted");

        assert_eq!(patch.auto_refresh_enabled, Some(false));
    }

    #[test]
    fn effective_kiro_policy_merges_partial_override_with_global_policy() {
        let config = AdminRuntimeConfig::default();
        let keys = vec![sample_kiro_key(Some(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":50000}}"#.to_string(),
        ))];

        let keys =
            apply_effective_kiro_cache_policies(keys, &config).expect("effective policy merge");
        let policy: serde_json::Value =
            serde_json::from_str(&keys[0].effective_kiro_cache_policy_json)
                .expect("effective policy json");

        assert_eq!(policy["small_input_high_credit_boost"]["target_input_tokens"], 50_000);
        assert_eq!(policy["small_input_high_credit_boost"]["credit_start"], 1.0);
        assert!(!keys[0].uses_global_kiro_cache_policy);
    }

    #[test]
    fn apply_kiro_candidate_credit_summaries_uses_all_accounts_for_auto_pool() {
        let keys = vec![sample_kiro_key(None)];
        let accounts = vec![
            sample_kiro_account("kiro-a", 800.0, 1_000.0),
            sample_kiro_account("kiro-b", 650.0, 1_000.0),
            sample_kiro_account("kiro-c", 900.0, 1_000.0),
        ];

        let keys = apply_kiro_candidate_credit_summaries(keys, &accounts, &[]);
        let summary = keys[0]
            .kiro_candidate_credit_summary
            .expect("summary should be attached");

        assert_eq!(summary.candidate_count, 3);
        assert_eq!(summary.preferred_pool_candidate_count, 3);
        assert_eq!(summary.loaded_balance_count, 3);
        assert_eq!(summary.missing_balance_count, 0);
        assert_eq!(summary.total_limit, 3_000.0);
        assert_eq!(summary.total_remaining, 2_350.0);
    }

    #[test]
    fn apply_kiro_candidate_credit_summaries_respects_account_group_scope() {
        let mut key = sample_kiro_key(None);
        key.account_group_id = Some("group-beta".to_string());
        let accounts = vec![
            sample_kiro_account("kiro-a", 800.0, 1_000.0),
            sample_kiro_account("kiro-b", 650.0, 1_000.0),
            sample_kiro_account("kiro-c", 900.0, 1_000.0),
        ];
        let groups = vec![sample_kiro_group("group-beta", &["kiro-b", "kiro-c"])];

        let keys = apply_kiro_candidate_credit_summaries(vec![key], &accounts, &groups);
        let summary = keys[0]
            .kiro_candidate_credit_summary
            .expect("summary should be attached");

        assert_eq!(summary.candidate_count, 2);
        assert_eq!(summary.preferred_pool_candidate_count, 2);
        assert_eq!(summary.loaded_balance_count, 2);
        assert_eq!(summary.total_limit, 2_000.0);
        assert_eq!(summary.total_remaining, 1_550.0);
    }

    #[test]
    fn apply_kiro_candidate_credit_summaries_counts_preferred_pool_matches() {
        let mut key = sample_kiro_key(None);
        key.preferred_pool_strategy = core_store::KIRO_POOL_STRATEGY_CREDIT_FIRST.to_string();
        let mut credit_first = sample_kiro_account("kiro-a", 800.0, 1_000.0);
        credit_first.pool_strategy = core_store::KIRO_POOL_STRATEGY_CREDIT_FIRST.to_string();
        let accounts = vec![
            credit_first,
            sample_kiro_account("kiro-b", 650.0, 1_000.0),
            sample_kiro_account("kiro-c", 900.0, 1_000.0),
        ];

        let keys = apply_kiro_candidate_credit_summaries(vec![key], &accounts, &[]);
        let summary = keys[0]
            .kiro_candidate_credit_summary
            .expect("summary should be attached");

        assert_eq!(summary.candidate_count, 3);
        assert_eq!(summary.preferred_pool_candidate_count, 1);
    }

    #[test]
    fn runtime_config_update_accepts_duckdb_usage_runtime_settings() {
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                duckdb_usage_memory_limit_mib: Some(1024),
                duckdb_usage_checkpoint_threshold_mib: Some(32),
                usage_journal_enabled: Some(false),
                usage_journal_max_file_bytes: Some(128 * 1024 * 1024),
                usage_journal_max_file_age_ms: Some(600_000),
                usage_journal_max_files: Some(64),
                usage_journal_block_target_uncompressed_bytes: Some(2 * 1024 * 1024),
                usage_journal_block_max_events: Some(2048),
                usage_journal_fsync_interval_ms: Some(500),
                usage_journal_zstd_level: Some(5),
                usage_journal_consumer_lease_ms: Some(600_000),
                usage_journal_delete_bad_files: Some(true),
                usage_analytics_retention_days: Some(14),
                usage_query_bind_addr: Some("127.0.0.1:19091".to_string()),
                usage_query_base_url: Some("http://127.0.0.1:19091/".to_string()),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("duckdb runtime settings should be valid");

        assert_eq!(updated.duckdb_usage_memory_limit_mib, 1024);
        assert_eq!(updated.duckdb_usage_checkpoint_threshold_mib, 32);
        assert!(!updated.usage_journal_enabled);
        assert_eq!(updated.usage_journal_max_file_bytes, 128 * 1024 * 1024);
        assert_eq!(updated.usage_journal_max_file_age_ms, 600_000);
        assert_eq!(updated.usage_journal_max_files, 64);
        assert_eq!(updated.usage_journal_block_target_uncompressed_bytes, 2 * 1024 * 1024);
        assert_eq!(updated.usage_journal_block_max_events, 2048);
        assert_eq!(updated.usage_journal_fsync_interval_ms, 500);
        assert_eq!(updated.usage_journal_zstd_level, 5);
        assert_eq!(updated.usage_journal_consumer_lease_ms, 600_000);
        assert!(updated.usage_journal_delete_bad_files);
        assert_eq!(updated.usage_analytics_retention_days, 14);
        assert_eq!(updated.usage_query_bind_addr, "127.0.0.1:19091");
        assert_eq!(updated.usage_query_base_url, "http://127.0.0.1:19091");
    }

    #[test]
    fn runtime_config_update_rejects_zero_usage_analytics_retention_days() {
        let err =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                usage_analytics_retention_days: Some(0),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect_err("zero retention days should be rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("usage_analytics_retention_days"));
    }

    #[test]
    fn runtime_config_update_accepts_codex_affinity_settings() {
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                codex_session_affinity_enabled: Some(false),
                codex_session_affinity_max_entries: Some(12_345),
                codex_session_affinity_ttl_seconds: Some(7_200),
                codex_fallback_affinity_enabled: Some(false),
                codex_fallback_affinity_ttl_seconds: Some(900),
                codex_fallback_affinity_prefix_bytes: Some(2_048),
                codex_fallback_affinity_min_body_bytes: Some(64),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("codex affinity settings should be valid");

        assert!(!updated.codex_session_affinity_enabled);
        assert_eq!(updated.codex_session_affinity_max_entries, 12_345);
        assert_eq!(updated.codex_session_affinity_ttl_seconds, 7_200);
        assert!(!updated.codex_fallback_affinity_enabled);
        assert_eq!(updated.codex_fallback_affinity_ttl_seconds, 900);
        assert_eq!(updated.codex_fallback_affinity_prefix_bytes, 2_048);
        assert_eq!(updated.codex_fallback_affinity_min_body_bytes, 64);
    }

    #[test]
    fn runtime_config_update_accepts_kiro_context_usage_threshold() {
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_context_usage_min_request_tokens: Some(12_345),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("kiro context usage threshold should be valid");

        assert_eq!(updated.kiro_context_usage_min_request_tokens, 12_345);
    }

    #[test]
    fn runtime_config_update_rejects_zero_kiro_context_usage_threshold() {
        let err =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_context_usage_min_request_tokens: Some(0),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect_err("zero threshold should be rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err
            .message
            .contains("kiro_context_usage_min_request_tokens"));
    }

    #[test]
    fn runtime_config_update_accepts_kiro_compact_trigger_tokens() {
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_compact_trigger_tokens: Some(640_000),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("kiro compact trigger should be valid");

        assert_eq!(updated.kiro_compact_trigger_tokens, 640_000);
    }

    #[test]
    fn runtime_config_update_accepts_cctest_proxy_config() {
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_cctest_proxy_base_url: Some(" https://example.com/anthropic/ ".to_string()),
                kiro_cctest_proxy_api_key: Some(" sk-test ".to_string()),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("cctest proxy config should be valid");

        assert_eq!(
            updated.kiro_cctest_proxy_base_url.as_deref(),
            Some("https://example.com/anthropic")
        );
        assert_eq!(updated.kiro_cctest_proxy_api_key.as_deref(), Some("sk-test"));
    }

    #[test]
    fn runtime_config_update_rejects_invalid_cctest_proxy_url() {
        let err =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_cctest_proxy_base_url: Some("ftp://example.com".to_string()),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect_err("unsupported cctest proxy URL scheme should be rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("kiro_cctest_proxy_base_url"));
    }

    #[test]
    fn runtime_config_update_accepts_zero_kiro_compact_trigger_tokens() {
        // `0` is a valid value that disables the proactive gate.
        let updated =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_compact_trigger_tokens: Some(0),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect("zero compact trigger disables the gate and should be valid");

        assert_eq!(updated.kiro_compact_trigger_tokens, 0);
    }

    #[test]
    fn runtime_config_update_rejects_oversized_kiro_compact_trigger_tokens() {
        let err =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                kiro_compact_trigger_tokens: Some(2_000_000),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect_err("a trigger above the cap should be rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("kiro_compact_trigger_tokens"));
    }

    #[test]
    fn usage_journal_file_lists_split_current_and_orphan_files() {
        let journal = JournalStatusSnapshot {
            journal_enabled: true,
            journal_root: "/tmp/journal".to_string(),
            active_file_sequence: Some(9),
            active_file_bytes: 4096,
            ..JournalStatusSnapshot::default()
        };
        let files = JournalFileListsSnapshot {
            active: vec![
                JournalFileSnapshot {
                    file_name: "usage-000000000007.open".to_string(),
                    path: "/tmp/journal/active/usage-000000000007.open".to_string(),
                    sequence: Some(7),
                    bytes: 1024,
                    age_ms: Some(1000),
                },
                JournalFileSnapshot {
                    file_name: "usage-000000000009.open".to_string(),
                    path: "/tmp/journal/active/usage-000000000009.open".to_string(),
                    sequence: Some(9),
                    bytes: 4096,
                    age_ms: Some(200),
                },
            ],
            consuming: vec![
                JournalFileSnapshot {
                    file_name: "usage-000000000003.journal".to_string(),
                    path: "/tmp/journal/consuming/usage-000000000003.journal".to_string(),
                    sequence: Some(3),
                    bytes: 8192,
                    age_ms: Some(3000),
                },
                JournalFileSnapshot {
                    file_name: "usage-000000000004.journal".to_string(),
                    path: "/tmp/journal/consuming/usage-000000000004.journal".to_string(),
                    sequence: Some(4),
                    bytes: 16384,
                    age_ms: Some(4000),
                },
            ],
            ..JournalFileListsSnapshot::default()
        };
        let worker = AdminUsageWorkerProgressView {
            state: "importing".to_string(),
            current_file_path: Some(
                "/tmp/journal/consuming/usage-000000000004.journal".to_string(),
            ),
            current_file_sequence: Some(4),
            total_compressed_bytes: 16384,
            ..AdminUsageWorkerProgressView::default()
        };

        let partitioned = partition_usage_journal_files(&journal, &files, &worker);

        assert_eq!(
            partitioned
                .producer_current_file
                .as_ref()
                .and_then(|file| file.sequence),
            Some(9)
        );
        assert_eq!(partitioned.orphan_active_files.len(), 1);
        assert_eq!(partitioned.orphan_active_files[0].sequence, Some(7));
        assert_eq!(
            partitioned
                .current_consuming_file
                .as_ref()
                .and_then(|file| file.sequence),
            Some(4)
        );
        assert_eq!(partitioned.orphan_consuming_files.len(), 1);
        assert_eq!(partitioned.orphan_consuming_files[0].sequence, Some(3));
    }

    #[test]
    fn proxied_usage_list_body_preserves_api_process_activity_counters() {
        let body = br#"{
            "total": 0,
            "offset": 0,
            "limit": 20,
            "has_more": false,
            "current_rpm": 0,
            "current_in_flight": 0,
            "events": [],
            "generated_at": 1700000000000
        }"#;
        let activity = crate::activity::RequestActivitySnapshot {
            rpm: 7,
            in_flight: 2,
        };

        let overlaid =
            overlay_usage_activity_response_body(body, activity).expect("usage list overlay");
        let value: serde_json::Value =
            serde_json::from_slice(&overlaid).expect("overlaid response json");

        assert_eq!(value["current_rpm"], 7);
        assert_eq!(value["current_in_flight"], 2);
    }

    #[test]
    fn usage_activity_key_id_comes_from_query_string() {
        let uri: Uri = "/admin/llm-gateway/usage?limit=20&key_id=key-a"
            .parse()
            .expect("uri");

        assert_eq!(usage_activity_key_id_from_uri(&uri).as_deref(), Some("key-a"));
    }

    #[test]
    fn runtime_config_update_rejects_too_small_duckdb_checkpoint_threshold() {
        let err =
            apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
                duckdb_usage_checkpoint_threshold_mib: Some(8),
                ..UpdateAdminRuntimeConfig::default()
            })
            .expect_err("checkpoint threshold below 16 MiB should be rejected");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err
            .message
            .contains("duckdb_usage_checkpoint_threshold_mib"));
    }

    #[test]
    fn admin_codex_accounts_include_cached_rate_limits_and_usage_errors() {
        let accounts = vec![core_store::AdminCodexAccount {
            name: "alpha".to_string(),
            status: "active".to_string(),
            account_id: Some("acct-alpha".to_string()),
            plan_type: None,
            route_weight_tier: "auto".to_string(),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            request_max_concurrency: Some(3),
            request_min_start_interval_ms: Some(1000),
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
            effective_proxy_config_name: Some("us-home1".to_string()),
            last_refresh: Some(900),
            access_token_expires_at: Some(1_800),
            auth_refresh_error_message: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }];
        let status = core_store::CodexRateLimitStatus {
            status: "degraded".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1200),
            last_success_at: Some(1100),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![core_store::CodexPublicAccountStatus {
                name: "alpha".to_string(),
                status: "active".to_string(),
                plan_type: Some("Pro".to_string()),
                primary_remaining_percent: Some(62.0),
                secondary_remaining_percent: Some(39.0),
                last_usage_checked_at: Some(1200),
                last_usage_success_at: Some(1100),
                usage_error_message: Some("upstream 503".to_string()),
            }],
            buckets: Vec::new(),
        };

        let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

        assert_eq!(accounts[0].plan_type.as_deref(), Some("Pro"));
        assert_eq!(accounts[0].primary_remaining_percent, Some(62.0));
        assert_eq!(accounts[0].secondary_remaining_percent, Some(39.0));
        assert_eq!(accounts[0].last_refresh, Some(900));
        assert_eq!(accounts[0].last_usage_checked_at, Some(1200));
        assert_eq!(accounts[0].last_usage_success_at, Some(1100));
        assert_eq!(accounts[0].usage_error_message.as_deref(), Some("upstream 503"));
    }

    #[test]
    fn disabled_admin_codex_accounts_do_not_keep_cached_rate_limits() {
        let accounts = vec![core_store::AdminCodexAccount {
            name: "alpha".to_string(),
            status: "disabled".to_string(),
            account_id: Some("acct-alpha".to_string()),
            plan_type: None,
            route_weight_tier: "auto".to_string(),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            request_max_concurrency: Some(3),
            request_min_start_interval_ms: Some(1000),
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
            effective_proxy_config_name: Some("us-home1".to_string()),
            last_refresh: Some(900),
            access_token_expires_at: Some(1_800),
            auth_refresh_error_message: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }];
        let status = core_store::CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1200),
            last_success_at: Some(1100),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![core_store::CodexPublicAccountStatus {
                name: "alpha".to_string(),
                status: "active".to_string(),
                plan_type: Some("Pro".to_string()),
                primary_remaining_percent: Some(62.0),
                secondary_remaining_percent: Some(39.0),
                last_usage_checked_at: Some(1200),
                last_usage_success_at: Some(1100),
                usage_error_message: None,
            }],
            buckets: Vec::new(),
        };

        let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

        assert_eq!(accounts[0].status, "disabled");
        assert_eq!(accounts[0].plan_type, None);
        assert_eq!(accounts[0].primary_remaining_percent, None);
        assert_eq!(accounts[0].secondary_remaining_percent, None);
    }

    #[test]
    fn admin_codex_accounts_keep_newer_local_error_until_status_catches_up() {
        let accounts = vec![core_store::AdminCodexAccount {
            name: "alpha".to_string(),
            status: "active".to_string(),
            account_id: Some("acct-alpha".to_string()),
            plan_type: None,
            route_weight_tier: "auto".to_string(),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            request_max_concurrency: Some(3),
            request_min_start_interval_ms: Some(1000),
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
            effective_proxy_config_name: Some("us-home1".to_string()),
            last_refresh: Some(1300),
            access_token_expires_at: Some(1_800),
            auth_refresh_error_message: Some(
                "codex refresh token returned 401 Unauthorized: \
                 {\"error\":{\"code\":\"refresh_token_reused\"}}"
                    .to_string(),
            ),
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }];
        let status = core_store::CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 300,
            last_checked_at: Some(1200),
            last_success_at: Some(1200),
            source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
            error_message: None,
            accounts: vec![core_store::CodexPublicAccountStatus {
                name: "alpha".to_string(),
                status: "active".to_string(),
                plan_type: Some("Pro".to_string()),
                primary_remaining_percent: Some(62.0),
                secondary_remaining_percent: Some(39.0),
                last_usage_checked_at: Some(1200),
                last_usage_success_at: Some(1200),
                usage_error_message: None,
            }],
            buckets: Vec::new(),
        };

        let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

        assert_eq!(accounts[0].plan_type.as_deref(), Some("Pro"));
        assert_eq!(accounts[0].last_refresh, Some(1300));
        assert_eq!(accounts[0].usage_error_message, None);
        assert_eq!(
            accounts[0].auth_refresh_error_message.as_deref(),
            Some(
                "codex refresh token returned 401 Unauthorized: \
                 {\"error\":{\"code\":\"refresh_token_reused\"}}"
            )
        );
    }

    #[test]
    fn imported_codex_auth_accepts_partial_and_preserves_raw_json() {
        let raw = serde_json::json!({
            "tokens": {
                "refreshToken": " refresh-token ",
                "accountId": "acct-1"
            },
            "device_id": "device-1"
        });

        let auth = normalize_imported_codex_auth(Some(raw), None).expect("normalize auth json");
        let stored: serde_json::Value =
            serde_json::from_str(&auth.auth_json).expect("stored auth json");

        assert_eq!(auth.account_id.as_deref(), Some("acct-1"));
        assert_eq!(auth.refresh_token.as_deref(), Some("refresh-token"));
        assert_eq!(auth.id_token, None);
        assert_eq!(auth.access_token, None);
        assert_eq!(stored["device_id"], "device-1");
        assert_eq!(stored["tokens"]["refreshToken"], " refresh-token ");
    }

    #[test]
    fn imported_codex_auth_extracts_principal_id_from_jwt_claims() {
        let auth = normalize_imported_codex_auth(
            Some(serde_json::json!({
                "id_token": test_jwt(serde_json::json!({
                    "sub": "subject-1"
                })),
                "access_token": test_jwt(serde_json::json!({
                    "sub": "subject-2"
                })),
                "account_id": "acct-1"
            })),
            None,
        )
        .expect("normalize auth json");

        assert_eq!(auth.principal_id.as_deref(), Some("subject-1"));
    }

    #[test]
    fn codex_import_validation_prefers_present_access_token() {
        let auth = normalize_imported_codex_auth(
            Some(serde_json::json!({
                "access_token": "access-token",
                "refresh_token": "refresh-token",
                "account_id": "acct-1"
            })),
            None,
        )
        .expect("normalize auth json");

        assert!(should_validate_codex_access_token_directly(&auth));
    }

    #[test]
    fn codex_access_token_validation_requires_models_payload() {
        validate_codex_models_probe_payload(&serde_json::json!({
            "models": [{"slug": "gpt-5.5"}]
        }))
        .expect("models payload should validate");

        let err = validate_codex_models_probe_payload(&serde_json::json!({"models": []}))
            .expect_err("empty models should not validate");
        assert!(err.to_string().contains("models array"));
    }

    #[test]
    fn codex_batch_import_request_rejects_empty_items() {
        let err = normalize_codex_batch_import_request(CreateCodexBatchImportJobRequest {
            provider_type: "codex".to_string(),
            source_type: "local_json".to_string(),
            validate_before_import: false,
            items: Vec::new(),
        })
        .expect_err("empty items must fail");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("items"));
    }

    #[test]
    fn codex_batch_import_item_reuses_auth_json_normalization() {
        let normalized = normalize_codex_batch_import_request(CreateCodexBatchImportJobRequest {
            provider_type: "codex".to_string(),
            source_type: "local_json".to_string(),
            validate_before_import: false,
            items: vec![CreateCodexBatchImportJobItemRequest {
                name: "codex_primary".to_string(),
                tokens: None,
                auth_json: Some(serde_json::json!({
                    "tokens": {
                        "refreshToken": " refresh-token ",
                        "accountId": "acct-1"
                    },
                    "device_id": "device-1"
                })),
            }],
        })
        .expect("normalize batch request");

        assert_eq!(normalized.items.len(), 1);
        assert_eq!(normalized.items[0].requested_name, "codex_primary");
        assert_eq!(normalized.items[0].requested_account_id.as_deref(), Some("acct-1"));
        assert!(normalized.items[0].raw_auth_json.contains("device_id"));
    }

    #[test]
    fn codex_batch_import_normalization_distinguishes_same_account_id_by_principal() {
        let normalized = normalize_codex_batch_import_request(CreateCodexBatchImportJobRequest {
            provider_type: "codex".to_string(),
            source_type: "local_json".to_string(),
            validate_before_import: false,
            items: vec![
                CreateCodexBatchImportJobItemRequest {
                    name: "codex_alpha".to_string(),
                    tokens: None,
                    auth_json: Some(serde_json::json!({
                        "account_id": "acct-shared",
                        "access_token": test_jwt(serde_json::json!({
                            "sub": "subject-alpha"
                        }))
                    })),
                },
                CreateCodexBatchImportJobItemRequest {
                    name: "codex_beta".to_string(),
                    tokens: None,
                    auth_json: Some(serde_json::json!({
                        "account_id": "acct-shared",
                        "access_token": test_jwt(serde_json::json!({
                            "sub": "subject-beta"
                        }))
                    })),
                },
            ],
        })
        .expect("normalize batch request");

        assert_eq!(normalized.items[0].requested_account_id.as_deref(), Some("acct-shared"));
        assert_eq!(normalized.items[1].requested_account_id.as_deref(), Some("acct-shared"));
        assert_eq!(normalized.items[0].auth.principal_id.as_deref(), Some("subject-alpha"));
        assert_eq!(normalized.items[1].auth.principal_id.as_deref(), Some("subject-beta"));
    }

    #[test]
    fn account_contribution_issue_email_policy_skips_blank_recipient() {
        let request = sample_account_contribution_request("   ");

        assert_eq!(
            account_contribution_issue_email_policy(&request, false),
            AccountContributionIssueEmailPolicy::SkipNoRecipient
        );
    }

    #[test]
    fn account_contribution_issue_email_policy_skips_when_notifier_missing() {
        let request = sample_account_contribution_request("user@example.com");

        assert_eq!(
            account_contribution_issue_email_policy(&request, false),
            AccountContributionIssueEmailPolicy::SkipNoNotifier
        );
    }

    #[test]
    fn account_contribution_issue_email_policy_sends_when_recipient_and_notifier_exist() {
        let request = sample_account_contribution_request("user@example.com");

        assert_eq!(
            account_contribution_issue_email_policy(&request, true),
            AccountContributionIssueEmailPolicy::Send
        );
    }

    #[test]
    fn account_contribution_access_artifacts_skip_blank_recipient() {
        let request = sample_account_contribution_request("   ");

        assert!(!should_issue_account_contribution_access_artifacts(&request));
    }

    #[test]
    fn account_contribution_access_artifacts_keep_email_backed_issue_flow() {
        let request = sample_account_contribution_request("user@example.com");

        assert!(should_issue_account_contribution_access_artifacts(&request));
    }

    #[test]
    fn normalize_kiro_key_patch_accepts_preferred_pool_strategy() {
        let patch = normalize_kiro_key_patch(PatchLlmGatewayKeyRequest {
            preferred_pool_strategy: Some("credit_first".to_string()),
            ..empty_key_patch_request()
        })
        .expect("kiro key patch should normalize");

        assert_eq!(
            patch.preferred_pool_strategy.as_deref(),
            Some(core_store::KIRO_POOL_STRATEGY_CREDIT_FIRST)
        );
    }

    #[test]
    fn normalize_kiro_account_patch_accepts_pool_strategy() {
        let patch = normalize_kiro_account_patch(PatchKiroAccountRequest {
            pool_strategy: Some("balanced".to_string()),
            status: None,
            kiro_channel_max_concurrency: None,
            kiro_channel_min_start_interval_ms: None,
            minimum_remaining_credits_before_block: None,
            manual_usage_limit: None,
            proxy_mode: None,
            proxy_config_id: None,
        })
        .expect("kiro account patch should normalize");

        assert_eq!(patch.pool_strategy.as_deref(), Some(core_store::KIRO_POOL_STRATEGY_BALANCED));
    }

    #[test]
    fn normalize_kiro_account_patch_accepts_manual_usage_limit_updates() {
        let patch = normalize_kiro_account_patch(PatchKiroAccountRequest {
            manual_usage_limit: Some(Some(500.0)),
            status: None,
            kiro_channel_max_concurrency: None,
            kiro_channel_min_start_interval_ms: None,
            minimum_remaining_credits_before_block: None,
            pool_strategy: None,
            proxy_mode: None,
            proxy_config_id: None,
        })
        .expect("manual Kiro usage limit should normalize");

        assert_eq!(patch.manual_usage_limit, Some(Some(500.0)));

        let patch = normalize_kiro_account_patch(PatchKiroAccountRequest {
            manual_usage_limit: Some(None),
            status: None,
            kiro_channel_max_concurrency: None,
            kiro_channel_min_start_interval_ms: None,
            minimum_remaining_credits_before_block: None,
            pool_strategy: None,
            proxy_mode: None,
            proxy_config_id: None,
        })
        .expect("clearing manual Kiro usage limit should normalize");

        assert_eq!(patch.manual_usage_limit, Some(None));
    }

    #[test]
    fn patch_kiro_account_request_deserializes_manual_usage_limit_clear() {
        let request = serde_json::from_value::<PatchKiroAccountRequest>(serde_json::json!({
            "manual_usage_limit": null
        }))
        .expect("manual usage limit null should deserialize");
        assert_eq!(request.manual_usage_limit, Some(None));

        let request = serde_json::from_value::<PatchKiroAccountRequest>(serde_json::json!({}))
            .expect("missing manual usage limit should deserialize");
        assert_eq!(request.manual_usage_limit, None);
    }

    #[test]
    fn normalize_kiro_account_patch_rejects_invalid_manual_usage_limit() {
        let err = normalize_kiro_account_patch(PatchKiroAccountRequest {
            manual_usage_limit: Some(Some(-1.0)),
            status: None,
            kiro_channel_max_concurrency: None,
            kiro_channel_min_start_interval_ms: None,
            minimum_remaining_credits_before_block: None,
            pool_strategy: None,
            proxy_mode: None,
            proxy_config_id: None,
        })
        .expect_err("negative manual Kiro usage limit should be rejected");

        assert!(err.message.contains("manual_usage_limit"));
    }

    #[test]
    fn kiro_auth_from_import_request_preserves_import_metadata() {
        let auth = kiro_auth_from_import_request(ImportParsedKiroAccountRequest {
            auth: KiroAuthRequestFields {
                name: " org-main ".to_string(),
                access_token: Some(" access-token ".to_string()),
                refresh_token: Some(" refresh-token ".to_string()),
                profile_arn: Some(
                    " arn:aws:iam::123456789012:role/OrganizationProfile ".to_string(),
                ),
                expires_at: Some(" 2032-03-04T05:06:07Z ".to_string()),
                auth_method: Some(" IDC ".to_string()),
                client_id: Some(" client-id ".to_string()),
                client_secret: Some(" client-secret ".to_string()),
                region: Some(" eu-west-1 ".to_string()),
                auth_region: None,
                api_region: None,
                machine_id: None,
                provider: Some(" aws ".to_string()),
                email: None,
                subscription_title: None,
                kiro_channel_max_concurrency: Some(32),
                kiro_channel_min_start_interval_ms: Some(123),
                minimum_remaining_credits_before_block: Some(10.0),
                manual_usage_limit: Some(Some(500.0)),
                pool_strategy: Some(core_store::KIRO_POOL_STRATEGY_CREDIT_FIRST.to_string()),
                disabled: false,
            },
            source_db_path: Some(" /home/ubuntu/.local/share/kiro-cli/data.sqlite3 ".to_string()),
            last_imported_at: Some(1_780_988_532_000),
        })
        .expect("import request should normalize");

        assert_eq!(auth.name, "org-main");
        assert_eq!(auth.auth_method(), "idc");
        assert_eq!(auth.provider.as_deref(), Some("aws"));
        assert_eq!(auth.region.as_deref(), Some("eu-west-1"));
        assert_eq!(auth.source.as_deref(), Some("kiro-cli"));
        assert_eq!(
            auth.source_db_path.as_deref(),
            Some("/home/ubuntu/.local/share/kiro-cli/data.sqlite3")
        );
        assert_eq!(auth.last_imported_at, Some(1_780_988_532_000));
        assert_eq!(auth.kiro_channel_max_concurrency, Some(32));
        assert_eq!(auth.kiro_channel_min_start_interval_ms, Some(123));
        assert_eq!(auth.manual_usage_limit, Some(500.0));
        assert_eq!(
            auth.pool_strategy.as_deref(),
            Some(core_store::KIRO_POOL_STRATEGY_CREDIT_FIRST)
        );
    }
}
