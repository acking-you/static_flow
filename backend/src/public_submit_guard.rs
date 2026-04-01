//! Shared rate-limit helpers for low-frequency public submission endpoints.
//!
//! The `/llm-access` write APIs, article requests, music wishes, and similar
//! public forms all use the same "one submission per IP per minute" guard. The
//! helpers in this module normalize the client identity and provide a single
//! enforcement path so every public write surface behaves consistently.

use std::collections::HashMap;

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::Json,
};
use parking_lot::RwLock;
use sha2::{Digest, Sha256};

use crate::handlers::ErrorResponse;

/// In-memory map from a normalized client identity key to the last submission
/// timestamp in milliseconds.
pub(crate) type PublicSubmitGuard = RwLock<HashMap<String, i64>>;

/// Build a stable client fingerprint from IP + User-Agent.
///
/// This is only used as a fallback key when a trustworthy IP cannot be
/// extracted from forwarding headers.
pub(crate) fn build_client_fingerprint(headers: &HeaderMap) -> String {
    let ip = extract_client_ip(headers);
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("unknown");
    let raw = format!("{ip}|{user_agent}");

    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Build the effective rate-limit key for one public submission.
///
/// We prefer a normalized IP-based key so retries from the same client are
/// blocked even if the browser state changes. When no IP can be recovered we
/// fall back to the hashed fingerprint.
pub(crate) fn build_submit_rate_limit_key(headers: &HeaderMap, fingerprint: &str) -> String {
    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        format!("fp:{fingerprint}")
    } else {
        format!("ip:{ip}")
    }
}

/// Extract the best-effort client IP from common proxy/CDN forwarding headers.
///
/// The search order is intentionally explicit so production deployments behind
/// reverse proxies, Cloudflare, or custom forwarding stacks yield the same
/// canonical IP string.
pub(crate) fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

/// Enforce a coarse public-write rate limit for one normalized client key.
///
/// On success the current timestamp is recorded immediately. On failure a
/// detailed `429` response is returned so the user can see both the configured
/// window and the remaining retry delay.
pub(crate) fn enforce_public_submit_rate_limit(
    guard: &PublicSubmitGuard,
    rate_limit_key: &str,
    now_ms: i64,
    rate_limit_seconds: u64,
    action_label: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let window_ms = (rate_limit_seconds.max(1) as i64) * 1_000;
    let mut writer = guard.write();
    if let Some(last) = writer.get(rate_limit_key) {
        let elapsed_ms = now_ms.saturating_sub(*last);
        if elapsed_ms < window_ms {
            let remaining_seconds = ((window_ms - elapsed_ms) + 999) / 1_000;
            return Err((
                StatusCode::TOO_MANY_REQUESTS,
                Json(ErrorResponse {
                    error: format!(
                        "{action_label} is rate-limited for this IP. Same IP may submit once \
                         every {} seconds. Retry in {} seconds.",
                        rate_limit_seconds.max(1),
                        remaining_seconds.max(1)
                    ),
                    code: 429,
                }),
            ));
        }
    }
    writer.insert(rate_limit_key.to_string(), now_ms);
    let stale_before = now_ms - window_ms * 6;
    writer.retain(|_, value| *value >= stale_before);
    Ok(())
}

fn parse_first_ip_from_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',').find_map(normalize_ip_token)
}

fn parse_ip_from_forwarded_header(value: Option<&axum::http::HeaderValue>) -> Option<String> {
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
                if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                    return Some(ip.to_string());
                }
            }
        }
    }

    if let Ok(ip) = value.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }

    if let Some((host, port)) = value.rsplit_once(':') {
        if host.contains('.') && !port.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(ip) = host.parse::<std::net::IpAddr>() {
                return Some(ip.to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::{build_submit_rate_limit_key, extract_client_ip};

    #[test]
    fn extract_client_ip_prefers_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.9"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_falls_back_to_x_forwarded_for() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.1, 198.51.100.2"));

        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_supports_cf_connecting_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-connecting-ip", HeaderValue::from_static("203.0.113.11"));

        assert_eq!(extract_client_ip(&headers), "203.0.113.11");
    }

    #[test]
    fn extract_client_ip_normalizes_ip_with_port() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.1:4567"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.1");
    }

    #[test]
    fn extract_client_ip_supports_rfc7239_for_token() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("for=198.51.100.77"));
        assert_eq!(extract_client_ip(&headers), "198.51.100.77");
    }

    #[test]
    fn extract_client_ip_supports_forwarded_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=198.51.100.88;proto=https;by=203.0.113.1"),
        );
        assert_eq!(extract_client_ip(&headers), "198.51.100.88");

        headers.insert(
            "forwarded",
            HeaderValue::from_static("for=\"[2001:db8::7]:1234\";proto=https"),
        );
        assert_eq!(extract_client_ip(&headers), "2001:db8::7");
    }

    #[test]
    fn extract_client_ip_returns_unknown_when_no_valid_header() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("not-an-ip"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("unknown, bad-token"));

        assert_eq!(extract_client_ip(&headers), "unknown");
    }

    #[test]
    fn submit_rate_limit_key_prefers_ip_over_fingerprint() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.66"));
        let key = build_submit_rate_limit_key(&headers, "fp-abc");
        assert_eq!(key, "ip:198.51.100.66");
    }
}
