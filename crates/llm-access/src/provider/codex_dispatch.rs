//! Codex proxy dispatch, upstream-response adaptation, and stream relay.

use std::{
    collections::HashSet,
    sync::Arc,
    time::{Duration, Instant},
};

use async_stream::stream;
use axum::{
    body::{to_bytes, Body, Bytes},
    http::{header, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use eventsource_stream::Eventsource;
use futures_util::{StreamExt, TryStreamExt};
use llm_access_codex::{
    anthropic_messages::{
        convert_json_response_to_anthropic_message, convert_response_event_to_anthropic_sse_chunks,
        AnthropicStreamMetadata,
    },
    request::{
        align_responses_store_with_upstream, apply_codex_fast_policy, apply_codex_resolved_session,
        apply_gpt53_codex_spark_mapping, build_codex_session_resume_anchor_hash,
        extract_last_message_content as extract_codex_last_message_content,
        inject_codex_resolved_session_into_request_body, prepare_gateway_request_from_bytes,
    },
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, rewrite_json_value_model_alias, SseUsageCollector,
    },
    types::{ChatStreamMetadata, CodexResolvedSessionSource, GatewayResponseAdapter},
};
use llm_access_core::store::{AuthenticatedKey, ProviderCodexRoute};
use rand::Rng;
use serde_json::{json, Value};

use super::{
    build_codex_affinity_id,
    client::provider_client,
    codex_auth::{
        add_codex_upstream_headers, codex_upstream_base_url, compute_codex_upstream_url,
        is_codex_invalid_encrypted_content_response, load_codex_dispatch_runtime_config,
        normalized_codex_gateway_path, retry_codex_without_encrypted_reasoning,
    },
    codex_error_disposition::{codex_error_disposition, CodexErrorDisposition},
    codex_models::codex_openai_models_response,
    codex_session_rejection::CodexSessionRejectionEntry,
    codex_sse::{
        completed_response_from_sse_bytes, missing_codex_usage, record_codex_preflight_failure,
        record_codex_usage,
    },
    codex_upstream_error::{
        classify_codex_sse_event_failure, classify_codex_success_error_body,
        classify_codex_upstream_failure, CodexClassifiedUpstreamError, CodexUpstreamErrorClass,
    },
    errors::{
        codex_error_type_for_status, codex_surface_error_body, codex_surface_error_body_with_code,
        codex_surface_error_response, codex_surface_error_response_with_code,
        extract_error_message_from_json_value, summarize_error_bytes,
    },
    limiter::{codex_key_limit_response, try_acquire_key_permit},
    route_selection::{hydrate_codex_route_for_dispatch, select_codex_route_with_account_permit},
    usage_meta::{
        capture_client_request_body_json, capture_codex_dispatch_request_json,
        capture_codex_prepared_request_json, capture_error_body, capture_error_bytes,
        capture_error_message, extract_model_from_json_body, strip_codex_stream_request_bodies,
    },
    util::clamp_duration_ms,
    CodexAccountCooldowns, CodexAffinityId, CodexAffinityRuntimeConfig, CodexAuthSnapshot,
    CodexCompletedResponseContext, CodexPreflightFailureRecord, CodexSessionAffinity,
    CodexSessionRecovery, CodexSessionRecoveryLookup, CodexSessionRecoveryStoreResult,
    CodexStreamContext, CodexStreamRecordGuard, CodexUpstreamResponseContext,
    CodexUpstreamResponseParts, LimitPermit, ProviderDispatchDeps, ProviderUsageMetadata,
    StreamRecordState, CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MAX,
    CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MIN, MAX_PROVIDER_PROXY_BODY_BYTES,
};
use crate::codex_refresh;

pub async fn dispatch_codex_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> Response {
    let ProviderDispatchDeps {
        route_store,
        control_store,
        geoip,
        admin_config_store,
        request_limiter,
        codex_account_cooldowns,
        codex_session_affinity,
        codex_session_recovery,
        codex_session_rejection,
        ..
    } = deps;
    let mut usage_meta = ProviderUsageMetadata::from_request_parts(
        request.method(),
        request.uri(),
        request.headers(),
        &geoip,
    )
    .await;
    let routes = match route_store.resolve_codex_route_candidates(&key).await {
        Ok(routes) if !routes.is_empty() => routes,
        Ok(_) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response()
        },
        Err(_) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, "codex route resolution failed")
                .into_response()
        },
    };
    let Some(gateway_path) =
        normalized_codex_gateway_path(request.uri().path()).map(str::to_string)
    else {
        return (StatusCode::NOT_FOUND, "unsupported codex gateway endpoint").into_response();
    };
    let query = request
        .uri()
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let strict_session_rejection_enabled = routes
        .iter()
        .any(|route| route.codex_strict_session_rejection_enabled);
    let upstream_base = codex_upstream_base_url();
    let method = request.method().clone();
    let request_headers = request.headers().clone();
    let runtime_config = match load_codex_dispatch_runtime_config(admin_config_store.as_ref()).await
    {
        Ok(config) => config,
        Err(response) => return response,
    };

    if gateway_path == "/v1/models" && method == Method::GET {
        let Some(route) = routes.into_iter().next() else {
            return (StatusCode::SERVICE_UNAVAILABLE, "codex route is not configured")
                .into_response();
        };
        let route = match hydrate_codex_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(response) => return response,
        };
        return codex_openai_models_response(
            route,
            route_store,
            &request_headers,
            query.trim_start_matches('?'),
            &upstream_base,
            &runtime_config.client_version,
        )
        .await;
    }

    let body_read_started = Instant::now();
    let body = match to_bytes(request.into_body(), MAX_PROVIDER_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            let message = "request body is too large";
            capture_error_message(&mut usage_meta, message);
            capture_error_body(
                &mut usage_meta,
                &codex_surface_error_body(&gateway_path, StatusCode::BAD_REQUEST, message),
            );
            record_codex_preflight_failure(CodexPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                endpoint: &gateway_path,
                model: None,
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
            })
            .await;
            return codex_surface_error_response(
                &gateway_path,
                StatusCode::BAD_REQUEST,
                "request body is too large",
            );
        },
    };
    usage_meta =
        usage_meta.with_request_body(&body, clamp_duration_ms(body_read_started.elapsed()));
    let parse_started = Instant::now();
    let mut prepared = match prepare_gateway_request_from_bytes(
        &gateway_path,
        &query,
        method,
        &request_headers,
        body.clone(),
        MAX_PROVIDER_PROXY_BODY_BYTES,
    ) {
        Ok(prepared) => prepared,
        Err(err) => {
            capture_client_request_body_json(&mut usage_meta, &body);
            if usage_meta.last_message_content.is_none() {
                usage_meta.last_message_content =
                    extract_codex_last_message_content(&body).ok().flatten();
            }
            tracing::error!(
                key_id = %key.key_id,
                endpoint = %gateway_path,
                status = %err.status,
                error_message = %err.message,
                "codex request rejected before upstream dispatch"
            );
            capture_error_message(&mut usage_meta, &err.message);
            capture_error_body(
                &mut usage_meta,
                &codex_surface_error_body(&gateway_path, err.status, &err.message),
            );
            record_codex_preflight_failure(CodexPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                endpoint: &gateway_path,
                model: extract_model_from_json_body(&body),
                status: err.status,
                meta: &mut usage_meta,
            })
            .await;
            return codex_surface_error_response(&gateway_path, err.status, &err.message);
        },
    };
    usage_meta.mark_pre_handler_done(clamp_duration_ms(parse_started.elapsed()));
    let recovery_outcome = match recover_codex_session_from_projection(
        codex_session_recovery.as_ref(),
        &key,
        &runtime_config.affinity,
        &mut prepared,
    ) {
        Ok(outcome) => outcome,
        Err(response) => return *response,
    };
    usage_meta.last_message_content = prepared.last_message_content.clone();
    let codex_affinity_id = build_codex_affinity_id(
        &key.key_id,
        prepared.resolved_session_id.as_deref(),
        prepared.resolved_session_source,
    );
    if strict_session_rejection_enabled {
        if let Some((affinity_id, rejection)) = codex_affinity_id.as_ref().and_then(|id| {
            codex_session_rejection
                .lookup(id, &runtime_config.affinity)
                .map(|entry| (id, entry))
        }) {
            let message = strict_session_rejection_message(&rejection);
            tracing::warn!(
                key_id = %key.key_id,
                account = %rejection.account_name,
                error_class = %rejection.error_class.as_str(),
                session = %session_preview(&affinity_id.key),
                "codex strict session rejection returned before account selection"
            );
            capture_client_request_body_json(&mut usage_meta, &body);
            capture_error_message(&mut usage_meta, &message);
            capture_error_body(
                &mut usage_meta,
                &codex_surface_error_body(&gateway_path, StatusCode::BAD_REQUEST, &message),
            );
            record_codex_preflight_failure(CodexPreflightFailureRecord {
                control_store: control_store.as_ref(),
                key: &key,
                endpoint: &gateway_path,
                model: prepared
                    .client_visible_model
                    .clone()
                    .or_else(|| prepared.model.clone()),
                status: StatusCode::BAD_REQUEST,
                meta: &mut usage_meta,
            })
            .await;
            return codex_surface_error_response(&gateway_path, StatusCode::BAD_REQUEST, &message);
        }
    }
    let preferred_account_name = codex_affinity_id.as_ref().and_then(|affinity_id| {
        codex_session_affinity.lookup(affinity_id, &runtime_config.affinity)
    });
    let session_counts =
        (routes.len() > 1 && codex_affinity_id.is_some() && preferred_account_name.is_none())
            .then(|| codex_session_affinity.account_session_counts(&runtime_config.affinity));
    usage_meta.routing_diagnostics_json = Some(
        json!({
            "codex_session_source": prepared
                .resolved_session_source
                .map(|source| source.as_str()),
            "codex_session_hash_preview": prepared.resolved_session_hash_preview.as_deref(),
            "codex_session_recovery": recovery_outcome.as_str(),
            "codex_session_bootstrap": prepared
                .resolved_session_source
                .is_some_and(|source| source == CodexResolvedSessionSource::BootstrapRequest),
            "codex_affinity_hit": preferred_account_name.is_some(),
            "codex_new_session_spread": session_counts.is_some(),
        })
        .to_string(),
    );
    let method = match reqwest::Method::from_bytes(prepared.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::METHOD_NOT_ALLOWED, "unsupported method").into_response(),
    };
    let key_permit = match try_acquire_key_permit(
        &request_limiter,
        &key,
        routes[0].request_max_concurrency,
        routes[0].request_min_start_interval_ms,
    ) {
        Ok(permit) => permit,
        Err(rejection) => return codex_key_limit_response(&rejection),
    };
    let account_attempt_limit = runtime_config.account_attempt_limit;
    let mut key_permit = Some(key_permit);
    let mut failed_accounts = HashSet::new();
    let mut attempt_count = 0_usize;
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_codex_route_with_account_permit(
            &request_limiter,
            &codex_account_cooldowns,
            &routes,
            &failed_accounts,
            preferred_account_name.as_deref(),
            session_counts.as_ref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
        };
        usage_meta.add_routing_wait(clamp_duration_ms(route_started.elapsed()));
        attempt_count = attempt_count.saturating_add(1);
        let selected_account_name = route.account_name.clone();
        let route = match hydrate_codex_route_for_dispatch(route, route_store.as_ref()).await {
            Ok(route) => route,
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &selected_account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(selected_account_name);
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        let mut auth = match codex_refresh::ensure_context_for_route(
            &route,
            route_store.as_ref(),
            false,
        )
        .await
        {
            Ok(ctx) => CodexAuthSnapshot {
                access_token: ctx.access_token,
                account_id: ctx.account_id,
                is_fedramp_account: ctx.is_fedramp_account,
            },
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        let prepared =
            match apply_gpt53_codex_spark_mapping(&prepared, route.map_gpt53_codex_to_spark) {
                Ok(prepared) => prepared,
                Err(err) => return (err.status, err.message).into_response(),
            };
        let prepared = match apply_codex_fast_policy(&prepared, route.codex_fast_enabled) {
            Ok(prepared) => prepared,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let prepared = match align_responses_store_with_upstream(&prepared, &upstream_base) {
            Ok(prepared) => prepared,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let prepared = match inject_codex_resolved_session_into_request_body(&prepared) {
            Ok(prepared) => prepared,
            Err(err) => return (err.status, err.message).into_response(),
        };
        let upstream_url = compute_codex_upstream_url(&upstream_base, &prepared.upstream_path);
        let client = match provider_client(route.proxy.as_ref()) {
            Ok(client) => client,
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        let upstream = add_codex_upstream_headers(
            client.request(method.clone(), upstream_url.clone()),
            &request_headers,
            &prepared,
            &auth,
            &runtime_config.client_version,
        );
        let mut response = match upstream.send().await {
            Ok(response) => {
                usage_meta.mark_upstream_headers();
                response
            },
            Err(_) => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                if attempt_count >= account_attempt_limit {
                    return (
                        StatusCode::BAD_GATEWAY,
                        "all eligible codex accounts failed for this request",
                    )
                        .into_response();
                }
                continue;
            },
        };
        if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            match codex_refresh::ensure_context_for_route(&route, route_store.as_ref(), true).await
            {
                Ok(ctx) => {
                    auth = CodexAuthSnapshot {
                        access_token: ctx.access_token,
                        account_id: ctx.account_id,
                        is_fedramp_account: ctx.is_fedramp_account,
                    };
                    let retry = add_codex_upstream_headers(
                        client.request(method.clone(), upstream_url.clone()),
                        &request_headers,
                        &prepared,
                        &auth,
                        &runtime_config.client_version,
                    );
                    response = match retry.send().await {
                        Ok(response) => {
                            usage_meta.mark_upstream_headers();
                            response
                        },
                        Err(_) => {
                            mark_codex_transient_request_failure_cooldown(
                                &codex_account_cooldowns,
                                &route.account_name,
                            );
                            usage_meta.mark_failover();
                            failed_accounts.insert(route.account_name.clone());
                            if attempt_count >= account_attempt_limit {
                                return (
                                    StatusCode::BAD_GATEWAY,
                                    "all eligible codex accounts failed for this request",
                                )
                                    .into_response();
                            }
                            continue;
                        },
                    };
                },
                Err(_) => {
                    mark_codex_transient_request_failure_cooldown(
                        &codex_account_cooldowns,
                        &route.account_name,
                    );
                    usage_meta.mark_failover();
                    failed_accounts.insert(route.account_name.clone());
                    if attempt_count >= account_attempt_limit {
                        return (
                            StatusCode::BAD_GATEWAY,
                            "all eligible codex accounts failed for this request",
                        )
                            .into_response();
                    }
                    continue;
                },
            }
        }
        if matches!(response.status(), StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            let status = response.status();
            let upstream_headers = response.headers().clone();
            let content_type = upstream_headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                        .into_response()
                },
            };
            codex_refresh::persist_terminal_request_auth_error(
                &route,
                route_store.as_ref(),
                status,
                &bytes,
            )
            .await;
            mark_codex_transient_request_failure_cooldown(
                &codex_account_cooldowns,
                &route.account_name,
            );
            if attempt_count < account_attempt_limit
                && routes.iter().any(|candidate| {
                    !failed_accounts.contains(&candidate.account_name)
                        && candidate.account_name != route.account_name
                })
            {
                usage_meta.mark_failover();
                failed_accounts.insert(route.account_name.clone());
                continue;
            }
            let permits = vec![
                key_permit
                    .take()
                    .expect("codex key permit should be held until response is returned"),
                account_permit,
            ];
            capture_codex_dispatch_request_json(&mut usage_meta, &body, &prepared);
            return codex_outcome_response(
                adapt_codex_upstream_response_from_parts(
                    CodexUpstreamResponseParts {
                        status,
                        upstream_headers,
                        content_type,
                        bytes,
                    },
                    CodexCompletedResponseContext {
                        prepared,
                        key,
                        route,
                        control_store,
                        codex_session_recovery: Arc::clone(&codex_session_recovery),
                        codex_session_rejection: Arc::clone(&codex_session_rejection),
                        affinity_config: runtime_config.affinity.clone(),
                        codex_affinity_id: codex_affinity_id.clone(),
                        permits,
                        usage_meta,
                    },
                )
                .await,
            )
            .await;
        }
        if response.status().is_success() {
            remember_codex_affinity(
                codex_session_affinity.as_ref(),
                codex_affinity_id.as_ref(),
                &route.account_name,
                &runtime_config.affinity,
            );
            let permits = vec![
                key_permit
                    .take()
                    .expect("codex key permit should be held until response is returned"),
                account_permit,
            ];
            match classify_codex_upstream_outcome(
                adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                    // Clone the loop-invariant state so it survives a failover
                    // `continue` (the `Retry` arm re-enters the loop and reuses
                    // these for the next account).
                    prepared: prepared.clone(),
                    key: key.clone(),
                    route,
                    control_store: Arc::clone(&control_store),
                    codex_session_recovery: Arc::clone(&codex_session_recovery),
                    codex_session_rejection: Arc::clone(&codex_session_rejection),
                    affinity_config: runtime_config.affinity.clone(),
                    codex_affinity_id: codex_affinity_id.clone(),
                    permits,
                    usage_meta,
                })
                .await,
                &routes,
                &mut failed_accounts,
                account_attempt_limit,
                attempt_count,
                &codex_account_cooldowns,
            ) {
                CodexLoopStep::Respond(response) => return response,
                CodexLoopStep::Surface {
                    error,
                    ctx,
                } => return record_codex_stream_preflight_failure(error, *ctx).await,
                CodexLoopStep::Retry {
                    usage_meta: retry_usage_meta,
                    key_permit: retry_key_permit,
                } => {
                    usage_meta = *retry_usage_meta;
                    key_permit = Some(retry_key_permit);
                    continue;
                },
            }
        }
        let mut response_prepared = prepared.clone();
        let mut status = response.status();
        let mut upstream_headers = response.headers().clone();
        let mut content_type = upstream_headers
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/json")
            .to_string();
        let mut bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                    .into_response()
            },
        };
        if is_codex_invalid_encrypted_content_response(status, &bytes) {
            if let Some(retry_prepared) = retry_codex_without_encrypted_reasoning(&prepared) {
                let retry = add_codex_upstream_headers(
                    client.request(method.clone(), upstream_url.clone()),
                    &request_headers,
                    &retry_prepared,
                    &auth,
                    &runtime_config.client_version,
                );
                response = match retry.send().await {
                    Ok(response) => {
                        usage_meta.mark_upstream_headers();
                        response
                    },
                    Err(_) => {
                        mark_codex_transient_request_failure_cooldown(
                            &codex_account_cooldowns,
                            &route.account_name,
                        );
                        usage_meta.mark_failover();
                        failed_accounts.insert(route.account_name.clone());
                        if attempt_count >= account_attempt_limit {
                            return (
                                StatusCode::BAD_GATEWAY,
                                "all eligible codex accounts failed for this request",
                            )
                                .into_response();
                        }
                        continue;
                    },
                };
                if response.status().is_success() {
                    remember_codex_affinity(
                        codex_session_affinity.as_ref(),
                        codex_affinity_id.as_ref(),
                        &route.account_name,
                        &runtime_config.affinity,
                    );
                    let permits = vec![
                        key_permit
                            .take()
                            .expect("codex key permit should be held until response is returned"),
                        account_permit,
                    ];
                    match adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                        prepared: retry_prepared,
                        key,
                        route,
                        control_store,
                        codex_session_recovery: Arc::clone(&codex_session_recovery),
                        codex_session_rejection: Arc::clone(&codex_session_rejection),
                        affinity_config: runtime_config.affinity.clone(),
                        codex_affinity_id: codex_affinity_id.clone(),
                        permits,
                        usage_meta,
                    })
                    .await
                    {
                        CodexUpstreamOutcome::Responded(response) => return response,
                        CodexUpstreamOutcome::Failover {
                            error,
                            ctx,
                        } => return record_codex_stream_preflight_failure(error, *ctx).await,
                    }
                }
                response_prepared = retry_prepared;
                status = response.status();
                upstream_headers = response.headers().clone();
                content_type = upstream_headers
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("application/json")
                    .to_string();
                bytes = match response.bytes().await {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                            .into_response()
                    },
                };
            }
        }
        let mut classified_error =
            classify_codex_upstream_failure(status, &upstream_headers, bytes.clone());
        let mut disposition = codex_error_disposition(&classified_error);
        log_codex_error_disposition(
            &key,
            &route.account_name,
            attempt_count,
            status,
            &classified_error,
            disposition,
        );
        if let CodexErrorDisposition::RetrySameAccount {
            retry_after,
        } = disposition
        {
            if let Some(delay) = retry_after {
                tokio::time::sleep(delay).await;
            }
            let retry = add_codex_upstream_headers(
                client.request(method.clone(), upstream_url.clone()),
                &request_headers,
                &response_prepared,
                &auth,
                &runtime_config.client_version,
            );
            response = match retry.send().await {
                Ok(response) => {
                    usage_meta.mark_upstream_headers();
                    response
                },
                Err(_) => {
                    mark_codex_transient_request_failure_cooldown(
                        &codex_account_cooldowns,
                        &route.account_name,
                    );
                    usage_meta.mark_failover();
                    failed_accounts.insert(route.account_name.clone());
                    if attempt_count >= account_attempt_limit {
                        return (
                            StatusCode::BAD_GATEWAY,
                            "all eligible codex accounts failed for this request",
                        )
                            .into_response();
                    }
                    continue;
                },
            };
            if response.status().is_success() {
                remember_codex_affinity(
                    codex_session_affinity.as_ref(),
                    codex_affinity_id.as_ref(),
                    &route.account_name,
                    &runtime_config.affinity,
                );
                let permits = vec![
                    key_permit
                        .take()
                        .expect("codex key permit should be held until response is returned"),
                    account_permit,
                ];
                match adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                    prepared: response_prepared,
                    key,
                    route,
                    control_store,
                    codex_session_recovery: Arc::clone(&codex_session_recovery),
                    codex_session_rejection: Arc::clone(&codex_session_rejection),
                    affinity_config: runtime_config.affinity.clone(),
                    codex_affinity_id: codex_affinity_id.clone(),
                    permits,
                    usage_meta,
                })
                .await
                {
                    CodexUpstreamOutcome::Responded(response) => return response,
                    CodexUpstreamOutcome::Failover {
                        error,
                        ctx,
                    } => return record_codex_stream_preflight_failure(error, *ctx).await,
                }
            }
            status = response.status();
            upstream_headers = response.headers().clone();
            content_type = upstream_headers
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("application/json")
                .to_string();
            bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(_) => {
                    return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                        .into_response()
                },
            };
            classified_error =
                classify_codex_upstream_failure(status, &upstream_headers, bytes.clone());
            disposition = codex_error_disposition(&classified_error);
            log_codex_error_disposition(
                &key,
                &route.account_name,
                attempt_count,
                status,
                &classified_error,
                disposition,
            );
        }
        match disposition {
            CodexErrorDisposition::ReturnToClient {
                strict_session_block,
            } => {
                if strict_session_block {
                    maybe_remember_codex_session_rejection(
                        codex_session_rejection.as_ref(),
                        route.codex_strict_session_rejection_enabled,
                        codex_affinity_id.as_ref(),
                        &classified_error,
                        &route.account_name,
                        &runtime_config.affinity,
                    );
                }
            },
            CodexErrorDisposition::FailoverWithCooldown {
                cooldown,
            } => {
                codex_account_cooldowns.mark_account_cooldown(&route.account_name, cooldown);
            },
            CodexErrorDisposition::Failover
            | CodexErrorDisposition::RetrySameAccount {
                ..
            } => {
                mark_codex_transient_request_failure_cooldown(
                    &codex_account_cooldowns,
                    &route.account_name,
                );
            },
        }
        if !matches!(disposition, CodexErrorDisposition::ReturnToClient { .. })
            && attempt_count < account_attempt_limit
            && has_codex_failover_candidate(&routes, &failed_accounts, &route.account_name)
        {
            usage_meta.mark_failover();
            failed_accounts.insert(route.account_name.clone());
            continue;
        }
        let permits = vec![
            key_permit
                .take()
                .expect("codex key permit should be held until response is returned"),
            account_permit,
        ];
        capture_codex_dispatch_request_json(&mut usage_meta, &body, &response_prepared);
        return codex_outcome_response(
            adapt_codex_upstream_response_from_parts(
                CodexUpstreamResponseParts {
                    status,
                    upstream_headers,
                    content_type,
                    bytes,
                },
                CodexCompletedResponseContext {
                    prepared: response_prepared,
                    key,
                    route,
                    control_store,
                    codex_session_recovery: Arc::clone(&codex_session_recovery),
                    codex_session_rejection: Arc::clone(&codex_session_rejection),
                    affinity_config: runtime_config.affinity.clone(),
                    codex_affinity_id: codex_affinity_id.clone(),
                    permits,
                    usage_meta,
                },
            )
            .await,
        )
        .await;
    }
}

