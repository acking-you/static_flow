//! Import Kiro credentials from the local kiro-cli SQLite database.
//!
//! The kiro-cli desktop app stores auth records in a SQLite database at
//! `~/.local/share/kiro-cli/data.sqlite3`. This module reads either the
//! legacy social token entry or the AWS IDC/OIDC token + device-registration
//! entries and converts them into a [`KiroAuthRecord`] for use by the gateway.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use static_flow_shared::llm_gateway_store::{
    DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY, DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
};

use super::auth_file::{KiroAuthRecord, DEFAULT_KIRO_REGION};

const SOCIAL_TOKEN_KEY: &str = "kirocli:social:token";
const IDC_TOKEN_KEYS: &[&str] = &["kirocli:odic:token", "kirocli:oidc:token"];
const IDC_DEVICE_REGISTRATION_KEYS: &[&str] =
    &["kirocli:odic:device-registration", "kirocli:oidc:device-registration"];

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
struct StoredTokenRecord {
    #[serde(default, alias = "accessToken")]
    access_token: Option<String>,
    #[serde(default, alias = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(default, alias = "expiresAt")]
    expires_at: Option<String>,
    #[serde(default, alias = "profileArn")]
    profile_arn: Option<String>,
    #[serde(default)]
    provider: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DeviceRegistrationRecord {
    #[serde(default, alias = "clientId")]
    client_id: Option<String>,
    #[serde(default, alias = "clientSecret")]
    client_secret: Option<String>,
}

/// Read the local Kiro auth record from the kiro-cli SQLite database at
/// `path` and return a [`KiroAuthRecord`]. Runs the blocking SQLite I/O on a
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

    if let Some(raw_token_json) = load_first_auth_kv_value(&conn, &[SOCIAL_TOKEN_KEY])? {
        let token_record: StoredTokenRecord =
            serde_json::from_str(&raw_token_json).context("parse social token json")?;
        let refresh_token = required_token_field(
            token_record.refresh_token.as_deref(),
            "kiro cli db missing refresh_token",
        )?;
        return Ok(KiroAuthRecord {
            name: resolved_account_name(requested_name),
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
            minimum_remaining_credits_before_block: Some(0.0),
            proxy_mode: Default::default(),
            proxy_config_id: None,
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            disabled: false,
            disabled_reason: None,
            source: Some("kiro-cli".to_string()),
            source_db_path: Some(path.display().to_string()),
            last_imported_at: Some(Utc::now().timestamp_millis()),
        }
        .canonicalize());
    }

    let raw_token_json = load_first_auth_kv_value(&conn, IDC_TOKEN_KEYS)?
        .ok_or_else(|| anyhow!("no supported Kiro auth token found in auth_kv"))?;
    let raw_device_registration_json =
        load_first_auth_kv_value(&conn, IDC_DEVICE_REGISTRATION_KEYS)?
            .ok_or_else(|| anyhow!("missing Kiro IDC device registration in auth_kv"))?;
    let token_record: StoredTokenRecord =
        serde_json::from_str(&raw_token_json).context("parse idc token json")?;
    let device_registration: DeviceRegistrationRecord =
        serde_json::from_str(&raw_device_registration_json)
            .context("parse idc device registration json")?;
    let refresh_token = required_token_field(
        token_record.refresh_token.as_deref(),
        "kiro cli db missing refresh_token",
    )?;
    let client_id = required_token_field(
        device_registration.client_id.as_deref(),
        "kiro cli db missing client_id",
    )?;
    let client_secret = required_token_field(
        device_registration.client_secret.as_deref(),
        "kiro cli db missing client_secret",
    )?;

