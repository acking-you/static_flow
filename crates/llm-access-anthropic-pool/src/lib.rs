//! Standard Anthropic upstream channel-pool routing and usage parsing.

use std::collections::HashMap;

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

        let mut selected_index: Option<usize> = None;
        let mut selected_weight = i128::MIN;
        for (index, channel) in channels.iter().enumerate() {
            if channel.weight == 0 {
                continue;
            }
            let current = self
                .current_weights
                .entry(channel.name.clone())
                .or_insert(0);
            *current = current.saturating_add(i128::from(channel.weight));
            if *current >= selected_weight {
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

/// Build the standard Anthropic Messages endpoint from a versioned base URL.
pub fn build_messages_url(base_url: &str) -> anyhow::Result<String> {
    let trimmed = base_url.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        anyhow::bail!("anthropic upstream base_url is empty");
    }
    let url = reqwest::Url::parse(&format!("{trimmed}/messages"))?;
    Ok(url.to_string())
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
            "alpha", "beta", "alpha", "alpha", "alpha", "beta", "alpha", "alpha"
        ]);
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
}
