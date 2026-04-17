use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use static_flow_shared::llm_gateway_store::LlmGatewayKeyRecord;

use crate::state::LlmGatewayRuntimeConfig;

fn normalized_override_json(key: &LlmGatewayKeyRecord) -> Option<&str> {
    key.kiro_billable_model_multipliers_override_json
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub(crate) fn parse_kiro_billable_model_multipliers_override_json(
    value: &str,
) -> Result<BTreeMap<String, f64>> {
    let overrides: BTreeMap<String, f64> =
        serde_json::from_str(value).map_err(|err| anyhow!("invalid json: {err}"))?;
    for (family, multiplier) in &overrides {
        if !matches!(family.as_str(), "opus" | "sonnet" | "haiku") {
            return Err(anyhow!(
                "billable multiplier family `{family}` must be one of `opus`, `sonnet`, `haiku`"
            ));
        }
        if !multiplier.is_finite() || *multiplier <= 0.0 {
            return Err(anyhow!("billable multiplier `{family}` must be a positive finite number"));
        }
    }
    Ok(overrides)
}

pub(crate) fn canonicalize_kiro_billable_model_multipliers_override_json(
    value: Option<&str>,
) -> Result<Option<String>> {
    let Some(value) = value.map(str::trim).filter(|raw| !raw.is_empty()) else {
        return Ok(None);
    };
    let overrides = parse_kiro_billable_model_multipliers_override_json(value)?;
    if overrides.is_empty() {
        Ok(None)
    } else {
        serde_json::to_string(&overrides)
            .map(Some)
            .map_err(|err| anyhow!("failed to serialize billable multiplier override: {err}"))
    }
}

pub(crate) fn resolve_effective_kiro_billable_model_multipliers(
    runtime: &LlmGatewayRuntimeConfig,
    key: &LlmGatewayKeyRecord,
) -> Result<BTreeMap<String, f64>> {
    let mut effective = runtime.kiro_billable_model_multipliers.clone();
    let override_map = normalized_override_json(key)
        .map(parse_kiro_billable_model_multipliers_override_json)
        .transpose()?;
    if let Some(override_map) = override_map {
        effective.extend(override_map);
    }
    Ok(effective)
}

pub(crate) fn uses_global_kiro_billable_model_multipliers(key: &LlmGatewayKeyRecord) -> bool {
    normalized_override_json(key).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

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
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: override_json.map(ToString::to_string),
        }
    }

    #[test]
    fn resolve_effective_kiro_billable_model_multipliers_merges_key_override() {
        let mut runtime = LlmGatewayRuntimeConfig::default();
        runtime
            .kiro_billable_model_multipliers
            .insert("opus".to_string(), 2.0);
        let key = sample_key(Some(r#"{"opus":1.5,"haiku":0.8}"#));

        let effective = resolve_effective_kiro_billable_model_multipliers(&runtime, &key)
            .expect("override should parse");

        assert_eq!(effective.get("opus"), Some(&1.5));
        assert_eq!(effective.get("haiku"), Some(&0.8));
        assert_eq!(effective.get("sonnet"), Some(&1.0));
    }
}
