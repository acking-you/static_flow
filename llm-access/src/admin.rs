//! Local admin compatibility endpoints for the standalone LLM access service.

use std::{collections::BTreeMap, net::IpAddr};

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use llm_access_core::store::{
    self as core_store, AdminRuntimeConfig, UpdateAdminRuntimeConfig,
    KIRO_PREFIX_CACHE_MODE_FORMULA,
};
use serde::{Deserialize, Serialize};

use crate::HttpState;

const MAX_CODEX_CLIENT_VERSION_LEN: usize = 64;
const MAX_RUNTIME_CACHE_TTL_SECONDS: u64 = 86_400;
const MIN_RUNTIME_CACHE_TTL_SECONDS: u64 = 1;
const MAX_RUNTIME_REQUEST_BODY_BYTES: u64 = 256 * 1024 * 1024;
const MIN_RUNTIME_REQUEST_BODY_BYTES: u64 = 1024;
const MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 100;
const MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 0;
const MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 240;
const MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS: u64 = 3_600;
const MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS: u64 = 60;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 16_384;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 1;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 3_600;
const MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 1_024;
const MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 256 * 1024 * 1024;
const BAND_CONTIGUITY_TOLERANCE: f64 = 1e-12;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    code: u16,
}

#[derive(Debug)]
struct AdminHttpError {
    status: StatusCode,
    message: String,
}

