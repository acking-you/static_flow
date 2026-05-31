//! Completed-Codex SSE accumulation + Codex usage/preflight recording.


use axum::http::StatusCode;
use llm_access_codex::{
    response::extract_usage_from_bytes,
    types::{PreparedGatewayRequest, UsageBreakdown},
};
use llm_access_core::{
    provider::ProviderType,
    store::{AuthenticatedKey, ControlStore, ProviderCodexRoute},
    usage::UsageEvent,
};
use serde_json::Value;

use super::{
    codex_auth::codex_protocol_family_for_endpoint,
    codex_dispatch::codex_status_from_error_json_value,
    errors::extract_error_message_from_json_value,
    usage_meta::captured_body_json,
    util::{clamp_u64_to_i64, clamp_usize_to_i64, now_millis},
    CodexPreflightFailureRecord, CompletedCodexSse, CompletedCodexSseAccumulator,
    CompletedCodexSseError, ProviderUsageMetadata, SsePayload,
};

impl CompletedCodexSseAccumulator {
    fn observe_payload(
        &mut self,
        event_type: Option<&str>,
        data: &str,
    ) -> Result<(), &'static str> {
        let mut value =
            serde_json::from_str::<Value>(data).map_err(|_| "invalid codex upstream SSE JSON")?;
        if let (Some(event_type), Some(object)) = (event_type, value.as_object_mut()) {
            object
                .entry("type")
                .or_insert_with(|| Value::String(event_type.to_string()));
        }
        if let Some(observed_usage) = extract_usage_from_bytes(data.as_bytes()) {
            self.usage = Some(observed_usage);
        }
        self.capture_failure(&value);

