use std::sync::Arc;

use anyhow::{Context, Result};
use arrow_array::{
    builder::{Int32Builder, StringBuilder, TimestampMillisecondBuilder},
    Array, ArrayRef, Int32Array, RecordBatch, RecordBatchIterator, StringArray,
    TimestampMillisecondArray,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::Utc;
use futures::TryStreamExt;
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase, Select},
    Connection, Table,
};
use serde::{Deserialize, Serialize};

pub const REQUEST_STATUS_PENDING: &str = "pending";
pub const REQUEST_STATUS_APPROVED: &str = "approved";
pub const REQUEST_STATUS_RUNNING: &str = "running";
pub const REQUEST_STATUS_DONE: &str = "done";
pub const REQUEST_STATUS_FAILED: &str = "failed";
pub const REQUEST_STATUS_REJECTED: &str = "rejected";
pub const REQUEST_AI_RUN_STATUS_RUNNING: &str = "running";
pub const REQUEST_AI_RUN_STATUS_SUCCESS: &str = "success";
pub const REQUEST_AI_RUN_STATUS_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArticleRequestInput {
    pub request_id: String,
    pub article_url: String,
    pub title_hint: Option<String>,
    pub request_message: String,
    pub nickname: String,
    pub requester_email: Option<String>,
    pub frontend_page_url: Option<String>,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
    pub parent_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArticleRequestRecord {
    pub request_id: String,
    pub article_url: String,
    pub title_hint: Option<String>,
    pub request_message: String,
    pub nickname: String,
    #[serde(skip_serializing)]
    pub requester_email: Option<String>,
    #[serde(skip_serializing)]
    pub frontend_page_url: Option<String>,
    pub status: String,
    pub fingerprint: String,
    pub client_ip: String,
    pub ip_region: String,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub ingested_article_id: Option<String>,
    pub attempt_count: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub ai_reply: Option<String>,
    pub parent_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArticleRequestAiRunInput {
    pub run_id: String,
    pub request_id: String,
    pub runner_program: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArticleRequestAiRunRecord {
    pub run_id: String,
    pub request_id: String,
    pub status: String,
    pub runner_program: String,
    pub exit_code: Option<i32>,
    pub final_reply_markdown: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewArticleRequestAiRunChunkInput {
    pub chunk_id: String,
    pub run_id: String,
    pub request_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArticleRequestAiRunChunkRecord {
    pub chunk_id: String,
    pub run_id: String,
    pub request_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
    pub created_at: i64,
}

pub const ARTICLE_REQUEST_TABLE_NAMES: &[&str] = &[
    "article_requests",
    "article_request_ai_runs",
    "article_request_ai_run_chunks",
];

pub struct ArticleRequestStore {
    db: Connection,
    requests_table: String,
    ai_runs_table: String,
    ai_chunks_table: String,
}

impl ArticleRequestStore {
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub async fn connect(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect article-request LanceDB")?;
        Ok(Self {
            db,
            requests_table: "article_requests".to_string(),
            ai_runs_table: "article_request_ai_runs".to_string(),
            ai_chunks_table: "article_request_ai_run_chunks".to_string(),
        })
    }

    async fn requests_table(&self) -> Result<Table> {
        let table = ensure_table(&self.db, &self.requests_table, request_schema()).await?;
        let schema = table.schema().await.ok();
        if schema
            .as_ref()
            .map(|s| s.field_with_name("parent_request_id").is_err())
            .unwrap_or(false)
        {
            let new_field =
                Arc::new(Schema::new(vec![Field::new("parent_request_id", DataType::Utf8, true)]));
            table
                .add_columns(lancedb::table::NewColumnTransform::AllNulls(new_field), None)
                .await
                .ok();
        }
        Ok(table)
    }
    async fn ai_runs_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.ai_runs_table, request_ai_runs_schema()).await
    }
    async fn ai_chunks_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.ai_chunks_table, request_ai_chunks_schema()).await
    }

    pub async fn create_request(&self, input: NewArticleRequestInput) -> Result<ArticleRequestRecord> {
        let now = now_ms();
        let record = ArticleRequestRecord {
            request_id: input.request_id,
            article_url: input.article_url,
            title_hint: normalize_opt(input.title_hint),
            request_message: input.request_message,
            nickname: input.nickname,
            requester_email: normalize_opt(input.requester_email),
            frontend_page_url: normalize_opt(input.frontend_page_url),
            status: REQUEST_STATUS_PENDING.to_string(),
            fingerprint: input.fingerprint,
            client_ip: input.client_ip,
            ip_region: input.ip_region,
            admin_note: None,
            failure_reason: None,
            ingested_article_id: None,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            ai_reply: None,
            parent_request_id: normalize_opt(input.parent_request_id),
        };
        let table = self.requests_table().await?;
        upsert_request_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn get_request(&self, request_id: &str) -> Result<Option<ArticleRequestRecord>> {
        let table = self.requests_table().await?;
        let filter = format!("request_id = '{}'", escape_literal(request_id));
        let rows = query_requests(&table, Some(&filter), Some(1), None).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_requests(
        &self,
        status: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<ArticleRequestRecord>> {
        self.list_requests_page(status, limit.unwrap_or(100), 0).await
    }

    pub async fn list_requests_page(
        &self,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ArticleRequestRecord>> {
        let table = self.requests_table().await?;
        let filter = status.map(|s| format!("status = '{}'", escape_literal(s)));
        query_requests(&table, filter.as_deref(), Some(limit), Some(offset)).await
    }

    pub async fn list_requests_public(&self, limit: Option<usize>) -> Result<Vec<ArticleRequestRecord>> {
        self.list_requests_public_page(limit.unwrap_or(50), 0).await
    }

    pub async fn list_requests_public_page(
        &self,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ArticleRequestRecord>> {
        let table = self.requests_table().await?;
        let filter = format!("status != '{}'", escape_literal(REQUEST_STATUS_REJECTED));
        query_requests(&table, Some(&filter), Some(limit), Some(offset)).await
    }

    pub async fn count_requests(&self, status: Option<&str>) -> Result<usize> {
        let table = self.requests_table().await?;
        let filter = status.map(|s| format!("status = '{}'", escape_literal(s)));
        let total = table
            .count_rows(filter)
            .await
            .context("failed to count article requests")?;
        Ok(total as usize)
    }

    pub async fn count_requests_public(&self) -> Result<usize> {
        let table = self.requests_table().await?;
        let filter = format!("status != '{}'", escape_literal(REQUEST_STATUS_REJECTED));
        let total = table
            .count_rows(Some(filter))
            .await
            .context("failed to count public article requests")?;
        Ok(total as usize)
    }

    pub async fn transition_request(
        &self,
        request_id: &str,
        next_status: &str,
        admin_note: Option<&str>,
        failure_reason: Option<&str>,
        ingested_article_id: Option<&str>,
        ai_reply: Option<&str>,
    ) -> Result<ArticleRequestRecord> {
        let mut record = self
            .get_request(request_id)
            .await?
            .with_context(|| format!("article request not found: {request_id}"))?;
        validate_request_transition(&record.status, next_status)?;
        record.status = next_status.to_string();
        record.updated_at = now_ms();
        if let Some(note) = admin_note {
            record.admin_note = Some(note.to_string());
        }
        if let Some(reason) = failure_reason {
            record.failure_reason = Some(reason.to_string());
        }
        if let Some(sid) = ingested_article_id {
            record.ingested_article_id = Some(sid.to_string());
        }
        if let Some(reply) = ai_reply {
            record.ai_reply = Some(reply.to_string());
        }
        if next_status == REQUEST_STATUS_RUNNING {
            record.attempt_count += 1;
        }
        let table = self.requests_table().await?;
        upsert_request_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn delete_request(&self, request_id: &str) -> Result<()> {
        let esc = escape_literal(request_id);
        let table = self.requests_table().await?;
        table.delete(&format!("request_id = '{esc}'")).await?;
        let runs = self.ai_runs_table().await?;
        runs.delete(&format!("request_id = '{esc}'")).await?;
        let chunks = self.ai_chunks_table().await?;
        chunks.delete(&format!("request_id = '{esc}'")).await?;
        Ok(())
    }

    /// Walk the parent_request_id chain upward, collecting ancestor records.
    /// Returns `[self_or_parent, grandparent, ...]` up to `max_depth` entries.
    pub async fn build_parent_context_chain(
        &self,
        request_id: &str,
        max_depth: usize,
    ) -> Result<Vec<ArticleRequestRecord>> {
        let mut chain = Vec::new();
        let mut current_id = request_id.to_string();
        for _ in 0..max_depth {
            let record = match self.get_request(&current_id).await? {
                Some(r) => r,
                None => break,
            };
            let next_parent = record.parent_request_id.clone();
            chain.push(record);
            match next_parent {
                Some(pid) if !pid.is_empty() => current_id = pid,
                _ => break,
            }
        }
        Ok(chain)
    }

    pub async fn create_ai_run(
        &self,
        input: NewArticleRequestAiRunInput,
    ) -> Result<ArticleRequestAiRunRecord> {
        let now = now_ms();
        let record = ArticleRequestAiRunRecord {
            run_id: input.run_id,
            request_id: input.request_id,
            status: REQUEST_AI_RUN_STATUS_RUNNING.to_string(),
            runner_program: input.runner_program,
            exit_code: None,
            final_reply_markdown: None,
            failure_reason: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };
        let table = self.ai_runs_table().await?;
        upsert_ai_run_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn get_ai_run(&self, run_id: &str) -> Result<Option<ArticleRequestAiRunRecord>> {
        let table = self.ai_runs_table().await?;
        let filter = format!("run_id = '{}'", escape_literal(run_id));
        let rows = query_ai_runs(&table, Some(&filter), Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_ai_runs(
        &self,
        request_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<ArticleRequestAiRunRecord>> {
        let table = self.ai_runs_table().await?;
        let filter = format!("request_id = '{}'", escape_literal(request_id));
        query_ai_runs(&table, Some(&filter), limit).await
    }

    pub async fn finalize_ai_run(
        &self,
        run_id: &str,
        status: &str,
        exit_code: Option<i32>,
        failure_reason: Option<&str>,
        final_reply_markdown: Option<&str>,
    ) -> Result<()> {
        let mut record = self
            .get_ai_run(run_id)
            .await?
            .with_context(|| format!("ai run not found: {run_id}"))?;
        let now = now_ms();
        record.status = status.to_string();
        record.exit_code = exit_code;
        record.failure_reason = failure_reason.map(|s| s.to_string());
        record.final_reply_markdown = final_reply_markdown.map(|s| s.to_string());
        record.updated_at = now;
        record.completed_at = Some(now);
        let table = self.ai_runs_table().await?;
        upsert_ai_run_record(&table, &record).await
    }

    pub async fn append_ai_run_chunk(&self, input: NewArticleRequestAiRunChunkInput) -> Result<()> {
        let now = now_ms();
        let record = ArticleRequestAiRunChunkRecord {
            chunk_id: input.chunk_id,
            run_id: input.run_id,
            request_id: input.request_id,
            stream: input.stream,
            batch_index: input.batch_index,
            content: input.content,
            created_at: now,
        };
        let table = self.ai_chunks_table().await?;
        upsert_ai_chunk_record(&table, &record).await
    }

    pub async fn list_ai_run_chunks(
        &self,
        run_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<ArticleRequestAiRunChunkRecord>> {
        let table = self.ai_chunks_table().await?;
        let filter = format!("run_id = '{}'", escape_literal(run_id));
        query_ai_chunks(&table, Some(&filter), limit).await
    }
}

fn validate_request_transition(current: &str, next: &str) -> Result<()> {
    if current == next {
        anyhow::bail!("invalid request transition: {current} -> {next}");
    }
    let ok = matches!(
        (current, next),
        (REQUEST_STATUS_PENDING, REQUEST_STATUS_APPROVED | REQUEST_STATUS_RUNNING | REQUEST_STATUS_REJECTED)
            | (REQUEST_STATUS_APPROVED, REQUEST_STATUS_RUNNING | REQUEST_STATUS_REJECTED)
            | (REQUEST_STATUS_RUNNING, REQUEST_STATUS_DONE | REQUEST_STATUS_FAILED)
            | (
                REQUEST_STATUS_FAILED,
                REQUEST_STATUS_APPROVED
                    | REQUEST_STATUS_RUNNING
                    | REQUEST_STATUS_REJECTED
                    | REQUEST_STATUS_DONE
            )
    );
    if ok {
        Ok(())
    } else {
        anyhow::bail!("invalid request transition: {current} -> {next}")
    }
}

async fn ensure_table(db: &Connection, name: &str, schema: Arc<Schema>) -> Result<Table> {
    match db.open_table(name).execute().await {
        Ok(t) => Ok(t),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            db.create_table(name, Box::new(batches))
                .execute()
                .await
                .with_context(|| format!("failed to create table {name}"))?;
            db.open_table(name)
                .execute()
                .await
                .with_context(|| format!("failed to open table {name}"))
        },
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn normalize_opt(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn escape_literal(s: &str) -> String {
    s.replace('\'', "''")
}

fn request_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("request_id", DataType::Utf8, false),
        Field::new("article_url", DataType::Utf8, false),
        Field::new("title_hint", DataType::Utf8, true),
        Field::new("request_message", DataType::Utf8, false),
        Field::new("nickname", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("admin_note", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("ingested_article_id", DataType::Utf8, true),
        Field::new("attempt_count", DataType::Int32, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("ai_reply", DataType::Utf8, true),
        Field::new("requester_email", DataType::Utf8, true),
        Field::new("frontend_page_url", DataType::Utf8, true),
        Field::new("parent_request_id", DataType::Utf8, true),
    ]))
}

fn request_ai_runs_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("run_id", DataType::Utf8, false),
        Field::new("request_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("runner_program", DataType::Utf8, false),
        Field::new("exit_code", DataType::Int32, true),
        Field::new("final_reply_markdown", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("started_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("completed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

fn request_ai_chunks_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("request_id", DataType::Utf8, false),
        Field::new("stream", DataType::Utf8, false),
        Field::new("batch_index", DataType::Int32, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn build_request_batch(r: &ArticleRequestRecord) -> Result<RecordBatch> {
    let mut request_id = StringBuilder::new();
    let mut article_url = StringBuilder::new();
    let mut title_hint = StringBuilder::new();
    let mut request_message = StringBuilder::new();
    let mut nickname = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut fingerprint = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut admin_note = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut ingested_article_id = StringBuilder::new();
    let mut attempt_count = Int32Builder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut ai_reply = StringBuilder::new();
    let mut requester_email = StringBuilder::new();
    let mut frontend_page_url = StringBuilder::new();
    let mut parent_request_id = StringBuilder::new();

    request_id.append_value(&r.request_id);
    article_url.append_value(&r.article_url);
    title_hint.append_option(r.title_hint.as_deref());
    request_message.append_value(&r.request_message);
    nickname.append_value(&r.nickname);
    status.append_value(&r.status);
    fingerprint.append_value(&r.fingerprint);
    client_ip.append_value(&r.client_ip);
    ip_region.append_value(&r.ip_region);
    admin_note.append_option(r.admin_note.as_deref());
    failure_reason.append_option(r.failure_reason.as_deref());
    ingested_article_id.append_option(r.ingested_article_id.as_deref());
    attempt_count.append_value(r.attempt_count);
    created_at.append_value(r.created_at);
    updated_at.append_value(r.updated_at);
    ai_reply.append_option(r.ai_reply.as_deref());
    requester_email.append_option(r.requester_email.as_deref());
    frontend_page_url.append_option(r.frontend_page_url.as_deref());
    parent_request_id.append_option(r.parent_request_id.as_deref());

    let columns: Vec<ArrayRef> = vec![
        Arc::new(request_id.finish()),
        Arc::new(article_url.finish()),
        Arc::new(title_hint.finish()),
        Arc::new(request_message.finish()),
        Arc::new(nickname.finish()),
        Arc::new(status.finish()),
        Arc::new(fingerprint.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(admin_note.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(ingested_article_id.finish()),
        Arc::new(attempt_count.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(ai_reply.finish()),
        Arc::new(requester_email.finish()),
        Arc::new(frontend_page_url.finish()),
        Arc::new(parent_request_id.finish()),
    ];
    Ok(RecordBatch::try_new(request_schema(), columns)?)
}

async fn upsert_request_record(table: &Table, record: &ArticleRequestRecord) -> Result<()> {
    let batch = build_request_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["request_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

fn build_ai_run_batch(r: &ArticleRequestAiRunRecord) -> Result<RecordBatch> {
    let mut run_id = StringBuilder::new();
    let mut request_id = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut runner_program = StringBuilder::new();
    let mut exit_code = Int32Builder::new();
    let mut final_reply_markdown = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut started_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut completed_at = TimestampMillisecondBuilder::new();

    run_id.append_value(&r.run_id);
    request_id.append_value(&r.request_id);
    status.append_value(&r.status);
    runner_program.append_value(&r.runner_program);
    exit_code.append_option(r.exit_code);
    final_reply_markdown.append_option(r.final_reply_markdown.as_deref());
    failure_reason.append_option(r.failure_reason.as_deref());
    started_at.append_value(r.started_at);
    updated_at.append_value(r.updated_at);
    completed_at.append_option(r.completed_at);

    let columns: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(request_id.finish()),
        Arc::new(status.finish()),
        Arc::new(runner_program.finish()),
        Arc::new(exit_code.finish()),
        Arc::new(final_reply_markdown.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(started_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(completed_at.finish()),
    ];
    Ok(RecordBatch::try_new(request_ai_runs_schema(), columns)?)
}

async fn upsert_ai_run_record(table: &Table, record: &ArticleRequestAiRunRecord) -> Result<()> {
    let batch = build_ai_run_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["run_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

fn build_ai_chunk_batch(r: &ArticleRequestAiRunChunkRecord) -> Result<RecordBatch> {
    let mut chunk_id = StringBuilder::new();
    let mut run_id = StringBuilder::new();
    let mut request_id = StringBuilder::new();
    let mut stream = StringBuilder::new();
    let mut batch_index = Int32Builder::new();
    let mut content = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    chunk_id.append_value(&r.chunk_id);
    run_id.append_value(&r.run_id);
    request_id.append_value(&r.request_id);
    stream.append_value(&r.stream);
    batch_index.append_value(r.batch_index);
    content.append_value(&r.content);
    created_at.append_value(r.created_at);

    let columns: Vec<ArrayRef> = vec![
        Arc::new(chunk_id.finish()),
        Arc::new(run_id.finish()),
        Arc::new(request_id.finish()),
        Arc::new(stream.finish()),
        Arc::new(batch_index.finish()),
        Arc::new(content.finish()),
        Arc::new(created_at.finish()),
    ];
    Ok(RecordBatch::try_new(request_ai_chunks_schema(), columns)?)
}

async fn upsert_ai_chunk_record(table: &Table, record: &ArticleRequestAiRunChunkRecord) -> Result<()> {
    let batch = build_ai_chunk_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["chunk_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

async fn query_requests(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<Vec<ArticleRequestRecord>> {
    let mut query = table.query();
    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(o) = offset {
        query = query.offset(o);
    }
    if let Some(l) = limit {
        query = query.limit(l.max(1));
    }
    let cols = &[
        "request_id",
        "article_url",
        "title_hint",
        "request_message",
        "nickname",
        "requester_email",
        "frontend_page_url",
        "status",
        "fingerprint",
        "client_ip",
        "ip_region",
        "admin_note",
        "failure_reason",
        "ingested_article_id",
        "attempt_count",
        "created_at",
        "updated_at",
        "ai_reply",
        "parent_request_id",
    ];
    let batches = query
        .select(Select::columns(cols))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let c_request_id = string_col(&batch, "request_id")?;
        let c_article_url = string_col(&batch, "article_url")?;
        let c_title_hint = string_col(&batch, "title_hint")?;
        let c_request_message = string_col(&batch, "request_message")?;
        let c_nickname = string_col(&batch, "nickname")?;
        let c_requester_email = string_col(&batch, "requester_email")?;
        let c_frontend_page_url = string_col(&batch, "frontend_page_url")?;
        let c_status = string_col(&batch, "status")?;
        let c_fingerprint = string_col(&batch, "fingerprint")?;
        let c_client_ip = string_col(&batch, "client_ip")?;
        let c_ip_region = string_col(&batch, "ip_region")?;
        let c_admin_note = string_col(&batch, "admin_note")?;
        let c_failure_reason = string_col(&batch, "failure_reason")?;
        let c_ingested_article_id = string_col(&batch, "ingested_article_id")?;
        let c_attempt_count = int32_col(&batch, "attempt_count")?;
        let c_created_at = ts_col(&batch, "created_at")?;
        let c_updated_at = ts_col(&batch, "updated_at")?;
        let c_ai_reply = string_col(&batch, "ai_reply")?;
        let c_parent_request_id = string_col(&batch, "parent_request_id")?;

        for i in 0..batch.num_rows() {
            rows.push(ArticleRequestRecord {
                request_id: c_request_id.value(i).to_string(),
                article_url: c_article_url.value(i).to_string(),
                title_hint: nullable_str(c_title_hint, i),
                request_message: c_request_message.value(i).to_string(),
                nickname: c_nickname.value(i).to_string(),
                requester_email: nullable_str(c_requester_email, i),
                frontend_page_url: nullable_str(c_frontend_page_url, i),
                status: c_status.value(i).to_string(),
                fingerprint: c_fingerprint.value(i).to_string(),
                client_ip: c_client_ip.value(i).to_string(),
                ip_region: c_ip_region.value(i).to_string(),
                admin_note: nullable_str(c_admin_note, i),
                failure_reason: nullable_str(c_failure_reason, i),
                ingested_article_id: nullable_str(c_ingested_article_id, i),
                attempt_count: c_attempt_count.value(i),
                created_at: c_created_at.value(i),
                updated_at: c_updated_at.value(i),
                ai_reply: nullable_str(c_ai_reply, i),
                parent_request_id: nullable_str(c_parent_request_id, i),
            });
        }
    }
    Ok(rows)
}

async fn query_ai_runs(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<ArticleRequestAiRunRecord>> {
    let mut query = table.query();
    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(l) = limit {
        query = query.limit(l.max(1));
    }
    let cols = &[
        "run_id",
        "request_id",
        "status",
        "runner_program",
        "exit_code",
        "final_reply_markdown",
        "failure_reason",
        "started_at",
        "updated_at",
        "completed_at",
    ];
    let batches = query
        .select(Select::columns(cols))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let c_run_id = string_col(&batch, "run_id")?;
        let c_request_id = string_col(&batch, "request_id")?;
        let c_status = string_col(&batch, "status")?;
        let c_runner_program = string_col(&batch, "runner_program")?;
        let c_exit_code = int32_col(&batch, "exit_code")?;
        let c_final_reply = string_col(&batch, "final_reply_markdown")?;
        let c_failure_reason = string_col(&batch, "failure_reason")?;
        let c_started_at = ts_col(&batch, "started_at")?;
        let c_updated_at = ts_col(&batch, "updated_at")?;
        let c_completed_at = ts_col(&batch, "completed_at")?;

        for i in 0..batch.num_rows() {
            rows.push(ArticleRequestAiRunRecord {
                run_id: c_run_id.value(i).to_string(),
                request_id: c_request_id.value(i).to_string(),
                status: c_status.value(i).to_string(),
                runner_program: c_runner_program.value(i).to_string(),
                exit_code: nullable_i32(c_exit_code, i),
                final_reply_markdown: nullable_str(c_final_reply, i),
                failure_reason: nullable_str(c_failure_reason, i),
                started_at: c_started_at.value(i),
                updated_at: c_updated_at.value(i),
                completed_at: nullable_ts(c_completed_at, i),
            });
        }
    }
    Ok(rows)
}

async fn query_ai_chunks(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<ArticleRequestAiRunChunkRecord>> {
    let mut query = table.query();
    if let Some(f) = filter {
        query = query.only_if(f);
    }
    if let Some(l) = limit {
        query = query.limit(l.max(1));
    }
    let cols = &["chunk_id", "run_id", "request_id", "stream", "batch_index", "content", "created_at"];
    let batches = query
        .select(Select::columns(cols))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let c_chunk_id = string_col(&batch, "chunk_id")?;
        let c_run_id = string_col(&batch, "run_id")?;
        let c_request_id = string_col(&batch, "request_id")?;
        let c_stream = string_col(&batch, "stream")?;
        let c_batch_index = int32_col(&batch, "batch_index")?;
        let c_content = string_col(&batch, "content")?;
        let c_created_at = ts_col(&batch, "created_at")?;

        for i in 0..batch.num_rows() {
            rows.push(ArticleRequestAiRunChunkRecord {
                chunk_id: c_chunk_id.value(i).to_string(),
                run_id: c_run_id.value(i).to_string(),
                request_id: c_request_id.value(i).to_string(),
                stream: c_stream.value(i).to_string(),
                batch_index: c_batch_index.value(i),
                content: c_content.value(i).to_string(),
                created_at: c_created_at.value(i),
            });
        }
    }
    Ok(rows)
}

fn string_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column: {name}"))?
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("column {name} is not Utf8"))
}

fn int32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column: {name}"))?
        .as_any()
        .downcast_ref::<Int32Array>()
        .with_context(|| format!("column {name} is not Int32"))
}

fn ts_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a TimestampMillisecondArray> {
    batch
        .column_by_name(name)
        .with_context(|| format!("missing column: {name}"))?
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .with_context(|| format!("column {name} is not Timestamp"))
}

fn nullable_str(arr: &StringArray, i: usize) -> Option<String> {
    if arr.is_null(i) {
        None
    } else {
        Some(arr.value(i).to_string())
    }
}

fn nullable_i32(arr: &Int32Array, i: usize) -> Option<i32> {
    if arr.is_null(i) {
        None
    } else {
        Some(arr.value(i))
    }
}

fn nullable_ts(arr: &TimestampMillisecondArray, i: usize) -> Option<i64> {
    if arr.is_null(i) {
        None
    } else {
        Some(arr.value(i))
    }
}
