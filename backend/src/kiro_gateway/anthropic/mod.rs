//! Anthropic-compatible API handler for the Kiro gateway.
//!
//! Routes `/v1/messages`, `/cc/v1/messages`, `/v1/models`, and `count_tokens`
//! requests. Converts Anthropic request payloads to Kiro wire format, streams
//! responses as SSE (with optional buffered mode for Claude Code), and persists
//! usage events.

use std::{convert::Infallible, time::Instant};

use async_stream::stream;
use axum::{
    body::{to_bytes, Body},
    extract::{Json as JsonExtractor, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json, Response},
};
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
pub(crate) use llm_access_kiro::anthropic::supported_model_ids;
use llm_access_kiro::anthropic::{
    count_tokens_response,
    stream::{KIRO_HIDDEN_PROMPT_BASELINE_TOKENS, KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS},
    supported_models_response,
};
use serde::de::DeserializeOwned;
use tokio::{
    sync::{oneshot, watch},
    time::{interval, Duration},
};

use super::{
    cache_sim::{
        KiroCacheSimulationConfig, KiroCacheSimulationMode, PrefixCacheMatch, PromptProjection,
    },
    parser::decoder::EventStreamDecoder,
    provider::{KiroProvider, ProviderCallError, ProviderCallResult},
    token, AppKiroStateExt, KiroUsageSummary,
};
use crate::kiro_gateway::{
    cache_policy::{
        adjust_input_tokens_for_cache_creation_cost_with_policy,
        prefix_tree_credit_ratio_cap_basis_points_with_policy, resolve_effective_kiro_cache_policy,
        should_capture_full_kiro_request_bodies,
    },
    record_messages_usage, FailedKiroRequestEvent, KiroEventContext,
};

pub mod converter;
pub mod stream;
pub mod types;
pub mod websearch;

use static_flow_shared::llm_gateway_store::{
    compute_kiro_billable_tokens, KiroCachePolicy, LlmGatewayKeyRecord,
    DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
};

use self::{
    converter::{
        classify_tool_name_rewrite_reason, convert_normalized_request_with_resolved_session,
        current_user_message_range, extract_tool_result_content, normalize_request,
        preview_session_value, resolve_conversation_id_from_metadata, ConversionError,
        NormalizationEvent, NormalizedRequest, ResolvedConversationId, SessionFallbackReason,
        SessionIdSource, SessionTracking, ToolNormalizationEvent,
    },
    stream::{build_inline_thinking_content_blocks, BufferedStreamContext, StreamContext},
    types::{CountTokensRequest, ErrorResponse, MessagesRequest, Metadata, OutputConfig, Thinking},
    websearch::handle_websearch_request,
};
use crate::{
    kiro_gateway::wire::{AssistantMessage, ConversationState, Event, ToolUseEntry},
    request_context::RequestReceivedAt,
    state::{AppState, LlmGatewayRuntimeConfig},
};

const KIRO_CLIENT_REQUEST_MAX_BODY_BYTES: usize =
    DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES as usize;

#[derive(Clone, Debug, Default)]
pub struct KiroRequestIngressTimings {
    pub request_body_bytes: u64,
    pub request_body_read_ms: Option<i32>,
    pub request_json_parse_ms: Option<i32>,
    pub pre_handler_ms: Option<i32>,
}

pub(super) fn clamp_u64_ms_to_i32(value: u64) -> i32 {
    value.min(i32::MAX as u64) as i32
}

fn elapsed_ms_i32(started_at: Instant) -> i32 {
    started_at.elapsed().as_millis().min(i32::MAX as u128) as i32
}

fn json_error_response(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ErrorResponse::new("invalid_request_error", message.into()))).into_response()
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    let Some(content_type) = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    media_type == "application/json" || media_type.ends_with("+json")
}

async fn read_timed_json_request<T>(
    request: Request,
) -> Result<(HeaderMap, T, KiroRequestIngressTimings), Response>
where
    T: DeserializeOwned,
{
    let (parts, body) = request.into_parts();
    let request_received_at = parts
        .extensions
        .get::<RequestReceivedAt>()
        .map(|value| value.0);
    let headers = parts.headers;

    if !is_json_content_type(&headers) {
        return Err(json_error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "Expected request with `Content-Type: application/json`",
        ));
    }

    let body_read_started_at = Instant::now();
    let body = to_bytes(body, KIRO_CLIENT_REQUEST_MAX_BODY_BYTES)
        .await
        .map_err(|err| {
            let status = if err
                .to_string()
                .to_ascii_lowercase()
                .contains("length limit")
            {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            };
            json_error_response(status, format!("Failed to read request body: {err}"))
        })?;
    let request_body_read_ms = Some(elapsed_ms_i32(body_read_started_at));

    let parse_started_at = Instant::now();
    let payload = serde_json::from_slice::<T>(&body).map_err(|err| {
        json_error_response(StatusCode::BAD_REQUEST, format!("Invalid JSON body: {err}"))
    })?;
    let request_json_parse_ms = Some(elapsed_ms_i32(parse_started_at));
    let pre_handler_ms = request_received_at.map(elapsed_ms_i32);

    Ok((headers, payload, KiroRequestIngressTimings {
        request_body_bytes: body.len().min(u64::MAX as usize) as u64,
        request_body_read_ms,
        request_json_parse_ms,
        pre_handler_ms,
    }))
}

fn apply_provider_metrics(event_context: &mut KiroEventContext, response: &ProviderCallResult) {
    event_context.routing_wait_ms = Some(clamp_u64_ms_to_i32(response.routing_wait_ms));
    event_context.upstream_headers_ms = Some(clamp_u64_ms_to_i32(response.upstream_headers_ms));
    event_context.upstream_headers_at = Some(Instant::now());
    event_context.quota_failover_count = response.quota_failover_count;
    event_context.routing_diagnostics_json = response.routing_diagnostics_json.clone();
}

const KIRO_UPSTREAM_LOG_PREVIEW_CHARS: usize = 8_192;
const KIRO_STREAM_FAILURE_STATUS_CODE: i32 = 599;
const KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS: usize = 160;
const KIRO_LAST_MESSAGE_TOTAL_PREVIEW_CHARS: usize = 1_024;
const TARGETED_DEBUG_KEY_HASH: &str =
    "16c6e5b7c8accc911719ea856027bda9fd3dd9ad98240a153dff6fb67d071df1";
const CLAUDE_CODE_BILLING_HEADER_PREFIX: &str = "x-anthropic-billing-header:";
const REQUEST_SESSION_ID_HEADERS: [&str; 8] = [
    "x-claude-code-session-id",
    "x-codex-session-id",
    "x-openclaw-session-id",
    "conversation_id",
    "conversation-id",
    "session_id",
    "session-id",
    "x-session-id",
];

fn extract_trimmed_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

// Bundles the state needed to persist usage after a streaming response
// completes.
struct UsagePersistContext {
    state: AppState,
    key_record: LlmGatewayKeyRecord,
    event_context: KiroEventContext,
    effective_cache_policy: KiroCachePolicy,
}

#[derive(Clone)]
struct StreamRequestContext {
    state: AppState,
    event_context: KiroEventContext,
    request_validation_enabled: bool,
    cache_estimation_enabled: bool,
    simulation: KiroSimulationRequestContext,
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
    effective_cache_policy: &'a KiroCachePolicy,
    diagnostic: DiagnosticRequestContext<'a>,
}

struct NonStreamRequestContext {
    model: String,
    input_tokens: i32,
    thinking_enabled: bool,
    tool_name_map: std::collections::HashMap<String, String>,
    structured_output_tool_name: Option<String>,
    request_validation_enabled: bool,
    simulation: KiroSimulationRequestContext,
}

#[derive(Clone)]
struct KiroSimulationRequestContext {
    runtime_config: LlmGatewayRuntimeConfig,
    effective_cache_policy: static_flow_shared::llm_gateway_store::KiroCachePolicy,
    simulation_config: KiroCacheSimulationConfig,
    projection: PromptProjection,
    prefix_cache_match: PrefixCacheMatch,
    conversation_id: String,
}

struct MissingUpstreamInputLogContext<'a> {
    key_id: &'a str,
    key_name: &'a str,
    event_context: &'a KiroEventContext,
    request_validation_enabled: bool,
    cache_estimation_enabled: bool,
    simulation: &'a KiroSimulationRequestContext,
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
}

