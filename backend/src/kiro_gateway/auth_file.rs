//! On-disk Kiro account model and JSON persistence helpers.
//!
//! Each Kiro account is stored as one JSON file under
//! `~/.static-flow/auths/kiro/`. The helpers here keep file naming,
//! canonicalization, and current-account selection deterministic so the rest
//! of the runtime can treat account records as stable config objects.

use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use static_flow_shared::llm_gateway_store::{
    DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY, DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS,
};

/// Default AWS region used for Kiro authentication and API calls when none is
/// configured.
pub const DEFAULT_KIRO_REGION: &str = "us-east-1";

/// Default Kiro client version string sent in request headers.
pub const DEFAULT_KIRO_VERSION: &str = "0.10.0";

/// Default system/OS version string sent in request headers.
pub const DEFAULT_SYSTEM_VERSION: &str = "darwin#24.6.0";

/// Default Node.js version string sent in request headers.
pub const DEFAULT_NODE_VERSION: &str = "22.21.1";

/// A single Kiro authentication credential record, persisted as JSON on disk.
///
/// Each record represents one named account with its OAuth tokens, region
/// configuration, optional proxy settings, and import provenance metadata.
/// Fields use camelCase serialization to match the upstream Kiro JSON format.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct KiroAuthRecord {
    /// Unique account name used as the file stem and lookup key.
    pub name: String,
    /// Short-lived OAuth access token for API requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    /// Long-lived OAuth refresh token used to obtain new access tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// AWS IAM Identity Center profile ARN associated with this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_arn: Option<String>,
    /// ISO-8601 timestamp when the current access token expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Authentication method identifier (e.g. `"idc"`, `"social"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_method: Option<String>,
    /// OAuth client ID for IDC-based authentication flows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// OAuth client secret for IDC-based authentication flows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// General-purpose AWS region; used as fallback for both auth and API
    /// regions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    /// Explicit region override for the authentication endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_region: Option<String>,
    /// Explicit region override for the API endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_region: Option<String>,
    /// Opaque machine identifier for telemetry and device tracking.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    /// Identity provider name (e.g. `"aws"`, `"github"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// Email address associated with the authenticated account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Human-readable subscription tier or plan title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscription_title: Option<String>,
    /// Maximum number of concurrent upstream Kiro requests allowed for this
    /// account before the scheduler rotates to another account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_channel_max_concurrency: Option<u64>,
    /// Minimum spacing between upstream Kiro request starts for this account,
    /// in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kiro_channel_min_start_interval_ms: Option<u64>,
    /// HTTP(S) proxy URL for outbound API requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_url: Option<String>,
    /// Username for proxy authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_username: Option<String>,
    /// Password for proxy authentication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_password: Option<String>,
    /// Whether this account is disabled and should be skipped during selection.
    #[serde(default)]
    pub disabled: bool,
    /// Origin label indicating where this record was imported from (e.g.
    /// `"llm_gateway"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Filesystem path to the source database this record was imported from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_db_path: Option<String>,
    /// Unix timestamp (seconds) of the last import from the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_imported_at: Option<i64>,
}

impl KiroAuthRecord {
    /// Normalize this record into a canonical form.
    ///
    /// - Sets `name` to `"default"` if blank.
    /// - Lowercases `auth_method` and maps legacy values (`"builder-id"`,
    ///   `"iam"`) to `"idc"`.
    /// - Fills `region` with [`DEFAULT_KIRO_REGION`] when missing or empty.
    ///
    /// Consumes and returns `self` for builder-style chaining.
    pub fn canonicalize(mut self) -> Self {
        if self.name.trim().is_empty() {
            self.name = "default".to_string();
        }
        if let Some(method) = self.auth_method.as_mut() {
            let lower = method.trim().to_ascii_lowercase();
            if lower == "builder-id" || lower == "iam" {
                *method = "idc".to_string();
            } else {
                *method = lower;
            }
        }
        if self.region.as_deref().is_none_or(str::is_empty) {
            self.region = Some(DEFAULT_KIRO_REGION.to_string());
        }
        self
    }

    /// Return the normalized auth method, inferring a default when the source
    /// record omitted it.
    pub fn auth_method(&self) -> &str {
        self.auth_method.as_deref().unwrap_or_else(|| {
            if self.client_id.is_some() && self.client_secret.is_some() {
                "idc"
            } else {
                "social"
            }
        })
    }

    /// Return the effective authentication region, falling back through the
    /// generic region and finally the global default.
    pub fn effective_auth_region(&self) -> &str {
        self.auth_region
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.region
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or(DEFAULT_KIRO_REGION)
    }

    /// Return the effective API region, falling back through the generic
    /// region and finally the global default.
    pub fn effective_api_region(&self) -> &str {
        self.api_region
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.region
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .unwrap_or(DEFAULT_KIRO_REGION)
    }

    /// Whether this record contains a non-empty refresh token.
    pub fn has_refresh_token(&self) -> bool {
        self.refresh_token
            .as_deref()
            .map(str::trim)
            .is_some_and(|value| !value.is_empty())
    }

