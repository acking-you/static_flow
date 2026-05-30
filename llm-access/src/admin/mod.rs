//! Local admin endpoints for the standalone LLM access service.
//! ## Module map
//!
//! `admin.rs` is the facade for the local admin endpoints. It keeps the
//! request/ response DTOs and the few small `impl` blocks (`AdminHttpError`,
//! `AdminKiroProbeProxySource`, `NormalizedCodexAuth`), the validation-bound
//! constants, and the tests. The handler + helper free functions are grouped by
//! admin domain into descendant submodules:
//!
//! ```text
//!  /admin/* request
//!    +-- [auth]            admin-token gate + client-IP extraction
//!    +-- [config]          runtime/gateway config + field validators
//!    +-- [keys]            API key CRUD + Kiro credit summaries
//!    +-- [groups]          account-group CRUD (both planes)
//!    +-- [proxy]           proxy config/binding CRUD + connectivity checks
//!    +-- [codex_accounts]  Codex account lifecycle + import jobs + auth
//!    +-- [kiro_accounts]   Kiro account lifecycle + cache/balance/probe
//!    +-- [usage]           usage events/metrics + journal/worker inspection
//!    +-- [review_queue]    token/contribution/sponsor review + issuance
//!    +-- [util]            id/secret/hash + normalization + error builders
//! ```

pub(crate) use std::{
    collections::{BTreeMap, HashSet},
    fs,
    net::{IpAddr, SocketAddr},
    path::{Path as FsPath, PathBuf},
    time::{Duration, Instant},
};

