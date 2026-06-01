//! Request-path caching layer: cached loads/builds of runtime config,
//! proxy configs, rate-limit status, authenticated keys, request snapshots
//! and per-provider account views, plus their invalidation.

use std::{collections::BTreeMap, time::Instant};

use llm_access_core::store::{
    self as core_store, AdminProxyBinding, AdminProxyConfig, AuthenticatedKey, CodexRateLimitStatus,
};

use super::{
    cache_convert::{
        authenticated_key_from_cached, build_cached_kiro_account_view,
        cached_authenticated_key_from_bundle, cached_authenticated_key_from_value,
        cached_proxy_from_option, effective_kiro_cache_policy_json,
    },
    decode::decode_codex_account_settings,
    json::non_negative_i64_to_u64,
    proxy_support::resolve_provider_proxy_config_from_context,
    CachedCodexRateLimitStatus, PostgresControlRepository, CODEX_STATUS_CACHE_TTL,
};

impl PostgresControlRepository {
    pub(super) async fn load_admin_proxy_configs_cached(
        &self,
    ) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.list_admin_proxy_configs_rows().await;
        };
        let generation = self.current_proxy_metadata_generation().await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.proxy_configs_key(scope);
        match cache
            .get_json::<crate::request_cache::CachedProxyConfigsLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.configs),
            Ok(Some(_)) => {},
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache proxy-configs read failed; falling back to postgres"
                );
            },
        }
        let configs = self.list_admin_proxy_configs_rows().await?;
        let lookup = crate::request_cache::CachedProxyConfigsLookup {
            generation,
            configs: configs.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.proxy_configs_ttl(scope))
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache proxy-configs write failed"
            );
        }
        Ok(configs)
    }

    pub(super) async fn load_admin_proxy_binding_cached(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<AdminProxyBinding> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_admin_proxy_binding_row(provider_type).await;
        };
        let generation = self.current_dispatch_generation(provider_type).await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.proxy_binding_key(provider_type, scope);
        match cache
            .get_json::<crate::request_cache::CachedProxyBindingLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.binding),
            Ok(Some(_)) => {},
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    provider = provider_type,
                    key = %cache_key,
                    error = %err,
                    "request cache proxy-binding read failed; falling back to postgres"
                );
            },
        }
        let proxy_configs_by_id = self
            .load_admin_proxy_configs_cached()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let binding = self
            .load_admin_proxy_binding_from_configs(provider_type, &proxy_configs_by_id)
            .await?;
        let lookup = crate::request_cache::CachedProxyBindingLookup {
            generation,
            binding: binding.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.proxy_binding_ttl(provider_type, scope))
            .await
        {
            tracing::warn!(
                provider = provider_type,
                key = %cache_key,
                error = %err,
                "request cache proxy-binding write failed"
            );
        }
        Ok(binding)
    }

    async fn cached_codex_rate_limit_status(&self) -> Option<CodexRateLimitStatus> {
        let guard = self.codex_status_cache.read().await;
        let cached = guard.as_ref()?;
        if cached.loaded_at.elapsed() > CODEX_STATUS_CACHE_TTL {
            return None;
        }
        Some(cached.snapshot.clone())
    }

    pub(super) async fn store_cached_codex_rate_limit_status(
        &self,
        snapshot: Option<CodexRateLimitStatus>,
    ) {
        let mut guard = self.codex_status_cache.write().await;
        *guard = snapshot.map(|snapshot| CachedCodexRateLimitStatus {
            snapshot,
            loaded_at: Instant::now(),
        });
    }

    pub(super) async fn load_authenticated_key_cached(
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

    pub(super) async fn invalidate_authenticated_key_cache_by_ids(&self, key_ids: &[String]) {
        if key_ids.is_empty() {
            return;
        }
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

    pub(super) async fn invalidate_request_snapshot_cache(&self, provider: &str, key_id: &str) {
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

    pub(super) async fn invalidate_account_cache(&self, provider: &str, account_name: &str) {
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

    pub(super) async fn invalidate_all_account_views_for_provider(&self, provider: &str) {
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
        if account_names.is_empty() {
            return;
        }
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

    pub(super) async fn load_codex_request_snapshot_cached(
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

    pub(super) async fn load_kiro_request_snapshot_cached(
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

    async fn build_codex_request_snapshot(
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

    async fn build_kiro_request_snapshot(
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
            compact_trigger_tokens: runtime_config.kiro_compact_trigger_tokens.max(0) as u64,
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

    pub(super) async fn load_codex_account_views_cached(
        &self,
        account_names: &[String],
    ) -> anyhow::Result<BTreeMap<String, crate::request_cache::CachedCodexAccountView>> {
        if account_names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let Some(cache) = self.request_cache.as_ref() else {
            let proxy_context = self
                .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
                .await?;
            return self
                .list_codex_route_candidate_rows_by_names(account_names)
                .await?
                .into_iter()
                .map(|row| {
                    let settings = decode_codex_account_settings(&row.settings_json)?;
                    let proxy = resolve_provider_proxy_config_from_context(
                        &settings.proxy_mode,
                        settings.proxy_config_id.as_deref(),
                        &proxy_context,
                    )?;
                    Ok((row.account_name.clone(), crate::request_cache::CachedCodexAccountView {
                        account_name: row.account_name,
                        generation: 0,
                        status: row.status,
                        map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
                        auth_refresh_enabled: settings.auth_refresh_enabled,
                        route_weight_tier: settings.route_weight_tier,
                        request_max_concurrency: settings.request_max_concurrency,
                        request_min_start_interval_ms: settings.request_min_start_interval_ms,
                        last_refresh_at_ms: row.last_refresh_at_ms,
                        last_error: row.last_error,
                        access_token: row.access_token,
                        proxy: cached_proxy_from_option(proxy),
                    }))
                })
                .collect();
        };

        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_keys = account_names
            .iter()
            .map(|name| cache.account_view_key(core_store::PROVIDER_CODEX, name, scope))
            .collect::<Vec<_>>();
        let cached_values = match cache
            .mget_json::<crate::request_cache::CachedCodexAccountView>(&cache_keys)
            .await
        {
            Ok(values) => values,
            Err(err) => {
                tracing::warn!(error = %err, "request cache codex account view batch read failed");
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
            .load_provider_proxy_resolution_context(core_store::PROVIDER_CODEX)
            .await?;
        for row in self
            .list_codex_route_candidate_rows_by_names(&missing)
            .await?
        {
            let settings = decode_codex_account_settings(&row.settings_json)?;
            let proxy = resolve_provider_proxy_config_from_context(
                &settings.proxy_mode,
                settings.proxy_config_id.as_deref(),
                &proxy_context,
            )?;
            let view = crate::request_cache::CachedCodexAccountView {
                account_name: row.account_name.clone(),
                generation,
                status: row.status,
                map_gpt53_codex_to_spark: settings.map_gpt53_codex_to_spark,
                auth_refresh_enabled: settings.auth_refresh_enabled,
                route_weight_tier: settings.route_weight_tier,
                request_max_concurrency: settings.request_max_concurrency,
                request_min_start_interval_ms: settings.request_min_start_interval_ms,
                last_refresh_at_ms: row.last_refresh_at_ms,
                last_error: row.last_error,
                access_token: row.access_token,
                proxy: cached_proxy_from_option(proxy),
            };
            let cache_key =
                cache.account_view_key(core_store::PROVIDER_CODEX, &row.account_name, scope);
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    &view,
                    cache.account_view_ttl(core_store::PROVIDER_CODEX, &row.account_name, scope),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex account view write failed"
                );
            }
            views_by_name.insert(row.account_name, view);
        }
        Ok(views_by_name)
    }

    pub(super) async fn load_kiro_account_views_cached(
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

    pub(super) async fn load_codex_account_auth_cached(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<crate::request_cache::CachedAccountAuth>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return Ok(self
                .get_codex_account_row(account_name)
                .await?
                .map(|record| crate::request_cache::CachedAccountAuth {
                    auth_json: record.auth_json,
                }));
        };
        let cache_key = cache.account_auth_key(core_store::PROVIDER_CODEX, account_name);
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
                    "request cache codex auth read failed; falling back to postgres"
                );
            },
        }
        let auth = self
            .get_codex_account_row(account_name)
            .await?
            .map(|record| crate::request_cache::CachedAccountAuth {
                auth_json: record.auth_json,
            });
        if let Some(auth_ref) = auth.as_ref() {
            if let Err(err) = cache
                .set_json(
                    &cache_key,
                    auth_ref,
                    cache.account_auth_ttl(core_store::PROVIDER_CODEX, account_name),
                )
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex auth write failed"
                );
            }
        }
        Ok(auth)
    }

    pub(super) async fn load_kiro_account_auth_cached(
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

    pub(super) async fn load_codex_rate_limit_status_cached(
        &self,
    ) -> anyhow::Result<Option<CodexRateLimitStatus>> {
        if let Some(snapshot) = self.cached_codex_rate_limit_status().await {
            return Ok(Some(snapshot));
        }
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            match cache
                .get_json::<crate::request_cache::CachedCodexStatusLookup>(&cache_key)
                .await
            {
                Ok(Some(lookup)) => {
                    self.store_cached_codex_rate_limit_status(lookup.snapshot.clone())
                        .await;
                    return Ok(lookup.snapshot);
                },
                Ok(None) => {},
                Err(err) => {
                    tracing::warn!(
                        key = %cache_key,
                        error = %err,
                        "request cache codex status read failed; falling back to postgres"
                    );
                },
            }
        }
        let snapshot = self.load_codex_rate_limit_status_row().await?;
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.codex_status_key();
            let lookup = crate::request_cache::CachedCodexStatusLookup {
                snapshot: snapshot.clone(),
            };
            if let Err(err) = cache
                .set_json(&cache_key, &lookup, cache.codex_status_ttl())
                .await
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache codex status write failed"
                );
            }
        }
        self.store_cached_codex_rate_limit_status(snapshot.clone())
            .await;
        Ok(snapshot)
    }
}
