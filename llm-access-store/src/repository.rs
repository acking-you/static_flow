//! Async repository adapters for llm-access runtime traits.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use llm_access_core::{
    store::{AuthenticatedKey, ControlStore, UsageEventSink},
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

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType, RouteStrategy},
        store::{ControlStore, UsageEventSink},
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
}
