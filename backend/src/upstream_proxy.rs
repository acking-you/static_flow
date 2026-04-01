//! Shared upstream proxy registry for Codex and Kiro.
//!
//! Provider-specific upstream requests should not read environment variables or
//! provider-local proxy settings directly. Instead they resolve through this
//! registry, which merges persisted admin-managed proxy configs, provider
//! bindings, and the temporary env fallback chain in one place.

use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::RwLock;
use reqwest::Proxy;
use serde::{Deserialize, Serialize};
use static_flow_shared::llm_gateway_store::{
    now_ms, LlmGatewayProxyBindingRecord, LlmGatewayProxyConfigRecord, LlmGatewayStore,
    LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_PROVIDER_CODEX, LLM_GATEWAY_PROVIDER_KIRO,
};
use uuid::Uuid;

use crate::kiro_gateway::auth_file::{
    load_auth_records, resolve_auths_dir, save_auth_record, KiroAuthRecord,
};

pub const DEFAULT_UPSTREAM_PROXY_URL: &str = "http://127.0.0.1:11111";

/// Account-level proxy override mode stored alongside Codex/Kiro account
/// settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountProxyMode {
    /// Reuse the existing provider-level binding or env fallback.
    #[default]
    Inherit,
    /// Bypass the shared upstream proxy and connect directly.
    Direct,
    /// Pin this account to one reusable shared proxy config.
    Fixed,
}

impl AccountProxyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Direct => "direct",
            Self::Fixed => "fixed",
        }
    }
}

/// Account-level proxy selection persisted on Codex/Kiro account records.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct AccountProxySelection {
    #[serde(default)]
    pub proxy_mode: AccountProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_config_id: Option<String>,
}

impl AccountProxySelection {
    pub fn canonicalize(mut self) -> Self {
        self.proxy_config_id = self
            .proxy_config_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        if self.proxy_mode != AccountProxyMode::Fixed {
            self.proxy_config_id = None;
        }
        self
    }

    pub fn is_default(&self) -> bool {
        self.proxy_mode == AccountProxyMode::Inherit && self.proxy_config_id.is_none()
    }
}

/// Normalized reqwest client profile used as the cache key for pooled clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HttpClientProfile {
    pub timeout_secs: Option<u64>,
    pub pool_max_idle_per_host: usize,
    pub pool_idle_timeout_secs: u64,
}

impl HttpClientProfile {
    pub const fn new(
        timeout_secs: Option<u64>,
        pool_max_idle_per_host: usize,
        pool_idle_timeout_secs: u64,
    ) -> Self {
        Self {
            timeout_secs,
            pool_max_idle_per_host,
            pool_idle_timeout_secs,
        }
    }

    fn client_builder(self) -> reqwest::ClientBuilder {
        let builder = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .pool_max_idle_per_host(self.pool_max_idle_per_host)
            .pool_idle_timeout(Duration::from_secs(self.pool_idle_timeout_secs))
            .tcp_keepalive(Duration::from_secs(30));
        if let Some(timeout_secs) = self.timeout_secs {
            builder.timeout(Duration::from_secs(timeout_secs))
        } else {
            builder
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ClientCacheKey {
    proxy_url: Option<String>,
    proxy_username: Option<String>,
    proxy_password: Option<String>,
    timeout_secs: Option<u64>,
    pool_max_idle_per_host: usize,
    pool_idle_timeout_secs: u64,
}

impl ClientCacheKey {
    fn new(resolved: &ResolvedUpstreamProxy, profile: HttpClientProfile) -> Self {
        Self {
            proxy_url: resolved.proxy_url.clone(),
            proxy_username: resolved.proxy_username.clone(),
            proxy_password: resolved.proxy_password.clone(),
            timeout_secs: profile.timeout_secs,
            pool_max_idle_per_host: profile.pool_max_idle_per_host,
            pool_idle_timeout_secs: profile.pool_idle_timeout_secs,
        }
    }
}

/// Cached snapshot of all persisted proxy configs and provider bindings.
#[derive(Debug, Clone, Default)]
struct UpstreamProxySnapshot {
    configs_by_id: HashMap<String, LlmGatewayProxyConfigRecord>,
    bindings_by_provider: HashMap<String, LlmGatewayProxyBindingRecord>,
}

/// Resolved upstream proxy settings after binding/env lookup.
///
/// This keeps both the concrete proxy URL/credentials and provenance metadata
/// so handlers can log whether a request used an explicit admin binding or an
/// environment fallback.
#[derive(Debug, Clone)]
pub struct ResolvedUpstreamProxy {
    pub source: ResolvedUpstreamProxySource,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub proxy_config_id: Option<String>,
    pub proxy_config_name: Option<String>,
    pub binding_updated_at: Option<i64>,
}

impl ResolvedUpstreamProxy {
    pub fn proxy_url_label(&self) -> &str {
        self.proxy_url.as_deref().unwrap_or("direct")
    }
}

/// Where a resolved upstream proxy setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedUpstreamProxySource {
    Binding,
    EnvFallback,
    AccountBinding,
    Direct,
}

impl ResolvedUpstreamProxySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::EnvFallback => "env_fallback",
            Self::AccountBinding => "account_binding",
            Self::Direct => "direct",
        }
    }
}

