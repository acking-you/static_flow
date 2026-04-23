//! LanceDB-backed storage for StaticFlow's LLM gateway.
//!
//! This module is the persistence boundary for gateway keys, usage events,
//! runtime config, upstream proxy configs/bindings, and the public request
//! queues shown in the admin UI.

mod codec;
mod kiro_cache_policy;
mod schema;
mod types;

use std::collections::HashMap;

use anyhow::{Context, Result};
use arrow_array::{
    Array, Float64Array, Int64Array, RecordBatchIterator, RecordBatchReader, StringArray,
    TimestampMillisecondArray, UInt64Array,
};
use futures::TryStreamExt;
use lance::Dataset;
use lancedb::{
    connect,
    database::CreateTableMode,
    query::{ExecutableQuery, QueryBase, Select},
    table::{OptimizeAction, OptimizeOptions},
    Connection, Table,
};

use self::{
    codec::{
        batches_to_account_contribution_requests, batches_to_account_groups, batches_to_keys,
        batches_to_proxy_bindings, batches_to_proxy_configs, batches_to_runtime_config,
        batches_to_sponsor_requests, batches_to_token_requests, batches_to_usage_event_summaries,
        batches_to_usage_events, build_account_contribution_requests_batch,
        build_account_groups_batch, build_keys_batch, build_proxy_bindings_batch,
        build_proxy_configs_batch, build_runtime_config_batch, build_sponsor_requests_batch,
        build_token_requests_batch, build_usage_events_batch,
    },
    schema::{
        account_contribution_request_columns, account_group_columns,
        ensure_account_contribution_requests_table, ensure_account_groups_table, ensure_keys_table,
        ensure_proxy_bindings_table, ensure_proxy_configs_table, ensure_runtime_config_table,
        ensure_sponsor_requests_table, ensure_token_requests_table, ensure_usage_events_table,
        escape_literal, key_columns, proxy_binding_columns, proxy_config_columns,
        runtime_config_columns, sponsor_request_columns, token_request_columns,
        usage_event_columns, usage_event_rebuild_columns, usage_event_summary_columns,
    },
};
pub use self::{
    kiro_cache_policy::{
        default_kiro_cache_policy, default_kiro_cache_policy_json,
        interpolate_prefix_tree_cache_ratio, merge_kiro_cache_policy, parse_kiro_cache_policy_json,
        parse_kiro_cache_policy_override_json, validate_kiro_cache_policy,
        validate_kiro_cache_policy_override, KiroCachePolicy, KiroCachePolicyOverride,
        KiroCreditRatioBand, KiroSmallInputHighCreditBoostOverride,
        KiroSmallInputHighCreditBoostPolicy,
    },
    types::{
        compute_billable_tokens, compute_kiro_billable_tokens,
        default_kiro_billable_model_multipliers, default_kiro_billable_model_multipliers_json,
        default_kiro_cache_kmodels, default_kiro_cache_kmodels_json,
        is_valid_kiro_prefix_cache_mode, now_ms, LlmGatewayAccountContributionRequestRecord,
        LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayKeyUsageRollupRecord,
        LlmGatewayProxyBindingRecord, LlmGatewayProxyConfigRecord, LlmGatewayRuntimeConfigRecord,
        LlmGatewaySponsorRequestRecord, LlmGatewayTokenRequestRecord, LlmGatewayUsageEventRecord,
        LlmGatewayUsageEventSummaryRecord, NewLlmGatewayAccountContributionRequestInput,
        NewLlmGatewaySponsorRequestInput, NewLlmGatewayTokenRequestInput,
        DEFAULT_CODEX_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_CODEX_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
        DEFAULT_KIRO_BILLABLE_MODEL_MULTIPLIER_HAIKU, DEFAULT_KIRO_BILLABLE_MODEL_MULTIPLIER_OPUS,
        DEFAULT_KIRO_BILLABLE_MODEL_MULTIPLIER_SONNET, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
        DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, DEFAULT_KIRO_CONVERSATION_ANCHOR_MAX_ENTRIES,
        DEFAULT_KIRO_CONVERSATION_ANCHOR_TTL_SECONDS, DEFAULT_KIRO_PREFIX_CACHE_ENTRY_TTL_SECONDS,
        DEFAULT_KIRO_PREFIX_CACHE_MAX_TOKENS, DEFAULT_KIRO_PREFIX_CACHE_MODE,
        DEFAULT_KIRO_STATUS_ACCOUNT_JITTER_MAX_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MAX_INTERVAL_SECONDS,
        DEFAULT_KIRO_STATUS_REFRESH_MIN_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_ACCOUNT_FAILURE_RETRY_LIMIT,
        DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS, DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_DETAIL_RETENTION_DAYS,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_BATCH_SIZE,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_INTERVAL_SECONDS,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_FLUSH_MAX_BUFFER_BYTES,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_MAINTENANCE_ENABLED,
        DEFAULT_LLM_GATEWAY_USAGE_EVENT_MAINTENANCE_INTERVAL_SECONDS,
        KIRO_PREFIX_CACHE_MODE_FORMULA, KIRO_PREFIX_CACHE_MODE_PREFIX_TREE,
        LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE, LLM_GATEWAY_ACCOUNT_GROUPS_TABLE,
        LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED,
        LLM_GATEWAY_PROTOCOL_ANTHROPIC, LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX,
        LLM_GATEWAY_PROVIDER_KIRO, LLM_GATEWAY_PROXY_BINDINGS_TABLE,
        LLM_GATEWAY_PROXY_CONFIGS_TABLE, LLM_GATEWAY_RUNTIME_CONFIG_TABLE,
        LLM_GATEWAY_SPONSOR_REQUESTS_TABLE, LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED,
        LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT,
        LLM_GATEWAY_SPONSOR_REQUEST_STATUS_SUBMITTED, LLM_GATEWAY_TABLE_NAMES,
        LLM_GATEWAY_TOKEN_REQUESTS_TABLE, LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED,
        LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED, LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING,
        LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED, LLM_GATEWAY_USAGE_EVENTS_TABLE,
    },
};
use crate::optimize::{
    acquire_table_access_file_lock, compact_table_with_fallback, local_table_access_lock_path,
    TableAccessMode,
};

/// Owns the LanceDB-backed storage layer for all LLM gateway admin data.
pub struct LlmGatewayStore {
    db: Connection,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LlmGatewayUsageEventCounts {
    pub total_event_count: usize,
    pub provider_event_counts: HashMap<String, usize>,
    pub key_event_counts: HashMap<String, usize>,
}

impl LlmGatewayStore {
    /// Open the store connection, ensure all required tables exist, and create
    /// the default runtime-config row if it is missing.
    pub async fn connect(db_uri: &str) -> Result<Self> {
        tracing::info!("Opening LLM gateway store at `{db_uri}`");
        let mut store = Self::connect_inner(db_uri).await?;
        let rewritten = store.canonicalize_keys_table_if_needed().await?;
        if rewritten {
            tracing::warn!(
                "reconnecting llm gateway store after canonical key-table rewrite to avoid stale \
                 Lance table state"
            );
            store = Self::connect_inner(db_uri).await?;
        }
        store.repair_keys_table_for_safe_reads().await?;
        tracing::info!("LLM gateway store ready");
        Ok(store)
    }

    async fn connect_inner(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect llm gateway LanceDB")?;
        let store = Self {
            db,
        };
        store.bootstrap_tables().await?;
        store.ensure_default_runtime_config().await?;
        Ok(store)
    }

    /// Expose the underlying LanceDB connection for advanced callers.
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    async fn bootstrap_tables(&self) -> Result<()> {
        ensure_keys_table(&self.db).await?;
        ensure_usage_events_table(&self.db).await?;
        ensure_runtime_config_table(&self.db).await?;
        ensure_account_groups_table(&self.db).await?;
        ensure_proxy_configs_table(&self.db).await?;
        ensure_proxy_bindings_table(&self.db).await?;
        ensure_token_requests_table(&self.db).await?;
        ensure_account_contribution_requests_table(&self.db).await?;
        ensure_sponsor_requests_table(&self.db).await?;
        Ok(())
    }

    async fn open_table(&self, table_name: &str) -> Result<Table> {
        self.db
            .open_table(table_name)
            .execute()
            .await
            .with_context(|| format!("failed to open llm gateway table `{table_name}`"))
    }

