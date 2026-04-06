//! Anthropic-compatible API handler for the Kiro gateway.
//!
//! Routes `/v1/messages`, `/cc/v1/messages`, `/v1/models`, and `count_tokens`
//! requests. Converts Anthropic request payloads to Kiro wire format, streams
//! responses as SSE (with optional buffered mode for Claude Code), and persists
//! usage events.

use std::convert::Infallible;

use async_stream::stream;
use axum::{
    body::Body,
    extract::{Json as JsonExtractor, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use tokio::{
    sync::{oneshot, watch},
    time::{interval, Duration},
};

use super::{
    parser::decoder::EventStreamDecoder,
    provider::{KiroProvider, ProviderCallError},
    token, AppKiroStateExt, KiroUsageSummary,
};
use crate::kiro_gateway::{record_messages_usage, KiroEventContext};

pub mod converter;
pub mod stream;
pub mod types;
pub mod websearch;

use static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord;

use self::{
    converter::{
        convert_normalized_request_with_validation, current_user_message_range,
        extract_tool_result_content, normalize_request, ConversionError, NormalizationEvent,
        NormalizedRequest, ToolNormalizationEvent,
    },
    stream::{BufferedStreamContext, StreamContext},
    types::{
        CountTokensRequest, CountTokensResponse, ErrorResponse, MessagesRequest, Model,
        ModelsResponse, OutputConfig, Thinking,
    },
    websearch::handle_websearch_request,
};
use crate::{kiro_gateway::wire::Event, state::AppState};

const SUPPORTED_MODEL_CATALOG: [(&str, &str, i64); 10] = [
    ("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5", 1727568000),
    ("claude-sonnet-4-5-20250929-thinking", "Claude Sonnet 4.5 (Thinking)", 1727568000),
    ("claude-opus-4-5-20251101", "Claude Opus 4.5", 1730419200),
    ("claude-opus-4-5-20251101-thinking", "Claude Opus 4.5 (Thinking)", 1730419200),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6", 1770314400),
    ("claude-sonnet-4-6-thinking", "Claude Sonnet 4.6 (Thinking)", 1770314400),
    ("claude-opus-4-6", "Claude Opus 4.6", 1770314400),
    ("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)", 1770314400),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5", 1727740800),
    ("claude-haiku-4-5-20251001-thinking", "Claude Haiku 4.5 (Thinking)", 1727740800),
];
const KIRO_UPSTREAM_LOG_PREVIEW_CHARS: usize = 8_192;
const KIRO_STREAM_FAILURE_STATUS_CODE: i32 = 599;
const KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS: usize = 160;
const KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS: usize = 1_024;

// Bundles the state needed to persist usage after a streaming response
// completes.
struct UsagePersistContext {
    state: AppState,
    key_record: LlmGatewayKeyRecord,
    event_context: KiroEventContext,
}

#[derive(Clone, Copy)]
pub(super) struct DiagnosticRequestContext<'a> {
    event_context: &'a KiroEventContext,
    request_validation_enabled: bool,
    stream: bool,
    buffered_for_cc: bool,
}

pub(super) struct ProviderFailureContext<'a> {
    state: &'a AppState,
    key_record: &'a LlmGatewayKeyRecord,
    diagnostic: DiagnosticRequestContext<'a>,
}

struct NonStreamRequestContext {
    model: String,
    input_tokens: i32,
    tool_name_map: std::collections::HashMap<String, String>,
    request_validation_enabled: bool,
}

enum UsagePersistOutcome {
    Success {
        summary: KiroUsageSummary,
        usage_missing: bool,
    },
    Failure {
        status_code: i32,
        summary: KiroUsageSummary,
        usage_missing: bool,
        diagnostic_payload: String,
    },
}

#[derive(Clone)]
struct KiroUpstreamLogContext {
    key_id: String,
    key_name: String,
    account_name: String,
    model: String,
    buffered_for_cc: bool,
}

struct NormalizationLogContext<'a> {
    key_record: &'a LlmGatewayKeyRecord,
    public_path: &'a str,
    requested_model: &'a str,
    effective_model: &'a str,
    stream: bool,
    buffered_for_cc: bool,
    request_validation_enabled: bool,
}

fn log_normalization_event(event: &NormalizationEvent, ctx: &NormalizationLogContext<'_>) {
    tracing::warn!(
        key_id = %ctx.key_record.id,
        key_name = %ctx.key_record.name,
        route = ctx.public_path,
        requested_model = ctx.requested_model,
        effective_model = ctx.effective_model,
        stream = ctx.stream,
        buffered_for_cc = ctx.buffered_for_cc,
        request_validation_enabled = ctx.request_validation_enabled,
        normalized_message_index = event.message_index,
        normalized_message_role = %event.role,
        normalized_action = event.action,
        normalized_reason = event.reason,
        normalized_content_block_index = event.content_block_index,
        normalized_block_type = event.block_type.as_deref().unwrap_or(""),
        "normalized kiro anthropic request before validation"
    );
}

fn log_tool_normalization_event(event: &ToolNormalizationEvent, ctx: &NormalizationLogContext<'_>) {
    tracing::warn!(
        key_id = %ctx.key_record.id,
        key_name = %ctx.key_record.name,
        route = ctx.public_path,
        requested_model = ctx.requested_model,
        effective_model = ctx.effective_model,
        stream = ctx.stream,
        buffered_for_cc = ctx.buffered_for_cc,
        request_validation_enabled = ctx.request_validation_enabled,
        tool_index = event.tool_index,
        tool_name = %event.tool_name,
        normalization_action = event.action,
        normalization_reason = event.reason,
        "normalized kiro tool metadata before validation"
    );
}

fn log_tool_validation_summary(normalized: &NormalizedRequest, ctx: &NormalizationLogContext<'_>) {
    tracing::info!(
        key_id = %ctx.key_record.id,
        key_name = %ctx.key_record.name,
        route = ctx.public_path,
        requested_model = ctx.requested_model,
        effective_model = ctx.effective_model,
        stream = ctx.stream,
        buffered_for_cc = ctx.buffered_for_cc,
        request_validation_enabled = ctx.request_validation_enabled,
        normalized_tool_description_count =
            normalized.tool_validation_summary.normalized_tool_description_count,
        empty_tool_name_count = normalized.tool_validation_summary.empty_tool_name_count,
        schema_keyword_counts = ?normalized.tool_validation_summary.schema_keyword_counts,
        "prepared kiro tool validation summary before upstream call"
    );
}

impl KiroUpstreamLogContext {
    fn new(
        key_record: &LlmGatewayKeyRecord,
        account_name: Option<&str>,
        model: &str,
        buffered_for_cc: bool,
    ) -> Self {
        Self {
            key_id: key_record.id.clone(),
            key_name: key_record.name.clone(),
            account_name: account_name.unwrap_or("unknown").to_string(),
            model: model.to_string(),
            buffered_for_cc,
        }
    }
}

