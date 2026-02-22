use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum QueryOutputFormat {
    Table,
    Vertical,
}

#[derive(Parser)]
#[command(name = "sf-cli", version, about = "StaticFlow LanceDB CLI")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize LanceDB schema and indexes.
    Init {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
    },
    /// Ensure all expected indexes for managed tables.
    EnsureIndexes {
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
        /// Custom article id (defaults to markdown file stem).
        #[arg(long)]
        id: Option<String>,
        /// Article summary (optional if frontmatter provides it).
        #[arg(long)]
        summary: Option<String>,
        /// Comma-separated tags list (optional if frontmatter provides it).
        #[arg(long)]
        tags: Option<String>,
        /// Article category (optional if frontmatter provides it).
        #[arg(long)]
        category: Option<String>,
        /// Category description metadata (required if frontmatter does not
        /// provide `category_description`; stored in taxonomies table).
        #[arg(long)]
        category_description: Option<String>,
        /// Article publication date in `YYYY-MM-DD` format. Overrides
        /// frontmatter `date` when provided.
        #[arg(long)]
        date: Option<String>,
        /// Path to translated English markdown for `content_en`.
        #[arg(long)]
        content_en_file: Option<PathBuf>,
        /// Path to Chinese detailed summary markdown.
        #[arg(long)]
        summary_zh_file: Option<PathBuf>,
        /// Path to English detailed summary markdown.
        #[arg(long)]
        summary_en_file: Option<PathBuf>,
        /// Import local image links from markdown into `images` and rewrite
        /// links.
        #[arg(long)]
        import_local_images: bool,
        /// Additional Obsidian/global media root directories used when an
        /// image cannot be resolved relative to the markdown file.
        #[arg(long = "media-root")]
        media_roots: Vec<PathBuf>,
        /// Generate thumbnails when importing local images.
        #[arg(long)]
        generate_thumbnail: bool,
        /// Thumbnail size (pixels) used with --import-local-images
        /// --generate-thumbnail.
        #[arg(long, default_value_t = 256)]
        thumbnail_size: u32,
        /// Optional embedding vector as JSON array.
        #[arg(long)]
        vector: Option<String>,
        /// Optional English embedding vector as JSON array.
        #[arg(long)]
        vector_en: Option<String>,
        /// Optional Chinese embedding vector as JSON array.
        #[arg(long)]
        vector_zh: Option<String>,
        /// Optional language hint for auto-embedding (en/zh).
        #[arg(long, value_parser = ["en", "zh"])]
        language: Option<String>,
        /// Disable automatic index optimization after write.
        #[arg(long)]
        no_auto_optimize: bool,
    },
    /// Sync a local notes directory (markdown + images) into LanceDB.
    SyncNotes {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Notes directory path.
        #[arg(long)]
        dir: PathBuf,
        /// Recursively scan notes directory.
        #[arg(long)]
        recursive: bool,
        /// Generate thumbnails for imported images.
        #[arg(long)]
        generate_thumbnail: bool,
        /// Thumbnail size (pixels).
        #[arg(long, default_value_t = 256)]
        thumbnail_size: u32,
        /// Optional language hint for auto-embedding (en/zh).
        #[arg(long, value_parser = ["en", "zh"])]
        language: Option<String>,
        /// Default category used when frontmatter category is missing.
        #[arg(long, default_value = "Notes")]
        default_category: String,
        /// Default author used when frontmatter author is missing.
        #[arg(long, default_value = "Unknown")]
        default_author: String,
        /// Disable automatic index optimization after sync.
        #[arg(long)]
        no_auto_optimize: bool,
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
        /// Disable automatic index optimization after image write.
        #[arg(long)]
        no_auto_optimize: bool,
    },
    /// Write a music file (mp3/flac) into the music LanceDB.
    WriteMusic {
        /// Music LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb-music")]
        db_path: PathBuf,
        /// Audio file path (mp3/flac).
        #[arg(long)]
        file: PathBuf,
        /// Custom song id (defaults to "manual-{file_stem}").
        #[arg(long)]
        id: Option<String>,
        /// Song title (auto-extracted from file tags if omitted).
        #[arg(long)]
        title: Option<String>,
        /// Artist name (auto-extracted from file tags if omitted).
        #[arg(long)]
        artist: Option<String>,
        /// Album name (auto-extracted from file tags if omitted).
        #[arg(long)]
        album: Option<String>,
        /// Album ID for grouping.
        #[arg(long)]
        album_id: Option<String>,
        /// Cover image file path.
        #[arg(long)]
        cover: Option<PathBuf>,
        /// Content DB path for cover image import.
        #[arg(long, default_value = "./data/lancedb")]
        content_db_path: PathBuf,
        /// LRC lyrics file path.
        #[arg(long)]
        lyrics: Option<PathBuf>,
        /// Translated LRC lyrics file path.
        #[arg(long)]
        lyrics_translation: Option<PathBuf>,
        /// Source identifier.
        #[arg(long, default_value = "manual")]
        source: String,
        /// Source platform track ID.
        #[arg(long)]
        source_id: Option<String>,
        /// Comma-separated tags.
        #[arg(long)]
        tags: Option<String>,
    },
    /// Query a table and print the first rows.
    Query {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Table name (articles/images/taxonomies).
        #[arg(long)]
        table: String,
        /// SQL filter expression.
        #[arg(long = "where")]
        where_clause: Option<String>,
        /// Comma-separated columns to project.
        #[arg(long, value_delimiter = ',')]
        columns: Vec<String>,
        /// Number of rows to fetch.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Number of rows to skip.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Output format (`table` or `vertical`).
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Table, ignore_case = true)]
        format: QueryOutputFormat,
    },
    /// Backend-like API commands for local debugging.
    Api {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        #[command(subcommand)]
        command: ApiCommands,
    },
    /// Database-style management commands for LanceDB tables.
    Db {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        #[command(subcommand)]
        command: DbCommands,
    },
}