    async fn keys_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_KEYS_TABLE).await
    }

    async fn usage_events_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_USAGE_EVENTS_TABLE).await
    }

    async fn runtime_config_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_RUNTIME_CONFIG_TABLE).await
    }

    async fn account_groups_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_ACCOUNT_GROUPS_TABLE).await
    }

    async fn proxy_configs_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_PROXY_CONFIGS_TABLE).await
    }

    async fn proxy_bindings_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_PROXY_BINDINGS_TABLE).await
    }

    async fn token_requests_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_TOKEN_REQUESTS_TABLE).await
    }

    async fn account_contribution_requests_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE)
            .await
    }

    async fn sponsor_requests_table(&self) -> Result<Table> {
        self.open_table(LLM_GATEWAY_SPONSOR_REQUESTS_TABLE).await
    }

    async fn ensure_default_runtime_config(&self) -> Result<()> {
        if self.get_runtime_config().await?.is_none() {
            self.upsert_runtime_config(&LlmGatewayRuntimeConfigRecord::default())
                .await?;
        }
        Ok(())
    }

    /// Rebuild scalar indices on the keys table after any write operation.
    /// Keeps filtered queries (by id, status, etc.) fast on small tables.
    async fn optimize_key_table_indices(&self) -> Result<()> {
        let table = self.keys_table().await?;
        table
            .optimize(OptimizeAction::Index(OptimizeOptions::default()))
            .await
            .context("failed to optimize llm gateway key-table indices")?;
        Ok(())
    }

    /// Detect and fix legacy keys tables that have nullable columns where the
    /// canonical schema requires non-null (e.g. `provider_type`,
    /// `protocol_family`, `usage_credit_total`,
    /// `usage_credit_missing_events`).
    ///
    /// When any of these columns are nullable in the Arrow schema or contain
    /// actual NULL values, the entire table is rewritten with the canonical
    /// non-null schema. This is a one-time migration that runs at startup.
    async fn canonicalize_keys_table_if_needed(&self) -> Result<bool> {
        let table = self.keys_table().await?;
        let schema = table.schema().await?;
        let canonical_fields = [
            "provider_type",
            "protocol_family",
            "usage_credit_total",
            "usage_credit_missing_events",
        ];
        let nullable_fields = canonical_fields
            .iter()
            .filter_map(|name| {
                schema
                    .field_with_name(name)
                    .ok()
                    .and_then(|field| field.is_nullable().then_some((*name).to_string()))
            })
            .collect::<Vec<_>>();
        let null_fields = self
            .keys_table_null_fields(&table, &canonical_fields)
            .await?;
        if nullable_fields.is_empty() && null_fields.is_empty() {
            return Ok(false);
        }
        let mut reasons = Vec::new();
        if !nullable_fields.is_empty() {
            reasons.push(format!("nullable_schema_fields={}", nullable_fields.join(",")));
        }
        if !null_fields.is_empty() {
            reasons.push(format!("null_rows={}", null_fields.join(",")));
        }
        let keys = self
            .list_keys()
            .await
            .context("failed to read llm gateway keys before canonical rewrite")?;
        let batch = build_keys_batch(&keys)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        tracing::warn!(
            row_count = keys.len(),
            reasons = %reasons.join(";"),
            "rewriting llm gateway keys table to canonical non-null schema"
        );
        self.db
            .create_table(
                LLM_GATEWAY_KEYS_TABLE,
                Box::new(batches) as Box<dyn RecordBatchReader + Send>,
            )
            .mode(CreateTableMode::Overwrite)
            .storage_option("new_table_enable_stable_row_ids", "true")
            .storage_option("new_table_enable_v2_manifest_paths", "true")
            .execute()
            .await
            .context("failed to overwrite llm gateway keys table with canonical schema")?;
        tracing::info!(
            row_count = keys.len(),
            "completed llm gateway keys table canonical rewrite"
        );
        Ok(true)
    }

    /// Scan the given columns and return the names of any that contain at
    /// least one NULL value in the current data.
    async fn keys_table_null_fields(&self, table: &Table, columns: &[&str]) -> Result<Vec<String>> {
        let batches = table
            .query()
            .select(Select::columns(columns))
            .execute()
            .await
            .context("failed to scan llm gateway key-table canonical fields")?;
        let batch_list = batches
            .try_collect::<Vec<_>>()
            .await
            .context("failed to collect llm gateway key-table canonical scan")?;
        let mut null_fields = Vec::new();
        for &column in columns {
            let has_nulls = batch_list.iter().any(|batch| {
                batch
                    .column_by_name(column)
                    .is_some_and(|array| array.null_count() > 0)
            });
            if has_nulls {
                null_fields.push(column.to_string());
            }
        }
        Ok(null_fields)
    }

    /// Ensure the keys table is in a healthy state for filtered reads.
    ///
    /// Checks fragment count and index coverage; if any rows are unindexed,
    /// compacts multi-fragment tables and rebuilds indices. Also repairs
    /// `frag_reuse` metadata that can go stale after delete+add cycles.
    pub async fn repair_keys_table_for_safe_reads(&self) -> Result<()> {
        let table = self.keys_table().await?;
        let fragment_count = table
            .dataset()
            .context("llm gateway key-table maintenance requires a native Lance table")?
            .get()
            .await
            .context("failed to open key-table dataset for maintenance")?
            .fragments()
            .len();
        let indices = table
            .list_indices()
            .await
            .context("failed to list llm gateway key-table indices")?;
        let mut max_unindexed_rows = 0usize;
        for index in &indices {
            if let Some(stats) = table
                .index_stats(&index.name)
                .await
                .with_context(|| format!("failed to inspect key-table index `{}`", index.name))?
            {
                max_unindexed_rows = max_unindexed_rows.max(stats.num_unindexed_rows);
            }
        }
        if max_unindexed_rows == 0 && fragment_count <= 1 {
            tracing::debug!(
                fragment_count,
                "llm gateway key table already has full index coverage"
            );
            return Ok(());
        }
        table
            .repair_missing_frag_reuse_index()
            .await
            .context("failed to repair llm gateway key-table frag_reuse metadata")?;
        if max_unindexed_rows > 0 {
            if fragment_count > 1 {
                let action = compact_table_with_fallback(&table)
                    .await
                    .map_err(anyhow::Error::msg)
                    .context("failed to compact llm gateway key table for read safety")?;
                tracing::info!(
                    fragment_count,
                    max_unindexed_rows,
                    compact_action = action.as_str(),
                    "compacted llm gateway key table before rebuilding indices"
                );
            }
            table
                .optimize(OptimizeAction::Index(OptimizeOptions::default()))
                .await
                .context("failed to rebuild llm gateway key-table indices for read safety")?;
        }
        tracing::info!(
            fragment_count,
            max_unindexed_rows,
            "verified llm gateway key table read-path safety"
        );
        Ok(())
    }

    /// Load the singleton runtime-config row if it exists.
    pub async fn get_runtime_config(&self) -> Result<Option<LlmGatewayRuntimeConfigRecord>> {
        let table = self.runtime_config_table().await?;
        let batches = table
            .query()
            .only_if("id = 'default'")
            .limit(1)
            .select(Select::columns(&runtime_config_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_runtime_config(&batch_list).map(|mut rows| rows.pop())
    }

    /// Load the runtime config, synthesizing the default struct when the table
    /// is empty.
    pub async fn get_runtime_config_or_default(&self) -> Result<LlmGatewayRuntimeConfigRecord> {
        Ok(self.get_runtime_config().await?.unwrap_or_default())
    }

    /// Insert or replace the singleton runtime-config row.
    pub async fn upsert_runtime_config(
        &self,
        record: &LlmGatewayRuntimeConfigRecord,
    ) -> Result<()> {
        let table = self.runtime_config_table().await?;
        let batch = build_runtime_config_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway runtime config")?;
        Ok(())
    }

    /// Insert a new reusable account group.
    pub async fn create_account_group(&self, record: &LlmGatewayAccountGroupRecord) -> Result<()> {
        let table = self.account_groups_table().await?;
        let batch = build_account_groups_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to create llm gateway account group")?;
        Ok(())
    }

    /// Upsert an account group by `id`.
    pub async fn upsert_account_group(&self, record: &LlmGatewayAccountGroupRecord) -> Result<()> {
        let table = self.account_groups_table().await?;
        let batch = build_account_groups_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway account group")?;
        Ok(())
    }

    /// Replace an account group exactly, allowing callers to clear or reorder
    /// membership.
    pub async fn replace_account_group(&self, record: &LlmGatewayAccountGroupRecord) -> Result<()> {
        self.delete_account_group(&record.id).await?;
        self.create_account_group(record).await
    }

    /// Look up one account group by id.
    pub async fn get_account_group_by_id(
        &self,
        group_id: &str,
    ) -> Result<Option<LlmGatewayAccountGroupRecord>> {
        let table = self.account_groups_table().await?;
        let escaped = escape_literal(group_id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&account_group_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_account_groups(&batch_list).map(|mut rows| rows.pop())
    }

    /// List all account groups sorted by provider then display name.
    pub async fn list_account_groups(&self) -> Result<Vec<LlmGatewayAccountGroupRecord>> {
        let table = self.account_groups_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&account_group_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_account_groups(&batch_list)?;
        rows.sort_by_cached_key(|row| {
            (row.provider_type.to_ascii_lowercase(), row.name.to_ascii_lowercase())
        });
        Ok(rows)
    }

    /// List all account groups for one provider sorted by display name.
    pub async fn list_account_groups_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<Vec<LlmGatewayAccountGroupRecord>> {
        let table = self.account_groups_table().await?;
        let escaped_provider = escape_literal(provider_type);
        let batches = table
            .query()
            .only_if(format!("provider_type = '{escaped_provider}'"))
            .select(Select::columns(&account_group_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_account_groups(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    /// Delete one account group by id.
    pub async fn delete_account_group(&self, group_id: &str) -> Result<()> {
        let table = self.account_groups_table().await?;
        let escaped = escape_literal(group_id);
        table
            .delete(&format!("id = '{escaped}'"))
            .await
            .with_context(|| format!("failed to delete llm gateway account group `{group_id}`"))?;
        Ok(())
    }

    /// Insert a new upstream proxy config.
    pub async fn create_proxy_config(&self, record: &LlmGatewayProxyConfigRecord) -> Result<()> {
        let table = self.proxy_configs_table().await?;
        let batch = build_proxy_configs_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to create llm gateway proxy config")?;
        Ok(())
    }

    /// Upsert an upstream proxy config by `id`.
    pub async fn upsert_proxy_config(&self, record: &LlmGatewayProxyConfigRecord) -> Result<()> {
        let table = self.proxy_configs_table().await?;
        let batch = build_proxy_configs_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway proxy config")?;
        Ok(())
    }

    /// Look up one upstream proxy config by id.
    pub async fn get_proxy_config_by_id(
        &self,
        proxy_id: &str,
    ) -> Result<Option<LlmGatewayProxyConfigRecord>> {
        let table = self.proxy_configs_table().await?;
        let escaped = escape_literal(proxy_id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&proxy_config_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_proxy_configs(&batch_list).map(|mut rows| rows.pop())
    }

    /// List all upstream proxy configs sorted by display name.
    pub async fn list_proxy_configs(&self) -> Result<Vec<LlmGatewayProxyConfigRecord>> {
        let table = self.proxy_configs_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&proxy_config_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_proxy_configs(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    /// Delete one upstream proxy config by id.
    pub async fn delete_proxy_config(&self, proxy_id: &str) -> Result<()> {
        let table = self.proxy_configs_table().await?;
        let escaped = escape_literal(proxy_id);
        table
            .delete(&format!("id = '{escaped}'"))
            .await
            .with_context(|| format!("failed to delete llm gateway proxy config `{proxy_id}`"))?;
        Ok(())
    }

    /// Look up the provider-level upstream proxy binding for one provider.
    pub async fn get_proxy_binding(
        &self,
        provider_type: &str,
    ) -> Result<Option<LlmGatewayProxyBindingRecord>> {
        let table = self.proxy_bindings_table().await?;
        let escaped = escape_literal(provider_type);
        let batches = table
            .query()
            .only_if(format!("provider_type = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&proxy_binding_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_proxy_bindings(&batch_list).map(|mut rows| rows.pop())
    }

    /// List all provider-level upstream proxy bindings.
    pub async fn list_proxy_bindings(&self) -> Result<Vec<LlmGatewayProxyBindingRecord>> {
        let table = self.proxy_bindings_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&proxy_binding_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_proxy_bindings(&batch_list)?;
        rows.sort_by_cached_key(|row| row.provider_type.to_ascii_lowercase());
        Ok(rows)
    }

    /// Insert or replace one provider-level upstream proxy binding.
    pub async fn upsert_proxy_binding(&self, record: &LlmGatewayProxyBindingRecord) -> Result<()> {
        let table = self.proxy_bindings_table().await?;
        let batch = build_proxy_bindings_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["provider_type"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway proxy binding")?;
        Ok(())
    }

    /// Delete a provider-level upstream proxy binding.
    pub async fn delete_proxy_binding(&self, provider_type: &str) -> Result<()> {
        let table = self.proxy_bindings_table().await?;
        let escaped = escape_literal(provider_type);
        table
            .delete(&format!("provider_type = '{escaped}'"))
            .await
            .with_context(|| {
                format!("failed to delete llm gateway proxy binding for `{provider_type}`")
            })?;
        Ok(())
    }

    /// Upsert a gateway API key by `id`.
    ///
    /// This is appropriate for in-place updates where nullable fields are not
    /// expected to be cleared back to `NULL`.
    pub async fn upsert_key(&self, record: &LlmGatewayKeyRecord) -> Result<()> {
        let table = self.keys_table().await?;
        let batch = build_keys_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway key")?;
        self.optimize_key_table_indices().await?;
        Ok(())
    }

    /// Inserts a new gateway key via append (not upsert).
    ///
    /// Unlike [`upsert_key`](Self::upsert_key), this performs a plain `add` so
    /// it will create a duplicate row if a key with the same `id` already
    /// exists. Use this for first-time key creation where uniqueness is
    /// guaranteed by the caller.
    /// Insert a brand-new gateway API key.
    pub async fn create_key(&self, record: &LlmGatewayKeyRecord) -> Result<()> {
        self.create_key_raw(record).await?;
        self.optimize_key_table_indices().await?;
        Ok(())
    }

    /// Replaces a key row by `id`, allowing nullable fields to be cleared.
    ///
    /// This is intended for admin edit flows where the caller is writing the
    /// full logical record and expects `None` values to overwrite prior
    /// non-null state (for example resetting per-key request limits back to
    /// "unlimited").
    /// Replace an existing gateway API key record exactly.
    ///
    /// This is used by admin edit flows that must be able to clear nullable
    /// fields back to `NULL` instead of relying on merge semantics.
    pub async fn replace_key(&self, record: &LlmGatewayKeyRecord) -> Result<()> {
        self.delete_key_raw(&record.id).await?;
        self.create_key_raw(record).await?;
        self.optimize_key_table_indices().await
    }

    /// Delete a gateway key by id and rebuild indices.
    pub async fn delete_key(&self, key_id: &str) -> Result<()> {
        self.delete_key_raw(key_id).await?;
        self.optimize_key_table_indices().await?;
        Ok(())
    }

    /// Low-level append of a single key row without index optimization.
    /// Callers are responsible for calling `optimize_key_table_indices`
    /// after all writes in a logical operation are complete.
    async fn create_key_raw(&self, record: &LlmGatewayKeyRecord) -> Result<()> {
        let table = self.keys_table().await?;
        let batch = build_keys_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to create llm gateway key")?;
        Ok(())
    }

    /// Low-level delete of a key row without index optimization.
    async fn delete_key_raw(&self, key_id: &str) -> Result<()> {
        let table = self.keys_table().await?;
        let escaped = escape_literal(key_id);
        table
            .delete(&format!("id = '{escaped}'"))
            .await
            .with_context(|| format!("failed to delete llm gateway key `{key_id}`"))?;
        Ok(())
    }

    pub async fn get_key_by_id(&self, key_id: &str) -> Result<Option<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let escaped = escape_literal(key_id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_keys(&batch_list).map(|mut rows| rows.pop())
    }

    /// Looks up a single key by its `id` scoped to a specific `provider_type`.
    ///
    /// Returns `None` if no key matches both the id and provider.
    pub async fn get_key_by_id_for_provider(
        &self,
        key_id: &str,
        provider_type: &str,
    ) -> Result<Option<LlmGatewayKeyRecord>> {
        let escaped_key_id = escape_literal(key_id);
        let escaped_provider = escape_literal(provider_type);
        let table = self.keys_table().await?;
        let batches = table
            .query()
            .only_if(format!("id = '{escaped_key_id}' AND provider_type = '{escaped_provider}'"))
            .limit(1)
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_keys(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn get_key_by_hash(&self, key_hash: &str) -> Result<Option<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let escaped = escape_literal(key_hash);
        let batches = table
            .query()
            .only_if(format!("key_hash = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_keys(&batch_list).map(|mut rows| rows.pop())
    }

    /// Looks up a single key by its `key_hash` scoped to a specific
    /// `provider_type`.
    ///
    /// Used during request authentication to resolve the hashed bearer token
    /// to the correct provider-specific key record.
    pub async fn get_key_by_hash_for_provider(
        &self,
        key_hash: &str,
        provider_type: &str,
    ) -> Result<Option<LlmGatewayKeyRecord>> {
        let escaped_hash = escape_literal(key_hash);
        let escaped_provider = escape_literal(provider_type);
        let table = self.keys_table().await?;
        let batches = table
            .query()
            .only_if(format!(
                "key_hash = '{escaped_hash}' AND provider_type = '{escaped_provider}'"
            ))
            .limit(1)
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_keys(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn list_keys(&self) -> Result<Vec<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_keys(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    /// Lists all keys belonging to the given `provider_type`, sorted by name
    /// (case-insensitive).
    pub async fn list_keys_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<Vec<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let escaped_provider = escape_literal(provider_type);
        let batches = table
            .query()
            .only_if(format!("provider_type = '{escaped_provider}'"))
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_keys(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    pub async fn list_public_keys(&self) -> Result<Vec<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let batches = table
            .query()
            .only_if(format!(
                "status = '{}' AND public_visible = true",
                LLM_GATEWAY_KEY_STATUS_ACTIVE
            ))
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_keys(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    /// Lists active, publicly visible keys for the given `provider_type`,
    /// sorted by name.
    ///
    /// Filters on `status = active AND public_visible = true AND provider_type
    /// = <provider>`.
    pub async fn list_public_keys_for_provider(
        &self,
        provider_type: &str,
    ) -> Result<Vec<LlmGatewayKeyRecord>> {
        let table = self.keys_table().await?;
        let escaped_provider = escape_literal(provider_type);
        let batches = table
            .query()
            .only_if(format!(
                "status = '{}' AND public_visible = true AND provider_type = '{escaped_provider}'",
                LLM_GATEWAY_KEY_STATUS_ACTIVE
            ))
            .select(Select::columns(&key_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut rows = batches_to_keys(&batch_list)?;
        rows.sort_by_cached_key(|row| row.name.to_ascii_lowercase());
        Ok(rows)
    }

    /// Append one or more usage event rows to the events table (append-only).
    /// Does not update the key record; callers maintain usage totals
    /// via in-memory rollups.
    pub async fn append_usage_events(&self, records: &[LlmGatewayUsageEventRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let lock_path = local_table_access_lock_path(self.db.uri(), LLM_GATEWAY_USAGE_EVENTS_TABLE);
        let _file_guard = acquire_table_access_file_lock(&lock_path, TableAccessMode::Shared)
            .await
            .map_err(anyhow::Error::msg)?;
        let table = self.usage_events_table().await?;
        let batch = build_usage_events_batch(records)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to append llm gateway usage events")?;
        Ok(())
    }

    /// Append a single usage event row to the events table (append-only).
    pub async fn append_usage_event(&self, record: &LlmGatewayUsageEventRecord) -> Result<()> {
        self.append_usage_events(std::slice::from_ref(record)).await
    }

    pub async fn list_usage_events(
        &self,
        key_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        self.query_usage_events(key_id, None, limit, Some(0)).await
    }

    pub async fn count_usage_events(&self, key_id: Option<&str>) -> Result<usize> {
        self.count_usage_events_for_provider(key_id, None).await
    }

    /// Counts usage events, optionally filtered by `key_id` and/or
    /// `provider_type`.
    ///
    /// Both filters are trimmed and ignored when empty. Delegates to
    /// [`join_filters`] to combine the optional clauses.
    pub async fn count_usage_events_for_provider(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
    ) -> Result<usize> {
        let table = self.usage_events_table().await?;
        let filter = join_filters([
            key_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("key_id = '{}'", escape_literal(value))),
            provider_type
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("provider_type = '{}'", escape_literal(value))),
        ]);
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count llm gateway usage events")?;
        tracing::debug!(
            key_id = key_id.unwrap_or("all"),
            total = total as usize,
            "Counted LLM gateway usage events"
        );
        Ok(total as usize)
    }

    /// Queries a raw slice of usage events in table order.
    ///
    /// Optionally filters by `key_id` and/or `provider_type` (both trimmed,
    /// ignored when empty). The caller is responsible for translating
    /// user-facing "newest first" pagination into the corresponding tail
    /// offset, mirroring the existing api_behavior pagination strategy.
    pub async fn query_usage_events(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        self.query_usage_events_filtered(key_id, provider_type, None, limit, offset)
            .await
    }

    /// Queries a raw slice of usage events with an optional lower bound on
    /// `created_at`.
    ///
    /// This is used by public and admin read paths that need a bounded time
    /// window without scanning the full immutable event history into memory.
    pub async fn query_usage_events_since(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
        created_at_gte: Option<i64>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        self.query_usage_events_filtered(key_id, provider_type, created_at_gte, limit, offset)
            .await
    }

    pub async fn query_usage_event_summaries(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventSummaryRecord>> {
        let table = self.usage_events_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&usage_event_summary_columns()));
        if let Some(filter) = join_filters([
            key_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("key_id = '{}'", escape_literal(value))),
            provider_type
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("provider_type = '{}'", escape_literal(value))),
        ]) {
            query = query.only_if(filter);
        }
        if let Some(offset) = offset {
            query = query.offset(offset);
        }
        if let Some(limit) = limit {
            query = query.limit(limit.max(1));
        }
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_usage_event_summaries(&batch_list)
    }

    pub async fn query_usage_event_rebuild_rows(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        query_usage_event_rebuild_rows_from_connection(
            &self.db,
            LLM_GATEWAY_USAGE_EVENTS_TABLE,
            key_id,
            provider_type,
            limit,
            offset,
        )
        .await
    }

    async fn query_usage_events_filtered(
        &self,
        key_id: Option<&str>,
        provider_type: Option<&str>,
        created_at_gte: Option<i64>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        let table = self.usage_events_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&usage_event_columns()));
        if let Some(filter) = join_filters([
            key_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("key_id = '{}'", escape_literal(value))),
            provider_type
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| format!("provider_type = '{}'", escape_literal(value))),
            created_at_gte.map(|value| {
                format!("created_at >= arrow_cast({value}, 'Timestamp(Millisecond, None)')")
            }),
        ]) {
            query = query.only_if(filter);
        }
        if let Some(offset) = offset {
            query = query.offset(offset);
        }
        if let Some(limit) = limit {
            query = query.limit(limit.max(1));
        }
        tracing::debug!(
            key_id = key_id.unwrap_or("all"),
            provider_type = provider_type.unwrap_or("all"),
            created_at_gte = created_at_gte.unwrap_or_default(),
            limit = limit.unwrap_or_default(),
            offset = offset.unwrap_or_default(),
            "Querying LLM gateway usage events"
        );
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_usage_events(&batch_list)
    }

    pub async fn get_usage_event_detail_by_id(
        &self,
        event_id: &str,
    ) -> Result<Option<LlmGatewayUsageEventRecord>> {
        let table = self.usage_events_table().await?;
        let escaped = escape_literal(event_id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&usage_event_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_usage_events(&batch_list).map(|mut rows| rows.pop())
    }

    /// Aggregate all usage events into per-key rollup totals via a SQL
    /// GROUP BY over the underlying Lance dataset.
    pub async fn aggregate_usage_rollups(&self) -> Result<Vec<LlmGatewayKeyUsageRollupRecord>> {
        let table = self.usage_events_table().await?;
        let dataset = table
            .dataset()
            .context("llm gateway usage-event aggregation requires a native Lance table")?
            .get()
            .await
            .context("failed to open usage-event dataset for aggregation")?;
        aggregate_usage_rollups_from_dataset(&dataset).await
    }

    pub async fn aggregate_usage_event_counts(&self) -> Result<LlmGatewayUsageEventCounts> {
        let table = self.usage_events_table().await?;
        let dataset = table
            .dataset()
            .context("usage-event counts require a native Lance table")?
            .get()
            .await
            .context("failed to open usage-event dataset for counts")?;
        aggregate_usage_event_counts_from_dataset(&dataset).await
    }

    pub async fn upsert_token_request(&self, record: &LlmGatewayTokenRequestRecord) -> Result<()> {
        let table = self.token_requests_table().await?;
        let batch = build_token_requests_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["request_id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway token request")?;
        Ok(())
    }

    pub async fn create_token_request(
        &self,
        input: NewLlmGatewayTokenRequestInput,
    ) -> Result<LlmGatewayTokenRequestRecord> {
        let now = now_ms();
        let record = LlmGatewayTokenRequestRecord {
            request_id: input.request_id,
            requester_email: input.requester_email,
            requested_quota_billable_limit: input.requested_quota_billable_limit,
            request_reason: input.request_reason,
            frontend_page_url: input.frontend_page_url,
            status: LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING.to_string(),
            fingerprint: input.fingerprint,
            client_ip: input.client_ip,
            ip_region: input.ip_region,
            admin_note: None,
            failure_reason: None,
            issued_key_id: None,
            issued_key_name: None,
            created_at: now,
            updated_at: now,
            processed_at: None,
        };
        self.upsert_token_request(&record).await?;
        Ok(record)
    }

    pub async fn get_token_request(
        &self,
        request_id: &str,
    ) -> Result<Option<LlmGatewayTokenRequestRecord>> {
        let table = self.token_requests_table().await?;
        let escaped = escape_literal(request_id);
        let batches = table
            .query()
            .only_if(format!("request_id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&token_request_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_token_requests(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn count_token_requests(&self, status: Option<&str>) -> Result<usize> {
        let table = self.token_requests_table().await?;
        let filter = status
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("status = '{}'", escape_literal(value)));
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count llm gateway token requests")?;
        Ok(total as usize)
    }

    pub async fn list_token_requests_page(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewayTokenRequestRecord>> {
        let total = self.count_token_requests(status).await?;
        if total == 0 || offset >= total {
            return Ok(vec![]);
        }
        let fetch_count = (total - offset).min(limit.max(1));
        let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
        let mut rows = self
            .query_token_requests(status, fetch_count, reverse_offset)
            .await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn query_token_requests(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewayTokenRequestRecord>> {
        let table = self.token_requests_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&token_request_columns()))
            .offset(offset)
            .limit(limit.max(1));
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            query = query.only_if(format!("status = '{}'", escape_literal(status)));
        }
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_token_requests(&batch_list)
    }

    pub async fn upsert_account_contribution_request(
        &self,
        record: &LlmGatewayAccountContributionRequestRecord,
    ) -> Result<()> {
        let table = self.account_contribution_requests_table().await?;
        let batch = build_account_contribution_requests_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["request_id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway account contribution request")?;
        Ok(())
    }

    pub async fn create_account_contribution_request(
        &self,
        input: NewLlmGatewayAccountContributionRequestInput,
    ) -> Result<LlmGatewayAccountContributionRequestRecord> {
        let now = now_ms();
        let record = LlmGatewayAccountContributionRequestRecord {
            request_id: input.request_id,
            account_name: input.account_name,
            account_id: input.account_id,
            id_token: input.id_token,
            access_token: input.access_token,
            refresh_token: input.refresh_token,
            requester_email: input.requester_email,
            contributor_message: input.contributor_message,
            github_id: input.github_id,
            frontend_page_url: input.frontend_page_url,
            status: LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING.to_string(),
            fingerprint: input.fingerprint,
            client_ip: input.client_ip,
            ip_region: input.ip_region,
            admin_note: None,
            failure_reason: None,
            imported_account_name: None,
            issued_key_id: None,
            issued_key_name: None,
            created_at: now,
            updated_at: now,
            processed_at: None,
        };
        self.upsert_account_contribution_request(&record).await?;
        Ok(record)
    }

    pub async fn get_account_contribution_request(
        &self,
        request_id: &str,
    ) -> Result<Option<LlmGatewayAccountContributionRequestRecord>> {
        let table = self.account_contribution_requests_table().await?;
        let escaped = escape_literal(request_id);
        let batches = table
            .query()
            .only_if(format!("request_id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&account_contribution_request_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_account_contribution_requests(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn count_account_contribution_requests(&self, status: Option<&str>) -> Result<usize> {
        let table = self.account_contribution_requests_table().await?;
        let filter = status
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("status = '{}'", escape_literal(value)));
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count llm gateway account contribution requests")?;
        Ok(total as usize)
    }

    pub async fn list_account_contribution_requests_page(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewayAccountContributionRequestRecord>> {
        let total = self.count_account_contribution_requests(status).await?;
        if total == 0 || offset >= total {
            return Ok(vec![]);
        }
        let fetch_count = (total - offset).min(limit.max(1));
        let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
        let mut rows = self
            .query_account_contribution_requests(status, fetch_count, reverse_offset)
            .await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn query_account_contribution_requests(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewayAccountContributionRequestRecord>> {
        let table = self.account_contribution_requests_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&account_contribution_request_columns()))
            .offset(offset)
            .limit(limit.max(1));
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            query = query.only_if(format!("status = '{}'", escape_literal(status)));
        }
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_account_contribution_requests(&batch_list)
    }

    pub async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> Result<Vec<LlmGatewayAccountContributionRequestRecord>> {
        let mut rows = self
            .list_account_contribution_requests_page(
                Some(LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED),
                limit.max(1),
                0,
            )
            .await?;
        rows.sort_by(|left, right| {
            right
                .processed_at
                .unwrap_or(right.created_at)
                .cmp(&left.processed_at.unwrap_or(left.created_at))
        });
        Ok(rows)
    }

    pub async fn upsert_sponsor_request(
        &self,
        record: &LlmGatewaySponsorRequestRecord,
    ) -> Result<()> {
        let table = self.sponsor_requests_table().await?;
        let batch = build_sponsor_requests_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["request_id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert llm gateway sponsor request")?;
        Ok(())
    }

    pub async fn create_sponsor_request(
        &self,
        input: NewLlmGatewaySponsorRequestInput,
    ) -> Result<LlmGatewaySponsorRequestRecord> {
        let now = now_ms();
        let record = LlmGatewaySponsorRequestRecord {
            request_id: input.request_id,
            requester_email: input.requester_email,
            sponsor_message: input.sponsor_message,
            display_name: input.display_name,
            github_id: input.github_id,
            frontend_page_url: input.frontend_page_url,
            status: LLM_GATEWAY_SPONSOR_REQUEST_STATUS_SUBMITTED.to_string(),
            fingerprint: input.fingerprint,
            client_ip: input.client_ip,
            ip_region: input.ip_region,
            admin_note: None,
            failure_reason: None,
            payment_email_sent_at: None,
            created_at: now,
            updated_at: now,
            processed_at: None,
        };
        self.upsert_sponsor_request(&record).await?;
        Ok(record)
    }

    pub async fn get_sponsor_request(
        &self,
        request_id: &str,
    ) -> Result<Option<LlmGatewaySponsorRequestRecord>> {
        let table = self.sponsor_requests_table().await?;
        let escaped = escape_literal(request_id);
        let batches = table
            .query()
            .only_if(format!("request_id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&sponsor_request_columns()))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_sponsor_requests(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn delete_sponsor_request(&self, request_id: &str) -> Result<()> {
        let table = self.sponsor_requests_table().await?;
        let escaped = escape_literal(request_id);
        table
            .delete(&format!("request_id = '{escaped}'"))
            .await
            .with_context(|| {
                format!("failed to delete llm gateway sponsor request `{request_id}`")
            })?;
        Ok(())
    }

    pub async fn count_sponsor_requests(&self, status: Option<&str>) -> Result<usize> {
        let table = self.sponsor_requests_table().await?;
        let filter = status
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("status = '{}'", escape_literal(value)));
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count llm gateway sponsor requests")?;
        Ok(total as usize)
    }

    pub async fn list_sponsor_requests_page(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewaySponsorRequestRecord>> {
        let total = self.count_sponsor_requests(status).await?;
        if total == 0 || offset >= total {
            return Ok(vec![]);
        }
        let fetch_count = (total - offset).min(limit.max(1));
        let reverse_offset = total.saturating_sub(offset.saturating_add(fetch_count));
        let mut rows = self
            .query_sponsor_requests(status, fetch_count, reverse_offset)
            .await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn query_sponsor_requests(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<LlmGatewaySponsorRequestRecord>> {
        let table = self.sponsor_requests_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&sponsor_request_columns()))
            .offset(offset)
            .limit(limit.max(1));
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            query = query.only_if(format!("status = '{}'", escape_literal(status)));
        }
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_sponsor_requests(&batch_list)
    }

    pub async fn list_public_sponsors(
        &self,
        limit: usize,
    ) -> Result<Vec<LlmGatewaySponsorRequestRecord>> {
        let mut rows = self
            .list_sponsor_requests_page(
                Some(LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED),
                limit.max(1),
                0,
            )
            .await?;
        rows.sort_by(|left, right| {
            right
                .processed_at
                .unwrap_or(right.created_at)
                .cmp(&left.processed_at.unwrap_or(left.created_at))
        });
        Ok(rows)
    }
}

pub async fn query_usage_event_rebuild_rows_from_connection(
    db: &Connection,
    table_name: &str,
    key_id: Option<&str>,
    provider_type: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<LlmGatewayUsageEventRecord>> {
    let table = db
        .open_table(table_name)
        .execute()
        .await
        .with_context(|| format!("failed to open llm gateway table `{table_name}`"))?;
    let mut query = table
        .query()
        .select(Select::columns(&usage_event_rebuild_columns()));
    if let Some(filter) = join_filters([
        key_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("key_id = '{}'", escape_literal(value))),
        provider_type
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("provider_type = '{}'", escape_literal(value))),
    ]) {
        query = query.only_if(filter);
    }
    if let Some(offset) = offset {
        query = query.offset(offset);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }
    let batches = query.execute().await?;
    let batch_list = batches.try_collect::<Vec<_>>().await?;
    batches_to_usage_events(&batch_list)
}

/// Run a SQL `GROUP BY key_id` aggregation over the raw usage-event dataset
/// and return per-key rollup totals. Uses Lance's built-in SQL engine so the
/// aggregation happens inside the storage layer without materializing all rows.
async fn aggregate_usage_rollups_from_dataset(
    dataset: &Dataset,
) -> Result<Vec<LlmGatewayKeyUsageRollupRecord>> {
    let sql = r#"
        SELECT
            key_id,
            CAST(COALESCE(SUM(input_uncached_tokens), 0) AS BIGINT) AS input_uncached_tokens,
            CAST(COALESCE(SUM(input_cached_tokens), 0) AS BIGINT) AS input_cached_tokens,
            CAST(COALESCE(SUM(output_tokens), 0) AS BIGINT) AS output_tokens,
            CAST(COALESCE(SUM(billable_tokens), 0) AS BIGINT) AS billable_tokens,
            CAST(COALESCE(SUM(credit_usage), 0.0) AS DOUBLE) AS credit_total,
            CAST(
                COALESCE(
                    SUM(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END),
                    0
                ) AS BIGINT
            ) AS credit_missing_events,
            MAX(created_at) AS last_used_at
        FROM dataset
        GROUP BY key_id
    "#;
    let batches = dataset
        .sql(sql)
        .table_name("dataset")
        .build()
        .await
        .context("failed to build usage rollup aggregate query")?
        .into_batch_records()
        .await
        .context("failed to execute usage rollup aggregate query")?;

    let mut rows = Vec::<LlmGatewayKeyUsageRollupRecord>::new();
    for batch in batches {
        let key_ids = batch
            .column_by_name("key_id")
            .context("aggregate result missing key_id")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("aggregate key_id column was not Utf8")?;
        let input_uncached_tokens = batch
            .column_by_name("input_uncached_tokens")
            .context("aggregate result missing input_uncached_tokens")?;
        let input_cached_tokens = batch
            .column_by_name("input_cached_tokens")
            .context("aggregate result missing input_cached_tokens")?;
        let output_tokens = batch
            .column_by_name("output_tokens")
            .context("aggregate result missing output_tokens")?;
        let billable_tokens = batch
            .column_by_name("billable_tokens")
            .context("aggregate result missing billable_tokens")?;
        let credit_total = batch
            .column_by_name("credit_total")
            .context("aggregate result missing credit_total")?
            .as_any()
            .downcast_ref::<Float64Array>()
            .context("aggregate credit_total column was not Float64")?;
        let credit_missing_events = batch
            .column_by_name("credit_missing_events")
            .context("aggregate result missing credit_missing_events")?;
        let last_used_at = batch
            .column_by_name("last_used_at")
            .context("aggregate result missing last_used_at")?
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .context("aggregate last_used_at column was not Timestamp(Millisecond)")?;

        for idx in 0..batch.num_rows() {
            rows.push(LlmGatewayKeyUsageRollupRecord {
                key_id: key_ids.value(idx).to_string(),
                input_uncached_tokens: array_value_as_u64(input_uncached_tokens, idx)
                    .context("failed to decode aggregate input_uncached_tokens")?,
                input_cached_tokens: array_value_as_u64(input_cached_tokens, idx)
                    .context("failed to decode aggregate input_cached_tokens")?,
                output_tokens: array_value_as_u64(output_tokens, idx)
                    .context("failed to decode aggregate output_tokens")?,
                billable_tokens: array_value_as_u64(billable_tokens, idx)
                    .context("failed to decode aggregate billable_tokens")?,
                credit_total: credit_total.value(idx),
                credit_missing_events: array_value_as_u64(credit_missing_events, idx)
                    .context("failed to decode aggregate credit_missing_events")?,
                last_used_at: (!last_used_at.is_null(idx)).then(|| last_used_at.value(idx)),
            });
        }
    }
    Ok(rows)
}

async fn aggregate_usage_event_counts_from_dataset(
    dataset: &Dataset,
) -> Result<LlmGatewayUsageEventCounts> {
    let sql = r#"
        SELECT
            COALESCE(provider_type, 'unknown') AS provider_type,
            key_id,
            CAST(COUNT(*) AS BIGINT) AS event_count
        FROM dataset
        GROUP BY COALESCE(provider_type, 'unknown'), key_id
    "#;
    let batches = dataset
        .sql(sql)
        .table_name("dataset")
        .build()
        .await
        .context("failed to build usage count aggregate query")?
        .into_batch_records()
        .await
        .context("failed to execute usage count aggregate query")?;

    let mut counts = LlmGatewayUsageEventCounts::default();
    for batch in batches {
        let provider_types = batch
            .column_by_name("provider_type")
            .context("aggregate result missing provider_type")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("aggregate provider_type column was not Utf8")?;
        let key_ids = batch
            .column_by_name("key_id")
            .context("aggregate result missing key_id")?
            .as_any()
            .downcast_ref::<StringArray>()
            .context("aggregate key_id column was not Utf8")?;
        let event_count = batch
            .column_by_name("event_count")
            .context("aggregate result missing event_count")?;

        for idx in 0..batch.num_rows() {
            let row_count = usize::try_from(
                array_value_as_u64(event_count, idx)
                    .context("failed to decode aggregate event_count")?,
            )
            .context("aggregate event_count did not fit into usize")?;
            counts.total_event_count = counts.total_event_count.saturating_add(row_count);
            *counts
                .provider_event_counts
                .entry(provider_types.value(idx).to_string())
                .or_default() += row_count;
            *counts
                .key_event_counts
                .entry(key_ids.value(idx).to_string())
                .or_default() += row_count;
        }
    }

    Ok(counts)
}

/// Extract a `u64` from an Arrow array at `idx`, accepting both `UInt64` and
/// non-negative `Int64` (Lance SQL CAST may produce either).
fn array_value_as_u64(array: &dyn Array, idx: usize) -> Result<u64> {
    if let Some(values) = array.as_any().downcast_ref::<UInt64Array>() {
        return Ok(values.value(idx));
    }
    if let Some(values) = array.as_any().downcast_ref::<Int64Array>() {
        return u64::try_from(values.value(idx)).context(
            "aggregate produced a negative Int64 where a non-negative value was expected",
        );
    }
    anyhow::bail!("aggregate column had unsupported integer type")
}

/// Joins an iterator of optional SQL filter clauses with ` AND `.
///
/// `None` and empty/whitespace-only entries are silently dropped.
/// Returns `None` when no clauses survive, suitable for passing directly
/// to LanceDB's `count_rows(Option<String>)`.
fn join_filters<I>(filters: I) -> Option<String>
where
    I: IntoIterator<Item = Option<String>>,
{
    let parts = filters
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" AND "))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap, fs, fs::OpenOptions, path::PathBuf, sync::Arc, time::Duration,
    };

    use arrow_array::{
        builder::{
            BooleanBuilder, Float64Builder, StringBuilder, TimestampMillisecondBuilder,
            UInt64Builder,
        },
        RecordBatch,
    };
    use arrow_schema::{DataType, Field, Schema};
    use fs2::FileExt;
    use lancedb::connect;

    use super::*;

    fn temp_store_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("static-flow-llm-gateway-store-{name}-{}", now_ms()))
    }

    fn sample_key_record(id: &str, name: &str) -> LlmGatewayKeyRecord {
        let now = now_ms();
        LlmGatewayKeyRecord {
            id: id.to_string(),
            name: name.to_string(),
            secret: "sf-test-secret".to_string(),
            key_hash: "sf-test-hash".to_string(),
            status: LLM_GATEWAY_KEY_STATUS_ACTIVE.to_string(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            protocol_family: LLM_GATEWAY_PROTOCOL_ANTHROPIC.to_string(),
            public_visible: false,
            quota_billable_limit: 1_000,
            usage_input_uncached_tokens: 0,
            usage_input_cached_tokens: 0,
            usage_output_tokens: 0,
            usage_billable_tokens: 0,
            usage_credit_total: 0.0,
            usage_credit_missing_events: 0,
            last_used_at: None,
            created_at: now,
            updated_at: now,
            route_strategy: None,
            fixed_account_name: None,
            auto_account_names: None,
            account_group_id: None,
            model_name_map: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            kiro_request_validation_enabled: true,
            kiro_cache_estimation_enabled: true,
            kiro_cache_policy_override_json: None,
            kiro_billable_model_multipliers_override_json: None,
        }
    }

    fn sample_account_group_record(
        id: &str,
        provider_type: &str,
        name: &str,
        account_names: &[&str],
    ) -> LlmGatewayAccountGroupRecord {
        let now = now_ms();
        LlmGatewayAccountGroupRecord {
            id: id.to_string(),
            provider_type: provider_type.to_string(),
            name: name.to_string(),
            account_names: account_names
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn create_key_inserts_and_upsert_key_updates() {
        let dir = temp_store_dir("key-roundtrip");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let record = sample_key_record("test-key-1", "Test Key");
        store.create_key(&record).await.expect("create key");

        let loaded = store
            .get_key_by_id(&record.id)
            .await
            .expect("load created key")
            .expect("created key exists");
        assert_eq!(loaded.name, "Test Key");
        assert_eq!(loaded.provider_type, LLM_GATEWAY_PROVIDER_KIRO);

        let mut updated = loaded.clone();
        updated.status = LLM_GATEWAY_KEY_STATUS_DISABLED.to_string();
        updated.model_name_map = Some(BTreeMap::from([(
            "claude-haiku-4-5-20251001".to_string(),
            "claude-sonnet-4-6".to_string(),
        )]));
        updated.request_max_concurrency = Some(2);
        updated.request_min_start_interval_ms = Some(1_250);
        updated.kiro_request_validation_enabled = false;
        updated.kiro_cache_estimation_enabled = false;
        updated.updated_at = now_ms();
        store.upsert_key(&updated).await.expect("update key");

        let reloaded = store
            .get_key_by_id(&record.id)
            .await
            .expect("load updated key")
            .expect("updated key exists");
        assert_eq!(reloaded.status, LLM_GATEWAY_KEY_STATUS_DISABLED);
        assert_eq!(reloaded.model_name_map, updated.model_name_map);
        assert_eq!(reloaded.request_max_concurrency, Some(2));
        assert_eq!(reloaded.request_min_start_interval_ms, Some(1_250));
        assert!(!reloaded.kiro_request_validation_enabled);
        assert!(!reloaded.kiro_cache_estimation_enabled);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replace_key_can_clear_nullable_request_limit_fields() {
        let dir = temp_store_dir("key-replace-clears-nullable");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let mut record = sample_key_record("test-key-clear", "Clearable Key");
        record.model_name_map = Some(BTreeMap::from([(
            "claude-haiku-4-5-20251001".to_string(),
            "claude-sonnet-4-6".to_string(),
        )]));
        record.request_max_concurrency = Some(3);
        record.request_min_start_interval_ms = Some(1_500);
        record.kiro_cache_policy_override_json =
            Some(r#"{"high_credit_diagnostic_threshold":1.4}"#.to_string());
        store.create_key(&record).await.expect("create key");

        let mut updated = record.clone();
        updated.model_name_map = None;
        updated.request_max_concurrency = None;
        updated.request_min_start_interval_ms = None;
        updated.kiro_cache_policy_override_json = None;
        updated.updated_at = now_ms();
        store.replace_key(&updated).await.expect("replace key");

        let reloaded = store
            .get_key_by_id(&record.id)
            .await
            .expect("load replaced key")
            .expect("replaced key exists");
        assert_eq!(reloaded.model_name_map, None);
        assert_eq!(reloaded.request_max_concurrency, None);
        assert_eq!(reloaded.request_min_start_interval_ms, None);
        assert_eq!(reloaded.kiro_cache_policy_override_json, None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn create_and_list_account_groups_round_trip() {
        let dir = temp_store_dir("account-group-roundtrip");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let record = sample_account_group_record(
            "group-codex-team-a",
            LLM_GATEWAY_PROVIDER_CODEX,
            "Codex Team A",
            &["alpha", "beta"],
        );
        store
            .create_account_group(&record)
            .await
            .expect("create account group");

        let loaded = store
            .get_account_group_by_id(&record.id)
            .await
            .expect("load account group")
            .expect("created account group exists");
        assert_eq!(loaded.name, "Codex Team A");
        assert_eq!(loaded.provider_type, LLM_GATEWAY_PROVIDER_CODEX);
        assert_eq!(loaded.account_names, vec!["alpha".to_string(), "beta".to_string()]);

        let listed = store
            .list_account_groups_for_provider(LLM_GATEWAY_PROVIDER_CODEX)
            .await
            .expect("list codex account groups");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, record.id);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn key_round_trip_preserves_account_group_id() {
        let dir = temp_store_dir("key-account-group-id");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let group = sample_account_group_record(
            "group-kiro-migrated",
            LLM_GATEWAY_PROVIDER_KIRO,
            "Migrated Kiro Pool",
            &["kiro-a", "kiro-b"],
        );
        store
            .create_account_group(&group)
            .await
            .expect("create account group");

        let mut key = sample_key_record("test-key-group", "Grouped Key");
        key.account_group_id = Some(group.id.clone());
        store.create_key(&key).await.expect("create key");

        let loaded = store
            .get_key_by_id(&key.id)
            .await
            .expect("load key")
            .expect("grouped key exists");
        assert_eq!(loaded.account_group_id.as_deref(), Some(group.id.as_str()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_account_failure_retry_limit() {
        let dir = temp_store_dir("runtime-config-retry-limit");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            account_failure_retry_limit: 7,
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(loaded.account_failure_retry_limit, 7);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_polling_and_usage_flush_fields() {
        let dir = temp_store_dir("runtime-config-polling-and-flush");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            codex_status_refresh_min_interval_seconds: 240,
            codex_status_refresh_max_interval_seconds: 300,
            codex_status_account_jitter_max_seconds: 10,
            kiro_status_refresh_min_interval_seconds: 240,
            kiro_status_refresh_max_interval_seconds: 300,
            kiro_status_account_jitter_max_seconds: 10,
            usage_event_flush_batch_size: 256,
            usage_event_flush_interval_seconds: 15,
            usage_event_flush_max_buffer_bytes: 8 * 1024 * 1024,
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(loaded.codex_status_refresh_min_interval_seconds, 240);
        assert_eq!(loaded.codex_status_refresh_max_interval_seconds, 300);
        assert_eq!(loaded.codex_status_account_jitter_max_seconds, 10);
        assert_eq!(loaded.kiro_status_refresh_min_interval_seconds, 240);
        assert_eq!(loaded.kiro_status_refresh_max_interval_seconds, 300);
        assert_eq!(loaded.kiro_status_account_jitter_max_seconds, 10);
        assert_eq!(loaded.usage_event_flush_batch_size, 256);
        assert_eq!(loaded.usage_event_flush_interval_seconds, 15);
        assert_eq!(loaded.usage_event_flush_max_buffer_bytes, 8 * 1024 * 1024);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn runtime_config_default_uses_finite_usage_event_detail_retention() {
        assert_eq!(LlmGatewayRuntimeConfigRecord::default().usage_event_detail_retention_days, 7);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_kiro_cache_kmodels_json() {
        let dir = temp_store_dir("runtime-config-kiro-cache-kmodels");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            kiro_cache_kmodels_json: r#"{"claude-opus-4-6":8.061927916785985e-06,"claude-sonnet-4-6":5.055065250835128e-06}"#.to_string(),
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(loaded.kiro_cache_kmodels_json, config.kiro_cache_kmodels_json);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_kiro_billable_model_multipliers_json() {
        let dir = temp_store_dir("runtime-config-kiro-billable-multipliers");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            kiro_billable_model_multipliers_json: r#"{"haiku":0.8,"opus":1.6,"sonnet":1.2}"#
                .to_string(),
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(
            loaded.kiro_billable_model_multipliers_json,
            config.kiro_billable_model_multipliers_json
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_kiro_prefix_cache_and_anchor_fields() {
        let dir = temp_store_dir("runtime-config-kiro-prefix-cache-anchor");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            kiro_prefix_cache_mode: "prefix_tree".to_string(),
            kiro_prefix_cache_max_tokens: 262_144,
            kiro_prefix_cache_entry_ttl_seconds: 1_800,
            kiro_conversation_anchor_max_entries: 1_024,
            kiro_conversation_anchor_ttl_seconds: 43_200,
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(loaded.kiro_prefix_cache_mode, "prefix_tree");
        assert_eq!(loaded.kiro_prefix_cache_max_tokens, 262_144);
        assert_eq!(loaded.kiro_prefix_cache_entry_ttl_seconds, 1_800);
        assert_eq!(loaded.kiro_conversation_anchor_max_entries, 1_024);
        assert_eq!(loaded.kiro_conversation_anchor_ttl_seconds, 43_200);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn runtime_config_round_trip_preserves_kiro_cache_policy_json() {
        let dir = temp_store_dir("runtime-config-kiro-cache-policy");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let config = LlmGatewayRuntimeConfigRecord {
            kiro_cache_policy_json: r#"{"small_input_high_credit_boost":{"target_input_tokens":80000,"credit_start":0.9,"credit_end":1.6},"prefix_tree_credit_ratio_bands":[{"credit_start":0.2,"credit_end":0.8,"cache_ratio_start":0.6,"cache_ratio_end":0.3},{"credit_start":0.8,"credit_end":1.4,"cache_ratio_start":0.3,"cache_ratio_end":0.1}],"high_credit_diagnostic_threshold":1.4}"#.to_string(),
            updated_at: now_ms(),
            ..LlmGatewayRuntimeConfigRecord::default()
        };
        store
            .upsert_runtime_config(&config)
            .await
            .expect("upsert runtime config");

        let loaded = store
            .get_runtime_config_or_default()
            .await
            .expect("load runtime config");
        assert_eq!(loaded.kiro_cache_policy_json, config.kiro_cache_policy_json);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn key_round_trip_preserves_kiro_cache_policy_override_json() {
        let dir = temp_store_dir("key-kiro-cache-policy-override");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let mut key = sample_key_record("test-key-cache-policy", "Policy override key");
        key.kiro_cache_policy_override_json = Some(r#"{"small_input_high_credit_boost":{"target_input_tokens":120000},"high_credit_diagnostic_threshold":1.75}"#.to_string());
        store
            .create_key(&key)
            .await
            .expect("create key with override");

        let loaded = store
            .get_key_by_id(&key.id)
            .await
            .expect("load key")
            .expect("created key exists");
        assert_eq!(loaded.kiro_cache_policy_override_json, key.kiro_cache_policy_override_json,);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn key_round_trip_preserves_kiro_billable_model_multipliers_override_json() {
        let dir = temp_store_dir("key-kiro-billable-multipliers-override");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let mut key = sample_key_record("test-key-billable-multipliers", "Multiplier override key");
        key.kiro_billable_model_multipliers_override_json =
            Some(r#"{"haiku":0.8,"opus":1.6}"#.to_string());
        store
            .create_key(&key)
            .await
            .expect("create key with billable multiplier override");

        let loaded = store
            .get_key_by_id(&key.id)
            .await
            .expect("load key")
            .expect("created key exists");
        assert_eq!(
            loaded.kiro_billable_model_multipliers_override_json,
            key.kiro_billable_model_multipliers_override_json,
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn apply_usage_event_tracks_kiro_credit_rollups() {
        let dir = temp_store_dir("kiro-credit-rollup");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("test-key-credit", "Credit Key");
        store.create_key(&key).await.expect("create key");

        let now = now_ms();
        let event = LlmGatewayUsageEventRecord {
            id: "evt-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            latency_ms: 42,
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status_code: 200,
            input_uncached_tokens: 10,
            input_cached_tokens: 0,
            output_tokens: 5,
            billable_tokens: 15,
            usage_missing: false,
            credit_usage: Some(0.125),
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some("{\"conversationState\":{}}".to_string()),
            full_request_json: Some("{\"messages\":[]}".to_string()),
            created_at: now,
        };
        store
            .append_usage_event(&event)
            .await
            .expect("append usage event");
        let rollups = store
            .aggregate_usage_rollups()
            .await
            .expect("aggregate usage rollups");
        let updated = rollups
            .iter()
            .find(|row| row.key_id == key.id)
            .expect("rollup row for key");
        assert_eq!(updated.credit_total, 0.125);
        assert_eq!(updated.credit_missing_events, 0);

        let missing = LlmGatewayUsageEventRecord {
            id: "evt-2".to_string(),
            created_at: now + 1,
            credit_usage: None,
            credit_usage_missing: true,
            ..event
        };
        store
            .append_usage_event(&missing)
            .await
            .expect("append missing-credit usage event");
        let rollups = store
            .aggregate_usage_rollups()
            .await
            .expect("aggregate usage rollups after missing event");
        let updated = rollups
            .iter()
            .find(|row| row.key_id == key.id)
            .expect("rollup row for key after missing event");
        assert_eq!(updated.credit_total, 0.125);
        assert_eq!(updated.credit_missing_events, 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn connect_creates_usage_events_provider_type_index_and_provider_filters_work() {
        let dir = temp_store_dir("usage-provider-type-index");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let table = store
            .connection()
            .open_table(LLM_GATEWAY_USAGE_EVENTS_TABLE)
            .execute()
            .await
            .expect("open usage events table");
        let indexes = table
            .list_indices()
            .await
            .expect("list usage table indices");
        assert!(indexes
            .iter()
            .any(|index| { index.columns.len() == 1 && index.columns[0] == "provider_type" }));

        let key = sample_key_record("test-key-provider-filter", "Provider Filter Key");
        store.create_key(&key).await.expect("create key");

        let now = now_ms();
        let base_event = LlmGatewayUsageEventRecord {
            id: "evt-provider-base".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 12,
            endpoint: "/generateAssistantResponse".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status_code: 200,
            input_uncached_tokens: 10,
            input_cached_tokens: 0,
            output_tokens: 5,
            billable_tokens: 15,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now,
        };
        store
            .append_usage_event(&base_event)
            .await
            .expect("append kiro usage event");
        store
            .append_usage_event(&LlmGatewayUsageEventRecord {
                id: "evt-provider-codex".to_string(),
                provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
                created_at: now + 1,
                ..base_event.clone()
            })
            .await
            .expect("append codex usage event");

        assert_eq!(
            store
                .count_usage_events_for_provider(None, Some(LLM_GATEWAY_PROVIDER_KIRO))
                .await
                .expect("count kiro events"),
            1
        );
        assert_eq!(
            store
                .count_usage_events_for_provider(None, Some(LLM_GATEWAY_PROVIDER_CODEX))
                .await
                .expect("count codex events"),
            1
        );
        assert_eq!(
            store
                .query_usage_events(None, Some(LLM_GATEWAY_PROVIDER_KIRO), Some(10), Some(0))
                .await
                .expect("query kiro events")
                .len(),
            1
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn query_usage_events_since_filters_by_created_at_lower_bound() {
        let dir = temp_store_dir("usage-created-at-lower-bound");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("test-key-created-at", "Created At Key");
        store.create_key(&key).await.expect("create key");

        let now = now_ms();
        let base_event = LlmGatewayUsageEventRecord {
            id: "evt-created-at-1".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 25,
            endpoint: "/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 10,
            input_cached_tokens: 3,
            output_tokens: 5,
            billable_tokens: 15,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now - 10_000,
        };
        store
            .append_usage_event(&base_event)
            .await
            .expect("append older event");
        store
            .append_usage_event(&LlmGatewayUsageEventRecord {
                id: "evt-created-at-2".to_string(),
                client_request_body_json: Some("{\"messages\":[]}".to_string()),
                upstream_request_body_json: Some("{\"conversationState\":{}}".to_string()),
                full_request_json: Some("{\"messages\":[]}".to_string()),
                created_at: now - 1_000,
                ..base_event.clone()
            })
            .await
            .expect("append newer event");

        let filtered = store
            .query_usage_events_since(
                Some(&key.id),
                Some(LLM_GATEWAY_PROVIDER_CODEX),
                Some(now - 5_000),
                None,
                None,
            )
            .await
            .expect("query filtered events");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "evt-created-at-2");
        assert_eq!(filtered[0].client_request_body_json.as_deref(), Some("{\"messages\":[]}"));
        assert_eq!(
            filtered[0].upstream_request_body_json.as_deref(),
            Some("{\"conversationState\":{}}")
        );
        assert_eq!(filtered[0].full_request_json.as_deref(), Some("{\"messages\":[]}"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn usage_event_round_trip_preserves_full_request_json() {
        let dir = temp_store_dir("usage-full-request-json");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("test-key-full-request", "Full Request Key");
        store.create_key(&key).await.expect("create key");

        let record = LlmGatewayUsageEventRecord {
            id: "evt-full-request".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 25,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 12,
            input_cached_tokens: 3,
            output_tokens: 5,
            billable_tokens: 40,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: Some(
                "{\"model\":\"gpt-5\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}]}"
                    .to_string(),
            ),
            created_at: now_ms(),
        };

        store
            .append_usage_event(&record)
            .await
            .expect("append usage event");
        let loaded = store
            .list_usage_events(Some(&key.id), Some(1))
            .await
            .expect("list usage events");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].full_request_json, record.full_request_json);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn query_usage_event_summaries_keep_preview_and_exclude_detail_payloads() {
        let dir = temp_store_dir("usage-summary-projection");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("test-key-summary", "Summary Key");
        store.create_key(&key).await.expect("create key");

        let record = LlmGatewayUsageEventRecord {
            id: "evt-summary".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            latency_ms: 21,
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-6".to_string()),
            status_code: 200,
            input_uncached_tokens: 14,
            input_cached_tokens: 2,
            output_tokens: 3,
            billable_tokens: 31,
            usage_missing: false,
            credit_usage: Some(0.75),
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"x-test\":\"1\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some("{\"conversationState\":{}}".to_string()),
            full_request_json: Some("{\"messages\":[]}".to_string()),
            created_at: now_ms(),
        };
        store
            .append_usage_event(&record)
            .await
            .expect("append usage event");

        let rows = store
            .query_usage_event_summaries(
                Some(&key.id),
                Some(LLM_GATEWAY_PROVIDER_KIRO),
                Some(10),
                Some(0),
            )
            .await
            .expect("query usage summaries");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, record.id);
        assert_eq!(rows[0].billable_tokens, record.billable_tokens);
        assert_eq!(rows[0].request_url, record.request_url);
        assert_eq!(rows[0].last_message_content, record.last_message_content);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn get_usage_event_detail_by_id_returns_heavy_fields() {
        let dir = temp_store_dir("usage-detail-lookup");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("test-key-detail", "Detail Key");
        store.create_key(&key).await.expect("create key");

        let record = LlmGatewayUsageEventRecord {
            id: "evt-detail".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 19,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 20,
            input_cached_tokens: 0,
            output_tokens: 4,
            billable_tokens: 40,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"x-test\":\"1\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[]}".to_string()),
            upstream_request_body_json: Some("{\"input\":[\"hello\"]}".to_string()),
            full_request_json: Some("{\"messages\":[]}".to_string()),
            created_at: now_ms(),
        };
        store
            .append_usage_event(&record)
            .await
            .expect("append usage event");

        let loaded = store
            .get_usage_event_detail_by_id(&record.id)
            .await
            .expect("load usage detail")
            .expect("usage detail exists");

        assert_eq!(loaded.id, record.id);
        assert_eq!(loaded.request_headers_json, record.request_headers_json);
        assert_eq!(loaded.client_request_body_json, record.client_request_body_json);
        assert_eq!(loaded.upstream_request_body_json, record.upstream_request_body_json);
        assert_eq!(loaded.full_request_json, record.full_request_json);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn append_usage_events_waits_for_usage_table_lock() {
        let dir = temp_store_dir("usage-append-lock");
        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );

        let key = sample_key_record("test-key-lock", "Lock Key");
        store.create_key(&key).await.expect("create key");
        let event = LlmGatewayUsageEventRecord {
            id: "evt-lock".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 19,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 20,
            input_cached_tokens: 0,
            output_tokens: 4,
            billable_tokens: 40,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"x-test\":\"1\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms(),
        };

        let lock_path =
            local_table_access_lock_path(&dir.to_string_lossy(), LLM_GATEWAY_USAGE_EVENTS_TABLE);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).expect("create table lock dir");
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open usage-events table lock");
        file.lock_exclusive()
            .expect("acquire exclusive usage-events table lock");

        let store_for_task = Arc::clone(&store);
        let handle = tokio::spawn(async move { store_for_task.append_usage_event(&event).await });
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !handle.is_finished(),
            "append_usage_event must wait for the usage-events table lock"
        );

        file.unlock()
            .expect("release exclusive usage-events table lock");
        handle
            .await
            .expect("append task join")
            .expect("append usage event after lock release");

        let persisted = store
            .get_usage_event_detail_by_id("evt-lock")
            .await
            .expect("load appended event");
        assert!(persisted.is_some(), "usage event should persist after lock release");

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn append_usage_events_does_not_wait_for_usage_table_rewrite_lock() {
        let dir = temp_store_dir("usage-append-rewrite-lock");
        let store = Arc::new(
            LlmGatewayStore::connect(&dir.to_string_lossy())
                .await
                .expect("connect llm gateway store"),
        );

        let key = sample_key_record("test-key-rewrite-lock", "Rewrite Lock Key");
        store.create_key(&key).await.expect("create key");
        let event = LlmGatewayUsageEventRecord {
            id: "evt-rewrite-lock".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("default".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 21,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 24,
            input_cached_tokens: 0,
            output_tokens: 5,
            billable_tokens: 48,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"x-test\":\"rewrite\"}".to_string(),
            last_message_content: Some("hello rewrite".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now_ms(),
        };

        let lock_path = crate::optimize::local_table_rewrite_lock_path(
            &dir.to_string_lossy(),
            LLM_GATEWAY_USAGE_EVENTS_TABLE,
        );
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).expect("create rewrite lock dir");
        }
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .expect("open usage-events rewrite lock");
        file.lock_exclusive()
            .expect("acquire exclusive usage-events rewrite lock");

        let store_for_task = Arc::clone(&store);
        let handle = tokio::spawn(async move { store_for_task.append_usage_event(&event).await });
        handle
            .await
            .expect("append task join")
            .expect("append usage event while rewrite lock is held");

        let persisted = store
            .get_usage_event_detail_by_id("evt-rewrite-lock")
            .await
            .expect("load appended event");
        assert!(
            persisted.is_some(),
            "usage event should persist even while the rewrite lock is held"
        );

        file.unlock()
            .expect("release exclusive usage-events rewrite lock");
        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn connect_canonicalizes_legacy_nullable_key_fields() {
        let dir = temp_store_dir("legacy-key-canonicalize");
        let db = connect(&dir.to_string_lossy())
            .execute()
            .await
            .expect("connect raw lancedb");
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("name", DataType::Utf8, false),
            Field::new("secret", DataType::Utf8, false),
            Field::new("key_hash", DataType::Utf8, false),
            Field::new("status", DataType::Utf8, false),
            Field::new("public_visible", DataType::Boolean, false),
            Field::new("quota_billable_limit", DataType::UInt64, false),
            Field::new("usage_input_uncached_tokens", DataType::UInt64, false),
            Field::new("usage_input_cached_tokens", DataType::UInt64, false),
            Field::new("usage_output_tokens", DataType::UInt64, false),
            Field::new("usage_billable_tokens", DataType::UInt64, false),
            Field::new(
                "last_used_at",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                true,
            ),
            Field::new(
                "created_at",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                false,
            ),
            Field::new(
                "updated_at",
                DataType::Timestamp(arrow_schema::TimeUnit::Millisecond, None),
                false,
            ),
            Field::new("route_strategy", DataType::Utf8, true),
            Field::new("fixed_account_name", DataType::Utf8, true),
            Field::new("auto_account_names_json", DataType::Utf8, true),
            Field::new("provider_type", DataType::Utf8, true),
            Field::new("protocol_family", DataType::Utf8, true),
            Field::new("usage_credit_total", DataType::Float64, true),
            Field::new("usage_credit_missing_events", DataType::UInt64, true),
            Field::new("request_max_concurrency", DataType::UInt64, true),
            Field::new("request_min_start_interval_ms", DataType::UInt64, true),
        ]));
        let now = now_ms();
        let mut id = StringBuilder::new();
        let mut name = StringBuilder::new();
        let mut secret = StringBuilder::new();
        let mut key_hash = StringBuilder::new();
        let mut status = StringBuilder::new();
        let mut public_visible = BooleanBuilder::new();
        let mut quota_billable_limit = UInt64Builder::new();
        let mut usage_input_uncached_tokens = UInt64Builder::new();
        let mut usage_input_cached_tokens = UInt64Builder::new();
        let mut usage_output_tokens = UInt64Builder::new();
        let mut usage_billable_tokens = UInt64Builder::new();
        let mut last_used_at = TimestampMillisecondBuilder::new();
        let mut created_at = TimestampMillisecondBuilder::new();
        let mut updated_at = TimestampMillisecondBuilder::new();
        let mut route_strategy = StringBuilder::new();
        let mut fixed_account_name = StringBuilder::new();
        let mut auto_account_names_json = StringBuilder::new();
        let mut provider_type = StringBuilder::new();
        let mut protocol_family = StringBuilder::new();
        let mut usage_credit_total = Float64Builder::new();
        let mut usage_credit_missing_events = UInt64Builder::new();
        let mut request_max_concurrency = UInt64Builder::new();
        let mut request_min_start_interval_ms = UInt64Builder::new();
        id.append_value("legacy-key");
        name.append_value("Legacy Key");
        secret.append_value("sf-legacy-secret");
        key_hash.append_value("sf-legacy-hash");
        status.append_value(LLM_GATEWAY_KEY_STATUS_ACTIVE);
        public_visible.append_value(true);
        quota_billable_limit.append_value(1000);
        usage_input_uncached_tokens.append_value(0);
        usage_input_cached_tokens.append_value(0);
        usage_output_tokens.append_value(0);
        usage_billable_tokens.append_value(0);
        last_used_at.append_null();
        created_at.append_value(now);
        updated_at.append_value(now);
        route_strategy.append_null();
        fixed_account_name.append_null();
        auto_account_names_json.append_null();
        provider_type.append_null();
        protocol_family.append_null();
        usage_credit_total.append_null();
        usage_credit_missing_events.append_null();
        request_max_concurrency.append_null();
        request_min_start_interval_ms.append_null();
        let batch = RecordBatch::try_new(schema, vec![
            Arc::new(id.finish()),
            Arc::new(name.finish()),
            Arc::new(secret.finish()),
            Arc::new(key_hash.finish()),
            Arc::new(status.finish()),
            Arc::new(public_visible.finish()),
            Arc::new(quota_billable_limit.finish()),
            Arc::new(usage_input_uncached_tokens.finish()),
            Arc::new(usage_input_cached_tokens.finish()),
            Arc::new(usage_output_tokens.finish()),
            Arc::new(usage_billable_tokens.finish()),
            Arc::new(last_used_at.finish()),
            Arc::new(created_at.finish()),
            Arc::new(updated_at.finish()),
            Arc::new(route_strategy.finish()),
            Arc::new(fixed_account_name.finish()),
            Arc::new(auto_account_names_json.finish()),
            Arc::new(provider_type.finish()),
            Arc::new(protocol_family.finish()),
            Arc::new(usage_credit_total.finish()),
            Arc::new(usage_credit_missing_events.finish()),
            Arc::new(request_max_concurrency.finish()),
            Arc::new(request_min_start_interval_ms.finish()),
        ])
        .expect("build legacy batch");
        db.create_table(LLM_GATEWAY_KEYS_TABLE, batch)
            .storage_option("new_table_enable_stable_row_ids", "true")
            .storage_option("new_table_enable_v2_manifest_paths", "true")
            .execute()
            .await
            .expect("create legacy keys table");

        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");
        let reloaded = store
            .get_key_by_id("legacy-key")
            .await
            .expect("load canonicalized key")
            .expect("legacy key exists");
        assert_eq!(reloaded.provider_type, LLM_GATEWAY_PROVIDER_CODEX);
        assert_eq!(reloaded.protocol_family, LLM_GATEWAY_PROTOCOL_OPENAI);
        assert_eq!(reloaded.usage_credit_total, 0.0);
        assert_eq!(reloaded.usage_credit_missing_events, 0);
        assert!(reloaded.kiro_request_validation_enabled);
        assert!(reloaded.kiro_cache_estimation_enabled);

        let schema = store
            .connection()
            .open_table(LLM_GATEWAY_KEYS_TABLE)
            .execute()
            .await
            .expect("open canonicalized keys table")
            .schema()
            .await
            .expect("load canonicalized schema");
        assert!(!schema
            .field_with_name("provider_type")
            .expect("provider_type field")
            .is_nullable());
        assert!(!schema
            .field_with_name("protocol_family")
            .expect("protocol_family field")
            .is_nullable());
        assert!(!schema
            .field_with_name("usage_credit_total")
            .expect("usage_credit_total field")
            .is_nullable());
        assert!(!schema
            .field_with_name("usage_credit_missing_events")
            .expect("usage_credit_missing_events field")
            .is_nullable());

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn aggregate_usage_event_counts_groups_by_provider_and_key() {
        let dir = temp_store_dir("usage-event-counts");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let key = sample_key_record("key-count", "Count Key");
        store.create_key(&key).await.expect("create key");

        let now = now_ms();
        let kiro_event = LlmGatewayUsageEventRecord {
            id: "evt-kiro".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_KIRO.to_string(),
            account_name: Some("alpha".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/kiro-gateway/v1/messages".to_string(),
            latency_ms: 10,
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-sonnet-4-5".to_string()),
            status_code: 200,
            input_uncached_tokens: 1,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 6,
            usage_missing: false,
            credit_usage: Some(1.0),
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"messages\":[\"hello\"]}".to_string()),
            upstream_request_body_json: Some("{\"conversationState\":{\"id\":1}}".to_string()),
            full_request_json: Some("{\"messages\":[\"hello\"]}".to_string()),
            created_at: now,
        };
        let codex_event = LlmGatewayUsageEventRecord {
            id: "evt-codex".to_string(),
            key_id: key.id.clone(),
            key_name: key.name.clone(),
            provider_type: LLM_GATEWAY_PROVIDER_CODEX.to_string(),
            account_name: Some("beta".to_string()),
            request_method: "POST".to_string(),
            request_url: "/api/llm-gateway/v1/responses".to_string(),
            latency_ms: 12,
            endpoint: "/v1/responses".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            status_code: 200,
            input_uncached_tokens: 2,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 7,
            usage_missing: false,
            credit_usage: None,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some("world".to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            created_at: now + 1,
        };

        store
            .append_usage_events(&[kiro_event, codex_event])
            .await
            .expect("append usage events");

        let counts = store
            .aggregate_usage_event_counts()
            .await
            .expect("aggregate usage counts");

        assert_eq!(counts.total_event_count, 2);
        assert_eq!(counts.provider_event_counts.get(LLM_GATEWAY_PROVIDER_KIRO), Some(&1));
        assert_eq!(counts.provider_event_counts.get(LLM_GATEWAY_PROVIDER_CODEX), Some(&1));
        assert_eq!(counts.key_event_counts.get(&key.id), Some(&2));

        let _ = fs::remove_dir_all(&dir);
    }
}