fn remember_codex_affinity(
    affinity: &CodexSessionAffinity,
    affinity_id: Option<&CodexAffinityId>,
    account_name: &str,
    config: &CodexAffinityRuntimeConfig,
) {
    if let Some(affinity_id) = affinity_id {
        tracing::debug!(
            account = %account_name,
            affinity_source = ?affinity_id.source,
            affinity_key = %session_preview(&affinity_id.key),
            "codex session affinity recorded"
        );
        affinity.remember(affinity_id, account_name, config);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexSessionRecoveryOutcome {
    NotNeeded,
    MissingProjection,
    Disabled,
    InvalidKey,
    Expired,
    Miss,
    Hit,
}

impl CodexSessionRecoveryOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::MissingProjection => "missing_projection",
            Self::Disabled => "disabled",
            Self::InvalidKey => "invalid_key",
            Self::Expired => "expired",
            Self::Miss => "miss",
            Self::Hit => "hit",
        }
    }
}

fn recover_codex_session_from_projection(
    recovery: &CodexSessionRecovery,
    key: &AuthenticatedKey,
    config: &CodexAffinityRuntimeConfig,
    prepared: &mut llm_access_codex::types::PreparedGatewayRequest,
) -> Result<CodexSessionRecoveryOutcome, Box<Response>> {
    let Some(source) = prepared.resolved_session_source else {
        return Ok(CodexSessionRecoveryOutcome::NotNeeded);
    };
    if !source.is_derived() {
        return Ok(CodexSessionRecoveryOutcome::NotNeeded);
    }
    let Some(projection) = prepared.session_projection.as_ref() else {
        tracing::warn!(
            key_id = %key.key_id,
            session_source = %source.as_str(),
            "codex derived session has no projection for recovery"
        );
        return Ok(CodexSessionRecoveryOutcome::MissingProjection);
    };
    let lookup_anchor_preview = hex_preview(&projection.lookup_anchor_hash);
    let bootstrap_anchor_preview = hex_preview(&projection.bootstrap_anchor_hash);
    let request_anchor_preview = hex_preview(&projection.request_anchor_hash);
    let current_session_preview = prepared
        .resolved_session_id
        .as_deref()
        .map(session_preview)
        .unwrap_or_else(|| "none".to_string());
    let recovered_session_id =
        match recovery.recover(&key.key_id, &projection.lookup_anchor_hash, config) {
            CodexSessionRecoveryLookup::Disabled(reason) => {
                tracing::debug!(
                    key_id = %key.key_id,
                    session_source = %source.as_str(),
                    lookup_anchor_hash = %lookup_anchor_preview,
                    request_anchor_hash = %request_anchor_preview,
                    bootstrap_anchor_hash = %bootstrap_anchor_preview,
                    current_session = %current_session_preview,
                    recovery_config = %reason.as_str(),
                    "codex session recovery skipped by config"
                );
                return Ok(CodexSessionRecoveryOutcome::Disabled);
            },
            CodexSessionRecoveryLookup::InvalidKey => {
                tracing::warn!(
                    key_id = %key.key_id,
                    session_source = %source.as_str(),
                    lookup_anchor_hash = %lookup_anchor_preview,
                    request_anchor_hash = %request_anchor_preview,
                    bootstrap_anchor_hash = %bootstrap_anchor_preview,
                    current_session = %current_session_preview,
                    "codex session recovery skipped because lookup key is invalid"
                );
                return Ok(CodexSessionRecoveryOutcome::InvalidKey);
            },
            CodexSessionRecoveryLookup::Expired => {
                tracing::debug!(
                    key_id = %key.key_id,
                    session_source = %source.as_str(),
                    lookup_anchor_hash = %lookup_anchor_preview,
                    request_anchor_hash = %request_anchor_preview,
                    bootstrap_anchor_hash = %bootstrap_anchor_preview,
                    current_session = %current_session_preview,
                    "codex session recovery entry expired"
                );
                return Ok(CodexSessionRecoveryOutcome::Expired);
            },
            CodexSessionRecoveryLookup::Miss => {
                tracing::debug!(
                    key_id = %key.key_id,
                    session_source = %source.as_str(),
                    lookup_anchor_hash = %lookup_anchor_preview,
                    request_anchor_hash = %request_anchor_preview,
                    bootstrap_anchor_hash = %bootstrap_anchor_preview,
                    current_session = %current_session_preview,
                    "codex session recovery missed"
                );
                return Ok(CodexSessionRecoveryOutcome::Miss);
            },
            CodexSessionRecoveryLookup::Hit(session_id) => session_id,
        };
    if prepared.resolved_session_id.as_deref() == Some(recovered_session_id.as_str()) {
        tracing::debug!(
            key_id = %key.key_id,
            session_source = %source.as_str(),
            lookup_anchor_hash = %lookup_anchor_preview,
            request_anchor_hash = %request_anchor_preview,
            bootstrap_anchor_hash = %bootstrap_anchor_preview,
            current_session = %current_session_preview,
            "codex session recovery hit current session"
        );
        return Ok(CodexSessionRecoveryOutcome::Hit);
    }
    let recovered_session_preview = session_preview(&recovered_session_id);
    apply_codex_resolved_session(
        prepared,
        recovered_session_id,
        CodexResolvedSessionSource::StablePrefix,
        Some(hex_preview(&projection.lookup_anchor_hash)),
    )
    .map_err(|err| {
        tracing::error!(
            key_id = %key.key_id,
            error = %err,
            "failed to apply recovered codex session"
        );
        Box::new(
            (StatusCode::INTERNAL_SERVER_ERROR, "failed to apply recovered codex session")
                .into_response(),
        )
    })?;
    tracing::info!(
        key_id = %key.key_id,
        session_source = %source.as_str(),
        lookup_anchor_hash = %lookup_anchor_preview,
        request_anchor_hash = %request_anchor_preview,
        bootstrap_anchor_hash = %bootstrap_anchor_preview,
        previous_session = %current_session_preview,
        recovered_session = %recovered_session_preview,
        "codex session recovered from prompt anchor"
    );
    Ok(CodexSessionRecoveryOutcome::Hit)
}

