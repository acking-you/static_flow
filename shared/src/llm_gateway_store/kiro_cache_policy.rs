use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

const BAND_CONTIGUITY_TOLERANCE: f64 = 1e-12;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KiroCachePolicy {
    pub small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy,
    pub prefix_tree_credit_ratio_bands: Vec<KiroCreditRatioBand>,
    pub high_credit_diagnostic_threshold: f64,
    #[serde(default)]
    pub anthropic_cache_creation_input_ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KiroSmallInputHighCreditBoostPolicy {
    pub target_input_tokens: u64,
    pub credit_start: f64,
    pub credit_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KiroCreditRatioBand {
    pub credit_start: f64,
    pub credit_end: f64,
    pub cache_ratio_start: f64,
    pub cache_ratio_end: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct KiroCachePolicyOverride {
    #[serde(default)]
    pub small_input_high_credit_boost: Option<KiroSmallInputHighCreditBoostOverride>,
    #[serde(default)]
    pub prefix_tree_credit_ratio_bands: Option<Vec<KiroCreditRatioBand>>,
    #[serde(default)]
    pub high_credit_diagnostic_threshold: Option<f64>,
    #[serde(default)]
    pub anthropic_cache_creation_input_ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct KiroSmallInputHighCreditBoostOverride {
    #[serde(default)]
    pub target_input_tokens: Option<u64>,
    #[serde(default)]
    pub credit_start: Option<f64>,
    #[serde(default)]
    pub credit_end: Option<f64>,
}

pub fn default_kiro_cache_policy() -> KiroCachePolicy {
    KiroCachePolicy {
        small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy {
            target_input_tokens: 100_000,
            credit_start: 1.0,
            credit_end: 1.8,
        },
        prefix_tree_credit_ratio_bands: vec![
            KiroCreditRatioBand {
                credit_start: 0.3,
                credit_end: 1.0,
                cache_ratio_start: 0.7,
                cache_ratio_end: 0.2,
            },
            KiroCreditRatioBand {
                credit_start: 1.0,
                credit_end: 2.5,
                cache_ratio_start: 0.2,
                cache_ratio_end: 0.0,
            },
        ],
        high_credit_diagnostic_threshold: 2.0,
        anthropic_cache_creation_input_ratio: 0.0,
    }
}

pub fn default_kiro_cache_policy_json() -> String {
    serde_json::to_string(&default_kiro_cache_policy())
        .expect("default kiro cache policy should serialize")
}

pub fn parse_kiro_cache_policy_json(value: &str) -> Result<KiroCachePolicy> {
    let policy: KiroCachePolicy = serde_json::from_str(value)?;
    validate_kiro_cache_policy(&policy)?;
    Ok(policy)
}

pub fn parse_kiro_cache_policy_override_json(value: &str) -> Result<KiroCachePolicyOverride> {
    let override_policy: KiroCachePolicyOverride = serde_json::from_str(value)?;
    validate_kiro_cache_policy_override(&override_policy)?;
    Ok(override_policy)
}

pub fn merge_kiro_cache_policy(
    base: &KiroCachePolicy,
    override_policy: Option<&KiroCachePolicyOverride>,
) -> Result<KiroCachePolicy> {
    let Some(override_policy) = override_policy else {
        return Ok(base.clone());
    };

    let mut merged = base.clone();
    if let Some(boost) = override_policy.small_input_high_credit_boost.as_ref() {
        if let Some(value) = boost.target_input_tokens {
            merged.small_input_high_credit_boost.target_input_tokens = value;
        }
        if let Some(value) = boost.credit_start {
            merged.small_input_high_credit_boost.credit_start = value;
        }
        if let Some(value) = boost.credit_end {
            merged.small_input_high_credit_boost.credit_end = value;
        }
    }
    if let Some(bands) = override_policy.prefix_tree_credit_ratio_bands.clone() {
        merged.prefix_tree_credit_ratio_bands = bands;
    }
    if let Some(value) = override_policy.high_credit_diagnostic_threshold {
        merged.high_credit_diagnostic_threshold = value;
    }
    if let Some(value) = override_policy.anthropic_cache_creation_input_ratio {
        merged.anthropic_cache_creation_input_ratio = value;
    }
    validate_kiro_cache_policy(&merged)?;
    Ok(merged)
}

pub fn interpolate_prefix_tree_cache_ratio(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<f64> {
    let observed_credit = credit_usage.filter(|value| value.is_finite())?;
    let first = policy.prefix_tree_credit_ratio_bands.first()?;
    if observed_credit < first.credit_start - BAND_CONTIGUITY_TOLERANCE {
        return None;
    }
    for band in &policy.prefix_tree_credit_ratio_bands {
        let allowed_credit_start = band.credit_start - BAND_CONTIGUITY_TOLERANCE;
        if observed_credit < allowed_credit_start {
            return None;
        }
        if observed_credit <= band.credit_end {
            let effective_credit = observed_credit.max(band.credit_start);
            let progress = ((effective_credit - band.credit_start)
                / (band.credit_end - band.credit_start))
                .clamp(0.0, 1.0);
            return Some(
                band.cache_ratio_start + (band.cache_ratio_end - band.cache_ratio_start) * progress,
            );
        }
    }
    policy
        .prefix_tree_credit_ratio_bands
        .last()
        .map(|band| band.cache_ratio_end)
}

pub fn validate_kiro_cache_policy(policy: &KiroCachePolicy) -> Result<()> {
    let boost = &policy.small_input_high_credit_boost;
    if boost.target_input_tokens == 0 {
        return Err(anyhow!("small_input_high_credit_boost.target_input_tokens must be positive"));
    }
    if !boost.credit_start.is_finite()
        || !boost.credit_end.is_finite()
        || boost.credit_start >= boost.credit_end
    {
        return Err(anyhow!("small_input_high_credit_boost credit range is invalid"));
    }
    let diagnostic = policy.high_credit_diagnostic_threshold;
    if !diagnostic.is_finite() || diagnostic < 0.0 {
        return Err(anyhow!("high_credit_diagnostic_threshold must be finite and >= 0"));
    }
    let anthropic_cache_creation_input_ratio = policy.anthropic_cache_creation_input_ratio;
    if !anthropic_cache_creation_input_ratio.is_finite()
        || !(0.0..=1.0).contains(&anthropic_cache_creation_input_ratio)
    {
        return Err(anyhow!(
            "anthropic_cache_creation_input_ratio must be finite and between 0 and 1"
        ));
    }
    if policy.prefix_tree_credit_ratio_bands.is_empty() {
        return Err(anyhow!("prefix_tree_credit_ratio_bands must contain at least one band"));
    }

    let mut previous_credit_end = None;
    let mut previous_ratio_end = None;
    for (index, band) in policy.prefix_tree_credit_ratio_bands.iter().enumerate() {
        if !band.credit_start.is_finite() || !band.credit_end.is_finite() {
            return Err(anyhow!(
                "prefix_tree_credit_ratio_bands[{}] credit bounds must be finite",
                index
            ));
        }
        if band.credit_start >= band.credit_end {
            return Err(anyhow!(
                "prefix_tree_credit_ratio_bands[{}] credit_start must be < credit_end",
                index
            ));
        }
        if !band.cache_ratio_start.is_finite() || !band.cache_ratio_end.is_finite() {
            return Err(anyhow!(
                "prefix_tree_credit_ratio_bands[{}] cache ratios must be finite",
                index
            ));
        }
        if !(0.0..=1.0).contains(&band.cache_ratio_start)
            || !(0.0..=1.0).contains(&band.cache_ratio_end)
        {
            return Err(anyhow!(
                "prefix_tree_credit_ratio_bands[{}] cache ratios must be between 0 and 1",
                index
            ));
        }
        if band.cache_ratio_start < band.cache_ratio_end {
            return Err(anyhow!(
                "prefix_tree_credit_ratio_bands[{}] cache ratio must not increase within the band",
                index
            ));
        }
        if let Some(prev_end) = previous_credit_end {
            if band.credit_start < prev_end - BAND_CONTIGUITY_TOLERANCE {
                return Err(anyhow!(
                    "prefix_tree_credit_ratio_bands[{}] overlaps previous band",
                    index
                ));
            }
            if band.credit_start > prev_end + BAND_CONTIGUITY_TOLERANCE {
                return Err(anyhow!(
                    "prefix_tree_credit_ratio_bands[{}] has a gap after previous band",
                    index
                ));
            }
        }
        if let Some(prev_ratio) = previous_ratio_end {
            if band.cache_ratio_start > prev_ratio {
                return Err(anyhow!(
                    "prefix_tree_credit_ratio_bands[{}] cache ratio increases between bands",
                    index
                ));
            }
        }
        previous_credit_end = Some(band.credit_end);
        previous_ratio_end = Some(band.cache_ratio_end);
    }
    Ok(())
}

/// Validate override fields in isolation without touching a base policy.
pub fn validate_kiro_cache_policy_override(
    override_policy: &KiroCachePolicyOverride,
) -> Result<()> {
    if let Some(boost) = override_policy.small_input_high_credit_boost.as_ref() {
        if let Some(value) = boost.target_input_tokens {
            if value == 0 {
                return Err(anyhow!("target_input_tokens must be positive"));
            }
        }
        if let (Some(start), Some(end)) = (boost.credit_start, boost.credit_end) {
            if !start.is_finite() || !end.is_finite() || start >= end {
                return Err(anyhow!("small_input_high_credit_boost credit range is invalid"));
            }
        }
    }
    if let Some(value) = override_policy.high_credit_diagnostic_threshold {
        if !value.is_finite() || value < 0.0 {
            return Err(anyhow!("high_credit_diagnostic_threshold must be finite and >= 0"));
        }
    }
    if let Some(value) = override_policy.anthropic_cache_creation_input_ratio {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(anyhow!(
                "anthropic_cache_creation_input_ratio must be finite and between 0 and 1"
            ));
        }
    }
    if let Some(bands) = override_policy.prefix_tree_credit_ratio_bands.as_ref() {
        validate_kiro_cache_policy(&KiroCachePolicy {
            prefix_tree_credit_ratio_bands: bands.clone(),
            ..default_kiro_cache_policy()
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    fn assert_ratio(actual: Option<f64>, expected: f64) {
        let actual = actual.expect("expected a cache ratio");
        assert!((actual - expected).abs() < 1e-12, "expected {expected:?}, got {actual:?}");
    }

    #[test]
    fn default_policy_matches_current_hard_coded_thresholds() {
        let policy = default_kiro_cache_policy();

        assert_eq!(policy.small_input_high_credit_boost.target_input_tokens, 100_000);
        assert_eq!(policy.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(policy.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(policy.prefix_tree_credit_ratio_bands.len(), 2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].credit_start, 0.3);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].credit_end, 1.0);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].cache_ratio_start, 0.7);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[0].cache_ratio_end, 0.2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].credit_start, 1.0);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].credit_end, 2.5);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].cache_ratio_start, 0.2);
        assert_eq!(policy.prefix_tree_credit_ratio_bands[1].cache_ratio_end, 0.0);
        assert_eq!(policy.high_credit_diagnostic_threshold, 2.0);
        assert_eq!(policy.anthropic_cache_creation_input_ratio, 0.0);
    }

    #[test]
    fn merge_override_keeps_unspecified_fields_from_global_policy() {
        let global = default_kiro_cache_policy();
        let merged = merge_kiro_cache_policy(
            &global,
            Some(&KiroCachePolicyOverride {
                small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                    target_input_tokens: Some(80_000),
                    credit_start: None,
                    credit_end: None,
                }),
                prefix_tree_credit_ratio_bands: None,
                high_credit_diagnostic_threshold: Some(1.4),
                anthropic_cache_creation_input_ratio: Some(0.25),
            }),
        )
        .expect("override should merge");

        assert_eq!(merged.small_input_high_credit_boost.target_input_tokens, 80_000);
        assert_eq!(merged.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(merged.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(merged.prefix_tree_credit_ratio_bands, global.prefix_tree_credit_ratio_bands);
        assert_eq!(merged.high_credit_diagnostic_threshold, 1.4);
        assert_eq!(merged.anthropic_cache_creation_input_ratio, 0.25);
    }

    #[test]
    fn validate_policy_rejects_overlapping_credit_bands() {
        let err = validate_kiro_cache_policy(&KiroCachePolicy {
            prefix_tree_credit_ratio_bands: vec![
                KiroCreditRatioBand {
                    credit_start: 0.3,
                    credit_end: 1.0,
                    cache_ratio_start: 0.7,
                    cache_ratio_end: 0.2,
                },
                KiroCreditRatioBand {
                    credit_start: 0.9,
                    credit_end: 2.0,
                    cache_ratio_start: 0.2,
                    cache_ratio_end: 0.0,
                },
            ],
            ..default_kiro_cache_policy()
        })
        .expect_err("overlapping bands must fail");

        assert!(err.to_string().contains("overlaps"));
    }

    #[test]
    fn validate_policy_rejects_band_gaps() {
        let err = validate_kiro_cache_policy(&KiroCachePolicy {
            prefix_tree_credit_ratio_bands: vec![
                KiroCreditRatioBand {
                    credit_start: 0.3,
                    credit_end: 1.0,
                    cache_ratio_start: 0.7,
                    cache_ratio_end: 0.2,
                },
                KiroCreditRatioBand {
                    credit_start: 1.2,
                    credit_end: 2.5,
                    cache_ratio_start: 0.2,
                    cache_ratio_end: 0.0,
                },
            ],
            ..default_kiro_cache_policy()
        })
        .expect_err("gaps should fail");

        assert!(err.to_string().contains("gap"));
    }

    #[test]
    fn interpolate_prefix_tree_ratio_matches_current_curve_points() {
        let policy = default_kiro_cache_policy();

        assert_ratio(interpolate_prefix_tree_cache_ratio(&policy, Some(0.3)), 0.7);
        assert_ratio(interpolate_prefix_tree_cache_ratio(&policy, Some(0.65)), 0.45);
        assert_ratio(interpolate_prefix_tree_cache_ratio(&policy, Some(1.0)), 0.2);
        assert_ratio(interpolate_prefix_tree_cache_ratio(&policy, Some(1.75)), 0.1);
        assert_ratio(interpolate_prefix_tree_cache_ratio(&policy, Some(2.5)), 0.0);
    }

    #[test]
    fn interpolate_prefix_tree_ratio_returns_none_below_first_band() {
        assert_eq!(
            interpolate_prefix_tree_cache_ratio(&default_kiro_cache_policy(), Some(0.1)),
            None
        );
    }

    #[test]
    fn interpolate_prefix_tree_ratio_returns_last_ratio_above_last_band() {
        assert_ratio(
            interpolate_prefix_tree_cache_ratio(&default_kiro_cache_policy(), Some(3.0)),
            0.0,
        );
    }

    #[test]
    fn interpolate_prefix_tree_ratio_returns_none_inside_gap() {
        let policy = KiroCachePolicy {
            prefix_tree_credit_ratio_bands: vec![
                KiroCreditRatioBand {
                    credit_start: 0.3,
                    credit_end: 1.0,
                    cache_ratio_start: 0.7,
                    cache_ratio_end: 0.2,
                },
                KiroCreditRatioBand {
                    credit_start: 1.3,
                    credit_end: 2.5,
                    cache_ratio_start: 0.2,
                    cache_ratio_end: 0.0,
                },
            ],
            ..default_kiro_cache_policy()
        };

        assert_eq!(interpolate_prefix_tree_cache_ratio(&policy, Some(1.1)), None);
    }

    #[test]
    fn interpolate_prefix_tree_ratio_accepts_tolerated_boundary() {
        let policy = default_kiro_cache_policy();
        let tolerated =
            policy.prefix_tree_credit_ratio_bands[1].credit_start - BAND_CONTIGUITY_TOLERANCE / 2.0;

        assert_ratio(
            interpolate_prefix_tree_cache_ratio(&policy, Some(tolerated)),
            policy.prefix_tree_credit_ratio_bands[1].cache_ratio_start,
        );
    }

    #[test]
    fn validate_override_allows_partial_small_input_boost() {
        let override_policy = KiroCachePolicyOverride {
            small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                target_input_tokens: Some(10_000),
                credit_start: None,
                credit_end: None,
            }),
            ..Default::default()
        };
        assert!(validate_kiro_cache_policy_override(&override_policy).is_ok());
    }

    #[test]
    fn merge_fails_when_override_breaks_boost_range() {
        let base = KiroCachePolicy {
            small_input_high_credit_boost: KiroSmallInputHighCreditBoostPolicy {
                target_input_tokens: 100_000,
                credit_start: 1.0,
                credit_end: 1.2,
            },
            ..default_kiro_cache_policy()
        };
        let override_policy = KiroCachePolicyOverride {
            small_input_high_credit_boost: Some(KiroSmallInputHighCreditBoostOverride {
                credit_start: Some(2.0),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert!(merge_kiro_cache_policy(&base, Some(&override_policy)).is_err());
    }

    #[test]
    fn validate_override_rejects_out_of_range_anthropic_cache_creation_input_ratio() {
        let override_policy = KiroCachePolicyOverride {
            anthropic_cache_creation_input_ratio: Some(1.5),
            ..Default::default()
        };

        assert!(validate_kiro_cache_policy_override(&override_policy).is_err());
    }

    #[test]
    fn parse_policy_rejects_unknown_field() {
        let mut value: Value =
            serde_json::from_str(&default_kiro_cache_policy_json()).expect("default json");
        value["unexpected"] = Value::Bool(true);

        assert!(parse_kiro_cache_policy_json(&value.to_string()).is_err());
    }

    #[test]
    fn parse_override_rejects_unknown_field() {
        assert!(parse_kiro_cache_policy_override_json(r#"{"unexpected":true}"#).is_err());
    }

    #[test]
    fn parse_policy_defaults_missing_anthropic_cache_creation_input_ratio_to_zero() {
        let policy = parse_kiro_cache_policy_json(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":100000,"credit_start":1.0,"credit_end":1.8},"prefix_tree_credit_ratio_bands":[{"credit_start":0.3,"credit_end":1.0,"cache_ratio_start":0.7,"cache_ratio_end":0.2},{"credit_start":1.0,"credit_end":2.5,"cache_ratio_start":0.2,"cache_ratio_end":0.0}],"high_credit_diagnostic_threshold":2.0}"#,
        )
        .expect("legacy policy json without the new field should still parse");

        assert_eq!(policy.anthropic_cache_creation_input_ratio, 0.0);
    }
}
