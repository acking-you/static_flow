//! Storage trait contracts: every `*Store` trait plus the `UsageEventSink`,
//! defining the read/write surface that backends (Postgres/DuckDB) implement
//! and that provider runtimes consume.

use async_trait::async_trait;

use super::{
    codex_account::{
        apply_admin_codex_account_query, summarize_admin_accounts, AdminCodexAccount,
        AdminCodexAccountPageQuery, AdminCodexAccountPatch, AdminCodexAccountsPage,
        AdminCodexImportJobDetail, AdminCodexImportJobItemResult, AdminCodexImportJobSummary,
        CodexStatusRefreshTarget, NewAdminCodexAccount, NewAdminCodexImportJob,
    },
    codex_status::CodexRateLimitStatus,
    config::AdminRuntimeConfig,
    groups::{
        AdminAccountGroup, AdminAccountGroupOption, AdminAccountGroupPatch, AdminAccountGroupsPage,
        NewAdminAccountGroup,
    },
    keys::{
        apply_admin_key_query, summarize_admin_keys, AdminKey, AdminKeyPageQuery, AdminKeyPatch,
        AdminKeysPage, AdminPageRequest, NewAdminKey,
    },
    kiro_account::{
        AdminKiroAccount, AdminKiroAccountPatch, AdminKiroAccountsPage, AdminKiroBalanceView,
        AdminKiroStatusCacheUpdate, KiroStatusRefreshTarget, NewAdminKiroAccount,
    },
    proxy::{
        AdminProxyBinding, AdminProxyConfig, AdminProxyConfigPatch, AdminProxyEndpointCheckUpdate,
        NewAdminProxyConfig,
    },
    public::{
        AdminAccountContributionRequest, AdminAccountContributionRequestsPage,
        AdminReviewQueueAction, AdminReviewQueueQuery, AdminSponsorRequest,
        AdminSponsorRequestsPage, AdminTokenRequest, AdminTokenRequestsPage,
        NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
        PublicAccessKey, PublicAccountContribution, PublicSponsor, PublicUsageLookupKey,
    },
    routes::{
        AuthenticatedKey, ProviderCodexAuthUpdate, ProviderCodexRoute, ProviderKiroAuthUpdate,
        ProviderKiroRoute,
    },
    usage::{
        AdminLegacyKiroProxyMigration, KiroLatencyRankingQuery, KiroLatencyRankingSnapshot,
        ProxyTrafficQuery, ProxyTrafficSnapshot, UsageChartPoint, UsageEventPage, UsageEventQuery,
        UsageFilterOptions, UsageMetricsQuery, UsageMetricsSnapshot, UsageRollupApplyReport,
        UsageRollupBatch,
    },
};
use crate::usage::UsageEvent;

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

    /// Increment usage counters for one owned usage event.
    async fn apply_usage_rollup_owned(&self, event: UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollup(&event).await
    }
}

