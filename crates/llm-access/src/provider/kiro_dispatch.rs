//! Kiro proxy dispatch, websearch, generate/MCP calls, and failover.

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{bail, Context};
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, HeaderMap, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use llm_access_codex::request::external_origin;
use llm_access_core::store::{AuthenticatedKey, ProviderKiroRoute, ProviderRouteStore};
use llm_access_kiro::{
    anthropic::{
        converter::{
            convert_normalized_request_with_resolved_session, normalize_request, ConversionResult,
            ResponseModelIdentity, SessionIdSource,
        },
        protected_content::validate_protected_content,
        stream::anthropic_usage_json,
        supported_models_response,
        types::MessagesRequest,
        websearch::{self, McpResponse},
    },
    cache_sim::AnchorTokenCounts,
    parser::decoder::EventStreamDecoder,
    scheduler::KiroRequestScheduler,
    token,
    wire::KiroRequest,
};

use super::{
    anthropic_upstream_dispatch::{
        maybe_dispatch_anthropic_upstream_pool, AnthropicUpstreamDispatchOutcome,
    },
    cctest::{self, build_direct_replay_body, bytes_to_string, CctestProbeMatch},
    client::{cctest_proxy_client, provider_client},
    errors::{
        anthropic_json_error, anthropic_json_error_body, is_monthly_request_limit,
        kiro_chunk_contains_content_length_exceeded, kiro_proactive_compact_message,
        kiro_proactive_compact_response, kiro_prompt_too_long_message,
        kiro_prompt_too_long_response_for_body, kiro_rate_limit_cooldown,
        proxy_cooldown_key_for_route, randomized_same_account_retry_delay,
        retry_after_header_duration, transient_invalid_model_cooldown, SameAccountRetryReason,
    },
    kiro_error::{
        kiro_bedrock_anthropic_error, kiro_bedrock_anthropic_error_body,
        kiro_conversion_error_response, kiro_json_error, kiro_upstream_error_response,
        KiroRouteFailure, KiroRouteFailureKind, AWS_BEDROCK_ALL_ACCOUNTS_COOLING_DOWN_MESSAGE,
        AWS_BEDROCK_ROUTE_SELECTION_FAILED_MESSAGE,
    },
    kiro_media::{resolve_kiro_remote_media_sources, strip_kiro_remote_media_sources},
    kiro_model::{
        apply_kiro_model_mapping, build_kiro_cache_context, kiro_affinity_session_id,
        override_kiro_thinking_from_model_name, remember_kiro_session_affinity,
        resolve_kiro_request_session,
    },
    kiro_protocol::{
        add_kiro_mcp_headers, add_kiro_upstream_headers, normalized_kiro_messages_path,
    },
    kiro_summary::extract_last_message_from_kiro_messages,
    kiro_usage::{
        build_kiro_usage_summary, record_kiro_cctest_usage, record_kiro_preflight_failure,
        record_kiro_selection_failure, record_kiro_usage, record_kiro_websearch_usage,
    },
    limiter::{kiro_key_limit_response, try_acquire_key_permit},
    route_selection::{hydrate_kiro_route_for_dispatch, select_kiro_route_with_account_permit},
    stream_guards::{non_stream_kiro_response, stream_kiro_upstream_response},
    usage_meta::{
        capture_client_request_body_json, capture_error_body, capture_error_bytes,
        capture_error_message, capture_response_body, capture_upstream_request_body_json,
    },
    util::{clamp_duration_ms, now_millis},
    KiroCctestUsageRecord, KiroPeekedStream, KiroPreflightFailureRecord,
    KiroResponseAffinityUpdate, KiroResponseContext, KiroSelectionFailureRecord,
    KiroStreamPeekError, KiroUsageInputs, KiroUsageRecord, KiroUsageSummary, KiroWebsearchDispatch,
    KiroWebsearchUsageRecord, ProviderDispatchDeps, ProviderUsageMetadata, WebsearchResponseInput,
    KIRO_EMPTY_STREAM_MAX_RETRIES, MAX_PROVIDER_PROXY_BODY_BYTES,
};
use crate::kiro_refresh;

const INCONSISTENT_ROUTE_CONFIGURATION_MESSAGE: &str = "Route configuration is inconsistent.";
const KIRO_SAME_ACCOUNT_MAX_ATTEMPTS: usize = 3;

fn kiro_model_group_preferred_account_names_for_model(
    routes: &[ProviderKiroRoute],
    requested_model: &str,
) -> Option<HashSet<String>> {
    let requested_model = requested_model.trim();
    if requested_model.is_empty() {
        return None;
    }
    routes
        .first()
        .and_then(|route| {
            route
                .model_group_preferred_account_names
                .get(requested_model)
        })
        .map(|account_names| account_names.iter().cloned().collect())
}

fn log_kiro_model_group_preference_routing(
    key: &AuthenticatedKey,
    endpoint: &str,
    requested_model: &str,
    effective_model: &str,
    model_preferred_account_names: Option<&HashSet<String>>,
    preferred_account_name: Option<&str>,
) {
    let Some(model_preferred_account_names) = model_preferred_account_names else {
        tracing::debug!(
            key_id = %key.key_id,
            key_name = %key.key_name,
            endpoint = %endpoint,
            requested_model = %requested_model,
            effective_model = %effective_model,
            "kiro model group routing preference not configured for request model"
        );
        return;
    };
    let mut preferred_account_names = model_preferred_account_names
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    preferred_account_names.sort_unstable();
    tracing::info!(
        key_id = %key.key_id,
        key_name = %key.key_name,
        endpoint = %endpoint,
        requested_model = %requested_model,
        effective_model = %effective_model,
        preferred_account_count = preferred_account_names.len(),
        preferred_account_names = ?preferred_account_names,
        sticky_affinity_account = preferred_account_name.unwrap_or(""),
        sticky_affinity_ignored = preferred_account_name.is_some(),
        "kiro model group routing preference is active"
    );
}

fn route_private_prompt_safety(
    route: &ProviderKiroRoute,
    conversion: &ConversionResult,
) -> (bool, Option<ResponseModelIdentity>) {
    let enabled = route.cctest_text_handling_enabled;
    let response_identity = if enabled { conversion.response_identity.clone() } else { None };
    (enabled, response_identity)
}

