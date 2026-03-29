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
use reqwest::Proxy;
use static_flow_shared::llm_gateway_store::{
    now_ms, LlmGatewayProxyBindingRecord, LlmGatewayProxyConfigRecord, LlmGatewayStore,
    LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_PROVIDER_CODEX, LLM_GATEWAY_PROVIDER_KIRO,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::kiro_gateway::auth_file::{
    load_auth_records, resolve_auths_dir, save_auth_record, KiroAuthRecord,
};

pub const DEFAULT_UPSTREAM_PROXY_URL: &str = "http://127.0.0.1:11111";

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
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub proxy_config_id: Option<String>,
    pub proxy_config_name: Option<String>,
    pub binding_updated_at: Option<i64>,
}

/// Where a resolved upstream proxy setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedUpstreamProxySource {
    Binding,
    EnvFallback,
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
        *self.snapshot.write().await = snapshot;
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
        let snapshot = self.snapshot.read().await;
        if let Some(binding) = snapshot.bindings_by_provider.get(provider_type) {
            let config = snapshot
                .configs_by_id
                .get(&binding.proxy_config_id)
                .cloned()
                .ok_or_else(|| {
                    anyhow!(
                        "proxy binding for provider `{provider_type}` points to missing config \
                         `{}`",
                        binding.proxy_config_id
                    )
                })?;
            if config.status != LLM_GATEWAY_KEY_STATUS_ACTIVE {
                bail!(
                    "proxy binding for provider `{provider_type}` points to disabled config `{}`",
                    config.name
                );
            }
            validate_proxy_url(&config.proxy_url)?;
            return Ok(ResolvedUpstreamProxy {
                source: ResolvedUpstreamProxySource::Binding,
                proxy_url: config.proxy_url.clone(),
                proxy_username: config.proxy_username.clone(),
                proxy_password: config.proxy_password.clone(),
                proxy_config_id: Some(config.id.clone()),
                proxy_config_name: Some(config.name.clone()),
                binding_updated_at: Some(binding.updated_at),
            });
        }

        let proxy_url = env_fallback_proxy_url(provider_type)
            .ok_or_else(|| anyhow!("no upstream proxy available for provider `{provider_type}`"))?;
        validate_proxy_url(&proxy_url)?;
        Ok(ResolvedUpstreamProxy {
            source: ResolvedUpstreamProxySource::EnvFallback,
            proxy_url,
            proxy_username: None,
            proxy_password: None,
            proxy_config_id: None,
            proxy_config_name: None,
            binding_updated_at: None,
        })
    }

    /// Apply the resolved provider proxy to a `reqwest::ClientBuilder`.
    pub async fn apply_provider_proxy(
        &self,
        provider_type: &str,
        builder: reqwest::ClientBuilder,
    ) -> Result<reqwest::ClientBuilder> {
        let resolved = self.resolve_provider_proxy(provider_type).await?;
        Ok(builder.proxy(build_proxy(&resolved)?))
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
}

/// Build a `reqwest` proxy instance from an already-resolved proxy record.
pub fn build_proxy(resolved: &ResolvedUpstreamProxy) -> Result<Proxy> {
    let proxy = Proxy::all(&resolved.proxy_url)
        .with_context(|| format!("failed to build upstream proxy `{}`", resolved.proxy_url))?;
    Ok(match resolved.proxy_username.as_deref() {
        Some(username) => {
            proxy.basic_auth(username, resolved.proxy_password.as_deref().unwrap_or(""))
        },
        None => proxy,
    })
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

/// Standard HTTP client baseline shared by proxy health checks and refresh
/// clients before provider-specific proxy settings are applied.
pub fn standard_client_builder(
    timeout_secs: u64,
    pool_max_idle_per_host: usize,
    pool_idle_timeout_secs: u64,
) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(timeout_secs))
        .pool_max_idle_per_host(pool_max_idle_per_host)
        .pool_idle_timeout(Duration::from_secs(pool_idle_timeout_secs))
        .tcp_keepalive(Duration::from_secs(30))
}
