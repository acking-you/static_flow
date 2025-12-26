use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use arrow::util::pretty::print_batches;
use arrow_array::builder::{
    BinaryBuilder, FixedSizeListBuilder, Int32Builder, ListBuilder, StringBuilder,
    TimestampMillisecondBuilder,
};
use arrow_array::{ArrayRef, RecordBatch, RecordBatchIterator};
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use clap::{Parser, Subcommand};
use futures::TryStreamExt;
use image::{DynamicImage, GenericImageView, ImageFormat};
use lancedb::index::scalar::FullTextSearchQuery;
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::{connect, Connection, Table};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use static_flow_shared::embedding::{embed_text, TEXT_VECTOR_DIM};

const IMAGE_VECTOR_DIM: usize = 512;

#[derive(Parser)]
#[command(name = "sf-cli", version, about = "StaticFlow LanceDB CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize LanceDB schema and indexes.
    Init {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
    },
    /// Write a Markdown article into LanceDB.
    WriteArticle {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Markdown file path.
        #[arg(long)]
        file: PathBuf,
        /// Article summary (AI generated).
        #[arg(long)]
        summary: String,
        /// Comma-separated tags list.
        #[arg(long)]
        tags: String,
        /// Article category.
        #[arg(long)]
        category: String,
        /// Optional embedding vector as JSON array.
        #[arg(long)]
        vector: Option<String>,
    },
    /// Batch write images into LanceDB.
    WriteImages {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Directory to scan for images.
        #[arg(long)]
        dir: PathBuf,
        /// Recursively scan directories.
        #[arg(long)]
        recursive: bool,
        /// Generate thumbnails for images.
        #[arg(long)]
        generate_thumbnail: bool,
        /// Thumbnail size (pixels).
        #[arg(long, default_value_t = 256)]
        thumbnail_size: u32,
    },
    /// Query a table and print the first rows.
    Query {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Table name (articles/images).
        #[arg(long)]
        table: String,
        /// Number of rows to fetch.
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Init { db_path } => init_db(&db_path).await,
        Commands::WriteArticle {
            db_path,
            file,
            summary,
            tags,
            category,
            vector,
        } => write_article(&db_path, &file, summary, tags, category, vector).await,
        Commands::WriteImages {
            db_path,
            dir,
            recursive,
            generate_thumbnail,
            thumbnail_size,
        } => write_images(&db_path, &dir, recursive, generate_thumbnail, thumbnail_size).await,
        Commands::Query {
            db_path,
            table,
            limit,
        } => query_table(&db_path, &table, limit).await,
    }
}

async fn init_db(db_path: &Path) -> Result<()> {
    let db = connect(db_path.to_string_lossy().as_ref()).execute().await?;

    let articles_schema = Arc::new(article_schema());
    let images_schema = Arc::new(image_schema());

    let articles_table = ensure_table(&db, "articles", articles_schema).await?;
    let images_table = ensure_table(&db, "images", images_schema).await?;

    // Vector index for article semantic search.
    if let Err(err) = articles_table
        .create_index(&["vector"], Index::Auto)
        .execute()
        .await
    {
        eprintln!("Failed to create vector index on articles: {err}");
    }

    // FTS index for keyword search.
    if let Err(err) = articles_table
        .create_index(&["content"], Index::FTS(Default::default()))
        .execute()
        .await
    {
        eprintln!("Failed to create FTS index on articles: {err}");
    }

    // Vector index for image search.
    if let Err(err) = images_table
        .create_index(&["vector"], Index::Auto)
        .execute()
        .await
    {
        eprintln!("Failed to create vector index on images: {err}");
    }

    // Sanity query to ensure FTS is ready.
    let _ = articles_table
        .query()
        .full_text_search(FullTextSearchQuery::new("init".to_string()))
        .limit(1)
        .execute()
        .await;

    println!("LanceDB initialized at {}", db_path.display());
    Ok(())
}

