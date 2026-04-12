use std::{path::Path, process::Stdio};

use anyhow::{Context, Result};
use serde::Deserialize;
use tokio::process::Command;

use crate::{ffmpeg::BinaryPaths, types::PlaybackMode};

#[derive(Debug, Clone)]
pub struct MediaProbe {
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub duration_seconds: Option<f64>,
}

impl MediaProbe {
    pub fn has_audio(&self) -> bool {
        self.audio_codec.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStrategy {
    Raw { mime_type: &'static str },
    HlsCopy,
    HlsTranscode,
}

#[derive(Debug, Deserialize)]
struct FfprobePayload {
    #[serde(default)]
    streams: Vec<FfprobeStream>,
    format: Option<FfprobeFormat>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    codec_type: Option<String>,
    codec_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
}

pub async fn probe_media(bins: &BinaryPaths, input_path: &Path) -> Result<MediaProbe> {
    let output = Command::new(&bins.ffprobe)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .arg("-v")
        .arg("error")
        .arg("-print_format")
        .arg("json")
        .arg("-show_streams")
        .arg("-show_format")
        .arg(input_path)
        .output()
        .await
        .with_context(|| format!("failed to run ffprobe on {}", input_path.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!("ffprobe failed for {}: {}", input_path.display(), stderr);
    }

    let payload: FfprobePayload = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("failed to parse ffprobe output for {}", input_path.display()))?;

    let mut video_codec = None;
    let mut audio_codec = None;
    for stream in payload.streams {
        match stream.codec_type.as_deref() {
            Some("video") if video_codec.is_none() => video_codec = stream.codec_name.clone(),
            Some("audio") if audio_codec.is_none() => audio_codec = stream.codec_name.clone(),
            _ => {},
        }
    }

    Ok(MediaProbe {
        video_codec,
        audio_codec,
        duration_seconds: payload
            .format
            .and_then(|format| format.duration)
            .and_then(|value| value.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value > 0.0),
    })
}

pub fn choose_playback_strategy(path: &Path, probe: &MediaProbe) -> PlaybackStrategy {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    let video = probe.video_codec.as_deref().unwrap_or_default();
    let audio = probe.audio_codec.as_deref();

    let direct_mp4 =
        matches!(ext.as_str(), "mp4" | "m4v") && video == "h264" && audio_is_mp4_safe(audio);
    if direct_mp4 {
        return PlaybackStrategy::Raw {
            mime_type: "video/mp4",
        };
    }

    let direct_webm = ext == "webm"
        && matches!(video, "vp8" | "vp9" | "av1")
        && matches!(audio, None | Some("opus") | Some("vorbis"));
    if direct_webm {
        return PlaybackStrategy::Raw {
            mime_type: "video/webm",
        };
    }

    if video == "h264" && audio_is_mp4_safe(audio) {
        PlaybackStrategy::HlsCopy
    } else {
        PlaybackStrategy::HlsTranscode
    }
}

pub fn cache_profile_for_strategy(strategy: PlaybackStrategy) -> &'static str {
    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => "raw",
        PlaybackStrategy::HlsCopy => "hls-copy",
        PlaybackStrategy::HlsTranscode => "hls-x264-aac",
    }
}

fn audio_is_mp4_safe(audio: Option<&str>) -> bool {
    matches!(audio, None | Some("aac") | Some("mp3") | Some("mp2"))
}

pub fn mode_for_strategy(strategy: PlaybackStrategy) -> PlaybackMode {
    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => PlaybackMode::Raw,
        PlaybackStrategy::HlsCopy | PlaybackStrategy::HlsTranscode => PlaybackMode::Hls,
    }
}
