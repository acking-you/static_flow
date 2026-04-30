//! Re-exported Kiro cache policy helpers from the standalone LLM access
//! runtime.

use anyhow::{Context, Result};
use llm_access_kiro::cache_policy::{
    KiroCachePolicy as RuntimeKiroCachePolicy, KiroCreditRatioBand as RuntimeKiroCreditRatioBand,
    KiroSmallInputHighCreditBoostPolicy as RuntimeKiroSmallInputHighCreditBoostPolicy,
};
use static_flow_shared::llm_gateway_store::{
    KiroCachePolicy, KiroCreditRatioBand, KiroSmallInputHighCreditBoostPolicy, LlmGatewayKeyRecord,
};

use crate::state::LlmGatewayRuntimeConfig;

pub(crate) fn resolve_effective_kiro_cache_policy(
    runtime: &LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> Result<KiroCachePolicy> {
    let runtime_policy = to_runtime_policy(&runtime.kiro_cache_policy);
    let effective = llm_access_kiro::cache_policy::resolve_effective_kiro_cache_policy(
        &runtime_policy,
        key.kiro_cache_policy_override_json.as_deref(),
    )
    .context("resolve effective kiro cache policy")?;
    Ok(from_runtime_policy(effective))
}

pub(crate) fn uses_global_kiro_cache_policy(key: &LlmGatewayKeyRecord) -> bool {
    llm_access_kiro::cache_policy::uses_global_kiro_cache_policy(
        key.kiro_cache_policy_override_json.as_deref(),
    )
}

pub(crate) fn should_capture_full_kiro_request_bodies(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> bool {
    llm_access_kiro::cache_policy::should_capture_full_kiro_request_bodies(
        &to_runtime_policy(policy),
        credit_usage,
    )
}

pub(crate) fn adjust_input_tokens_for_cache_creation_cost_with_policy(
    policy: &KiroCachePolicy,
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    cache_estimation_enabled: bool,
) -> i32 {
    llm_access_kiro::cache_policy::adjust_input_tokens_for_cache_creation_cost_with_policy(
        &to_runtime_policy(policy),
        authoritative_input_tokens,
        credit_usage,
        cache_estimation_enabled,
    )
}

pub(crate) fn prefix_tree_credit_ratio_cap_basis_points_with_policy(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<u32> {
    llm_access_kiro::cache_policy::prefix_tree_credit_ratio_cap_basis_points_with_policy(
        &to_runtime_policy(policy),
        credit_usage,
    )
}

fn to_runtime_policy(policy: &KiroCachePolicy) -> RuntimeKiroCachePolicy {
    RuntimeKiroCachePolicy {
        small_input_high_credit_boost: RuntimeKiroSmallInputHighCreditBoostPolicy {
            target_input_tokens: policy.small_input_high_credit_boost.target_input_tokens,
            credit_start: policy.small_input_high_credit_boost.credit_start,
            credit_end: policy.small_input_high_credit_boost.credit_end,
        },
        prefix_tree_credit_ratio_bands: policy
            .prefix_tree_credit_ratio_bands
            .iter()
            .map(|band| RuntimeKiroCreditRatioBand {
                credit_start: band.credit_start,
                credit_end: band.credit_end,
                cache_ratio_start: band.cache_ratio_start,
                cache_ratio_end: band.cache_ratio_end,
            })
            .collect(),
        high_credit_diagnostic_threshold: policy.high_credit_diagnostic_threshold,
        anthropic_cache_creation_input_ratio: policy.anthropic_cache_creation_input_ratio,
    }
}

fn from_runtime_policy(policy: RuntimeKiroCachePolicy) -> KiroCachePolicy {
    KiroCachePolicy {
        small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy {
            target_input_tokens: policy.small_input_high_credit_boost.target_input_tokens,
            credit_start: policy.small_input_high_credit_boost.credit_start,
            credit_end: policy.small_input_high_credit_boost.credit_end,
        },
        prefix_tree_credit_ratio_bands: policy
            .prefix_tree_credit_ratio_bands
            .into_iter()
            .map(|band| KiroCreditRatioBand {
                credit_start: band.credit_start,
                credit_end: band.credit_end,
                cache_ratio_start: band.cache_ratio_start,
                cache_ratio_end: band.cache_ratio_end,
            })
            .collect(),
        high_credit_diagnostic_threshold: policy.high_credit_diagnostic_threshold,
        anthropic_cache_creation_input_ratio: policy.anthropic_cache_creation_input_ratio,
    }
}