/// In-memory registry that serves provider-scoped upstream proxy resolution.
///
/// The registry mirrors LanceDB-backed proxy configs/bindings into a cheap
/// read-mostly snapshot so hot request paths can resolve their proxy without
/// hitting storage for every call.
#[derive(Clone)]
pub struct UpstreamProxyRegistry {
    store: Arc<LlmGatewayStore>,
    snapshot: Arc<RwLock<UpstreamProxySnapshot>>,
    clients: Arc<RwLock<HashMap<ClientCacheKey, reqwest::Client>>>,
}

/// Result of migrating legacy per-account Kiro proxy settings into the shared
/// provider-level proxy registry.
#[derive(Debug, Clone)]
pub struct LegacyKiroProxyMigrationResult {
    pub created_configs: Vec<LlmGatewayProxyConfigRecord>,
    pub reused_configs: Vec<LlmGatewayProxyConfigRecord>,
    pub migrated_account_names: Vec<String>,
}

impl UpstreamProxyRegistry {
    /// Load the registry from persistent storage and build the initial
    /// in-memory snapshot.
    pub async fn new(store: Arc<LlmGatewayStore>) -> Result<Self> {
        let registry = Self {
            store,
            snapshot: Arc::new(RwLock::new(UpstreamProxySnapshot::default())),
            clients: Arc::new(RwLock::new(HashMap::new())),
        };
        registry.refresh().await?;
        Ok(registry)
    }

    /// Rebuild the in-memory snapshot from LanceDB after admin mutations.
    pub async fn refresh(&self) -> Result<()> {
        let configs = self.store.list_proxy_configs().await?;
        let bindings = self.store.list_proxy_bindings().await?;
        let snapshot = UpstreamProxySnapshot {
            configs_by_id: configs
                .into_iter()
                .map(|record| (record.id.clone(), record))
                .collect(),
            bindings_by_provider: bindings
                .into_iter()
                .map(|record| (record.provider_type.clone(), record))
                .collect(),
        };
        *self.snapshot.write() = snapshot;
        self.clients.write().clear();
        Ok(())
    }

    /// Resolve the effective upstream proxy for one provider.
    ///
    /// Resolution order is:
    /// 1. explicit provider binding
    /// 2. provider-specific environment fallback chain
    ///
    /// If a binding exists but points to a missing or disabled config, this
    /// returns an error instead of silently falling back.
    pub async fn resolve_provider_proxy(
        &self,
        provider_type: &str,
    ) -> Result<ResolvedUpstreamProxy> {
        self.resolve_proxy_for_selection(provider_type, None).await
    }

    /// Resolve the effective upstream proxy for one account, falling back to
    /// the provider-level setting when the account inherits.
    pub async fn resolve_proxy_for_selection(
        &self,
        provider_type: &str,
        selection: Option<&AccountProxySelection>,
    ) -> Result<ResolvedUpstreamProxy> {
        let normalized = selection.cloned().unwrap_or_default().canonicalize();
        let snapshot = self.snapshot.read();
        Self::resolve_proxy_with_snapshot(&snapshot, provider_type, &normalized)
    }

