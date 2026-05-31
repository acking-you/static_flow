//! API-key bundle reads/pagination/summaries + the `AdminKeyStore` impl.

use std::collections::BTreeMap;

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    self as core_store, AdminKey, AdminKeyPageQuery, AdminKeyPatch, AdminKeySortMode,
    AdminKeyStore, AdminKeysPage, AdminPageRequest, AuthenticatedKey, NewAdminKey,
};

use super::{
    decode::{admin_key_from_bundle, decode_key_bundle_row, decode_kiro_admin_key_row},
    PostgresControlRepository, SqlxClient,
};
use crate::records::{KeyBundle, KeyRecord, KeyRouteConfig, KeyUsageRollup};

impl PostgresControlRepository {
    pub(super) async fn load_authenticated_key_by_hash(
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

    pub(super) async fn load_key_hashes_by_ids(
        &self,
        key_ids: &[String],
    ) -> anyhow::Result<BTreeMap<String, String>> {
        if key_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT key_id, key_hash
                 FROM llm_keys
                 WHERE key_id = ANY($1)",
                &[&key_ids],
            )
            .await
            .context("load key hashes by ids")?;
        Ok(rows
            .into_iter()
            .map(|row| (row.get::<_, String>(0), row.get::<_, String>(1)))
            .collect())
    }

