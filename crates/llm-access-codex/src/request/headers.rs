//! HTTP header utilities: header/IP/origin/url parsing from inbound proxy
//! headers.


use std::{collections::BTreeMap, net::IpAddr};

use axum::http::header;
use http::HeaderMap;
/// Read one trimmed header value as UTF-8 text.
/// Read one trimmed header value as UTF-8 text.
pub fn extract_header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}
/// Serialize request headers into a stable JSON object for admin diagnostics.
///
/// These values are intentionally captured from the original inbound request so
/// operators can inspect what the reverse proxy and client actually sent. The
/// serialized JSON is stored in the usage ledger only; it is **not** reused as
/// an upstream header set when the gateway later calls the Codex backend.
pub fn serialize_headers_json(headers: &HeaderMap) -> String {
    let mut map = BTreeMap::<String, Vec<String>>::new();
    for name in headers.keys() {
        let key = name.as_str().to_string();
        let values = headers
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !values.is_empty() {
            map.insert(key, values);
        }
    }
    serde_json::to_string(&map).unwrap_or_else(|err| {
        tracing::warn!("Failed to serialize LLM gateway request headers to JSON: {err}");
        "{}".to_string()
    })
}
/// Builds the operator-facing absolute URL when proxy headers are available.
///
/// Reverse-proxy headers such as `x-forwarded-host` and
/// `x-forwarded-proto` are consumed here only to reconstruct the public URL
/// that the caller hit. They are not forwarded to the upstream Codex API.
pub fn resolve_request_url_from_headers(headers: &HeaderMap, uri: &http::Uri) -> String {
    let scheme = extract_header_value(headers, "x-forwarded-proto")
        .or_else(|| extract_header_value(headers, "x-scheme"))
        .unwrap_or_else(|| "http".to_string());
    let host = extract_header_value(headers, "x-forwarded-host")
        .or_else(|| extract_header_value(headers, header::HOST.as_str()));
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str().to_string())
        .unwrap_or_else(|| uri.path().to_string());
    match host {
        Some(host) => format!("{scheme}://{host}{path_and_query}"),
        None => path_and_query,
    }
}
/// Extracts the first trustworthy client IP from the reverse-proxy header
/// chain.
///
/// The gateway uses these proxy headers strictly for local diagnostics,
/// behavior analysis, and admin troubleshooting. Upstream Codex requests are
/// rebuilt from a narrow allowlist and therefore do not inherit
/// `x-forwarded-for`, `x-real-ip`, `forwarded`, or similar network-path
/// headers.
pub fn extract_client_ip_from_headers(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}
/// Parse the first IP candidate from a comma-delimited proxy header.
fn parse_first_ip_from_header(value: Option<&http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}
/// Parse the RFC 7239 `Forwarded` header and extract the first usable IP.
fn parse_ip_from_forwarded_header(value: Option<&http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(|entry| {
        entry.split(';').find_map(|segment| {
            let token = segment.trim();
            if token
                .get(..4)
                .map(|prefix| prefix.eq_ignore_ascii_case("for="))
                .unwrap_or(false)
            {
                normalize_ip_token(token)
            } else {
                None
            }
        })
    })
}
/// Normalize raw proxy IP tokens across IPv4, IPv6, and host:port forms.
fn normalize_ip_token(token: &str) -> Option<String> {
    let mut value = token.trim().trim_matches('"');
    if value.is_empty() || value.eq_ignore_ascii_case("unknown") {
        return None;
    }

    if value
        .get(..4)
        .map(|prefix| prefix.eq_ignore_ascii_case("for="))
        .unwrap_or(false)
    {
        value = value[4..].trim().trim_matches('"');
    }

    if value.starts_with('[') {
        if let Some(end) = value.find(']') {
            let host = &value[1..end];
            let remain = value[end + 1..].trim();
            let valid_suffix = remain.is_empty()
                || (remain.starts_with(':') && remain[1..].chars().all(|ch| ch.is_ascii_digit()));
            if valid_suffix {
                if let Ok(ip) = host.parse::<IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    if let Ok(ip) = value.parse::<IpAddr>() {
        return Some(ip.to_string());
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}
/// Reconstruct the externally visible origin from reverse-proxy headers.
pub fn external_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("http");
    Some(format!("{scheme}://{host}"))
}
