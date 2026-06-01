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
        align_responses_store_with_upstream, apply_codex_fast_policy,
        apply_gpt53_codex_spark_mapping,
        extract_last_message_content as extract_codex_last_message_content,
        prepare_gateway_request_from_bytes,
    },
    response::{
        adapt_completed_response_json, apply_upstream_response_headers,
        convert_json_response_to_chat_completion, convert_response_event_to_chat_chunk,
        encode_json_sse_chunk, encode_sse_event_with_model_alias, extract_usage_from_bytes,
        rewrite_json_response_model_alias, rewrite_json_value_model_alias, SseUsageCollector,
    },
    types::{ChatStreamMetadata, GatewayResponseAdapter},
};
use llm_access_core::store::AuthenticatedKey;
use rand::Rng;
use serde_json::{json, Value};

use super::{
    client::provider_client,
    codex_auth::{
        add_codex_upstream_headers, codex_upstream_base_url, compute_codex_upstream_url,
        is_codex_invalid_encrypted_content_response, is_codex_non_retryable_client_error_response,
        load_codex_dispatch_runtime_config, normalized_codex_gateway_path,
        retry_codex_without_encrypted_reasoning,
    },
    codex_models::codex_openai_models_response,
    codex_sse::{
        completed_response_from_sse_bytes, missing_codex_usage, record_codex_preflight_failure,
        record_codex_usage,
    },
    errors::{
        codex_error_type_for_status, codex_surface_error_body, codex_surface_error_response,
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
    CodexAccountCooldowns, CodexAuthSnapshot, CodexCompletedResponseContext,
    CodexPreflightFailureRecord, CodexStreamContext, CodexStreamRecordGuard,
    CodexUpstreamResponseContext, CodexUpstreamResponseParts, ProviderDispatchDeps,
    ProviderUsageMetadata, StreamRecordState, CODEX_QUOTA_EXHAUSTION_COOLDOWN,
    CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MAX, CODEX_TRANSIENT_ACCOUNT_FAILURE_COOLDOWN_MIN,
    MAX_PROVIDER_PROXY_BODY_BYTES,
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
    let prepared = match prepare_gateway_request_from_bytes(
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
    usage_meta.last_message_content = prepared.last_message_content.clone();
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
                    permits,
                    usage_meta,
                },
            )
            .await;
        }
        if response.status().is_success() {
            let permits = vec![
                key_permit
                    .take()
                    .expect("codex key permit should be held until response is returned"),
                account_permit,
            ];
            return adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                prepared,
                key,
                route,
                control_store,
                permits,
                usage_meta,
            })
            .await;
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
                    let permits = vec![
                        key_permit
                            .take()
                            .expect("codex key permit should be held until response is returned"),
                        account_permit,
                    ];
                    return adapt_codex_upstream_response(response, CodexUpstreamResponseContext {
                        prepared: retry_prepared,
                        key,
                        route,
                        control_store,
                        permits,
                        usage_meta,
                    })
                    .await;
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
        if let Some(cooldown) = codex_temporary_request_failure_cooldown(status, &bytes) {
            codex_account_cooldowns.mark_account_cooldown(&route.account_name, cooldown);
        }
        if !is_codex_invalid_encrypted_content_response(status, &bytes)
            && !is_codex_non_retryable_client_error_response(status, &bytes)
            && attempt_count < account_attempt_limit
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
        capture_codex_dispatch_request_json(&mut usage_meta, &body, &response_prepared);
        return adapt_codex_upstream_response_from_parts(
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
                permits,
                usage_meta,
            },
        )
        .await;
    }
}
async fn adapt_codex_upstream_response(
    response: reqwest::Response,
    ctx: CodexUpstreamResponseContext,
) -> Response {
    let CodexUpstreamResponseContext {
        prepared,
        key,
        route,
        control_store,
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
                return (StatusCode::BAD_GATEWAY, "codex upstream response read failed")
                    .into_response()
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
                tracing::error!(
                    endpoint = %prepared.original_path,
                    status = %err.status,
                    message = %err.message,
                    "codex forced-SSE upstream request failed before response.completed"
                );
                capture_codex_prepared_request_json(&mut usage_meta, &prepared);
                capture_error_message(&mut usage_meta, &err.message);
                if let Some(body) = err.body.as_deref() {
                    capture_error_body(&mut usage_meta, body);
                }
                if let Err(record_err) = record_codex_usage(
                    control_store.as_ref(),
                    &key,
                    &prepared,
                    err.status,
                    &route,
                    missing_codex_usage(),
                    &usage_meta,
                )
                .await
                {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("failed to record codex usage: {record_err}"),
                    )
                        .into_response();
                }
                return codex_surface_error_response(
                    &prepared.original_path,
                    err.status,
                    &err.message,
                );
            },
        };
        let completed_response = rewrite_json_value_model_alias(
            completed.response,
            prepared.model.as_deref(),
            prepared.client_visible_model.as_deref(),
        );
        let adapted = adapt_completed_response_json(
            completed_response,
            prepared.response_adapter,
            Some(&prepared.tool_name_restore_map),
        );
        let body = match serde_json::to_vec(&adapted) {
            Ok(body) => body,
            Err(_) => {
                return (StatusCode::BAD_GATEWAY, "codex upstream response adaptation failed")
                    .into_response()
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
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("failed to record codex usage: {err}"),
            )
                .into_response();
        }
        let builder = Response::builder()
            .status(status)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CACHE_CONTROL, "no-store");
        return apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from(body))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
            });
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
                permits,
                usage_meta,
            },
        );
    }

    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::BAD_GATEWAY, "codex upstream response read failed").into_response()
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
            permits,
            usage_meta,
        },
    )
    .await
}
async fn adapt_codex_upstream_response_from_parts(
    parts: CodexUpstreamResponseParts,
    ctx: CodexCompletedResponseContext,
) -> Response {
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
        permits: _permits,
        mut usage_meta,
    } = ctx;
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    let effective_success_bytes = &bytes;
    let usage = if status.is_success() {
        extract_usage_from_bytes(effective_success_bytes).unwrap_or_else(missing_codex_usage)
    } else {
        capture_error_bytes(&mut usage_meta, &bytes);
        missing_codex_usage()
    };
    if let Err(err) = record_codex_usage(
        control_store.as_ref(),
        &key,
        &prepared,
        status,
        &route,
        usage,
        &usage_meta,
    )
    .await
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, format!("failed to record codex usage: {err}"))
            .into_response();
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
        return apply_upstream_response_headers(builder, &upstream_headers)
            .body(Body::from(body.to_string()))
            .unwrap_or_else(|_| {
                (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
            });
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
                    Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
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
                    Err(err) => return (StatusCode::BAD_GATEWAY, err).into_response(),
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
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(response_body)
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "codex upstream response build failed").into_response()
        })
}
fn codex_quota_exhaustion_cooldown(status: StatusCode, bytes: &Bytes) -> Option<Duration> {
    if !matches!(
        status,
        StatusCode::TOO_MANY_REQUESTS | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN
    ) {
        return None;
    }
    let body = String::from_utf8_lossy(bytes.as_ref());
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes.as_ref()) {
        for pointer in ["/error/code", "/code", "/response/error/code"] {
            if value.pointer(pointer).and_then(serde_json::Value::as_str)
                == Some("insufficient_quota")
            {
                return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
            }
        }
        for pointer in ["/error/message", "/message", "/response/error/message"] {
            if value
                .pointer(pointer)
                .and_then(serde_json::Value::as_str)
                .is_some_and(codex_message_indicates_usage_limit)
            {
                return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
            }
        }
    }
    if codex_message_indicates_usage_limit(&body) {
        return Some(CODEX_QUOTA_EXHAUSTION_COOLDOWN);
    }
    None
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
fn codex_temporary_request_failure_cooldown(status: StatusCode, bytes: &Bytes) -> Option<Duration> {
    // Request-shape failures must stay on the existing same-account retry path.
    // Cooling the account for those errors would poison healthy accounts for a
    // client-side bug that is independent of the selected route.
    if is_codex_invalid_encrypted_content_response(status, bytes) {
        return None;
    }

    // Explicit upstream quota signals still deserve the stronger existing
    // cooldown window because they are not a transient transport blip.
    if let Some(cooldown) = codex_quota_exhaustion_cooldown(status, bytes) {
        return Some(cooldown);
    }

    // Everything else here is a request-path account failure signal: a
    // transport/proxy/upstream problem happened after we already selected an
    // account. We do not write this into persisted account status. We only
    // keep the account out of the selection pool for a short randomized window
    // so subsequent requests stop paying the same failover tax immediately.
    if status.is_server_error()
        || matches!(
            status,
            StatusCode::UNAUTHORIZED
                | StatusCode::FORBIDDEN
                | StatusCode::PAYMENT_REQUIRED
                | StatusCode::TOO_MANY_REQUESTS
                | StatusCode::REQUEST_TIMEOUT
        )
    {
        return Some(randomized_codex_transient_account_failure_cooldown(&mut rand::thread_rng()));
    }

    None
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
fn stream_codex_upstream_response(
    response: reqwest::Response,
    status: StatusCode,
    upstream_headers: reqwest::header::HeaderMap,
    content_type: String,
    ctx: CodexStreamContext,
) -> Response {
    let response_adapter = ctx.prepared.response_adapter;
    let body_stream = stream! {
        let CodexStreamContext {
            prepared,
            key,
            route,
            control_store,
            permits,
            usage_meta,
        } = ctx;
        let _permits = permits;
        let mut events = response
            .bytes_stream()
            .map_err(std::io::Error::other)
            .eventsource();
        let mut chat_metadata = ChatStreamMetadata::default();
        let mut anthropic_metadata = AnthropicStreamMetadata::default();
        let mut guard = CodexStreamRecordGuard {
            prepared,
            key,
            route,
            control_store,
            status,
            usage_meta,
            usage_collector: SseUsageCollector::default(),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        while let Some(event) = events.next().await {
            match event {
                Ok(event) => {
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
    apply_upstream_response_headers(builder, &upstream_headers)
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            (StatusCode::BAD_GATEWAY, "codex upstream stream response build failed").into_response()
        })
}
