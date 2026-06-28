//! Direct Anthropic upstream channel store and lightweight usage rollups.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    self as core_store, AdminAnthropicUpstreamChannel, AdminAnthropicUpstreamChannelPatch,
    AdminAnthropicUpstreamChannelsPage, AdminAnthropicUpstreamModelsStatusUpdate,
    AdminAnthropicUpstreamProbeTarget, AdminAnthropicUpstreamStore,
    AdminAnthropicUpstreamTestStatusUpdate, AdminAnthropicUpstreamUsageRollup, AdminPageRequest,
    AnthropicUpstreamChannelUsageDelta, NewAdminAnthropicUpstreamChannel,
};
use serde::{Deserialize, Serialize};

use super::{
    now_ms, proxy_support::resolve_provider_proxy_config_from_context, AnthropicUpstreamChannelRow,
    PostgresControlRepository,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedAnthropicUpstreamChannelsLookup {
    generation: i64,
    rows: Vec<AnthropicUpstreamChannelRow>,
}

fn non_negative_i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn optional_non_negative_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.map(non_negative_i64_to_u64)
}

fn model_ids_from_json_text(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn auth_json_for_api_key(api_key: &str) -> anyhow::Result<String> {
    serde_json::to_string(&serde_json::json!({ "api_key": api_key }))
        .context("serialize anthropic upstream auth json")
}

fn admin_channel_from_row(row: AnthropicUpstreamChannelRow) -> AdminAnthropicUpstreamChannel {
    AdminAnthropicUpstreamChannel {
        name: row.channel_name,
        status: row.status,
        base_url: row.base_url,
        has_api_key: row
            .api_key
            .as_deref()
            .is_some_and(|value| !value.is_empty()),
        weight: non_negative_i64_to_u64(row.weight),
        max_concurrency: non_negative_i64_to_u64(row.max_concurrency),
        min_start_interval_ms: non_negative_i64_to_u64(row.min_start_interval_ms),
        proxy_mode: row.proxy_mode,
        proxy_config_id: row.proxy_config_id,
        last_error: row.last_error,
        models: row.model_ids,
        last_models_status: row.last_models_status,
        last_models_latency_ms: optional_non_negative_i64_to_u64(row.last_models_latency_ms),
        last_models_checked_at: row.last_models_checked_at_ms,
        last_models_error: row.last_models_error,
        last_test_model: row.last_test_model,
        last_test_status: row.last_test_status,
        last_test_latency_ms: optional_non_negative_i64_to_u64(row.last_test_latency_ms),
        last_test_at: row.last_test_at_ms,
        last_test_error: row.last_test_error,
        usage: AdminAnthropicUpstreamUsageRollup {
            input_uncached_tokens: non_negative_i64_to_u64(row.input_uncached_tokens),
            input_cached_tokens: non_negative_i64_to_u64(row.input_cached_tokens),
            output_tokens: non_negative_i64_to_u64(row.output_tokens),
            billable_tokens: non_negative_i64_to_u64(row.billable_tokens),
            usage_missing_events: non_negative_i64_to_u64(row.usage_missing_events),
            last_used_at: row.last_used_at_ms,
        },
        created_at: row.created_at_ms,
        updated_at: row.updated_at_ms,
    }
}

impl PostgresControlRepository {
    fn decode_anthropic_upstream_channel_row(row: super::PgRow) -> AnthropicUpstreamChannelRow {
        AnthropicUpstreamChannelRow {
            channel_name: row.get(0),
            status: row.get(1),
            base_url: row.get(2),
            api_key: row.get(3),
            weight: row.get(4),
            max_concurrency: row.get(5),
            min_start_interval_ms: row.get(6),
            proxy_mode: row.get(7),
            proxy_config_id: row.get(8),
            last_error: row.get(9),
            model_ids: model_ids_from_json_text(row.get::<_, String>(10).as_str()),
            last_models_status: row.get(11),
            last_models_latency_ms: row.get(12),
            last_models_checked_at_ms: row.get(13),
            last_models_error: row.get(14),
            last_test_model: row.get(15),
            last_test_status: row.get(16),
            last_test_latency_ms: row.get(17),
            last_test_at_ms: row.get(18),
            last_test_error: row.get(19),
            created_at_ms: row.get(20),
            updated_at_ms: row.get(21),
            input_uncached_tokens: row.get(22),
            input_cached_tokens: row.get(23),
            output_tokens: row.get(24),
            billable_tokens: row.get(25),
            usage_missing_events: row.get(26),
            last_used_at_ms: row.get(27),
        }
    }

    pub(super) async fn list_anthropic_upstream_channel_rows(
        &self,
        active_only: bool,
    ) -> anyhow::Result<Vec<AnthropicUpstreamChannelRow>> {
        self.ensure_connection_alive()?;
        let status_filter = if active_only { "WHERE c.status = 'active'" } else { "" };
        let sql = format!(
            "SELECT
                c.channel_name,
                c.status,
                c.base_url,
                NULLIF(c.auth_json ->> 'api_key', ''),
                c.weight,
                c.max_concurrency,
                c.min_start_interval_ms,
                c.proxy_mode,
                c.proxy_config_id,
                c.last_error,
                c.model_ids::text,
                c.last_models_status,
                c.last_models_latency_ms,
                c.last_models_checked_at_ms,
                c.last_models_error,
                c.last_test_model,
                c.last_test_status,
                c.last_test_latency_ms,
                c.last_test_at_ms,
                c.last_test_error,
                c.created_at_ms,
                c.updated_at_ms,
                COALESCE(u.input_uncached_tokens, 0),
                COALESCE(u.input_cached_tokens, 0),
                COALESCE(u.output_tokens, 0),
                COALESCE(u.billable_tokens, 0),
                COALESCE(u.usage_missing_events, 0),
                u.last_used_at_ms
             FROM llm_anthropic_upstream_channels c
             LEFT JOIN llm_anthropic_upstream_channel_usage_rollups u
                ON u.channel_name = c.channel_name
             {status_filter}
             ORDER BY c.status ASC, c.weight DESC, c.channel_name ASC"
        );
        let rows = self
            .client
            .query(&sql, &[])
            .await
            .context("list postgres anthropic upstream channels")?;
        Ok(rows
            .into_iter()
            .map(Self::decode_anthropic_upstream_channel_row)
            .collect())
    }

    pub(super) async fn load_active_anthropic_upstream_channel_rows_cached(
        &self,
    ) -> anyhow::Result<Vec<AnthropicUpstreamChannelRow>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.list_anthropic_upstream_channel_rows(true).await;
        };
        let generation = self
            .current_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        let scope = self.proxy_scope.cache_key_segment();
        let cache_key = cache.anthropic_upstream_channels_key(scope);
        match cache
            .get_json::<CachedAnthropicUpstreamChannelsLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.rows),
            Ok(Some(_)) => {},
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache direct anthropic upstream channel read failed; falling back to postgres"
                );
            },
        }
        let rows = self.list_anthropic_upstream_channel_rows(true).await?;
        let lookup = CachedAnthropicUpstreamChannelsLookup {
            generation,
            rows: rows.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.anthropic_upstream_channels_ttl(scope))
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache direct anthropic upstream channel write failed"
            );
        }
        Ok(rows)
    }

    async fn load_anthropic_upstream_channel_row(
        &self,
        channel_name: &str,
    ) -> anyhow::Result<Option<AnthropicUpstreamChannelRow>> {
        self.ensure_connection_alive()?;
        self.client
            .query_opt(
                "SELECT
                    c.channel_name,
                    c.status,
                    c.base_url,
                    NULLIF(c.auth_json ->> 'api_key', ''),
                    c.weight,
                    c.max_concurrency,
                    c.min_start_interval_ms,
                    c.proxy_mode,
                    c.proxy_config_id,
                    c.last_error,
                    c.model_ids::text,
                    c.last_models_status,
                    c.last_models_latency_ms,
                    c.last_models_checked_at_ms,
                    c.last_models_error,
                    c.last_test_model,
                    c.last_test_status,
                    c.last_test_latency_ms,
                    c.last_test_at_ms,
                    c.last_test_error,
                    c.created_at_ms,
                    c.updated_at_ms,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.usage_missing_events, 0),
                    u.last_used_at_ms
                 FROM llm_anthropic_upstream_channels c
                 LEFT JOIN llm_anthropic_upstream_channel_usage_rollups u
                    ON u.channel_name = c.channel_name
                 WHERE c.channel_name = $1",
                &[&channel_name],
            )
            .await
            .context("load postgres anthropic upstream channel")?
            .map(Self::decode_anthropic_upstream_channel_row)
            .map_or(Ok(None), |row| Ok(Some(row)))
    }

    pub(crate) async fn record_anthropic_upstream_channel_usage(
        &self,
        channel_name: &str,
        delta: AnthropicUpstreamChannelUsageDelta,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let input_uncached_tokens = delta.input_uncached_tokens.min(i64::MAX as u64) as i64;
        let input_cached_tokens = delta.input_cached_tokens.min(i64::MAX as u64) as i64;
        let output_tokens = delta.output_tokens.min(i64::MAX as u64) as i64;
        let billable_tokens = delta.billable_tokens.min(i64::MAX as u64) as i64;
        let usage_missing_events = if delta.usage_missing { 1_i64 } else { 0_i64 };
        let updated_at_ms = now_ms().max(delta.used_at_ms);
        self.client
            .execute(
                "INSERT INTO llm_anthropic_upstream_channel_usage_rollups (
                    channel_name,
                    input_uncached_tokens,
                    input_cached_tokens,
                    output_tokens,
                    billable_tokens,
                    usage_missing_events,
                    last_used_at_ms,
                    updated_at_ms
                 )
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (channel_name) DO UPDATE SET
                    input_uncached_tokens =
                        llm_anthropic_upstream_channel_usage_rollups.input_uncached_tokens
                        + EXCLUDED.input_uncached_tokens,
                    input_cached_tokens =
                        llm_anthropic_upstream_channel_usage_rollups.input_cached_tokens
                        + EXCLUDED.input_cached_tokens,
                    output_tokens =
                        llm_anthropic_upstream_channel_usage_rollups.output_tokens
                        + EXCLUDED.output_tokens,
                    billable_tokens =
                        llm_anthropic_upstream_channel_usage_rollups.billable_tokens
                        + EXCLUDED.billable_tokens,
                    usage_missing_events =
                        llm_anthropic_upstream_channel_usage_rollups.usage_missing_events
                        + EXCLUDED.usage_missing_events,
                    last_used_at_ms = GREATEST(
                        COALESCE(llm_anthropic_upstream_channel_usage_rollups.last_used_at_ms, 0),
                        EXCLUDED.last_used_at_ms
                    ),
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &channel_name,
                    &input_uncached_tokens,
                    &input_cached_tokens,
                    &output_tokens,
                    &billable_tokens,
                    &usage_missing_events,
                    &delta.used_at_ms,
                    &updated_at_ms,
                ],
            )
            .await
            .context("record postgres anthropic upstream channel usage")?;
        Ok(())
    }
}

