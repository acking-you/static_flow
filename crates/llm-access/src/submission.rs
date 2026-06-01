//! Public unauthenticated submission endpoints.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, RwLock},
};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::store::{
    NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
    PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
    PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    geoip::GeoIpResolver,
    support::{load_support_config, render_payment_email_markdown},
    HttpState,
};

const MAX_PUBLIC_TOKEN_WISH_REASON_CHARS: usize = 4000;
const MAX_PUBLIC_TOKEN_WISH_QUOTA: u64 = 100_000_000_000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_BATCH_ITEMS: usize = 200;
const MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS: usize = 39;
const MAX_PUBLIC_SPONSOR_MESSAGE_CHARS: usize = 4000;
const MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS: usize = 80;
const PUBLIC_SUBMIT_RATE_LIMIT_SECONDS: u64 = 60;

/// In-memory per-router public submission rate-limit state.
#[derive(Default)]
pub(crate) struct PublicSubmitGuard {
    entries: RwLock<HashMap<String, i64>>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayTokenRequest {
    requested_quota_billable_limit: u64,
    request_reason: String,
    requester_email: String,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayTokenRequestResponse {
    request_id: String,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayAccountContributionRequest {
    account_name: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    requester_email: Option<String>,
    contributor_message: String,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayAccountContributionRequestResponse {
    request_id: String,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayAccountContributionBatchRequest {
    #[serde(default)]
    requester_email: Option<String>,
    contributor_message: String,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
    items: Vec<SubmitLlmGatewayAccountContributionBatchItem>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayAccountContributionBatchItem {
    account_name: String,
    #[serde(default)]
    requester_email: Option<String>,
    #[serde(default)]
    contributor_message: Option<String>,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
    #[serde(default)]
    auth_json: Option<Value>,
    #[serde(default)]
    tokens: Option<SubmitLlmGatewayAccountContributionTokens>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewayAccountContributionTokens {
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayAccountContributionBatchResponse {
    total: usize,
    created_count: usize,
    invalid_count: usize,
    conflict_count: usize,
    results: Vec<SubmitLlmGatewayAccountContributionBatchItemResponse>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewayAccountContributionBatchItemResponse {
    item_index: usize,
    account_name: String,
    status: &'static str,
    request_id: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SubmitLlmGatewaySponsorRequest {
    requester_email: String,
    sponsor_message: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    github_id: Option<String>,
    #[serde(default)]
    frontend_page_url: Option<String>,
}

#[derive(Debug, Serialize)]
struct SubmitLlmGatewaySponsorRequestResponse {
    request_id: String,
    status: &'static str,
    payment_email_sent: bool,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

pub(crate) async fn submit_public_token_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayTokenRequest>,
) -> Response {
    let request =
        match normalize_token_request(request, &headers, &state.public_submit_guard, &state.geoip)
            .await
        {
            Ok(request) => request,
            Err(err) => return err.into_response(),
        };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .create_public_token_request(request)
        .await
    {
        Ok(()) => Json(SubmitLlmGatewayTokenRequestResponse {
            request_id,
            status: PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
        })
        .into_response(),
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

pub(crate) async fn submit_public_account_contribution_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayAccountContributionRequest>,
) -> Response {
    let request = match normalize_account_contribution_request(
        request,
        &headers,
        &state.public_submit_guard,
        &state.geoip,
    )
    .await
    {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .public_account_contribution_name_exists(&request.account_name)
        .await
    {
        Ok(false) => {},
        Ok(true) => {
            return conflict("account_name already exists or is already pending review")
                .into_response()
        },
        Err(_) => return internal_error("public submission store error").into_response(),
    }
    match state
        .public_submission_store
        .create_public_account_contribution_request(request)
        .await
    {
        Ok(()) => Json(SubmitLlmGatewayAccountContributionRequestResponse {
            request_id,
            status: PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
        })
        .into_response(),
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

pub(crate) async fn submit_public_account_contribution_batch_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewayAccountContributionBatchRequest>,
) -> Response {
    if request.items.is_empty() {
        return bad_request("items must not be empty").into_response();
    }
    if request.items.len() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_BATCH_ITEMS {
        return bad_request("items is too large").into_response();
    }

    let default_requester_email = match normalize_requester_email_input(request.requester_email) {
        Ok(value) => value.unwrap_or_default(),
        Err(err) => return bad_request(&format!("invalid requester_email: {err}")).into_response(),
    };
    let default_contributor_message =
        match normalize_contributor_message_input(Some(request.contributor_message)) {
            Ok(Some(value)) => value,
            Ok(None) => return bad_request("contributor_message is required").into_response(),
            Err(err) => {
                return bad_request(&format!("invalid contributor_message: {err}")).into_response()
            },
        };
    let default_github_id = match normalize_optional_github_id_input(request.github_id) {
        Ok(value) => value,
        Err(err) => return bad_request(&format!("invalid github_id: {err}")).into_response(),
    };
    let default_frontend_page_url =
        match normalize_frontend_page_url_input(request.frontend_page_url) {
            Ok(value) => value,
            Err(err) => {
                return bad_request(&format!("invalid frontend_page_url: {err}")).into_response()
            },
        };
    let submit_context =
        match submit_context(&headers, &state.public_submit_guard, &state.geoip).await {
            Ok(context) => context,
            Err(err) => return err.into_response(),
        };

    let mut seen_names = HashSet::new();
    let mut results = Vec::with_capacity(request.items.len());
    let mut created_count = 0;
    let mut invalid_count = 0;
    let mut conflict_count = 0;

    for (item_index, item) in request.items.into_iter().enumerate() {
        let account_name = match validate_account_name(&item.account_name) {
            Ok(value) => value,
            Err(err) => {
                invalid_count += 1;
                results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                    item_index,
                    account_name: item.account_name,
                    status: "invalid",
                    request_id: None,
                    error_message: Some(err),
                });
                continue;
            },
        };

        let auth = match normalize_batch_account_contribution_auth(item.auth_json, item.tokens) {
            Ok(value) => value,
            Err(err) => {
                invalid_count += 1;
                results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                    item_index,
                    account_name,
                    status: "invalid",
                    request_id: None,
                    error_message: Some(err),
                });
                continue;
            },
        };

        let requester_email = match item.requester_email {
            Some(value) => match normalize_requester_email_input(Some(value)) {
                Ok(value) => value.unwrap_or_default(),
                Err(err) => {
                    invalid_count += 1;
                    results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                        item_index,
                        account_name,
                        status: "invalid",
                        request_id: None,
                        error_message: Some(format!("invalid requester_email: {err}")),
                    });
                    continue;
                },
            },
            None => default_requester_email.clone(),
        };
        let contributor_message = match item.contributor_message {
            Some(value) => match normalize_contributor_message_input(Some(value)) {
                Ok(Some(value)) => value,
                Ok(None) => {
                    invalid_count += 1;
                    results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                        item_index,
                        account_name,
                        status: "invalid",
                        request_id: None,
                        error_message: Some("contributor_message is required".to_string()),
                    });
                    continue;
                },
                Err(err) => {
                    invalid_count += 1;
                    results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                        item_index,
                        account_name,
                        status: "invalid",
                        request_id: None,
                        error_message: Some(format!("invalid contributor_message: {err}")),
                    });
                    continue;
                },
            },
            None => default_contributor_message.clone(),
        };
        let github_id = match item.github_id {
            Some(value) => match normalize_optional_github_id_input(Some(value)) {
                Ok(value) => value,
                Err(err) => {
                    invalid_count += 1;
                    results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                        item_index,
                        account_name,
                        status: "invalid",
                        request_id: None,
                        error_message: Some(format!("invalid github_id: {err}")),
                    });
                    continue;
                },
            },
            None => default_github_id.clone(),
        };
        let frontend_page_url = match item.frontend_page_url {
            Some(value) => match normalize_frontend_page_url_input(Some(value)) {
                Ok(value) => value,
                Err(err) => {
                    invalid_count += 1;
                    results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                        item_index,
                        account_name,
                        status: "invalid",
                        request_id: None,
                        error_message: Some(format!("invalid frontend_page_url: {err}")),
                    });
                    continue;
                },
            },
            None => default_frontend_page_url.clone(),
        };