    pub(super) async fn load_key_bundle_by_id(
        &self,
        key_id: &str,
    ) -> anyhow::Result<Option<KeyBundle>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, 0),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_id = $1",
                &[&key_id],
            )
            .await
            .context("load key bundle by id")?;
        row.map(decode_key_bundle_row).transpose()
    }

    async fn list_key_bundles(&self) -> anyhow::Result<Vec<KeyBundle>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 ORDER BY k.created_at_ms DESC, k.key_id DESC",
                &[],
            )
            .await
            .context("list key bundles")?;
        rows.into_iter()
            .map(decode_key_bundle_row)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    async fn admin_keys_summary(
        &self,
        provider_type: Option<&str>,
    ) -> anyhow::Result<core_store::AdminKeysSummary> {
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let row = self
            .client
            .query_one(
                "SELECT
                    COUNT(*)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.public_visible THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.status = 'active' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(CASE WHEN k.status = 'disabled' THEN 1 ELSE 0 END), 0)::BIGINT,
                    COALESCE(SUM(k.quota_billable_limit), 0)::BIGINT,
                    COALESCE(SUM(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)), \
                 0)::BIGINT,
                    COALESCE(SUM(u.input_uncached_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.input_cached_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.output_tokens), 0)::BIGINT,
                    COALESCE(SUM(u.billable_tokens), 0)::BIGINT,
                    COALESCE(SUM((u.credit_total)::DOUBLE PRECISION), 0)::DOUBLE PRECISION,
                    COALESCE(SUM(u.credit_missing_events), 0)::BIGINT
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE ($1::text IS NULL OR k.provider_type = $1)",
                &[&provider],
            )
            .await
            .context("summarize postgres key bundles")?;
        Ok(core_store::AdminKeysSummary {
            total: row.get::<_, i64>(0).max(0) as usize,
            public_visible_count: row.get::<_, i64>(1).max(0) as usize,
            active_count: row.get::<_, i64>(2).max(0) as usize,
            disabled_count: row.get::<_, i64>(3).max(0) as usize,
            quota_billable_limit_sum: row.get::<_, i64>(4).max(0) as u64,
            remaining_billable_sum: row.get(5),
            usage_input_uncached_tokens_sum: row.get::<_, i64>(6).max(0) as u64,
            usage_input_cached_tokens_sum: row.get::<_, i64>(7).max(0) as u64,
            usage_output_tokens_sum: row.get::<_, i64>(8).max(0) as u64,
            usage_billable_tokens_sum: row.get::<_, i64>(9).max(0) as u64,
            usage_credit_total: row.get(10),
            usage_credit_missing_events: row.get::<_, i64>(11).max(0) as u64,
        })
    }

    async fn list_key_bundles_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        if provider_type == Some(core_store::PROVIDER_KIRO) {
            return self
                .list_kiro_key_bundles_page_with_candidate_summaries(page)
                .await;
        }
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let total = self
            .client
            .query_one(
                "SELECT COUNT(*)
                 FROM llm_keys k
                 WHERE ($1::text IS NULL OR k.provider_type = $1)",
                &[&provider],
            )
            .await
            .context("count postgres key bundles page")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let limit = page.limit.max(1);
        let offset = page.offset;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE ($1::text IS NULL OR k.provider_type = $1)
                 ORDER BY k.created_at_ms DESC, k.key_id DESC
                 LIMIT $2 OFFSET $3",
                &[&provider, &(limit as i64), &(offset as i64)],
            )
            .await
            .context("list postgres key bundles page")?;
        let keys = rows
            .into_iter()
            .map(decode_key_bundle_row)
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let summary = self.admin_keys_summary(provider_type).await?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit,
            offset,
        })
    }

    async fn list_kiro_key_bundles_page_with_candidate_summaries(
        &self,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        self.ensure_connection_alive()?;
        let summary = self
            .admin_keys_summary(Some(core_store::PROVIDER_KIRO))
            .await?;
        let total = summary.total;
        let limit = page.limit.max(1);
        let offset = page.offset;
        let provider = core_store::PROVIDER_KIRO.to_string();
        let rows = self
            .client
            .query(
                "WITH page_keys AS (
                    SELECT
                        k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                        k.protocol_family, k.public_visible, k.quota_billable_limit,
                        k.created_at_ms, k.updated_at_ms,
                        r.route_strategy, r.fixed_account_name, r.auto_account_names_json,
                        r.account_group_id, r.model_name_map_json,
                        r.request_max_concurrency, r.request_min_start_interval_ms,
                        r.codex_fast_enabled, r.kiro_request_validation_enabled,
                        r.kiro_cache_estimation_enabled,
                        r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                        r.kiro_remote_media_resolution_enabled,
                        r.kiro_latency_routing_enabled,
                        r.kiro_cache_policy_override_json,
                        r.kiro_billable_model_multipliers_override_json,
                        COALESCE(u.input_uncached_tokens, 0) AS input_uncached_tokens,
                        COALESCE(u.input_cached_tokens, 0) AS input_cached_tokens,
                        COALESCE(u.output_tokens, 0) AS output_tokens,
                        COALESCE(u.billable_tokens, 0) AS billable_tokens,
                        COALESCE(u.credit_total, '0') AS credit_total,
                        COALESCE(u.credit_missing_events, 0) AS credit_missing_events,
                        u.last_used_at_ms,
                        COALESCE(u.updated_at_ms, k.updated_at_ms) AS rollup_updated_at_ms,
                        g.account_names_json AS group_account_names_json,
                        COALESCE(NULLIF(r.route_strategy, ''), 'auto') AS route_strategy_norm
                    FROM llm_keys k
                    LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                    LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                    LEFT JOIN llm_account_groups g
                        ON g.group_id = r.account_group_id
                       AND g.provider_type = $1
                    WHERE k.provider_type = $1
                    ORDER BY k.created_at_ms DESC, k.key_id DESC
                    LIMIT $2 OFFSET $3
                 ),
                 all_accounts AS (
                    SELECT account_name
                    FROM llm_kiro_accounts
                 ),
                 group_candidates AS (
                    SELECT
                        page_keys.key_id,
                        candidate.account_name AS account_name
                    FROM page_keys
                    CROSS JOIN LATERAL jsonb_array_elements_text(
                        COALESCE(page_keys.group_account_names_json, '[]'::jsonb)
                    ) AS candidate(account_name)
                    WHERE page_keys.route_strategy_norm IN ('auto', 'fixed')
                      AND page_keys.group_account_names_json IS NOT NULL
                 ),
                 fixed_candidates AS (
                    SELECT
                        page_keys.key_id,
                        page_keys.fixed_account_name AS account_name
                    FROM page_keys
                    WHERE page_keys.route_strategy_norm = 'fixed'
                      AND page_keys.group_account_names_json IS NULL
                 ),
                 auto_named_candidates AS (
                    SELECT
                        page_keys.key_id,
                        candidate.account_name AS account_name
                    FROM page_keys
                    CROSS JOIN LATERAL jsonb_array_elements_text(
                        COALESCE(page_keys.auto_account_names_json, '[]'::jsonb)
                    ) AS candidate(account_name)
                    WHERE page_keys.route_strategy_norm = 'auto'
                      AND page_keys.group_account_names_json IS NULL
                      AND COALESCE(
                            jsonb_typeof(page_keys.auto_account_names_json) = 'array'
                            AND jsonb_array_length(page_keys.auto_account_names_json) > 0,
                            FALSE
                        )
                 ),
                 all_auto_candidates AS (
                    SELECT
                        page_keys.key_id,
                        all_accounts.account_name
                    FROM page_keys
                    CROSS JOIN all_accounts
                    WHERE page_keys.route_strategy_norm = 'auto'
                      AND page_keys.group_account_names_json IS NULL
                      AND NOT COALESCE(
                            jsonb_typeof(page_keys.auto_account_names_json) = 'array'
                            AND jsonb_array_length(page_keys.auto_account_names_json) > 0,
                            FALSE
                        )
                 ),
                 key_candidate_names AS (
                    SELECT key_id, account_name FROM group_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM fixed_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM auto_named_candidates
                    UNION ALL
                    SELECT key_id, account_name FROM all_auto_candidates
                 ),
                 valid_key_candidates AS (
                    SELECT DISTINCT candidates.key_id, accounts.account_name
                    FROM key_candidate_names candidates
                    JOIN llm_kiro_accounts accounts
                      ON accounts.account_name = candidates.account_name
                    WHERE candidates.account_name IS NOT NULL
                 ),
                 key_candidate_summary AS (
                    SELECT
                        candidates.key_id,
                        COUNT(*)::BIGINT AS candidate_count,
                        COUNT(*) FILTER (
                            WHERE status.balance_json IS NOT NULL
                              AND status.balance_json <> 'null'::jsonb
                        )::BIGINT AS loaded_balance_count,
                        COUNT(*) FILTER (
                            WHERE status.balance_json IS NULL
                              OR status.balance_json = 'null'::jsonb
                        )::BIGINT AS missing_balance_count,
                        COALESCE(SUM(
                            CASE
                                WHEN status.balance_json IS NOT NULL
                                 AND status.balance_json <> 'null'::jsonb
                                THEN GREATEST(
                                    COALESCE(
                                        (status.balance_json ->> 'usage_limit')::double precision,
                                        0.0
                                    ),
                                    0.0
                                )
                                ELSE 0.0
                            END
                        ), 0.0) AS total_limit,
                        COALESCE(SUM(
                            CASE
                                WHEN status.balance_json IS NOT NULL
                                 AND status.balance_json <> 'null'::jsonb
                                THEN GREATEST(
                                    COALESCE(
                                        (status.balance_json ->> 'remaining')::double precision,
                                        0.0
                                    ),
                                    0.0
                                )
                                ELSE 0.0
                            END
                        ), 0.0) AS total_remaining
                    FROM valid_key_candidates candidates
                    LEFT JOIN llm_kiro_status_cache status
                      ON status.account_name = candidates.account_name
                    GROUP BY candidates.key_id
                 )
                 SELECT
                    page_keys.key_id, page_keys.name, page_keys.secret, page_keys.key_hash,
                    page_keys.status, page_keys.provider_type, page_keys.protocol_family,
                    page_keys.public_visible, page_keys.quota_billable_limit,
                    page_keys.created_at_ms, page_keys.updated_at_ms,
                    page_keys.route_strategy, page_keys.fixed_account_name,
                    page_keys.auto_account_names_json::text,
                    page_keys.account_group_id, page_keys.model_name_map_json::text,
                    page_keys.request_max_concurrency, page_keys.request_min_start_interval_ms,
                    page_keys.codex_fast_enabled, page_keys.kiro_request_validation_enabled,
                    page_keys.kiro_cache_estimation_enabled,
                    page_keys.kiro_zero_cache_debug_enabled,
                    page_keys.kiro_full_request_logging_enabled,
                    page_keys.kiro_remote_media_resolution_enabled,
                    page_keys.kiro_cache_policy_override_json::text,
                    page_keys.kiro_billable_model_multipliers_override_json::text,
                    page_keys.input_uncached_tokens,
                    page_keys.input_cached_tokens,
                    page_keys.output_tokens,
                    page_keys.billable_tokens,
                    page_keys.credit_total,
                    page_keys.credit_missing_events,
                    page_keys.last_used_at_ms,
                    page_keys.rollup_updated_at_ms,
                    COALESCE(summary.candidate_count, 0),
                    COALESCE(summary.loaded_balance_count, 0),
                    COALESCE(summary.missing_balance_count, 0),
                    COALESCE(summary.total_limit, 0.0),
                    COALESCE(summary.total_remaining, 0.0),
                    page_keys.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM page_keys
                 LEFT JOIN key_candidate_summary summary
                   ON summary.key_id = page_keys.key_id
                 ORDER BY page_keys.created_at_ms DESC, page_keys.key_id DESC",
                &[&provider, &(limit as i64), &(offset as i64)],
            )
            .await
            .context("list postgres kiro key bundles page with candidate summaries")?;
        let keys = rows
            .into_iter()
            .map(decode_kiro_admin_key_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit,
            offset,
        })
    }

    async fn find_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                    r.account_group_id, r.model_name_map_json::text,
                    r.request_max_concurrency, r.request_min_start_interval_ms,
                    r.codex_fast_enabled, r.kiro_request_validation_enabled,
                    r.kiro_cache_estimation_enabled,
                    r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                    r.kiro_remote_media_resolution_enabled,
                    r.kiro_cache_policy_override_json::text,
                    r.kiro_billable_model_multipliers_override_json::text,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms,
                    COALESCE(u.updated_at_ms, k.updated_at_ms),
                    r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
                 FROM llm_keys k
                 JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.provider_type = $1 AND r.account_group_id = $2
                 ORDER BY k.created_at_ms DESC, k.key_id DESC
                 LIMIT 1",
                &[&provider_type, &group_id],
            )
            .await
            .context("find postgres key referencing account group")?;
        row.map(decode_key_bundle_row)
            .transpose()
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
    }

    async fn upsert_key_bundle_client(
        client: &SqlxClient,
        key: &KeyRecord,
        route: &KeyRouteConfig,
        rollup: &KeyUsageRollup,
    ) -> anyhow::Result<()> {
        client
            .execute(
                "INSERT INTO llm_keys (
                    key_id, name, secret, key_hash, status, provider_type, protocol_family,
                    public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT(key_id) DO UPDATE SET
                    name = EXCLUDED.name,
                    secret = EXCLUDED.secret,
                    key_hash = EXCLUDED.key_hash,
                    status = EXCLUDED.status,
                    provider_type = EXCLUDED.provider_type,
                    protocol_family = EXCLUDED.protocol_family,
                    public_visible = EXCLUDED.public_visible,
                    quota_billable_limit = EXCLUDED.quota_billable_limit,
                    created_at_ms = EXCLUDED.created_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &key.key_id,
                    &key.name,
                    &key.secret,
                    &key.key_hash,
                    &key.status,
                    &key.provider_type,
                    &key.protocol_family,
                    &key.public_visible,
                    &key.quota_billable_limit,
                    &key.created_at_ms,
                    &key.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres llm key")?;
        client
            .execute(
                "INSERT INTO llm_key_route_config (
                    key_id, route_strategy, fixed_account_name, auto_account_names_json,
                    account_group_id, model_name_map_json, request_max_concurrency,
                    request_min_start_interval_ms, codex_fast_enabled,
                    kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                    kiro_remote_media_resolution_enabled, kiro_latency_routing_enabled,
                    kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json
                 ) VALUES (
                    $1, $2, $3, $4::jsonb, $5, $6::jsonb, $7, $8, $9, $10, $11, $12,
                    $13, $14, $15, $16::jsonb, $17::jsonb
                 )
                 ON CONFLICT(key_id) DO UPDATE SET
                    route_strategy = EXCLUDED.route_strategy,
                    fixed_account_name = EXCLUDED.fixed_account_name,
                    auto_account_names_json = EXCLUDED.auto_account_names_json,
                    account_group_id = EXCLUDED.account_group_id,
                    model_name_map_json = EXCLUDED.model_name_map_json,
                    request_max_concurrency = EXCLUDED.request_max_concurrency,
                    request_min_start_interval_ms = EXCLUDED.request_min_start_interval_ms,
                    codex_fast_enabled = EXCLUDED.codex_fast_enabled,
                    kiro_request_validation_enabled = EXCLUDED.kiro_request_validation_enabled,
                    kiro_cache_estimation_enabled = EXCLUDED.kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled = EXCLUDED.kiro_zero_cache_debug_enabled,
                    kiro_full_request_logging_enabled =
                        EXCLUDED.kiro_full_request_logging_enabled,
                    kiro_remote_media_resolution_enabled =
                        EXCLUDED.kiro_remote_media_resolution_enabled,
                    kiro_latency_routing_enabled =
                        EXCLUDED.kiro_latency_routing_enabled,
                    kiro_cache_policy_override_json =
                        EXCLUDED.kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json =
                        EXCLUDED.kiro_billable_model_multipliers_override_json",
                &[
                    &route.key_id,
                    &route.route_strategy,
                    &route.fixed_account_name,
                    &route.auto_account_names_json,
                    &route.account_group_id,
                    &route.model_name_map_json,
                    &route.request_max_concurrency,
                    &route.request_min_start_interval_ms,
                    &route.codex_fast_enabled,
                    &route.kiro_request_validation_enabled,
                    &route.kiro_cache_estimation_enabled,
                    &route.kiro_zero_cache_debug_enabled,
                    &route.kiro_full_request_logging_enabled,
                    &route.kiro_remote_media_resolution_enabled,
                    &route.kiro_latency_routing_enabled,
                    &route.kiro_cache_policy_override_json,
                    &route.kiro_billable_model_multipliers_override_json,
                ],
            )
            .await
            .context("upsert postgres key route config")?;
        client
            .execute(
                "INSERT INTO llm_key_usage_rollups (
                    key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                    billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                    updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT(key_id) DO UPDATE SET
                    input_uncached_tokens = EXCLUDED.input_uncached_tokens,
                    input_cached_tokens = EXCLUDED.input_cached_tokens,
                    output_tokens = EXCLUDED.output_tokens,
                    billable_tokens = EXCLUDED.billable_tokens,
                    credit_total = EXCLUDED.credit_total,
                    credit_missing_events = EXCLUDED.credit_missing_events,
                    last_used_at_ms = EXCLUDED.last_used_at_ms,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &rollup.key_id,
                    &rollup.input_uncached_tokens,
                    &rollup.input_cached_tokens,
                    &rollup.output_tokens,
                    &rollup.billable_tokens,
                    &rollup.credit_total.to_string(),
                    &rollup.credit_missing_events,
                    &rollup.last_used_at_ms,
                    &rollup.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres key usage rollup")?;
        Ok(())
    }

    pub(super) async fn disable_admin_key_if_present(
        &self,
        key_id: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        // `patch_admin_key` already returns `Ok(None)` for a missing key without
        // side effects, so the prior `load_key_bundle_by_id` presence check was a
        // redundant round-trip. Discard the returned row as before.
        self.patch_admin_key(key_id, AdminKeyPatch {
            status: Some(core_store::KEY_STATUS_DISABLED.to_string()),
            updated_at_ms,
            ..AdminKeyPatch::default()
        })
        .await?;
        Ok(())
    }
}
#[async_trait]
impl AdminKeyStore for PostgresControlRepository {
    async fn list_admin_keys(&self) -> anyhow::Result<Vec<AdminKey>> {
        Ok(self
            .list_key_bundles()
            .await?
            .iter()
            .map(admin_key_from_bundle)
            .collect())
    }