impl IntoResponse for AdminHttpError {
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

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCachePolicy {
    small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy,
    prefix_tree_credit_ratio_bands: Vec<KiroCreditRatioBand>,
    high_credit_diagnostic_threshold: f64,
    #[serde(default)]
    anthropic_cache_creation_input_ratio: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroSmallInputHighCreditBoostPolicy {
    target_input_tokens: u64,
    credit_start: f64,
    credit_end: f64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct KiroCreditRatioBand {
    credit_start: f64,
    credit_end: f64,
    cache_ratio_start: f64,
    cache_ratio_end: f64,
}

pub(crate) async fn get_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to load llm gateway config").into_response(),
    }
}

pub(crate) async fn post_llm_gateway_config(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<UpdateAdminRuntimeConfig>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let current = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let config = match apply_runtime_config_update(current, request) {
        Ok(config) => config,
        Err(response) => return response.into_response(),
    };
    match state
        .admin_config_store
        .update_admin_runtime_config(config)
        .await
    {
        Ok(config) => Json(config).into_response(),
        Err(_) => internal_error("Failed to update llm gateway config").into_response(),
    }
}

fn apply_runtime_config_update(
    current: AdminRuntimeConfig,
    request: UpdateAdminRuntimeConfig,
) -> Result<AdminRuntimeConfig, AdminHttpError> {
    let auth_cache_ttl_seconds = request
        .auth_cache_ttl_seconds
        .unwrap_or(current.auth_cache_ttl_seconds);
    validate_range(
        "auth_cache_ttl_seconds",
        auth_cache_ttl_seconds,
        MIN_RUNTIME_CACHE_TTL_SECONDS,
        MAX_RUNTIME_CACHE_TTL_SECONDS,
    )?;

    let max_request_body_bytes = request
        .max_request_body_bytes
        .unwrap_or(current.max_request_body_bytes);
    validate_range(
        "max_request_body_bytes",
        max_request_body_bytes,
        MIN_RUNTIME_REQUEST_BODY_BYTES,
        MAX_RUNTIME_REQUEST_BODY_BYTES,
    )?;

    let account_failure_retry_limit = request
        .account_failure_retry_limit
        .unwrap_or(current.account_failure_retry_limit);
    validate_range(
        "account_failure_retry_limit",
        account_failure_retry_limit,
        MIN_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
        MAX_RUNTIME_ACCOUNT_FAILURE_RETRY_LIMIT,
    )?;

    let codex_client_version = match request.codex_client_version.as_deref() {
        Some(value) => normalize_codex_client_version(value)
            .ok_or_else(|| bad_request("codex_client_version is invalid"))?,
        None => current.codex_client_version,
    };

    let codex_status_refresh_min_interval_seconds = request
        .codex_status_refresh_min_interval_seconds
        .unwrap_or(current.codex_status_refresh_min_interval_seconds);
    let codex_status_refresh_max_interval_seconds = request
        .codex_status_refresh_max_interval_seconds
        .unwrap_or(current.codex_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
    )?;
    let codex_status_account_jitter_max_seconds = request
        .codex_status_account_jitter_max_seconds
        .unwrap_or(current.codex_status_account_jitter_max_seconds);
    validate_max(
        "codex_status_account_jitter_max_seconds",
        codex_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let kiro_status_refresh_min_interval_seconds = request
        .kiro_status_refresh_min_interval_seconds
        .unwrap_or(current.kiro_status_refresh_min_interval_seconds);
    let kiro_status_refresh_max_interval_seconds = request
        .kiro_status_refresh_max_interval_seconds
        .unwrap_or(current.kiro_status_refresh_max_interval_seconds);
    validate_runtime_refresh_window(
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
    )?;
    let kiro_status_account_jitter_max_seconds = request
        .kiro_status_account_jitter_max_seconds
        .unwrap_or(current.kiro_status_account_jitter_max_seconds);
    validate_max(
        "kiro_status_account_jitter_max_seconds",
        kiro_status_account_jitter_max_seconds,
        MAX_RUNTIME_STATUS_ACCOUNT_JITTER_SECONDS,
    )?;

    let usage_event_flush_batch_size = request
        .usage_event_flush_batch_size
        .unwrap_or(current.usage_event_flush_batch_size);
    validate_range(
        "usage_event_flush_batch_size",
        usage_event_flush_batch_size,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_BATCH_SIZE,
    )?;
    let usage_event_flush_interval_seconds = request
        .usage_event_flush_interval_seconds
        .unwrap_or(current.usage_event_flush_interval_seconds);
    validate_range(
        "usage_event_flush_interval_seconds",
        usage_event_flush_interval_seconds,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
    )?;
    let usage_event_flush_max_buffer_bytes = request
        .usage_event_flush_max_buffer_bytes
        .unwrap_or(current.usage_event_flush_max_buffer_bytes);
    validate_range(
        "usage_event_flush_max_buffer_bytes",
        usage_event_flush_max_buffer_bytes,
        MIN_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
        MAX_RUNTIME_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
    )?;

    let kiro_cache_kmodels_json = request
        .kiro_cache_kmodels_json
        .unwrap_or(current.kiro_cache_kmodels_json);
    parse_kiro_cache_kmodels_json(&kiro_cache_kmodels_json)
        .map_err(|_| bad_request("kiro_cache_kmodels_json is invalid"))?;

    let kiro_billable_model_multipliers_json = match request.kiro_billable_model_multipliers_json {
        Some(value) => {
            let multipliers = parse_kiro_billable_model_multipliers_json(&value)
                .map_err(|_| bad_request("kiro_billable_model_multipliers_json is invalid"))?;
            serde_json::to_string(&multipliers).map_err(|_| {
                internal_error("Failed to normalize kiro billable multiplier config")
            })?
        },
        None => current.kiro_billable_model_multipliers_json,
    };

    let kiro_cache_policy_json = request
        .kiro_cache_policy_json
        .unwrap_or(current.kiro_cache_policy_json);
    parse_kiro_cache_policy_json(&kiro_cache_policy_json)
        .map_err(|_| bad_request("kiro_cache_policy_json is invalid"))?;

    let kiro_prefix_cache_mode = request
        .kiro_prefix_cache_mode
        .unwrap_or(current.kiro_prefix_cache_mode);
    validate_kiro_prefix_cache_mode(&kiro_prefix_cache_mode)?;

    let kiro_prefix_cache_max_tokens = request
        .kiro_prefix_cache_max_tokens
        .unwrap_or(current.kiro_prefix_cache_max_tokens);
    validate_positive("kiro_prefix_cache_max_tokens", kiro_prefix_cache_max_tokens)?;
    let kiro_prefix_cache_entry_ttl_seconds = request
        .kiro_prefix_cache_entry_ttl_seconds
        .unwrap_or(current.kiro_prefix_cache_entry_ttl_seconds);
    validate_positive("kiro_prefix_cache_entry_ttl_seconds", kiro_prefix_cache_entry_ttl_seconds)?;
    let kiro_conversation_anchor_max_entries = request
        .kiro_conversation_anchor_max_entries
        .unwrap_or(current.kiro_conversation_anchor_max_entries);
    validate_positive(
        "kiro_conversation_anchor_max_entries",
        kiro_conversation_anchor_max_entries,
    )?;
    let kiro_conversation_anchor_ttl_seconds = request
        .kiro_conversation_anchor_ttl_seconds
        .unwrap_or(current.kiro_conversation_anchor_ttl_seconds);
    validate_positive(
        "kiro_conversation_anchor_ttl_seconds",
        kiro_conversation_anchor_ttl_seconds,
    )?;

    Ok(AdminRuntimeConfig {
        auth_cache_ttl_seconds,
        max_request_body_bytes,
        account_failure_retry_limit,
        codex_client_version,
        codex_status_refresh_min_interval_seconds,
        codex_status_refresh_max_interval_seconds,
        codex_status_account_jitter_max_seconds,
        kiro_status_refresh_min_interval_seconds,
        kiro_status_refresh_max_interval_seconds,
        kiro_status_account_jitter_max_seconds,
        usage_event_flush_batch_size,
        usage_event_flush_interval_seconds,
        usage_event_flush_max_buffer_bytes,
        kiro_cache_kmodels_json,
        kiro_billable_model_multipliers_json,
        kiro_cache_policy_json,
        kiro_prefix_cache_mode,
        kiro_prefix_cache_max_tokens,
        kiro_prefix_cache_entry_ttl_seconds,
        kiro_conversation_anchor_max_entries,
        kiro_conversation_anchor_ttl_seconds,
    })
}

fn ensure_admin_access(headers: &HeaderMap) -> Result<(), AdminHttpError> {
    if let Some(expected_token) = admin_token() {
        let provided = headers
            .get("x-admin-token")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
        if provided == expected_token {
            return Ok(());
        }
    }

    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        if is_local_host_header(headers) {
            return Ok(());
        }
        return Err(forbidden("Admin endpoint is local-only"));
    }
    let ip = ip
        .parse::<IpAddr>()
        .map_err(|_| forbidden("Admin endpoint is local-only"))?;
    if is_private_or_loopback_ip(ip) {
        Ok(())
    } else {
        Err(forbidden("Admin endpoint is local-only"))
    }
}

fn admin_token() -> Option<String> {
    std::env::var("LLM_ACCESS_ADMIN_TOKEN")
        .ok()
        .or_else(|| std::env::var("ADMIN_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn validate_range(field: &str, value: u64, min: u64, max: u64) -> Result<(), AdminHttpError> {
    if (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_max(field: &str, value: u64, max: u64) -> Result<(), AdminHttpError> {
    if value <= max {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} is out of range")))
    }
}

fn validate_positive(field: &str, value: u64) -> Result<(), AdminHttpError> {
    if value > 0 {
        Ok(())
    } else {
        Err(bad_request(&format!("{field} must be positive")))
    }
}

fn validate_runtime_refresh_window(
    min_seconds: u64,
    max_seconds: u64,
) -> Result<(), AdminHttpError> {
    if !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
        .contains(&min_seconds)
        || !(MIN_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS
            ..=MAX_RUNTIME_STATUS_REFRESH_INTERVAL_SECONDS)
            .contains(&max_seconds)
    {
        return Err(bad_request("refresh window seconds must be between 240 and 3600"));
    }
    if min_seconds > max_seconds {
        return Err(bad_request("refresh min interval must be less than or equal to max interval"));
    }
    Ok(())
}

fn validate_kiro_prefix_cache_mode(mode: &str) -> Result<(), AdminHttpError> {
    if matches!(mode, KIRO_PREFIX_CACHE_MODE_FORMULA | core_store::DEFAULT_KIRO_PREFIX_CACHE_MODE) {
        Ok(())
    } else {
        Err(bad_request("kiro_prefix_cache_mode is invalid"))
    }
}

fn parse_kiro_cache_kmodels_json(value: &str) -> anyhow::Result<BTreeMap<String, f64>> {
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

fn parse_kiro_billable_model_multipliers_json(
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

fn parse_kiro_cache_policy_json(value: &str) -> anyhow::Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}

fn validate_kiro_cache_policy(policy: &KiroCachePolicy) -> anyhow::Result<()> {
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

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn parse_first_ip_from_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',')
        .find_map(|part| normalize_ip_token(part.trim()))
}

fn parse_ip_from_forwarded_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    for segment in raw.split(',') {
        for pair in segment.split(';') {
            let (key, value) = pair.split_once('=')?;
            if key.trim().eq_ignore_ascii_case("for") {
                if let Some(ip) = normalize_ip_token(value.trim().trim_matches('"')) {
                    return Some(ip);
                }
            }
        }
    }
    None
}

fn normalize_ip_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    if let Some(host) = token
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    if let Some((host, _port)) = token.rsplit_once(':') {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    None
}

fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

fn is_local_host_header(headers: &HeaderMap) -> bool {
    let Some(raw_host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = raw_host.trim();
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("[::1]") {
        return true;
    }
    if let Some(host_only) = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host_only.parse::<IpAddr>() {
            return is_private_or_loopback_ip(ip);
        }
    }
    let host_only = host
        .split_once(':')
        .map(|parts| parts.0)
        .unwrap_or(host)
        .trim();
    if host_only.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host_only
        .parse::<IpAddr>()
        .map(is_private_or_loopback_ip)
        .unwrap_or(false)
}

fn bad_request(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}

fn forbidden(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::FORBIDDEN,
        message: message.to_string(),
    }
}

fn internal_error(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}