        if !seen_names.insert(account_name.clone()) {
            conflict_count += 1;
            results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                item_index,
                account_name,
                status: "conflict",
                request_id: None,
                error_message: Some("account_name is duplicated within the batch".to_string()),
            });
            continue;
        }

        match state
            .public_submission_store
            .public_account_contribution_name_exists(&account_name)
            .await
        {
            Ok(false) => {},
            Ok(true) => {
                conflict_count += 1;
                results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                    item_index,
                    account_name,
                    status: "conflict",
                    request_id: None,
                    error_message: Some(
                        "account_name already exists or is already pending review".to_string(),
                    ),
                });
                continue;
            },
            Err(_) => return internal_error("public submission store error").into_response(),
        }

        let request = NewPublicAccountContributionRequest {
            request_id: generate_task_id("llmacct"),
            account_name: account_name.clone(),
            account_id: auth.account_id,
            id_token: auth.id_token,
            access_token: auth.access_token,
            refresh_token: auth.refresh_token,
            requester_email,
            contributor_message,
            github_id,
            frontend_page_url,
            show_on_public_wall: false,
            fingerprint: submit_context.fingerprint.clone(),
            client_ip: submit_context.client_ip.clone(),
            ip_region: submit_context.ip_region.clone(),
            created_at_ms: submit_context.now_ms,
        };
        let request_id = request.request_id.clone();
        match state
            .public_submission_store
            .create_public_account_contribution_request(request)
            .await
        {
            Ok(()) => {
                created_count += 1;
                results.push(SubmitLlmGatewayAccountContributionBatchItemResponse {
                    item_index,
                    account_name,
                    status: "pending",
                    request_id: Some(request_id),
                    error_message: None,
                });
            },
            Err(_) => return internal_error("public submission store error").into_response(),
        }
    }

    Json(SubmitLlmGatewayAccountContributionBatchResponse {
        total: results.len(),
        created_count,
        invalid_count,
        conflict_count,
        results,
    })
    .into_response()
}

