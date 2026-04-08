//! Runtime state and token lifecycle management for the Kiro gateway.
//!
//! This module owns refresh-token driven access token renewal and the
//! account-backed status/runtime caches that sit beneath the
//! Anthropic-compatible HTTP handlers.

use std::{
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use parking_lot::RwLock;
use static_flow_shared::llm_gateway_store::LlmGatewayStore;
use tokio::sync::Mutex;

use super::{
    auth_file::{
        delete_auth_record, load_auth_records, resolve_auths_dir, save_auth_record, KiroAuthRecord,
        DEFAULT_KIRO_VERSION, DEFAULT_NODE_VERSION, DEFAULT_SYSTEM_VERSION,
    },
    cache_sim::KiroCacheSimulator,
    local_import,
    provider::{build_client, KIRO_AUX_CLIENT_PROFILE},
    scheduler::KiroRequestScheduler,
    status_cache::KiroStatusCacheSnapshot,
    wire::{
        IdcRefreshRequest, IdcRefreshResponse, RefreshRequest, RefreshResponse, UsageLimitsResponse,
    },
};
use crate::{state::LlmGatewayRuntimeConfig, upstream_proxy::UpstreamProxyRegistry};

const REFRESH_EARLY_MINUTES: i64 = 10;
const KIRO_USAGE_AWS_SDK_VERSION: &str = "1.0.0";
const KIRO_IDC_AWS_SDK_VERSION: &str = "3.980.0";
const KIRO_IDC_AMZ_SDK_REQUEST: &str = "attempt=1; max=4";

/// Permanent refresh-token failure returned by the upstream OAuth/OIDC
/// endpoints. Unlike transient refresh errors, this means the stored
/// credential can no longer mint new access tokens and should be disabled
/// immediately instead of being retried on every request.
#[derive(Debug)]
struct RefreshTokenInvalidGrantError {
    message: String,
}

impl fmt::Display for RefreshTokenInvalidGrantError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for RefreshTokenInvalidGrantError {}

/// Holds the authenticated identity and resolved access token for a single Kiro
/// API call.
///
/// Produced by [`KiroTokenManager::ensure_context_for_account`] after
/// validating (and optionally refreshing) the stored credentials. Handlers use
/// the `token` field directly as a Bearer token in upstream requests.
#[derive(Debug, Clone)]
pub struct CallContext {
    /// The full auth record for the account, including refresh token and
    /// metadata.
    pub auth: KiroAuthRecord,
    /// A valid, non-empty access token ready for use in `Authorization: Bearer`
    /// headers.
    pub token: String,
}

/// Top-level runtime state shared across all Kiro gateway request handlers.
///
/// Wraps the [`KiroTokenManager`] (for credential lifecycle), a cached status
/// snapshot (for dashboard polling), and the per-account request scheduler.
///
/// Cloneable via inner `Arc`s; intended to be stored in Axum's application
/// state.
#[derive(Clone)]
pub struct KiroGatewayRuntimeState {
    pub(crate) token_manager: Arc<KiroTokenManager>,
    pub(crate) status_cache: Arc<RwLock<KiroStatusCacheSnapshot>>,
    pub(crate) request_scheduler: Arc<KiroRequestScheduler>,
    pub(crate) cache_simulator: Arc<KiroCacheSimulator>,
    pub(crate) runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
    pub(crate) upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
}

impl KiroGatewayRuntimeState {
    /// Construct the shared Kiro runtime and migrate any legacy global
    /// scheduler defaults into account-local settings.
    pub async fn new(
        _store: Arc<LlmGatewayStore>,
        runtime_config: Arc<RwLock<LlmGatewayRuntimeConfig>>,
        upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
    ) -> Result<Self> {
        let scheduler_defaults = runtime_config.read().clone();
        let token_manager = Arc::new(KiroTokenManager::new(upstream_proxy_registry.clone()).await?);
        let migrated_accounts = token_manager
            .backfill_missing_scheduler_limits(
                scheduler_defaults.kiro_channel_max_concurrency,
                scheduler_defaults.kiro_channel_min_start_interval_ms,
            )
            .await?;
        if migrated_accounts > 0 {
            tracing::info!(
                migrated_accounts,
                inherited_max_concurrency = scheduler_defaults.kiro_channel_max_concurrency,
                inherited_min_start_interval_ms =
                    scheduler_defaults.kiro_channel_min_start_interval_ms,
                "backfilled missing per-account Kiro scheduler settings from legacy global config"
            );
        }
        Ok(Self {
            token_manager,
            status_cache: Arc::new(RwLock::new(KiroStatusCacheSnapshot::default())),
            request_scheduler: KiroRequestScheduler::new(),
            cache_simulator: Arc::new(KiroCacheSimulator::default()),
            runtime_config,
            upstream_proxy_registry,
        })
    }

    /// Return a clone of the latest cached per-account status snapshot.
    pub async fn cached_status_snapshot(&self) -> KiroStatusCacheSnapshot {
        self.status_cache.read().clone()
    }
}

/// File-backed Kiro credential manager with refresh-token support.
///
/// The manager exposes CRUD helpers for the persisted auth files and guarantees
/// that concurrent refresh attempts are serialized per account so independent
/// Kiro credentials can refresh in parallel.
pub struct KiroTokenManager {
    auths_dir: PathBuf,
    refresh_locks: RwLock<HashMap<String, Arc<Mutex<()>>>>,
    upstream_proxy_registry: Arc<UpstreamProxyRegistry>,
}

impl KiroTokenManager {
    /// Initialize the token manager from the auth directory.
    pub async fn new(upstream_proxy_registry: Arc<UpstreamProxyRegistry>) -> Result<Self> {
        let auths_dir = resolve_auths_dir();
        Ok(Self {
            auths_dir,
            refresh_locks: RwLock::new(HashMap::new()),
            upstream_proxy_registry,
        })
    }

    /// Load all configured accounts.
    pub async fn list_auths(&self) -> Result<Vec<KiroAuthRecord>> {
        load_auth_records(&self.auths_dir).await
    }

    /// Load one configured account by name.
    pub async fn auth_by_name(&self, name: &str) -> Result<Option<KiroAuthRecord>> {
        let records = load_auth_records(&self.auths_dir).await?;
        Ok(records.into_iter().find(|record| record.name == name))
    }

    /// Insert or update a persisted auth record.
    pub async fn upsert_auth(&self, auth: KiroAuthRecord) -> Result<KiroAuthRecord> {
        let auth = auth.canonicalize();
        save_auth_record(&self.auths_dir, &auth).await?;
        Ok(auth)
    }

    /// Delete one configured account.
    pub async fn delete_auth(&self, name: &str) -> Result<()> {
        delete_auth_record(&self.auths_dir, name).await?;
        self.refresh_locks.write().remove(name);
        Ok(())
    }

    /// Import one account from the local Kiro CLI SQLite store.
    pub async fn import_local_account(
        &self,
        name: Option<&str>,
        sqlite_path: Option<&str>,
    ) -> Result<KiroAuthRecord> {
        let sqlite_path = sqlite_path
            .map(PathBuf::from)
            .unwrap_or_else(local_import::default_sqlite_path);
        let auth = local_import::import_from_sqlite(&sqlite_path, name).await?;
        self.upsert_auth(auth).await
    }

    /// Fill in missing per-account scheduler settings from the legacy global
    /// defaults stored in runtime config.
    pub async fn backfill_missing_scheduler_limits(
        &self,
        default_max_concurrency: u64,
        default_min_start_interval_ms: u64,
    ) -> Result<usize> {
        let auths = load_auth_records(&self.auths_dir).await?;
        let mut updated = 0usize;
        for mut auth in auths {
            let mut changed = false;
            if auth.kiro_channel_max_concurrency.is_none() {
                auth.kiro_channel_max_concurrency = Some(default_max_concurrency.max(1));
                changed = true;
            }
            if auth.kiro_channel_min_start_interval_ms.is_none() {
                auth.kiro_channel_min_start_interval_ms = Some(default_min_start_interval_ms);
                changed = true;
            }
            if !changed {
                continue;
            }
            save_auth_record(&self.auths_dir, &auth).await?;
            updated += 1;
        }
        if updated > 0 {
            tracing::info!(updated, "backfilled missing scheduler limits on kiro accounts");
        }
        Ok(updated)
    }

    /// Return a ready-to-use access token for `account_name`, refreshing it if
    /// required.
    pub async fn ensure_context_for_account(
        &self,
        account_name: &str,
        force_refresh: bool,
    ) -> Result<CallContext> {
        let auth = self
            .auth_by_name(account_name)
            .await?
            .ok_or_else(|| anyhow!("kiro account `{account_name}` is not configured"))?;
        if auth.disabled {
            bail!("kiro account `{account_name}` is disabled");
        }
        if !force_refresh && !needs_refresh(&auth) {
            let token = auth
                .access_token
                .clone()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("kiro access token missing"))?;
            return Ok(CallContext {
                auth,
                token,
            });
        }

        let refresh_lock = self.refresh_lock_for_account(account_name).await;
        let _guard = refresh_lock.lock().await;
        let latest = self
            .auth_by_name(account_name)
            .await?
            .ok_or_else(|| anyhow!("kiro account `{account_name}` is not configured"))?;
        if latest.disabled {
            bail!("kiro account `{account_name}` is disabled");
        }
        if !force_refresh && !needs_refresh(&latest) {
            let token = latest
                .access_token
                .clone()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .ok_or_else(|| anyhow!("kiro access token missing"))?;
            return Ok(CallContext {
                auth: latest,
                token,
            });
        }

        tracing::info!(
            account_name = %latest.name,
            auth_method = latest.auth_method(),
            reason = if force_refresh {
                "forced_retry_after_upstream_auth_failure"
            } else {
                "missing_or_expiring_access_token"
            },
            expires_at = ?latest.expires_at,
            "refreshing kiro access token"
        );
        let refreshed = match refresh_auth(self.upstream_proxy_registry.as_ref(), &latest).await {
            Ok(refreshed) => refreshed,
            Err(err) => {
                let invalid_refresh_message = err
                    .downcast_ref::<RefreshTokenInvalidGrantError>()
                    .map(|value| value.to_string());
                if let Some(message) = invalid_refresh_message {
                    tracing::error!(
                        account_name = %latest.name,
                        auth_method = latest.auth_method(),
                        error = %message,
                        "kiro refresh token is permanently invalid; disabling account"
                    );
                    disable_auth_for_invalid_refresh_token(&self.auths_dir, &latest)
                        .await
                        .with_context(|| {
                            format!(
                                "disable kiro account `{}` after invalid refresh token",
                                latest.name
                            )
                        })?;
                }
                return Err(err);
            },
        };
        self.persist_refreshed_auth(&refreshed).await?;
        tracing::info!(
            account_name = %refreshed.name,
            auth_method = refreshed.auth_method(),
            expires_at = ?refreshed.expires_at,
            has_profile_arn = refreshed.profile_arn.is_some(),
            "refreshed kiro access token"
        );
        let token = refreshed
            .access_token
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("kiro access token missing after refresh"))?;
        Ok(CallContext {
            auth: refreshed,
            token,
        })
    }

    /// Fetch and normalize the upstream usage-limit snapshot for one account.
    ///
    /// When `force_refresh` is `true`, token renewal happens before the usage
    /// query even if the local expiry check would otherwise skip it.
    pub async fn fetch_usage_limits_for_account(
        &self,
        account_name: &str,
        force_refresh: bool,
    ) -> Result<UsageLimitsResponse> {
        let ctx = self
            .ensure_context_for_account(account_name, force_refresh)
            .await?;
        let auth = ctx.auth.clone();
        let region = auth.effective_api_region().to_string();
        let host = format!("q.{region}.amazonaws.com");
        let url = if let Some(profile_arn) = auth.profile_arn.as_deref() {
            format!(
                "https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST&profileArn={}",
                urlencoding::encode(profile_arn)
            )
        } else {
            format!("https://{host}/getUsageLimits?origin=AI_EDITOR&resourceType=AGENTIC_REQUEST")
        };
        let (client, _) =
            build_client(self.upstream_proxy_registry.as_ref(), &auth, KIRO_AUX_CLIENT_PROFILE)
                .await?;
        let machine_id = super::machine_id::generate_from_auth(&auth)
            .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
        let (amz_user_agent, user_agent) = usage_request_user_agents(&machine_id);
        let response = client
            .get(url)
            .header("x-amz-user-agent", amz_user_agent)
            .header("user-agent", user_agent)
            .header("host", host)
            .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
            .header("amz-sdk-request", "attempt=1; max=1")
            .header("authorization", format!("Bearer {}", ctx.token))
            .header("connection", "close")
            .send()
            .await
            .context("request kiro usage limits")?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            bail!("kiro usage limit request failed: {status} {body}");
        }
        let usage: UsageLimitsResponse = response.json().await.context("parse usage limits")?;
        tracing::info!(
            account_name = %auth.name,
            subscription_title = ?usage.subscription_title(),
            current_usage = usage.current_usage(),
            usage_limit = usage.usage_limit(),
            next_date_reset = ?usage.next_date_reset,
            "fetched kiro usage limits"
        );

        let mut next_auth = auth.clone();
        if let Some(subscription_title) = usage.subscription_title() {
            if next_auth.subscription_title.as_deref() != Some(subscription_title) {
                next_auth.subscription_title = Some(subscription_title.to_string());
                self.persist_refreshed_auth(&next_auth).await?;
            }
        }

        Ok(usage)
    }

    async fn persist_refreshed_auth(&self, auth: &KiroAuthRecord) -> Result<()> {
        save_auth_record(&self.auths_dir, auth).await?;
        Ok(())
    }

    async fn refresh_lock_for_account(&self, account_name: &str) -> Arc<Mutex<()>> {
        if let Some(lock) = self.refresh_locks.read().get(account_name).cloned() {
            return lock;
        }
        let mut refresh_locks = self.refresh_locks.write();
        refresh_locks
            .entry(account_name.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}

fn needs_refresh(auth: &KiroAuthRecord) -> bool {
    let Some(access_token) = auth.access_token.as_deref() else {
        return true;
    };
    if access_token.trim().is_empty() {
        return true;
    }
    let expires_at = auth
        .expires_at
        .as_deref()
        .and_then(parse_rfc3339_utc)
        .or_else(|| access_token_expiry(access_token));
    let Some(expires_at) = expires_at else {
        return true;
    };
    expires_at <= Utc::now() + Duration::minutes(REFRESH_EARLY_MINUTES)
}

async fn refresh_auth(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &KiroAuthRecord,
) -> Result<KiroAuthRecord> {
    validate_refresh_token(auth)?;
    let auth_method = auth.auth_method();
    if matches!(auth_method, "idc" | "builder-id" | "iam") {
        refresh_idc(proxy_registry, auth).await
    } else {
        refresh_social(proxy_registry, auth).await
    }
}

fn validate_refresh_token(auth: &KiroAuthRecord) -> Result<()> {
    let refresh_token = auth
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("missing kiro refresh token"))?;
    if refresh_token.len() < 100 || refresh_token.ends_with("...") || refresh_token.contains("...")
    {
        bail!("kiro refresh token appears truncated");
    }
    Ok(())
}

