//! Storage traits consumed by provider runtimes.

use async_trait::async_trait;

use crate::usage::UsageEvent;

/// Default public auth-cache TTL used when no runtime config row exists yet.
pub const DEFAULT_AUTH_CACHE_TTL_SECONDS: u64 = 60;

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

impl AuthenticatedKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        self.quota_billable_limit
            .saturating_sub(self.billable_tokens_used)
    }
}

/// Public-safe key summary used by the unauthenticated access endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccessKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Plaintext public key secret.
    pub secret: String,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated billable tokens.
    pub usage_billable_tokens: u64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
}

impl PublicAccessKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        let limit = i64::try_from(self.quota_billable_limit).unwrap_or(i64::MAX);
        let used = i64::try_from(self.usage_billable_tokens).unwrap_or(i64::MAX);
        limit.saturating_sub(used)
    }
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

/// Public read-only queries used by unauthenticated compatibility endpoints.
#[async_trait]
pub trait PublicAccessStore: Send + Sync {
    /// Current auth-cache TTL in seconds.
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64>;

    /// Active, public-visible LLM gateway keys.
    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>>;
}

/// Empty public-access store used by isolated unit tests.
pub struct EmptyPublicAccessStore;

#[async_trait]
impl PublicAccessStore for EmptyPublicAccessStore {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        Ok(DEFAULT_AUTH_CACHE_TTL_SECONDS)
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        Ok(Vec::new())
    }
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()>;
}