fn summarize_log_text(text: &str) -> String {
    let total_chars = text.chars().count();
    if total_chars <= KIRO_UPSTREAM_LOG_PREVIEW_CHARS {
        return text.to_string();
    }
    let preview = text
        .chars()
        .take(KIRO_UPSTREAM_LOG_PREVIEW_CHARS)
        .collect::<String>();
    format!("{preview}...[truncated,total_chars={total_chars}]")
}

fn log_kiro_upstream_event(log_ctx: &KiroUpstreamLogContext, stream_kind: &str, event: &Event) {
    match event {
        Event::Error {
            error_code,
            error_message,
        } => {
            tracing::error!(
                key_id = %log_ctx.key_id,
                key_name = %log_ctx.key_name,
                account_name = %log_ctx.account_name,
                model = %log_ctx.model,
                buffered_for_cc = log_ctx.buffered_for_cc,
                stream_kind,
                error_code = %error_code,
                message_len = error_message.len(),
                message_preview = %summarize_log_text(error_message),
                "kiro upstream emitted error event"
            );
        },
        Event::Exception {
            exception_type,
            message,
        } => {
            tracing::error!(
                key_id = %log_ctx.key_id,
                key_name = %log_ctx.key_name,
                account_name = %log_ctx.account_name,
                model = %log_ctx.model,
                buffered_for_cc = log_ctx.buffered_for_cc,
                stream_kind,
                exception_type = %exception_type,
                message_len = message.len(),
                message_preview = %summarize_log_text(message),
                "kiro upstream emitted exception event"
            );
        },
        _ => {},
    }
}

fn log_kiro_event_parse_error(
    log_ctx: &KiroUpstreamLogContext,
    stream_kind: &str,
    err: &impl std::fmt::Display,
) {
    tracing::error!(
        key_id = %log_ctx.key_id,
        key_name = %log_ctx.key_name,
        account_name = %log_ctx.account_name,
        model = %log_ctx.model,
        buffered_for_cc = log_ctx.buffered_for_cc,
        stream_kind,
        error = %err,
        "failed to decode kiro upstream event"
    );
}

fn log_kiro_stream_read_error(
    log_ctx: &KiroUpstreamLogContext,
    stream_kind: &str,
    err: &reqwest::Error,
) {
    tracing::error!(
        key_id = %log_ctx.key_id,
        key_name = %log_ctx.key_name,
        account_name = %log_ctx.account_name,
        model = %log_ctx.model,
        buffered_for_cc = log_ctx.buffered_for_cc,
        stream_kind,
        is_timeout = err.is_timeout(),
        is_connect = err.is_connect(),
        upstream_url = ?err.url(),
        error = %err,
        "failed to read kiro upstream event stream"
    );
}

/// Maps a Kiro provider error into an appropriate HTTP error response.
/// Recognizes context-length, input-length, and quota-exhaustion errors.
fn classify_provider_error(err_text: &str) -> (StatusCode, &'static str, String) {
    if err_text.contains("CONTENT_LENGTH_EXCEEDS_THRESHOLD") {
        (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Context window is full. Reduce conversation history, system prompt, or tools."
                .to_string(),
        )
    } else if err_text.contains("Input is too long") {
        (
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Input is too long. Reduce the size of your messages.".to_string(),
        )
    } else if err_text.contains("quota exhausted") {
        (
            StatusCode::PAYMENT_REQUIRED,
            "rate_limit_error",
            "All configured Kiro accounts are out of quota. Wait for reset or refresh another \
             account."
                .to_string(),
        )
    } else if err_text.contains("minimum remaining credits threshold") {
        (
            StatusCode::PAYMENT_REQUIRED,
            "rate_limit_error",
            "All configured Kiro accounts are below the configured minimum remaining credits \
             threshold."
                .to_string(),
        )
    } else {
        (StatusCode::BAD_GATEWAY, "api_error", format!("Kiro upstream request failed: {err_text}"))
    }
}

fn provider_error_response(err_text: &str) -> Response {
    let (status, error_type, message) = classify_provider_error(err_text);
    tracing::error!(
        status = status.as_u16(),
        error_type,
        error = err_text,
        response_message = %message,
        "kiro public request failed while calling upstream"
    );
    (status, Json(ErrorResponse::new(error_type, message))).into_response()
}

pub(super) async fn map_provider_error(
    ctx: ProviderFailureContext<'_>,
    err: ProviderCallError,
    failure_stage: &str,
) -> Response {
    let err_text = err.to_string();
    let (status, _, _) = classify_provider_error(&err_text);
    let mut diagnostic_event_context = ctx.diagnostic.event_context.clone();
    if err.request_body.is_some() {
        diagnostic_event_context.upstream_request_body_json = err.request_body.clone();
    }
    let diagnostic_payload = build_failure_diagnostic_payload(
        DiagnosticRequestContext {
            event_context: &diagnostic_event_context,
            ..ctx.diagnostic
        },
        failure_stage,
        &err_text,
        status.as_u16() as i32,
        None,
    );
    if let Err(persist_err) = crate::kiro_gateway::record_failed_request_event(
        ctx.state,
        ctx.key_record,
        ctx.diagnostic.event_context,
        status.as_u16() as i32,
        diagnostic_payload,
        zero_usage_summary(),
        false,
    )
    .await
    {
        tracing::warn!("failed to persist kiro failure usage event: {persist_err:#}");
    }
    provider_error_response(&err_text)
}

fn zero_usage_summary() -> KiroUsageSummary {
    KiroUsageSummary {
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        credit_usage: None,
        credit_usage_missing: false,
    }
}

fn maybe_parse_json_text(raw: Option<&str>) -> serde_json::Value {
    match raw {
        Some(text) => serde_json::from_str::<serde_json::Value>(text)
            .unwrap_or_else(|_| serde_json::Value::String(text.to_string())),
        None => serde_json::Value::Null,
    }
}

fn build_failure_diagnostic_payload(
    ctx: DiagnosticRequestContext<'_>,
    failure_stage: &str,
    error: &str,
    status_code: i32,
    details: Option<serde_json::Value>,
) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "kind": "kiro_failure_diagnostic",
        "failure_stage": failure_stage,
        "status_code": status_code,
        "request_method": ctx.event_context.request_method,
        "request_url": ctx.event_context.request_url,
        "endpoint": ctx.event_context.endpoint,
        "model": ctx.event_context.model,
        "account_name": ctx.event_context.account_name,
        "original_last_message_content": ctx.event_context.last_message_content,
        "request_validation_enabled": ctx.request_validation_enabled,
        "stream": ctx.stream,
        "buffered_for_cc": ctx.buffered_for_cc,
        "client_request_body": maybe_parse_json_text(ctx.event_context.client_request_body_json.as_deref()),
        "upstream_request_body": maybe_parse_json_text(ctx.event_context.upstream_request_body_json.as_deref()),
        "error": error,
        "details": details.unwrap_or(serde_json::Value::Object(serde_json::Map::new())),
    }))
    .unwrap_or_else(|serialize_err| {
        format!(
            "{{\"kind\":\"kiro_failure_diagnostic\",\"failure_stage\":{:?},\"status_code\":{},\"error\":{:?},\"serialize_error\":{:?}}}",
            failure_stage,
            status_code,
            error,
            serialize_err.to_string()
        )
    })
}

