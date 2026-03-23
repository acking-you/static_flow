use chrono::Utc;
use serde::{Deserialize, Serialize};

pub const LLM_GATEWAY_KEYS_TABLE: &str = "llm_gateway_keys";
pub const LLM_GATEWAY_USAGE_EVENTS_TABLE: &str = "llm_gateway_usage_events";
pub const LLM_GATEWAY_RUNTIME_CONFIG_TABLE: &str = "llm_gateway_runtime_config";

pub const LLM_GATEWAY_TABLE_NAMES: &[&str] =
    &[LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_USAGE_EVENTS_TABLE, LLM_GATEWAY_RUNTIME_CONFIG_TABLE];

pub const LLM_GATEWAY_KEY_STATUS_ACTIVE: &str = "active";
pub const LLM_GATEWAY_KEY_STATUS_DISABLED: &str = "disabled";
pub const DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS: u64 = 60;

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
    pub created_at: i64,
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
