use std::{collections::HashMap, sync::Arc, time::Instant};

use anyhow::{Context, Result};
use arrow_array::{
    builder::{Int32Builder, StringBuilder, TimestampMillisecondBuilder, UInt64Builder},
    Array, ArrayRef, BooleanArray, Int32Array, RecordBatch, RecordBatchIterator, RecordBatchReader,
    StringArray, TimestampMillisecondArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::Utc;
use futures::TryStreamExt;
use lance::{blob_field, BlobArrayBuilder};
use lancedb::{
    connect,
    query::{ExecutableQuery, QueryBase, Select},
    table::NewColumnTransform,
    Connection, Table,
};
use serde::{Deserialize, Serialize};

pub const INTERACTIVE_PAGES_TABLE: &str = "interactive_pages";
pub const INTERACTIVE_PAGE_LOCALES_TABLE: &str = "interactive_page_locales";
pub const INTERACTIVE_ASSETS_TABLE: &str = "interactive_assets";

pub const INTERACTIVE_TABLE_NAMES: &[&str] =
    &[INTERACTIVE_PAGES_TABLE, INTERACTIVE_PAGE_LOCALES_TABLE, INTERACTIVE_ASSETS_TABLE];

pub const INTERACTIVE_PAGE_STATUS_READY: &str = "ready";
pub const INTERACTIVE_PAGE_STATUS_TEXT_ONLY: &str = "text_only";
pub const INTERACTIVE_PAGE_STATUS_BLOCKED: &str = "blocked";

pub const MIRROR_POLICY_WHITELISTED: &str = "whitelisted";
pub const MIRROR_POLICY_REJECTED: &str = "rejected";

pub const TRANSLATION_SCOPE_ARTICLE_ONLY: &str = "article_only";
pub const TRANSLATION_SCOPE_ARTICLE_AND_INTERACTIVE: &str = "article_and_interactive";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractivePageRecord {
    pub id: String,
    pub article_id: String,
    pub source_url: String,
    pub source_host: String,
    pub source_lang: String,
    pub title: String,
    pub status: String,
    pub mirror_policy: String,
    pub translation_scope: String,
    pub entry_asset_id: String,
    pub entry_asset_path: String,
    pub asset_count: u64,
    pub content_sha256: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractiveAssetRecord {
    pub id: String,
    pub page_id: String,
    pub logical_path: String,
    pub resolved_url: String,
    pub kind: String,
    pub mime_type: String,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub is_entry: bool,
    pub http_status: i32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub bytes: Vec<u8>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractivePageLocaleRecord {
    pub id: String,
    pub page_id: String,
    pub locale: String,
    pub title: String,
    pub entry_asset_id: String,
    pub entry_asset_path: String,
    pub content_sha256: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InteractiveAssetMeta {
    pub id: String,
    pub page_id: String,
    pub logical_path: String,
    pub resolved_url: String,
    pub kind: String,
    pub mime_type: String,
    pub content_sha256: String,
    pub size_bytes: u64,
    pub is_entry: bool,
    pub http_status: i32,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InteractiveAssetBlob {
    pub meta: InteractiveAssetMeta,
    pub bytes: Vec<u8>,
}

pub struct InteractivePageStore {
    db: Connection,
}

impl InteractivePageStore {
    pub async fn connect(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect interactive page LanceDB")?;
        let store = Self {
            db,
        };
        store.pages_table().await?;
        store.page_locales_table().await?;
        store.assets_table().await?;
        Ok(store)
    }

    pub fn connection(&self) -> &Connection {
        &self.db
    }

    async fn pages_table(&self) -> Result<Table> {
        let table = ensure_table(&self.db, INTERACTIVE_PAGES_TABLE, interactive_pages_schema(), &[
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;

        let schema = table.schema().await?;
        if schema.field_with_name("content_sha256").is_err() {
            table
                .add_columns(
                    NewColumnTransform::AllNulls(Arc::new(Schema::new(vec![Field::new(
                        "content_sha256",
                        DataType::Utf8,
                        true,
                    )]))),
                    None,
                )
                .await
                .context("failed to add content_sha256 to interactive_pages")?;
        }
        Ok(table)
    }

    async fn assets_table(&self) -> Result<Table> {
        ensure_table(&self.db, INTERACTIVE_ASSETS_TABLE, interactive_assets_schema(), &[
            ("new_table_data_storage_version", "2.2"),
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await
    }

    async fn page_locales_table(&self) -> Result<Table> {
        ensure_table(
            &self.db,
            INTERACTIVE_PAGE_LOCALES_TABLE,
            interactive_page_locales_schema(),
            &[
                ("new_table_enable_stable_row_ids", "true"),
                ("new_table_enable_v2_manifest_paths", "true"),
            ],
        )
        .await
    }

    pub async fn upsert_page(&self, record: &InteractivePageRecord) -> Result<()> {
        let table = self.pages_table().await?;
        let batch = build_interactive_pages_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert interactive page")?;
        Ok(())
    }

    pub async fn replace_assets(
        &self,
        page_id: &str,
        records: &[InteractiveAssetRecord],
    ) -> Result<()> {
        let table = self.assets_table().await?;
        let esc = escape_literal(page_id);
        table
            .delete(&format!("page_id = '{esc}'"))
            .await
            .with_context(|| format!("failed to delete old interactive assets for `{page_id}`"))?;

        if records.is_empty() {
            return Ok(());
        }

        let batch = build_interactive_assets_batch(records)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        table
            .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .execute()
            .await
            .context("failed to insert interactive assets")?;
        Ok(())
    }

    pub async fn delete_assets_with_prefix(&self, page_id: &str, prefix: &str) -> Result<()> {
        let table = self.assets_table().await?;
        let esc_page = escape_literal(page_id);
        let esc_prefix = escape_literal(prefix);
        table
            .delete(&format!("page_id = '{esc_page}' AND logical_path LIKE '{esc_prefix}%'"))
            .await
            .with_context(|| {
                format!(
                    "failed to delete interactive assets for `{page_id}` with prefix `{prefix}`"
                )
            })?;
        Ok(())
    }

    pub async fn upsert_assets(&self, records: &[InteractiveAssetRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        let table = self.assets_table().await?;
        let batch = build_interactive_assets_batch(records)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["page_id", "logical_path"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert interactive assets")?;
        Ok(())
    }

    pub async fn upsert_page_locale(&self, record: &InteractivePageLocaleRecord) -> Result<()> {
        let table = self.page_locales_table().await?;
        let batch = build_interactive_page_locales_batch(std::slice::from_ref(record))?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
            .await
            .context("failed to upsert interactive page locale")?;
        Ok(())
    }

    pub async fn get_page(&self, page_id: &str) -> Result<Option<InteractivePageRecord>> {
        let table = self.pages_table().await?;
        let esc = escape_literal(page_id);
        let batches = table
            .query()
            .only_if(format!("id = '{esc}'"))
            .limit(1)
            .select(Select::columns(&[
                "id",
                "article_id",
                "source_url",
                "source_host",
                "source_lang",
                "title",
                "status",
                "mirror_policy",
                "translation_scope",
                "entry_asset_id",
                "entry_asset_path",
                "asset_count",
                "content_sha256",
                "created_at",
                "updated_at",
            ]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_interactive_pages(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn get_asset_meta(
        &self,
        page_id: &str,
        logical_path: &str,
    ) -> Result<Option<InteractiveAssetMeta>> {
        let table = self.assets_table().await?;
        let esc_page = escape_literal(page_id);
        let esc_path = escape_literal(logical_path);
        let batches = table
            .query()
            .only_if(format!("page_id = '{esc_page}' AND logical_path = '{esc_path}'"))
            .limit(1)
            .select(Select::columns(&[
                "id",
                "page_id",
                "logical_path",
                "resolved_url",
                "kind",
                "mime_type",
                "content_sha256",
                "size_bytes",
                "is_entry",
                "http_status",
                "etag",
                "last_modified",
                "created_at",
                "updated_at",
            ]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_interactive_assets_meta(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn get_asset_blob(
        &self,
        page_id: &str,
        logical_path: &str,
    ) -> Result<Option<InteractiveAssetBlob>> {
        let table = self.assets_table().await?;
        let esc_page = escape_literal(page_id);
        let esc_path = escape_literal(logical_path);

        let ds_wrapper = table
            .dataset()
            .context("interactive_assets table has no dataset")?;
        let dataset = ds_wrapper.get().await?;

        let mut scanner = dataset.scan();
        scanner.project(&[
            "id",
            "page_id",
            "logical_path",
            "resolved_url",
            "kind",
            "mime_type",
            "content_sha256",
            "size_bytes",
            "is_entry",
            "http_status",
            "etag",
            "last_modified",
            "created_at",
            "updated_at",
        ])?;
        scanner
            .filter(format!("page_id = '{esc_page}' AND logical_path = '{esc_path}'").as_str())?;
        scanner.limit(Some(1), None)?;
        scanner.with_row_id();
        let stream = scanner.try_into_stream().await?;
        let batch_list: Vec<RecordBatch> = stream.try_collect().await?;

        let (row_id, meta) = match batch_list.first() {
            Some(batch) if batch.num_rows() > 0 => {
                let row_id = batch
                    .column_by_name("_rowid")
                    .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                    .map(|a| a.value(0))
                    .context("interactive asset row missing _rowid")?;
                let meta = batch_to_interactive_asset_meta(batch, 0)?;
                (row_id, meta)
            },
            _ => return Ok(None),
        };

        let started = Instant::now();
        let blobs = Arc::new(dataset.clone())
            .take_blobs(&[row_id], "bytes")
            .await
            .context("failed to read interactive asset blob")?;
        let blob = match blobs.into_iter().next() {
            Some(blob) => blob,
            None => return Ok(None),
        };
        let bytes = blob
            .read()
            .await
            .context("interactive asset blob read failed")?
            .to_vec();
        tracing::debug!(
            page_id,
            logical_path,
            bytes = bytes.len(),
            elapsed_ms = started.elapsed().as_millis(),
            "interactive asset blob loaded"
        );
        Ok(Some(InteractiveAssetBlob {
            meta,
            bytes,
        }))
    }

    pub async fn get_page_locale(
        &self,
        page_id: &str,
        locale: &str,
    ) -> Result<Option<InteractivePageLocaleRecord>> {
        let table = self.page_locales_table().await?;
        let esc_page = escape_literal(page_id);
        let esc_locale = escape_literal(locale);
        let batches = table
            .query()
            .only_if(format!("page_id = '{esc_page}' AND locale = '{esc_locale}'"))
            .limit(1)
            .select(Select::columns(&[
                "id",
                "page_id",
                "locale",
                "title",
                "entry_asset_id",
                "entry_asset_path",
                "content_sha256",
                "created_at",
                "updated_at",
            ]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_interactive_page_locales(&batch_list).map(|mut rows| rows.pop())
    }

    pub async fn list_page_locales(
        &self,
        page_id: &str,
    ) -> Result<Vec<InteractivePageLocaleRecord>> {
        let table = self.page_locales_table().await?;
        let esc_page = escape_literal(page_id);
        let batches = table
            .query()
            .only_if(format!("page_id = '{esc_page}'"))
            .select(Select::columns(&[
                "id",
                "page_id",
                "locale",
                "title",
                "entry_asset_id",
                "entry_asset_path",
                "content_sha256",
                "created_at",
                "updated_at",
            ]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_interactive_page_locales(&batch_list)
    }

    pub async fn list_assets_for_page(&self, page_id: &str) -> Result<Vec<InteractiveAssetMeta>> {
        let table = self.assets_table().await?;
        let esc_page = escape_literal(page_id);
        let batches = table
            .query()
            .only_if(format!("page_id = '{esc_page}'"))
            .select(Select::columns(&[
                "id",
                "page_id",
                "logical_path",
                "resolved_url",
                "kind",
                "mime_type",
                "content_sha256",
                "size_bytes",
                "is_entry",
                "http_status",
                "etag",
                "last_modified",
                "created_at",
                "updated_at",
            ]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        batches_to_interactive_assets_meta(&batch_list)
    }
}

fn interactive_pages_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("article_id", DataType::Utf8, false),
        Field::new("source_url", DataType::Utf8, false),
        Field::new("source_host", DataType::Utf8, false),
        Field::new("source_lang", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("status", DataType::Utf8, false),
        Field::new("mirror_policy", DataType::Utf8, false),
        Field::new("translation_scope", DataType::Utf8, false),
        Field::new("entry_asset_id", DataType::Utf8, false),
        Field::new("entry_asset_path", DataType::Utf8, false),
        Field::new("asset_count", DataType::UInt64, false),
        Field::new("content_sha256", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn interactive_page_locales_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("page_id", DataType::Utf8, false),
        Field::new("locale", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("entry_asset_id", DataType::Utf8, false),
        Field::new("entry_asset_path", DataType::Utf8, false),
        Field::new("content_sha256", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn interactive_assets_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("page_id", DataType::Utf8, false),
        Field::new("logical_path", DataType::Utf8, false),
        Field::new("resolved_url", DataType::Utf8, false),
        Field::new("kind", DataType::Utf8, false),
        Field::new("mime_type", DataType::Utf8, false),
        Field::new("content_sha256", DataType::Utf8, false),
        Field::new("size_bytes", DataType::UInt64, false),
        Field::new("is_entry", DataType::Boolean, false),
        Field::new("http_status", DataType::Int32, false),
        Field::new("etag", DataType::Utf8, true),
        Field::new("last_modified", DataType::Utf8, true),
        dedicated_blob_field("bytes", false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

async fn ensure_table(
    db: &Connection,
    table_name: &str,
    schema: Arc<Schema>,
    storage_options: &[(&str, &str)],
) -> Result<Table> {
    match db.open_table(table_name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            let mut builder =
                db.create_table(table_name, Box::new(batches) as Box<dyn RecordBatchReader + Send>);
            for &(k, v) in storage_options {
                builder = builder.storage_option(k, v);
            }
            builder
                .execute()
                .await
                .with_context(|| format!("failed to create table `{table_name}`"))?;
            db.open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("failed to open table `{table_name}`"))
        },
    }
}

fn build_interactive_pages_batch(records: &[InteractivePageRecord]) -> Result<RecordBatch> {
    let schema = interactive_pages_schema();
    let mut id = StringBuilder::new();
    let mut article_id = StringBuilder::new();
    let mut source_url = StringBuilder::new();
    let mut source_host = StringBuilder::new();
    let mut source_lang = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut status = StringBuilder::new();
    let mut mirror_policy = StringBuilder::new();
    let mut translation_scope = StringBuilder::new();
    let mut entry_asset_id = StringBuilder::new();
    let mut entry_asset_path = StringBuilder::new();
    let mut asset_count = UInt64Builder::new();
    let mut content_sha256 = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        article_id.append_value(&record.article_id);
        source_url.append_value(&record.source_url);
        source_host.append_value(&record.source_host);
        source_lang.append_value(&record.source_lang);
        title.append_value(&record.title);
        status.append_value(&record.status);
        mirror_policy.append_value(&record.mirror_policy);
        translation_scope.append_value(&record.translation_scope);
        entry_asset_id.append_value(&record.entry_asset_id);
        entry_asset_path.append_value(&record.entry_asset_path);
        asset_count.append_value(record.asset_count);
        content_sha256.append_value(&record.content_sha256);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(article_id.finish()),
        Arc::new(source_url.finish()),
        Arc::new(source_host.finish()),
        Arc::new(source_lang.finish()),
        Arc::new(title.finish()),
        Arc::new(status.finish()),
        Arc::new(mirror_policy.finish()),
        Arc::new(translation_scope.finish()),
        Arc::new(entry_asset_id.finish()),
        Arc::new(entry_asset_path.finish()),
        Arc::new(asset_count.finish()),
        Arc::new(content_sha256.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build interactive_pages batch")
}

fn build_interactive_page_locales_batch(
    records: &[InteractivePageLocaleRecord],
) -> Result<RecordBatch> {
    let schema = interactive_page_locales_schema();
    let mut id = StringBuilder::new();
    let mut page_id = StringBuilder::new();
    let mut locale = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut entry_asset_id = StringBuilder::new();
    let mut entry_asset_path = StringBuilder::new();
    let mut content_sha256 = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        page_id.append_value(&record.page_id);
        locale.append_value(&record.locale);
        title.append_value(&record.title);
        entry_asset_id.append_value(&record.entry_asset_id);
        entry_asset_path.append_value(&record.entry_asset_path);
        content_sha256.append_value(&record.content_sha256);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(page_id.finish()),
        Arc::new(locale.finish()),
        Arc::new(title.finish()),
        Arc::new(entry_asset_id.finish()),
        Arc::new(entry_asset_path.finish()),
        Arc::new(content_sha256.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build interactive_page_locales batch")
}

fn build_interactive_assets_batch(records: &[InteractiveAssetRecord]) -> Result<RecordBatch> {
    let schema = interactive_assets_schema();
    let mut id = StringBuilder::new();
    let mut page_id = StringBuilder::new();
    let mut logical_path = StringBuilder::new();
    let mut resolved_url = StringBuilder::new();
    let mut kind = StringBuilder::new();
    let mut mime_type = StringBuilder::new();
    let mut content_sha256 = StringBuilder::new();
    let mut size_bytes = UInt64Builder::new();
    let mut is_entry = arrow_array::builder::BooleanBuilder::new();
    let mut http_status = Int32Builder::new();
    let mut etag = StringBuilder::new();
    let mut last_modified = StringBuilder::new();
    let mut bytes = BlobArrayBuilder::new(records.len());
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    for record in records {
        id.append_value(&record.id);
        page_id.append_value(&record.page_id);
        logical_path.append_value(&record.logical_path);
        resolved_url.append_value(&record.resolved_url);
        kind.append_value(&record.kind);
        mime_type.append_value(&record.mime_type);
        content_sha256.append_value(&record.content_sha256);
        size_bytes.append_value(record.size_bytes);
        is_entry.append_value(record.is_entry);
        http_status.append_value(record.http_status);
        append_optional_str(&mut etag, record.etag.as_deref());
        append_optional_str(&mut last_modified, record.last_modified.as_deref());
        bytes.push_bytes(&record.bytes)?;
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()) as ArrayRef,
        Arc::new(page_id.finish()),
        Arc::new(logical_path.finish()),
        Arc::new(resolved_url.finish()),
        Arc::new(kind.finish()),
        Arc::new(mime_type.finish()),
        Arc::new(content_sha256.finish()),
        Arc::new(size_bytes.finish()),
        Arc::new(is_entry.finish()),
        Arc::new(http_status.finish()),
        Arc::new(etag.finish()),
        Arc::new(last_modified.finish()),
        bytes.finish()?,
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build interactive_assets batch")
}

fn batches_to_interactive_pages(batches: &[RecordBatch]) -> Result<Vec<InteractivePageRecord>> {
    let mut rows = Vec::new();
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let article_id = required_str_col(batch, "article_id")?;
        let source_url = required_str_col(batch, "source_url")?;
        let source_host = required_str_col(batch, "source_host")?;
        let source_lang = required_str_col(batch, "source_lang")?;
        let title = required_str_col(batch, "title")?;
        let status = required_str_col(batch, "status")?;
        let mirror_policy = required_str_col(batch, "mirror_policy")?;
        let translation_scope = required_str_col(batch, "translation_scope")?;
        let entry_asset_id = required_str_col(batch, "entry_asset_id")?;
        let entry_asset_path = required_str_col(batch, "entry_asset_path")?;
        let asset_count = required_u64_col(batch, "asset_count")?;
        let content_sha256 = required_str_col(batch, "content_sha256")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;

        for i in 0..batch.num_rows() {
            rows.push(InteractivePageRecord {
                id: id.value(i).to_string(),
                article_id: article_id.value(i).to_string(),
                source_url: source_url.value(i).to_string(),
                source_host: source_host.value(i).to_string(),
                source_lang: source_lang.value(i).to_string(),
                title: title.value(i).to_string(),
                status: status.value(i).to_string(),
                mirror_policy: mirror_policy.value(i).to_string(),
                translation_scope: translation_scope.value(i).to_string(),
                entry_asset_id: entry_asset_id.value(i).to_string(),
                entry_asset_path: entry_asset_path.value(i).to_string(),
                asset_count: asset_count.value(i),
                content_sha256: content_sha256.value(i).to_string(),
                created_at: created_at.value(i),
                updated_at: updated_at.value(i),
            });
        }
    }
    Ok(rows)
}

fn batches_to_interactive_page_locales(
    batches: &[RecordBatch],
) -> Result<Vec<InteractivePageLocaleRecord>> {
    let mut rows = Vec::new();
    for batch in batches {
        let id = required_str_col(batch, "id")?;
        let page_id = required_str_col(batch, "page_id")?;
        let locale = required_str_col(batch, "locale")?;
        let title = required_str_col(batch, "title")?;
        let entry_asset_id = required_str_col(batch, "entry_asset_id")?;
        let entry_asset_path = required_str_col(batch, "entry_asset_path")?;
        let content_sha256 = required_str_col(batch, "content_sha256")?;
        let created_at = required_ts_col(batch, "created_at")?;
        let updated_at = required_ts_col(batch, "updated_at")?;

        for i in 0..batch.num_rows() {
            rows.push(InteractivePageLocaleRecord {
                id: id.value(i).to_string(),
                page_id: page_id.value(i).to_string(),
                locale: locale.value(i).to_string(),
                title: title.value(i).to_string(),
                entry_asset_id: entry_asset_id.value(i).to_string(),
                entry_asset_path: entry_asset_path.value(i).to_string(),
                content_sha256: content_sha256.value(i).to_string(),
                created_at: created_at.value(i),
                updated_at: updated_at.value(i),
            });
        }
    }
    Ok(rows)
}

fn batches_to_interactive_assets_meta(
    batches: &[RecordBatch],
) -> Result<Vec<InteractiveAssetMeta>> {
    let mut rows = Vec::new();
    for batch in batches {
        for i in 0..batch.num_rows() {
            rows.push(batch_to_interactive_asset_meta(batch, i)?);
        }
    }
    Ok(rows)
}

fn batch_to_interactive_asset_meta(
    batch: &RecordBatch,
    idx: usize,
) -> Result<InteractiveAssetMeta> {
    let id = required_str_col(batch, "id")?;
    let page_id = required_str_col(batch, "page_id")?;
    let logical_path = required_str_col(batch, "logical_path")?;
    let resolved_url = required_str_col(batch, "resolved_url")?;
    let kind = required_str_col(batch, "kind")?;
    let mime_type = required_str_col(batch, "mime_type")?;
    let content_sha256 = required_str_col(batch, "content_sha256")?;
    let size_bytes = required_u64_col(batch, "size_bytes")?;
    let is_entry = required_bool_col(batch, "is_entry")?;
    let http_status = required_i32_col(batch, "http_status")?;
    let etag = optional_str_col(batch, "etag")?;
    let last_modified = optional_str_col(batch, "last_modified")?;
    let created_at = required_ts_col(batch, "created_at")?;
    let updated_at = required_ts_col(batch, "updated_at")?;

    Ok(InteractiveAssetMeta {
        id: id.value(idx).to_string(),
        page_id: page_id.value(idx).to_string(),
        logical_path: logical_path.value(idx).to_string(),
        resolved_url: resolved_url.value(idx).to_string(),
        kind: kind.value(idx).to_string(),
        mime_type: mime_type.value(idx).to_string(),
        content_sha256: content_sha256.value(idx).to_string(),
        size_bytes: size_bytes.value(idx),
        is_entry: is_entry.value(idx),
        http_status: http_status.value(idx),
        etag: value_string_opt(etag, idx),
        last_modified: value_string_opt(last_modified, idx),
        created_at: created_at.value(idx),
        updated_at: updated_at.value(idx),
    })
}

fn required_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .with_context(|| format!("column `{name}` is not StringArray"))
}

fn optional_str_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a StringArray> {
    required_str_col(batch, name)
}

fn required_u64_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a UInt64Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
        .with_context(|| format!("column `{name}` is not UInt64Array"))
}

fn required_i32_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a Int32Array> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<Int32Array>())
        .with_context(|| format!("column `{name}` is not Int32Array"))
}

fn required_bool_col<'a>(batch: &'a RecordBatch, name: &str) -> Result<&'a BooleanArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<BooleanArray>())
        .with_context(|| format!("column `{name}` is not BooleanArray"))
}

fn required_ts_col<'a>(
    batch: &'a RecordBatch,
    name: &str,
) -> Result<&'a TimestampMillisecondArray> {
    batch
        .column_by_name(name)
        .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
        .with_context(|| format!("column `{name}` is not TimestampMillisecondArray"))
}

fn value_string_opt(array: &StringArray, idx: usize) -> Option<String> {
    if array.is_null(idx) {
        None
    } else {
        let value = array.value(idx).trim();
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    }
}

fn append_optional_str(builder: &mut StringBuilder, value: Option<&str>) {
    match value {
        Some(value) if !value.trim().is_empty() => builder.append_value(value),
        _ => builder.append_null(),
    }
}

fn dedicated_blob_field(name: &str, nullable: bool) -> Field {
    let field = blob_field(name, nullable);
    let mut metadata: HashMap<String, String> = field.metadata().clone();
    metadata.insert("lance-encoding:blob-dedicated-size-threshold".to_string(), "1".to_string());
    field.with_metadata(metadata)
}

fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}

pub fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}
