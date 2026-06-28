//! Direct Anthropic upstream channel store and lightweight usage rollups.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    self as core_store, AdminAnthropicUpstreamChannel, AdminAnthropicUpstreamChannelPatch,
    AdminAnthropicUpstreamChannelsPage, AdminAnthropicUpstreamStore,
    AdminAnthropicUpstreamUsageRollup, AdminPageRequest, AnthropicUpstreamChannelUsageDelta,
    NewAdminAnthropicUpstreamChannel,
};

use super::{now_ms, AnthropicUpstreamChannelRow, PostgresControlRepository};

fn non_negative_i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
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
            created_at_ms: row.get(10),
            updated_at_ms: row.get(11),
            input_uncached_tokens: row.get(12),
            input_cached_tokens: row.get(13),
            output_tokens: row.get(14),
            billable_tokens: row.get(15),
            usage_missing_events: row.get(16),
            last_used_at_ms: row.get(17),
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
}