/// Returns the list of available models for the `/v1/models` endpoint.
pub async fn get_models() -> impl IntoResponse {
    Json(supported_models_response())
}

pub(crate) fn supported_model_ids() -> Vec<String> {
    SUPPORTED_MODEL_CATALOG
        .iter()
        .map(|(id, _, _)| (*id).to_string())
        .collect()
}

fn supported_models_response() -> ModelsResponse {
    ModelsResponse {
        object: "list".to_string(),
        data: SUPPORTED_MODEL_CATALOG
            .iter()
            .map(|(id, display_name, created)| model(id, display_name, *created))
            .collect(),
    }
}

/// Estimates token count for the given request payload.
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    Json(CountTokensResponse {
        input_tokens: token::count_all_tokens(
            payload.model,
            payload.system,
            payload.messages,
            payload.tools,
        ) as i32,
    })
}

/// Handler for `POST /v1/messages` — standard streaming mode.
pub async fn post_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    handle_messages(state, headers, &mut payload, false).await
}

/// Handler for `POST /cc/v1/messages` — buffered mode for Claude Code.
/// Collects all upstream events before flushing, so input_tokens can be
/// rewritten with the actual value from context-usage feedback.
pub async fn post_messages_cc(
    State(state): State<AppState>,
    headers: HeaderMap,
    JsonExtractor(mut payload): JsonExtractor<MessagesRequest>,
) -> Response {
    handle_messages(state, headers, &mut payload, true).await
}

// Shared implementation for both /v1/messages and /cc/v1/messages.
// Authenticates the key, converts the request, and dispatches to the
// appropriate stream/non-stream handler.
async fn handle_messages(
    state: AppState,
    headers: HeaderMap,
    payload: &mut MessagesRequest,
    buffered_for_cc: bool,
) -> Response {
    let (key_record, mut event_context) = match state.authenticate_kiro_key(&headers).await {
        Ok(value) => value,
        Err(err) => return err.into_response(),
    };
    event_context.client_request_body_json = serde_json::to_string(&*payload).ok();
    let request_validation_enabled = key_record.kiro_request_validation_enabled;
    let requested_model = payload.model.clone();
    if let Some((source_model, target_model)) = apply_key_model_mapping(&key_record, payload) {
        tracing::info!(
            key_id = %key_record.id,
            key_name = %key_record.name,
            requested_model = %source_model,
            effective_model = %target_model,
            "applied kiro key model mapping before request conversion"
        );
    }
    let public_path = if buffered_for_cc { "/cc/v1/messages" } else { "/v1/messages" };
    event_context.request_url.push_str(public_path);
    event_context.model = Some(payload.model.clone());
    event_context.last_message_content = extract_last_message_content(payload);
    let pure_web_search = websearch::has_web_search_tool(payload);
    let tool_names = payload
        .tools
        .as_ref()
        .map(|tools| {
            tools
                .iter()
                .map(|tool| tool.name.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let web_search_tool_count = payload
        .tools
        .as_ref()
        .map(|tools| tools.iter().filter(|tool| tool.is_web_search()).count())
        .unwrap_or(0);
    tracing::info!(
        requested_model = %requested_model,
        effective_model = %payload.model,
        model_mapping_applied = requested_model != payload.model,
        stream = payload.stream,
        buffered_for_cc,
        route = if pure_web_search { "mcp_web_search" } else { "assistant_generate" },
        message_count = payload.messages.len(),
        tool_count = tool_names.len(),
        web_search_tool_count,
        request_validation_enabled,
        tool_names = ?tool_names,
        "received kiro anthropic request"
    );
    let input_tokens = token::count_all_tokens(
        payload.model.clone(),
        payload.system.clone(),
        payload.messages.clone(),
        payload.tools.clone(),
    ) as i32;
    override_thinking_from_model_name(payload);
    let provider = KiroProvider::new(state.kiro_gateway.clone());
    if pure_web_search {
        event_context.endpoint = "/mcp".to_string();
        return handle_websearch_request(
            state,
            key_record,
            event_context,
            &provider,
            payload,
            input_tokens,
        )
        .await;
    }
    // Normalize only transport noise on a working copy before validation.
    // The raw client payload stays untouched in event_context for auditing.
    let normalized = match normalize_request(payload) {
        Ok(result) => result,
        Err(err) => {
            let message = match &err {
                ConversionError::UnsupportedModel(model) => format!("Unsupported model: {model}"),
                ConversionError::EmptyMessages => "messages are empty".to_string(),
                ConversionError::InvalidRequest(message) => message.clone(),
            };
            tracing::error!(
                key_id = %key_record.id,
                key_name = %key_record.name,
                route = public_path,
                requested_model = %requested_model,
                effective_model = %payload.model,
                stream = payload.stream,
                buffered_for_cc,
                request_validation_enabled,
                error = %message,
                "rejected malformed kiro public request before upstream call"
            );
            let diagnostic_payload = build_failure_diagnostic_payload(
                DiagnosticRequestContext {
                    event_context: &event_context,
                    request_validation_enabled,
                    stream: payload.stream,
                    buffered_for_cc,
                },
                "request_validation",
                &message,
                StatusCode::BAD_REQUEST.as_u16() as i32,
                Some(serde_json::json!({
                    "public_route": public_path,
                    "requested_model": requested_model,
                    "effective_model": payload.model,
                })),
            );
            if let Err(persist_err) = crate::kiro_gateway::record_failed_request_event(
                &state,
                &key_record,
                &event_context,
                StatusCode::BAD_REQUEST.as_u16() as i32,
                diagnostic_payload,
                zero_usage_summary(),
                false,
            )
            .await
            {
                tracing::warn!(
                    "failed to persist kiro validation failure usage event: {persist_err:#}"
                );
            }
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::new("invalid_request_error", message)),
            )
                .into_response();
        },
    };
    let normalization_log_ctx = NormalizationLogContext {
        key_record: &key_record,
        public_path,
        requested_model: &requested_model,
        effective_model: &payload.model,
        stream: payload.stream,
        buffered_for_cc,
        request_validation_enabled,
    };
    for event in &normalized.normalization_events {
        log_normalization_event(event, &normalization_log_ctx);
    }
    for event in &normalized.tool_normalization_events {
        log_tool_normalization_event(event, &normalization_log_ctx);
    }
    log_tool_validation_summary(&normalized, &normalization_log_ctx);
    for rewrite in &normalized.tool_use_id_rewrites {
        tracing::warn!(
            key_id = %key_record.id,
            key_name = %key_record.name,
            route = public_path,
            requested_model = %requested_model,
            effective_model = %payload.model,
            stream = payload.stream,
            buffered_for_cc,
            request_validation_enabled,
            original_tool_use_id = %rewrite.original_tool_use_id,
            rewritten_tool_use_id = %rewrite.rewritten_tool_use_id,
            assistant_message_index = rewrite.assistant_message_index,
            content_block_index = rewrite.content_block_index,
            rewritten_tool_result_count = rewrite.rewritten_tool_result_count,
            "rewrote duplicate completed tool_use id before upstream call"
        );
    }
    let conversion =
        match convert_normalized_request_with_validation(normalized, request_validation_enabled) {
            Ok(result) => result,
            Err(err) => {
                let message = match &err {
                    ConversionError::UnsupportedModel(model) => {
                        format!("Unsupported model: {model}")
                    },
                    ConversionError::EmptyMessages => "messages are empty".to_string(),
                    ConversionError::InvalidRequest(message) => message.clone(),
                };
                tracing::error!(
                    key_id = %key_record.id,
                    key_name = %key_record.name,
                    route = public_path,
                    requested_model = %requested_model,
                    effective_model = %payload.model,
                    stream = payload.stream,
                    buffered_for_cc,
                    request_validation_enabled,
                    error = %message,
                    "rejected malformed kiro public request before upstream call"
                );
                let diagnostic_payload = build_failure_diagnostic_payload(
                    DiagnosticRequestContext {
                        event_context: &event_context,
                        request_validation_enabled,
                        stream: payload.stream,
                        buffered_for_cc,
                    },
                    "request_validation",
                    &message,
                    StatusCode::BAD_REQUEST.as_u16() as i32,
                    Some(serde_json::json!({
                        "public_route": public_path,
                        "requested_model": requested_model,
                        "effective_model": payload.model,
                    })),
                );
                if let Err(persist_err) = crate::kiro_gateway::record_failed_request_event(
                    &state,
                    &key_record,
                    &event_context,
                    StatusCode::BAD_REQUEST.as_u16() as i32,
                    diagnostic_payload,
                    zero_usage_summary(),
                    false,
                )
                .await
                {
                    tracing::warn!(
                        "failed to persist kiro validation failure usage event: {persist_err:#}"
                    );
                }
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::new("invalid_request_error", message)),
                )
                    .into_response();
            },
        };
    let conversation_state = conversion.conversation_state;
    let tool_name_map = conversion.tool_name_map;
    event_context.upstream_request_body_json =
        serde_json::to_string(&crate::kiro_gateway::wire::KiroRequest {
            conversation_state: conversation_state.clone(),
            profile_arn: None,
        })
        .ok();
    let thinking_enabled = payload
        .thinking
        .as_ref()
        .map(Thinking::is_enabled)
        .unwrap_or(false);
    event_context.endpoint = "/generateAssistantResponse".to_string();

    if payload.stream {
        let response = match provider
            .call_api_stream(&key_record, &conversation_state)
            .await
        {
            Ok(response) => response,
            Err(err) => {
                return map_provider_error(
                    ProviderFailureContext {
                        state: &state,
                        key_record: &key_record,
                        diagnostic: DiagnosticRequestContext {
                            event_context: &event_context,
                            request_validation_enabled,
                            stream: true,
                            buffered_for_cc,
                        },
                    },
                    err,
                    "provider_call_stream",
                )
                .await;
            },
        };
        event_context.upstream_request_body_json = Some(response.request_body.clone());
        event_context.account_name = Some(response.account_name);
        if buffered_for_cc {
            return handle_stream_request_buffered(
                UsagePersistContext {
                    state,
                    key_record,
                    event_context,
                },
                response.response,
                &payload.model,
                input_tokens,
                thinking_enabled,
                tool_name_map,
                request_validation_enabled,
            )
            .await;
        }
        return handle_stream_request(
            UsagePersistContext {
                state,
                key_record,
                event_context,
            },
            response.response,
            &payload.model,
            input_tokens,
            thinking_enabled,
            tool_name_map,
            request_validation_enabled,
        )
        .await;
    }

    let response = match provider.call_api(&key_record, &conversation_state).await {
        Ok(response) => response,
        Err(err) => {
            return map_provider_error(
                ProviderFailureContext {
                    state: &state,
                    key_record: &key_record,
                    diagnostic: DiagnosticRequestContext {
                        event_context: &event_context,
                        request_validation_enabled,
                        stream: false,
                        buffered_for_cc,
                    },
                },
                err,
                "provider_call_non_stream",
            )
            .await;
        },
    };
    event_context.upstream_request_body_json = Some(response.request_body.clone());
    event_context.account_name = Some(response.account_name);
    handle_non_stream_request(
        state,
        key_record,
        event_context,
        response.response,
        NonStreamRequestContext {
            model: payload.model.clone(),
            input_tokens,
            tool_name_map,
            request_validation_enabled,
        },
    )
    .await
}