async fn write_article(
    db_path: &Path,
    file: &Path,
    summary: String,
    tags: String,
    category: String,
    vector: Option<String>,
) -> Result<()> {
    let db = connect(db_path.to_string_lossy().as_ref()).execute().await?;
    let table = db
        .open_table("articles")
        .execute()
        .await
        .context("articles table not found; run `sf-cli init` first")?;

    let content = fs::read_to_string(file).context("failed to read markdown file")?;
    let (frontmatter, body) = parse_markdown(&content)?;
    if frontmatter.title.trim().is_empty() {
        anyhow::bail!("frontmatter title is required");
    }

    let id = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    let tags = parse_tags(&tags);
    let read_time = frontmatter.read_time.unwrap_or_else(|| estimate_read_time(&body));
    let date = frontmatter
        .date
        .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
    let author = frontmatter.author.unwrap_or_else(|| "Unknown".to_string());

    let embedding = match vector {
        Some(json) => parse_vector(&json, TEXT_VECTOR_DIM)?,
        None => embed_text(&format!("{} {} {}", frontmatter.title, summary, body)),
    };

    let now_ms = chrono::Utc::now().timestamp_millis();
    let record = ArticleRecord {
        id,
        title: frontmatter.title,
        content: body,
        summary,
        tags,
        category,
        author,
        date,
        featured_image: frontmatter.featured_image,
        read_time,
        vector: embedding,
        created_at: now_ms,
        updated_at: now_ms,
    };

    upsert_articles(&table, &[record]).await?;
    println!("Article written to LanceDB.");
    Ok(())
}

async fn write_images(
    db_path: &Path,
    dir: &Path,
    recursive: bool,
    generate_thumbnail: bool,
    thumbnail_size: u32,
) -> Result<()> {
    let db = connect(db_path.to_string_lossy().as_ref()).execute().await?;
    let table = db
        .open_table("images")
        .execute()
        .await
        .context("images table not found; run `sf-cli init` first")?;

    let files = collect_image_files(dir, recursive)?;
    if files.is_empty() {
        println!("No images found in {}", dir.display());
        return Ok(());
    }

    let mut records = Vec::new();
    for path in files {
        let bytes = fs::read(&path)
            .with_context(|| format!("failed to read image {}", path.display()))?;
        let id = hash_bytes(&bytes);
        let filename = relative_filename(dir, &path);

        let mut metadata = serde_json::json!({
            "filename": filename,
            "bytes": bytes.len(),
        });

        let (vector, thumbnail) = match image::load_from_memory(&bytes) {
            Ok(img) => {
                let (w, h) = img.dimensions();
                let format = ImageFormat::from_path(&path).ok();
                metadata["width"] = serde_json::json!(w);
                metadata["height"] = serde_json::json!(h);
                metadata["format"] = serde_json::json!(format.map(|f| format!("{:?}", f)));
                let thumb = if generate_thumbnail {
                    Some(encode_thumbnail(&img, thumbnail_size)?)
                } else {
                    None
                };
                (embed_image(&img), thumb)
            },
            Err(_) => {
                metadata["format"] = serde_json::json!(None::<String>);
                (embed_bytes(&bytes, IMAGE_VECTOR_DIM), None)
            },
        };

        records.push(ImageRecord {
            id,
            filename,
            data: bytes,
            thumbnail,
            vector,
            metadata: metadata.to_string(),
            created_at: chrono::Utc::now().timestamp_millis(),
        });
    }

    for chunk in records.chunks(64) {
        upsert_images(&table, chunk).await?;
    }

    println!("Wrote {} images to LanceDB.", records.len());
    Ok(())
}

async fn query_table(db_path: &Path, table: &str, limit: usize) -> Result<()> {
    let db = connect(db_path.to_string_lossy().as_ref()).execute().await?;
    let table = db.open_table(table).execute().await?;

    let batches = table
        .query()
        .limit(limit)
        .execute()
        .await?
        .try_collect::<Vec<_>>()
        .await?;

    print_batches(&batches)?;
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct Frontmatter {
    title: String,
    summary: Option<String>,
    tags: Option<Vec<String>>,
    category: Option<String>,
    author: Option<String>,
    date: Option<String>,
    featured_image: Option<String>,
    read_time: Option<i32>,
}

fn parse_markdown(content: &str) -> Result<(Frontmatter, String)> {
    let matter = gray_matter::Matter::<gray_matter::engine::YAML>::new();
    let parsed = matter.parse(content);

    let frontmatter = parsed
        .data
        .map(|data| data.deserialize::<Frontmatter>())
        .transpose()?
        .unwrap_or_default();

    Ok((frontmatter, parsed.content))
}

fn parse_tags(tags: &str) -> Vec<String> {
    tags.split(',')
        .map(|tag| tag.trim())
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.to_string())
        .collect()
}

fn estimate_read_time(content: &str) -> i32 {
    let words = content.split_whitespace().count();
    let minutes = (words as f32 / 200.0).ceil() as i32;
    minutes.max(1)
}

fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn relative_filename(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    relative
        .to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string()
}

fn parse_vector(json: &str, dim: usize) -> Result<Vec<f32>> {
    let vector: Vec<f32> = serde_json::from_str(json).context("invalid vector JSON")?;
    if vector.len() != dim {
        anyhow::bail!("vector length {} does not match {}", vector.len(), dim);
    }
    Ok(vector)
}

