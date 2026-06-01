//! Codex/OpenAI-compatible request normalization.
//! ## Module map
//!
//! `request.rs` is the facade for OpenAI/Codex-compatible request handling: it
//! keeps the shared consts, two generic value helpers, and the unit tests,
//! and delegates each concern to a focused submodule:
//!
//! ```text
//!  bytes -> [prepare] read/decode/normalize
//!     +-- [policy] spark/fast/billing   +-- [chat_completions] chat->responses
//!     +-- [native_responses] repair     +-- [tools] schema + name maps
//!     +-- [normalization] url/model     +-- [last_message] preview
//!     +-- [headers] header/IP/origin    +-- [path] gateway path classify
//! ```

use serde_json::Value;

mod chat_completions;
mod headers;
mod last_message;
mod native_responses;
mod normalization;
mod path;
mod policy;
mod prepare;
mod tools;

pub use headers::{
    external_origin, extract_client_ip_from_headers, resolve_request_url_from_headers,
    serialize_headers_json,
};
pub use last_message::extract_last_message_content;
pub use normalization::normalize_upstream_base_url;
pub use policy::{
    align_responses_store_with_upstream, apply_codex_fast_policy, apply_gpt53_codex_spark_mapping,
};
pub use prepare::prepare_gateway_request_from_bytes;
pub use tools::{normalize_tool_parameters_schema, restore_openai_tool_name};

