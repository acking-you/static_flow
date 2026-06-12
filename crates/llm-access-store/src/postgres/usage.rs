//! Usage proxy-attribution resolution, rollup batching, and the
//! `UsageEventSink` impl.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::{
    store::{
        self as core_store, KeyUsageRollupDelta, UsageEventSink, UsageRollupApplyReport,
        UsageRollupBatch, UsageRollupBatchSink, UsageRollupDigestMismatch,
    },
    usage::UsageEvent,
};
use llm_usage_journal::wire::encode_rollup_batch_v1;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx_core::{query::query, query_builder::QueryBuilder, row::Row};
use sqlx_postgres::Postgres;

use super::{
    aggregate_usage_rollup_deltas, decode::decode_codex_account_settings,
    json::optional_json_string_any, now_ms, PostgresControlRepository, UsageProxyAttribution,
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
        let skipped = self.upsert_usage_rollup_deltas(&deltas).await?;
        if skipped > 0 {
            tracing::warn!(
                missing_key_delta_count = skipped,
                delta_count = deltas.len(),
                "skipped postgres usage rollup deltas for missing keys"
            );
        }
        let key_ids = deltas
            .iter()
            .map(|delta| delta.key_id.clone())
            .collect::<Vec<_>>();
        self.invalidate_authenticated_key_cache_by_ids(&key_ids)
            .await;
        Ok(())
    }

    async fn upsert_usage_rollup_deltas(
        &self,
        deltas: &[KeyUsageRollupDelta],
    ) -> anyhow::Result<usize> {
        let mut affected_rows = 0usize;
        for chunk in deltas.chunks(USAGE_ROLLUP_BATCH_ROW_LIMIT.max(1)) {
            let mut builder = QueryBuilder::<Postgres>::new(
                "INSERT INTO llm_key_usage_rollups (
                    key_id,
                    input_uncached_tokens,
                    input_cached_tokens,
                    output_tokens,
                    billable_tokens,
                    credit_total,
                    credit_missing_events,
                    last_used_at_ms,
                    updated_at_ms
                 )
                 SELECT
                    v.key_id,
                    v.input_uncached_tokens,
                    v.input_cached_tokens,
                    v.output_tokens,
                    v.billable_tokens,
                    v.credit_total::text,
                    v.credit_missing_events,
                    v.last_used_at_ms,
                    v.updated_at_ms
                 FROM (",
            );
            builder.push_values(chunk.iter(), |mut row, delta| {
                row.push_bind(&delta.key_id)
                    .push_bind(delta.input_uncached_tokens)
                    .push_bind(delta.input_cached_tokens)
                    .push_bind(delta.output_tokens)
                    .push_bind(delta.billable_tokens)
                    .push_bind(delta.credit_total)
                    .push_bind(delta.credit_missing_events)
                    .push_bind(delta.last_used_at_ms)
                    .push_bind(delta.last_used_at_ms.unwrap_or_else(now_ms));
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
                    last_used_at_ms,
                    updated_at_ms
                 )
                 JOIN llm_keys AS k ON k.key_id = v.key_id
                 WHERE TRUE
                 ON CONFLICT (key_id) DO UPDATE SET
                    input_uncached_tokens =
                        llm_key_usage_rollups.input_uncached_tokens
                        + EXCLUDED.input_uncached_tokens,
                    input_cached_tokens =
                        llm_key_usage_rollups.input_cached_tokens
                        + EXCLUDED.input_cached_tokens,
                    output_tokens =
                        llm_key_usage_rollups.output_tokens
                        + EXCLUDED.output_tokens,
                    billable_tokens =
                        llm_key_usage_rollups.billable_tokens
                        + EXCLUDED.billable_tokens,
                    credit_total = (
                        (llm_key_usage_rollups.credit_total)::numeric
                        + (EXCLUDED.credit_total)::numeric
                    )::text,
                    credit_missing_events =
                        llm_key_usage_rollups.credit_missing_events
                        + EXCLUDED.credit_missing_events,
                    last_used_at_ms = CASE
                        WHEN EXCLUDED.last_used_at_ms IS NULL THEN
                            llm_key_usage_rollups.last_used_at_ms
                        WHEN llm_key_usage_rollups.last_used_at_ms IS NULL THEN
                            EXCLUDED.last_used_at_ms
                        ELSE GREATEST(
                            llm_key_usage_rollups.last_used_at_ms,
                            EXCLUDED.last_used_at_ms
                        )
                    END,
                    updated_at_ms = GREATEST(
                        llm_key_usage_rollups.updated_at_ms,
                        EXCLUDED.updated_at_ms
                    )",
            );
            let changed = builder
                .build()
                .persistent(false)
                .execute(&self.client.pool)
                .await
                .context("batch upsert postgres usage rollups")?
                .rows_affected();
            affected_rows =
                affected_rows.saturating_add(usize::try_from(changed).unwrap_or(usize::MAX));
        }
        Ok(deltas.len().saturating_sub(affected_rows))
    }

    async fn apply_usage_rollup_batches_impl(
        &self,
        batches: &[UsageRollupBatch],
    ) -> anyhow::Result<UsageRollupApplyReport> {
        self.ensure_connection_alive()?;
        if batches.is_empty() {
            return Ok(UsageRollupApplyReport::default());
        }

        let mut tx = self
            .client
            .pool
            .begin()
            .await
            .context("begin postgres usage rollup batch transaction")?;
        let applied_at_ms = now_ms();
        let mut report = UsageRollupApplyReport::default();
        let mut deltas_by_key = std::collections::BTreeMap::<String, KeyUsageRollupDelta>::new();
        let mut applied_batch_ids = Vec::new();

        for batch in batches {
            let digest = usage_rollup_batch_digest(batch)?;
            let source_event_count = i64::try_from(batch.source_event_count)
                .context("usage rollup batch source_event_count exceeds i64")?;
            let delta_count = i64::try_from(batch.deltas.len())
                .context("usage rollup batch delta count exceeds i64")?;
            let inserted = query(
                "INSERT INTO llm_key_usage_rollup_applied_batches (
                    batch_id, digest, source_node_id, source_event_count,
                    delta_count, applied_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (batch_id) DO NOTHING",
            )
            .bind(&batch.batch_id)
            .bind(&digest)
            .bind(&batch.source_node_id)
            .bind(source_event_count)
            .bind(delta_count)
            .bind(applied_at_ms)
            .execute(&mut *tx)
            .await
            .context("insert postgres usage rollup applied batch marker")?
            .rows_affected();

            if inserted == 0 {
                let row = query(
                    "SELECT digest
                     FROM llm_key_usage_rollup_applied_batches
                     WHERE batch_id = $1",
                )
                .bind(&batch.batch_id)
                .fetch_one(&mut *tx)
                .await
                .context("load postgres usage rollup applied batch marker")?;
                let existing_digest: String = row
                    .try_get("digest")
                    .context("decode usage rollup applied batch digest")?;
                if existing_digest != digest {
                    let legacy_json_digest = legacy_json_usage_rollup_batch_digest(batch)?;
                    if existing_digest == legacy_json_digest {
                        let upgraded = query(
                            "UPDATE llm_key_usage_rollup_applied_batches
                             SET digest = $2
                             WHERE batch_id = $1 AND digest = $3",
                        )
                        .bind(&batch.batch_id)
                        .bind(&digest)
                        .bind(&existing_digest)
                        .execute(&mut *tx)
                        .await
                        .context("upgrade postgres usage rollup legacy batch marker digest")?
                        .rows_affected();
                        tracing::warn!(
                            batch_id = %batch.batch_id,
                            upgraded_marker_count = upgraded,
                            "accepted legacy json usage rollup batch marker digest and upgraded to \
                             stable wire digest"
                        );
                    } else {
                        return Err(UsageRollupDigestMismatch {
                            batch_id: batch.batch_id.clone(),
                        }
                        .into());
                    }
                }
                report.already_applied_batch_count =
                    report.already_applied_batch_count.saturating_add(1);
                continue;
            }

            report.applied_batch_count = report.applied_batch_count.saturating_add(1);
            report.delta_count = report.delta_count.saturating_add(batch.deltas.len());
            applied_batch_ids.push(batch.batch_id.clone());
            for delta in &batch.deltas {
                deltas_by_key
                    .entry(delta.key_id.clone())
                    .and_modify(|current| current.add_assign(delta))
                    .or_insert_with(|| delta.clone());
            }
        }

        let deltas = deltas_by_key.into_values().collect::<Vec<_>>();
        let mut affected_rows = 0usize;
        for chunk in deltas.chunks(USAGE_ROLLUP_BATCH_ROW_LIMIT.max(1)) {
            let mut builder = QueryBuilder::<Postgres>::new(
                "INSERT INTO llm_key_usage_rollups (
                    key_id,
                    input_uncached_tokens,
                    input_cached_tokens,
                    output_tokens,
                    billable_tokens,
                    credit_total,
                    credit_missing_events,
                    last_used_at_ms,
                    updated_at_ms
                 )
                 SELECT
                    v.key_id,
                    v.input_uncached_tokens,
                    v.input_cached_tokens,
                    v.output_tokens,
                    v.billable_tokens,
                    v.credit_total::text,
                    v.credit_missing_events,
                    v.last_used_at_ms,
                    v.updated_at_ms
                 FROM (",
            );
            builder.push_values(chunk.iter(), |mut row, delta| {
                row.push_bind(&delta.key_id)
                    .push_bind(delta.input_uncached_tokens)
                    .push_bind(delta.input_cached_tokens)
                    .push_bind(delta.output_tokens)
                    .push_bind(delta.billable_tokens)
                    .push_bind(delta.credit_total)
                    .push_bind(delta.credit_missing_events)
                    .push_bind(delta.last_used_at_ms)
                    .push_bind(delta.last_used_at_ms.unwrap_or(applied_at_ms));
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
                    last_used_at_ms,
                    updated_at_ms
                 )
                 JOIN llm_keys AS k ON k.key_id = v.key_id
                 WHERE TRUE
                 ON CONFLICT (key_id) DO UPDATE SET
                    input_uncached_tokens =
                        llm_key_usage_rollups.input_uncached_tokens
                        + EXCLUDED.input_uncached_tokens,
                    input_cached_tokens =
                        llm_key_usage_rollups.input_cached_tokens
                        + EXCLUDED.input_cached_tokens,
                    output_tokens =
                        llm_key_usage_rollups.output_tokens
                        + EXCLUDED.output_tokens,
                    billable_tokens =
                        llm_key_usage_rollups.billable_tokens
                        + EXCLUDED.billable_tokens,
                    credit_total = (
                        (llm_key_usage_rollups.credit_total)::numeric
                        + (EXCLUDED.credit_total)::numeric
                    )::text,
                    credit_missing_events =
                        llm_key_usage_rollups.credit_missing_events
                        + EXCLUDED.credit_missing_events,
                    last_used_at_ms = CASE
                        WHEN EXCLUDED.last_used_at_ms IS NULL THEN
                            llm_key_usage_rollups.last_used_at_ms
                        WHEN llm_key_usage_rollups.last_used_at_ms IS NULL THEN
                            EXCLUDED.last_used_at_ms
                        ELSE GREATEST(
                            llm_key_usage_rollups.last_used_at_ms,
                            EXCLUDED.last_used_at_ms
                        )
                    END,
                    updated_at_ms = GREATEST(
                        llm_key_usage_rollups.updated_at_ms,
                        EXCLUDED.updated_at_ms
                    )",
            );
            let changed = builder
                .build()
                .persistent(false)
                .execute(&mut *tx)
                .await
                .context("batch upsert postgres idempotent usage rollups")?
                .rows_affected();
            affected_rows =
                affected_rows.saturating_add(usize::try_from(changed).unwrap_or(usize::MAX));
        }

        report.missing_key_delta_count = deltas.len().saturating_sub(affected_rows);
        if report.missing_key_delta_count > 0 {
            tracing::warn!(
                missing_key_delta_count = report.missing_key_delta_count,
                applied_batch_count = report.applied_batch_count,
                delta_count = deltas.len(),
                batch_ids = ?applied_batch_ids.iter().take(8).collect::<Vec<_>>(),
                "skipped postgres idempotent usage rollup deltas for missing keys"
            );
        }

        tx.commit()
            .await
            .context("commit postgres usage rollup batch transaction")?;

        tracing::debug!(
            requested_batch_count = batches.len(),
            applied_batch_count = report.applied_batch_count,
            already_applied_batch_count = report.already_applied_batch_count,
            delta_count = report.delta_count,
            missing_key_delta_count = report.missing_key_delta_count,
            affected_rollup_rows = affected_rows,
            "applied postgres usage rollup batches"
        );

        let key_ids = deltas
            .iter()
            .map(|delta| delta.key_id.clone())
            .collect::<Vec<_>>();
        self.invalidate_authenticated_key_cache_by_ids(&key_ids)
            .await;
        Ok(report)
    }
}

