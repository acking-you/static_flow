use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::Request,
    http::{header::HeaderName, HeaderMap, HeaderValue},
    middleware::Next,
    response::Response,
};
use tracing::Instrument;

pub const REQUEST_ID_HEADER: &str = "x-request-id";
pub const TRACE_ID_HEADER: &str = "x-trace-id";

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(1);

pub async fn request_context_middleware(request: Request, next: Next) -> Response {
    let request_id = read_or_generate_header_id(request.headers(), REQUEST_ID_HEADER, "req");
    let trace_id = read_or_generate_header_id(request.headers(), TRACE_ID_HEADER, "trace");

    let method = request.method().clone();
    let path = request.uri().path().to_owned();
    let started_at = Instant::now();

    let span = tracing::info_span!(
        "http_request",
        request_id = %request_id,
        trace_id = %trace_id,
        method = %method,
        path = %path,
    );

    let mut response = next.run(request).instrument(span.clone()).await;

    set_response_header(response.headers_mut(), REQUEST_ID_HEADER, request_id.as_str());
    set_response_header(response.headers_mut(), TRACE_ID_HEADER, trace_id.as_str());

    tracing::info!(
        parent: &span,
        status = response.status().as_u16(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "request completed"
    );

    response
}

fn read_or_generate_header_id(
    headers: &HeaderMap,
    header_name: &'static str,
    prefix: &str,
) -> String {
    headers
        .get(header_name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| generate_id(prefix))
}

fn generate_id(prefix: &str) -> String {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now_ns:032x}-{counter:016x}")
}

fn set_response_header(headers: &mut HeaderMap, header_name: &'static str, value: &str) {
    let Ok(header_value) = HeaderValue::from_str(value) else {
        return;
    };
    headers.insert(HeaderName::from_static(header_name), header_value);
}
