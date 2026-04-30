//! Anthropic-compatible Kiro request and stream conversion.

pub mod converter;
pub mod stream;
pub mod types;
pub mod websearch;

use self::types::{CountTokensRequest, CountTokensResponse, Model, ModelsResponse};
use crate::token;

const SUPPORTED_MODEL_CATALOG: [(&str, &str, i64); 10] = [
    ("claude-sonnet-4-5-20250929", "Claude Sonnet 4.5", 1727568000),
    ("claude-sonnet-4-5-20250929-thinking", "Claude Sonnet 4.5 (Thinking)", 1727568000),
    ("claude-opus-4-5-20251101", "Claude Opus 4.5", 1730419200),
    ("claude-opus-4-5-20251101-thinking", "Claude Opus 4.5 (Thinking)", 1730419200),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6", 1770314400),
    ("claude-sonnet-4-6-thinking", "Claude Sonnet 4.6 (Thinking)", 1770314400),
    ("claude-opus-4-6", "Claude Opus 4.6", 1770314400),
    ("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)", 1770314400),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5", 1727740800),
    ("claude-haiku-4-5-20251001-thinking", "Claude Haiku 4.5 (Thinking)", 1727740800),
];

pub fn supported_model_ids() -> Vec<String> {
    SUPPORTED_MODEL_CATALOG
        .iter()
        .map(|(id, _, _)| (*id).to_string())
        .collect()
}

pub fn supported_models_response() -> ModelsResponse {
    ModelsResponse {
        object: "list".to_string(),
        data: SUPPORTED_MODEL_CATALOG
            .iter()
            .map(|(id, display_name, created)| model(id, display_name, *created))
            .collect(),
    }
}

pub fn count_tokens_response(payload: CountTokensRequest) -> CountTokensResponse {
    CountTokensResponse {
        input_tokens: token::count_all_tokens(
            payload.model,
            payload.system,
            payload.messages,
            payload.tools,
        ) as i32,
    }
}

fn model(id: &str, display_name: &str, created: i64) -> Model {
    Model {
        id: id.to_string(),
        object: "model".to_string(),
        created,
        owned_by: "anthropic".to_string(),
        display_name: display_name.to_string(),
        model_type: "chat".to_string(),
        max_tokens: 32_000,
    }
}
