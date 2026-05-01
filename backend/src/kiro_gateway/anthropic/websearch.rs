//! Web search shim for the Anthropic-compatible endpoint.
//!
//! When a request contains only a `web_search` tool, this module short-circuits
//! the normal LLM flow: it routes the query through Kiro's MCP web_search
//! endpoint and wraps the results in Anthropic-compatible SSE or JSON
//! responses.

use std::convert::Infallible;

use axum::{
    body::Body,
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures_util::{stream, Stream};
use llm_access_kiro::anthropic::websearch as runtime_websearch;
use serde_json::json;
use static_flow_shared::llm_gateway_store::{KiroCachePolicy, LlmGatewayKeyRecord};
use tokio::time::Instant;
use uuid::Uuid;

use super::{
    anthropic_usage_json, build_failure_diagnostic_payload, map_provider_error,
    stream::SseEvent,
    types::{ErrorResponse, MessagesRequest},
    zero_usage_summary,
};
use crate::{
    kiro_gateway::{
        provider::ProviderCallError, record_messages_usage, FailedKiroRequestEvent,
        KiroEventContext, KiroUsageSummary,
    },
    state::AppState,
};

type McpRequest = runtime_websearch::McpRequest;
type McpResponse = runtime_websearch::McpResponse;
type WebSearchResults = runtime_websearch::WebSearchResults;

pub fn should_route_mcp_web_search(req: &MessagesRequest) -> bool {
    runtime_websearch::should_route_mcp_web_search(req)
}

pub fn remove_web_search_tools(req: &mut MessagesRequest) -> bool {
    runtime_websearch::remove_web_search_tools(req)
}

/// Handles a pure web_search request by calling Kiro's MCP endpoint and
/// returning the results as either an SSE stream or a JSON response,
/// depending on `payload.stream`.
pub async fn handle_websearch_request(
    state: AppState,
    key_record: LlmGatewayKeyRecord,
    mut event_context: KiroEventContext,
    effective_cache_policy: KiroCachePolicy,
    provider: &crate::kiro_gateway::provider::KiroProvider,
    payload: &MessagesRequest,
    input_tokens: i32,
) -> Response {
    let query = match extract_search_query(payload) {
        Some(query) => query,
        None => {
            let diagnostic_payload = build_failure_diagnostic_payload(
                super::DiagnosticRequestContext {
                    event_context: &event_context,
                    request_validation_enabled: key_record.kiro_request_validation_enabled,
                    stream: payload.stream,
                    buffered_for_cc: false,
                },
                "websearch_query_extract",
                "Unable to extract web search query from messages.",
                StatusCode::BAD_REQUEST.as_u16() as i32,
                None,
            );
            if let Err(err) = crate::kiro_gateway::record_failed_request_event(
                &state,
                &key_record,
                &event_context,
                FailedKiroRequestEvent {
                    _effective_policy: &effective_cache_policy,
                    status_code: StatusCode::BAD_REQUEST.as_u16() as i32,
                    diagnostic_payload,
                    usage: zero_usage_summary(),
                    usage_missing: false,
                },
            )
            .await
            {
                tracing::warn!("failed to persist kiro web_search validation failure: {err:#}");
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new(
                    "invalid_request_error",
                    "Unable to extract web search query from messages.",
                )),
            )
                .into_response();
        },
    };

    tracing::info!(
        model = %payload.model,
        stream = payload.stream,
        query = %query,
        "routing pure web_search request through kiro mcp shim"
    );

    let (tool_use_id, mcp_request) = create_mcp_request(&query);
    event_context.upstream_request_body_json = serde_json::to_string(&mcp_request).ok();
    let search_results = match call_mcp_api(&key_record, provider, &mcp_request).await {
        Ok(success) => {
            let McpCallSuccess {
                response,
                account_name,
                routing_wait_ms,
                upstream_headers_ms,
                post_headers_body_ms,
                quota_failover_count,
                routing_diagnostics_json,
            } = success;
            event_context.account_name = Some(account_name);
            event_context.routing_wait_ms = Some(super::clamp_u64_ms_to_i32(routing_wait_ms));
            event_context.upstream_headers_ms =
                Some(super::clamp_u64_ms_to_i32(upstream_headers_ms));
            event_context.post_headers_body_ms =
                Some(super::clamp_u64_ms_to_i32(post_headers_body_ms));
            event_context.quota_failover_count = quota_failover_count;
            event_context.routing_diagnostics_json = routing_diagnostics_json;
            parse_search_results(&response)
        },
        Err(err) => {
            if should_propagate_mcp_error(&err) {
                return map_provider_error(
                    super::ProviderFailureContext {
                        state: &state,
                        key_record: &key_record,
                        effective_cache_policy: &effective_cache_policy,
                        diagnostic: super::DiagnosticRequestContext {
                            event_context: &event_context,
                            request_validation_enabled: key_record.kiro_request_validation_enabled,
                            stream: payload.stream,
                            buffered_for_cc: false,
                        },
                    },
                    err,
                    "websearch_mcp_call",
                )
                .await;
            }
            tracing::warn!(
                query = %query,
                error = %err,
                "kiro mcp web_search failed; returning empty search results fallback"
            );
            None
        },
    };
    let summary = generate_search_summary(&query, &search_results);
    let output_tokens = estimate_output_tokens(&summary);
    tracing::info!(
        model = %payload.model,
        stream = payload.stream,
        query = %query,
        result_count = search_results.as_ref().map(|results| results.results.len()).unwrap_or(0),
        fallback_empty = search_results.is_none(),
        output_tokens,
        "finished kiro web_search shim"
    );
    let usage = KiroUsageSummary {
        input_uncached_tokens: input_tokens,
        input_cached_tokens: 0,
        output_tokens,
        credit_usage: None,
        credit_usage_missing: true,
    };
    if let Err(err) = record_messages_usage(
        &state,
        &key_record,
        &event_context,
        &effective_cache_policy,
        usage,
        false,
    )
    .await
    {
        tracing::warn!("failed to persist kiro web_search usage event: {err:#}");
    }

    if payload.stream {
        let stream = create_websearch_sse_stream(
            payload.model.clone(),
            query,
            tool_use_id,
            search_results,
            input_tokens,
            &summary,
            output_tokens,
        );
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from_stream(stream))
            .unwrap_or_else(|err| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "internal_error",
                        format!("Failed to create web search response: {err}"),
                    )),
                )
                    .into_response()
            });
    }

    Json(json!({
        "id": format!("msg_{}", Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": create_non_stream_content_blocks(&query, &tool_use_id, &search_results, &summary),
        "model": payload.model,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": anthropic_usage_json(input_tokens, output_tokens, 0)
    }))
    .into_response()
}