    /// Return a pooled reqwest client for the resolved provider/account proxy
    /// combination. `reqwest::Client` is internally reference-counted, so hot
    /// paths only clone the cached handle instead of rebuilding connector state
    /// and connection pools on every request.
    pub async fn client_for_selection(
        &self,
        provider_type: &str,
        selection: Option<&AccountProxySelection>,
        profile: HttpClientProfile,
    ) -> Result<(reqwest::Client, ResolvedUpstreamProxy)> {
        let resolved = self
            .resolve_proxy_for_selection(provider_type, selection)
            .await?;
        let cache_key = ClientCacheKey::new(&resolved, profile);
        if let Some(client) = self.clients.read().get(&cache_key).cloned() {
            return Ok((client, resolved));
        }

        let mut clients = self.clients.write();
        if let Some(client) = clients.get(&cache_key).cloned() {
            return Ok((client, resolved));
        }

        let builder = apply_resolved_proxy(profile.client_builder(), &resolved)?;
        let client = builder
            .build()
            .context("failed to build cached upstream reqwest client")?;
        clients.insert(cache_key, client.clone());
        Ok((client, resolved))
    }

    /// Drop one cached client instance. The next request will rebuild it with
    /// a fresh connector/pool.
    pub async fn invalidate_client(
        &self,
        resolved: &ResolvedUpstreamProxy,
        profile: HttpClientProfile,
    ) -> bool {
        self.clients
            .write()
            .remove(&ClientCacheKey::new(resolved, profile))
            .is_some()
    }

    /// Rebuild the cached client only when reqwest reports a connect-level
    /// transport failure. Hyper/reqwest already retries stale pooled sockets by
    /// opening fresh connections, so normal HTTP failures should not evict the
    /// whole client.
    pub async fn invalidate_client_if_connect_error(
        &self,
        resolved: &ResolvedUpstreamProxy,
        profile: HttpClientProfile,
        err: &reqwest::Error,
    ) -> bool {
        if !err.is_connect() {
            return false;
        }
        self.invalidate_client(resolved, profile).await
    }

    /// Import legacy proxy credentials embedded in Kiro account JSON files into
    /// the shared provider proxy registry.
    ///
    /// Duplicate proxy tuples are de-duplicated, migrated accounts have their
    /// embedded proxy fields cleared, and the registry snapshot is refreshed
    /// before returning.
    pub async fn import_legacy_kiro_account_proxies(
        &self,
    ) -> Result<LegacyKiroProxyMigrationResult> {
        let auths_dir = resolve_auths_dir();
        let auths = load_auth_records(&auths_dir).await?;
        let mut tuples_to_accounts =
            BTreeMap::<(String, Option<String>, Option<String>), Vec<KiroAuthRecord>>::new();

        for auth in auths {
            let Some(proxy_url) = normalize_proxy_field(auth.proxy_url.as_deref()) else {
                continue;
            };
            let proxy_username = normalize_proxy_field(auth.proxy_username.as_deref());
            let proxy_password = normalize_proxy_field(auth.proxy_password.as_deref());
            tuples_to_accounts
                .entry((proxy_url, proxy_username, proxy_password))
                .or_default()
                .push(auth);
        }

        if tuples_to_accounts.is_empty() {
            return Ok(LegacyKiroProxyMigrationResult {
                created_configs: vec![],
                reused_configs: vec![],
                migrated_account_names: vec![],
            });
        }

        let existing = self.store.list_proxy_configs().await?;
        let mut existing_by_tuple =
            HashMap::<(String, Option<String>, Option<String>), LlmGatewayProxyConfigRecord>::new();
        for config in existing {
            existing_by_tuple.insert(
                (
                    config.proxy_url.clone(),
                    config.proxy_username.clone(),
                    config.proxy_password.clone(),
                ),
                config,
            );
        }

        let mut created_configs = Vec::new();
        let mut reused_configs = Vec::new();
        let mut migrated_account_names = Vec::new();

        for (index, (tuple, mut accounts)) in tuples_to_accounts.into_iter().enumerate() {
            let config = if let Some(existing) = existing_by_tuple.get(&tuple).cloned() {
                reused_configs.push(existing.clone());
                existing
            } else {
                let now = now_ms();
                let config = LlmGatewayProxyConfigRecord {
                    id: format!("llm-proxy-{}-{}", now, &Uuid::new_v4().simple().to_string()[..12]),
                    name: format!("legacy-kiro-{}", index + 1),
                    proxy_url: tuple.0.clone(),
                    proxy_username: tuple.1.clone(),
                    proxy_password: tuple.2.clone(),
                    status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
                    created_at: now,
                    updated_at: now,
                };
                self.store.create_proxy_config(&config).await?;
                existing_by_tuple.insert(tuple.clone(), config.clone());
                created_configs.push(config.clone());
                config
            };

            accounts.sort_by_cached_key(|auth| auth.name.to_ascii_lowercase());
            for mut auth in accounts {
                auth.proxy_mode = AccountProxyMode::Fixed;
                auth.proxy_config_id = Some(config.id.clone());
                auth.proxy_url = None;
                auth.proxy_username = None;
                auth.proxy_password = None;
                save_auth_record(&auths_dir, &auth).await?;
                migrated_account_names.push(auth.name.clone());
            }

            tracing::info!(
                proxy_config_id = %config.id,
                proxy_config_name = %config.name,
                "imported legacy kiro account proxy config"
            );
        }

        migrated_account_names.sort();
        migrated_account_names.dedup();
        self.refresh().await?;
        Ok(LegacyKiroProxyMigrationResult {
            created_configs,
            reused_configs,
            migrated_account_names,
        })
    }