    Ok(KiroAuthRecord {
        name: resolved_account_name(requested_name),
        access_token: token_record.access_token,
        refresh_token: Some(refresh_token.to_string()),
        profile_arn: token_record.profile_arn.or(profile_arn_from_state),
        expires_at: token_record.expires_at,
        auth_method: Some("idc".to_string()),
        client_id: Some(client_id.to_string()),
        client_secret: Some(client_secret.to_string()),
        region: Some(DEFAULT_KIRO_REGION.to_string()),
        auth_region: Some(DEFAULT_KIRO_REGION.to_string()),
        api_region: Some(DEFAULT_KIRO_REGION.to_string()),
        machine_id: None,
        provider: token_record.provider.or(Some("aws".to_string())),
        email: None,
        subscription_title: None,
        kiro_channel_max_concurrency: Some(DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY),
        kiro_channel_min_start_interval_ms: Some(DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
        minimum_remaining_credits_before_block: Some(0.0),
        proxy_mode: Default::default(),
        proxy_config_id: None,
        proxy_url: None,
        proxy_username: None,
        proxy_password: None,
        disabled: false,
        disabled_reason: None,
        source: Some("kiro-cli".to_string()),
        source_db_path: Some(path.display().to_string()),
        last_imported_at: Some(Utc::now().timestamp_millis()),
    }
    .canonicalize())
}

fn load_first_auth_kv_value(conn: &Connection, keys: &[&str]) -> Result<Option<String>> {
    for key in keys {
        let value = conn
            .query_row("SELECT value FROM auth_kv WHERE key = ?1 LIMIT 1", params![key], |row| {
                row.get::<_, String>(0)
            })
            .optional()?;
        if value.is_some() {
            return Ok(value);
        }
    }
    Ok(None)
}

fn required_token_field<'a>(value: Option<&'a str>, missing_message: &str) -> Result<&'a str> {
    value
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .ok_or_else(|| anyhow!("{missing_message}"))
}

fn resolved_account_name(requested_name: Option<&str>) -> String {
    requested_name
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    fn create_test_db_path() -> PathBuf {
        std::env::temp_dir().join(format!(
            "staticflow-kiro-local-import-test-{}.sqlite3",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn open_test_db(path: &Path) -> Connection {
        let _ = std::fs::remove_file(path);
        let conn = Connection::open(path).expect("open temp sqlite db");
        conn.execute_batch(
            "CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )
        .expect("create temp sqlite schema");
        conn
    }

    #[test]
    fn import_from_sqlite_blocking_imports_social_auth() {
        let path = create_test_db_path();
        let conn = open_test_db(&path);
        conn.execute(
            "INSERT INTO auth_kv(key, value) VALUES (?1, ?2)",
            params![
                "kirocli:social:token",
                r#"{
                    "access_token":"social-access",
                    "refresh_token":"rrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrrr",
                    "expires_at":"2030-01-01T00:00:00Z",
                    "profile_arn":"arn:aws:iam::123456789012:role/SocialProfile",
                    "provider":"github"
                }"#
            ],
        )
        .expect("insert social token");
        drop(conn);

        let imported =
            import_from_sqlite_blocking(&path, Some("github-main")).expect("import social auth");

        assert_eq!(imported.name, "github-main");
        assert_eq!(imported.auth_method(), "social");
        assert_eq!(imported.provider.as_deref(), Some("github"));
        assert_eq!(imported.client_id, None);
        assert_eq!(imported.client_secret, None);
        assert_eq!(
            imported.profile_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/SocialProfile")
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn import_from_sqlite_blocking_imports_idc_auth_from_aws_login() {
        let path = create_test_db_path();
        let conn = open_test_db(&path);
        conn.execute(
            "INSERT INTO auth_kv(key, value) VALUES (?1, ?2)",
            params![
                "kirocli:odic:token",
                r#"{
                    "access_token":"idc-access",
                    "refresh_token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                    "expires_at":"2031-02-03T04:05:06Z"
                }"#
            ],
        )
        .expect("insert idc token");
        conn.execute("INSERT INTO auth_kv(key, value) VALUES (?1, ?2)", params![
            "kirocli:odic:device-registration",
            r#"{
                    "client_id":"aws-client-id",
                    "client_secret":"aws-client-secret"
                }"#
        ])
        .expect("insert idc device registration");
        conn.execute("INSERT INTO state(key, value) VALUES (?1, ?2)", params![
            "api.codewhisperer.profile",
            r#"{"profileArn":"arn:aws:iam::123456789012:role/AwsProfile"}"#
        ])
        .expect("insert profile state");
        drop(conn);

        let imported =
            import_from_sqlite_blocking(&path, Some("aws-main")).expect("import idc auth");

        assert_eq!(imported.name, "aws-main");
        assert_eq!(imported.auth_method(), "idc");
        assert_eq!(imported.provider.as_deref(), Some("aws"));
        assert_eq!(imported.client_id.as_deref(), Some("aws-client-id"));
        assert_eq!(imported.client_secret.as_deref(), Some("aws-client-secret"));
        assert_eq!(
            imported.profile_arn.as_deref(),
            Some("arn:aws:iam::123456789012:role/AwsProfile")
        );

        let _ = std::fs::remove_file(&path);
    }
}
