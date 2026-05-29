//! Kiro account lifecycle: list/import/manual-create/patch/delete, status sync,
//! balance, cache stats, model probing, and cache-policy parsing/validation.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_admin_kiro_accounts(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(query);
    match state
        .admin_kiro_account_store
        .list_admin_kiro_accounts_page(page_request)
        .await
    {
        Ok(page) => Json(AdminKiroAccountsResponse {
            accounts: page.accounts,
            summary: page.summary,
            total: page.total,
            limit: page.limit,
            offset: page.offset,
            has_more: page.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list Kiro gateway accounts").into_response(),
    }
}
pub(crate) async fn list_admin_kiro_account_statuses(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ListKiroAccountStatusesRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = AdminPageRequest {
        limit: query.limit.unwrap_or(24).clamp(1, 200),
        offset: query.offset.unwrap_or(0),
    };
    let prefix = query
        .prefix
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let page = match state
        .admin_kiro_account_store
        .list_admin_kiro_accounts_filtered_page(prefix, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list Kiro gateway accounts").into_response(),
    };
    Json(AdminKiroAccountStatusesResponse {
        accounts: page.accounts,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) async fn get_admin_kiro_cache_stats(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => {
            return internal_error("Failed to load llm gateway runtime config").into_response()
        },
    };
    Json(AdminKiroCacheStatsResponse {
        stats: state
            .provider_state
            .kiro_cache_stats(kiro_cache_simulation_config_from_admin_config(&config)),
        process_memory: read_current_process_memory_stats(),
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) fn kiro_cache_simulation_config_from_admin_config(
    config: &AdminRuntimeConfig,
) -> KiroCacheSimulationConfig {
    KiroCacheSimulationConfig {
        mode: KiroCacheSimulationMode::from_runtime_value(&config.kiro_prefix_cache_mode),
        prefix_cache_max_tokens: config.kiro_prefix_cache_max_tokens,
        prefix_cache_entry_ttl: Duration::from_secs(config.kiro_prefix_cache_entry_ttl_seconds),
        conversation_anchor_max_entries: usize::try_from(
            config.kiro_conversation_anchor_max_entries,
        )
        .unwrap_or(usize::MAX),
        conversation_anchor_ttl: Duration::from_secs(config.kiro_conversation_anchor_ttl_seconds),
    }
}
pub(crate) async fn import_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportLocalKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_kiro_channel_limit_inputs(
        request.kiro_channel_max_concurrency,
        request.kiro_channel_min_start_interval_ms,
    ) {
        return response.into_response();
    }
    let sqlite_path = request
        .sqlite_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(local_import::default_sqlite_path);
    let mut auth =
        match local_import::import_from_sqlite(&sqlite_path, request.name.as_deref()).await {
            Ok(auth) => auth,
            Err(_) => return internal_error("Failed to import local Kiro auth").into_response(),
        };
    if let Some(value) = request.kiro_channel_max_concurrency {
        auth.kiro_channel_max_concurrency = Some(value);
    }
    if let Some(value) = request.kiro_channel_min_start_interval_ms {
        auth.kiro_channel_min_start_interval_ms = Some(value);
    }
    create_or_replace_kiro_account(state, auth.canonicalize()).await
}
pub(crate) async fn create_admin_kiro_manual_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateManualKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let auth = match kiro_auth_from_manual_request(request) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    create_or_replace_kiro_account(state, auth).await
}
pub(crate) async fn patch_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchKiroAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let patch = match normalize_kiro_account_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    let sync_status_cache = patch.status.is_some();
    if let Some(Some(proxy_id)) = patch.proxy_config_id.as_ref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
            Err(_) => {
                return internal_error("Failed to load llm gateway proxy config").into_response()
            },
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return bad_request("proxy config must be active before account binding")
                .into_response();
        }
    }
    match state
        .admin_kiro_account_store
        .patch_admin_kiro_account(&name, patch)
        .await
    {
        Ok(Some(account)) => {
            if sync_status_cache {
                sync_kiro_status_after_account_update(&state, &account).await;
            }
            Json(account).into_response()
        },
        Ok(None) => not_found("Kiro account not found").into_response(),
        Err(_) => internal_error("Failed to update Kiro account").into_response(),
    }
}
pub(crate) async fn delete_admin_kiro_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_kiro_account_store
        .delete_admin_kiro_account(&name)
        .await
    {
        Ok(Some(_account)) => Json(serde_json::json!({"status": "ok"})).into_response(),
        Ok(None) => not_found("Kiro account not found").into_response(),
        Err(_) => internal_error("Failed to delete Kiro account").into_response(),
    }
}
pub(crate) async fn get_admin_kiro_account_balance(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_kiro_account_store
        .get_admin_kiro_balance(&name)
        .await
    {
        Ok(Some(balance)) => Json(balance).into_response(),
        Ok(None) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Kiro balance cache is not ready yet".to_string(),
                code: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
            }),
        )
            .into_response(),
        Err(_) => internal_error("Failed to load Kiro account balance").into_response(),
    }
}
pub(crate) async fn refresh_admin_kiro_account_balance(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("Kiro account not found").into_response(),
        Err(_) => return internal_error("Failed to load Kiro account").into_response(),
    };
    let now = now_ms();
    let route_store = state.provider_state.route_store();
    match kiro_refresh::fetch_usage_limits_for_route(&route, route_store.as_ref(), true).await {
        Ok(usage) => {
            let balance = admin_kiro_balance_from_usage(&usage);
            let cache = core_store::AdminKiroCacheView {
                status: "ready".to_string(),
                last_checked_at: Some(now),
                last_success_at: Some(now),
                error_message: None,
                ..core_store::AdminKiroCacheView::default()
            };
            if let Err(err) = state
                .admin_kiro_account_store
                .save_admin_kiro_status_cache(core_store::AdminKiroStatusCacheUpdate {
                    account_name: name.clone(),
                    balance: Some(balance.clone()),
                    refreshed_at_ms: now,
                    expires_at_ms: now
                        + (cache.refresh_interval_seconds.min(i64::MAX as u64 / 1000) as i64
                            * 1000),
                    cache,
                    last_error: None,
                })
                .await
            {
                tracing::warn!(account_name = %name, "failed to persist kiro balance cache: {err:#}");
            }
            Json(balance).into_response()
        },
        Err(err) => {
            let cache = core_store::AdminKiroCacheView {
                status: "error".to_string(),
                last_checked_at: Some(now),
                error_message: Some(err.to_string()),
                ..core_store::AdminKiroCacheView::default()
            };
            let _ = state
                .admin_kiro_account_store
                .save_admin_kiro_status_cache(core_store::AdminKiroStatusCacheUpdate {
                    account_name: name,
                    balance: None,
                    refreshed_at_ms: now,
                    expires_at_ms: now + 60_000,
                    cache,
                    last_error: Some(err.to_string()),
                })
                .await;
            (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh Kiro account balance: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response()
        },
    }
}
pub(crate) async fn probe_admin_kiro_account_model(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<ProbeKiroAccountModelRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let request = match normalize_probe_kiro_account_model_request(request) {
        Ok(request) => request,
        Err(response) => return response.into_response(),
    };
    let mut route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("Kiro account not found").into_response(),
        Err(_) => return internal_error("Failed to load Kiro account").into_response(),
    };
    let (proxy, proxy_source) = match resolve_admin_kiro_probe_proxy(&state, &route, &request).await
    {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    route.proxy = proxy.clone();
    let upstream_request =
        build_direct_kiro_model_probe_request(&request.model, route.profile_arn.clone());
    let request_body = match serde_json::to_vec(&upstream_request) {
        Ok(body) => body,
        Err(_) => {
            return internal_error("Failed to encode Kiro model probe request").into_response();
        },
    };
    let upstream_url = format!(
        "{}/generateAssistantResponse",
        kiro_refresh::runtime_upstream_base_url(&route.api_region)
    );
    let started_at = Instant::now();
    match provider::call_kiro_generate_for_route(
        &route,
        state.provider_state.route_store().as_ref(),
        upstream_url,
        &request_body,
    )
    .await
    {
        Ok(response) => {
            let upstream_status_code = response.status().as_u16();
            let bytes = match response.bytes().await {
                Ok(bytes) => bytes,
                Err(err) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(AdminKiroModelProbeResponse {
                            ok: false,
                            account_name: name,
                            model: request.model,
                            api_region: route.api_region,
                            proxy_source: proxy_source.as_str().to_string(),
                            proxy_url: proxy.map(|value| value.proxy_url),
                            upstream_status_code,
                            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128)
                                as i64,
                            checked_at: now_ms(),
                            message: format!("failed to read Kiro model probe response: {err}"),
                        }),
                    )
                        .into_response();
                },
            };
            let events = match provider::decode_kiro_events_from_bytes(&bytes) {
                Ok(events) => events,
                Err(err) => {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(AdminKiroModelProbeResponse {
                            ok: false,
                            account_name: name,
                            model: request.model,
                            api_region: route.api_region,
                            proxy_source: proxy_source.as_str().to_string(),
                            proxy_url: proxy.map(|value| value.proxy_url),
                            upstream_status_code,
                            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128)
                                as i64,
                            checked_at: now_ms(),
                            message: err,
                        }),
                    )
                        .into_response();
                },
            };
            if let Some(message) = kiro_probe_eventstream_error_message(&events) {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(AdminKiroModelProbeResponse {
                        ok: false,
                        account_name: name,
                        model: request.model,
                        api_region: route.api_region,
                        proxy_source: proxy_source.as_str().to_string(),
                        proxy_url: proxy.map(|value| value.proxy_url),
                        upstream_status_code,
                        latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                        checked_at: now_ms(),
                        message,
                    }),
                )
                    .into_response();
            }
            Json(AdminKiroModelProbeResponse {
                ok: true,
                account_name: name,
                model: request.model,
                api_region: route.api_region,
                proxy_source: proxy_source.as_str().to_string(),
                proxy_url: proxy.map(|value| value.proxy_url),
                upstream_status_code,
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                checked_at: now_ms(),
                message: "Kiro model probe succeeded".to_string(),
            })
            .into_response()
        },
        Err(err) => (
            err.status(),
            Json(AdminKiroModelProbeResponse {
                ok: false,
                account_name: name,
                model: request.model,
                api_region: route.api_region,
                proxy_source: proxy_source.as_str().to_string(),
                proxy_url: proxy.map(|value| value.proxy_url),
                upstream_status_code: err.status().as_u16(),
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                checked_at: now_ms(),
                message: summarize_upstream_error_body(&err.body_text()),
            }),
        )
            .into_response(),
    }
}
pub(crate) fn normalize_probe_kiro_account_model_request(
    request: ProbeKiroAccountModelRequest,
) -> Result<NormalizedProbeKiroAccountModelRequest, AdminHttpError> {
    let model = normalize_optional_string(&request.model)
        .ok_or_else(|| bad_request("model is required"))?;
    let proxy_url = normalize_optional_string_option(request.proxy_url.as_deref());
    let proxy_username = normalize_optional_string_option(request.proxy_username.as_deref());
    let proxy_password = normalize_optional_string_option(request.proxy_password.as_deref());
    if proxy_url.is_none() && (proxy_username.is_some() || proxy_password.is_some()) {
        return Err(bad_request(
            "proxy_url is required when inline proxy credentials are provided",
        ));
    }
    Ok(NormalizedProbeKiroAccountModelRequest {
        model,
        proxy_config_id: normalize_optional_string_option(request.proxy_config_id.as_deref()),
        inline_proxy: proxy_url.map(|proxy_url| core_store::ProviderProxyConfig {
            proxy_url,
            proxy_username,
            proxy_password,
        }),
    })
}
pub(crate) fn build_direct_kiro_model_probe_request(
    model: &str,
    profile_arn: Option<String>,
) -> llm_access_kiro::wire::KiroRequest {
    let conversation_id = format!("admin-model-probe-{}", uuid::Uuid::new_v4().simple());
    let current_message = llm_access_kiro::wire::CurrentMessage::new(
        llm_access_kiro::wire::UserInputMessage::new(ADMIN_KIRO_MODEL_PROBE_PROMPT, model),
    );
    llm_access_kiro::wire::KiroRequest {
        conversation_state: llm_access_kiro::wire::ConversationState::new(conversation_id)
            .with_chat_trigger_type("MANUAL")
            .with_current_message(current_message),
        profile_arn,
    }
}
pub(crate) fn kiro_probe_eventstream_error_message(
    events: &[llm_access_kiro::wire::Event],
) -> Option<String> {
    for event in events {
        match event {
            llm_access_kiro::wire::Event::Error {
                error_code,
                error_message,
            } => {
                return Some(format!(
                    "Kiro model probe stream error {error_code}: {}",
                    summarize_upstream_error_body(error_message)
                ));
            },
            llm_access_kiro::wire::Event::Exception {
                exception_type,
                message,
            } => {
                return Some(format!(
                    "Kiro model probe stream exception {exception_type}: {}",
                    summarize_upstream_error_body(message)
                ));
            },
            _ => {},
        }
    }
    None
}
pub(crate) fn select_kiro_candidate_account_names(
    key: &core_store::AdminKey,
    groups_by_id: &BTreeMap<&str, &core_store::AdminAccountGroup>,
    all_account_names: &[String],
) -> Vec<String> {
    let route_strategy = key.route_strategy.as_deref().unwrap_or("auto");
    let group_account_names = key
        .account_group_id
        .as_deref()
        .and_then(|group_id| groups_by_id.get(group_id))
        .map(|group| group.account_names.clone());
    match route_strategy {
        "fixed" => {
            if let Some(group_account_names) = group_account_names {
                group_account_names
            } else {
                key.fixed_account_name
                    .as_ref()
                    .filter(|value| !value.trim().is_empty())
                    .map(|value| vec![value.clone()])
                    .unwrap_or_default()
            }
        },
        "auto" => {
            if let Some(group_account_names) = group_account_names {
                group_account_names
            } else if let Some(auto_account_names) = key
                .auto_account_names
                .as_ref()
                .filter(|names| !names.is_empty())
            {
                auto_account_names.clone()
            } else {
                all_account_names.to_vec()
            }
        },
        _ => Vec::new(),
    }
}
pub(crate) fn kiro_auth_from_manual_request(
    request: CreateManualKiroAccountRequest,
) -> Result<KiroAuthRecord, AdminHttpError> {
    let name = normalize_account_name(&request.name)?;
    validate_kiro_channel_limit_inputs(
        request.kiro_channel_max_concurrency,
        request.kiro_channel_min_start_interval_ms,
    )?;
    if let Some(value) = request.minimum_remaining_credits_before_block {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
        }
    }
    Ok(KiroAuthRecord {
        name,
        access_token: normalize_optional_string_option(request.access_token.as_deref()),
        refresh_token: normalize_optional_string_option(request.refresh_token.as_deref()),
        profile_arn: normalize_optional_string_option(request.profile_arn.as_deref()),
        expires_at: normalize_optional_string_option(request.expires_at.as_deref()),
        auth_method: normalize_optional_string_option(request.auth_method.as_deref()),
        client_id: normalize_optional_string_option(request.client_id.as_deref()),
        client_secret: normalize_optional_string_option(request.client_secret.as_deref()),
        region: normalize_optional_string_option(request.region.as_deref()),
        auth_region: normalize_optional_string_option(request.auth_region.as_deref()),
        api_region: normalize_optional_string_option(request.api_region.as_deref()),
        machine_id: normalize_optional_string_option(request.machine_id.as_deref()),
        provider: normalize_optional_string_option(request.provider.as_deref()),
        email: normalize_optional_string_option(request.email.as_deref()),
        subscription_title: normalize_optional_string_option(request.subscription_title.as_deref()),
        kiro_channel_max_concurrency: request.kiro_channel_max_concurrency,
        kiro_channel_min_start_interval_ms: request.kiro_channel_min_start_interval_ms,
        minimum_remaining_credits_before_block: request.minimum_remaining_credits_before_block,
        disabled: request.disabled,
        disabled_reason: None,
        source: Some("manual".to_string()),
        last_imported_at: Some(now_ms()),
        ..KiroAuthRecord::default()
    }
    .canonicalize())
}
pub(crate) fn new_admin_kiro_account_from_auth(
    auth: KiroAuthRecord,
    created_at_ms: i64,
) -> Result<NewAdminKiroAccount, AdminHttpError> {
    let name = auth.name.clone();
    let auth_method = auth.auth_method().to_string();
    let profile_arn = auth.profile_arn.clone();
    let max_concurrency = auth.effective_kiro_channel_max_concurrency();
    let min_start_interval_ms = auth.effective_kiro_channel_min_start_interval_ms();
    let proxy_config_id = auth.proxy_selection().proxy_config_id;
    let status = if auth.disabled { KEY_STATUS_DISABLED } else { KEY_STATUS_ACTIVE }.to_string();
    let auth_json = serde_json::to_string(&auth)
        .map_err(|_| internal_error("Failed to encode Kiro account auth"))?;
    Ok(NewAdminKiroAccount {
        name,
        auth_method,
        account_id: None,
        profile_arn,
        user_id: None,
        status,
        auth_json,
        max_concurrency: Some(max_concurrency),
        min_start_interval_ms: Some(min_start_interval_ms),
        proxy_config_id,
        created_at_ms,
    })
}
pub(crate) async fn create_or_replace_kiro_account(
    state: HttpState,
    auth: KiroAuthRecord,
) -> Response {
    let account = match new_admin_kiro_account_from_auth(auth, now_ms()) {
        Ok(account) => account,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_kiro_account_store
        .create_admin_kiro_account(account)
        .await
    {
        Ok(account) => {
            sync_kiro_status_after_account_update(&state, &account).await;
            Json(account).into_response()
        },
        Err(_) => internal_error("Failed to save Kiro account").into_response(),
    }
}
pub(crate) async fn sync_kiro_status_after_account_update(
    state: &HttpState,
    account: &core_store::AdminKiroAccount,
) {
    if account.disabled {
        let now = now_ms();
        let refresh_interval_seconds = account.cache.refresh_interval_seconds;
        let update = core_store::AdminKiroStatusCacheUpdate {
            account_name: account.name.clone(),
            balance: None,
            refreshed_at_ms: now,
            expires_at_ms: now
                .saturating_add((refresh_interval_seconds as i64).saturating_mul(1000)),
            cache: core_store::AdminKiroCacheView {
                status: KEY_STATUS_DISABLED.to_string(),
                refresh_interval_seconds,
                last_checked_at: Some(now),
                last_success_at: account.cache.last_success_at,
                error_message: None,
            },
            last_error: None,
        };
        if let Err(err) = state
            .admin_kiro_account_store
            .save_admin_kiro_status_cache(update)
            .await
        {
            tracing::warn!(
                account_name = %account.name,
                "failed to persist disabled Kiro status after account update: {err:#}"
            );
        }
        return;
    }

    let route = match state
        .admin_kiro_account_store
        .resolve_admin_kiro_account_route(&account.name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return,
        Err(err) => {
            tracing::warn!(
                account_name = %account.name,
                "failed to resolve Kiro route after account update: {err:#}"
            );
            return;
        },
    };
    let route_store = state.provider_state.route_store();
    if let Err(err) =
        kiro_status::refresh_and_persist_route_status(&route, route_store.as_ref(), false).await
    {
        tracing::warn!(
            account_name = %account.name,
            "failed to refresh cached Kiro status after account update: {err:#}"
        );
    }
}
pub(crate) fn normalize_kiro_account_patch(
    request: PatchKiroAccountRequest,
) -> Result<core_store::AdminKiroAccountPatch, AdminHttpError> {
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    validate_kiro_channel_limit_inputs(
        request.kiro_channel_max_concurrency,
        request.kiro_channel_min_start_interval_ms,
    )?;
    if let Some(value) = request.minimum_remaining_credits_before_block {
        if !value.is_finite() || value < 0.0 {
            return Err(bad_request("minimum_remaining_credits_before_block must be >= 0"));
        }
    }
    let proxy_mode = request
        .proxy_mode
        .as_deref()
        .map(normalize_proxy_mode)
        .transpose()?;
    let proxy_config_id = request
        .proxy_config_id
        .as_deref()
        .map(|value| normalize_optional_string_option(Some(value)));
    if matches!(proxy_mode.as_deref(), Some("fixed"))
        && proxy_config_id
            .as_ref()
            .and_then(|value| value.as_ref())
            .is_none()
    {
        return Err(bad_request("fixed proxy_mode requires proxy_config_id"));
    }
    Ok(core_store::AdminKiroAccountPatch {
        status,
        max_concurrency: request.kiro_channel_max_concurrency,
        min_start_interval_ms: request.kiro_channel_min_start_interval_ms,
        minimum_remaining_credits_before_block: request.minimum_remaining_credits_before_block,
        proxy_mode,
        proxy_config_id,
        updated_at_ms: now_ms(),
    })
}
pub(crate) fn admin_kiro_balance_from_usage(
    usage: &llm_access_kiro::wire::UsageLimitsResponse,
) -> core_store::AdminKiroBalanceView {
    let usage_limit = usage.usage_limit();
    let current_usage = usage.current_usage();
    core_store::AdminKiroBalanceView {
        current_usage,
        usage_limit,
        remaining: (usage_limit - current_usage).max(0.0),
        next_reset_at: usage
            .usage_breakdown_list
            .first()
            .and_then(|item| item.next_date_reset.or(usage.next_date_reset))
            .map(|value| value as i64),
        subscription_title: usage.subscription_title().map(ToString::to_string),
        user_id: usage.user_id().map(ToString::to_string),
    }
}
pub(crate) fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
    let map: BTreeMap<String, f64> = serde_json::from_str(value)?;
    anyhow::ensure!(!map.is_empty(), "kmodel map must not be empty");
    for (model, coeff) in &map {
        anyhow::ensure!(!model.trim().is_empty(), "kmodel entry has empty model name");
        anyhow::ensure!(
            coeff.is_finite() && *coeff > 0.0,
            "kmodel entry `{model}` must be a positive finite number"
        );
    }
    Ok(map)
}
pub(crate) fn parse_kiro_billable_model_multipliers_json(
    value: &str,
) -> anyhow::Result<BTreeMap<String, f64>> {
    let overrides: BTreeMap<String, f64> = serde_json::from_str(value)?;
    let mut merged = BTreeMap::from([
        ("haiku".to_string(), 1.0),
        ("opus".to_string(), 1.0),
        ("sonnet".to_string(), 1.0),
    ]);
    for (family, multiplier) in overrides {
        anyhow::ensure!(
            matches!(family.as_str(), "opus" | "sonnet" | "haiku"),
            "billable multiplier family `{family}` must be one of `opus`, `sonnet`, `haiku`"
        );
        anyhow::ensure!(
            multiplier.is_finite() && multiplier > 0.0,
            "billable multiplier `{family}` must be a positive finite number"
        );
        merged.insert(family, multiplier);
    }
    Ok(merged)
}
pub(crate) fn parse_kiro_cache_policy_json(value: &str) -> anyhow::Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}
pub(crate) fn validate_kiro_cache_policy(policy: &KiroCachePolicy) -> anyhow::Result<()> {
    let boost = &policy.small_input_high_credit_boost;
    anyhow::ensure!(
        boost.target_input_tokens > 0,
        "small_input_high_credit_boost.target_input_tokens must be positive"
    );
    anyhow::ensure!(
        boost.credit_start.is_finite()
            && boost.credit_end.is_finite()
            && boost.credit_start < boost.credit_end,
        "small_input_high_credit_boost credit range is invalid"
    );
    anyhow::ensure!(
        policy.high_credit_diagnostic_threshold.is_finite()
            && policy.high_credit_diagnostic_threshold >= 0.0,
        "high_credit_diagnostic_threshold must be finite and >= 0"
    );
    anyhow::ensure!(
        policy.anthropic_cache_creation_input_ratio.is_finite()
            && (0.0..=1.0).contains(&policy.anthropic_cache_creation_input_ratio),
        "anthropic_cache_creation_input_ratio must be finite and between 0 and 1"
    );
    anyhow::ensure!(
        !policy.prefix_tree_credit_ratio_bands.is_empty(),
        "prefix_tree_credit_ratio_bands must contain at least one band"
    );

    let mut previous_credit_end = None;
    let mut previous_ratio_end = None;
    for (index, band) in policy.prefix_tree_credit_ratio_bands.iter().enumerate() {
        anyhow::ensure!(
            band.credit_start.is_finite() && band.credit_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] credit bounds must be finite"
        );
        anyhow::ensure!(
            band.credit_start < band.credit_end,
            "prefix_tree_credit_ratio_bands[{index}] credit_start must be < credit_end"
        );
        anyhow::ensure!(
            band.cache_ratio_start.is_finite() && band.cache_ratio_end.is_finite(),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be finite"
        );
        anyhow::ensure!(
            (0.0..=1.0).contains(&band.cache_ratio_start)
                && (0.0..=1.0).contains(&band.cache_ratio_end),
            "prefix_tree_credit_ratio_bands[{index}] cache ratios must be between 0 and 1"
        );
        anyhow::ensure!(
            band.cache_ratio_start >= band.cache_ratio_end,
            "prefix_tree_credit_ratio_bands[{index}] cache ratio must not increase within the band"
        );
        if let Some(prev_end) = previous_credit_end {
            anyhow::ensure!(
                band.credit_start >= prev_end - BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] overlaps previous band"
            );
            anyhow::ensure!(
                band.credit_start <= prev_end + BAND_CONTIGUITY_TOLERANCE,
                "prefix_tree_credit_ratio_bands[{index}] has a gap after previous band"
            );
        }
        if let Some(prev_ratio) = previous_ratio_end {
            anyhow::ensure!(
                band.cache_ratio_start <= prev_ratio,
                "prefix_tree_credit_ratio_bands[{index}] cache ratio increases between bands"
            );
        }
        previous_credit_end = Some(band.credit_end);
        previous_ratio_end = Some(band.cache_ratio_end);
    }
    Ok(())
}
