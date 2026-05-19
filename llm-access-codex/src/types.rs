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
    ///
    /// This is retained only when a caller explicitly needs the original
    /// client shape for diagnostics. Most requests avoid keeping a second full
    /// body copy and leave this empty.
    pub client_request_body: Option<Bytes>,
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
    /// Last text-like user content extracted from the original client body.
    pub last_message_content: Option<String>,
}

impl PreparedGatewayRequest {
    /// Return the original client body when it was retained; otherwise fall
    /// back to the upstream body.
    pub fn client_request_body_or_upstream(&self) -> &Bytes {
        self.client_request_body
            .as_ref()
            .unwrap_or(&self.request_body)
    }
}

/// Response adaptation mode selected by the incoming OpenAI-compatible
/// endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GatewayResponseAdapter {
    /// Forward the Codex responses payload.
    Responses,
    /// Convert responses payloads back to chat/completions shape.
    ChatCompletions,
    /// Convert responses payloads into Anthropic messages shape.
    AnthropicMessages,
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
    /// Stable chat chunk indices assigned to streamed tool calls.
    pub tool_call_indices: BTreeMap<String, usize>,
    /// Whether one streamed tool call has already emitted its start chunk.
    pub tool_call_started: BTreeMap<String, bool>,
    /// Whether one streamed tool call has emitted any incremental argument
    /// delta.
    pub tool_call_delta_seen: BTreeMap<String, bool>,
    /// Whether the emitted start chunk already carried non-empty payload.
    pub tool_call_start_had_payload: BTreeMap<String, bool>,
    /// Next tool call index to allocate for a newly observed streamed call.
    pub next_tool_call_index: usize,
}

impl ChatStreamMetadata {
    /// Assign a stable OpenAI chat chunk index to one streamed tool call.
    pub fn tool_call_index(&mut self, lookup_key: &str) -> usize {
        if let Some(index) = self.tool_call_indices.get(lookup_key).copied() {
            return index;
        }
        let index = self.next_tool_call_index;
        self.tool_call_indices.insert(lookup_key.to_string(), index);
        self.next_tool_call_index += 1;
        index
    }

    /// Record that one streamed tool call emitted its first structural chunk.
    pub fn mark_tool_call_started(&mut self, lookup_key: &str, had_payload: bool) -> bool {
        let first = self
            .tool_call_started
            .insert(lookup_key.to_string(), true)
            .is_none();
        if first {
            self.tool_call_start_had_payload
                .insert(lookup_key.to_string(), had_payload);
        }
        first
    }

    /// Record that one streamed tool call emitted incremental argument input.
    pub fn mark_tool_call_delta_seen(&mut self, lookup_key: &str) {
        self.tool_call_delta_seen
            .insert(lookup_key.to_string(), true);
    }

    /// Whether one streamed tool call already emitted its start chunk.
    pub fn tool_call_started(&self, lookup_key: &str) -> bool {
        self.tool_call_started
            .get(lookup_key)
            .copied()
            .unwrap_or(false)
    }

    /// Whether one streamed tool call already emitted incremental argument
    /// input.
    pub fn tool_call_delta_seen(&self, lookup_key: &str) -> bool {
        self.tool_call_delta_seen
            .get(lookup_key)
            .copied()
            .unwrap_or(false)
    }

    /// Whether the first structural chunk for this tool call already carried
    /// payload.
    pub fn tool_call_start_had_payload(&self, lookup_key: &str) -> bool {
        self.tool_call_start_had_payload
            .get(lookup_key)
            .copied()
            .unwrap_or(false)
    }
}

/// Adapted responses body plus shortened tool-name restore map.
pub type OpenAiChatAdaptedRequest = (Map<String, Value>, BTreeMap<String, String>);
