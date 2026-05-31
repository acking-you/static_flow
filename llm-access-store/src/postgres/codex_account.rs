//! Codex admin-account reads/pagination/views/upsert + the
//! `AdminCodexAccountStore` impl.

use std::collections::BTreeMap;

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::{
    provider::RouteStrategy,
    store::{
        self as core_store, AdminCodexAccount, AdminCodexAccountPageQuery, AdminCodexAccountPatch,
        AdminCodexAccountSortMode, AdminCodexAccountStore, AdminCodexAccountsPage,
        AdminCodexImportJobDetail, AdminCodexImportJobItemResult, AdminCodexImportJobSummary,
        AdminPageRequest, CodexRateLimitStatus, CodexStatusRefreshTarget, NewAdminCodexAccount,
        NewAdminCodexImportJob, ProviderCodexRoute,
    },
};

use super::{
    codex_routing::codex_cached_error_message,
    decode::{
        decode_codex_account_row, decode_codex_account_settings,
        decode_codex_admin_account_list_row, decode_codex_import_job_summary_row,
    },
    json::non_negative_i64_to_u64,
    proxy_support::resolve_provider_proxy_config_from_context,
    CodexAccountSettings, CodexAdminAccountListRow, CodexAdminAccountViewContext,
    PostgresControlRepository,
};
use crate::records::CodexAccountRecord;

impl PostgresControlRepository {
    pub(super) async fn load_codex_rate_limit_status_row(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        self.ensure_connection_alive()?;
        let snapshot_json = self
            .client
            .query_opt(
                "SELECT snapshot_json::text FROM llm_codex_status_cache WHERE id = 'default'",
                &[],
            )
            .await
            .context("load codex rate-limit status snapshot")?
            .map(|row| row.get::<_, String>(0));
        snapshot_json
            .map(|json| {
                serde_json::from_str::<CodexRateLimitStatus>(&json)
                    .context("decode codex rate-limit status snapshot")
            })
            .transpose()
    }

