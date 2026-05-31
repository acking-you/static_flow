//! Usage-metadata capture helpers (request/response body + error capture).

use std::time::Instant;

use axum::{
    body::Bytes,
    http::{HeaderMap, Method},
};
use llm_access_codex::{
    request::{
        extract_client_ip_from_headers, resolve_request_url_from_headers, serialize_headers_json,
    },
    types::PreparedGatewayRequest,
};
use llm_access_core::usage::{UsageStreamDetails, UsageTiming};
use serde_json::Value;

use super::{errors::summarize_error_bytes, util::clamp_usize_to_i64, ProviderUsageMetadata};
use crate::geoip::GeoIpResolver;

impl ProviderUsageMetadata {
    pub(super) async fn from_request_parts(
        method: &Method,
        uri: &axum::http::Uri,
        headers: &HeaderMap,
        geoip: &GeoIpResolver,
    ) -> Self {
        let client_ip = extract_client_ip_from_headers(headers);
        let ip_region = geoip.resolve_region(&client_ip).await;
        Self {
            started_at: Instant::now(),
            request_method: method.as_str().to_string(),
            request_url: resolve_request_url_from_headers(headers, uri),
            request_body_bytes: None,
            request_body_read_ms: None,
            request_json_parse_ms: None,
            pre_handler_ms: None,
            routing_wait_ms: None,
            upstream_headers_ms: None,
            post_headers_body_ms: None,
            first_sse_write_ms: None,
            stream_finish_ms: None,
            stream_completed_cleanly: None,
            downstream_disconnect: None,
            final_event_type: None,
            bytes_streamed: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            client_ip,
            ip_region,
            request_headers_json: serialize_headers_json(headers),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
        }
    }

    fn elapsed_ms(&self) -> i64 {
        self.started_at.elapsed().as_millis().min(i64::MAX as u128) as i64
    }

    pub(super) fn with_request_body(mut self, body: &Bytes, read_ms: i64) -> Self {
        self.request_body_bytes = Some(clamp_usize_to_i64(body.len()));
        self.request_body_read_ms = Some(read_ms);
        self
    }

    pub(super) fn mark_pre_handler_done(&mut self, parse_ms: i64) {
        self.request_json_parse_ms = Some(parse_ms);
        self.pre_handler_ms = Some(self.elapsed_ms());
    }

    pub(super) fn mark_upstream_headers(&mut self) {
        self.upstream_headers_ms = Some(self.elapsed_ms());
    }

    pub(super) fn mark_failover(&mut self) {
        self.quota_failover_count = self.quota_failover_count.saturating_add(1);
    }

    pub(super) fn add_routing_wait(&mut self, elapsed_ms: i64) {
        self.routing_wait_ms = Some(
            self.routing_wait_ms
                .unwrap_or_default()
                .saturating_add(elapsed_ms),
        );
    }

    pub(super) fn mark_post_headers_body(&mut self) {
        self.post_headers_body_ms = Some(
            self.elapsed_ms()
                .saturating_sub(self.upstream_headers_ms.unwrap_or_default()),
        );
    }

    fn mark_first_sse_write(&mut self) {
        if self.first_sse_write_ms.is_none() {
            self.first_sse_write_ms = Some(self.elapsed_ms());
        }
    }

    pub(super) fn observe_stream_write(&mut self, bytes_len: usize, event_type: Option<&str>) {
        self.mark_first_sse_write();
        self.stream_completed_cleanly.get_or_insert(false);
        self.downstream_disconnect.get_or_insert(false);
        self.bytes_streamed = Some(
            self.bytes_streamed
                .unwrap_or_default()
                .saturating_add(clamp_usize_to_i64(bytes_len)),
        );
        if let Some(event_type) = event_type.map(str::trim).filter(|value| !value.is_empty()) {
            self.final_event_type = Some(event_type.to_string());
        }
    }

    pub(super) fn mark_stream_finish(&mut self) {
        self.stream_finish_ms = Some(self.elapsed_ms());
    }

    pub(super) fn mark_stream_completed_cleanly(&mut self) {
        self.stream_completed_cleanly = Some(true);
        self.downstream_disconnect = Some(false);
        self.mark_stream_finish();
    }

    pub(super) fn mark_stream_internal_incomplete(&mut self) {
        self.stream_completed_cleanly = Some(false);
        self.downstream_disconnect = Some(false);
        self.mark_stream_finish();
    }

    pub(super) fn mark_downstream_disconnect(&mut self) {
        self.stream_completed_cleanly = Some(false);
        self.downstream_disconnect = Some(true);
        self.mark_stream_finish();
    }

    pub(super) fn to_timing(&self) -> UsageTiming {
        UsageTiming {
            latency_ms: self.stream_finish_ms.or(Some(self.elapsed_ms())),
            routing_wait_ms: self.routing_wait_ms,
            upstream_headers_ms: self.upstream_headers_ms,
            post_headers_body_ms: self.post_headers_body_ms,
            request_body_read_ms: self.request_body_read_ms,
            request_json_parse_ms: self.request_json_parse_ms,
            pre_handler_ms: self.pre_handler_ms,
            first_sse_write_ms: self.first_sse_write_ms,
            stream_finish_ms: self.stream_finish_ms,
        }
    }

    pub(super) fn to_stream_details(&self) -> UsageStreamDetails {
        UsageStreamDetails {
            stream_completed_cleanly: self.stream_completed_cleanly,
            downstream_disconnect: self.downstream_disconnect,
            final_event_type: self.final_event_type.clone(),
            bytes_streamed: self.bytes_streamed,
        }
    }
}
pub fn capture_client_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}
pub fn capture_upstream_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}
pub fn capture_error_message(meta: &mut ProviderUsageMetadata, message: &str) {
    if meta.error_message.is_some() {
        return;
    }
    let trimmed = message.trim();
    if !trimmed.is_empty() {
        meta.error_message = Some(trimmed.to_string());
    }
}
pub fn capture_error_body(meta: &mut ProviderUsageMetadata, body: &str) {
    if meta.error_body.is_some() {
        return;
    }
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        meta.error_body = Some(trimmed.to_string());
    }
}
pub fn capture_error_bytes(meta: &mut ProviderUsageMetadata, bytes: &Bytes) {
    capture_error_message(meta, &summarize_error_bytes(bytes));
    let body = String::from_utf8_lossy(bytes.as_ref());
    capture_error_body(meta, &body);
}
pub fn capture_codex_dispatch_request_json(
    meta: &mut ProviderUsageMetadata,
    client_body: &Bytes,
    prepared: &PreparedGatewayRequest,
) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(client_body.clone());
    }
    meta.upstream_request_body_json = Some(prepared.request_body.clone());
}
pub fn capture_codex_prepared_request_json(
    meta: &mut ProviderUsageMetadata,
    prepared: &PreparedGatewayRequest,
) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(prepared.client_request_body_or_upstream().clone());
    }
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(prepared.request_body.clone());
    }
}
pub fn strip_codex_stream_request_bodies(
    mut prepared: PreparedGatewayRequest,
) -> PreparedGatewayRequest {
    prepared.client_request_body = None;
    prepared.request_body = Bytes::new();
    prepared
}
pub fn captured_body_json(body: &Option<Bytes>) -> Option<String> {
    body.as_ref()
        .map(|bytes| String::from_utf8_lossy(bytes.as_ref()).into_owned())
}
pub fn extract_model_from_json_body(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("model").cloned())
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
}
