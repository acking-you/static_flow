use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Context, Result};
use arrow_array::{
    builder::{
        FixedSizeListBuilder, Float32Builder, StringBuilder, TimestampMillisecondBuilder,
        UInt64Builder,
    },
    Array, BinaryArray, FixedSizeListArray, LargeBinaryArray, RecordBatch, RecordBatchIterator,
    RecordBatchReader, StringArray, TimestampMillisecondArray, UInt64Array,
};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use chrono::Utc;
use futures::TryStreamExt;
use lance::{blob_field, BlobArrayBuilder};
use lancedb::{
    connect,
    index::{scalar::BTreeIndexBuilder, Index},
    query::{ExecutableQuery, QueryBase, Select},
    table::NewColumnTransform,
    Connection, Table,
};
use serde::{Deserialize, Serialize};

use crate::embedding::text::{
    detect_language, embed_text_with_language, TextEmbeddingLanguage, TEXT_VECTOR_DIM_EN,
    TEXT_VECTOR_DIM_ZH,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const SONGS_TABLE: &str = "songs";
const MUSIC_PLAYS_TABLE: &str = "music_plays";
const MUSIC_COMMENTS_TABLE: &str = "music_comments";

pub const MUSIC_TABLE_NAMES: &[&str] = &["songs", "music_plays", "music_comments"];

// ---------------------------------------------------------------------------
// Record structs (DB rows)
// ---------------------------------------------------------------------------

pub struct SongRecord {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: Option<String>,
    pub cover_image: Option<String>,
    pub duration_ms: u64,
    pub format: String,
    pub bitrate: u64,
    pub lyrics_lrc: Option<String>,
    pub lyrics_translation: Option<String>,
    pub audio_data: Vec<u8>,
    pub source: String,
    pub source_id: Option<String>,
    pub tags: String,
    pub searchable_text: String,
    pub vector_en: Option<Vec<f32>>,
    pub vector_zh: Option<Vec<f32>>,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct MusicPlayRecord {
    pub id: String,
    pub song_id: String,
    pub played_at: i64,
    pub day_bucket: String,
    pub client_fingerprint: String,
    pub created_at: i64,
    pub updated_at: i64,
}

pub struct MusicCommentRecord {
    pub id: String,
    pub song_id: String,
    pub nickname: String,
    pub comment_text: String,
    pub client_fingerprint: String,
    pub client_ip: Option<String>,
    pub ip_region: Option<String>,
    pub created_at: i64,
}

// ---------------------------------------------------------------------------
// Shared response types (Serialize + Deserialize for frontend/backend)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongListItem {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub duration_ms: u64,
    pub format: String,
    pub tags: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongDetail {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub duration_ms: u64,
    pub format: String,
    pub bitrate: u64,
    pub tags: String,
    pub source: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongLyrics {
    pub song_id: String,
    pub lyrics_lrc: Option<String>,
    pub lyrics_translation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongSearchResult {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub cover_image: Option<String>,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtistInfo {
    pub name: String,
    pub song_count: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlbumInfo {
    pub name: String,
    pub artist: String,
    pub song_count: usize,
    pub cover_image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayTrackResponse {
    pub song_id: String,
    pub counted: bool,
    pub total_plays: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicCommentItem {
    pub id: String,
    pub song_id: String,
    pub nickname: String,
    pub comment_text: String,
    pub ip_region: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongListResponse {
    pub songs: Vec<SongListItem>,
    pub total: usize,
    pub offset: usize,
    pub limit: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MusicCommentListResponse {
    pub comments: Vec<MusicCommentItem>,
    pub total: usize,
    pub song_id: String,
}

// ---------------------------------------------------------------------------
// Arrow schemas
// ---------------------------------------------------------------------------

fn songs_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("artist", DataType::Utf8, false),
        Field::new("album", DataType::Utf8, false),
        Field::new("album_id", DataType::Utf8, true),
        Field::new("cover_image", DataType::Utf8, true),
        Field::new("duration_ms", DataType::UInt64, false),
        Field::new("format", DataType::Utf8, false),
        Field::new("bitrate", DataType::UInt64, false),
        Field::new("lyrics_lrc", DataType::Utf8, true),
        Field::new("lyrics_translation", DataType::Utf8, true),
        blob_field("audio_data", false),
        Field::new("source", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("searchable_text", DataType::Utf8, false),
        Field::new(
            "vector_en",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_EN as i32,
            ),
            true,
        ),
        Field::new(
            "vector_zh",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_ZH as i32,
            ),
            true,
        ),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

/// Schema for metadata-only updates (all columns except `audio_data`).
/// Used by partial upsert to avoid rewriting the large audio blob.
fn songs_metadata_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("artist", DataType::Utf8, false),
        Field::new("album", DataType::Utf8, false),
        Field::new("album_id", DataType::Utf8, true),
        Field::new("cover_image", DataType::Utf8, true),
        Field::new("duration_ms", DataType::UInt64, false),
        Field::new("format", DataType::Utf8, false),
        Field::new("bitrate", DataType::UInt64, false),
        Field::new("lyrics_lrc", DataType::Utf8, true),
        Field::new("lyrics_translation", DataType::Utf8, true),
        // audio_data intentionally omitted
        Field::new("source", DataType::Utf8, false),
        Field::new("source_id", DataType::Utf8, true),
        Field::new("tags", DataType::Utf8, false),
        Field::new("searchable_text", DataType::Utf8, false),
        Field::new(
            "vector_en",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_EN as i32,
            ),
            true,
        ),
        Field::new(
            "vector_zh",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM_ZH as i32,
            ),
            true,
        ),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn music_plays_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("song_id", DataType::Utf8, false),
        Field::new("played_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("day_bucket", DataType::Utf8, false),
        Field::new("client_fingerprint", DataType::Utf8, false),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
        Field::new("updated_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

fn music_comments_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("song_id", DataType::Utf8, false),
        Field::new("nickname", DataType::Utf8, false),
        Field::new("comment_text", DataType::Utf8, false),
        Field::new("client_fingerprint", DataType::Utf8, false),
        Field::new("client_ip", DataType::Utf8, true),
        Field::new("ip_region", DataType::Utf8, true),
        Field::new("created_at", DataType::Timestamp(TimeUnit::Millisecond, None), false),
    ]))
}

// ---------------------------------------------------------------------------
// Table helpers (reuse comments_store pattern)
// ---------------------------------------------------------------------------

async fn ensure_table(
    db: &Connection,
    table_name: &str,
    schema: Arc<Schema>,
    storage_options: &[(&str, &str)],
) -> Result<Table> {
    match db.open_table(table_name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            tracing::info!(
                table = table_name,
                storage_options = ?storage_options,
                "Table not found, creating with schema"
            );
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema.clone());
            let mut builder = db.create_table(table_name, Box::new(batches) as Box<dyn RecordBatchReader + Send>);
            for &(k, v) in storage_options {
                builder = builder.storage_option(k, v);
            }
            builder
                .execute()
                .await
                .with_context(|| format!("failed to create table {table_name}"))?;
            tracing::info!(table = table_name, "Table created successfully");
            db.open_table(table_name)
                .execute()
                .await
                .with_context(|| format!("failed to open table {table_name}"))
        },
    }
}

fn escape_literal(value: &str) -> String {
    value.replace('\'', "''")
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn append_optional_str(builder: &mut StringBuilder, value: &Option<String>) {
    match value {
        Some(v) => builder.append_value(v),
        None => builder.append_null(),
    }
}

// ---------------------------------------------------------------------------
// Batch builders
// ---------------------------------------------------------------------------

fn build_song_batch(record: &SongRecord) -> Result<RecordBatch> {
    let schema = songs_schema();
    let mut id = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut artist = StringBuilder::new();
    let mut album = StringBuilder::new();
    let mut album_id = StringBuilder::new();
    let mut cover_image = StringBuilder::new();
    let mut duration_ms = UInt64Builder::new();
    let mut format = StringBuilder::new();
    let mut bitrate = UInt64Builder::new();
    let mut lyrics_lrc = StringBuilder::new();
    let mut lyrics_translation = StringBuilder::new();
    let mut audio_data = BlobArrayBuilder::new(1);
    let mut source = StringBuilder::new();
    let mut source_id = StringBuilder::new();
    let mut tags = StringBuilder::new();
    let mut searchable_text = StringBuilder::new();
    let mut vector_en_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_EN as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut vector_zh_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_ZH as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    id.append_value(&record.id);
    title.append_value(&record.title);
    artist.append_value(&record.artist);
    album.append_value(&record.album);
    append_optional_str(&mut album_id, &record.album_id);
    append_optional_str(&mut cover_image, &record.cover_image);
    duration_ms.append_value(record.duration_ms);
    format.append_value(&record.format);
    bitrate.append_value(record.bitrate);
    append_optional_str(&mut lyrics_lrc, &record.lyrics_lrc);
    append_optional_str(&mut lyrics_translation, &record.lyrics_translation);
    audio_data.push_bytes(&record.audio_data)?;
    source.append_value(&record.source);
    append_optional_str(&mut source_id, &record.source_id);
    tags.append_value(&record.tags);
    searchable_text.append_value(&record.searchable_text);

    // vector_en
    match &record.vector_en {
        Some(v) if v.len() == TEXT_VECTOR_DIM_EN => {
            for val in v {
                vector_en_builder.values().append_value(*val);
            }
            vector_en_builder.append(true);
        },
        _ => {
            for _ in 0..TEXT_VECTOR_DIM_EN {
                vector_en_builder.values().append_value(0.0);
            }
            vector_en_builder.append(false);
        },
    }

    // vector_zh
    match &record.vector_zh {
        Some(v) if v.len() == TEXT_VECTOR_DIM_ZH => {
            for val in v {
                vector_zh_builder.values().append_value(*val);
            }
            vector_zh_builder.append(true);
        },
        _ => {
            for _ in 0..TEXT_VECTOR_DIM_ZH {
                vector_zh_builder.values().append_value(0.0);
            }
            vector_zh_builder.append(false);
        },
    }

    created_at.append_value(record.created_at);
    updated_at.append_value(record.updated_at);

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()),
        Arc::new(title.finish()),
        Arc::new(artist.finish()),
        Arc::new(album.finish()),
        Arc::new(album_id.finish()),
        Arc::new(cover_image.finish()),
        Arc::new(duration_ms.finish()),
        Arc::new(format.finish()),
        Arc::new(bitrate.finish()),
        Arc::new(lyrics_lrc.finish()),
        Arc::new(lyrics_translation.finish()),
        audio_data.finish()?,
        Arc::new(source.finish()),
        Arc::new(source_id.finish()),
        Arc::new(tags.finish()),
        Arc::new(searchable_text.finish()),
        Arc::new(vector_en_builder.finish()),
        Arc::new(vector_zh_builder.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build song batch")
}

/// Build a partial batch with all columns except `audio_data`.
/// Used for metadata-only updates so the large audio blob is not rewritten.
fn build_song_metadata_batch(record: &SongRecord) -> Result<RecordBatch> {
    let schema = songs_metadata_schema();
    let mut id = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut artist = StringBuilder::new();
    let mut album = StringBuilder::new();
    let mut album_id = StringBuilder::new();
    let mut cover_image = StringBuilder::new();
    let mut duration_ms = UInt64Builder::new();
    let mut format = StringBuilder::new();
    let mut bitrate = UInt64Builder::new();
    let mut lyrics_lrc = StringBuilder::new();
    let mut lyrics_translation = StringBuilder::new();
    let mut source = StringBuilder::new();
    let mut source_id = StringBuilder::new();
    let mut tags = StringBuilder::new();
    let mut searchable_text = StringBuilder::new();
    let mut vector_en_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_EN as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut vector_zh_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_ZH as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    id.append_value(&record.id);
    title.append_value(&record.title);
    artist.append_value(&record.artist);
    album.append_value(&record.album);
    append_optional_str(&mut album_id, &record.album_id);
    append_optional_str(&mut cover_image, &record.cover_image);
    duration_ms.append_value(record.duration_ms);
    format.append_value(&record.format);
    bitrate.append_value(record.bitrate);
    append_optional_str(&mut lyrics_lrc, &record.lyrics_lrc);
    append_optional_str(&mut lyrics_translation, &record.lyrics_translation);
    source.append_value(&record.source);
    append_optional_str(&mut source_id, &record.source_id);
    tags.append_value(&record.tags);
    searchable_text.append_value(&record.searchable_text);

    // vector_en
    match &record.vector_en {
        Some(v) if v.len() == TEXT_VECTOR_DIM_EN => {
            for val in v {
                vector_en_builder.values().append_value(*val);
            }
            vector_en_builder.append(true);
        },
        _ => {
            for _ in 0..TEXT_VECTOR_DIM_EN {
                vector_en_builder.values().append_value(0.0);
            }
            vector_en_builder.append(false);
        },
    }

    // vector_zh
    match &record.vector_zh {
        Some(v) if v.len() == TEXT_VECTOR_DIM_ZH => {
            for val in v {
                vector_zh_builder.values().append_value(*val);
            }
            vector_zh_builder.append(true);
        },
        _ => {
            for _ in 0..TEXT_VECTOR_DIM_ZH {
                vector_zh_builder.values().append_value(0.0);
            }
            vector_zh_builder.append(false);
        },
    }

    created_at.append_value(record.created_at);
    updated_at.append_value(record.updated_at);

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()),
        Arc::new(title.finish()),
        Arc::new(artist.finish()),
        Arc::new(album.finish()),
        Arc::new(album_id.finish()),
        Arc::new(cover_image.finish()),
        Arc::new(duration_ms.finish()),
        Arc::new(format.finish()),
        Arc::new(bitrate.finish()),
        Arc::new(lyrics_lrc.finish()),
        Arc::new(lyrics_translation.finish()),
        Arc::new(source.finish()),
        Arc::new(source_id.finish()),
        Arc::new(tags.finish()),
        Arc::new(searchable_text.finish()),
        Arc::new(vector_en_builder.finish()),
        Arc::new(vector_zh_builder.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build song metadata batch")
}

fn build_music_play_batch(record: &MusicPlayRecord) -> Result<RecordBatch> {
    let schema = music_plays_schema();
    let mut id = StringBuilder::new();
    let mut song_id = StringBuilder::new();
    let mut played_at = TimestampMillisecondBuilder::new();
    let mut day_bucket = StringBuilder::new();
    let mut client_fingerprint = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();
    let mut updated_at = TimestampMillisecondBuilder::new();

    id.append_value(&record.id);
    song_id.append_value(&record.song_id);
    played_at.append_value(record.played_at);
    day_bucket.append_value(&record.day_bucket);
    client_fingerprint.append_value(&record.client_fingerprint);
    created_at.append_value(record.created_at);
    updated_at.append_value(record.updated_at);

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()),
        Arc::new(song_id.finish()),
        Arc::new(played_at.finish()),
        Arc::new(day_bucket.finish()),
        Arc::new(client_fingerprint.finish()),
        Arc::new(created_at.finish()),
        Arc::new(updated_at.finish()),
    ])
    .context("failed to build music play batch")
}

fn build_music_comment_batch(record: &MusicCommentRecord) -> Result<RecordBatch> {
    let schema = music_comments_schema();
    let mut id = StringBuilder::new();
    let mut song_id = StringBuilder::new();
    let mut nickname = StringBuilder::new();
    let mut comment_text = StringBuilder::new();
    let mut client_fingerprint = StringBuilder::new();
    let mut client_ip = StringBuilder::new();
    let mut ip_region = StringBuilder::new();
    let mut created_at = TimestampMillisecondBuilder::new();

    id.append_value(&record.id);
    song_id.append_value(&record.song_id);
    nickname.append_value(&record.nickname);
    comment_text.append_value(&record.comment_text);
    client_fingerprint.append_value(&record.client_fingerprint);
    append_optional_str(&mut client_ip, &record.client_ip);
    append_optional_str(&mut ip_region, &record.ip_region);
    created_at.append_value(record.created_at);

    RecordBatch::try_new(schema, vec![
        Arc::new(id.finish()),
        Arc::new(song_id.finish()),
        Arc::new(nickname.finish()),
        Arc::new(comment_text.finish()),
        Arc::new(client_fingerprint.finish()),
        Arc::new(client_ip.finish()),
        Arc::new(ip_region.finish()),
        Arc::new(created_at.finish()),
    ])
    .context("failed to build music comment batch")
}

// ---------------------------------------------------------------------------
// Row extraction helpers
// ---------------------------------------------------------------------------

fn extract_string(batch: &RecordBatch, col: &str, row: usize) -> String {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .and_then(|a| if a.is_null(row) { None } else { Some(a.value(row).to_string()) })
        .unwrap_or_default()
}

fn extract_optional_string(batch: &RecordBatch, col: &str, row: usize) -> Option<String> {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<StringArray>())
        .and_then(|a| if a.is_null(row) { None } else { Some(a.value(row).to_string()) })
}