pub(super) fn remember_codex_session_recovery(
    recovery: &CodexSessionRecovery,
    key: &AuthenticatedKey,
    prepared: &llm_access_codex::types::PreparedGatewayRequest,
    completed_response: &Value,
    config: &CodexAffinityRuntimeConfig,
    account_name: &str,
) {
    let Some(projection) = prepared.session_projection.as_ref() else {
        tracing::debug!(
            key_id = %key.key_id,
            account = %account_name,
            session_source = ?prepared.resolved_session_source,
            "codex session recovery anchor not recorded because projection is missing"
        );
        return;
    };
    let Some(source) = prepared.resolved_session_source else {
        tracing::debug!(
            key_id = %key.key_id,
            account = %account_name,
            request_anchor_hash = %hex_preview(&projection.request_anchor_hash),
            "codex session recovery anchor not recorded because session source is missing"
        );
        return;
    };
    if !source.is_derived() {
        tracing::debug!(
            key_id = %key.key_id,
            account = %account_name,
            session_source = %source.as_str(),
            request_anchor_hash = %hex_preview(&projection.request_anchor_hash),
            "codex session recovery anchor not recorded for explicit session"
        );
        return;
    }
    let Some(session_id) = prepared.resolved_session_id.as_deref() else {
        tracing::debug!(
            key_id = %key.key_id,
            account = %account_name,
            session_source = %source.as_str(),
            request_anchor_hash = %hex_preview(&projection.request_anchor_hash),
            "codex session recovery anchor not recorded because session id is missing"
        );
        return;
    };
    let resume_anchor_hash = build_codex_session_resume_anchor_hash(projection, completed_response);
    let resume_anchor_preview = hex_preview(&resume_anchor_hash);
    let request_anchor_preview = hex_preview(&projection.request_anchor_hash);
    let store_result = recovery.remember(&key.key_id, &resume_anchor_hash, session_id, config);
    match store_result {
        CodexSessionRecoveryStoreResult::Stored => {
            tracing::debug!(
                key_id = %key.key_id,
                account = %account_name,
                session_source = %source.as_str(),
                request_anchor_hash = %request_anchor_preview,
                resume_anchor_hash = %resume_anchor_preview,
                session = %session_preview(session_id),
                "codex session recovery anchor recorded"
            );
        },
        CodexSessionRecoveryStoreResult::Disabled(reason) => {
            tracing::debug!(
                key_id = %key.key_id,
                account = %account_name,
                session_source = %source.as_str(),
                request_anchor_hash = %request_anchor_preview,
                resume_anchor_hash = %resume_anchor_preview,
                session = %session_preview(session_id),
                recovery_config = %reason.as_str(),
                "codex session recovery anchor not recorded because recovery is disabled"
            );
        },
        CodexSessionRecoveryStoreResult::InvalidKey => {
            tracing::warn!(
                key_id = %key.key_id,
                account = %account_name,
                session_source = %source.as_str(),
                request_anchor_hash = %request_anchor_preview,
                resume_anchor_hash = %resume_anchor_preview,
                session = %session_preview(session_id),
                "codex session recovery anchor not recorded because store key is invalid"
            );
        },
        CodexSessionRecoveryStoreResult::EmptySession => {
            tracing::warn!(
                key_id = %key.key_id,
                account = %account_name,
                session_source = %source.as_str(),
                request_anchor_hash = %request_anchor_preview,
                resume_anchor_hash = %resume_anchor_preview,
                "codex session recovery anchor not recorded because session id is empty"
            );
        },
    }
}

