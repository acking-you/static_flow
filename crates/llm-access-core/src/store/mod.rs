//! Storage traits consumed by provider runtimes.
//!
//! ## Module map
//!
//! `store/` keeps the shared default consts in this `mod.rs` facade and
//! splits the data model + `*Store` trait interfaces + their `Empty*`
//! no-op impls into focused submodules, re-exported by name.

mod codex_account;
mod codex_status;
mod config;
mod empty;
mod groups;
mod keys;
mod kiro_account;
mod proxy;
mod public;
mod routes;
mod traits;
mod usage;

pub use codex_account::{
    AdminAccountsSummary, AdminCodexAccount, AdminCodexAccountPageQuery, AdminCodexAccountPatch,
    AdminCodexAccountSortMode, AdminCodexAccountsPage, AdminCodexImportJobDetail,
    AdminCodexImportJobItem, AdminCodexImportJobItemResult, AdminCodexImportJobSummary,
    CodexStatusRefreshTarget, NewAdminCodexAccount, NewAdminCodexImportJob,
    NewAdminCodexImportJobItem,
};
pub use codex_status::{
    CodexCredits, CodexPublicAccountStatus, CodexRateLimitBucket, CodexRateLimitStatus,
    CodexRateLimitWindow,
};
pub use config::{
    compute_billable_tokens, compute_kiro_billable_tokens,
    default_kiro_billable_model_multipliers_json, default_kiro_cache_kmodels_json,
    default_kiro_cache_policy_json, AdminRuntimeConfig, UpdateAdminRuntimeConfig,
};
pub use empty::{
    EmptyAdminAccountGroupStore, EmptyAdminCodexAccountStore, EmptyAdminConfigStore,
    EmptyAdminKeyStore, EmptyAdminKiroAccountStore, EmptyAdminProxyStore,
    EmptyAdminReviewQueueStore, EmptyProviderRouteStore, EmptyPublicAccessStore,
    EmptyPublicCommunityStore, EmptyPublicStatusStore, EmptyPublicSubmissionStore,
    EmptyPublicUsageStore, EmptyUsageAnalyticsStore, NoopUsageEventSink,
};
pub use groups::{
    AdminAccountGroup, AdminAccountGroupOption, AdminAccountGroupPatch, AdminAccountGroupsPage,
    NewAdminAccountGroup,
};
pub use keys::{
    AdminKey, AdminKeyPageQuery, AdminKeyPatch, AdminKeySortMode, AdminKeysPage, AdminKeysSummary,
    AdminKiroKeyCandidateCreditSummary, AdminPageRequest, NewAdminKey,
};
pub use kiro_account::{
    AdminKiroAccount, AdminKiroAccountPatch, AdminKiroAccountsPage, AdminKiroBalanceView,
    AdminKiroCacheView, AdminKiroStatusCacheUpdate, KiroStatusRefreshTarget, NewAdminKiroAccount,
};
pub use proxy::{
    default_proxy_bindings, AdminProxyBinding, AdminProxyConfig, AdminProxyConfigPatch,
    AdminProxyEndpointCheck, AdminProxyEndpointCheckUpdate, NewAdminProxyConfig,
};
pub use public::{
    AdminAccountContributionRequest, AdminAccountContributionRequestsPage, AdminReviewQueueAction,
    AdminReviewQueueQuery, AdminSponsorRequest, AdminSponsorRequestsPage, AdminTokenRequest,
    AdminTokenRequestsPage, NewPublicAccountContributionRequest, NewPublicSponsorRequest,
    NewPublicTokenRequest, PublicAccessKey, PublicAccountContribution, PublicSponsor,
    PublicUsageLookupKey,
};
pub use routes::{
    codex_access_token_expires_at_ms, codex_auth_access_token_expires_at_ms,
    is_terminal_codex_auth_error, jwt_expiry_unix_ms, AuthenticatedKey, ProviderCodexAuthUpdate,
    ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute, ProviderProxyConfig,
};
pub use traits::{
    AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
    AdminKiroAccountStore, AdminProxyStore, AdminReviewQueueStore, ControlStore,
    ProviderRouteStore, PublicAccessStore, PublicCommunityStore, PublicStatusStore,
    PublicSubmissionStore, PublicUsageStore, UsageAnalyticsStore, UsageEventSink,
};
pub use usage::{
    AdminLegacyKiroProxyMigration, KiroLatencyRankingQuery, KiroLatencyRankingRow,
    KiroLatencyRankingSnapshot, UsageChartPoint, UsageEventPage, UsageEventQuery, UsageEventSource,
    UsageEventStatusKind, UsageEventTotals, UsageFilterOptions, UsageMetricsDimensionView,
    UsageMetricsQuery, UsageMetricsSnapshot, UsageMetricsStatusCodeView, UsageMetricsSummary,
};