    fn resolve_proxy_with_snapshot(
        snapshot: &UpstreamProxySnapshot,
        provider_type: &str,
        selection: &AccountProxySelection,
    ) -> Result<ResolvedUpstreamProxy> {
        match selection.proxy_mode {
            AccountProxyMode::Direct => Ok(ResolvedUpstreamProxy {
                source: ResolvedUpstreamProxySource::Direct,
                proxy_url: None,
                proxy_username: None,
                proxy_password: None,
                proxy_config_id: None,
                proxy_config_name: None,
                binding_updated_at: None,
            }),
            AccountProxyMode::Fixed => {
                let proxy_config_id = selection.proxy_config_id.as_deref().ok_or_else(|| {
                    anyhow!("proxy_config_id is required when proxy_mode=`fixed`")
                })?;
                let config = snapshot
                    .configs_by_id
                    .get(proxy_config_id)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "account proxy selection for provider `{provider_type}` points to \
                             missing config `{proxy_config_id}`"
                        )
                    })?;
                if config.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
                    bail!(
                        "account proxy selection for provider `{provider_type}` points to \
                         disabled config `{}`",
                        config.name
                    );
                }
                validate_proxy_url(&config.proxy_url)?;
                Ok(ResolvedUpstreamProxy {
                    source: ResolvedUpstreamProxySource::AccountBinding,
                    proxy_url: Some(config.proxy_url.clone()),
                    proxy_username: config.proxy_username.clone(),
                    proxy_password: config.proxy_password.clone(),
                    proxy_config_id: Some(config.id.clone()),
                    proxy_config_name: Some(config.name.clone()),
                    binding_updated_at: None,
                })
            },
            AccountProxyMode::Inherit => {
                if let Some(binding) = snapshot.bindings_by_provider.get(provider_type) {
                    let config = snapshot
                        .configs_by_id
                        .get(&binding.proxy_config_id)
                        .cloned()
                        .ok_or_else(|| {
                            anyhow!(
                                "proxy binding for provider `{provider_type}` points to missing \
                                 config `{}`",
                                binding.proxy_config_id
                            )
                        })?;
                    if config.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
                        bail!(
                            "proxy binding for provider `{provider_type}` points to disabled \
                             config `{}`",
                            config.name
                        );
                    }
                    validate_proxy_url(&config.proxy_url)?;
                    return Ok(ResolvedUpstreamProxy {
                        source: ResolvedUpstreamProxySource::Binding,
                        proxy_url: Some(config.proxy_url.clone()),
                        proxy_username: config.proxy_username.clone(),
                        proxy_password: config.proxy_password.clone(),
                        proxy_config_id: Some(config.id.clone()),
                        proxy_config_name: Some(config.name.clone()),
                        binding_updated_at: Some(binding.updated_at),
                    });
                }

                let proxy_url = env_fallback_proxy_url(provider_type).ok_or_else(|| {
                    anyhow!("no upstream proxy available for provider `{provider_type}`")
                })?;
                validate_proxy_url(&proxy_url)?;
                Ok(ResolvedUpstreamProxy {
                    source: ResolvedUpstreamProxySource::EnvFallback,
                    proxy_url: Some(proxy_url),
                    proxy_username: None,
                    proxy_password: None,
                    proxy_config_id: None,
                    proxy_config_name: None,
                    binding_updated_at: None,
                })
            },
        }
    }
}