pub(crate) async fn submit_public_sponsor_request(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<SubmitLlmGatewaySponsorRequest>,
) -> Response {
    let request = match normalize_sponsor_request(
        request,
        &headers,
        &state.public_submit_guard,
        &state.geoip,
    )
    .await
    {
        Ok(request) => request,
        Err(err) => return err.into_response(),
    };
    let request_id = request.request_id.clone();
    match state
        .public_submission_store
        .create_public_sponsor_request(request)
        .await
    {
        Ok(()) => {
            let (status, payment_email_sent, failure_reason) =
                match send_sponsor_payment_email(&state, &request_id).await {
                    Ok(true) => (PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, true, None),
                    Ok(false) => (
                        PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
                        false,
                        Some("email notifier is not configured".to_string()),
                    ),
                    Err(err) => {
                        (PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED, false, Some(err.to_string()))
                    },
                };
            let sent_at_ms = payment_email_sent.then(now_ms);
            if state
                .public_submission_store
                .record_public_sponsor_payment_email_result(&request_id, sent_at_ms, failure_reason)
                .await
                .is_err()
            {
                return internal_error("public submission store error").into_response();
            }
            Json(SubmitLlmGatewaySponsorRequestResponse {
                request_id,
                status,
                payment_email_sent,
            })
            .into_response()
        },
        Err(_) => internal_error("public submission store error").into_response(),
    }
}

async fn send_sponsor_payment_email(state: &HttpState, request_id: &str) -> anyhow::Result<bool> {
    let Some(notifier) = state.email_notifier.clone() else {
        return Ok(false);
    };
    let sponsor_request = state
        .admin_review_queue_store
        .get_admin_sponsor_request(request_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("sponsor request not found after create"))?;
    let config = load_support_config()?;
    let markdown = render_payment_email_markdown(&config)?;
    notifier
        .send_llm_sponsor_payment_instructions(
            &sponsor_request.requester_email,
            &config.payment_email_subject,
            &markdown,
            &config.base_dir,
            config.reply_to_email.as_deref(),
        )
        .await?;
    Ok(true)
}

