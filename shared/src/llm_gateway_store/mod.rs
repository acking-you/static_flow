//! LanceDB-backed storage for StaticFlow's LLM gateway.
//!
//! This module is the persistence boundary for gateway keys, usage events,
//! runtime config, upstream proxy configs/bindings, and the public request
//! queues shown in the admin UI.

mod codec;
mod schema;
mod types;

use anyhow::{Context, Result};
use arrow_array::{RecordBatchIterator, RecordBatchReader};
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase, Select},
    Connection, Table,
};

pub use self::types::{
    now_ms, LlmGatewayAccountContributionRequestRecord, LlmGatewayKeyRecord,
    LlmGatewayProxyBindingRecord, LlmGatewayProxyConfigRecord, LlmGatewayRuntimeConfigRecord,
    LlmGatewaySponsorRequestRecord, LlmGatewayTokenRequestRecord, LlmGatewayUsageEventRecord,
    NewLlmGatewayAccountContributionRequestInput, NewLlmGatewaySponsorRequestInput,
    NewLlmGatewayTokenRequestInput, DEFAULT_KIRO_CHANNEL_MAX_CONCURRENCY,
    DEFAULT_KIRO_CHANNEL_MIN_START_INTERVAL_MS, DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS,
    DEFAULT_LLM_GATEWAY_MAX_REQUEST_BODY_BYTES, LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
    LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED,
    LLM_GATEWAY_PROTOCOL_ANTHROPIC, LLM_GATEWAY_PROTOCOL_OPENAI, LLM_GATEWAY_PROVIDER_CODEX,
    LLM_GATEWAY_PROVIDER_KIRO, LLM_GATEWAY_PROXY_BINDINGS_TABLE, LLM_GATEWAY_PROXY_CONFIGS_TABLE,
    LLM_GATEWAY_RUNTIME_CONFIG_TABLE, LLM_GATEWAY_SPONSOR_REQUESTS_TABLE,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_APPROVED,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT,
    LLM_GATEWAY_SPONSOR_REQUEST_STATUS_SUBMITTED, LLM_GATEWAY_TABLE_NAMES,
    LLM_GATEWAY_TOKEN_REQUESTS_TABLE, LLM_GATEWAY_TOKEN_REQUEST_STATUS_FAILED,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_ISSUED, LLM_GATEWAY_TOKEN_REQUEST_STATUS_PENDING,
    LLM_GATEWAY_TOKEN_REQUEST_STATUS_REJECTED, LLM_GATEWAY_USAGE_EVENTS_TABLE,
};
use self::{
    codec::{
        batches_to_account_contribution_requests, batches_to_keys, batches_to_proxy_bindings,
        batches_to_proxy_configs, batches_to_runtime_config, batches_to_sponsor_requests,
        batches_to_token_requests, batches_to_usage_events,
        build_account_contribution_requests_batch, build_keys_batch, build_proxy_bindings_batch,
        build_proxy_configs_batch, build_runtime_config_batch, build_sponsor_requests_batch,
        build_token_requests_batch, build_usage_events_batch,
    },
    schema::{
        account_contribution_request_columns, ensure_account_contribution_requests_table,
        ensure_keys_table, ensure_proxy_bindings_table, ensure_proxy_configs_table,
        ensure_runtime_config_table, ensure_sponsor_requests_table, ensure_token_requests_table,
        ensure_usage_events_table, escape_literal, key_columns, proxy_binding_columns,
        proxy_config_columns, sponsor_request_columns, token_request_columns, usage_event_columns,
    },
};

/// Owns the LanceDB-backed storage layer for all LLM gateway admin data.
pub struct LlmGatewayStore {
    db: Connection,
}

impl LlmGatewayStore {
    /// Open the store connection, ensure all required tables exist, and create
    /// the default runtime-config row if it is missing.
    pub async fn connect(db_uri: &str) -> Result<Self> {
        tracing::info!("Opening LLM gateway store at `{db_uri}`");
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect llm gateway LanceDB")?;
        let store = Self {
            db,
        };
        store.keys_table().await?;
        store.usage_events_table().await?;
        store.runtime_config_table().await?;
        store.proxy_configs_table().await?;
        store.proxy_bindings_table().await?;
        store.token_requests_table().await?;
        store.account_contribution_requests_table().await?;
        store.sponsor_requests_table().await?;
        store.ensure_default_runtime_config().await?;
        tracing::info!("LLM gateway store ready");
        Ok(store)
    }

    /// Expose the underlying LanceDB connection for advanced callers.
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    async fn keys_table(&self) -> Result<Table> {
        ensure_keys_table(&self.db).await
    }

    async fn usage_events_table(&self) -> Result<Table> {
        ensure_usage_events_table(&self.db).await
    }

    async fn runtime_config_table(&self) -> Result<Table> {
        ensure_runtime_config_table(&self.db).await
    }

    async fn proxy_configs_table(&self) -> Result<Table> {
        ensure_proxy_configs_table(&self.db).await
    }

    async fn proxy_bindings_table(&self) -> Result<Table> {
        ensure_proxy_bindings_table(&self.db).await
    }

    async fn token_requests_table(&self) -> Result<Table> {
        ensure_token_requests_table(&self.db).await
    }

    async fn account_contribution_requests_table(&self) -> Result<Table> {
        ensure_account_contribution_requests_table(&self.db).await
    }

