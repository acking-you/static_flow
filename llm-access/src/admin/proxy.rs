//! Proxy config/binding CRUD, legacy-proxy migration, connectivity + full-chain
//! checks, proxy client construction, and proxy-scope views.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_proxy_configs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let proxy_config_scope = admin_proxy_config_scope_view(&state).await;
    match state.admin_proxy_store.list_admin_proxy_configs().await {
        Ok(proxy_configs) => Json(AdminProxyConfigsResponse {
            proxy_config_scope,
            proxy_configs,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy configs").into_response(),
    }
}
pub(crate) async fn create_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_proxy_config_scope_view(&state)
        .await
        .can_edit_slot_metadata
    {
        return bad_request("proxy slots can only be created on the core node").into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let proxy_url = match normalize_required_proxy_url(&request.proxy_url) {
        Ok(proxy_url) => proxy_url,
        Err(response) => return response.into_response(),
    };
    let proxy = NewAdminProxyConfig {
        id: generate_id("llm-proxy"),
        name,
        proxy_url,
        proxy_username: normalize_optional_string_option(request.proxy_username.as_deref()),
        proxy_password: normalize_optional_string_option(request.proxy_password.as_deref()),
        created_at_ms: now_ms(),
    };
    match state
        .admin_proxy_store
        .create_admin_proxy_config(proxy)
        .await
    {
        Ok(proxy) => Json(proxy).into_response(),
        Err(_) => internal_error("Failed to create llm gateway proxy config").into_response(),
    }
}
pub(crate) async fn patch_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
    Json(request): Json<PatchLlmGatewayProxyConfigRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if request.name.is_some()
        && !admin_proxy_config_scope_view(&state)
            .await
            .can_edit_slot_metadata
    {
        return bad_request("proxy slot names can only be changed on the core node")
            .into_response();
    }
    let patch = match normalize_proxy_config_patch(request) {
        Ok(patch) => patch,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_proxy_store
        .patch_admin_proxy_config(&proxy_id, patch)
        .await
    {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy config").into_response(),
    }
}
pub(crate) async fn delete_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if !admin_proxy_config_scope_view(&state)
        .await
        .can_edit_slot_metadata
    {
        return bad_request("proxy slots can only be deleted on the core node").into_response();
    }
    let bindings = match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => bindings,
        Err(_) => {
            return internal_error("Failed to inspect llm gateway proxy bindings").into_response()
        },
    };
    if let Some(binding) = bindings
        .iter()
        .find(|binding| binding.bound_proxy_config_id.as_deref() == Some(proxy_id.as_str()))
    {
        return conflict(&format!(
            "proxy config is still bound to provider `{}`",
            binding.provider_type
        ))
        .into_response();
    }
    match state
        .admin_proxy_store
        .delete_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => Json(DeleteResponse {
            deleted: true,
            id: proxy.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway proxy config").into_response(),
    }
}
pub(crate) async fn reset_llm_gateway_proxy_config_override(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(proxy_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_proxy_store
        .reset_admin_proxy_config_override(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => Json(proxy).into_response(),
        Ok(None) => not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => {
            internal_error("Failed to reset llm gateway proxy config override").into_response()
        },
    }
}
pub(crate) async fn list_llm_gateway_proxy_bindings(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_proxy_store.list_admin_proxy_bindings().await {
        Ok(bindings) => Json(AdminProxyBindingsResponse {
            bindings,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway proxy bindings").into_response(),
    }
}
pub(crate) async fn update_llm_gateway_proxy_binding(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(provider_type): Path<String>,
    Json(request): Json<UpdateLlmGatewayProxyBindingRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let proxy_config_id = normalize_optional_string_option(request.proxy_config_id.as_deref());
    if let Some(proxy_id) = proxy_config_id.as_deref() {
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
            return bad_request("proxy config must be active before binding").into_response();
        }
    }
    match state
        .admin_proxy_store
        .update_admin_proxy_binding(&provider_type, proxy_config_id)
        .await
    {
        Ok(binding) => Json(binding).into_response(),
        Err(_) => internal_error("Failed to update llm gateway proxy binding").into_response(),
    }
}
pub(crate) async fn check_llm_gateway_proxy_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((proxy_id, provider_type)): Path<(String, String)>,
    payload: Option<Json<CheckLlmGatewayProxyConfigRequest>>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    if let Err(response) = validate_provider_type(&provider_type) {
        return response.into_response();
    }
    let mode = match normalize_proxy_check_mode(payload.as_ref().map(|payload| &payload.0)) {
        Ok(mode) => mode,
        Err(response) => return response.into_response(),
    };
    let proxy = match state
        .admin_proxy_store
        .get_admin_proxy_config(&proxy_id)
        .await
    {
        Ok(Some(proxy)) => proxy,
        Ok(None) => return not_found("LLM gateway proxy config not found").into_response(),
        Err(_) => return internal_error("Failed to load llm gateway proxy config").into_response(),
    };
    let check_result = match mode {
        AdminProxyCheckMode::Connectivity => run_proxy_connectivity_check(&proxy, &provider_type)
            .await
            .map_err(|_| internal_error("Failed to check upstream proxy config")),
        AdminProxyCheckMode::FullChain => {
            run_proxy_full_chain_check(&state, &proxy, &provider_type).await
        },
    };
    match check_result {
        Ok(result) => {
            if let Some(update) = proxy_endpoint_check_update_from_response(&result) {
                match state
                    .admin_proxy_store
                    .record_admin_proxy_endpoint_check(update)
                    .await
                {
                    Ok(Some(_)) => {},
                    Ok(None) => {
                        return not_found("LLM gateway proxy config not found").into_response();
                    },
                    Err(err) => {
                        tracing::warn!(
                            proxy_config_id = %result.proxy_config_id,
                            provider_type = %result.provider_type,
                            "failed to persist upstream proxy check: {err:#}"
                        );
                        return internal_error("Failed to save upstream proxy check")
                            .into_response();
                    },
                }
            }
            Json(result).into_response()
        },
        Err(response) => response.into_response(),
    }
}
pub(crate) async fn import_legacy_kiro_proxy_configs(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_proxy_store
        .import_legacy_kiro_proxy_configs()
        .await
    {
        Ok(result) => Json(AdminLegacyKiroProxyMigrationResponse {
            created_configs: result.created_configs,
            reused_configs: result.reused_configs,
            migrated_account_names: result.migrated_account_names,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to import legacy Kiro proxy configs").into_response(),
    }
}
pub(crate) async fn required_codex_default_proxy(
    state: &HttpState,
) -> Result<core_store::ProviderProxyConfig, AdminHttpError> {
    let bindings = state
        .admin_proxy_store
        .list_admin_proxy_bindings()
        .await
        .map_err(|_| internal_error("Failed to list llm gateway proxy bindings"))?;
    let binding = bindings
        .into_iter()
        .find(|binding| binding.provider_type == PROVIDER_CODEX)
        .ok_or_else(|| bad_request("default Codex proxy binding is not configured"))?;
    if let Some(message) = binding
        .error_message
        .as_deref()
        .and_then(normalize_optional_string)
    {
        return Err(bad_request(&format!("default Codex proxy binding is invalid: {message}")));
    }
    let proxy_url = binding
        .effective_proxy_url
        .as_deref()
        .and_then(normalize_optional_string)
        .ok_or_else(|| bad_request("default Codex proxy is required for validation"))?;
    Ok(core_store::ProviderProxyConfig {
        proxy_url,
        proxy_username: binding.effective_proxy_username,
        proxy_password: binding.effective_proxy_password,
    })
}
pub(crate) async fn run_proxy_connectivity_check(
    proxy: &core_store::AdminProxyConfig,
    provider_type: &str,
) -> anyhow::Result<AdminProxyCheckResponse> {
    let target_url = match provider_type {
        PROVIDER_CODEX => "https://chatgpt.com/backend-api/codex/models".to_string(),
        PROVIDER_KIRO => format!(
            "{}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST",
            crate::kiro_refresh::management_upstream_base_url("us-east-1")
        ),
        _ => unreachable!("provider type must be validated before proxy check"),
    };
    let client = build_proxy_client(proxy)?;
    let started_at = Instant::now();
    let result = client.get(&target_url).send().await;
    let target = match result {
        Ok(response) => {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            AdminProxyCheckTargetView {
                target: provider_type.to_string(),
                url: target_url,
                reachable: true,
                status_code: Some(status.as_u16()),
                latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
                error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
            }
        },
        Err(err) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: target_url,
            reachable: false,
            status_code: None,
            latency_ms: started_at.elapsed().as_millis().min(i64::MAX as u128) as i64,
            error_message: Some(err.to_string()),
        },
    };
    Ok(AdminProxyCheckResponse {
        proxy_config_id: proxy.id.clone(),
        proxy_config_name: proxy.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label: "anonymous connectivity probe".to_string(),
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}
pub(crate) async fn run_proxy_full_chain_check(
    state: &HttpState,
    proxy: &core_store::AdminProxyConfig,
    provider_type: &str,
) -> Result<AdminProxyCheckResponse, AdminHttpError> {
    let spec = proxy_full_chain_probe_spec(provider_type);
    let admin_key = load_full_chain_probe_key(state, spec.key_name, provider_type).await?;
    let authenticated_key = state
        .provider_state
        .authenticate_bearer_secret(&admin_key.secret)
        .await
        .map_err(|_| internal_error("Failed to authenticate real proxy probe key"))?
        .ok_or_else(|| internal_error("Real proxy probe key secret is not accepted"))?;
    if authenticated_key.key_id != admin_key.id
        || authenticated_key.provider_type != provider_type
        || authenticated_key.status != KEY_STATUS_ACTIVE
    {
        return Err(internal_error("Real proxy probe key state is inconsistent"));
    }
    let proxy_config = provider_proxy_config_from_admin(proxy);
    let request = Request::builder()
        .method(Method::POST)
        .uri(spec.path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, "llm-access-admin-proxy-probe")
        .body(Body::from(spec.body))
        .map_err(|_| internal_error("Failed to build real proxy probe request"))?;

    let started_at = Instant::now();
    let result =
        tokio::time::timeout(Duration::from_secs(PROXY_FULL_CHAIN_CHECK_TIMEOUT_SECONDS), async {
            let response = state
                .provider_state
                .dispatch_admin_probe_with_proxy(authenticated_key, request, proxy_config)
                .await;
            let status = response.status();
            let bytes = to_bytes(response.into_body(), PROXY_FULL_CHAIN_CHECK_MAX_BODY_BYTES)
                .await
                .context("read real proxy probe response body")?;
            Ok::<_, anyhow::Error>((status, String::from_utf8_lossy(&bytes).to_string()))
        })
        .await;
    let latency_ms = started_at.elapsed().as_millis().min(i64::MAX as u128) as i64;
    let target = match result {
        Ok(Ok((status, body))) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: status.is_success(),
            status_code: Some(status.as_u16()),
            latency_ms,
            error_message: (!status.is_success()).then(|| summarize_upstream_error_body(&body)),
        },
        Ok(Err(err)) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: false,
            status_code: None,
            latency_ms,
            error_message: Some(err.to_string()),
        },
        Err(_) => AdminProxyCheckTargetView {
            target: provider_type.to_string(),
            url: spec.path.to_string(),
            reachable: false,
            status_code: None,
            latency_ms,
            error_message: Some(format!(
                "real gateway probe timed out after {PROXY_FULL_CHAIN_CHECK_TIMEOUT_SECONDS}s"
            )),
        },
    };
    Ok(AdminProxyCheckResponse {
        proxy_config_id: proxy.id.clone(),
        proxy_config_name: proxy.name.clone(),
        provider_type: provider_type.to_string(),
        auth_label: format!("{} real gateway probe", spec.key_name),
        ok: target.reachable,
        targets: vec![target],
        checked_at: now_ms(),
    })
}
pub(crate) fn proxy_full_chain_probe_spec(provider_type: &str) -> ProxyFullChainProbeSpec {
    match provider_type {
        PROVIDER_CODEX => ProxyFullChainProbeSpec {
            key_name: PROXY_FULL_CHAIN_CODEX_KEY_NAME,
            path: "/api/codex-gateway/v1/responses",
            body: serde_json::to_vec(&serde_json::json!({
                "model": PROXY_FULL_CHAIN_CODEX_MODEL,
                "input": "ping",
                "instructions": "Reply with one short word.",
                "max_output_tokens": 1,
                "stream": true
            }))
            .expect("static codex probe json should serialize"),
        },
        PROVIDER_KIRO => ProxyFullChainProbeSpec {
            key_name: PROXY_FULL_CHAIN_KIRO_KEY_NAME,
            path: "/api/kiro-gateway/v1/messages",
            body: serde_json::to_vec(&serde_json::json!({
                "model": PROXY_FULL_CHAIN_KIRO_MODEL,
                "max_tokens": 1,
                "stream": true,
                "messages": [{
                    "role": "user",
                    "content": "ping"
                }]
            }))
            .expect("static kiro probe json should serialize"),
        },
        _ => unreachable!("provider type must be validated before proxy check"),
    }
}
pub(crate) fn provider_proxy_config_from_admin(
    proxy: &core_store::AdminProxyConfig,
) -> core_store::ProviderProxyConfig {
    core_store::ProviderProxyConfig {
        proxy_url: proxy.proxy_url.clone(),
        proxy_username: proxy.proxy_username.clone(),
        proxy_password: proxy.proxy_password.clone(),
    }
}
pub(crate) fn select_admin_kiro_probe_proxy(
    inline_proxy: Option<core_store::ProviderProxyConfig>,
    proxy_config_proxy: Option<core_store::ProviderProxyConfig>,
    resolved_proxy: Option<core_store::ProviderProxyConfig>,
) -> (Option<core_store::ProviderProxyConfig>, AdminKiroProbeProxySource) {
    if let Some(proxy) = inline_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::Inline);
    }
    if let Some(proxy) = proxy_config_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::ProxyConfig);
    }
    if let Some(proxy) = resolved_proxy {
        return (Some(proxy), AdminKiroProbeProxySource::Resolved);
    }
    (None, AdminKiroProbeProxySource::None)
}
pub(crate) async fn resolve_admin_kiro_probe_proxy(
    state: &HttpState,
    route: &core_store::ProviderKiroRoute,
    request: &NormalizedProbeKiroAccountModelRequest,
) -> Result<(Option<core_store::ProviderProxyConfig>, AdminKiroProbeProxySource), AdminHttpError> {
    let proxy_config_proxy = if let Some(proxy_id) = request.proxy_config_id.as_deref() {
        let proxy = match state
            .admin_proxy_store
            .get_admin_proxy_config(proxy_id)
            .await
        {
            Ok(Some(proxy)) => proxy,
            Ok(None) => return Err(not_found("LLM gateway proxy config not found")),
            Err(_) => return Err(internal_error("Failed to load llm gateway proxy config")),
        };
        if proxy.status != KEY_STATUS_ACTIVE {
            return Err(bad_request("proxy config must be active before probing"));
        }
        Some(provider_proxy_config_from_admin(&proxy))
    } else {
        None
    };
    Ok(select_admin_kiro_probe_proxy(
        request.inline_proxy.clone(),
        proxy_config_proxy,
        route.proxy.clone(),
    ))
}
pub(crate) fn normalize_proxy_check_mode(
    request: Option<&CheckLlmGatewayProxyConfigRequest>,
) -> Result<AdminProxyCheckMode, AdminHttpError> {
    let Some(mode) = request
        .and_then(|request| request.mode.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(AdminProxyCheckMode::Connectivity);
    };
    match mode {
        "connectivity" => Ok(AdminProxyCheckMode::Connectivity),
        "full_chain" | "real" | "real_gateway" => Ok(AdminProxyCheckMode::FullChain),
        _ => Err(bad_request("unknown proxy check mode")),
    }
}
pub(crate) fn proxy_endpoint_check_update_from_response(
    response: &AdminProxyCheckResponse,
) -> Option<core_store::AdminProxyEndpointCheckUpdate> {
    let target = response.targets.first()?;
    Some(core_store::AdminProxyEndpointCheckUpdate {
        proxy_config_id: response.proxy_config_id.clone(),
        provider_type: response.provider_type.clone(),
        target_url: target.url.clone(),
        reachable: target.reachable,
        status_code: target.status_code,
        latency_ms: target.latency_ms.max(0),
        error_message: target.error_message.clone(),
        checked_at_ms: response.checked_at,
    })
}
pub(crate) fn build_proxy_client(
    proxy: &core_store::AdminProxyConfig,
) -> anyhow::Result<reqwest::Client> {
    let mut proxy_config = reqwest::Proxy::all(&proxy.proxy_url)?;
    if let Some(username) = proxy.proxy_username.as_deref() {
        proxy_config =
            proxy_config.basic_auth(username, proxy.proxy_password.as_deref().unwrap_or(""));
    }
    reqwest::Client::builder()
        .proxy(proxy_config)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(PROXY_CONNECTIVITY_CHECK_TIMEOUT_SECONDS))
        .build()
        .map_err(Into::into)
}
pub(crate) fn normalize_required_proxy_url(raw: &str) -> Result<String, AdminHttpError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(bad_request("proxy_url is required"));
    }
    let parsed =
        url::Url::parse(value).map_err(|_| bad_request("proxy_url must be a valid URL"))?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h") {
        return Err(bad_request("proxy_url scheme must be http, https, socks5, or socks5h"));
    }
    if parsed.host_str().is_none() {
        return Err(bad_request("proxy_url must include a host"));
    }
    Ok(value.to_string())
}
pub(crate) fn normalize_proxy_config_patch(
    request: PatchLlmGatewayProxyConfigRequest,
) -> Result<AdminProxyConfigPatch, AdminHttpError> {
    let name = request.name.as_deref().map(normalize_name).transpose()?;
    let proxy_url = request
        .proxy_url
        .as_deref()
        .map(normalize_required_proxy_url)
        .transpose()?;
    let status = request
        .status
        .as_deref()
        .map(normalize_status)
        .transpose()?;
    Ok(AdminProxyConfigPatch {
        name,
        proxy_url,
        proxy_username: request
            .proxy_username
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        proxy_password: request
            .proxy_password
            .as_deref()
            .map(|value| normalize_optional_string_option(Some(value))),
        status,
        updated_at_ms: now_ms(),
    })
}
pub(crate) fn normalize_proxy_mode(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    match trimmed {
        "inherit" | "fixed" | "none" => Ok(trimmed.to_string()),
        _ => Err(bad_request("proxy_mode must be `inherit`, `fixed`, or `none`")),
    }
}
pub(crate) async fn admin_proxy_config_scope_view(state: &HttpState) -> AdminProxyConfigScopeView {
    match state.cluster_state.as_ref() {
        Some(cluster_state) => {
            let snapshot = cluster_state.snapshot().await;
            let is_core = snapshot.node.node_class == crate::cluster::NodeClass::Core;
            AdminProxyConfigScopeView {
                node_id: snapshot.node.node_id,
                is_core,
                can_edit_slot_metadata: is_core,
            }
        },
        None => AdminProxyConfigScopeView {
            node_id: "core".to_string(),
            is_core: true,
            can_edit_slot_metadata: true,
        },
    }
}
