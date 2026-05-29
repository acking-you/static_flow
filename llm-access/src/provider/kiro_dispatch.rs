//! Kiro upstream proxy dispatch: generate/MCP calls, websearch dispatch, stream
//! peeking, and stream/non-stream response building.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn hydrate_kiro_route_for_dispatch(
    route: ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
) -> Result<ProviderKiroRoute, Response> {
    if !route.auth_json.is_empty() {
        return Ok(route);
    }
    let account_name = route.account_name.clone();
    let loaded = route_store
        .resolve_kiro_account_route(&account_name)
        .await
        .map_err(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "kiro route resolution failed",
            )
        })?;
    let Some(loaded) = loaded else {
        return Err(kiro_json_error(
            StatusCode::BAD_GATEWAY,
            "api_error",
            "all eligible kiro accounts failed for this request",
        ));
    };
    let mut route = route;
    route.auth_json = loaded.auth_json;
    if route.profile_arn.is_none() {
        route.profile_arn = loaded.profile_arn;
    }
    if route.api_region.trim().is_empty() {
        route.api_region = loaded.api_region;
    }
    route.account_request_max_concurrency = loaded.account_request_max_concurrency;
    route.account_request_min_start_interval_ms = loaded.account_request_min_start_interval_ms;
    route.proxy = loaded.proxy;
    Ok(route)
}
pub(crate) async fn dispatch_kiro_proxy(
    key: AuthenticatedKey,
    request: Request<Body>,
    deps: ProviderDispatchDeps,
) -> Response {
    let ProviderDispatchDeps {
        route_store,
        control_store,
        geoip,
        kiro_cache_simulator,
        request_limiter,
        kiro_request_scheduler,
        kiro_latency_ranker,
        ..
    } = deps;
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
    if let Err(err) = apply_kiro_model_mapping(&routes[0].model_name_map_json, &mut payload) {
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            &format!("Kiro model mapping configuration is invalid: {err}"),
        );
    }
    let effective_model = payload.model.clone();
    let route_mcp_web_search = websearch::should_route_mcp_web_search(&payload);
    if !route_mcp_web_search {
        websearch::remove_web_search_tools(&mut payload);
    }
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
    if route_mcp_web_search {
        let request_input_tokens = token::count_all_tokens(
            &payload.model,
            payload.system.as_deref(),
            &payload.messages,
            payload.tools.as_deref(),
        ) as i32;
        override_kiro_thinking_from_model_name(&mut payload);
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
            kiro_latency_ranker,
            request_input_tokens,
            usage_meta,
        })
        .await;
    }
    let request_input_tokens = token::count_all_tokens(
        &payload.model,
        payload.system.as_deref(),
        &payload.messages,
        payload.tools.as_deref(),
    ) as i32;
    override_kiro_thinking_from_model_name(&mut payload);
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
    let resolved_session =
        resolve_kiro_request_session(&request_headers, payload.metadata.as_ref());
    let conversion = match convert_normalized_request_with_resolved_session(
        normalized,
        routes[0].request_validation_enabled,
        resolved_session,
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
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
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
        let response = match call_kiro_generate_for_route(
            &route,
            route_store.as_ref(),
            upstream_url.clone(),
            &request_body,
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
                let status = failure.status;
                capture_client_request_body_json(&mut usage_meta, &body);
                capture_upstream_request_body_json(&mut usage_meta, &request_body);
                capture_error_bytes(&mut usage_meta, &failure.body);
                usage_meta.mark_stream_finish();
                let error_response = failure.into_response();
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
            let status = response.status();
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
            return kiro_upstream_error_response(status, &content_type, bytes);
        }
        if payload.stream {
            let stream_response = match prepare_kiro_stream_response_for_route(
                response,
                &route,
                route_store.as_ref(),
                &upstream_url,
                &request_body,
                &effective_model,
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
            let response_ctx = KiroResponseContext {
                key,
                route,
                public_path: public_path.to_string(),
                model: effective_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                tool_name_map: conversion.tool_name_map.clone(),
                structured_output_tool_name: conversion.structured_output_tool_name.clone(),
                response_identity: conversion.response_identity.clone(),
                cache_ctx,
                control_store,
                kiro_cache_simulator,
                usage_meta,
                _key_permit: key_permit
                    .take()
                    .expect("kiro key permit should be held until response is returned"),
                _account_permit: account_permit,
            };
            return stream_kiro_upstream_response(stream_response, response_ctx);
        }
        let response_ctx = KiroResponseContext {
            key,
            route,
            public_path: public_path.to_string(),
            model: effective_model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            tool_name_map: conversion.tool_name_map.clone(),
            structured_output_tool_name: conversion.structured_output_tool_name.clone(),
            response_identity: conversion.response_identity.clone(),
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            _key_permit: key_permit
                .take()
                .expect("kiro key permit should be held until response is returned"),
            _account_permit: account_permit,
        };
        return non_stream_kiro_response(response, response_ctx).await;
    }
}
pub(crate) async fn dispatch_kiro_websearch(input: KiroWebsearchDispatch) -> Response {
    let KiroWebsearchDispatch {
        key,
        payload,
        routes,
        control_store,
        route_store,
        request_limiter,
        kiro_request_scheduler,
        kiro_latency_ranker,
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
    loop {
        let route_started = Instant::now();
        let (route, account_permit) = match select_kiro_route_with_account_permit(
            &kiro_request_scheduler,
            &routes,
            &failed_accounts,
            kiro_latency_ranker.as_ref(),
        )
        .await
        {
            Ok(value) => value,
            Err(response) => return response,
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
        match call_kiro_mcp_for_route(&route, route_store.as_ref(), &request_body).await {
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
pub(crate) async fn build_kiro_websearch_response(input: WebsearchResponseInput) -> Response {
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
pub(crate) async fn peek_kiro_stream(
    response: reqwest::Response,
) -> Result<KiroPeekedStream, KiroStreamPeekError> {
    let status = response.status();
    let mut body_stream = response.bytes_stream();
    while let Some(chunk_result) = body_stream.next().await {
        match chunk_result {
            Ok(chunk) if !chunk.is_empty() => {
                return Ok(KiroPeekedStream {
                    status,
                    first_chunk: chunk,
                    remaining: body_stream.boxed(),
                })
            },
            Ok(_) => continue,
            Err(err) => return Err(KiroStreamPeekError::Read(err)),
        }
    }
    Err(KiroStreamPeekError::Empty)
}
pub(crate) async fn prepare_kiro_stream_response_for_route(
    initial_response: reqwest::Response,
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: &str,
    request_body: &[u8],
    model: &str,
) -> Result<KiroPeekedStream, KiroRouteFailure> {
    let mut response = initial_response;
    for retry in 0..=KIRO_EMPTY_STREAM_MAX_RETRIES {
        match peek_kiro_stream(response).await {
            Ok(stream) => {
                if retry > 0 {
                    tracing::info!(
                        model = %model,
                        attempt = retry + 1,
                        "Kiro empty stream retry succeeded"
                    );
                }
                return Ok(stream);
            },
            Err(KiroStreamPeekError::Empty) if retry < KIRO_EMPTY_STREAM_MAX_RETRIES => {
                tracing::warn!(
                    model = %model,
                    attempt = retry + 1,
                    "Kiro returned an empty generateAssistantResponse stream; retrying"
                );
                tokio::time::sleep(Duration::from_millis(200 * (retry as u64 + 1))).await;
                response = call_kiro_generate_for_route(
                    route,
                    route_store,
                    upstream_url.to_string(),
                    request_body,
                )
                .await?;
            },
            Err(KiroStreamPeekError::Empty) => {
                tracing::error!(
                    model = %model,
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
            Err(KiroStreamPeekError::Read(err)) => {
                tracing::error!(
                    model = %model,
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
pub(crate) async fn call_kiro_generate_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    upstream_url: String,
    request_body: &[u8],
) -> Result<reqwest::Response, KiroRouteFailure> {
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    for attempt in 0..3 {
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
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            },
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        if status.as_u16() == 402 && is_monthly_request_limit(&body) {
            return Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
        }
        if status.as_u16() == 429 {
            if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                return Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: false,
                }));
            }
        }
        if status.as_u16() == 400 {
            if let Some(cooldown) = transient_invalid_model_cooldown(&body) {
                return Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: true,
                }));
            }
            return Err(failure.with_kind(KiroRouteFailureKind::Fatal));
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) && !force_refresh {
            force_refresh = true;
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            continue;
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
        }
        if matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS)
            || status.is_server_error()
        {
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            if attempt < 2 {
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            return Err(last_failure.expect("retryable kiro failure should be captured"));
        }
        return Err(failure.with_kind(KiroRouteFailureKind::Fatal));
    }
    Err(last_failure.unwrap_or_else(|| {
        KiroRouteFailure::synthetic(
            StatusCode::BAD_GATEWAY,
            "kiro upstream request failed".to_string(),
            KiroRouteFailureKind::RetryNext,
        )
    }))
}
pub(crate) async fn call_kiro_mcp_for_route(
    route: &ProviderKiroRoute,
    route_store: &dyn ProviderRouteStore,
    request_body: &str,
) -> Result<McpResponse, KiroRouteFailure> {
    let upstream_url =
        format!("{}/mcp", kiro_refresh::runtime_upstream_base_url(&route.api_region));
    let mut force_refresh = false;
    let mut last_failure: Option<KiroRouteFailure> = None;
    let mut attempt = 0usize;
    let response = loop {
        attempt += 1;
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
                if attempt < 3 {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    continue;
                }
                break Err(last_failure.expect("mcp transport failure should be captured"));
            },
        };
        if response.status().is_success() {
            break Ok(response);
        }
        let status = response.status();
        let failure = KiroRouteFailure::from_response(response, KiroRouteFailureKind::Fatal).await;
        let body = failure.body_text();
        if status.as_u16() == 402 && is_monthly_request_limit(&body) {
            break Err(failure.with_kind(KiroRouteFailureKind::QuotaExhausted));
        }
        if status.as_u16() == 429 {
            if let Some(cooldown) = daily_request_limit_cooldown(&body) {
                break Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: false,
                }));
            }
        }
        if status.as_u16() == 400 {
            if let Some(cooldown) = transient_invalid_model_cooldown(&body) {
                break Err(failure.with_kind(KiroRouteFailureKind::RateLimited {
                    cooldown,
                    mark_proxy: true,
                }));
            }
            break Err(failure.with_kind(KiroRouteFailureKind::Fatal));
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) && !force_refresh {
            force_refresh = true;
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            continue;
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            break Err(failure.with_kind(KiroRouteFailureKind::RetryNext));
        }
        if matches!(status, StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS)
            || status.is_server_error()
        {
            last_failure = Some(failure.with_kind(KiroRouteFailureKind::RetryNext));
            if attempt < 3 {
                tokio::time::sleep(Duration::from_millis(350)).await;
                continue;
            }
            break Err(last_failure.expect("retryable mcp failure should be captured"));
        }
        break Err(failure.with_kind(KiroRouteFailureKind::Fatal));
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
pub(crate) async fn send_kiro_generate_request(
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
pub(crate) async fn send_kiro_mcp_request(
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
pub(crate) fn kiro_request_body_with_profile_arn(
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
pub(crate) fn stream_kiro_upstream_response(
    response: KiroPeekedStream,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status;
    let body_stream = stream! {
        let KiroResponseContext {
            key,
            route,
            public_path,
            model,
            request_input_tokens,
            thinking_enabled,
            hidden_thinking_enabled,
            tool_name_map,
            structured_output_tool_name,
            response_identity,
            cache_ctx,
            control_store,
            kiro_cache_simulator,
            usage_meta,
            _key_permit,
            _account_permit,
        } = ctx;
        let stream_model = model.clone();
        let context_usage_min_request_tokens = route.context_usage_min_request_tokens;
        let mut guard = KiroStreamRecordGuard {
            control_store,
            key,
            route,
            endpoint: public_path,
            model,
            status,
            cache_ctx,
            usage_meta,
            stream_ctx: StreamContext::new_with_thinking_visibility(
                &stream_model,
                request_input_tokens,
                thinking_enabled,
                hidden_thinking_enabled,
                tool_name_map,
                structured_output_tool_name,
            )
            .with_context_usage_min_request_tokens(context_usage_min_request_tokens)
            .with_response_identity(response_identity),
            state: StreamRecordState::Pending,
            record_committed: false,
        };
        for event in guard.stream_ctx.generate_initial_events() {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        let mut body_stream = futures_util::stream::once(async move { Ok(response.first_chunk) })
            .chain(response.remaining)
            .boxed();
        let mut decoder = EventStreamDecoder::new();
        while let Some(chunk_result) = body_stream.next().await {
            let chunk = match chunk_result {
                Ok(chunk) => chunk,
                Err(err) => {
                    guard.mark_internal_failure();
                    yield Err(std::io::Error::other(format!("failed to read kiro upstream stream: {err}")));
                    return;
                },
            };
            let _ = decoder.feed(&chunk);
            for frame in decoder.decode_iter() {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to decode kiro event frame: {err}")));
                        return;
                    },
                };
                let event = match Event::from_frame(frame) {
                    Ok(event) => event,
                    Err(err) => {
                        guard.mark_internal_failure();
                        yield Err(std::io::Error::other(format!("failed to parse kiro event: {err}")));
                        return;
                    },
                };
                for sse_event in guard.stream_ctx.process_kiro_event(&event) {
                    let bytes = Bytes::from(sse_event.to_sse_string());
                    guard.observe_chunk(&bytes, Some(sse_event.event.as_str()));
                    yield Ok::<Bytes, std::io::Error>(bytes);
                }
            }
        }
        guard.usage_meta.mark_post_headers_body();
        let (_resolved_input_tokens, output_tokens) = guard.stream_ctx.final_usage();
        let (credit_usage, credit_usage_missing) = guard.stream_ctx.final_credit_usage();
        let usage = build_kiro_usage_summary(
            &guard.model,
            KiroUsageInputs {
                request_input_tokens,
                context_input_tokens: guard.stream_ctx.context_input_tokens(),
                context_usage_min_request_tokens: guard.route.context_usage_min_request_tokens,
                output_tokens,
                credit_usage,
                credit_usage_missing,
                cache_estimation_enabled: guard.route.cache_estimation_enabled,
            },
            &guard.cache_ctx,
        );
        let mut final_events = guard.stream_ctx.generate_final_events();
        let anthropic_usage = anthropic_usage_json_from_summary_with_policy(usage, &guard.cache_ctx);
        for event in &mut final_events {
            if event.event == "message_delta" {
                if let Some(value) = event.data.get_mut("usage") {
                    *value = anthropic_usage.clone();
                }
            }
        }
        let assistant_message = guard.stream_ctx.final_assistant_message();
        kiro_cache_simulator.record_success_from_runtime_projection(
            &guard.cache_ctx.projection,
            &assistant_message,
            &guard.cache_ctx.conversation_id,
            guard.route.cache_estimation_enabled,
            guard.cache_ctx.simulation_config,
            Instant::now(),
        );
        for event in final_events {
            let bytes = Bytes::from(event.to_sse_string());
            guard.observe_chunk(&bytes, Some(event.event.as_str()));
            yield Ok::<Bytes, std::io::Error>(bytes);
        }
        guard.finish_success(usage).await;
    };
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .header(header::CONNECTION, "keep-alive")
        .body(Body::from_stream(body_stream))
        .unwrap_or_else(|_| {
            kiro_json_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "api_error",
                "failed to build stream response",
            )
        })
}
pub(crate) async fn non_stream_kiro_response(
    response: reqwest::Response,
    ctx: KiroResponseContext,
) -> Response {
    let status = response.status();
    let mut usage_meta = ctx.usage_meta.clone();
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(_) => {
            return kiro_json_error(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "failed to read kiro upstream response",
            )
        },
    };
    usage_meta.mark_post_headers_body();
    usage_meta.mark_stream_finish();
    let events = match decode_kiro_events_from_bytes(&bytes) {
        Ok(events) => events,
        Err(err) => return kiro_json_error(StatusCode::BAD_GATEWAY, "api_error", &err),
    };
    let mut stream_ctx = StreamContext::new_with_thinking_visibility(
        &ctx.model,
        ctx.request_input_tokens,
        ctx.thinking_enabled,
        ctx.hidden_thinking_enabled,
        ctx.tool_name_map,
        ctx.structured_output_tool_name.clone(),
    )
    .with_context_usage_min_request_tokens(ctx.route.context_usage_min_request_tokens)
    .with_response_identity(ctx.response_identity.clone());
    for event in &events {
        let _ = stream_ctx.process_kiro_event(event);
    }
    let _ = stream_ctx.generate_final_events();
    let (_resolved_input_tokens, output_tokens) = stream_ctx.final_usage();
    let (credit_usage, credit_usage_missing) = stream_ctx.final_credit_usage();
    let usage = build_kiro_usage_summary(
        &ctx.model,
        KiroUsageInputs {
            request_input_tokens: ctx.request_input_tokens,
            context_input_tokens: stream_ctx.context_input_tokens(),
            context_usage_min_request_tokens: ctx.route.context_usage_min_request_tokens,
            output_tokens,
            credit_usage,
            credit_usage_missing,
            cache_estimation_enabled: ctx.route.cache_estimation_enabled,
        },
        &ctx.cache_ctx,
    );
    let assistant_message = stream_ctx.final_assistant_message();
    let mut content = stream_ctx.final_content_blocks();
    if let Some(tool_uses) = assistant_message.tool_uses.clone() {
        content.extend(tool_uses.into_iter().map(|tool_use| {
            serde_json::json!({
                "type": "tool_use",
                "id": tool_use.tool_use_id,
                "name": tool_use.name,
                "input": tool_use.input,
            })
        }));
    }
    let stop_reason = stream_ctx.state_manager.get_stop_reason();
    ctx.kiro_cache_simulator
        .record_success_from_runtime_projection(
            &ctx.cache_ctx.projection,
            &assistant_message,
            &ctx.cache_ctx.conversation_id,
            ctx.route.cache_estimation_enabled,
            ctx.cache_ctx.simulation_config,
            Instant::now(),
        );
    if let Err(err) = record_kiro_usage(KiroUsageRecord {
        control_store: ctx.control_store.as_ref(),
        key: &ctx.key,
        route: &ctx.route,
        endpoint: &ctx.public_path,
        model: &ctx.model,
        status,
        usage,
        cache_ctx: &ctx.cache_ctx,
        meta: &usage_meta,
    })
    .await
    {
        tracing::error!(error = %err, "Failed to record gateway usage for non-stream response");
        return kiro_json_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "api_error",
            "failed to record usage",
        );
    }
    let body = serde_json::json!({
        "id": format!("msg_{}", uuid::Uuid::new_v4().simple()),
        "type": "message",
        "role": "assistant",
        "content": content,
        "model": ctx.model,
        "stop_reason": stop_reason,
        "stop_sequence": null,
        "usage": anthropic_usage_json_from_summary_with_policy(usage, &ctx.cache_ctx),
    });
    Response::builder()
        .status(status)
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
