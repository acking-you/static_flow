use anyhow::Result;
use static_flow_shared::llm_gateway_store::{
    interpolate_prefix_tree_cache_ratio, merge_kiro_cache_policy,
    parse_kiro_cache_policy_override_json, KiroCachePolicy, LlmGatewayKeyRecord,
};

use crate::state::LlmGatewayRuntimeConfig;

pub(crate) fn resolve_effective_kiro_cache_policy(
    runtime: &LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> Result<KiroCachePolicy> {
    let override_policy = key
        .kiro_cache_policy_override_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_kiro_cache_policy_override_json)
        .transpose()?;
    merge_kiro_cache_policy(&runtime.kiro_cache_policy, override_policy.as_ref())
}

pub(crate) fn uses_global_kiro_cache_policy(key: &LlmGatewayKeyRecord) -> bool {
    key.kiro_cache_policy_override_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
}

/// Gates the extra diagnostic request-body snapshots. It does not control the
/// canonical `full_request_json` field that remains available on usage events.
pub(crate) fn should_capture_full_kiro_request_bodies(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> bool {
    credit_usage
        .is_some_and(|value| value.is_finite() && value > policy.high_credit_diagnostic_threshold)
}

pub(crate) fn adjust_input_tokens_for_cache_creation_cost_with_policy(
    policy: &KiroCachePolicy,
    authoritative_input_tokens: i32,
    credit_usage: Option<f64>,
    cache_estimation_enabled: bool,
) -> i32 {
    let authoritative_input_tokens = authoritative_input_tokens.max(0);
    let boost = &policy.small_input_high_credit_boost;
    if !cache_estimation_enabled || authoritative_input_tokens >= boost.target_input_tokens as i32 {
        return authoritative_input_tokens;
    }
    let Some(observed_credit) = credit_usage.filter(|value| value.is_finite()) else {
        return authoritative_input_tokens;
    };
    if observed_credit <= boost.credit_start {
        return authoritative_input_tokens;
    }
    if observed_credit >= boost.credit_end {
        return boost.target_input_tokens as i32;
    }
    let progress = ((observed_credit - boost.credit_start)
        / (boost.credit_end - boost.credit_start))
        .clamp(0.0, 1.0);
    let boosted = authoritative_input_tokens as f64
        + (boost.target_input_tokens as f64 - authoritative_input_tokens as f64) * progress;
    boosted.round() as i32
}

pub(crate) fn prefix_tree_credit_ratio_cap_basis_points_with_policy(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<u32> {
    interpolate_prefix_tree_cache_ratio(policy, credit_usage)
        .map(|ratio| (ratio.clamp(0.0, 1.0) * 10_000.0).round() as u32)
}

#[cfg(test)]
mod tests {
    use static_flow_shared::llm_gateway_store::{
        default_kiro_cache_policy, default_kiro_cache_policy_json,
    };

    use super::*;

    fn sample_runtime() -> LlmGatewayRuntimeConfig {
        LlmGatewayRuntimeConfig {
            kiro_cache_policy_json: default_kiro_cache_policy_json(),
            kiro_cache_policy: default_kiro_cache_policy(),
            ..LlmGatewayRuntimeConfig::default()
        }
    }

    fn sample_key(override_json: Option<&str>) -> LlmGatewayKeyRecord {
        LlmGatewayKeyRecord {
            id: "key".to_string(),
            name: "key".to_string(),
            secret: "secret".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: false,
            quota_billable_limit: 1,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: 0,
            updated_at: 0,
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names: None,
            account_group_id: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_cache_policy_override_json: override_json.map(ToString::to_string),
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    #[test]
    fn effective_policy_uses_key_override_for_only_changed_fields() {
        let runtime = sample_runtime();
        let key = sample_key(Some(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":80000},"high_credit_diagnostic_threshold":1.6}"#,
        ));

        let effective = resolve_effective_kiro_cache_policy(&runtime, &key).unwrap();

        assert_eq!(effective.small_input_high_credit_boost.target_input_tokens, 80_000);
        assert_eq!(effective.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(effective.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(effective.high_credit_diagnostic_threshold, 1.6);
        assert_eq!(effective.prefix_tree_credit_ratio_bands.len(), 2);
    }

    #[test]
    fn should_capture_full_kiro_request_bodies_uses_effective_threshold() {
        let runtime = sample_runtime();
        let key = sample_key(Some(r#"{"high_credit_diagnostic_threshold":1.2}"#));
        let effective = resolve_effective_kiro_cache_policy(&runtime, &key).unwrap();

        assert!(should_capture_full_kiro_request_bodies(&effective, Some(1.3)));
        assert!(!should_capture_full_kiro_request_bodies(&effective, Some(1.1)));
    }

    #[test]
    fn effective_policy_accepts_anthropic_cache_creation_input_ratio_override() {
        let runtime = sample_runtime();
        let key = sample_key(Some(r#"{"anthropic_cache_creation_input_ratio":0.25}"#));

        let effective = resolve_effective_kiro_cache_policy(&runtime, &key).unwrap();

        assert!((effective.anthropic_cache_creation_input_ratio - 0.25).abs() < f64::EPSILON);
    }
}