pub async fn dispatch_kiro_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> Response {
    if request.uri().path() == "/v1/models" {
        if request.method() == Method::GET {
            return axum::Json(supported_models_response()).into_response();
        }
        return kiro_json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "unsupported method",
        );
    }
    let request =
        match maybe_dispatch_anthropic_upstream_pool(key.clone(), request, deps.clone()).await {
            AnthropicUpstreamDispatchOutcome::Handled(response) => return response,
            AnthropicUpstreamDispatchOutcome::Fallback(request) => request,
        };
    let ProviderDispatchDeps {
        route_store,
        control_store,
        geoip,
        kiro_cache_simulator,
        request_limiter,
        kiro_request_scheduler,
        kiro_session_affinity,
        kiro_latency_ranker,
        protected_thinking_signature_secret,
        ..
    } = deps;
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
        &geoip,
    )
    .await;
    let routes = match route_store.resolve_kiro_route_candidates(&key).await {
        Ok(routes) if !routes.is_empty() => routes,
        Ok(_) => {
            return kiro_json_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "api_error",
                "route is not configured",
            )
        },
        Err(_) => {
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "route resolution failed",
            )
        },
    };
    let Some(public_path) = normalized_kiro_messages_path(request.uri().path()) else {
        return kiro_json_error(
            StatusCode::NOT_FOUND,
            "invalid_request_error",
            "unsupported endpoint",
        );
    };
    usage_meta.request_url = external_origin(request.headers())
        .map(|origin| format!("{origin}/api/kiro-gateway{public_path}"))
        .unwrap_or_else(|| format!("/api/kiro-gateway{public_path}"));
    if request.method() != Method::POST {
        return kiro_json_error(
            StatusCode::METHOD_NOT_ALLOWED,
            "invalid_request_error",
            "unsupported method",
        );
    }
    if let Some(response) = inconsistent_cctest_text_handling_response(&routes) {
        return response;
    }
    let request_headers = request.headers().clone();
    let body_read_started = Instant::now();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "request body is too large",
            )
        },
    };
    usage_meta =
        usage_meta.with_request_body(&body, clamp_duration_ms(body_read_started.elapsed()));
    if routes[0].cctest_text_handling_enabled {
        let cctest_started = Instant::now();
        let inspection = cctest::inspect_cctest_text_probe(&body);
        if inspection.should_log_debug() {
            capture_client_request_body_json(&mut usage_meta, &body);
            usage_meta.full_request_json = Some(body.clone());
            usage_meta.routing_diagnostics_json = Some(inspection.diagnostics_json());
            tracing::info!(
                key_id = %key.key_id,
                key_name = %key.key_name,
                endpoint = %public_path,
                request_url = %usage_meta.request_url,
                cctest_body_bytes = inspection.body_bytes,
                cctest_candidate = inspection.looks_like_cctest_candidate,
                cctest_has_billing_header = inspection.has_billing_header,
                cctest_has_cli_entrypoint = inspection.has_cli_entrypoint,
                cctest_has_cli_version = inspection.has_cli_version,
                cctest_has_messages_field = inspection.has_messages_field,
                cctest_json_parsed = inspection.json_parsed,
                cctest_has_multimodal_content = inspection.has_multimodal_content,
                cctest_has_web_search_tool = inspection.has_web_search_tool,
                cctest_request_id = inspection.request_id.as_deref().unwrap_or(""),
                cctest_probe_kind = inspection.probe_kind.unwrap_or(""),
                cctest_requires_signature = inspection.requires_signature,
                cctest_rejection_reason = inspection.rejection_reason.unwrap_or(""),
                cctest_matched = inspection.matched_probe.is_some(),
                "kiro cctest probe inspection"
            );
        }
        if let Some(probe) = inspection.matched_probe {
            usage_meta.mark_pre_handler_done(clamp_duration_ms(cctest_started.elapsed()));
            return handle_cctest_text_probe(CctestTextProbeDispatch {
                key,
                route: routes[0].clone(),
                endpoint: public_path,
                request_headers,
                body,
                probe,
                control_store,
                usage_meta,
            })
            .await;
        }
    }
    let parse_started = Instant::now();
    let mut payload = match serde_json::from_slice::<MessagesRequest>(&body) {
        Ok(payload) => payload,
        Err(err) => {
            return kiro_json_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                &format!("failed to parse request JSON: {err}"),
            )
        },
    };
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    usage_meta.last_message_content = extract_last_message_from_kiro_messages(&payload);
    let requested_model = payload.model.clone();
    if let Err(err) = apply_kiro_model_mapping(&routes[0].model_name_map_json, &mut payload) {
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &format!("Kiro model mapping configuration is invalid: {err}"),
        );
    }
    let effective_model = payload.model.clone();
    let model_group_preferred_account_names =
        kiro_model_group_preferred_account_names_for_model(&routes, &requested_model);
    let route_mcp_web_search = websearch::should_route_mcp_web_search(&payload);
    if !route_mcp_web_search {
        websearch::remove_web_search_tools(&mut payload);
    }
    let resolved_session =
        resolve_kiro_request_session(&request_headers, payload.metadata.as_ref());
    let affinity_session_id = kiro_affinity_session_id(&resolved_session).map(str::to_string);
    if routes[0].remote_media_resolution_enabled {
        if let Err(err) = resolve_kiro_remote_media_sources(&mut payload).await {
            let message = err.to_string();
            let response =
                kiro_json_error(StatusCode::BAD_REQUEST, "invalid_request_error", &message);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        }
    } else {
        let removed_sources = strip_kiro_remote_media_sources(&mut payload);
        if !removed_sources.is_empty() {
            tracing::warn!(
                key_id = %key.key_id,
                key_name = %key.key_name,
                endpoint = %public_path,
                request_url = %usage_meta.request_url,
                model = %effective_model,
                removed_remote_media_sources = removed_sources.len(),
                removed_remote_media_details = ?removed_sources,
                "kiro remote media sources were stripped because key remote media resolution is disabled"
            );
        }
    }
    let request_input_tokens = token::count_all_tokens(
        &payload.model,
        payload.system.as_deref(),
        &payload.messages,
        payload.tools.as_deref(),
    ) as i32;
    override_kiro_thinking_from_model_name(&mut payload);
    let protected_thinking_signature_secret = if routes[0].protected_content_validation_enabled {
        let Some(secret) = protected_thinking_signature_secret.clone() else {
            return kiro_bedrock_anthropic_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "InternalServerException",
                "protected content validation is enabled but KIRO_THINKING_SIGNATURE_SECRET is \
                 not configured",
            );
        };
        if let Err(err) =
            validate_protected_content(&payload, &key.key_id, &effective_model, secret.as_ref())
        {
            let message = err.to_string();
            let response = kiro_bedrock_anthropic_error(
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "ValidationException",
                &message,
            );
            let response_body = kiro_bedrock_anthropic_error_body(
                "invalid_request_error",
                "ValidationException",
                &message,
            );
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(&mut usage_meta, &response_body);
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        }
        Some(secret)
    } else {
        None
    };
    if route_mcp_web_search {
        // Proactive auto-compaction gate (websearch path): this branch performs
        // no anchor-based recovery, so it gates on the local request estimate
        // only. The main path below uses the contextUsage-accurate gate. Trigger
        // comes from PG runtime config (route.compact_trigger_tokens; 0 = off).
        let trigger = routes[0].compact_trigger_tokens;
        if trigger > 0
            && (request_input_tokens as u64) >= trigger
            && !request_is_compaction_summary(&payload)
        {
            let trigger_i32 = trigger.min(i32::MAX as u64) as i32;
            let message = kiro_proactive_compact_message(request_input_tokens, trigger_i32);
            tracing::info!(
                key_id = %key.key_id,
                key_name = %key.key_name,
                endpoint = %public_path,
                model = %effective_model,
                request_input_tokens,
                trigger,
                "proactively returning prompt-too-long to trigger client compaction (websearch)"
            );
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::PAYLOAD_TOO_LARGE,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return kiro_proactive_compact_response(request_input_tokens, trigger_i32);
        }
        if routes[0].full_request_logging_enabled {
            capture_client_request_body_json(&mut usage_meta, &body);
        }
        return dispatch_kiro_websearch(KiroWebsearchDispatch {
            key,
            payload,
            routes,
            control_store,
            route_store,
            request_limiter,
            kiro_request_scheduler,
            kiro_session_affinity,
            kiro_latency_ranker,
            affinity_session_id,
            requested_model,
            model_group_preferred_account_names,
            request_input_tokens,
            usage_meta,
        })
        .await;
    }
    let normalized = match normalize_request(&payload) {
        Ok(normalized) => normalized,
        Err(err) => {
            let message = err.to_string();
            let response = kiro_conversion_error_response(err);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        },
    };
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        routes[0].request_validation_enabled,
        resolved_session,
        routes[0].cctest_text_handling_enabled,
    ) {
        Ok(conversion) => conversion,
        Err(err) => {
            let message = err.to_string();
            let response = kiro_conversion_error_response(err);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &anthropic_json_error_body("invalid_request_error", &message),
            );
            capture_client_request_body_json(&mut usage_meta, &body);
            record_kiro_preflight_failure(KiroPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &routes[0],
                endpoint: public_path,
                model: &effective_model,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
                cache_simulator: kiro_cache_simulator.as_ref(),
            })
            .await;
            return response;
        },
    };
    let thinking_enabled = payload.thinking.as_ref().is_some_and(|thinking| {
        thinking.exposes_anthropic_thinking(payload.output_config.as_ref())
    });
    let hidden_thinking_enabled = payload.thinking.as_ref().is_some_and(|thinking| {
        thinking.is_enabled()
            && !thinking.exposes_anthropic_thinking(payload.output_config.as_ref())
    });
    let base_conversation_state = conversion.conversation_state.clone();
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    let preferred_account_name = affinity_session_id
        .as_deref()
        .and_then(|session_id| kiro_session_affinity.lookup(&key.key_id, session_id));
    log_kiro_model_group_preference_routing(
        &key,
        public_path,
        &requested_model,
        &effective_model,
        model_group_preferred_account_names.as_ref(),
        preferred_account_name.as_deref(),
    );
    // New session (has a session id but no existing affinity): balance it onto
    // the least-bound account. Skipped for single-account keys (nothing to
    // spread). Computed once — affinity only mutates on success after the loop,
    // so it is stable across failover retries.
    let session_counts =
        (routes.len() > 1 && affinity_session_id.is_some() && preferred_account_name.is_none())
            .then(|| kiro_session_affinity.account_session_counts());
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
            preferred_account_name.as_deref(),
            model_group_preferred_account_names.as_ref(),
            session_counts.as_ref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => {
                record_kiro_route_selection_failure_response(
                    control_store.as_ref(),
                    &key,
                    public_path,
                    Some(&effective_model),
                    &mut usage_meta,
                    response.status(),
                )
                .await;
                return response;
            },
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_kiro_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => {
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if has_remaining_kiro_candidate(&routes, &failed_accounts, "") {
                    continue;
                }
                return response;
            },
        };
        let mut conversation_state = base_conversation_state.clone();
        let mut cache_ctx =
            match build_kiro_cache_context(&route, &conversation_state, &kiro_cache_simulator) {
                Ok(context) => context,
                Err(err) => {
                    return kiro_json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "api_error",
                        &format!("Kiro cache configuration is invalid: {err}"),
                    )
                },
            };
        if matches!(conversion.session_tracking.source, SessionIdSource::GeneratedFallback(_)) {
            if let Some(recovered) = kiro_cache_simulator
                .recover_conversation_id_from_runtime_projection(
                    &cache_ctx.projection,
                    cache_ctx.simulation_config,
                    Instant::now(),
                )
            {
                conversation_state.conversation_id = recovered.clone();
                cache_ctx.conversation_id = recovered;
            }
        }
        // Proactive auto-compaction gate (main path): recover the previous
        // turn's cached token counts (real + local) for this conversation prefix
        // and estimate the current turn's real consumption as
        // `real_prev + max(0, local_now - local_prev)`, gated against
        // max(local_now, estimated_real). The recovered real value is accurate
        // where the local estimate drifts (large context + bridge scaffolding +
        // the 15k/ratio rule), while the delta keeps the newest turn in play (so
        // a large follow-up paste still fires the gate). After a compaction the
        // prefix changes so recovery misses and the gate falls back to the (now
        // small) local count — no deadlock. Compaction-summary requests are
        // always exempt. Trigger comes from PG runtime config
        // (route.compact_trigger_tokens; 0 = off).
        let compact_trigger = route.compact_trigger_tokens;
        if compact_trigger > 0 {
            let recovered = kiro_cache_simulator.recover_token_counts_from_runtime_projection(
                &cache_ctx.projection,
                cache_ctx.simulation_config,
                Instant::now(),
            );
            let effective_input_tokens =
                estimate_effective_input_tokens(request_input_tokens, recovered);
            if (effective_input_tokens as u64) >= compact_trigger
                && !request_is_compaction_summary(&payload)
            {
                let trigger_i32 = compact_trigger.min(i32::MAX as u64) as i32;
                let message = kiro_proactive_compact_message(effective_input_tokens, trigger_i32);
                tracing::info!(
                    key_id = %key.key_id,
                    key_name = %key.key_name,
                    endpoint = %public_path,
                    model = %effective_model,
                    request_input_tokens,
                    recovered_real_tokens = recovered.map(|c| c.real_input_tokens).unwrap_or(0),
                    effective_input_tokens,
                    trigger = compact_trigger,
                    "proactively returning prompt-too-long to trigger client compaction"
                );
                capture_error_message(&mut usage_meta, &message);
                capture_error_body(
                    &mut usage_meta,
                    &anthropic_json_error_body("invalid_request_error", &message),
                );
                capture_client_request_body_json(&mut usage_meta, &body);
                record_kiro_preflight_failure(KiroPreflightFailureRecord {
                    control_store: control_store.as_ref(),
                    key: &key,
                    route: &route,
                    endpoint: public_path,
                    model: &effective_model,
                    status: StatusCode::PAYLOAD_TOO_LARGE,
                    meta: &mut usage_meta,
                    cache_simulator: kiro_cache_simulator.as_ref(),
                })
                .await;
                return kiro_proactive_compact_response(effective_input_tokens, trigger_i32);
            }
        }
        let request_body = match serde_json::to_vec(&KiroRequest {
            conversation_state,
            profile_arn: route.profile_arn.clone(),
        }) {
            Ok(body) => body,
            Err(_) => {
                return kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "failed to encode kiro request",
                )
            },
        };
        if route.zero_cache_debug_enabled || route.full_request_logging_enabled {
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
        }
        let upstream_url = format!(
            "{}/generateAssistantResponse",
            kiro_refresh::runtime_upstream_base_url(&route.api_region)
        );
        let response = match call_kiro_generate_for_route_with_usage(
            &route,
            route_store.as_ref(),
            upstream_url.clone(),
            &request_body,
            &mut usage_meta,
        )
        .await
        {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(failure) => {
                if should_failover_after_kiro_route_failure(
                    &failure,
                    &route,
                    &routes,
                    &mut failed_accounts,
                    route_store.as_ref(),
                    &kiro_request_scheduler,
                )
                .await
                {
                    usage_meta.mark_failover();
                    continue;
                }
                let prompt_too_long_response = kiro_prompt_too_long_response_for_body(
                    failure.status,
                    &failure.body,
                    &effective_model,
                    request_input_tokens,
                );
                let status = if prompt_too_long_response.is_some() {
                    StatusCode::PAYLOAD_TOO_LARGE
                } else {
                    failure.status
                };
                capture_client_request_body_json(&mut usage_meta, &body);
                capture_upstream_request_body_json(&mut usage_meta, &request_body);
                capture_error_bytes(&mut usage_meta, &failure.body);
                usage_meta.mark_stream_finish();
                let error_response =
                    prompt_too_long_response.unwrap_or_else(|| failure.into_response());
                let usage = build_kiro_usage_summary(
                    &effective_model,
                    KiroUsageInputs {
                        request_input_tokens,
                        context_input_tokens: None,
                        context_usage_min_request_tokens: route.context_usage_min_request_tokens,
                        output_tokens: 0,
                        credit_usage: None,
                        credit_usage_missing: true,
                        cache_estimation_enabled: false,
                    },
                    &cache_ctx,
                );
                if let Err(err) = record_kiro_usage(KiroUsageRecord {
                    control_store: control_store.as_ref(),
                    key: &key,
                    route: &route,
                    endpoint: public_path,
                    model: &effective_model,
                    status,
                    usage,
                    cache_ctx: &cache_ctx,
                    meta: &usage_meta,
                })
                .await
                {
                    tracing::error!(
                        error = %err,
                        "Failed to record gateway usage for route establishment failure"
                    );
                    return kiro_json_error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "api_error",
                        "failed to record usage",
                    );
                }
                return error_response;
            },
        };
        if !response.status().is_success() {
            let upstream_status = response.status();
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = response.bytes().await.unwrap_or_else(|_| Bytes::new());
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_upstream_request_body_json(&mut usage_meta, &request_body);
            capture_error_bytes(&mut usage_meta, &bytes);
            usage_meta.mark_stream_finish();
            let prompt_too_long_response = kiro_prompt_too_long_response_for_body(
                upstream_status,
                &bytes,
                &effective_model,
                request_input_tokens,
            );
            let status = if prompt_too_long_response.is_some() {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                upstream_status
            };
            let usage = build_kiro_usage_summary(
                &effective_model,
                KiroUsageInputs {
                    request_input_tokens,
                    context_input_tokens: None,
                    context_usage_min_request_tokens: route.context_usage_min_request_tokens,
                    output_tokens: 0,
                    credit_usage: None,
                    credit_usage_missing: true,
                    cache_estimation_enabled: false,
                },
                &cache_ctx,
            );
            if let Err(err) = record_kiro_usage(KiroUsageRecord {
                control_store: control_store.as_ref(),
                key: &key,
                route: &route,
                endpoint: public_path,
                model: &effective_model,
                status,
                usage,
                cache_ctx: &cache_ctx,
                meta: &usage_meta,
            })
            .await
            {
                tracing::error!(
                    error = %err,
                    "Failed to record gateway usage for upstream error response"
                );
                return kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "failed to record usage",
                );
            }
            return prompt_too_long_response.unwrap_or_else(|| {
                kiro_upstream_error_response(upstream_status, &content_type, bytes)
            });
        }
        if payload.stream {
            let stream_response = match prepare_kiro_stream_response_for_route(
                response,
                &route,
                KiroStreamPrepareContext {
                    route_store: route_store.as_ref(),
                    upstream_url: &upstream_url,
                    request_body: &request_body,
                    usage_meta: &mut usage_meta,
                    model: &effective_model,
                    request_input_tokens,
                },
            )
            .await
            {
                Ok(stream_response) => stream_response,
                Err(failure) => {
                    if should_failover_after_kiro_route_failure(
                        &failure,
                        &route,
                        &routes,
                        &mut failed_accounts,
                        route_store.as_ref(),
                        &kiro_request_scheduler,
                    )
                    .await
                    {
                        usage_meta.mark_failover();
                        continue;
                    }
                    let status = failure.status;
                    capture_client_request_body_json(&mut usage_meta, &body);
                    capture_upstream_request_body_json(&mut usage_meta, &request_body);
                    capture_error_bytes(&mut usage_meta, &failure.body);
                    usage_meta.mark_stream_finish();
                    let usage = build_kiro_usage_summary(
                        &effective_model,
                        KiroUsageInputs {
                            request_input_tokens,
                            context_input_tokens: None,
                            context_usage_min_request_tokens: route
                                .context_usage_min_request_tokens,
                            output_tokens: 0,
                            credit_usage: None,
                            credit_usage_missing: true,
                            cache_estimation_enabled: false,
                        },
                        &cache_ctx,
                    );
                    if let Err(err) = record_kiro_usage(KiroUsageRecord {
                        control_store: control_store.as_ref(),
                        key: &key,
                        route: &route,
                        endpoint: public_path,
                        model: &effective_model,
                        status,
                        usage,
                        cache_ctx: &cache_ctx,
                        meta: &usage_meta,
                    })
                    .await
                    {
                        tracing::error!(
                            error = %err,
                            "Failed to record gateway usage for buffered stream failure"
                        );
                        return kiro_json_error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "api_error",
                            "failed to record usage",
                        );
                    }
                    return failure.into_response();
                },
            };
            remember_kiro_session_affinity(
                kiro_session_affinity.as_ref(),
                &key.key_id,
                affinity_session_id.as_deref(),
                &route.account_name,
            );
            let (private_prompt_safety_enabled, response_identity) =
                route_private_prompt_safety(&route, &conversion);
            let response_ctx = KiroResponseContext {
                key,
                route,
                public_path: public_path.to_string(),
                model: effective_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                protected_thinking_signature_secret: protected_thinking_signature_secret.clone(),
                tool_name_map: conversion.tool_name_map.clone(),
                structured_output_tool_name: conversion.structured_output_tool_name.clone(),
                response_identity,
                private_prompt_safety_enabled,
                cache_ctx,
                control_store,
                kiro_cache_simulator,
                usage_meta,
                affinity_update: None,
                _key_permit: key_permit
                    .take()
                    .expect("kiro key permit should be held until response is returned"),
                _account_permit: account_permit,
            };
            return stream_kiro_upstream_response(stream_response, response_ctx);
        }
        let affinity_update =
            affinity_session_id
                .clone()
                .map(|session_id| KiroResponseAffinityUpdate {
                    affinity: Arc::clone(&kiro_session_affinity),
                    session_id,
                });
        let (private_prompt_safety_enabled, response_identity) =
            route_private_prompt_safety(&route, &conversion);
        let response_ctx = KiroResponseContext {
            key,
            route,
            public_path: public_path.to_string(),
            model: effective_model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            protected_thinking_signature_secret,
            tool_name_map: conversion.tool_name_map.clone(),
            structured_output_tool_name: conversion.structured_output_tool_name.clone(),
            response_identity,
            private_prompt_safety_enabled,
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            affinity_update,
            _key_permit: key_permit
                .take()
                .expect("kiro key permit should be held until response is returned"),
            _account_permit: account_permit,
        };
        return non_stream_kiro_response(response, response_ctx).await;
    }
}

