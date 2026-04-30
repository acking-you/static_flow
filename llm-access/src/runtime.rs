//! Runtime startup validation for the standalone LLM access service.

use std::sync::Arc;

use anyhow::{anyhow, Context};
use llm_access_core::store::{
    AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
    AdminProxyStore, AdminReviewQueueStore, ControlStore, EmptyAdminAccountGroupStore,
    EmptyAdminCodexAccountStore, EmptyAdminConfigStore, EmptyAdminKeyStore, EmptyAdminProxyStore,
    EmptyAdminReviewQueueStore, EmptyProviderRouteStore, EmptyPublicAccessStore,
    EmptyPublicCommunityStore, EmptyPublicStatusStore, EmptyPublicSubmissionStore,
    EmptyPublicUsageStore, ProviderRouteStore, PublicAccessStore, PublicCommunityStore,
    PublicStatusStore, PublicSubmissionStore, PublicUsageStore,
};
use llm_access_store::repository::SqliteControlRepository;

use crate::config::StorageConfig;

/// Runtime dependencies shared by provider routes.
#[derive(Clone)]
pub struct LlmAccessRuntime {
    control_store: Arc<dyn ControlStore>,
    provider_route_store: Arc<dyn ProviderRouteStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    admin_review_queue_store: Arc<dyn AdminReviewQueueStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
}

/// Runtime dependency bundle used to keep construction explicit as the
/// standalone service grows.
struct LlmAccessStores {
    control_store: Arc<dyn ControlStore>,
    provider_route_store: Arc<dyn ProviderRouteStore>,
    admin_config_store: Arc<dyn AdminConfigStore>,
    admin_key_store: Arc<dyn AdminKeyStore>,
    admin_account_group_store: Arc<dyn AdminAccountGroupStore>,
    admin_proxy_store: Arc<dyn AdminProxyStore>,
    admin_codex_account_store: Arc<dyn AdminCodexAccountStore>,
    admin_review_queue_store: Arc<dyn AdminReviewQueueStore>,
    public_access_store: Arc<dyn PublicAccessStore>,
    public_community_store: Arc<dyn PublicCommunityStore>,
    public_usage_store: Arc<dyn PublicUsageStore>,
    public_submission_store: Arc<dyn PublicSubmissionStore>,
    public_status_store: Arc<dyn PublicStatusStore>,
}

impl LlmAccessRuntime {
    /// Create runtime dependencies from explicit storage adapters.
    pub fn new(control_store: Arc<dyn ControlStore>) -> Self {
        Self::with_stores(LlmAccessStores {
            control_store,
            provider_route_store: Arc::new(EmptyProviderRouteStore),
            admin_config_store: Arc::new(EmptyAdminConfigStore),
            admin_key_store: Arc::new(EmptyAdminKeyStore),
            admin_account_group_store: Arc::new(EmptyAdminAccountGroupStore),
            admin_proxy_store: Arc::new(EmptyAdminProxyStore),
            admin_codex_account_store: Arc::new(EmptyAdminCodexAccountStore),
            admin_review_queue_store: Arc::new(EmptyAdminReviewQueueStore),
            public_access_store: Arc::new(EmptyPublicAccessStore),
            public_community_store: Arc::new(EmptyPublicCommunityStore),
            public_usage_store: Arc::new(EmptyPublicUsageStore),
            public_submission_store: Arc::new(EmptyPublicSubmissionStore),
            public_status_store: Arc::new(EmptyPublicStatusStore),
        })
    }

    /// Create runtime dependencies from explicit storage adapters.
    fn with_stores(stores: LlmAccessStores) -> Self {
        Self {
            control_store: stores.control_store,
            provider_route_store: stores.provider_route_store,
            admin_config_store: stores.admin_config_store,
            admin_key_store: stores.admin_key_store,
            admin_account_group_store: stores.admin_account_group_store,
            admin_proxy_store: stores.admin_proxy_store,
            admin_codex_account_store: stores.admin_codex_account_store,
            admin_review_queue_store: stores.admin_review_queue_store,
            public_access_store: stores.public_access_store,
            public_community_store: stores.public_community_store,
            public_usage_store: stores.public_usage_store,
            public_submission_store: stores.public_submission_store,
            public_status_store: stores.public_status_store,
        }
    }

    /// Open runtime dependencies from configured persistent storage.
    pub fn from_storage_config(config: &StorageConfig) -> anyhow::Result<Self> {
        validate_state_root(config)?;
        let repository = Arc::new(SqliteControlRepository::open_path(&config.sqlite_control)?);
        let control_store: Arc<dyn ControlStore> = repository.clone();
        let provider_route_store: Arc<dyn ProviderRouteStore> = repository.clone();
        let admin_config_store: Arc<dyn AdminConfigStore> = repository.clone();
        let admin_key_store: Arc<dyn AdminKeyStore> = repository.clone();
        let admin_account_group_store: Arc<dyn AdminAccountGroupStore> = repository.clone();
        let admin_proxy_store: Arc<dyn AdminProxyStore> = repository.clone();
        let admin_codex_account_store: Arc<dyn AdminCodexAccountStore> = repository.clone();
        let admin_review_queue_store: Arc<dyn AdminReviewQueueStore> = repository.clone();
        let public_access_store: Arc<dyn PublicAccessStore> = repository.clone();
        let public_community_store: Arc<dyn PublicCommunityStore> = repository.clone();
        let public_usage_store: Arc<dyn PublicUsageStore> = repository.clone();
        let public_submission_store: Arc<dyn PublicSubmissionStore> = repository.clone();
        let public_status_store: Arc<dyn PublicStatusStore> = repository;
        Ok(Self::with_stores(LlmAccessStores {
            control_store,
            provider_route_store,
            admin_config_store,
            admin_key_store,
            admin_account_group_store,
            admin_proxy_store,
            admin_codex_account_store,
            admin_review_queue_store,
            public_access_store,
            public_community_store,
            public_usage_store,
            public_submission_store,
            public_status_store,
        }))
    }