fn extract_u64(batch: &RecordBatch, col: &str, row: usize) -> u64 {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
        .map(|a| a.value(row))
        .unwrap_or(0)
}

fn extract_ts_ms(batch: &RecordBatch, col: &str, row: usize) -> i64 {
    batch
        .column_by_name(col)
        .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
        .map(|a| a.value(row))
        .unwrap_or(0)
}

fn row_to_song_list_item(batch: &RecordBatch, row: usize) -> SongListItem {
    SongListItem {
        id: extract_string(batch, "id", row),
        title: extract_string(batch, "title", row),
        artist: extract_string(batch, "artist", row),
        album: extract_string(batch, "album", row),
        cover_image: extract_optional_string(batch, "cover_image", row),
        duration_ms: extract_u64(batch, "duration_ms", row),
        format: extract_string(batch, "format", row),
        tags: extract_string(batch, "tags", row),
    }
}

fn row_to_song_detail(batch: &RecordBatch, row: usize) -> SongDetail {
    SongDetail {
        id: extract_string(batch, "id", row),
        title: extract_string(batch, "title", row),
        artist: extract_string(batch, "artist", row),
        album: extract_string(batch, "album", row),
        cover_image: extract_optional_string(batch, "cover_image", row),
        duration_ms: extract_u64(batch, "duration_ms", row),
        format: extract_string(batch, "format", row),
        bitrate: extract_u64(batch, "bitrate", row),
        tags: extract_string(batch, "tags", row),
        source: extract_string(batch, "source", row),
        created_at: extract_ts_ms(batch, "created_at", row),
    }
}