    async fn list_codex_admin_account_rows(&self) -> anyhow::Result<Vec<CodexAdminAccountListRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    account_id,
                    status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ),
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ),
                    NULLIF(BTRIM(settings_json ->> 'route_weight_tier'), ''),
                    COALESCE(NULLIF(BTRIM(settings_json ->> 'proxy_mode'), ''), 'inherit'),
                    NULLIF(BTRIM(settings_json ->> 'proxy_config_id'), ''),
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_max_concurrency') = 'number'
                        THEN (settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    ),
                    NULL::text,
                    NULL::double precision,
                    NULL::double precision,
                    NULL::bigint,
                    NULL::bigint,
                    NULL::text
                 FROM llm_codex_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list postgres codex admin account rows")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_admin_account_list_row)
            .collect())
    }

    async fn admin_codex_accounts_summary(
        &self,
    ) -> anyhow::Result<core_store::AdminAccountsSummary> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'active' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'disabled' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'unavailable' THEN 1 ELSE 0 END), 0)::BIGINT
                 FROM llm_codex_accounts",
                &[],
            )
            .await
            .context("summarize postgres codex accounts")?;
        Ok(core_store::AdminAccountsSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            active_count: row.get::<_, i64>(1).max(0) as usize,
            disabled_count: row.get::<_, i64>(2).max(0) as usize,
            unavailable_count: row.get::<_, i64>(3).max(0) as usize,
        })
    }

    async fn list_codex_admin_account_rows_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<(Vec<CodexAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let total = self
            .client
            .query_one("SELECT COUNT(*) FROM llm_codex_accounts", &[])
            .await
            .context("count postgres codex admin account rows")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    account_id,
                    status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ),
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ),
                    NULLIF(BTRIM(settings_json ->> 'route_weight_tier'), ''),
                    COALESCE(NULLIF(BTRIM(settings_json ->> 'proxy_mode'), ''), 'inherit'),
                    NULLIF(BTRIM(settings_json ->> 'proxy_config_id'), ''),
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_max_concurrency') = 'number'
                        THEN (settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    ),
                    NULL::text,
                    NULL::double precision,
                    NULL::double precision,
                    NULL::bigint,
                    NULL::bigint,
                    NULL::text
                 FROM llm_codex_accounts
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $1 OFFSET $2",
                &[&(page.limit.max(1) as i64), &(page.offset as i64)],
            )
            .await
            .context("list postgres codex admin account rows page")?;
        Ok((
            rows.into_iter()
                .map(decode_codex_admin_account_list_row)
                .collect(),
            total,
        ))
    }

    async fn list_codex_admin_account_rows_filtered_page(
        &self,
        query: &AdminCodexAccountPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<(Vec<CodexAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_ascii_lowercase()));
        let base_cte = "
            WITH status_snapshot AS (
                SELECT snapshot_json
                FROM llm_codex_status_cache
                WHERE id = 'default'
            ),
            status_accounts AS (
                SELECT
                    NULLIF(BTRIM(account ->> 'name'), '') AS account_name,
                    COALESCE(NULLIF(BTRIM(account ->> 'status'), ''), 'unknown') AS status,
                    NULLIF(BTRIM(account ->> 'plan_type'), '') AS plan_type,
                    CASE
                        WHEN jsonb_typeof(account -> 'primary_remaining_percent') = 'number'
                        THEN (account ->> 'primary_remaining_percent')::double precision
                        ELSE NULL
                    END AS primary_remaining_percent,
                    CASE
                        WHEN jsonb_typeof(account -> 'secondary_remaining_percent') = 'number'
                        THEN (account ->> 'secondary_remaining_percent')::double precision
                        ELSE NULL
                    END AS secondary_remaining_percent,
                    CASE
                        WHEN jsonb_typeof(account -> 'last_usage_checked_at') = 'number'
                        THEN (account ->> 'last_usage_checked_at')::bigint
                        ELSE NULL
                    END AS last_usage_checked_at_ms,
                    CASE
                        WHEN jsonb_typeof(account -> 'last_usage_success_at') = 'number'
                        THEN (account ->> 'last_usage_success_at')::bigint
                        ELSE NULL
                    END AS last_usage_success_at_ms,
                    NULLIF(BTRIM(account ->> 'usage_error_message'), '') AS usage_error_message
                FROM status_snapshot snapshot
                CROSS JOIN LATERAL jsonb_array_elements(
                    COALESCE(snapshot.snapshot_json -> 'accounts', '[]'::jsonb)
                ) account
            ),
            account_rows AS (
                SELECT
                    a.account_name,
                    a.account_id,
                    a.status,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(a.settings_json -> 'map_gpt53_codex_to_spark')
                                = 'boolean'
                            THEN (a.settings_json ->> 'map_gpt53_codex_to_spark')::boolean
                        END,
                        false
                    ) AS map_gpt53_codex_to_spark,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(a.settings_json -> 'auth_refresh_enabled')
                                = 'boolean'
                            THEN (a.settings_json ->> 'auth_refresh_enabled')::boolean
                        END,
                        true
                    ) AS auth_refresh_enabled,
                    NULLIF(BTRIM(a.settings_json ->> 'route_weight_tier'), '') AS \
                        route_weight_tier,
                    COALESCE(NULLIF(BTRIM(a.settings_json ->> 'proxy_mode'), ''), 'inherit')
                        AS proxy_mode,
                    NULLIF(BTRIM(a.settings_json ->> 'proxy_config_id'), '') AS proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(a.settings_json -> 'request_max_concurrency') = 'number'
                        THEN (a.settings_json ->> 'request_max_concurrency')::bigint
                        ELSE NULL
                    END AS request_max_concurrency,
                    CASE
                        WHEN jsonb_typeof(a.settings_json -> 'request_min_start_interval_ms')
                            = 'number'
                        THEN (a.settings_json ->> 'request_min_start_interval_ms')::bigint
                        ELSE NULL
                    END AS request_min_start_interval_ms,
                    a.last_refresh_at_ms,
                    a.last_error,
                    COALESCE(
                        a.auth_json #>> '{tokens,access_token}',
                        a.auth_json #>> '{tokens,accessToken}',
                        a.auth_json ->> 'access_token',
                        a.auth_json ->> 'accessToken'
                    ) AS access_token,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.plan_type
                        ELSE NULL
                    END AS plan_type,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.primary_remaining_percent
                        ELSE NULL
                    END AS primary_remaining_percent,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.secondary_remaining_percent
                        ELSE NULL
                    END AS secondary_remaining_percent,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.last_usage_checked_at_ms
                        ELSE NULL
                    END AS last_usage_checked_at_ms,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.last_usage_success_at_ms
                        ELSE NULL
                    END AS last_usage_success_at_ms,
                    CASE
                        WHEN a.status = 'active' AND COALESCE(sa.status, '') = 'active'
                        THEN sa.usage_error_message
                        ELSE NULL
                    END AS usage_error_message,
                    a.created_at_ms
                FROM llm_codex_accounts a
                LEFT JOIN status_accounts sa ON sa.account_name = a.account_name
            )";
        let filter_sql = "
            WHERE ($1::text IS NULL
                   OR lower(account_name) LIKE $1
                   OR lower(status) LIKE $1
                   OR lower(COALESCE(plan_type, '')) LIKE $1
                   OR lower(COALESCE(account_id, '')) LIKE $1
                   OR lower(COALESCE(route_weight_tier, '')) LIKE $1)
              AND ($2::boolean = FALSE OR status <> 'disabled')
              AND ($3::boolean = FALSE
                   OR status = 'disabled'
                   OR last_error IS NOT NULL
                   OR usage_error_message IS NOT NULL)";
        let total_sql = format!(
            "{base_cte}
             SELECT COUNT(*)
             FROM account_rows
             {filter_sql}"
        );
        let total = self
            .client
            .query_one(total_sql.as_str(), &[&search, &query.active_only, &query.unhealthy_only])
            .await
            .context("count postgres filtered codex admin account rows")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let order_by = match query.sort {
            AdminCodexAccountSortMode::Newest => "created_at_ms DESC, account_name DESC",
            AdminCodexAccountSortMode::PrimaryAsc => {
                "COALESCE(primary_remaining_percent, 100.0) ASC, account_name DESC"
            },
            AdminCodexAccountSortMode::PrimaryDesc => {
                "COALESCE(primary_remaining_percent, 100.0) DESC, account_name DESC"
            },
            AdminCodexAccountSortMode::SecondaryAsc => {
                "COALESCE(secondary_remaining_percent, 100.0) ASC, account_name DESC"
            },
            AdminCodexAccountSortMode::SecondaryDesc => {
                "COALESCE(secondary_remaining_percent, 100.0) DESC, account_name DESC"
            },
        };
        let rows_sql = format!(
            "{base_cte}
             SELECT
                account_name,
                account_id,
                status,
                map_gpt53_codex_to_spark,
                auth_refresh_enabled,
                route_weight_tier,
                proxy_mode,
                proxy_config_id,
                request_max_concurrency,
                request_min_start_interval_ms,
                last_refresh_at_ms,
                last_error,
                access_token,
                plan_type,
                primary_remaining_percent,
                secondary_remaining_percent,
                last_usage_checked_at_ms,
                last_usage_success_at_ms,
                usage_error_message
             FROM account_rows
             {filter_sql}
             ORDER BY {order_by}
             LIMIT $4 OFFSET $5"
        );
        let rows = self
            .client
            .query(rows_sql.as_str(), &[
                &search,
                &query.active_only,
                &query.unhealthy_only,
                &(page.limit.max(1) as i64),
                &(page.offset as i64),
            ])
            .await
            .context("list postgres filtered codex admin account rows page")?;
        Ok((
            rows.into_iter()
                .map(decode_codex_admin_account_list_row)
                .collect(),
            total,
        ))
    }

    pub(super) async fn get_codex_account_row(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<CodexAccountRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    account_name, account_id, email, status, auth_json::text, settings_json::text,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_codex_accounts
                 WHERE account_name = $1",
                &[&name],
            )
            .await
            .context("load codex account")?;
        Ok(row.map(decode_codex_account_row))
    }

    pub(super) async fn load_codex_admin_account_view_context(
        &self,
    ) -> anyhow::Result<CodexAdminAccountViewContext> {
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let codex_proxy_binding = self
            .load_admin_proxy_binding_from_configs(core_store::PROVIDER_CODEX, &proxy_configs_by_id)
            .await?;
        Ok(CodexAdminAccountViewContext {
            proxy_configs_by_id,
            codex_proxy_binding,
        })
    }

    pub(super) fn resolve_codex_account_proxy_view_with_context(
        &self,
        settings: &CodexAccountSettings,
        context: &CodexAdminAccountViewContext,
    ) -> (String, Option<String>, Option<String>) {
        match settings.proxy_mode.as_str() {
            "none" => ("none".to_string(), None, None),
            "fixed" => {
                let Some(proxy_id) = settings.proxy_config_id.as_deref() else {
                    return ("invalid".to_string(), None, None);
                };
                match context.proxy_configs_by_id.get(proxy_id) {
                    Some(proxy) if proxy.status == core_store::KEY_STATUS_ACTIVE => (
                        "fixed".to_string(),
                        Some(proxy.proxy_url.clone()),
                        Some(proxy.name.clone()),
                    ),
                    Some(proxy) => ("invalid".to_string(), None, Some(proxy.name.clone())),
                    None => ("invalid".to_string(), None, None),
                }
            },
            _ => (
                context.codex_proxy_binding.effective_source.clone(),
                context.codex_proxy_binding.effective_proxy_url.clone(),
                context
                    .codex_proxy_binding
                    .effective_proxy_config_name
                    .clone(),
            ),
        }
    }

    fn admin_codex_account_from_list_row_with_context(
        &self,
        row: &CodexAdminAccountListRow,
        context: &CodexAdminAccountViewContext,
    ) -> AdminCodexAccount {
        let settings = CodexAccountSettings {
            map_gpt53_codex_to_spark: row.map_gpt53_codex_to_spark,
            auth_refresh_enabled: row.auth_refresh_enabled,
            route_weight_tier: row.route_weight_tier.clone(),
            proxy_mode: row.proxy_mode.clone(),
            proxy_config_id: row.proxy_config_id.clone(),
            request_max_concurrency: row
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: row
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
        };
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, context);
        AdminCodexAccount {
            name: row.account_name.clone(),
            status: row.status.clone(),
            account_id: row.account_id.clone(),
            plan_type: row.plan_type.clone(),
            route_weight_tier: settings
                .route_weight_tier
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: row.primary_remaining_percent,
            secondary_remaining_percent: row.secondary_remaining_percent,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auto_refresh_enabled: settings.auth_refresh_enabled,
            request_max_concurrency: settings.request_max_concurrency,
            request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy_mode: settings.proxy_mode,
            proxy_config_id: settings.proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            last_refresh: row.last_refresh_at_ms,
            access_token_expires_at: core_store::codex_access_token_expires_at_ms(
                row.access_token.as_deref(),
            ),
            auth_refresh_error_message: row.last_error.clone(),
            last_usage_checked_at: row.last_usage_checked_at_ms,
            last_usage_success_at: row.last_usage_success_at_ms,
            usage_error_message: row.usage_error_message.clone(),
        }
    }

    fn admin_codex_account_from_record_with_context(
        &self,
        record: &CodexAccountRecord,
        context: &CodexAdminAccountViewContext,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, context);
        Ok(AdminCodexAccount {
            name: record.account_name.clone(),
            status: record.status.clone(),
            account_id: record.account_id.clone(),
            plan_type: None,
            route_weight_tier: settings
                .route_weight_tier
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            primary_remaining_percent: None,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auto_refresh_enabled: settings.auth_refresh_enabled,
            request_max_concurrency: settings.request_max_concurrency,
            request_min_start_interval_ms: settings.request_min_start_interval_ms,
            proxy_mode: settings.proxy_mode,
            proxy_config_id: settings.proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            last_refresh: record.last_refresh_at_ms,
            access_token_expires_at: core_store::codex_auth_access_token_expires_at_ms(
                &record.auth_json,
            ),
            auth_refresh_error_message: record.last_error.clone(),
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        })
    }

    async fn admin_codex_account_from_record(
        &self,
        record: &CodexAccountRecord,
    ) -> anyhow::Result<AdminCodexAccount> {
        let context = self.load_codex_admin_account_view_context().await?;
        self.admin_codex_account_from_record_with_context(record, &context)
    }

    pub(super) async fn upsert_codex_account(
        &self,
        record: &CodexAccountRecord,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_codex_accounts (
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5::jsonb, $6::jsonb, $7, $8, $9, $10)
                 ON CONFLICT(account_name) DO UPDATE SET
                    account_id = EXCLUDED.account_id,
                    email = EXCLUDED.email,
                    status = EXCLUDED.status,
                    auth_json = EXCLUDED.auth_json,
                    settings_json = EXCLUDED.settings_json,
                    last_refresh_at_ms = EXCLUDED.last_refresh_at_ms,
                    last_error = EXCLUDED.last_error,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.account_name,
                    &record.account_id,
                    &record.email,
                    &record.status,
                    &record.auth_json,
                    &record.settings_json,
                    &record.last_refresh_at_ms,
                    &record.last_error,
                    &record.created_at_ms,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres codex account")?;
        Ok(())
    }
}
#[async_trait]
impl AdminCodexAccountStore for PostgresControlRepository {
    async fn list_admin_codex_accounts(&self) -> anyhow::Result<Vec<AdminCodexAccount>> {
        let rows = self.list_codex_admin_account_rows().await?;
        let context = self.load_codex_admin_account_view_context().await?;
        Ok(rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect())
    }

