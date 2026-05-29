//! Runtime configuration and connection management: pooled/fresh client
//! connect paths, runtime-config row load/store (cached), liveness checks, the
//! generic row-count helper, plus the `AdminConfigStore`/`ControlStore` impls.

use async_trait::async_trait;

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

#[async_trait]
impl AdminConfigStore for PostgresControlRepository {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        let record = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        Ok(record.to_admin_runtime_config())
    }

    async fn update_admin_runtime_config(
        &self,
        config: AdminRuntimeConfig,
    ) -> anyhow::Result<AdminRuntimeConfig> {
        let mut record = self
            .load_runtime_config_record_cached()
            .await?
            .unwrap_or_default();
        record.apply_admin_runtime_config(&config);
        self.upsert_runtime_config_record(&record).await?;
        Ok(record.to_admin_runtime_config())
    }
}
#[async_trait]
impl ControlStore for PostgresControlRepository {
    async fn authenticate_bearer_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        self.load_authenticated_key_cached(&hash_bearer_secret(secret))
            .await
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollups_batch(std::slice::from_ref(event))
            .await
    }
}
impl PostgresControlRepository {
    /// Connect to the Postgres control plane and run pending migrations.
    pub async fn connect(
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
    ) -> anyhow::Result<Self> {
        Self::connect_with_proxy_scope(database_url, request_cache_config, ProxyConfigScope::core())
            .await
    }
    /// Connect to the Postgres control plane with an explicit proxy resolution
    /// scope.
    pub async fn connect_with_proxy_scope(
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
        proxy_scope: ProxyConfigScope,
    ) -> anyhow::Result<Self> {
        let client = SqlxClient::connect(database_url).await?;
        llm_access_migrations::run_postgres_migrations(&client.pool).await?;
        let request_cache = request_cache_config.map(RequestCache::new).transpose()?;
        Ok(Self {
            client,
            codex_status_cache: Arc::new(RwLock::new(None)),
            request_cache,
            proxy_scope,
        })
    }
    pub(crate) async fn connect_fresh_client(&self) -> anyhow::Result<SqlxClient> {
        Ok(self.client.clone())
    }
    pub(crate) async fn load_runtime_config_record(
        &self,
    ) -> anyhow::Result<Option<RuntimeConfigRecord>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    id,
                    auth_cache_ttl_seconds,
                    max_request_body_bytes,
                    account_failure_retry_limit,
                    codex_client_version,
                    kiro_channel_max_concurrency,
                    kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    codex_weight_free,
                    codex_weight_plus,
                    codex_weight_pro5x,
                    codex_weight_pro20x,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib,
                    usage_analytics_retention_days,
                    usage_journal_enabled,
                    usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms,
                    usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events,
                    usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level,
                    usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files,
                    usage_query_bind_addr,
                    usage_query_base_url,
                    usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days,
                    kiro_cache_kmodels_json::text AS kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json::text
                        AS kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json::text AS kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms
                 FROM llm_runtime_config
                 WHERE id = 'default'",
                &[],
            )
            .await
            .context("load runtime config")?;
        row.map(decode_runtime_config_row).transpose()
    }
    pub(crate) async fn load_runtime_config_record_cached(
        &self,
    ) -> anyhow::Result<Option<RuntimeConfigRecord>> {
        let Some(cache) = self.request_cache.as_ref() else {
            return self.load_runtime_config_record().await;
        };
        let cache_key = cache.runtime_config_key();
        match cache
            .get_json::<crate::request_cache::CachedRuntimeConfigLookup>(&cache_key)
            .await
        {
            Ok(Some(lookup)) => return Ok(lookup.record),
            Ok(None) => {},
            Err(err) => {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache runtime-config read failed; falling back to postgres"
                );
            },
        }
        let record = self.load_runtime_config_record().await?;
        let lookup = crate::request_cache::CachedRuntimeConfigLookup {
            record: record.clone(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.runtime_config_ttl())
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache runtime-config write failed"
            );
        }
        Ok(record)
    }
    pub(crate) async fn store_runtime_config_record_cached(
        &self,
        record: Option<&RuntimeConfigRecord>,
    ) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let cache_key = cache.runtime_config_key();
        let lookup = crate::request_cache::CachedRuntimeConfigLookup {
            record: record.cloned(),
        };
        if let Err(err) = cache
            .set_json(&cache_key, &lookup, cache.runtime_config_ttl())
            .await
        {
            tracing::warn!(
                key = %cache_key,
                error = %err,
                "request cache runtime-config write-through failed"
            );
        }
    }
    pub(crate) fn ensure_connection_alive(&self) -> anyhow::Result<()> {
        if self.client.is_closed() {
            anyhow::bail!("sqlx postgres control pool is closed");
        }
        Ok(())
    }
    pub(crate) async fn count_rows(
        &self,
        count_all_sql: &str,
        count_status_sql: &str,
        status: Option<&str>,
    ) -> anyhow::Result<usize> {
        self.ensure_connection_alive()?;
        let count: i64 = if let Some(status) = status {
            self.client
                .query_one(count_status_sql, &[&status])
                .await
                .context("count filtered rows")?
                .get(0)
        } else {
            self.client
                .query_one(count_all_sql, &[])
                .await
                .context("count rows")?
                .get(0)
        };
        Ok(count.max(0) as usize)
    }
    pub(crate) async fn upsert_runtime_config_record(
        &self,
        record: &RuntimeConfigRecord,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_runtime_config (
                    id, auth_cache_ttl_seconds, max_request_body_bytes,
                    account_failure_retry_limit, codex_client_version,
                    kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    codex_weight_free, codex_weight_plus, codex_weight_pro5x,
                    codex_weight_pro20x, kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size, usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes, duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib, usage_analytics_retention_days,
                    usage_journal_enabled, usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms, usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events, usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level, usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files, usage_query_bind_addr,
                    usage_query_base_url, usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days, kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json, kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode, kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds, updated_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                    $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24,
                    $25, $26, $27, $28, $29, $30, $31, $32, $33, $34, $35,
                    $36, $37, $38, $39::jsonb, $40::jsonb, $41::jsonb, $42,
                    $43, $44, $45, $46, $47, $48
                )
                ON CONFLICT(id) DO UPDATE SET
                    auth_cache_ttl_seconds = EXCLUDED.auth_cache_ttl_seconds,
                    max_request_body_bytes = EXCLUDED.max_request_body_bytes,
                    account_failure_retry_limit = EXCLUDED.account_failure_retry_limit,
                    codex_client_version = EXCLUDED.codex_client_version,
                    kiro_channel_max_concurrency = EXCLUDED.kiro_channel_max_concurrency,
                    kiro_channel_min_start_interval_ms =
                        EXCLUDED.kiro_channel_min_start_interval_ms,
                    codex_status_refresh_min_interval_seconds =
                        EXCLUDED.codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds =
                        EXCLUDED.codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds =
                        EXCLUDED.codex_status_account_jitter_max_seconds,
                    codex_weight_free = EXCLUDED.codex_weight_free,
                    codex_weight_plus = EXCLUDED.codex_weight_plus,
                    codex_weight_pro5x = EXCLUDED.codex_weight_pro5x,
                    codex_weight_pro20x = EXCLUDED.codex_weight_pro20x,
                    kiro_status_refresh_min_interval_seconds =
                        EXCLUDED.kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds =
                        EXCLUDED.kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds =
                        EXCLUDED.kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size = EXCLUDED.usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds =
                        EXCLUDED.usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes =
                        EXCLUDED.usage_event_flush_max_buffer_bytes,
                    duckdb_usage_memory_limit_mib =
                        EXCLUDED.duckdb_usage_memory_limit_mib,
                    duckdb_usage_checkpoint_threshold_mib =
                        EXCLUDED.duckdb_usage_checkpoint_threshold_mib,
                    usage_analytics_retention_days =
                        EXCLUDED.usage_analytics_retention_days,
                    usage_journal_enabled = EXCLUDED.usage_journal_enabled,
                    usage_journal_max_file_bytes = EXCLUDED.usage_journal_max_file_bytes,
                    usage_journal_max_file_age_ms = EXCLUDED.usage_journal_max_file_age_ms,
                    usage_journal_max_files = EXCLUDED.usage_journal_max_files,
                    usage_journal_block_target_uncompressed_bytes =
                        EXCLUDED.usage_journal_block_target_uncompressed_bytes,
                    usage_journal_block_max_events =
                        EXCLUDED.usage_journal_block_max_events,
                    usage_journal_fsync_interval_ms =
                        EXCLUDED.usage_journal_fsync_interval_ms,
                    usage_journal_zstd_level = EXCLUDED.usage_journal_zstd_level,
                    usage_journal_consumer_lease_ms =
                        EXCLUDED.usage_journal_consumer_lease_ms,
                    usage_journal_delete_bad_files =
                        EXCLUDED.usage_journal_delete_bad_files,
                    usage_query_bind_addr = EXCLUDED.usage_query_bind_addr,
                    usage_query_base_url = EXCLUDED.usage_query_base_url,
                    usage_event_maintenance_enabled =
                        EXCLUDED.usage_event_maintenance_enabled,
                    usage_event_maintenance_interval_seconds =
                        EXCLUDED.usage_event_maintenance_interval_seconds,
                    usage_event_detail_retention_days =
                        EXCLUDED.usage_event_detail_retention_days,
                    kiro_cache_kmodels_json = EXCLUDED.kiro_cache_kmodels_json,
                    kiro_billable_model_multipliers_json =
                        EXCLUDED.kiro_billable_model_multipliers_json,
                    kiro_cache_policy_json = EXCLUDED.kiro_cache_policy_json,
                    kiro_context_usage_min_request_tokens =
                        EXCLUDED.kiro_context_usage_min_request_tokens,
                    kiro_prefix_cache_mode = EXCLUDED.kiro_prefix_cache_mode,
                    kiro_prefix_cache_max_tokens = EXCLUDED.kiro_prefix_cache_max_tokens,
                    kiro_prefix_cache_entry_ttl_seconds =
                        EXCLUDED.kiro_prefix_cache_entry_ttl_seconds,
                    kiro_conversation_anchor_max_entries =
                        EXCLUDED.kiro_conversation_anchor_max_entries,
                    kiro_conversation_anchor_ttl_seconds =
                        EXCLUDED.kiro_conversation_anchor_ttl_seconds,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &record.id,
                    &record.auth_cache_ttl_seconds,
                    &record.max_request_body_bytes,
                    &record.account_failure_retry_limit,
                    &record.codex_client_version,
                    &record.kiro_channel_max_concurrency,
                    &record.kiro_channel_min_start_interval_ms,
                    &record.codex_status_refresh_min_interval_seconds,
                    &record.codex_status_refresh_max_interval_seconds,
                    &record.codex_status_account_jitter_max_seconds,
                    &record.codex_weight_free,
                    &record.codex_weight_plus,
                    &record.codex_weight_pro5x,
                    &record.codex_weight_pro20x,
                    &record.kiro_status_refresh_min_interval_seconds,
                    &record.kiro_status_refresh_max_interval_seconds,
                    &record.kiro_status_account_jitter_max_seconds,
                    &record.usage_event_flush_batch_size,
                    &record.usage_event_flush_interval_seconds,
                    &record.usage_event_flush_max_buffer_bytes,
                    &record.duckdb_usage_memory_limit_mib,
                    &record.duckdb_usage_checkpoint_threshold_mib,
                    &record.usage_analytics_retention_days,
                    &record.usage_journal_enabled,
                    &record.usage_journal_max_file_bytes,
                    &record.usage_journal_max_file_age_ms,
                    &record.usage_journal_max_files,
                    &record.usage_journal_block_target_uncompressed_bytes,
                    &record.usage_journal_block_max_events,
                    &record.usage_journal_fsync_interval_ms,
                    &record.usage_journal_zstd_level,
                    &record.usage_journal_consumer_lease_ms,
                    &(record.usage_journal_delete_bad_files as i64),
                    &record.usage_query_bind_addr,
                    &record.usage_query_base_url,
                    &record.usage_event_maintenance_enabled,
                    &record.usage_event_maintenance_interval_seconds,
                    &record.usage_event_detail_retention_days,
                    &record.kiro_cache_kmodels_json,
                    &record.kiro_billable_model_multipliers_json,
                    &record.kiro_cache_policy_json,
                    &record.kiro_context_usage_min_request_tokens,
                    &record.kiro_prefix_cache_mode,
                    &record.kiro_prefix_cache_max_tokens,
                    &record.kiro_prefix_cache_entry_ttl_seconds,
                    &record.kiro_conversation_anchor_max_entries,
                    &record.kiro_conversation_anchor_ttl_seconds,
                    &record.updated_at_ms,
                ],
            )
            .await
            .context("upsert postgres runtime config")?;
        self.store_runtime_config_record_cached(Some(record)).await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(())
    }
}