fn inconsistent_cctest_text_handling_response(routes: &[ProviderKiroRoute]) -> Option<Response> {
    let first = routes.first()?;
    if routes
        .iter()
        .skip(1)
        .any(|route| route.cctest_text_handling_enabled != first.cctest_text_handling_enabled)
    {
        tracing::error!(
            accounts = ?routes
                .iter()
                .map(|route| (
                    route.account_name.as_str(),
                    route.cctest_text_handling_enabled,
                ))
                .collect::<Vec<_>>(),
            "kiro route candidates disagree on cctest text handling"
        );
        return Some(kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            INCONSISTENT_ROUTE_CONFIGURATION_MESSAGE,
        ));
    }
    None
}

struct CctestTextProbeDispatch {
    key: AuthenticatedKey,
    route: ProviderKiroRoute,
    endpoint: &'static str,
    request_headers: HeaderMap,
    body: Bytes,
    probe: CctestProbeMatch,
    control_store: Arc<dyn llm_access_core::store::ControlStore>,
    usage_meta: ProviderUsageMetadata,
}

async fn handle_cctest_text_probe(input: CctestTextProbeDispatch) -> Response {
    let CctestTextProbeDispatch {
        key,
        route,
        endpoint,
        request_headers,
        body,
        probe,
        control_store,
        mut usage_meta,
    } = input;
    capture_client_request_body_json(&mut usage_meta, &body);
    usage_meta.full_request_json = Some(body.clone());
    let proxy_base_url = route.cctest_proxy_base_url.clone();
    let proxy_api_key = route.cctest_proxy_api_key.clone();
    tracing::info!(
        key_id = %key.key_id,
        key_name = %key.key_name,
        account = %route.account_name,
        endpoint,
        cctest_request_id = %probe.request_id,
        cctest_probe_kind = %probe.kind.as_str(),
        cctest_requires_signature = probe.requires_signature,
        "handling kiro cctest probe"
    );
    let (status, content_type, response_body, handling_mode) = if probe.requires_signature {
        match (proxy_base_url.as_deref(), proxy_api_key.as_deref()) {
            (Some(base_url), Some(api_key)) => {
                forward_cctest_signature_probe(
                    &request_headers,
                    &body,
                    base_url,
                    api_key,
                    endpoint,
                    &mut usage_meta,
                )
                .await
            },
            (None, _) => cctest_proxy_config_error(
                &mut usage_meta,
                "AWS Bedrock error message: cctest signature proxy base URL is not configured",
            ),
            (_, None) => cctest_proxy_config_error(
                &mut usage_meta,
                "AWS Bedrock error message: cctest signature proxy API key is not configured",
            ),
        }
    } else {
        let (content_type, response_text) = build_direct_replay_body(&probe);
        let response_body = Bytes::from(response_text);
        (StatusCode::OK, content_type, response_body, "replay")
    };
    capture_response_body(&mut usage_meta, &bytes_to_string(&response_body));
    mark_cctest_response_stream_details(&mut usage_meta, &content_type, response_body.len());
    tracing::info!(
        key_id = %key.key_id,
        key_name = %key.key_name,
        account = %route.account_name,
        endpoint,
        cctest_request_id = %probe.request_id,
        cctest_probe_kind = %probe.kind.as_str(),
        cctest_handling_mode = handling_mode,
        status = status.as_u16(),
        content_type = %content_type,
        response_bytes = response_body.len(),
        "completed kiro cctest probe"
    );
    let response = response_with_body(status, &content_type, response_body.clone());
    if let Err(err) = record_kiro_cctest_usage(KiroCctestUsageRecord {
        control_store: control_store.as_ref(),
        key: &key,
        route: &route,
        endpoint,
        model: probe.model.as_deref(),
        status,
        request_id: &probe.request_id,
        probe_kind: probe.kind.as_str(),
        handling_mode,
        requires_signature: probe.requires_signature,
        meta: &usage_meta,
    })
    .await
    {
        tracing::warn!(
            key_id = %key.key_id,
            account = %route.account_name,
            request_id = %probe.request_id,
            error = %err,
            "failed to record kiro cctest usage"
        );
    }
    response
}

