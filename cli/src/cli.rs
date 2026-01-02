use std::path::PathBuf;

use clap::{Parser, Subcommand};

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
    /// Write a Markdown article into LanceDB.
    WriteArticle {
        /// LanceDB directory path.
        #[arg(long, default_value = "./data/lancedb")]
        db_path: PathBuf,
        /// Markdown file path.
        #[arg(long)]
        file: PathBuf,
        /// Article summary (optional if frontmatter provides it).
        #[arg(long)]
        summary: Option<String>,
        /// Comma-separated tags list (optional if frontmatter provides it).
        #[arg(long)]
        tags: Option<String>,
        /// Article category (optional if frontmatter provides it).
        #[arg(long)]
        category: Option<String>,
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