// Streams SSE events directly to the client as they arrive from Kiro.
// Usage is persisted asynchronously via a oneshot channel after the stream
// ends.
async fn handle_stream_request(
    usage_ctx: UsagePersistContext,
    response: reqwest::Response,
    model: &str,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    request_validation_enabled: bool,
) -> Response {
    let (done_tx, done_rx) = oneshot::channel::<UsagePersistOutcome>();
    let log_ctx = KiroUpstreamLogContext::new(
        &usage_ctx.key_record,
        usage_ctx.event_context.account_name.as_deref(),
        model,
        false,
    );
    let stream = create_sse_stream(
        response,
        StreamContext::new_with_thinking(model, input_tokens, thinking_enabled, tool_name_map),
        log_ctx,
        usage_ctx.event_context.clone(),
        request_validation_enabled,
        done_tx,
        usage_ctx.state.shutdown_rx.clone(),
    );
    tokio::spawn(async move {
        if let Ok(outcome) = done_rx.await {
            let persist_result = match outcome {
                UsagePersistOutcome::Success {
                    summary,
                    usage_missing,
                } => {
                    record_messages_usage(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &usage_ctx.event_context,
                        summary,
                        usage_missing,
                    )
                    .await
                },
                UsagePersistOutcome::Failure {
                    status_code,
                    summary,
                    usage_missing,
                    diagnostic_payload,
                } => {
                    crate::kiro_gateway::record_failed_request_event(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &usage_ctx.event_context,
                        status_code,
                        diagnostic_payload,
                        summary,
                        usage_missing,
                    )
                    .await
                },
            };
            if let Err(err) = persist_result {
                tracing::warn!("failed to persist kiro usage event: {err:#}");
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

// Buffers all Kiro events, then flushes them as SSE in one burst.
// Allows rewriting input_tokens in message_start with the actual value.
async fn handle_stream_request_buffered(
    usage_ctx: UsagePersistContext,
    response: reqwest::Response,
    model: &str,
    estimated_input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    request_validation_enabled: bool,
) -> Response {
    let (done_tx, done_rx) = oneshot::channel::<UsagePersistOutcome>();
    let log_ctx = KiroUpstreamLogContext::new(
        &usage_ctx.key_record,
        usage_ctx.event_context.account_name.as_deref(),
        model,
        true,
    );
    let stream = create_buffered_sse_stream(
        response,
        BufferedStreamContext::new(model, estimated_input_tokens, thinking_enabled, tool_name_map),
        log_ctx,
        usage_ctx.event_context.clone(),
        request_validation_enabled,
        done_tx,
        usage_ctx.state.shutdown_rx.clone(),
    );
    tokio::spawn(async move {
        if let Ok(outcome) = done_rx.await {
            let persist_result = match outcome {
                UsagePersistOutcome::Success {
                    summary,
                    usage_missing,
                } => {
                    record_messages_usage(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &usage_ctx.event_context,
                        summary,
                        usage_missing,
                    )
                    .await
                },
                UsagePersistOutcome::Failure {
                    status_code,
                    summary,
                    usage_missing,
                    diagnostic_payload,
                } => {
                    crate::kiro_gateway::record_failed_request_event(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &usage_ctx.event_context,
                        status_code,
                        diagnostic_payload,
                        summary,
                        usage_missing,
                    )
                    .await
                },
            };
            if let Err(err) = persist_result {
                tracing::warn!("failed to persist kiro usage event: {err:#}");
            }
        }
    });

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(stream))
        .unwrap()
}

// Reads the full Kiro response body, decodes all events, assembles a
// single JSON response with content blocks, and persists usage synchronously.
async fn handle_non_stream_request(
    state: AppState,
    key_record: static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord,
    event_context: KiroEventContext,
    response: reqwest::Response,
    request_ctx: NonStreamRequestContext,
) -> Response {
    let log_ctx = KiroUpstreamLogContext::new(
        &key_record,
        event_context.account_name.as_deref(),
        &request_ctx.model,
        false,
    );
    tracing::info!(
        model = request_ctx.model,
        input_tokens = request_ctx.input_tokens,
        "starting kiro non-stream upstream request"
    );
    let body = match response.bytes().await {
        Ok(body) => body,
        Err(err) => {
            log_kiro_stream_read_error(&log_ctx, "non_stream_body", &err);
            let diagnostic_payload = build_failure_diagnostic_payload(
                DiagnosticRequestContext {
                    event_context: &event_context,
                    request_validation_enabled: request_ctx.request_validation_enabled,
                    stream: false,
                    buffered_for_cc: false,
                },
                "non_stream_body_read",
                &err.to_string(),
                StatusCode::BAD_GATEWAY.as_u16() as i32,
                Some(serde_json::json!({
                    "stream_kind": "non_stream_body",
                    "is_timeout": err.is_timeout(),
                    "is_connect": err.is_connect(),
                    "upstream_url": err.url().map(|url| url.to_string()),
                })),
            );
            if let Err(persist_err) = crate::kiro_gateway::record_failed_request_event(
                &state,
                &key_record,
                &event_context,
                StatusCode::BAD_GATEWAY.as_u16() as i32,
                diagnostic_payload,
                zero_usage_summary(),
                false,
            )
            .await
            {
                tracing::warn!(
                    "failed to persist kiro non-stream body read failure: {persist_err:#}"
                );
            }
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse::new(
                    "api_error",
                    format!("Failed to read kiro response: {err}"),
                )),
            )
                .into_response();
        },
    };
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(&body);
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();
    let mut stop_reason = "end_turn".to_string();
    let mut context_input_tokens = None;
    let mut credit_usage = 0.0;
    let mut credit_usage_observed = false;
    let mut tool_json_buffers = std::collections::HashMap::<String, String>::new();
    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(Event::AssistantResponse(event)) => text_content.push_str(&event.content),
                Ok(Event::ToolUse(event)) => {
                    let buffer = tool_json_buffers
                        .entry(event.tool_use_id.clone())
                        .or_default();
                    buffer.push_str(&event.input);
                    if event.stop {
                        let input = if buffer.is_empty() {
                            serde_json::json!({})
                        } else {
                            serde_json::from_str(buffer).unwrap_or_else(|_| serde_json::json!({}))
                        };
                        let original_name = request_ctx
                            .tool_name_map
                            .get(&event.name)
                            .cloned()
                            .unwrap_or_else(|| event.name.clone());
                        tool_uses.push(serde_json::json!({
                            "type":"tool_use",
                            "id":event.tool_use_id,
                            "name":original_name,
                            "input":input
                        }));
                    }
                },
                Ok(Event::ContextUsage(event)) => {
                    let actual_input_tokens = (event.context_usage_percentage
                        * converter::get_context_window_size(&request_ctx.model) as f64
                        / 100.0) as i32;
                    context_input_tokens = Some(actual_input_tokens);
                    if event.context_usage_percentage >= 100.0 {
                        stop_reason = "model_context_window_exceeded".to_string();
                    }
                },
                Ok(Event::Metering(event)) => {
                    if let Some(usage) = event.credit_usage() {
                        credit_usage += usage;
                        credit_usage_observed = true;
                    }
                },
                Ok(
                    ref event @ Event::Error {
                        ..
                    },
                ) => {
                    log_kiro_upstream_event(&log_ctx, "non_stream", event);
                },
                Ok(
                    ref event @ Event::Exception {
                        ref exception_type, ..
                    },
                ) => {
                    if exception_type == "ContentLengthExceededException" {
                        stop_reason = "max_tokens".to_string();
                    }
                    log_kiro_upstream_event(&log_ctx, "non_stream", event);
                },
                Ok(Event::Unknown {}) => {},
                Err(err) => log_kiro_event_parse_error(&log_ctx, "non_stream_frame", &err),
            },
            Err(err) => log_kiro_event_parse_error(&log_ctx, "non_stream_decoder", &err),
        }
    }

    if !tool_uses.is_empty() && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }
    let mut content = Vec::new();
    if !text_content.is_empty() {
        content.push(serde_json::json!({"type":"text","text":text_content}));
    }
    content.extend(tool_uses);
    let output_tokens = token::estimate_output_tokens(&content);
    let usage = KiroUsageSummary {
        input_uncached_tokens: context_input_tokens.unwrap_or(request_ctx.input_tokens),
        input_cached_tokens: 0,
        output_tokens,
        credit_usage: credit_usage_observed.then_some(credit_usage.max(0.0)),
        credit_usage_missing: !credit_usage_observed,
    };
    tracing::info!(
        model = %request_ctx.model,
        stop_reason = %stop_reason,
        content_block_count = content.len(),
        usage_input_uncached_tokens = usage.input_uncached_tokens,
        usage_input_cached_tokens = usage.input_cached_tokens,
        usage_output_tokens = usage.output_tokens,
        "finished kiro non-stream request"
    );
    if let Err(err) = record_messages_usage(&state, &key_record, &event_context, usage, false).await
    {
        tracing::warn!("failed to persist kiro usage event: {err:#}");
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
            "type": "message",
            "role": "assistant",
            "content": content,
            "model": request_ctx.model,
            "stop_reason": stop_reason,
            "stop_sequence": null,
            "usage": {
                "input_tokens": usage.input_uncached_tokens + usage.input_cached_tokens,
                "output_tokens": usage.output_tokens,
                "cache_creation_input_tokens": 0,
                "cache_read_input_tokens": usage.input_cached_tokens,
            }
        })),
    )
        .into_response()
}

