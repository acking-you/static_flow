//! Standard Anthropic upstream channel-pool routing and usage parsing.

use std::{
    collections::{HashMap, HashSet},
    net::IpAddr,
};

/// Default Anthropic API version used when a caller does not supply one.
pub const ANTHROPIC_VERSION_2023_06_01: &str = "2023-06-01";

/// Apply the standard direct Anthropic authentication/version headers.
pub fn apply_anthropic_auth_headers(
    request: reqwest::RequestBuilder,
    api_key: &str,
    anthropic_version: &str,
) -> reqwest::RequestBuilder {
    request
        .header("x-api-key", api_key)
        .header("anthropic-version", anthropic_version)
}

/// Candidate channel with an admin-controlled routing weight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeightedChannel {
    /// Stable channel name.
    pub name: String,
    /// Manual routing weight. `0` drains the channel from selection.
    pub weight: u64,
}

impl WeightedChannel {
    /// Build a weighted channel candidate.
    pub fn new(name: impl Into<String>, weight: u64) -> Self {
        Self {
            name: name.into(),
            weight,
        }
    }
}

/// In-process smooth weighted round-robin scheduler.
#[derive(Debug, Default, Clone)]
pub struct SmoothWeightedRoundRobin {
    current_weights: HashMap<String, i128>,
}

impl SmoothWeightedRoundRobin {
    /// Select one eligible channel name.
    pub fn select<'a>(&mut self, channels: &'a [WeightedChannel]) -> Option<&'a str> {
        let total_weight = channels
            .iter()
            .filter(|channel| channel.weight > 0)
            .map(|channel| i128::from(channel.weight))
            .sum::<i128>();
        if total_weight <= 0 {
            return None;
        }

        let active_names = channels
            .iter()
            .filter(|channel| channel.weight > 0)
            .map(|channel| channel.name.as_str())
            .collect::<HashSet<_>>();
        self.current_weights
            .retain(|name, _| active_names.contains(name.as_str()));

        let mut selected_index: Option<usize> = None;
        let mut selected_weight = i128::MIN;
        for (index, channel) in channels.iter().enumerate() {
            if channel.weight == 0 {
                continue;
            }
            let current = match self.current_weights.get_mut(&channel.name) {
                Some(current) => current,
                None => {
                    self.current_weights.insert(channel.name.clone(), 0);
                    self.current_weights
                        .get_mut(&channel.name)
                        .expect("inserted current weight should exist")
                },
            };
            *current = current.saturating_add(i128::from(channel.weight));
            if *current > selected_weight {
                selected_weight = *current;
                selected_index = Some(index);
            }
        }

        let selected = selected_index?;
        if let Some(current) = self.current_weights.get_mut(&channels[selected].name) {
            *current = current.saturating_sub(total_weight);
        }
        Some(channels[selected].name.as_str())
    }
}

/// Token usage observed from a standard Anthropic Messages response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnthropicUsageSummary {
    /// Non-cached input plus cache-write input tokens.
    pub input_uncached_tokens: i64,
    /// Cache-read input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Whether no upstream token usage was available.
    pub usage_missing: bool,
}

impl AnthropicUsageSummary {
    /// Build a summary that explicitly marks usage as unavailable.
    pub fn missing() -> Self {
        Self {
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            usage_missing: true,
        }
    }
}

/// Parse standard Anthropic usage from either a response object or a usage
/// object.
pub fn parse_usage_from_value(value: &serde_json::Value) -> AnthropicUsageSummary {
    let usage = value
        .get("usage")
        .or_else(|| {
            value
                .get("message")
                .and_then(|message| message.get("usage"))
        })
        .unwrap_or(value);
    let input = usage_i64(usage, "input_tokens");
    let cache_creation = usage_i64(usage, "cache_creation_input_tokens");
    let cache_read = usage_i64(usage, "cache_read_input_tokens");
    let output = usage_i64(usage, "output_tokens");
    if input.is_none() && cache_creation.is_none() && cache_read.is_none() && output.is_none() {
        return AnthropicUsageSummary::missing();
    }

    AnthropicUsageSummary {
        input_uncached_tokens: input
            .unwrap_or_default()
            .saturating_add(cache_creation.unwrap_or_default()),
        input_cached_tokens: cache_read.unwrap_or_default(),
        output_tokens: output.unwrap_or_default(),
        usage_missing: false,
    }
}