async fn normalize_token_request(
    request: SubmitLlmGatewayTokenRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicTokenRequest, SubmitError> {
    if request.requested_quota_billable_limit == 0 {
        return Err(bad_request("requested_quota_billable_limit must be > 0"));
    }
    if request.requested_quota_billable_limit > MAX_PUBLIC_TOKEN_WISH_QUOTA {
        return Err(bad_request("requested_quota_billable_limit is too large"));
    }
    let request_reason = request.request_reason.trim();
    if request_reason.is_empty() {
        return Err(bad_request("request_reason is required"));
    }
    if request_reason.chars().count() > MAX_PUBLIC_TOKEN_WISH_REASON_CHARS {
        return Err(bad_request("request_reason is too long"));
    }
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicTokenRequest {
        request_id: generate_task_id("llmwish"),
        requester_email,
        requested_quota_billable_limit: request.requested_quota_billable_limit,
        request_reason: request_reason.to_string(),
        frontend_page_url,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

async fn normalize_account_contribution_request(
    request: SubmitLlmGatewayAccountContributionRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicAccountContributionRequest, SubmitError> {
    let account_name =
        validate_account_name(&request.account_name).map_err(|err| bad_request(&err))?;
    let auth = normalize_account_contribution_auth_from_fields(
        request.account_id,
        request.id_token,
        request.access_token,
        request.refresh_token,
    )
    .map_err(|err| bad_request(&err))?;
    let requester_email = normalize_requester_email_input(request.requester_email)
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .unwrap_or_default();
    let contributor_message =
        normalize_contributor_message_input(Some(request.contributor_message))
            .map_err(|err| bad_request(&format!("invalid contributor_message: {err}")))?
            .ok_or_else(|| bad_request("contributor_message is required"))?;
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request(&format!("invalid github_id: {err}")))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicAccountContributionRequest {
        request_id: generate_task_id("llmacct"),
        account_name,
        account_id: auth.account_id,
        id_token: auth.id_token,
        access_token: auth.access_token,
        refresh_token: auth.refresh_token,
        requester_email,
        contributor_message,
        github_id,
        frontend_page_url,
        show_on_public_wall: true,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

async fn normalize_sponsor_request(
    request: SubmitLlmGatewaySponsorRequest,
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<NewPublicSponsorRequest, SubmitError> {
    let requester_email = normalize_requester_email_input(Some(request.requester_email))
        .map_err(|err| bad_request(&format!("invalid requester_email: {err}")))?
        .ok_or_else(|| bad_request("requester_email is required"))?;
    let sponsor_message = request.sponsor_message.trim();
    if sponsor_message.is_empty() {
        return Err(bad_request("sponsor_message is required"));
    }
    if sponsor_message.chars().count() > MAX_PUBLIC_SPONSOR_MESSAGE_CHARS {
        return Err(bad_request("sponsor_message is too long"));
    }
    let display_name = normalize_optional_display_name_input(request.display_name)
        .map_err(|err| bad_request(&format!("invalid display_name: {err}")))?;
    let github_id = normalize_optional_github_id_input(request.github_id)
        .map_err(|err| bad_request(&format!("invalid github_id: {err}")))?;
    let frontend_page_url = normalize_frontend_page_url_input(request.frontend_page_url)
        .map_err(|err| bad_request(&format!("invalid frontend_page_url: {err}")))?;
    let submit_context = submit_context(headers, guard, geoip).await?;

    Ok(NewPublicSponsorRequest {
        request_id: generate_task_id("llmsponsor"),
        requester_email,
        sponsor_message: sponsor_message.to_string(),
        display_name,
        github_id,
        frontend_page_url,
        fingerprint: submit_context.fingerprint,
        client_ip: submit_context.client_ip,
        ip_region: submit_context.ip_region,
        created_at_ms: submit_context.now_ms,
    })
}

struct SubmitContext {
    fingerprint: String,
    client_ip: String,
    ip_region: String,
    now_ms: i64,
}

async fn submit_context(
    headers: &HeaderMap,
    guard: &Arc<PublicSubmitGuard>,
    geoip: &GeoIpResolver,
) -> Result<SubmitContext, SubmitError> {
    let now_ms = now_ms();
    let client_ip = extract_client_ip(headers);
    let fingerprint = build_client_fingerprint(headers);
    let rate_limit_key = build_submit_rate_limit_key(headers, &fingerprint);
    enforce_public_submit_rate_limit(
        guard,
        &rate_limit_key,
        now_ms,
        PUBLIC_SUBMIT_RATE_LIMIT_SECONDS,
        "llm-access public submission",
    )?;
    let ip_region = geoip.resolve_region(&client_ip).await;
    Ok(SubmitContext {
        fingerprint,
        client_ip,
        ip_region,
        now_ms,
    })
}

fn normalize_requester_email_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 254 {
                anyhow::bail!("`requester_email` must be <= 254 chars");
            }
            Ok(Some(static_flow_email::normalize_email(raw)?))
        },
        None => Ok(None),
    }
}

fn normalize_contributor_message_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_MESSAGE_CHARS {
                anyhow::bail!("contributor_message is too long");
            }
            Ok(Some(raw))
        },
        None => Ok(None),
    }
}