// Extracts a compact human-readable summary of the current user turn for
// logging/event context. Uses the same trailing-user-turn boundary as the
// converter so logs stay aligned with the actual upstream request shape.
fn extract_last_message_content(payload: &MessagesRequest) -> Option<String> {
    let current_range = current_user_message_range(&payload.messages).ok()?;
    let tool_name_by_id = collect_tool_name_map(&payload.messages[..current_range.start]);
    let mut parts = Vec::new();
    for message in &payload.messages[current_range] {
        append_message_summary_parts(&message.content, &tool_name_by_id, &mut parts);
    }
    if parts.is_empty() {
        None
    } else {
        Some(truncate_summary(&parts.join("\n"), KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS))
    }
}

fn collect_tool_name_map(messages: &[types::Message]) -> std::collections::HashMap<String, String> {
    let mut tool_name_by_id = std::collections::HashMap::new();
    for message in messages {
        let Some(blocks) = message.content.as_array() else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(|value| value.as_str()) != Some("tool_use") {
                continue;
            }
            let Some(tool_use_id) = block
                .get("id")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            let Some(tool_name) = block
                .get("name")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
            else {
                continue;
            };
            tool_name_by_id.insert(tool_use_id.to_string(), tool_name.to_string());
        }
    }
    tool_name_by_id
}

