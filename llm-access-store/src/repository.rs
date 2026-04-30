//! Async repository adapters for llm-access runtime traits.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use llm_access_core::{
    store::{
        AdminAccountGroup, AdminAccountGroupPatch, AdminAccountGroupStore, AdminCodexAccount,
        AdminCodexAccountPatch, AdminCodexAccountStore, AdminConfigStore, AdminKey, AdminKeyPatch,
        AdminKeyStore, AdminProxyBinding, AdminProxyConfig, AdminProxyConfigPatch, AdminProxyStore,
        AdminRuntimeConfig, AuthenticatedKey, CodexRateLimitStatus, ControlStore,
        NewAdminAccountGroup, NewAdminCodexAccount, NewAdminKey, NewAdminProxyConfig,
        NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
        PublicAccessKey, PublicAccessStore, PublicAccountContribution, PublicCommunityStore,
        PublicSponsor, PublicStatusStore, PublicSubmissionStore, PublicUsageLookupKey,
        PublicUsageStore, UsageEventSink, DEFAULT_AUTH_CACHE_TTL_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_SECONDS,
    },
    usage::UsageEvent,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tokio::task;

use crate::sqlite::SqliteControlStore;

/// Thread-safe SQLite control repository.
pub struct SqliteControlRepository {
    inner: Arc<Mutex<SqliteControlStore>>,
}

impl SqliteControlRepository {
    /// Create a repository from an opened SQLite connection.
    pub fn new(conn: Connection) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SqliteControlStore::new(conn))),
        }
    }

    /// Open a repository from a SQLite database path.
    pub fn open_path(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path).with_context(|| {
            format!("failed to open sqlite control database `{}`", path.display())
        })?;
        Ok(Self::new(conn))
    }
}

fn hash_bearer_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[async_trait]
impl AdminConfigStore for SqliteControlRepository {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store
                .get_runtime_config_or_default()
                .map(|record| record.to_admin_runtime_config())
        })
        .await
        .context("sqlite control repository admin config read task failed")?
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.update_admin_runtime_config(&config)
        })
        .await
        .context("sqlite control repository admin config update task failed")?
    }
}

#[async_trait]
impl AdminKeyStore for SqliteControlRepository {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_admin_keys()
        })
        .await
        .context("sqlite control repository admin key list task failed")?
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_admin_key(&key)
        })
        .await
        .context("sqlite control repository admin key create task failed")?
    }

    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        let key_id = key_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.patch_admin_key(&key_id, &patch)
        })
        .await
        .context("sqlite control repository admin key patch task failed")?
    }

    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        let key_id = key_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.delete_admin_key(&key_id)
        })
        .await
        .context("sqlite control repository admin key delete task failed")?
    }
}

#[async_trait]
impl AdminAccountGroupStore for SqliteControlRepository {
    async fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        let provider_type = provider_type.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_admin_account_groups(&provider_type)
        })
        .await
        .context("sqlite control repository account group list task failed")?
    }

    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_admin_account_group(&group)
        })
        .await
        .context("sqlite control repository account group create task failed")?
    }

    async fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let group_id = group_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.patch_admin_account_group(&group_id, &patch)
        })
        .await
        .context("sqlite control repository account group patch task failed")?
    }

    async fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let group_id = group_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.delete_admin_account_group(&group_id)
        })
        .await
        .context("sqlite control repository account group delete task failed")?
    }
}

