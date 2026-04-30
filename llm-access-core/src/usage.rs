//! Provider-neutral usage event contract.

use serde::{Deserialize, Serialize};

use crate::provider::{ProtocolFamily, ProviderType, RouteStrategy};

/// Timing fields captured by provider handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTiming {
    /// Time from route entry until upstream headers in milliseconds.
    pub upstream_headers_ms: Option<i64>,
    /// Time from upstream headers until upstream body completion in
    /// milliseconds.
    pub post_headers_body_ms: Option<i64>,
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
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<RouteStrategy>,
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
    /// Provider timing fields.
    pub timing: UsageTiming,
}
