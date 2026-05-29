//! Kiro account storage: account view caches, admin-account list/summary/
//! page-filter, route-candidate and cached-status-parts queries, view-context
//! resolution, record conversions, and upsert, plus the `AdminKiroAccountStore`
//! impl.

use async_trait::async_trait;

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[async_trait]
impl AdminKiroAccountStore for PostgresControlRepository {
    async fn list_admin_kiro_accounts(&self) -> anyhow::Result<Vec<AdminKiroAccount>> {
        let rows = self.list_kiro_admin_account_rows().await?;
        let context = self.load_kiro_admin_account_view_context().await?;
        Ok(rows
            .iter()
            .map(|row| self.admin_kiro_account_from_list_row_with_context(row, &context))
            .collect())
    }

    async fn list_admin_kiro_accounts_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        self.list_admin_kiro_accounts_filtered_page(None, page)
            .await
    }

    async fn list_admin_kiro_accounts_filtered_page(
        &self,
        prefix: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKiroAccountsPage> {
        let page = AdminPageRequest {
            limit: page.limit.max(1),
            offset: page.offset,
        };
        let (rows, total) = self
            .list_kiro_admin_account_rows_page_filtered(page, prefix)
            .await?;
        let context = self.load_kiro_admin_account_view_context().await?;
        let accounts = rows
            .iter()
            .map(|row| self.admin_kiro_account_from_list_row_with_context(row, &context))
            .collect::<Vec<_>>();
        let summary = self.admin_kiro_accounts_summary().await?;
        Ok(AdminKiroAccountsPage {
            has_more: page.has_more(accounts.len(), total),
            accounts,
            summary,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    async fn list_kiro_status_refresh_targets(
        &self,
    ) -> anyhow::Result<Vec<KiroStatusRefreshTarget>> {
        self.ensure_connection_alive()?;
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|config| config.kiro_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS);
        let default_cache = AdminKiroCacheView {
            refresh_interval_seconds,
            ..AdminKiroCacheView::default()
        };
        let mut status_by_account = self.list_kiro_cached_status_parts_rows().await?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END
                 FROM llm_kiro_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres kiro status refresh targets")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name: String = row.get(0);
                let status: String = row.get(1);
                let disabled_json: bool = row.get(2);
                let cache = status_by_account
                    .remove(&name)
                    .map(|(_, cache)| cache)
                    .unwrap_or_else(|| default_cache.clone());
                KiroStatusRefreshTarget {
                    name,
                    disabled: disabled_json || status != core_store::KEY_STATUS_ACTIVE,
                    cache,
                }
            })
            .collect())
    }

    async fn create_admin_kiro_account(
        &self,
        account: NewAdminKiroAccount,
    ) -> anyhow::Result<AdminKiroAccount> {
        let record = KiroAccountRecord {
            account_name: account.name.clone(),
            auth_method: account.auth_method.clone(),
            account_id: account.account_id.clone(),
            profile_arn: account.profile_arn.clone(),
            user_id: account.user_id.clone(),
            status: account.status.clone(),
            auth_json: account.auth_json.clone(),
            max_concurrency: account.max_concurrency.map(|value| value as i64),
            min_start_interval_ms: account.min_start_interval_ms.map(|value| value as i64),
            proxy_config_id: account.proxy_config_id.clone(),
            last_refresh_at_ms: Some(account.created_at_ms),
            last_error: None,
            created_at_ms: account.created_at_ms,
            updated_at_ms: account.created_at_ms,
        };
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &account.name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let Some(record) = self.get_kiro_account_row(&account.name).await? else {
            anyhow::bail!("created postgres kiro account disappeared");
        };
        self.admin_kiro_account_from_record(&record).await
    }

    async fn patch_admin_kiro_account(
        &self,
        name: &str,
        patch: AdminKiroAccountPatch,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(mut record) = self.get_kiro_account_row(name).await? else {
            return Ok(None);
        };
        let mut auth_value = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse postgres kiro auth json for patch")?;
        let object = auth_value
            .as_object_mut()
            .context("kiro auth json must be an object")?;
        if let Some(status) = patch.status.as_ref() {
            record.status = status.clone();
            set_json_optional_bool(
                object,
                "disabled",
                Some(status == core_store::KEY_STATUS_DISABLED),
            );
            object.remove("disabledReason");
            object.remove("disabled_reason");
        }
        if let Some(value) = patch.max_concurrency {
            record.max_concurrency = Some(value as i64);
            set_json_optional_u64(object, "kiroChannelMaxConcurrency", Some(value));
        }
        if let Some(value) = patch.min_start_interval_ms {
            record.min_start_interval_ms = Some(value as i64);
            set_json_optional_u64(object, "kiroChannelMinStartIntervalMs", Some(value));
        }
        if let Some(value) = patch.minimum_remaining_credits_before_block {
            set_json_optional_f64(
                object,
                "minimumRemainingCreditsBeforeBlock",
                Some(value.max(0.0)),
            )?;
        }
        if let Some(proxy_mode) = patch.proxy_mode.as_ref() {
            set_json_optional_string(object, "proxyMode", Some(proxy_mode.clone()));
        }
        if let Some(proxy_config_id) = patch.proxy_config_id.as_ref() {
            record.proxy_config_id = proxy_config_id.clone();
            set_json_optional_string(object, "proxyConfigId", proxy_config_id.clone());
        }
        record.auth_json =
            serde_json::to_string(&auth_value).context("serialize postgres kiro auth json")?;
        record.updated_at_ms = patch.updated_at_ms;
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(self.admin_kiro_account_from_record(&record).await?))
    }

    async fn delete_admin_kiro_account(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroAccount>> {
        let Some(record) = self.get_kiro_account_row(name).await? else {
            return Ok(None);
        };
        let view = self.admin_kiro_account_from_record(&record).await?;
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_kiro_accounts WHERE account_name = $1", &[&name])
            .await
            .context("delete postgres kiro account")?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(view))
    }

    async fn get_admin_kiro_balance(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminKiroBalanceView>> {
        let Some((balance, _cache)) = self.get_kiro_cached_status_parts_row(name).await? else {
            return Ok(None);
        };
        Ok(balance)
    }

    async fn resolve_admin_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        let Some(record) = self.get_kiro_account_row(account_name).await? else {
            return Ok(None);
        };
        if record.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let auth_json = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro account auth json")?;
        let profile_arn = record
            .profile_arn
            .clone()
            .or_else(|| optional_json_string(&auth_json, "profileArn"))
            .or_else(|| optional_json_string(&auth_json, "profile_arn"));
        let api_region = optional_json_string(&auth_json, "apiRegion")
            .or_else(|| optional_json_string(&auth_json, "api_region"))
            .or_else(|| optional_json_string(&auth_json, "region"))
            .unwrap_or_else(|| "us-east-1".to_string());
        let minimum_remaining_credits_before_block = optional_json_f64_any(&auth_json, &[
            "minimumRemainingCreditsBeforeBlock",
            "minimum_remaining_credits_before_block",
        ])
        .filter(|value| value.is_finite())
        .unwrap_or(0.0)
        .max(0.0);
        let cached_status = self
            .get_kiro_cached_status_parts_row(&record.account_name)
            .await?;
        let cached_balance = cached_status
            .as_ref()
            .and_then(|(balance, _)| balance.as_ref());
        let cached_balance_view = cached_balance.cloned();
        let cached_cache_view = cached_status.as_ref().map(|(_, cache)| cache.clone());
        let cached_status_label = cached_status
            .as_ref()
            .map(|(_, cache)| cache.status.clone());
        let cached_remaining_credits = cached_balance.map(|balance| balance.remaining);
        let routing_identity = cached_balance
            .and_then(|balance| balance.user_id.clone())
            .or_else(|| record.user_id.clone())
            .unwrap_or_else(|| record.account_name.clone());
        let proxy_mode = optional_json_string_any(&auth_json, &["proxyMode", "proxy_mode"])
            .unwrap_or_else(|| {
                if record.proxy_config_id.is_some() {
                    "fixed".to_string()
                } else {
                    "inherit".to_string()
                }
            });
        let proxy_config_id = record.proxy_config_id.clone().or_else(|| {
            optional_json_string_any(&auth_json, &["proxyConfigId", "proxy_config_id"])
        });
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
            .await?;
        let proxy = resolve_provider_proxy_config_from_context(
            &proxy_mode,
            proxy_config_id.as_deref(),
            &proxy_context,
        )?;
        Ok(Some(ProviderKiroRoute {
            account_name: record.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: record.auth_json,
            profile_arn,
            api_region,
            request_validation_enabled: true,
            cache_estimation_enabled: true,
            zero_cache_debug_enabled: false,
            full_request_logging_enabled: false,
            remote_media_resolution_enabled: false,
            latency_routing_enabled: true,
            model_name_map_json: "{}".to_string(),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json,
            cache_policy_json: runtime_config.kiro_cache_policy_json,
            context_usage_min_request_tokens: runtime_config
                .kiro_context_usage_min_request_tokens
                .max(0) as u64,
            prefix_cache_mode: runtime_config.kiro_prefix_cache_mode,
            prefix_cache_max_tokens: runtime_config.kiro_prefix_cache_max_tokens.max(0) as u64,
            prefix_cache_entry_ttl_seconds: runtime_config
                .kiro_prefix_cache_entry_ttl_seconds
                .max(0) as u64,
            conversation_anchor_max_entries: runtime_config
                .kiro_conversation_anchor_max_entries
                .max(0) as u64,
            conversation_anchor_ttl_seconds: runtime_config
                .kiro_conversation_anchor_ttl_seconds
                .max(0) as u64,
            billable_model_multipliers_json: runtime_config.kiro_billable_model_multipliers_json,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: record
                .max_concurrency
                .and_then(non_negative_i64_to_u64),
            account_request_min_start_interval_ms: record
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            proxy,
            routing_identity,
            cached_status: cached_status_label,
            cached_remaining_credits,
            cached_balance: cached_balance_view,
            cached_cache: cached_cache_view,
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
            minimum_remaining_credits_before_block,
        }))
    }

    async fn save_admin_kiro_status_cache(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_kiro_status_cache (
                    account_name, status, balance_json, cache_json, refreshed_at_ms,
                    expires_at_ms, last_error
                ) VALUES ($1, $2, $3::jsonb, $4::jsonb, $5, $6, $7)
                ON CONFLICT(account_name) DO UPDATE SET
                    status = EXCLUDED.status,
                    balance_json = EXCLUDED.balance_json,
                    cache_json = EXCLUDED.cache_json,
                    refreshed_at_ms = EXCLUDED.refreshed_at_ms,
                    expires_at_ms = EXCLUDED.expires_at_ms,
                    last_error = EXCLUDED.last_error",
                &[
                    &update.account_name,
                    &update.cache.status,
                    &serde_json::to_string(&update.balance)
                        .context("encode postgres kiro balance cache")?,
                    &serde_json::to_string(&update.cache)
                        .context("encode postgres kiro cache view")?,
                    &update.refreshed_at_ms,
                    &update.expires_at_ms,
                    &update.last_error,
                ],
            )
            .await
            .context("upsert postgres kiro status cache")?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &update.account_name)
            .await;
        Ok(())
    }
}
impl PostgresControlRepository {
    pub(crate) async fn load_kiro_account_views_cached(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, crate::request_cache::CachedKiroAccountView>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let Some(cache) = self.request_cache.as_ref() else {
            let proxy_context = self
                .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
                .await?;
            let status_by_account = self
                .list_kiro_cached_status_parts_rows_by_names(account_names)
                .await?;
            return self
                .list_kiro_route_candidate_rows_by_names(account_names)
                .await?
                .into_iter()
                .map(|row| {
                    build_cached_kiro_account_view(
                        &row,
                        status_by_account.get(&row.account_name).cloned(),
                        &proxy_context,
                        0,
                    )
                })
                .map(|result| result.map(|view| (view.account_name.clone(), view)))
                .collect();
        };

        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(core_store::PROVIDER_KIRO, name, scope))
            .collect::<Vec<_>>();
        let cached_values = match cache
            .mget_json::<crate::request_cache::CachedKiroAccountView>(&cache_keys)
            .await
        {
            Ok(values) => values,
            Err(err) => {
                tracing::warn!(error = %err, "request cache kiro account view batch read failed");
                vec![None; account_names.len()]
            },
        };
        let mut views_by_name = BTreeMap::new();
        let mut missing = Vec::new();
        for (account_name, cached) in account_names.iter().cloned().zip(cached_values.into_iter()) {
            if let Some(view) = cached.filter(|view| view.generation == generation) {
                views_by_name.insert(account_name, view);
            } else {
                missing.push(account_name);
            }
        }
        if missing.is_empty() {
            return Ok(views_by_name);
        }
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
            .await?;
        let status_by_account = self
            .list_kiro_cached_status_parts_rows_by_names(&missing)
            .await?;
        for row in self
            .list_kiro_route_candidate_rows_by_names(&missing)
            .await?
        {
            let view = build_cached_kiro_account_view(
                &row,
                status_by_account.get(&row.account_name).cloned(),
                &proxy_context,
                generation,
            )?;
            let cache_key =
                cache.account_view_key(core_store::PROVIDER_KIRO, &view.account_name, scope);
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    &view,
                    cache.account_view_ttl(core_store::PROVIDER_KIRO, &view.account_name, scope),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro account view write failed"
                );
            }
            views_by_name.insert(view.account_name.clone(), view);
        }
        Ok(views_by_name)
    }
    pub(crate) async fn load_kiro_account_auth_cached(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedAccountAuth>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return Ok(self
                .get_kiro_account_row(account_name)
                .await?
                .map(|record| crate::request_cache::CachedAccountAuth {
                    auth_json: record.auth_json,
                }));
        };
        let cache_key = cache.account_auth_key(core_store::PROVIDER_KIRO, account_name);
        match cache
            .get_json::<crate::request_cache::CachedAccountAuth>(&cache_key)
            .await
        {
            Ok(Some(value)) => return Ok(Some(value)),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro auth read failed; falling back to postgres"
                );
            },
        }
        let auth = self
            .get_kiro_account_row(account_name)
            .await?
            .map(|record| crate::request_cache::CachedAccountAuth {
                auth_json: record.auth_json,
            });
        if let Some(auth_ref) = auth.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    auth_ref,
                    cache.account_auth_ttl(core_store::PROVIDER_KIRO, account_name),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro auth write failed"
                );
            }
        }
        Ok(auth)
    }
    pub(crate) async fn list_kiro_accounts_rows(&self) -> anyhow::Result<Vec<KiroAccountRecord>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id, status,
                    auth_json::text, max_concurrency, min_start_interval_ms, proxy_config_id,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list kiro accounts")?;
        Ok(rows.into_iter().map(decode_kiro_account_row).collect())
    }
    pub(crate) async fn list_kiro_admin_account_rows(
        &self,
    ) -> anyhow::Result<Vec<KiroAdminAccountListRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC",
                &[],
            )
            .await
            .context("list postgres kiro admin account rows")?;
        Ok(rows
            .into_iter()
            .map(decode_kiro_admin_account_list_row)
            .collect())
    }
    pub(crate) async fn admin_kiro_accounts_summary(
        &self,
    ) -> anyhow::Result<core_store::AdminAccountsSummary> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE
                        WHEN status = 'active'
                            AND NOT CASE
                                WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                                THEN (auth_json ->> 'disabled')::boolean
                                ELSE false
                            END
                        THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE
                        WHEN status <> 'active'
                            OR CASE
                                WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                                THEN (auth_json ->> 'disabled')::boolean
                                ELSE false
                            END
                        THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN status = 'unavailable' THEN 1 ELSE 0 END), 0)::BIGINT
                 FROM llm_kiro_accounts",
                &[],
            )
            .await
            .context("summarize postgres kiro accounts")?;
        Ok(core_store::AdminAccountsSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            active_count: row.get::<_, i64>(1).max(0) as usize,
            disabled_count: row.get::<_, i64>(2).max(0) as usize,
            unavailable_count: row.get::<_, i64>(3).max(0) as usize,
        })
    }
    pub(crate) async fn list_kiro_admin_account_rows_page_filtered(
        &self,
        page: AdminPageRequest,
        prefix: Option<&str>,
    ) -> anyhow::Result<(Vec<KiroAdminAccountListRow>, usize)> {
        self.ensure_connection_alive()?;
        let normalized_prefix = prefix
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase);
        let total = if let Some(prefix) = normalized_prefix.as_deref() {
            self.client
                .query_one(
                    "SELECT COUNT(*)
                     FROM llm_kiro_accounts
                     WHERE lower(account_name) LIKE $1 || '%'",
                    &[&prefix],
                )
                .await
                .context("count filtered postgres kiro admin account rows")?
                .get::<_, i64>(0)
                .max(0) as usize
        } else {
            self.client
                .query_one("SELECT COUNT(*) FROM llm_kiro_accounts", &[])
                .await
                .context("count postgres kiro admin account rows")?
                .get::<_, i64>(0)
                .max(0) as usize
        };
        let sql = if normalized_prefix.is_some() {
            "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 WHERE lower(account_name) LIKE $1 || '%'
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $2 OFFSET $3"
        } else {
            "SELECT
                    account_name,
                    auth_method,
                    profile_arn,
                    user_id,
                    status,
                    NULLIF(BTRIM(auth_json ->> 'provider'), ''),
                    NULLIF(BTRIM(auth_json ->> 'email'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'expiresAt',
                                auth_json ->> 'expires_at'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'profileArn',
                                auth_json ->> 'profile_arn'
                            )
                        ),
                        ''
                    ),
                    COALESCE(
                        NULLIF(BTRIM(auth_json ->> 'refreshToken'), ''),
                        NULLIF(BTRIM(auth_json ->> 'refresh_token'), '')
                    ) IS NOT NULL,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE false
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'disabledReason',
                                auth_json ->> 'disabled_reason'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'source'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'sourceDbPath',
                                auth_json ->> 'source_db_path'
                            )
                        ),
                        ''
                    ),
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'lastImportedAt') = 'number'
                        THEN (auth_json ->> 'lastImportedAt')::bigint
                        WHEN jsonb_typeof(auth_json -> 'last_imported_at') = 'number'
                        THEN (auth_json ->> 'last_imported_at')::bigint
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'subscriptionTitle',
                                auth_json ->> 'subscription_title'
                            )
                        ),
                        ''
                    ),
                    NULLIF(BTRIM(auth_json ->> 'region'), ''),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'authRegion',
                                auth_json ->> 'auth_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'apiRegion',
                                auth_json ->> 'api_region'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'machineId',
                                auth_json ->> 'machine_id'
                            )
                        ),
                        ''
                    ),
                    max_concurrency,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMaxConcurrency') = 'number'
                        THEN (auth_json ->> 'kiroChannelMaxConcurrency')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_max_concurrency')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_max_concurrency')::bigint
                        ELSE NULL
                    END,
                    min_start_interval_ms,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'kiroChannelMinStartIntervalMs')
                            = 'number'
                        THEN (auth_json ->> 'kiroChannelMinStartIntervalMs')::bigint
                        WHEN jsonb_typeof(auth_json -> 'kiro_channel_min_start_interval_ms')
                            = 'number'
                        THEN (auth_json ->> 'kiro_channel_min_start_interval_ms')::bigint
                        ELSE NULL
                    END,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock')
                            = 'number'
                        THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double precision
                        WHEN jsonb_typeof(
                            auth_json -> 'minimum_remaining_credits_before_block'
                        ) = 'number'
                        THEN (
                            auth_json ->> 'minimum_remaining_credits_before_block'
                        )::double precision
                        ELSE NULL
                    END,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyMode',
                                auth_json ->> 'proxy_mode'
                            )
                        ),
                        ''
                    ),
                    proxy_config_id,
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyConfigId',
                                auth_json ->> 'proxy_config_id'
                            )
                        ),
                        ''
                    ),
                    NULLIF(
                        BTRIM(
                            COALESCE(
                                auth_json ->> 'proxyUrl',
                                auth_json ->> 'proxy_url'
                            )
                        ),
                        ''
                    ),
                    last_error
                 FROM llm_kiro_accounts
                 ORDER BY created_at_ms DESC, account_name DESC
                 LIMIT $1 OFFSET $2"
        };
        let rows = if let Some(prefix) = normalized_prefix.as_deref() {
            self.client
                .query(sql, &[&prefix, &(page.limit.max(1) as i64), &(page.offset as i64)])
                .await
                .context("list filtered postgres kiro admin account rows page")?
        } else {
            self.client
                .query(sql, &[&(page.limit.max(1) as i64), &(page.offset as i64)])
                .await
                .context("list postgres kiro admin account rows page")?
        };
        Ok((
            rows.into_iter()
                .map(decode_kiro_admin_account_list_row)
                .collect(),
            total,
        ))
    }
    pub(crate) async fn get_kiro_account_row(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<KiroAccountRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    account_name, auth_method, account_id, profile_arn, user_id, status,
                    auth_json::text, max_concurrency, min_start_interval_ms, proxy_config_id,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                 FROM llm_kiro_accounts
                 WHERE account_name = $1",
                &[&account_name],
            )
            .await
            .context("load kiro account")?;
        Ok(row.map(decode_kiro_account_row))
    }
    pub(crate) async fn list_kiro_route_candidate_rows(
        &self,
    ) -> anyhow::Result<Vec<KiroRouteCandidateRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    profile_arn,
                    user_id,
                    status,
                    max_concurrency,
                    min_start_interval_ms,
                    proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE FALSE
                    END,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock') = \
                 'number'
                            THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double \
                 precision
                        END,
                        CASE
                            WHEN jsonb_typeof(auth_json -> \
                 'minimum_remaining_credits_before_block') = 'number'
                            THEN (auth_json ->> 'minimum_remaining_credits_before_block')::double \
                 precision
                        END,
                        0.0
                    ),
                    NULLIF(COALESCE(auth_json ->> 'profileArn', auth_json ->> 'profile_arn'), ''),
                    NULLIF(COALESCE(auth_json ->> 'apiRegion', auth_json ->> 'api_region', \
                 auth_json ->> 'region'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyMode', auth_json ->> 'proxy_mode'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyConfigId', auth_json ->> \
                 'proxy_config_id'), '')
                 FROM llm_kiro_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres kiro route candidates")?;
        Ok(rows
            .into_iter()
            .map(|row| KiroRouteCandidateRow {
                account_name: row.get(0),
                profile_arn: row.get(1),
                user_id: row.get(2),
                status: row.get(3),
                max_concurrency: row.get(4),
                min_start_interval_ms: row.get(5),
                proxy_config_id: row.get(6),
                disabled: row.get(7),
                minimum_remaining_credits_before_block: row.get::<_, f64>(8).max(0.0),
                auth_profile_arn: row.get(9),
                api_region: row.get(10),
                proxy_mode: row.get(11),
                auth_proxy_config_id: row.get(12),
            })
            .collect())
    }
    pub(crate) async fn list_kiro_route_candidate_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<Vec<KiroRouteCandidateRow>> {
        if account_names.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    profile_arn,
                    user_id,
                    status,
                    max_concurrency,
                    min_start_interval_ms,
                    proxy_config_id,
                    CASE
                        WHEN jsonb_typeof(auth_json -> 'disabled') = 'boolean'
                        THEN (auth_json ->> 'disabled')::boolean
                        ELSE FALSE
                    END,
                    COALESCE(
                        CASE
                            WHEN jsonb_typeof(auth_json -> 'minimumRemainingCreditsBeforeBlock') = \
                 'number'
                            THEN (auth_json ->> 'minimumRemainingCreditsBeforeBlock')::double \
                 precision
                        END,
                        CASE
                            WHEN jsonb_typeof(auth_json -> \
                 'minimum_remaining_credits_before_block') = 'number'
                            THEN (auth_json ->> 'minimum_remaining_credits_before_block')::double \
                 precision
                        END,
                        0.0
                    ),
                    NULLIF(COALESCE(auth_json ->> 'profileArn', auth_json ->> 'profile_arn'), ''),
                    NULLIF(COALESCE(auth_json ->> 'apiRegion', auth_json ->> 'api_region', \
                 auth_json ->> 'region'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyMode', auth_json ->> 'proxy_mode'), ''),
                    NULLIF(COALESCE(auth_json ->> 'proxyConfigId', auth_json ->> \
                 'proxy_config_id'), '')
                 FROM llm_kiro_accounts
                 WHERE account_name = ANY($1)
                 ORDER BY account_name",
                &[&account_names.to_vec()],
            )
            .await
            .context("list postgres kiro route candidates by names")?;
        Ok(rows
            .into_iter()
            .map(|row| KiroRouteCandidateRow {
                account_name: row.get(0),
                profile_arn: row.get(1),
                user_id: row.get(2),
                status: row.get(3),
                max_concurrency: row.get(4),
                min_start_interval_ms: row.get(5),
                proxy_config_id: row.get(6),
                disabled: row.get(7),
                minimum_remaining_credits_before_block: row.get::<_, f64>(8).max(0.0),
                auth_profile_arn: row.get(9),
                api_region: row.get(10),
                proxy_mode: row.get(11),
                auth_proxy_config_id: row.get(12),
            })
            .collect())
    }
    pub(crate) async fn list_kiro_cached_status_parts_rows(
        &self,
    ) -> anyhow::Result<BTreeMap<String, KiroCachedStatusParts>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache",
                &[],
            )
            .await
            .context("list kiro cached status")?;
        let mut status_by_account = BTreeMap::new();
        for row in rows {
            let account_name: String = row.get(0);
            let balance_json: String = row.get(1);
            let cache_json: String = row.get(2);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            status_by_account.insert(account_name, (balance, cache));
        }
        Ok(status_by_account)
    }
    pub(crate) async fn list_kiro_cached_status_parts_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, KiroCachedStatusParts>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT account_name, balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache
                 WHERE account_name = ANY($1)",
                &[&account_names.to_vec()],
            )
            .await
            .context("list kiro cached status by names")?;
        let mut status_by_account = BTreeMap::new();
        for row in rows {
            let account_name: String = row.get(0);
            let balance_json: String = row.get(1);
            let cache_json: String = row.get(2);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            status_by_account.insert(account_name, (balance, cache));
        }
        Ok(status_by_account)
    }
    pub(crate) async fn get_kiro_cached_status_parts_row(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<KiroCachedStatusParts>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT balance_json::text, cache_json::text
                 FROM llm_kiro_status_cache
                 WHERE account_name = $1",
                &[&account_name],
            )
            .await
            .context("load kiro cached status")?;
        row.map(|row| {
            let balance_json: String = row.get(0);
            let cache_json: String = row.get(1);
            let balance = serde_json::from_str::<Option<AdminKiroBalanceView>>(&balance_json)
                .context("decode kiro cached balance")?;
            let cache = serde_json::from_str::<AdminKiroCacheView>(&cache_json)
                .context("decode kiro cached cache view")?;
            Ok((balance, cache))
        })
        .transpose()
    }
    pub(crate) async fn load_kiro_admin_account_view_context(
        &self,
    ) -> anyhow::Result<KiroAdminAccountViewContext> {
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .map(|config| config.kiro_status_refresh_max_interval_seconds.max(0) as u64)
            .unwrap_or(core_store::DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS);
        let default_cache = AdminKiroCacheView {
            refresh_interval_seconds,
            ..AdminKiroCacheView::default()
        };
        let status_by_account = self.list_kiro_cached_status_parts_rows().await?;
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let kiro_proxy_binding = self
            .load_admin_proxy_binding_from_configs(core_store::PROVIDER_KIRO, &proxy_configs_by_id)
            .await?;
        Ok(KiroAdminAccountViewContext {
            default_cache,
            status_by_account,
            proxy_configs_by_id,
            kiro_proxy_binding,
        })
    }
    pub(crate) fn resolve_kiro_account_proxy_view_with_context(
        &self,
        proxy_mode: &str,
        proxy_config_id: Option<&str>,
        context: &KiroAdminAccountViewContext,
    ) -> (String, Option<String>, Option<String>) {
        match proxy_mode {
            "none" => ("none".to_string(), None, None),
            "fixed" => {
                let Some(proxy_id) = proxy_config_id else {
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
                context.kiro_proxy_binding.effective_source.clone(),
                context.kiro_proxy_binding.effective_proxy_url.clone(),
                context
                    .kiro_proxy_binding
                    .effective_proxy_config_name
                    .clone(),
            ),
        }
    }
    pub(crate) fn admin_kiro_account_from_list_row_with_context(
        &self,
        row: &KiroAdminAccountListRow,
        context: &KiroAdminAccountViewContext,
    ) -> AdminKiroAccount {
        let (balance, cache) = context
            .status_by_account
            .get(&row.account_name)
            .cloned()
            .unwrap_or_else(|| (None, context.default_cache.clone()));
        let proxy_mode = row.proxy_mode.clone().unwrap_or_else(|| {
            if row
                .proxy_config_id
                .as_deref()
                .or(row.auth_proxy_config_id.as_deref())
                .is_some()
            {
                "fixed".to_string()
            } else {
                "inherit".to_string()
            }
        });
        let proxy_config_id = row
            .proxy_config_id
            .clone()
            .or_else(|| row.auth_proxy_config_id.clone());
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                context,
            );
        let disabled = row.disabled_json || row.status != core_store::KEY_STATUS_ACTIVE;
        let disabled_reason = row
            .disabled_reason
            .clone()
            .or_else(|| row.last_error.clone());
        let balance = if disabled { None } else { balance };
        let subscription_title = balance
            .as_ref()
            .and_then(|value| value.subscription_title.clone())
            .or_else(|| row.subscription_title.clone());
        AdminKiroAccount {
            name: row.account_name.clone(),
            auth_method: row.auth_method.clone(),
            provider: row.provider.clone(),
            upstream_user_id: balance
                .as_ref()
                .and_then(|value| value.user_id.clone())
                .or_else(|| row.user_id.clone()),
            email: row.email.clone(),
            expires_at: row.expires_at.clone(),
            profile_arn: row
                .profile_arn
                .clone()
                .or_else(|| row.auth_profile_arn.clone()),
            has_refresh_token: row.has_refresh_token,
            disabled,
            disabled_reason,
            source: row.source.clone(),
            source_db_path: row.source_db_path.clone(),
            last_imported_at: row.last_imported_at,
            subscription_title,
            region: row.region.clone(),
            auth_region: row.auth_region.clone(),
            api_region: row.api_region.clone(),
            machine_id: row.machine_id.clone(),
            kiro_channel_max_concurrency: row
                .max_concurrency
                .and_then(non_negative_i64_to_u64)
                .or_else(|| row.auth_max_concurrency.and_then(non_negative_i64_to_u64))
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY)
                .max(1),
            kiro_channel_min_start_interval_ms: row
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    row.auth_min_start_interval_ms
                        .and_then(non_negative_i64_to_u64)
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
            minimum_remaining_credits_before_block: row
                .minimum_remaining_credits_before_block
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
                .max(0.0),
            proxy_mode,
            proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            proxy_url: row.proxy_url.clone(),
            balance,
            cache,
        }
    }
    pub(crate) fn admin_kiro_account_from_record_with_context(
        &self,
        record: &KiroAccountRecord,
        context: &KiroAdminAccountViewContext,
    ) -> anyhow::Result<AdminKiroAccount> {
        let auth = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro auth json for admin view")?;
        let (balance, cache) = context
            .status_by_account
            .get(&record.account_name)
            .cloned()
            .unwrap_or_else(|| (None, context.default_cache.clone()));
        let proxy_mode = optional_json_string_any(&auth, &["proxyMode", "proxy_mode"])
            .unwrap_or_else(|| {
                if record.proxy_config_id.is_some() {
                    "fixed".to_string()
                } else {
                    "inherit".to_string()
                }
            });
        let proxy_config_id = record
            .proxy_config_id
            .clone()
            .or_else(|| optional_json_string_any(&auth, &["proxyConfigId", "proxy_config_id"]));
        let (effective_proxy_source, effective_proxy_url, effective_proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                context,
            );
        let disabled_json = optional_json_bool_any(&auth, &["disabled"]).unwrap_or(false);
        let disabled = disabled_json || record.status != core_store::KEY_STATUS_ACTIVE;
        let disabled_reason =
            optional_json_string_any(&auth, &["disabledReason", "disabled_reason"])
                .or_else(|| record.last_error.clone());
        let balance = if disabled { None } else { balance };
        let subscription_title = balance
            .as_ref()
            .and_then(|value| value.subscription_title.clone())
            .or_else(|| {
                optional_json_string_any(&auth, &["subscriptionTitle", "subscription_title"])
            });
        Ok(AdminKiroAccount {
            name: record.account_name.clone(),
            auth_method: record.auth_method.clone(),
            provider: optional_json_string_any(&auth, &["provider"]),
            upstream_user_id: balance
                .as_ref()
                .and_then(|value| value.user_id.clone())
                .or_else(|| record.user_id.clone()),
            email: optional_json_string_any(&auth, &["email"]),
            expires_at: optional_json_string_any(&auth, &["expiresAt", "expires_at"]),
            profile_arn: record
                .profile_arn
                .clone()
                .or_else(|| optional_json_string_any(&auth, &["profileArn", "profile_arn"])),
            has_refresh_token: optional_json_string_any(&auth, &["refreshToken", "refresh_token"])
                .is_some(),
            disabled,
            disabled_reason,
            source: optional_json_string_any(&auth, &["source"]),
            source_db_path: optional_json_string_any(&auth, &["sourceDbPath", "source_db_path"]),
            last_imported_at: optional_json_i64_any(&auth, &["lastImportedAt", "last_imported_at"]),
            subscription_title,
            region: optional_json_string_any(&auth, &["region"]),
            auth_region: optional_json_string_any(&auth, &["authRegion", "auth_region"]),
            api_region: optional_json_string_any(&auth, &["apiRegion", "api_region"]),
            machine_id: optional_json_string_any(&auth, &["machineId", "machine_id"]),
            kiro_channel_max_concurrency: record
                .max_concurrency
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    optional_json_u64_any(&auth, &[
                        "kiroChannelMaxConcurrency",
                        "kiro_channel_max_concurrency",
                    ])
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY)
                .max(1),
            kiro_channel_min_start_interval_ms: record
                .min_start_interval_ms
                .and_then(non_negative_i64_to_u64)
                .or_else(|| {
                    optional_json_u64_any(&auth, &[
                        "kiroChannelMinStartIntervalMs",
                        "kiro_channel_min_start_interval_ms",
                    ])
                })
                .unwrap_or(core_store::DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS),
            minimum_remaining_credits_before_block: optional_json_f64_any(&auth, &[
                "minimumRemainingCreditsBeforeBlock",
                "minimum_remaining_credits_before_block",
            ])
            .filter(|value| value.is_finite())
            .unwrap_or(0.0)
            .max(0.0),
            proxy_mode,
            proxy_config_id,
            effective_proxy_source,
            effective_proxy_url,
            effective_proxy_config_name,
            proxy_url: optional_json_string_any(&auth, &["proxyUrl", "proxy_url"]),
            balance,
            cache,
        })
    }
    pub(crate) async fn admin_kiro_account_from_record(
        &self,
        record: &KiroAccountRecord,
    ) -> anyhow::Result<AdminKiroAccount> {
        let context = self.load_kiro_admin_account_view_context().await?;
        self.admin_kiro_account_from_record_with_context(record, &context)
    }
    pub(crate) async fn upsert_kiro_account(
        &self,
        record: &KiroAccountRecord,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_kiro_accounts (
                    account_name, auth_method, account_id, profile_arn, user_id,
                    status, auth_json, max_concurrency, min_start_interval_ms,
                    proxy_config_id, last_refresh_at_ms, last_error, created_at_ms,
                    updated_at_ms
                 ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7::jsonb, $8, $9, $10, $11, $12, $13, $14
                 )
                 ON CONFLICT(account_name) DO UPDATE SET
                    auth_method = EXCLUDED.auth_method,
                    account_id = EXCLUDED.account_id,
                    profile_arn = EXCLUDED.profile_arn,
                    user_id = EXCLUDED.user_id,
                    status = EXCLUDED.status,
                    auth_json = EXCLUDED.auth_json,
                    max_concurrency = EXCLUDED.max_concurrency,
                    min_start_interval_ms = EXCLUDED.min_start_interval_ms,
                    proxy_config_id = EXCLUDED.proxy_config_id,
                    last_refresh_at_ms = EXCLUDED.last_refresh_at_ms,
                    last_error = EXCLUDED.last_error,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.account_name,
                    &record.auth_method,
                    &record.account_id,
                    &record.profile_arn,
                    &record.user_id,
                    &record.status,
                    &record.auth_json,
                    &record.max_concurrency,
                    &record.min_start_interval_ms,
                    &record.proxy_config_id,
                    &record.last_refresh_at_ms,
                    &record.last_error,
                    &record.created_at_ms,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres kiro account")?;
        Ok(())
    }
}