fn encode_thumbnail(image: &DynamicImage, size: u32) -> Result<Vec<u8>> {
    let thumbnail = image.thumbnail(size, size);
    let mut buffer = std::io::Cursor::new(Vec::new());
    thumbnail.write_to(&mut buffer, ImageFormat::Png)?;
    Ok(buffer.into_inner())
}

fn embed_image(image: &DynamicImage) -> Vec<f32> {
    // 8x8x8 color histogram = 512 dimensions.
    let mut vector = vec![0.0f32; IMAGE_VECTOR_DIM];
    let rgb = image.to_rgb8();

    for pixel in rgb.pixels() {
        let [r, g, b] = pixel.0;
        let r_bin = (r / 32) as usize;
        let g_bin = (g / 32) as usize;
        let b_bin = (b / 32) as usize;
        let idx = r_bin * 64 + g_bin * 8 + b_bin;
        vector[idx] += 1.0;
    }

    normalize_vector(&mut vector);
    vector
}

fn embed_bytes(bytes: &[u8], dim: usize) -> Vec<f32> {
    // Fallback embedding when image decoding fails.
    let mut vector = vec![0.0f32; dim];
    for (idx, byte) in bytes.iter().enumerate() {
        let bucket = (idx * 31 + (*byte as usize)) % dim;
        vector[bucket] += 1.0;
    }
    normalize_vector(&mut vector);
    vector
}

fn normalize_vector(vector: &mut [f32]) {
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vector.iter_mut() {
            *v /= norm;
        }
    }
}

struct ArticleRecord {
    id: String,
    title: String,
    content: String,
    summary: String,
    tags: Vec<String>,
    category: String,
    author: String,
    date: String,
    featured_image: Option<String>,
    read_time: i32,
    vector: Vec<f32>,
    created_at: i64,
    updated_at: i64,
}

struct ImageRecord {
    id: String,
    filename: String,
    data: Vec<u8>,
    thumbnail: Option<Vec<u8>>,
    vector: Vec<f32>,
    metadata: String,
    created_at: i64,
}

fn article_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("title", DataType::Utf8, false),
        Field::new("content", DataType::Utf8, false),
        Field::new("summary", DataType::Utf8, false),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("category", DataType::Utf8, false),
        Field::new("author", DataType::Utf8, false),
        Field::new("date", DataType::Utf8, false),
        Field::new("featured_image", DataType::Utf8, true),
        Field::new("read_time", DataType::Int32, false),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                TEXT_VECTOR_DIM as i32,
            ),
            false,
        ),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
        Field::new(
            "updated_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ])
}

fn image_schema() -> Schema {
    Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("filename", DataType::Utf8, false),
        Field::new("data", DataType::Binary, false),
        Field::new("thumbnail", DataType::Binary, true),
        Field::new(
            "vector",
            DataType::FixedSizeList(
                Arc::new(Field::new("item", DataType::Float32, false)),
                IMAGE_VECTOR_DIM as i32,
            ),
            false,
        ),
        Field::new("metadata", DataType::Utf8, false),
        Field::new(
            "created_at",
            DataType::Timestamp(TimeUnit::Millisecond, None),
            false,
        ),
    ])
}

async fn ensure_table(db: &Connection, name: &str, schema: Arc<Schema>) -> Result<Table> {
    match db.open_table(name).execute().await {
        Ok(table) => Ok(table),
        Err(_) => {
            let batch = RecordBatch::new_empty(schema.clone());
            let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
            db.create_table(name, Box::new(batches))
                .execute()
                .await?;
            Ok(db.open_table(name).execute().await?)
        },
    }
}