    async fn list_admin_codex_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self.list_codex_admin_account_rows_page(page).await?;
        let context = self.load_codex_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_codex_accounts_summary().await?;
        Ok(AdminCodexAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_admin_codex_accounts_filtered_page(
        &self,
        query: &AdminCodexAccountPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminCodexAccountsPage> {
        if query == &AdminCodexAccountPageQuery::default() {
            return self.list_admin_codex_accounts_page(page).await;
        }
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self
            .list_codex_admin_account_rows_filtered_page(query, page)
            .await?;
        let context = self.load_codex_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_codex_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_codex_accounts_summary().await?;
        Ok(AdminCodexAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_codex_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<CodexStatusRefreshTarget>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, status
                 FROM llm_codex_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres codex status refresh targets")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexStatusRefreshTarget {
                name: row.get(0),
                status: row.get(1),
            })
            .collect())
    }

    async fn get_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        match self.get_codex_account_row(name).await? {
            Some(record) => self
                .admin_codex_account_from_record(&record)
                .await
                .map(Some),
            None => Ok(None),
        }
    }

    async fn find_admin_codex_account_name_by_account_id(
        &self,
        account_id: &str,
    ) -> anyhow::Result<Option<String>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT account_name
                 FROM llm_codex_accounts
                 WHERE account_id = $1
                 ORDER BY account_name
                 LIMIT 1",
                &[&account_id],
            )
            .await
            .context("load codex account name by account id")?;
        Ok(row.map(|row| row.get(0)))
    }

    async fn create_admin_codex_account(
        &self,
        account: NewAdminCodexAccount,
    ) -> anyhow::Result<AdminCodexAccount> {
        let settings = CodexAccountSettings {
            map_gpt53_codex_to_spark: account.map_gpt53_codex_to_spark,
            auth_refresh_enabled: account.auto_refresh_enabled,
            route_weight_tier: account.route_weight_tier.clone(),
            ..CodexAccountSettings::default()
        };
        let record = CodexAccountRecord {
            account_name: account.name.clone(),
            account_id: account.account_id.clone(),
            email: None,
            status: core_store::KEY_STATUS_ACTIVE.to_string(),
            auth_json: account.auth_json.clone(),
            settings_json: serde_json::to_string(&settings)
                .context("serialize postgres codex settings")?,
            last_refresh_at_ms: Some(account.created_at_ms),
            last_error: None,
            created_at_ms: account.created_at_ms,
            updated_at_ms: account.created_at_ms,
        };
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &account.name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.get_admin_codex_account(&account.name)
            .await?
            .context("created postgres codex account disappeared")
    }

    async fn patch_admin_codex_account(
        &self,
        name: &str,
        patch: AdminCodexAccountPatch,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        if let Some(value) = patch.status.as_ref() {
            record.status = value.clone();
        }
        let mut settings = decode_codex_account_settings(&record.settings_json)?;
        if let Some(value) = patch.map_gpt53_codex_to_spark {
            settings.map_gpt53_codex_to_spark = value;
        }
        if let Some(value) = patch.auto_refresh_enabled {
            settings.auth_refresh_enabled = value;
        }
        if let Some(value) = patch.route_weight_tier.as_ref() {
            settings.route_weight_tier = Some(value.clone());
        }
        if let Some(value) = patch.proxy_mode.as_ref() {
            settings.proxy_mode = value.clone();
        }
        if let Some(value) = patch.proxy_config_id.as_ref() {
            settings.proxy_config_id = value.clone();
        }
        if let Some(value) = patch.request_max_concurrency {
            settings.request_max_concurrency = value;
        }
        if let Some(value) = patch.request_min_start_interval_ms {
            settings.request_min_start_interval_ms = value;
        }
        record.settings_json =
            serde_json::to_string(&settings).context("serialize postgres codex settings")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(Some(self.admin_codex_account_from_record(&record).await?))
    }

    async fn delete_admin_codex_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        let view = self.admin_codex_account_from_record(&record).await?;
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_codex_accounts WHERE account_name = $1", &[&name])
            .await
            .context("delete postgres codex account")?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(Some(view))
    }

    async fn refresh_admin_codex_account(
        &self,
        name: &str,
        refreshed_at_ms: i64,
    ) -> anyhow::Result<Option<AdminCodexAccount>> {
        let Some(mut record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        record.last_refresh_at_ms = Some(refreshed_at_ms);
        record.last_error = None;
        record.updated_at_ms = refreshed_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        Ok(Some(self.admin_codex_account_from_record(&record).await?))
    }

    async fn resolve_admin_codex_account_route(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        let Some(record) = self.get_codex_account_row(name).await? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
            .await?;
        let proxy = resolve_provider_proxy_config_from_context(
            &settings.proxy_mode,
            settings.proxy_config_id.as_deref(),
            &proxy_context,
        )?;
        let status_by_account = self
            .load_codex_rate_limit_status_cached()
            .await?
            .map(|status| {
                status
                    .accounts
                    .into_iter()
                    .map(|account| (account.name.clone(), account))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default();
        let cached_error_message = codex_cached_error_message(
            &record.account_name,
            record.last_error.as_deref(),
            record.last_refresh_at_ms,
            settings.auth_refresh_enabled,
            &record.auth_json,
            &status_by_account,
        );
        Ok(Some(ProviderCodexRoute {
            account_name: record.account_name.clone(),
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
            auth_refresh_enabled: settings.auth_refresh_enabled,
            codex_fast_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: settings.request_max_concurrency,
            account_request_min_start_interval_ms: settings.request_min_start_interval_ms,
            cached_error_message,
            proxy,
        }))
    }

    async fn create_admin_codex_import_job(
        &self,
        job: NewAdminCodexImportJob,
    ) -> anyhow::Result<AdminCodexImportJobDetail> {
        let client = self.connect_fresh_client().await?;
        let tx = client
            .transaction()
            .await
            .context("begin postgres codex import job transaction")?;
        tx.execute(
            "INSERT INTO llm_account_import_jobs (
                job_id, provider_type, source_type, validate_before_import, status,
                total_count, completed_count, succeeded_count, skipped_count, failed_count,
                batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
            ) VALUES (
                $1, $2, $3, $4, 'pending',
                $5, 0, 0, 0, 0,
                NULL, $6, $6, NULL
            )",
            &[
                &job.job_id,
                &job.provider_type,
                &job.source_type,
                &job.validate_before_import,
                &(job.items.len() as i64),
                &job.created_at_ms,
            ],
        )
        .await
        .context("insert postgres codex import job")?;
        for (item_index, item) in job.items.iter().enumerate() {
            tx.execute(
                "INSERT INTO llm_account_import_job_items (
                    job_id, item_index, requested_name, requested_account_id, raw_auth_json,
                    status, error_message, imported_account_name, final_account_id,
                    validated_at_ms, imported_at_ms, created_at_ms, updated_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5::jsonb,
                    'pending', NULL, NULL, NULL,
                    NULL, NULL, $6, $6
                )",
                &[
                    &job.job_id,
                    &(item_index as i64),
                    &item.requested_name,
                    &item.requested_account_id,
                    &item.raw_auth_json,
                    &job.created_at_ms,
                ],
            )
            .await
            .with_context(|| format!("insert postgres codex import job item {item_index}"))?;
        }
        tx.commit()
            .await
            .context("commit postgres codex import job transaction")?;
        self.get_admin_codex_import_job(&job.job_id)
            .await?
            .context("created postgres codex import job disappeared")
    }

    async fn list_admin_codex_import_jobs(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<AdminCodexImportJobSummary>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    job_id, provider_type, source_type, validate_before_import, status,
                    total_count, completed_count, succeeded_count, skipped_count, failed_count,
                    batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
                 FROM llm_account_import_jobs
                 ORDER BY created_at_ms DESC, job_id DESC
                 LIMIT $1",
                &[&(limit as i64)],
            )
            .await
            .context("list postgres codex import jobs")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_import_job_summary_row)
            .collect())
    }

    async fn get_admin_codex_import_job(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobDetail>> {
        let Some(summary) = self.load_codex_import_job_summary_row(job_id).await? else {
            return Ok(None);
        };
        let items = self.load_codex_import_job_items_rows(job_id).await?;
        Ok(Some(AdminCodexImportJobDetail {
            summary,
            items,
        }))
    }

    async fn mark_admin_codex_import_job_running(
        &self,
        job_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_jobs
                 SET status = 'running', updated_at_ms = $2
                 WHERE job_id = $1",
                &[&job_id, &updated_at_ms],
            )
            .await
            .context("mark postgres codex import job running")?;
        if changed == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        Ok(())
    }

    async fn mark_admin_codex_import_job_item_running(
        &self,
        job_id: &str,
        item_index: usize,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_job_items
                 SET status = 'running', updated_at_ms = $3
                 WHERE job_id = $1 AND item_index = $2",
                &[&job_id, &(item_index as i64), &updated_at_ms],
            )
            .await
            .context("mark postgres codex import job item running")?;
        if changed == 0 {
            anyhow::bail!("codex import job item `{job_id}`:{item_index} not found");
        }
        Ok(())
    }

    async fn complete_admin_codex_import_job_item(
        &self,
        job_id: &str,
        result: AdminCodexImportJobItemResult,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        if self
            .load_codex_import_job_summary_row(job_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let client = self.connect_fresh_client().await?;
        let tx = client
            .transaction()
            .await
            .context("begin postgres codex import job item completion transaction")?;
        let item_rows = tx
            .execute(
                "UPDATE llm_account_import_job_items
                 SET
                    raw_auth_json = NULL,
                    status = $3,
                    error_message = $4,
                    imported_account_name = $5,
                    final_account_id = $6,
                    validated_at_ms = $7,
                    imported_at_ms = $8,
                    updated_at_ms = $9
                 WHERE job_id = $1 AND item_index = $2",
                &[
                    &job_id,
                    &(result.item_index as i64),
                    &result.status,
                    &result.error_message,
                    &result.imported_account_name,
                    &result.final_account_id,
                    &result.validated_at_ms,
                    &result.imported_at_ms,
                    &result.updated_at_ms,
                ],
            )
            .await
            .context("update postgres codex import job item terminal state")?;
        if item_rows == 0 {
            anyhow::bail!("codex import job item `{job_id}`:{} not found", result.item_index);
        }
        let job_rows = tx
            .execute(
                "UPDATE llm_account_import_jobs
                 SET
                    completed_count = completed_count + $2,
                    succeeded_count = succeeded_count + $3,
                    skipped_count = skipped_count + $4,
                    failed_count = failed_count + $5,
                    status = CASE
                        WHEN completed_count + $2 >= total_count THEN 'completed'
                        ELSE status
                    END,
                    updated_at_ms = $6,
                    finished_at_ms = CASE
                        WHEN completed_count + $2 >= total_count THEN $6
                        ELSE finished_at_ms
                    END
                 WHERE job_id = $1",
                &[
                    &job_id,
                    &(result.completed_delta as i64),
                    &(result.succeeded_delta as i64),
                    &(result.skipped_delta as i64),
                    &(result.failed_delta as i64),
                    &result.updated_at_ms,
                ],
            )
            .await
            .context("roll up postgres codex import job counters")?;
        if job_rows == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        tx.commit()
            .await
            .context("commit postgres codex import job item completion transaction")?;
        self.load_codex_import_job_summary_row(job_id).await
    }

    async fn fail_admin_codex_import_job(
        &self,
        job_id: &str,
        error_message: &str,
        finished_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute(
                "UPDATE llm_account_import_jobs
                 SET
                    status = 'failed',
                    batch_error_message = $2,
                    updated_at_ms = $3,
                    finished_at_ms = $3
                 WHERE job_id = $1",
                &[&job_id, &error_message, &finished_at_ms],
            )
            .await
            .context("mark postgres codex import job failed")?;
        if changed == 0 {
            anyhow::bail!("codex import job `{job_id}` not found");
        }
        Ok(())
    }
}
