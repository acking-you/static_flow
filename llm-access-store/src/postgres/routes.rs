//! Provider route-candidate reads, account-name resolution, and the
//! `ProviderRouteStore` impl.

use std::collections::BTreeMap;

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::{
    provider::RouteStrategy,
    store::{
        self as core_store, AdminCodexAccountStore, AdminKiroAccountStore, AdminKiroBalanceView,
        AdminKiroCacheView, AdminKiroStatusCacheUpdate, AuthenticatedKey, ProviderCodexAuthUpdate,
        ProviderCodexRoute, ProviderKiroAuthUpdate, ProviderKiroRoute, ProviderRouteStore,
    },
};

use super::{
    cache_convert::proxy_from_cached_option,
    codex_routing::{
        codex_cached_error_message, minimal_codex_auth_json_for_access_token,
        sort_codex_routes_by_cached_quota,
    },
    decode::decode_codex_account_settings,
    json::decode_optional_json,
    CodexRouteCandidateRow, KiroCachedStatusParts, KiroRouteCandidateRow,
    PostgresControlRepository,
};
use crate::records::{KeyRouteConfig, RuntimeConfigRecord};

impl PostgresControlRepository {
    pub(super) async fn list_codex_route_candidate_rows(
        &self,
    ) -> anyhow::Result<Vec<CodexRouteCandidateRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    settings_json::text,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    )
                 FROM llm_codex_accounts
                 ORDER BY account_name",
                &[],
            )
            .await
            .context("list postgres codex route candidates")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexRouteCandidateRow {
                account_name: row.get(0),
                status: row.get(1),
                settings_json: row.get(2),
                last_refresh_at_ms: row.get(3),
                last_error: row.get(4),
                access_token: row.get(5),
            })
            .collect())
    }

    pub(super) async fn list_codex_route_candidate_rows_by_names(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<Vec<CodexRouteCandidateRow>> {
        if account_names.is_empty() {
            return Ok(Vec::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    account_name,
                    status,
                    settings_json::text,
                    last_refresh_at_ms,
                    last_error,
                    COALESCE(
                        auth_json #>> '{tokens,access_token}',
                        auth_json #>> '{tokens,accessToken}',
                        auth_json ->> 'access_token',
                        auth_json ->> 'accessToken'
                    )
                 FROM llm_codex_accounts
                 WHERE account_name = ANY($1)
                 ORDER BY account_name",
                &[&account_names],
            )
            .await
            .context("list postgres codex route candidates by names")?;
        Ok(rows
            .into_iter()
            .map(|row| CodexRouteCandidateRow {
                account_name: row.get(0),
                status: row.get(1),
                settings_json: row.get(2),
                last_refresh_at_ms: row.get(3),
                last_error: row.get(4),
                access_token: row.get(5),
            })
            .collect())
    }

    pub(super) async fn list_kiro_route_candidate_rows(
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

    pub(super) async fn list_kiro_route_candidate_rows_by_names(
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
                &[&account_names],
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

    pub(super) async fn list_kiro_cached_status_parts_rows(
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

    pub(super) async fn list_kiro_cached_status_parts_rows_by_names(
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
                &[&account_names],
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

    pub(super) async fn get_kiro_cached_status_parts_row(
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

    pub(super) async fn resolve_route_account_names(
        &self,
        provider_type: &str,
        route: &KeyRouteConfig,
        default_active_account_names: Vec<String>,
    ) -> anyhow::Result<Vec<String>> {
        let strategy = route.route_strategy.as_deref().unwrap_or("auto");
        match strategy {
            "fixed" => {
                let account_name = if let Some(group_id) = route.account_group_id.as_deref() {
                    let group = self
                        .get_admin_account_group_row(group_id)
                        .await?
                        .with_context(|| {
                            format!("configured account_group_id `{group_id}` does not exist")
                        })?;
                    if group.provider_type != provider_type {
                        anyhow::bail!(
                            "configured account_group_id belongs to a different provider"
                        );
                    }
                    if group.account_names.len() != 1 {
                        anyhow::bail!(
                            "fixed route_strategy requires an account group with exactly one \
                             account"
                        );
                    }
                    group.account_names[0].clone()
                } else {
                    route
                        .fixed_account_name
                        .clone()
                        .filter(|value| !value.trim().is_empty())
                        .context("fixed route_strategy requires account_group_id")?
                };
                Ok(vec![account_name])
            },
            "auto" => {
                if let Some(group_id) = route.account_group_id.as_deref() {
                    let group = self
                        .get_admin_account_group_row(group_id)
                        .await?
                        .with_context(|| {
                            format!("configured account_group_id `{group_id}` does not exist")
                        })?;
                    if group.provider_type != provider_type {
                        anyhow::bail!(
                            "configured account_group_id belongs to a different provider"
                        );
                    }
                    if !group.account_names.is_empty() {
                        return Ok(group.account_names);
                    }
                }
                if let Some(account_names) =
                    decode_optional_json::<Vec<String>>(route.auto_account_names_json.as_deref())
                {
                    if !account_names.is_empty() {
                        return Ok(account_names);
                    }
                }
                Ok(default_active_account_names)
            },
            other => anyhow::bail!("unsupported route strategy `{other}`"),
        }
    }
}
#[async_trait]
impl ProviderRouteStore for PostgresControlRepository {
    async fn resolve_codex_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        Ok(self
            .resolve_codex_route_candidates(key)
            .await?
            .into_iter()
            .next())
    }

    async fn resolve_codex_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderCodexRoute>> {
        let Some(snapshot) = self.load_codex_request_snapshot_cached(&key.key_id).await? else {
            return Ok(Vec::new());
        };
        if snapshot.key.provider_type != core_store::PROVIDER_CODEX {
            return Ok(Vec::new());
        }
        let route_strategy_at_event = match snapshot.route_strategy.as_str() {
            "fixed" => RouteStrategy::Fixed,
            _ => RouteStrategy::Auto,
        };
        let account_group_id_at_event = snapshot.account_group_id_at_event.clone();
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
        let views_by_name = self
            .load_codex_account_views_cached(&snapshot.selected_account_names)
            .await?;
        let route_weight_tiers = views_by_name
            .iter()
            .map(|(name, view)| (name.clone(), view.route_weight_tier.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut routes = Vec::new();
        for account_name in snapshot.selected_account_names {
            let Some(view) = views_by_name.get(&account_name).cloned() else {
                continue;
            };
            if view.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            let minimal_auth_json =
                minimal_codex_auth_json_for_access_token(view.access_token.as_deref());
            let cached_error_message = codex_cached_error_message(
                &account_name,
                view.last_error.as_deref(),
                view.last_refresh_at_ms,
                view.auth_refresh_enabled,
                &minimal_auth_json,
                &status_by_account,
            );
            routes.push(ProviderCodexRoute {
                account_name: view.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: String::new(),
                map_gpt53_codex_to_spark: view.map_gpt53_codex_to_spark,
                auth_refresh_enabled: view.auth_refresh_enabled,
                codex_fast_enabled: snapshot.codex_fast_enabled,
                request_max_concurrency: snapshot.request_max_concurrency,
                request_min_start_interval_ms: snapshot.request_min_start_interval_ms,
                account_request_max_concurrency: view.request_max_concurrency,
                account_request_min_start_interval_ms: view.request_min_start_interval_ms,
                cached_error_message,
                proxy: proxy_from_cached_option(view.proxy),
            });
        }
        let codex_status = self.load_codex_rate_limit_status_cached().await?;
        let runtime_config = RuntimeConfigRecord {
            codex_weight_free: snapshot.codex_weight_free,
            codex_weight_plus: snapshot.codex_weight_plus,
            codex_weight_pro5x: snapshot.codex_weight_pro5x,
            codex_weight_pro20x: snapshot.codex_weight_pro20x,
            ..RuntimeConfigRecord::default()
        };
        sort_codex_routes_by_cached_quota(
            &mut routes,
            codex_status.as_ref(),
            &runtime_config,
            &route_weight_tiers,
        );
        Ok(routes)
    }

    async fn resolve_codex_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderCodexRoute>> {
        if self.request_cache.is_none() {
            return self.resolve_admin_codex_account_route(account_name).await;
        }
        let Some(view) = self
            .load_codex_account_views_cached(&[account_name.to_string()])
            .await?
            .remove(account_name)
        else {
            return Ok(None);
        };
        if view.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let Some(auth) = self.load_codex_account_auth_cached(account_name).await? else {
            return Ok(None);
        };
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
            account_name,
            view.last_error.as_deref(),
            view.last_refresh_at_ms,
            view.auth_refresh_enabled,
            &auth.auth_json,
            &status_by_account,
        );
        Ok(Some(ProviderCodexRoute {
            account_name: view.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: auth.auth_json,
            map_gpt53_codex_to_spark: view.map_gpt53_codex_to_spark,
            auth_refresh_enabled: view.auth_refresh_enabled,
            codex_fast_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            account_request_max_concurrency: view.request_max_concurrency,
            account_request_min_start_interval_ms: view.request_min_start_interval_ms,
            cached_error_message,
            proxy: proxy_from_cached_option(view.proxy),
        }))
    }

    async fn resolve_kiro_route(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        Ok(self
            .resolve_kiro_route_candidates(key)
            .await?
            .into_iter()
            .next())
    }

    async fn resolve_kiro_route_candidates(
        &self,
        key: &AuthenticatedKey,
    ) -> anyhow::Result<Vec<ProviderKiroRoute>> {
        let Some(snapshot) = self.load_kiro_request_snapshot_cached(&key.key_id).await? else {
            return Ok(Vec::new());
        };
        if snapshot.key.provider_type != core_store::PROVIDER_KIRO {
            return Ok(Vec::new());
        }
        let route_strategy_at_event = match snapshot.route_strategy.as_str() {
            "fixed" => RouteStrategy::Fixed,
            _ => RouteStrategy::Auto,
        };
        let account_group_id_at_event = snapshot.account_group_id_at_event.clone();
        let views_by_name = self
            .load_kiro_account_views_cached(&snapshot.selected_account_names)
            .await?;
        let mut routes = Vec::new();
        for account_name in snapshot.selected_account_names {
            let Some(view) = views_by_name.get(&account_name).cloned() else {
                continue;
            };
            if view.status != core_store::KEY_STATUS_ACTIVE {
                continue;
            }
            if view.disabled {
                continue;
            }
            if let Some(cache_view) = &view.cached_cache {
                if matches!(cache_view.status.as_str(), "disabled" | "quota_exhausted") {
                    continue;
                }
            }
            if view.cached_balance.as_ref().is_some_and(|balance| {
                balance.remaining <= 0.0
                    || balance.remaining <= view.minimum_remaining_credits_before_block
            }) {
                continue;
            }
            routes.push(ProviderKiroRoute {
                account_name: view.account_name,
                account_group_id_at_event: account_group_id_at_event.clone(),
                route_strategy_at_event,
                auth_json: String::new(),
                profile_arn: view.profile_arn,
                api_region: view.api_region,
                request_validation_enabled: snapshot.request_validation_enabled,
                cache_estimation_enabled: snapshot.cache_estimation_enabled,
                zero_cache_debug_enabled: snapshot.zero_cache_debug_enabled,
                full_request_logging_enabled: snapshot.full_request_logging_enabled,
                remote_media_resolution_enabled: snapshot.remote_media_resolution_enabled,
                latency_routing_enabled: snapshot.latency_routing_enabled,
                model_name_map_json: snapshot.model_name_map_json.clone(),
                cache_kmodels_json: snapshot.cache_kmodels_json.clone(),
                cache_policy_json: snapshot.cache_policy_json.clone(),
                context_usage_min_request_tokens: snapshot.context_usage_min_request_tokens,
                prefix_cache_mode: snapshot.prefix_cache_mode.clone(),
                prefix_cache_max_tokens: snapshot.prefix_cache_max_tokens,
                prefix_cache_entry_ttl_seconds: snapshot.prefix_cache_entry_ttl_seconds,
                conversation_anchor_max_entries: snapshot.conversation_anchor_max_entries,
                conversation_anchor_ttl_seconds: snapshot.conversation_anchor_ttl_seconds,
                billable_model_multipliers_json: snapshot.billable_model_multipliers_json.clone(),
                request_max_concurrency: snapshot.request_max_concurrency,
                request_min_start_interval_ms: snapshot.request_min_start_interval_ms,
                account_request_max_concurrency: view.request_max_concurrency,
                account_request_min_start_interval_ms: view.request_min_start_interval_ms,
                proxy: proxy_from_cached_option(view.proxy),
                routing_identity: view.routing_identity,
                cached_status: view.cached_cache.as_ref().map(|cache| cache.status.clone()),
                cached_remaining_credits: view
                    .cached_balance
                    .as_ref()
                    .map(|balance| balance.remaining),
                cached_balance: view.cached_balance,
                cached_cache: view.cached_cache,
                status_refresh_interval_seconds: snapshot.status_refresh_interval_seconds,
                minimum_remaining_credits_before_block: view.minimum_remaining_credits_before_block,
            });
        }
        Ok(routes)
    }

    async fn resolve_kiro_account_route(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<ProviderKiroRoute>> {
        if self.request_cache.is_none() {
            return self.resolve_admin_kiro_account_route(account_name).await;
        }
        let Some(view) = self
            .load_kiro_account_views_cached(&[account_name.to_string()])
            .await?
            .remove(account_name)
        else {
            return Ok(None);
        };
        if view.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(None);
        }
        let Some(auth) = self.load_kiro_account_auth_cached(account_name).await? else {
            return Ok(None);
        };
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        Ok(Some(ProviderKiroRoute {
            account_name: view.account_name,
            account_group_id_at_event: None,
            route_strategy_at_event: RouteStrategy::Auto,
            auth_json: auth.auth_json,
            profile_arn: view.profile_arn,
            api_region: view.api_region,
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
            account_request_max_concurrency: view.request_max_concurrency,
            account_request_min_start_interval_ms: view.request_min_start_interval_ms,
            proxy: proxy_from_cached_option(view.proxy),
            routing_identity: view.routing_identity,
            cached_status: view.cached_cache.as_ref().map(|cache| cache.status.clone()),
            cached_remaining_credits: view
                .cached_balance
                .as_ref()
                .map(|balance| balance.remaining),
            cached_balance: view.cached_balance,
            cached_cache: view.cached_cache,
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
            minimum_remaining_credits_before_block: view.minimum_remaining_credits_before_block,
        }))
    }

    async fn save_kiro_auth_update(&self, update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_kiro_account_row(&update.account_name).await? else {
            anyhow::bail!("kiro account `{}` is not configured", update.account_name);
        };
        record.auth_json = update.auth_json.clone();
        record.auth_method = update.auth_method.clone();
        record.account_id = update.account_id.clone();
        record.profile_arn = update.profile_arn.clone();
        record.user_id = update.user_id.clone();
        record.status = update.status.clone();
        record.last_refresh_at_ms = Some(update.refreshed_at_ms);
        record.last_error = update.last_error.clone();
        record.updated_at_ms = update.refreshed_at_ms;
        self.upsert_kiro_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_KIRO, &record.account_name)
            .await;
        Ok(())
    }

    async fn save_codex_auth_update(&self, update: ProviderCodexAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_codex_account_row(&update.account_name).await? else {
            anyhow::bail!("codex account `{}` is not configured", update.account_name);
        };
        record.auth_json = update.auth_json.clone();
        if update.account_id.is_some() {
            record.account_id = update.account_id.clone();
        }
        record.status = update.status.clone();
        record.last_refresh_at_ms = Some(update.refreshed_at_ms);
        record.last_error = update.last_error.clone();
        record.updated_at_ms = update.refreshed_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        Ok(())
    }

    async fn set_codex_account_auto_refresh_enabled(
        &self,
        account_name: &str,
        enabled: bool,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        let Some(mut record) = self.get_codex_account_row(account_name).await? else {
            anyhow::bail!("codex account `{account_name}` is not configured");
        };
        let mut settings = decode_codex_account_settings(&record.settings_json)?;
        if settings.auth_refresh_enabled == enabled {
            return Ok(());
        }
        settings.auth_refresh_enabled = enabled;
        record.settings_json =
            serde_json::to_string(&settings).context("serialize postgres codex settings")?;
        record.updated_at_ms = updated_at_ms;
        self.upsert_codex_account(&record).await?;
        self.invalidate_account_cache(core_store::PROVIDER_CODEX, &record.account_name)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        Ok(())
    }

    async fn mark_kiro_account_quota_exhausted(
        &self,
        account_name: &str,
        error_message: &str,
        checked_at_ms: i64,
    ) -> anyhow::Result<()> {
        let refresh_interval_seconds = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default()
            .kiro_status_refresh_max_interval_seconds
            .max(0) as u64;
        self.save_admin_kiro_status_cache(AdminKiroStatusCacheUpdate {
            account_name: account_name.to_string(),
            balance: None,
            cache: AdminKiroCacheView {
                status: "quota_exhausted".to_string(),
                refresh_interval_seconds,
                last_checked_at: Some(checked_at_ms),
                last_success_at: Some(checked_at_ms),
                error_message: Some(error_message.to_string()),
            },
            refreshed_at_ms: checked_at_ms,
            expires_at_ms: checked_at_ms
                .saturating_add((refresh_interval_seconds as i64).saturating_mul(1000)),
            last_error: Some(error_message.to_string()),
        })
        .await
    }

    async fn save_kiro_status_cache_update(
        &self,
        update: AdminKiroStatusCacheUpdate,
    ) -> anyhow::Result<()> {
        self.save_admin_kiro_status_cache(update).await
    }
}