/// Build a `reqwest` proxy instance from an already-resolved proxy record.
pub fn build_proxy(resolved: &ResolvedUpstreamProxy) -> Result<Proxy> {
    let proxy_url = resolved
        .proxy_url
        .as_deref()
        .ok_or_else(|| anyhow!("direct upstream transport does not use reqwest::Proxy"))?;
    let proxy = Proxy::all(proxy_url)
        .with_context(|| format!("failed to build upstream proxy `{proxy_url}`"))?;
    Ok(match resolved.proxy_username.as_deref() {
        Some(username) => {
            proxy.basic_auth(username, resolved.proxy_password.as_deref().unwrap_or(""))
        },
        None => proxy,
    })
}

pub fn apply_resolved_proxy(
    builder: reqwest::ClientBuilder,
    resolved: &ResolvedUpstreamProxy,
) -> Result<reqwest::ClientBuilder> {
    if resolved.proxy_url.is_some() {
        Ok(builder.proxy(build_proxy(resolved)?))
    } else {
        Ok(builder)
    }
}

/// Validate that a proxy URL is syntactically acceptable to `reqwest`.
pub fn validate_proxy_url(proxy_url: &str) -> Result<()> {
    Proxy::all(proxy_url).with_context(|| format!("invalid proxy url `{proxy_url}`"))?;
    Ok(())
}

fn normalize_proxy_field(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .and_then(
            |value| {
                if value.eq_ignore_ascii_case("direct") {
                    None
                } else {
                    Some(value.to_string())
                }
            },
        )
}

fn env_fallback_proxy_url(provider_type: &str) -> Option<String> {
    let codex = std::env::var("STATICFLOW_LLM_GATEWAY_UPSTREAM_PROXY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let kiro = std::env::var("STATICFLOW_KIRO_UPSTREAM_PROXY_URL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    match provider_type {
        LLM_GATEWAY_PROVIDER_CODEX => {
            codex.or_else(|| Some(DEFAULT_UPSTREAM_PROXY_URL.to_string()))
        },
        LLM_GATEWAY_PROVIDER_KIRO => kiro
            .or(codex)
            .or_else(|| Some(DEFAULT_UPSTREAM_PROXY_URL.to_string())),
        _ => None,
    }
}

pub fn parse_account_proxy_selection_patch(
    proxy_mode: Option<&str>,
    proxy_config_id: Option<&str>,
) -> Result<Option<AccountProxySelection>> {
    if proxy_mode.is_none() && proxy_config_id.is_none() {
        return Ok(None);
    }
    let proxy_mode = parse_account_proxy_mode(
        proxy_mode.ok_or_else(|| anyhow!("proxy_mode is required when updating proxy settings"))?,
    )?;
    let selection = AccountProxySelection {
        proxy_mode,
        proxy_config_id: proxy_config_id.map(str::to_string),
    }
    .canonicalize();
    if selection.proxy_mode == AccountProxyMode::Fixed && selection.proxy_config_id.is_none() {
        bail!("proxy_config_id is required when proxy_mode=`fixed`");
    }
    Ok(Some(selection))
}

pub fn parse_account_proxy_mode(raw: &str) -> Result<AccountProxyMode> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "inherit" => Ok(AccountProxyMode::Inherit),
        "direct" => Ok(AccountProxyMode::Direct),
        "fixed" => Ok(AccountProxyMode::Fixed),
        other => bail!("unsupported proxy_mode `{other}`"),
    }
}

