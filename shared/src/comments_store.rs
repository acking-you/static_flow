use std::{collections::HashMap, sync::Arc};

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

pub const COMMENT_STATUS_PENDING: &str = "pending";
pub const COMMENT_STATUS_APPROVED: &str = "approved";
pub const COMMENT_STATUS_RUNNING: &str = "running";
pub const COMMENT_STATUS_DONE: &str = "done";
pub const COMMENT_STATUS_FAILED: &str = "failed";
pub const COMMENT_STATUS_REJECTED: &str = "rejected";
pub const COMMENT_AI_RUN_STATUS_RUNNING: &str = "running";
pub const COMMENT_AI_RUN_STATUS_SUCCESS: &str = "success";
pub const COMMENT_AI_RUN_STATUS_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCommentTaskInput {
    pub task_id: String,
    pub article_id: String,
    pub entry_type: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub client_ip: String,
    pub ip_region: String,
    pub fingerprint: String,
    pub ua: Option<String>,
    pub language: Option<String>,
    pub platform: Option<String>,
    pub timezone: Option<String>,
    pub viewport: Option<String>,
    pub referrer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentTaskPatch {
    pub comment_text: Option<String>,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub admin_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentTaskRecord {
    pub task_id: String,
    pub article_id: String,
    pub entry_type: String,
    pub status: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub client_ip: String,
    pub ip_region: String,
    pub fingerprint: String,
    pub ua: Option<String>,
    pub language: Option<String>,
    pub platform: Option<String>,
    pub timezone: Option<String>,
    pub viewport: Option<String>,
    pub referrer: Option<String>,
    pub admin_note: Option<String>,
    pub failure_reason: Option<String>,
    pub attempt_count: i32,
    pub created_at: i64,
    pub updated_at: i64,
    pub approved_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewPublishedCommentInput {
    pub comment_id: String,
    pub task_id: String,
    pub article_id: String,
    pub author_name: String,
    pub author_avatar_seed: String,
    pub author_hash: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub ai_reply_markdown: String,
    pub ip_region: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishedCommentPatch {
    pub ai_reply_markdown: Option<String>,
    pub comment_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PublishedCommentRecord {
    pub comment_id: String,
    pub task_id: String,
    pub article_id: String,
    pub author_name: String,
    pub author_avatar_seed: String,
    pub author_hash: String,
    pub comment_text: String,
    pub selected_text: Option<String>,
    pub anchor_block_id: Option<String>,
    pub anchor_context_before: Option<String>,
    pub anchor_context_after: Option<String>,
    pub reply_to_comment_id: Option<String>,
    pub reply_to_comment_text: Option<String>,
    pub reply_to_ai_reply_markdown: Option<String>,
    pub ai_reply_markdown: String,
    pub ip_region: String,
    pub published_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCommentAuditInput {
    pub log_id: String,
    pub task_id: String,
    pub action: String,
    pub operator: String,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentAuditRecord {
    pub log_id: String,
    pub task_id: String,
    pub action: String,
    pub operator: String,
    pub before_json: Option<String>,
    pub after_json: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCommentAiRunInput {
    pub run_id: String,
    pub task_id: String,
    pub runner_program: String,
    pub runner_args_json: String,
    pub skill_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentAiRunRecord {
    pub run_id: String,
    pub task_id: String,
    pub status: String,
    pub runner_program: String,
    pub runner_args_json: String,
    pub skill_path: String,
    pub exit_code: Option<i32>,
    pub final_reply_markdown: Option<String>,
    pub failure_reason: Option<String>,
    pub started_at: i64,
    pub updated_at: i64,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCommentAiRunChunkInput {
    pub chunk_id: String,
    pub run_id: String,
    pub task_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommentAiRunChunkRecord {
    pub chunk_id: String,
    pub run_id: String,
    pub task_id: String,
    pub stream: String,
    pub batch_index: i32,
    pub content: String,
    pub created_at: i64,
}

pub const COMMENT_TABLE_NAMES: &[&str] = &[
    "comment_tasks",
    "comment_published",
    "comment_audit_logs",
    "comment_ai_runs",
    "comment_ai_run_chunks",
];

pub struct CommentDataStore {
    db: Connection,
    tasks_table: String,
    published_table: String,
    audit_table: String,
    ai_runs_table: String,
    ai_chunks_table: String,
}

impl CommentDataStore {
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub async fn connect(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect comments LanceDB")?;

        Ok(Self {
            db,
            tasks_table: "comment_tasks".to_string(),
            published_table: "comment_published".to_string(),
            audit_table: "comment_audit_logs".to_string(),
            ai_runs_table: "comment_ai_runs".to_string(),
            ai_chunks_table: "comment_ai_run_chunks".to_string(),
        })
    }

    async fn tasks_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.tasks_table, comment_task_schema()).await
    }

    async fn published_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.published_table, comment_published_schema()).await
    }

    async fn audit_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.audit_table, comment_audit_schema()).await
    }

    async fn ai_runs_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.ai_runs_table, comment_ai_runs_schema()).await
    }

    async fn ai_chunks_table(&self) -> Result<Table> {
        ensure_table(&self.db, &self.ai_chunks_table, comment_ai_chunks_schema()).await
    }

    pub async fn create_comment_task(
        &self,
        input: NewCommentTaskInput,
    ) -> Result<CommentTaskRecord> {
        let now = now_ms();
        let record = CommentTaskRecord {
            task_id: input.task_id,
            article_id: input.article_id,
            entry_type: input.entry_type,
            status: COMMENT_STATUS_PENDING.to_string(),
            comment_text: input.comment_text,
            selected_text: normalize_optional_text(input.selected_text),
            anchor_block_id: normalize_optional_text(input.anchor_block_id),
            anchor_context_before: normalize_optional_text(input.anchor_context_before),
            anchor_context_after: normalize_optional_text(input.anchor_context_after),
            reply_to_comment_id: normalize_optional_text(input.reply_to_comment_id),
            reply_to_comment_text: normalize_optional_text(input.reply_to_comment_text),
            reply_to_ai_reply_markdown: normalize_optional_text(input.reply_to_ai_reply_markdown),
            client_ip: input.client_ip,
            ip_region: input.ip_region,
            fingerprint: input.fingerprint,
            ua: normalize_optional_text(input.ua),
            language: normalize_optional_text(input.language),
            platform: normalize_optional_text(input.platform),
            timezone: normalize_optional_text(input.timezone),
            viewport: normalize_optional_text(input.viewport),
            referrer: normalize_optional_text(input.referrer),
            admin_note: None,
            failure_reason: None,
            attempt_count: 0,
            created_at: now,
            updated_at: now,
            approved_at: None,
            completed_at: None,
        };

        let table = self.tasks_table().await?;
        upsert_comment_task_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn get_comment_task(&self, task_id: &str) -> Result<Option<CommentTaskRecord>> {
        let table = self.tasks_table().await?;
        let filter = format!("task_id = '{}'", escape_literal(task_id));
        let rows = query_comment_tasks(&table, Some(&filter), Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_comment_tasks(
        &self,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommentTaskRecord>> {
        let table = self.tasks_table().await?;
        let filter = status
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("status = '{}'", escape_literal(value)));
        let mut rows = query_comment_tasks(&table, filter.as_deref(), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn list_comment_tasks_by_article(
        &self,
        article_id: &str,
        limit: usize,
    ) -> Result<Vec<CommentTaskRecord>> {
        let article_id = article_id.trim();
        if article_id.is_empty() {
            return Ok(vec![]);
        }

        let table = self.tasks_table().await?;
        let filter = format!("article_id = '{}'", escape_literal(article_id));
        let mut rows = query_comment_tasks(&table, Some(&filter), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn count_comment_tasks_by_article(
        &self,
        article_id: &str,
        exclude_statuses: &[&str],
    ) -> Result<usize> {
        let article_id = article_id.trim();
        if article_id.is_empty() {
            return Ok(0);
        }

        let table = self.tasks_table().await?;
        let mut filters = vec![format!("article_id = '{}'", escape_literal(article_id))];
        for status in exclude_statuses {
            let normalized = status.trim();
            if !normalized.is_empty() {
                filters.push(format!("status != '{}'", escape_literal(normalized)));
            }
        }
        let count = table
            .count_rows(Some(filters.join(" AND ")))
            .await
            .context("failed to count comment tasks by article")? as usize;
        Ok(count)
    }

    pub async fn patch_comment_task(
        &self,
        task_id: &str,
        patch: CommentTaskPatch,
    ) -> Result<Option<CommentTaskRecord>> {
        let mut record = match self.get_comment_task(task_id).await? {
            Some(record) => record,
            None => return Ok(None),
        };

        if let Some(value) = patch.comment_text {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                record.comment_text = trimmed.to_string();
            }
        }
        if patch.selected_text.is_some() {
            record.selected_text = normalize_optional_text(patch.selected_text);
        }
        if patch.anchor_block_id.is_some() {
            record.anchor_block_id = normalize_optional_text(patch.anchor_block_id);
        }
        if patch.anchor_context_before.is_some() {
            record.anchor_context_before = normalize_optional_text(patch.anchor_context_before);
        }
        if patch.anchor_context_after.is_some() {
            record.anchor_context_after = normalize_optional_text(patch.anchor_context_after);
        }
        if patch.admin_note.is_some() {
            record.admin_note = normalize_optional_text(patch.admin_note);
        }
        record.updated_at = now_ms();

        let table = self.tasks_table().await?;
        upsert_comment_task_record(&table, &record).await?;
        Ok(Some(record))
    }

    pub async fn transition_comment_task(
        &self,
        task_id: &str,
        next_status: &str,
        admin_note: Option<String>,
        failure_reason: Option<String>,
        bump_attempt: bool,
    ) -> Result<Option<CommentTaskRecord>> {
        let mut record = match self.get_comment_task(task_id).await? {
            Some(record) => record,
            None => return Ok(None),
        };

        validate_transition(&record.status, next_status)?;

        let now = now_ms();
        record.status = next_status.to_string();
        record.updated_at = now;
        if let Some(note) = admin_note {
            record.admin_note = normalize_optional_text(Some(note));
        }
        if let Some(reason) = failure_reason {
            record.failure_reason = normalize_optional_text(Some(reason));
        }
        if bump_attempt {
            record.attempt_count = record.attempt_count.saturating_add(1);
        }
        if (next_status == COMMENT_STATUS_APPROVED || next_status == COMMENT_STATUS_RUNNING)
            && record.approved_at.is_none()
        {
            record.approved_at = Some(now);
        }
        if next_status == COMMENT_STATUS_DONE || next_status == COMMENT_STATUS_REJECTED {
            record.completed_at = Some(now);
        }
        if next_status == COMMENT_STATUS_RUNNING {
            record.failure_reason = None;
            record.completed_at = None;
        }

        let table = self.tasks_table().await?;
        upsert_comment_task_record(&table, &record).await?;
        Ok(Some(record))
    }

    pub async fn delete_comment_task(&self, task_id: &str) -> Result<()> {
        let tasks_table = self.tasks_table().await?;
        tasks_table
            .delete(&format!("task_id = '{}'", escape_literal(task_id)))
            .await
            .context("failed to delete comment task")?;

        let task_filter = format!("task_id = '{}'", escape_literal(task_id));
        self.published_table()
            .await?
            .delete(&task_filter)
            .await
            .context("failed to delete related published comments")?;
        self.audit_table()
            .await?
            .delete(&task_filter)
            .await
            .context("failed to delete related audit logs")?;
        self.ai_runs_table()
            .await?
            .delete(&task_filter)
            .await
            .context("failed to delete related ai runs")?;
        self.ai_chunks_table()
            .await?
            .delete(&task_filter)
            .await
            .context("failed to delete related ai chunks")?;
        Ok(())
    }

    pub async fn upsert_published_comment(
        &self,
        input: NewPublishedCommentInput,
    ) -> Result<PublishedCommentRecord> {
        let record = PublishedCommentRecord {
            comment_id: input.comment_id,
            task_id: input.task_id,
            article_id: input.article_id,
            author_name: input.author_name,
            author_avatar_seed: input.author_avatar_seed,
            author_hash: input.author_hash,
            comment_text: input.comment_text,
            selected_text: normalize_optional_text(input.selected_text),
            anchor_block_id: normalize_optional_text(input.anchor_block_id),
            anchor_context_before: normalize_optional_text(input.anchor_context_before),
            anchor_context_after: normalize_optional_text(input.anchor_context_after),
            reply_to_comment_id: normalize_optional_text(input.reply_to_comment_id),
            reply_to_comment_text: normalize_optional_text(input.reply_to_comment_text),
            reply_to_ai_reply_markdown: normalize_optional_text(input.reply_to_ai_reply_markdown),
            ai_reply_markdown: input.ai_reply_markdown,
            ip_region: input.ip_region,
            published_at: now_ms(),
        };

        let table = self.published_table().await?;
        upsert_published_comment_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn get_published_comment_by_task_id(
        &self,
        task_id: &str,
    ) -> Result<Option<PublishedCommentRecord>> {
        let task_id = task_id.trim();
        if task_id.is_empty() {
            return Ok(None);
        }

        let table = self.published_table().await?;
        let filter = format!("task_id = '{}'", escape_literal(task_id));
        let rows = query_published_comments(&table, Some(&filter), Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn get_published_comment_by_comment_id(
        &self,
        comment_id: &str,
    ) -> Result<Option<PublishedCommentRecord>> {
        let comment_id = comment_id.trim();
        if comment_id.is_empty() {
            return Ok(None);
        }

        let table = self.published_table().await?;
        let filter = format!("comment_id = '{}'", escape_literal(comment_id));
        let rows = query_published_comments(&table, Some(&filter), Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_published_comments(
        &self,
        article_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PublishedCommentRecord>> {
        let table = self.published_table().await?;
        let filter = article_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| format!("article_id = '{}'", escape_literal(value)));
        let mut rows =
            query_published_comments(&table, filter.as_deref(), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| right.published_at.cmp(&left.published_at));
        Ok(rows)
    }

    pub async fn patch_published_comment(
        &self,
        comment_id: &str,
        patch: PublishedCommentPatch,
    ) -> Result<Option<PublishedCommentRecord>> {
        let mut record = match self.get_published_comment_by_comment_id(comment_id).await? {
            Some(record) => record,
            None => return Ok(None),
        };

        if let Some(value) = patch.comment_text {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                record.comment_text = trimmed.to_string();
            }
        }

        if let Some(value) = patch.ai_reply_markdown {
            record.ai_reply_markdown = value.trim().to_string();
        }

        record.published_at = now_ms();
        let table = self.published_table().await?;
        upsert_published_comment_record(&table, &record).await?;
        Ok(Some(record))
    }

    pub async fn delete_published_comment(&self, comment_id: &str) -> Result<()> {
        let table = self.published_table().await?;
        table
            .delete(&format!("comment_id = '{}'", escape_literal(comment_id)))
            .await
            .context("failed to delete published comment")?;
        Ok(())
    }

    pub async fn count_published_comments(&self, article_id: &str) -> Result<usize> {
        let table = self.published_table().await?;
        let filter = format!("article_id = '{}'", escape_literal(article_id));
        let count = table
            .count_rows(Some(filter))
            .await
            .context("failed to count published comments")? as usize;
        Ok(count)
    }

    pub async fn append_audit_log(
        &self,
        input: NewCommentAuditInput,
    ) -> Result<CommentAuditRecord> {
        let record = CommentAuditRecord {
            log_id: input.log_id,
            task_id: input.task_id,
            action: input.action,
            operator: input.operator,
            before_json: normalize_optional_text(input.before_json),
            after_json: normalize_optional_text(input.after_json),
            created_at: now_ms(),
        };

        let table = self.audit_table().await?;
        upsert_comment_audit_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn list_audit_logs(
        &self,
        task_id: Option<&str>,
        action: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommentAuditRecord>> {
        let table = self.audit_table().await?;
        let mut filters = Vec::new();
        if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
            filters.push(format!("task_id = '{}'", escape_literal(task_id)));
        }
        if let Some(action) = action.map(str::trim).filter(|value| !value.is_empty()) {
            filters.push(format!("action = '{}'", escape_literal(action)));
        }
        let filter = if filters.is_empty() { None } else { Some(filters.join(" AND ")) };

        let mut rows =
            query_comment_audit_logs(&table, filter.as_deref(), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(rows)
    }

    pub async fn create_ai_run(&self, input: NewCommentAiRunInput) -> Result<CommentAiRunRecord> {
        let now = now_ms();
        let record = CommentAiRunRecord {
            run_id: input.run_id,
            task_id: input.task_id,
            status: COMMENT_AI_RUN_STATUS_RUNNING.to_string(),
            runner_program: input.runner_program,
            runner_args_json: input.runner_args_json,
            skill_path: input.skill_path,
            exit_code: None,
            final_reply_markdown: None,
            failure_reason: None,
            started_at: now,
            updated_at: now,
            completed_at: None,
        };

        let table = self.ai_runs_table().await?;
        upsert_comment_ai_run_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn get_ai_run(&self, run_id: &str) -> Result<Option<CommentAiRunRecord>> {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Ok(None);
        }
        let table = self.ai_runs_table().await?;
        let filter = format!("run_id = '{}'", escape_literal(run_id));
        let rows = query_comment_ai_runs(&table, Some(&filter), Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn list_ai_runs(
        &self,
        task_id: Option<&str>,
        status: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CommentAiRunRecord>> {
        let table = self.ai_runs_table().await?;
        let mut filters = Vec::new();
        if let Some(task_id) = task_id.map(str::trim).filter(|value| !value.is_empty()) {
            filters.push(format!("task_id = '{}'", escape_literal(task_id)));
        }
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            filters.push(format!("status = '{}'", escape_literal(status)));
        }
        let filter = if filters.is_empty() { None } else { Some(filters.join(" AND ")) };
        let mut rows = query_comment_ai_runs(&table, filter.as_deref(), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| right.started_at.cmp(&left.started_at));
        Ok(rows)
    }

    pub async fn append_ai_run_chunk(
        &self,
        input: NewCommentAiRunChunkInput,
    ) -> Result<CommentAiRunChunkRecord> {
        let content = input.content;
        if content.is_empty() {
            anyhow::bail!("ai run chunk content cannot be empty");
        }

        let record = CommentAiRunChunkRecord {
            chunk_id: input.chunk_id,
            run_id: input.run_id,
            task_id: input.task_id,
            stream: input.stream,
            batch_index: input.batch_index,
            content,
            created_at: now_ms(),
        };

        let table = self.ai_chunks_table().await?;
        upsert_comment_ai_chunk_record(&table, &record).await?;
        Ok(record)
    }

    pub async fn list_ai_run_chunks(
        &self,
        run_id: &str,
        limit: usize,
    ) -> Result<Vec<CommentAiRunChunkRecord>> {
        let run_id = run_id.trim();
        if run_id.is_empty() {
            return Ok(vec![]);
        }
        let table = self.ai_chunks_table().await?;
        let filter = format!("run_id = '{}'", escape_literal(run_id));
        let mut rows = query_comment_ai_chunks(&table, Some(&filter), Some(limit.max(1))).await?;
        rows.sort_by(|left, right| left.batch_index.cmp(&right.batch_index));
        Ok(rows)
    }

    pub async fn finalize_ai_run(
        &self,
        run_id: &str,
        status: &str,
        exit_code: Option<i32>,
        failure_reason: Option<String>,
        final_reply_markdown: Option<String>,
    ) -> Result<Option<CommentAiRunRecord>> {
        let mut record = match self.get_ai_run(run_id).await? {
            Some(record) => record,
            None => return Ok(None),
        };

        if status != COMMENT_AI_RUN_STATUS_SUCCESS && status != COMMENT_AI_RUN_STATUS_FAILED {
            anyhow::bail!("invalid ai run status: {status}");
        }

        let now = now_ms();
        record.status = status.to_string();
        record.exit_code = exit_code;
        record.failure_reason = normalize_optional_text(failure_reason);
        record.final_reply_markdown = normalize_optional_text(final_reply_markdown);
        record.updated_at = now;
        record.completed_at = Some(now);

        let table = self.ai_runs_table().await?;
        upsert_comment_ai_run_record(&table, &record).await?;
        Ok(Some(record))
    }

    pub async fn cleanup_comment_tasks(
        &self,
        status: Option<&str>,
        before_ms: Option<i64>,
    ) -> Result<usize> {
        let table = self.tasks_table().await?;
        let mut filters = Vec::new();
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            filters.push(format!("status = '{}'", escape_literal(status)));
        }
        if let Some(before) = before_ms {
            filters.push(format!("created_at < {before}"));
        }
        if filters.is_empty() {
            return Ok(0);
        }

        let filter = filters.join(" AND ");
        let candidates = query_comment_tasks(&table, Some(&filter), None).await?;
        if candidates.is_empty() {
            return Ok(0);
        }

        for candidate in &candidates {
            self.delete_comment_task(&candidate.task_id)
                .await
                .with_context(|| format!("failed to cleanup task {}", candidate.task_id))?;
        }

        Ok(candidates.len())
    }

    pub async fn status_breakdown(&self) -> Result<HashMap<String, usize>> {
        let table = self.tasks_table().await?;
        let rows = query_comment_tasks(&table, None, None).await?;
        let mut counts = HashMap::new();
        for row in rows {
            *counts.entry(row.status).or_insert(0) += 1;
        }
        Ok(counts)
    }
}

fn validate_transition(current_status: &str, next_status: &str) -> Result<()> {
    if current_status == next_status {
        anyhow::bail!("invalid comment task transition: {current_status} -> {next_status}");
    }

    let is_valid = matches!(
        (current_status, next_status),
        (
            COMMENT_STATUS_PENDING,
            COMMENT_STATUS_APPROVED | COMMENT_STATUS_RUNNING | COMMENT_STATUS_REJECTED
        ) | (COMMENT_STATUS_APPROVED, COMMENT_STATUS_RUNNING | COMMENT_STATUS_REJECTED)
            | (COMMENT_STATUS_RUNNING, COMMENT_STATUS_DONE | COMMENT_STATUS_FAILED)
            | (
                COMMENT_STATUS_FAILED,
                COMMENT_STATUS_APPROVED | COMMENT_STATUS_RUNNING | COMMENT_STATUS_REJECTED
            )
    );

    if is_valid {
        Ok(())
    } else {
        anyhow::bail!("invalid comment task transition: {current_status} -> {next_status}")
    }
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn normalize_optional_text(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

async fn ensure_table(db: &Connection, table_name: &str, schema: Arc<Schema>) -> Result<Table> {
    match db.open_table(table_name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            db.create_table(table_name, Box::new(batches))
                .execute()
                .await
                .with_context(|| format!("failed to create table {table_name}"))?;
            db.open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("failed to open table {table_name}"))
        },
    }
}

fn comment_task_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("task_id", DataType::Utf8, false),
        Field::new("article_id", DataType::Utf8, false),
        Field::new("entry_type", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("comment_text", DataType::Utf8, false),
        Field::new("selected_text", DataType::Utf8, true),
        Field::new("anchor_block_id", DataType::Utf8, true),
        Field::new("anchor_context_before", DataType::Utf8, true),
        Field::new("anchor_context_after", DataType::Utf8, true),
        Field::new("reply_to_comment_id", DataType::Utf8, true),
        Field::new("reply_to_comment_text", DataType::Utf8, true),
        Field::new("reply_to_ai_reply_markdown", DataType::Utf8, true),
        Field::new("client_ip", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("fingerprint", DataType::Utf8, false),
        Field::new("ua", DataType::Utf8, true),
        Field::new("language", DataType::Utf8, true),
        Field::new("platform", DataType::Utf8, true),
        Field::new("timezone", DataType::Utf8, true),
        Field::new("viewport", DataType::Utf8, true),
        Field::new("referrer", DataType::Utf8, true),
        Field::new("admin_note", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("attempt_count", DataType::Int32, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("approved_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
        Field::new("completed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

fn comment_published_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("comment_id", DataType::Utf8, false),
        Field::new("task_id", DataType::Utf8, false),
        Field::new("article_id", DataType::Utf8, false),
        Field::new("author_name", DataType::Utf8, false),
        Field::new("author_avatar_seed", DataType::Utf8, false),
        Field::new("author_hash", DataType::Utf8, false),
        Field::new("comment_text", DataType::Utf8, false),
        Field::new("selected_text", DataType::Utf8, true),
        Field::new("anchor_block_id", DataType::Utf8, true),
        Field::new("anchor_context_before", DataType::Utf8, true),
        Field::new("anchor_context_after", DataType::Utf8, true),
        Field::new("reply_to_comment_id", DataType::Utf8, true),
        Field::new("reply_to_comment_text", DataType::Utf8, true),
        Field::new("reply_to_ai_reply_markdown", DataType::Utf8, true),
        Field::new("ai_reply_markdown", DataType::Utf8, false),
        Field::new("ip_region", DataType::Utf8, false),
        Field::new("published_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn comment_audit_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("log_id", DataType::Utf8, false),
        Field::new("task_id", DataType::Utf8, false),
        Field::new("action", DataType::Utf8, false),
        Field::new("operator", DataType::Utf8, false),
        Field::new("before_json", DataType::Utf8, true),
        Field::new("after_json", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn comment_ai_runs_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("run_id", DataType::Utf8, false),
        Field::new("task_id", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("runner_program", DataType::Utf8, false),
        Field::new("runner_args_json", DataType::Utf8, false),
        Field::new("skill_path", DataType::Utf8, false),
        Field::new("exit_code", DataType::Int32, true),
        Field::new("final_reply_markdown", DataType::Utf8, true),
        Field::new("failure_reason", DataType::Utf8, true),
        Field::new("started_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("completed_at", DataType::Timestamp(TimeUnit::Millisecond, None), true),
    ]))
}

fn comment_ai_chunks_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("chunk_id", DataType::Utf8, false),
        Field::new("run_id", DataType::Utf8, false),
        Field::new("task_id", DataType::Utf8, false),
        Field::new("stream", DataType::Utf8, false),
        Field::new("batch_index", DataType::Int32, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn build_comment_task_batch(record: &CommentTaskRecord) -> Result<RecordBatch> {
    let mut task_id = StringBuilder::new();
    let mut article_id = StringBuilder::new();
    let mut entry_type = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut comment_text = StringBuilder::new();
    let mut selected_text = StringBuilder::new();
    let mut anchor_block_id = StringBuilder::new();
    let mut anchor_context_before = StringBuilder::new();
    let mut anchor_context_after = StringBuilder::new();
    let mut reply_to_comment_id = StringBuilder::new();
    let mut reply_to_comment_text = StringBuilder::new();
    let mut reply_to_ai_reply_markdown = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut fingerprint = StringBuilder::new();
    let mut ua = StringBuilder::new();
    let mut language = StringBuilder::new();
    let mut platform = StringBuilder::new();
    let mut timezone = StringBuilder::new();
    let mut viewport = StringBuilder::new();
    let mut referrer = StringBuilder::new();
    let mut admin_note = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut attempt_count = Int32Builder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut approved_at = TimestampMillisecondBuilder::new();
    let mut completed_at = TimestampMillisecondBuilder::new();

    task_id.append_value(&record.task_id);
    article_id.append_value(&record.article_id);
    entry_type.append_value(&record.entry_type);
    status.append_value(&record.status);
    comment_text.append_value(&record.comment_text);
    selected_text.append_option(record.selected_text.as_deref());
    anchor_block_id.append_option(record.anchor_block_id.as_deref());
    anchor_context_before.append_option(record.anchor_context_before.as_deref());
    anchor_context_after.append_option(record.anchor_context_after.as_deref());
    reply_to_comment_id.append_option(record.reply_to_comment_id.as_deref());
    reply_to_comment_text.append_option(record.reply_to_comment_text.as_deref());
    reply_to_ai_reply_markdown.append_option(record.reply_to_ai_reply_markdown.as_deref());
    client_ip.append_value(&record.client_ip);
    ip_region.append_value(&record.ip_region);
    fingerprint.append_value(&record.fingerprint);
    ua.append_option(record.ua.as_deref());
    language.append_option(record.language.as_deref());
    platform.append_option(record.platform.as_deref());
    timezone.append_option(record.timezone.as_deref());
    viewport.append_option(record.viewport.as_deref());
    referrer.append_option(record.referrer.as_deref());
    admin_note.append_option(record.admin_note.as_deref());
    failure_reason.append_option(record.failure_reason.as_deref());
    attempt_count.append_value(record.attempt_count);
    created_at.append_value(record.created_at);
    updated_at.append_value(record.updated_at);
    append_optional_ts(&mut approved_at, record.approved_at);
    append_optional_ts(&mut completed_at, record.completed_at);

    let schema = comment_task_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(task_id.finish()),
        Arc::new(article_id.finish()),
        Arc::new(entry_type.finish()),
        Arc::new(status.finish()),
        Arc::new(comment_text.finish()),
        Arc::new(selected_text.finish()),
        Arc::new(anchor_block_id.finish()),
        Arc::new(anchor_context_before.finish()),
        Arc::new(anchor_context_after.finish()),
        Arc::new(reply_to_comment_id.finish()),
        Arc::new(reply_to_comment_text.finish()),
        Arc::new(reply_to_ai_reply_markdown.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(fingerprint.finish()),
        Arc::new(ua.finish()),
        Arc::new(language.finish()),
        Arc::new(platform.finish()),
        Arc::new(timezone.finish()),
        Arc::new(viewport.finish()),
        Arc::new(referrer.finish()),
        Arc::new(admin_note.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(attempt_count.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(approved_at.finish()),
        Arc::new(completed_at.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn build_published_comment_batch(record: &PublishedCommentRecord) -> Result<RecordBatch> {
    let mut comment_id = StringBuilder::new();
    let mut task_id = StringBuilder::new();
    let mut article_id = StringBuilder::new();
    let mut author_name = StringBuilder::new();
    let mut author_avatar_seed = StringBuilder::new();
    let mut author_hash = StringBuilder::new();
    let mut comment_text = StringBuilder::new();
    let mut selected_text = StringBuilder::new();
    let mut anchor_block_id = StringBuilder::new();
    let mut anchor_context_before = StringBuilder::new();
    let mut anchor_context_after = StringBuilder::new();
    let mut reply_to_comment_id = StringBuilder::new();
    let mut reply_to_comment_text = StringBuilder::new();
    let mut reply_to_ai_reply_markdown = StringBuilder::new();
    let mut ai_reply_markdown = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut published_at = TimestampMillisecondBuilder::new();

    comment_id.append_value(&record.comment_id);
    task_id.append_value(&record.task_id);
    article_id.append_value(&record.article_id);
    author_name.append_value(&record.author_name);
    author_avatar_seed.append_value(&record.author_avatar_seed);
    author_hash.append_value(&record.author_hash);
    comment_text.append_value(&record.comment_text);
    selected_text.append_option(record.selected_text.as_deref());
    anchor_block_id.append_option(record.anchor_block_id.as_deref());
    anchor_context_before.append_option(record.anchor_context_before.as_deref());
    anchor_context_after.append_option(record.anchor_context_after.as_deref());
    reply_to_comment_id.append_option(record.reply_to_comment_id.as_deref());
    reply_to_comment_text.append_option(record.reply_to_comment_text.as_deref());
    reply_to_ai_reply_markdown.append_option(record.reply_to_ai_reply_markdown.as_deref());
    ai_reply_markdown.append_value(&record.ai_reply_markdown);
    ip_region.append_value(&record.ip_region);
    published_at.append_value(record.published_at);

    let schema = comment_published_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(comment_id.finish()),
        Arc::new(task_id.finish()),
        Arc::new(article_id.finish()),
        Arc::new(author_name.finish()),
        Arc::new(author_avatar_seed.finish()),
        Arc::new(author_hash.finish()),
        Arc::new(comment_text.finish()),
        Arc::new(selected_text.finish()),
        Arc::new(anchor_block_id.finish()),
        Arc::new(anchor_context_before.finish()),
        Arc::new(anchor_context_after.finish()),
        Arc::new(reply_to_comment_id.finish()),
        Arc::new(reply_to_comment_text.finish()),
        Arc::new(reply_to_ai_reply_markdown.finish()),
        Arc::new(ai_reply_markdown.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(published_at.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn build_comment_audit_batch(record: &CommentAuditRecord) -> Result<RecordBatch> {
    let mut log_id = StringBuilder::new();
    let mut task_id = StringBuilder::new();
    let mut action = StringBuilder::new();
    let mut operator = StringBuilder::new();
    let mut before_json = StringBuilder::new();
    let mut after_json = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    log_id.append_value(&record.log_id);
    task_id.append_value(&record.task_id);
    action.append_value(&record.action);
    operator.append_value(&record.operator);
    before_json.append_option(record.before_json.as_deref());
    after_json.append_option(record.after_json.as_deref());
    created_at.append_value(record.created_at);

    let schema = comment_audit_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(log_id.finish()),
        Arc::new(task_id.finish()),
        Arc::new(action.finish()),
        Arc::new(operator.finish()),
        Arc::new(before_json.finish()),
        Arc::new(after_json.finish()),
        Arc::new(created_at.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn build_comment_ai_run_batch(record: &CommentAiRunRecord) -> Result<RecordBatch> {
    let mut run_id = StringBuilder::new();
    let mut task_id = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut runner_program = StringBuilder::new();
    let mut runner_args_json = StringBuilder::new();
    let mut skill_path = StringBuilder::new();
    let mut exit_code = Int32Builder::new();
    let mut final_reply_markdown = StringBuilder::new();
    let mut failure_reason = StringBuilder::new();
    let mut started_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();
    let mut completed_at = TimestampMillisecondBuilder::new();

    run_id.append_value(&record.run_id);
    task_id.append_value(&record.task_id);
    status.append_value(&record.status);
    runner_program.append_value(&record.runner_program);
    runner_args_json.append_value(&record.runner_args_json);
    skill_path.append_value(&record.skill_path);
    match record.exit_code {
        Some(value) => exit_code.append_value(value),
        None => exit_code.append_null(),
    }
    final_reply_markdown.append_option(record.final_reply_markdown.as_deref());
    failure_reason.append_option(record.failure_reason.as_deref());
    started_at.append_value(record.started_at);
    updated_at.append_value(record.updated_at);
    append_optional_ts(&mut completed_at, record.completed_at);

    let schema = comment_ai_runs_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(run_id.finish()),
        Arc::new(task_id.finish()),
        Arc::new(status.finish()),
        Arc::new(runner_program.finish()),
        Arc::new(runner_args_json.finish()),
        Arc::new(skill_path.finish()),
        Arc::new(exit_code.finish()),
        Arc::new(final_reply_markdown.finish()),
        Arc::new(failure_reason.finish()),
        Arc::new(started_at.finish()),
        Arc::new(updated_at.finish()),
        Arc::new(completed_at.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn build_comment_ai_chunk_batch(record: &CommentAiRunChunkRecord) -> Result<RecordBatch> {
    let mut chunk_id = StringBuilder::new();
    let mut run_id = StringBuilder::new();
    let mut task_id = StringBuilder::new();
    let mut stream = StringBuilder::new();
    let mut batch_index = Int32Builder::new();
    let mut content = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    chunk_id.append_value(&record.chunk_id);
    run_id.append_value(&record.run_id);
    task_id.append_value(&record.task_id);
    stream.append_value(&record.stream);
    batch_index.append_value(record.batch_index);
    content.append_value(&record.content);
    created_at.append_value(record.created_at);

    let schema = comment_ai_chunks_schema();
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(chunk_id.finish()),
        Arc::new(run_id.finish()),
        Arc::new(task_id.finish()),
        Arc::new(stream.finish()),
        Arc::new(batch_index.finish()),
        Arc::new(content.finish()),
        Arc::new(created_at.finish()),
    ];
    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn append_optional_ts(builder: &mut TimestampMillisecondBuilder, value: Option<i64>) {
    match value {
        Some(v) => builder.append_value(v),
        None => builder.append_null(),
    }
}

async fn upsert_comment_task_record(table: &Table, record: &CommentTaskRecord) -> Result<()> {
    let batch = build_comment_task_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["task_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge
        .execute(Box::new(batches))
        .await
        .context("failed to upsert comment task")?;
    Ok(())
}

async fn upsert_published_comment_record(
    table: &Table,
    record: &PublishedCommentRecord,
) -> Result<()> {
    let batch = build_published_comment_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["task_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge
        .execute(Box::new(batches))
        .await
        .context("failed to upsert published comment")?;
    Ok(())
}

async fn upsert_comment_audit_record(table: &Table, record: &CommentAuditRecord) -> Result<()> {
    let batch = build_comment_audit_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["log_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge
        .execute(Box::new(batches))
        .await
        .context("failed to upsert comment audit log")?;
    Ok(())
}

async fn upsert_comment_ai_run_record(table: &Table, record: &CommentAiRunRecord) -> Result<()> {
    let batch = build_comment_ai_run_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["run_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge
        .execute(Box::new(batches))
        .await
        .context("failed to upsert comment ai run")?;
    Ok(())
}

async fn upsert_comment_ai_chunk_record(
    table: &Table,
    record: &CommentAiRunChunkRecord,
) -> Result<()> {
    let batch = build_comment_ai_chunk_batch(record)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
    let mut merge = table.merge_insert(&["chunk_id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge
        .execute(Box::new(batches))
        .await
        .context("failed to upsert comment ai chunk")?;
    Ok(())
}

async fn query_comment_tasks(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<CommentTaskRecord>> {
    let mut query = table.query();
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }

    let batches = query
        .select(Select::columns(&[
            "task_id",
            "article_id",
            "entry_type",
            "status",
            "comment_text",
            "selected_text",
            "anchor_block_id",
            "anchor_context_before",
            "anchor_context_after",
            "reply_to_comment_id",
            "reply_to_comment_text",
            "reply_to_ai_reply_markdown",
            "client_ip",
            "ip_region",
            "fingerprint",
            "ua",
            "language",
            "platform",
            "timezone",
            "viewport",
            "referrer",
            "admin_note",
            "failure_reason",
            "attempt_count",
            "created_at",
            "updated_at",
            "approved_at",
            "completed_at",
        ]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let task_id = string_array(&batch, "task_id")?;
        let article_id = string_array(&batch, "article_id")?;
        let entry_type = string_array(&batch, "entry_type")?;
        let status = string_array(&batch, "status")?;
        let comment_text = string_array(&batch, "comment_text")?;
        let selected_text = string_array(&batch, "selected_text")?;
        let anchor_block_id = string_array(&batch, "anchor_block_id")?;
        let anchor_context_before = string_array(&batch, "anchor_context_before")?;
        let anchor_context_after = string_array(&batch, "anchor_context_after")?;
        let reply_to_comment_id = string_array(&batch, "reply_to_comment_id")?;
        let reply_to_comment_text = string_array(&batch, "reply_to_comment_text")?;
        let reply_to_ai_reply_markdown = string_array(&batch, "reply_to_ai_reply_markdown")?;
        let client_ip = string_array(&batch, "client_ip")?;
        let ip_region = string_array(&batch, "ip_region")?;
        let fingerprint = string_array(&batch, "fingerprint")?;
        let ua = string_array(&batch, "ua")?;
        let language = string_array(&batch, "language")?;
        let platform = string_array(&batch, "platform")?;
        let timezone = string_array(&batch, "timezone")?;
        let viewport = string_array(&batch, "viewport")?;
        let referrer = string_array(&batch, "referrer")?;
        let admin_note = string_array(&batch, "admin_note")?;
        let failure_reason = string_array(&batch, "failure_reason")?;
        let attempt_count = int32_array(&batch, "attempt_count")?;
        let created_at = ts_array(&batch, "created_at")?;
        let updated_at = ts_array(&batch, "updated_at")?;
        let approved_at = ts_array(&batch, "approved_at")?;
        let completed_at = ts_array(&batch, "completed_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(CommentTaskRecord {
                task_id: task_id.value(idx).to_string(),
                article_id: article_id.value(idx).to_string(),
                entry_type: entry_type.value(idx).to_string(),
                status: status.value(idx).to_string(),
                comment_text: comment_text.value(idx).to_string(),
                selected_text: nullable_string_value(selected_text, idx),
                anchor_block_id: nullable_string_value(anchor_block_id, idx),
                anchor_context_before: nullable_string_value(anchor_context_before, idx),
                anchor_context_after: nullable_string_value(anchor_context_after, idx),
                reply_to_comment_id: nullable_string_value(reply_to_comment_id, idx),
                reply_to_comment_text: nullable_string_value(reply_to_comment_text, idx),
                reply_to_ai_reply_markdown: nullable_string_value(reply_to_ai_reply_markdown, idx),
                client_ip: client_ip.value(idx).to_string(),
                ip_region: ip_region.value(idx).to_string(),
                fingerprint: fingerprint.value(idx).to_string(),
                ua: nullable_string_value(ua, idx),
                language: nullable_string_value(language, idx),
                platform: nullable_string_value(platform, idx),
                timezone: nullable_string_value(timezone, idx),
                viewport: nullable_string_value(viewport, idx),
                referrer: nullable_string_value(referrer, idx),
                admin_note: nullable_string_value(admin_note, idx),
                failure_reason: nullable_string_value(failure_reason, idx),
                attempt_count: attempt_count.value(idx),
                created_at: created_at.value(idx),
                updated_at: updated_at.value(idx),
                approved_at: nullable_ts_value(approved_at, idx),
                completed_at: nullable_ts_value(completed_at, idx),
            });
        }
    }

    Ok(rows)
}

async fn query_published_comments(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<PublishedCommentRecord>> {
    let mut query = table.query();
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }

    let batches = query
        .select(Select::columns(&[
            "comment_id",
            "task_id",
            "article_id",
            "author_name",
            "author_avatar_seed",
            "author_hash",
            "comment_text",
            "selected_text",
            "anchor_block_id",
            "anchor_context_before",
            "anchor_context_after",
            "reply_to_comment_id",
            "reply_to_comment_text",
            "reply_to_ai_reply_markdown",
            "ai_reply_markdown",
            "ip_region",
            "published_at",
        ]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let comment_id = string_array(&batch, "comment_id")?;
        let task_id = string_array(&batch, "task_id")?;
        let article_id = string_array(&batch, "article_id")?;
        let author_name = string_array(&batch, "author_name")?;
        let author_avatar_seed = string_array(&batch, "author_avatar_seed")?;
        let author_hash = string_array(&batch, "author_hash")?;
        let comment_text = string_array(&batch, "comment_text")?;
        let selected_text = string_array(&batch, "selected_text")?;
        let anchor_block_id = string_array(&batch, "anchor_block_id")?;
        let anchor_context_before = string_array(&batch, "anchor_context_before")?;
        let anchor_context_after = string_array(&batch, "anchor_context_after")?;
        let reply_to_comment_id = string_array(&batch, "reply_to_comment_id")?;
        let reply_to_comment_text = string_array(&batch, "reply_to_comment_text")?;
        let reply_to_ai_reply_markdown = string_array(&batch, "reply_to_ai_reply_markdown")?;
        let ai_reply_markdown = string_array(&batch, "ai_reply_markdown")?;
        let ip_region = string_array(&batch, "ip_region")?;
        let published_at = ts_array(&batch, "published_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(PublishedCommentRecord {
                comment_id: comment_id.value(idx).to_string(),
                task_id: task_id.value(idx).to_string(),
                article_id: article_id.value(idx).to_string(),
                author_name: author_name.value(idx).to_string(),
                author_avatar_seed: author_avatar_seed.value(idx).to_string(),
                author_hash: author_hash.value(idx).to_string(),
                comment_text: comment_text.value(idx).to_string(),
                selected_text: nullable_string_value(selected_text, idx),
                anchor_block_id: nullable_string_value(anchor_block_id, idx),
                anchor_context_before: nullable_string_value(anchor_context_before, idx),
                anchor_context_after: nullable_string_value(anchor_context_after, idx),
                reply_to_comment_id: nullable_string_value(reply_to_comment_id, idx),
                reply_to_comment_text: nullable_string_value(reply_to_comment_text, idx),
                reply_to_ai_reply_markdown: nullable_string_value(reply_to_ai_reply_markdown, idx),
                ai_reply_markdown: ai_reply_markdown.value(idx).to_string(),
                ip_region: ip_region.value(idx).to_string(),
                published_at: published_at.value(idx),
            });
        }
    }

    Ok(rows)
}

async fn query_comment_audit_logs(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<CommentAuditRecord>> {
    let mut query = table.query();
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }

    let batches = query
        .select(Select::columns(&[
            "log_id",
            "task_id",
            "action",
            "operator",
            "before_json",
            "after_json",
            "created_at",
        ]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let log_id = string_array(&batch, "log_id")?;
        let task_id = string_array(&batch, "task_id")?;
        let action = string_array(&batch, "action")?;
        let operator = string_array(&batch, "operator")?;
        let before_json = string_array(&batch, "before_json")?;
        let after_json = string_array(&batch, "after_json")?;
        let created_at = ts_array(&batch, "created_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(CommentAuditRecord {
                log_id: log_id.value(idx).to_string(),
                task_id: task_id.value(idx).to_string(),
                action: action.value(idx).to_string(),
                operator: operator.value(idx).to_string(),
                before_json: nullable_string_value(before_json, idx),
                after_json: nullable_string_value(after_json, idx),
                created_at: created_at.value(idx),
            });
        }
    }

    Ok(rows)
}

async fn query_comment_ai_runs(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<CommentAiRunRecord>> {
    let mut query = table.query();
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }

    let batches = query
        .select(Select::columns(&[
            "run_id",
            "task_id",
            "status",
            "runner_program",
            "runner_args_json",
            "skill_path",
            "exit_code",
            "final_reply_markdown",
            "failure_reason",
            "started_at",
            "updated_at",
            "completed_at",
        ]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let run_id = string_array(&batch, "run_id")?;
        let task_id = string_array(&batch, "task_id")?;
        let status = string_array(&batch, "status")?;
        let runner_program = string_array(&batch, "runner_program")?;
        let runner_args_json = string_array(&batch, "runner_args_json")?;
        let skill_path = string_array(&batch, "skill_path")?;
        let exit_code = int32_array(&batch, "exit_code")?;
        let final_reply_markdown = string_array(&batch, "final_reply_markdown")?;
        let failure_reason = string_array(&batch, "failure_reason")?;
        let started_at = ts_array(&batch, "started_at")?;
        let updated_at = ts_array(&batch, "updated_at")?;
        let completed_at = ts_array(&batch, "completed_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(CommentAiRunRecord {
                run_id: run_id.value(idx).to_string(),
                task_id: task_id.value(idx).to_string(),
                status: status.value(idx).to_string(),
                runner_program: runner_program.value(idx).to_string(),
                runner_args_json: runner_args_json.value(idx).to_string(),
                skill_path: skill_path.value(idx).to_string(),
                exit_code: nullable_i32_value(exit_code, idx),
                final_reply_markdown: nullable_string_value(final_reply_markdown, idx),
                failure_reason: nullable_string_value(failure_reason, idx),
                started_at: started_at.value(idx),
                updated_at: updated_at.value(idx),
                completed_at: nullable_ts_value(completed_at, idx),
            });
        }
    }

    Ok(rows)
}

async fn query_comment_ai_chunks(
    table: &Table,
    filter: Option<&str>,
    limit: Option<usize>,
) -> Result<Vec<CommentAiRunChunkRecord>> {
    let mut query = table.query();
    if let Some(filter) = filter {
        query = query.only_if(filter);
    }
    if let Some(limit) = limit {
        query = query.limit(limit.max(1));
    }

    let batches = query
        .select(Select::columns(&[
            "chunk_id",
            "run_id",
            "task_id",
            "stream",
            "batch_index",
            "content",
            "created_at",
        ]))
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    let mut rows = Vec::new();
    for batch in batches {
        let chunk_id = string_array(&batch, "chunk_id")?;
        let run_id = string_array(&batch, "run_id")?;
        let task_id = string_array(&batch, "task_id")?;
        let stream = string_array(&batch, "stream")?;
        let batch_index = int32_array(&batch, "batch_index")?;
        let content = string_array(&batch, "content")?;
        let created_at = ts_array(&batch, "created_at")?;

        for idx in 0..batch.num_rows() {
            rows.push(CommentAiRunChunkRecord {
                chunk_id: chunk_id.value(idx).to_string(),
                run_id: run_id.value(idx).to_string(),
                task_id: task_id.value(idx).to_string(),
                stream: stream.value(idx).to_string(),
                batch_index: batch_index.value(idx),
                content: content.value(idx).to_string(),
                created_at: created_at.value(idx),
            });
        }
    }

    Ok(rows)
}

fn string_array<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a StringArray> {
    let array = batch
        .column_by_name(column)
        .with_context(|| format!("column not found: {column}"))?;
    array
        .as_any()
        .downcast_ref::<StringArray>()
        .with_context(|| format!("column {column} is not StringArray"))
}

fn int32_array<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a Int32Array> {
    let array = batch
        .column_by_name(column)
        .with_context(|| format!("column not found: {column}"))?;
    array
        .as_any()
        .downcast_ref::<Int32Array>()
        .with_context(|| format!("column {column} is not Int32Array"))
}

fn ts_array<'a>(batch: &'a RecordBatch, column: &str) -> Result<&'a TimestampMillisecondArray> {
    let array = batch
        .column_by_name(column)
        .with_context(|| format!("column not found: {column}"))?;
    array
        .as_any()
        .downcast_ref::<TimestampMillisecondArray>()
        .with_context(|| format!("column {column} is not TimestampMillisecondArray"))
}

fn nullable_string_value(array: &StringArray, idx: usize) -> Option<String> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx).to_string())
    }
}

fn nullable_ts_value(array: &TimestampMillisecondArray, idx: usize) -> Option<i64> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn nullable_i32_value(array: &Int32Array, idx: usize) -> Option<i32> {
    if array.is_null(idx) {
        None
    } else {
        Some(array.value(idx))
    }
}

fn escape_literal(input: &str) -> String {
    input.replace('\'', "''")
}