async fn refresh_social(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &KiroAuthRecord,
) -> Result<KiroAuthRecord> {
    let refresh_token = auth
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token"))?;
    let region = auth.effective_auth_region();
    let url = format!("https://prod.{region}.auth.desktop.kiro.dev/refreshToken");
    let host = format!("prod.{region}.auth.desktop.kiro.dev");
    let (client, _) = build_client(proxy_registry, auth, KIRO_AUX_CLIENT_PROFILE).await?;
    let machine_id = super::machine_id::generate_from_auth(auth)
        .ok_or_else(|| anyhow!("failed to derive kiro machine id"))?;
    let response = client
        .post(url)
        .header("accept", "application/json, text/plain, */*")
        .header("content-type", "application/json")
        .header("user-agent", social_refresh_user_agent(&machine_id))
        .header("accept-encoding", "gzip, compress, deflate, br")
        .header("host", host)
        .header("connection", "close")
        .json(&RefreshRequest {
            refresh_token,
        })
        .send()
        .await
        .context("refresh kiro social token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if is_invalid_refresh_token_grant(status.as_u16(), &body) {
            tracing::error!(
                account_name = %auth.name,
                auth_method = auth.auth_method(),
                status = %status,
                body_preview = %summarize_refresh_error_body(&body),
                "kiro social refresh token returned invalid_grant"
            );
            return Err(RefreshTokenInvalidGrantError {
                message: format!("kiro social refresh token is invalid: {status} {body}"),
            }
            .into());
        }
        bail!("kiro social token refresh failed: {status} {body}");
    }
    let payload: RefreshResponse = response.json().await.context("parse refresh response")?;
    let mut next_auth = auth.clone();
    next_auth.access_token = Some(payload.access_token);
    if let Some(refresh_token) = payload.refresh_token {
        next_auth.refresh_token = Some(refresh_token);
    }
    if let Some(profile_arn) = payload.profile_arn {
        next_auth.profile_arn = Some(profile_arn);
    }
    next_auth.expires_at =
        derive_refreshed_expires_at(next_auth.access_token.as_deref(), payload.expires_in);
    Ok(next_auth)
}