fn hex_preview(hex: &str) -> String {
    hex.chars().take(12).collect()
}

fn session_preview(session_id: &str) -> String {
    session_id.chars().take(32).collect()
}

fn strict_session_rejection_message(rejection: &CodexSessionRejectionEntry) -> String {
    let blocked_age_secs = rejection.blocked_at.elapsed().as_secs();
    format!(
        "This Codex session is blocked for this key after a previous {} error on account {} {}s \
         ago. start a new session to continue. Upstream message: {}",
        rejection.error_class.as_str(),
        rejection.account_name,
        blocked_age_secs,
        rejection.message
    )
}

fn codex_status_for_error_class(
    default_status: StatusCode,
    class: CodexUpstreamErrorClass,
) -> StatusCode {
    match class {
        CodexUpstreamErrorClass::ContextWindowExceeded
        | CodexUpstreamErrorClass::CyberPolicy
        | CodexUpstreamErrorClass::InvalidRequest => StatusCode::BAD_REQUEST,
        CodexUpstreamErrorClass::UsageNotIncluded => StatusCode::PAYMENT_REQUIRED,
        CodexUpstreamErrorClass::QuotaExceeded | CodexUpstreamErrorClass::Retryable => {
            StatusCode::TOO_MANY_REQUESTS
        },
        CodexUpstreamErrorClass::ServerOverloaded => StatusCode::SERVICE_UNAVAILABLE,
        CodexUpstreamErrorClass::Stream | CodexUpstreamErrorClass::UnexpectedStatus => {
            if default_status.is_success() {
                StatusCode::BAD_GATEWAY
            } else {
                default_status
            }
        },
    }
}

