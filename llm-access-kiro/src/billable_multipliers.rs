use std::collections::BTreeMap;

use anyhow::{anyhow, Result};

fn normalized_override_json(override_json: Option<&str>) -> Option<&str> {
    override_json
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn parse_kiro_billable_model_multipliers_override_json(
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

pub fn canonicalize_kiro_billable_model_multipliers_override_json(
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

pub fn resolve_effective_kiro_billable_model_multipliers(
    default_multipliers: &BTreeMap<String, f64>,
    override_json: Option<&str>,
) -> Result<BTreeMap<String, f64>> {
    let mut effective = default_multipliers.clone();
    let override_map = normalized_override_json(override_json)
        .map(parse_kiro_billable_model_multipliers_override_json)
        .transpose()?;
    if let Some(override_map) = override_map {
        effective.extend(override_map);
    }
    Ok(effective)
}

pub fn uses_global_kiro_billable_model_multipliers(override_json: Option<&str>) -> bool {
    normalized_override_json(override_json).is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_kiro_billable_model_multipliers() -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("haiku".to_string(), 1.0),
            ("opus".to_string(), 1.0),
            ("sonnet".to_string(), 1.0),
        ])
    }

    #[test]
    fn resolve_effective_kiro_billable_model_multipliers_merges_key_override() {
        let mut default_multipliers = default_kiro_billable_model_multipliers();
        default_multipliers.insert("opus".to_string(), 2.0);
        let override_json = Some(r#"{"opus":1.5,"haiku":0.8}"#);

        let effective =
            resolve_effective_kiro_billable_model_multipliers(&default_multipliers, override_json)
                .expect("override should parse");

        assert_eq!(effective.get("opus"), Some(&1.5));
        assert_eq!(effective.get("haiku"), Some(&0.8));
        assert_eq!(effective.get("sonnet"), Some(&1.0));
    }
}