fn resolve_request_session(
    headers: &HeaderMap,
    metadata: Option<&Metadata>,
) -> ResolvedConversationId {
    let mut first_invalid_header: Option<(&'static str, String)> = None;
    for header_name in REQUEST_SESSION_ID_HEADERS {
        let Some(raw_value) = extract_trimmed_header_value(headers, header_name) else {
            continue;
        };
        if uuid::Uuid::try_parse(&raw_value).is_ok() {
            return ResolvedConversationId {
                conversation_id: raw_value.clone(),
                session_tracking: SessionTracking {
                    source: SessionIdSource::RequestHeader,
                    source_name: Some(header_name),
                    source_value_preview: Some(preview_session_value(&raw_value)),
                },
            };
        }
        if first_invalid_header.is_none() {
            first_invalid_header = Some((header_name, preview_session_value(&raw_value)));
        }
    }

    let mut resolved = resolve_conversation_id_from_metadata(metadata);
    if matches!(resolved.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
        if let Some((header_name, preview)) = first_invalid_header {
            resolved.session_tracking = SessionTracking {
                source: SessionIdSource::GeneratedFallback(
                    SessionFallbackReason::InvalidHeaderSessionId,
                ),
                source_name: Some(header_name),
                source_value_preview: Some(preview),
            };
        }
    }
    resolved
}

fn prepare_simulation_request_context(
    state: &AppState,
    runtime_config: LlmGatewayRuntimeConfig,
    effective_cache_policy: KiroCachePolicy,
    conversation_state: ConversationState,
    session_tracking: SessionTracking,
    cache_estimation_enabled: bool,
) -> (ConversationState, SessionTracking, KiroSimulationRequestContext) {
    let simulation_config = KiroCacheSimulationConfig::from(&runtime_config);
    let projection = PromptProjection::from_conversation_state(&conversation_state);
    let now = std::time::Instant::now();
    let (conversation_state, session_tracking) = maybe_recover_conversation_id_from_anchor(
        state,
        conversation_state,
        session_tracking,
        &projection,
        simulation_config,
        now,
    );
    let prefix_cache_match =
        if should_run_prefix_cache_match(cache_estimation_enabled, simulation_config.mode) {
            state
                .kiro_gateway
                .cache_simulator
                .match_prefix(&projection, simulation_config, now)
        } else {
            PrefixCacheMatch::default()
        };
    let simulation = KiroSimulationRequestContext {
        runtime_config,
        effective_cache_policy,
        simulation_config,
        projection,
        prefix_cache_match,
        conversation_id: conversation_state.conversation_id.clone(),
    };
    (conversation_state, session_tracking, simulation)
}

fn should_run_prefix_cache_match(
    cache_estimation_enabled: bool,
    simulation_mode: KiroCacheSimulationMode,
) -> bool {
    cache_estimation_enabled && matches!(simulation_mode, KiroCacheSimulationMode::PrefixTree)
}

fn log_simulation_request_context(
    key_record: &LlmGatewayKeyRecord,
    ctx: &NormalizationLogContext<'_>,
    cache_estimation_enabled: bool,
    session_tracking: &SessionTracking,
    simulation: &KiroSimulationRequestContext,
) {
    let projected_total = simulation.projection.projected_input_token_count.max(1);
    let matched_tokens = simulation
        .prefix_cache_match
        .matched_tokens
        .min(projected_total);
    let matched_ratio_basis_points =
        ((u128::from(matched_tokens) * 10_000) / u128::from(projected_total)) as u64;
    let prefix_cache_active =
        should_run_prefix_cache_match(cache_estimation_enabled, simulation.simulation_config.mode);
    tracing::info!(
        key_id = %key_record.id,
        key_name = %key_record.name,
        route = ctx.public_path,
        requested_model = ctx.requested_model,
        effective_model = ctx.effective_model,
        stream = ctx.stream,
        buffered_for_cc = ctx.buffered_for_cc,
        request_validation_enabled = ctx.request_validation_enabled,
        cache_estimation_enabled,
        prefix_cache_active,
        cache_simulation_mode = match simulation.simulation_config.mode {
            KiroCacheSimulationMode::Formula => "formula",
            KiroCacheSimulationMode::PrefixTree => "prefix_tree",
        },
        session_resolution = session_resolution_label(&session_tracking.source),
        conversation_id = %simulation.conversation_id,
        lookup_anchor_preview = preview_session_value(&simulation.projection.lookup_anchor_hash),
        stable_prefix_page_count = simulation.projection.stable_prefix_pages.len(),
        stable_prefix_token_count = simulation.projection.stable_prefix_token_count(),
        projected_input_token_count = simulation.projection.projected_input_token_count,
        matched_page_count = simulation.prefix_cache_match.matched_pages,
        matched_token_count = simulation.prefix_cache_match.matched_tokens,
        matched_ratio_basis_points,
        "prepared kiro request context before upstream call"
    );
    if should_log_targeted_kiro_debug(key_record) {
        tracing::info!(
            key_id = %key_record.id,
            key_name = %key_record.name,
            route = ctx.public_path,
            requested_model = ctx.requested_model,
            effective_model = ctx.effective_model,
            stream = ctx.stream,
            buffered_for_cc = ctx.buffered_for_cc,
            lookup_anchor_hash = %simulation.projection.lookup_anchor_hash,
            history_anchor_segments = ?simulation.projection.history_anchor_segments(),
            stable_prefix_segment_keys = ?simulation.projection.stable_prefix_segment_keys(),
            current_turn_history_segments = ?simulation.projection.current_turn_history_segments(),
            projection_contains_billing_header = projection_contains_volatile_claude_code_marker(
                &simulation.projection
            ),
            "targeted kiro cache projection debug"
        );
    }
}

fn should_log_targeted_kiro_debug(key_record: &LlmGatewayKeyRecord) -> bool {
    key_record.key_hash == TARGETED_DEBUG_KEY_HASH
}

fn request_body_contains_volatile_claude_code_marker(body: Option<&str>) -> bool {
    body.is_some_and(|value| value.contains(CLAUDE_CODE_BILLING_HEADER_PREFIX))
}

fn projection_contains_volatile_claude_code_marker(projection: &PromptProjection) -> bool {
    projection
        .history_anchor_segments()
        .iter()
        .chain(projection.stable_prefix_segment_keys().iter())
        .chain(projection.current_turn_history_segments().iter())
        .any(|segment| segment.contains(CLAUDE_CODE_BILLING_HEADER_PREFIX))
}

fn log_targeted_request_rewrite_debug(
    key_record: &LlmGatewayKeyRecord,
    event_context: &KiroEventContext,
    payload: &MessagesRequest,
    requested_model: &str,
    buffered_for_cc: bool,
    input_tokens: i32,
) {
    if !should_log_targeted_kiro_debug(key_record) {
        return;
    }
    let transformed_request_body = serde_json::to_string(payload).unwrap_or_default();
    tracing::info!(
        key_id = %key_record.id,
        key_name = %key_record.name,
        requested_model,
        effective_model = %payload.model,
        stream = payload.stream,
        buffered_for_cc,
        input_tokens,
        thinking_enabled = payload
            .thinking
            .as_ref()
            .map(Thinking::is_enabled)
            .unwrap_or(false),
        thinking_type = payload
            .thinking
            .as_ref()
            .map(|thinking| thinking.thinking_type.as_str())
            .unwrap_or(""),
        thinking_budget_tokens = payload
            .thinking
            .as_ref()
            .map(|thinking| thinking.budget_tokens)
            .unwrap_or_default(),
        thinking_effort = payload
            .output_config
            .as_ref()
            .map(|config| config.effective_effort())
            .unwrap_or(""),
        output_format_type = payload
            .output_config
            .as_ref()
            .and_then(|config| config.format.as_ref())
            .map(|format| format.format_type.as_str())
            .unwrap_or(""),
        raw_client_contains_billing_header = request_body_contains_volatile_claude_code_marker(
            event_context.client_request_body_json.as_deref()
        ),
        transformed_request_contains_billing_header =
            request_body_contains_volatile_claude_code_marker(Some(&transformed_request_body)),
        raw_client_request_body = event_context.client_request_body_json.as_deref().unwrap_or(""),
        transformed_request_body,
        "targeted kiro anthropic request debug before normalization"
    );
}

fn log_targeted_non_stream_debug(
    key_record: &LlmGatewayKeyRecord,
    request_ctx: &NonStreamRequestContext,
    stop_reason: &str,
    raw_upstream_body: &str,
    assistant_message: &AssistantMessage,
    content: &[serde_json::Value],
) {
    if !should_log_targeted_kiro_debug(key_record) {
        return;
    }
    tracing::info!(
        key_id = %key_record.id,
        key_name = %key_record.name,
        model = %request_ctx.model,
        thinking_enabled = request_ctx.thinking_enabled,
        stop_reason,
        raw_upstream_contains_thinking = raw_upstream_body.contains("<thinking>"),
        assistant_message_contains_thinking = assistant_message.content.contains("<thinking>"),
        emitted_thinking_blocks = content.iter().any(|block| {
            block
                .get("type")
                .and_then(|value| value.as_str())
                .is_some_and(|value| value == "thinking")
        }),
        raw_upstream_response_body = raw_upstream_body,
        assistant_message_content = %assistant_message.content,
        anthropic_content_json = %serde_json::Value::Array(content.to_vec()),
        "targeted kiro non-stream response debug"
    );
}

fn log_high_credit_usage_anomaly(
    key_id: &str,
    key_name: &str,
    event_context: &KiroEventContext,
    request_validation_enabled: bool,
    cache_estimation_enabled: bool,
    simulation: &KiroSimulationRequestContext,
    usage: KiroUsageSummary,
) {
    let Some(credit_usage) = usage
        .credit_usage
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return;
    };
    if !should_capture_full_kiro_request_bodies(
        &simulation.effective_cache_policy,
        Some(credit_usage),
    ) {
        return;
    }
    let projected_total = simulation.projection.projected_input_token_count.max(1);
    let matched_tokens = simulation
        .prefix_cache_match
        .matched_tokens
        .min(projected_total);
    let authoritative_input_tokens =
        usage.input_uncached_tokens.max(0) + usage.input_cached_tokens.max(0);
    let prefix_cached_tokens =
        estimate_prefix_tree_cached_tokens(authoritative_input_tokens, simulation);
    let policy_cap_basis_points = prefix_tree_credit_ratio_cap_basis_points_with_policy(
        &simulation.effective_cache_policy,
        usage.credit_usage,
    );
    let policy_cap_tokens = policy_cap_basis_points.map(|basis_points| {
        ((u128::from(authoritative_input_tokens.max(0) as u64) * u128::from(basis_points))
            / 10_000_u128)
            .min(u128::from(authoritative_input_tokens.max(0) as u64)) as i32
    });
    let matched_ratio_basis_points =
        ((u128::from(matched_tokens) * 10_000) / u128::from(projected_total)) as u64;
    tracing::warn!(
        key_id,
        key_name,
        account_name = event_context.account_name.as_deref().unwrap_or("unknown"),
        request_method = %event_context.request_method,
        request_url = %event_context.request_url,
        endpoint = %event_context.endpoint,
        model = event_context.model.as_deref().unwrap_or("unknown"),
        request_validation_enabled,
        cache_estimation_enabled,
        cache_simulation_mode = match simulation.simulation_config.mode {
            KiroCacheSimulationMode::Formula => "formula",
            KiroCacheSimulationMode::PrefixTree => "prefix_tree",
        },
        conversation_id = event_context.conversation_id.as_deref().unwrap_or("unknown"),
        session_resolution = event_context.session_resolution.as_deref().unwrap_or("unknown"),
        session_source_name = event_context.session_source_name.as_deref().unwrap_or("unknown"),
        session_source_preview =
            event_context.session_source_value_preview.as_deref().unwrap_or(""),
        credit_usage,
        input_uncached_tokens = usage.input_uncached_tokens,
        input_cached_tokens = usage.input_cached_tokens,
        output_tokens = usage.output_tokens,
        billable_tokens = compute_kiro_billable_tokens(
            event_context.model.as_deref(),
            usage.input_uncached_tokens.max(0) as u64,
            usage.input_cached_tokens.max(0) as u64,
            usage.output_tokens.max(0) as u64,
            &simulation.runtime_config.kiro_billable_model_multipliers,
        ),
        stable_prefix_page_count = simulation.projection.stable_prefix_pages.len(),
        stable_prefix_token_count = simulation.projection.stable_prefix_token_count(),
        projected_input_token_count = simulation.projection.projected_input_token_count,
        matched_page_count = simulation.prefix_cache_match.matched_pages,
        matched_token_count = simulation.prefix_cache_match.matched_tokens,
        matched_ratio_basis_points,
        prefix_cached_tokens,
        policy_cap_basis_points = policy_cap_basis_points.unwrap_or(10_000),
        policy_cap_tokens = policy_cap_tokens.unwrap_or(-1),
        policy_cap_applied = policy_cap_tokens
            .is_some_and(|cap_tokens| usage.input_cached_tokens < prefix_cached_tokens
                && usage.input_cached_tokens <= cap_tokens),
        client_request_body_bytes = event_context
            .client_request_body_json
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default(),
        upstream_request_body_bytes = event_context
            .upstream_request_body_json
            .as_ref()
            .map(|value| value.len())
            .unwrap_or_default(),
        last_message_content_preview = event_context
            .last_message_content
            .as_deref()
            .map(|value| compact_preview(value, KIRO_LAST_MESSAGE_PART_PREVIEW_CHARS))
            .unwrap_or_default(),
        "observed unusually high kiro credit usage; full request bodies will be persisted for diagnostics"
    );
}

fn maybe_recover_conversation_id_from_anchor(
    state: &AppState,
    mut conversation_state: ConversationState,
    session_tracking: SessionTracking,
    projection: &PromptProjection,
    simulation_config: KiroCacheSimulationConfig,
    now: std::time::Instant,
) -> (ConversationState, SessionTracking) {
    let SessionIdSource::GeneratedFallback(reason) = session_tracking.source.clone() else {
        return (conversation_state, session_tracking);
    };
    let Some(recovered_conversation_id) = state
        .kiro_gateway
        .cache_simulator
        .recover_conversation_id(projection, simulation_config, now)
    else {
        return (conversation_state, session_tracking);
    };
    conversation_state.conversation_id = recovered_conversation_id;
    (conversation_state, SessionTracking {
        source: SessionIdSource::RecoveredAnchor(reason),
        source_name: session_tracking.source_name,
        source_value_preview: session_tracking.source_value_preview,
    })
}

fn log_non_header_session_resolution(
    session_tracking: &SessionTracking,
    conversation_id: &str,
    projection: &PromptProjection,
    ctx: &NormalizationLogContext<'_>,
) {
    match &session_tracking.source {
        SessionIdSource::RequestHeader => {},
        SessionIdSource::MetadataJson | SessionIdSource::MetadataLegacy => {
            let session_resolution = match &session_tracking.source {
                SessionIdSource::MetadataJson => "metadata_json",
                SessionIdSource::MetadataLegacy => "metadata_legacy",
                _ => unreachable!("non-header metadata logging only handles metadata sources"),
            };
            tracing::warn!(
                key_id = %ctx.key_record.id,
                key_name = %ctx.key_record.name,
                route = ctx.public_path,
                requested_model = ctx.requested_model,
                effective_model = ctx.effective_model,
                stream = ctx.stream,
                buffered_for_cc = ctx.buffered_for_cc,
                request_validation_enabled = ctx.request_validation_enabled,
                session_resolution,
                conversation_id,
                session_source_name = session_tracking.source_name.unwrap_or(""),
                session_source_preview = session_tracking
                    .source_value_preview
                    .as_deref()
                    .unwrap_or(""),
                "resolved kiro conversation id from non-header session source before upstream call"
            );
        },
        SessionIdSource::RecoveredAnchor(reason) => {
            tracing::warn!(
                key_id = %ctx.key_record.id,
                key_name = %ctx.key_record.name,
                route = ctx.public_path,
                requested_model = ctx.requested_model,
                effective_model = ctx.effective_model,
                stream = ctx.stream,
                buffered_for_cc = ctx.buffered_for_cc,
                request_validation_enabled = ctx.request_validation_enabled,
                fallback_reason = reason.as_str(),
                recovered_conversation_id = conversation_id,
                lookup_anchor_preview = preview_session_value(&projection.lookup_anchor_hash),
                session_source_name = session_tracking.source_name.unwrap_or(""),
                session_source_preview = session_tracking
                    .source_value_preview
                    .as_deref()
                    .unwrap_or(""),
                "recovered kiro conversation id from canonical history anchor because no supported request header produced a valid session id"
            );
        },
        SessionIdSource::GeneratedFallback(reason) => {
            tracing::warn!(
                key_id = %ctx.key_record.id,
                key_name = %ctx.key_record.name,
                route = ctx.public_path,
                requested_model = ctx.requested_model,
                effective_model = ctx.effective_model,
                stream = ctx.stream,
                buffered_for_cc = ctx.buffered_for_cc,
                request_validation_enabled = ctx.request_validation_enabled,
                fallback_reason = reason.as_str(),
                session_source_name = session_tracking.source_name.unwrap_or(""),
                generated_conversation_id = conversation_id,
                session_source_preview = session_tracking
                    .source_value_preview
                    .as_deref()
                    .unwrap_or(""),
                "generated fallback kiro conversation id before upstream call because no supported request header produced a valid session id"
            );
        },
    }
}