fn row_to_comment_item(batch: &RecordBatch, row: usize) -> MusicCommentItem {
    MusicCommentItem {
        id: extract_string(batch, "id", row),
        song_id: extract_string(batch, "song_id", row),
        nickname: extract_string(batch, "nickname", row),
        comment_text: extract_string(batch, "comment_text", row),
        ip_region: extract_optional_string(batch, "ip_region", row),
        created_at: extract_ts_ms(batch, "created_at", row),
    }
}

// ---------------------------------------------------------------------------
// MusicDataStore
// ---------------------------------------------------------------------------

pub struct MusicDataStore {
    db: Connection,
}

impl MusicDataStore {
    pub fn connection(&self) -> &Connection {
        &self.db
    }

    pub async fn connect(db_uri: &str) -> Result<Self> {
        let db = connect(db_uri)
            .execute()
            .await
            .context("failed to connect music LanceDB")?;
        Ok(Self {
            db,
        })
    }

    async fn songs_table(&self) -> Result<Table> {
        let table = ensure_table(&self.db, SONGS_TABLE, songs_schema(), &[
            ("new_table_data_storage_version", "2.2"),
            ("new_table_enable_stable_row_ids", "true"),
            ("new_table_enable_v2_manifest_paths", "true"),
        ])
        .await?;

        // Auto-migrate: add missing vector columns to existing tables
        let schema = table.schema().await?;
        let mut missing_fields = Vec::new();
        if schema.field_with_name("vector_en").is_err() {
            missing_fields.push(Field::new(
                "vector_en",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    TEXT_VECTOR_DIM_EN as i32,
                ),
                true,
            ));
        }
        if schema.field_with_name("vector_zh").is_err() {
            missing_fields.push(Field::new(
                "vector_zh",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    TEXT_VECTOR_DIM_ZH as i32,
                ),
                true,
            ));
        }
        if !missing_fields.is_empty() {
            let names: Vec<&str> = missing_fields.iter().map(|f| f.name().as_str()).collect();
            tracing::info!("Auto-migrating songs table: adding vector columns {:?}", names);
            let new_schema = Arc::new(Schema::new(missing_fields));
            table
                .add_columns(NewColumnTransform::AllNulls(new_schema), None)
                .await
                .context("failed to add vector columns to songs table")?;
        }

        // Auto-ensure FTS index on searchable_text for full-text search
        let indices = table.list_indices().await.unwrap_or_default();
        if !indices.iter().any(|idx| idx.columns == ["searchable_text"]) {
            tracing::info!("Creating FTS index on songs.searchable_text...");
            if let Err(err) = table
                .create_index(&["searchable_text"], Index::FTS(Default::default()))
                .execute()
                .await
            {
                tracing::warn!("Failed to auto-create FTS index on songs.searchable_text: {err}");
            } else {
                tracing::info!("FTS index on songs.searchable_text created successfully");
            }
        }
        // Auto-ensure BTree index on id for point lookups
        if !indices.iter().any(|idx| idx.columns == ["id"]) {
            tracing::info!("Creating BTree index on songs.id...");
            if let Err(err) = table
                .create_index(&["id"], Index::BTree(BTreeIndexBuilder::default()))
                .execute()
                .await
            {
                tracing::warn!("Failed to auto-create BTree index on songs.id: {err}");
            } else {
                tracing::info!("BTree index on songs.id created successfully");
            }
        }
        Ok(table)
    }

    async fn plays_table(&self) -> Result<Table> {
        ensure_table(&self.db, MUSIC_PLAYS_TABLE, music_plays_schema(), &[]).await
    }

    async fn comments_table(&self) -> Result<Table> {
        ensure_table(&self.db, MUSIC_COMMENTS_TABLE, music_comments_schema(), &[]).await
    }

    // -- Song CRUD --

    pub async fn upsert_song(&self, record: &SongRecord) -> Result<()> {
        let table = self.songs_table().await?;
        let escaped_id = escape_literal(&record.id);
        let existing_count = table
            .count_rows(Some(format!("id = '{escaped_id}'")))
            .await
            .unwrap_or(0);
        if existing_count == 0 {
            // New ID: write full row including audio_data.
            tracing::info!(
                song_id = %record.id,
                audio_len = record.audio_data.len(),
                "Inserting new song with audio blob v2"
            );
            let batch = build_song_batch(record)?;
            let schema = batch.schema();
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
            table
                .add(Box::new(batches) as Box<dyn RecordBatchReader + Send>)
                .execute()
                .await
                .context("failed to add new song record")?;
            tracing::info!(song_id = %record.id, "New song inserted successfully");
        } else {
            // Existing song: partial update — metadata only, skip audio_data
            // to avoid copy-on-write bloat of the large audio blob.
            tracing::info!(
                song_id = %record.id,
                "Updating existing song metadata (audio_data unchanged)"
            );
            let batch = build_song_metadata_batch(record)?;
            let schema = batch.schema();
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
            let mut merge = table.merge_insert(&["id"]);
            merge.when_matched_update_all(None);
            merge
                .execute(Box::new(batches))
                .await
                .context("failed to upsert song metadata")?;
            tracing::info!(song_id = %record.id, "Song metadata updated successfully");
        }
        Ok(())
    }

    pub async fn song_exists(&self, id: &str) -> Result<bool> {
        let table = self.songs_table().await?;
        let escaped = escape_literal(id);
        let count = table
            .count_rows(Some(format!("id = '{escaped}'")))
            .await
            .context("failed to check song existence")?;
        Ok(count > 0)
    }

    pub async fn get_song(&self, id: &str) -> Result<Option<SongDetail>> {
        let table = self.songs_table().await?;
        let escaped = escape_literal(id);
        let cols = &[
            "id",
            "title",
            "artist",
            "album",
            "cover_image",
            "duration_ms",
            "format",
            "bitrate",
            "tags",
            "source",
            "created_at",
        ];
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(cols))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        for batch in &batch_list {
            if batch.num_rows() > 0 {
                return Ok(Some(row_to_song_detail(batch, 0)));
            }
        }
        Ok(None)
    }

    pub async fn get_song_audio(&self, id: &str) -> Result<Option<(Vec<u8>, String)>> {
        let call_started = Instant::now();
        tracing::info!(song_id = %id, "get_song_audio started");

        let table = self.songs_table().await?;
        let escaped = escape_literal(id);

        let ds_wrapper = table.dataset().context("songs table has no dataset")?;
        let dataset = ds_wrapper.get().await?;

        // Phase 1: scanner finds row_addr + format (BTree index pushdown, no audio_data
        // read)
        let mut scanner = dataset.scan();
        scanner.project(&["format"])?;
        scanner.filter(format!("id = '{escaped}'").as_str())?;
        scanner.limit(Some(1), None)?;
        scanner.with_row_address();
        let stream = scanner.try_into_stream().await?;
        let batch_list: Vec<RecordBatch> = stream.try_collect().await?;

        let (row_addr, fmt) = match batch_list.first() {
            Some(b) if b.num_rows() > 0 => {
                let addr = b
                    .column_by_name("_rowaddr")
                    .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                    .map(|a| a.value(0))
                    .context("missing _rowaddr column")?;
                (addr, extract_string(b, "format", 0))
            },
            _ => {
                tracing::info!(
                    song_id = %id,
                    total_ms = call_started.elapsed().as_millis(),
                    "get_song_audio: song not found"
                );
                return Ok(None);
            },
        };

        tracing::info!(
            song_id = %id,
            row_addr,
            format = %fmt,
            phase1_ms = call_started.elapsed().as_millis(),
            "get_song_audio phase 1 complete: row_addr resolved"
        );

        // Phase 2: take_blobs_by_addresses — direct seek into .blob file, O(1)
        let dataset_arc = Arc::new(dataset.clone());
        let blobs = dataset_arc
            .take_blobs_by_addresses(&[row_addr], "audio_data")
            .await
            .context("take_blobs_by_addresses failed")?;

        match blobs.into_iter().next() {
            Some(blob) => {
                let data = blob.read().await.context("blob read failed")?.to_vec();
                if data.is_empty() {
                    tracing::info!(
                        song_id = %id,
                        total_ms = call_started.elapsed().as_millis(),
                        "get_song_audio: audio data empty"
                    );
                    return Ok(None);
                }
                tracing::info!(
                    song_id = %id,
                    audio_len = data.len(),
                    format = %fmt,
                    total_ms = call_started.elapsed().as_millis(),
                    "get_song_audio completed successfully"
                );
                Ok(Some((data, fmt)))
            },
            None => {
                tracing::info!(
                    song_id = %id,
                    total_ms = call_started.elapsed().as_millis(),
                    "get_song_audio: no blob returned for row_addr"
                );
                Ok(None)
            },
        }
    }

    pub async fn get_song_lyrics(&self, id: &str) -> Result<Option<SongLyrics>> {
        let table = self.songs_table().await?;
        let escaped = escape_literal(id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&["lyrics_lrc", "lyrics_translation"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        for batch in &batch_list {
            if batch.num_rows() > 0 {
                return Ok(Some(SongLyrics {
                    song_id: id.to_string(),
                    lyrics_lrc: extract_optional_string(batch, "lyrics_lrc", 0),
                    lyrics_translation: extract_optional_string(batch, "lyrics_translation", 0),
                }));
            }
        }
        Ok(None)
    }

    pub async fn list_songs(
        &self,
        limit: usize,
        offset: usize,
        artist: Option<&str>,
        album: Option<&str>,
        sort_by: Option<&str>,
    ) -> Result<SongListResponse> {
        let table = self.songs_table().await?;
        let cols = &[
            "id",
            "title",
            "artist",
            "album",
            "cover_image",
            "duration_ms",
            "format",
            "tags",
            "created_at",
        ];

        let mut filters = Vec::new();
        if let Some(a) = artist {
            filters.push(format!("artist = '{}'", escape_literal(a)));
        }
        if let Some(a) = album {
            filters.push(format!("album = '{}'", escape_literal(a)));
        }
        let filter_expr = if filters.is_empty() { None } else { Some(filters.join(" AND ")) };

        let total = table
            .count_rows(filter_expr.clone())
            .await
            .context("failed to count songs")? as usize;

        let effective_limit = limit.clamp(1, 100);
        let mut query = table.query();
        if let Some(f) = &filter_expr {
            query = query.only_if(f.clone());
        }
        let batches = query
            .select(Select::columns(cols))
            .limit(effective_limit + 1)
            .offset(offset)
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut songs = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                songs.push(row_to_song_list_item(batch, row));
            }
        }

        let has_more = songs.len() > effective_limit;
        if has_more {
            songs.truncate(effective_limit);
        }

        // Sort client-side (LanceDB doesn't support ORDER BY directly)
        match sort_by {
            Some("popular") => {}, // would need play counts; skip for now
            Some("random") => {
                use rand::seq::SliceRandom;
                songs.shuffle(&mut rand::thread_rng());
            },
            _ => songs.reverse(), // latest first (default insert order)
        }

        Ok(SongListResponse {
            songs,
            total,
            offset,
            limit: effective_limit,
            has_more,
        })
    }

    pub async fn list_random_recommendations(
        &self,
        limit: usize,
        exclude_ids: &[String],
    ) -> Result<Vec<SongListItem>> {
        let table = self.songs_table().await?;
        let cols =
            &["id", "title", "artist", "album", "cover_image", "duration_ms", "format", "tags"];
        let effective_limit = limit.clamp(1, 50);
        let excluded: HashSet<String> = exclude_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string())
            .collect();

        let batches = table
            .query()
            .select(Select::columns(cols))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut songs = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                let item = row_to_song_list_item(batch, row);
                if excluded.contains(&item.id) {
                    continue;
                }
                songs.push(item);
            }
        }

        use rand::seq::SliceRandom;
        songs.shuffle(&mut rand::thread_rng());
        if songs.len() > effective_limit {
            songs.truncate(effective_limit);
        }
        Ok(songs)
    }

    pub async fn resolve_next_random_song(
        &self,
        exclude_ids: &[String],
    ) -> Result<Option<SongDetail>> {
        let table = self.songs_table().await?;
        let cols = &[
            "id",
            "title",
            "artist",
            "album",
            "cover_image",
            "duration_ms",
            "format",
            "bitrate",
            "tags",
            "source",
            "created_at",
        ];
        let excluded: HashSet<String> = exclude_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string())
            .collect();

        let batches = table
            .query()
            .select(Select::columns(cols))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut songs = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                let item = row_to_song_detail(batch, row);
                if excluded.contains(&item.id) {
                    continue;
                }
                songs.push(item);
            }
        }
        if songs.is_empty() {
            return Ok(None);
        }

        use rand::seq::SliceRandom;
        songs.shuffle(&mut rand::thread_rng());
        Ok(songs.into_iter().next())
    }

    pub async fn resolve_next_semantic_song(
        &self,
        current_song_id: &str,
        exclude_ids: &[String],
        top_k: usize,
    ) -> Result<Option<SongDetail>> {
        let table = self.songs_table().await?;
        let escaped = escape_literal(current_song_id);
        let batches = table
            .query()
            .only_if(format!("id = '{escaped}'"))
            .limit(1)
            .select(Select::columns(&["title", "artist", "album", "tags", "searchable_text"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;

        let mut query_text = String::new();
        for batch in &batch_list {
            if batch.num_rows() == 0 {
                continue;
            }
            let searchable = extract_string(batch, "searchable_text", 0);
            if !searchable.trim().is_empty() {
                query_text = searchable;
            } else {
                query_text = format!(
                    "{} {} {} {}",
                    extract_string(batch, "title", 0),
                    extract_string(batch, "artist", 0),
                    extract_string(batch, "album", 0),
                    extract_string(batch, "tags", 0)
                );
            }
            break;
        }

        if query_text.trim().is_empty() {
            return Ok(None);
        }

        let mut excluded: HashSet<String> = exclude_ids
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
            .map(|id| id.to_string())
            .collect();
        excluded.insert(current_song_id.to_string());

        let effective_top_k = top_k.clamp(1, 20);
        let candidates = self
            .search_songs_hybrid(&query_text, effective_top_k, None, None, None)
            .await?;

        for candidate in candidates {
            if excluded.contains(&candidate.id) {
                continue;
            }
            if let Some(song) = self.get_song(&candidate.id).await? {
                return Ok(Some(song));
            }
        }
        Ok(None)
    }

    pub async fn search_songs_fts(
        &self,
        query_text: &str,
        limit: usize,
    ) -> Result<Vec<SongSearchResult>> {
        let table = self.songs_table().await?;
        let cols = &["id", "title", "artist", "album", "cover_image"];
        let effective_limit = limit.clamp(1, 50);

        let fts_query = lancedb::index::scalar::FullTextSearchQuery::new(query_text.to_string());
        let batches = table
            .query()
            .full_text_search(fts_query)
            .select(Select::columns(cols))
            .limit(effective_limit)
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut results = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                results.push(SongSearchResult {
                    id: extract_string(batch, "id", row),
                    title: extract_string(batch, "title", row),
                    artist: extract_string(batch, "artist", row),
                    album: extract_string(batch, "album", row),
                    cover_image: extract_optional_string(batch, "cover_image", row),
                    score: 1.0,
                });
            }
        }
        Ok(results)
    }

    pub async fn search_songs_semantic(
        &self,
        query_text: &str,
        limit: usize,
        max_distance: Option<f32>,
    ) -> Result<Vec<SongSearchResult>> {
        let table = self.songs_table().await?;
        let cols = &["id", "title", "artist", "album", "cover_image"];
        let effective_limit = limit.clamp(1, 50);

        let lang = detect_language(query_text);
        let (primary_col, fallback_col) = match lang {
            TextEmbeddingLanguage::Chinese => ("vector_zh", "vector_en"),
            TextEmbeddingLanguage::English => ("vector_en", "vector_zh"),
        };

        let vector = embed_text_with_language(query_text, lang);

        let results = self
            .run_vector_search(&table, cols, &vector, primary_col, effective_limit, max_distance)
            .await?;

        if !results.is_empty() {
            return Ok(results);
        }

        // Fallback to the other language column
        let fallback_lang = match lang {
            TextEmbeddingLanguage::Chinese => TextEmbeddingLanguage::English,
            TextEmbeddingLanguage::English => TextEmbeddingLanguage::Chinese,
        };
        let fallback_vector = embed_text_with_language(query_text, fallback_lang);
        self.run_vector_search(
            &table,
            cols,
            &fallback_vector,
            fallback_col,
            effective_limit,
            max_distance,
        )
        .await
    }

    async fn run_vector_search(
        &self,
        table: &Table,
        cols: &[&str],
        vector: &[f32],
        column: &str,
        limit: usize,
        max_distance: Option<f32>,
    ) -> Result<Vec<SongSearchResult>> {
        // Check if the vector column exists in the table schema.
        // Existing tables created before vector support won't have it.
        let schema = table.schema().await?;
        if schema.field_with_name(column).is_err() {
            tracing::debug!("Column {column} not found in songs table, skipping vector search");
            return Ok(vec![]);
        }

        let mut query = table
            .query()
            .nearest_to(vector)?
            .column(column)
            .only_if(format!("{column} IS NOT NULL"))
            .select(Select::columns(cols))
            .limit(limit);

        if let Some(dist) = max_distance {
            query = query.distance_range(None, Some(dist));
        }

        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut results = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                let distance = batch
                    .column_by_name("_distance")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::Float32Array>())
                    .map(|a| a.value(row))
                    .unwrap_or(f32::MAX);
                results.push(SongSearchResult {
                    id: extract_string(batch, "id", row),
                    title: extract_string(batch, "title", row),
                    artist: extract_string(batch, "artist", row),
                    album: extract_string(batch, "album", row),
                    cover_image: extract_optional_string(batch, "cover_image", row),
                    score: 1.0 / (1.0 + distance),
                });
            }
        }
        Ok(results)
    }

    pub async fn search_songs_hybrid(
        &self,
        query_text: &str,
        limit: usize,
        rrf_k: Option<f32>,
        vector_limit: Option<usize>,
        fts_limit: Option<usize>,
    ) -> Result<Vec<SongSearchResult>> {
        let effective_limit = limit.clamp(1, 50);
        let vec_limit = vector_limit.unwrap_or(effective_limit * 2);
        let lex_limit = fts_limit.unwrap_or(effective_limit * 2);

        // Run FTS and vector search in parallel
        let fts_fut = self.search_songs_fts(query_text, lex_limit);
        let vec_fut = self.search_songs_semantic(query_text, vec_limit, None);
        let (fts_res, vec_res) = futures::join!(fts_fut, vec_fut);

        let fts_rows = fts_res.unwrap_or_default();
        let vec_rows = vec_res.unwrap_or_default();

        let k = rrf_k.unwrap_or(60.0);
        let mut fused = fuse_song_rrf(vec_rows, fts_rows, k);
        fused.truncate(effective_limit);
        Ok(fused)
    }

    // -- Related songs (vector similarity) --

    async fn fetch_song_vector(
        &self,
        table: &Table,
        id: &str,
    ) -> Result<Option<(Vec<f32>, &'static str)>> {
        let filter = format!("id = '{}'", escape_literal(id));
        let batches = table
            .query()
            .only_if(filter)
            .limit(1)
            .select(Select::columns(&["vector_en", "vector_zh"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;

        if let Some(vector) = Self::extract_fsl_vector(&batch_list, "vector_en") {
            return Ok(Some((vector, "vector_en")));
        }
        if let Some(vector) = Self::extract_fsl_vector(&batch_list, "vector_zh") {
            return Ok(Some((vector, "vector_zh")));
        }
        Ok(None)
    }

    fn extract_fsl_vector(batches: &[RecordBatch], column: &str) -> Option<Vec<f32>> {
        for batch in batches {
            if batch.num_rows() == 0 {
                continue;
            }
            let arr = batch.schema().index_of(column).ok().and_then(|idx| {
                batch
                    .column(idx)
                    .as_any()
                    .downcast_ref::<FixedSizeListArray>()
            })?;
            if arr.is_null(0) {
                return None;
            }
            let values = arr.value(0);
            let float_arr = values
                .as_any()
                .downcast_ref::<arrow_array::Float32Array>()?;
            return Some(float_arr.values().to_vec());
        }
        None
    }

    pub async fn related_songs(
        &self,
        song_id: &str,
        limit: usize,
    ) -> Result<Vec<SongSearchResult>> {
        let table = self.songs_table().await?;
        let vector_info = self.fetch_song_vector(&table, song_id).await?;
        let (vector, col) = match vector_info {
            Some(v) => v,
            None => return Ok(vec![]),
        };

        let filter = format!("{col} IS NOT NULL AND id != '{}'", escape_literal(song_id));
        let cols = &["id", "title", "artist", "album", "cover_image"];
        let effective_limit = limit.clamp(1, 20);

        let query = table
            .query()
            .nearest_to(vector.as_slice())?
            .column(col)
            .only_if(filter)
            .select(Select::columns(cols))
            .limit(effective_limit);

        let batches = query.execute().await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut results = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                let distance = batch
                    .column_by_name("_distance")
                    .and_then(|c| c.as_any().downcast_ref::<arrow_array::Float32Array>())
                    .map(|a| a.value(row))
                    .unwrap_or(f32::MAX);
                results.push(SongSearchResult {
                    id: extract_string(batch, "id", row),
                    title: extract_string(batch, "title", row),
                    artist: extract_string(batch, "artist", row),
                    album: extract_string(batch, "album", row),
                    cover_image: extract_optional_string(batch, "cover_image", row),
                    score: 1.0 / (1.0 + distance),
                });
            }
        }
        Ok(results)
    }

    // -- Aggregation --

    pub async fn list_artists(&self) -> Result<Vec<ArtistInfo>> {
        let table = self.songs_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&["artist"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        for batch in &batch_list {
            if let Some(col) = batch.column_by_name("artist") {
                if let Some(arr) = col.as_any().downcast_ref::<StringArray>() {
                    for i in 0..arr.len() {
                        if !arr.is_null(i) {
                            *counts.entry(arr.value(i).to_string()).or_default() += 1;
                        }
                    }
                }
            }
        }
        let mut artists: Vec<ArtistInfo> = counts
            .into_iter()
            .map(|(name, song_count)| ArtistInfo {
                name,
                song_count,
            })
            .collect();
        artists.sort_by(|a, b| b.song_count.cmp(&a.song_count));
        Ok(artists)
    }

    pub async fn list_albums(&self) -> Result<Vec<AlbumInfo>> {
        let table = self.songs_table().await?;
        let batches = table
            .query()
            .select(Select::columns(&["album", "artist", "cover_image"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut album_map: HashMap<String, (String, usize, Option<String>)> = HashMap::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                let album = extract_string(batch, "album", row);
                let artist = extract_string(batch, "artist", row);
                let cover = extract_optional_string(batch, "cover_image", row);
                let entry = album_map.entry(album).or_insert_with(|| (artist, 0, cover));
                entry.1 += 1;
            }
        }
        let mut albums: Vec<AlbumInfo> = album_map
            .into_iter()
            .map(|(name, (artist, song_count, cover_image))| AlbumInfo {
                name,
                artist,
                song_count,
                cover_image,
            })
            .collect();
        albums.sort_by(|a, b| b.song_count.cmp(&a.song_count));
        Ok(albums)
    }

    // -- Play tracking --

    pub async fn track_play(
        &self,
        song_id: &str,
        fingerprint: &str,
        dedupe_window_seconds: u64,
    ) -> Result<PlayTrackResponse> {
        let table = self.plays_table().await?;
        let now = now_ms();
        let dedupe_window_ms = (dedupe_window_seconds.max(1) as i64) * 1_000;
        let dedupe_bucket = now / dedupe_window_ms;
        let record_id = format!("{song_id}:{fingerprint}:{dedupe_bucket}");
        let escaped_id = escape_literal(&record_id);
        let escaped_song_id = escape_literal(song_id);

        let counted = table
            .count_rows(Some(format!("id = '{escaped_id}'")))
            .await
            .context("failed to check play dedupe key")?
            == 0;

        let tz = chrono::FixedOffset::east_opt(8 * 3600).unwrap();
        let now_local = Utc::now().with_timezone(&tz);
        let day_bucket = now_local.format("%Y-%m-%d").to_string();

        let record = MusicPlayRecord {
            id: record_id,
            song_id: song_id.to_string(),
            played_at: now,
            day_bucket,
            client_fingerprint: fingerprint.to_string(),
            created_at: now,
            updated_at: now,
        };
        let batch = build_music_play_batch(&record)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches))
            .await
            .context("failed to upsert music play")?;

        let total_plays = table
            .count_rows(Some(format!("song_id = '{escaped_song_id}'")))
            .await
            .context("failed to count total plays")? as u64;

        Ok(PlayTrackResponse {
            song_id: song_id.to_string(),
            counted,
            total_plays,
        })
    }

    // -- Comments --

    pub async fn submit_comment(&self, record: MusicCommentRecord) -> Result<MusicCommentItem> {
        let table = self.comments_table().await?;
        let item = MusicCommentItem {
            id: record.id.clone(),
            song_id: record.song_id.clone(),
            nickname: record.nickname.clone(),
            comment_text: record.comment_text.clone(),
            ip_region: record.ip_region.clone(),
            created_at: record.created_at,
        };
        let batch = build_music_comment_batch(&record)?;
        let schema = batch.schema();
        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(batches))
            .await
            .context("failed to insert music comment")?;
        Ok(item)
    }

    pub async fn list_comments(
        &self,
        song_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<MusicCommentListResponse> {
        let table = self.comments_table().await?;
        let escaped = escape_literal(song_id);
        let filter = format!("song_id = '{escaped}'");

        let total = table
            .count_rows(Some(filter.clone()))
            .await
            .context("failed to count music comments")? as usize;

        let effective_limit = limit.clamp(1, 100);
        let batches = table
            .query()
            .only_if(filter)
            .limit(effective_limit)
            .offset(offset)
            .execute()
            .await?;

        let batch_list = batches.try_collect::<Vec<_>>().await?;
        let mut comments = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                comments.push(row_to_comment_item(batch, row));
            }
        }
        // newest first
        comments.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        Ok(MusicCommentListResponse {
            comments,
            total,
            song_id: song_id.to_string(),
        })
    }

    // -- Vector backfill --

    /// Backfill vector embeddings for all songs that have NULL vector_en.
    /// Returns the number of songs updated.
    pub async fn backfill_song_vectors(&self) -> Result<usize> {
        let table = self.songs_table().await?;

        // Read songs missing vectors
        let batches = table
            .query()
            .only_if("vector_en IS NULL")
            .select(Select::columns(&["id", "searchable_text"]))
            .execute()
            .await?;
        let batch_list = batches.try_collect::<Vec<_>>().await?;

        let mut ids = Vec::new();
        let mut texts = Vec::new();
        for batch in &batch_list {
            for row in 0..batch.num_rows() {
                ids.push(extract_string(batch, "id", row));
                texts.push(extract_string(batch, "searchable_text", row));
            }
        }

        if ids.is_empty() {
            return Ok(0);
        }

        let total = ids.len();
        tracing::info!("Backfilling vectors for {total} songs...");

        // Build partial batch: id + vector_en + vector_zh
        let partial_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new(
                "vector_en",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    TEXT_VECTOR_DIM_EN as i32,
                ),
                true,
            ),
            Field::new(
                "vector_zh",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, false)),
                    TEXT_VECTOR_DIM_ZH as i32,
                ),
                true,
            ),
        ]));

        let mut id_builder = StringBuilder::new();
        let mut vec_en_builder =
            FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_EN as i32)
                .with_field(Field::new("item", DataType::Float32, false));
        let mut vec_zh_builder =
            FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_ZH as i32)
                .with_field(Field::new("item", DataType::Float32, false));

        for (i, text) in texts.iter().enumerate() {
            id_builder.append_value(&ids[i]);

            let lang = detect_language(text);
            let primary_vector = embed_text_with_language(text, lang);

            match lang {
                TextEmbeddingLanguage::Chinese => {
                    let en_vector = embed_text_with_language(text, TextEmbeddingLanguage::English);
                    let en_vals = vec_en_builder.values();
                    for v in &en_vector {
                        en_vals.append_value(*v);
                    }
                    vec_en_builder.append(true);

                    let zh_vals = vec_zh_builder.values();
                    for v in &primary_vector {
                        zh_vals.append_value(*v);
                    }
                    vec_zh_builder.append(true);
                },
                TextEmbeddingLanguage::English => {
                    let en_vals = vec_en_builder.values();
                    for v in &primary_vector {
                        en_vals.append_value(*v);
                    }
                    vec_en_builder.append(true);

                    // NULL zh vector: fill zeros + append(false)
                    let zh_vals = vec_zh_builder.values();
                    for _ in 0..TEXT_VECTOR_DIM_ZH {
                        zh_vals.append_value(0.0);
                    }
                    vec_zh_builder.append(false);
                },
            }

            if (i + 1) % 10 == 0 || i + 1 == total {
                tracing::info!("  embedded {}/{total}", i + 1);
            }
        }

        let batch = RecordBatch::try_new(partial_schema.clone(), vec![
            Arc::new(id_builder.finish()),
            Arc::new(vec_en_builder.finish()),
            Arc::new(vec_zh_builder.finish()),
        ])
        .context("failed to build vector backfill batch")?;

        let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), partial_schema);
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge
            .execute(Box::new(batches))
            .await
            .context("failed to merge vector backfill batch")?;

        tracing::info!("Backfilled vectors for {total} songs");
        Ok(total)
    }

    /// Rebuild the songs table with the current schema (LargeBinary blob
    /// encoding for `audio_data`). Reads all rows in batches, drops the old
    /// table, and re-creates it with the new schema. This also eliminates
    /// fragment bloat.
    ///
    /// **Must be called while the backend is stopped.**
    ///
    /// Strategy: read batches from old table → write to a temp DB dir →
    /// atomic filesystem rename to swap in the new `songs.lance`, keeping
    /// the old one as `.bak`.
    pub async fn rebuild_songs_table(&self, batch_size: usize, db_uri: &str) -> Result<usize> {
        use std::path::Path;

        let table = self.songs_table().await?;
        let total = table.count_rows(None).await? as usize;
        if total == 0 {
            tracing::warn!("songs table is empty, nothing to rebuild");
            return Ok(0);
        }
        tracing::info!("Rebuilding songs table: {total} rows, batch_size={batch_size}");

        // --- paths ---
        let db_path = Path::new(db_uri);
        let tmp_db_uri = format!("{}-rebuild", db_uri);
        let tmp_db_path = Path::new(&tmp_db_uri);
        if tmp_db_path.exists() {
            tracing::info!("Cleaning stale rebuild tmp dir: {}", tmp_db_path.display());
            std::fs::remove_dir_all(tmp_db_path)
                .context("failed to clean stale rebuild tmp dir")?;
        }

        // --- connect temp DB ---
        tracing::info!("Connecting to temporary rebuild DB: {tmp_db_uri}");
        let tmp_db = connect(&tmp_db_uri)
            .execute()
            .await
            .context("failed to connect rebuild tmp DB")?;

        // Use lance Dataset scanner with AllBinary to read blob data from old table
        tracing::info!("Opening lance Dataset scanner with AllBinary blob handling");
        let ds_wrapper = table.dataset().context("no dataset on songs table")?;
        let dataset = ds_wrapper.get().await?;

        let schema = songs_schema();
        let mut written: usize = 0;
        let mut tmp_table: Option<Table> = None;

        // --- batched read → write loop ---
        let mut offset: usize = 0;
        while offset < total {
            tracing::info!("Reading batch: offset={offset}, batch_size={batch_size}");
            let mut scanner = dataset.scan();
            scanner.limit(Some(batch_size as i64), Some(offset as i64))?;
            scanner.blob_handling(lance::datatypes::BlobHandling::AllBinary);
            let stream = scanner.try_into_stream().await?;
            let batches: Vec<RecordBatch> = stream
                .try_collect()
                .await
                .with_context(|| format!("collect songs offset={offset}"))?;

            let mut rows: Vec<RebuildRow> = Vec::new();
            for b in &batches {
                for i in 0..b.num_rows() {
                    rows.push(RebuildRow::from_batch(b, i)?);
                }
            }
            if rows.is_empty() {
                tracing::info!("No more rows at offset={offset}, ending read loop");
                break;
            }
            tracing::info!("Read {} rows from old table at offset={offset}", rows.len());

            let batch = rebuild_rows_to_batch(&rows, &schema)?;
            let s = batch.schema();
            let iter = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), s);

            match &tmp_table {
                None => {
                    tracing::info!(
                        "Creating tmp songs table with blob v2 (data_storage_version=2.2, \
                         stable_row_ids=true)"
                    );
                    let t = tmp_db
                        .create_table("songs", Box::new(iter) as Box<dyn RecordBatchReader + Send>)
                        .storage_option("new_table_data_storage_version", "2.2")
                        .storage_option("new_table_enable_stable_row_ids", "true")
                        .storage_option("new_table_enable_v2_manifest_paths", "true")
                        .execute()
                        .await
                        .context("create tmp songs table")?;
                    tmp_table = Some(t);
                },
                Some(t) => {
                    t.add(Box::new(iter) as Box<dyn RecordBatchReader + Send>)
                        .execute()
                        .await
                        .context("add batch to tmp songs")?;
                },
            }

            written += rows.len();
            offset += rows.len();
            tracing::info!("Written {written}/{total} rows to tmp DB");
        }

        if written == 0 {
            tracing::warn!("No rows written, aborting rebuild");
            let _ = std::fs::remove_dir_all(tmp_db_path);
            return Ok(0);
        }

        // --- rebuild FTS + BTree indices on tmp table ---
        if let Some(t) = &tmp_table {
            tracing::info!("Rebuilding FTS index on tmp table...");
            t.create_index(&["searchable_text"], Index::FTS(Default::default()))
                .replace(true)
                .execute()
                .await
                .context("rebuild FTS on tmp table")?;
            tracing::info!("FTS index on tmp table created successfully");

            tracing::info!("Creating BTree index on id...");
            if let Err(err) = t
                .create_index(&["id"], Index::BTree(BTreeIndexBuilder::default()))
                .execute()
                .await
            {
                tracing::warn!("Failed to create BTree index on tmp table: {err}");
            } else {
                tracing::info!("BTree index on tmp table created successfully");
            }
        }

        // --- filesystem swap (cp+rm for 9p/NTFS compat, rename not supported) ---
        tracing::info!("Starting filesystem swap...");
        let old_lance = db_path.join("songs.lance");
        let bak_lance = db_path.join("songs.lance.bak");
        let new_lance = tmp_db_path.join("songs.lance");

        if !new_lance.exists() {
            anyhow::bail!("tmp songs.lance not found at {}", new_lance.display());
        }

        // remove stale backup if present
        if bak_lance.exists() {
            tracing::info!("Removing stale backup: {}", bak_lance.display());
            std::fs::remove_dir_all(&bak_lance).context("remove stale .bak")?;
        }

        // old → bak (cp + rm for cross-fs compat)
        if old_lance.exists() {
            tracing::info!(
                "Backing up old table: {} → {}",
                old_lance.display(),
                bak_lance.display()
            );
            copy_dir_recursive(&old_lance, &bak_lance).context("copy old songs.lance → .bak")?;
            std::fs::remove_dir_all(&old_lance).context("remove old songs.lance after backup")?;
            tracing::info!("Backed up old table to {}", bak_lance.display());
        }

        // new → final (cp + rm for cross-fs compat)
        tracing::info!("Installing new table: {} → {}", new_lance.display(), old_lance.display());
        copy_dir_recursive(&new_lance, &old_lance).context("copy tmp songs.lance → songs.lance")?;
        tracing::info!("Swapped in new songs.lance");

        // cleanup tmp dir shell (songs.lance already moved out)
        let _ = std::fs::remove_dir_all(tmp_db_path);

        // --- post-rebuild compaction ---
        tracing::info!("Running post-rebuild compaction...");
        let new_db = connect(db_uri)
            .execute()
            .await
            .context("reconnect after rebuild")?;
        let new_table = new_db
            .open_table(SONGS_TABLE)
            .execute()
            .await
            .context("open rebuilt songs table")?;
        match new_table
            .optimize(lancedb::table::OptimizeAction::All)
            .await
        {
            Ok(_stats) => tracing::info!("Post-rebuild compaction done"),
            Err(err) => tracing::warn!("Post-rebuild compaction failed (non-fatal): {err}"),
        }

        tracing::info!(
            "Rebuild complete: {written} songs. Backup at {} — delete manually after verification.",
            bak_lance.display()
        );
        Ok(written)
    }
}