    async fn get_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        Ok(self
            .load_key_bundle_by_id(key_id)
            .await?
            .map(|bundle| admin_key_from_bundle(&bundle)))
    }

    async fn list_admin_keys_page(
        &self,
        provider_type: Option<&str>,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        self.list_key_bundles_page(provider_type, page).await
    }

    async fn list_admin_keys_filtered_page(
        &self,
        provider_type: Option<&str>,
        query: &AdminKeyPageQuery,
        page: AdminPageRequest,
    ) -> anyhow::Result<AdminKeysPage> {
        if provider_type == Some(core_store::PROVIDER_KIRO)
            && query == &AdminKeyPageQuery::default()
        {
            return self
                .list_kiro_key_bundles_page_with_candidate_summaries(page)
                .await;
        }
        self.ensure_connection_alive()?;
        let provider = provider_type.map(str::to_string);
        let search = query
            .search
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("%{}%", value.to_ascii_lowercase()));
        let total = self
            .client
            .query_one(
                "SELECT COUNT(*)
                 FROM llm_keys k
                 WHERE ($1::text IS NULL OR k.provider_type = $1)
                   AND ($2::text IS NULL
                        OR lower(k.key_id) LIKE $2
                        OR lower(k.name) LIKE $2
                        OR lower(k.provider_type) LIKE $2
                        OR lower(k.status) LIKE $2)
                   AND ($3::boolean = FALSE OR k.status <> 'disabled')",
                &[&provider, &search, &query.active_only],
            )
            .await
            .context("count postgres filtered key bundles page")?
            .get::<_, i64>(0)
            .max(0) as usize;
        let order_by = match query.sort {
            AdminKeySortMode::Newest => "k.created_at_ms DESC, k.key_id DESC",
            AdminKeySortMode::QuotaAsc => {
                "(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)) ASC, k.created_at_ms \
                 DESC, k.key_id DESC"
            },
            AdminKeySortMode::QuotaDesc => {
                "(k.quota_billable_limit - COALESCE(u.billable_tokens, 0)) DESC, k.created_at_ms \
                 DESC, k.key_id DESC"
            },
            AdminKeySortMode::UsageAsc => {
                "COALESCE(u.credit_total, '0') ASC, k.created_at_ms DESC, k.key_id DESC"
            },
            AdminKeySortMode::UsageDesc => {
                "COALESCE(u.credit_total, '0') DESC, k.created_at_ms DESC, k.key_id DESC"
            },
        };
        let sql = format!(
            "SELECT
                k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                k.protocol_family, k.public_visible, k.quota_billable_limit,
                k.created_at_ms, k.updated_at_ms,
                r.route_strategy, r.fixed_account_name, r.auto_account_names_json::text,
                r.account_group_id, r.model_name_map_json::text,
                r.request_max_concurrency, r.request_min_start_interval_ms,
                r.codex_fast_enabled, r.kiro_request_validation_enabled,
                r.kiro_cache_estimation_enabled,
                r.kiro_zero_cache_debug_enabled, r.kiro_full_request_logging_enabled,
                r.kiro_remote_media_resolution_enabled,
                r.kiro_cache_policy_override_json::text,
                r.kiro_billable_model_multipliers_override_json::text,
                COALESCE(u.input_uncached_tokens, 0),
                COALESCE(u.input_cached_tokens, 0),
                COALESCE(u.output_tokens, 0),
                COALESCE(u.billable_tokens, 0),
                COALESCE(u.credit_total, '0'),
                COALESCE(u.credit_missing_events, 0),
                u.last_used_at_ms,
                COALESCE(u.updated_at_ms, k.updated_at_ms),
                r.kiro_latency_routing_enabled AS kiro_latency_routing_enabled
             FROM llm_keys k
             LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
             LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
             WHERE ($1::text IS NULL OR k.provider_type = $1)
               AND ($2::text IS NULL
                    OR lower(k.key_id) LIKE $2
                    OR lower(k.name) LIKE $2
                    OR lower(k.provider_type) LIKE $2
                    OR lower(k.status) LIKE $2)
               AND ($3::boolean = FALSE OR k.status <> 'disabled')
             ORDER BY {order_by}
             LIMIT $4 OFFSET $5"
        );
        let rows = self
            .client
            .query(sql.as_str(), &[
                &provider,
                &search,
                &query.active_only,
                &(page.limit.max(1) as i64),
                &(page.offset as i64),
            ])
            .await
            .context("list postgres filtered key bundles page")?;
        let keys = rows
            .into_iter()
            .map(decode_key_bundle_row)
            .map(|bundle| bundle.map(|bundle| admin_key_from_bundle(&bundle)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let summary = self.admin_keys_summary(provider_type).await?;
        Ok(AdminKeysPage {
            has_more: page.has_more(keys.len(), total),
            keys,
            summary,
            total,
            limit: page.limit.max(1),
            offset: page.offset,
        })
    }

    async fn find_admin_key_referencing_account_group(
        &self,
        provider_type: &str,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminKey>> {
        self.find_key_referencing_account_group(provider_type, group_id)
            .await
    }

    async fn create_admin_key(&self, key: NewAdminKey) -> anyhow::Result<AdminKey> {
        let key_record = KeyRecord {
            key_id: key.id.clone(),
            name: key.name.clone(),
            secret: key.secret.clone(),
            key_hash: key.key_hash.clone(),
            status: core_store::KEY_STATUS_ACTIVE.to_string(),
            provider_type: key.provider_type.clone(),
            protocol_family: key.protocol_family.clone(),
            public_visible: key.public_visible,
            quota_billable_limit: key.quota_billable_limit as i64,
            created_at_ms: key.created_at_ms,
            updated_at_ms: key.created_at_ms,
        };
        let route = KeyRouteConfig {
            key_id: key.id.clone(),
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names_json: None,
            account_group_id: None,
            model_name_map_json: None,
            request_max_concurrency: key.request_max_concurrency.map(|value| value as i64),
            request_min_start_interval_ms: key
                .request_min_start_interval_ms
                .map(|value| value as i64),
            codex_fast_enabled: true,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_zero_cache_debug_enabled: false,
            kiro_full_request_logging_enabled: false,
            kiro_remote_media_resolution_enabled: false,
            kiro_latency_routing_enabled: true,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        };
        let rollup = KeyUsageRollup {
            key_id: key.id.clone(),
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            credit_total: 0.0,
            credit_missing_events: 0,
            last_used_at_ms: None,
            updated_at_ms: key.created_at_ms,
        };
        Self::upsert_key_bundle_client(&self.client, &key_record, &route, &rollup).await?;
        self.bump_dispatch_generation(&key.provider_type).await;
        self.load_key_bundle_by_id(&key.id)
            .await?
            .map(|bundle| admin_key_from_bundle(&bundle))
            .context("created postgres admin key disappeared")
    }

    async fn patch_admin_key(
        &self,
        key_id: &str,
        patch: AdminKeyPatch,
    ) -> anyhow::Result<Option<AdminKey>> {
        let Some(mut bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            bundle.key.name = name.clone();
        }
        if let Some(status) = patch.status.as_ref() {
            bundle.key.status = status.clone();
        }
        if let Some(public_visible) = patch.public_visible {
            bundle.key.public_visible = public_visible;
        }
        if let Some(limit) = patch.quota_billable_limit {
            bundle.key.quota_billable_limit = limit as i64;
        }
        if let Some(value) = patch.route_strategy.as_ref() {
            bundle.route.route_strategy = value.clone();
        }
        if let Some(value) = patch.account_group_id.as_ref() {
            bundle.route.account_group_id = value.clone();
        }
        if let Some(value) = patch.fixed_account_name.as_ref() {
            bundle.route.fixed_account_name = value.clone();
        }
        if let Some(value) = patch.auto_account_names.as_ref() {
            bundle.route.auto_account_names_json = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("serialize postgres auto account names")?;
        }
        if let Some(value) = patch.model_name_map.as_ref() {
            bundle.route.model_name_map_json = value
                .as_ref()
                .map(serde_json::to_string)
                .transpose()
                .context("serialize postgres model name map")?;
        }
        if let Some(value) = patch.request_max_concurrency {
            bundle.route.request_max_concurrency = value.map(|value| value as i64);
        }
        if let Some(value) = patch.request_min_start_interval_ms {
            bundle.route.request_min_start_interval_ms = value.map(|value| value as i64);
        }
        if let Some(value) = patch.codex_fast_enabled {
            bundle.route.codex_fast_enabled = value;
        }
        if let Some(value) = patch.kiro_request_validation_enabled {
            bundle.route.kiro_request_validation_enabled = value;
        }
        if let Some(value) = patch.kiro_cache_estimation_enabled {
            bundle.route.kiro_cache_estimation_enabled = value;
        }
        if let Some(value) = patch.kiro_zero_cache_debug_enabled {
            bundle.route.kiro_zero_cache_debug_enabled = value;
        }
        if let Some(value) = patch.kiro_full_request_logging_enabled {
            bundle.route.kiro_full_request_logging_enabled = value;
        }
        if let Some(value) = patch.kiro_remote_media_resolution_enabled {
            bundle.route.kiro_remote_media_resolution_enabled = value;
        }
        if let Some(value) = patch.kiro_latency_routing_enabled {
            bundle.route.kiro_latency_routing_enabled = value;
        }
        if let Some(value) = patch.kiro_cache_policy_override_json.as_ref() {
            bundle.route.kiro_cache_policy_override_json = value.clone();
        }
        if let Some(value) = patch.kiro_billable_model_multipliers_override_json.as_ref() {
            bundle.route.kiro_billable_model_multipliers_override_json = value.clone();
        }
        bundle.key.updated_at_ms = patch.updated_at_ms;
        bundle.rollup.updated_at_ms = bundle.rollup.updated_at_ms.max(patch.updated_at_ms);
        Self::upsert_key_bundle_client(&self.client, &bundle.key, &bundle.route, &bundle.rollup)
            .await?;
        self.invalidate_authenticated_key_cache_by_ids(std::slice::from_ref(&bundle.key.key_id))
            .await;
        self.invalidate_request_snapshot_cache(&bundle.key.provider_type, &bundle.key.key_id)
            .await;
        self.bump_dispatch_generation(&bundle.key.provider_type)
            .await;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }

    async fn delete_admin_key(&self, key_id: &str) -> anyhow::Result<Option<AdminKey>> {
        let Some(bundle) = self.load_key_bundle_by_id(key_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_keys WHERE key_id = $1", &[&key_id])
            .await
            .context("delete postgres admin key")?;
        self.invalidate_authenticated_key_cache_by_ids(std::slice::from_ref(&bundle.key.key_id))
            .await;
        self.invalidate_request_snapshot_cache(&bundle.key.provider_type, &bundle.key.key_id)
            .await;
        self.bump_dispatch_generation(&bundle.key.provider_type)
            .await;
        Ok(Some(admin_key_from_bundle(&bundle)))
    }
}
