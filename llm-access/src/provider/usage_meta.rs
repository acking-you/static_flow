//! Per-request usage metadata capture: client/upstream request-body JSON, error
//! message/body/bytes capture, and model extraction.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn capture_client_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}
pub(crate) fn capture_upstream_request_body_json(meta: &mut ProviderUsageMetadata, body: &[u8]) {
    if meta.upstream_request_body_json.is_none() {
        meta.upstream_request_body_json = Some(Bytes::copy_from_slice(body));
    }
}
pub(crate) fn capture_codex_dispatch_request_json(
    meta: &mut ProviderUsageMetadata,
    client_body: &Bytes,
    prepared: &PreparedGatewayRequest,
) {
    if meta.client_request_body_json.is_none() {
        meta.client_request_body_json = Some(client_body.clone());
    }
    meta.upstream_request_body_json = Some(prepared.request_body.clone());
}
pub(crate) fn capture_codex_prepared_request_json(
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
pub(crate) fn strip_codex_stream_request_bodies(
    mut prepared: PreparedGatewayRequest,
) -> PreparedGatewayRequest {
    prepared.client_request_body = None;
    prepared.request_body = Bytes::new();
    prepared
}
pub(crate) fn captured_body_json(body: &Option<Bytes>) -> Option<String> {
    body.as_ref()
        .map(|bytes| String::from_utf8_lossy(bytes.as_ref()).into_owned())
}
pub(crate) fn extract_model_from_json_body(body: &Bytes) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.get("model").cloned())
        .and_then(|value| value.as_str().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
}
