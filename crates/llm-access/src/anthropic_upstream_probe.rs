//! Admin probes for direct Anthropic upstream channels.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use futures_util::StreamExt;
use llm_access_anthropic_pool::{
    apply_anthropic_auth_headers, build_messages_url, build_models_url,
    parse_model_ids_from_models_response, parse_usage_from_value, AnthropicUsageSummary,
    ANTHROPIC_VERSION_2023_06_01,
};
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType},
    store::{self as core_store, AdminAnthropicUpstreamProbeTarget},
    usage::{UsageEvent, UsageTiming},
};
use reqwest::StatusCode;

use crate::provider;

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_PROBE_RESPONSE_BYTES: usize = 1024 * 1024;
const ADMIN_TEST_KEY_ID: &str = "admin-direct-anthropic-test";
const ADMIN_TEST_KEY_NAME: &str = "Admin direct Anthropic test";

#[derive(Debug, Clone)]
pub(crate) struct ModelsProbeOutput {
    pub model_ids: Vec<String>,
    pub status: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub checked_at_ms: i64,
    pub error: Option<String>,
}

impl ModelsProbeOutput {
    fn ok(model_ids: Vec<String>, started: Instant, checked_at_ms: i64, status_code: u16) -> Self {
        Self {
            model_ids,
            status: "ok".to_string(),
            status_code: Some(status_code),
            latency_ms: elapsed_ms(started),
            checked_at_ms,
            error: None,
        }
    }

    fn failure(
        started: Instant,
        checked_at_ms: i64,
        status: impl Into<String>,
        status_code: Option<u16>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            model_ids: Vec::new(),
            status: status.into(),
            status_code,
            latency_ms: elapsed_ms(started),
            checked_at_ms,
            error: Some(error.into()),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MessagesProbeOutput {
    pub status: String,
    pub status_code: Option<u16>,
    pub latency_ms: u64,
    pub checked_at_ms: i64,
    pub error: Option<String>,
    pub error_class: Option<String>,
    pub usage: AnthropicUsageSummary,
    pub upstream_request_body_json: String,
}

impl MessagesProbeOutput {
    fn ok(
        started: Instant,
        checked_at_ms: i64,
        status_code: u16,
        usage: AnthropicUsageSummary,
        upstream_request_body_json: String,
    ) -> Self {
        Self {
            status: "ok".to_string(),
            status_code: Some(status_code),
            latency_ms: elapsed_ms(started),
            checked_at_ms,
            error: None,
            error_class: None,
            usage,
            upstream_request_body_json,
        }
    }

    fn failure(
        started: Instant,
        checked_at_ms: i64,
        status: impl Into<String>,
        status_code: Option<u16>,
        error: impl Into<String>,
        error_class: impl Into<String>,
        upstream_request_body_json: String,
    ) -> Self {
        Self {
            status: status.into(),
            status_code,
            latency_ms: elapsed_ms(started),
            checked_at_ms,
            error: Some(error.into()),
            error_class: Some(error_class.into()),
            usage: AnthropicUsageSummary::missing(),
            upstream_request_body_json,
        }
    }
}

pub(crate) async fn refresh_models(
    target: &AdminAnthropicUpstreamProbeTarget,
) -> ModelsProbeOutput {
    let checked_at_ms = now_ms();
    let started = Instant::now();
    if let Some(error) = target.proxy_error.as_deref() {
        return ModelsProbeOutput::failure(
            started,
            checked_at_ms,
            "error",
            None,
            sanitize_error(error),
        );
    }
    let url = match build_models_url(&target.base_url) {
        Ok(url) => url,
        Err(err) => {
            return ModelsProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
            );
        },
    };
    let client = match provider::anthropic_upstream_client(target.proxy.as_ref()) {
        Ok(client) => client,
        Err(err) => {
            return ModelsProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
            );
        },
    };
    let request = apply_anthropic_auth_headers(
        client.get(url),
        &target.api_key,
        ANTHROPIC_VERSION_2023_06_01,
    )
    .header(reqwest::header::ACCEPT, "application/json")
    .timeout(PROBE_TIMEOUT);
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            return ModelsProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
            );
        },
    };
    let status = response.status();
    let status_code = status.as_u16();
    let body = match read_limited_response_body(response).await {
        Ok(body) => body,
        Err(err) => {
            return ModelsProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                Some(status_code),
                err,
            );
        },
    };
    if !status.is_success() {
        return ModelsProbeOutput::failure(
            started,
            checked_at_ms,
            http_status_label(status),
            Some(status_code),
            upstream_error_summary(status, &body),
        );
    }
    match parse_model_ids_from_models_response(&body) {
        Ok(model_ids) => ModelsProbeOutput::ok(model_ids, started, checked_at_ms, status_code),
        Err(err) => ModelsProbeOutput::failure(
            started,
            checked_at_ms,
            "error",
            Some(status_code),
            sanitize_error(&format!("failed to parse models response: {err}")),
        ),
    }
}

