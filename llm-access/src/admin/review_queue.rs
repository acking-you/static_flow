//! Public review-queue handlers: token/account-contribution/sponsor request
//! listing, approval+issuance, validation, and rejection.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_token_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_token_requests(query)
        .await
    {
        Ok(page) => Json(AdminTokenRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway token requests").into_response(),
    }
}
pub(crate) async fn list_llm_gateway_account_contribution_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_account_contribution_requests(query)
        .await
    {
        Ok(page) => Json(AdminAccountContributionRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway account contribution requests")
            .into_response(),
    }
}
pub(crate) async fn list_llm_gateway_sponsor_requests(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(request): Query<ListReviewQueueRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let query = normalize_review_queue_query(request);
    match state
        .admin_review_queue_store
        .list_admin_sponsor_requests(query)
        .await
    {
        Ok(page) => Json(AdminSponsorRequestsResponse {
            total: page.total,
            offset: page.offset,
            limit: page.limit,
            has_more: page.has_more,
            requests: page.requests,
            generated_at: now_ms(),
        })
        .into_response(),
        Err(_) => internal_error("Failed to list llm gateway sponsor requests").into_response(),
    }
}
pub(crate) async fn approve_and_issue_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway token request is finalized").into_response();
    }
    let Some(notifier) = state.email_notifier.clone() else {
        return internal_error(
            "Failed to send llm gateway token email: email notifier is not configured",
        )
        .into_response();
    };
    if current.issued_key_id.is_some() {
        return conflict("LLM gateway token request already has an issued key").into_response();
    }
    let key = if current.issued_key_id.is_none() {
        let secret = generate_secret();
        Some(NewAdminKey {
            id: generate_id("llm-key"),
            name: normalize_name(&format!("wish-{}", current.request_id))
                .unwrap_or_else(|_| format!("wish-{}", current.request_id)),
            key_hash: sha256_hex(secret.as_bytes()),
            secret,
            provider_type: PROVIDER_CODEX.to_string(),
            protocol_family: PROTOCOL_OPENAI.to_string(),
            public_visible: false,
            quota_billable_limit: current.requested_quota_billable_limit,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            created_at_ms: now_ms(),
        })
    } else {
        None
    };
    let email_key = key.clone();
    match state
        .admin_review_queue_store
        .issue_admin_token_request(&request_id, key, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => {
            let Some(email_key) = email_key.as_ref() else {
                return internal_error("Failed to send llm gateway token email").into_response();
            };
            if notifier
                .send_user_llm_token_issued_notification(&request, email_key)
                .await
                .is_err()
            {
                return internal_error("Failed to send llm gateway token email").into_response();
            }
            Json(request).into_response()
        },
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway token request").into_response(),
    }
}
pub(crate) async fn reject_llm_gateway_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_token_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway token request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway token request").into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway token request cannot be rejected").into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway token request is already rejected").into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_token_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway token request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway token request").into_response(),
    }
}
pub(crate) async fn validate_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway account contribution request is finalized").into_response();
    }
    let action = review_queue_action(request);
    let auth = match codex_auth_from_fields(
        current.account_id.as_deref(),
        Some(&current.id_token),
        Some(&current.access_token),
        Some(&current.refresh_token),
    ) {
        Ok(auth) => auth,
        Err(response) => return response.into_response(),
    };
    let validated_auth = match validate_codex_import_auth(&state, &current.account_name, &auth)
        .await
    {
        Ok(auth) => auth,
        Err(err) => {
            let failure_reason = format!("Codex auth validation failed: {err}");
            return match state
                .admin_review_queue_store
                .fail_admin_account_contribution_request(&request_id, failure_reason, action)
                .await
            {
                Ok(Some(request)) => Json(request).into_response(),
                Ok(None) => {
                    not_found("LLM gateway account contribution request not found").into_response()
                },
                Err(_) => internal_error("Failed to fail llm gateway account contribution request")
                    .into_response(),
            };
        },
    };
    let validated_id_token = validated_auth.id_token_or_empty();
    let validated_access_token = validated_auth.access_token_or_empty();
    let validated_refresh_token = validated_auth.refresh_token_or_empty();
    match state
        .admin_review_queue_store
        .validate_admin_account_contribution_request(
            &request_id,
            validated_auth.account_id,
            validated_id_token,
            validated_access_token,
            validated_refresh_token,
            action,
        )
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to validate llm gateway account contribution request")
            .into_response(),
    }
}
pub(crate) fn account_contribution_issue_email_policy(
    request: &core_store::AdminAccountContributionRequest,
    notifier_available: bool,
) -> AccountContributionIssueEmailPolicy {
    if request.requester_email.trim().is_empty() {
        return AccountContributionIssueEmailPolicy::SkipNoRecipient;
    }
    if !notifier_available {
        return AccountContributionIssueEmailPolicy::SkipNoNotifier;
    }
    AccountContributionIssueEmailPolicy::Send
}
pub(crate) fn should_issue_account_contribution_access_artifacts(
    request: &core_store::AdminAccountContributionRequest,
) -> bool {
    !request.requester_email.trim().is_empty()
}
pub(crate) async fn approve_and_issue_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if matches!(current.status.as_str(), "issued" | "rejected") {
        return conflict("LLM gateway account contribution request is finalized").into_response();
    }
    if current.status != core_store::PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED {
        return conflict("LLM gateway account contribution request must be validated before issue")
            .into_response();
    }
    if current.issued_key_id.is_some() {
        return conflict("LLM gateway account contribution request already has an issued key")
            .into_response();
    }
    let action = review_queue_action(request);
    let imported_account_name = current
        .imported_account_name
        .clone()
        .unwrap_or_else(|| current.account_name.clone());
    let account = if current.imported_account_name.is_none() {
        let auth = match codex_auth_from_fields(
            current.account_id.as_deref(),
            Some(&current.id_token),
            Some(&current.access_token),
            Some(&current.refresh_token),
        ) {
            Ok(auth) => auth,
            Err(response) => return response.into_response(),
        };
        Some(NewAdminCodexAccount {
            name: imported_account_name.clone(),
            account_id: auth.account_id,
            auth_json: auth.auth_json,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            route_weight_tier: None,
            created_at_ms: action.updated_at_ms,
        })
    } else {
        None
    };
    let (account_group, key) = if current.issued_key_id.is_none()
        && should_issue_account_contribution_access_artifacts(&current)
    {
        let group_id = generate_id("llm-group");
        let name = format!("contrib-{}", current.request_id);
        let secret = generate_secret();
        (
            Some(NewAdminAccountGroup {
                id: group_id,
                provider_type: PROVIDER_CODEX.to_string(),
                name: name.clone(),
                account_names: vec![imported_account_name],
                created_at_ms: action.updated_at_ms,
            }),
            Some(NewAdminKey {
                id: generate_id("llm-key"),
                name,
                key_hash: sha256_hex(secret.as_bytes()),
                secret,
                provider_type: PROVIDER_CODEX.to_string(),
                protocol_family: PROTOCOL_OPENAI.to_string(),
                public_visible: false,
                quota_billable_limit: 100_000_000_000,
                request_max_concurrency: None,
                request_min_start_interval_ms: None,
                created_at_ms: action.updated_at_ms,
            }),
        )
    } else {
        (None, None)
    };
    let email_key = key.clone();
    match state
        .admin_review_queue_store
        .issue_admin_account_contribution_request(&request_id, account, account_group, key, action)
        .await
    {
        Ok(Some(request)) => {
            prime_codex_status_after_account_contribution_issue(&state, &request).await;
            match account_contribution_issue_email_policy(&request, state.email_notifier.is_some())
            {
                AccountContributionIssueEmailPolicy::SkipNoRecipient => {},
                AccountContributionIssueEmailPolicy::SkipNoNotifier => {
                    tracing::warn!(
                        request_id = %request.request_id,
                        account_name = %request.account_name,
                        "skipping issued account contribution email because email notifier is not configured",
                    );
                },
                AccountContributionIssueEmailPolicy::Send => {
                    if let (Some(email_key), Some(notifier)) =
                        (email_key.as_ref(), state.email_notifier.as_ref())
                    {
                        if let Err(err) = notifier
                            .send_user_llm_account_contribution_issued_notification(
                                &request, email_key,
                            )
                            .await
                        {
                            tracing::warn!(
                                request_id = %request.request_id,
                                account_name = %request.account_name,
                                requester_email = %request.requester_email,
                                "failed to send issued account contribution email: {err:#}",
                            );
                        }
                    }
                },
            }
            Json(request).into_response()
        },
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to issue llm gateway account contribution request")
            .into_response(),
    }
}
pub(crate) async fn prime_codex_status_after_account_contribution_issue(
    state: &HttpState,
    request: &AdminAccountContributionRequest,
) {
    let account_name = request
        .imported_account_name
        .as_deref()
        .unwrap_or(request.account_name.as_str());
    let route_store = state.provider_state.route_store();
    let refreshed = match codex_status::prime_single_codex_account_status(
        &state.admin_config_store,
        &state.admin_codex_account_store,
        &route_store,
        &state.public_status_store,
        account_name,
    )
    .await
    {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!(
                request_id = %request.request_id,
                account_name,
                "failed to prime issued Codex account status: {err:#}",
            );
            return;
        },
    };
    tracing::info!(
        request_id = %request.request_id,
        account_name,
        plan_type = refreshed.plan_type.as_deref().unwrap_or("unknown"),
        primary_remaining_percent = refreshed.primary_remaining_percent.unwrap_or_default(),
        secondary_remaining_percent = refreshed.secondary_remaining_percent.unwrap_or_default(),
        "primed issued Codex account status",
    );
}
pub(crate) async fn reject_llm_gateway_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_account_contribution_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => {
            return not_found("LLM gateway account contribution request not found").into_response()
        },
        Err(_) => {
            return internal_error("Failed to load llm gateway account contribution request")
                .into_response()
        },
    };
    if current.status == "issued" {
        return conflict("Issued LLM gateway account contribution request cannot be rejected")
            .into_response();
    }
    if current.status == "rejected" {
        return conflict("LLM gateway account contribution request is already rejected")
            .into_response();
    }
    match state
        .admin_review_queue_store
        .reject_admin_account_contribution_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway account contribution request not found").into_response(),
        Err(_) => internal_error("Failed to reject llm gateway account contribution request")
            .into_response(),
    }
}
pub(crate) async fn approve_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
    Json(request): Json<ReviewQueueActionRequest>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state
        .admin_review_queue_store
        .get_admin_sponsor_request(&request_id)
        .await
    {
        Ok(Some(request)) => request,
        Ok(None) => return not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => {
            return internal_error("Failed to load llm gateway sponsor request").into_response()
        },
    };
    if current.status == "approved" {
        return conflict("LLM gateway sponsor request is already approved").into_response();
    }
    match state
        .admin_review_queue_store
        .approve_admin_sponsor_request(&request_id, review_queue_action(request))
        .await
    {
        Ok(Some(request)) => Json(request).into_response(),
        Ok(None) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to approve llm gateway sponsor request").into_response(),
    }
}
pub(crate) async fn delete_llm_gateway_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(request_id): Path<String>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state
        .admin_review_queue_store
        .delete_admin_sponsor_request(&request_id)
        .await
    {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => not_found("LLM gateway sponsor request not found").into_response(),
        Err(_) => internal_error("Failed to delete llm gateway sponsor request").into_response(),
    }
}
pub(crate) fn normalize_review_queue_query(
    request: ListReviewQueueRequest,
) -> core_store::AdminReviewQueueQuery {
    core_store::AdminReviewQueueQuery {
        status: request
            .status
            .and_then(|status| normalize_optional_string(&status)),
        limit: request
            .limit
            .unwrap_or(DEFAULT_ADMIN_REVIEW_QUEUE_LIMIT)
            .clamp(1, MAX_ADMIN_REVIEW_QUEUE_LIMIT),
        offset: request.offset.unwrap_or(0),
    }
}
pub(crate) fn review_queue_action(request: ReviewQueueActionRequest) -> AdminReviewQueueAction {
    AdminReviewQueueAction {
        admin_note: normalize_optional_string_option(request.admin_note.as_deref()),
        updated_at_ms: now_ms(),
    }
}