#[async_trait]
impl AdminAnthropicUpstreamStore for PostgresControlRepository {
    async fn list_admin_anthropic_upstream_channels(
        &self,
    ) -> anyhow::Result<Vec<AdminAnthropicUpstreamChannel>> {
        Ok(self
            .list_anthropic_upstream_channel_rows(false)
            .await?
            .into_iter()
            .map(admin_channel_from_row)
            .collect())
    }

    async fn list_admin_anthropic_upstream_channels_page(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminAnthropicUpstreamChannelsPage> {
        let channels = self.list_admin_anthropic_upstream_channels().await?;
        let total = channels.len();
        let offset = page.offset.min(total);
        let limit = page.limit.max(1);
        let end = offset.saturating_add(limit).min(total);
        Ok(AdminAnthropicUpstreamChannelsPage {
            channels: channels[offset..end].to_vec(),
            total,
            limit,
            offset,
            has_more: end < total,
        })
    }

    async fn create_admin_anthropic_upstream_channel(
        &self,
        channel: NewAdminAnthropicUpstreamChannel,
    ) -> anyhow::Result<AdminAnthropicUpstreamChannel> {
        self.ensure_connection_alive()?;
        let auth_json = auth_json_for_api_key(&channel.api_key)?;
        let weight = channel.weight.min(i64::MAX as u64) as i64;
        let max_concurrency = channel.max_concurrency.min(i64::MAX as u64) as i64;
        let min_start_interval_ms = channel.min_start_interval_ms.min(i64::MAX as u64) as i64;
        self.client
            .execute(
                "INSERT INTO llm_anthropic_upstream_channels (
                    channel_name,
                    status,
                    base_url,
                    auth_json,
                    weight,
                    max_concurrency,
                    min_start_interval_ms,
                    proxy_mode,
                    proxy_config_id,
                    created_at_ms,
                    updated_at_ms
                 )
                 VALUES ($1, $2, $3, $4::jsonb, $5, $6, $7, $8, $9, $10, $10)",
                &[
                    &channel.name,
                    &channel.status,
                    &channel.base_url,
                    &auth_json,
                    &weight,
                    &max_concurrency,
                    &min_start_interval_ms,
                    &channel.proxy_mode,
                    &channel.proxy_config_id,
                    &channel.created_at_ms,
                ],
            )
            .await
            .context("create postgres anthropic upstream channel")?;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.load_anthropic_upstream_channel_row(&channel.name)
            .await?
            .map(admin_channel_from_row)
            .context("created postgres anthropic upstream channel disappeared")
    }

