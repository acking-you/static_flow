//! Small shared helpers: id/secret generation, hashing, name/status/string
//! normalization, and the AdminHttpError response constructors.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
pub(crate) fn admin_page_request(query: AdminListQuery) -> core_store::AdminPageRequest {
    core_store::AdminPageRequest {
        limit: query
            .limit
            .unwrap_or(DEFAULT_ADMIN_LIST_LIMIT)
            .clamp(1, MAX_ADMIN_LIST_LIMIT),
        offset: query.offset.unwrap_or(0),
    }
}
pub(crate) fn generate_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4().simple())
}
pub(crate) fn generate_secret() -> String {
    format!("sfk_{}", uuid::Uuid::new_v4().simple())
}
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
pub(crate) fn normalize_codex_client_version(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > MAX_CODEX_CLIENT_VERSION_LEN {
        return None;
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return None;
    }
    Some(trimmed.to_string())
}
pub(crate) fn summarize_upstream_error_body(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        "empty body".to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}
pub(crate) fn normalize_name(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(bad_request("name is required"))
    } else {
        Ok(trimmed.to_string())
    }
}
pub(crate) fn normalize_status(raw: &str) -> Result<String, AdminHttpError> {
    let trimmed = raw.trim();
    if matches!(trimmed, KEY_STATUS_ACTIVE | KEY_STATUS_DISABLED) {
        Ok(trimmed.to_string())
    } else {
        Err(bad_request("status must be `active` or `disabled`"))
    }
}
pub(crate) fn normalize_route_strategy_input(raw: &str) -> Result<Option<String>, AdminHttpError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    match trimmed {
        "auto" | "fixed" => Ok(Some(trimmed.to_string())),
        _ => Err(bad_request("route_strategy must be `auto` or `fixed`")),
    }
}
pub(crate) fn validate_provider_type(provider_type: &str) -> Result<(), AdminHttpError> {
    match provider_type {
        PROVIDER_CODEX | PROVIDER_KIRO => Ok(()),
        _ => Err(bad_request("provider_type must be `codex` or `kiro`")),
    }
}
pub(crate) fn normalize_optional_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
pub(crate) fn normalize_optional_string_option(raw: Option<&str>) -> Option<String> {
    raw.and_then(normalize_optional_string)
}
pub(crate) fn validate_kiro_channel_limit_inputs(
    max_concurrency: Option<u64>,
    min_start_interval_ms: Option<u64>,
) -> Result<(), AdminHttpError> {
    if let Some(value) = max_concurrency {
        if value == 0 || value > MAX_CODEX_KEY_REQUEST_MAX_CONCURRENCY {
            return Err(bad_request("kiro_channel_max_concurrency is out of range"));
        }
    }
    if let Some(value) = min_start_interval_ms {
        if value > MAX_CODEX_KEY_REQUEST_MIN_START_INTERVAL_MS {
            return Err(bad_request("kiro_channel_min_start_interval_ms is out of range"));
        }
    }
    Ok(())
}
pub(crate) fn bad_request(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::BAD_REQUEST,
        message: message.to_string(),
    }
}
pub(crate) fn forbidden(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::FORBIDDEN,
        message: message.to_string(),
    }
}
pub(crate) fn conflict(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::CONFLICT,
        message: message.to_string(),
    }
}
pub(crate) fn too_many_requests(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::TOO_MANY_REQUESTS,
        message: message.to_string(),
    }
}
pub(crate) fn not_found(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::NOT_FOUND,
        message: message.to_string(),
    }
}
pub(crate) fn internal_error(message: &str) -> AdminHttpError {
    AdminHttpError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: message.to_string(),
    }
}