const DEFAULT_PUBLIC_GPT_MODEL_ID: &str = "gpt-5.5";
const NATIVE_RESPONSES_UPSTREAM_UNSUPPORTED_FIELDS: &[&str] = &[
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "user",
    "metadata",
    "prompt_cache_retention",
    "safety_identifier",
    "stream_options",
];
const NATIVE_RESPONSES_MESSAGE_ROLES: &[&str] = &["assistant", "system", "developer", "user"];
/// Return a non-empty trimmed JSON string field.
/// Return a non-empty trimmed JSON string field.
pub fn extract_non_empty_string(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
fn coerce_non_empty_scalar_to_string(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => {
            let trimmed = text.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        },
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(boolean) => Some(boolean.to_string()),
        _ => None,
    }
}
#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use axum::{
        body::{Body, Bytes},
        http::{header, HeaderValue, Method, StatusCode},
    };
    use serde_json::json;

    use super::{
        policy::{align_responses_store_with_upstream, apply_codex_fast_policy},
        prepare::prepare_gateway_request,
    };
    use crate::{
        instructions::codex_default_instructions,
        types::{GatewayResponseAdapter, PreparedGatewayRequest},
    };

    fn prepared_responses_request(path: &str, body: serde_json::Value) -> PreparedGatewayRequest {
        PreparedGatewayRequest {
            original_path: path.to_string(),
            upstream_path: path.to_string(),
            method: Method::POST,
            client_request_body: None,
            request_body: Bytes::from(serde_json::to_vec(&body).expect("request body json")),
            model: body
                .get("model")
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string),
            client_visible_model: None,
            wants_stream: false,
            force_upstream_stream: false,
            content_type: "application/json".to_string(),
            response_adapter: GatewayResponseAdapter::Responses,
            thread_anchor: None,
            tool_name_restore_map: Default::default(),
            billable_multiplier: 1,
            last_message_content: None,
        }
    }

    #[test]
    fn align_responses_store_with_upstream_sets_false_for_non_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "previous_response_id": "resp_1"
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), Some(&json!(false)));
        assert_eq!(body.get("previous_response_id"), None);
    }

    #[test]
    fn align_responses_store_with_upstream_removes_input_item_ids_for_non_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": [
                    {
                        "type": "message",
                        "id": "rs_item_1",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "pong"}]
                    }
                ]
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body["input"][0].get("id"), None);
        assert_eq!(body.get("store"), Some(&json!(false)));
    }

    #[test]
    fn align_responses_store_with_upstream_sets_true_for_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "store": false,
                "previous_response_id": "resp_1"
            }),
        );

        let aligned = align_responses_store_with_upstream(
            &prepared,
            "https://foo.openai.azure.com/openai/deployments/bar",
        )
        .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), Some(&json!(true)));
        assert_eq!(body.get("previous_response_id"), Some(&json!("resp_1")));
    }

    #[test]
    fn align_responses_store_with_upstream_keeps_input_item_ids_for_azure() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": [
                    {
                        "type": "message",
                        "id": "rs_item_1",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": "pong"}]
                    }
                ],
                "store": false
            }),
        );

        let aligned = align_responses_store_with_upstream(
            &prepared,
            "https://foo.openai.azure.com/openai/deployments/bar",
        )
        .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body["input"][0]["id"], json!("rs_item_1"));
        assert_eq!(body.get("store"), Some(&json!(true)));
    }

    #[test]
    fn align_responses_store_with_upstream_removes_compact_store_field() {
        let prepared = prepared_responses_request(
            "/v1/responses/compact",
            json!({
                "model": "gpt-5.3-codex",
                "input": [{
                    "type": "message",
                    "id": "rs_item_1",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "hello compact"}]
                }],
                "store": true,
                "previous_response_id": "resp_compact_1"
            }),
        );

        let aligned =
            align_responses_store_with_upstream(&prepared, "https://chatgpt.com/backend-api/codex")
                .expect("store alignment should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&aligned.request_body).expect("aligned body json");

        assert_eq!(body.get("store"), None);
        assert_eq!(body.get("previous_response_id"), None);
        assert_eq!(body["input"][0].get("id"), None);
    }

    #[test]
    fn apply_codex_fast_policy_keeps_priority_and_multiplier_when_enabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "fast"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, true).expect("fast-enabled policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), Some(&json!("priority")));
        assert_eq!(adjusted.billable_multiplier, 2);
    }

    #[test]
    fn apply_codex_fast_policy_strips_priority_and_multiplier_when_disabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "priority"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, false).expect("fast-disabled policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), None);
        assert_eq!(adjusted.billable_multiplier, 1);
    }

    #[test]
    fn apply_codex_fast_policy_leaves_flex_unchanged_when_disabled() {
        let prepared = prepared_responses_request(
            "/v1/responses",
            json!({
                "model": "gpt-5.3-codex",
                "input": "hello",
                "service_tier": "flex"
            }),
        );

        let adjusted =
            apply_codex_fast_policy(&prepared, false).expect("non-fast tier policy should succeed");
        let body: serde_json::Value =
            serde_json::from_slice(&adjusted.request_body).expect("adjusted body json");

        assert_eq!(body.get("service_tier"), Some(&json!("flex")));
        assert_eq!(adjusted.billable_multiplier, 1);
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_chat_message_without_role() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","messages":[{"content":"hello"}]}"#);

        let err = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect_err("message without role should fail");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("role"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_keeps_native_responses_body_and_last_message_content() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","input":"hello"}"#);

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert!(prepared.client_request_body.is_none());
        assert_eq!(prepared.last_message_content.as_deref(), Some("hello"));
        assert!(!prepared.wants_stream);
        assert!(prepared.force_upstream_stream);
        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][0]["content"][0]["type"], json!("input_text"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("hello"));
        assert_eq!(upstream["stream"], json!(true));
    }

    #[tokio::test]
    async fn prepare_gateway_request_injects_default_instructions_for_native_responses_when_missing(
    ) {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","input":"hello"}"#);

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_native_codex_fields() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello",
                "previous_response_id":"resp_123",
                "tools":[{"type":"function","name":"lookup","description":"Look up data.","parameters":{"type":"object","properties":{"q":{"type":"string"}}}}],
                "tool_choice":{"type":"function","name":"lookup"},
                "service_tier":"flex",
                "store":true,
                "client_metadata":{"source":"test"},
                "max_output_tokens":64,
                "max_completion_tokens":32,
                "max_tokens":16,
                "verbosity":"high"
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
        assert_eq!(upstream["service_tier"], "flex");
        assert_eq!(upstream["stream"], json!(true));
        assert_eq!(upstream["store"], json!(true));
        assert_eq!(upstream["client_metadata"], json!({"source":"test"}));
        assert_eq!(upstream["previous_response_id"], json!("resp_123"));
        assert!(upstream.get("max_output_tokens").is_none());
        assert_eq!(upstream["max_completion_tokens"], json!(32));
        assert_eq!(upstream["max_tokens"], json!(16));
        assert_eq!(upstream["verbosity"], json!("high"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_strips_temperature_before_upstream() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello",
                "stream":false,
                "temperature":0.7
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert!(prepared.force_upstream_stream);
        assert_eq!(upstream["stream"], json!(true));
        assert!(upstream.get("temperature").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_injects_default_instructions_for_bare_chat() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{"model":"gpt-5.3-codex","messages":[{"role":"user","content":"hello"}]}"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_chat_maps_system_message_to_developer_for_json_object() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "response_format":{"type":"json_object"},
                "messages":[
                    {"role":"system","content":"Return valid JSON only."},
                    {"role":"user","content":"hello"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with system json instruction should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["text"]["format"]["type"], "json_object");
        assert_eq!(upstream["input"][0]["type"], "message");
        assert_eq!(upstream["input"][0]["role"], "developer");
        assert_eq!(upstream["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "Return valid JSON only.");
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_system_message_role() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"message","role":"system","content":[{"type":"input_text","text":"Reply with exactly PONG."}]},
                    {"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request with system message should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"][0]["role"], "system");
        assert_eq!(upstream["input"][1]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_native_responses_tool_role_message() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"role":"user","content":"call lookup"},
                    {"role":"tool","tool_call_id":"call_1","content":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses tool role should be repaired before upstream dispatch");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][1]["type"], json!("function_call_output"));
        assert_eq!(upstream["input"][1]["call_id"], json!("call_1"));
        assert_eq!(upstream["input"][1]["output"], json!("{\"ok\":true}"));
        assert!(upstream["input"][1].get("role").is_none());
        assert!(upstream["input"][1].get("tool_call_id").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rewrites_native_responses_tool_role_without_call_id_to_user_message(
    ) {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"role":"tool","content":"standalone tool output"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses tool role without call id should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["role"], json!("user"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("standalone tool output"));
        assert!(upstream["input"][0].get("tool_call_id").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_skips_local_json_object_validation() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "text":{"format":{"type":"json_object"}},
                "input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"hello"}]}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["text"]["format"]["type"], "json_object");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "hello");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_tool_call_without_output() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"id":"callauto12","type":"function","function":{"name":"lookup","arguments":"{}"}}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with unmatched tool call should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_chat_tool_call_without_id() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"type":"function","function":{"name":"lookup","arguments":"{}"}}]}
                ]
            }"#,
        );

        let err = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect_err("chat request without tool call id should fail locally");

        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert!(err.message.contains("missing id"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_tool_call_without_string_function_name() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"assistant","tool_calls":[{"id":"callauto12","type":"function","function":{"name":{"bad":true},"arguments":"{}"}}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with malformed tool call should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_chat_orphan_tool_output() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[
                    {"role":"user","content":"hello"},
                    {"role":"tool","tool_call_id":"callauto12","content":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with orphan tool output should be repaired");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"].as_array().map(Vec::len), Some(1));
        assert_eq!(upstream["input"][0]["role"], "user");
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_previous_response_tool_output_delta() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "previous_response_id":"resp_1",
                "input":[
                    {"type":"function_call_output","call_id":"callauto12","output":"{\"ok\":true}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("responses request with previous_response_id should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["previous_response_id"], json!("resp_1"));
        assert_eq!(upstream["input"][0]["type"], json!("function_call_output"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_function_call_namespace() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {
                        "type":"function_call",
                        "call_id":"call_ns_1",
                        "name":"find_gameobjects",
                        "namespace":"game_tools",
                        "arguments":"{\"scene\":\"main\"}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_ns_1",
                        "output":"[]"
                    }
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should preserve namespace");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("function_call"));
        assert_eq!(upstream["input"][0]["namespace"], json!("game_tools"));
        assert_eq!(upstream["input"][0]["name"], json!("find_gameobjects"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_custom_tool_call_shape() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"custom_tool_call","call_id":"callpatch1","name":"apply_patch","arguments":"*** Begin Patch"},
                    {"type":"custom_tool_call_output","call_id":"callpatch1","output":"ok"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("custom_tool_call"));
        assert_eq!(upstream["input"][0]["call_id"], json!("callpatch1"));
        assert_eq!(upstream["input"][0]["arguments"], json!("*** Begin Patch"));
        assert!(upstream["input"][0].get("input").is_none());
        assert_eq!(upstream["input"][1]["type"], json!("custom_tool_call_output"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_repairs_function_tool_schema_missing_properties() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":"mcp__matlab__detect_matlab_toolboxes",
                        "description":"Detect installed MATLAB toolboxes.",
                        "parameters":{"type":"object"}
                    }
                }]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("tool schema without properties should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["parameters"], json!({"type":"object","properties":{}}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_coerces_chat_function_tool_scalar_name_to_string() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":123,
                        "parameters":{"type":"object","properties":{}}
                    }
                }]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with scalar function tool name should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["name"], json!("123"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_chat_function_tool_with_top_level_name() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "name":"lookup",
                    "description":"Look up data.",
                    "parameters":{"type":"object","properties":{"q":{"type":"string"}}}
                }],
                "tool_choice":{"type":"function","name":"lookup"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with responses-style function tool should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["type"], json!("function"));
        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(
            upstream["tools"][0]["parameters"],
            json!({"type":"object","properties":{"q":{"type":"string"}}})
        );
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_legacy_functions_and_function_call() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "functions":[{
                    "name":"lookup",
                    "description":"Look up data.",
                    "parameters":{"type":"object","properties":{"q":{"type":"string"}}}
                }],
                "function_call":{"name":"lookup"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("legacy chat function request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tools"][0]["type"], json!("function"));
        assert_eq!(upstream["tools"][0]["name"], json!("lookup"));
        assert_eq!(
            upstream["tools"][0]["parameters"],
            json!({"type":"object","properties":{"q":{"type":"string"}}})
        );
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_coerces_chat_function_tool_choice_scalar_name_to_string() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tool_choice":{
                    "type":"function",
                    "function":{"name":123}
                }
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("chat request with scalar function tool_choice name should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"123"}));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_unmatched_tool_calls() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"function_call","call_id":"callauto12","name":"lookup","arguments":"{}"}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["type"], json!("function_call"));
        assert_eq!(upstream["input"][0]["call_id"], json!("callauto12"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_responses_preserves_message_item_ids() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":[
                    {"type":"message","id":"item_bad","role":"assistant","content":[{"type":"output_text","text":"pong"}]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("native responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(upstream["input"][0]["id"], json!("item_bad"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_maps_to_responses() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "max_tokens":512,
                "stream":false,
                "system":"Return JSON only.",
                "tools":[{
                    "name":"lookup_weather",
                    "description":"Look up the weather.",
                    "input_schema":{
                        "type":"object",
                        "properties":{"city":{"type":"string"}},
                        "required":["city"]
                    }
                }],
                "tool_choice":{"type":"tool","name":"lookup_weather"},
                "thinking":{"type":"adaptive","budget_tokens":4096},
                "output_config":{
                    "effort":"high",
                    "format":{
                        "type":"json_schema",
                        "schema":{
                            "type":"object",
                            "properties":{"answer":{"type":"string"}},
                            "required":["answer"],
                            "additionalProperties":false
                        }
                    }
                },
                "messages":[
                    {"role":"user","content":"weather in tokyo"},
                    {"role":"assistant","content":[
                        {"type":"text","text":"Let me check."},
                        {"type":"tool_use","id":"toolu_1","name":"lookup_weather","input":{"city":"Tokyo"}}
                    ]},
                    {"role":"user","content":[
                        {"type":"tool_result","tool_use_id":"toolu_1","content":"{\"temp_c\":24}"}
                    ]}
                ]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("messages request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/responses");
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert_eq!(upstream["input"][0]["role"], "developer");
        assert_eq!(upstream["input"][0]["content"][0]["text"], "Return JSON only.");
        assert_eq!(upstream["input"][1]["role"], "user");
        assert_eq!(upstream["input"][1]["content"][0]["text"], "weather in tokyo");
        assert_eq!(upstream["input"][2]["role"], "assistant");
        assert_eq!(upstream["input"][2]["content"][0]["type"], "output_text");
        assert_eq!(upstream["input"][2]["content"][0]["text"], "Let me check.");
        assert_eq!(upstream["input"][3]["type"], "function_call");
        assert_eq!(upstream["input"][3]["call_id"], "toolu_1");
        assert_eq!(upstream["input"][3]["name"], "lookup_weather");
        assert_eq!(upstream["input"][4]["type"], "function_call_output");
        assert_eq!(upstream["input"][4]["call_id"], "toolu_1");
        assert_eq!(upstream["text"]["format"]["type"], "json_schema");
        assert_eq!(upstream["reasoning"]["effort"], "high");
        assert_eq!(upstream["tool_choice"], json!({"type":"function","name":"lookup_weather"}));
        assert_eq!(upstream["stream"], true);
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_falls_back_non_gpt_model_to_latest_gpt() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"claude-sonnet-4-6",
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("non-gpt anthropic model should fall back locally");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(prepared.client_visible_model, None);
        assert_eq!(upstream["model"], json!("gpt-5.5"));
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_maps_enabled_thinking_budget_to_xhigh() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.5",
                "thinking":{"type":"enabled","budget_tokens":24576},
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("enabled thinking request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["reasoning"]["effort"], "xhigh");
    }

    #[tokio::test]
    async fn prepare_gateway_request_anthropic_messages_normalizes_tool_input_schema() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.5",
                "tools":[{
                    "name":"inspect_file",
                    "input_schema":{
                        "type":"object",
                        "properties":{
                            "payload":{
                                "type":"object"
                            }
                        }
                    }
                }],
                "messages":[{"role":"user","content":"hello"}]
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/messages",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("tool schema request should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(
            upstream["tools"][0]["parameters"]["properties"]["payload"]["properties"],
            json!({})
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_compact_keeps_native_body_without_local_normalization() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "input":"hello compact",
                "tools":[{"type":"web_search"}],
                "parallel_tool_calls":true,
                "reasoning":{"effort":"high","summary":"auto"},
                "text":{"verbosity":"low"}
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses/compact",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("compact request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"], json!("hello compact"));
        assert_eq!(upstream["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(upstream["parallel_tool_calls"], json!(true));
        assert_eq!(upstream["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(upstream["text"], json!({"verbosity":"low"}));
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert!(
            upstream.get("stream").is_none(),
            "compact requests should not inject stream control"
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_compact_preserves_native_fields_and_history() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r#"{
                "model":"gpt-5.3-codex",
                "previous_response_id":"resp_compact_1",
                "input":[
                    {
                        "type":"function_call",
                        "call_id":"call_ns_1",
                        "name":"find_gameobjects",
                        "namespace":"game_tools",
                        "arguments":"{\"scene\":\"main\"}"
                    },
                    {
                        "type":"function_call_output",
                        "call_id":"call_ns_1",
                        "output":"[]"
                    }
                ],
                "tools":[{"type":"web_search"}],
                "parallel_tool_calls":true,
                "reasoning":{"effort":"high","summary":"auto"},
                "text":{"verbosity":"low"},
                "max_output_tokens":64,
                "store":true,
                "include":["reasoning.encrypted_content"],
                "client_metadata":{"source":"test"},
                "tool_choice":"required"
            }"#,
        );

        let prepared = prepare_gateway_request(
            "/v1/responses/compact",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("compact request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert!(upstream.get("previous_response_id").is_none());
        assert_eq!(upstream["input"][0]["namespace"], json!("game_tools"));
        assert_eq!(upstream["tools"], json!([{ "type": "web_search" }]));
        assert_eq!(upstream["parallel_tool_calls"], json!(true));
        assert_eq!(upstream["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(upstream["text"], json!({"verbosity":"low"}));
        assert!(upstream.get("max_output_tokens").is_none());
        assert_eq!(upstream["instructions"].as_str(), Some(codex_default_instructions()));
        assert!(upstream.get("store").is_none());
        assert!(upstream.get("include").is_none());
        assert!(upstream.get("client_metadata").is_none());
        assert!(upstream.get("tool_choice").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rewrites_array_style_local_schema_refs() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(
            r##"{
                "model":"gpt-5.3-codex",
                "messages":[{"role":"user","content":"hello"}],
                "tools":[{
                    "type":"function",
                    "function":{
                        "name":"ai-game-developer_assets-create-folder",
                        "parameters":{
                            "type":"object",
                            "properties":{
                                "folders":{"$ref":"#/$defs/game_tools.CreateFolderInput[]"}
                            },
                            "$defs":{
                                "game_tools.CreateFolderInput":{
                                    "type":"object",
                                    "properties":{"path":{"type":"string"}}
                                }
                            }
                        }
                    }
                }]
            }"##,
        );

        let prepared = prepare_gateway_request(
            "/v1/chat/completions",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("array-style local refs should normalize");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");
        assert_eq!(
            upstream["tools"][0]["parameters"]["properties"]["folders"],
            json!({
                "type":"array",
                "items":{"$ref":"#/$defs/game_tools.CreateFolderInput"}
            })
        );
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_memories_trace_summarize_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{"model":"gpt-5.3-codex","raw_memories":["alpha"]}"#);

        let prepared = prepare_gateway_request(
            "/v1/memories/trace_summarize",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("memory summarize request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/memories/trace_summarize");
        assert_eq!(upstream["raw_memories"], json!(["alpha"]));
        assert!(upstream.get("instructions").is_none());
        assert!(upstream.get("tools").is_none());
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_file_finalize_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body = Body::from(r#"{}"#);

        let prepared = prepare_gateway_request(
            "/v1/files/file_abc123/uploaded",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("file finalize request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/files/file_abc123/uploaded");
        assert_eq!(upstream, json!({}));
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_file_create_without_responses_defaults() {
        let headers = axum::http::HeaderMap::new();
        let body =
            Body::from(r#"{"file_name":"patch.txt","file_size":42,"use_case":"assistants"}"#);

        let prepared = prepare_gateway_request(
            "/v1/files",
            "",
            axum::http::Method::POST,
            &headers,
            body,
            1024 * 1024,
        )
        .await
        .expect("file create request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(prepared.upstream_path, "/v1/files");
        assert_eq!(upstream["file_name"], "patch.txt");
        assert_eq!(upstream["file_size"], 42);
        assert!(upstream.get("stream").is_none());
    }

    #[tokio::test]
    async fn prepare_gateway_request_rejects_nested_file_finalize_path() {
        let headers = axum::http::HeaderMap::new();
        let err = prepare_gateway_request(
            "/v1/files/a/b/uploaded",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(r#"{}"#),
            1024 * 1024,
        )
        .await
        .expect_err("nested file ids should not match the Codex finalize path");

        assert_eq!(err.status, StatusCode::METHOD_NOT_ALLOWED);
    }

    #[tokio::test]
    async fn prepare_gateway_request_accepts_realtime_sdp_without_json_parsing() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/sdp"));
        let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\n";

        let prepared = prepare_gateway_request(
            "/v1/realtime/calls",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(sdp),
            1024 * 1024,
        )
        .await
        .expect("realtime SDP request should pass through");

        assert_eq!(prepared.upstream_path, "/v1/realtime/calls");
        assert_eq!(prepared.content_type, "application/sdp");
        assert_eq!(prepared.model, None);
        assert_eq!(prepared.request_body.as_ref(), sdp.as_bytes());
    }

    #[tokio::test]
    async fn prepare_gateway_request_decodes_zstd_json_body_before_preserving_native_responses() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("zstd"));
        let compressed = zstd::stream::encode_all(
            Cursor::new(br#"{"model":"gpt-5.3-codex","input":"compressed hello"}"#),
            3,
        )
        .expect("compress request body");

        let prepared = prepare_gateway_request(
            "/v1/responses",
            "",
            axum::http::Method::POST,
            &headers,
            Body::from(compressed),
            1024 * 1024,
        )
        .await
        .expect("compressed responses request should pass through");

        let upstream: serde_json::Value =
            serde_json::from_slice(&prepared.request_body).expect("upstream body json");

        assert_eq!(upstream["input"][0]["type"], json!("message"));
        assert_eq!(upstream["input"][0]["content"][0]["text"], json!("compressed hello"));
        assert_eq!(upstream["stream"], json!(true));
        assert!(prepared.client_request_body.is_none());
        assert_eq!(prepared.last_message_content.as_deref(), Some("compressed hello"));
    }
}
