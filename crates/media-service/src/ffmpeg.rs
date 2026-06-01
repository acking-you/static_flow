use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, Result};
use ffmpeg_sidecar::{ffprobe, paths};
use tokio::process::Command;

use crate::{config::LocalMediaConfig, probe::PlaybackStrategy};

#[derive(Debug, Clone)]
pub struct BinaryPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

pub async fn ensure_binary_paths(config: &LocalMediaConfig) -> Result<BinaryPaths> {
    let config = config.clone();
    tokio::task::spawn_blocking(move || ensure_binary_paths_blocking(&config))
        .await
        .context("ffmpeg sidecar resolution task failed")?
}

fn ensure_binary_paths_blocking(config: &LocalMediaConfig) -> Result<BinaryPaths> {
    let mut ffmpeg = config.ffmpeg_bin.clone().unwrap_or_else(paths::ffmpeg_path);
    let mut ffprobe_path = config
        .ffprobe_bin
        .clone()
        .unwrap_or_else(ffprobe::ffprobe_path);

    if (!binary_works(&ffmpeg) || !binary_works(&ffprobe_path)) && config.auto_download_ffmpeg {
        ffmpeg_sidecar::download::auto_download()
            .context("failed to auto-download ffmpeg sidecar")?;
        ffmpeg = config.ffmpeg_bin.clone().unwrap_or_else(paths::ffmpeg_path);
        ffprobe_path = config
            .ffprobe_bin
            .clone()
            .unwrap_or_else(ffprobe::ffprobe_path);
    }

    if !binary_works(&ffmpeg) {
        anyhow::bail!("ffmpeg binary is unavailable: {}", ffmpeg.display());
    }
    if !binary_works(&ffprobe_path) {
        anyhow::bail!("ffprobe binary is unavailable: {}", ffprobe_path.display());
    }

    Ok(BinaryPaths {
        ffmpeg,
        ffprobe: ffprobe_path,
    })
}

fn binary_works(path: &Path) -> bool {
    std::process::Command::new(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

pub fn build_hls_command(
    bins: &BinaryPaths,
    source: &Path,
    output_dir: &Path,
    strategy: PlaybackStrategy,
    has_audio: bool,
) -> Command {
    let mut command = Command::new(&bins.ffmpeg);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(source)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("0:a:0?")
        .arg("-sn")
        .arg("-dn");

    match strategy {
        PlaybackStrategy::Raw {
            ..
        } => {},
        PlaybackStrategy::Mp4Remux => unreachable!("mp4 remux uses build_mp4_remux_command"),
        PlaybackStrategy::HlsCopy => {
            command.arg("-c:v").arg("copy");
            if has_audio {
                command.arg("-c:a").arg("copy");
            } else {
                command.arg("-an");
            }
        },
        PlaybackStrategy::HlsTranscode => {
            command
                .arg("-c:v")
                .arg("libx264")
                .arg("-preset")
                .arg("veryfast")
                .arg("-crf")
                .arg("23")
                .arg("-pix_fmt")
                .arg("yuv420p");
            if has_audio {
                command
                    .arg("-c:a")
                    .arg("aac")
                    .arg("-b:a")
                    .arg("128k")
                    .arg("-ac")
                    .arg("2");
            } else {
                command.arg("-an");
            }
        },
    }

    command
        .arg("-max_muxing_queue_size")
        .arg("1024")
        .arg("-start_number")
        .arg("0")
        .arg("-hls_time")
        .arg("6")
        .arg("-hls_list_size")
        .arg("0")
        .arg("-hls_playlist_type")
        .arg("event")
        .arg("-hls_flags")
        .arg("independent_segments")
        .arg("-hls_segment_filename")
        .arg(output_dir.join("segment_%05d.ts"))
        .arg(output_dir.join("index.m3u8"));

    command
}

pub fn build_mp4_remux_command(
    bins: &BinaryPaths,
    source: &Path,
    output_path: &Path,
    has_audio: bool,
) -> Command {
    let mut command = Command::new(&bins.ffmpeg);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-i")
        .arg(source)
        .arg("-map")
        .arg("0:v:0")
        .arg("-map")
        .arg("0:a:0?")
        .arg("-sn")
        .arg("-dn")
        .arg("-c:v")
        .arg("copy");

    if has_audio {
        command.arg("-c:a").arg("copy");
    } else {
        command.arg("-an");
    }

    command.arg("-movflags").arg("+faststart").arg(output_path);

    command
}

pub fn build_poster_command(
    bins: &BinaryPaths,
    source: &Path,
    output_path: &Path,
    seek_seconds: f64,
) -> Command {
    let mut command = Command::new(&bins.ffmpeg);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-y")
        .arg("-ss")
        .arg(format!("{seek_seconds:.3}"))
        .arg("-i")
        .arg(source)
        .arg("-frames:v")
        .arg("1")
        .arg("-f")
        .arg("image2")
        .arg("-q:v")
        .arg("4")
        .arg("-vf")
        .arg("scale='min(960,iw)':-2")
        .arg(output_path);
    command
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{build_hls_command, build_mp4_remux_command, build_poster_command, BinaryPaths};
    use crate::probe::PlaybackStrategy;

    #[test]
    fn build_hls_command_uses_incremental_event_playlist() {
        let bins = BinaryPaths {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        };
        let command = build_hls_command(
            &bins,
            PathBuf::from("/tmp/input.mkv").as_path(),
            PathBuf::from("/tmp/output").as_path(),
            PlaybackStrategy::HlsCopy,
            true,
        );
        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|pair| pair == ["-hls_list_size", "0"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-hls_playlist_type", "event"]));
        assert!(!args.iter().any(|arg| arg == "temp_file"));
        assert!(!args.iter().any(|arg| arg == "vod"));
    }

    #[test]
    fn build_mp4_remux_command_copies_streams_into_mp4_output() {
        let bins = BinaryPaths {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        };
        let command = build_mp4_remux_command(
            &bins,
            PathBuf::from("/tmp/input.mkv").as_path(),
            PathBuf::from("/tmp/output.mp4").as_path(),
            true,
        );
        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:a", "copy"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-movflags", "+faststart"]));
        assert_eq!(args.last().map(String::as_str), Some("/tmp/output.mp4"));
    }

    #[test]
    fn build_poster_command_scales_down_large_frames() {
        let bins = BinaryPaths {
            ffmpeg: PathBuf::from("ffmpeg"),
            ffprobe: PathBuf::from("ffprobe"),
        };
        let command = build_poster_command(
            &bins,
            PathBuf::from("/tmp/input.mkv").as_path(),
            PathBuf::from("/tmp/poster.jpg").as_path(),
            97.021,
        );
        let args = command
            .as_std()
            .get_args()
            .map(|value| value.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert!(args.windows(2).any(|pair| pair == ["-frames:v", "1"]));
        assert!(args.windows(2).any(|pair| pair == ["-f", "image2"]));
        assert!(args.windows(2).any(|pair| pair == ["-q:v", "4"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-vf", "scale='min(960,iw)':-2"]));
    }
}