    async fn patch_admin_anthropic_upstream_channel(
        &self,
        name: &str,
        patch: AdminAnthropicUpstreamChannelPatch,
    ) -> anyhow::Result<Option<AdminAnthropicUpstreamChannel>> {
        let Some(mut row) = self.load_anthropic_upstream_channel_row(name).await? else {
            return Ok(None);
        };
        if let Some(value) = patch.status {
            row.status = value;
        }
        if let Some(value) = patch.base_url {
            row.base_url = value;
        }
        if let Some(value) = patch.weight {
            row.weight = value.min(i64::MAX as u64) as i64;
        }
        if let Some(value) = patch.max_concurrency {
            row.max_concurrency = value.min(i64::MAX as u64) as i64;
        }
        if let Some(value) = patch.min_start_interval_ms {
            row.min_start_interval_ms = value.min(i64::MAX as u64) as i64;
        }
        if let Some(value) = patch.proxy_mode {
            row.proxy_mode = value;
        }
        if let Some(value) = patch.proxy_config_id {
            row.proxy_config_id = value;
        }
        if patch.clear_last_error {
            row.last_error = None;
        }
        let auth_json = patch
            .api_key
            .as_deref()
            .map(auth_json_for_api_key)
            .transpose()?;
        let updated_at_ms = patch.updated_at_ms;
        self.client
            .execute(
                "UPDATE llm_anthropic_upstream_channels
                 SET status = $2,
                     base_url = $3,
                     auth_json = COALESCE($4::jsonb, auth_json),
                     weight = $5,
                     max_concurrency = $6,
                     min_start_interval_ms = $7,
                     proxy_mode = $8,
                     proxy_config_id = $9,
                     last_error = $10,
                     updated_at_ms = $11
                 WHERE channel_name = $1",
                &[
                    &name,
                    &row.status,
                    &row.base_url,
                    &auth_json,
                    &row.weight,
                    &row.max_concurrency,
                    &row.min_start_interval_ms,
                    &row.proxy_mode,
                    &row.proxy_config_id,
                    &row.last_error,
                    &updated_at_ms,
                ],
            )
            .await
            .context("patch postgres anthropic upstream channel")?;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.load_anthropic_upstream_channel_row(name)
            .await
            .map(|row| row.map(admin_channel_from_row))
    }

