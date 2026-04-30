//! Async repository adapters for llm-access runtime traits.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use llm_access_core::{
    store::{
        AuthenticatedKey, CodexRateLimitStatus, ControlStore, PublicAccessKey, PublicAccessStore,
        PublicStatusStore, UsageEventSink, DEFAULT_AUTH_CACHE_TTL_SECONDS,
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
            CodexCredits, CodexPublicAccountStatus, CodexRateLimitBucket, CodexRateLimitStatus,
            CodexRateLimitWindow, ControlStore, PublicAccessStore, PublicStatusStore,
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