async fn upsert_articles(table: &Table, records: &[ArticleRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let batch = build_article_batch(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

async fn upsert_images(table: &Table, records: &[ImageRecord]) -> Result<()> {
    if records.is_empty() {
        return Ok(());
    }
    let batch = build_image_batch(records)?;
    let schema = batch.schema();
    let batches = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);

    let mut merge = table.merge_insert(&["id"]);
    merge.when_matched_update_all(None);
    merge.when_not_matched_insert_all();
    merge.execute(Box::new(batches)).await?;
    Ok(())
}

fn build_article_batch(records: &[ArticleRecord]) -> Result<RecordBatch> {
    let mut id_builder = StringBuilder::new();
    let mut title_builder = StringBuilder::new();
    let mut content_builder = StringBuilder::new();
    let mut summary_builder = StringBuilder::new();
    let mut tags_builder = ListBuilder::new(StringBuilder::new());
    let mut category_builder = StringBuilder::new();
    let mut author_builder = StringBuilder::new();
    let mut date_builder = StringBuilder::new();
    let mut featured_builder = StringBuilder::new();
    let mut read_time_builder = Int32Builder::new();
    let mut vector_builder =
        FixedSizeListBuilder::new(arrow_array::builder::Float32Builder::new(), TEXT_VECTOR_DIM as i32);
    let mut created_at_builder = TimestampMillisecondBuilder::new();
    let mut updated_at_builder = TimestampMillisecondBuilder::new();

    for record in records {
        id_builder.append_value(&record.id);
        title_builder.append_value(&record.title);
        content_builder.append_value(&record.content);
        summary_builder.append_value(&record.summary);

        for tag in &record.tags {
            tags_builder.values().append_value(tag);
        }
        tags_builder.append(true);

        category_builder.append_value(&record.category);
        author_builder.append_value(&record.author);
        date_builder.append_value(&record.date);

        if let Some(featured) = &record.featured_image {
            featured_builder.append_value(featured);
        } else {
            featured_builder.append_null();
        }

        read_time_builder.append_value(record.read_time);

        if record.vector.len() != TEXT_VECTOR_DIM {
            anyhow::bail!(
                "article vector length {} does not match {}",
                record.vector.len(),
                TEXT_VECTOR_DIM
            );
        }
        for value in &record.vector {
            vector_builder.values().append_value(*value);
        }
        vector_builder.append(true);

        created_at_builder.append_value(record.created_at);
        updated_at_builder.append_value(record.updated_at);
    }

    let schema = Arc::new(article_schema());
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(id_builder.finish()),
        Arc::new(title_builder.finish()),
        Arc::new(content_builder.finish()),
        Arc::new(summary_builder.finish()),
        Arc::new(tags_builder.finish()),
        Arc::new(category_builder.finish()),
        Arc::new(author_builder.finish()),
        Arc::new(date_builder.finish()),
        Arc::new(featured_builder.finish()),
        Arc::new(read_time_builder.finish()),
        Arc::new(vector_builder.finish()),
        Arc::new(created_at_builder.finish()),
        Arc::new(updated_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn build_image_batch(records: &[ImageRecord]) -> Result<RecordBatch> {
    let mut id_builder = StringBuilder::new();
    let mut filename_builder = StringBuilder::new();
    let mut data_builder = BinaryBuilder::new();
    let mut thumb_builder = BinaryBuilder::new();
    let mut vector_builder =
        FixedSizeListBuilder::new(arrow_array::builder::Float32Builder::new(), IMAGE_VECTOR_DIM as i32);
    let mut metadata_builder = StringBuilder::new();
    let mut created_at_builder = TimestampMillisecondBuilder::new();

    for record in records {
        id_builder.append_value(&record.id);
        filename_builder.append_value(&record.filename);
        data_builder.append_value(&record.data);

        if let Some(thumb) = &record.thumbnail {
            thumb_builder.append_value(thumb);
        } else {
            thumb_builder.append_null();
        }

        if record.vector.len() != IMAGE_VECTOR_DIM {
            anyhow::bail!(
                "image vector length {} does not match {}",
                record.vector.len(),
                IMAGE_VECTOR_DIM
            );
        }
        for value in &record.vector {
            vector_builder.values().append_value(*value);
        }
        vector_builder.append(true);

        metadata_builder.append_value(&record.metadata);
        created_at_builder.append_value(record.created_at);
    }

    let schema = Arc::new(image_schema());
    let arrays: Vec<ArrayRef> = vec![
        Arc::new(id_builder.finish()),
        Arc::new(filename_builder.finish()),
        Arc::new(data_builder.finish()),
        Arc::new(thumb_builder.finish()),
        Arc::new(vector_builder.finish()),
        Arc::new(metadata_builder.finish()),
        Arc::new(created_at_builder.finish()),
    ];

    Ok(RecordBatch::try_new(schema, arrays)?)
}

fn collect_image_files(dir: &Path, recursive: bool) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let exts = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];

    if recursive {
        for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok) {
            if entry.file_type().is_file() {
                let path = entry.path();
                if has_image_extension(path, &exts) {
                    files.push(path.to_path_buf());
                }
            }
        }
    } else {
        for entry in fs::read_dir(dir).context("failed to read image directory")? {
            let entry = entry?;
            let path = entry.path();
            if path.is_file() && has_image_extension(&path, &exts) {
                files.push(path);
            }
        }
    }

    Ok(files)
}

fn has_image_extension(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| exts.iter().any(|item| ext.eq_ignore_ascii_case(item)))
        .unwrap_or(false)
}