pub(crate) async fn test_messages_model(
    target: &AdminAnthropicUpstreamProbeTarget,
    model: &str,
) -> MessagesProbeOutput {
    let checked_at_ms = now_ms();
    let started = Instant::now();
    let payload = serde_json::json!({
        "model": model,
        "max_tokens": 8,
        "messages": [
            { "role": "user", "content": "hi" }
        ]
    });
    let upstream_request_body_json = payload.to_string();
    if let Some(error) = target.proxy_error.as_deref() {
        return MessagesProbeOutput::failure(
            started,
            checked_at_ms,
            "error",
            None,
            sanitize_error(error),
            "probe_proxy_error",
            upstream_request_body_json,
        );
    }
    let url = match build_messages_url(&target.base_url) {
        Ok(url) => url,
        Err(err) => {
            return MessagesProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
                "probe_config_error",
                upstream_request_body_json,
            );
        },
    };
    let client = match provider::anthropic_upstream_client(target.proxy.as_ref()) {
        Ok(client) => client,
        Err(err) => {
            return MessagesProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
                "probe_client_error",
                upstream_request_body_json,
            );
        },
    };
    let request = apply_anthropic_auth_headers(
        client.post(url),
        &target.api_key,
        ANTHROPIC_VERSION_2023_06_01,
    )
    .header(reqwest::header::CONTENT_TYPE, "application/json")
    .header(reqwest::header::ACCEPT, "application/json")
    .timeout(PROBE_TIMEOUT)
    .body(upstream_request_body_json.clone());
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            return MessagesProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                None,
                sanitize_error(&err.to_string()),
                "upstream_transport_error",
                upstream_request_body_json,
            );
        },
    };
    let status = response.status();
    let status_code = status.as_u16();
    let body = match read_limited_response_body(response).await {
        Ok(body) => body,
        Err(err) => {
            return MessagesProbeOutput::failure(
                started,
                checked_at_ms,
                "error",
                Some(status_code),
                err,
                "upstream_body_error",
                upstream_request_body_json,
            );
        },
    };
    if !status.is_success() {
        return MessagesProbeOutput::failure(
            started,
            checked_at_ms,
            http_status_label(status),
            Some(status_code),
            upstream_error_summary(status, &body),
            "upstream_error",
            upstream_request_body_json,
        );
    }
    let usage = serde_json::from_slice::<serde_json::Value>(&body)
        .map(|value| parse_usage_from_value(&value))
        .unwrap_or_else(|_| AnthropicUsageSummary::missing());
    MessagesProbeOutput::ok(started, checked_at_ms, status_code, usage, upstream_request_body_json)
}

pub(crate) fn usage_event_for_messages_test(
    channel_name: &str,
    model: &str,
    output: &MessagesProbeOutput,
) -> UsageEvent {
    UsageEvent {
        event_id: format!("llm-usage-{}", uuid::Uuid::new_v4()),
        created_at_ms: output.checked_at_ms,
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: ADMIN_TEST_KEY_ID.to_string(),
        key_name: ADMIN_TEST_KEY_NAME.to_string(),
        account_name: Some(channel_name.to_string()),
        account_group_id_at_event: None,
        route_strategy_at_event: None,
        request_method: "POST".to_string(),
        request_url: format!("/admin/kiro-gateway/anthropic-upstreams/{channel_name}/test"),
        endpoint: "/v1/messages".to_string(),
        model: Some(model.to_string()),
        mapped_model: None,
        status_code: i64::from(output.status_code.unwrap_or(502)),
        request_body_bytes: Some(output.upstream_request_body_json.len() as i64),
        quota_failover_count: 0,
        retry: Default::default(),
        routing_diagnostics_json: Some(
            serde_json::json!({
                "upstream_pool": "direct_anthropic_test",
                "channel_name": channel_name,
                "admin_probe": true,
                "probe_kind": "messages_model_test",
                "admin_probe_billable_tokens": admin_probe_billable_tokens(output.usage),
            })
            .to_string(),
        ),
        input_uncached_tokens: output.usage.input_uncached_tokens.max(0),
        input_cached_tokens: output.usage.input_cached_tokens.max(0),
        output_tokens: output.usage.output_tokens.max(0),
        billable_tokens: 0,
        credit_usage: None,
        usage_missing: output.usage.usage_missing,
        credit_usage_missing: true,
        client_ip: "admin".to_string(),
        ip_region: "admin".to_string(),
        request_headers_json: serde_json::json!({
            "anthropic-version": ANTHROPIC_VERSION_2023_06_01,
        })
        .to_string(),
        last_message_content: Some("hi".to_string()),
        client_request_body_json: None,
        upstream_request_body_json: Some(output.upstream_request_body_json.clone()),
        full_request_json: None,
        error_message: output.error.clone(),
        error_class: output.error_class.clone(),
        session_blocked: false,
        response_image_count: None,
        error_body: None,
        response_body: None,
        timing: UsageTiming {
            latency_ms: Some(output.latency_ms.min(i64::MAX as u64) as i64),
            upstream_headers_ms: Some(output.latency_ms.min(i64::MAX as u64) as i64),
            ..UsageTiming::default()
        },
        stream: Default::default(),
    }
}

