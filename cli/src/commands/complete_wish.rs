use std::path::Path;

use anyhow::Result;
use static_flow_shared::music_wish_store::{MusicWishStore, WISH_STATUS_DONE};

pub async fn run(
    db_path: &Path,
    wish_id: &str,
    ingested_song_id: Option<&str>,
    ai_reply: Option<&str>,
    admin_note: Option<&str>,
) -> Result<()> {
    let store = MusicWishStore::connect(db_path.to_str().unwrap_or(".")).await?;
    let record = store
        .transition_wish(wish_id, WISH_STATUS_DONE, admin_note, None, ingested_song_id, ai_reply)
        .await?;
    println!("Wish {} -> status={}", record.wish_id, record.status);
    if let Some(sid) = &record.ingested_song_id {
        println!("  ingested_song_id: {sid}");
    }
    if let Some(reply) = &record.ai_reply {
        let preview: String = reply.chars().take(80).collect();
        let suffix = if reply.chars().count() > 80 { "..." } else { "" };
        println!("  ai_reply: {preview}{suffix}");
    }
    Ok(())
}
