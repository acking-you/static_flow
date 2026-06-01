//! Derive a stable machine identifier from auth credentials for upstream
//! telemetry headers.
//!
//! The Kiro upstream API expects a 64-hex-char machine ID in the `User-Agent`
//! header. If the auth record carries an explicit `machine_id`, it is
//! normalized; otherwise a deterministic ID is derived by SHA-256 hashing
//! the refresh token.

use sha2::{Digest, Sha256};

use super::auth_file::KiroAuthRecord;

/// Normalize a raw machine ID string to a 64-hex-char canonical form.
///
/// Accepts either a 64-char hex string directly, or a 32-char UUID-style
/// hex string (with dashes stripped) which is doubled to reach 64 chars.
fn normalize_machine_id(machine_id: &str) -> Option<String> {
    let trimmed = machine_id.trim();
    if trimmed.len() == 64 && trimmed.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(trimmed.to_string());
    }
    let without_dashes: String = trimmed.chars().filter(|ch| *ch != '-').collect();
    if without_dashes.len() == 32 && without_dashes.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(format!("{without_dashes}{without_dashes}"));
    }
    None
}

/// Generate a 64-hex-char machine ID from the auth record.
///
/// Prefers an explicit `machine_id` field when present; falls back to
/// `SHA-256("KotlinNativeAPI/{refresh_token}")` to match the upstream
/// client's derivation scheme.
pub fn generate_from_auth(auth: &KiroAuthRecord) -> Option<String> {
    if let Some(machine_id) = auth.machine_id.as_deref() {
        if let Some(normalized) = normalize_machine_id(machine_id) {
            return Some(normalized);
        }
    }
    auth.refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            let mut hasher = Sha256::new();
            hasher.update(format!("KotlinNativeAPI/{value}").as_bytes());
            hex::encode(hasher.finalize())
        })
}
