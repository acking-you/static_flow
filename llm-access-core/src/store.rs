//! Storage traits consumed by provider runtimes.

use async_trait::async_trait;

use crate::usage::UsageEvent;

/// Key state used on the hot request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type as snake_case string.
    pub provider_type: String,
    /// Protocol family as snake_case string.
    pub protocol_family: String,
    /// Key status.
    pub status: String,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Billable usage already consumed.
    pub billable_tokens_used: i64,
}

/// Control-plane queries used by request handlers.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Authenticate a bearer secret by hashing it and loading the key state.
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>>;

    /// Increment usage counters for a key after a usage event is accepted.
    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()>;
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()>;
}