/// OpenAI-style `error.code` to attach when surfacing a classified upstream
/// failure to the client. Codex (and OpenAI-compatible clients) classify some
/// errors by `code` rather than message text, so emitting the canonical code
/// lets the client handle the failure cleanly (e.g. an overload becomes a tidy
/// "high load" notice via `server_is_overloaded`) instead of showing the raw
/// upstream message verbatim.
fn codex_surface_code_for_error_class(class: CodexUpstreamErrorClass) -> Option<&'static str> {
    match class {
        CodexUpstreamErrorClass::ServerOverloaded => Some("server_is_overloaded"),
        CodexUpstreamErrorClass::CyberPolicy => Some("cyber_policy"),
        _ => None,
    }
}

fn maybe_remember_codex_session_rejection(
    rejection: &super::codex_session_rejection::CodexSessionRejection,
    enabled: bool,
    affinity_id: Option<&CodexAffinityId>,
    error: &CodexClassifiedUpstreamError,
    account_name: &str,
    config: &CodexAffinityRuntimeConfig,
) {
    if !enabled {
        return;
    }
    let CodexErrorDisposition::ReturnToClient {
        strict_session_block: true,
    } = codex_error_disposition(error)
    else {
        return;
    };
    let Some(affinity_id) = affinity_id else {
        return;
    };
    tracing::warn!(
        account = %account_name,
        error_class = %error.class.as_str(),
        session = %session_preview(&affinity_id.key),
        "codex strict session rejection recorded"
    );
    rejection.remember(affinity_id, error.class, &error.message, account_name, config);
}

fn has_codex_failover_candidate(
    routes: &[ProviderCodexRoute],
    failed_accounts: &HashSet<String>,
    current_account: &str,
) -> bool {
    routes.iter().any(|candidate| {
        candidate.account_name != current_account
            && !failed_accounts.contains(&candidate.account_name)
    })
}

/// Result of adapting a 2xx upstream response. A success (or a non-recoverable
/// error that belongs on the client) is `Responded`; a recoverable failure that
/// was detected *before any byte reached the client* (an SSE preflight failure)
/// is `Failover`, handing the context back so the dispatch loop can try another
/// account exactly like the non-2xx error path does.
enum CodexUpstreamOutcome {
    Responded(Response),
    Failover { error: CodexClassifiedUpstreamError, ctx: Box<CodexStreamContext> },
}

/// What the dispatch loop should do with an adapted upstream outcome.
enum CodexLoopStep {
    Respond(Response),
    Surface { error: CodexClassifiedUpstreamError, ctx: Box<CodexStreamContext> },
    Retry { usage_meta: Box<ProviderUsageMetadata>, key_permit: LimitPermit },
}