    /// Shared control store used by request handlers.
    pub fn control_store(&self) -> Arc<dyn ControlStore> {
        Arc::clone(&self.control_store)
    }

    /// Provider route store used by data-plane dispatch.
    pub fn provider_route_store(&self) -> Arc<dyn ProviderRouteStore> {
        Arc::clone(&self.provider_route_store)
    }

    /// Admin config store used by local admin compatibility endpoints.
    pub fn admin_config_store(&self) -> Arc<dyn AdminConfigStore> {
        Arc::clone(&self.admin_config_store)
    }

    /// Admin key store used by local admin compatibility endpoints.
    pub fn admin_key_store(&self) -> Arc<dyn AdminKeyStore> {
        Arc::clone(&self.admin_key_store)
    }

    /// Admin account-group store used by local admin compatibility endpoints.
    pub fn admin_account_group_store(&self) -> Arc<dyn AdminAccountGroupStore> {
        Arc::clone(&self.admin_account_group_store)
    }

    /// Admin proxy store used by local admin compatibility endpoints.
    pub fn admin_proxy_store(&self) -> Arc<dyn AdminProxyStore> {
        Arc::clone(&self.admin_proxy_store)
    }

    /// Admin Codex account store used by local admin compatibility endpoints.
    pub fn admin_codex_account_store(&self) -> Arc<dyn AdminCodexAccountStore> {
        Arc::clone(&self.admin_codex_account_store)
    }

    /// Admin review queue store used by local admin compatibility endpoints.
    pub fn admin_review_queue_store(&self) -> Arc<dyn AdminReviewQueueStore> {
        Arc::clone(&self.admin_review_queue_store)
    }

    /// Public access store used by unauthenticated compatibility endpoints.
    pub fn public_access_store(&self) -> Arc<dyn PublicAccessStore> {
        Arc::clone(&self.public_access_store)
    }

    /// Public community store used by unauthenticated compatibility endpoints.
    pub fn public_community_store(&self) -> Arc<dyn PublicCommunityStore> {
        Arc::clone(&self.public_community_store)
    }

    /// Public usage store used by unauthenticated compatibility endpoints.
    pub fn public_usage_store(&self) -> Arc<dyn PublicUsageStore> {
        Arc::clone(&self.public_usage_store)
    }

    /// Public submission store used by unauthenticated compatibility endpoints.
    pub fn public_submission_store(&self) -> Arc<dyn PublicSubmissionStore> {
        Arc::clone(&self.public_submission_store)
    }

    /// Public status store used by unauthenticated compatibility endpoints.
    pub fn public_status_store(&self) -> Arc<dyn PublicStatusStore> {
        Arc::clone(&self.public_status_store)
    }
}

/// Validate and prepare the persistent state root before storage is opened.
pub fn validate_state_root(config: &StorageConfig) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(&config.state_root).with_context(|| {
        format!("state root `{}` is not accessible", config.state_root.display())
    })?;
    if !metadata.is_dir() {
        return Err(anyhow!("state root `{}` is not a directory", config.state_root.display()));
    }
    for dir in [&config.kiro_auths_dir, &config.codex_auths_dir, &config.cdc_dir, &config.logs_dir]
    {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    fn temp_storage_config(name: &str) -> crate::config::StorageConfig {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("llm-access-{name}-{}-{unique}", std::process::id()));
        crate::config::StorageConfig {
            state_root: root.clone(),
            sqlite_control: root.join("control/llm-access.sqlite3"),
            duckdb: root.join("analytics/usage.duckdb"),
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            cdc_dir: root.join("cdc"),
            logs_dir: root.join("logs"),
        }
    }

    #[test]
    fn validate_state_root_creates_expected_subdirectories() {
        let config = temp_storage_config("state-root");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");

        super::validate_state_root(&config).expect("validate root");

        assert!(config.kiro_auths_dir.is_dir());
        assert!(config.codex_auths_dir.is_dir());
        assert!(config.cdc_dir.is_dir());
        assert!(config.logs_dir.is_dir());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[tokio::test]
    async fn opens_runtime_control_store_from_sqlite_path() {
        let config = temp_storage_config("runtime-open");
        let root = config.state_root.clone();
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)
            .expect("initialize sqlite");

        let runtime =
            super::LlmAccessRuntime::from_storage_config(&config).expect("open runtime storage");
        let key = runtime
            .control_store()
            .authenticate_bearer_secret("missing")
            .await
            .expect("query store");

        assert!(key.is_none());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
