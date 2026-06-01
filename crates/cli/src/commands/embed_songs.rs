use std::path::Path;

use anyhow::{Context, Result};
use static_flow_shared::music_store::MusicDataStore;

pub async fn run(db_path: &Path) -> Result<()> {
    let db_uri = db_path.to_string_lossy();
    let store = MusicDataStore::connect(&db_uri)
        .await
        .context("failed to connect to music DB")?;

    let count = store.backfill_song_vectors().await?;
    if count == 0 {
        tracing::info!("All songs already have vector embeddings.");
    } else {
        tracing::info!("Done. Backfilled vectors for {count} songs.");
    }
    Ok(())
}