/// Provider route/account resolution used by data-plane dispatch.
#[async_trait]
pub trait ProviderRouteStore: Send + Sync {
    /// Resolve the Codex account to use for an authenticated key.
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Resolve all Codex account candidates for one authenticated key.
    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        Ok(self.resolve_codex_route(key).await?.into_iter().collect())
    }

    /// Reload one active Codex account route by account name.
    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Resolve the Kiro account to use for an authenticated key.
    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>>;

    /// Resolve all Kiro account candidates for one authenticated key.
    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        Ok(self.resolve_kiro_route(key).await?.into_iter().collect())
    }

    /// Reload one active Kiro account route by account name.
    async fn resolve_kiro_account_route(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    /// Persist a refreshed Kiro credential snapshot.
    async fn save_kiro_auth_update(&self, update: ProviderKiroAuthUpdate) -> anyhow::Result<()>;

    /// Persist a refreshed Codex credential snapshot.
    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()>;

    /// Enable or disable automatic Codex auth refresh for one account.
    async fn set_codex_account_auto_refresh_enabled(
        &self,
        _account_name: &str,
        _enabled: bool,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Persist a hot-path Kiro account quota-exhausted marker.
    async fn mark_kiro_account_quota_exhausted(
        &self,
        _account_name: &str,
        _error_message: &str,
        _checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// Persist a Kiro account status-cache update produced on the hot path.
    async fn save_kiro_status_cache_update(
        &self,
        _update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Public read-only queries used by unauthenticated public endpoints.
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

/// Public usage lookup queries used by unauthenticated public endpoints.
#[async_trait]
pub trait PublicUsageStore: Send + Sync {
    /// Load one key by its presented plaintext secret for public usage lookup.
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>>;
}

/// Analytics queries over settled usage events.
#[async_trait]
pub trait UsageAnalyticsStore: Send + Sync {
    /// List settled usage events.
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage>;

    /// Load one settled usage event.
    async fn get_usage_event(&self, event_id: &str) -> anyhow::Result<Option<UsageEvent>>;

    /// Return chart buckets for one key.
    async fn usage_chart_points(
        &self,
        key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>>;

    /// Return distinct model/account/endpoint values for filter autocomplete
    /// within the current usage query scope.
    async fn list_usage_filter_options(
        &self,
        query: UsageEventQuery,
    ) -> anyhow::Result<UsageFilterOptions>;

    /// Return one recent operational monitoring snapshot.
    async fn usage_metrics_snapshot(
        &self,
        query: UsageMetricsQuery,
    ) -> anyhow::Result<UsageMetricsSnapshot>;

    /// Return one proxy traffic snapshot.
    async fn proxy_traffic_snapshot(
        &self,
        query: ProxyTrafficQuery,
    ) -> anyhow::Result<ProxyTrafficSnapshot>;

    /// Return the compact Kiro latency snapshot used by API-side routing.
    async fn kiro_latency_ranking_snapshot(
        &self,
        _query: KiroLatencyRankingQuery,
    ) -> anyhow::Result<KiroLatencyRankingSnapshot> {
        Ok(KiroLatencyRankingSnapshot::default())
    }
}

/// Public write queries used by unauthenticated public endpoints.
#[async_trait]
pub trait PublicSubmissionStore: Send + Sync {
    /// Persist one public token request.
    async fn create_public_token_request(
        &self,
        request: NewPublicTokenRequest,
    ) -> anyhow::Result<()>;

    /// Persist one public account contribution request.
    async fn create_public_account_contribution_request(
        &self,
        request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()>;

    /// Return whether the proposed account contribution name conflicts with an
    /// existing account or live contribution request.
    async fn public_account_contribution_name_exists(
        &self,
        account_name: &str,
    ) -> anyhow::Result<bool>;

    /// Persist one public sponsor request.
    async fn create_public_sponsor_request(
        &self,
        request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()>;

    /// Persist the payment-email result for one public sponsor request.
    async fn record_public_sponsor_payment_email_result(
        &self,
        request_id: &str,
        sent_at_ms: Option<i64>,
        failure_reason: Option<String>,
    ) -> anyhow::Result<()>;
}

/// Admin runtime config queries used by the standalone frontend surface.
#[async_trait]
pub trait AdminConfigStore: Send + Sync {
    /// Load the current runtime config, or the built-in defaults if no row has
    /// been imported yet.
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig>;

    /// Persist a full runtime config row and return the stored view.
    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig>;
}

/// Admin key management queries used by the current frontend.
#[async_trait]
pub trait AdminKeyStore: Send + Sync {
    /// List all managed keys.
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>>;

    /// Load one managed key by id.
    async fn get_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>>;

    /// List one page of managed keys, optionally scoped to one provider.
    async fn list_admin_keys_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage>;

    /// List one filtered page of managed keys.
    async fn list_admin_keys_filtered_page(
        &self,
        provider_type: Option<&str>,
        query: &AdminKeyPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        let mut keys = self.list_admin_keys().await?;
        if let Some(provider_type) = provider_type {
            keys.retain(|key| key.provider_type == provider_type);
        }
        let summary = summarize_admin_keys(&keys);
        apply_admin_key_query(&mut keys, query);
        let total = keys.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let keys = keys[start..end].to_vec();
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// Find one key that references an account group.
    async fn find_admin_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>>;

    /// Create one managed key.
    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey>;

    /// Patch one managed key by id.
    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>>;

    /// Delete one managed key by id and return the removed row.
    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>>;
}

/// Admin account-group management queries used by the current frontend.
#[async_trait]
pub trait AdminAccountGroupStore: Send + Sync {
    /// List all account groups for one provider.
    async fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>>;

    /// List one page of account groups for one provider.
    async fn list_admin_account_groups_page(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminAccountGroupsPage> {
        let groups = self.list_admin_account_groups(provider_type).await?;
        let total = groups.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let groups = groups[start..end].to_vec();
        Ok(AdminAccountGroupsPage {
            has_more: page.has_more(groups.len(), total),
            groups,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// List lightweight account-group selector options for one provider.
    async fn list_admin_account_group_options(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroupOption>> {
        Ok(self
            .list_admin_account_groups(provider_type)
            .await?
            .into_iter()
            .map(|group| AdminAccountGroupOption {
                account_count: group.account_names.len(),
                single_account_name: (group.account_names.len() == 1)
                    .then(|| group.account_names[0].clone()),
                id: group.id,
                provider_type: group.provider_type,
                name: group.name,
            })
            .collect())
    }

    /// Create one account group.
    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup>;

    /// Patch one account group by id.
    async fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>>;

    /// Delete one account group by id and return the removed row.
    async fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>>;
}

/// Admin reusable proxy configuration queries used by the current frontend.
#[async_trait]
pub trait AdminProxyStore: Send + Sync {
    /// List all proxy configs.
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>>;

    /// Load one proxy config by id.
    async fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// Create one proxy config.
    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig>;

    /// Patch one proxy config by id.
    async fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// Persist the latest endpoint connectivity check for this node scope.
    async fn record_admin_proxy_endpoint_check(
        &self,
        _update: AdminProxyEndpointCheckUpdate,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        anyhow::bail!("proxy endpoint checks are not supported by this store")
    }

    /// Delete one proxy config by id and return the removed row.
    async fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>>;

    /// Reset the current node-local override for one proxy config.
    async fn reset_admin_proxy_config_override(
        &self,
        _proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        anyhow::bail!("proxy config overrides are not supported by this store")
    }

    /// List effective provider-level proxy bindings.
    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>>;

    /// Update or clear one provider-level proxy binding.
    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding>;

    /// Import legacy embedded Kiro proxy fields into shared proxy configs.
    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration>;
}

/// Admin Codex account management queries used by the current frontend.
#[async_trait]
pub trait AdminCodexAccountStore: Send + Sync {
    /// List all imported Codex accounts.
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>>;

    /// List one page of imported Codex accounts.
    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage>;

    /// List one filtered page of imported Codex accounts.
    async fn list_admin_codex_accounts_filtered_page(
        &self,
        query: &AdminCodexAccountPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        let mut accounts = self.list_admin_codex_accounts().await?;
        let summary = summarize_admin_accounts(&accounts);
        apply_admin_codex_account_query(&mut accounts, query);
        let total = accounts.len();
        let start = page.offset.min(total);
        let end = start.saturating_add(page.limit).min(total);
        let accounts = accounts[start..end].to_vec();
        let page_len = accounts.len();
        Ok(AdminCodexAccountsPage {
            has_more: page.has_more(page_len, total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    /// List the minimal Codex account fields needed by background status
    /// refresh.
    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>>;

    /// Get one imported Codex account by name.
    async fn get_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Resolve one existing Codex account name by upstream principal identity
    /// derived from auth JWT claims.
    async fn find_admin_codex_account_name_by_principal_id(
        &self,
        principal_id: &str,
    ) -> anyhow::Result<Option<String>>;

    /// Import one Codex account.
    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount>;

    /// Patch one Codex account.
    async fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Delete one Codex account.
    async fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Mark one Codex account as refreshed and return its latest summary.
    async fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>>;

    /// Resolve a single Codex account as a provider route for admin refreshes.
    async fn resolve_admin_codex_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>>;

    /// Persist one new Codex batch import job and its queued items.
    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail>;

    /// List recent Codex batch import jobs ordered newest first.
    async fn list_admin_codex_import_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>>;

    /// Load one Codex batch import job with all item states.
    async fn get_admin_codex_import_job(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>>;

    /// Mark one batch job as actively running.
    async fn mark_admin_codex_import_job_running(
        &self,
        job_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()>;

    /// Mark one batch item as actively running.
    async fn mark_admin_codex_import_job_item_running(
        &self,
        job_id: &str,
        item_index: usize,
        updated_at_ms: i64,
    ) -> anyhow::Result<()>;

    /// Complete one batch item and roll up job counters.
    async fn complete_admin_codex_import_job_item(
        &self,
        job_id: &str,
        result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>>;

    /// Mark one batch job as failed before all items could complete.
    async fn fail_admin_codex_import_job(
        &self,
        job_id: &str,
        error_message: &str,
        finished_at_ms: i64,
    ) -> anyhow::Result<()>;
}

/// Admin Kiro account management queries used by the current frontend.
#[async_trait]
pub trait AdminKiroAccountStore: Send + Sync {
    /// List all persisted Kiro accounts with cached status information.
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>>;

    /// List one page of persisted Kiro accounts.
    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage>;

    /// List one page of persisted Kiro accounts, optionally filtered by a
    /// case-insensitive account-name prefix.
    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage>;

    /// List the minimal Kiro account fields needed by background status
    /// refresh.
    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>>;

    /// Create or replace one Kiro account.
    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount>;

    /// Patch one Kiro account.
    async fn patch_admin_kiro_account(
        &self,
        name: &str,
        patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>>;

    /// Delete one Kiro account.
    async fn delete_admin_kiro_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>>;

    /// Return the cached Kiro balance for one account.
    async fn get_admin_kiro_balance(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>>;

    /// Resolve a single Kiro account as a provider route for admin refreshes.
    async fn resolve_admin_kiro_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>>;

    /// Persist one Kiro status-cache update.
    async fn save_admin_kiro_status_cache(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()>;
}

/// Admin review queue queries used by the current frontend.
#[async_trait]
pub trait AdminReviewQueueStore: Send + Sync {
    /// Load one token request.
    async fn get_admin_token_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// List token requests.
    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage>;

    /// Load one account contribution request.
    async fn get_admin_account_contribution_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// List account contribution requests.
    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage>;

    /// Load one sponsor request.
    async fn get_admin_sponsor_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>>;

    /// List sponsor requests.
    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage>;

    /// Issue a token request and create the key when needed.
    async fn issue_admin_token_request(
        &self,
        request_id: &str,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// Reject a token request and disable any partially issued key.
    async fn reject_admin_token_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>>;

    /// Issue an account contribution request and create account, group, and key
    /// records when needed.
    async fn issue_admin_account_contribution_request(
        &self,
        request_id: &str,
        account: Option<NewAdminCodexAccount>,
        account_group: Option<NewAdminAccountGroup>,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Mark an account contribution request as validated after a successful
    /// Codex auth refresh check.
    async fn validate_admin_account_contribution_request(
        &self,
        request_id: &str,
        account_id: Option<String>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Mark an account contribution request as failed after validation rejects
    /// the supplied auth.
    async fn fail_admin_account_contribution_request(
        &self,
        request_id: &str,
        failure_reason: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Reject an account contribution request and disable/remove partial
    /// records.
    async fn reject_admin_account_contribution_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>>;

    /// Approve one sponsor request.
    async fn approve_admin_sponsor_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>>;

    /// Delete one sponsor request from admin review/history.
    async fn delete_admin_sponsor_request(&self, request_id: &str) -> anyhow::Result<bool>;
}

/// Public read-only queries for compatibility status endpoints.
#[async_trait]
pub trait PublicStatusStore: Send + Sync {
    /// Current cached Codex public rate-limit status.
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus>;

    /// Persist a refreshed Codex public rate-limit snapshot.
    async fn save_codex_rate_limit_status(
        &self,
        _snapshot: CodexRateLimitStatus,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist a batch of usage events.
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()>;

    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.append_usage_events(std::slice::from_ref(event)).await
    }

    /// Persist an owned batch of usage events.
    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        self.append_usage_events(&events).await
    }
}

/// Idempotent control-plane usage rollup sink used by the billing/quota path.
#[async_trait]
pub trait UsageRollupBatchSink: Send + Sync {
    /// Apply durable rollup batches exactly once by `batch_id`.
    async fn apply_usage_rollup_batches(
        &self,
        batches: &[UsageRollupBatch],
    ) -> anyhow::Result<UsageRollupApplyReport>;

    /// Prune idempotency markers older than the guaranteed replay horizon.
    async fn prune_usage_rollup_batch_markers(
        &self,
        _applied_before_ms: i64,
    ) -> anyhow::Result<u64> {
        Ok(0)
    }
}
