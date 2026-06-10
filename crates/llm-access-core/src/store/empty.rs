//! No-op store implementations used as defaults and in tests: every
//! `Empty*Store` and the `NoopUsageEventSink`, each implementing its
//! corresponding trait with empty/zero results.

use async_trait::async_trait;

use super::{
    codex_account::{
        AdminAccountsSummary, AdminCodexAccount, AdminCodexAccountPatch, AdminCodexAccountsPage,
        AdminCodexImportJobDetail, AdminCodexImportJobItem, AdminCodexImportJobItemResult,
        AdminCodexImportJobSummary, CodexStatusRefreshTarget, NewAdminCodexAccount,
        NewAdminCodexImportJob,
    },
    codex_status::CodexRateLimitStatus,
    config::{
        default_kiro_billable_model_multipliers_json, default_kiro_cache_policy_json,
        AdminRuntimeConfig,
    },
    groups::{AdminAccountGroup, AdminAccountGroupPatch, NewAdminAccountGroup},
    keys::{
        AdminKey, AdminKeyPatch, AdminKeysPage, AdminKeysSummary, AdminPageRequest, NewAdminKey,
    },
    kiro_account::{
        AdminKiroAccount, AdminKiroAccountPatch, AdminKiroAccountsPage, AdminKiroBalanceView,
        AdminKiroCacheView, AdminKiroStatusCacheUpdate, KiroStatusRefreshTarget,
        NewAdminKiroAccount,
    },
    proxy::{
        default_proxy_binding, default_proxy_bindings, AdminProxyBinding, AdminProxyConfig,
        AdminProxyConfigPatch, NewAdminProxyConfig,
    },
    public::{
        AdminAccountContributionRequest, AdminAccountContributionRequestsPage,
        AdminReviewQueueAction, AdminReviewQueueQuery, AdminSponsorRequest,
        AdminSponsorRequestsPage, AdminTokenRequest, AdminTokenRequestsPage,
        NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
        PublicAccessKey, PublicAccountContribution, PublicSponsor, PublicUsageLookupKey,
    },
    routes::{
        codex_auth_access_token_expires_at_ms, AuthenticatedKey, ProviderCodexAuthUpdate,
        ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute,
    },
    traits::{
        AdminAccountGroupStore, AdminCodexAccountStore, AdminConfigStore, AdminKeyStore,
        AdminKiroAccountStore, AdminProxyStore, AdminReviewQueueStore, ProviderRouteStore,
        PublicAccessStore, PublicCommunityStore, PublicStatusStore, PublicSubmissionStore,
        PublicUsageStore, UsageAnalyticsStore, UsageEventSink, UsageRollupBatchSink,
    },
    usage::{
        AdminLegacyKiroProxyMigration, UsageChartPoint, UsageEventPage, UsageEventQuery,
        UsageEventTotals, UsageFilterOptions, UsageMetricsQuery, UsageMetricsSnapshot,
        UsageRollupApplyReport, UsageRollupBatch,
    },
    DEFAULT_AUTH_CACHE_TTL_SECONDS, DEFAULT_CODEX_STATUS_REFRESH_SECONDS, KEY_STATUS_ACTIVE,
};
use crate::usage::UsageEvent;

/// Empty public-access store used by isolated unit tests.
pub struct EmptyPublicAccessStore;

/// Empty provider route store used by isolated unit tests.
pub struct EmptyProviderRouteStore;