fn append_message_summary_parts(
    content: &serde_json::Value,
    tool_name_by_id: &std::collections::HashMap<String, String>,
    parts: &mut Vec<String>,
) {
    match content {
        serde_json::Value::String(text) => {
            if let Some(summary) = summarize_text(text) {
                parts.push(summary);
            }
        },
        serde_json::Value::Array(blocks) => {
            for block in blocks {
                match block.get("type").and_then(|value| value.as_str()) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
                            if let Some(summary) = summarize_text(text) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("tool_result") => {
                        if let Some(summary) = summarize_tool_result(block, tool_name_by_id) {
                            parts.push(summary);
                        }
                    },
                    Some("tool_use") => {
                        if let Some(name) = block.get("name").and_then(|value| value.as_str()) {
                            if let Some(summary) = summarize_text(&format!("[tool_use:{name}]")) {
                                parts.push(summary);
                            }
                        }
                    },
                    Some("image") => parts.push("[image]".to_string()),
                    _ => {},
                }
            }
        },
        _ => {},
    }
}

fn summarize_tool_result(
    block: &serde_json::Value,
    tool_name_by_id: &std::collections::HashMap<String, String>,
) -> Option<String> {
    let tool_use_id = block
        .get("tool_use_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let label = tool_name_by_id
        .get(tool_use_id)
        .map(String::as_str)
        .unwrap_or(tool_use_id);
    let preview = extract_tool_result_content(&block.get("content").cloned());
    let preview = compact_preview(&preview, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    Some(if preview.is_empty() {
        format!("[tool_result:{label}]")
    } else {
        format!("[tool_result:{label}] {preview}")
    })
}

fn summarize_text(text: &str) -> Option<String> {
    let preview = compact_preview(text, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS);
    (!preview.is_empty()).then_some(preview)
}

fn compact_preview(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_summary(compact.trim(), max_chars)
}

fn truncate_summary(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn create_sse_stream(
    response: reqwest::Response,
    mut ctx: StreamContext,
    log_ctx: KiroUpstreamLogContext,
    event_context: KiroEventContext,
    request_validation_enabled: bool,
    done_tx: oneshot::Sender<UsagePersistOutcome>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream! {
        tracing::info!(
            model = %ctx.model,
            estimated_input_tokens = ctx.input_tokens,
            thinking_enabled = ctx.thinking_enabled,
            "starting kiro streaming response"
        );
        for event in ctx.generate_initial_events() {
            yield Ok(Bytes::from(event.to_sse_string()));
        }
        let mut body_stream = response.bytes_stream();
        let mut decoder = EventStreamDecoder::new();
        let mut ping_interval = interval(Duration::from_secs(25));
        ping_interval.tick().await;
        let mut done_tx = Some(done_tx);
        let mut failure_diagnostic_payload = None;

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(
                            model = %ctx.model,
                            "stopping kiro streaming response because backend is shutting down"
                        );
                        break;
                    }
                }
                _ = ping_interval.tick() => {
                    yield Ok(Bytes::from("event: ping\ndata: {\"type\":\"ping\"}\n\n"));
                }
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            let _ = decoder.feed(&chunk);
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => match Event::from_frame(frame) {
                                        Ok(event) => {
                                            log_kiro_upstream_event(&log_ctx, "stream", &event);
                                            for sse_event in ctx.process_kiro_event(&event) {
                                                yield Ok(Bytes::from(sse_event.to_sse_string()));
                                            }
                                        },
                                        Err(err) => {
                                            log_kiro_event_parse_error(&log_ctx, "stream_frame", &err);
                                        },
                                    },
                                    Err(err) => {
                                        log_kiro_event_parse_error(&log_ctx, "stream_decoder", &err);
                                    },
                                }
                            }
                        }
                        Some(Err(err)) => {
                            log_kiro_stream_read_error(&log_ctx, "stream", &err);
                            failure_diagnostic_payload = Some(build_failure_diagnostic_payload(
                                DiagnosticRequestContext {
                                    event_context: &event_context,
                                    request_validation_enabled,
                                    stream: true,
                                    buffered_for_cc: false,
                                },
                                "stream_read",
                                &err.to_string(),
                                KIRO_STREAM_FAILURE_STATUS_CODE,
                                Some(serde_json::json!({
                                    "stream_kind": "stream",
                                    "is_timeout": err.is_timeout(),
                                    "is_connect": err.is_connect(),
                                    "upstream_url": err.url().map(|url| url.to_string()),
                                })),
                            ));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        let final_events = ctx.generate_final_events();
        let (input_tokens, output_tokens) = ctx.final_usage();
        let (credit_usage, credit_usage_missing) = ctx.final_credit_usage();
        tracing::info!(
            model = %ctx.model,
            final_event_count = final_events.len(),
            input_tokens,
            output_tokens,
            credit_usage = credit_usage.unwrap_or_default(),
            credit_usage_missing,
            "finished kiro streaming response"
        );
        if let Some(sender) = done_tx.take() {
            let summary = KiroUsageSummary {
                input_uncached_tokens: input_tokens,
                input_cached_tokens: 0,
                output_tokens,
                credit_usage,
                credit_usage_missing,
            };
            let _ = match failure_diagnostic_payload {
                Some(diagnostic_payload) => sender.send(UsagePersistOutcome::Failure {
                    status_code: KIRO_STREAM_FAILURE_STATUS_CODE,
                    summary,
                    usage_missing: true,
                    diagnostic_payload,
                }),
                None => sender.send(UsagePersistOutcome::Success {
                    summary,
                    usage_missing: false,
                }),
            };
        }
        for event in final_events {
            yield Ok(Bytes::from(event.to_sse_string()));
        }
    }
}