async fn refresh_idc(
    proxy_registry: &UpstreamProxyRegistry,
    auth: &KiroAuthRecord,
) -> Result<KiroAuthRecord> {
    let refresh_token = auth
        .refresh_token
        .clone()
        .ok_or_else(|| anyhow!("missing refresh token"))?;
    let client_id = auth
        .client_id
        .clone()
        .ok_or_else(|| anyhow!("missing kiro clientId"))?;
    let client_secret = auth
        .client_secret
        .clone()
        .ok_or_else(|| anyhow!("missing kiro clientSecret"))?;
    let region = auth.effective_auth_region();
    let (client, _) = build_client(proxy_registry, auth, KIRO_AUX_CLIENT_PROFILE).await?;
    let response = client
        .post(format!("https://oidc.{region}.amazonaws.com/token"))
        .header("content-type", "application/json")
        .header("host", format!("oidc.{region}.amazonaws.com"))
        .header("amz-sdk-invocation-id", uuid::Uuid::new_v4().to_string())
        .header("amz-sdk-request", KIRO_IDC_AMZ_SDK_REQUEST)
        .header("connection", "close")
        .header("x-amz-user-agent", idc_refresh_amz_user_agent())
        .header("accept", "*/*")
        .header("user-agent", idc_refresh_user_agent())
        .json(&IdcRefreshRequest {
            client_id,
            client_secret,
            refresh_token,
            grant_type: "refresh_token".to_string(),
        })
        .send()
        .await
        .context("refresh kiro idc token")?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if is_invalid_refresh_token_grant(status.as_u16(), &body) {
            tracing::error!(
                account_name = %auth.name,
                auth_method = auth.auth_method(),
                status = %status,
                body_preview = %summarize_refresh_error_body(&body),
                "kiro idc refresh token returned invalid_grant"
            );
            return Err(RefreshTokenInvalidGrantError {
                message: format!("kiro idc refresh token is invalid: {status} {body}"),
            }
            .into());
        }
        bail!("kiro idc token refresh failed: {status} {body}");
    }
    let payload: IdcRefreshResponse = response.json().await.context("parse idc refresh")?;
    let mut next_auth = auth.clone();
    next_auth.access_token = Some(payload.access_token);
    if let Some(refresh_token) = payload.refresh_token {
        next_auth.refresh_token = Some(refresh_token);
    }
    if let Some(profile_arn) = payload.profile_arn {
        next_auth.profile_arn = Some(profile_arn);
    }
    next_auth.expires_at =
        derive_refreshed_expires_at(next_auth.access_token.as_deref(), payload.expires_in);
    Ok(next_auth)
}

