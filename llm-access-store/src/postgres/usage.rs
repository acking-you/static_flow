//! Usage proxy-attribution resolution, rollup batching, and the
//! `UsageEventSink` impl.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::{
    store::{self as core_store, UsageEventSink},
    usage::UsageEvent,
};
use sqlx_core::query_builder::QueryBuilder;
use sqlx_postgres::Postgres;

use super::{
    aggregate_usage_rollup_deltas, decode::decode_codex_account_settings,
    json::optional_json_string_any, PostgresControlRepository, UsageProxyAttribution,
    USAGE_ROLLUP_BATCH_ROW_LIMIT,
};

impl PostgresControlRepository {
    /// Resolve the effective proxy attribution for one consumed usage event.
    pub async fn resolve_usage_proxy_attribution(
        &self,
        provider: &str,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let provider = provider.trim();
        let account_name = account_name.trim();
        if provider.is_empty() || account_name.is_empty() {
            return Ok(None);
        }
        let Some(cache) = self.request_cache.as_ref() else {
            return self
                .build_usage_proxy_attribution(provider, account_name)
                .await;
        };
        let generation = self.current_dispatch_generation(provider).await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.usage_proxy_attribution_key(provider, account_name, scope);
        match cache
            .get_json::<crate::request_cache::CachedUsageProxyAttributionLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => {
                return Ok(lookup.attribution.map(|value| UsageProxyAttribution {
                    provider_type: value.provider_type,
                    account_name: value.account_name,
                    proxy_source: value.proxy_source,
                    proxy_config_id: value.proxy_config_id,
                    proxy_config_name: value.proxy_config_name,
                    proxy_url: value.proxy_url,
                }));
            },
            Ok(_) => {},
            Err(err) => tracing::warn!(
                provider,
                account_name,
                key = %cache_key,
                error = %err,
                "request cache usage proxy attribution read failed; falling back to postgres"
            ),
        }
        let attribution = self
            .build_usage_proxy_attribution(provider, account_name)
            .await?;
        let lookup = crate::request_cache::CachedUsageProxyAttributionLookup {
            generation,
            attribution: attribution.clone().map(|value| {
                crate::request_cache::CachedUsageProxyAttributionView {
                    provider_type: value.provider_type,
                    account_name: value.account_name,
                    proxy_source: value.proxy_source,
                    proxy_config_id: value.proxy_config_id,
                    proxy_config_name: value.proxy_config_name,
                    proxy_url: value.proxy_url,
                }
            }),
        };
        if let Err(err) = cache
            .set_json(
                &cache_key,
                &lookup,
                cache.usage_proxy_attribution_ttl(provider, account_name, scope),
            )
            .await
        {
            tracing::warn!(
                provider,
                account_name,
                key = %cache_key,
                error = %err,
                "request cache usage proxy attribution write failed"
            );
        }
        Ok(attribution)
    }

    async fn build_usage_proxy_attribution(
        &self,
        provider: &str,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        match provider {
            core_store::PROVIDER_CODEX => {
                self.build_codex_usage_proxy_attribution(account_name).await
            },
            core_store::PROVIDER_KIRO => {
                self.build_kiro_usage_proxy_attribution(account_name).await
            },
            _ => Ok(None),
        }
    }

    async fn build_codex_usage_proxy_attribution(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let Some(record) = self.get_codex_account_row(account_name).await? else {
            return Ok(None);
        };
        let settings = decode_codex_account_settings(&record.settings_json)?;
        let context = self.load_codex_admin_account_view_context().await?;
        let (proxy_source, proxy_url, proxy_config_name) =
            self.resolve_codex_account_proxy_view_with_context(&settings, &context);
        let proxy_config_id = match settings.proxy_mode.as_str() {
            "fixed" => settings.proxy_config_id.clone(),
            _ => context.codex_proxy_binding.bound_proxy_config_id.clone(),
        };
        Ok(Some(UsageProxyAttribution {
            provider_type: core_store::PROVIDER_CODEX.to_string(),
            account_name: record.account_name,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url,
        }))
    }

    async fn build_kiro_usage_proxy_attribution(
        &self,
        account_name: &str,
    ) -> anyhow::Result<Option<UsageProxyAttribution>> {
        let Some(record) = self.get_kiro_account_row(account_name).await? else {
            return Ok(None);
        };
        let auth_json = serde_json::from_str::<serde_json::Value>(&record.auth_json)
            .context("parse kiro account auth json for usage proxy attribution")?;
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
        let context = self.load_kiro_admin_account_view_context().await?;
        let (proxy_source, proxy_url, proxy_config_name) = self
            .resolve_kiro_account_proxy_view_with_context(
                &proxy_mode,
                proxy_config_id.as_deref(),
                &context,
            );
        let proxy_config_id = match proxy_mode.as_str() {
            "fixed" => proxy_config_id,
            _ => context.kiro_proxy_binding.bound_proxy_config_id.clone(),
        };
        Ok(Some(UsageProxyAttribution {
            provider_type: core_store::PROVIDER_KIRO.to_string(),
            account_name: record.account_name,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url,
        }))
    }

    pub(super) async fn apply_usage_rollups_batch(
        &self,
        events: &[UsageEvent],
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        if events.is_empty() {
            return Ok(());
        }
        let deltas = aggregate_usage_rollup_deltas(events)?;
        for chunk in deltas.chunks(USAGE_ROLLUP_BATCH_ROW_LIMIT.max(1)) {
            let mut builder = QueryBuilder::<Postgres>::new(
                "UPDATE llm_key_usage_rollups AS u
                 SET input_uncached_tokens = u.input_uncached_tokens + v.input_uncached_tokens,
                     input_cached_tokens = u.input_cached_tokens + v.input_cached_tokens,
                     output_tokens = u.output_tokens + v.output_tokens,
                     billable_tokens = u.billable_tokens + v.billable_tokens,
                     credit_total = ((u.credit_total)::numeric + (v.credit_total::double \
                 precision)::numeric)::text,
                     credit_missing_events = u.credit_missing_events + v.credit_missing_events,
                     last_used_at_ms = CASE
                         WHEN u.last_used_at_ms IS NULL THEN v.last_used_at_ms
                         ELSE GREATEST(u.last_used_at_ms, v.last_used_at_ms)
                     END,
                     updated_at_ms = GREATEST(u.updated_at_ms, v.last_used_at_ms)
                 FROM (",
            );
            builder.push_values(chunk.iter(), |mut row, (key_id, delta)| {
                row.push_bind(*key_id)
                    .push_bind(delta.input_uncached_tokens)
                    .push_bind(delta.input_cached_tokens)
                    .push_bind(delta.output_tokens)
                    .push_bind(delta.billable_tokens)
                    .push_bind(delta.credit_total)
                    .push_bind(delta.credit_missing_events)
                    .push_bind(delta.last_used_at_ms);
            });
            builder.push(
                ") AS v(
                    key_id,
                    input_uncached_tokens,
                    input_cached_tokens,
                    output_tokens,
                    billable_tokens,
                    credit_total,
                    credit_missing_events,
                    last_used_at_ms
                 )
                 WHERE u.key_id = v.key_id",
            );
            let changed = builder
                .build()
                .persistent(false)
                .execute(&self.client.pool)
                .await
                .context("batch update postgres usage rollups")?
                .rows_affected();
            if changed != chunk.len() as u64 {
                anyhow::bail!(
                    "usage rollup rows missing for {} key(s) in postgres batch update",
                    chunk.len().saturating_sub(changed as usize)
                );
            }
        }
        let key_ids = deltas
            .iter()
            .map(|(key_id, _)| (*key_id).to_string())
            .collect::<Vec<_>>();
        self.invalidate_authenticated_key_cache_by_ids(&key_ids)
            .await;
        Ok(())
    }
}
#[async_trait]
impl UsageEventSink for PostgresControlRepository {
    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(events).await
    }

    async fn append_usage_events_owned(&self, events: Vec<UsageEvent>) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(&events).await
    }
}
