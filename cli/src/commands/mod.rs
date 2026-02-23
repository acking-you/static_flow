pub mod api;
pub mod db_manage;
pub mod embed_songs;
pub mod ensure_indexes;
pub mod init;
pub mod query;
pub mod sync_notes;
pub mod write_article;
pub mod write_images;
pub mod write_music;

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
            date,
            content_en_file,
            summary_zh_file,
            summary_en_file,
            import_local_images,
            media_roots,
            generate_thumbnail,
            thumbnail_size,
            vector,
            vector_en,
            vector_zh,
            language,
            no_auto_optimize,
        } => {
            write_article::run(&db_path, &file, write_article::WriteArticleOptions {
                id,
                summary,
                tags,
                category,
                category_description,
                date,
                content_en_file,
                summary_zh_file,
                summary_en_file,
                import_local_images,
                media_roots,
                generate_thumbnail,
                thumbnail_size,
                vector,
                vector_en,
                vector_zh,
                language,
                auto_optimize: !no_auto_optimize,
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
            no_auto_optimize,
        } => {
            sync_notes::run(&db_path, &dir, sync_notes::SyncNotesOptions {
                recursive,
                generate_thumbnail,
                thumbnail_size,
                language,
                default_category,
                default_author,
                auto_optimize: !no_auto_optimize,
            })
            .await
        },
        Commands::WriteImages {
            db_path,
            dir,
            recursive,
            generate_thumbnail,
            thumbnail_size,
            no_auto_optimize,
        } => {
            write_images::run(
                &db_path,
                &dir,
                recursive,
                generate_thumbnail,
                thumbnail_size,
                !no_auto_optimize,
            )
            .await
        },
        Commands::WriteMusic {
            db_path,
            file,
            id,
            title,
            artist,
            album,
            album_id,
            cover,
            content_db_path,
            lyrics,
            lyrics_translation,
            source,
            source_id,
            tags,
        } => {
            write_music::run(&db_path, &file, write_music::WriteMusicOptions {
                id,
                title,
                artist,
                album,
                album_id,
                cover,
                _content_db_path: content_db_path,
                lyrics,
                lyrics_translation,
                source,
                source_id,
                tags,
            })
            .await
        },
        Commands::EmbedSongs { db_path } => {
            embed_songs::run(&db_path).await
        },
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
            DbCommands::UpdateArticleBilingual {
                id,
                content_en_file,
                summary_zh_file,
                summary_en_file,
            } => {
                db_manage::update_article_bilingual(
                    &db_path,
                    &id,
                    content_en_file.as_deref(),
                    summary_zh_file.as_deref(),
                    summary_en_file.as_deref(),
                )
                .await
            },
            DbCommands::BackfillArticleVectors {
                limit,
                dry_run,
            } => db_manage::backfill_article_vectors(&db_path, limit, dry_run).await,
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
                prune_now,
            } => db_manage::optimize_table(&db_path, &table, all, prune_now).await,
            DbCommands::CleanupOrphans {
                table,
            } => db_manage::cleanup_orphans(&db_path, table.as_deref()).await,
            DbCommands::ReembedSvgImages {
                limit,
                dry_run,
            } => db_manage::reembed_svg_images(&db_path, limit, dry_run).await,
            DbCommands::UpsertArticle {
                json,
            } => db_manage::upsert_article_json(&db_path, &json).await,
            DbCommands::UpsertImage {
                json,
            } => db_manage::upsert_image_json(&db_path, &json).await,
        },
    }
}