fn derive_refreshed_expires_at(
    access_token: Option<&str>,
    expires_in: Option<i64>,
) -> Option<String> {
    if let Some(expires_in) = expires_in.filter(|value| *value > 0) {
        return Some((Utc::now() + Duration::seconds(expires_in)).to_rfc3339());
    }
    access_token.and_then(jwt_exp_to_rfc3339)
}

fn is_invalid_refresh_token_grant(status: u16, body: &str) -> bool {
    status == 400
        && body.contains("\"invalid_grant\"")
        && body.contains("Invalid refresh token provided")
}

async fn disable_auth_for_invalid_refresh_token(dir: &Path, auth: &KiroAuthRecord) -> Result<()> {
    if auth.disabled {
        return Ok(());
    }
    let mut next = auth.clone();
    next.disabled = true;
    next.disabled_reason = Some("invalid_refresh_token".to_string());
    save_auth_record(dir, &next).await?;
    tracing::warn!(
        account_name = %next.name,
        auth_method = next.auth_method(),
        disabled_reason = %next.disabled_reason.as_deref().unwrap_or("unknown"),
        "persisted kiro account as disabled after invalid refresh token"
    );
    Ok(())
}

fn summarize_refresh_error_body(body: &str) -> String {
    const MAX_PREVIEW_CHARS: usize = 240;
    let total_chars = body.chars().count();
    if total_chars <= MAX_PREVIEW_CHARS {
        return body.to_string();
    }
    let preview = body.chars().take(MAX_PREVIEW_CHARS).collect::<String>();
    format!("{preview}...[truncated,total_chars={total_chars}]")
}

