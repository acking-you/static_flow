//! Admin API key CRUD (Codex + Kiro planes), key-patch normalization, and Kiro
//! candidate-credit-summary computation/attachment.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_keys(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminKeyListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(AdminListQuery {
        limit: query.limit,
        offset: query.offset,
    });
    let filter = admin_key_page_query(&query);
    let page = match state
        .admin_key_store
        .list_admin_keys_filtered_page(None, &filter, page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list llm gateway keys").into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let keys = match apply_effective_kiro_cache_policies(page.keys, &config) {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to resolve Kiro cache policy").into_response(),
    };
    Json(AdminKeysResponse {
        keys,
        summary: page.summary,
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) async fn create_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    if let Err(response) =
        validate_i64_backed_u64("quota_billable_limit", request.quota_billable_limit)
    {
        return response.into_response();
    }
    if let Err(response) = validate_codex_request_limit_inputs(
        request.request_max_concurrency,
        request.request_min_start_interval_ms,
    ) {
        return response.into_response();
    }
    let secret = generate_secret();
    let key = NewAdminKey {
        id: generate_id("llm-key"),
        name,
        key_hash: sha256_hex(secret.as_bytes()),
        secret,
        provider_type: PROVIDER_CODEX.to_string(),
        protocol_family: PROTOCOL_OPENAI.to_string(),
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        request_max_concurrency: request.request_max_concurrency,
        request_min_start_interval_ms: request.request_min_start_interval_ms,
        created_at_ms: now_ms(),
    };
    match state.admin_key_store.create_admin_key(key).await {
        Ok(key) => Json(key).into_response(),
        Err(_) => internal_error("Failed to create llm gateway key").into_response(),
    }
}
pub(crate) async fn patch_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match admin_key_provider(&state, &key_id).await {
        Ok(Some(provider_type)) if provider_type == PROVIDER_CODEX => {},
        Ok(Some(_)) => {
            return bad_request("Kiro keys must be managed from /admin/kiro-gateway")
                .into_response();
        },
        Ok(None) => return not_found("LLM gateway key not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway key").into_response(),
    }
    let patch = match normalize_key_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state.admin_key_store.patch_admin_key(&key_id, patch).await {
        Ok(Some(key)) => match resolve_key_effective_kiro_cache_policy(&state, key).await {
            Ok(key) => Json(key).into_response(),
            Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
        },
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway key").into_response(),
    }
}
pub(crate) async fn delete_llm_gateway_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_key_store.delete_admin_key(&key_id).await {
        Ok(Some(key)) => Json(DeleteResponse {
            deleted: true,
            id: key.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway key not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway key").into_response(),
    }
}
pub(crate) fn admin_key_page_query(query: &AdminKeyListQuery) -> core_store::AdminKeyPageQuery {
    core_store::AdminKeyPageQuery {
        search: query.q.clone(),
        active_only: query.active_only.unwrap_or(false),
        sort: match query.sort.as_deref() {
            Some("quota_asc") => core_store::AdminKeySortMode::QuotaAsc,
            Some("quota_desc") => core_store::AdminKeySortMode::QuotaDesc,
            Some("usage_asc") => core_store::AdminKeySortMode::UsageAsc,
            Some("usage_desc") => core_store::AdminKeySortMode::UsageDesc,
            _ => core_store::AdminKeySortMode::Newest,
        },
    }
}
pub(crate) async fn list_admin_kiro_keys(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page_request = admin_page_request(query);
    let page = match state
        .admin_key_store
        .list_admin_keys_page(Some(PROVIDER_KIRO), page_request)
        .await
    {
        Ok(page) => page,
        Err(_) => return internal_error("Failed to list Kiro gateway keys").into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let keys = match apply_effective_kiro_cache_policies(page.keys, &config) {
        Ok(keys) => keys,
        Err(_) => return internal_error("Failed to resolve Kiro cache policy").into_response(),
    };
    let keys = match attach_kiro_candidate_credit_summaries(&state, keys).await {
        Ok(keys) => keys,
        Err(_) => {
            return internal_error("Failed to compute Kiro candidate credit summary")
                .into_response();
        },
    };
    Json(AdminKeysResponse {
        keys,
        summary: page.summary,
        auth_cache_ttl_seconds: config.auth_cache_ttl_seconds,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        has_more: page.has_more,
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) async fn create_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    if let Err(response) =
        validate_i64_backed_u64("quota_billable_limit", request.quota_billable_limit)
    {
        return response.into_response();
    }
    let secret = generate_secret();
    let key = NewAdminKey {
        id: generate_id("kiro-key"),
        name,
        key_hash: sha256_hex(secret.as_bytes()),
        secret,
        provider_type: PROVIDER_KIRO.to_string(),
        protocol_family: PROTOCOL_ANTHROPIC.to_string(),
        public_visible: false,
        quota_billable_limit: request.quota_billable_limit,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        created_at_ms: now_ms(),
    };
    match state.admin_key_store.create_admin_key(key).await {
        Ok(key) => match resolve_key_effective_kiro_cache_policy(&state, key).await {
            Ok(key) => Json(key).into_response(),
            Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
        },
        Err(_) => internal_error("Failed to create Kiro gateway key").into_response(),
    }
}
pub(crate) async fn patch_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
    Json(request): Json<PatchLlmGatewayKeyRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_key_matches_provider(&state, &key_id, PROVIDER_KIRO).await {
        return not_found("Kiro gateway key not found").into_response();
    }
    let patch = match normalize_kiro_key_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state.admin_key_store.patch_admin_key(&key_id, patch).await {
        Ok(Some(key)) if key.provider_type == PROVIDER_KIRO => {
            match resolve_key_effective_kiro_cache_policy(&state, key).await {
                Ok(key) => Json(key).into_response(),
                Err(_) => internal_error("Failed to resolve Kiro cache policy").into_response(),
            }
        },
        Ok(_) => not_found("Kiro gateway key not found").into_response(),
        Err(_) => internal_error("Failed to update Kiro gateway key").into_response(),
    }
}
pub(crate) async fn delete_admin_kiro_key(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(key_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_key_matches_provider(&state, &key_id, PROVIDER_KIRO).await {
        return not_found("Kiro gateway key not found").into_response();
    }
    match state.admin_key_store.delete_admin_key(&key_id).await {
        Ok(Some(key)) => Json(DeleteResponse {
            deleted: true,
            id: key.id,
        })
        .into_response(),
        Ok(None) => not_found("Kiro gateway key not found").into_response(),
        Err(_) => internal_error("Failed to delete Kiro gateway key").into_response(),
    }
}
pub(crate) async fn load_full_chain_probe_key(
    state: &HttpState,
    key_name: &str,
    provider_type: &str,
) -> Result<core_store::AdminKey, AdminHttpError> {
    let keys = state
        .admin_key_store
        .list_admin_keys()
        .await
        .map_err(|_| internal_error("Failed to load real proxy probe keys"))?;
    keys.into_iter()
        .find(|key| {
            key.name == key_name
                && key.provider_type == provider_type
                && key.status == KEY_STATUS_ACTIVE
        })
        .ok_or_else(|| {
            not_found(&format!(
                "Active {provider_type} real proxy probe key '{key_name}' not found"
            ))
        })
}
pub(crate) async fn resolve_key_effective_kiro_cache_policy(
    state: &HttpState,
    key: core_store::AdminKey,
) -> anyhow::Result<core_store::AdminKey> {
    let config = state.admin_config_store.get_admin_runtime_config().await?;
    let keys = apply_effective_kiro_cache_policies(vec![key], &config)?;
    let keys = attach_kiro_candidate_credit_summaries(state, keys).await?;
    Ok(keys.into_iter().next().expect("single key should remain"))
}
pub(crate) fn apply_effective_kiro_cache_policies(
    mut keys: Vec<core_store::AdminKey>,
    config: &AdminRuntimeConfig,
) -> anyhow::Result<Vec<core_store::AdminKey>> {
    let runtime_policy = parse_kiro_cache_policy_json(&config.kiro_cache_policy_json)?;
    for key in keys
        .iter_mut()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
    {
        let effective = resolve_effective_kiro_cache_policy(
            &runtime_policy,
            key.kiro_cache_policy_override_json.as_deref(),
        )?;
        key.effective_kiro_cache_policy_json = serde_json::to_string(&effective)?;
        key.uses_global_kiro_cache_policy =
            uses_global_kiro_cache_policy(key.kiro_cache_policy_override_json.as_deref());
    }
    Ok(keys)
}
pub(crate) async fn attach_kiro_candidate_credit_summaries(
    state: &HttpState,
    keys: Vec<core_store::AdminKey>,
) -> anyhow::Result<Vec<core_store::AdminKey>> {
    if !keys.iter().any(|key| key.provider_type == PROVIDER_KIRO) {
        return Ok(keys);
    }
    if keys
        .iter()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
        .all(|key| key.kiro_candidate_credit_summary.is_some())
    {
        return Ok(keys);
    }
    let accounts = state
        .admin_kiro_account_store
        .list_admin_kiro_accounts()
        .await?;
    let groups = state
        .admin_account_group_store
        .list_admin_account_groups(PROVIDER_KIRO)
        .await?;
    Ok(apply_kiro_candidate_credit_summaries(keys, &accounts, &groups))
}
pub(crate) fn apply_kiro_candidate_credit_summaries(
    mut keys: Vec<core_store::AdminKey>,
    accounts: &[core_store::AdminKiroAccount],
    groups: &[core_store::AdminAccountGroup],
) -> Vec<core_store::AdminKey> {
    let all_account_names = accounts
        .iter()
        .map(|account| account.name.clone())
        .collect::<Vec<_>>();
    let accounts_by_name = accounts
        .iter()
        .map(|account| (account.name.as_str(), account))
        .collect::<BTreeMap<_, _>>();
    let groups_by_id = groups
        .iter()
        .map(|group| (group.id.as_str(), group))
        .collect::<BTreeMap<_, _>>();
    for key in keys
        .iter_mut()
        .filter(|key| key.provider_type == PROVIDER_KIRO)
    {
        key.kiro_candidate_credit_summary = Some(build_kiro_candidate_credit_summary(
            key,
            &accounts_by_name,
            &groups_by_id,
            &all_account_names,
        ));
    }
    keys
}
pub(crate) fn build_kiro_candidate_credit_summary(
    key: &core_store::AdminKey,
    accounts_by_name: &BTreeMap<&str, &core_store::AdminKiroAccount>,
    groups_by_id: &BTreeMap<&str, &core_store::AdminAccountGroup>,
    all_account_names: &[String],
) -> core_store::AdminKiroKeyCandidateCreditSummary {
    let mut seen = HashSet::<String>::new();
    let mut summary = core_store::AdminKiroKeyCandidateCreditSummary::default();
    for account_name in select_kiro_candidate_account_names(key, groups_by_id, all_account_names) {
        if !seen.insert(account_name.clone()) {
            continue;
        }
        let Some(account) = accounts_by_name.get(account_name.as_str()) else {
            continue;
        };
        summary.candidate_count += 1;
        if let Some(balance) = account.balance.as_ref() {
            summary.loaded_balance_count += 1;
            summary.total_limit += balance.usage_limit.max(0.0);
            summary.total_remaining += balance.remaining.max(0.0);
        } else {
            summary.missing_balance_count += 1;
        }
    }
    summary
}
pub(crate) fn normalize_key_patch(
    request: PatchLlmGatewayKeyRequest,
) -> Result<AdminKeyPatch, AdminHttpError> {
    let name = match request.name.as_deref() {
        Some(raw) => Some(normalize_name(raw)?),
        None => None,
    };
    let status = match request.status.as_deref() {
        Some(raw) => Some(normalize_status(raw)?),
        None => None,
    };
    if let Some(limit) = request.quota_billable_limit {
        validate_i64_backed_u64("quota_billable_limit", limit)?;
    }
    let route_strategy = match request.route_strategy.as_deref() {
        Some(raw) => Some(normalize_route_strategy_input(raw)?),
        None => None,
    };
    let account_group_id = request
        .account_group_id
        .as_deref()
        .map(normalize_optional_string);
    let fixed_account_name = request
        .fixed_account_name
        .as_deref()
        .map(normalize_optional_string);
    let auto_account_names = request.auto_account_names.map(normalize_auto_account_names);
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
    if let Some(Some(raw)) = request.kiro_cache_policy_override_json.as_ref() {
        parse_kiro_cache_policy_override_json(raw)
            .map_err(|_| bad_request("kiro_cache_policy_override_json is invalid"))?;
    }
    let kiro_billable_model_multipliers_override_json =
        match request.kiro_billable_model_multipliers_override_json {
            Some(Some(raw)) => {
                let normalized = parse_kiro_billable_model_multipliers_json(&raw)
                    .and_then(|value| serde_json::to_string(&value).map_err(Into::into))
                    .map_err(|_| {
                        bad_request("kiro_billable_model_multipliers_override_json is invalid")
                    })?;
                Some(Some(normalized))
            },
            Some(None) => Some(None),
            None => None,
        };
    Ok(AdminKeyPatch {
        name,
        status,
        public_visible: request.public_visible,
        quota_billable_limit: request.quota_billable_limit,
        route_strategy,
        account_group_id,
        fixed_account_name,
        auto_account_names,
        model_name_map: request.model_name_map.map(Some),
        request_max_concurrency,
        request_min_start_interval_ms,
        codex_fast_enabled: request.codex_fast_enabled,
        kiro_request_validation_enabled: request.kiro_request_validation_enabled,
        kiro_cache_estimation_enabled: request.kiro_cache_estimation_enabled,
        kiro_zero_cache_debug_enabled: request.kiro_zero_cache_debug_enabled,
        kiro_full_request_logging_enabled: request.kiro_full_request_logging_enabled,
        kiro_remote_media_resolution_enabled: request.kiro_remote_media_resolution_enabled,
        kiro_latency_routing_enabled: request.kiro_latency_routing_enabled,
        kiro_cache_policy_override_json: request.kiro_cache_policy_override_json,
        kiro_billable_model_multipliers_override_json,
        updated_at_ms: now_ms(),
    })
}
pub(crate) fn normalize_kiro_key_patch(
    mut request: PatchLlmGatewayKeyRequest,
) -> Result<AdminKeyPatch, AdminHttpError> {
    request.public_visible = None;
    request.request_max_concurrency = None;
    request.request_min_start_interval_ms = None;
    request.request_max_concurrency_unlimited = false;
    request.request_min_start_interval_ms_unlimited = false;
    request.codex_fast_enabled = None;
    normalize_key_patch(request)
}