#[async_trait]
impl AdminProxyStore for SqliteControlRepository {
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_admin_proxy_configs()
        })
        .await
        .context("sqlite control repository proxy config list task failed")?
    }

    async fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let proxy_id = proxy_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.get_admin_proxy_config(&proxy_id)
        })
        .await
        .context("sqlite control repository proxy config get task failed")?
    }

    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_admin_proxy_config(&proxy)
        })
        .await
        .context("sqlite control repository proxy config create task failed")?
    }

    async fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let proxy_id = proxy_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.patch_admin_proxy_config(&proxy_id, &patch)
        })
        .await
        .context("sqlite control repository proxy config patch task failed")?
    }

    async fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let proxy_id = proxy_id.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.delete_admin_proxy_config(&proxy_id)
        })
        .await
        .context("sqlite control repository proxy config delete task failed")?
    }

    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_admin_proxy_bindings()
        })
        .await
        .context("sqlite control repository proxy binding list task failed")?
    }

    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        let provider_type = provider_type.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.update_admin_proxy_binding(&provider_type, proxy_config_id)
        })
        .await
        .context("sqlite control repository proxy binding update task failed")?
    }
}

#[async_trait]
impl AdminCodexAccountStore for SqliteControlRepository {
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_admin_codex_accounts()
        })
        .await
        .context("sqlite control repository codex account list task failed")?
    }

    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_admin_codex_account(&account)
        })
        .await
        .context("sqlite control repository codex account create task failed")?
    }

    async fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let name = name.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.patch_admin_codex_account(&name, &patch)
        })
        .await
        .context("sqlite control repository codex account patch task failed")?
    }

    async fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let name = name.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.delete_admin_codex_account(&name)
        })
        .await
        .context("sqlite control repository codex account delete task failed")?
    }

    async fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let name = name.to_string();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.refresh_admin_codex_account(&name, refreshed_at_ms)
        })
        .await
        .context("sqlite control repository codex account refresh task failed")?
    }
}

#[async_trait]
impl ControlStore for SqliteControlRepository {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let key_hash = hash_bearer_secret(secret);
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.get_key_by_hash(&key_hash).map(|record| {
                record.map(|bundle| AuthenticatedKey {
                    key_id: bundle.key.key_id,
                    key_name: bundle.key.name,
                    provider_type: bundle.key.provider_type,
                    protocol_family: bundle.key.protocol_family,
                    status: bundle.key.status,
                    quota_billable_limit: bundle.key.quota_billable_limit,
                    billable_tokens_used: bundle.rollup.billable_tokens,
                })
            })
        })
        .await
        .context("sqlite control repository authenticate task failed")?
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        let event = event.clone();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.increment_key_usage_rollup(&event)
        })
        .await
        .context("sqlite control repository rollup task failed")?
    }
}

#[async_trait]
impl UsageEventSink for SqliteControlRepository {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollup(event).await
    }
}

#[async_trait]
impl PublicAccessStore for SqliteControlRepository {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.get_runtime_config().map(|record| {
                record.map_or(DEFAULT_AUTH_CACHE_TTL_SECONDS, |record| {
                    record.auth_cache_ttl_seconds as u64
                })
            })
        })
        .await
        .context("sqlite control repository runtime config task failed")?
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_public_access_keys()
        })
        .await
        .context("sqlite control repository public keys task failed")?
    }
}

#[async_trait]
impl PublicCommunityStore for SqliteControlRepository {
    async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_public_account_contributions(limit)
        })
        .await
        .context("sqlite control repository public account contributions task failed")?
    }

    async fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.list_public_sponsors(limit)
        })
        .await
        .context("sqlite control repository public sponsors task failed")?
    }
}

#[async_trait]
impl PublicUsageStore for SqliteControlRepository {
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        let key_hash = hash_bearer_secret(secret);
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.get_public_usage_key_by_hash(&key_hash)
        })
        .await
        .context("sqlite control repository public usage key task failed")?
    }
}

#[async_trait]
impl PublicSubmissionStore for SqliteControlRepository {
    async fn create_public_token_request(
        &self,
        request: NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_public_token_request(&request)
        })
        .await
        .context("sqlite control repository public token request task failed")?
    }

    async fn create_public_account_contribution_request(
        &self,
        request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_public_account_contribution_request(&request)
        })
        .await
        .context("sqlite control repository public account contribution task failed")?
    }

    async fn create_public_sponsor_request(
        &self,
        request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            store.create_public_sponsor_request(&request)
        })
        .await
        .context("sqlite control repository public sponsor request task failed")?
    }
}