        match value.get("type").and_then(Value::as_str) {
            Some("response.output_item.done") => {
                if let Some(item) = value.get("item") {
                    let output_index = value
                        .get("output_index")
                        .and_then(Value::as_u64)
                        .unwrap_or(self.output_items.len() as u64);
                    self.output_items.insert(output_index, item.clone());
                }
            },
            Some("response.output_text.delta") => {
                self.capture_fallback_item_id(&value);
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    self.delta_text.push_str(delta);
                }
            },
            Some("response.output_text.done") => {
                self.capture_fallback_item_id(&value);
                if let Some(text) = value.get("text").and_then(Value::as_str) {
                    self.done_text = Some(text.to_string());
                }
            },
            Some("response.completed") => {
                self.response = Some(
                    value
                        .get("response")
                        .cloned()
                        .ok_or("codex upstream response.completed event is missing response")?,
                );
            },
            _ => {},
        }

        Ok(())
    }

    fn capture_failure(&mut self, value: &Value) {
        if self.failure.is_some() || self.response.is_some() {
            return;
        }
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let looks_like_failure = matches!(
            event_type,
            "error" | "response.error" | "response.failed" | "response.incomplete"
        ) || value.pointer("/response/error").is_some()
            || value.get("error").is_some();
        if looks_like_failure
            && extract_error_message_from_json_value(value)
                .map(|message| !message.trim().is_empty())
                .unwrap_or(false)
        {
            self.failure = Some(value.clone());
        }
    }

    fn capture_fallback_item_id(&mut self, value: &Value) {
        if self.fallback_item_id.is_none() {
            self.fallback_item_id = value
                .get("item_id")
                .and_then(Value::as_str)
                .map(ToString::to_string);
        }
    }

    fn finish(mut self) -> Result<CompletedCodexSse, CompletedCodexSseError> {
        let Some(mut response) = self.response.take() else {
            if let Some(failure) = self.failure.as_ref() {
                return Err(completed_codex_sse_error_from_value(failure));
            }
            return Err(CompletedCodexSseError {
                status: StatusCode::BAD_GATEWAY,
                message: "codex upstream SSE stream did not include response.completed".to_string(),
                body: None,
            });
        };
        self.patch_empty_completed_output(&mut response);
        Ok(CompletedCodexSse {
            response,
            usage: self.usage,
        })
    }

    fn patch_empty_completed_output(&self, response: &mut Value) {
        if response
            .get("output")
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
        {
            return;
        }

        let output = if self.output_items.is_empty() {
            let Some(text) = self
                .done_text
                .as_deref()
                .filter(|text| !text.is_empty())
                .or_else(|| (!self.delta_text.is_empty()).then_some(self.delta_text.as_str()))
            else {
                return;
            };
            let item_id = self.fallback_item_id.as_deref().unwrap_or("msg_0");
            serde_json::json!([{
                "id": item_id,
                "type": "message",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "text": text
                }]
            }])
        } else {
            Value::Array(self.output_items.values().cloned().collect())
        };

        if let Some(response) = response.as_object_mut() {
            response.insert("output".to_string(), output);
        }
    }
}
fn completed_codex_sse_error_from_value(value: &Value) -> CompletedCodexSseError {
    let message = extract_error_message_from_json_value(value)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| "Unknown upstream error".to_string());
    let status = codex_status_from_error_json_value(value).unwrap_or(StatusCode::BAD_GATEWAY);
    CompletedCodexSseError {
        status,
        message,
        body: Some(value.to_string()),
    }
}
pub fn completed_response_from_sse_bytes(
    bytes: &[u8],
) -> Result<CompletedCodexSse, CompletedCodexSseError> {
    let mut accumulator = CompletedCodexSseAccumulator::default();
    for payload in sse_payloads(bytes) {
        let data = payload.data;
        if data.trim() == "[DONE]" {
            continue;
        }
        accumulator
            .observe_payload(payload.event.as_deref(), &data)
            .map_err(|message| CompletedCodexSseError {
                status: StatusCode::BAD_GATEWAY,
                message: message.to_string(),
                body: None,
            })?;
    }
    accumulator.finish()
}
fn sse_payloads(bytes: &[u8]) -> Vec<SsePayload> {
    let text = String::from_utf8_lossy(bytes).replace("\r\n", "\n");
    text.split("\n\n")
        .filter_map(|event| {
            let event_type = event.lines().find_map(|line| {
                line.strip_prefix("event:")
                    .map(|value| value.strip_prefix(' ').unwrap_or(value).trim())
                    .filter(|value| !value.is_empty())
                    .map(ToString::to_string)
            });
            let data = event
                .lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(|line| line.strip_prefix(' ').unwrap_or(line))
                .collect::<Vec<_>>();
            if data.is_empty() {
                None
            } else {
                Some(SsePayload {
                    event: event_type,
                    data: data.join("\n"),
                })
            }
        })
        .collect()
}
pub async fn record_codex_preflight_failure(record: CodexPreflightFailureRecord<'_>) {
    record.meta.mark_stream_finish();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(record.endpoint),
        key_id: record.key.key_id.clone(),
        key_name: record.key.key_name.clone(),
        account_name: None,
        account_group_id_at_event: None,
        route_strategy_at_event: None,
        request_method: record.meta.request_method.clone(),
        request_url: record.meta.request_url.clone(),
        endpoint: record.endpoint.to_string(),
        model: record.model,
        mapped_model: None,
        status_code: i64::from(record.status.as_u16()),
        request_body_bytes: record.meta.request_body_bytes,
        quota_failover_count: record.meta.quota_failover_count,
        routing_diagnostics_json: record.meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: 0,
        input_cached_tokens: 0,
        output_tokens: 0,
        billable_tokens: 0,
        credit_usage: None,
        usage_missing: true,
        credit_usage_missing: false,
        client_ip: record.meta.client_ip.clone(),
        ip_region: record.meta.ip_region.clone(),
        request_headers_json: record.meta.request_headers_json.clone(),
        last_message_content: record.meta.last_message_content.clone(),
        client_request_body_json: captured_body_json(&record.meta.client_request_body_json),
        upstream_request_body_json: captured_body_json(&record.meta.upstream_request_body_json),
        full_request_json: captured_body_json(&record.meta.full_request_json),
        error_message: record.meta.error_message.clone(),
        error_body: record.meta.error_body.clone(),
        timing: record.meta.to_timing(),
        stream: record.meta.to_stream_details(),
    };
    if let Err(err) = record.control_store.apply_usage_rollup_owned(event).await {
        tracing::warn!(
            key_id = %record.key.key_id,
            endpoint = record.endpoint,
            status = %record.status,
            error = %err,
            "failed to record codex preflight failure usage"
        );
    }
}
pub async fn record_codex_usage(
    control_store: &dyn ControlStore,
    key: &AuthenticatedKey,
    prepared: &PreparedGatewayRequest,
    status: StatusCode,
    route: &ProviderCodexRoute,
    usage: UsageBreakdown,
    meta: &ProviderUsageMetadata,
) -> anyhow::Result<()> {
    let capture_request_details = !status.is_success();
    let event = UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: now_millis(),
        provider_type: ProviderType::Codex,
        protocol_family: codex_protocol_family_for_endpoint(&prepared.original_path),
        key_id: key.key_id.clone(),
        key_name: key.key_name.clone(),
        account_name: Some(route.account_name.clone()),
        account_group_id_at_event: route.account_group_id_at_event.clone(),
        route_strategy_at_event: Some(route.route_strategy_at_event),
        request_method: meta.request_method.clone(),
        request_url: meta.request_url.clone(),
        endpoint: prepared.original_path.clone(),
        model: prepared
            .client_visible_model
            .clone()
            .or_else(|| prepared.model.clone()),
        mapped_model: prepared.model.clone(),
        status_code: i64::from(status.as_u16()),
        request_body_bytes: meta
            .request_body_bytes
            .or(Some(clamp_usize_to_i64(prepared.request_body.len()))),
        quota_failover_count: meta.quota_failover_count,
        routing_diagnostics_json: meta.routing_diagnostics_json.clone(),
        input_uncached_tokens: clamp_u64_to_i64(usage.input_uncached_tokens),
        input_cached_tokens: clamp_u64_to_i64(usage.input_cached_tokens),
        output_tokens: clamp_u64_to_i64(usage.output_tokens),
        billable_tokens: clamp_u64_to_i64(
            usage.billable_tokens_with_multiplier(prepared.billable_multiplier),
        ),
        credit_usage: None,
        usage_missing: usage.usage_missing,
        credit_usage_missing: false,
        client_ip: meta.client_ip.clone(),
        ip_region: meta.ip_region.clone(),
        request_headers_json: meta.request_headers_json.clone(),
        last_message_content: meta.last_message_content.clone(),
        client_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.client_request_body_json))
            .flatten(),
        upstream_request_body_json: capture_request_details
            .then(|| captured_body_json(&meta.upstream_request_body_json))
            .flatten(),
        full_request_json: capture_request_details
            .then(|| {
                captured_body_json(&meta.full_request_json)
                    .or_else(|| captured_body_json(&meta.client_request_body_json))
            })
            .flatten(),
        error_message: meta.error_message.clone(),
        error_body: meta.error_body.clone(),
        timing: meta.to_timing(),
        stream: meta.to_stream_details(),
    };
    control_store.apply_usage_rollup_owned(event).await
}
pub fn missing_codex_usage() -> UsageBreakdown {
    UsageBreakdown {
        usage_missing: true,
        ..UsageBreakdown::default()
    }
}
