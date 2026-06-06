//! Input-token accounting for the Anthropic-compatible Kiro endpoint.
//!
//! Resolves the user-visible input-token count from the local request estimate
//! versus Kiro's upstream contextUsage feedback, and shapes the Anthropic
//! `usage` JSON object.

use llm_access_core::store::DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS;
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KiroInputTokenSource {
    UpstreamContextUsage,
    LocalRequestEstimateFallback,
}

/// Kiro reports bridge/system prompt scaffolding inside contextUsage. For
/// small client requests the request-side estimate is the user-visible source
/// of truth; contextUsage remains useful for large context-window requests.
pub const KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS: u64 =
    DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS;

pub fn anthropic_usage_json(
    input_tokens_total: i32,
    output_tokens: i32,
    _cache_read_input_tokens: i32,
) -> serde_json::Value {
    let input_tokens_total = input_tokens_total.max(0);
    // Kiro does not expose Anthropic-native prompt-cache metadata. Public
    // Anthropic responses should therefore report the resolved total input
    // tokens and leave cache breakdown fields at zero instead of inventing a
    // synthetic split.
    json!({
        "input_tokens": input_tokens_total,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": 0,
        "cache_read_input_tokens": 0,
        "cache_creation": {
            "ephemeral_5m_input_tokens": 0,
            "ephemeral_1h_input_tokens": 0
        },
        "output_tokens_details": {
            "thinking_tokens": 0
        },
        "service_tier": "standard",
        "inference_geo": "not_available",
    })
}

pub fn resolve_input_tokens(
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
) -> (i32, KiroInputTokenSource) {
    resolve_input_tokens_with_threshold(
        request_input_tokens,
        context_input_tokens,
        KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS,
    )
}

pub fn resolve_input_tokens_with_threshold(
    request_input_tokens: i32,
    context_input_tokens: Option<i32>,
    context_usage_min_request_tokens: u64,
) -> (i32, KiroInputTokenSource) {
    let request_input = request_input_tokens.max(0);
    if request_input as u64 <= context_usage_min_request_tokens {
        return (request_input, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    let context_input = context_input_tokens.unwrap_or_default().max(0);
    if context_input > 0 {
        (context_input, KiroInputTokenSource::UpstreamContextUsage)
    } else {
        (request_input, KiroInputTokenSource::LocalRequestEstimateFallback)
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_input_tokens, resolve_input_tokens_with_threshold, KiroInputTokenSource};

    #[test]
    fn resolve_input_tokens_prefers_request_estimate_for_small_requests() {
        let (input_tokens, source) = resolve_input_tokens(18, Some(4_118));

        assert_eq!(input_tokens, 18);
        assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    #[test]
    fn resolve_input_tokens_prefers_request_estimate_for_inflated_small_context_usage() {
        let (input_tokens, source) = resolve_input_tokens(148, Some(8_008));

        assert_eq!(input_tokens, 148);
        assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    #[test]
    fn resolve_input_tokens_prefers_request_estimate_for_small_request_when_context_exceeds_local()
    {
        let (input_tokens, source) = resolve_input_tokens(1_000, Some(6_000));

        assert_eq!(input_tokens, 1_000);
        assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    #[test]
    fn resolve_input_tokens_uses_context_usage_above_default_threshold() {
        let (input_tokens, source) = resolve_input_tokens(16_000, Some(20_000));

        assert_eq!(input_tokens, 20_000);
        assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
    }

    #[test]
    fn resolve_input_tokens_respects_configured_threshold() {
        let (input_tokens, source) =
            resolve_input_tokens_with_threshold(16_000, Some(20_000), 50_000);

        assert_eq!(input_tokens, 16_000);
        assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    #[test]
    fn resolve_input_tokens_keeps_upstream_context_for_large_requests() {
        let (input_tokens, source) = resolve_input_tokens(60_000, Some(90_000));

        assert_eq!(input_tokens, 90_000);
        assert_eq!(source, KiroInputTokenSource::UpstreamContextUsage);
    }

    #[test]
    fn resolve_input_tokens_falls_back_to_local_request_without_context_usage() {
        let (input_tokens, source) = resolve_input_tokens(123, None);

        assert_eq!(input_tokens, 123);
        assert_eq!(source, KiroInputTokenSource::LocalRequestEstimateFallback);
    }

    #[test]
    fn anthropic_usage_json_exposes_extended_anthropic_fields() {
        let usage = super::anthropic_usage_json(125, 16, 0);

        assert_eq!(usage["input_tokens"], 125);
        assert_eq!(usage["cache_creation_input_tokens"], 0);
        assert_eq!(usage["cache_read_input_tokens"], 0);
        assert_eq!(usage["cache_creation"]["ephemeral_5m_input_tokens"], 0);
        assert_eq!(usage["cache_creation"]["ephemeral_1h_input_tokens"], 0);
        assert_eq!(usage["output_tokens_details"]["thinking_tokens"], 0);
        assert_eq!(usage["service_tier"], "standard");
        assert_eq!(usage["inference_geo"], "not_available");
    }
}
