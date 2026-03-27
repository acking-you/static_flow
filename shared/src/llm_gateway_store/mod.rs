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
    LlmGatewayRuntimeConfigRecord, LlmGatewaySponsorRequestRecord, LlmGatewayTokenRequestRecord,
    LlmGatewayUsageEventRecord, NewLlmGatewayAccountContributionRequestInput,
    NewLlmGatewaySponsorRequestInput, NewLlmGatewayTokenRequestInput,
    DEFAULT_LLM_GATEWAY_AUTH_CACHE_TTL_SECONDS, LLM_GATEWAY_ACCOUNT_CONTRIBUTION_REQUESTS_TABLE,
    LLM_GATEWAY_KEYS_TABLE, LLM_GATEWAY_KEY_STATUS_ACTIVE, LLM_GATEWAY_KEY_STATUS_DISABLED,
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
        batches_to_account_contribution_requests, batches_to_keys, batches_to_runtime_config,
        batches_to_sponsor_requests, batches_to_token_requests, batches_to_usage_events,
        build_account_contribution_requests_batch, build_keys_batch, build_runtime_config_batch,
        build_sponsor_requests_batch, build_token_requests_batch, build_usage_events_batch,
    },
    schema::{
        account_contribution_request_columns, ensure_account_contribution_requests_table,
        ensure_keys_table, ensure_runtime_config_table, ensure_sponsor_requests_table,
        ensure_token_requests_table, ensure_usage_events_table, escape_literal, key_columns,
        sponsor_request_columns, token_request_columns, usage_event_columns,
    },
};

/// Owns the LanceDB-backed storage layer for all LLM gateway admin data.
pub struct LlmGatewayStore {
    db: Connection,
}

impl LlmGatewayStore {
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
        store.token_requests_table().await?;
        store.account_contribution_requests_table().await?;
        store.sponsor_requests_table().await?;
        store.ensure_default_runtime_config().await?;
        tracing::info!("LLM gateway store ready");
        Ok(store)
    }

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

    pub async fn get_runtime_config(&self) -> Result<Option<LlmGatewayRuntimeConfigRecord>> {
        let table = self.runtime_config_table().await?;
        let batches = table
            .query()
            .only_if("id = 'default'")
            .limit(1)
            .select(Select::columns(&["id", "auth_cache_ttl_seconds", "updated_at"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_runtime_config(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn get_runtime_config_or_default(&self) -> Result<LlmGatewayRuntimeConfigRecord> {
        Ok(self.get_runtime_config().await?.unwrap_or_default())
    }

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
        self.query_usage_events(key_id, limit, Some(0)).await
    }

    pub async fn count_usage_events(&self, key_id: Option<&str>) -> Result<usize> {
        let table = self.usage_events_table().await?;
        let filter = key_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("key_id = '{}'", escape_literal(value)));
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
    /// The caller is responsible for translating user-facing "newest first"
    /// pagination into the corresponding tail offset, mirroring the existing
    /// api_behavior pagination strategy.
    pub async fn query_usage_events(
        &self,
        key_id: Option<&str>,
        limit: Option<usize>,
        offset: Option<usize>,
    ) -> Result<Vec<LlmGatewayUsageEventRecord>> {
        let table = self.usage_events_table().await?;
        let mut query = table
            .query()
            .select(Select::columns(&usage_event_columns()));
        if let Some(key_id) = key_id.map(str::trim).filter(|value| !value.is_empty()) {
            query = query.only_if(format!("key_id = '{}'", escape_literal(key_id)));
        }
        if let Some(offset) = offset {
            query = query.offset(offset);
        }
        if let Some(limit) = limit {
            query = query.limit(limit.max(1));
        }
        tracing::debug!(
            key_id = key_id.unwrap_or("all"),
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