enum UsagePersistOutcome {
    Success {
        event_context: KiroEventContext,
        summary: KiroUsageSummary,
        usage_missing: bool,
    },
    Failure {
        event_context: KiroEventContext,
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

fn session_resolution_label(source: &SessionIdSource) -> &'static str {
    match source {
        SessionIdSource::RequestHeader => "request_header",
        SessionIdSource::MetadataJson => "metadata_json",
        SessionIdSource::MetadataLegacy => "metadata_legacy",
        SessionIdSource::RecoveredAnchor(_) => "recovered_anchor",
        SessionIdSource::GeneratedFallback(_) => "generated_fallback",
    }
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

fn request_value_contains_images(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(map) => map.iter().any(|(key, child)| {
            (key == "images" && child.as_array().is_some_and(|items| !items.is_empty()))
                || (key == "type" && child.as_str() == Some("image"))
                || request_value_contains_images(child)
        }),
        serde_json::Value::Array(items) => items.iter().any(request_value_contains_images),
        _ => false,
    }
}

fn request_body_contains_images(raw_body: Option<&str>) -> bool {
    let Some(raw_body) = raw_body else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(raw_body)
        .map(|value| request_value_contains_images(&value))
        .unwrap_or_else(|_| {
            raw_body.contains("\"images\"") || raw_body.contains("\"type\":\"image\"")
        })
}

fn history_images_require_stable_session(
    has_history_images: bool,
    session_tracking: &SessionTracking,
) -> bool {
    has_history_images && matches!(session_tracking.source, SessionIdSource::GeneratedFallback(_))
}

fn unsupported_history_image_replay_message(
    has_history_images: bool,
    session_tracking: &SessionTracking,
) -> Option<String> {
    history_images_require_stable_session(has_history_images, session_tracking).then(|| {
        "Historical image turns require a stable session id. Re-send the image in the current \
         message or provide a stable session id via request headers or metadata."
            .to_string()
    })
}

/// Maps a Kiro provider error into an appropriate HTTP error response.
/// Recognizes context-length, input-length, quota, and malformed-request
/// errors.
fn classify_provider_error(
    err_text: &str,
    request_contains_images: bool,
) -> (StatusCode, &'static str, String) {
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
    } else if err_text.contains("Improperly formed request") {
        let message = if request_contains_images {
            "Kiro rejected the image request as malformed. Current-turn images cannot include \
             `origin`, and historical image turns require a stable session id."
                .to_string()
        } else {
            "Kiro upstream rejected the request as malformed. Check tool schema, session history, \
             and multimodal content."
                .to_string()
        };
        (StatusCode::BAD_REQUEST, "invalid_request_error", message)
    } else {
        (StatusCode::BAD_GATEWAY, "api_error", format!("Kiro upstream request failed: {err_text}"))
    }
}

fn provider_error_response(err_text: &str, request_contains_images: bool) -> Response {
    let (status, error_type, message) = classify_provider_error(err_text, request_contains_images);
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
    let request_contains_images = request_body_contains_images(err.request_body.as_deref())
        || request_body_contains_images(
            ctx.diagnostic
                .event_context
                .client_request_body_json
                .as_deref(),
        );
    let (status, _, _) = classify_provider_error(&err_text, request_contains_images);
    let diagnostic_event_context =
        provider_failure_event_context(ctx.diagnostic.event_context, &err);
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
        &diagnostic_event_context,
        FailedKiroRequestEvent {
            _effective_policy: ctx.effective_cache_policy,
            status_code: status.as_u16() as i32,
            diagnostic_payload,
            usage: zero_usage_summary(),
            usage_missing: false,
        },
    )
    .await
    {
        tracing::warn!("failed to persist kiro failure usage event: {persist_err:#}");
    }
    provider_error_response(&err_text, request_contains_images)
}

fn provider_failure_event_context(
    event_context: &KiroEventContext,
    err: &ProviderCallError,
) -> KiroEventContext {
    let mut diagnostic_event_context = event_context.clone();
    if let Some(account_name) = &err.account_name {
        diagnostic_event_context.account_name = Some(account_name.clone());
    }
    if let Some(routing_wait_ms) = err.routing_wait_ms {
        diagnostic_event_context.routing_wait_ms = Some(clamp_u64_ms_to_i32(routing_wait_ms));
    }
    if let Some(upstream_headers_ms) = err.upstream_headers_ms {
        diagnostic_event_context.upstream_headers_ms =
            Some(clamp_u64_ms_to_i32(upstream_headers_ms));
        diagnostic_event_context.upstream_headers_at = Some(Instant::now());
    }
    if err.quota_failover_count > 0 {
        diagnostic_event_context.quota_failover_count = err.quota_failover_count;
    }
    if err.routing_diagnostics_json.is_some() {
        diagnostic_event_context.routing_diagnostics_json = err.routing_diagnostics_json.clone();
    }
    if let Some(request_body) = err.request_body.as_deref() {
        diagnostic_event_context.upstream_request_body_json = Some(request_body.to_string());
    }
    diagnostic_event_context
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

fn anthropic_total_input_tokens(usage: KiroUsageSummary) -> i32 {
    usage
        .input_uncached_tokens
        .max(0)
        .saturating_add(usage.input_cached_tokens.max(0))
}

fn anthropic_cache_creation_input_tokens_with_policy(
    policy: &KiroCachePolicy,
    non_cached_input_tokens_total: i32,
    cache_read_input_tokens: i32,
) -> i32 {
    if non_cached_input_tokens_total <= 0 {
        return 0;
    }
    if cache_read_input_tokens == 0 {
        return non_cached_input_tokens_total / 2;
    }
    let ratio = policy.anthropic_cache_creation_input_ratio;
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0;
    }
    (((non_cached_input_tokens_total as f64) * ratio).floor() as i32)
        .max(0)
        .min(non_cached_input_tokens_total)
}

fn anthropic_input_usage_breakdown(
    input_tokens_total: i32,
    cache_read_input_tokens: i32,
) -> (i32, i32, i32) {
    let input_tokens_total = input_tokens_total.max(0);
    let cache_read_input_tokens = cache_read_input_tokens.max(0).min(input_tokens_total);
    let non_cached_input_tokens_total = input_tokens_total.saturating_sub(cache_read_input_tokens);
    let cache_creation_input_tokens =
        if cache_read_input_tokens == 0 { non_cached_input_tokens_total / 2 } else { 0 };
    let input_tokens = non_cached_input_tokens_total.saturating_sub(cache_creation_input_tokens);
    (input_tokens, cache_creation_input_tokens, cache_read_input_tokens)
}

fn anthropic_input_usage_breakdown_with_policy(
    policy: &KiroCachePolicy,
    input_tokens_total: i32,
    cache_read_input_tokens: i32,
) -> (i32, i32, i32) {
    let input_tokens_total = input_tokens_total.max(0);
    let cache_read_input_tokens = cache_read_input_tokens.max(0).min(input_tokens_total);
    let non_cached_input_tokens_total = input_tokens_total.saturating_sub(cache_read_input_tokens);
    let cache_creation_input_tokens = anthropic_cache_creation_input_tokens_with_policy(
        policy,
        non_cached_input_tokens_total,
        cache_read_input_tokens,
    );
    let input_tokens = non_cached_input_tokens_total.saturating_sub(cache_creation_input_tokens);
    (input_tokens, cache_creation_input_tokens, cache_read_input_tokens)
}

pub(super) fn anthropic_usage_json(
    input_tokens_total: i32,
    output_tokens: i32,
    cache_read_input_tokens: i32,
) -> serde_json::Value {
    let (input_tokens, cache_creation_input_tokens, cache_read_input_tokens) =
        anthropic_input_usage_breakdown(input_tokens_total, cache_read_input_tokens);
    serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
    })
}

pub(super) fn anthropic_usage_json_with_policy(
    policy: &KiroCachePolicy,
    input_tokens_total: i32,
    output_tokens: i32,
    cache_read_input_tokens: i32,
) -> serde_json::Value {
    let (input_tokens, cache_creation_input_tokens, cache_read_input_tokens) =
        anthropic_input_usage_breakdown_with_policy(
            policy,
            input_tokens_total,
            cache_read_input_tokens,
        );
    serde_json::json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
    })
}

#[cfg(test)]
fn anthropic_usage_json_from_summary(usage: KiroUsageSummary) -> serde_json::Value {
    anthropic_usage_json(
        anthropic_total_input_tokens(usage),
        usage.output_tokens,
        usage.input_cached_tokens,
    )
}

fn anthropic_usage_json_from_summary_with_policy(
    usage: KiroUsageSummary,
    policy: &KiroCachePolicy,
) -> serde_json::Value {
    anthropic_usage_json_with_policy(
        policy,
        anthropic_total_input_tokens(usage),
        usage.output_tokens,
        usage.input_cached_tokens,
    )
}

#[derive(Debug, Clone)]
struct KiroCacheEstimateInput<'a> {
    model: &'a str,
    input_tokens_total: i32,
    output_tokens: i32,
    credit_usage: Option<f64>,
    kmodels: &'a std::collections::BTreeMap<String, f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KiroCacheEstimate {
    input_tokens_total: i32,
    input_uncached_tokens: i32,
    input_cached_tokens: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum KiroInputTokenSource {
    UpstreamContextUsage,
    LocalRequestEstimateFallback,
}

fn normalize_kiro_kmodel_name(model: &str) -> &str {
    match model {
        "claude-opus-4.6" => "claude-opus-4-6",
        _ => model,
    }
}

fn estimate_kiro_cache_usage(input: KiroCacheEstimateInput<'_>) -> KiroCacheEstimate {
    let safe_input = input.input_tokens_total.max(0);
    let output_tokens = input.output_tokens.max(0);
    let Some(observed_credit) = input
        .credit_usage
        .filter(|value| value.is_finite() && *value >= 0.0)
    else {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    };
    let Some(kmodel) = input
        .kmodels
        .get(normalize_kiro_kmodel_name(input.model))
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    };
    let safe_full_cost = kmodel * (safe_input as f64 + 5.0 * output_tokens as f64);
    if !safe_full_cost.is_finite() || safe_full_cost <= observed_credit || safe_input <= 0 {
        return KiroCacheEstimate {
            input_tokens_total: safe_input,
            input_uncached_tokens: safe_input,
            input_cached_tokens: 0,
        };
    }
    let cached = ((safe_full_cost - observed_credit) / (0.9 * kmodel)).floor();
    let cached = cached.max(0.0).min(safe_input as f64) as i32;
    KiroCacheEstimate {
        input_tokens_total: safe_input,
        input_uncached_tokens: safe_input - cached,
        input_cached_tokens: cached,
    }
}