// ---------------------------------------------------------------------------
// RRF fusion for hybrid search
// ---------------------------------------------------------------------------

fn fuse_song_rrf(
    vector_rows: Vec<SongSearchResult>,
    fts_rows: Vec<SongSearchResult>,
    rrf_k: f32,
) -> Vec<SongSearchResult> {
    struct Accum {
        score: f32,
        best_rank: usize,
        row: SongSearchResult,
    }

    let rrf_score = |rank: usize| -> f32 { 1.0 / (rrf_k + rank as f32 + 1.0) };

    let mut fused: HashMap<String, Accum> = HashMap::new();

    for (rank, row) in vector_rows.into_iter().enumerate() {
        let boost = rrf_score(rank);
        let entry = fused.entry(row.id.clone()).or_insert_with(|| Accum {
            score: 0.0,
            best_rank: rank,
            row: row.clone(),
        });
        entry.score += boost;
        if rank < entry.best_rank {
            entry.best_rank = rank;
            entry.row = row;
        }
    }

    for (rank, row) in fts_rows.into_iter().enumerate() {
        let boost = rrf_score(rank);
        let entry = fused.entry(row.id.clone()).or_insert_with(|| Accum {
            score: 0.0,
            best_rank: rank,
            row: row.clone(),
        });
        entry.score += boost;
        if rank < entry.best_rank {
            entry.best_rank = rank;
            entry.row = row;
        }
    }

    let mut results: Vec<_> = fused.into_values().collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.best_rank.cmp(&b.best_rank))
    });

    results
        .into_iter()
        .map(|a| {
            let mut row = a.row;
            row.score = a.score;
            row
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Helpers for rebuild_songs_table
// ---------------------------------------------------------------------------

/// Recursively copy a directory. Used instead of `std::fs::rename` for
/// cross-filesystem compatibility (WSL 9p / NTFS mounts reject rename).
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(dst).with_context(|| format!("create dir {}", dst.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("read dir {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)
                .with_context(|| format!("copy {} → {}", src_path.display(), dst_path.display()))?;
        }
    }
    Ok(())
}

/// Intermediate row representation used during table rebuild.
struct RebuildRow {
    id: String,
    title: String,
    artist: String,
    album: String,
    album_id: Option<String>,
    cover_image: Option<String>,
    duration_ms: u64,
    format: String,
    bitrate: u64,
    lyrics_lrc: Option<String>,
    lyrics_translation: Option<String>,
    audio_data: Vec<u8>,
    source: String,
    source_id: Option<String>,
    tags: String,
    searchable_text: String,
    vector_en: Option<Vec<f32>>,
    vector_zh: Option<Vec<f32>>,
    created_at: i64,
    updated_at: i64,
}

impl RebuildRow {
    fn from_batch(batch: &RecordBatch, i: usize) -> Result<Self> {
        let str_col = |name: &str| -> String {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .map(|a| a.value(i).to_string())
                .unwrap_or_default()
        };
        let opt_str_col = |name: &str| -> Option<String> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<StringArray>())
                .and_then(|a| {
                    if a.is_null(i) {
                        None
                    } else {
                        let v = a.value(i);
                        if v.is_empty() {
                            None
                        } else {
                            Some(v.to_string())
                        }
                    }
                })
        };
        let u64_col = |name: &str| -> u64 {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<UInt64Array>())
                .map(|a| a.value(i))
                .unwrap_or(0)
        };
        let ts_col = |name: &str| -> i64 {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<TimestampMillisecondArray>())
                .map(|a| a.value(i))
                .unwrap_or(0)
        };

        // audio_data: try LargeBinary first, fallback to Binary
        let audio_data = batch
            .column_by_name("audio_data")
            .and_then(|c| {
                c.as_any()
                    .downcast_ref::<LargeBinaryArray>()
                    .map(|a| a.value(i).to_vec())
                    .or_else(|| {
                        c.as_any()
                            .downcast_ref::<BinaryArray>()
                            .map(|a| a.value(i).to_vec())
                    })
            })
            .unwrap_or_default();

        // vector columns
        let vec_col = |name: &str, dim: usize| -> Option<Vec<f32>> {
            batch
                .column_by_name(name)
                .and_then(|c| c.as_any().downcast_ref::<FixedSizeListArray>())
                .and_then(|a| {
                    if a.is_null(i) {
                        return None;
                    }
                    let values = a.value(i);
                    let floats = values
                        .as_any()
                        .downcast_ref::<arrow_array::Float32Array>()?;
                    let v: Vec<f32> = (0..dim).map(|j| floats.value(j)).collect();
                    Some(v)
                })
        };

        Ok(RebuildRow {
            id: str_col("id"),
            title: str_col("title"),
            artist: str_col("artist"),
            album: str_col("album"),
            album_id: opt_str_col("album_id"),
            cover_image: opt_str_col("cover_image"),
            duration_ms: u64_col("duration_ms"),
            format: str_col("format"),
            bitrate: u64_col("bitrate"),
            lyrics_lrc: opt_str_col("lyrics_lrc"),
            lyrics_translation: opt_str_col("lyrics_translation"),
            audio_data,
            source: str_col("source"),
            source_id: opt_str_col("source_id"),
            tags: str_col("tags"),
            searchable_text: str_col("searchable_text"),
            vector_en: vec_col("vector_en", TEXT_VECTOR_DIM_EN),
            vector_zh: vec_col("vector_zh", TEXT_VECTOR_DIM_ZH),
            created_at: ts_col("created_at"),
            updated_at: ts_col("updated_at"),
        })
    }
}