fn normalize_frontend_page_url_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    match normalize_optional_string(value) {
        Some(raw) => {
            if raw.chars().count() > 2000 {
                anyhow::bail!("`frontend_page_url` must be <= 2000 chars");
            }
            let parsed = Url::parse(&raw).map_err(|err| anyhow::anyhow!("invalid URL: {err}"))?;
            match parsed.scheme() {
                "http" | "https" => {},
                _ => anyhow::bail!("`frontend_page_url` must use http or https"),
            }
            if parsed.host_str().is_none() {
                anyhow::bail!("`frontend_page_url` must include a host");
            }
            Ok(Some(raw))
        },
        None => Ok(None),
    }
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

fn normalize_optional_github_id_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = normalize_optional_string(value) else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_PUBLIC_ACCOUNT_CONTRIBUTION_GITHUB_ID_CHARS {
        anyhow::bail!("github_id is too long");
    }
    if trimmed.starts_with('-') || trimmed.ends_with('-') {
        anyhow::bail!("github_id cannot start or end with `-`");
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
    {
        anyhow::bail!("github_id may contain only ASCII letters, digits, or `-`");
    }
    Ok(Some(trimmed))
}

fn normalize_optional_display_name_input(value: Option<String>) -> anyhow::Result<Option<String>> {
    let Some(trimmed) = normalize_optional_string(value) else {
        return Ok(None);
    };
    if trimmed.chars().count() > MAX_PUBLIC_SPONSOR_DISPLAY_NAME_CHARS {
        anyhow::bail!("display_name is too long");
    }
    Ok(Some(trimmed))
}

fn validate_account_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("account name is required".to_string());
    }
    if trimmed.len() > 64 {
        return Err("account name must be 64 characters or fewer".to_string());
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
    {
        return Err("account name must contain only ASCII letters, digits, hyphens, or \
                    underscores"
            .to_string());
    }
    Ok(trimmed.to_string())
}

#[derive(Debug, Clone)]
struct NormalizedAccountContributionAuth {
    account_id: Option<String>,
    id_token: String,
    access_token: String,
    refresh_token: String,
}

fn normalize_account_contribution_auth_from_fields(
    account_id: Option<String>,
    id_token: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
) -> Result<NormalizedAccountContributionAuth, String> {
    let refresh_token = normalize_optional_string(refresh_token)
        .ok_or_else(|| "refresh_token is required".to_string())?;
    Ok(NormalizedAccountContributionAuth {
        account_id: normalize_optional_string(account_id),
        id_token: normalize_optional_string(id_token).unwrap_or_default(),
        access_token: normalize_optional_string(access_token).unwrap_or_default(),
        refresh_token,
    })
}

fn normalize_account_contribution_auth_from_json(
    value: Value,
) -> Result<NormalizedAccountContributionAuth, String> {
    if !value.is_object() {
        return Err("auth_json must be a JSON object".to_string());
    }
    normalize_account_contribution_auth_from_fields(
        optional_auth_json_string(&value, &["account_id", "accountId"]),
        optional_auth_json_string(&value, &["id_token", "idToken"]),
        optional_auth_json_string(&value, &["access_token", "accessToken"]),
        optional_auth_json_string(&value, &["refresh_token", "refreshToken"]),
    )
}

fn normalize_batch_account_contribution_auth(
    auth_json: Option<Value>,
    tokens: Option<SubmitLlmGatewayAccountContributionTokens>,
) -> Result<NormalizedAccountContributionAuth, String> {
    if let Some(auth_json) = auth_json {
        return normalize_account_contribution_auth_from_json(auth_json);
    }
    let Some(tokens) = tokens else {
        return Err("auth_json or tokens is required".to_string());
    };
    normalize_account_contribution_auth_from_fields(
        tokens.account_id,
        tokens.id_token,
        tokens.access_token,
        tokens.refresh_token,
    )
}

fn optional_auth_json_string(value: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|field| value.get(*field).and_then(Value::as_str))
        .or_else(|| {
            value.get("tokens").and_then(|tokens| {
                fields
                    .iter()
                    .find_map(|field| tokens.get(*field).and_then(Value::as_str))
            })
        })
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn build_client_fingerprint(headers: &HeaderMap) -> String {
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let raw = format!("{ip}|{user_agent}");

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn build_submit_rate_limit_key(headers: &HeaderMap, fingerprint: &str) -> String {
    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        format!("fp:{fingerprint}")
    } else {
        format!("ip:{ip}")
    }
}