#[derive(Subcommand)]
pub enum ApiCommands {
    /// GET /api/articles
    ListArticles {
        /// Optional tag filter.
        #[arg(long)]
        tag: Option<String>,
        /// Optional category filter.
        #[arg(long)]
        category: Option<String>,
    },
    /// GET /api/articles/:id
    GetArticle {
        /// Article id.
        id: String,
    },
    /// GET /api/articles/:id/related
    RelatedArticles {
        /// Article id.
        id: String,
    },
    /// GET /api/search?q=
    Search {
        /// Search keyword.
        #[arg(long)]
        q: String,
    },
    /// GET /api/semantic-search?q=
    SemanticSearch {
        /// Search keyword.
        #[arg(long)]
        q: String,
        /// Enable high-precision semantic highlight reranking (slower).
        #[arg(long)]
        enhanced_highlight: bool,
    },
    /// GET /api/tags
    ListTags,
    /// GET /api/categories
    ListCategories,
    /// GET /api/images
    ListImages,
    /// GET /api/image-search?id=
    SearchImages {
        /// Image id.
        #[arg(long)]
        id: String,
    },
    /// GET /api/image-search-text?q=
    SearchImagesText {
        /// Text query.
        #[arg(long)]
        q: String,
    },
    /// GET /api/images/:id-or-filename
    GetImage {
        /// Image id or filename.
        id_or_filename: String,
        /// Return thumbnail when available.
        #[arg(long)]
        thumb: bool,
        /// Output file path (defaults to current dir + image filename).
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum DbCommands {
    /// List all tables.
    ListTables {
        /// Maximum table names to return.
        #[arg(long, default_value_t = 100)]
        limit: u32,
    },
    /// Create a managed table (`articles`, `images`, or `taxonomies`).
    CreateTable {
        /// Table name.
        table: String,
        /// Drop existing table first.
        #[arg(long)]
        replace: bool,
    },
    /// Drop a table (requires --yes).
    DropTable {
        /// Table name.
        table: String,
        /// Confirm destructive operation.
        #[arg(long)]
        yes: bool,
    },
    /// Show table schema and row count.
    DescribeTable {
        /// Table name.
        table: String,
    },
    /// Count rows with optional SQL filter.
    CountRows {
        /// Table name.
        table: String,
        /// SQL filter expression.
        #[arg(long = "where")]
        where_clause: Option<String>,
    },
    /// Query rows with projection/filter/pagination.
    QueryRows {
        /// Table name.
        table: String,
        /// SQL filter expression.
        #[arg(long = "where")]
        where_clause: Option<String>,
        /// Comma-separated columns to project.
        #[arg(long, value_delimiter = ',')]
        columns: Vec<String>,
        /// Number of rows to fetch.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Number of rows to skip.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Output format (`table` or `vertical`).
        #[arg(long, value_enum, default_value_t = QueryOutputFormat::Table, ignore_case = true)]
        format: QueryOutputFormat,
    },
    /// Update rows with SQL expressions, e.g. --set "title='new'".
    UpdateRows {
        /// Table name.
        table: String,
        /// Column assignment expression (column=sql_expr). Repeat for multiple
        /// columns.
        #[arg(long = "set", required = true)]
        assignments: Vec<String>,
        /// SQL filter expression.
        #[arg(long = "where")]
        where_clause: Option<String>,
        /// Allow updating all rows when no --where is provided.
        #[arg(long)]
        all: bool,
    },
    /// Update one article's bilingual fields from files.
    UpdateArticleBilingual {
        /// Article id in `articles.id`.
        #[arg(long)]
        id: String,
        /// Path to translated English markdown for `content_en`.
        #[arg(long)]
        content_en_file: Option<PathBuf>,
        /// Path to Chinese detailed summary markdown.
        #[arg(long)]
        summary_zh_file: Option<PathBuf>,
        /// Path to English detailed summary markdown.
        #[arg(long)]
        summary_en_file: Option<PathBuf>,
    },
    /// Backfill missing article vectors from `content`/`content_en`.
    ///
    /// Mapping:
    /// - `content` -> `vector_zh` (Chinese model)
    /// - `content_en` -> `vector_en` (English model)
    BackfillArticleVectors {
        /// Optional upper bound of rows to update in this run.
        #[arg(long)]
        limit: Option<usize>,
        /// Print candidates only, do not write changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Delete rows by SQL filter.
    DeleteRows {
        /// Table name.
        table: String,
        /// SQL filter expression.
        #[arg(long = "where")]
        where_clause: Option<String>,
        /// Allow deleting all rows when no --where is provided.
        #[arg(long)]
        all: bool,
    },
    /// Ensure indexes for managed tables.
    EnsureIndexes {
        /// Optional table filter (`articles`, `images`, or `taxonomies`).
        #[arg(long)]
        table: Option<String>,
    },
    /// List indexes and optional coverage stats.
    ListIndexes {
        /// Table name.
        table: String,
        /// Show index coverage statistics.
        #[arg(long)]
        with_stats: bool,
    },
    /// Drop an index by name.
    DropIndex {
        /// Table name.
        table: String,
        /// Index name.
        name: String,
    },
    /// Optimize index coverage (default) or whole table.
    Optimize {
        /// Table name.
        table: String,
        /// Run full optimization instead of index-only optimization.
        #[arg(long)]
        all: bool,
        /// Run an aggressive prune pass immediately after optimization
        /// (older_than=0, delete_unverified=true).
        #[arg(long)]
        prune_now: bool,
    },
    /// Cleanup unreferenced/orphan files via prune action only.
    ///
    /// This command intentionally avoids full-table rewrite (`--all`) and is
    /// safer for large binary-heavy tables (for example `images`).
    CleanupOrphans {
        /// Optional target table (`articles`, `images`, `taxonomies`, or
        /// `article_views`).
        /// If omitted, runs on all cleanup target tables.
        #[arg(long)]
        table: Option<String>,
    },
    /// Recompute embeddings for SVG rows in `images` table using rasterized
    /// PNG input while keeping original SVG bytes.
    ReembedSvgImages {
        /// Optional upper bound of rows to update in this run.
        #[arg(long)]
        limit: Option<usize>,
        /// Print candidates only, do not write changes.
        #[arg(long)]
        dry_run: bool,
    },
    /// Upsert one article row from JSON payload.
    UpsertArticle {
        /// Full JSON object matching `ArticleRecord` fields.
        #[arg(long)]
        json: String,
    },
    /// Upsert one image row from JSON payload.
    UpsertImage {
        /// Full JSON object matching `ImageRecord` fields.
        #[arg(long)]
        json: String,
    },
}