pub(crate) use anyhow::Context;
pub(crate) use axum::{
    body::{to_bytes, Body},
    extract::{OriginalUri, Path, Query, State},
    http::{header, HeaderMap, Method, Request, StatusCode, Uri},
    response::{IntoResponse, Response},
    Json,
};
pub(crate) use llm_access_core::{
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
pub(crate) use llm_access_kiro::{
    auth_file::KiroAuthRecord,
    cache_policy::{
        parse_kiro_cache_policy_override_json, resolve_effective_kiro_cache_policy,
        uses_global_kiro_cache_policy, KiroCachePolicy,
    },
    cache_sim::{KiroCacheRuntimeStats, KiroCacheSimulationConfig, KiroCacheSimulationMode},
    local_import,
};
pub(crate) use llm_usage_journal::{
    collect_journal_file_lists, JournalFileListsSnapshot, JournalFileSnapshot,
    JournalPreviewReader, JournalPreviewReport, JournalStatusSnapshot, WorkerProgressSnapshot,
};
pub(crate) use serde::{Deserialize, Serialize};
pub(crate) use sha2::{Digest, Sha256};
pub(crate) use tokio::sync::OwnedSemaphorePermit;

pub(crate) use crate::{
    activity::RequestActivitySnapshot,
    codex_refresh, codex_status, kiro_refresh, kiro_status,
    process_memory::{read_current_process_memory_stats, ProcessMemoryStats},
    provider, HttpState,
};

mod auth;
mod codex_accounts;
mod config;
mod groups;
mod keys;
mod kiro_accounts;
mod proxy;
mod review_queue;
mod usage;
mod util;

pub(crate) use auth::*;
pub(crate) use codex_accounts::*;
pub(crate) use config::*;
pub(crate) use groups::*;
pub(crate) use keys::*;
pub(crate) use kiro_accounts::*;
pub(crate) use proxy::*;
pub(crate) use review_queue::*;
pub(crate) use usage::*;
pub(crate) use util::*;

const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
const MAX_RUNTIME_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MIN_RUNTIME_REQUEST_BODY_BYTES: u64 = 1024;
const MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 100;
const MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 0;
const MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 240;
const MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 3_600;
const MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS: u64 = 60;
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
pub(crate) struct ErrorResponse {
    error: String,
    code: u16,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminKeysResponse {
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
pub(crate) struct DeleteResponse {
    deleted: bool,
    id: String,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminAccountGroupsResponse {
    groups: Vec<core_store::AdminAccountGroup>,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminAccountGroupOptionsResponse {
    options: Vec<core_store::AdminAccountGroupOption>,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminProxyConfigsResponse {
    proxy_config_scope: AdminProxyConfigScopeView,
    proxy_configs: Vec<core_store::AdminProxyConfig>,
    generated_at: i64,
}
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AdminProxyConfigScopeView {
    node_id: String,
    is_core: bool,
    can_edit_slot_metadata: bool,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminProxyBindingsResponse {
    bindings: Vec<core_store::AdminProxyBinding>,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminAccountsResponse {
    accounts: Vec<core_store::AdminCodexAccount>,
    summary: core_store::AdminAccountsSummary,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminCodexModelsProbeResponse {
    ok: bool,
    message: String,
    checked_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminCodexImportJobsResponse {
    jobs: Vec<core_store::AdminCodexImportJobSummary>,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminKiroAccountsResponse {
    accounts: Vec<core_store::AdminKiroAccount>,
    summary: core_store::AdminAccountsSummary,
    total: usize,
    limit: usize,
    offset: usize,
    has_more: bool,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminKiroAccountStatusesResponse {
    accounts: Vec<core_store::AdminKiroAccount>,
    total: usize,
    limit: usize,
    offset: usize,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminKiroCacheStatsResponse {
    #[serde(flatten)]
    stats: KiroCacheRuntimeStats,
    process_memory: ProcessMemoryStats,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminKiroModelProbeResponse {
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
pub(crate) struct AdminTokenRequestsResponse {
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
pub(crate) struct AdminAccountContributionRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminAccountContributionRequest>,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminSponsorRequestsResponse {
    total: usize,
    offset: usize,
    limit: usize,
    has_more: bool,
    requests: Vec<core_store::AdminSponsorRequest>,
    generated_at: i64,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminUsageJournalStatusResponse {
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
pub(crate) struct AdminClusterNodeStatusView {
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
pub(crate) struct AdminUsageJournalPreviewResponse {
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
pub(crate) struct AdminUsageJournalPreviewFileView {
    path: String,
    file_sequence: u64,
    bytes_scanned: u64,
    complete_blocks: u64,
    truncated_tail: bool,
    total_events: usize,
    events: Vec<AdminUsageJournalPreviewEventView>,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminUsageJournalPreviewEventView {
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
pub(crate) struct AdminUsageJournalFileView {
    file_name: String,
    path: String,
    sequence: Option<u64>,
    bytes: u64,
    age_ms: Option<i64>,
}
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct AdminUsageWorkerProgressView {
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
pub(crate) struct PartitionedUsageJournalFiles {
    producer_current_file: Option<AdminUsageJournalFileView>,
    orphan_active_files: Vec<AdminUsageJournalFileView>,
    current_consuming_file: Option<AdminUsageJournalFileView>,
    orphan_consuming_files: Vec<AdminUsageJournalFileView>,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminProxyCheckTargetView {
    target: String,
    url: String,
    reachable: bool,
    status_code: Option<u16>,
    latency_ms: i64,
    error_message: Option<String>,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminProxyCheckResponse {
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
pub(crate) enum AdminProxyCheckMode {
    Connectivity,
    FullChain,
}
#[derive(Debug, Serialize)]
pub(crate) struct AdminLegacyKiroProxyMigrationResponse {
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
pub(crate) struct CreateManualKiroAccountRequest {
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
    disabled: bool,
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
    #[serde(default)]
    proxy_mode: Option<String>,
    #[serde(default)]
    proxy_config_id: Option<String>,
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
pub(crate) struct NormalizedProbeKiroAccountModelRequest {
    model: String,
    proxy_config_id: Option<String>,
    inline_proxy: Option<core_store::ProviderProxyConfig>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AdminKiroProbeProxySource {
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
pub(crate) struct AdminHttpError {
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountContributionIssueEmailPolicy {
    SkipNoRecipient,
    SkipNoNotifier,
    Send,
}
#[derive(Default)]
pub(crate) struct ActiveJournalStats {
    file_sequence: Option<u64>,
    bytes: u64,
}
#[derive(Default)]
pub(crate) struct JournalDirStats {
    file_count: u64,
    bytes: u64,
    oldest_age_ms: Option<i64>,
}
#[derive(Debug, Default, Deserialize)]
pub(crate) struct WorkerStatusSnapshot {
    #[serde(flatten)]
    progress: WorkerProgressSnapshot,
    #[serde(default)]
    process_memory: ProcessMemoryStats,
}
pub(crate) struct ProxyFullChainProbeSpec {
    key_name: &'static str,
    path: &'static str,
    body: Vec<u8>,
}
#[derive(Debug, Clone)]
pub(crate) struct NormalizedCodexAuth {
    auth_json: String,
    account_id: Option<String>,
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
pub(crate) struct NormalizedCodexBatchImportJobRequest {
    provider_type: String,
    source_type: String,
    validate_before_import: bool,
    items: Vec<NormalizedCodexBatchImportJobItem>,
}
#[derive(Debug, Clone)]
pub(crate) struct NormalizedCodexBatchImportJobItem {
    item_index: usize,
    requested_name: String,
    requested_account_id: Option<String>,
    raw_auth_json: String,
    auth: NormalizedCodexAuth,
}

#[cfg(test)]
mod tests;