    async fn delete_admin_anthropic_upstream_channel(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminAnthropicUpstreamChannel>> {
        let Some(row) = self.load_anthropic_upstream_channel_row(name).await? else {
            return Ok(None);
        };
        self.client
            .execute("DELETE FROM llm_anthropic_upstream_channels WHERE channel_name = $1", &[
                &name,
            ])
            .await
            .context("delete postgres anthropic upstream channel")?;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(admin_channel_from_row(row)))
    }

    async fn load_admin_anthropic_upstream_probe_target(
        &self,
        name: &str,
    ) -> anyhow::Result<Option<AdminAnthropicUpstreamProbeTarget>> {
        let Some(row) = self.load_anthropic_upstream_channel_row(name).await? else {
            return Ok(None);
        };
        let Some(api_key) = row
            .api_key
            .clone()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
        else {
            anyhow::bail!("anthropic upstream channel api key is missing");
        };
        let proxy_context = self
            .load_provider_proxy_resolution_context(core_store::PROVIDER_KIRO)
            .await?;
        let (proxy, proxy_error) = match resolve_provider_proxy_config_from_context(
            &row.proxy_mode,
            row.proxy_config_id.as_deref(),
            &proxy_context,
        ) {
            Ok(proxy) => (proxy, None),
            Err(err) => (None, Some(err.to_string())),
        };
        Ok(Some(AdminAnthropicUpstreamProbeTarget {
            name: row.channel_name,
            base_url: row.base_url,
            api_key,
            proxy,
            proxy_error,
        }))
    }