pub(super) fn resolve_input_tokens(
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
) -> (i32, KiroInputTokenSource) {
    let request_input = request_input_tokens.max(0);
    let context_input = context_input_tokens.unwrap_or_default().max(0);
    if context_input > 0 {
        let resolved_context_input =
            if request_input <= KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS {
                context_input
                    .saturating_sub(KIRO_HIDDEN_PROMPT_BASELINE_TOKENS)
                    .max(request_input)
            } else {
                context_input
            };
        (resolved_context_input, KiroInputTokenSource::UpstreamContextUsage)
    } else {
        (request_input, KiroInputTokenSource::LocalRequestEstimateFallback)
    }
}

fn estimate_prefix_tree_cached_tokens(
    authoritative_input_tokens: i32,
    simulation: &KiroSimulationRequestContext,
) -> i32 {
    let authoritative_input_u64 = authoritative_input_tokens.max(0) as u64;
    let projected_total = simulation.projection.projected_input_token_count.max(1);
    let matched = simulation
        .prefix_cache_match
        .matched_tokens
        .min(projected_total);
    ((u128::from(authoritative_input_u64) * u128::from(matched)) / u128::from(projected_total))
        .min(u128::from(authoritative_input_u64)) as i32
}

fn clamp_prefix_tree_cached_tokens_with_credit_ratio_cap(
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    prefix_cached_tokens: i32,
    simulation: &KiroSimulationRequestContext,
) -> i32 {
    let Some(cap_basis_points) = prefix_tree_credit_ratio_cap_basis_points_with_policy(
        &simulation.effective_cache_policy,
        credit_usage,
    ) else {
        return prefix_cached_tokens;
    };
    let authoritative_input_u64 = authoritative_input_tokens.max(0) as u64;
    let ratio_cap = ((u128::from(authoritative_input_u64) * u128::from(cap_basis_points))
        / 10_000_u128)
        .min(u128::from(authoritative_input_u64)) as i32;
    prefix_cached_tokens.min(ratio_cap)
}

fn build_kiro_usage_summary(
    model: &str,
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
    output_tokens: i32,
    credit_usage: Option<f64>,
    cache_estimation_enabled: bool,
    simulation: &KiroSimulationRequestContext,
) -> KiroUsageSummary {
    let (resolved_input_tokens, _) =
        resolve_input_tokens(request_input_tokens, context_input_tokens);
    if !cache_estimation_enabled {
        return KiroUsageSummary {
            input_uncached_tokens: resolved_input_tokens,
            input_cached_tokens: 0,
            output_tokens,
            credit_usage,
            credit_usage_missing: credit_usage.is_none(),
        };
    }
    let authoritative_input_tokens = adjust_input_tokens_for_cache_creation_cost_with_policy(
        &simulation.effective_cache_policy,
        resolved_input_tokens,
        credit_usage,
        cache_estimation_enabled,
    );
    let estimate = match simulation.simulation_config.mode {
        KiroCacheSimulationMode::Formula => estimate_kiro_cache_usage(KiroCacheEstimateInput {
            model,
            input_tokens_total: authoritative_input_tokens,
            output_tokens,
            credit_usage,
            kmodels: &simulation.runtime_config.kiro_cache_kmodels,
        }),
        KiroCacheSimulationMode::PrefixTree => {
            // Prefix-tree mode computes a conservative cache-hit ratio from the
            // corrected prompt projection, then applies that ratio to the
            // authoritative upstream-reported input total. Local parsing only
            // provides the ratio basis and never overrides upstream totals.
            // As upstream credit rises, the prefix-derived cache is treated as
            // a candidate and clamped by an explicit product-policy ratio cap.
            // The cap starts at credit 0.3, ramps from 70% down to 20% before
            // credit 1.0, then keeps shrinking from 20% to zero by credit 2.5.
            let prefix_cached =
                estimate_prefix_tree_cached_tokens(authoritative_input_tokens, simulation);
            let cached = clamp_prefix_tree_cached_tokens_with_credit_ratio_cap(
                authoritative_input_tokens,
                credit_usage,
                prefix_cached,
                simulation,
            );
            KiroCacheEstimate {
                input_tokens_total: authoritative_input_tokens,
                input_uncached_tokens: authoritative_input_tokens - cached,
                input_cached_tokens: cached,
            }
        },
    };
    KiroUsageSummary {
        input_uncached_tokens: estimate.input_uncached_tokens,
        input_cached_tokens: estimate.input_cached_tokens,
        output_tokens,
        credit_usage,
        credit_usage_missing: credit_usage.is_none(),
    }
}

fn log_missing_upstream_input_tokens(ctx: MissingUpstreamInputLogContext<'_>) {
    let (_, source) = resolve_input_tokens(ctx.request_input_tokens, ctx.context_input_tokens);
    if source != KiroInputTokenSource::LocalRequestEstimateFallback {
        return;
    }

    tracing::error!(
        key_id = ctx.key_id,
        key_name = ctx.key_name,
        account_name = ctx.event_context.account_name.as_deref().unwrap_or("unknown"),
        request_method = %ctx.event_context.request_method,
        request_url = %ctx.event_context.request_url,
        endpoint = %ctx.event_context.endpoint,
        model = ctx.event_context.model.as_deref().unwrap_or("unknown"),
        request_validation_enabled = ctx.request_validation_enabled,
        cache_estimation_enabled = ctx.cache_estimation_enabled,
        cache_simulation_mode = match ctx.simulation.simulation_config.mode {
            KiroCacheSimulationMode::Formula => "formula",
            KiroCacheSimulationMode::PrefixTree => "prefix_tree",
        },
        conversation_id = ctx.event_context.conversation_id.as_deref().unwrap_or("unknown"),
        session_resolution = ctx.event_context.session_resolution.as_deref().unwrap_or("unknown"),
        session_source_name = ctx.event_context.session_source_name.as_deref().unwrap_or("unknown"),
        request_input_tokens = ctx.request_input_tokens,
        context_input_tokens = ctx.context_input_tokens.unwrap_or_default(),
        "kiro request completed without authoritative upstream input token usage; falling back to local request token estimate"
    );
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
        "conversation_id": ctx.event_context.conversation_id,
        "session_resolution": ctx.event_context.session_resolution,
        "session_source_name": ctx.event_context.session_source_name,
        "session_source_value_preview": ctx.event_context.session_source_value_preview,
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

fn build_assistant_message(content: String, tool_uses: Vec<ToolUseEntry>) -> AssistantMessage {
    let mut assistant = AssistantMessage::new(content);
    if !tool_uses.is_empty() {
        assistant = assistant.with_tool_uses(tool_uses);
    }
    assistant
}

fn canonicalize_structured_output_json(input: &str) -> String {
    let value = if input.is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(input).unwrap_or_else(|_| serde_json::json!({}))
    };
    serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_string())
}

fn record_successful_kiro_prefix_state(
    state: &AppState,
    simulation: &KiroSimulationRequestContext,
    cache_estimation_enabled: bool,
    assistant_message: &AssistantMessage,
) {
    let resume_anchor_hash = simulation
        .projection
        .build_resume_anchor_hash(assistant_message);
    let prefix_tree_write_enabled =
        should_run_prefix_cache_match(cache_estimation_enabled, simulation.simulation_config.mode);
    tracing::info!(
        cache_simulation_mode = match simulation.simulation_config.mode {
            KiroCacheSimulationMode::Formula => "formula",
            KiroCacheSimulationMode::PrefixTree => "prefix_tree",
        },
        conversation_id = %simulation.conversation_id,
        stable_prefix_page_count = simulation.projection.stable_prefix_pages.len(),
        stable_prefix_token_count = simulation.projection.stable_prefix_token_count(),
        projected_input_token_count = simulation.projection.projected_input_token_count,
        cache_estimation_enabled,
        prefix_tree_write_enabled,
        anchor_write_enabled = true,
        resume_anchor_preview = preview_session_value(&resume_anchor_hash),
        "recording successful kiro request state into shared session/cache stores"
    );
    state.kiro_gateway.cache_simulator.record_success(
        &simulation.projection,
        assistant_message,
        &simulation.conversation_id,
        prefix_tree_write_enabled,
        simulation.simulation_config,
        std::time::Instant::now(),
    );
}

/// Returns the list of available models for the `/v1/models` endpoint.
pub async fn get_models() -> impl IntoResponse {
    Json(supported_models_response())
}

/// Estimates token count for the given request payload.
pub async fn count_tokens(
    JsonExtractor(payload): JsonExtractor<CountTokensRequest>,
) -> impl IntoResponse {
    Json(count_tokens_response(payload))
}

/// Handler for `POST /v1/messages` — standard streaming mode.
pub async fn post_messages(State(state): State<AppState>, request: Request) -> Response {
    let (headers, mut payload, ingress_timings) =
        match read_timed_json_request::<MessagesRequest>(request).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    handle_messages(state, headers, &mut payload, false, ingress_timings).await
}

/// Handler for `POST /cc/v1/messages` — buffered mode for Claude Code.
/// Collects all upstream events before flushing, so input_tokens can be
/// rewritten with the actual value from context-usage feedback.
pub async fn post_messages_cc(State(state): State<AppState>, request: Request) -> Response {
    let (headers, mut payload, ingress_timings) =
        match read_timed_json_request::<MessagesRequest>(request).await {
            Ok(value) => value,
            Err(response) => return response,
        };
    handle_messages(state, headers, &mut payload, true, ingress_timings).await
}

