//! Core persisted record types for the LLM gateway LanceDB store.
//!
//! These structs are intentionally storage-oriented: they mirror table rows
//! closely and are shared by backend admin handlers, runtime accounting, and
//! migration code.

use std::collections::BTreeMap;

use chrono::Utc;
use serde::{Deserialize, Serialize};

pub const LLM_GATEWAY_KEYS_TABLE: &str = "llm_gateway_keys";
pub const LLM_GATEWAY_USAGE_EVENTS_TABLE: &str = "llm_gateway_usage_events";
pub const LLM_GATEWAY_RUNTIME_CONFIG_TABLE: &str = "llm_gateway_runtime_config";
pub const LLM_GATEWAY_PROXY_CONFIGS_TABLE: &str = "llm_gateway_proxy_configs";
pub const LLM_GATEWAY_PROXY_BINDINGS_TABLE: &str = "llm_gateway_proxy_bindings";
pub const LLM_GATEWAY_TOKEN_REQUESTS_TABLE: &str = "llm_gateway_token_requests";
pub const LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE: &str =
    "llm_gateway_account_contribution_requests";
pub const LLM_GATEWAY_SPONSOR_REQUESTS_TABLE: &str = "llm_gateway_sponsor_requests";

pub const LLM_GATEWAY_TABLE_NAMES: &[&str] = &[
    LLM_GATEWAY_KEYS_TABLE,
    LLM_GATEWAY_USAGE_EVENTS_TABLE,
    LLM_GATEWAY_RUNTIME_CONFIG_TABLE,
    LLM_GATEWAY_PROXY_CONFIGS_TABLE,
    LLM_GATEWAY_PROXY_BINDINGS_TABLE,
    LLM_GATEWAY_TOKEN_REQUESTS_TABLE,
    LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
    LLM_GATEWAY_SPONSOR_REQUESTS_TABLE,
];

pub const LLM_GATEWAY_KEY_STATUS_ACTIVE: &str = "active";
pub const LLM_GATEWAY_KEY_STATUS_DISABLED: &str = "disabled";
/// Provider type for Codex-based gateway keys (uses OpenAI-compatible
/// protocol).
pub const LLM_GATEWAY_PROVIDER_CODEX: &str = "codex";
/// Provider type for Kiro-based gateway keys (uses Anthropic-compatible
/// protocol).
pub const LLM_GATEWAY_PROVIDER_KIRO: &str = "kiro";
/// Protocol family identifier for OpenAI-compatible API endpoints.
pub const LLM_GATEWAY_PROTOCOL_OPENAI: &str = "openai";
/// Protocol family identifier for Anthropic-compatible API endpoints.
pub const LLM_GATEWAY_PROTOCOL_ANTHROPIC: &str = "anthropic";
pub const DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS: u64 = 60;
/// Default maximum request body size (8 MiB) enforced by the gateway proxy
/// layer.
pub const DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Allow a few transient upstream failures before one Codex account is marked
/// unavailable for routing.
pub const DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 3;
/// Default Kiro upstream channel concurrency. `1` serializes requests to avoid
/// bursty Claude Code traffic against the undocumented 5-minute credit window.
pub const DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 1;
/// Default spacing between Kiro upstream request starts, in milliseconds.
///
/// We intentionally default to `0` and rely on channel serialization first,
/// because Kiro does not publish a stable RPM/TPM contract for Student plans.
pub const DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 0;
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING: &str = "pending";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED: &str = "issued";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED: &str = "rejected";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED: &str = "failed";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_SUBMITTED: &str = "submitted";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT: &str = "payment_email_sent";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED: &str = "approved";

/// Persisted gateway API key row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayKeyRecord {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub key_hash: String,
    pub status: String,
    /// Upstream provider this key targets (e.g. `"codex"`, `"kiro"`).
    pub provider_type: String,
    /// Wire protocol family used when proxying requests (e.g. `"openai"`,
    /// `"anthropic"`).
    pub protocol_family: String,
    pub public_visible: bool,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_billable_tokens: u64,
    /// Exact cumulative Kiro credits consumed by this key when the upstream
    /// emitted authoritative metering data.
    pub usage_credit_total: f64,
    /// Number of Kiro usage events for this key whose credit metering was not
    /// present in the upstream response.
    pub usage_credit_missing_events: u64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub route_strategy: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names: Option<Vec<String>>,
    /// Optional per-key model rewrite map.
    ///
    /// When present, a request asking for model `key` is rewritten to model
    /// `value` before the provider-specific adapter runs. Identity entries are
    /// intentionally omitted from storage.
    pub model_name_map: Option<BTreeMap<String, String>>,
    /// Optional per-key cap on concurrent in-flight Codex gateway requests.
    ///
    /// `None` means unlimited.
    pub request_max_concurrency: Option<u64>,
    /// Optional minimum milliseconds between consecutive Codex request starts
    /// for this key.
    ///
    /// `None` means unlimited/no pacing constraint.
    pub request_min_start_interval_ms: Option<u64>,
    /// Whether Kiro requests using this key should run strict local request
    /// validation before conversion and proxying.
    pub kiro_request_validation_enabled: bool,
}