#[async_trait]
impl PublicStatusStore for SqliteControlRepository {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner
                .lock()
                .map_err(|_| anyhow!("sqlite control store mutex poisoned"))?;
            if let Some(snapshot) = store.get_codex_rate_limit_status()? {
                return Ok(snapshot);
            }
            let refresh_interval_seconds = store
                .get_runtime_config()?
                .map(|record| record.codex_status_refresh_max_interval_seconds as u64)
                .unwrap_or(DEFAULT_CODEX_STATUS_REFRESH_SECONDS);
            Ok(CodexRateLimitStatus::loading(refresh_interval_seconds))
        })
        .await
        .context("sqlite control repository codex status task failed")?
    }
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType, RouteStrategy},
        store::{
            AdminAccountGroupPatch, AdminAccountGroupStore, AdminCodexAccountPatch,
            AdminCodexAccountStore, AdminConfigStore, AdminKeyPatch, AdminKeyStore,
            AdminProxyConfigPatch, AdminProxyStore, CodexCredits, CodexPublicAccountStatus,
            CodexRateLimitBucket, CodexRateLimitStatus, CodexRateLimitWindow, ControlStore,
            NewAdminAccountGroup, NewAdminCodexAccount, NewAdminKey, NewAdminProxyConfig,
            PublicAccessStore, PublicCommunityStore, PublicStatusStore, PublicUsageStore,
            UsageEventSink,
        },
        usage::{UsageEvent, UsageTiming},
    };

    fn sample_event(key_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: "event-repository".to_string(),
            created_at_ms: 700,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: key_id.to_string(),
            key_name: "repo key".to_string(),
            account_name: None,
            route_strategy_at_event: Some(RouteStrategy::Auto),
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            mapped_model: Some("gpt-5.3-codex-spark".to_string()),
            status_code: 200,
            request_body_bytes: Some(512),
            input_uncached_tokens: 3,
            input_cached_tokens: 4,
            output_tokens: 5,
            billable_tokens: 6,
            credit_usage: None,
            usage_missing: false,
            credit_usage_missing: true,
            timing: UsageTiming::default(),
        }
    }

    fn sample_codex_status_snapshot() -> CodexRateLimitStatus {
        CodexRateLimitStatus {
            status: "ready".to_string(),
            refresh_interval_seconds: 120,
            last_checked_at: Some(1000),
            last_success_at: Some(1000),
            source_url: "https://chatgpt.com/backend-api/codex/usage".to_string(),
            error_message: None,
            accounts: vec![CodexPublicAccountStatus {
                name: "primary".to_string(),
                status: "active".to_string(),
                plan_type: Some("Pro".to_string()),
                primary_remaining_percent: Some(62.0),
                secondary_remaining_percent: Some(39.0),
                last_usage_checked_at: Some(1000),
                last_usage_success_at: Some(1000),
                usage_error_message: None,
            }],
            buckets: vec![CodexRateLimitBucket {
                limit_id: "codex".to_string(),
                limit_name: None,
                display_name: "codex".to_string(),
                is_primary: true,
                plan_type: Some("Pro".to_string()),
                primary: Some(CodexRateLimitWindow {
                    used_percent: 38.0,
                    remaining_percent: 62.0,
                    window_duration_mins: Some(300),
                    resets_at: Some(2000),
                }),
                secondary: Some(CodexRateLimitWindow {
                    used_percent: 61.0,
                    remaining_percent: 39.0,
                    window_duration_mins: Some(10080),
                    resets_at: Some(3000),
                }),
                credits: Some(CodexCredits {
                    has_credits: true,
                    unlimited: false,
                    balance: Some("24".to_string()),
                }),
                account_name: Some("primary".to_string()),
            }],
        }
    }

    #[tokio::test]
    async fn sqlite_repository_reads_and_updates_admin_runtime_config() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");

        let repo = super::SqliteControlRepository::new(conn);
        let mut config = repo
            .get_admin_runtime_config()
            .await
            .expect("load default config");
        assert_eq!(config.auth_cache_ttl_seconds, 60);
        assert_eq!(config.max_request_body_bytes, 8 * 1024 * 1024);
        assert_eq!(config.kiro_prefix_cache_mode, "prefix_tree");

        config.auth_cache_ttl_seconds = 75;
        config.codex_client_version = "0.125.0".to_string();
        config.kiro_prefix_cache_mode = "formula".to_string();
        let updated = repo
            .update_admin_runtime_config(config)
            .await
            .expect("update config");

        assert_eq!(updated.auth_cache_ttl_seconds, 75);
        assert_eq!(updated.codex_client_version, "0.125.0");
        assert_eq!(updated.kiro_prefix_cache_mode, "formula");

        let stored = repo
            .get_admin_runtime_config()
            .await
            .expect("reload config");
        assert_eq!(stored.auth_cache_ttl_seconds, 75);
        assert_eq!(stored.codex_client_version, "0.125.0");
        assert_eq!(stored.kiro_prefix_cache_mode, "formula");
    }

    #[tokio::test]
    async fn sqlite_repository_manages_admin_key_lifecycle() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlRepository::new(conn);

        let created = repo
            .create_admin_key(NewAdminKey {
                id: "llm-key-test".to_string(),
                name: "test key".to_string(),
                secret: "sfk_test".to_string(),
                key_hash: "hash-test".to_string(),
                public_visible: true,
                quota_billable_limit: 1000,
                request_max_concurrency: Some(2),
                request_min_start_interval_ms: Some(50),
                created_at_ms: 100,
            })
            .await
            .expect("create key");

        assert_eq!(created.id, "llm-key-test");
        assert_eq!(created.status, "active");
        assert_eq!(created.provider_type, "codex");
        assert_eq!(created.remaining_billable, 1000);
        assert_eq!(created.request_max_concurrency, Some(2));

        let listed = repo.list_admin_keys().await.expect("list keys");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, "llm-key-test");

        let patched = repo
            .patch_admin_key("llm-key-test", AdminKeyPatch {
                name: Some("patched key".to_string()),
                status: Some("disabled".to_string()),
                public_visible: Some(false),
                quota_billable_limit: Some(750),
                route_strategy: Some(Some("auto".to_string())),
                auto_account_names: Some(Some(vec!["codex-a".to_string(), "codex-b".to_string()])),
                request_max_concurrency: Some(None),
                updated_at_ms: 200,
                ..AdminKeyPatch::default()
            })
            .await
            .expect("patch key")
            .expect("key exists");

        assert_eq!(patched.name, "patched key");
        assert_eq!(patched.status, "disabled");
        assert!(!patched.public_visible);
        assert_eq!(patched.quota_billable_limit, 750);
        assert_eq!(patched.route_strategy.as_deref(), Some("auto"));
        assert_eq!(
            patched.auto_account_names,
            Some(vec!["codex-a".to_string(), "codex-b".to_string()])
        );
        assert_eq!(patched.request_max_concurrency, None);

        let deleted = repo
            .delete_admin_key("llm-key-test")
            .await
            .expect("delete key")
            .expect("key exists");
        assert_eq!(deleted.id, "llm-key-test");
        assert!(repo.list_admin_keys().await.expect("list keys").is_empty());
    }

    #[tokio::test]
    async fn sqlite_repository_manages_admin_account_groups_and_proxies() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlRepository::new(conn);

        let group = repo
            .create_admin_account_group(NewAdminAccountGroup {
                id: "llm-group-test".to_string(),
                provider_type: "codex".to_string(),
                name: "primary pool".to_string(),
                account_names: vec!["codex-a".to_string(), "codex-b".to_string()],
                created_at_ms: 100,
            })
            .await
            .expect("create account group");
        assert_eq!(group.id, "llm-group-test");
        assert_eq!(group.provider_type, "codex");

        let groups = repo
            .list_admin_account_groups("codex")
            .await
            .expect("list account groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].account_names, vec!["codex-a", "codex-b"]);

        let patched_group = repo
            .patch_admin_account_group("llm-group-test", AdminAccountGroupPatch {
                name: Some("patched pool".to_string()),
                account_names: Some(vec!["codex-c".to_string()]),
                updated_at_ms: 200,
            })
            .await
            .expect("patch account group")
            .expect("group exists");
        assert_eq!(patched_group.name, "patched pool");
        assert_eq!(patched_group.account_names, vec!["codex-c"]);

        let proxy = repo
            .create_admin_proxy_config(NewAdminProxyConfig {
                id: "llm-proxy-test".to_string(),
                name: "hk".to_string(),
                proxy_url: "http://127.0.0.1:11111".to_string(),
                proxy_username: Some("user".to_string()),
                proxy_password: Some("pass".to_string()),
                created_at_ms: 300,
            })
            .await
            .expect("create proxy config");
        assert_eq!(proxy.status, "active");

        let binding = repo
            .update_admin_proxy_binding("codex", Some("llm-proxy-test".to_string()))
            .await
            .expect("bind proxy");
        assert_eq!(binding.effective_source, "binding");
        assert_eq!(binding.effective_proxy_url.as_deref(), Some("http://127.0.0.1:11111"));

        let patched_proxy = repo
            .patch_admin_proxy_config("llm-proxy-test", AdminProxyConfigPatch {
                status: Some("disabled".to_string()),
                updated_at_ms: 400,
                ..AdminProxyConfigPatch::default()
            })
            .await
            .expect("patch proxy config")
            .expect("proxy exists");
        assert_eq!(patched_proxy.status, "disabled");

        let bindings = repo
            .list_admin_proxy_bindings()
            .await
            .expect("list proxy bindings");
        let codex_binding = bindings
            .iter()
            .find(|binding| binding.provider_type == "codex")
            .expect("codex binding");
        assert_eq!(codex_binding.effective_source, "invalid");
        assert_eq!(codex_binding.error_message.as_deref(), Some("bound proxy config is disabled"));

        repo.update_admin_proxy_binding("codex", None)
            .await
            .expect("clear proxy binding");
        let deleted_proxy = repo
            .delete_admin_proxy_config("llm-proxy-test")
            .await
            .expect("delete proxy config")
            .expect("proxy exists");
        assert_eq!(deleted_proxy.id, "llm-proxy-test");

        let deleted_group = repo
            .delete_admin_account_group("llm-group-test")
            .await
            .expect("delete account group")
            .expect("group exists");
        assert_eq!(deleted_group.id, "llm-group-test");
    }

    #[tokio::test]
    async fn sqlite_repository_manages_admin_codex_account_lifecycle() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlRepository::new(conn);

        let account = repo
            .create_admin_codex_account(NewAdminCodexAccount {
                name: "codex_primary".to_string(),
                account_id: Some("acct-1".to_string()),
                auth_json: serde_json::json!({
                    "id_token": "id",
                    "access_token": "access",
                    "refresh_token": "refresh",
                    "account_id": "acct-1",
                })
                .to_string(),
                map_gpt53_codex_to_spark: false,
                created_at_ms: 100,
            })
            .await
            .expect("create codex account");
        assert_eq!(account.name, "codex_primary");
        assert_eq!(account.status, "active");
        assert_eq!(account.proxy_mode, "inherit");

        let accounts = repo
            .list_admin_codex_accounts()
            .await
            .expect("list codex accounts");
        assert_eq!(accounts.len(), 1);

        let patched = repo
            .patch_admin_codex_account("codex_primary", AdminCodexAccountPatch {
                map_gpt53_codex_to_spark: Some(true),
                proxy_mode: Some("none".to_string()),
                request_max_concurrency: Some(Some(2)),
                request_min_start_interval_ms: Some(Some(50)),
                updated_at_ms: 200,
                ..AdminCodexAccountPatch::default()
            })
            .await
            .expect("patch codex account")
            .expect("account exists");
        assert!(patched.map_gpt53_codex_to_spark);
        assert_eq!(patched.proxy_mode, "none");
        assert_eq!(patched.request_max_concurrency, Some(2));
        assert_eq!(patched.request_min_start_interval_ms, Some(50));

        let refreshed = repo
            .refresh_admin_codex_account("codex_primary", 300)
            .await
            .expect("refresh codex account")
            .expect("account exists");
        assert_eq!(refreshed.last_refresh, Some(300));

        let deleted = repo
            .delete_admin_codex_account("codex_primary")
            .await
            .expect("delete codex account")
            .expect("account exists");
        assert_eq!(deleted.name, "codex_primary");
        assert!(repo
            .list_admin_codex_accounts()
            .await
            .expect("list codex accounts")
            .is_empty());
    }

    #[tokio::test]
    async fn sqlite_repository_authenticates_secret_and_applies_rollup() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let secret = "sk-repository";
        let key_hash = super::hash_bearer_secret(secret);
        conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (
                'key-repository', 'repo key', ?1, ?2, 'active', 'codex', 'openai',
                0, 1000, 10, 10
            )",
            rusqlite::params![secret, key_hash],
        )
        .expect("insert key");
        conn.execute(
            "INSERT INTO llm_key_route_config (
                key_id, route_strategy, kiro_request_validation_enabled,
                kiro_cache_estimation_enabled, kiro_zero_cache_debug_enabled
            ) VALUES ('key-repository', 'auto', 0, 0, 0)",
            [],
        )
        .expect("insert route");
        conn.execute(
            "INSERT INTO llm_key_usage_rollups (
                key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_total, credit_missing_events, updated_at_ms
            ) VALUES ('key-repository', 1, 2, 3, 4, '0', 0, 10)",
            [],
        )
        .expect("insert rollup");

        let repo = super::SqliteControlRepository::new(conn);
        let key = repo
            .authenticate_bearer_secret(secret)
            .await
            .expect("authenticate")
            .expect("key exists");
        assert_eq!(key.key_id, "key-repository");
        assert_eq!(key.billable_tokens_used, 4);

        repo.append_usage_event(&sample_event("key-repository"))
            .await
            .expect("append usage event");

        let key = repo
            .authenticate_bearer_secret(secret)
            .await
            .expect("authenticate")
            .expect("key exists");
        assert_eq!(key.billable_tokens_used, 10);
    }

    #[tokio::test]
    async fn sqlite_repository_lists_public_access_keys_with_rollups() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        conn.execute(
            "INSERT INTO llm_runtime_config (
                id, auth_cache_ttl_seconds, max_request_body_bytes,
                account_failure_retry_limit, codex_client_version,
                kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
                codex_status_refresh_min_interval_seconds,
                codex_status_refresh_max_interval_seconds,
                codex_status_account_jitter_max_seconds,
                kiro_status_refresh_min_interval_seconds,
                kiro_status_refresh_max_interval_seconds,
                kiro_status_account_jitter_max_seconds,
                usage_event_flush_batch_size,
                usage_event_flush_interval_seconds,
                usage_event_flush_max_buffer_bytes,
                usage_event_maintenance_enabled,
                usage_event_maintenance_interval_seconds,
                usage_event_detail_retention_days,
                kiro_cache_kmodels_json,
                kiro_billable_model_multipliers_json,
                kiro_cache_policy_json,
                kiro_prefix_cache_mode,
                kiro_prefix_cache_max_tokens,
                kiro_prefix_cache_entry_ttl_seconds,
                kiro_conversation_anchor_max_entries,
                kiro_conversation_anchor_ttl_seconds,
                updated_at_ms
            ) VALUES (
                'default', 42, 1048576, 3, '0.124.0',
                1, 0, 240, 300, 10, 240, 300, 10,
                100, 5, 1048576, 1, 3600, 30,
                '{}', '{}', '{}', 'prefix_tree', 4000000, 21600, 20000, 86400, 10
            )",
            [],
        )
        .expect("insert runtime config");
        conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES
                ('key-hidden', 'hidden key', 'sk-hidden', 'hash-hidden', 'active', 'codex',
                    'openai', 0, 1000, 10, 10),
                ('key-public', 'public key', 'sk-public', 'hash-public', 'active', 'codex',
                    'openai', 1, 1000, 10, 10)",
            [],
        )
        .expect("insert keys");
        conn.execute(
            "INSERT INTO llm_key_usage_rollups (
                key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                updated_at_ms
            ) VALUES ('key-public', 10, 20, 30, 40, '0', 0, 99, 99)",
            [],
        )
        .expect("insert rollup");

        let repo = super::SqliteControlRepository::new(conn);
        assert_eq!(repo.auth_cache_ttl_seconds().await.expect("load ttl"), 42);
        let keys = repo
            .list_public_access_keys()
            .await
            .expect("list public keys");

        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].key_id, "key-public");
        assert_eq!(keys[0].usage_billable_tokens, 40);
        assert_eq!(keys[0].remaining_billable(), 960);
        assert_eq!(keys[0].last_used_at_ms, Some(99));
    }

    #[tokio::test]
    async fn sqlite_repository_loads_public_usage_key_by_secret() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let secret = "sk-public-usage";
        let key_hash = super::hash_bearer_secret(secret);
        conn.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
            ) VALUES (
                'key-public-usage', 'usage key', ?1, ?2, 'active', 'codex',
                'openai', 0, 1000, 10, 10
            )",
            rusqlite::params![secret, key_hash],
        )
        .expect("insert key");
        conn.execute(
            "INSERT INTO llm_key_usage_rollups (
                key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                updated_at_ms
            ) VALUES ('key-public-usage', 10, 20, 30, 40, '1.25', 2, 99, 99)",
            [],
        )
        .expect("insert rollup");

        let repo = super::SqliteControlRepository::new(conn);
        let key = repo
            .get_public_usage_key_by_secret(secret)
            .await
            .expect("lookup key")
            .expect("key exists");

        assert_eq!(key.key_id, "key-public-usage");
        assert_eq!(key.provider_type, "codex");
        assert_eq!(key.status, "active");
        assert!(!key.public_visible);
        assert_eq!(key.usage_billable_tokens, 40);
        assert_eq!(key.usage_credit_total, 1.25);
        assert_eq!(key.usage_credit_missing_events, 2);
        assert_eq!(key.remaining_billable(), 960);
    }

    #[tokio::test]
    async fn sqlite_repository_lists_issued_public_account_contributions() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        conn.execute(
            "INSERT INTO llm_account_contribution_requests (
                request_id, account_name, account_id, id_token, access_token, refresh_token,
                requester_email, contributor_message, github_id, frontend_page_url, status,
                fingerprint, client_ip, ip_region, admin_note, failure_reason,
                imported_account_name, issued_key_id, issued_key_name, created_at_ms,
                updated_at_ms, processed_at_ms
            ) VALUES
                (
                    'contribution-pending', 'pending account', NULL, 'id', 'access', 'refresh',
                    'pending@example.test', 'not visible', NULL, NULL, 'pending',
                    'fp-pending', '127.0.0.1', 'local', NULL, NULL, NULL, NULL, NULL,
                    100, 100, NULL
                ),
                (
                    'contribution-old', 'old account', NULL, 'id', 'access', 'refresh',
                    'old@example.test', 'old message', 'old-gh', NULL, 'issued',
                    'fp-old', '127.0.0.1', 'local', NULL, NULL, NULL, 'key-old', 'key old',
                    200, 300, 300
                ),
                (
                    'contribution-new', 'raw account', NULL, 'id', 'access', 'refresh',
                    'new@example.test', 'new message', 'new-gh', NULL, 'issued',
                    'fp-new', '127.0.0.1', 'local', NULL, NULL, 'imported account',
                    'key-new', 'key new', 400, 500, 500
                )",
            [],
        )
        .expect("insert account contribution requests");

        let repo = super::SqliteControlRepository::new(conn);
        let contributions = repo
            .list_public_account_contributions(10)
            .await
            .expect("list public account contributions");

        assert_eq!(contributions.len(), 2);
        assert_eq!(contributions[0].request_id, "contribution-new");
        assert_eq!(contributions[0].account_name, "imported account");
        assert_eq!(contributions[0].contributor_message, "new message");
        assert_eq!(contributions[0].github_id.as_deref(), Some("new-gh"));
        assert_eq!(contributions[0].processed_at_ms, Some(500));
        assert_eq!(contributions[1].request_id, "contribution-old");
        assert_eq!(contributions[1].account_name, "old account");
    }

    #[tokio::test]
    async fn sqlite_repository_lists_approved_public_sponsors() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        conn.execute(
            "INSERT INTO llm_sponsor_requests (
                request_id, requester_email, sponsor_message, display_name, github_id,
                frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                processed_at_ms
            ) VALUES
                (
                    'sponsor-submitted', 'submitted@example.test', 'not visible',
                    'submitted', NULL, NULL, 'submitted', 'fp-submitted', '127.0.0.1',
                    'local', NULL, NULL, NULL, 100, 100, NULL
                ),
                (
                    'sponsor-old', 'old@example.test', 'old sponsor', NULL, 'old-gh',
                    NULL, 'approved', 'fp-old', '127.0.0.1', 'local', NULL, NULL, NULL,
                    200, 300, 300
                ),
                (
                    'sponsor-new', 'new@example.test', 'new sponsor', 'New Sponsor',
                    'new-gh', NULL, 'approved', 'fp-new', '127.0.0.1', 'local',
                    NULL, NULL, NULL, 400, 500, 500
                )",
            [],
        )
        .expect("insert sponsor requests");

        let repo = super::SqliteControlRepository::new(conn);
        let sponsors = repo
            .list_public_sponsors(10)
            .await
            .expect("list public sponsors");

        assert_eq!(sponsors.len(), 2);
        assert_eq!(sponsors[0].request_id, "sponsor-new");
        assert_eq!(sponsors[0].display_name.as_deref(), Some("New Sponsor"));
        assert_eq!(sponsors[0].sponsor_message, "new sponsor");
        assert_eq!(sponsors[0].github_id.as_deref(), Some("new-gh"));
        assert_eq!(sponsors[0].processed_at_ms, Some(500));
        assert_eq!(sponsors[1].request_id, "sponsor-old");
        assert_eq!(sponsors[1].display_name, None);
    }

    #[tokio::test]
    async fn sqlite_repository_returns_loading_codex_status_without_snapshot() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");

        let repo = super::SqliteControlRepository::new(conn);
        let status = repo
            .codex_rate_limit_status()
            .await
            .expect("load codex status");

        assert_eq!(status, CodexRateLimitStatus::loading(300));
    }

    #[tokio::test]
    async fn sqlite_repository_returns_persisted_codex_status_snapshot() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let snapshot = sample_codex_status_snapshot();
        let snapshot_json = serde_json::to_string(&snapshot).expect("serialize snapshot");
        conn.execute(
            "INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
             VALUES ('default', ?1, 1000)",
            rusqlite::params![snapshot_json],
        )
        .expect("insert status snapshot");

        let repo = super::SqliteControlRepository::new(conn);
        let loaded = repo
            .codex_rate_limit_status()
            .await
            .expect("load codex status");

        assert_eq!(loaded, snapshot);
    }
}
