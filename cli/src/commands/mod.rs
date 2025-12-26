pub mod init;
pub mod query;
pub mod write_article;
pub mod write_images;

use anyhow::Result;

use crate::cli::{Cli, Commands};

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init { db_path } => init::run(&db_path).await,
        Commands::WriteArticle {
            db_path,
            file,
            summary,
            tags,
            category,
            vector,
            vector_en,
            vector_zh,
            language,
        } => {
            write_article::run(
                &db_path,
                &file,
                summary,
                tags,
                category,
                vector,
                vector_en,
                vector_zh,
                language,
            )
            .await
        },
        Commands::WriteImages {
            db_path,
            dir,
            recursive,
            generate_thumbnail,
            thumbnail_size,
        } => write_images::run(&db_path, &dir, recursive, generate_thumbnail, thumbnail_size).await,
        Commands::Query {
            db_path,
            table,
            limit,
        } => query::run(&db_path, &table, limit).await,
    }
}