fn admin_probe_billable_tokens(usage: AnthropicUsageSummary) -> u64 {
    if usage.usage_missing {
        return 0;
    }
    core_store::compute_billable_tokens(
        usage.input_uncached_tokens.max(0) as u64,
        usage.input_cached_tokens.max(0) as u64,
        usage.output_tokens.max(0) as u64,
    )
}

async fn read_limited_response_body(response: reqwest::Response) -> Result<Bytes, String> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_PROBE_RESPONSE_BYTES as u64)
    {
        return Err("upstream probe response is too large".to_string());
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result
            .map_err(|err| sanitize_error(&format!("failed to read response: {err}")))?;
        if body.len().saturating_add(chunk.len()) > MAX_PROBE_RESPONSE_BYTES {
            return Err("upstream probe response is too large".to_string());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Bytes::from(body))
}

fn upstream_error_summary(status: StatusCode, body: &Bytes) -> String {
    let message = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.get("message"))
                .or_else(|| value.get("message"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| String::from_utf8_lossy(body).trim().to_string());
    if message.is_empty() {
        format!("upstream returned HTTP {}", status.as_u16())
    } else {
        sanitize_error(&format!("upstream returned HTTP {}: {message}", status.as_u16()))
    }
}

fn sanitize_error(message: &str) -> String {
    const MAX_ERROR_CHARS: usize = 500;
    let mut sanitized = String::new();
    for part in message.split_whitespace() {
        if !sanitized.is_empty() {
            sanitized.push(' ');
        }
        sanitized.push_str(part);
    }
    let mut chars = sanitized.chars();
    let mut truncated = chars.by_ref().take(MAX_ERROR_CHARS).collect::<String>();
    if chars.next().is_some() {
        truncated.push_str("...");
        sanitized = truncated;
    }
    if sanitized.is_empty() {
        "upstream probe failed".to_string()
    } else {
        sanitized
    }
}

fn http_status_label(status: StatusCode) -> String {
    format!("http_{}", status.as_u16())
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;

    #[test]
    fn messages_test_usage_event_keeps_quota_zero_but_exposes_probe_cost() {
        let output = MessagesProbeOutput::ok(
            Instant::now(),
            1_700_000_000_000,
            200,
            AnthropicUsageSummary {
                input_uncached_tokens: 100,
                input_cached_tokens: 20,
                output_tokens: 3,
                usage_missing: false,
            },
            "{}".to_string(),
        );

        let event = usage_event_for_messages_test("yl", "claude-haiku-4-5", &output);
        let diagnostics: serde_json::Value = serde_json::from_str(
            event
                .routing_diagnostics_json
                .as_deref()
                .expect("diagnostics"),
        )
        .expect("diagnostics json");

        assert_eq!(event.billable_tokens, 0);
        assert_eq!(diagnostics["upstream_pool"], "direct_anthropic_test");
        assert_eq!(diagnostics["admin_probe_billable_tokens"], 117);
    }

    #[test]
    fn upstream_error_summary_prefers_anthropic_error_message() {
        let summary = upstream_error_summary(
            StatusCode::UNAUTHORIZED,
            &Bytes::from_static(br#"{"error":{"message":"bad api key"}}"#),
        );

        assert_eq!(summary, "upstream returned HTTP 401: bad api key");
    }

    #[tokio::test]
    async fn read_limited_response_body_rejects_large_content_length() {
        let url = serve_one_http_response(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                MAX_PROBE_RESPONSE_BYTES + 1
            )
            .into_bytes(),
        )
        .await;
        let response = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("response");

        let err = read_limited_response_body(response)
            .await
            .expect_err("body cap");

        assert_eq!(err, "upstream probe response is too large");
    }

    #[tokio::test]
    async fn read_limited_response_body_rejects_stream_past_cap() {
        let mut raw = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        raw.extend(vec![b'a'; MAX_PROBE_RESPONSE_BYTES + 1]);
        let url = serve_one_http_response(raw).await;
        let response = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .expect("response");

        let err = read_limited_response_body(response)
            .await
            .expect_err("body cap");

        assert_eq!(err, "upstream probe response is too large");
    }

    async fn serve_one_http_response(raw_response: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request_buffer = [0u8; 1024];
            let _ = stream.read(&mut request_buffer).await;
            stream
                .write_all(&raw_response)
                .await
                .expect("write response");
        });
        format!("http://{addr}/probe")
    }
}