fn rebuild_rows_to_batch(rows: &[RebuildRow], schema: &Arc<Schema>) -> Result<RecordBatch> {
    let mut id = StringBuilder::new();
    let mut title = StringBuilder::new();
    let mut artist = StringBuilder::new();
    let mut album = StringBuilder::new();
    let mut album_id = StringBuilder::new();
    let mut cover_image = StringBuilder::new();
    let mut duration_ms = UInt64Builder::new();
    let mut format = StringBuilder::new();
    let mut bitrate = UInt64Builder::new();
    let mut lyrics_lrc = StringBuilder::new();
    let mut lyrics_translation = StringBuilder::new();
    let mut audio_data = BlobArrayBuilder::new(rows.len());
    let mut source = StringBuilder::new();
    let mut source_id = StringBuilder::new();
    let mut tags = StringBuilder::new();
    let mut searchable_text = StringBuilder::new();
    let mut vector_en_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_EN as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut vector_zh_builder =
        FixedSizeListBuilder::new(Float32Builder::new(), TEXT_VECTOR_DIM_ZH as i32)
            .with_field(Field::new("item", DataType::Float32, false));
    let mut created_at_b = TimestampMillisecondBuilder::new();
    let mut updated_at_b = TimestampMillisecondBuilder::new();

    for r in rows {
        id.append_value(&r.id);
        title.append_value(&r.title);
        artist.append_value(&r.artist);
        album.append_value(&r.album);
        append_optional_str(&mut album_id, &r.album_id);
        append_optional_str(&mut cover_image, &r.cover_image);
        duration_ms.append_value(r.duration_ms);
        format.append_value(&r.format);
        bitrate.append_value(r.bitrate);
        append_optional_str(&mut lyrics_lrc, &r.lyrics_lrc);
        append_optional_str(&mut lyrics_translation, &r.lyrics_translation);
        audio_data.push_bytes(&r.audio_data)?;
        source.append_value(&r.source);
        append_optional_str(&mut source_id, &r.source_id);
        tags.append_value(&r.tags);
        searchable_text.append_value(&r.searchable_text);

        match &r.vector_en {
            Some(v) if v.len() == TEXT_VECTOR_DIM_EN => {
                for val in v {
                    vector_en_builder.values().append_value(*val);
                }
                vector_en_builder.append(true);
            },
            _ => {
                for _ in 0..TEXT_VECTOR_DIM_EN {
                    vector_en_builder.values().append_value(0.0);
                }
                vector_en_builder.append(false);
            },
        }
        match &r.vector_zh {
            Some(v) if v.len() == TEXT_VECTOR_DIM_ZH => {
                for val in v {
                    vector_zh_builder.values().append_value(*val);
                }
                vector_zh_builder.append(true);
            },
            _ => {
                for _ in 0..TEXT_VECTOR_DIM_ZH {
                    vector_zh_builder.values().append_value(0.0);
                }
                vector_zh_builder.append(false);
            },
        }

        created_at_b.append_value(r.created_at);
        updated_at_b.append_value(r.updated_at);
    }

    RecordBatch::try_new(schema.clone(), vec![
        Arc::new(id.finish()),
        Arc::new(title.finish()),
        Arc::new(artist.finish()),
        Arc::new(album.finish()),
        Arc::new(album_id.finish()),
        Arc::new(cover_image.finish()),
        Arc::new(duration_ms.finish()),
        Arc::new(format.finish()),
        Arc::new(bitrate.finish()),
        Arc::new(lyrics_lrc.finish()),
        Arc::new(lyrics_translation.finish()),
        audio_data.finish()?,
        Arc::new(source.finish()),
        Arc::new(source_id.finish()),
        Arc::new(tags.finish()),
        Arc::new(searchable_text.finish()),
        Arc::new(vector_en_builder.finish()),
        Arc::new(vector_zh_builder.finish()),
        Arc::new(created_at_b.finish()),
        Arc::new(updated_at_b.finish()),
    ])
    .context("failed to build rebuild batch")
}
