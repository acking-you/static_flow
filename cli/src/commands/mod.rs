pub mod api;
pub mod db_manage;
pub mod ensure_indexes;
pub mod init;
pub mod query;
pub mod sync_notes;
pub mod write_article;
pub mod write_images;

use anyhow::Result;

use crate::cli::{Cli, Commands, DbCommands};

pub async fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Init {
            db_path,
        } => init::run(&db_path).await,
        Commands::EnsureIndexes {
            db_path,
        } => ensure_indexes::run(&db_path).await,
        Commands::WriteArticle {
            db_path,
            file,
            id,
            summary,
            tags,
            category,
            category_description,
            vector,
            vector_en,
            vector_zh,
            language,
        } => {
            write_article::run(&db_path, &file, write_article::WriteArticleOptions {
                id,
                summary,
                tags,
                category,
                category_description,
                vector,
                vector_en,
                vector_zh,
                language,
            })
            .await
        },
        Commands::SyncNotes {
            db_path,
            dir,
            recursive,
            generate_thumbnail,
            thumbnail_size,
            language,
            default_category,
            default_author,
        } => {
            sync_notes::run(&db_path, &dir, sync_notes::SyncNotesOptions {
                recursive,
                generate_thumbnail,
                thumbnail_size,
                language,
                default_category,
                default_author,
            })
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
            where_clause,
            columns,
            limit,
            offset,
            format,
        } => {
            query::run(&db_path, db_manage::QueryRowsOptions {
                table,
                where_clause,
                columns,
                limit,
                offset,
                format,
            })
            .await
        },
        Commands::Api {
            db_path,
            command,
        } => api::run(&db_path, command).await,
        Commands::Db {
            db_path,
            command,
        } => match command {
            DbCommands::ListTables {
                limit,
            } => db_manage::list_tables(&db_path, limit).await,
            DbCommands::CreateTable {
                table,
                replace,
            } => db_manage::create_table(&db_path, &table, replace).await,
            DbCommands::DropTable {
                table,
                yes,
            } => db_manage::drop_table(&db_path, &table, yes).await,
            DbCommands::DescribeTable {
                table,
            } => db_manage::describe_table(&db_path, &table).await,
            DbCommands::CountRows {
                table,
                where_clause,
            } => db_manage::count_rows(&db_path, &table, where_clause).await,
            DbCommands::QueryRows {
                table,
                where_clause,
                columns,
                limit,
                offset,
                format,
            } => {
                db_manage::query_rows(&db_path, db_manage::QueryRowsOptions {
                    table,
                    where_clause,
                    columns,
                    limit,
                    offset,
                    format,
                })
                .await
            },
            DbCommands::UpdateRows {
                table,
                assignments,
                where_clause,
                all,
            } => db_manage::update_rows(&db_path, &table, &assignments, where_clause, all).await,
            DbCommands::DeleteRows {
                table,
                where_clause,
                all,
            } => db_manage::delete_rows(&db_path, &table, where_clause, all).await,
            DbCommands::EnsureIndexes {
                table,
            } => db_manage::ensure_indexes(&db_path, table).await,
            DbCommands::ListIndexes {
                table,
                with_stats,
            } => db_manage::list_indexes(&db_path, &table, with_stats).await,
            DbCommands::DropIndex {
                table,
                name,
            } => db_manage::drop_index(&db_path, &table, &name).await,
            DbCommands::Optimize {
                table,
                all,
            } => db_manage::optimize_table(&db_path, &table, all).await,
            DbCommands::UpsertArticle {
                json,
            } => db_manage::upsert_article_json(&db_path, &json).await,
            DbCommands::UpsertImage {
                json,
            } => db_manage::upsert_image_json(&db_path, &json).await,
        },
    }
}
