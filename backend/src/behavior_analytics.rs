use std::{
    net::IpAddr,
    sync::atomic::{AtomicU64, Ordering},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::{Request, State},
    http::{header, HeaderMap},
    middleware::Next,
    response::Response,
};
use static_flow_shared::lancedb_api::NewApiBehaviorEventInput;

use crate::{
    request_context::{REQUEST_ID_HEADER, TRACE_ID_HEADER},
    state::AppState,
};

const CLIENT_SOURCE_HEADER: &str = "x-sf-client";
const PAGE_PATH_HEADER: &str = "x-sf-page";
static EVENT_COUNTER: AtomicU64 = AtomicU64::new(1);
/// Compact the api_behavior_events table every N appends to prevent fragment accumulation.
const COMPACT_EVERY_N_EVENTS: u64 = 500;

pub async fn behavior_analytics_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let method = request.method().as_str().to_string();
    let uri = request.uri().clone();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or_default().to_string();
    let headers = request.headers().clone();
    let started_at = Instant::now();

    let response = next.run(request).await;

    if !path.starts_with("/api/") {
        return response;
    }

    // Skip local/loopback requests to avoid polluting analytics
    let client_ip = extract_client_ip(&headers);
    if is_local_request(&client_ip) {
        return response;
    }

    let status_code = response.status().as_u16() as i32;
    let latency_ms = started_at.elapsed().as_millis().min(i32::MAX as u128) as i32;
    let response_headers = response.headers().clone();

    // Fire-and-forget: GeoIP + DB write run in background, don't block response
    tokio::spawn(async move {
        let occurred_at = chrono::Utc::now().timestamp_millis();
        let ip_region = state.geoip.resolve_region(&client_ip).await;
        let ua_raw = header_value(&headers, header::USER_AGENT.as_str());
        let (device_type, os_family, browser_family) = parse_user_agent(ua_raw.as_deref());

        let request_id = header_value(&response_headers, REQUEST_ID_HEADER)
            .or_else(|| header_value(&headers, REQUEST_ID_HEADER))
            .unwrap_or_else(|| "unknown".to_string());
        let trace_id = header_value(&response_headers, TRACE_ID_HEADER)
            .or_else(|| header_value(&headers, TRACE_ID_HEADER))
            .unwrap_or_else(|| "unknown".to_string());
        let client_source =
            header_value(&headers, CLIENT_SOURCE_HEADER).unwrap_or_else(|| "unknown".to_string());
        let page_path =
            header_value(&headers, PAGE_PATH_HEADER).unwrap_or_else(|| "unknown".to_string());
        let referrer = header_value(&headers, header::REFERER.as_str());

        let input = NewApiBehaviorEventInput {
            event_id: generate_event_id(),
            occurred_at,
            client_source,
            method,
            path,
            query,
            page_path,
            referrer,
            status_code,
            latency_ms,
            client_ip,
            ip_region,
            ua_raw,
            device_type,
            os_family,
            browser_family,
            request_id,
            trace_id,
        };

        if let Err(err) = state.store.append_api_behavior_event(input).await {
            tracing::warn!("failed to append api behavior event: {err}");
        }

        // Periodic compaction to merge small fragments
        let count = EVENT_COUNTER.load(Ordering::Relaxed);
        if count % COMPACT_EVERY_N_EVENTS == 0 {
            tracing::info!("auto-compacting api_behavior_events (event #{count})");
            if let Err(err) = state.store.compact_api_behavior_table().await {
                tracing::warn!("auto-compact api_behavior_events failed: {err}");
            }
        }
    });

    response
}

fn generate_event_id() -> String {
    let now_ns = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    let counter = EVENT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("apievt-{now_ns:032x}-{counter:016x}")
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_client_ip(headers: &HeaderMap) -> String {
    parse_first_ip_from_header(headers.get("x-forwarded-for"))
        .or_else(|| parse_first_ip_from_header(headers.get("x-real-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("cf-connecting-ip")))
        .or_else(|| parse_first_ip_from_header(headers.get("x-client-ip")))
        .or_else(|| parse_ip_from_forwarded_header(headers.get("forwarded")))
        .unwrap_or_else(|| "unknown".to_string())
}

fn is_local_request(client_ip: &str) -> bool {
    // No proxy headers → direct local connection
    if client_ip == "unknown" {
        return true;
    }
    // Explicit loopback check
    if let Ok(ip) = client_ip.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    false
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

fn parse_user_agent(ua: Option<&str>) -> (String, String, String) {
    let raw = ua.unwrap_or_default().trim();
    if raw.is_empty() {
        return ("unknown".to_string(), "unknown".to_string(), "unknown".to_string());
    }

    let lower = raw.to_ascii_lowercase();

    let device_type =
        if lower.contains("bot") || lower.contains("spider") || lower.contains("crawler") {
            "bot"
        } else if lower.contains("ipad")
            || lower.contains("tablet")
            || lower.contains("kindle")
            || lower.contains("playbook")
        {
            "tablet"
        } else if lower.contains("mobile")
            || lower.contains("iphone")
            || lower.contains("ipod")
            || lower.contains("windows phone")
        {
            "mobile"
        } else {
            "desktop"
        };

    let os_family = if lower.contains("windows nt") {
        "Windows"
    } else if lower.contains("android") {
        "Android"
    } else if lower.contains("iphone") || lower.contains("ipad") || lower.contains("cpu iphone") {
        "iOS"
    } else if lower.contains("mac os x") || lower.contains("macintosh") {
        "macOS"
    } else if lower.contains("cros") {
        "ChromeOS"
    } else if lower.contains("linux") {
        "Linux"
    } else {
        "unknown"
    };

    let browser_family = if lower.contains("edg/") {
        "Edge"
    } else if lower.contains("opr/") || lower.contains("opera") {
        "Opera"
    } else if lower.contains("firefox/") {
        "Firefox"
    } else if lower.contains("chrome/") && !lower.contains("edg/") {
        "Chrome"
    } else if lower.contains("safari/") && !lower.contains("chrome/") && !lower.contains("chromium")
    {
        "Safari"
    } else if lower.contains("msie") || lower.contains("trident/") {
        "IE"
    } else if lower.contains("curl/") {
        "curl"
    } else if lower.contains("postmanruntime") {
        "Postman"
    } else {
        "unknown"
    };

    (device_type.to_string(), os_family.to_string(), browser_family.to_string())
}

#[cfg(test)]
mod tests {
    use super::parse_user_agent;

    #[test]
    fn parse_user_agent_detects_mobile_safari() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 \
                  (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        let (device, os, browser) = parse_user_agent(Some(ua));
        assert_eq!(device, "mobile");
        assert_eq!(os, "iOS");
        assert_eq!(browser, "Safari");
    }

    #[test]
    fn parse_user_agent_detects_desktop_chrome() {
        let ua = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) \
                  Chrome/123.0.0.0 Safari/537.36";
        let (device, os, browser) = parse_user_agent(Some(ua));
        assert_eq!(device, "desktop");
        assert_eq!(os, "Linux");
        assert_eq!(browser, "Chrome");
    }
}