async fn forward_cctest_signature_probe(
    request_headers: &HeaderMap,
    body: &Bytes,
    base_url: &str,
    api_key: &str,
    endpoint: &str,
    usage_meta: &mut ProviderUsageMetadata,
) -> (StatusCode, String, Bytes, &'static str) {
    capture_upstream_request_body_json(usage_meta, body);
    let target_url = cctest::proxy_target_url(base_url, endpoint);
    if let Err(message) = cctest::validate_proxy_target_url(&target_url) {
        return cctest_proxy_config_error(usage_meta, message);
    }
    tracing::info!(
        endpoint,
        target_url = %target_url,
        request_bytes = body.len(),
        "forwarding kiro cctest signature probe"
    );
    let mut request = cctest_proxy_client()
        .post(&target_url)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {api_key}"))
        .header("x-api-key", api_key)
        .body(body.clone());
    for header_name in ["anthropic-version", "anthropic-beta", "user-agent"] {
        if let Some(value) = request_headers.get(header_name) {
            request = request.header(header_name, value.clone());
        }
    }
    match request.send().await {
        Ok(response) => {
            usage_meta.mark_upstream_headers();
            let status =
                StatusCode::from_u16(response.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            let content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = response.bytes().await.unwrap_or_else(|_| Bytes::new());
            usage_meta.mark_post_headers_body();
            if !status.is_success() {
                capture_error_bytes(usage_meta, &bytes);
            }
            tracing::info!(
                endpoint,
                target_url = %target_url,
                status = status.as_u16(),
                content_type = %content_type,
                response_bytes = bytes.len(),
                "received kiro cctest signature proxy response"
            );
            (status, content_type, bytes, "proxy")
        },
        Err(err) => {
            let message =
                format!("AWS Bedrock error message: cctest signature proxy request failed: {err}");
            let body = anthropic_json_error_body("api_error", &message);
            capture_error_message(usage_meta, &message);
            capture_error_body(usage_meta, &body);
            tracing::warn!(
                endpoint,
                target_url = %target_url,
                error = %err,
                "kiro cctest signature proxy request failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                "application/json".to_string(),
                Bytes::from(body),
                "proxy_error",
            )
        },
    }
}

fn response_with_body(status: StatusCode, content_type: &str, body: Bytes) -> Response {
    Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap_or_else(|_| {
            anthropic_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build response",
            )
        })
}

