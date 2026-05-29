//! Codex account lifecycle: list/patch/delete/refresh, model probing, batch +
//! single import jobs, and Codex auth normalization/validation.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_accounts(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminCodexAccountListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(AdminListQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let status = match state.public_status_store.codex_rate_limit_status().await {
        Ok(status) => Some(status),
        Err(_) => {
            return internal_error("Failed to load llm gateway account status").into_response();
        },
    };
    let filter = admin_codex_account_page_query(&query);
    let page = match state
        .admin_codex_account_store
        .list_admin_codex_accounts_filtered_page(&filter, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list llm gateway accounts").into_response(),
    };
    Json(AdminAccountsResponse {
        accounts: apply_cached_codex_status_to_admin_accounts(page.accounts, status),
        summary: page.summary,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) fn admin_codex_account_page_query(
    query: &AdminCodexAccountListQuery,
) -> core_store::AdminCodexAccountPageQuery {
    core_store::AdminCodexAccountPageQuery {
        search: query.q.clone(),
        active_only: query.active_only.unwrap_or(false),
        unhealthy_only: query.unhealthy_only.unwrap_or(false),
        sort: match query.sort.as_deref() {
            Some("primary_asc") => core_store::AdminCodexAccountSortMode::PrimaryAsc,
            Some("primary_desc") => core_store::AdminCodexAccountSortMode::PrimaryDesc,
            Some("secondary_asc") => core_store::AdminCodexAccountSortMode::SecondaryAsc,
            Some("secondary_desc") => core_store::AdminCodexAccountSortMode::SecondaryDesc,
            _ => core_store::AdminCodexAccountSortMode::Newest,
        },
    }
}
pub(crate) fn apply_cached_codex_status_to_admin_accounts(
    mut accounts: Vec<core_store::AdminCodexAccount>,
    status: Option<core_store::CodexRateLimitStatus>,
) -> Vec<core_store::AdminCodexAccount> {
    let Some(status) = status else {
        return accounts;
    };
    let mut status_by_name = status
        .accounts
        .into_iter()
        .map(|account| (account.name.clone(), account))
        .collect::<BTreeMap<_, _>>();
    for account in &mut accounts {
        let Some(status_account) = status_by_name.remove(&account.name) else {
            continue;
        };
        apply_codex_public_status_to_admin_account(account, status_account, status.last_checked_at);
    }
    accounts
}
pub(crate) fn apply_codex_public_status_to_admin_account(
    account: &mut core_store::AdminCodexAccount,
    status_account: core_store::CodexPublicAccountStatus,
    _status_last_checked_at: Option<i64>,
) {
    if account.status != KEY_STATUS_ACTIVE || status_account.status != KEY_STATUS_ACTIVE {
        account.plan_type = None;
        account.primary_remaining_percent = None;
        account.secondary_remaining_percent = None;
        account.last_usage_checked_at = None;
        account.last_usage_success_at = None;
        account.usage_error_message = None;
        return;
    }
    account.plan_type = status_account.plan_type;
    account.primary_remaining_percent = status_account.primary_remaining_percent;
    account.secondary_remaining_percent = status_account.secondary_remaining_percent;
    account.last_usage_checked_at = status_account.last_usage_checked_at;
    account.last_usage_success_at = status_account.last_usage_success_at;
    account.usage_error_message = status_account.usage_error_message;
}
pub(crate) async fn import_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<ImportLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let auth = match normalize_imported_codex_auth(request.auth_json, request.tokens) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    let account = NewAdminCodexAccount {
        name,
        account_id: auth.account_id,
        auth_json: auth.auth_json,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        route_weight_tier: None,
        created_at_ms: now_ms(),
    };
    match state
        .admin_codex_account_store
        .create_admin_codex_account(account)
        .await
    {
        Ok(account) => Json(account).into_response(),
        Err(_) => internal_error("Failed to import llm gateway account").into_response(),
    }
}
pub(crate) async fn create_llm_gateway_account_import_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateCodexBatchImportJobRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let request = match normalize_codex_batch_import_request(request) {
        Ok(request) => request,
        Err(response) => return response.into_response(),
    };
    let created_at_ms = now_ms();
    let job_id = generate_id("llm-import");
    let persisted = NewAdminCodexImportJob {
        job_id: job_id.clone(),
        provider_type: request.provider_type.clone(),
        source_type: request.source_type.clone(),
        validate_before_import: request.validate_before_import,
        items: request
            .items
            .iter()
            .map(|item| NewAdminCodexImportJobItem {
                requested_name: item.requested_name.clone(),
                requested_account_id: item.requested_account_id.clone(),
                raw_auth_json: item.raw_auth_json.clone(),
            })
            .collect(),
        created_at_ms,
    };
    let detail = match state
        .admin_codex_account_store
        .create_admin_codex_import_job(persisted)
        .await
    {
        Ok(detail) => detail,
        Err(_) => {
            return internal_error("Failed to create llm gateway account import job")
                .into_response();
        },
    };

    let worker_state = state.clone();
    tokio::spawn(async move {
        if let Err(err) =
            run_codex_batch_import_job(worker_state.clone(), job_id.clone(), request).await
        {
            let _ = worker_state
                .admin_codex_account_store
                .fail_admin_codex_import_job(&job_id, &err.to_string(), now_ms())
                .await;
        }
    });

    Json(detail).into_response()
}
pub(crate) async fn list_llm_gateway_account_import_jobs(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<ListCodexImportJobsRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let limit = query
        .limit
        .unwrap_or(DEFAULT_ADMIN_IMPORT_JOB_LIMIT)
        .clamp(1, MAX_ADMIN_IMPORT_JOB_LIMIT);
    match state
        .admin_codex_account_store
        .list_admin_codex_import_jobs(limit)
        .await
    {
        Ok(jobs) => Json(AdminCodexImportJobsResponse {
            jobs,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account import jobs").into_response(),
    }
}
pub(crate) async fn get_llm_gateway_account_import_job(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_codex_account_store
        .get_admin_codex_import_job(&job_id)
        .await
    {
        Ok(Some(detail)) => Json(detail).into_response(),
        Ok(None) => not_found("LLM gateway account import job not found").into_response(),
        Err(_) => internal_error("Failed to load llm gateway account import job").into_response(),
    }
}
pub(crate) async fn patch_llm_gateway_account(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_account_name(&name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let patch = match normalize_account_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    let refresh_public_status = should_refresh_codex_public_status_after_patch(&patch);
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
        .admin_codex_account_store
        .patch_admin_codex_account(&name, patch)
        .await
    {
        Ok(Some(mut account)) => {
            if refresh_public_status {
                if let Err(err) =
                    refresh_codex_public_status_after_account_update(&state, &mut account).await
                {
                    tracing::warn!(
                        account_name = %account.name,
                        "failed to refresh Codex public status after account update: {err:#}"
                    );
                }
            }
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account").into_response(),
    }
}
pub(crate) fn should_refresh_codex_public_status_after_patch(
    patch: &AdminCodexAccountPatch,
) -> bool {
    patch.status.is_some()
        || patch.auto_refresh_enabled.is_some()
        || patch.proxy_mode.is_some()
        || patch.proxy_config_id.is_some()
}
pub(crate) async fn refresh_codex_public_status_after_account_update(
    state: &HttpState,
    account: &mut core_store::AdminCodexAccount,
) -> anyhow::Result<()> {
    let route_store = state.provider_state.route_store();
    let refreshed_status = if account.status == KEY_STATUS_ACTIVE && !account.auto_refresh_enabled {
        codex_status::refresh_single_codex_account_usage_only(
            &state.admin_config_store,
            &state.admin_codex_account_store,
            &route_store,
            &state.public_status_store,
            &account.name,
        )
        .await?
    } else {
        codex_status::prime_single_codex_account_status(
            &state.admin_config_store,
            &state.admin_codex_account_store,
            &route_store,
            &state.public_status_store,
            &account.name,
        )
        .await?
    };
    apply_codex_public_status_to_admin_account(account, refreshed_status, None);
    Ok(())
}
pub(crate) async fn delete_llm_gateway_account(
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
        .admin_codex_account_store
        .delete_admin_codex_account(&name)
        .await
    {
        Ok(Some(account)) => Json(DeleteResponse {
            deleted: true,
            id: account.name,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account").into_response(),
    }
}
pub(crate) async fn refresh_llm_gateway_account(
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
    let route_store = state.provider_state.route_store();
    let refreshed_status = match codex_status::refresh_single_codex_account_status(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        &name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(mut account)) => {
            apply_codex_public_status_to_admin_account(&mut account, refreshed_status, None);
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to refresh llm gateway account").into_response(),
    }
}
pub(crate) async fn refresh_llm_gateway_account_auth(
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
    let route = match state
        .admin_codex_account_store
        .resolve_admin_codex_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("LLM gateway account not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway account").into_response(),
    };
    let refreshed = match codex_refresh::refresh_auth_json_for_route(&route).await {
        Ok(refreshed) => refreshed,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account auth: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    if let Err(err) = state
        .provider_state
        .route_store()
        .save_codex_auth_update(refreshed)
        .await
    {
        return (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to persist llm gateway account auth refresh: {err}"),
                code: StatusCode::BAD_GATEWAY.as_u16(),
            }),
        )
            .into_response();
    }
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(account)) => Json(account).into_response(),
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to load refreshed llm gateway account").into_response(),
    }
}
pub(crate) async fn refresh_llm_gateway_account_usage(
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
    let route_store = state.provider_state.route_store();
    let refreshed_status = match codex_status::refresh_single_codex_account_usage_only(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        &name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to refresh llm gateway account usage: {err}"),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    match state
        .admin_codex_account_store
        .get_admin_codex_account(&name)
        .await
    {
        Ok(Some(mut account)) => {
            apply_codex_public_status_to_admin_account(&mut account, refreshed_status, None);
            Json(account).into_response()
        },
        Ok(None) => not_found("LLM gateway account not found").into_response(),
        Err(_) => internal_error("Failed to refresh llm gateway account usage").into_response(),
    }
}
pub(crate) async fn probe_llm_gateway_account_models(
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
    let route = match state
        .admin_codex_account_store
        .resolve_admin_codex_account_route(&name)
        .await
    {
        Ok(Some(route)) => route,
        Ok(None) => return not_found("LLM gateway account not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway account").into_response(),
    };
    let auth = match normalize_codex_auth_json(&route.auth_json) {
        Ok(auth) => auth,
        Err(err) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(ErrorResponse {
                    error: format!("Failed to parse llm gateway account auth: {}", err.message),
                    code: StatusCode::BAD_GATEWAY.as_u16(),
                }),
            )
                .into_response();
        },
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let client_version =
        crate::provider::resolve_codex_client_version(Some(&config.codex_client_version));
    match validate_codex_access_token_for_import_with_client_version(&route, &auth, &client_version)
        .await
    {
        Ok(()) => Json(AdminCodexModelsProbeResponse {
            ok: true,
            message: "Codex models probe succeeded".to_string(),
            checked_at: now_ms(),
        })
        .into_response(),
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(ErrorResponse {
                error: format!("Failed to probe llm gateway account models: {err}"),
                code: StatusCode::BAD_GATEWAY.as_u16(),
            }),
        )
            .into_response(),
    }
}
pub(crate) async fn run_codex_batch_import_job(
    state: HttpState,
    job_id: String,
    request: NormalizedCodexBatchImportJobRequest,
) -> anyhow::Result<()> {
    state
        .admin_codex_account_store
        .mark_admin_codex_import_job_running(&job_id, now_ms())
        .await
        .with_context(|| format!("mark codex import job `{job_id}` running"))?;

    let mut seen_names = HashSet::new();
    for item in request.items {
        let item_updated_at_ms = now_ms();
        state
            .admin_codex_account_store
            .mark_admin_codex_import_job_item_running(&job_id, item.item_index, item_updated_at_ms)
            .await
            .with_context(|| {
                format!("mark codex import job `{job_id}` item {} running", item.item_index)
            })?;

        if !seen_names.insert(item.requested_name.clone()) {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some("account name is duplicated within the batch".to_string()),
                        item.requested_account_id.clone(),
                        None,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete duplicated codex import job `{job_id}` item {}",
                        item.item_index
                    )
                })?;
            continue;
        }

        if state
            .admin_codex_account_store
            .get_admin_codex_account(&item.requested_name)
            .await
            .with_context(|| format!("load codex account `{}`", item.requested_name))?
            .is_some()
        {
            state
                .admin_codex_account_store
                .complete_admin_codex_import_job_item(
                    &job_id,
                    codex_import_job_failure_result(
                        item.item_index,
                        "conflict",
                        Some("account name already exists".to_string()),
                        item.requested_account_id.clone(),
                        None,
                        None,
                    ),
                )
                .await
                .with_context(|| {
                    format!(
                        "complete existing-name conflict for codex import job `{job_id}` item {}",
                        item.item_index
                    )
                })?;
            continue;
        }

        if let Some(account_id) = item.requested_account_id.as_deref() {
            if let Some(existing_name) = state
                .admin_codex_account_store
                .find_admin_codex_account_name_by_account_id(account_id)
                .await
                .with_context(|| format!("lookup codex account id `{account_id}`"))?
            {
                if existing_name != item.requested_name {
                    state
                        .admin_codex_account_store
                        .complete_admin_codex_import_job_item(
                            &job_id,
                            codex_import_job_failure_result(
                                item.item_index,
                                "conflict",
                                Some("account_id already belongs to another account".to_string()),
                                Some(account_id.to_string()),
                                None,
                                None,
                            ),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "complete account-id conflict for codex import job `{job_id}` \
                                 item {}",
                                item.item_index
                            )
                        })?;
                    continue;
                }
            }
        }

        let (auth, validated_at_ms) = if request.validate_before_import {
            match validate_codex_batch_import_auth(&state, &item).await {
                Ok(auth) => (auth, Some(now_ms())),
                Err(err) => {
                    state
                        .admin_codex_account_store
                        .complete_admin_codex_import_job_item(
                            &job_id,
                            codex_import_job_failure_result(
                                item.item_index,
                                "failed",
                                Some(err.to_string()),
                                item.requested_account_id.clone(),
                                None,
                                None,
                            ),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "complete validation failure for codex import job `{job_id}` item \
                                 {}",
                                item.item_index
                            )
                        })?;
                    continue;
                },
            }
        } else {
            (item.auth.clone(), None)
        };

        if let Some(account_id) = auth.account_id.as_deref() {
            if let Some(existing_name) = state
                .admin_codex_account_store
                .find_admin_codex_account_name_by_account_id(account_id)
                .await
                .with_context(|| format!("lookup refreshed codex account id `{account_id}`"))?
            {
                if existing_name != item.requested_name {
                    state
                        .admin_codex_account_store
                        .complete_admin_codex_import_job_item(
                            &job_id,
                            codex_import_job_failure_result(
                                item.item_index,
                                "conflict",
                                Some(
                                    "validated account_id already belongs to another account"
                                        .to_string(),
                                ),
                                Some(account_id.to_string()),
                                validated_at_ms,
                                None,
                            ),
                        )
                        .await
                        .with_context(|| {
                            format!(
                                "complete validated account-id conflict for codex import job \
                                 `{job_id}` item {}",
                                item.item_index
                            )
                        })?;
                    continue;
                }
            }
        }

        let imported_at_ms = now_ms();
        match state
            .admin_codex_account_store
            .create_admin_codex_account(NewAdminCodexAccount {
                name: item.requested_name.clone(),
                account_id: auth.account_id.clone(),
                auth_json: auth.auth_json.clone(),
                map_gpt53_codex_to_spark: false,
                auto_refresh_enabled: true,
                route_weight_tier: None,
                created_at_ms: imported_at_ms,
            })
            .await
        {
            Ok(account) => {
                state
                    .admin_codex_account_store
                    .complete_admin_codex_import_job_item(
                        &job_id,
                        codex_import_job_success_result(
                            item.item_index,
                            account.name,
                            account.account_id.or(auth.account_id.clone()),
                            validated_at_ms,
                            imported_at_ms,
                        ),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "complete imported codex import job `{job_id}` item {}",
                            item.item_index
                        )
                    })?;
            },
            Err(err) => {
                state
                    .admin_codex_account_store
                    .complete_admin_codex_import_job_item(
                        &job_id,
                        codex_import_job_failure_result(
                            item.item_index,
                            "failed",
                            Some(err.to_string()),
                            auth.account_id.clone(),
                            validated_at_ms,
                            None,
                        ),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "complete create failure for codex import job `{job_id}` item {}",
                            item.item_index
                        )
                    })?;
            },
        }
    }

    Ok(())
}
pub(crate) async fn validate_codex_batch_import_auth(
    state: &HttpState,
    item: &NormalizedCodexBatchImportJobItem,
) -> anyhow::Result<NormalizedCodexAuth> {
    validate_codex_import_auth(state, &item.requested_name, &item.auth)
        .await
        .with_context(|| {
            format!("validate auth for codex batch import account `{}`", item.requested_name)
        })
}
pub(crate) async fn validate_codex_import_auth(
    state: &HttpState,
    account_name: &str,
    auth: &NormalizedCodexAuth,
) -> anyhow::Result<NormalizedCodexAuth> {
    let proxy = required_codex_default_proxy(state)
        .await
        .map_err(|err| anyhow::anyhow!(err.message))?;
    let route = codex_validation_route(account_name, auth, proxy.clone());
    let has_refresh_token = auth
        .refresh_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty());
    if should_validate_codex_access_token_directly(auth) {
        match validate_codex_access_token_for_import(&route, auth).await {
            Ok(()) => return Ok(auth.clone()),
            Err(access_err) if !has_refresh_token => {
                return Err(access_err)
                    .context("validate codex import access token against models");
            },
            Err(_) => {},
        }
    }

    let refreshed = codex_refresh::refresh_auth_json_for_route(&route)
        .await
        .context("refresh auth for codex import account")?;
    let refreshed_auth = normalize_codex_auth_json(&refreshed.auth_json)
        .map_err(|err| anyhow::anyhow!(err.message))?;
    let refreshed_route = codex_validation_route(account_name, &refreshed_auth, proxy);
    validate_codex_access_token_for_import(&refreshed_route, &refreshed_auth)
        .await
        .context("validate refreshed codex import access token against models")?;
    Ok(refreshed_auth)
}
pub(crate) fn codex_validation_route(
    account_name: &str,
    auth: &NormalizedCodexAuth,
    proxy: core_store::ProviderProxyConfig,
) -> core_store::ProviderCodexRoute {
    core_store::ProviderCodexRoute {
        account_name: account_name.to_string(),
        account_group_id_at_event: None,
        route_strategy_at_event: RouteStrategy::Fixed,
        auth_json: auth.auth_json.clone(),
        map_gpt53_codex_to_spark: false,
        auth_refresh_enabled: true,
        codex_fast_enabled: true,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        account_request_max_concurrency: None,
        account_request_min_start_interval_ms: None,
        cached_error_message: None,
        proxy: Some(proxy),
    }
}
pub(crate) fn should_validate_codex_access_token_directly(auth: &NormalizedCodexAuth) -> bool {
    auth.access_token
        .as_deref()
        .map(str::trim)
        .is_some_and(|token| !token.is_empty() && !codex_refresh::access_token_is_expired(token))
}
pub(crate) async fn validate_codex_access_token_for_import(
    route: &core_store::ProviderCodexRoute,
    auth: &NormalizedCodexAuth,
) -> anyhow::Result<()> {
    validate_codex_access_token_for_import_with_client_version(
        route,
        auth,
        core_store::DEFAULT_CODEX_CLIENT_VERSION,
    )
    .await
}
pub(crate) async fn validate_codex_access_token_for_import_with_client_version(
    route: &core_store::ProviderCodexRoute,
    auth: &NormalizedCodexAuth,
    client_version: &str,
) -> anyhow::Result<()> {
    let access_token = auth
        .access_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing codex access_token"))?;
    let upstream_url = llm_access_codex::models::append_client_version_query(
        &crate::provider::compute_codex_upstream_url(
            &crate::provider::codex_upstream_base_url(),
            "/v1/models",
        ),
        client_version,
    );
    let client = codex_refresh::provider_client(route.proxy.as_ref())?;
    let mut request = client
        .get(&upstream_url)
        .bearer_auth(access_token)
        .header(reqwest::header::ACCEPT, "application/json")
        .header(
            reqwest::header::USER_AGENT,
            format!("{}/{}", CODEX_WIRE_ORIGINATOR, core_store::DEFAULT_CODEX_CLIENT_VERSION),
        )
        .header(reqwest::header::HeaderName::from_static("originator"), CODEX_WIRE_ORIGINATOR)
        .timeout(Duration::from_secs(CODEX_ACCESS_TOKEN_VALIDATION_TIMEOUT_SECONDS));
    if let Some(account_id) = auth.account_id.as_deref() {
        request = request.header("chatgpt-account-id", account_id);
    }
    if auth
        .id_token
        .as_deref()
        .is_some_and(codex_refresh::id_token_is_fedramp_account)
    {
        request = request.header("x-openai-fedramp", "true");
    }

    let response = request
        .send()
        .await
        .context("request Codex models with access token")?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        anyhow::bail!(
            "codex access token validation returned {status}: {}",
            summarize_upstream_error_body(&body)
        );
    }
    let payload = response
        .json::<serde_json::Value>()
        .await
        .context("parse Codex models response")?;
    validate_codex_models_probe_payload(&payload)
}
pub(crate) fn validate_codex_models_probe_payload(
    payload: &serde_json::Value,
) -> anyhow::Result<()> {
    let models = payload
        .get("models")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("Codex models response is missing models array"))?;
    if models.is_empty() {
        anyhow::bail!("Codex models response has empty models array");
    }
    Ok(())
}
pub(crate) fn codex_import_job_failure_result(
    item_index: usize,
    status: &str,
    error_message: Option<String>,
    final_account_id: Option<String>,
    validated_at_ms: Option<i64>,
    imported_at_ms: Option<i64>,
) -> AdminCodexImportJobItemResult {
    AdminCodexImportJobItemResult {
        item_index,
        status: status.to_string(),
        error_message,
        imported_account_name: None,
        final_account_id,
        validated_at_ms,
        imported_at_ms,
        completed_delta: 1,
        succeeded_delta: 0,
        skipped_delta: 0,
        failed_delta: 1,
        updated_at_ms: now_ms(),
    }
}
pub(crate) fn codex_import_job_success_result(
    item_index: usize,
    imported_account_name: String,
    final_account_id: Option<String>,
    validated_at_ms: Option<i64>,
    imported_at_ms: i64,
) -> AdminCodexImportJobItemResult {
    AdminCodexImportJobItemResult {
        item_index,
        status: "imported".to_string(),
        error_message: None,
        imported_account_name: Some(imported_account_name),
        final_account_id,
        validated_at_ms,
        imported_at_ms: Some(imported_at_ms),
        completed_delta: 1,
        succeeded_delta: 1,
        skipped_delta: 0,
        failed_delta: 0,
        updated_at_ms: now_ms(),
    }
}
pub(crate) fn normalize_codex_batch_import_request(
    request: CreateCodexBatchImportJobRequest,
) -> Result<NormalizedCodexBatchImportJobRequest, AdminHttpError> {
    if request.provider_type.trim() != PROVIDER_CODEX {
        return Err(bad_request("provider_type must be codex"));
    }
    if request.source_type.trim() != "local_json" {
        return Err(bad_request("source_type must be local_json"));
    }
    if request.items.is_empty() {
        return Err(bad_request("items must not be empty"));
    }
    let mut items = Vec::with_capacity(request.items.len());
    for (item_index, item) in request.items.into_iter().enumerate() {
        let requested_name = normalize_account_name(&item.name)?;
        let auth = normalize_imported_codex_auth(item.auth_json, item.tokens)?;
        items.push(NormalizedCodexBatchImportJobItem {
            item_index,
            requested_name,
            requested_account_id: auth.account_id.clone(),
            raw_auth_json: auth.auth_json.clone(),
            auth,
        });
    }
    Ok(NormalizedCodexBatchImportJobRequest {
        provider_type: PROVIDER_CODEX.to_string(),
        source_type: "local_json".to_string(),
        validate_before_import: request.validate_before_import,
        items,
    })
}
pub(crate) fn normalize_imported_codex_auth(
    raw_auth_json: Option<serde_json::Value>,
    tokens: Option<ImportLlmGatewayAccountTokens>,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    if let Some(value) = raw_auth_json {
        return normalize_codex_auth_value(value);
    }
    let Some(tokens) = tokens else {
        return Err(bad_request("auth_json or tokens is required"));
    };
    codex_auth_from_fields(
        tokens.account_id.as_deref(),
        tokens.id_token.as_deref(),
        tokens.access_token.as_deref(),
        tokens.refresh_token.as_deref(),
    )
}
pub(crate) fn codex_auth_from_fields(
    account_id: Option<&str>,
    id_token: Option<&str>,
    access_token: Option<&str>,
    refresh_token: Option<&str>,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    let account_id = normalize_optional_string_option(account_id);
    let id_token = normalize_optional_string_option(id_token);
    let access_token = normalize_optional_string_option(access_token);
    let refresh_token = normalize_optional_string_option(refresh_token);
    if access_token.is_none() && refresh_token.is_none() {
        return Err(bad_request("access_token or refresh_token is required"));
    }
    let mut object = serde_json::Map::new();
    if let Some(value) = id_token.as_ref() {
        object.insert("id_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = access_token.as_ref() {
        object.insert("access_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = refresh_token.as_ref() {
        object.insert("refresh_token".to_string(), serde_json::Value::String(value.clone()));
    }
    if let Some(value) = account_id.as_ref() {
        object.insert("account_id".to_string(), serde_json::Value::String(value.clone()));
    }
    normalize_codex_auth_value(serde_json::Value::Object(object))
}
pub(crate) fn normalize_codex_auth_json(raw: &str) -> Result<NormalizedCodexAuth, AdminHttpError> {
    let value = serde_json::from_str::<serde_json::Value>(raw)
        .map_err(|_| bad_request("auth_json must be valid JSON"))?;
    normalize_codex_auth_value(value)
}
pub(crate) fn normalize_codex_auth_value(
    value: serde_json::Value,
) -> Result<NormalizedCodexAuth, AdminHttpError> {
    if !value.is_object() {
        return Err(bad_request("auth_json must be a JSON object"));
    }
    let id_token = optional_auth_json_string(&value, &["id_token", "idToken"]);
    let access_token = optional_auth_json_string(&value, &["access_token", "accessToken"]);
    let refresh_token = optional_auth_json_string(&value, &["refresh_token", "refreshToken"]);
    let account_id = optional_auth_json_string(&value, &["account_id", "accountId"]);
    if access_token.is_none() && refresh_token.is_none() {
        return Err(bad_request("auth_json must contain access_token or refresh_token"));
    }
    let auth_json = serde_json::to_string(&value)
        .map_err(|_| internal_error("Failed to encode account auth"))?;
    Ok(NormalizedCodexAuth {
        auth_json,
        account_id,
        id_token,
        access_token,
        refresh_token,
    })
}
pub(crate) fn optional_auth_json_string(
    value: &serde_json::Value,
    fields: &[&str],
) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(serde_json::Value::as_str))
        .or_else(|| {
            value.get("tokens").and_then(|tokens| {
                fields
                    .iter()
                    .find_map(|field| tokens.get(*field).and_then(serde_json::Value::as_str))
            })
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
pub(crate) fn normalize_account_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(bad_request("account name is required"));
    }
    if trimmed.len() > 64 {
        return Err(bad_request("account name must be 64 characters or fewer"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err(bad_request(
            "account name must contain only ASCII letters, digits, hyphens, or underscores",
        ));
    }
    Ok(trimmed.to_string())
}
pub(crate) fn normalize_account_names(
    values: Vec<String>,
) -> Result<Option<Vec<String>>, AdminHttpError> {
    let mut names = values
        .into_iter()
        .map(|value| normalize_account_name(&value))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    names.dedup();
    if names.is_empty() {
        Ok(None)
    } else {
        Ok(Some(names))
    }
}
pub(crate) fn normalize_auto_account_names(values: Vec<String>) -> Option<Vec<String>> {
    let mut names = values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    names.sort();
    names.dedup();
    if names.is_empty() {
        None
    } else {
        Some(names)
    }
}
pub(crate) fn normalize_account_patch(
    request: PatchLlmGatewayAccountRequest,
) -> Result<AdminCodexAccountPatch, AdminHttpError> {
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
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
    let route_weight_tier = request
        .route_weight_tier
        .as_deref()
        .map(normalize_codex_route_weight_tier)
        .transpose()?;
    let request_max_concurrency = if request.request_max_concurrency_unlimited {
        Some(None)
    } else {
        request.request_max_concurrency.map(Some)
    };
    let request_min_start_interval_ms = if request.request_min_start_interval_ms_unlimited {
        Some(None)
    } else {
        request.request_min_start_interval_ms.map(Some)
    };
    validate_codex_request_limit_inputs(
        request_max_concurrency.flatten(),
        request_min_start_interval_ms.flatten(),
    )?;
    Ok(AdminCodexAccountPatch {
        status,
        map_gpt53_codex_to_spark: request.map_gpt53_codex_to_spark,
        auto_refresh_enabled: request.auto_refresh_enabled,
        route_weight_tier,
        proxy_mode,
        proxy_config_id,
        request_max_concurrency,
        request_min_start_interval_ms,
        updated_at_ms: now_ms(),
    })
}
pub(crate) fn normalize_codex_route_weight_tier(raw: &str) -> Result<String, AdminHttpError> {
    let Some(value) = normalize_optional_string(raw) else {
        return Err(bad_request("route_weight_tier cannot be empty"));
    };
    match value.to_ascii_lowercase().as_str() {
        "auto" | "free" | "plus" | "pro5x" | "pro20x" => Ok(value.to_ascii_lowercase()),
        _ => Err(bad_request("route_weight_tier must be one of auto, free, plus, pro5x, pro20x")),
    }
}
pub(crate) fn validate_codex_request_limit_inputs(
    request_max_concurrency: Option<u64>,
    request_min_start_interval_ms: Option<u64>,
) -> Result<(), AdminHttpError> {
    if let Some(value) = request_max_concurrency {
        if value == 0 || value > MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY {
            return Err(bad_request("request_max_concurrency is out of range"));
        }
    }
    if let Some(value) = request_min_start_interval_ms {
        if value > MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS {
            return Err(bad_request("request_min_start_interval_ms is out of range"));
        }
    }
    Ok(())
}
