//! Account-group CRUD across providers plus the per-provider dispatch helpers
//! and key/provider association lookups.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_account_groups(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page = admin_page_request(query);
    match state
        .admin_account_group_store
        .list_admin_account_groups_page(PROVIDER_CODEX, page)
        .await
    {
        Ok(groups) => Json(AdminAccountGroupsResponse {
            groups: groups.groups,
            total: groups.total,
            limit: groups.limit,
            offset: groups.offset,
            has_more: groups.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account groups").into_response(),
    }
}
pub(crate) async fn list_llm_gateway_account_group_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    list_account_group_options_for_provider(state, headers, PROVIDER_CODEX, "llm gateway").await
}
pub(crate) async fn create_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match normalize_account_names(request.account_names) {
        Ok(Some(names)) => names,
        Ok(None) => return bad_request("account_names must not be empty").into_response(),
        Err(response) => return response.into_response(),
    };
    let group = NewAdminAccountGroup {
        id: generate_id("llm-group"),
        provider_type: PROVIDER_CODEX.to_string(),
        name,
        account_names,
        created_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .create_admin_account_group(group)
        .await
    {
        Ok(group) => Json(group).into_response(),
        Err(_) => internal_error("Failed to create llm gateway account group").into_response(),
    }
}
pub(crate) async fn patch_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountGroupRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match request.name.as_deref().map(normalize_name).transpose() {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match request
        .account_names
        .map(normalize_account_names)
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(response) => return response.into_response(),
    };
    let patch = AdminAccountGroupPatch {
        name,
        account_names,
        updated_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .patch_admin_account_group(&group_id, patch)
        .await
    {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to update llm gateway account group").into_response(),
    }
}
pub(crate) async fn delete_llm_gateway_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let key = match state
        .admin_key_store
        .find_admin_key_referencing_account_group(PROVIDER_CODEX, &group_id)
        .await
    {
        Ok(key) => key,
        Err(_) => return internal_error("Failed to inspect llm gateway keys").into_response(),
    };
    if let Some(key) = key {
        return bad_request(&format!("account group is still referenced by key `{}`", key.name))
            .into_response();
    }
    match state
        .admin_account_group_store
        .delete_admin_account_group(&group_id)
        .await
    {
        Ok(Some(group)) => Json(DeleteResponse {
            deleted: true,
            id: group.id,
        })
        .into_response(),
        Ok(None) => not_found("LLM gateway account group not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway account group").into_response(),
    }
}
pub(crate) async fn list_admin_kiro_account_groups(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminListQuery>,
) -> Response {
    list_account_groups_for_provider(state, headers, query, PROVIDER_KIRO, "Kiro gateway").await
}
pub(crate) async fn list_admin_kiro_account_group_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    list_account_group_options_for_provider(state, headers, PROVIDER_KIRO, "Kiro gateway").await
}
pub(crate) async fn create_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<CreateLlmGatewayAccountGroupRequest>,
) -> Response {
    create_account_group_for_provider(state, headers, request, PROVIDER_KIRO, "kiro-group").await
}
pub(crate) async fn patch_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
    Json(request): Json<PatchLlmGatewayAccountGroupRequest>,
) -> Response {
    patch_account_group_for_provider(state, headers, group_id, request, PROVIDER_KIRO).await
}
pub(crate) async fn delete_admin_kiro_account_group(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(group_id): Path<String>,
) -> Response {
    delete_account_group_for_provider(state, headers, group_id, PROVIDER_KIRO).await
}
pub(crate) async fn admin_key_matches_provider(
    state: &HttpState,
    key_id: &str,
    provider_type: &str,
) -> bool {
    state
        .admin_key_store
        .get_admin_key(key_id)
        .await
        .ok()
        .flatten()
        .is_some_and(|key| key.provider_type == provider_type)
}
pub(crate) async fn admin_key_provider(
    state: &HttpState,
    key_id: &str,
) -> anyhow::Result<Option<String>> {
    Ok(state
        .admin_key_store
        .get_admin_key(key_id)
        .await?
        .map(|key| key.provider_type))
}
pub(crate) async fn list_account_groups_for_provider(
    state: HttpState,
    headers: HeaderMap,
    query: AdminListQuery,
    provider_type: &str,
    label: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let page = admin_page_request(query);
    match state
        .admin_account_group_store
        .list_admin_account_groups_page(provider_type, page)
        .await
    {
        Ok(groups) => Json(AdminAccountGroupsResponse {
            groups: groups.groups,
            total: groups.total,
            limit: groups.limit,
            offset: groups.offset,
            has_more: groups.has_more,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error(&format!("Failed to list {label} account groups")).into_response(),
    }
}
pub(crate) async fn list_account_group_options_for_provider(
    state: HttpState,
    headers: HeaderMap,
    provider_type: &str,
    label: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_account_group_store
        .list_admin_account_group_options(provider_type)
        .await
    {
        Ok(options) => Json(AdminAccountGroupOptionsResponse {
            options,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => {
            internal_error(&format!("Failed to list {label} account group options")).into_response()
        },
    }
}
pub(crate) async fn create_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    request: CreateLlmGatewayAccountGroupRequest,
    provider_type: &str,
    id_prefix: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let name = match normalize_name(&request.name) {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match normalize_account_names(request.account_names) {
        Ok(Some(names)) => names,
        Ok(None) => return bad_request("account_names must not be empty").into_response(),
        Err(response) => return response.into_response(),
    };
    let group = NewAdminAccountGroup {
        id: generate_id(id_prefix),
        provider_type: provider_type.to_string(),
        name,
        account_names,
        created_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .create_admin_account_group(group)
        .await
    {
        Ok(group) => Json(group).into_response(),
        Err(_) => internal_error("Failed to create account group").into_response(),
    }
}
pub(crate) async fn patch_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    group_id: String,
    request: PatchLlmGatewayAccountGroupRequest,
    provider_type: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current_groups = match state
        .admin_account_group_store
        .list_admin_account_groups(provider_type)
        .await
    {
        Ok(groups) => groups,
        Err(_) => return internal_error("Failed to inspect account groups").into_response(),
    };
    if !current_groups.iter().any(|group| group.id == group_id) {
        return not_found("Account group not found").into_response();
    }
    let name = match request.name.as_deref().map(normalize_name).transpose() {
        Ok(name) => name,
        Err(response) => return response.into_response(),
    };
    let account_names = match request
        .account_names
        .map(normalize_account_names)
        .transpose()
    {
        Ok(value) => value.flatten(),
        Err(response) => return response.into_response(),
    };
    let patch = AdminAccountGroupPatch {
        name,
        account_names,
        updated_at_ms: now_ms(),
    };
    match state
        .admin_account_group_store
        .patch_admin_account_group(&group_id, patch)
        .await
    {
        Ok(Some(group)) => Json(group).into_response(),
        Ok(None) => not_found("Account group not found").into_response(),
        Err(_) => internal_error("Failed to update account group").into_response(),
    }
}
pub(crate) async fn delete_account_group_for_provider(
    state: HttpState,
    headers: HeaderMap,
    group_id: String,
    provider_type: &str,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let key = match state
        .admin_key_store
        .find_admin_key_referencing_account_group(provider_type, &group_id)
        .await
    {
        Ok(key) => key,
        Err(_) => return internal_error("Failed to inspect gateway keys").into_response(),
    };
    if let Some(key) = key {
        return bad_request(&format!("account group is still referenced by key `{}`", key.name))
            .into_response();
    }
    let current_groups = match state
        .admin_account_group_store
        .list_admin_account_groups(provider_type)
        .await
    {
        Ok(groups) => groups,
        Err(_) => return internal_error("Failed to inspect account groups").into_response(),
    };
    if !current_groups.iter().any(|group| group.id == group_id) {
        return not_found("Account group not found").into_response();
    }
    match state
        .admin_account_group_store
        .delete_admin_account_group(&group_id)
        .await
    {
        Ok(Some(group)) => Json(DeleteResponse {
            deleted: true,
            id: group.id,
        })
        .into_response(),
        Ok(None) => not_found("Account group not found").into_response(),
        Err(_) => internal_error("Failed to delete account group").into_response(),
    }
}