fn mark_cctest_response_stream_details(
    usage_meta: &mut ProviderUsageMetadata,
    content_type: &str,
    bytes_len: usize,
) {
    if content_type.starts_with("text/event-stream") {
        usage_meta.observe_stream_write(bytes_len, Some("message_stop"));
        usage_meta.mark_stream_completed_cleanly();
    } else {
        usage_meta.mark_stream_finish();
    }
}

fn cctest_proxy_config_error(
    usage_meta: &mut ProviderUsageMetadata,
    message: &str,
) -> (StatusCode, String, Bytes, &'static str) {
    let body = anthropic_json_error_body("api_error", message);
    capture_error_message(usage_meta, message);
    capture_error_body(usage_meta, &body);
    tracing::warn!(error_message = %message, "kiro cctest proxy configuration is incomplete");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "application/json".to_string(),
        Bytes::from(body),
        "proxy_config_error",
    )
}

async fn record_kiro_route_selection_failure_response(
    control_store: &dyn llm_access_core::store::ControlStore,
    key: &AuthenticatedKey,
    endpoint: &str,
    model: Option<&str>,
    usage_meta: &mut ProviderUsageMetadata,
    status: StatusCode,
) {
    let (class, message) = if status == StatusCode::TOO_MANY_REQUESTS {
        ("aws_bedrock_all_accounts_cooling_down", AWS_BEDROCK_ALL_ACCOUNTS_COOLING_DOWN_MESSAGE)
    } else {
        ("aws_bedrock_route_selection_failed", AWS_BEDROCK_ROUTE_SELECTION_FAILED_MESSAGE)
    };
    capture_error_message(usage_meta, message);
    capture_error_body(usage_meta, &anthropic_json_error_body("api_error", message));
    usage_meta.capture_error_class(class);
    usage_meta.mark_stream_finish();
    if let Err(err) = record_kiro_selection_failure(KiroSelectionFailureRecord {
        control_store,
        key,
        endpoint,
        model,
        status,
        meta: usage_meta,
    })
    .await
    {
        tracing::warn!(
            key_id = %key.key_id,
            endpoint,
            status = %status,
            error = %err,
            "failed to record kiro route selection failure usage"
        );
    }
}

async fn dispatch_kiro_websearch(input: KiroWebsearchDispatch) -> Response {
    let KiroWebsearchDispatch {
        key,
        payload,
        routes,
        control_store,
        route_store,
        request_limiter,
        kiro_request_scheduler,
        kiro_session_affinity,
        kiro_latency_ranker,
        affinity_session_id,
        requested_model,
        model_group_preferred_account_names,
        request_input_tokens,
        mut usage_meta,
    } = input;
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return kiro_key_limit_response(&rejection),
    };
    let Some(query) = websearch::extract_search_query(&payload) else {
        return kiro_json_error(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "Unable to extract web search query from messages.",
        );
    };
    let (tool_use_id, mcp_request) = websearch::create_mcp_request(&query);
    let request_body = match serde_json::to_string(&mcp_request) {
        Ok(body) => body,
        Err(err) => {
            return kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                &format!("failed to encode kiro mcp request: {err}"),
            )
        },
    };

    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    let preferred_account_name = affinity_session_id
        .as_deref()
        .and_then(|session_id| kiro_session_affinity.lookup(&key.key_id, session_id));
    log_kiro_model_group_preference_routing(
        &key,
        "/mcp",
        &requested_model,
        payload.model.as_str(),
        model_group_preferred_account_names.as_ref(),
        preferred_account_name.as_deref(),
    );
    // New session (has a session id but no existing affinity): balance it onto
    // the least-bound account. Skipped for single-account keys (nothing to
    // spread). Computed once — affinity only mutates on success after the loop,
    // so it is stable across failover retries.
    let session_counts =
        (routes.len() > 1 && affinity_session_id.is_some() && preferred_account_name.is_none())
            .then(|| kiro_session_affinity.account_session_counts());
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
            preferred_account_name.as_deref(),
            model_group_preferred_account_names.as_ref(),
            session_counts.as_ref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => {
                record_kiro_route_selection_failure_response(
                    control_store.as_ref(),
                    &key,
                    "/mcp",
                    Some(payload.model.as_str()),
                    &mut usage_meta,
                    response.status(),
                )
                .await;
                return response;
            },
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_kiro_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => {
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if has_remaining_kiro_candidate(&routes, &failed_accounts, "") {
                    continue;
                }
                return response;
            },
        };
        let mut route_usage_meta = usage_meta.clone();
        match call_kiro_mcp_for_route(
            &route,
            route_store.as_ref(),
            &request_body,
            &mut route_usage_meta,
        )
        .await
        {
            Ok(mcp_response) => {
                let capture_request_details = route.full_request_logging_enabled;
                if capture_request_details {
                    capture_upstream_request_body_json(
                        &mut route_usage_meta,
                        request_body.as_bytes(),
                    );
                }
                route_usage_meta.mark_upstream_headers();
                route_usage_meta.mark_post_headers_body();
                route_usage_meta.mark_stream_finish();
                remember_kiro_session_affinity(
                    kiro_session_affinity.as_ref(),
                    &key.key_id,
                    affinity_session_id.as_deref(),
                    &route.account_name,
                );
                return build_kiro_websearch_response(WebsearchResponseInput {
                    key,
                    route,
                    payload,
                    query,
                    tool_use_id,
                    search_results: websearch::parse_search_results(&mcp_response),
                    request_input_tokens,
                    status: StatusCode::OK,
                    control_store,
                    usage_meta: route_usage_meta,
                    capture_request_details,
                    _key_permit: key_permit
                        .take()
                        .expect("kiro key permit should be held until response is returned"),
                    _account_permit: account_permit,
                })
                .await;
            },
            Err(failure) => {
                if should_failover_after_kiro_route_failure(
                    &failure,
                    &route,
                    &routes,
                    &mut failed_accounts,
                    route_store.as_ref(),
                    &kiro_request_scheduler,
                )
                .await
                {
                    usage_meta.merge_retry_from(&route_usage_meta.retry);
                    usage_meta.mark_failover();
                    continue;
                }
                let message = failure.body_text();
                if websearch::should_propagate_mcp_error_text(&message) {
                    return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &message);
                }
                capture_upstream_request_body_json(&mut route_usage_meta, request_body.as_bytes());
                route_usage_meta.mark_stream_finish();
                return build_kiro_websearch_response(WebsearchResponseInput {
                    key,
                    route,
                    payload,
                    query,
                    tool_use_id,
                    search_results: None,
                    request_input_tokens,
                    status: StatusCode::OK,
                    control_store,
                    usage_meta: route_usage_meta,
                    capture_request_details: true,
                    _key_permit: key_permit
                        .take()
                        .expect("kiro key permit should be held until response is returned"),
                    _account_permit: account_permit,
                })
                .await;
            },
        }
    }
}
async fn build_kiro_websearch_response(input: WebsearchResponseInput) -> Response {
    let summary = websearch::generate_search_summary(&input.query, &input.search_results);
    let output_tokens = websearch::estimate_output_tokens(&summary);
    let usage = KiroUsageSummary {
        input_uncached_tokens: input.request_input_tokens,
        input_cached_tokens: 0,
        output_tokens,
        credit_usage: None,
        credit_usage_missing: true,
    };
    if let Err(err) = record_kiro_websearch_usage(KiroWebsearchUsageRecord {
        control_store: input.control_store.as_ref(),
        key: &input.key,
        route: &input.route,
        model: &input.payload.model,
        status: input.status,
        usage,
        meta: &input.usage_meta,
        capture_request_details: input.capture_request_details,
    })
    .await
    {
        tracing::error!(error = %err, "Failed to record gateway usage for web search response");
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "failed to record usage",
        );
    }

    if input.payload.stream {
        let body = websearch::generate_websearch_events(
            &input.payload.model,
            &input.query,
            &input.tool_use_id,
            input.search_results.as_ref(),
            input.request_input_tokens,
            &summary,
            output_tokens,
        )
        .into_iter()
        .map(|event| event.to_sse_string())
        .collect::<String>();
        return Response::builder()
            .status(input.status)
            .header(header::CONTENT_TYPE, "text/event-stream")
            .header(header::CACHE_CONTROL, "no-cache")
            .header(header::CONNECTION, "keep-alive")
            .body(Body::from(body))
            .unwrap_or_else(|_| {
                kiro_json_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "failed to build stream response",
                )
            });
    }

    let body = serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": websearch::create_non_stream_content_blocks(
            &input.query,
            &input.tool_use_id,
            &input.search_results,
            &summary,
        ),
        "model": input.payload.model,
        "stop_details": null,
        "stop_reason": "end_turn",
        "stop_sequence": null,
        "usage": anthropic_usage_json(input.request_input_tokens, output_tokens, 0),
    });
    Response::builder()
        .status(input.status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(body.to_string()))
        .unwrap_or_else(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build response",
            )
        })
}
async fn peek_kiro_stream(
    response: reqwest::Response,
) -> Result<KiroPeekedStream, KiroStreamPeekError> {
    let status = response.status();
    let mut body_stream = response.bytes_stream();
    let mut buffered_prefix = Vec::new();
    let mut decoder = EventStreamDecoder::new();
    while let Some(chunk_result) = body_stream.next().await {
        match chunk_result {
            Ok(chunk) if !chunk.is_empty() => {
                decoder
                    .feed(&chunk)
                    .map_err(|err| KiroStreamPeekError::Decode(err.to_string()))?;
                buffered_prefix.extend_from_slice(chunk.as_ref());
                let mut decoded_frame = false;
                for frame in decoder.decode_iter() {
                    frame.map_err(|err| KiroStreamPeekError::Decode(err.to_string()))?;
                    decoded_frame = true;
                }
                if decoded_frame {
                    return Ok(KiroPeekedStream {
                        status,
                        buffered_prefix: Bytes::from(buffered_prefix),
                        remaining: body_stream.boxed(),
                    });
                }
            },
            Ok(_) => continue,
            Err(err) => return Err(KiroStreamPeekError::Read(err)),
        }
    }
    if buffered_prefix.is_empty() {
        Err(KiroStreamPeekError::Empty)
    } else {
        Err(KiroStreamPeekError::Incomplete)
    }
}
struct KiroStreamPrepareContext<'a> {
    route_store: &'a dyn ProviderRouteStore,
    upstream_url: &'a str,
    request_body: &'a [u8],
    usage_meta: &'a mut ProviderUsageMetadata,
    model: &'a str,
    request_input_tokens: i32,
}

