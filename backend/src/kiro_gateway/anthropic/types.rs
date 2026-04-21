//! Anthropic Messages API request/response types.
//!
//! Defines the wire types for the Anthropic-compatible `/v1/messages` endpoint:
//! request payloads, content blocks, tool definitions, thinking/output config,
//! and error/model response envelopes. Used by the converter and handler
//! modules.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Anthropic-style error response envelope.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetail {
    #[serde(rename = "type")]
    pub error_type: String,
    pub message: String,
}

impl ErrorResponse {
    pub fn new(error_type: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                error_type: error_type.into(),
                message: message.into(),
            },
        }
    }
}

/// A single model entry returned by the `/v1/models` endpoint.
#[derive(Debug, Serialize)]
pub struct Model {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub owned_by: String,
    pub display_name: String,
    #[serde(rename = "type")]
    pub model_type: String,
    pub max_tokens: i32,
}

/// List wrapper for the `/v1/models` response.
#[derive(Debug, Serialize)]
pub struct ModelsResponse {
    pub object: String,
    pub data: Vec<Model>,
}

// Hard cap on thinking budget tokens to prevent runaway usage.
const MAX_BUDGET_TOKENS: i32 = 24_576;

/// Extended thinking configuration from the Anthropic request.
///
/// Supports:
/// - `"enabled"`: budget-driven thinking without an explicit effort level
/// - `"adaptive"`: effort-driven thinking controlled by [`OutputConfig`]
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub thinking_type: String,
    #[serde(default = "default_budget_tokens", deserialize_with = "deserialize_budget_tokens")]
    pub budget_tokens: i32,
}

impl Thinking {
    pub fn is_enabled(&self) -> bool {
        self.thinking_type == "enabled" || self.thinking_type == "adaptive"
    }
}

fn default_budget_tokens() -> i32 {
    20_000
}

fn deserialize_budget_tokens<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = i32::deserialize(deserializer)?;
    Ok(value.min(MAX_BUDGET_TOKENS))
}

/// Output configuration controlling adaptive thinking effort.
///
/// The Anthropic-facing bridge accepts the full five-level effort ladder:
/// - `low`
/// - `medium`
/// - `high`
/// - `xhigh`
/// - `max`
///
/// Observed locally on April 20, 2026 with `claude-opus-4-6` and a fixed
/// cache-stability prompt:
///
/// | effort | non-stream thinking chars | stream thinking chars | observed effect |
/// | --- | ---: | ---: | --- |
/// | `low` | 5 | 5 | Opens the thinking channel, but may collapse to a minimal acknowledgement. |
/// | `medium` | 5 | 5 | Similar to `low` for simple prompts; do not assume visibly deeper reasoning. |
/// | `high` | 43 | 43 | Produces a short hidden rationale. |
/// | `xhigh` | 3560 | 3215 | Large jump in hidden reasoning depth. |
/// | `max` | 3303 | 3452 | Large jump in hidden reasoning depth with dense streaming deltas. |
///
/// The exact counts are prompt-dependent and can vary across runs, but the
/// local validation consistently showed a real separation between
/// `low`/`medium`, `high`, and `xhigh`/`max`.
#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OutputConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<OutputFormat>,
}

impl OutputConfig {
    pub fn effective_effort(&self) -> &str {
        self.effort.as_deref().unwrap_or("xhigh")
    }

    pub fn json_schema(&self) -> Option<&serde_json::Value> {
        self.format.as_ref().and_then(OutputFormat::json_schema)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct OutputFormat {
    #[serde(rename = "type")]
    pub format_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<serde_json::Value>,
}

impl OutputFormat {
    pub fn json_schema(&self) -> Option<&serde_json::Value> {
        (self.format_type == "json_schema")
            .then_some(self.schema.as_ref())
            .flatten()
    }
}

/// Optional request metadata (e.g. session tracking via `user_id`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Metadata {
    pub user_id: Option<String>,
}

/// Top-level Anthropic Messages API request body.
///
/// Accepts the standard Anthropic fields: model, messages, system prompt,
/// tools, thinking config, and streaming flag. The `system` field is
/// polymorphic (string or array) via a custom deserializer.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MessagesRequest {
    pub model: String,
    #[serde(rename = "max_tokens")]
    pub _max_tokens: i32,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default, deserialize_with = "deserialize_system")]
    pub system: Option<Vec<SystemMessage>>,
    pub tools: Option<Vec<Tool>>,
    #[serde(rename = "tool_choice")]
    pub _tool_choice: Option<serde_json::Value>,
    pub thinking: Option<Thinking>,
    pub output_config: Option<OutputConfig>,
    pub metadata: Option<Metadata>,
}

// Deserializes the `system` field which can be either a plain string or an
// array of `SystemMessage` objects. A plain string is wrapped into a
// single-element vec.
fn deserialize_system<'de, D>(deserializer: D) -> Result<Option<Vec<SystemMessage>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct SystemVisitor;

    impl<'de> serde::de::Visitor<'de> for SystemVisitor {
        type Value = Option<Vec<SystemMessage>>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or an array of system messages")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(Some(vec![SystemMessage {
                text: value.to_string(),
            }]))
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::SeqAccess<'de>,
        {
            let mut messages = Vec::new();
            while let Some(message) = seq.next_element()? {
                messages.push(message);
            }
            Ok(if messages.is_empty() { None } else { Some(messages) })
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: serde::de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: serde::Deserializer<'de>,
        {
            serde::de::Deserialize::deserialize(deserializer)
        }
    }

    deserializer.deserialize_any(SystemVisitor)
}

/// A single conversation message (user or assistant turn).
///
/// `content` is kept as raw JSON because it can be a plain string or an
/// array of typed content blocks.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Message {
    pub role: String,
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SystemMessage {
    pub text: String,
}

/// Tool definition from the Anthropic request.
///
/// Covers both custom tools (with `input_schema`) and built-in tool types
/// like `web_search_*` identified by `tool_type`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Tool {
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub tool_type: Option<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub input_schema: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<i32>,
}

impl Tool {
    pub fn is_web_search(&self) -> bool {
        self.tool_type
            .as_ref()
            .is_some_and(|tool_type| tool_type.starts_with("web_search"))
            || self.name == "web_search"
    }
}

/// A polymorphic content block in a message or response.
///
/// Represents text, thinking, tool_use, tool_result, or image blocks.
/// Optional fields are populated depending on `block_type`.
#[derive(Debug, Deserialize, Serialize)]
pub struct ContentBlock {
    #[serde(rename = "type")]
    pub block_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<ImageSource>,
}

/// Base64-encoded image source for inline image content blocks.
#[derive(Debug, Deserialize, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// Request body for the `/v1/messages/count_tokens` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensRequest {
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_system"
    )]
    pub system: Option<Vec<SystemMessage>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Tool>>,
}

/// Response body for the `/v1/messages/count_tokens` endpoint.
#[derive(Debug, Serialize, Deserialize)]
pub struct CountTokensResponse {
    pub input_tokens: i32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::OutputConfig;

    #[test]
    fn output_config_preserves_json_schema_format_without_default_effort() {
        let config: OutputConfig = serde_json::from_value(json!({
            "format": {
                "type": "json_schema",
                "schema": {
                    "type": "object",
                    "properties": {
                        "result": { "type": "integer" }
                    },
                    "required": ["result"],
                    "additionalProperties": false
                }
            }
        }))
        .expect("format-only output_config should deserialize");

        assert!(config.effort.is_none());
        assert_eq!(
            config
                .json_schema()
                .expect("json_schema format should be preserved")["required"],
            json!(["result"])
        );
    }
}