/// Merge two Anthropic usage observations from response chunks.
pub fn merge_usage(
    previous: AnthropicUsageSummary,
    next: AnthropicUsageSummary,
) -> AnthropicUsageSummary {
    if previous.usage_missing {
        return next;
    }
    if next.usage_missing {
        return previous;
    }
    AnthropicUsageSummary {
        input_uncached_tokens: if next.input_uncached_tokens > 0 {
            next.input_uncached_tokens
        } else {
            previous.input_uncached_tokens
        },
        input_cached_tokens: if next.input_cached_tokens > 0 {
            next.input_cached_tokens
        } else {
            previous.input_cached_tokens
        },
        output_tokens: if next.output_tokens > 0 {
            next.output_tokens
        } else {
            previous.output_tokens
        },
        usage_missing: false,
    }
}

fn build_anthropic_endpoint_url(base_url: &str, endpoint: &str) -> anyhow::Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("anthropic upstream base_url is empty");
    }
    let url = reqwest::Url::parse(&format!("{trimmed}/{endpoint}"))?;
    if url.scheme() != "https" {
        anyhow::bail!("anthropic upstream base_url must use https");
    }
    let Some(host) = url.host_str() else {
        anyhow::bail!("anthropic upstream base_url must include a host");
    };
    let normalized_host = host.trim_end_matches('.').to_ascii_lowercase();
    if normalized_host == "localhost"
        || normalized_host.ends_with(".localhost")
        || normalized_host == "metadata.google.internal"
    {
        anyhow::bail!("anthropic upstream base_url host is not allowed");
    }
    let ip_host = normalized_host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(&normalized_host);
    if let Ok(ip) = ip_host.parse::<IpAddr>() {
        if is_private_or_loopback_ip(ip) {
            anyhow::bail!("anthropic upstream base_url host is not allowed");
        }
    }
    Ok(url.to_string())
}

/// Build the standard Anthropic Messages endpoint from a versioned base URL.
pub fn build_messages_url(base_url: &str) -> anyhow::Result<String> {
    build_anthropic_endpoint_url(base_url, "messages")
}

/// Build the standard Anthropic Models endpoint from a versioned base URL.
pub fn build_models_url(base_url: &str) -> anyhow::Result<String> {
    build_anthropic_endpoint_url(base_url, "models")
}

/// Parse model ids from the standard Anthropic Models list response.
pub fn parse_model_ids_from_models_response(body: &[u8]) -> anyhow::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_slice(body)?;
    let Some(data) = value.get("data").and_then(serde_json::Value::as_array) else {
        anyhow::bail!("anthropic models response must contain data array");
    };
    let mut model_ids = Vec::with_capacity(data.len());
    for item in data {
        let Some(model_id) = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            anyhow::bail!("anthropic models response contains model without id");
        };
        model_ids.push(model_id.to_string());
    }
    Ok(model_ids)
}

/// Return whether an IP target is local/private and must not be used as a
/// direct Anthropic upstream host.
pub fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
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

