//! Input-token source resolution (request estimate vs. context-usage
//! feedback, with a minimum-request-tokens threshold) and Anthropic usage
//! JSON assembly.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub fn anthropic_usage_json(
    input_tokens_total: i32,
    output_tokens: i32,
    cache_read_input_tokens: i32,
) -> serde_json::Value {
    let input_tokens_total = input_tokens_total.max(0);
    let cache_read_input_tokens = cache_read_input_tokens.max(0).min(input_tokens_total);
    let non_cached_input_tokens_total = input_tokens_total.saturating_sub(cache_read_input_tokens);
    let cache_creation_input_tokens =
        if cache_read_input_tokens == 0 { non_cached_input_tokens_total / 2 } else { 0 };
    let input_tokens = non_cached_input_tokens_total.saturating_sub(cache_creation_input_tokens);
    json!({
        "input_tokens": input_tokens,
        "output_tokens": output_tokens.max(0),
        "cache_creation_input_tokens": cache_creation_input_tokens,
        "cache_read_input_tokens": cache_read_input_tokens,
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
