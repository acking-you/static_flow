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

pub fn resolve_effective_kiro_cache_policy(
    runtime_policy: &KiroCachePolicy,
    override_json: Option<&str>,
) -> Result<KiroCachePolicy> {
    let override_policy = override_json
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(parse_kiro_cache_policy_override_json)
        .transpose()?;
    merge_kiro_cache_policy(runtime_policy, override_policy.as_ref())
}

pub fn uses_global_kiro_cache_policy(override_json: Option<&str>) -> bool {
    override_json
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
}

/// Gates the extra diagnostic request-body snapshots. It does not control the
/// canonical `full_request_json` field that remains available on usage events.
pub fn should_capture_full_kiro_request_bodies(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> bool {
    credit_usage
        .is_some_and(|value| value.is_finite() && value > policy.high_credit_diagnostic_threshold)
}

pub fn adjust_input_tokens_for_cache_creation_cost_with_policy(
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

pub fn prefix_tree_credit_ratio_cap_basis_points_with_policy(
    policy: &KiroCachePolicy,
    credit_usage: Option<f64>,
) -> Option<u32> {
    interpolate_prefix_tree_cache_ratio(policy, credit_usage)
        .map(|ratio| (ratio.clamp(0.0, 1.0) * 10_000.0).round() as u32)
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
    use super::*;

    #[test]
    fn effective_policy_uses_key_override_for_only_changed_fields() {
        let policy = default_kiro_cache_policy();
        let override_json = Some(
            r#"{"small_input_high_credit_boost":{"target_input_tokens":80000},"high_credit_diagnostic_threshold":1.6}"#,
        );

        let effective = resolve_effective_kiro_cache_policy(&policy, override_json)
            .expect("partial cache policy override should resolve");

        assert_eq!(effective.small_input_high_credit_boost.target_input_tokens, 80_000);
        assert_eq!(effective.small_input_high_credit_boost.credit_start, 1.0);
        assert_eq!(effective.small_input_high_credit_boost.credit_end, 1.8);
        assert_eq!(effective.high_credit_diagnostic_threshold, 1.6);
        assert_eq!(effective.prefix_tree_credit_ratio_bands.len(), 2);
    }

    #[test]
    fn should_capture_full_kiro_request_bodies_uses_effective_threshold() {
        let policy = default_kiro_cache_policy();
        let effective = resolve_effective_kiro_cache_policy(
            &policy,
            Some(r#"{"high_credit_diagnostic_threshold":1.2}"#),
        )
        .expect("threshold cache policy override should resolve");

        assert!(should_capture_full_kiro_request_bodies(&effective, Some(1.3)));
        assert!(!should_capture_full_kiro_request_bodies(&effective, Some(1.1)));
    }

    #[test]
    fn effective_policy_accepts_anthropic_cache_creation_input_ratio_override() {
        let policy = default_kiro_cache_policy();
        let effective = resolve_effective_kiro_cache_policy(
            &policy,
            Some(r#"{"anthropic_cache_creation_input_ratio":0.25}"#),
        )
        .expect("cache creation ratio override should resolve");

        assert!((effective.anthropic_cache_creation_input_ratio - 0.25).abs() < f64::EPSILON);
    }
}
