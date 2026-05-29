//! Admin access control (token check) and client-IP extraction/classification
//! from request headers.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn ensure_admin_access(headers: &HeaderMap) -> Result<(), AdminHttpError> {
    if let Some(expected_token) = admin_token() {
        let provided = headers
            .get("x-admin-token")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .unwrap_or_default();
        if provided == expected_token {
            return Ok(());
        }
    }

    let ip = extract_client_ip(headers);
    if ip == "unknown" {
        if is_local_host_header(headers) {
            return Ok(());
        }
        return Err(forbidden("Admin endpoint is local-only"));
    }
    let ip = ip
        .parse::<IpAddr>()
        .map_err(|_| forbidden("Admin endpoint is local-only"))?;
    if is_private_or_loopback_ip(ip) {
        Ok(())
    } else {
        Err(forbidden("Admin endpoint is local-only"))
    }
}
pub(crate) fn admin_token() -> Option<String> {
    std::env::var("LLM_ACCESS_ADMIN_TOKEN")
        .ok()
        .or_else(|| std::env::var("ADMIN_TOKEN").ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
pub(crate) fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}
pub(crate) fn parse_first_ip_from_header(value: Option<&header::HeaderValue>) -> Option<String> {
    let raw = value?.to_str().ok()?;
    raw.split(',')
        .find_map(|part| normalize_ip_token(part.trim()))
}
pub(crate) fn parse_ip_from_forwarded_header(
    value: Option<&header::HeaderValue>,
) -> Option<String> {
    let raw = value?.to_str().ok()?;
    for segment in raw.split(',') {
        for pair in segment.split(';') {
            let (key, value) = pair.split_once('=')?;
            if key.trim().eq_ignore_ascii_case("for") {
                if let Some(ip) = normalize_ip_token(value.trim().trim_matches('"')) {
                    return Some(ip);
                }
            }
        }
    }
    None
}
pub(crate) fn normalize_ip_token(token: &str) -> Option<String> {
    let token = token.trim();
    if token.is_empty() || token.eq_ignore_ascii_case("unknown") {
        return None;
    }
    if let Ok(ip) = token.parse::<IpAddr>() {
        return Some(ip.to_string());
    }
    if let Some(host) = token
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    if let Some((host, _port)) = token.rsplit_once(':') {
        if let Ok(ip) = host.parse::<IpAddr>() {
            return Some(ip.to_string());
        }
    }
    None
}
pub(crate) fn is_private_or_loopback_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.octets()[0] == 169 && v4.octets()[1] == 254
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}
pub(crate) fn is_local_host_header(headers: &HeaderMap) -> bool {
    let Some(raw_host) = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    else {
        return false;
    };
    let host = raw_host.trim();
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("[::1]") {
        return true;
    }
    if let Some(host_only) = host
        .strip_prefix('[')
        .and_then(|value| value.split_once(']').map(|parts| parts.0))
    {
        if let Ok(ip) = host_only.parse::<IpAddr>() {
            return is_private_or_loopback_ip(ip);
        }
    }
    let host_only = host
        .split_once(':')
        .map(|parts| parts.0)
        .unwrap_or(host)
        .trim();
    if host_only.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host_only
        .parse::<IpAddr>()
        .map(is_private_or_loopback_ip)
        .unwrap_or(false)
}