    /// Account-level local concurrency limit with defaulting and lower-bound
    /// normalization applied.
    pub fn effective_kiro_channel_max_concurrency(&self) -> u64 {
        self.kiro_channel_max_concurrency
            .unwrap_or(DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY)
            .max(1)
    }

    /// Account-level local minimum start interval with defaulting applied.
    pub fn effective_kiro_channel_min_start_interval_ms(&self) -> u64 {
        self.kiro_channel_min_start_interval_ms
            .unwrap_or(DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS)
    }
}

/// Resolve the root directory that stores persisted Kiro account JSON files.
pub fn resolve_auths_dir() -> PathBuf {
    if let Ok(path) = env::var("STATICFLOW_KIRO_AUTHS_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home)
        .join(".static-flow")
        .join("auths")
        .join("kiro")
}

fn sanitize_auth_file_stem(name: &str) -> String {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return "default".to_string();
    }
    let sanitized = trimmed
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') { ch } else { '_' })
        .collect::<String>()
        .trim_matches('_')
        .to_string();
    if sanitized.is_empty() {
        "default".to_string()
    } else {
        sanitized
    }
}

/// Build the canonical auth-file path for a named Kiro account inside `dir`.
pub fn auth_path_for_name_in_dir(dir: &Path, name: &str) -> PathBuf {
    let stem = if name.trim().eq_ignore_ascii_case("default") {
        "default".to_string()
    } else {
        sanitize_auth_file_stem(name)
    };
    dir.join(format!("{stem}.json"))
}

/// Load one auth JSON file if it exists and contains non-empty JSON content.
pub async fn load_auth_file(path: &Path) -> Result<Option<KiroAuthRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let record: KiroAuthRecord = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse `{}`", path.display()))?;
    Ok(Some(record.canonicalize()))
}

/// Persist one canonicalized auth record to its JSON file.
pub async fn save_auth_file(path: &Path, record: &KiroAuthRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create `{}`", parent.display()))?;
    }
    let content =
        serde_json::to_string_pretty(&record.clone().canonicalize()).context("serialize auth")?;
    tokio::fs::write(path, content)
        .await
        .with_context(|| format!("failed to write `{}`", path.display()))
}

/// Delete one auth JSON file if it exists.
pub async fn delete_auth_file(path: &Path) -> Result<()> {
    if path.exists() {
        tokio::fs::remove_file(path)
            .await
            .with_context(|| format!("failed to delete `{}`", path.display()))?;
    }
    Ok(())
}

/// Load and canonicalize every Kiro account JSON file from `dir`.
pub async fn load_auth_records(dir: &Path) -> Result<Vec<KiroAuthRecord>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("failed to read `{}`", dir.display()))?;
    let mut records = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to read entry from `{}`", dir.display()))?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Some(record) = load_auth_file(&path).await? {
            records.push(record);
        }
    }
    records.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(records)
}

/// Save one auth record into the target directory and return the written path.
pub async fn save_auth_record(dir: &Path, record: &KiroAuthRecord) -> Result<PathBuf> {
    let canonical = record.clone().canonicalize();
    let path = auth_path_for_name_in_dir(dir, &canonical.name);
    save_auth_file(&path, &canonical).await?;
    Ok(path)
}

/// Delete one named auth record from the target directory.
///
/// The lookup first tries the canonical file name, then falls back to scanning
/// existing JSON files so old records with non-canonical stems can still be
/// removed cleanly.
pub async fn delete_auth_record(dir: &Path, name: &str) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let candidate = auth_path_for_name_in_dir(dir, name);
    if candidate.exists() {
        delete_auth_file(&candidate).await?;
        return Ok(());
    }
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .with_context(|| format!("failed to read `{}`", dir.display()))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .with_context(|| format!("failed to read entry from `{}`", dir.display()))?
    {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(record) = load_auth_file(&path).await? else {
            continue;
        };
        if record.name == name {
            delete_auth_file(&path).await?;
            return Ok(());
        }
    }
    Ok(())
}

/// Load the persisted "current account" marker from `.current`.
pub async fn load_current_account_name(dir: &Path) -> Result<Option<String>> {
    let path = dir.join(".current");
    if !path.exists() {
        return Ok(None);
    }
    let raw = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read `{}`", path.display()))?;
    let value = raw.trim();
    if value.is_empty() {
        Ok(None)
    } else {
        Ok(Some(value.to_string()))
    }
}

/// Persist the selected current account name into `.current`.
pub async fn save_current_account_name(dir: &Path, name: &str) -> Result<()> {
    tokio::fs::create_dir_all(dir)
        .await
        .with_context(|| format!("failed to create `{}`", dir.display()))?;
    let path = dir.join(".current");
    tokio::fs::write(&path, name.trim())
        .await
        .with_context(|| format!("failed to write `{}`", path.display()))
}

/// Remove the persisted `.current` marker if present.
pub async fn clear_current_account_name(dir: &Path) -> Result<()> {
    let path = dir.join(".current");
    if path.exists() {
        tokio::fs::remove_file(&path)
            .await
            .with_context(|| format!("failed to delete `{}`", path.display()))?;
    }
    Ok(())
}