#[cfg(test)]
mod tests {
    use static_flow_shared::llm_gateway_store::LLM_GATEWAY_PROVIDER_CODEX;

    use super::*;

    fn sample_config(id: &str, name: &str, proxy_url: &str) -> LlmGatewayProxyConfigRecord {
        LlmGatewayProxyConfigRecord {
            id: id.to_string(),
            name: name.to_string(),
            proxy_url: proxy_url.to_string(),
            proxy_username: None,
            proxy_password: None,
            status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
            created_at: 0,
            updated_at: 0,
        }
    }

    fn sample_binding(provider_type: &str, proxy_config_id: &str) -> LlmGatewayProxyBindingRecord {
        LlmGatewayProxyBindingRecord {
            provider_type: provider_type.to_string(),
            proxy_config_id: proxy_config_id.to_string(),
            updated_at: 123,
        }
    }

    #[test]
    fn resolve_proxy_inherit_uses_provider_binding() {
        let mut snapshot = UpstreamProxySnapshot::default();
        snapshot.configs_by_id.insert(
            "proxy-a".to_string(),
            sample_config("proxy-a", "alpha", "http://127.0.0.1:9001"),
        );
        snapshot.bindings_by_provider.insert(
            LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            sample_binding(LLM_GATEWAY_PROVIDER_CODEX, "proxy-a"),
        );

        let resolved = UpstreamProxyRegistry::resolve_proxy_with_snapshot(
            &snapshot,
            LLM_GATEWAY_PROVIDER_CODEX,
            &AccountProxySelection::default(),
        )
        .expect("resolve inherited provider binding");

        assert_eq!(resolved.source, ResolvedUpstreamProxySource::Binding);
        assert_eq!(resolved.proxy_url.as_deref(), Some("http://127.0.0.1:9001"));
        assert_eq!(resolved.proxy_config_id.as_deref(), Some("proxy-a"));
    }

    #[test]
    fn resolve_proxy_direct_bypasses_provider_binding() {
        let mut snapshot = UpstreamProxySnapshot::default();
        snapshot.configs_by_id.insert(
            "proxy-a".to_string(),
            sample_config("proxy-a", "alpha", "http://127.0.0.1:9001"),
        );
        snapshot.bindings_by_provider.insert(
            LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            sample_binding(LLM_GATEWAY_PROVIDER_CODEX, "proxy-a"),
        );

        let resolved = UpstreamProxyRegistry::resolve_proxy_with_snapshot(
            &snapshot,
            LLM_GATEWAY_PROVIDER_CODEX,
            &AccountProxySelection {
                proxy_mode: AccountProxyMode::Direct,
                proxy_config_id: None,
            },
        )
        .expect("resolve direct account override");

        assert_eq!(resolved.source, ResolvedUpstreamProxySource::Direct);
        assert_eq!(resolved.proxy_url, None);
        assert_eq!(resolved.proxy_config_id, None);
    }

    #[test]
    fn resolve_proxy_fixed_uses_account_override() {
        let mut snapshot = UpstreamProxySnapshot::default();
        snapshot.configs_by_id.insert(
            "proxy-a".to_string(),
            sample_config("proxy-a", "alpha", "http://127.0.0.1:9001"),
        );
        snapshot.configs_by_id.insert(
            "proxy-b".to_string(),
            sample_config("proxy-b", "beta", "http://127.0.0.1:9002"),
        );
        snapshot.bindings_by_provider.insert(
            LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            sample_binding(LLM_GATEWAY_PROVIDER_CODEX, "proxy-a"),
        );

        let resolved = UpstreamProxyRegistry::resolve_proxy_with_snapshot(
            &snapshot,
            LLM_GATEWAY_PROVIDER_CODEX,
            &AccountProxySelection {
                proxy_mode: AccountProxyMode::Fixed,
                proxy_config_id: Some("proxy-b".to_string()),
            },
        )
        .expect("resolve fixed account proxy");

        assert_eq!(resolved.source, ResolvedUpstreamProxySource::AccountBinding);
        assert_eq!(resolved.proxy_url.as_deref(), Some("http://127.0.0.1:9002"));
        assert_eq!(resolved.proxy_config_id.as_deref(), Some("proxy-b"));
    }
}