// Shared implementation for both /v1/messages and /cc/v1/messages.
// Authenticates the key, converts the request, and dispatches to the
// appropriate stream/non-stream handler.
async fn handle_messages(
    state: AppState,
    headers: HeaderMap,
    payload: &mut MessagesRequest,
    buffered_for_cc: bool,
    ingress_timings: KiroRequestIngressTimings,
) -> Response {
    let (key_record, mut event_context) = match state.authenticate_kiro_key(&headers).await {
        Ok(value) => value,
        Err(err) => return err.into_response(),
    };
    event_context.request_body_bytes = Some(ingress_timings.request_body_bytes);
    event_context.request_body_read_ms = ingress_timings.request_body_read_ms;
    event_context.request_json_parse_ms = ingress_timings.request_json_parse_ms;
    event_context.pre_handler_ms = ingress_timings.pre_handler_ms;
    let runtime_config = state.llm_gateway_runtime_config.read().clone();
    let effective_cache_policy =
        match resolve_effective_kiro_cache_policy(&runtime_config, &key_record) {
            Ok(policy) => policy,
            Err(err) => {
                tracing::error!(
                    key_id = %key_record.id,
                    key_name = %key_record.name,
                    error = ?err,
                    "failed to resolve effective kiro cache policy at request start"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::new(
                        "api_error",
                        "Kiro cache policy configuration is invalid.".to_string(),
                    )),
                )
                    .into_response();
            },
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
    let pure_web_search = websearch::should_route_mcp_web_search(payload);
    if !pure_web_search {
        websearch::remove_web_search_tools(payload);
    }
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
    log_targeted_request_rewrite_debug(
        &key_record,
        &event_context,
        payload,
        &requested_model,
        buffered_for_cc,
        input_tokens,
    );
    let provider = KiroProvider::new(state.kiro_gateway.clone());
    if pure_web_search {
        event_context.endpoint = "/mcp".to_string();
        let _activity_guard = state.llm_gateway.start_request_activity(&key_record.id);
        return handle_websearch_request(
            state,
            key_record,
            event_context,
            effective_cache_policy,
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
    let resolved_session = resolve_request_session(&headers, payload.metadata.as_ref());
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        request_validation_enabled,
        resolved_session,
    ) {
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
    for (rewritten_tool_name, original_tool_name) in &conversion.tool_name_map {
        tracing::warn!(
            key_id = %key_record.id,
            key_name = %key_record.name,
            route = public_path,
            requested_model = %requested_model,
            effective_model = %payload.model,
            stream = payload.stream,
            buffered_for_cc,
            request_validation_enabled,
            original_tool_name = %original_tool_name,
            rewritten_tool_name = %rewritten_tool_name,
            rewrite_reason = classify_tool_name_rewrite_reason(original_tool_name),
            "rewrote kiro tool name before upstream call"
        );
    }
    let has_history_images = conversion.has_history_images;
    let tool_name_map = conversion.tool_name_map;
    let structured_output_tool_name = conversion.structured_output_tool_name;
    let (conversation_state, session_tracking, simulation) = prepare_simulation_request_context(
        &state,
        runtime_config,
        effective_cache_policy.clone(),
        conversion.conversation_state,
        conversion.session_tracking,
        key_record.kiro_cache_estimation_enabled,
    );
    event_context.conversation_id = Some(conversation_state.conversation_id.clone());
    event_context.session_resolution =
        Some(session_resolution_label(&session_tracking.source).to_string());
    event_context.session_source_name = session_tracking.source_name.map(str::to_string);
    event_context.session_source_value_preview = session_tracking.source_value_preview.clone();
    log_non_header_session_resolution(
        &session_tracking,
        &conversation_state.conversation_id,
        &simulation.projection,
        &normalization_log_ctx,
    );
    log_simulation_request_context(
        &key_record,
        &normalization_log_ctx,
        key_record.kiro_cache_estimation_enabled,
        &session_tracking,
        &simulation,
    );
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
    if let Some(message) =
        unsupported_history_image_replay_message(has_history_images, &session_tracking)
    {
        tracing::error!(
            key_id = %key_record.id,
            key_name = %key_record.name,
            route = public_path,
            requested_model = %requested_model,
            effective_model = %payload.model,
            session_resolution = session_resolution_label(&session_tracking.source),
            stream = payload.stream,
            buffered_for_cc,
            error = %message,
            "rejected kiro public request because historical image turns cannot be replayed \
             without a stable upstream session"
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
                "session_resolution": session_resolution_label(&session_tracking.source),
                "has_history_images": true,
            })),
        );
        if let Err(persist_err) = crate::kiro_gateway::record_failed_request_event(
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
            tracing::warn!(
                "failed to persist kiro history-image validation failure usage event: \
                 {persist_err:#}"
            );
        }
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::new("invalid_request_error", message)),
        )
            .into_response();
    }
    let _activity_guard = state.llm_gateway.start_request_activity(&key_record.id);

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
                        effective_cache_policy: &simulation.effective_cache_policy,
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
        apply_provider_metrics(&mut event_context, &response);
        event_context.upstream_request_body_json = Some(response.request_body.clone());
        event_context.account_name = Some(response.account_name);
        let stream_request_ctx = StreamRequestContext {
            state: state.clone(),
            event_context: event_context.clone(),
            request_validation_enabled,
            cache_estimation_enabled: key_record.kiro_cache_estimation_enabled,
            simulation: simulation.clone(),
        };
        if buffered_for_cc {
            return handle_stream_request_buffered(
                UsagePersistContext {
                    state,
                    key_record,
                    event_context,
                    effective_cache_policy: simulation.effective_cache_policy.clone(),
                },
                response.response,
                stream_request_ctx,
                BufferedStreamContext::new(
                    &payload.model,
                    input_tokens,
                    thinking_enabled,
                    tool_name_map,
                    structured_output_tool_name.clone(),
                ),
            )
            .await;
        }
        return handle_stream_request(
            UsagePersistContext {
                state,
                key_record,
                event_context,
                effective_cache_policy: simulation.effective_cache_policy.clone(),
            },
            response.response,
            stream_request_ctx,
            StreamContext::new_with_thinking(
                &payload.model,
                input_tokens,
                thinking_enabled,
                tool_name_map,
                structured_output_tool_name.clone(),
            ),
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
                    effective_cache_policy: &simulation.effective_cache_policy,
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
    apply_provider_metrics(&mut event_context, &response);
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
            thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
            request_validation_enabled,
            simulation,
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
    request_ctx: StreamRequestContext,
    ctx: StreamContext,
) -> Response {
    let (done_tx, done_rx) = oneshot::channel::<UsagePersistOutcome>();
    let log_ctx = KiroUpstreamLogContext::new(
        &usage_ctx.key_record,
        usage_ctx.event_context.account_name.as_deref(),
        &ctx.model,
        false,
    );
    let stream = create_sse_stream(
        request_ctx,
        response,
        ctx,
        log_ctx,
        done_tx,
        usage_ctx.state.shutdown_rx.clone(),
    );
    tokio::spawn(async move {
        if let Ok(outcome) = done_rx.await {
            let persist_result = match outcome {
                UsagePersistOutcome::Success {
                    event_context,
                    summary,
                    usage_missing,
                } => {
                    record_messages_usage(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &event_context,
                        &usage_ctx.effective_cache_policy,
                        summary,
                        usage_missing,
                    )
                    .await
                },
                UsagePersistOutcome::Failure {
                    event_context,
                    status_code,
                    summary,
                    usage_missing,
                    diagnostic_payload,
                } => {
                    crate::kiro_gateway::record_failed_request_event(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &event_context,
                        FailedKiroRequestEvent {
                            _effective_policy: &usage_ctx.effective_cache_policy,
                            status_code,
                            diagnostic_payload,
                            usage: summary,
                            usage_missing,
                        },
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
        .expect("hardcoded SSE response headers are valid")
}

// Buffers all Kiro events, then flushes them as SSE in one burst.
// Allows rewriting input_tokens in message_start with the actual value.
async fn handle_stream_request_buffered(
    usage_ctx: UsagePersistContext,
    response: reqwest::Response,
    request_ctx: StreamRequestContext,
    ctx: BufferedStreamContext,
) -> Response {
    let (done_tx, done_rx) = oneshot::channel::<UsagePersistOutcome>();
    let log_ctx = KiroUpstreamLogContext::new(
        &usage_ctx.key_record,
        usage_ctx.event_context.account_name.as_deref(),
        ctx.model(),
        true,
    );
    let stream = create_buffered_sse_stream(
        request_ctx,
        response,
        ctx,
        log_ctx,
        done_tx,
        usage_ctx.state.shutdown_rx.clone(),
    );
    tokio::spawn(async move {
        if let Ok(outcome) = done_rx.await {
            let persist_result = match outcome {
                UsagePersistOutcome::Success {
                    event_context,
                    summary,
                    usage_missing,
                } => {
                    record_messages_usage(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &event_context,
                        &usage_ctx.effective_cache_policy,
                        summary,
                        usage_missing,
                    )
                    .await
                },
                UsagePersistOutcome::Failure {
                    event_context,
                    status_code,
                    summary,
                    usage_missing,
                    diagnostic_payload,
                } => {
                    crate::kiro_gateway::record_failed_request_event(
                        &usage_ctx.state,
                        &usage_ctx.key_record,
                        &event_context,
                        FailedKiroRequestEvent {
                            _effective_policy: &usage_ctx.effective_cache_policy,
                            status_code,
                            diagnostic_payload,
                            usage: summary,
                            usage_missing,
                        },
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
        .expect("hardcoded SSE response headers are valid")
}

// Reads the full Kiro response body, decodes all events, assembles a
// single JSON response with content blocks, and persists usage synchronously.
async fn handle_non_stream_request(
    state: AppState,
    key_record: static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord,
    mut event_context: KiroEventContext,
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
            event_context.stream_finish_ms = Some(elapsed_ms_i32(event_context.started_at));
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
                FailedKiroRequestEvent {
                    _effective_policy: &request_ctx.simulation.effective_cache_policy,
                    status_code: StatusCode::BAD_GATEWAY.as_u16() as i32,
                    diagnostic_payload,
                    usage: zero_usage_summary(),
                    usage_missing: false,
                },
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
    let raw_upstream_body = String::from_utf8_lossy(&body).into_owned();
    let mut decoder = EventStreamDecoder::new();
    let _ = decoder.feed(&body);
    let mut text_content = String::new();
    let mut tool_uses = Vec::new();
    let mut stop_reason = "end_turn".to_string();
    let mut context_input_tokens = None;
    let mut credit_usage = 0.0;
    let mut credit_usage_observed = false;
    let mut tool_json_buffers = std::collections::HashMap::<String, String>::new();
    let mut tool_use_orders = std::collections::HashMap::<String, usize>::new();
    let mut next_tool_use_order = 0usize;
    let mut structured_tool_uses = Vec::<(usize, ToolUseEntry)>::new();
    let mut structured_output_text = None::<String>;
    for result in decoder.decode_iter() {
        match result {
            Ok(frame) => match Event::from_frame(frame) {
                Ok(Event::AssistantResponse(event)) => text_content.push_str(&event.content),
                Ok(Event::ToolUse(event)) => {
                    tool_use_orders
                        .entry(event.tool_use_id.clone())
                        .or_insert_with(|| {
                            let start_order = next_tool_use_order;
                            next_tool_use_order += 1;
                            start_order
                        });
                    let buffer = tool_json_buffers
                        .entry(event.tool_use_id.clone())
                        .or_default();
                    buffer.push_str(&event.input);
                    if event.stop {
                        if request_ctx.structured_output_tool_name.as_deref() == Some(&event.name) {
                            structured_output_text =
                                Some(canonicalize_structured_output_json(buffer));
                            continue;
                        }
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
                        let start_order = tool_use_orders
                            .remove(&event.tool_use_id)
                            .unwrap_or(next_tool_use_order);
                        if start_order == next_tool_use_order {
                            next_tool_use_order += 1;
                        }
                        structured_tool_uses.push((
                            start_order,
                            ToolUseEntry::new(event.tool_use_id.clone(), original_name.clone())
                                .with_input(input.clone()),
                        ));
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

    if let Some(json_text) = structured_output_text {
        text_content = json_text;
        tool_uses.clear();
        structured_tool_uses.clear();
    }
    if !tool_uses.is_empty() && stop_reason == "end_turn" {
        stop_reason = "tool_use".to_string();
    }
    structured_tool_uses.sort_by_key(|(start_order, _)| *start_order);
    let assistant_message = build_assistant_message(
        text_content.clone(),
        structured_tool_uses
            .into_iter()
            .map(|(_, tool_use)| tool_use)
            .collect(),
    );
    let mut content = build_inline_thinking_content_blocks(
        &text_content,
        &request_ctx.model,
        request_ctx.thinking_enabled,
    );
    content.extend(tool_uses);
    let emitted_non_thinking_block = content.iter().any(|block| {
        block
            .get("type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "text" || value == "tool_use")
    });
    let emitted_thinking_block = content.iter().any(|block| {
        block
            .get("type")
            .and_then(|value| value.as_str())
            .is_some_and(|value| value == "thinking")
    });
    if request_ctx.thinking_enabled && emitted_thinking_block && !emitted_non_thinking_block {
        stop_reason = "max_tokens".to_string();
        content.push(serde_json::json!({"type":"text","text":" "}));
    }
    log_targeted_non_stream_debug(
        &key_record,
        &request_ctx,
        &stop_reason,
        &raw_upstream_body,
        &assistant_message,
        &content,
    );
    let output_tokens = token::estimate_output_tokens(&content);
    log_missing_upstream_input_tokens(MissingUpstreamInputLogContext {
        key_id: &key_record.id,
        key_name: &key_record.name,
        event_context: &event_context,
        request_validation_enabled: request_ctx.request_validation_enabled,
        cache_estimation_enabled: key_record.kiro_cache_estimation_enabled,
        simulation: &request_ctx.simulation,
        request_input_tokens: request_ctx.input_tokens,
        context_input_tokens,
    });
    let usage = build_kiro_usage_summary(
        &request_ctx.model,
        request_ctx.input_tokens,
        context_input_tokens,
        output_tokens,
        credit_usage_observed.then_some(credit_usage.max(0.0)),
        key_record.kiro_cache_estimation_enabled,
        &request_ctx.simulation,
    );
    log_high_credit_usage_anomaly(
        &key_record.id,
        &key_record.name,
        &event_context,
        request_ctx.request_validation_enabled,
        key_record.kiro_cache_estimation_enabled,
        &request_ctx.simulation,
        usage,
    );
    tracing::info!(
        model = %request_ctx.model,
        stop_reason = %stop_reason,
        content_block_count = content.len(),
        usage_input_uncached_tokens = usage.input_uncached_tokens,
        usage_input_cached_tokens = usage.input_cached_tokens,
        usage_output_tokens = usage.output_tokens,
        "finished kiro non-stream request"
    );
    event_context.stream_finish_ms = Some(elapsed_ms_i32(event_context.started_at));
    record_successful_kiro_prefix_state(
        &state,
        &request_ctx.simulation,
        key_record.kiro_cache_estimation_enabled,
        &assistant_message,
    );
    if let Err(err) = record_messages_usage(
        &state,
        &key_record,
        &event_context,
        &request_ctx.simulation.effective_cache_policy,
        usage,
        false,
    )
    .await
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
            "usage": anthropic_usage_json_from_summary_with_policy(
                usage,
                &request_ctx.simulation.effective_cache_policy,
            )
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
    mut request_ctx: StreamRequestContext,
    response: reqwest::Response,
    mut ctx: StreamContext,
    log_ctx: KiroUpstreamLogContext,
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
            if request_ctx.event_context.first_sse_write_ms.is_none() {
                request_ctx.event_context.first_sse_write_ms =
                    Some(elapsed_ms_i32(request_ctx.event_context.started_at));
            }
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
                                    event_context: &request_ctx.event_context,
                                    request_validation_enabled: request_ctx
                                        .request_validation_enabled,
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

        let (input_tokens, output_tokens) = ctx.final_usage();
        let (credit_usage, credit_usage_missing) = ctx.final_credit_usage();
        let summary = build_kiro_usage_summary(
            ctx.model.as_str(),
            ctx.request_input_tokens(),
            ctx.context_input_tokens(),
            output_tokens,
            credit_usage,
            request_ctx.cache_estimation_enabled,
            &request_ctx.simulation,
        );
        let mut final_events = ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(
            summary,
            &request_ctx.simulation.effective_cache_policy,
        );
        for event in &mut final_events {
            if event.event == "message_delta" {
                if let Some(usage) = event.data.get_mut("usage") {
                    usage["input_tokens"] = anthropic_usage["input_tokens"].clone();
                    usage["cache_creation_input_tokens"] =
                        anthropic_usage["cache_creation_input_tokens"].clone();
                    usage["cache_read_input_tokens"] =
                        anthropic_usage["cache_read_input_tokens"].clone();
                }
            }
        }
        tracing::info!(
            model = %ctx.model,
            final_event_count = final_events.len(),
            input_tokens,
            output_tokens,
            credit_usage = credit_usage.unwrap_or_default(),
            credit_usage_missing,
            "finished kiro streaming response"
        );
        request_ctx.event_context.stream_finish_ms =
            Some(elapsed_ms_i32(request_ctx.event_context.started_at));
        if request_ctx.event_context.first_sse_write_ms.is_none() {
            request_ctx.event_context.first_sse_write_ms =
                request_ctx.event_context.stream_finish_ms;
        }
        if let Some(sender) = done_tx.take() {
            let assistant_message = ctx.final_assistant_message();
            log_missing_upstream_input_tokens(MissingUpstreamInputLogContext {
                key_id: &log_ctx.key_id,
                key_name: &log_ctx.key_name,
                event_context: &request_ctx.event_context,
                request_validation_enabled: request_ctx.request_validation_enabled,
                cache_estimation_enabled: request_ctx.cache_estimation_enabled,
                simulation: &request_ctx.simulation,
                request_input_tokens: ctx.request_input_tokens(),
                context_input_tokens: ctx.context_input_tokens(),
            });
            log_high_credit_usage_anomaly(
                &log_ctx.key_id,
                &log_ctx.key_name,
                &request_ctx.event_context,
                request_ctx.request_validation_enabled,
                request_ctx.cache_estimation_enabled,
                &request_ctx.simulation,
                summary,
            );
            if failure_diagnostic_payload.is_none() {
                record_successful_kiro_prefix_state(
                    &request_ctx.state,
                    &request_ctx.simulation,
                    request_ctx.cache_estimation_enabled,
                    &assistant_message,
                );
            }
            let _ = match failure_diagnostic_payload {
                Some(diagnostic_payload) => sender.send(UsagePersistOutcome::Failure {
                    event_context: request_ctx.event_context.clone(),
                    status_code: KIRO_STREAM_FAILURE_STATUS_CODE,
                    summary,
                    usage_missing: true,
                    diagnostic_payload,
                }),
                None => sender.send(UsagePersistOutcome::Success {
                    event_context: request_ctx.event_context.clone(),
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
    mut request_ctx: StreamRequestContext,
    response: reqwest::Response,
    mut ctx: BufferedStreamContext,
    log_ctx: KiroUpstreamLogContext,
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
                                    event_context: &request_ctx.event_context,
                                    request_validation_enabled: request_ctx
                                        .request_validation_enabled,
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

        let mut all_events = ctx.finish_and_get_all_events();
        let (input_tokens, output_tokens) = ctx.final_usage();
        let (credit_usage, credit_usage_missing) = ctx.final_credit_usage();
        let assistant_message = ctx.final_assistant_message();
        log_missing_upstream_input_tokens(MissingUpstreamInputLogContext {
            key_id: &log_ctx.key_id,
            key_name: &log_ctx.key_name,
            event_context: &request_ctx.event_context,
            request_validation_enabled: request_ctx.request_validation_enabled,
            cache_estimation_enabled: request_ctx.cache_estimation_enabled,
            simulation: &request_ctx.simulation,
            request_input_tokens: ctx.estimated_input_tokens(),
            context_input_tokens: ctx.context_input_tokens(),
        });
        let summary = build_kiro_usage_summary(
            ctx.model(),
            ctx.estimated_input_tokens(),
            ctx.context_input_tokens(),
            output_tokens,
            credit_usage,
            request_ctx.cache_estimation_enabled,
            &request_ctx.simulation,
        );
        log_high_credit_usage_anomaly(
            &log_ctx.key_id,
            &log_ctx.key_name,
            &request_ctx.event_context,
            request_ctx.request_validation_enabled,
            request_ctx.cache_estimation_enabled,
            &request_ctx.simulation,
            summary,
        );
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(
            summary,
            &request_ctx.simulation.effective_cache_policy,
        );
        for event in &mut all_events {
            if let Some(usage) = event
                .data
                .get_mut("message")
                .and_then(|message| message.get_mut("usage"))
            {
                *usage = anthropic_usage.clone();
            }
            if event.event == "message_delta" {
                if let Some(usage) = event.data.get_mut("usage") {
                    usage["input_tokens"] = anthropic_usage["input_tokens"].clone();
                    usage["cache_creation_input_tokens"] =
                        anthropic_usage["cache_creation_input_tokens"].clone();
                    usage["cache_read_input_tokens"] =
                        anthropic_usage["cache_read_input_tokens"].clone();
                }
            }
        }
        tracing::info!(
            model = %ctx.model(),
            buffered_event_count = all_events.len(),
            input_tokens,
            output_tokens,
            credit_usage = credit_usage.unwrap_or_default(),
            credit_usage_missing,
            "finished kiro buffered streaming response"
        );
        request_ctx.event_context.stream_finish_ms =
            Some(elapsed_ms_i32(request_ctx.event_context.started_at));
        if request_ctx.event_context.first_sse_write_ms.is_none() {
            request_ctx.event_context.first_sse_write_ms =
                request_ctx.event_context.stream_finish_ms;
        }
        if let Some(sender) = done_tx.take() {
            if failure_diagnostic_payload.is_none() {
                record_successful_kiro_prefix_state(
                    &request_ctx.state,
                    &request_ctx.simulation,
                    request_ctx.cache_estimation_enabled,
                    &assistant_message,
                );
            }
            let _ = match failure_diagnostic_payload {
                Some(diagnostic_payload) => sender.send(UsagePersistOutcome::Failure {
                    event_context: request_ctx.event_context.clone(),
                    status_code: KIRO_STREAM_FAILURE_STATUS_CODE,
                    summary,
                    usage_missing: true,
                    diagnostic_payload,
                }),
                None => sender.send(UsagePersistOutcome::Success {
                    event_context: request_ctx.event_context.clone(),
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
/// Opus 4.6 gets adaptive/xhigh; all others get enabled with 20K budget.
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
        let output_config = payload.output_config.get_or_insert(OutputConfig {
            effort: None,
            format: None,
        });
        if output_config.effort.is_none() {
            output_config.effort = Some("xhigh".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use axum::http::{HeaderMap, HeaderValue, Request};
    use serde::Deserialize;
    use serde_json::json;
    use static_flow_shared::llm_gateway_store::{
        default_kiro_cache_policy, merge_kiro_cache_policy, KiroCachePolicyOverride,
        KiroCreditRatioBand, KiroSmallInputHighCreditBoostOverride,
    };

    use super::*;
    use crate::kiro_gateway::{
        anthropic::types::Metadata,
        wire::{
            CurrentMessage, HistoryAssistantMessage, HistoryUserMessage, Message, UserInputMessage,
        },
    };

    #[derive(Debug, Deserialize, PartialEq)]
    struct TimedJsonProbe {
        message: String,
    }

    #[tokio::test]
    async fn timed_json_request_records_ingress_read_and_parse_metrics() {
        let mut request = Request::builder()
            .uri("/api/kiro-gateway/v1/messages")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"message":"hello"}"#))
            .expect("build request");
        request
            .extensions_mut()
            .insert(crate::request_context::RequestReceivedAt(Instant::now()));

        let (_, payload, timings) = read_timed_json_request::<TimedJsonProbe>(request)
            .await
            .expect("parse timed json");

        assert_eq!(payload, TimedJsonProbe {
            message: "hello".to_string()
        });
        assert!(timings.request_body_bytes > 0);
        assert!(timings.request_body_read_ms.is_some());
        assert!(timings.request_json_parse_ms.is_some());
        assert!(timings.pre_handler_ms.is_some());
    }

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
            account_group_id: None,
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
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    fn history_user(content: &str) -> Message {
        Message::User(HistoryUserMessage::new(content, "ignored-model"))
    }

    fn history_assistant(content: &str) -> Message {
        Message::Assistant(HistoryAssistantMessage::new(content))
    }

    fn sample_kmodels() -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("claude-opus-4-6".to_string(), 8.061927916785985e-06),
            ("claude-sonnet-4-6".to_string(), 5.055065250835128e-06),
            ("claude-haiku-4-5-20251001".to_string(), 2.3681034438052206e-06),
        ])
    }

    fn sample_simulation(
        mode: KiroCacheSimulationMode,
        matched_tokens: u64,
    ) -> KiroSimulationRequestContext {
        let runtime_config = LlmGatewayRuntimeConfig {
            kiro_cache_kmodels: sample_kmodels(),
            kiro_prefix_cache_mode: match mode {
                KiroCacheSimulationMode::Formula => "formula".to_string(),
                KiroCacheSimulationMode::PrefixTree => "prefix_tree".to_string(),
            },
            ..LlmGatewayRuntimeConfig::default()
        };
        let simulation_config = KiroCacheSimulationConfig::from(&runtime_config);
        let projection = PromptProjection::from_conversation_state(
            &ConversationState::new("conv-1")
                .with_history(vec![history_user(
                    "existing context that is intentionally long enough to span several tokens",
                )])
                .with_current_message(crate::kiro_gateway::wire::CurrentMessage::new(
                    crate::kiro_gateway::wire::UserInputMessage::new(
                        "hello current turn",
                        "claude-sonnet-4.6",
                    ),
                )),
        );
        KiroSimulationRequestContext {
            runtime_config,
            effective_cache_policy:
                static_flow_shared::llm_gateway_store::default_kiro_cache_policy(),
            simulation_config,
            projection,
            prefix_cache_match: PrefixCacheMatch {
                matched_pages: usize::from(matched_tokens > 0),
                matched_tokens,
            },
            conversation_id: "conv-1".to_string(),
        }
    }

    #[test]
    fn resolve_request_session_prefers_explicit_claude_code_header_over_metadata() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-claude-code-session-id",
            HeaderValue::from_static("7dbefac5-5e9f-4542-b5e2-ea6a8ccdb72d"),
        );
        let metadata = Metadata {
            user_id: Some(
                r#"{"device_id":"dev","account_uuid":"acct","session_id":"c4dd850d-929f-48d1-9282-f0cfefeec16e"}"#
                    .to_string(),
            ),
        };

        let resolved = resolve_request_session(&headers, Some(&metadata));

        assert_eq!(resolved.conversation_id, "7dbefac5-5e9f-4542-b5e2-ea6a8ccdb72d");
        assert_eq!(resolved.session_tracking.source, SessionIdSource::RequestHeader);
        assert_eq!(resolved.session_tracking.source_name, Some("x-claude-code-session-id"));
    }

    #[test]
    fn resolve_request_session_accepts_session_id_header_without_metadata() {
        let mut headers = HeaderMap::new();
        headers
            .insert("session_id", HeaderValue::from_static("0c4df3fe-90fb-4fa8-b7cb-78c51409f3d5"));

        let resolved = resolve_request_session(&headers, None);

        assert_eq!(resolved.conversation_id, "0c4df3fe-90fb-4fa8-b7cb-78c51409f3d5");
        assert_eq!(resolved.session_tracking.source, SessionIdSource::RequestHeader);
        assert_eq!(resolved.session_tracking.source_name, Some("session_id"));
    }

    #[test]
    fn resolve_request_session_falls_back_to_legacy_metadata_when_header_is_invalid() {
        let mut headers = HeaderMap::new();
        headers.insert("x-claude-code-session-id", HeaderValue::from_static("invalid-session"));
        let metadata = Metadata {
            user_id: Some(
                r#"{"device_id":"dev","account_uuid":"acct","session_id":"c4dd850d-929f-48d1-9282-f0cfefeec16e"}"#
                    .to_string(),
            ),
        };

        let resolved = resolve_request_session(&headers, Some(&metadata));

        assert_eq!(resolved.conversation_id, "c4dd850d-929f-48d1-9282-f0cfefeec16e");
        assert_eq!(resolved.session_tracking.source, SessionIdSource::MetadataJson);
        assert_eq!(resolved.session_tracking.source_name, None);
    }

    #[test]
    fn resolve_request_session_accepts_openclaw_header_variant() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-openclaw-session-id",
            HeaderValue::from_static("2cc1dd58-fcb2-45c3-9170-eb2d48b0a1d8"),
        );

        let resolved = resolve_request_session(&headers, None);

        assert_eq!(resolved.conversation_id, "2cc1dd58-fcb2-45c3-9170-eb2d48b0a1d8");
        assert_eq!(resolved.session_tracking.source, SessionIdSource::RequestHeader);
        assert_eq!(resolved.session_tracking.source_name, Some("x-openclaw-session-id"));
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
    fn thinking_suffix_sets_adaptive_xhigh_for_opus_46_models() {
        let mut payload = base_request("claude-opus-4-6-thinking");
        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should be injected");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(thinking.budget_tokens, 20_000);
        assert_eq!(
            payload
                .output_config
                .as_ref()
                .and_then(|config| config.effort.as_deref()),
            Some("xhigh")
        );
    }

    #[test]
    fn thinking_suffix_preserves_existing_opus_46_effort() {
        let mut payload = base_request("claude-opus-4-6-thinking");
        payload.output_config = Some(OutputConfig {
            effort: Some("max".to_string()),
            format: None,
        });

        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should be injected");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(thinking.budget_tokens, 20_000);
        assert_eq!(
            payload
                .output_config
                .as_ref()
                .and_then(|config| config.effort.as_deref()),
            Some("max")
        );
    }

    #[test]
    fn thinking_suffix_preserves_existing_output_format_when_backfilling_effort() {
        let mut payload = base_request("claude-opus-4-6-thinking");
        payload.output_config = Some(OutputConfig {
            effort: None,
            format: Some(types::OutputFormat {
                format_type: "json_schema".to_string(),
                schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "result": { "type": "integer" }
                    },
                    "required": ["result"],
                    "additionalProperties": false
                })),
            }),
        });

        override_thinking_from_model_name(&mut payload);

        let output_config = payload.output_config.expect("output_config should remain");
        assert_eq!(output_config.effort.as_deref(), Some("xhigh"));
        assert_eq!(
            output_config
                .json_schema()
                .expect("json schema should remain preserved")["required"],
            json!(["result"])
        );
    }

    #[test]
    fn classify_provider_error_maps_minimum_remaining_threshold_to_payment_required() {
        let (status, error_type, message) = classify_provider_error(
            "all configured kiro accounts are below the configured minimum remaining credits \
             threshold",
            false,
        );

        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(error_type, "rate_limit_error");
        assert!(message.contains("minimum remaining credits threshold"));
    }

    #[test]
    fn classify_provider_error_maps_improperly_formed_image_request_to_bad_request() {
        let (status, error_type, message) = classify_provider_error(
            "kiro upstream rejected request: 400 Bad Request {\"message\":\"Improperly formed \
             request.\",\"reason\":null}",
            true,
        );

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(error_type, "invalid_request_error");
        assert!(message.contains("image request"));
    }

    #[test]
    fn unsupported_history_image_replay_requires_stable_session() {
        let generated_fallback = SessionTracking {
            source: SessionIdSource::GeneratedFallback(SessionFallbackReason::MissingMetadata),
            source_name: None,
            source_value_preview: None,
        };
        let header_session = SessionTracking {
            source: SessionIdSource::RequestHeader,
            source_name: Some("x-claude-code-session-id"),
            source_value_preview: Some("conv-1".to_string()),
        };

        let message = unsupported_history_image_replay_message(true, &generated_fallback)
            .expect("fallback session should reject historical image replay");
        assert!(message.contains("Historical image turns require a stable session id"));
        assert!(unsupported_history_image_replay_message(true, &header_session).is_none());
        assert!(unsupported_history_image_replay_message(false, &generated_fallback).is_none());
    }

    #[test]
    fn non_thinking_model_does_not_override_existing_configuration() {
        let mut payload = base_request("claude-sonnet-4-6");
        payload.thinking = Some(Thinking {
            thinking_type: "adaptive".to_string(),
            budget_tokens: 8192,
        });
        payload.output_config = Some(OutputConfig {
            effort: Some("medium".to_string()),
            format: None,
        });

        override_thinking_from_model_name(&mut payload);

        let thinking = payload.thinking.expect("thinking should remain");
        assert_eq!(thinking.thinking_type, "adaptive");
        assert_eq!(thinking.budget_tokens, 8192);
        assert_eq!(
            payload
                .output_config
                .as_ref()
                .and_then(|config| config.effort.as_deref()),
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
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            upstream_headers_at: None,
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
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("request_header".to_string()),
            session_source_name: Some("x-claude-code-session-id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
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
    fn provider_failure_event_context_uses_provider_error_observability() {
        let event_context = crate::kiro_gateway::KiroEventContext {
            account_name: None,
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            upstream_headers_at: None,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "[]".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some(
                r#"{"model":"claude-sonnet-4-6","messages":[{"role":"user","content":"hello"}]}"#
                    .to_string(),
            ),
            upstream_request_body_json: Some(r#"{"stale":"value"}"#.to_string()),
            conversation_id: Some("conv-1".to_string()),
            session_resolution: Some("request_header".to_string()),
            session_source_name: Some("x-claude-code-session-id".to_string()),
            session_source_value_preview: Some("conv-1".to_string()),
            started_at: std::time::Instant::now(),
        };
        let error = ProviderCallError::new(
            anyhow::anyhow!("kiro upstream rejected request"),
            Some(r#"{"conversationState":{"conversationId":"from-provider"}}"#.to_string()),
        )
        .with_attempt_observability("acct-b", 12, Some(34))
        .with_routing_observability(Some(r#"{"route_total_ms":12}"#.to_string()), 3);

        let diagnostic = provider_failure_event_context(&event_context, &error);

        assert_eq!(diagnostic.account_name.as_deref(), Some("acct-b"));
        assert_eq!(diagnostic.routing_wait_ms, Some(12));
        assert_eq!(diagnostic.upstream_headers_ms, Some(34));
        assert_eq!(
            diagnostic.routing_diagnostics_json.as_deref(),
            Some(r#"{"route_total_ms":12}"#)
        );
        assert_eq!(diagnostic.quota_failover_count, 3);
        assert_eq!(
            diagnostic.upstream_request_body_json.as_deref(),
            Some(r#"{"conversationState":{"conversationId":"from-provider"}}"#)
        );
        assert_eq!(
            event_context.upstream_request_body_json.as_deref(),
            Some(r#"{"stale":"value"}"#)
        );
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

    #[test]
    fn estimate_cache_uses_provided_input_total() {
        let kmodels = sample_kmodels();
        let estimate = estimate_kiro_cache_usage(KiroCacheEstimateInput {
            model: "claude-opus-4-6",
            input_tokens_total: 9_000,
            output_tokens: 400,
            credit_usage: Some(0.02),
            kmodels: &kmodels,
        });

        assert_eq!(estimate.input_tokens_total, 9_000);
        assert!(estimate.input_cached_tokens <= 9_000);
        assert_eq!(
            estimate.input_uncached_tokens + estimate.input_cached_tokens,
            estimate.input_tokens_total
        );
    }

    #[test]
    fn estimate_cache_returns_zero_when_credit_exceeds_safe_full_cost() {
        let kmodels = sample_kmodels();
        let estimate = estimate_kiro_cache_usage(KiroCacheEstimateInput {
            model: "claude-sonnet-4-6",
            input_tokens_total: 5_000,
            output_tokens: 200,
            credit_usage: Some(10.0),
            kmodels: &kmodels,
        });

        assert_eq!(estimate.input_tokens_total, 5_000);
        assert_eq!(estimate.input_cached_tokens, 0);
        assert_eq!(estimate.input_uncached_tokens, 5_000);
    }

    #[test]
    fn resolve_input_tokens_discounts_kiro_hidden_prompt_for_small_requests() {
        let request_input_tokens = 18;
        let upstream_context_tokens = KIRO_HIDDEN_PROMPT_BASELINE_TOKENS + request_input_tokens;
        let (input_tokens, source) =
            resolve_input_tokens(request_input_tokens, Some(upstream_context_tokens));

        assert_eq!(input_tokens, 18);
        assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
    }

    #[test]
    fn resolve_input_tokens_keeps_corrected_context_when_it_exceeds_local_request() {
        let (input_tokens, source) =
            resolve_input_tokens(1_000, Some(KIRO_HIDDEN_PROMPT_BASELINE_TOKENS + 1_900));

        assert_eq!(input_tokens, 1_900);
        assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
    }

    #[test]
    fn resolve_input_tokens_keeps_upstream_context_for_large_requests() {
        let (input_tokens, source) = resolve_input_tokens(60_000, Some(90_000));

        assert_eq!(input_tokens, 90_000);
        assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
    }

    #[test]
    fn build_usage_summary_disables_cache_estimation_per_key() {
        let simulation = sample_simulation(KiroCacheSimulationMode::Formula, 0);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(90_000),
            400,
            Some(0.02),
            false,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 0);
        assert_eq!(summary.input_uncached_tokens, 90_000);
        assert_eq!(summary.output_tokens, 400);
        assert_eq!(summary.credit_usage, Some(0.02));
        assert!(!summary.credit_usage_missing);
    }

    #[test]
    fn anthropic_usage_marks_half_input_as_cache_creation_when_cache_read_is_zero() {
        let usage = anthropic_usage_json(125, 16, 0);

        assert_eq!(usage["input_tokens"], serde_json::json!(63));
        assert_eq!(usage["output_tokens"], serde_json::json!(16));
        assert_eq!(usage["cache_creation_input_tokens"], serde_json::json!(62));
        assert_eq!(usage["cache_read_input_tokens"], serde_json::json!(0));
    }

    #[test]
    fn anthropic_usage_keeps_cache_creation_zero_when_cache_read_is_non_zero() {
        let usage = anthropic_usage_json(125, 16, 20);

        assert_eq!(usage["input_tokens"], serde_json::json!(105));
        assert_eq!(usage["output_tokens"], serde_json::json!(16));
        assert_eq!(usage["cache_creation_input_tokens"], serde_json::json!(0));
        assert_eq!(usage["cache_read_input_tokens"], serde_json::json!(20));
    }

    #[test]
    fn anthropic_usage_from_summary_reports_uncached_input_separately_from_cache_read() {
        let usage = anthropic_usage_json_from_summary(KiroUsageSummary {
            input_uncached_tokens: 80,
            input_cached_tokens: 20,
            output_tokens: 16,
            credit_usage: None,
            credit_usage_missing: false,
        });

        assert_eq!(usage["input_tokens"], serde_json::json!(80));
        assert_eq!(usage["output_tokens"], serde_json::json!(16));
        assert_eq!(usage["cache_creation_input_tokens"], serde_json::json!(0));
        assert_eq!(usage["cache_read_input_tokens"], serde_json::json!(20));
    }

    #[test]
    fn anthropic_usage_from_summary_uses_policy_ratio_to_emit_cache_creation_with_cache_read() {
        let mut policy = default_kiro_cache_policy();
        policy.anthropic_cache_creation_input_ratio = 0.25;

        let usage = anthropic_usage_json_from_summary_with_policy(
            KiroUsageSummary {
                input_uncached_tokens: 80,
                input_cached_tokens: 20,
                output_tokens: 16,
                credit_usage: None,
                credit_usage_missing: false,
            },
            &policy,
        );

        assert_eq!(usage["input_tokens"], serde_json::json!(60));
        assert_eq!(usage["output_tokens"], serde_json::json!(16));
        assert_eq!(usage["cache_creation_input_tokens"], serde_json::json!(20));
        assert_eq!(usage["cache_read_input_tokens"], serde_json::json!(20));
    }

    #[test]
    fn build_usage_summary_formula_mode_uses_upstream_input_total() {
        let simulation = sample_simulation(KiroCacheSimulationMode::Formula, 0);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(90_000),
            400,
            Some(0.02),
            true,
            &simulation,
        );

        assert_eq!(summary.input_uncached_tokens + summary.input_cached_tokens, 90_000);
        assert!(summary.input_cached_tokens <= 90_000);
    }

    #[test]
    fn effective_input_total_keeps_authoritative_total_when_credit_is_not_above_one() {
        let policy = default_kiro_cache_policy();
        assert_eq!(
            adjust_input_tokens_for_cache_creation_cost_with_policy(
                &policy,
                50_000,
                Some(1.0),
                true,
            ),
            50_000
        );
    }

    #[test]
    fn effective_input_total_moves_halfway_toward_hundred_k_at_credit_one_point_four() {
        let policy = default_kiro_cache_policy();
        assert_eq!(
            adjust_input_tokens_for_cache_creation_cost_with_policy(
                &policy,
                50_000,
                Some(1.4),
                true,
            ),
            75_000
        );
    }

    #[test]
    fn effective_input_total_caps_at_hundred_k_when_credit_reaches_one_point_eight() {
        let policy = default_kiro_cache_policy();
        assert_eq!(
            adjust_input_tokens_for_cache_creation_cost_with_policy(
                &policy,
                50_000,
                Some(1.8),
                true,
            ),
            100_000
        );
    }

    #[test]
    fn effective_input_total_keeps_large_authoritative_total_unchanged() {
        let policy = default_kiro_cache_policy();
        assert_eq!(
            adjust_input_tokens_for_cache_creation_cost_with_policy(
                &policy,
                120_000,
                Some(1.8),
                true,
            ),
            120_000
        );
    }

    #[test]
    fn build_usage_summary_uses_prefix_tree_match_when_enabled() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, 4);
        let expected_cached = ((100_000_u128 * 4_u128)
            / u128::from(simulation.projection.projected_input_token_count.max(1)))
            as i32;
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(0.02),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, expected_cached);
        assert_eq!(summary.input_uncached_tokens, 100_000 - expected_cached);
    }

    #[test]
    fn build_usage_summary_prefix_tree_applies_cache_creation_floor_before_ratio_split() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(50_000),
            400,
            Some(1.75),
            true,
            &simulation,
        );

        let authoritative_input_tokens = adjust_input_tokens_for_cache_creation_cost_with_policy(
            &simulation.effective_cache_policy,
            50_000,
            Some(1.75),
            true,
        );
        let cap_basis_points = prefix_tree_credit_ratio_cap_basis_points_with_policy(
            &simulation.effective_cache_policy,
            Some(1.75),
        )
        .expect("cap basis points");
        let expected_cached = ((u128::from(authoritative_input_tokens as u64)
            * u128::from(cap_basis_points))
            / 10_000_u128) as i32;

        assert_eq!(summary.input_cached_tokens, expected_cached);
        assert_eq!(summary.input_uncached_tokens, authoritative_input_tokens - expected_cached);
    }

    #[test]
    fn build_usage_summary_prefix_tree_caps_cache_at_twenty_percent_when_credit_reaches_one() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(1.0),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 20_000);
        assert_eq!(summary.input_uncached_tokens, 80_000);
    }

    #[test]
    fn build_usage_summary_uses_policy_override_for_boost_target() {
        let mut simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        simulation.effective_cache_policy = merge_kiro_cache_policy(
            &default_kiro_cache_policy(),
            Some(&KiroCachePolicyOverride {
                small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                    target_input_tokens: Some(80_000),
                    credit_start: Some(1.0),
                    credit_end: Some(1.8),
                }),
                ..KiroCachePolicyOverride::default()
            }),
        )
        .expect("boost target policy override should merge");

        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(50_000),
            400,
            Some(1.8),
            true,
            &simulation,
        );

        assert_eq!(summary.input_uncached_tokens + summary.input_cached_tokens, 80_000);
    }

    #[test]
    fn build_usage_summary_uses_policy_override_for_prefix_tree_bands() {
        let mut simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        simulation.effective_cache_policy = merge_kiro_cache_policy(
            &default_kiro_cache_policy(),
            Some(&KiroCachePolicyOverride {
                prefix_tree_credit_ratio_bands: Some(vec![KiroCreditRatioBand {
                    credit_start: 0.4,
                    credit_end: 1.4,
                    cache_ratio_start: 0.5,
                    cache_ratio_end: 0.1,
                }]),
                ..KiroCachePolicyOverride::default()
            }),
        )
        .expect("prefix tree band policy override should merge");

        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(0.9),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 30_000);
        assert_eq!(summary.input_uncached_tokens, 70_000);
    }

    #[test]
    fn build_usage_summary_prefix_tree_scales_policy_cap_inside_second_band() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(1.75),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 10_000);
        assert_eq!(summary.input_uncached_tokens, 90_000);
    }

    #[test]
    fn build_usage_summary_prefix_tree_scales_policy_cap_inside_first_band() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(0.65),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 45_000);
        assert_eq!(summary.input_uncached_tokens, 55_000);
    }

    #[test]
    fn build_usage_summary_prefix_tree_caps_cache_at_seventy_percent_when_credit_reaches_point_three(
    ) {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(0.3),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 70_000);
        assert_eq!(summary.input_uncached_tokens, 30_000);
    }

    #[test]
    fn build_usage_summary_prefix_tree_reports_zero_cache_when_credit_reaches_two_point_five() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(2.5),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 0);
        assert_eq!(summary.input_uncached_tokens, 100_000);
    }

    #[test]
    fn build_usage_summary_prefix_tree_keeps_prefix_ratio_when_credit_is_below_point_three() {
        let simulation = sample_simulation(KiroCacheSimulationMode::PrefixTree, u64::MAX);
        let summary = build_kiro_usage_summary(
            "claude-opus-4-6",
            KIRO_HIDDEN_PROMPT_DISCOUNT_MAX_REQUEST_TOKENS + 1,
            Some(100_000),
            400,
            Some(0.29),
            true,
            &simulation,
        );

        assert_eq!(summary.input_cached_tokens, 100_000);
        assert_eq!(summary.input_uncached_tokens, 0);
    }

    #[tokio::test]
    async fn formula_mode_still_recovers_conversation_id_from_anchor() {
        let temp_root = std::env::temp_dir()
            .join(format!("staticflow-kiro-anchor-test-{}", uuid::Uuid::new_v4()));
        let content_db = temp_root.join("lancedb");
        let comments_db = temp_root.join("lancedb-comments");
        let music_db = temp_root.join("lancedb-music");
        tokio::fs::create_dir_all(&content_db)
            .await
            .expect("content db dir should be created");
        tokio::fs::create_dir_all(&comments_db)
            .await
            .expect("comments db dir should be created");
        tokio::fs::create_dir_all(&music_db)
            .await
            .expect("music db dir should be created");

        let state = AppState::new(
            &content_db.to_string_lossy(),
            &comments_db.to_string_lossy(),
            &music_db.to_string_lossy(),
            "<html></html>".to_string(),
        )
        .await
        .expect("app state should initialize");

        {
            let mut runtime_config = state.llm_gateway_runtime_config.write();
            runtime_config.kiro_prefix_cache_mode = "formula".to_string();
        }

        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let runtime_config = state.llm_gateway_runtime_config.read().clone();
        let simulation_config = KiroCacheSimulationConfig::from(&runtime_config);
        let now = std::time::Instant::now();
        state.kiro_gateway.cache_simulator.record_success(
            &projection,
            &assistant,
            "real-conv",
            true,
            simulation_config,
            now,
        );

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let follow_up_projection = PromptProjection::from_conversation_state(&follow_up_state);
        let (conversation_state, session_tracking) = maybe_recover_conversation_id_from_anchor(
            &state,
            follow_up_state,
            SessionTracking {
                source: SessionIdSource::GeneratedFallback(SessionFallbackReason::MissingMetadata),
                source_name: None,
                source_value_preview: None,
            },
            &follow_up_projection,
            simulation_config,
            now + std::time::Duration::from_secs(1),
        );

        assert_eq!(conversation_state.conversation_id, "real-conv");
        assert_eq!(
            session_tracking.source,
            SessionIdSource::RecoveredAnchor(SessionFallbackReason::MissingMetadata)
        );

        let _ = tokio::fs::remove_dir_all(&temp_root).await;
    }

    #[tokio::test]
    async fn disabled_cache_estimation_skips_prefix_match_but_keeps_anchor_recovery() {
        let temp_root = std::env::temp_dir()
            .join(format!("staticflow-kiro-disabled-cache-test-{}", uuid::Uuid::new_v4()));
        let content_db = temp_root.join("lancedb");
        let comments_db = temp_root.join("lancedb-comments");
        let music_db = temp_root.join("lancedb-music");
        tokio::fs::create_dir_all(&content_db)
            .await
            .expect("content db dir should be created");
        tokio::fs::create_dir_all(&comments_db)
            .await
            .expect("comments db dir should be created");
        tokio::fs::create_dir_all(&music_db)
            .await
            .expect("music db dir should be created");

        let state = AppState::new(
            &content_db.to_string_lossy(),
            &comments_db.to_string_lossy(),
            &music_db.to_string_lossy(),
            "<html></html>".to_string(),
        )
        .await
        .expect("app state should initialize");

        {
            let mut runtime_config = state.llm_gateway_runtime_config.write();
            runtime_config.kiro_prefix_cache_mode = "prefix_tree".to_string();
        }

        let initial_state = ConversationState::new("fallback-conv")
            .with_history(vec![history_user("existing history"), history_assistant("done")])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "continue analysis",
                "ignored-model",
            )));
        let projection = PromptProjection::from_conversation_state(&initial_state);
        let assistant = AssistantMessage::new("assistant reply");
        let runtime_config = state.llm_gateway_runtime_config.read().clone();
        let simulation_config = KiroCacheSimulationConfig::from(&runtime_config);
        let now = std::time::Instant::now();
        state.kiro_gateway.cache_simulator.record_success(
            &projection,
            &assistant,
            "real-conv",
            false,
            simulation_config,
            now,
        );

        let follow_up_state = ConversationState::new("new-fallback")
            .with_history(vec![
                history_user("existing history"),
                history_assistant("done"),
                Message::User(HistoryUserMessage::new("continue analysis", "ignored-model")),
                Message::Assistant(HistoryAssistantMessage {
                    assistant_response_message: assistant.clone(),
                }),
            ])
            .with_current_message(CurrentMessage::new(UserInputMessage::new(
                "next step",
                "ignored-model",
            )));
        let (conversation_state, session_tracking, simulation) = prepare_simulation_request_context(
            &state,
            state.llm_gateway_runtime_config.read().clone(),
            default_kiro_cache_policy(),
            follow_up_state,
            SessionTracking {
                source: SessionIdSource::GeneratedFallback(SessionFallbackReason::MissingMetadata),
                source_name: None,
                source_value_preview: None,
            },
            false,
        );

        assert_eq!(conversation_state.conversation_id, "real-conv");
        assert_eq!(
            session_tracking.source,
            SessionIdSource::RecoveredAnchor(SessionFallbackReason::MissingMetadata)
        );
        assert_eq!(simulation.prefix_cache_match, PrefixCacheMatch::default());

        let _ = tokio::fs::remove_dir_all(&temp_root).await;
    }
}