fn create_buffered_sse_stream(
    response: reqwest::Response,
    mut ctx: BufferedStreamContext,
    log_ctx: KiroUpstreamLogContext,
    event_context: KiroEventContext,
    request_validation_enabled: bool,
    done_tx: oneshot::Sender<UsagePersistOutcome>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> impl Stream<Item = Result<Bytes, Infallible>> {
    stream! {
        tracing::info!(
            model = %ctx.model(),
            estimated_input_tokens = ctx.estimated_input_tokens(),
            thinking_enabled = ctx.thinking_enabled(),
            "starting kiro buffered streaming response"
        );
        let mut body_stream = response.bytes_stream();
        let mut decoder = EventStreamDecoder::new();
        let mut ping_interval = interval(Duration::from_secs(25));
        ping_interval.tick().await;
        let mut done_tx = Some(done_tx);
        let mut failure_diagnostic_payload = None;

        loop {
            tokio::select! {
                biased;
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!(
                            model = %ctx.model(),
                            "stopping kiro buffered streaming response because backend is shutting down"
                        );
                        break;
                    }
                }
                _ = ping_interval.tick() => {
                    yield Ok(Bytes::from("event: ping\ndata: {\"type\":\"ping\"}\n\n"));
                }
                chunk_result = body_stream.next() => {
                    match chunk_result {
                        Some(Ok(chunk)) => {
                            let _ = decoder.feed(&chunk);
                            for result in decoder.decode_iter() {
                                match result {
                                    Ok(frame) => match Event::from_frame(frame) {
                                        Ok(event) => {
                                            log_kiro_upstream_event(&log_ctx, "buffered_stream", &event);
                                            ctx.process_and_buffer(&event);
                                        },
                                        Err(err) => {
                                            log_kiro_event_parse_error(
                                                &log_ctx,
                                                "buffered_stream_frame",
                                                &err,
                                            );
                                        },
                                    },
                                    Err(err) => {
                                        log_kiro_event_parse_error(
                                            &log_ctx,
                                            "buffered_stream_decoder",
                                            &err,
                                        );
                                    },
                                }
                            }
                        }
                        Some(Err(err)) => {
                            log_kiro_stream_read_error(&log_ctx, "buffered_stream", &err);
                            failure_diagnostic_payload = Some(build_failure_diagnostic_payload(
                                DiagnosticRequestContext {
                                    event_context: &event_context,
                                    request_validation_enabled,
                                    stream: true,
                                    buffered_for_cc: true,
                                },
                                "buffered_stream_read",
                                &err.to_string(),
                                KIRO_STREAM_FAILURE_STATUS_CODE,
                                Some(serde_json::json!({
                                    "stream_kind": "buffered_stream",
                                    "is_timeout": err.is_timeout(),
                                    "is_connect": err.is_connect(),
                                    "upstream_url": err.url().map(|url| url.to_string()),
                                })),
                            ));
                            break;
                        }
                        None => break,
                    }
                }
            }
        }

        let all_events = ctx.finish_and_get_all_events();
        let (input_tokens, output_tokens) = ctx.final_usage();
        let (credit_usage, credit_usage_missing) = ctx.final_credit_usage();
        tracing::info!(
            model = %ctx.model(),
            buffered_event_count = all_events.len(),
            input_tokens,
            output_tokens,
            credit_usage = credit_usage.unwrap_or_default(),
            credit_usage_missing,
            "finished kiro buffered streaming response"
        );
        if let Some(sender) = done_tx.take() {
            let summary = KiroUsageSummary {
                input_uncached_tokens: input_tokens,
                input_cached_tokens: 0,
                output_tokens,
                credit_usage,
                credit_usage_missing,
            };
            let _ = match failure_diagnostic_payload {
                Some(diagnostic_payload) => sender.send(UsagePersistOutcome::Failure {
                    status_code: KIRO_STREAM_FAILURE_STATUS_CODE,
                    summary,
                    usage_missing: true,
                    diagnostic_payload,
                }),
                None => sender.send(UsagePersistOutcome::Success {
                    summary,
                    usage_missing: false,
                }),
            };
        }
        for event in all_events {
            yield Ok(Bytes::from(event.to_sse_string()));
        }
    }
}

fn model(id: &str, display_name: &str, created: i64) -> Model {
    Model {
        id: id.to_string(),
        object: "model".to_string(),
        created,
        owned_by: "anthropic".to_string(),
        display_name: display_name.to_string(),
        model_type: "chat".to_string(),
        max_tokens: 32_000,
    }
}

fn apply_key_model_mapping(
    key_record: &LlmGatewayKeyRecord,
    payload: &mut MessagesRequest,
) -> Option<(String, String)> {
    let target_model = key_record
        .model_name_map
        .as_ref()
        .and_then(|map| map.get(&payload.model))
        .cloned()?;
    if target_model == payload.model {
        return None;
    }
    let source_model = payload.model.clone();
    payload.model = target_model.clone();
    Some((source_model, target_model))
}