/// Default public auth-cache TTL used when no runtime config row exists yet.
pub const DEFAULT_AUTH_CACHE_TTL_SECONDS: u64 = 60;
/// Default Codex status refresh interval used before runtime config is
/// imported.
pub const DEFAULT_CODEX_STATUS_REFRESH_SECONDS: u64 = 300;
/// Default maximum request body size enforced by provider request handlers.
pub const DEFAULT_MAX_REQUEST_BODY_BYTES: u64 = 8 * 1024 * 1024;
/// Default consecutive upstream failure threshold before an account is skipped.
pub const DEFAULT_ACCOUNT_FAILURE_RETRY_LIMIT: u64 = 10;
/// Default Codex client version sent to upstream requests.
pub const DEFAULT_CODEX_CLIENT_VERSION: &str = "0.124.0";
/// Default lower bound for randomized Codex status refresh.
pub const DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
/// Default upper bound for randomized Codex status refresh.
pub const DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
/// Default maximum Codex account refresh jitter.
pub const DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
/// Default weighted auto-routing multiplier for Free Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_FREE: u64 = 1;
/// Default weighted auto-routing multiplier for Plus Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PLUS: u64 = 10;
/// Default weighted auto-routing multiplier for Pro 5x Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PRO5X: u64 = 50;
/// Default weighted auto-routing multiplier for Pro 20x Codex accounts.
pub const DEFAULT_CODEX_WEIGHT_PRO20X: u64 = 200;
/// Default lower bound for randomized Kiro status refresh.
pub const DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS: u64 = 240;
/// Default upper bound for randomized Kiro status refresh.
pub const DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS: u64 = 300;
/// Default maximum Kiro account refresh jitter.
pub const DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS: u64 = 10;
/// Default usage-event flush batch size.
pub const DEFAULT_USAGE_EVENT_FLUSH_BATCH_SIZE: u64 = 256;
/// Default usage-event timed flush interval.
pub const DEFAULT_USAGE_EVENT_FLUSH_INTERVAL_SECONDS: u64 = 15;
/// Default usage-event buffered payload cap.
pub const DEFAULT_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES: u64 = 8 * 1024 * 1024;
/// Default DuckDB usage writer memory limit in MiB.
pub const DEFAULT_DUCKDB_USAGE_MEMORY_LIMIT_MIB: u64 = 1024;
/// Default DuckDB usage writer WAL checkpoint threshold in MiB.
pub const DEFAULT_DUCKDB_USAGE_CHECKPOINT_THRESHOLD_MIB: u64 = 16;
/// Default retained usage analytics horizon in days.
pub const DEFAULT_USAGE_ANALYTICS_RETENTION_DAYS: u64 = 7;
/// Default usage-journal write toggle.
pub const DEFAULT_USAGE_JOURNAL_ENABLED: bool = true;
/// Default compressed journal file rollover size.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// Default journal file age rollover threshold.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILE_AGE_MS: u64 = 300_000;
/// Default maximum journal files kept on disk.
pub const DEFAULT_USAGE_JOURNAL_MAX_FILES: u64 = 128;
/// Default journal block target before compression.
pub const DEFAULT_USAGE_JOURNAL_BLOCK_TARGET_UNCOMPRESSED_BYTES: u64 = 1024 * 1024;
/// Default maximum usage events per journal block.
pub const DEFAULT_USAGE_JOURNAL_BLOCK_MAX_EVENTS: u64 = 1024;
/// Default journal fsync interval.
pub const DEFAULT_USAGE_JOURNAL_FSYNC_INTERVAL_MS: u64 = 250;
/// Default journal zstd compression level.
pub const DEFAULT_USAGE_JOURNAL_ZSTD_LEVEL: i64 = 3;
/// Default worker lease age before a claimed journal is recovered.
pub const DEFAULT_USAGE_JOURNAL_CONSUMER_LEASE_MS: u64 = 300_000;
/// Default corrupt-file policy.
pub const DEFAULT_USAGE_JOURNAL_DELETE_BAD_FILES: bool = false;
/// Default worker query bind address.
pub const DEFAULT_USAGE_QUERY_BIND_ADDR: &str = "127.0.0.1:19081";
/// Default worker query base URL used by the API process.
pub const DEFAULT_USAGE_QUERY_BASE_URL: &str = "http://127.0.0.1:19081";
/// Default usage maintenance toggle.
pub const DEFAULT_USAGE_EVENT_MAINTENANCE_ENABLED: bool = true;
/// Default usage maintenance interval.
pub const DEFAULT_USAGE_EVENT_MAINTENANCE_INTERVAL_SECONDS: u64 = 60 * 60;
/// Default detailed usage retention.
pub const DEFAULT_USAGE_EVENT_DETAIL_RETENTION_DAYS: i64 = 7;
/// Default request-token threshold below which Kiro contextUsage is ignored.
pub const DEFAULT_KIRO_CONTEXT_USAGE_MIN_REQUEST_TOKENS: u64 = 15_000;
/// Default Kiro prefix cache mode.
pub const DEFAULT_KIRO_PREFIX_CACHE_MODE: &str = "prefix_tree";
/// Alternate Kiro prefix cache mode retained for admin compatibility.
pub const KIRO_PREFIX_CACHE_MODE_FORMULA: &str = "formula";
/// Default Kiro prefix-cache budget.
pub const DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS: u64 = 1_000_000;
/// Default Kiro prefix-cache entry TTL.
pub const DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS: u64 = 2 * 60 * 60;
/// Default Kiro conversation anchor capacity.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES: u64 = 4_096;
/// Default Kiro conversation anchor TTL.
pub const DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS: u64 = 6 * 60 * 60;
/// Default Kiro account channel concurrency retained in storage.
pub const DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY: u64 = 1;
/// Default Kiro account request pacing interval retained in storage.
pub const DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS: u64 = 0;
/// Pending status used by public token/account contribution requests.
pub const PUBLIC_TOKEN_REQUEST_STATUS_PENDING: &str = "pending";
/// Validated status used by account contribution requests after auth refresh
/// checks.
pub const PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED: &str = "validated";
/// Submitted status used by public sponsor requests before payment email.
pub const PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED: &str = "submitted";
/// Sponsor status used after payment instructions were sent.
pub const PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT: &str = "payment_email_sent";
/// Active managed key status.
pub const KEY_STATUS_ACTIVE: &str = "active";
/// Disabled managed key status.
pub const KEY_STATUS_DISABLED: &str = "disabled";
/// Codex provider string used by current admin key records.
pub const PROVIDER_CODEX: &str = "codex";
/// Kiro provider string used by current admin key records.
pub const PROVIDER_KIRO: &str = "kiro";
/// OpenAI-compatible protocol family.
pub const PROTOCOL_OPENAI: &str = "openai";
/// Anthropic-compatible protocol family.
pub const PROTOCOL_ANTHROPIC: &str = "anthropic";