async fn prepare_kiro_stream_response_for_route(
    initial_response: reqwest::Response,
    route: &ProviderKiroRoute,
    ctx: KiroStreamPrepareContext<'_>,
) -> Result<KiroPeekedStream, KiroRouteFailure> {
    let mut response = initial_response;
    for retry in 0..=KIRO_EMPTY_STREAM_MAX_RETRIES {
        match peek_kiro_stream(response).await {
            Ok(stream) => {
                if kiro_chunk_contains_content_length_exceeded(&stream.buffered_prefix) {
                    return Err(KiroRouteFailure::synthetic(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        kiro_prompt_too_long_message(ctx.model, ctx.request_input_tokens),
                        KiroRouteFailureKind::Fatal,
                    ));
                }
                if retry > 0 {
                    tracing::info!(
                        model = %ctx.model,
                        attempt = retry + 1,
                        "Kiro empty stream retry succeeded"
                    );
                }
                return Ok(stream);
            },
            Err(KiroStreamPeekError::Empty) if retry < KIRO_EMPTY_STREAM_MAX_RETRIES => {
                tracing::warn!(
                    model = %ctx.model,
                    attempt = retry + 1,
                    "Kiro returned an empty generateAssistantResponse stream; retrying"
                );
                let delay =
                    randomized_same_account_retry_delay(SameAccountRetryReason::EmptyStream, None);
                ctx.usage_meta
                    .record_same_account_retry(SameAccountRetryReason::EmptyStream, delay);
                tokio::time::sleep(delay).await;
                response = call_kiro_generate_for_route_with_usage(
                    route,
                    ctx.route_store,
                    ctx.upstream_url.to_string(),
                    ctx.request_body,
                    ctx.usage_meta,
                )
                .await?;
            },
            Err(KiroStreamPeekError::Empty) => {
                tracing::error!(
                    model = %ctx.model,
                    attempts = KIRO_EMPTY_STREAM_MAX_RETRIES + 1,
                    "Kiro returned an empty generateAssistantResponse stream after retries"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    "kiro upstream returned empty generateAssistantResponse stream after retries"
                        .to_string(),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Incomplete) => {
                tracing::error!(
                    model = %ctx.model,
                    "Kiro upstream stream ended before the first complete eventstream frame"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    "kiro upstream ended before the first complete eventstream frame".to_string(),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Decode(err)) => {
                tracing::error!(
                    model = %ctx.model,
                    error = %err,
                    "Failed to decode Kiro upstream stream before sending any response bytes"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to decode kiro upstream stream: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
            Err(KiroStreamPeekError::Read(err)) => {
                tracing::error!(
                    model = %ctx.model,
                    error = %err,
                    "Failed to read Kiro upstream stream before sending any response bytes"
                );
                return Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("failed to read kiro upstream stream: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
        }
    }
    unreachable!("bounded kiro empty stream retry loop should return")
}

#[derive(Debug, Clone, Copy)]
enum KiroUpstreamFailureAction {
    QuotaExhausted,
    RateLimited { cooldown: Duration, mark_proxy: bool },
    Fatal,
    AuthRefresh,
    RetryNext,
    RetryableStatus { retry_after: Option<Duration> },
}

fn classify_kiro_upstream_failure(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    force_refresh: bool,
) -> KiroUpstreamFailureAction {
    if status.as_u16() == 402 && is_monthly_request_limit(body) {
        return KiroUpstreamFailureAction::QuotaExhausted;
    }
    if status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(cooldown) = kiro_rate_limit_cooldown(headers, body) {
            return KiroUpstreamFailureAction::RateLimited {
                cooldown,
                mark_proxy: false,
            };
        }
    }
    if status == StatusCode::BAD_REQUEST {
        return transient_invalid_model_cooldown(body)
            .map(|cooldown| KiroUpstreamFailureAction::RateLimited {
                cooldown,
                mark_proxy: true,
            })
            .unwrap_or(KiroUpstreamFailureAction::Fatal);
    }
    if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
        return if force_refresh {
            KiroUpstreamFailureAction::RetryNext
        } else {
            KiroUpstreamFailureAction::AuthRefresh
        };
    }
    if status == StatusCode::REQUEST_TIMEOUT || status.is_server_error() {
        return KiroUpstreamFailureAction::RetryableStatus {
            retry_after: retry_after_header_duration(headers),
        };
    }
    KiroUpstreamFailureAction::Fatal
}

async fn record_same_account_retry_and_sleep(
    usage_meta: &mut ProviderUsageMetadata,
    reason: SameAccountRetryReason,
    retry_after: Option<Duration>,
) {
    let delay = randomized_same_account_retry_delay(reason, retry_after);
    usage_meta.record_same_account_retry(reason, delay);
    tokio::time::sleep(delay).await;
}

pub(crate) async fn call_kiro_generate_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: String,
    request_body: &[u8],
) -> Result<reqwest::Response, KiroRouteFailure> {
    let mut usage_meta = ProviderUsageMetadata::synthetic_request("POST", upstream_url.clone());
    call_kiro_generate_for_route_inner(
        route,
        route_store,
        upstream_url,
        request_body,
        &mut usage_meta,
    )
    .await
}

async fn call_kiro_generate_for_route_with_usage(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: String,
    request_body: &[u8],
    usage_meta: &mut ProviderUsageMetadata,
) -> Result<reqwest::Response, KiroRouteFailure> {
    call_kiro_generate_for_route_inner(route, route_store, upstream_url, request_body, usage_meta)
        .await
}

async fn call_kiro_generate_for_route_inner(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: String,
    request_body: &[u8],
    usage_meta: &mut ProviderUsageMetadata,
) -> Result<reqwest::Response, KiroRouteFailure> {
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    for attempt in 0..KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
        let call_ctx =
            match kiro_refresh::ensure_context_for_route(route, route_store, force_refresh).await {
                Ok(ctx) => ctx,
                Err(err) => {
                    return Err(KiroRouteFailure::synthetic(
                        StatusCode::BAD_GATEWAY,
                        format!("kiro auth refresh failed for {}: {err}", route.account_name),
                        KiroRouteFailureKind::RetryNext,
                    ));
                },
            };
        let response = match send_kiro_generate_request(
            route,
            &call_ctx,
            upstream_url.clone(),
            request_body.to_vec(),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                last_failure = Some(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("kiro upstream transport failure: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
                if attempt + 1 < KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
                    record_same_account_retry_and_sleep(
                        usage_meta,
                        SameAccountRetryReason::Transport,
                        None,
                    )
                    .await;
                    continue;
                }
                return Err(last_failure.expect("kiro transport failure should be captured"));
            },
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let headers = response.headers().clone();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        match classify_kiro_upstream_failure(status, &headers, &body, force_refresh) {
            KiroUpstreamFailureAction::QuotaExhausted => {
                return Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
            },
            KiroUpstreamFailureAction::RateLimited {
                cooldown,
                mark_proxy,
            } => {
                return Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy,
                }));
            },
            KiroUpstreamFailureAction::Fatal => {
                return Err(failure.with_kind(KiroRouteFailureKind::Fatal));
            },
            KiroUpstreamFailureAction::AuthRefresh => {
                force_refresh = true;
                last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
                record_same_account_retry_and_sleep(
                    usage_meta,
                    SameAccountRetryReason::AuthRefresh,
                    None,
                )
                .await;
                continue;
            },
            KiroUpstreamFailureAction::RetryNext => {
                return Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
            },
            KiroUpstreamFailureAction::RetryableStatus {
                retry_after,
            } => {
                last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
                if attempt + 1 < KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
                    record_same_account_retry_and_sleep(
                        usage_meta,
                        SameAccountRetryReason::RetryableStatus,
                        retry_after,
                    )
                    .await;
                    continue;
                }
                return Err(last_failure.expect("retryable kiro failure should be captured"));
            },
        }
    }
    Err(last_failure.unwrap_or_else(|| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            "kiro upstream request failed".to_string(),
            KiroRouteFailureKind::RetryNext,
        )
    }))
}
pub async fn call_kiro_mcp_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    request_body: &str,
    usage_meta: &mut ProviderUsageMetadata,
) -> Result<McpResponse, KiroRouteFailure> {
    let upstream_url =
        format!("{}/mcp", kiro_refresh::runtime_upstream_base_url(&route.api_region));
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    let mut attempt = 0usize;
    let response = loop {
        if attempt >= KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
            break Err(last_failure.unwrap_or_else(|| {
                KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    "kiro mcp upstream request failed".to_string(),
                    KiroRouteFailureKind::RetryNext,
                )
            }));
        }
        let call_ctx = match kiro_refresh::ensure_context_for_route_requiring_profile(
            route,
            route_store,
            force_refresh,
        )
        .await
        {
            Ok(ctx) => ctx,
            Err(err) => {
                break Err(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("kiro mcp auth refresh failed for {}: {err}", route.account_name),
                    KiroRouteFailureKind::RetryNext,
                ));
            },
        };
        attempt += 1;
        let response = match send_kiro_mcp_request(
            route,
            &call_ctx,
            upstream_url.clone(),
            request_body.to_string(),
        )
        .await
        {
            Ok(response) => response,
            Err(err) => {
                last_failure = Some(KiroRouteFailure::synthetic(
                    StatusCode::BAD_GATEWAY,
                    format!("kiro mcp transport failure: {err}"),
                    KiroRouteFailureKind::RetryNext,
                ));
                if attempt < KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
                    record_same_account_retry_and_sleep(
                        usage_meta,
                        SameAccountRetryReason::Transport,
                        None,
                    )
                    .await;
                    continue;
                }
                break Err(last_failure.expect("mcp transport failure should be captured"));
            },
        };
        if response.status().is_success() {
            break Ok(response);
        }
        let status = response.status();
        let headers = response.headers().clone();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        match classify_kiro_upstream_failure(status, &headers, &body, force_refresh) {
            KiroUpstreamFailureAction::QuotaExhausted => {
                break Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
            },
            KiroUpstreamFailureAction::RateLimited {
                cooldown,
                mark_proxy,
            } => {
                break Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy,
                }));
            },
            KiroUpstreamFailureAction::Fatal => {
                break Err(failure.with_kind(KiroRouteFailureKind::Fatal));
            },
            KiroUpstreamFailureAction::AuthRefresh => {
                force_refresh = true;
                last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
                record_same_account_retry_and_sleep(
                    usage_meta,
                    SameAccountRetryReason::AuthRefresh,
                    None,
                )
                .await;
                continue;
            },
            KiroUpstreamFailureAction::RetryNext => {
                break Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
            },
            KiroUpstreamFailureAction::RetryableStatus {
                retry_after,
            } => {
                last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
                if attempt < KIRO_SAME_ACCOUNT_MAX_ATTEMPTS {
                    record_same_account_retry_and_sleep(
                        usage_meta,
                        SameAccountRetryReason::RetryableStatus,
                        retry_after,
                    )
                    .await;
                    continue;
                }
                break Err(last_failure.expect("retryable mcp failure should be captured"));
            },
        }
    }?;
    let body = response.text().await.map_err(|err| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!("read kiro mcp response body: {err}"),
            KiroRouteFailureKind::RetryNext,
        )
    })?;
    let mcp_response = serde_json::from_str::<McpResponse>(&body).map_err(|err| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!("parse kiro mcp response body: {err}; body={body}"),
            KiroRouteFailureKind::Fatal,
        )
    })?;
    if let Some(error) = &mcp_response.error {
        return Err(KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            format!(
                "MCP error: {} - {}",
                error.code.unwrap_or(-1),
                error.message.as_deref().unwrap_or("Unknown error")
            ),
            KiroRouteFailureKind::Fatal,
        ));
    }
    Ok(mcp_response)
}
async fn send_kiro_generate_request(
    route: &ProviderKiroRoute,
    call_ctx: &kiro_refresh::KiroCallContext,
    upstream_url: String,
    request_body: Vec<u8>,
) -> anyhow::Result<reqwest::Response> {
    let client = provider_client(route.proxy.as_ref())?;
    let request_body =
        kiro_request_body_with_profile_arn(request_body, call_ctx.auth.profile_arn.as_deref())?;
    Ok(add_kiro_upstream_headers(
        client.post(&upstream_url),
        &upstream_url,
        &call_ctx.access_token,
        Some(&call_ctx.auth),
    )?
    .body(request_body)
    .send()
    .await?)
}
async fn send_kiro_mcp_request(
    route: &ProviderKiroRoute,
    call_ctx: &kiro_refresh::KiroCallContext,
    upstream_url: String,
    request_body: String,
) -> anyhow::Result<reqwest::Response> {
    let client = provider_client(route.proxy.as_ref())?;
    Ok(add_kiro_mcp_headers(
        client.post(&upstream_url),
        &upstream_url,
        call_ctx.auth.profile_arn.as_deref(),
        &call_ctx.access_token,
        Some(&call_ctx.auth),
    )?
    .body(request_body)
    .send()
    .await?)
}
fn kiro_request_body_with_profile_arn(
    request_body: Vec<u8>,
    profile_arn: Option<&str>,
) -> anyhow::Result<Vec<u8>> {
    let mut value: serde_json::Value =
        serde_json::from_slice(&request_body).context("parse kiro request body json")?;
    let Some(object) = value.as_object_mut() else {
        bail!("kiro request body must be a json object");
    };
    if let Some(profile_arn) = profile_arn.map(str::trim).filter(|value| !value.is_empty()) {
        object.insert("profileArn".to_string(), serde_json::Value::String(profile_arn.to_string()));
    } else {
        object.remove("profileArn");
    }
    serde_json::to_vec(&value).context("serialize kiro request body json")
}
fn has_remaining_kiro_candidate(
    routes: &[ProviderKiroRoute],
    failed_accounts: &HashSet<String>,
    current_account_name: &str,
) -> bool {
    routes.iter().any(|candidate| {
        candidate.account_name != current_account_name
            && !failed_accounts.contains(&candidate.account_name)
    })
}
async fn should_failover_after_kiro_route_failure(
    failure: &KiroRouteFailure,
    route: &ProviderKiroRoute,
    routes: &[ProviderKiroRoute],
    failed_accounts: &mut HashSet<String>,
    route_store: &dyn ProviderRouteStore,
    scheduler: &KiroRequestScheduler,
) -> bool {
    match failure.kind {
        KiroRouteFailureKind::QuotaExhausted => {
            let error_message = failure.body_text();
            for account_name in
                account_names_for_kiro_routing_identity(routes, &route.routing_identity)
            {
                failed_accounts.insert(account_name.clone());
                let _ = route_store
                    .mark_kiro_account_quota_exhausted(&account_name, &error_message, now_millis())
                    .await;
            }
            has_remaining_kiro_candidate(routes, failed_accounts, &route.account_name)
        },
        KiroRouteFailureKind::RateLimited {
            cooldown,
            mark_proxy,
        } => {
            let error_message = failure.body_text();
            scheduler.mark_account_cooldown(
                &route.routing_identity,
                cooldown,
                error_message.clone(),
            );
            if mark_proxy {
                if let Some(proxy_key) = proxy_cooldown_key_for_route(route) {
                    scheduler.mark_proxy_cooldown(&proxy_key, cooldown, error_message);
                }
            }
            true
        },
        KiroRouteFailureKind::RetryNext => {
            failed_accounts.insert(route.account_name.clone());
            has_remaining_kiro_candidate(routes, failed_accounts, &route.account_name)
        },
        KiroRouteFailureKind::Fatal => false,
    }
}
pub fn account_names_for_kiro_routing_identity(
    routes: &[ProviderKiroRoute],
    routing_identity: &str,
) -> Vec<String> {
    routes
        .iter()
        .filter(|route| route.routing_identity == routing_identity)
        .map(|route| route.account_name.clone())
        .collect()
}