/// Applies the same disposition-driven cooldown + failover policy used by the
/// non-2xx path to an in-stream failure surfaced via [`CodexUpstreamOutcome`].
/// Returns `Retry` (caller restores `usage_meta`/`key_permit` and continues the
/// loop) when another account can be tried, otherwise the response to return.
fn classify_codex_upstream_outcome(
    outcome: CodexUpstreamOutcome,
    routes: &[ProviderCodexRoute],
    failed_accounts: &mut HashSet<String>,
    account_attempt_limit: usize,
    attempt_count: usize,
    codex_account_cooldowns: &Arc<CodexAccountCooldowns>,
) -> CodexLoopStep {
    let (error, ctx) = match outcome {
        CodexUpstreamOutcome::Responded(response) => return CodexLoopStep::Respond(response),
        CodexUpstreamOutcome::Failover {
            error,
            ctx,
        } => (error, ctx),
    };
    let disposition = codex_error_disposition(&error);
    let account_name = ctx.route.account_name.clone();
    match disposition {
        CodexErrorDisposition::ReturnToClient {
            strict_session_block,
        } => {
            if strict_session_block {
                maybe_remember_codex_session_rejection(
                    ctx.codex_session_rejection.as_ref(),
                    ctx.route.codex_strict_session_rejection_enabled,
                    ctx.codex_affinity_id.as_ref(),
                    &error,
                    &account_name,
                    &ctx.affinity_config,
                );
            }
        },
        CodexErrorDisposition::FailoverWithCooldown {
            cooldown,
        } => {
            codex_account_cooldowns.mark_account_cooldown(&account_name, cooldown);
        },
        CodexErrorDisposition::Failover
        | CodexErrorDisposition::RetrySameAccount {
            ..
        } => {
            mark_codex_transient_request_failure_cooldown(codex_account_cooldowns, &account_name);
        },
    }
    if !matches!(disposition, CodexErrorDisposition::ReturnToClient { .. })
        && attempt_count < account_attempt_limit
        && has_codex_failover_candidate(routes, failed_accounts, &account_name)
    {
        failed_accounts.insert(account_name);
        let CodexStreamContext {
            permits,
            mut usage_meta,
            ..
        } = *ctx;
        usage_meta.mark_failover();
        let mut permits = permits.into_iter();
        let key_permit = permits
            .next()
            .expect("codex key permit should be first in the stream context permits");
        // The account permit (next item) is dropped here, releasing the slot
        // before the loop selects another account.
        return CodexLoopStep::Retry {
            usage_meta: Box::new(usage_meta),
            key_permit,
        };
    }
    CodexLoopStep::Surface {
        error,
        ctx,
    }
}

/// Collapses an outcome to a client `Response` for terminal paths that cannot
/// fail over (the loop already exhausted accounts, or is surfacing a non-2xx
/// error). A `Failover` here is treated as a surface.
async fn codex_outcome_response(outcome: CodexUpstreamOutcome) -> Response {
    match outcome {
        CodexUpstreamOutcome::Responded(response) => response,
        CodexUpstreamOutcome::Failover {
            error,
            ctx,
        } => record_codex_stream_preflight_failure(error, *ctx).await,
    }
}

fn log_codex_error_disposition(
    key: &AuthenticatedKey,
    account_name: &str,
    attempt: usize,
    status: StatusCode,
    error: &CodexClassifiedUpstreamError,
    disposition: CodexErrorDisposition,
) {
    tracing::warn!(
        key_id = %key.key_id,
        account = %account_name,
        attempt,
        status = %status.as_u16(),
        error_class = %error.class.as_str(),
        disposition = %disposition.as_str(),
        error_message = %error.message,
        "codex upstream error classified"
    );
}