    async fn sponsor_requests_table(&self) -> Result<Table> {
        ensure_sponsor_requests_table(&self.db).await
    }

    async fn ensure_default_runtime_config(&self) -> Result<()> {
        if self.get_runtime_config().await?.is_none() {
            self.upsert_runtime_config(&LlmGatewayRuntimeConfigRecord::default())
                .await?;
        }
        Ok(())
    }

    /// Load the singleton runtime-config row if it exists.
    pub async fn get_runtime_config(&self) -> Result<Option<LlmGatewayRuntimeConfigRecord>> {
        let table = self.runtime_config_table().await?;
        let batches = table
            .query()
            .only_if("id = 'default'")
            .limit(1)
            .select(Select::columns(&[
                "id",
                "auth_cache_ttl_seconds",
                "max_request_body_bytes",
                "kiro_channel_max_concurrency",
                "kiro_channel_min_start_interval_ms",
                "updated_at",
            ]))
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
        self.delete_key(&record.id).await?;
        self.create_key(record).await
    }

    pub async fn delete_key(&self, key_id: &str) -> Result<()> {
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

    pub async fn append_usage_event(&self, record: &LlmGatewayUsageEventRecord) -> Result<()> {
        let table = self.usage_events_table().await?;
        let batch = build_usage_events_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to append llm gateway usage event")?;
        Ok(())
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
            limit = limit.unwrap_or_default(),
            offset = offset.unwrap_or_default(),
            "Querying LLM gateway usage events"
        );
        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_usage_events(&batch_list)
    }

    pub async fn apply_usage_event(
        &self,
        key: &LlmGatewayKeyRecord,
        usage_event: &LlmGatewayUsageEventRecord,
    ) -> Result<LlmGatewayKeyRecord> {
        self.append_usage_event(usage_event).await?;
        let mut updated = key.clone();
        updated.usage_input_uncached_tokens = updated
            .usage_input_uncached_tokens
            .saturating_add(usage_event.input_uncached_tokens);
        updated.usage_input_cached_tokens = updated
            .usage_input_cached_tokens
            .saturating_add(usage_event.input_cached_tokens);
        updated.usage_output_tokens = updated
            .usage_output_tokens
            .saturating_add(usage_event.output_tokens);
        updated.usage_billable_tokens = updated
            .usage_billable_tokens
            .saturating_add(usage_event.billable_tokens);
        if usage_event.provider_type == LLM_GATEWAY_PROVIDER_KIRO {
            updated.usage_credit_total += usage_event.credit_usage.unwrap_or(0.0);
            if usage_event.credit_usage_missing {
                updated.usage_credit_missing_events =
                    updated.usage_credit_missing_events.saturating_add(1);
            }
        }
        updated.last_used_at = Some(usage_event.created_at);
        updated.updated_at = usage_event.created_at;
        self.upsert_key(&updated).await?;
        Ok(updated)
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
    use std::{fs, path::PathBuf};

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
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
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
        updated.request_max_concurrency = Some(2);
        updated.request_min_start_interval_ms = Some(1_250);
        updated.updated_at = now_ms();
        store.upsert_key(&updated).await.expect("update key");

        let reloaded = store
            .get_key_by_id(&record.id)
            .await
            .expect("load updated key")
            .expect("updated key exists");
        assert_eq!(reloaded.status, LLM_GATEWAY_KEY_STATUS_DISABLED);
        assert_eq!(reloaded.request_max_concurrency, Some(2));
        assert_eq!(reloaded.request_min_start_interval_ms, Some(1_250));

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn replace_key_can_clear_nullable_request_limit_fields() {
        let dir = temp_store_dir("key-replace-clears-nullable");
        let store = LlmGatewayStore::connect(&dir.to_string_lossy())
            .await
            .expect("connect llm gateway store");

        let mut record = sample_key_record("test-key-clear", "Clearable Key");
        record.request_max_concurrency = Some(3);
        record.request_min_start_interval_ms = Some(1_500);
        store.create_key(&record).await.expect("create key");

        let mut updated = record.clone();
        updated.request_max_concurrency = None;
        updated.request_min_start_interval_ms = None;
        updated.updated_at = now_ms();
        store.replace_key(&updated).await.expect("replace key");

        let reloaded = store
            .get_key_by_id(&record.id)
            .await
            .expect("load replaced key")
            .expect("replaced key exists");
        assert_eq!(reloaded.request_max_concurrency, None);
        assert_eq!(reloaded.request_min_start_interval_ms, None);

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
            created_at: now,
        };
        let updated = store
            .apply_usage_event(&key, &event)
            .await
            .expect("apply usage event");
        assert_eq!(updated.usage_credit_total, 0.125);
        assert_eq!(updated.usage_credit_missing_events, 0);

        let missing = LlmGatewayUsageEventRecord {
            id: "evt-2".to_string(),
            created_at: now + 1,
            credit_usage: None,
            credit_usage_missing: true,
            ..event
        };
        let updated = store
            .apply_usage_event(&updated, &missing)
            .await
            .expect("apply missing-credit usage event");
        assert_eq!(updated.usage_credit_total, 0.125);
        assert_eq!(updated.usage_credit_missing_events, 1);

        let _ = fs::remove_dir_all(&dir);
    }
}
