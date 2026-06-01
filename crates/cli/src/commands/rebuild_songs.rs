use std::path::Path;

use anyhow::{Context, Result};
use static_flow_shared::music_store::MusicDataStore;

pub async fn run(db_path: &Path, batch_size: usize) -> Result<()> {
    let db_uri = db_path.to_string_lossy();
    let store = MusicDataStore::connect(&db_uri)
        .await
        .context("failed to connect to music DB")?;

    tracing::info!("Starting songs table rebuild (batch_size={batch_size})...");
    let count = store.rebuild_songs_table(batch_size, &db_uri).await?;
    tracing::info!("Rebuild complete: {count} songs written with new schema.");
    Ok(())
}