fn usage_i64(usage: &serde_json::Value, key: &str) -> Option<i64> {
    usage
        .get(key)
        .and_then(serde_json::Value::as_i64)
        .map(|value| value.max(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_weighted_round_robin_respects_manual_weights() {
        let mut scheduler = SmoothWeightedRoundRobin::default();
        let channels = vec![
            WeightedChannel::new("alpha", 3),
            WeightedChannel::new("beta", 1),
            WeightedChannel::new("zero", 0),
        ];

        let selected = (0..8)
            .map(|_| scheduler.select(&channels).expect("candidate").to_string())
            .collect::<Vec<_>>();

        assert_eq!(selected, vec![
            "alpha", "alpha", "beta", "alpha", "alpha", "alpha", "beta", "alpha"
        ]);
    }

    #[test]
    fn smooth_weighted_round_robin_drops_stale_channels_and_keeps_stable_ties() {
        let mut scheduler = SmoothWeightedRoundRobin::default();
        let initial = vec![WeightedChannel::new("alpha", 1), WeightedChannel::new("beta", 1)];

        assert_eq!(scheduler.select(&initial), Some("alpha"));
        assert_eq!(scheduler.select(&initial), Some("beta"));

        let remaining = vec![WeightedChannel::new("beta", 1)];
        assert_eq!(scheduler.select(&remaining), Some("beta"));
        assert_eq!(scheduler.current_weights.len(), 1);
        assert!(scheduler.current_weights.contains_key("beta"));

        let tied = vec![WeightedChannel::new("alpha", 1), WeightedChannel::new("beta", 1)];
        let mut fresh_scheduler = SmoothWeightedRoundRobin::default();
        assert_eq!(fresh_scheduler.select(&tied), Some("alpha"));
    }

    #[test]
    fn anthropic_usage_maps_native_non_overlapping_breakdown() {
        let usage = parse_usage_from_value(&serde_json::json!({
            "usage": {
                "input_tokens": 100,
                "cache_creation_input_tokens": 20,
                "cache_read_input_tokens": 30,
                "output_tokens": 40
            }
        }));

        assert_eq!(usage.input_uncached_tokens, 120);
        assert_eq!(usage.input_cached_tokens, 30);
        assert_eq!(usage.output_tokens, 40);
        assert!(!usage.usage_missing);
    }

    #[test]
    fn anthropic_usage_missing_does_not_fabricate_zeroes() {
        let usage = parse_usage_from_value(&serde_json::json!({"id":"msg_1"}));

        assert_eq!(usage, AnthropicUsageSummary::missing());
    }

    #[test]
    fn upstream_url_appends_messages_to_versioned_base_url() {
        assert_eq!(
            build_messages_url("https://api.anthropic.com/v1").expect("url"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            build_messages_url("https://example.com/root/").expect("url"),
            "https://example.com/root/messages"
        );
    }

    #[test]
    fn anthropic_upstream_url_appends_models_to_versioned_base_url() {
        assert_eq!(
            build_models_url("https://api.anthropic.com/v1").expect("url"),
            "https://api.anthropic.com/v1/models"
        );
        assert_eq!(
            build_models_url("https://example.com/root/").expect("url"),
            "https://example.com/root/models"
        );
    }

    #[test]
    fn upstream_url_rejects_plaintext_and_local_targets() {
        for base_url in [
            "http://api.anthropic.com/v1",
            "https://localhost/v1",
            "https://metadata.google.internal/v1",
            "https://127.0.0.1/v1",
            "https://10.0.0.1/v1",
            "https://[::1]/v1",
            "https://[fd00::1]/v1",
        ] {
            assert!(
                build_messages_url(base_url).is_err(),
                "base_url should be rejected: {base_url}"
            );
        }
    }

    #[test]
    fn parses_anthropic_upstream_models_response_ids() {
        let model_ids = parse_model_ids_from_models_response(
            br#"{"type":"list","data":[{"type":"model","id":"claude-sonnet-4-6"},{"type":"model","id":"claude-haiku-4-5"}]}"#,
        )
        .expect("models response");

        assert_eq!(model_ids, vec!["claude-sonnet-4-6", "claude-haiku-4-5"]);
    }

    #[test]
    fn rejects_malformed_anthropic_upstream_models_response() {
        for body in [
            br#"not json"#.as_slice(),
            br#"{}"#.as_slice(),
            br#"{"data":[{"type":"model"}]}"#.as_slice(),
            br#"{"data":[{"id":" "}]} "#.as_slice(),
        ] {
            assert!(
                parse_model_ids_from_models_response(body).is_err(),
                "malformed models response should be rejected"
            );
        }
    }
}
