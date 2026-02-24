use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use chrono::Utc;
use static_flow_shared::{
    embedding::text::{detect_language, embed_text_with_language, TextEmbeddingLanguage},
    music_store::{MusicDataStore, SongRecord},
};

pub struct WriteMusicOptions {
    pub id: Option<String>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_id: Option<String>,
    pub cover: Option<PathBuf>,
    pub _content_db_path: PathBuf, // reserved for cover image import
    pub lyrics: Option<PathBuf>,
    pub lyrics_translation: Option<PathBuf>,
    pub source: String,
    pub source_id: Option<String>,
    pub tags: Option<String>,
}

pub async fn run(db_path: &Path, file: &Path, opts: WriteMusicOptions) -> Result<()> {
    if !file.exists() {
        bail!("Audio file not found: {}", file.display());
    }

    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
        .unwrap_or_default();

    if ext != "mp3" && ext != "flac" {
        bail!("Unsupported audio format: {}. Only mp3 and flac are supported.", ext);
    }

    let audio_data = std::fs::read(file)
        .with_context(|| format!("failed to read audio file: {}", file.display()))?;

    // Extract metadata from audio tags using lofty
    let (tag_title, tag_artist, tag_album, tag_duration_ms, tag_bitrate) =
        extract_audio_tags(file)?;

    let title = opts.title.unwrap_or(tag_title);
    let artist = opts.artist.unwrap_or(tag_artist);
    let album = opts.album.unwrap_or(tag_album);

    let file_stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    let id = opts
        .id
        .unwrap_or_else(|| format!("{}-{}", opts.source, file_stem));

    // Read lyrics files if provided
    let lyrics_lrc = if let Some(ref path) = opts.lyrics {
        Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("failed to read lyrics: {}", path.display()))?,
        )
    } else {
        None
    };
    let lyrics_translation =
        if let Some(ref path) = opts.lyrics_translation {
            Some(std::fs::read_to_string(path).with_context(|| {
                format!("failed to read lyrics translation: {}", path.display())
            })?)
        } else {
            None
        };

    // Build searchable text
    let lyrics_plain = lyrics_lrc
        .as_deref()
        .map(strip_lrc_timestamps)
        .unwrap_or_default();
    let searchable_text = format!("{} {} {} {}", title, artist, album, lyrics_plain);

    // Generate vector embeddings for semantic search
    let lang = detect_language(&searchable_text);
    let primary_vector = embed_text_with_language(&searchable_text, lang);
    let (vector_en, vector_zh) = match lang {
        TextEmbeddingLanguage::Chinese => {
            let en_vector =
                embed_text_with_language(&searchable_text, TextEmbeddingLanguage::English);
            (Some(en_vector), Some(primary_vector))
        },
        TextEmbeddingLanguage::English => (Some(primary_vector), None),
    };

    // Handle cover image (just store the filename reference for now)
    let cover_image = if let Some(ref cover_path) = opts.cover {
        let cover_filename = cover_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("cover.jpg");
        Some(cover_filename.to_string())
    } else {
        None
    };

    let tags_str = opts
        .tags
        .as_deref()
        .map(|t| {
            let tags: Vec<&str> = t
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            serde_json::to_string(&tags).unwrap_or_else(|_| "[]".to_string())
        })
        .unwrap_or_else(|| "[]".to_string());

    let now = Utc::now().timestamp_millis();
    let record = SongRecord {
        id: id.clone(),
        title: title.clone(),
        artist: artist.clone(),
        album: album.clone(),
        album_id: opts.album_id,
        cover_image,
        duration_ms: tag_duration_ms,
        format: ext.clone(),
        bitrate: tag_bitrate,
        lyrics_lrc,
        lyrics_translation,
        audio_data,
        source: opts.source.clone(),
        source_id: opts.source_id,
        tags: tags_str,
        searchable_text,
        vector_en,
        vector_zh,
        created_at: now,
        updated_at: now,
    };

    let db_uri = db_path.to_string_lossy();
    let store = MusicDataStore::connect(&db_uri).await?;
    store.upsert_song(&record).await?;

    println!("Song written successfully:");
    println!("  id:       {}", id);
    println!("  title:    {}", title);
    println!("  artist:   {}", artist);
    println!("  album:    {}", album);
    println!("  format:   {}", ext);
    println!("  duration: {}ms", tag_duration_ms);

    Ok(())
}

fn extract_audio_tags(path: &Path) -> Result<(String, String, String, u64, u64)> {
    use lofty::prelude::*;

    let tagged_file = lofty::read_from_path(path)
        .with_context(|| format!("failed to read audio tags from: {}", path.display()))?;

    let tag = tagged_file
        .primary_tag()
        .or_else(|| tagged_file.first_tag());

    let title = tag
        .and_then(|t| t.title().map(|s| s.to_string()))
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("Unknown")
                .to_string()
        });
    let artist = tag
        .and_then(|t| t.artist().map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown".to_string());
    let album = tag
        .and_then(|t| t.album().map(|s| s.to_string()))
        .unwrap_or_else(|| "Unknown".to_string());

    let properties = tagged_file.properties();
    let duration_ms = properties.duration().as_millis() as u64;
    let bitrate = properties.audio_bitrate().unwrap_or(0) as u64 * 1000;

    Ok((title, artist, album, duration_ms, bitrate))
}

fn strip_lrc_timestamps(lrc: &str) -> String {
    let mut result = String::new();
    for line in lrc.lines() {
        let text = line.trim();
        // Strip [mm:ss.xx] timestamps
        let mut s = text;
        while s.starts_with('[') {
            if let Some(end) = s.find(']') {
                s = s[end + 1..].trim_start();
            } else {
                break;
            }
        }
        if !s.is_empty() {
            if !result.is_empty() {
                result.push(' ');
            }
            result.push_str(s);
        }
    }
    result
}