/// Allocation-free case-insensitive substring search. `needle` must be ASCII
/// (our markers are), so we can compare byte windows with
/// `eq_ignore_ascii_case` instead of allocating a lowercased copy of `haystack`
/// — which matters because the haystack can be megabytes of request text.
fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

/// True when `text` looks like Claude Code's conversation-compaction summary
/// instruction. Matching is intentionally generous: a false positive only lets
/// an oversized normal request through to the model (benign — the model's real
/// window still applies), whereas a false negative would gate the client's own
/// reactive-compaction summary request and deadlock its compaction loop.
fn text_is_compaction_summary_prompt(text: &str) -> bool {
    contains_ignore_ascii_case(text, "detailed summary of")
        && contains_ignore_ascii_case(text, "conversation")
}
fn json_value_contains_compaction_summary(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(text) => text_is_compaction_summary_prompt(text),
        serde_json::Value::Array(items) => items.iter().any(json_value_contains_compaction_summary),
        serde_json::Value::Object(map) => map.values().any(json_value_contains_compaction_summary),
        _ => false,
    }
}
/// Detects whether this request is the client's conversation-compaction summary
/// request (rather than a normal turn). Such requests are exempt from the
/// proactive compaction gate — they must always reach the model.
///
/// Only the active request's instruction surface is scanned — the system prompt
/// and the *last* message — not the whole history. Scanning all messages would
/// let any earlier turn that merely quoted "detailed summary of ...
/// conversation" permanently exempt every later turn in that conversation, so a
/// large normal follow-up could then slip past the gate and hit Kiro's hard
/// limit.
fn request_is_compaction_summary(payload: &MessagesRequest) -> bool {
    if let Some(system) = payload.system.as_deref() {
        if system
            .iter()
            .any(|message| text_is_compaction_summary_prompt(&message.text))
        {
            return true;
        }
    }
    payload
        .messages
        .last()
        .is_some_and(|message| json_value_contains_compaction_summary(&message.content))
}

