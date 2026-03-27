//! Multi-account pool for the LLM gateway.
//!
//! Each account is persisted as a Codex-compatible `auth.json` file under
//! `~/.staticflow/auths/<name>.json`. The pool loads all files on startup and
//! keeps an in-memory snapshot that the gateway proxy consults for routing.

use std::{
    collections::{HashMap, HashSet},
    env,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use super::runtime::CodexAuthSnapshot;

// ---------------------------------------------------------------------------
// On-disk format (Codex auth.json compatible)
// ---------------------------------------------------------------------------

/// Matches the Codex `~/.codex/auth.json` shape exactly so the file can be
/// copied there and used by `codex` directly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodexAuthFile {
    #[serde(default)]
    pub auth_mode: Option<String>,
    #[serde(rename = "OPENAI_API_KEY", default)]
    pub openai_api_key: Option<serde_json::Value>,
    #[serde(default)]
    pub tokens: Option<CodexAuthTokens>,
    #[serde(default)]
    pub last_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct CodexAuthTokens {
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub account_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AccountSettingsFile {
    #[serde(default)]
    pub map_gpt53_codex_to_spark: bool,
}

// ---------------------------------------------------------------------------
// In-memory account model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccountStatus {
    Active,
    Unavailable,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Unavailable => "unavailable",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CodexAccount {
    pub name: String,
    pub access_token: String,
    pub account_id: Option<String>,
    pub refresh_token: String,
    pub id_token: String,
    pub map_gpt53_codex_to_spark: bool,
    pub last_refresh: Option<DateTime<Utc>>,
    pub status: AccountStatus,
}

impl CodexAccount {
    pub fn to_auth_snapshot(&self) -> CodexAuthSnapshot {
        CodexAuthSnapshot::from_tokens(self.access_token.clone(), self.account_id.clone())
    }
}

/// Cached rate-limit snapshot for one account, updated by the background
/// refresh task. Stores the full bucket list so the public status page can
/// render all fields (resets_at, plan_type, credits, window_duration, etc.)
/// without re-fetching upstream.
#[derive(Debug, Clone, Default)]
pub(crate) struct AccountRateLimitSnapshot {
    /// Full buckets as returned by `map_rate_limit_status_payload`.
    pub buckets: Vec<super::types::LlmGatewayRateLimitBucketView>,
    pub last_checked_at: Option<i64>,
}

impl AccountRateLimitSnapshot {
    /// Convenience: primary remaining percent from the first primary bucket.
    pub fn primary_remaining_percent(&self) -> Option<f64> {
        self.buckets
            .iter()
            .find(|b| b.is_primary)
            .and_then(|b| b.primary.as_ref())
            .map(|w| w.remaining_percent)
    }

    /// Convenience: secondary remaining percent from the first primary bucket.
    pub fn secondary_remaining_percent(&self) -> Option<f64> {
        self.buckets
            .iter()
            .find(|b| b.is_primary)
            .and_then(|b| b.secondary.as_ref())
            .map(|w| w.remaining_percent)
    }

    pub fn primary_plan_type(&self) -> Option<String> {
        self.buckets
            .iter()
            .find(|bucket| bucket.is_primary)
            .and_then(|bucket| bucket.plan_type.clone())
    }

    pub fn is_gpt_pro(&self) -> bool {
        self.primary_plan_type()
            .as_deref()
            .map(str::trim)
            .is_some_and(|plan| {
                let normalized = plan.to_ascii_lowercase();
                normalized == "pro" || normalized == "gpt pro"
            })
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AccountSummarySnapshot {
    pub name: String,
    pub status: AccountStatus,
    pub account_id: Option<String>,
    pub rate_limits: AccountRateLimitSnapshot,
    pub last_refresh_ms: Option<i64>,
    pub map_gpt53_codex_to_spark: bool,
}

// ---------------------------------------------------------------------------
// Account pool
// ---------------------------------------------------------------------------

pub(crate) struct AccountPool {
    accounts: RwLock<HashMap<String, Arc<RwLock<CodexAccount>>>>,
    rate_limits: RwLock<HashMap<String, AccountRateLimitSnapshot>>,
    auth_file_mtimes: RwLock<HashMap<String, Option<SystemTime>>>,
    auths_dir: PathBuf,
}

impl AccountPool {
    pub fn new(auths_dir: PathBuf) -> Self {
        Self {
            accounts: RwLock::new(HashMap::new()),
            rate_limits: RwLock::new(HashMap::new()),
            auth_file_mtimes: RwLock::new(HashMap::new()),
            auths_dir,
        }
    }

    #[allow(dead_code)]
    pub fn auths_dir(&self) -> &Path {
        &self.auths_dir
    }

    /// Load all `*.json` files from the auths directory into memory.
    /// If the directory is empty, automatically imports `~/.codex/auth.json`
    /// as the `default` account so the gateway has at least one upstream
    /// credential without manual setup.
    pub async fn load_all(&self) -> Result<usize> {
        tokio::fs::create_dir_all(&self.auths_dir)
            .await
            .with_context(|| format!("failed to create auths dir: {}", self.auths_dir.display()))?;

        let mut entries = tokio::fs::read_dir(&self.auths_dir)
            .await
            .with_context(|| format!("failed to read auths dir: {}", self.auths_dir.display()))?;

        let mut loaded = 0usize;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let name = match path.file_stem().and_then(|s| s.to_str()) {
                Some(n) if !n.is_empty() => n.to_string(),
                _ => continue,
            };
            match load_account_from_file(&path).await {
                Ok(mut account) => {
                    account.name = name.clone();
                    let entry = Arc::new(RwLock::new(account));
                    self.accounts.write().await.insert(name.clone(), entry);
                    self.auth_file_mtimes
                        .write()
                        .await
                        .insert(name.clone(), auth_file_modified_at(&path).await);
                    loaded += 1;
                },
                Err(err) => {
                    tracing::warn!(
                        path = %path.display(),
                        "Failed to load codex account file, skipping: {err:#}"
                    );
                },
            }
        }

        // When the auths directory is empty, seed it from ~/.codex/auth.json
        // so that existing single-account setups work without manual import.
        if loaded == 0 {
            if let Some(imported) = self.try_import_codex_auth_json().await {
                loaded = imported;
            }
        }

        Ok(loaded)
    }

    /// Try to import the Codex CLI auth.json as the `default` account.
    async fn try_import_codex_auth_json(&self) -> Option<usize> {
        let codex_path = resolve_codex_auth_path();
        let mut account = match load_account_from_file(&codex_path).await {
            Ok(a) => a,
            Err(err) => {
                tracing::debug!(
                    path = %codex_path.display(),
                    "No codex auth.json to seed account pool: {err:#}"
                );
                return None;
            },
        };
        account.name = "default".to_string();
        if let Err(err) = persist_account_to_file(&self.auths_dir, &account).await {
            tracing::warn!("Failed to persist seeded default account: {err:#}");
            return None;
        }
        let entry = Arc::new(RwLock::new(account));
        self.accounts
            .write()
            .await
            .insert("default".to_string(), entry);
        let persisted_path = auth_file_path(&self.auths_dir, "default");
        self.auth_file_mtimes
            .write()
            .await
            .insert("default".to_string(), auth_file_modified_at(&persisted_path).await);
        tracing::info!(
            codex_path = %codex_path.display(),
            "Seeded account pool with default account from codex auth.json"
        );
        Some(1)
    }

    /// Check whether the given account name already exists.
    pub async fn exists(&self, name: &str) -> bool {
        self.accounts.read().await.contains_key(name)
    }

    /// Insert a new account into the pool and persist it to disk.
    pub async fn insert(&self, account: CodexAccount) -> Result<()> {
        let name = account.name.clone();
        persist_account_to_file(&self.auths_dir, &account).await?;
        let modified_at = auth_file_modified_at(&auth_file_path(&self.auths_dir, &name)).await;
        let entry = Arc::new(RwLock::new(account));
        self.accounts.write().await.insert(name.clone(), entry);
        self.auth_file_mtimes
            .write()
            .await
            .insert(name, modified_at);
        Ok(())
    }

    /// Remove an account from the pool and delete its file.
    pub async fn remove(&self, name: &str) -> Result<bool> {
        let existed = self.accounts.write().await.remove(name).is_some();
        self.rate_limits.write().await.remove(name);
        self.auth_file_mtimes.write().await.remove(name);
        let path = self.auths_dir.join(format!("{name}.json"));
        if path.is_file() {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("failed to delete {}", path.display()))?;
        }
        let settings_path = self.auths_dir.join(format!("{name}.meta"));
        if settings_path.is_file() {
            tokio::fs::remove_file(&settings_path)
                .await
                .with_context(|| format!("failed to delete {}", settings_path.display()))?;
        }
        Ok(existed)
    }

    /// Return a snapshot of all accounts with their rate-limit data.
    pub async fn list_summaries(&self) -> Vec<AccountSummarySnapshot> {
        let accounts = self.accounts.read().await;
        let rate_limits = self.rate_limits.read().await;
        let mut out = Vec::with_capacity(accounts.len());
        for (name, entry) in accounts.iter() {
            let account = entry.read().await;
            let rl = rate_limits.get(name).cloned().unwrap_or_default();
            let last_refresh_ms = account.last_refresh.map(|dt| dt.timestamp_millis());
            out.push(AccountSummarySnapshot {
                name: name.clone(),
                status: account.status,
                account_id: account.account_id.clone(),
                rate_limits: rl,
                last_refresh_ms,
                map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            });
        }
        out
    }

    /// Select the best active account by remaining primary quota, optionally
    /// restricted to a configured subset.
    pub async fn select_best_account(
        &self,
        allowed_names: Option<&HashSet<String>>,
    ) -> Option<(String, CodexAuthSnapshot, bool)> {
        let accounts = self.accounts.read().await;
        let rate_limits = self.rate_limits.read().await;

        let mut best: Option<(f64, String, CodexAuthSnapshot, bool)> = None;
        for (name, entry) in accounts.iter() {
            if let Some(allowed_names) = allowed_names {
                if !allowed_names.contains(name) {
                    continue;
                }
            }
            let account = entry.read().await;
            if account.status != AccountStatus::Active {
                continue;
            }
            let remaining = rate_limits
                .get(name)
                .and_then(|rl| rl.primary_remaining_percent())
                .unwrap_or(100.0);
            if remaining <= 0.0 {
                continue;
            }
            let snapshot = account.to_auth_snapshot();
            match &best {
                Some((best_remaining, _, _, _)) if remaining > *best_remaining => {
                    best =
                        Some((remaining, name.clone(), snapshot, account.map_gpt53_codex_to_spark));
                },
                None => {
                    best =
                        Some((remaining, name.clone(), snapshot, account.map_gpt53_codex_to_spark));
                },
                _ => {},
            }
        }
        best.map(|(_, name, snapshot, map_gpt53_codex_to_spark)| {
            (name, snapshot, map_gpt53_codex_to_spark)
        })
    }

    /// Get a specific account by name.
    pub async fn get_account(&self, name: &str) -> Option<(CodexAuthSnapshot, bool)> {
        let accounts = self.accounts.read().await;
        let entry = accounts.get(name)?;
        let account = entry.read().await;
        if account.status != AccountStatus::Active {
            return None;
        }
        Some((account.to_auth_snapshot(), account.map_gpt53_codex_to_spark))
    }

    pub async fn set_map_gpt53_codex_to_spark(&self, name: &str, enabled: bool) -> Result<bool> {
        let accounts = self.accounts.read().await;
        let Some(entry) = accounts.get(name) else {
            return Ok(false);
        };
        {
            let mut account = entry.write().await;
            account.map_gpt53_codex_to_spark = enabled;
            persist_account_settings_to_file(&self.auths_dir, &account).await?;
        }
        Ok(true)
    }

    /// Return clones of all account entries for the refresh task.
    pub async fn all_entries(&self) -> Vec<(String, Arc<RwLock<CodexAccount>>)> {
        self.accounts
            .read()
            .await
            .iter()
            .map(|(name, entry)| (name.clone(), entry.clone()))
            .collect()
    }

    /// Update the cached rate-limit snapshot for an account.
    pub async fn update_rate_limit(&self, name: &str, snapshot: AccountRateLimitSnapshot) {
        self.rate_limits
            .write()
            .await
            .insert(name.to_string(), snapshot);
    }

    /// Persist updated tokens for an account back to its file.
    pub async fn persist(&self, name: &str) -> Result<()> {
        let accounts = self.accounts.read().await;
        let entry = accounts
            .get(name)
            .with_context(|| format!("account `{name}` not found in pool"))?;
        let account = entry.read().await;
        persist_account_to_file(&self.auths_dir, &account).await?;
        drop(account);
        self.auth_file_mtimes.write().await.insert(
            name.to_string(),
            auth_file_modified_at(&auth_file_path(&self.auths_dir, name)).await,
        );
        Ok(())
    }

    /// Reload one account from disk when its auth file mtime changed.
    pub async fn sync_account_from_disk_if_changed(
        &self,
        name: &str,
        entry: &Arc<RwLock<CodexAccount>>,
    ) -> Result<bool> {
        let path = auth_file_path(&self.auths_dir, name);
        let modified_at = auth_file_modified_at(&path).await;
        let cached_modified_at = self
            .auth_file_mtimes
            .read()
            .await
            .get(name)
            .cloned()
            .flatten();
        if modified_matches(cached_modified_at, modified_at) {
            return Ok(false);
        }

        let mut account = load_account_from_file(&path).await?;
        account.name = name.to_string();
        *entry.write().await = account;
        self.auth_file_mtimes
            .write()
            .await
            .insert(name.to_string(), modified_at);
        Ok(true)
    }

    /// Return whether the pool has any accounts.
    #[allow(dead_code)]
    pub async fn is_empty(&self) -> bool {
        self.accounts.read().await.is_empty()
    }
}

// ---------------------------------------------------------------------------
// File I/O helpers
// ---------------------------------------------------------------------------

async fn load_account_from_file(path: &Path) -> Result<CodexAccount> {
    let raw = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    let auth_file: CodexAuthFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    let tokens = auth_file
        .tokens
        .with_context(|| format!("{}: missing tokens", path.display()))?;
    let access_token = tokens
        .access_token
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .with_context(|| format!("{}: missing access_token", path.display()))?;
    let refresh_token = tokens
        .refresh_token
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_default();
    let id_token = tokens
        .id_token
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_default();
    let account_id = tokens
        .account_id
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let settings = load_account_settings_from_file(path)
        .await
        .unwrap_or_default();

    Ok(CodexAccount {
        name: String::new(), // caller fills in
        access_token,
        account_id,
        refresh_token,
        id_token,
        map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
        last_refresh: auth_file.last_refresh,
        status: AccountStatus::Active,
    })
}

async fn persist_account_to_file(auths_dir: &Path, account: &CodexAccount) -> Result<()> {
    let auth_file = CodexAuthFile {
        auth_mode: Some("chatgpt".to_string()),
        openai_api_key: Some(serde_json::Value::Null),
        tokens: Some(CodexAuthTokens {
            id_token: Some(account.id_token.clone()),
            access_token: Some(account.access_token.clone()),
            refresh_token: Some(account.refresh_token.clone()),
            account_id: account.account_id.clone(),
        }),
        last_refresh: account.last_refresh.or_else(|| Some(Utc::now())),
    };
    let json = serde_json::to_string_pretty(&auth_file)
        .context("failed to serialize account auth file")?;
    let path = auths_dir.join(format!("{}.json", account.name));
    tokio::fs::write(&path, json.as_bytes())
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    persist_account_settings_to_file(auths_dir, account).await?;
    Ok(())
}

async fn load_account_settings_from_file(auth_path: &Path) -> Result<AccountSettingsFile> {
    let path = settings_path_for_auth_file(auth_path);
    let raw = tokio::fs::read_to_string(&path)
        .await
        .with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("failed to parse {}", path.display()))
}

async fn persist_account_settings_to_file(auths_dir: &Path, account: &CodexAccount) -> Result<()> {
    let path = auths_dir.join(format!("{}.meta", account.name));
    if !account.map_gpt53_codex_to_spark {
        if path.is_file() {
            tokio::fs::remove_file(&path)
                .await
                .with_context(|| format!("failed to delete {}", path.display()))?;
        }
        return Ok(());
    }

    let settings = AccountSettingsFile {
        map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
    };
    let json = serde_json::to_string_pretty(&settings)
        .context("failed to serialize account settings file")?;
    tokio::fs::write(&path, json.as_bytes())
        .await
        .with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn auth_file_path(auths_dir: &Path, name: &str) -> PathBuf {
    auths_dir.join(format!("{name}.json"))
}

async fn auth_file_modified_at(path: &Path) -> Option<SystemTime> {
    tokio::fs::metadata(path).await.ok()?.modified().ok()
}

fn modified_matches(left: Option<SystemTime>, right: Option<SystemTime>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => left == right,
        (None, None) => true,
        _ => false,
    }
}

fn settings_path_for_auth_file(auth_path: &Path) -> PathBuf {
    auth_path.with_extension("meta")
}

/// Resolve the auths directory, honoring `STATICFLOW_AUTHS_DIR` env override.
pub(crate) fn resolve_auths_dir() -> PathBuf {
    if let Ok(path) = env::var("STATICFLOW_AUTHS_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home).join(".static-flow").join("auths")
}

/// Resolve the Codex CLI auth.json path used as the seed source when the
/// auths directory is empty.
fn resolve_codex_auth_path() -> PathBuf {
    if let Ok(path) = env::var("CODEX_AUTH_JSON_PATH") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let home = env::var("HOME").unwrap_or_else(|_| "/home/ts_user".to_string());
    PathBuf::from(home).join(".codex").join("auth.json")
}

/// Validate that an account name is safe for use as a filename.
pub(crate) fn validate_account_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("account name is required".to_string());
    }
    if trimmed.len() > 64 {
        return Err("account name must be 64 characters or fewer".to_string());
    }
    if !trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("account name must contain only ASCII letters, digits, hyphens, or \
                    underscores"
            .to_string());
    }
    Ok(trimmed.to_string())
}