async fn adapt_codex_upstream_response(
    response: reqwest::Response,
    ctx: CodexUpstreamResponseContext,
) -> CodexUpstreamOutcome {
    let CodexUpstreamResponseContext {
        prepared,
        key,
        route,
        control_store,
        codex_session_recovery,
        codex_session_rejection,
        affinity_config,
        codex_affinity_id,
        permits,
        mut usage_meta,
    } = ctx;
    let status = response.status();
    let upstream_headers = response.headers().clone();
    let content_type = upstream_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("application/json")
        .to_string();
    let has_event_stream_content_type =
        status.is_success() && content_type.contains("text/event-stream");
    let expects_stream_response =
        status.is_success() && (has_event_stream_content_type || prepared.wants_stream);

    if status.is_success()
        && !prepared.wants_stream
        && (has_event_stream_content_type || prepared.force_upstream_stream)
    {
        let bytes = match response.bytes().await {
            Ok(bytes) => bytes,
            Err(_) => {
                return CodexUpstreamOutcome::Responded(
                    (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                        .into_response(),
                )
            },
        };
        if !has_event_stream_content_type && serde_json::from_slice::<Value>(&bytes).is_ok() {
            return adapt_codex_upstream_response_from_parts(
                CodexUpstreamResponseParts {
                    status,
                    upstream_headers,
                    content_type,
                    bytes,
                },
                CodexCompletedResponseContext {
                    prepared,
                    key,
                    route,
                    control_store,
                    codex_session_recovery,
                    codex_session_rejection,
                    affinity_config,
                    codex_affinity_id,
                    permits,
                    usage_meta,
                },
            )
            .await;
        }
        usage_meta.mark_post_headers_body();
        usage_meta.mark_stream_finish();
        let completed = match completed_response_from_sse_bytes(&bytes) {
            Ok(value) => value,
            Err(err) => {
                let classified_error = err
                    .body
                    .as_deref()
                    .map(|body| {
                        classify_codex_upstream_failure(
                            err.status,
                            &upstream_headers,
                            Bytes::copy_from_slice(body.as_bytes()),
                        )
                    })
                    .unwrap_or_else(|| CodexClassifiedUpstreamError {
                        class: CodexUpstreamErrorClass::Stream,
                        status: err.status,
                        message: err.message.clone(),
                        body: Bytes::new(),
                        retry_after: None,
                    });
                let effective_status =
                    codex_status_for_error_class(err.status, classified_error.class);
                maybe_remember_codex_session_rejection(
                    codex_session_rejection.as_ref(),
                    route.codex_strict_session_rejection_enabled,
                    codex_affinity_id.as_ref(),
                    &classified_error,
                    &route.account_name,
                    &affinity_config,
                );
                tracing::error!(
                    endpoint = %prepared.original_path,
                    status = %effective_status,
                    error_class = %classified_error.class.as_str(),
                    message = %classified_error.message,
                    "codex forced-SSE upstream request failed before response.completed"
                );
                capture_codex_prepared_request_json(&mut usage_meta, &prepared);
                if classified_error.body.is_empty() {
                    capture_error_message(&mut usage_meta, &classified_error.message);
                } else {
                    capture_error_bytes(&mut usage_meta, &classified_error.body);
                    capture_error_message(&mut usage_meta, &classified_error.message);
                }
                if let Err(record_err) = record_codex_usage(
                    control_store.as_ref(),
                    &key,
                    &prepared,
                    effective_status,
                    &route,
                    missing_codex_usage(),
                    &usage_meta,
                )
                .await
                {
                    return CodexUpstreamOutcome::Responded(
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("failed to record codex usage: {record_err}"),
                        )
                            .into_response(),
                    );
                }
                return CodexUpstreamOutcome::Responded(codex_surface_error_response_with_code(
                    &prepared.original_path,
                    effective_status,
                    &classified_error.message,
                    codex_surface_code_for_error_class(classified_error.class),
                ));
            },
        };
        let completed_response = rewrite_json_value_model_alias(
            completed.response,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        );
        remember_codex_session_recovery(
            codex_session_recovery.as_ref(),
            &key,
            &prepared,
            &completed_response,
            &affinity_config,
            &route.account_name,
        );
        let adapted = adapt_completed_response_json(
            completed_response,
            prepared.response_adapter,
            Some(&prepared.tool_name_restore_map),
        );
        let body = match serde_json::to_vec(&adapted) {
            Ok(body) => body,
            Err(_) => {
                return CodexUpstreamOutcome::Responded(
                    (StatusCode::BAD_GATEWAY, "codex upstream response adaptation failed")
                        .into_response(),
                )
            },
        };
        if let Err(err) = record_codex_usage(
            control_store.as_ref(),
            &key,
            &prepared,
            status,
            &route,
            completed.usage.unwrap_or_else(missing_codex_usage),
            &usage_meta,
        )
        .await
        {
            return CodexUpstreamOutcome::Responded(
                (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record codex usage: {err}"))
                    .into_response(),
            );
        }
        let builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        return CodexUpstreamOutcome::Responded(
            apply_upstream_response_headers(builder, &upstream_headers)
                .body(Body::from(body))
                .unwrap_or_else(|_| {
                    (StatusCode::BAD_GATEWAY, "codex upstream response build failed")
                        .into_response()
                }),
        );
    }

    if expects_stream_response {
        let prepared = strip_codex_stream_request_bodies(prepared);
        return stream_codex_upstream_response(
            response,
            status,
            upstream_headers,
            content_type,
            CodexStreamContext {
                prepared,
                key,
                route,
                control_store,
                codex_session_recovery,
                codex_session_rejection,
                affinity_config,
                codex_affinity_id,
                permits,
                usage_meta,
            },
        )
        .await;
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return CodexUpstreamOutcome::Responded(
                (StatusCode::BAD_GATEWAY, "codex upstream response read failed").into_response(),
            )
        },
    };
    adapt_codex_upstream_response_from_parts(
        CodexUpstreamResponseParts {
            status,
            upstream_headers,
            content_type,
            bytes,
        },
        CodexCompletedResponseContext {
            prepared,
            key,
            route,
            control_store,
            codex_session_recovery,
            codex_session_rejection,
            affinity_config,
            codex_affinity_id,
            permits,
            usage_meta,
        },
    )
    .await
}
async fn adapt_codex_upstream_response_from_parts(
    parts: CodexUpstreamResponseParts,
    ctx: CodexCompletedResponseContext,
) -> CodexUpstreamOutcome {
    let CodexUpstreamResponseParts {
        status,
        upstream_headers,
        content_type,
        bytes,
    } = parts;
    let CodexCompletedResponseContext {
        prepared,
        key,
        route,
        control_store,
        codex_session_recovery,
        codex_session_rejection,
        affinity_config,
        codex_affinity_id,
        permits: _permits,
        mut usage_meta,
    } = ctx;
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    let effective_success_bytes = &bytes;
    let success_error = status
        .is_success()
        .then(|| classify_codex_success_error_body(status, &upstream_headers, &bytes))
        .flatten();
    let effective_status = success_error
        .as_ref()
        .map(|error| codex_status_for_error_class(error.status, error.class))
        .unwrap_or(status);
    let usage = if status.is_success() && success_error.is_none() {
        extract_usage_from_bytes(effective_success_bytes).unwrap_or_else(missing_codex_usage)
    } else {
        capture_error_bytes(&mut usage_meta, &bytes);
        missing_codex_usage()
    };
    if let Some(error) = success_error.as_ref() {
        maybe_remember_codex_session_rejection(
            codex_session_rejection.as_ref(),
            route.codex_strict_session_rejection_enabled,
            codex_affinity_id.as_ref(),
            error,
            &route.account_name,
            &affinity_config,
        );
        capture_error_message(&mut usage_meta, &error.message);
    } else if status.is_success() {
        if let Ok(completed_response) = serde_json::from_slice::<Value>(effective_success_bytes) {
            remember_codex_session_recovery(
                codex_session_recovery.as_ref(),
                &key,
                &prepared,
                &completed_response,
                &affinity_config,
                &route.account_name,
            );
        }
    }
    if let Err(err) = record_codex_usage(
        control_store.as_ref(),
        &key,
        &prepared,
        effective_status,
        &route,
        usage,
        &usage_meta,
    )
    .await
    {
        return CodexUpstreamOutcome::Responded(
            (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record codex usage: {err}"))
                .into_response(),
        );
    }
    if let Some(error) = success_error.as_ref() {
        return CodexUpstreamOutcome::Responded(codex_surface_error_response_with_code(
            &prepared.original_path,
            effective_status,
            &error.message,
            codex_surface_code_for_error_class(error.class),
        ));
    }
    if !status.is_success()
        && prepared.response_adapter == GatewayResponseAdapter::AnthropicMessages
    {
        let message = summarize_error_bytes(&bytes);
        let body = json!({
            "error": {
                "type": codex_error_type_for_status(status),
                "message": message,
            }
        });
        let builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        return CodexUpstreamOutcome::Responded(
            apply_upstream_response_headers(builder, &upstream_headers)
                .body(Body::from(body.to_string()))
                .unwrap_or_else(|_| {
                    (StatusCode::BAD_GATEWAY, "codex upstream response build failed")
                        .into_response()
                }),
        );
    }
    let response_content_type =
        if status.is_success() && prepared.response_adapter != GatewayResponseAdapter::Responses {
            "application/json"
        } else {
            &content_type
        };
    let response_body = if status.is_success() {
        match prepared.response_adapter {
            GatewayResponseAdapter::Responses => {
                if let Some(body) = rewrite_json_response_model_alias(
                    effective_success_bytes,
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Body::from(body)
                } else {
                    Body::from(bytes)
                }
            },
            GatewayResponseAdapter::ChatCompletions => {
                match convert_json_response_to_chat_completion(
                    &bytes,
                    Some(&prepared.tool_name_restore_map),
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Ok(body) => Body::from(body),
                    Err(err) => {
                        return CodexUpstreamOutcome::Responded(
                            (StatusCode::BAD_GATEWAY, err).into_response(),
                        )
                    },
                }
            },
            GatewayResponseAdapter::AnthropicMessages => {
                match convert_json_response_to_anthropic_message(
                    &bytes,
                    Some(&prepared.tool_name_restore_map),
                    prepared.model.as_deref(),
                    prepared.client_visible_model.as_deref(),
                ) {
                    Ok(body) => Body::from(body),
                    Err(err) => {
                        return CodexUpstreamOutcome::Responded(
                            (StatusCode::BAD_GATEWAY, err).into_response(),
                        )
                    },
                }
            },
        }
    } else if let Some(body) = rewrite_json_response_model_alias(
        &bytes,
        prepared.model.as_deref(),
        prepared.client_visible_model.as_deref(),
    ) {
        Body::from(body)
    } else {
        Body::from(bytes)
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, response_content_type)
        .header(header::CACHE_CONTROL, "no-store");
    CodexUpstreamOutcome::Responded(
        apply_upstream_response_headers(builder, &upstream_headers)
            .body(response_body)
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
            }),
    )
}
fn codex_message_indicates_usage_limit(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("usage limit")
        || normalized.contains("insufficient_quota")
        || normalized.contains("quota_exceeded")
        || normalized.contains("quota exceeded")
}
fn randomized_codex_transient_account_failure_cooldown<R: Rng + ?Sized>(rng: &mut R) -> Duration {
    let min_ms = CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MIN
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let max_ms = CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MAX
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    Duration::from_millis(rng.gen_range(min_ms..=max_ms))
}
fn mark_codex_transient_request_failure_cooldown(
    codex_account_cooldowns: &Arc<CodexAccountCooldowns>,
    account_name: &str,
) {
    let cooldown = randomized_codex_transient_account_failure_cooldown(&mut rand::thread_rng());
    codex_account_cooldowns.mark_account_cooldown(account_name, cooldown);
}
pub fn codex_status_from_error_json_value(value: &Value) -> Option<StatusCode> {
    for pointer in ["/error/status", "/status", "/response/error/status"] {
        if let Some(status) = value.pointer(pointer).and_then(Value::as_u64) {
            if let Ok(status) = u16::try_from(status) {
                if let Ok(status) = StatusCode::from_u16(status) {
                    return Some(status);
                }
            }
        }
    }

    for pointer in ["/error/code", "/code", "/response/error/code"] {
        match value.pointer(pointer).and_then(Value::as_str) {
            Some("invalid_api_key") => return Some(StatusCode::UNAUTHORIZED),
            Some("insufficient_quota" | "quota_exceeded" | "rate_limit_exceeded") => {
                return Some(StatusCode::TOO_MANY_REQUESTS)
            },
            Some("bad_gateway") => return Some(StatusCode::BAD_GATEWAY),
            _ => {},
        }
    }

    for pointer in ["/error/type", "/type", "/response/error/type"] {
        match value.pointer(pointer).and_then(Value::as_str) {
            Some("invalid_request_error") => return Some(StatusCode::BAD_REQUEST),
            Some("authentication_error") => return Some(StatusCode::UNAUTHORIZED),
            Some("permission_error") => return Some(StatusCode::FORBIDDEN),
            Some("not_found_error") => return Some(StatusCode::NOT_FOUND),
            Some("rate_limit_error") => return Some(StatusCode::TOO_MANY_REQUESTS),
            Some("api_error") => return Some(StatusCode::BAD_GATEWAY),
            _ => {},
        }
    }

    if extract_error_message_from_json_value(value)
        .as_deref()
        .is_some_and(codex_message_indicates_usage_limit)
    {
        return Some(StatusCode::TOO_MANY_REQUESTS);
    }

    None
}
async fn stream_codex_upstream_response(
    response: reqwest::Response,
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    content_type: String,
    ctx: CodexStreamContext,
) -> CodexUpstreamOutcome {
    let response_adapter = ctx.prepared.response_adapter;
    let mut events = response
        .bytes_stream()
        .map_err(std::io::Error::other)
        .eventsource();
    let first_event = match events.next().await {
        Some(Ok(event)) => {
            if let Some(error) = classify_codex_sse_event_failure(
                status,
                &upstream_headers,
                Some(event.event.as_str()),
                &event.data,
            ) {
                return CodexUpstreamOutcome::Failover {
                    error,
                    ctx: Box::new(ctx),
                };
            }
            Some(event)
        },
        Some(Err(err)) => {
            let error = CodexClassifiedUpstreamError {
                class: CodexUpstreamErrorClass::Stream,
                status: StatusCode::BAD_GATEWAY,
                message: format!("failed to parse codex upstream SSE event: {err}"),
                body: Bytes::new(),
                retry_after: None,
            };
            return CodexUpstreamOutcome::Failover {
                error,
                ctx: Box::new(ctx),
            };
        },
        None => None,
    };
    let failure_headers = upstream_headers.clone();
    let body_stream = stream! {
        let CodexStreamContext {
            prepared,
            key,
            route,
            control_store,
            codex_session_recovery,
            codex_session_rejection,
            affinity_config,
            codex_affinity_id,
            permits,
            usage_meta,
        } = ctx;
        let _permits = permits;
        let mut first_event = first_event;
        let mut chat_metadata = ChatStreamMetadata::default();
        let mut anthropic_metadata = AnthropicStreamMetadata::default();
        let mut guard = CodexStreamRecordGuard {
            prepared,
            key,
            route,
            control_store,
            codex_session_recovery,
            affinity_config,
            status,
            usage_meta,
            usage_collector: SseUsageCollector::default(),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        loop {
            let event = if let Some(event) = first_event.take() {
                Some(Ok(event))
            } else {
                events.next().await
            };
            let Some(event) = event else {
                break;
            };
            match event {
                Ok(event) => {
                    if let Some(error) = classify_codex_sse_event_failure(
                        status,
                        &failure_headers,
                        Some(event.event.as_str()),
                        &event.data,
                    ) {
                        let effective_status =
                            codex_status_for_error_class(error.status, error.class);
                        guard.mark_upstream_failure(
                            effective_status,
                            &error.message,
                            &error.body,
                        );
                        maybe_remember_codex_session_rejection(
                            codex_session_rejection.as_ref(),
                            guard.route.codex_strict_session_rejection_enabled,
                            codex_affinity_id.as_ref(),
                            &error,
                            &guard.route.account_name,
                            &guard.affinity_config,
                        );
                        tracing::warn!(
                            key_id = %guard.key.key_id,
                            account = %guard.route.account_name,
                            status = %effective_status.as_u16(),
                            error_class = %error.class.as_str(),
                            "codex stream upstream failure detected after downstream write started"
                        );
                        // Intentionally stop the stream here without emitting `[DONE]`
                        // and without forwarding the raw upstream failure event: the
                        // partial content already reached the client and the failure is
                        // recorded server-side via the guard above. A well-behaved
                        // client detects the missing terminal sentinel. (See
                        // codex_dispatch_streaming_mid_failure_stops_without_done_*.)
                        return;
                    }
                    guard.usage_collector.observe_event(&event);
                    match response_adapter {
                        GatewayResponseAdapter::Responses => {
                            let bytes = encode_sse_event_with_model_alias(
                                &event,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            );
                            guard.observe_chunk(&bytes, Some(event.event.as_str()));
                            yield Ok::<Bytes, std::io::Error>(bytes);
                        },
                        GatewayResponseAdapter::ChatCompletions => {
                            if let Some(chunk) = convert_response_event_to_chat_chunk(
                                &event,
                                Some(&guard.prepared.tool_name_restore_map),
                                &mut chat_metadata,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            ) {
                                let bytes = encode_json_sse_chunk(&chunk);
                                guard.observe_chunk(&bytes, Some(event.event.as_str()));
                                yield Ok::<Bytes, std::io::Error>(bytes);
                            }
                        },
                        GatewayResponseAdapter::AnthropicMessages => {
                            for bytes in convert_response_event_to_anthropic_sse_chunks(
                                &event,
                                Some(&guard.prepared.tool_name_restore_map),
                                &mut anthropic_metadata,
                                guard.prepared.model.as_deref(),
                                guard.prepared.client_visible_model.as_deref(),
                            ) {
                                guard.observe_chunk(&bytes, Some(event.event.as_str()));
                                yield Ok::<Bytes, std::io::Error>(bytes);
                            }
                        },
                    }
                },
                Err(err) => {
                    guard.mark_internal_failure();
                    yield Err(std::io::Error::other(format!(
                        "failed to parse codex upstream SSE event: {err}"
                    )));
                    return;
                },
            }
        }
        if response_adapter == GatewayResponseAdapter::ChatCompletions {
            let bytes = Bytes::from_static(b"data: [DONE]\n\n");
            guard.observe_chunk(&bytes, Some("done"));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        guard.finish_success().await;
    };
    let response_content_type = if response_adapter != GatewayResponseAdapter::Responses {
        "text/event-stream"
    } else {
        content_type.as_str()
    };
    let builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, response_content_type)
        .header(header::CACHE_CONTROL, "no-store");
    CodexUpstreamOutcome::Responded(
        apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from_stream(body_stream))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream stream response build failed")
                    .into_response()
            }),
    )
}

