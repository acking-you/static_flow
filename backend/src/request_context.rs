use std::{net::SocketAddr, time::Instant};

use axum::{
    extract::{connect_info::ConnectInfo, Request},
    http::{header::HeaderName, HeaderMap, HeaderValue},
    middleware::Next,
    response::Response,
};
use static_flow_shared::request_ids::{read_or_generate_id, REQUEST_ID_HEADER, TRACE_ID_HEADER};
use tracing::Instrument;

pub async fn request_context_middleware(request: Request, next: Next) -> Response {
    let request_id = read_or_generate_id(
        request
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        "req",
    );
    let trace_id = read_or_generate_id(
        request
            .headers()
            .get(TRACE_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        "trace",
    );
    let remote_addr = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|value| value.0.to_string())
        .unwrap_or_else(|| "-".to_string());

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
        target: "staticflow_access",
        parent: &span,
        request_id = %request_id,
        trace_id = %trace_id,
        remote_addr = %remote_addr,
        method = %method,
        path = %path,
        status = response.status().as_u16(),
        elapsed_ms = started_at.elapsed().as_millis(),
        "backend access"
    );

    response
}

fn set_response_header(headers: &mut HeaderMap, header_name: &'static str, value: &str) {
    let Ok(header_value) = HeaderValue::from_str(value) else {
        return;
    };
    headers.insert(HeaderName::from_static(header_name), header_value);
}
