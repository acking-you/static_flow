//! Codex route quota scoring and cached auth-error derivation used when
//! ordering provider routes by remaining quota.

use std::collections::BTreeMap;

use llm_access_core::store::{
    self as core_store, CodexPublicAccountStatus, CodexRateLimitStatus, ProviderCodexRoute,
};

use super::now_ms;
use crate::records::RuntimeConfigRecord;

#[derive(Debug, Clone, Copy, PartialEq)]
struct CodexRouteQuotaScore {
    rank: u8,
    remaining: f64,
    last_success_at: i64,
}

pub fn sort_codex_routes_by_cached_quota(
    routes: &mut [ProviderCodexRoute],
    status: Option<&CodexRateLimitStatus>,
    runtime_config: &RuntimeConfigRecord,
    route_weight_tiers: &BTreeMap<String, Option<String>>,
) {
    let status_by_account = status
        .map(|status| {
            status
                .accounts
                .iter()
                .map(|account| (account.name.as_str(), account))
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    routes.sort_by(|left, right| {
        let left_score = codex_route_quota_score(
            &left.account_name,
            &status_by_account,
            runtime_config,
            route_weight_tiers
                .get(&left.account_name)
                .and_then(|value| value.as_deref()),
        );
        let right_score = codex_route_quota_score(
            &right.account_name,
            &status_by_account,
            runtime_config,
            route_weight_tiers
                .get(&right.account_name)
                .and_then(|value| value.as_deref()),
        );
        right_score
            .rank
            .cmp(&left_score.rank)
            .then_with(|| right_score.remaining.total_cmp(&left_score.remaining))
            .then_with(|| right_score.last_success_at.cmp(&left_score.last_success_at))
            .then_with(|| left.account_name.cmp(&right.account_name))
    });
}

fn codex_route_quota_score(
    account_name: &str,
    status_by_account: &BTreeMap<&str, &CodexPublicAccountStatus>,
    runtime_config: &RuntimeConfigRecord,
    route_weight_tier: Option<&str>,
) -> CodexRouteQuotaScore {
    let Some(status) = status_by_account.get(account_name) else {
        return CodexRouteQuotaScore {
            rank: 2,
            remaining: -1.0,
            last_success_at: 0,
        };
    };
    if status.status != core_store::KEY_STATUS_ACTIVE || status.usage_error_message.is_some() {
        return CodexRouteQuotaScore {
            rank: 0,
            remaining: -1.0,
            last_success_at: status.last_usage_success_at.unwrap_or(0),
        };
    }
    let Some(remaining) = codex_remaining_bottleneck(status) else {
        return CodexRouteQuotaScore {
            rank: 2,
            remaining: -1.0,
            last_success_at: status.last_usage_success_at.unwrap_or(0),
        };
    };
    CodexRouteQuotaScore {
        rank: if remaining > 0.0 { 3 } else { 1 },
        remaining: remaining
            * codex_route_weight_multiplier(
                status.plan_type.as_deref(),
                route_weight_tier,
                runtime_config,
            ),
        last_success_at: status.last_usage_success_at.unwrap_or(0),
    }
}

fn codex_route_weight_multiplier(
    plan_type: Option<&str>,
    route_weight_tier: Option<&str>,
    runtime_config: &RuntimeConfigRecord,
) -> f64 {
    match codex_effective_route_weight_tier(plan_type, route_weight_tier) {
        "free" => runtime_config.codex_weight_free.max(0) as f64,
        "plus" => runtime_config.codex_weight_plus.max(0) as f64,
        "pro20x" => runtime_config.codex_weight_pro20x.max(0) as f64,
        _ => runtime_config.codex_weight_pro5x.max(0) as f64,
    }
}

fn codex_effective_route_weight_tier(
    plan_type: Option<&str>,
    route_weight_tier: Option<&str>,
) -> &'static str {
    match route_weight_tier
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("free") => "free",
        Some("plus") => "plus",
        Some("pro5x") => "pro5x",
        Some("pro20x") => "pro20x",
        Some("auto") | None | Some(_) => match plan_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .as_deref()
        {
            Some("free") => "free",
            Some("plus") => "plus",
            Some("pro20x") => "pro20x",
            Some("pro") | Some("pro5x") => "pro5x",
            _ => "free",
        },
    }
}

fn codex_remaining_bottleneck(status: &CodexPublicAccountStatus) -> Option<f64> {
    [status.primary_remaining_percent, status.secondary_remaining_percent]
        .into_iter()
        .flatten()
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
        .reduce(f64::min)
}

pub fn codex_cached_error_message(
    account_name: &str,
    record_last_error: Option<&str>,
    record_last_refresh_at_ms: Option<i64>,
    auth_refresh_enabled: bool,
    auth_json: &str,
    status_by_account: &BTreeMap<String, CodexPublicAccountStatus>,
) -> Option<String> {
    let local_auth_error =
        codex_local_auth_error_message(record_last_error, auth_refresh_enabled, auth_json);
    match status_by_account.get(account_name) {
        Some(status) => {
            if status.usage_error_message.is_some() {
                return status.usage_error_message.clone();
            }
            let local_refresh = record_last_refresh_at_ms.unwrap_or(0);
            let status_checked_at = status.last_usage_checked_at.unwrap_or(0);
            if local_refresh > status_checked_at {
                local_auth_error
            } else {
                codex_disabled_expired_auth_error(auth_refresh_enabled, auth_json)
            }
        },
        None => local_auth_error,
    }
}

fn codex_local_auth_error_message(
    record_last_error: Option<&str>,
    auth_refresh_enabled: bool,
    auth_json: &str,
) -> Option<String> {
    if auth_refresh_enabled {
        return record_last_error.map(str::to_string);
    }
    if codex_access_token_is_still_usable(auth_json) {
        return None;
    }
    record_last_error
        .map(str::to_string)
        .or_else(|| codex_disabled_expired_auth_error(auth_refresh_enabled, auth_json))
}

fn codex_disabled_expired_auth_error(
    auth_refresh_enabled: bool,
    auth_json: &str,
) -> Option<String> {
    if auth_refresh_enabled || codex_access_token_is_still_usable(auth_json) {
        return None;
    }
    Some("codex auth refresh disabled and current access token expired".to_string())
}

fn codex_access_token_is_still_usable(auth_json: &str) -> bool {
    let Some(expires_at_ms) = core_store::codex_auth_access_token_expires_at_ms(auth_json) else {
        return true;
    };
    expires_at_ms > now_ms()
}

pub fn minimal_codex_auth_json_for_access_token(access_token: Option<&str>) -> String {
    match access_token {
        Some(token) if !token.trim().is_empty() => {
            serde_json::json!({ "access_token": token }).to_string()
        },
        _ => "{}".to_string(),
    }
}
