//! Provider routing storage: authenticated-key load/cache, dispatch
//! generation, request-snapshot caches, account-cache invalidation, route
//! account-name resolution, plus the `ProviderRouteStore` impl.

use async_trait::async_trait;

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

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

    async fn save_kiro_auth_update(&self, _update: ProviderKiroAuthUpdate) -> anyhow::Result<()> {
        let Some(mut record) = self.get_kiro_account_row(&_update.account_name).await? else {
            anyhow::bail!("kiro account `{}` is not configured", _update.account_name);
        };
        record.auth_json = _update.auth_json.clone();
        record.auth_method = _update.auth_method.clone();
        record.account_id = _update.account_id.clone();
        record.profile_arn = _update.profile_arn.clone();
        record.user_id = _update.user_id.clone();
        record.status = _update.status.clone();
        record.last_refresh_at_ms = Some(_update.refreshed_at_ms);
        record.last_error = _update.last_error.clone();
        record.updated_at_ms = _update.refreshed_at_ms;
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
impl PostgresControlRepository {
    pub(crate) async fn load_authenticated_key_by_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id,
                    k.name,
                    k.provider_type,
                    k.protocol_family,
                    k.status,
                    k.quota_billable_limit,
                    COALESCE(u.billable_tokens, 0)
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_hash = $1",
                &[&key_hash],
            )
            .await
            .context("load authenticated key by hash")?;
        Ok(row.map(|row| AuthenticatedKey {
            key_id: row.get(0),
            key_name: row.get(1),
            provider_type: row.get(2),
            protocol_family: row.get(3),
            status: row.get(4),
            quota_billable_limit: row.get(5),
            billable_tokens_used: row.get::<_, i64>(6),
        }))
    }
    pub(crate) async fn current_dispatch_generation(&self, provider: &str) -> i64 {
        let Some(cache) = self.request_cache.as_ref() else {
            return 0;
        };
        let key = cache.dispatch_generation_key(provider);
        match cache.get_i64(&key).await {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(err) => {
                tracing::warn!(
                    provider,
                    key = %key,
                    error = %err,
                    "request cache generation read failed; falling back to generation=0"
                );
                0
            },
        }
    }
    pub(crate) async fn bump_dispatch_generation(&self, provider: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let key = cache.dispatch_generation_key(provider);
        if let Err(err) = cache.incr(&key).await {
            tracing::warn!(
                provider,
                key = %key,
                error = %err,
                "request cache generation bump failed"
            );
        }
    }
    pub(crate) async fn load_authenticated_key_cached(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_authenticated_key_by_hash(key_hash).await;
        };
        let cache_key = cache.auth_key(key_hash);
        match cache
            .get_json::<crate::request_cache::CachedAuthLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) => return Ok(lookup.key.map(authenticated_key_from_cached)),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache auth read failed; falling back to postgres"
                );
            },
        }
        let key = self.load_authenticated_key_by_hash(key_hash).await?;
        let lookup = crate::request_cache::CachedAuthLookup {
            key: key
                .clone()
                .map(|value| cached_authenticated_key_from_value(&value)),
        };
        let ttl = if key.is_some() {
            cache.auth_ttl(key_hash)
        } else {
            cache.negative_auth_ttl(key_hash)
        };
        if let Err(err) = cache.set_json(&cache_key, &lookup, ttl).await {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache auth write failed"
            );
        }
        Ok(key)
    }
    pub(crate) async fn invalidate_authenticated_key_cache_by_ids(&self, key_ids: &[String]) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let key_hashes = match self.load_key_hashes_by_ids(key_ids).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(error = %err, "failed to load key hashes for auth-cache invalidation");
                return;
            },
        };
        let cache_keys = key_hashes
            .values()
            .map(|key_hash| cache.auth_key(key_hash))
            .collect::<Vec<_>>();
        let cache_key_refs = cache_keys.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(err) = cache.delete_many(cache_key_refs).await {
            tracing::warn!(error = %err, "failed to invalidate auth cache keys");
        }
    }
    pub(crate) async fn invalidate_request_snapshot_cache(&self, provider: &str, key_id: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let cache_key = cache.request_snapshot_key(provider, key_id);
        if let Err(err) = cache.delete(&cache_key).await {
            tracing::warn!(
                provider,
                key = %cache_key,
                error = %err,
                "failed to invalidate request snapshot cache"
            );
        }
    }
    pub(crate) async fn invalidate_account_cache(&self, provider: &str, account_name: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let scope = self.proxy_scope.cache_key_segment();
        let view_key = cache.account_view_key(provider, account_name, scope);
        let auth_key = cache.account_auth_key(provider, account_name);
        if let Err(err) = cache
            .delete_many([view_key.as_str(), auth_key.as_str()])
            .await
        {
            tracing::warn!(
                provider,
                account_name,
                error = %err,
                "failed to invalidate account cache entries"
            );
        }
    }
    pub(crate) async fn invalidate_all_account_views_for_provider(&self, provider: &str) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let account_names = match provider {
            core_store::PROVIDER_CODEX => match self.list_codex_route_candidate_rows().await {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| row.account_name)
                    .collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(provider, error = %err, "failed to list codex accounts for cache invalidation");
                    return;
                },
            },
            core_store::PROVIDER_KIRO => match self.list_kiro_route_candidate_rows().await {
                Ok(rows) => rows
                    .into_iter()
                    .map(|row| row.account_name)
                    .collect::<Vec<_>>(),
                Err(err) => {
                    tracing::warn!(provider, error = %err, "failed to list kiro accounts for cache invalidation");
                    return;
                },
            },
            _ => return,
        };
        let scope = self.proxy_scope.cache_key_segment();
        let view_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(provider, name, scope))
            .collect::<Vec<_>>();
        let view_key_refs = view_keys.iter().map(String::as_str).collect::<Vec<_>>();
        if let Err(err) = cache.delete_many(view_key_refs).await {
            tracing::warn!(provider, error = %err, "failed to invalidate provider account view cache");
        }
    }
    pub(crate) async fn load_codex_request_snapshot_cached(
        &self,
        key_id: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedCodexRequestSnapshot>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.build_codex_request_snapshot(key_id, 0).await;
        };
        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        let cache_key = cache.request_snapshot_key(core_store::PROVIDER_CODEX, key_id);
        match cache
            .get_json::<crate::request_cache::CachedCodexRequestSnapshot>(&cache_key)
            .await
        {
            Ok(Some(snapshot)) if snapshot.generation == generation => return Ok(Some(snapshot)),
            Ok(_) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex snapshot read failed; rebuilding from postgres"
                );
            },
        }
        let snapshot = self
            .build_codex_request_snapshot(key_id, generation)
            .await?;
        if let Some(snapshot_ref) = snapshot.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    snapshot_ref,
                    cache.request_snapshot_ttl(core_store::PROVIDER_CODEX, key_id),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex snapshot write failed"
                );
            }
        }
        Ok(snapshot)
    }
    pub(crate) async fn load_kiro_request_snapshot_cached(
        &self,
        key_id: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedKiroRequestSnapshot>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.build_kiro_request_snapshot(key_id, 0).await;
        };
        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let cache_key = cache.request_snapshot_key(core_store::PROVIDER_KIRO, key_id);
        match cache
            .get_json::<crate::request_cache::CachedKiroRequestSnapshot>(&cache_key)
            .await
        {
            Ok(Some(snapshot)) if snapshot.generation == generation => return Ok(Some(snapshot)),
            Ok(_) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro snapshot read failed; rebuilding from postgres"
                );
            },
        }
        let snapshot = self.build_kiro_request_snapshot(key_id, generation).await?;
        if let Some(snapshot_ref) = snapshot.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    snapshot_ref,
                    cache.request_snapshot_ttl(core_store::PROVIDER_KIRO, key_id),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache kiro snapshot write failed"
                );
            }
        }
        Ok(snapshot)
    }
    pub(crate) async fn build_codex_request_snapshot(
        &self,
        key_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<crate::request_cache::CachedCodexRequestSnapshot>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if bundle.key.provider_type != core_store::PROVIDER_CODEX {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let records = self.list_codex_route_candidate_rows().await?;
        let selected_account_names = self
            .resolve_route_account_names(
                core_store::PROVIDER_CODEX,
                &bundle.route,
                records
                    .iter()
                    .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                    .map(|record| record.account_name.clone())
                    .collect(),
            )
            .await?;
        Ok(Some(crate::request_cache::CachedCodexRequestSnapshot {
            key: cached_authenticated_key_from_bundle(&bundle),
            generation,
            route_strategy: bundle
                .route
                .route_strategy
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            account_group_id_at_event: bundle.route.account_group_id.clone(),
            selected_account_names,
            use_all_active_accounts: false,
            request_max_concurrency: bundle
                .route
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: bundle
                .route
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            codex_fast_enabled: bundle.route.codex_fast_enabled,
            codex_weight_free: runtime_config.codex_weight_free,
            codex_weight_plus: runtime_config.codex_weight_plus,
            codex_weight_pro5x: runtime_config.codex_weight_pro5x,
            codex_weight_pro20x: runtime_config.codex_weight_pro20x,
        }))
    }
    pub(crate) async fn build_kiro_request_snapshot(
        &self,
        key_id: &str,
        generation: i64,
    ) -> anyhow::Result<Option<crate::request_cache::CachedKiroRequestSnapshot>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if bundle.key.provider_type != core_store::PROVIDER_KIRO {
            return Ok(None);
        }
        let runtime_config = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        let records = self.list_kiro_route_candidate_rows().await?;
        let selected_account_names = self
            .resolve_route_account_names(
                core_store::PROVIDER_KIRO,
                &bundle.route,
                records
                    .iter()
                    .filter(|record| record.status == core_store::KEY_STATUS_ACTIVE)
                    .map(|record| record.account_name.clone())
                    .collect(),
            )
            .await?;
        let cache_policy_json = effective_kiro_cache_policy_json(
            &runtime_config.kiro_cache_policy_json,
            bundle.route.kiro_cache_policy_override_json.as_deref(),
        )?;
        Ok(Some(crate::request_cache::CachedKiroRequestSnapshot {
            key: cached_authenticated_key_from_bundle(&bundle),
            generation,
            route_strategy: bundle
                .route
                .route_strategy
                .clone()
                .unwrap_or_else(|| "auto".to_string()),
            account_group_id_at_event: bundle.route.account_group_id.clone(),
            selected_account_names,
            use_all_active_accounts: false,
            request_max_concurrency: bundle
                .route
                .request_max_concurrency
                .and_then(non_negative_i64_to_u64),
            request_min_start_interval_ms: bundle
                .route
                .request_min_start_interval_ms
                .and_then(non_negative_i64_to_u64),
            request_validation_enabled: bundle.route.kiro_request_validation_enabled,
            cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
            zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
            full_request_logging_enabled: bundle.route.kiro_full_request_logging_enabled,
            remote_media_resolution_enabled: bundle.route.kiro_remote_media_resolution_enabled,
            latency_routing_enabled: bundle.route.kiro_latency_routing_enabled,
            model_name_map_json: bundle
                .route
                .model_name_map_json
                .clone()
                .unwrap_or_else(|| "{}".to_string()),
            cache_kmodels_json: runtime_config.kiro_cache_kmodels_json.clone(),
            cache_policy_json,
            context_usage_min_request_tokens: runtime_config
                .kiro_context_usage_min_request_tokens
                .max(0) as u64,
            prefix_cache_mode: runtime_config.kiro_prefix_cache_mode.clone(),
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
            billable_model_multipliers_json: bundle
                .route
                .kiro_billable_model_multipliers_override_json
                .clone()
                .unwrap_or_else(|| runtime_config.kiro_billable_model_multipliers_json.clone()),
            status_refresh_interval_seconds: runtime_config
                .kiro_status_refresh_max_interval_seconds
                .max(0) as u64,
        }))
    }
    pub(crate) async fn resolve_route_account_names(
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