impl LlmGatewayKeyRecord {
    pub fn billable_used(&self) -> u64 {
        self.usage_billable_tokens
    }

    pub fn remaining_billable(&self) -> i64 {
        self.quota_billable_limit as i64 - self.billable_used() as i64
    }
}

/// Stores one settled gateway call after the final token usage is known.
///
/// The record intentionally keeps both billing fields and request diagnostics
/// so the admin UI can answer "who spent quota" and "what exactly was sent".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayUsageEventRecord {
    pub id: String,
    pub key_id: String,
    pub key_name: String,
    /// Provider that served this request, copied from the key at call time.
    pub provider_type: String,
    pub account_name: Option<String>,
    pub request_method: String,
    pub request_url: String,
    pub latency_ms: i32,
    pub endpoint: String,
    pub model: Option<String>,
    pub status_code: i32,
    pub input_uncached_tokens: u64,
    pub input_cached_tokens: u64,
    pub output_tokens: u64,
    pub billable_tokens: u64,
    pub usage_missing: bool,
    /// Exact Kiro credits consumed by this request when reported by upstream
    /// metering. Absent for providers that do not emit this signal.
    pub credit_usage: Option<f64>,
    /// Whether credit usage was expected but unavailable for this event.
    pub credit_usage_missing: bool,
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub last_message_content: Option<String>,
    pub created_at: i64,
}

/// Per-key usage totals aggregated from `llm_gateway_usage_events`.
///
/// These values are derived data rather than the source of truth. The gateway
/// rebuilds them from immutable usage events on startup and then maintains them
/// incrementally in memory for real-time quota enforcement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct LlmGatewayKeyUsageRollupRecord {
    /// The gateway key this rollup belongs to.
    pub key_id: String,
    /// Sum of non-cached input tokens across all events for this key.
    pub input_uncached_tokens: u64,
    /// Sum of cached (prompt-cache hit) input tokens.
    pub input_cached_tokens: u64,
    /// Sum of output (completion) tokens.
    pub output_tokens: u64,
    /// Sum of billable tokens (the quota-relevant metric).
    pub billable_tokens: u64,
    /// Accumulated Kiro credit cost (only meaningful for `provider_type =
    /// kiro`).
    pub credit_total: f64,
    /// Number of events where credit usage was expected but unavailable.
    pub credit_missing_events: u64,
    /// Timestamp (ms) of the most recent usage event, if any.
    pub last_used_at: Option<i64>,
}

/// Persisted upstream proxy config row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayProxyConfigRecord {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Persisted provider-to-proxy binding row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayProxyBindingRecord {
    pub provider_type: String,
    pub proxy_config_id: String,
    pub updated_at: i64,
}

/// Input payload used to create one public token-request queue record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewLlmGatewayTokenRequestInput {
    pub request_id: String,
    pub requester_email: String,
    pub requested_quota_billable_limit: u64,
    pub request_reason: String,
    pub frontend_page_url: Option<String>,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
}

/// Persisted token-request queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayTokenRequestRecord {
    pub request_id: String,
    pub requester_email: String,
    pub requested_quota_billable_limit: u64,
    pub request_reason: String,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Input payload used to create one public account-contribution queue record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewLlmGatewayAccountContributionRequestInput {
    pub request_id: String,
    pub account_name: String,
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
}

/// Persisted account-contribution queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayAccountContributionRequestRecord {
    pub request_id: String,
    pub account_name: String,
    pub account_id: Option<String>,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub requester_email: String,
    pub contributor_message: String,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub imported_account_name: Option<String>,
    pub issued_key_id: Option<String>,
    pub issued_key_name: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Input payload used to create one public sponsor-request queue record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewLlmGatewaySponsorRequestInput {
    pub request_id: String,
    pub requester_email: String,
    pub sponsor_message: String,
    pub display_name: Option<String>,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
}

/// Persisted sponsor-request queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewaySponsorRequestRecord {
    pub request_id: String,
    pub requester_email: String,
    pub sponsor_message: String,
    pub display_name: Option<String>,
    pub github_id: Option<String>,
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub payment_email_sent_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub processed_at: Option<i64>,
}

/// Singleton runtime configuration row for the LLM gateway.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayRuntimeConfigRecord {
    pub id: String,
    pub auth_cache_ttl_seconds: u64,
    /// Maximum allowed request body size in bytes; requests exceeding this are
    /// rejected.
    pub max_request_body_bytes: u64,
    /// Number of consecutive Codex account refresh failures tolerated before
    /// the account is marked unavailable.
    pub account_failure_retry_limit: u64,
    /// Maximum number of Kiro upstream requests allowed in flight at once.
    pub kiro_channel_max_concurrency: u64,
    /// Minimum spacing between Kiro upstream request starts.
    pub kiro_channel_min_start_interval_ms: u64,
    pub updated_at: i64,
}

impl Default for LlmGatewayRuntimeConfigRecord {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            auth_cache_ttl_seconds: DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
            max_request_body_bytes: DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
            account_failure_retry_limit: DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT,
            kiro_channel_max_concurrency: DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
            kiro_channel_min_start_interval_ms: DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
            updated_at: now_ms(),
        }
    }
}

/// Convenience helper returning the current Unix timestamp in milliseconds.
pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