async fn record_codex_stream_preflight_failure(
    error: CodexClassifiedUpstreamError,
    ctx: CodexStreamContext,
) -> Response {
    let CodexStreamContext {
        prepared,
        key,
        route,
        control_store,
        codex_session_recovery: _codex_session_recovery,
        codex_session_rejection,
        affinity_config,
        codex_affinity_id,
        permits,
        mut usage_meta,
    } = ctx;
    let _permits = permits;
    let effective_status = codex_status_for_error_class(error.status, error.class);
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    capture_codex_prepared_request_json(&mut usage_meta, &prepared);
    if error.body.is_empty() {
        capture_error_message(&mut usage_meta, &error.message);
        capture_error_body(
            &mut usage_meta,
            &codex_surface_error_body_with_code(
                &prepared.original_path,
                effective_status,
                &error.message,
                codex_surface_code_for_error_class(error.class),
            ),
        );
    } else {
        capture_error_bytes(&mut usage_meta, &error.body);
        capture_error_message(&mut usage_meta, &error.message);
    }
    maybe_remember_codex_session_rejection(
        codex_session_rejection.as_ref(),
        route.codex_strict_session_rejection_enabled,
        codex_affinity_id.as_ref(),
        &error,
        &route.account_name,
        &affinity_config,
    );
    tracing::warn!(
        key_id = %key.key_id,
        account = %route.account_name,
        status = %effective_status.as_u16(),
        error_class = %error.class.as_str(),
        "codex stream preflight failure returned before downstream write"
    );
    if let Err(err) = record_codex_usage(
        control_store.as_ref(),
        &key,
        &prepared,
        effective_status,
        &route,
        missing_codex_usage(),
        &usage_meta,
    )
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record codex usage: {err}"))
            .into_response();
    }
    codex_surface_error_response_with_code(
        &prepared.original_path,
        effective_status,
        &error.message,
        codex_surface_code_for_error_class(error.class),
    )
}
