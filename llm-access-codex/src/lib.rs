//! Codex/OpenAI-compatible request and response behavior for LLM access.

pub mod error;
pub mod instructions;
pub mod models;
pub mod request;
pub mod response;
pub mod types;

/// Client-facing Codex model id exposed by StaticFlow.
pub const GPT53_CODEX_MODEL_ID: &str = "gpt-5.3-codex";
/// Upstream model id currently used to satisfy `gpt-5.3-codex` requests.
pub const GPT53_CODEX_SPARK_MODEL_ID: &str = "gpt-5.3-codex-spark";
/// Billing multiplier for OpenAI-compatible fast/priority service tier hints.
pub const FAST_BILLABLE_MULTIPLIER: u64 = 2;
/// Maximum function/tool name length accepted by the upstream Codex wire API.
pub const MAX_OPENAI_TOOL_NAME_LEN: usize = 64;

#[cfg(test)]
mod tests {
    #[test]
    fn codex_request_and_model_helpers_are_owned_by_this_crate() {
        assert_eq!(
            crate::request::normalize_upstream_base_url("https://chatgpt.com"),
            "https://chatgpt.com/backend-api/codex"
        );
        assert_eq!(
            crate::models::append_client_version_query(
                "https://example.com/v1/models",
                "0.124.0 beta"
            ),
            "https://example.com/v1/models?client_version=0.124.0%20beta"
        );
    }
}