    async fn save_admin_anthropic_upstream_models_status(
        &self,
        name: &str,
        update: AdminAnthropicUpstreamModelsStatusUpdate,
    ) -> anyhow::Result<Option<AdminAnthropicUpstreamChannel>> {
        self.ensure_connection_alive()?;
        let model_ids_json = serde_json::to_string(&update.model_ids)
            .context("serialize anthropic upstream model ids")?;
        let latency_ms = update
            .latency_ms
            .map(|value| value.min(i64::MAX as u64) as i64);
        let updated_at_ms = now_ms().max(update.checked_at_ms);
        let updated = self
            .client
            .execute(
                "UPDATE llm_anthropic_upstream_channels
                 SET model_ids = $2::jsonb,
                     last_models_status = $3,
                     last_models_latency_ms = $4,
                     last_models_checked_at_ms = $5,
                     last_models_error = $6,
                     updated_at_ms = $7
                 WHERE channel_name = $1",
                &[
                    &name,
                    &model_ids_json,
                    &update.status,
                    &latency_ms,
                    &update.checked_at_ms,
                    &update.error,
                    &updated_at_ms,
                ],
            )
            .await
            .context("save postgres anthropic upstream models status")?;
        if updated == 0 {
            return Ok(None);
        }
        self.load_anthropic_upstream_channel_row(name)
            .await
            .map(|row| row.map(admin_channel_from_row))
    }

    async fn save_admin_anthropic_upstream_test_status(
        &self,
        name: &str,
        update: AdminAnthropicUpstreamTestStatusUpdate,
    ) -> anyhow::Result<Option<AdminAnthropicUpstreamChannel>> {
        self.ensure_connection_alive()?;
        let latency_ms = update
            .latency_ms
            .map(|value| value.min(i64::MAX as u64) as i64);
        let updated_at_ms = now_ms().max(update.checked_at_ms);
        let updated = self
            .client
            .execute(
                "UPDATE llm_anthropic_upstream_channels
                 SET last_test_model = $2,
                     last_test_status = $3,
                     last_test_latency_ms = $4,
                     last_test_at_ms = $5,
                     last_test_error = $6,
                     updated_at_ms = $7
                 WHERE channel_name = $1",
                &[
                    &name,
                    &update.model,
                    &update.status,
                    &latency_ms,
                    &update.checked_at_ms,
                    &update.error,
                    &updated_at_ms,
                ],
            )
            .await
            .context("save postgres anthropic upstream test status")?;
        if updated == 0 {
            return Ok(None);
        }
        self.load_anthropic_upstream_channel_row(name)
            .await
            .map(|row| row.map(admin_channel_from_row))
    }
}
