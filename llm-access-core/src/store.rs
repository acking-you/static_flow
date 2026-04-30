//! Storage traits consumed by provider runtimes.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::usage::UsageEvent;

/// Default public auth-cache TTL used when no runtime config row exists yet.
pub const DEFAULT_AUTH_CACHE_TTL_SECONDS: u64 = 60;
/// Default Codex status refresh interval used before runtime config is
/// imported.
pub const DEFAULT_CODEX_STATUS_REFRESH_SECONDS: u64 = 300;

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

/// Public thank-you card for an approved account contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccountContribution {
    /// Request id.
    pub request_id: String,
    /// Imported account display name.
    pub account_name: String,
    /// Contributor-supplied message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval/issuance timestamp.
    pub processed_at_ms: Option<i64>,
}

/// Public thank-you card for an approved sponsor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSponsor {
    /// Request id.
    pub request_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Sponsor-supplied message.
    pub sponsor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval timestamp.
    pub processed_at_ms: Option<i64>,
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

/// Public read-only community queries used by unauthenticated compatibility
/// endpoints.
#[async_trait]
pub trait PublicCommunityStore: Send + Sync {
    /// Approved account contribution cards.
    async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>>;

    /// Approved sponsor cards.
    async fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>>;
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

/// Empty community store used by isolated unit tests.
pub struct EmptyPublicCommunityStore;

#[async_trait]
impl PublicCommunityStore for EmptyPublicCommunityStore {
    async fn list_public_account_contributions(
        &self,
        _limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        Ok(Vec::new())
    }

    async fn list_public_sponsors(&self, _limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        Ok(Vec::new())
    }
}

/// Public read-only payload for the cached Codex rate-limit snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitStatus {
    /// Snapshot status label.
    pub status: String,
    /// Suggested client refresh interval in seconds.
    pub refresh_interval_seconds: u64,
    /// Last refresh attempt timestamp in Unix milliseconds.
    pub last_checked_at: Option<i64>,
    /// Last successful refresh timestamp in Unix milliseconds.
    pub last_success_at: Option<i64>,
    /// Upstream source URL used for the status refresh.
    pub source_url: String,
    /// Last refresh error, if any.
    pub error_message: Option<String>,
    /// Per-account public summaries.
    #[serde(default)]
    pub accounts: Vec<CodexPublicAccountStatus>,
    /// Flattened rate-limit buckets across active accounts.
    pub buckets: Vec<CodexRateLimitBucket>,
}

impl CodexRateLimitStatus {
    /// Construct the same empty loading state used before the status cache
    /// warms.
    pub fn loading(refresh_interval_seconds: u64) -> Self {
        Self {
            status: "loading".to_string(),
            refresh_interval_seconds,
            last_checked_at: None,
            last_success_at: None,
            source_url: String::new(),
            error_message: None,
            accounts: Vec::new(),
            buckets: Vec::new(),
        }
    }
}

/// One public Codex account summary rendered on `/llm-access`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexPublicAccountStatus {
    /// Account display name.
    pub name: String,
    /// Runtime status label.
    pub status: String,
    /// Upstream plan type when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    /// Primary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_remaining_percent: Option<f64>,
    /// Secondary bucket remaining percentage when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary_remaining_percent: Option<f64>,
    /// Last usage refresh attempt timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_checked_at: Option<i64>,
    /// Last successful usage refresh timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage_success_at: Option<i64>,
    /// Last usage refresh error, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage_error_message: Option<String>,
}

/// One limit bucket rendered on the public status surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitBucket {
    /// Upstream limit id.
    pub limit_id: String,
    /// Upstream limit name when available.
    pub limit_name: Option<String>,
    /// Human-readable bucket name.
    pub display_name: String,
    /// Whether this is the primary request bucket.
    pub is_primary: bool,
    /// Plan type attached to this bucket when known.
    pub plan_type: Option<String>,
    /// Primary rolling window.
    pub primary: Option<CodexRateLimitWindow>,
    /// Secondary rolling window.
    pub secondary: Option<CodexRateLimitWindow>,
    /// Credit metadata when upstream provides it.
    pub credits: Option<CodexCredits>,
    /// Account that owns this bucket in multi-account mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_name: Option<String>,
}

/// One usage window within a rate-limit bucket.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexRateLimitWindow {
    /// Used percentage.
    pub used_percent: f64,
    /// Remaining percentage.
    pub remaining_percent: f64,
    /// Window duration in minutes.
    pub window_duration_mins: Option<i64>,
    /// Reset timestamp in Unix milliseconds.
    pub resets_at: Option<i64>,
}

/// Credit metadata included in upstream usage payloads when available.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CodexCredits {
    /// Whether this bucket carries credit data.
    pub has_credits: bool,
    /// Whether the account reports unlimited credits.
    pub unlimited: bool,
    /// Printable balance value.
    pub balance: Option<String>,
}

/// Public read-only queries for compatibility status endpoints.
#[async_trait]
pub trait PublicStatusStore: Send + Sync {
    /// Current cached Codex public rate-limit status.
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus>;
}

/// Empty status store used by isolated unit tests.
pub struct EmptyPublicStatusStore;

#[async_trait]
impl PublicStatusStore for EmptyPublicStatusStore {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        Ok(CodexRateLimitStatus::loading(DEFAULT_CODEX_STATUS_REFRESH_SECONDS))
    }
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()>;
}
