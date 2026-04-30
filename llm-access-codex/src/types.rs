//! Internal Codex gateway request, response, and usage types.

use std::collections::BTreeMap;

use bytes::Bytes;
use http::Method;
use serde_json::{Map, Value};

/// Token usage accounting in the billing model used by the gateway.
#[derive(Debug, Clone, Default)]
pub struct UsageBreakdown {
    /// Uncached input tokens.
    pub input_uncached_tokens: u64,
    /// Cached input tokens.
    pub input_cached_tokens: u64,
    /// Output tokens.
    pub output_tokens: u64,
    /// Whether upstream usage was missing and had to be marked unknown.
    pub usage_missing: bool,
}

impl UsageBreakdown {
    /// Calculate billable tokens with StaticFlow's current weighting.
    pub fn billable_tokens(&self) -> u64 {
        self.input_uncached_tokens
            .saturating_add(self.input_cached_tokens / 10)
            .saturating_add(self.output_tokens.saturating_mul(5))
    }

    /// Calculate billable tokens after applying a request-level multiplier.
    pub fn billable_tokens_with_multiplier(&self, multiplier: u64) -> u64 {
        self.billable_tokens().saturating_mul(multiplier.max(1))
    }
}

/// Normalized proxy request ready to send to the upstream Codex backend.
#[derive(Debug, Clone)]
pub struct PreparedGatewayRequest {
    /// Original client-facing path and query.
    pub original_path: String,
    /// Upstream path and query after protocol adaptation.
    pub upstream_path: String,
    /// HTTP method.
    pub method: Method,
    /// Body exactly as presented by the client after local content decoding.
    pub client_request_body: Bytes,
    /// Body sent to the upstream Codex backend.
    pub request_body: Bytes,
    /// Upstream model after local mapping.
    pub model: Option<String>,
    /// Model id to expose back to the client when mapping was applied.
    pub client_visible_model: Option<String>,
    /// Whether the client originally requested streaming.
    pub wants_stream: bool,
    /// Whether the upstream request was forced to stream for local adaptation.
    pub force_upstream_stream: bool,
    /// Effective content type.
    pub content_type: String,
    /// Response adaptation mode selected by the incoming endpoint.
    pub response_adapter: GatewayResponseAdapter,
    /// Prompt cache anchor extracted from headers or request body.
    pub thread_anchor: Option<String>,
    /// Reverse map for restoring shortened OpenAI tool names.
    pub tool_name_restore_map: BTreeMap<String, String>,
    /// Request-level billable-token multiplier.
    pub billable_multiplier: u64,
}

/// Response adaptation mode selected by the incoming OpenAI-compatible
/// endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayResponseAdapter {
    /// Forward the Codex responses payload.
    Responses,
    /// Convert responses payloads back to chat/completions shape.
    ChatCompletions,
}

/// Internal normalized representation of one upstream model descriptor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct GatewayModelDescriptor {
    /// Model id.
    pub id: String,
    /// Owner label shown on the OpenAI-compatible model list.
    pub owned_by: &'static str,
}

/// Stream-scoped metadata needed to fill chat chunk defaults consistently.
#[derive(Debug, Clone, Default)]
pub struct ChatStreamMetadata {
    /// Last observed response id.
    pub response_id: Option<String>,
    /// Last observed model id.
    pub model: Option<String>,
    /// Last observed created timestamp.
    pub created: Option<i64>,
}

/// Adapted responses body plus shortened tool-name restore map.
pub type OpenAiChatAdaptedRequest = (Map<String, Value>, BTreeMap<String, String>);
