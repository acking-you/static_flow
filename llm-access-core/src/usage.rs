//! Provider-neutral usage event contract.

use serde::{Deserialize, Serialize};

use crate::provider::{ProtocolFamily, ProviderType, RouteStrategy};

/// Timing fields captured by provider handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTiming {
    /// End-to-end latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// Time waiting for local routing or scheduler in milliseconds.
    pub routing_wait_ms: Option<i64>,
    /// Time from route entry until upstream headers in milliseconds.
    pub upstream_headers_ms: Option<i64>,
    /// Time from upstream headers until upstream body completion in
    /// milliseconds.
    pub post_headers_body_ms: Option<i64>,
    /// Time spent reading the incoming request body in milliseconds.
    pub request_body_read_ms: Option<i64>,
    /// Time spent parsing the incoming request JSON in milliseconds.
    pub request_json_parse_ms: Option<i64>,
    /// Time from route entry until the provider handler has parsed the request.
    pub pre_handler_ms: Option<i64>,
    /// Time from route entry until first downstream SSE write in milliseconds.
    pub first_sse_write_ms: Option<i64>,
    /// Time from route entry until stream finish in milliseconds.
    pub stream_finish_ms: Option<i64>,
}

/// One normalized usage event before persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Stable event id.
    pub event_id: String,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Provider type.
    pub provider_type: ProviderType,
    /// Protocol family.
    pub protocol_family: ProtocolFamily,
    /// Key id at event time.
    pub key_id: String,
    /// Key name at event time.
    pub key_name: String,
    /// Account name used by the upstream request.
    pub account_name: Option<String>,
    /// Account group id captured at event time.
    pub account_group_id_at_event: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<RouteStrategy>,
    /// Incoming HTTP method.
    pub request_method: String,
    /// Operator-facing request URL.
    pub request_url: String,
    /// Client-facing endpoint.
    pub endpoint: String,
    /// Client-facing model.
    pub model: Option<String>,
    /// Upstream mapped model.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
    /// Number of upstream route failovers.
    pub quota_failover_count: u64,
    /// Provider routing diagnostics JSON.
    pub routing_diagnostics_json: Option<String>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Credit usage when known.
    pub credit_usage: Option<String>,
    /// Whether normal token usage was unavailable.
    pub usage_missing: bool,
    /// Whether credit usage was unavailable.
    pub credit_usage_missing: bool,
    /// Client IP captured from proxy headers.
    pub client_ip: String,
    /// Best-effort region label for the client IP.
    pub ip_region: String,
    /// JSON snapshot of request headers.
    pub request_headers_json: String,
    /// Last user message content when cheaply extractable.
    pub last_message_content: Option<String>,
    /// Original client request body for diagnostic events.
    pub client_request_body_json: Option<String>,
    /// Upstream request body for diagnostic events.
    pub upstream_request_body_json: Option<String>,
    /// Canonical full request body for diagnostic events.
    pub full_request_json: Option<String>,
    /// Provider timing fields.
    pub timing: UsageTiming,
}