fn enforce_public_submit_rate_limit(
    guard: &Arc<PublicSubmitGuard>,
    rate_limit_key: &str,
    now_ms: i64,
    rate_limit_seconds: u64,
    action_label: &str,
) -> Result<(), SubmitError> {
    let window_ms = (rate_limit_seconds.max(1) as i64) * 1_000;
    let mut writer = guard
        .entries
        .write()
        .map_err(|_| internal_error("public submit rate-limit state error"))?;
    if let Some(last) = writer.get(rate_limit_key) {
        let elapsed_ms = now_ms.saturating_sub(*last);
        if elapsed_ms < window_ms {
            let remaining_seconds = ((window_ms - elapsed_ms) + 999) / 1_000;
            return Err(SubmitError {
                status: StatusCode::TOO_MANY_REQUESTS,
                message: format!(
                    "{action_label} is rate-limited for this IP. Same IP may submit once every {} \
                     seconds. Retry in {} seconds.",
                    rate_limit_seconds.max(1),
                    remaining_seconds.max(1)
                ),
            });
        }
    }
    writer.insert(rate_limit_key.to_string(), now_ms);
    let stale_before = now_ms - window_ms * 6;
    writer.retain(|_, value| *value >= stale_before);
    Ok(())
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

fn parse_ip_from_forwarded_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(|entry| {
        entry.split(';').find_map(|segment| {
            let token = segment.trim();
            if token
                .get(..4)
                .map(|prefix| prefix.eq_ignore_ascii_case("for="))
                .unwrap_or(false)
            {
                normalize_ip_token(token)
            } else {
                None
            }
        })
    })
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    if value
        .get(..4)
        .map(|prefix| prefix.eq_ignore_ascii_case("for="))
        .unwrap_or(false)
    {
        value = value[4..].trim().trim_matches('"');
    }

    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    if let Ok(ip) = value.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

fn generate_task_id(prefix: &str) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{prefix}-{now_ms}-{nanos}")
}

fn now_ms() -> i64 {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    millis.min(i64::MAX as u128) as i64
}

#[derive(Debug)]
struct SubmitError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for SubmitError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
                code: self.status.as_u16(),
            }),
        )
            .into_response()
    }
}

fn bad_request(message: &str) -> SubmitError {
    SubmitError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn conflict(message: &str) -> SubmitError {
    SubmitError {
        status: StatusCode::CONFLICT,
        message: message.to_string(),
    }
}

fn internal_error(message: &str) -> SubmitError {
    SubmitError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn submit_context_resolves_client_ip_region() {
        let resolver = crate::geoip::GeoIpResolver::fixed_for_tests("Singapore/Singapore");
        let headers = HeaderMap::from_iter([(
            "x-forwarded-for".parse().expect("header name"),
            "208.77.246.15".parse().expect("header value"),
        )]);
        let guard = Arc::new(PublicSubmitGuard::default());

        let context = submit_context(&headers, &guard, &resolver)
            .await
            .expect("submit context");

        assert_eq!(context.client_ip, "208.77.246.15");
        assert_eq!(context.ip_region, "Singapore/Singapore");
    }

    #[tokio::test]
    async fn account_contribution_allows_optional_email_and_partial_auth() {
        let resolver = crate::geoip::GeoIpResolver::fixed_for_tests("unknown");
        let headers = HeaderMap::new();
        let guard = Arc::new(PublicSubmitGuard::default());

        let request = normalize_account_contribution_request(
            SubmitLlmGatewayAccountContributionRequest {
                account_name: "shared_account".to_string(),
                account_id: None,
                id_token: None,
                access_token: None,
                refresh_token: Some(" refresh-token ".to_string()),
                requester_email: None,
                contributor_message: "shared for validation".to_string(),
                github_id: None,
                frontend_page_url: None,
            },
            &headers,
            &guard,
            &resolver,
        )
        .await
        .expect("normalized account contribution");

        assert_eq!(request.account_name, "shared_account");
        assert_eq!(request.id_token, "");
        assert_eq!(request.access_token, "");
        assert_eq!(request.refresh_token, "refresh-token");
        assert_eq!(request.requester_email, "");
    }

    #[test]
    fn batch_account_contribution_auth_accepts_nested_tokens() {
        let auth = normalize_batch_account_contribution_auth(
            Some(json!({
                "tokens": {
                    "accountId": "acct-1",
                    "idToken": "id-token",
                    "accessToken": "access-token",
                    "refreshToken": "refresh-token"
                }
            })),
            None,
        )
        .expect("normalize auth");

        assert_eq!(auth.account_id.as_deref(), Some("acct-1"));
        assert_eq!(auth.id_token, "id-token");
        assert_eq!(auth.access_token, "access-token");
        assert_eq!(auth.refresh_token, "refresh-token");
    }
}
