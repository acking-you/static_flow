use std::{fs, path::Path};

use anyhow::{Context, Result};
use image::{GenericImageView, ImageFormat};
use static_flow_shared::embedding::embed_image_bytes;

use crate::{
    db::{connect_db, ensure_vector_index, upsert_images},
    schema::ImageRecord,
    utils::{collect_image_files, encode_thumbnail, hash_bytes, relative_filename},
};

pub async fn run(
    db_path: &Path,
    dir: &Path,
    recursive: bool,
    generate_thumbnail: bool,
    thumbnail_size: u32,
) -> Result<()> {
    let db = connect_db(db_path).await?;
    let table = db
        .open_table("images")
        .execute()
        .await
        .context("images table not found; run `sf-cli init` first")?;

    let files = collect_image_files(dir, recursive)?;
    if files.is_empty() {
        tracing::info!("No images found in {}", dir.display());
        return Ok(());
    }

    let mut records = Vec::new();
    for path in files {
        let bytes =
            fs::read(&path).with_context(|| format!("failed to read image {}", path.display()))?;
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
                (embed_image_bytes(&bytes), thumb)
            },
            Err(_) => {
                metadata["format"] = serde_json::json!(None::<String>);
                (embed_image_bytes(&bytes), None)
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

    if let Err(err) = ensure_vector_index(&table, "vector").await {
        tracing::warn!("Failed to create vector index on images: {err}");
    }

    tracing::info!("Wrote {} images to LanceDB.", records.len());
    Ok(())
}
