//! Import Kiro credentials from the local kiro-cli SQLite database.
//!
//! The kiro-cli desktop app stores OAuth tokens in a SQLite database at
//! `~/.local/share/kiro-cli/data.sqlite3`. This module reads the social
//! auth token and profile ARN from that database and converts them into a
//! [`KiroAuthRecord`] for use by the gateway.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use static_flow_shared::llm_gateway_store::{
    DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY, DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
};

use super::auth_file::{KiroAuthRecord, DEFAULT_KIRO_REGION};

/// Return the default path to the kiro-cli SQLite database
/// (`~/.local/share/kiro-cli/data.sqlite3`).
pub fn default_sqlite_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("kiro-cli")
        .join("data.sqlite3")
}

#[derive(Debug, Deserialize)]
struct SocialTokenRecord {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_at: Option<String>,
    #[serde(default)]
    profile_arn: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

/// Read the social auth token from the kiro-cli SQLite database at `path`
/// and return a [`KiroAuthRecord`]. Runs the blocking SQLite I/O on a
/// dedicated Tokio blocking thread.
pub async fn import_from_sqlite(
    path: &Path,
    requested_name: Option<&str>,
) -> Result<KiroAuthRecord> {
    let sqlite_path = path.to_path_buf();
    let requested_name = requested_name.map(str::to_string);
    tokio::task::spawn_blocking(move || {
        import_from_sqlite_blocking(&sqlite_path, requested_name.as_deref())
    })
    .await
    .context("join sqlite import task")?
}

fn import_from_sqlite_blocking(
    path: &Path,
    requested_name: Option<&str>,
) -> Result<KiroAuthRecord> {
    if !path.exists() {
        return Err(anyhow!("kiro cli auth db not found: {}", path.display()));
    }

    let conn =
        Connection::open(path).with_context(|| format!("failed to open `{}`", path.display()))?;
    let raw_token_json: String = conn
        .query_row(
            "SELECT value FROM auth_kv WHERE key = ?1 LIMIT 1",
            params!["kirocli:social:token"],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| anyhow!("`auth_kv.kirocli:social:token` not found"))?;
    let token_record: SocialTokenRecord =
        serde_json::from_str(&raw_token_json).context("parse social token json")?;
    let refresh_token = token_record
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("kiro cli db missing refresh_token"))?;

    let profile_arn_from_state = conn
        .query_row(
            "SELECT value FROM state WHERE key = ?1 LIMIT 1",
            params!["api.codewhisperer.profile"],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("profileArn")
                .and_then(|item| item.as_str())
                .map(ToString::to_string)
        });

    Ok(KiroAuthRecord {
        name: requested_name
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or("default")
            .to_string(),
        access_token: token_record.access_token,
        refresh_token: Some(refresh_token.to_string()),
        profile_arn: token_record.profile_arn.or(profile_arn_from_state),
        expires_at: token_record.expires_at,
        auth_method: Some("social".to_string()),
        client_id: None,
        client_secret: None,
        region: Some(DEFAULT_KIRO_REGION.to_string()),
        auth_region: Some(DEFAULT_KIRO_REGION.to_string()),
        api_region: Some(DEFAULT_KIRO_REGION.to_string()),
        machine_id: None,
        provider: token_record.provider,
        email: None,
        subscription_title: None,
        kiro_channel_max_concurrency: Some(DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
        kiro_channel_min_start_interval_ms: Some(DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
        proxy_url: None,
        proxy_username: None,
        proxy_password: None,
        disabled: false,
        source: Some("kiro-cli".to_string()),
        source_db_path: Some(path.display().to_string()),
        last_imported_at: Some(Utc::now().timestamp_millis()),
    }
    .canonicalize())
}