#[async_trait]
impl ProviderRouteStore for EmptyProviderRouteStore {
    async fn resolve_codex_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_codex_account_route(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn resolve_kiro_route(
        &self,
        _key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn save_codex_auth_update(&self, _update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        Ok(())
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        _account_name: &str,
        _error_message: &str,
        _checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

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

/// Empty public usage store used by isolated unit tests.
pub struct EmptyPublicUsageStore;

#[async_trait]
impl PublicUsageStore for EmptyPublicUsageStore {
    async fn get_public_usage_key_by_secret(
        &self,
        _secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        Ok(None)
    }
}

/// Empty analytics store used by isolated unit tests.
pub struct EmptyUsageAnalyticsStore;

#[async_trait]
impl UsageAnalyticsStore for EmptyUsageAnalyticsStore {
    async fn list_usage_events(&self, query: UsageEventQuery) -> anyhow::Result<UsageEventPage> {
        Ok(UsageEventPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            totals: UsageEventTotals::default(),
            events: Vec::new(),
        })
    }

    async fn get_usage_event(&self, _event_id: &str) -> anyhow::Result<Option<UsageEvent>> {
        Ok(None)
    }

    async fn usage_chart_points(
        &self,
        _key_id: &str,
        start_ms: i64,
        bucket_ms: i64,
        bucket_count: usize,
    ) -> anyhow::Result<Vec<UsageChartPoint>> {
        Ok((0..bucket_count)
            .map(|index| UsageChartPoint {
                bucket_start_ms: start_ms.saturating_add((index as i64).saturating_mul(bucket_ms)),
                tokens: 0,
            })
            .collect())
    }

    async fn list_usage_filter_options(
        &self,
        _query: UsageEventQuery,
    ) -> anyhow::Result<UsageFilterOptions> {
        Ok(UsageFilterOptions::default())
    }

    async fn usage_metrics_snapshot(
        &self,
        query: UsageMetricsQuery,
    ) -> anyhow::Result<UsageMetricsSnapshot> {
        Ok(UsageMetricsSnapshot {
            generated_at_ms: 0,
            start_ms: query.start_ms,
            end_ms: query.end_ms,
            provider_type: query.provider_type,
            source: query.source,
            ..UsageMetricsSnapshot::default()
        })
    }
}

/// No-op usage sink used by isolated unit tests.
pub struct NoopUsageEventSink;

#[async_trait]
impl UsageEventSink for NoopUsageEventSink {
    async fn append_usage_event(&self, _event: &UsageEvent) -> anyhow::Result<()> {
        Ok(())
    }

    async fn append_usage_events(&self, _events: &[UsageEvent]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// No-op rollup sink used by isolated unit tests.
pub struct NoopUsageRollupBatchSink;

#[async_trait]
impl UsageRollupBatchSink for NoopUsageRollupBatchSink {
    async fn apply_usage_rollup_batches(
        &self,
        _batches: &[UsageRollupBatch],
    ) -> anyhow::Result<UsageRollupApplyReport> {
        Ok(UsageRollupApplyReport::default())
    }
}

/// Empty public submission store used by isolated unit tests.
pub struct EmptyPublicSubmissionStore;

#[async_trait]
impl PublicSubmissionStore for EmptyPublicSubmissionStore {
    async fn create_public_token_request(
        &self,
        _request: NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn create_public_account_contribution_request(
        &self,
        _request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn public_account_contribution_name_exists(
        &self,
        _account_name: &str,
    ) -> anyhow::Result<bool> {
        Ok(false)
    }

    async fn create_public_sponsor_request(
        &self,
        _request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn record_public_sponsor_payment_email_result(
        &self,
        _request_id: &str,
        _sent_at_ms: Option<i64>,
        _failure_reason: Option<String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty admin config store used by isolated unit tests.
pub struct EmptyAdminConfigStore;

#[async_trait]
impl AdminConfigStore for EmptyAdminConfigStore {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(AdminRuntimeConfig::default())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        Ok(config)
    }
}

/// Empty admin key store used by isolated unit tests.
pub struct EmptyAdminKeyStore;

#[async_trait]
impl AdminKeyStore for EmptyAdminKeyStore {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        Ok(Vec::new())
    }

    async fn get_admin_key(&self, _key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn list_admin_keys_page(
        &self,
        _provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        Ok(AdminKeysPage {
            keys: Vec::new(),
            summary: AdminKeysSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn find_admin_key_referencing_account_group(
        &self,
        _provider_type: &str,
        _group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        Ok(AdminKey {
            id: key.id,
            name: key.name,
            secret: key.secret,
            key_hash: key.key_hash,
            status: KEY_STATUS_ACTIVE.to_string(),
            provider_type: key.provider_type,
            public_visible: key.public_visible,
            quota_billable_limit: key.quota_billable_limit,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            remaining_billable: key.quota_billable_limit as i64,
            last_used_at: None,
            created_at: key.created_at_ms,
            updated_at: key.created_at_ms,
            route_strategy: None,
            account_group_id: None,
            fixed_account_name: None,
            auto_account_names: None,
            model_name_map: None,
            request_max_concurrency: key.request_max_concurrency,
            request_min_start_interval_ms: key.request_min_start_interval_ms,
            codex_fast_enabled: true,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_remote_media_resolution_enabled: false,
            kiro_latency_routing_enabled: true,
            kiro_protected_content_validation_enabled: false,
            kiro_cctest_text_handling_enabled: false,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
            effective_kiro_cache_policy_json: default_kiro_cache_policy_json(),
            uses_global_kiro_cache_policy: true,
            effective_kiro_billable_model_multipliers_json:
                default_kiro_billable_model_multipliers_json(),
            uses_global_kiro_billable_model_multipliers: true,
            kiro_candidate_credit_summary: None,
        })
    }

    async fn patch_admin_key(
        &self,
        _key_id: &str,
        _patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }

    async fn delete_admin_key(&self, _key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(None)
    }
}

/// Empty admin account-group store used by isolated unit tests.
pub struct EmptyAdminAccountGroupStore;

#[async_trait]
impl AdminAccountGroupStore for EmptyAdminAccountGroupStore {
    async fn list_admin_account_groups(
        &self,
        _provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        Ok(Vec::new())
    }

    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        Ok(AdminAccountGroup {
            id: group.id,
            provider_type: group.provider_type,
            name: group.name,
            account_names: group.account_names,
            created_at: group.created_at_ms,
            updated_at: group.created_at_ms,
        })
    }

    async fn patch_admin_account_group(
        &self,
        _group_id: &str,
        _patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        Ok(None)
    }

    async fn delete_admin_account_group(
        &self,
        _group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        Ok(None)
    }
}

/// Empty admin proxy store used by isolated unit tests.
pub struct EmptyAdminProxyStore;

#[async_trait]
impl AdminProxyStore for EmptyAdminProxyStore {
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        Ok(Vec::new())
    }

    async fn get_admin_proxy_config(
        &self,
        _proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        Ok(AdminProxyConfig {
            id: proxy.id,
            name: proxy.name,
            proxy_url: proxy.proxy_url,
            proxy_username: proxy.proxy_username,
            proxy_password: proxy.proxy_password,
            status: KEY_STATUS_ACTIVE.to_string(),
            created_at: proxy.created_at_ms,
            updated_at: proxy.created_at_ms,
            scope_node_id: None,
            effective_source: "core".to_string(),
            has_node_override: false,
            can_edit_slot_metadata: true,
            latest_codex_check: None,
            latest_kiro_check: None,
        })
    }

    async fn patch_admin_proxy_config(
        &self,
        _proxy_id: &str,
        _patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn delete_admin_proxy_config(
        &self,
        _proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        Ok(None)
    }

    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        Ok(default_proxy_bindings())
    }

    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        _proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        Ok(default_proxy_binding(provider_type))
    }

    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration> {
        Ok(AdminLegacyKiroProxyMigration {
            created_configs: Vec::new(),
            reused_configs: Vec::new(),
            migrated_account_names: Vec::new(),
        })
    }
}

/// Empty admin Codex account store used by isolated unit tests.
pub struct EmptyAdminCodexAccountStore;

/// Empty admin Kiro account store used by isolated unit tests.
pub struct EmptyAdminKiroAccountStore;

#[async_trait]
impl AdminCodexAccountStore for EmptyAdminCodexAccountStore {
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        Ok(Vec::new())
    }

    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        Ok(AdminCodexAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>> {
        Ok(Vec::new())
    }

    async fn get_admin_codex_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn find_admin_codex_account_name_by_principal_id(
        &self,
        _principal_id: &str,
    ) -> anyhow::Result<Option<String>> {
        Ok(None)
    }

    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        Ok(AdminCodexAccount {
            name: account.name,
            status: KEY_STATUS_ACTIVE.to_string(),
            account_id: account.account_id,
            plan_type: None,
            route_weight_tier: account
                .route_weight_tier
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            auto_refresh_enabled: account.auto_refresh_enabled,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "none".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: Some(account.created_at_ms),
            access_token_expires_at: codex_auth_access_token_expires_at_ms(&account.auth_json),
            auth_refresh_error_message: None,
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    async fn patch_admin_codex_account(
        &self,
        _name: &str,
        _patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn delete_admin_codex_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn refresh_admin_codex_account(
        &self,
        _name: &str,
        _refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        Ok(None)
    }

    async fn resolve_admin_codex_account_route(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(None)
    }

    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail> {
        Ok(AdminCodexImportJobDetail {
            summary: AdminCodexImportJobSummary {
                job_id: job.job_id,
                provider_type: job.provider_type,
                source_type: job.source_type,
                validate_before_import: job.validate_before_import,
                status: "pending".to_string(),
                total_count: job.items.len(),
                completed_count: 0,
                succeeded_count: 0,
                skipped_count: 0,
                failed_count: 0,
                batch_error_message: None,
                created_at_ms: job.created_at_ms,
                updated_at_ms: job.created_at_ms,
                finished_at_ms: None,
            },
            items: job
                .items
                .into_iter()
                .enumerate()
                .map(|(item_index, item)| AdminCodexImportJobItem {
                    item_index,
                    requested_name: item.requested_name,
                    requested_account_id: item.requested_account_id,
                    status: "pending".to_string(),
                    error_message: None,
                    imported_account_name: None,
                    final_account_id: None,
                    validated_at_ms: None,
                    imported_at_ms: None,
                })
                .collect(),
        })
    }

    async fn list_admin_codex_import_jobs(
        &self,
        _limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>> {
        Ok(Vec::new())
    }

    async fn get_admin_codex_import_job(
        &self,
        _job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>> {
        Ok(None)
    }

    async fn mark_admin_codex_import_job_running(
        &self,
        _job_id: &str,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn mark_admin_codex_import_job_item_running(
        &self,
        _job_id: &str,
        _item_index: usize,
        _updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn complete_admin_codex_import_job_item(
        &self,
        _job_id: &str,
        _result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        Ok(None)
    }

    async fn fail_admin_codex_import_job(
        &self,
        _job_id: &str,
        _error_message: &str,
        _finished_at_ms: i64,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[async_trait]
impl AdminKiroAccountStore for EmptyAdminKiroAccountStore {
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>> {
        Ok(Vec::new())
    }

    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        Ok(AdminKiroAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        _prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        Ok(AdminKiroAccountsPage {
            accounts: Vec::new(),
            summary: AdminAccountsSummary::default(),
            total: 0,
            limit: page.limit,
            offset: page.offset,
            has_more: false,
        })
    }

    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>> {
        Ok(Vec::new())
    }

    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount> {
        Ok(AdminKiroAccount {
            name: account.name,
            auth_method: account.auth_method,
            provider: None,
            upstream_user_id: account.user_id,
            email: None,
            expires_at: None,
            profile_arn: account.profile_arn,
            has_refresh_token: false,
            disabled: account.status != KEY_STATUS_ACTIVE,
            disabled_reason: None,
            source: None,
            source_db_path: None,
            last_imported_at: None,
            subscription_title: None,
            region: None,
            auth_region: None,
            api_region: None,
            machine_id: None,
            kiro_channel_max_concurrency: account.max_concurrency.unwrap_or(1),
            kiro_channel_min_start_interval_ms: account.min_start_interval_ms.unwrap_or(0),
            minimum_remaining_credits_before_block: 0.0,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: account.proxy_config_id,
            effective_proxy_source: "none".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            proxy_url: None,
            balance: None,
            cache: AdminKiroCacheView::default(),
        })
    }

    async fn patch_admin_kiro_account(
        &self,
        _name: &str,
        _patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        Ok(None)
    }

    async fn delete_admin_kiro_account(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        Ok(None)
    }

    async fn get_admin_kiro_balance(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>> {
        Ok(None)
    }

    async fn resolve_admin_kiro_account_route(
        &self,
        _name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(None)
    }

    async fn save_admin_kiro_status_cache(
        &self,
        _update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Empty admin review queue store used by isolated unit tests.
pub struct EmptyAdminReviewQueueStore;

#[async_trait]
impl AdminReviewQueueStore for EmptyAdminReviewQueueStore {
    async fn get_admin_token_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage> {
        Ok(AdminTokenRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn get_admin_account_contribution_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage> {
        Ok(AdminAccountContributionRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn get_admin_sponsor_request(
        &self,
        _request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        Ok(None)
    }

    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage> {
        Ok(AdminSponsorRequestsPage {
            total: 0,
            offset: query.offset,
            limit: query.limit,
            has_more: false,
            requests: Vec::new(),
        })
    }

    async fn issue_admin_token_request(
        &self,
        _request_id: &str,
        _key: Option<NewAdminKey>,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn reject_admin_token_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        Ok(None)
    }

    async fn issue_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _account: Option<NewAdminCodexAccount>,
        _account_group: Option<NewAdminAccountGroup>,
        _key: Option<NewAdminKey>,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn validate_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _account_id: Option<String>,
        _id_token: String,
        _access_token: String,
        _refresh_token: String,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn fail_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _failure_reason: String,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn reject_admin_account_contribution_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        Ok(None)
    }

    async fn approve_admin_sponsor_request(
        &self,
        _request_id: &str,
        _action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        Ok(None)
    }

    async fn delete_admin_sponsor_request(&self, _request_id: &str) -> anyhow::Result<bool> {
        Ok(false)
    }
}

/// Empty status store used by isolated unit tests.
pub struct EmptyPublicStatusStore;

#[async_trait]
impl PublicStatusStore for EmptyPublicStatusStore {
    async fn codex_rate_limit_status(&self) -> anyhow::Result<CodexRateLimitStatus> {
        Ok(CodexRateLimitStatus::loading(DEFAULT_CODEX_STATUS_REFRESH_SECONDS))
    }
}