/// Estimates the current turn's real (upstream contextUsage-equivalent) input
/// tokens for the proactive-compaction gate, from the local estimate plus the
/// previous turn's cached counts. Returns `max(local_now, real_prev + delta)`
/// where `delta = max(0, local_now - local_prev)` adds this turn's growth on
/// top of the previous real baseline. Falls back to `local_now` when nothing is
/// cached (e.g. first turn, or just after a compaction changed the prefix).
fn estimate_effective_input_tokens(local_now: i32, recovered: Option<AnchorTokenCounts>) -> i32 {
    match recovered {
        Some(counts) => {
            let delta = local_now.saturating_sub(counts.local_input_tokens).max(0);
            let estimated_real = counts.real_input_tokens.saturating_add(delta);
            local_now.max(estimated_real)
        },
        None => local_now,
    }
}

#[cfg(test)]
mod compaction_gate_tests {
    use llm_access_kiro::{
        anthropic::types::{Message, MessagesRequest, SystemMessage},
        cache_sim::AnchorTokenCounts,
    };
    use serde_json::json;

    use super::{estimate_effective_input_tokens, request_is_compaction_summary};

    fn request(
        system: Option<Vec<&str>>,
        messages: Vec<(&str, serde_json::Value)>,
    ) -> MessagesRequest {
        MessagesRequest {
            model: "claude-opus-4-8".to_string(),
            _max_tokens: 32000,
            messages: messages
                .into_iter()
                .map(|(role, content)| Message {
                    role: role.to_string(),
                    content,
                })
                .collect(),
            stream: false,
            system: system.map(|texts| {
                texts
                    .into_iter()
                    .map(|text| SystemMessage {
                        text: text.to_string(),
                    })
                    .collect()
            }),
            tools: None,
            _tool_choice: None,
            thinking: None,
            output_config: None,
            metadata: None,
        }
    }

    #[test]
    fn detects_summary_instruction_in_last_user_message() {
        let req = request(None, vec![
            ("user", json!("help me write a function")),
            ("assistant", json!("sure")),
            (
                "user",
                json!(
                    "Your task is to create a detailed summary of this conversation. This summary \
                     will be placed at the start of a continuing session."
                ),
            ),
        ]);
        assert!(request_is_compaction_summary(&req));
    }

    #[test]
    fn detects_summary_instruction_in_content_blocks() {
        let req = request(None, vec![(
            "user",
            json!([{
                "type": "text",
                "text": "create a detailed summary of the conversation so far, paying close attention to the user's requests"
            }]),
        )]);
        assert!(request_is_compaction_summary(&req));
    }

    #[test]
    fn detects_summary_instruction_in_system() {
        let req = request(
            Some(vec!["Your task is to create a detailed summary of this conversation."]),
            vec![("user", json!("anything"))],
        );
        assert!(request_is_compaction_summary(&req));
    }

    #[test]
    fn normal_request_is_not_compaction() {
        let req = request(Some(vec!["You are a helpful coding assistant."]), vec![
            ("user", json!("refactor this module")),
            ("assistant", json!([{"type": "text", "text": "done"}])),
        ]);
        assert!(!request_is_compaction_summary(&req));
    }

    #[test]
    fn post_compaction_continuation_text_is_not_matched() {
        // The injected continuation text after a compaction lacks the "detailed
        // summary of" instruction fragment, so it must not be treated as a
        // compaction request (otherwise every post-compaction turn would be
        // exempted from the gate forever).
        let req = request(None, vec![(
            "user",
            json!(
                "This session is being continued from a previous conversation that ran out of \
                 context. The summary below covers the earlier portion of the conversation."
            ),
        )]);
        assert!(!request_is_compaction_summary(&req));
    }

    #[test]
    fn earlier_history_summary_phrase_does_not_exempt_later_turn() {
        // CR#5: only the LAST message (+ system) is scanned. An earlier turn
        // that merely quoted the instruction must not permanently exempt the
        // conversation — the final normal turn here must NOT be detected.
        let req = request(None, vec![
            ("user", json!("Your task is to create a detailed summary of this conversation.")),
            ("assistant", json!("(summary)")),
            ("user", json!("now add pagination to the /users endpoint")),
        ]);
        assert!(!request_is_compaction_summary(&req));
    }

    #[test]
    fn effective_tokens_falls_back_to_local_without_cache() {
        assert_eq!(estimate_effective_input_tokens(640_000, None), 640_000);
    }

    #[test]
    fn effective_tokens_adds_current_delta_to_recovered_real() {
        // prev: real 760k, local 740k; now local 770k → delta 30k → est 790k.
        // CR#4: max(local_now=770k, real_prev+delta=790k) = 790k crosses 780k
        // even though local_now alone (770k) would not.
        let recovered = Some(AnchorTokenCounts {
            real_input_tokens: 760_000,
            local_input_tokens: 740_000,
        });
        assert_eq!(estimate_effective_input_tokens(770_000, recovered), 790_000);
    }

    #[test]
    fn effective_tokens_ignores_shrunk_local_delta() {
        // After a partial trim local_now < local_prev → delta clamps to 0, so
        // the estimate is just the recovered real baseline (not inflated).
        let recovered = Some(AnchorTokenCounts {
            real_input_tokens: 800_000,
            local_input_tokens: 790_000,
        });
        assert_eq!(estimate_effective_input_tokens(500_000, recovered), 800_000);
    }
}
