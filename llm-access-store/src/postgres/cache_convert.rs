//! Conversions between control-plane records and request-cache value types
//! (authenticated keys, proxy configs, cached Kiro account views).

use anyhow::Context;
use llm_access_core::store::{AuthenticatedKey, ProviderProxyConfig};
use llm_access_kiro::cache_policy::{resolve_effective_kiro_cache_policy, KiroCachePolicy};

use super::{
    json::non_negative_i64_to_u64, proxy_support::resolve_provider_proxy_config_from_context,
    KiroCachedStatusParts, KiroRouteCandidateRow, ProviderProxyResolutionContext,
};
use crate::records::KeyBundle;

pub fn cached_authenticated_key_from_value(
    key: &AuthenticatedKey,
) -> crate::request_cache::CachedAuthenticatedKey {
    crate::request_cache::CachedAuthenticatedKey {
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        provider_type: key.provider_type.clone(),
        protocol_family: key.protocol_family.clone(),
        status: key.status.clone(),
        quota_billable_limit: key.quota_billable_limit,
        billable_tokens_used: key.billable_tokens_used,
    }
}

pub fn cached_authenticated_key_from_bundle(
    bundle: &KeyBundle,
) -> crate::request_cache::CachedAuthenticatedKey {
    cached_authenticated_key_from_value(&AuthenticatedKey {
        key_id: bundle.key.key_id.clone(),
        key_name: bundle.key.name.clone(),
        provider_type: bundle.key.provider_type.clone(),
        protocol_family: bundle.key.protocol_family.clone(),
        status: bundle.key.status.clone(),
        quota_billable_limit: bundle.key.quota_billable_limit,
        billable_tokens_used: bundle.rollup.billable_tokens,
    })
}

pub fn authenticated_key_from_cached(
    key: crate::request_cache::CachedAuthenticatedKey,
) -> AuthenticatedKey {
    AuthenticatedKey {
        key_id: key.key_id,
        key_name: key.key_name,
        provider_type: key.provider_type,
        protocol_family: key.protocol_family,
        status: key.status,
        quota_billable_limit: key.quota_billable_limit,
        billable_tokens_used: key.billable_tokens_used,
    }
}

pub fn cached_proxy_from_option(
    proxy: Option<ProviderProxyConfig>,
) -> Option<crate::request_cache::CachedProxyConfig> {
    proxy.map(Into::into)
}

pub fn proxy_from_cached_option(
    proxy: Option<crate::request_cache::CachedProxyConfig>,
) -> Option<ProviderProxyConfig> {
    proxy.map(Into::into)
}

pub fn build_cached_kiro_account_view(
    row: &KiroRouteCandidateRow,
    cached_status: Option<KiroCachedStatusParts>,
    proxy_context: &ProviderProxyResolutionContext,
    generation: i64,
) -> anyhow::Result<crate::request_cache::CachedKiroAccountView> {
    let cached_balance = cached_status
        .as_ref()
        .and_then(|(balance, _)| balance.as_ref());
    let routing_identity = cached_balance
        .and_then(|balance| balance.user_id.clone())
        .or_else(|| row.user_id.clone())
        .unwrap_or_else(|| row.account_name.clone());
    let proxy_mode = row.proxy_mode.clone().unwrap_or_else(|| {
        if row.proxy_config_id.is_some() {
            "fixed".to_string()
        } else {
            "inherit".to_string()
        }
    });
    let proxy_config_id = row
        .proxy_config_id
        .clone()
        .or_else(|| row.auth_proxy_config_id.clone());
    let proxy = resolve_provider_proxy_config_from_context(
        &proxy_mode,
        proxy_config_id.as_deref(),
        proxy_context,
    )?;
    Ok(crate::request_cache::CachedKiroAccountView {
        account_name: row.account_name.clone(),
        generation,
        profile_arn: row.profile_arn.clone().or(row.auth_profile_arn.clone()),
        user_id: row.user_id.clone(),
        status: row.status.clone(),
        request_max_concurrency: row.max_concurrency.and_then(non_negative_i64_to_u64),
        request_min_start_interval_ms: row.min_start_interval_ms.and_then(non_negative_i64_to_u64),
        disabled: row.disabled,
        minimum_remaining_credits_before_block: row.minimum_remaining_credits_before_block,
        api_region: row
            .api_region
            .clone()
            .unwrap_or_else(|| "us-east-1".to_string()),
        proxy: cached_proxy_from_option(proxy),
        routing_identity,
        cached_balance: cached_status
            .as_ref()
            .and_then(|(balance, _)| balance.clone()),
        cached_cache: cached_status.as_ref().map(|(_, cache)| cache.clone()),
    })
}

pub fn effective_kiro_cache_policy_json(
    runtime_policy_json: &str,
    override_json: Option<&str>,
) -> anyhow::Result<String> {
    let runtime_policy = serde_json::from_str::<KiroCachePolicy>(runtime_policy_json)
        .context("parse runtime kiro cache policy")?;
    let effective_policy = resolve_effective_kiro_cache_policy(&runtime_policy, override_json)
        .context("resolve effective kiro cache policy")?;
    serde_json::to_string(&effective_policy).context("serialize effective kiro cache policy")
}