// Extracts the search query string from the first message, stripping
// the "Perform a web search for the query: " prefix if present.
fn extract_search_query(req: &MessagesRequest) -> Option<String> {
    runtime_websearch::extract_search_query(req)
}

fn create_mcp_request(query: &str) -> (String, McpRequest) {
    runtime_websearch::create_mcp_request(query)
}

async fn call_mcp_api(
    key_record: &LlmGatewayKeyRecord,
    provider: &crate::kiro_gateway::provider::KiroProvider,
    request: &McpRequest,
) -> std::result::Result<McpCallSuccess, ProviderCallError> {
    let request_body = serde_json::to_string(request).map_err(|err| {
        ProviderCallError::new(anyhow::anyhow!("serialize mcp request: {err}"), None)
    })?;
    let response = provider.call_mcp(key_record, &request_body).await?;
    let account_name = response.account_name;
    let routing_wait_ms = response.routing_wait_ms;
    let upstream_headers_ms = response.upstream_headers_ms;
    let quota_failover_count = response.quota_failover_count;
    let routing_diagnostics_json = response.routing_diagnostics_json;
    let upstream_body_started = Instant::now();
    let body = response.response.text().await.map_err(|err| {
        ProviderCallError::new(
            anyhow::anyhow!("read mcp response body: {err}"),
            Some(request_body.clone()),
        )
    })?;
    let post_headers_body_ms = upstream_body_started
        .elapsed()
        .as_millis()
        .min(u64::MAX as u128) as u64;
    let mcp_response: McpResponse = serde_json::from_str(&body).map_err(|err| {
        ProviderCallError::new(
            anyhow::anyhow!("parse mcp response body: {err}; body={body}"),
            Some(request_body.clone()),
        )
    })?;
    if let Some(error) = &mcp_response.error {
        return Err(ProviderCallError::new(
            anyhow::anyhow!(
                "MCP error: {} - {}",
                error.code.unwrap_or(-1),
                error.message.as_deref().unwrap_or("Unknown error")
            ),
            Some(request_body),
        ));
    }
    Ok(McpCallSuccess {
        response: mcp_response,
        account_name,
        routing_wait_ms,
        upstream_headers_ms,
        post_headers_body_ms,
        quota_failover_count,
        routing_diagnostics_json,
    })
}

struct McpCallSuccess {
    response: McpResponse,
    account_name: String,
    routing_wait_ms: u64,
    upstream_headers_ms: u64,
    post_headers_body_ms: u64,
    quota_failover_count: u64,
    routing_diagnostics_json: Option<String>,
}

fn parse_search_results(mcp_response: &McpResponse) -> Option<WebSearchResults> {
    runtime_websearch::parse_search_results(mcp_response)
}

// Determines whether an MCP error should be propagated to the client
// (quota/auth errors) vs. silently returning empty results.
fn should_propagate_mcp_error(err: &impl std::fmt::Display) -> bool {
    runtime_websearch::should_propagate_mcp_error_text(&err.to_string())
}

fn estimate_output_tokens(summary: &str) -> i32 {
    runtime_websearch::estimate_output_tokens(summary)
}