fn usage_rollup_batch_digest(batch: &UsageRollupBatch) -> anyhow::Result<String> {
    let bytes = encode_rollup_batch_v1(batch).context("encode usage rollup batch v1 digest")?;
    Ok(sha256_hex(&bytes))
}

#[derive(Serialize)]
struct LegacyJsonUsageRollupBatch<'a> {
    batch_id: &'a str,
    source_node_id: &'a Option<String>,
    created_at_ms: i64,
    source_event_count: u64,
    deltas: &'a [KeyUsageRollupDelta],
}

fn legacy_json_usage_rollup_batch_digest(batch: &UsageRollupBatch) -> anyhow::Result<String> {
    let legacy = LegacyJsonUsageRollupBatch {
        batch_id: &batch.batch_id,
        source_node_id: &batch.source_node_id,
        created_at_ms: batch.created_at_ms,
        source_event_count: batch.source_event_count,
        deltas: &batch.deltas,
    };
    let bytes =
        serde_json::to_vec(&legacy).context("encode legacy json usage rollup batch digest")?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
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

#[async_trait]
impl UsageRollupBatchSink for PostgresControlRepository {
    async fn apply_usage_rollup_batches(
        &self,
        batches: &[UsageRollupBatch],
    ) -> anyhow::Result<UsageRollupApplyReport> {
        self.apply_usage_rollup_batches_impl(batches).await
    }

    async fn prune_usage_rollup_batch_markers(
        &self,
        applied_before_ms: i64,
    ) -> anyhow::Result<u64> {
        self.ensure_connection_alive()?;
        let deleted = query(
            "DELETE FROM llm_key_usage_rollup_applied_batches
             WHERE applied_at_ms < $1",
        )
        .bind(applied_before_ms)
        .execute(&self.client.pool)
        .await
        .context("prune postgres usage rollup applied batch markers")?
        .rows_affected();
        Ok(deleted)
    }
}