fn social_refresh_user_agent(machine_id: &str) -> String {
    format!("KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}")
}

fn usage_request_user_agents(machine_id: &str) -> (String, String) {
    (
        format!(
            "aws-sdk-js/{KIRO_USAGE_AWS_SDK_VERSION} KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
        format!(
            "aws-sdk-js/{KIRO_USAGE_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
             md/nodejs#{DEFAULT_NODE_VERSION} \
             api/codewhispererruntime#{KIRO_USAGE_AWS_SDK_VERSION} m/N,E \
             KiroIDE-{DEFAULT_KIRO_VERSION}-{machine_id}"
        ),
    )
}

fn idc_refresh_amz_user_agent() -> &'static str {
    "aws-sdk-js/3.980.0 KiroIDE"
}

fn idc_refresh_user_agent() -> String {
    format!(
        "aws-sdk-js/{KIRO_IDC_AWS_SDK_VERSION} ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
         md/nodejs#{DEFAULT_NODE_VERSION} api/sso-oidc#{KIRO_IDC_AWS_SDK_VERSION} m/E KiroIDE"
    )
}

fn jwt_exp_to_rfc3339(token: &str) -> Option<String> {
    access_token_expiry(token).map(|value| value.to_rfc3339())
}

fn access_token_expiry(token: &str) -> Option<DateTime<Utc>> {
    let exp = parse_jwt_exp(token)?;
    DateTime::<Utc>::from_timestamp(exp, 0)
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn parse_jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    value.get("exp").and_then(|value| value.as_i64())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::kiro_gateway::auth_file::KiroAuthRecord;

    fn test_jwt_with_exp(exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none","typ":"JWT"}"#);
        let payload = URL_SAFE_NO_PAD.encode(format!(r#"{{"sub":"test","exp":{exp}}}"#).as_bytes());
        format!("{header}.{payload}.sig")
    }

    #[test]
    fn derive_refreshed_expires_at_uses_jwt_exp_when_refresh_omits_expires_in() {
        let token = test_jwt_with_exp(1_900_000_000);
        assert_eq!(
            derive_refreshed_expires_at(Some(token.as_str()), None),
            Some("2030-03-17T17:46:40+00:00".to_string())
        );
    }

    #[test]
    fn needs_refresh_uses_access_token_expiry_when_expires_at_is_missing() {
        let future_exp = (Utc::now() + Duration::minutes(45)).timestamp();
        let auth = KiroAuthRecord {
            name: "default".to_string(),
            access_token: Some(test_jwt_with_exp(future_exp)),
            expires_at: None,
            ..KiroAuthRecord::default()
        };
        assert!(!needs_refresh(&auth));
    }

    #[test]
    fn usage_request_user_agents_follow_latest_kiro_signature() {
        let (amz_user_agent, user_agent) = usage_request_user_agents(&"a".repeat(64));

        assert_eq!(
            amz_user_agent,
            format!("aws-sdk-js/1.0.0 KiroIDE-{DEFAULT_KIRO_VERSION}-{}", "a".repeat(64))
        );
        assert!(user_agent.contains(&format!(
            "aws-sdk-js/1.0.0 ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
             md/nodejs#{DEFAULT_NODE_VERSION}"
        )));
        assert!(user_agent.contains("api/codewhispererruntime#1.0.0"));
    }

    #[test]
    fn idc_refresh_user_agent_uses_latest_upstream_version() {
        assert_eq!(idc_refresh_amz_user_agent(), "aws-sdk-js/3.980.0 KiroIDE");
        let user_agent = idc_refresh_user_agent();
        assert!(user_agent.contains(&format!(
            "aws-sdk-js/3.980.0 ua/2.1 os/{DEFAULT_SYSTEM_VERSION} lang/js \
             md/nodejs#{DEFAULT_NODE_VERSION}"
        )));
        assert!(user_agent.contains("api/sso-oidc#3.980.0"));
        assert_eq!(KIRO_IDC_AMZ_SDK_REQUEST, "attempt=1; max=4");
    }

    #[test]
    fn invalid_refresh_grant_detection_matches_upstream_contract() {
        let body =
            r#"{"error":"invalid_grant","error_description":"Invalid refresh token provided"}"#;
        assert!(is_invalid_refresh_token_grant(400, body));
        assert!(!is_invalid_refresh_token_grant(401, body));
        assert!(!is_invalid_refresh_token_grant(400, r#"{"error":"invalid_client"}"#));
    }

    #[tokio::test]
    async fn disable_auth_for_invalid_refresh_token_persists_disabled_flag() {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);

        let dir = std::env::temp_dir().join(format!(
            "staticflow-kiro-runtime-test-{}",
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::create_dir_all(&dir)
            .await
            .expect("create temp auth dir");

        let auth = KiroAuthRecord {
            name: "alpha".to_string(),
            refresh_token: Some("r".repeat(128)),
            disabled: false,
            ..KiroAuthRecord::default()
        };
        save_auth_record(&dir, &auth)
            .await
            .expect("persist seed auth record");

        disable_auth_for_invalid_refresh_token(&dir, &auth)
            .await
            .expect("disable invalid refresh token account");

        let persisted = load_auth_records(&dir).await.expect("load persisted auths");
        assert_eq!(persisted.len(), 1);
        assert!(persisted[0].disabled);
        assert_eq!(persisted[0].disabled_reason.as_deref(), Some("invalid_refresh_token"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
