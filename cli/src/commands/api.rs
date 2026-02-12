use std::{fs, path::Path};

use anyhow::{bail, Context, Result};
use serde::Serialize;
use static_flow_shared::lancedb_api::{
    ArticleListResponse, CategoriesResponse, ImageListResponse, ImageSearchResponse,
    ImageTextSearchResponse, SearchResponse, StaticFlowDataStore, TagsResponse,
};

use crate::cli::ApiCommands;

#[derive(Serialize)]
struct ImageWriteResult {
    filename: String,
    mime_type: String,
    bytes: usize,
    output: String,
}

pub async fn run(db_path: &Path, command: ApiCommands) -> Result<()> {
    let db_uri = db_path.to_string_lossy();
    let store = StaticFlowDataStore::connect(&db_uri).await?;

    match command {
        ApiCommands::ListArticles {
            tag,
            category,
        } => {
            let articles = store
                .list_articles(tag.as_deref(), category.as_deref())
                .await?;
            print_json(&ArticleListResponse {
                total: articles.len(),
                articles,
            })
        },
        ApiCommands::GetArticle {
            id,
        } => {
            let article = store.get_article(&id).await?;
            match article {
                Some(article) => print_json(&article),
                None => bail!("article not found: {id}"),
            }
        },
        ApiCommands::RelatedArticles {
            id,
        } => {
            let articles = store.related_articles(&id, 4).await?;
            print_json(&ArticleListResponse {
                total: articles.len(),
                articles,
            })
        },
        ApiCommands::Search {
            q,
        } => {
            let keyword = q.trim();
            let response = if keyword.is_empty() {
                SearchResponse {
                    results: vec![],
                    total: 0,
                    query: q,
                }
            } else {
                let results = store.search_articles(keyword, Some(10)).await?;
                SearchResponse {
                    total: results.len(),
                    results,
                    query: q,
                }
            };
            print_json(&response)
        },
        ApiCommands::SemanticSearch {
            q,
            enhanced_highlight,
        } => {
            let keyword = q.trim();
            let response = if keyword.is_empty() {
                SearchResponse {
                    results: vec![],
                    total: 0,
                    query: q,
                }
            } else {
                let results = store
                    .semantic_search(keyword, Some(10), None, enhanced_highlight)
                    .await?;
                SearchResponse {
                    total: results.len(),
                    results,
                    query: q,
                }
            };
            print_json(&response)
        },
        ApiCommands::ListTags => {
            let tags = store.list_tags().await?;
            print_json(&TagsResponse {
                tags,
            })
        },
        ApiCommands::ListCategories => {
            let categories = store.list_categories().await?;
            print_json(&CategoriesResponse {
                categories,
            })
        },
        ApiCommands::ListImages => {
            let images = store.list_images().await?;
            print_json(&ImageListResponse {
                total: images.len(),
                images,
            })
        },
        ApiCommands::SearchImages {
            id,
        } => {
            let images = store.search_images(&id, Some(12), None).await?;
            print_json(&ImageSearchResponse {
                total: images.len(),
                images,
                query_id: id,
            })
        },
        ApiCommands::SearchImagesText {
            q,
        } => {
            let keyword = q.trim();
            let response = if keyword.is_empty() {
                ImageTextSearchResponse {
                    images: vec![],
                    total: 0,
                    query: q,
                }
            } else {
                let images = store.search_images_by_text(keyword, Some(24), None).await?;
                ImageTextSearchResponse {
                    total: images.len(),
                    images,
                    query: q,
                }
            };
            print_json(&response)
        },
        ApiCommands::GetImage {
            id_or_filename,
            thumb,
            out,
        } => {
            let image = store.get_image(&id_or_filename, thumb).await?;
            let image = match image {
                Some(image) => image,
                None => bail!("image not found: {id_or_filename}"),
            };

            let out_path = out.unwrap_or_else(|| Path::new(&image.filename).to_path_buf());
            fs::write(&out_path, &image.bytes)
                .with_context(|| format!("failed to write image to {}", out_path.display()))?;

            print_json(&ImageWriteResult {
                filename: image.filename,
                mime_type: image.mime_type,
                bytes: image.bytes.len(),
                output: out_path.display().to_string(),
            })
        },
    }
}

fn print_json<T: Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
