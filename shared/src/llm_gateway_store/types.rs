use chrono::Utc;
use serde::{Deserialize, Serialize};

pub const LLM_GATEWAY_KEYS_TABLE: &str = "llm_gateway_keys";
pub const LLM_GATEWAY_USAGE_EVENTS_TABLE: &str = "llm_gateway_usage_events";
pub const LLM_GATEWAY_RUNTIME_CONFIG_TABLE: &str = "llm_gateway_runtime_config";
pub const LLM_GATEWAY_TOKEN_REQUESTS_TABLE: &str = "llm_gateway_token_requests";
pub const LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE: &str =
    "llm_gateway_account_contribution_requests";
pub const LLM_GATEWAY_SPONSOR_REQUESTS_TABLE: &str = "llm_gateway_sponsor_requests";

pub const LLM_GATEWAY_TABLE_NAMES: &[&str] = &[
    LLM_GATEWAY_KEYS_TABLE,
    LLM_GATEWAY_USAGE_EVENTS_TABLE,
    LLM_GATEWAY_RUNTIME_CONFIG_TABLE,
    LLM_GATEWAY_TOKEN_REQUESTS_TABLE,
    LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
    LLM_GATEWAY_SPONSOR_REQUESTS_TABLE,
];

pub const LLM_GATEWAY_KEY_STATUS_ACTIVE: &str = "active";
pub const LLM_GATEWAY_KEY_STATUS_DISABLED: &str = "disabled";
pub const DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS: u64 = 60;
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING: &str = "pending";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED: &str = "issued";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED: &str = "rejected";
pub const LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED: &str = "failed";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_SUBMITTED: &str = "submitted";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT: &str = "payment_email_sent";
pub const LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED: &str = "approved";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayKeyRecord {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub key_hash: String,
    pub status: String,
    pub public_visible: bool,
    pub quota_billable_limit: u64,
    pub usage_input_uncached_tokens: u64,
    pub usage_input_cached_tokens: u64,
    pub usage_output_tokens: u64,
    pub usage_billable_tokens: u64,
    pub last_used_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub route_strategy: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names: Option<Vec<String>>,
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
    pub client_ip: String,
    pub ip_region: String,
    pub request_headers_json: String,
    pub last_message_content: Option<String>,
    pub created_at: i64,
}

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LlmGatewayRuntimeConfigRecord {
    pub id: String,
    pub auth_cache_ttl_seconds: u64,
    pub updated_at: i64,
}

impl Default for LlmGatewayRuntimeConfigRecord {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            auth_cache_ttl_seconds: DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
            updated_at: now_ms(),
        }
    }
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