fn create_websearch_sse_stream(
    model: String,
    query: String,
    tool_use_id: String,
    search_results: Option<WebSearchResults>,
    input_tokens: i32,
    summary: &str,
    output_tokens: i32,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    let events = generate_websearch_events(
        &model,
        &query,
        &tool_use_id,
        search_results.as_ref(),
        input_tokens,
        summary,
        output_tokens,
    );
    stream::iter(
        events
            .into_iter()
            .map(|event| Ok(Bytes::from(event.to_sse_string()))),
    )
}

fn generate_websearch_events(
    model: &str,
    query: &str,
    tool_use_id: &str,
    search_results: Option<&WebSearchResults>,
    input_tokens: i32,
    summary: &str,
    output_tokens: i32,
) -> Vec<SseEvent> {
    runtime_websearch::generate_websearch_events(
        model,
        query,
        tool_use_id,
        search_results,
        input_tokens,
        summary,
        output_tokens,
    )
}

fn create_non_stream_content_blocks(
    query: &str,
    tool_use_id: &str,
    search_results: &Option<WebSearchResults>,
    summary: &str,
) -> Vec<serde_json::Value> {
    runtime_websearch::create_non_stream_content_blocks(query, tool_use_id, search_results, summary)
}

fn generate_search_summary(query: &str, results: &Option<WebSearchResults>) -> String {
    runtime_websearch::generate_search_summary(query, results)
}

#[cfg(test)]
mod tests {
    use super::{super::types::Message, *};

    fn base_request(
        tools: Option<Vec<super::super::types::Tool>>,
        content: serde_json::Value,
    ) -> MessagesRequest {
        MessagesRequest {
            model: "claude-sonnet-4-6".to_string(),
            _max_tokens: 1024,
            messages: vec![Message {
                role: "user".to_string(),
                content,
            }],
            stream: true,
            system: None,
            tools,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn detects_pure_web_search_tool_only() {
        let req = base_request(
            Some(vec![super::super::types::Tool {
                tool_type: Some("web_search_20250305".to_string()),
                name: "web_search".to_string(),
                description: String::new(),
                input_schema: Default::default(),
                max_uses: Some(8),
            }]),
            serde_json::json!("test"),
        );
        assert!(runtime_websearch::has_web_search_tool(&req));
    }

    #[test]
    fn rejects_mixed_tools_for_web_search_short_circuit() {
        let req = base_request(
            Some(vec![
                super::super::types::Tool {
                    tool_type: Some("web_search_20250305".to_string()),
                    name: "web_search".to_string(),
                    description: String::new(),
                    input_schema: Default::default(),
                    max_uses: Some(8),
                },
                super::super::types::Tool {
                    tool_type: Some("custom".to_string()),
                    name: "other".to_string(),
                    description: String::new(),
                    input_schema: Default::default(),
                    max_uses: None,
                },
            ]),
            serde_json::json!("test"),
        );
        assert!(!runtime_websearch::has_web_search_tool(&req));
    }

    #[test]
    fn extracts_prefixed_query() {
        let req = base_request(
            None,
            serde_json::json!([{
                "type": "text",
                "text": "Perform a web search for the query: static flow kiro"
            }]),
        );
        assert_eq!(extract_search_query(&req).as_deref(), Some("static flow kiro"));
    }

    #[test]
    fn websearch_stream_message_start_marks_half_input_as_cache_creation() {
        let events = generate_websearch_events(
            "claude-sonnet-4-6",
            "static flow kiro",
            "toolu_test",
            None,
            125,
            "summary",
            16,
        );
        let message_start = events
            .iter()
            .find(|event| event.event == "message_start")
            .expect("should include message_start");
        assert_eq!(
            message_start.data["message"]["usage"]["cache_creation_input_tokens"],
            serde_json::json!(62)
        );
        assert_eq!(
            message_start.data["message"]["usage"]["cache_read_input_tokens"],
            serde_json::json!(0)
        );
    }

    #[test]
    fn websearch_route_related_fixed_error_should_be_propagated() {
        let err = anyhow::anyhow!("fixed route account `alpha` is not available");
        assert!(should_propagate_mcp_error(&err));
    }

    #[test]
    fn websearch_route_related_auto_subset_error_should_be_propagated() {
        let err = anyhow::anyhow!("no configured auto accounts are available");
        assert!(should_propagate_mcp_error(&err));
    }

    #[test]
    fn websearch_route_strategy_requires_fixed_account_error_should_be_propagated() {
        let err = anyhow::anyhow!("fixed route_strategy requires fixed_account_name");
        assert!(should_propagate_mcp_error(&err));
    }

    #[test]
    fn websearch_unsupported_route_strategy_error_should_be_propagated() {
        let err = anyhow::anyhow!("unsupported route strategy `none`");
        assert!(should_propagate_mcp_error(&err));
    }

    #[test]
    fn websearch_non_route_error_should_fallback() {
        let err = anyhow::anyhow!("MCP error: -1 - temporary endpoint issue");
        assert!(!should_propagate_mcp_error(&err));
    }
}