/// If the model name contains "-thinking", auto-inject thinking configuration.
/// Opus 4.6 gets adaptive/high; all others get enabled with 20K budget.
fn override_thinking_from_model_name(payload: &mut MessagesRequest) {
    let model = payload.model.to_lowercase();
    if !model.contains("thinking") {
        return;
    }
    let is_opus_46 = model.contains("opus") && (model.contains("4-6") || model.contains("4.6"));
    payload.thinking = Some(Thinking {
        thinking_type: if is_opus_46 { "adaptive".to_string() } else { "enabled".to_string() },
        budget_tokens: 20_000,
    });
    if is_opus_46 {
        payload.output_config = Some(OutputConfig {
            effort: "high".to_string(),
        });
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn base_request(model: &str) -> MessagesRequest {
        MessagesRequest {
            model: model.to_string(),
            _max_tokens: 1024,
            messages: vec![types::Message {
                role: "user".to_string(),
                content: json!("hello"),
            }],
            stream: false,
            system: None,
            tools: None,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    fn sample_key(model_name_map: Option<Vec<(&str, &str)>>) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "test-key".to_string(),
            name: "test".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 1_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: model_name_map.map(|entries| {
                entries
                    .into_iter()
                    .map(|(source, target)| (source.to_string(), target.to_string()))
                    .collect()
            }),
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
        }
    }

    #[test]
    fn key_model_mapping_rewrites_requested_model_before_conversion() {
        let key = sample_key(Some(vec![("claude-haiku-4-5-20251001", "claude-sonnet-4-6")]));
        let mut payload = base_request("claude-haiku-4-5-20251001");

        let applied = apply_key_model_mapping(&key, &mut payload);

        assert_eq!(
            applied,
            Some(("claude-haiku-4-5-20251001".to_string(), "claude-sonnet-4-6".to_string()))
        );
        assert_eq!(payload.model, "claude-sonnet-4-6");
    }

    #[test]
    fn key_model_mapping_keeps_identity_when_no_override_exists() {
        let key = sample_key(None);
        let mut payload = base_request("claude-haiku-4-5-20251001");

        let applied = apply_key_model_mapping(&key, &mut payload);

        assert!(applied.is_none());
        assert_eq!(payload.model, "claude-haiku-4-5-20251001");
    }

    #[test]
    fn thinking_suffix_sets_enabled_mode_for_non_opus_46_models() {
        let mut payload = base_request("claude-sonnet-4-6-thinking");
        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should be injected");
        assert_eq!(thinking.thinking_type, "enabled");
        assert_eq!(thinking.budget_tokens, 20_000);
        assert!(payload.output_config.is_none());
    }

    #[test]
    fn thinking_suffix_sets_adaptive_high_for_opus_46_models() {
        let mut payload = base_request("claude-opus-4-6-thinking");
        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should be injected");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(thinking.budget_tokens, 20_000);
        assert_eq!(
            payload
                .output_config
                .as_ref()
                .map(|config| config.effort.as_str()),
            Some("high")
        );
    }

    #[test]
    fn classify_provider_error_maps_minimum_remaining_threshold_to_payment_required() {
        let (status, error_type, message) = classify_provider_error(
            "all configured kiro accounts are below the configured minimum remaining credits \
             threshold",
        );

        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(error_type, "rate_limit_error");
        assert!(message.contains("minimum remaining credits threshold"));
    }

    #[test]
    fn non_thinking_model_does_not_override_existing_configuration() {
        let mut payload = base_request("claude-sonnet-4-6");
        payload.thinking = Some(Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 8192,
        });
        payload.output_config = Some(OutputConfig {
            effort: "medium".to_string(),
        });

        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should remain");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(thinking.budget_tokens, 8192);
        assert_eq!(
            payload
                .output_config
                .as_ref()
                .map(|config| config.effort.as_str()),
            Some("medium")
        );
    }

    #[test]
    fn failure_diagnostic_payload_embeds_structured_request_bodies() {
        let mut event_context = crate::kiro_gateway::KiroEventContext {
            account_name: Some("acct-a".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some(
                r#"{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hello"}]}"#
                    .to_string(),
            ),
            upstream_request_body_json: Some(
                r#"{"conversationState":{"conversationId":"conv-1"}}"#.to_string(),
            ),
            started_at: std::time::Instant::now(),
        };
        let payload = build_failure_diagnostic_payload(
            DiagnosticRequestContext {
                event_context: &event_context,
                request_validation_enabled: true,
                stream: true,
                buffered_for_cc: false,
            },
            "provider_call",
            "upstream returned 400",
            502,
            Some(serde_json::json!({
                "proxy_url": "http://127.0.0.1:11113",
                "upstream_status": 400
            })),
        );

        let parsed: serde_json::Value =
            serde_json::from_str(&payload).expect("diagnostic payload should be valid json");
        assert_eq!(parsed["kind"], "kiro_failure_diagnostic");
        assert_eq!(parsed["failure_stage"], "provider_call");
        assert_eq!(parsed["status_code"], 502);
        assert_eq!(parsed["original_last_message_content"], "hello");
        assert_eq!(parsed["client_request_body"]["model"], "claude-sonnet-4-6");
        assert_eq!(
            parsed["upstream_request_body"]["conversationState"]["conversationId"],
            "conv-1"
        );
        assert_eq!(parsed["details"]["proxy_url"], "http://127.0.0.1:11113");
        assert_eq!(parsed["details"]["upstream_status"], 400);

        event_context.client_request_body_json = Some("not-json".to_string());
        let payload = build_failure_diagnostic_payload(
            DiagnosticRequestContext {
                event_context: &event_context,
                request_validation_enabled: false,
                stream: false,
                buffered_for_cc: false,
            },
            "request_validation",
            "bad request",
            400,
            None,
        );
        let parsed: serde_json::Value =
            serde_json::from_str(&payload).expect("fallback diagnostic payload should be json");
        assert_eq!(parsed["client_request_body"], "not-json");
        assert!(parsed["upstream_request_body"].is_object());
    }

    #[test]
    fn extract_last_message_content_summarizes_trailing_user_tool_results() {
        let mut payload = base_request("claude-sonnet-4-6");
        payload.messages = vec![
            types::Message {
                role: "user".to_string(),
                content: json!("帮我获得这个的vip"),
            },
            types::Message {
                role: "assistant".to_string(),
                content: json!([
                    {
                        "type": "text",
                        "text": "好的，让我先分析一下这个 APK 的结构。"
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-manifest",
                        "name": "get_manifest",
                        "input": {}
                    },
                    {
                        "type": "tool_use",
                        "id": "tool-search",
                        "name": "search_classes",
                        "input": {"keyword": "vip"}
                    }
                ]),
            },
            types::Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-manifest",
                        "content": "manifest output"
                    }
                ]),
            },
            types::Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-search",
                        "content": "search output"
                    }
                ]),
            },
        ];

        let summary = extract_last_message_content(&payload);

        assert_eq!(
            summary.as_deref(),
            Some(
                "[tool_result:get_manifest] manifest output\n[tool_result:search_classes] search \
                 output"
            )
        );
    }

    #[test]
    fn extract_last_message_content_merges_trailing_user_text_and_tool_result() {
        let mut payload = base_request("claude-sonnet-4-6");
        payload.messages = vec![
            types::Message {
                role: "user".to_string(),
                content: json!("Read the file"),
            },
            types::Message {
                role: "assistant".to_string(),
                content: json!([
                    {
                        "type": "tool_use",
                        "id": "tool-1",
                        "name": "read_file",
                        "input": {"path": "/tmp/test.txt"}
                    }
                ]),
            },
            types::Message {
                role: "user".to_string(),
                content: json!("Please continue"),
            },
            types::Message {
                role: "user".to_string(),
                content: json!([
                    {
                        "type": "tool_result",
                        "tool_use_id": "tool-1",
                        "content": "file content"
                    }
                ]),
            },
        ];

        let summary = extract_last_message_content(&payload);

        assert_eq!(
            summary.as_deref(),
            Some("Please continue\n[tool_result:read_file] file content")
        );
    }

    #[test]
    fn normalize_request_reports_tool_description_fill_summary() {
        let mut payload = base_request("claude-sonnet-4-6");
        payload.tools = Some(vec![types::Tool {
            tool_type: None,
            name: "demo_tool".to_string(),
            description: "".to_string(),
            input_schema: std::collections::HashMap::from([
                ("type".to_string(), json!("object")),
                ("properties".to_string(), json!({})),
                ("required".to_string(), json!([])),
                ("additionalProperties".to_string(), json!(true)),
            ]),
            max_uses: None,
        }]);

        let normalized = normalize_request(&payload).expect("tool normalization should succeed");

        assert_eq!(
            normalized
                .tool_validation_summary
                .normalized_tool_description_count,
            1
        );
        assert_eq!(normalized.tool_validation_summary.empty_tool_name_count, 0);
        assert_eq!(normalized.tool_normalization_events.len(), 1);
        assert_eq!(normalized.tool_normalization_events[0].tool_index, 0);
        assert_eq!(normalized.tool_normalization_events[0].tool_name, "demo_tool");
        assert_eq!(normalized.tool_normalization_events[0].reason, "empty_tool_description");
    }
}
