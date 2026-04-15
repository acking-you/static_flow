use std::{collections::BTreeMap, env, path::PathBuf};

use anyhow::{Context, Result};

const DEFAULT_CACHE_DIR: &str = "tmp/local-media-cache";
const DEFAULT_MAX_REMUX_JOBS: usize = 2;
const DEFAULT_MAX_TRANSCODE_JOBS: usize = 1;
const DEFAULT_MAX_POSTER_JOBS: usize = 2;
const DEFAULT_LIST_PAGE_SIZE: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMediaConfig {
    pub enabled: bool,
    pub root: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub auto_download_ffmpeg: bool,
    pub max_remux_jobs: usize,
    pub max_transcode_jobs: usize,
    pub max_poster_jobs: usize,
    pub list_page_size: usize,
    pub ffmpeg_bin: Option<PathBuf>,
    pub ffprobe_bin: Option<PathBuf>,
}

pub fn read_local_media_config_from_env() -> Result<LocalMediaConfig> {
    let env_map = env::vars().collect::<BTreeMap<_, _>>();
    read_local_media_config_from_map(&env_map)
}

fn read_local_media_config_from_map(
    env_map: &BTreeMap<String, String>,
) -> Result<LocalMediaConfig> {
    let enabled = parse_bool_env(env_map, "STATICFLOW_LOCAL_MEDIA_ENABLED", true);
    let root = env_path(env_map, "STATICFLOW_LOCAL_MEDIA_ROOT");
    let cache_dir = env_path(env_map, "STATICFLOW_LOCAL_MEDIA_CACHE_DIR")
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CACHE_DIR));
    let auto_download_ffmpeg =
        parse_bool_env(env_map, "STATICFLOW_LOCAL_MEDIA_AUTO_DOWNLOAD_FFMPEG", true);
    let max_remux_jobs =
        parse_usize_env(env_map, "STATICFLOW_LOCAL_MEDIA_MAX_REMUX_JOBS", DEFAULT_MAX_REMUX_JOBS)?;
    let max_transcode_jobs = parse_usize_env(
        env_map,
        "STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS",
        DEFAULT_MAX_TRANSCODE_JOBS,
    )?;
    let max_poster_jobs = parse_usize_env(
        env_map,
        "STATICFLOW_LOCAL_MEDIA_MAX_POSTER_JOBS",
        DEFAULT_MAX_POSTER_JOBS,
    )?;
    let list_page_size =
        parse_usize_env(env_map, "STATICFLOW_LOCAL_MEDIA_LIST_PAGE_SIZE", DEFAULT_LIST_PAGE_SIZE)?;
    let ffmpeg_bin = env_path(env_map, "STATICFLOW_FFMPEG_BIN");
    let ffprobe_bin = env_path(env_map, "STATICFLOW_FFPROBE_BIN");

    Ok(LocalMediaConfig {
        enabled,
        root,
        cache_dir,
        auto_download_ffmpeg,
        max_remux_jobs,
        max_transcode_jobs,
        max_poster_jobs,
        list_page_size,
        ffmpeg_bin,
        ffprobe_bin,
    })
}

fn parse_bool_env(env_map: &BTreeMap<String, String>, key: &str, default: bool) -> bool {
    env_map
        .get(key)
        .map(|value| {
            matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        })
        .unwrap_or(default)
}

fn parse_usize_env(env_map: &BTreeMap<String, String>, key: &str, default: usize) -> Result<usize> {
    let value = match env_map.get(key) {
        Some(value) if !value.trim().is_empty() => value,
        _ => return Ok(default),
    };
    let parsed = value
        .trim()
        .parse::<usize>()
        .with_context(|| format!("failed to parse {key} as usize"))?;
    if parsed == 0 {
        anyhow::bail!("{key} must be greater than zero");
    }
    Ok(parsed)
}

fn env_path(env_map: &BTreeMap<String, String>, key: &str) -> Option<PathBuf> {
    env_map
        .get(key)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
pub(crate) fn read_local_media_config_for_test(vars: &[(&str, &str)]) -> Result<LocalMediaConfig> {
    let env_map = vars
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect::<BTreeMap<_, _>>();
    read_local_media_config_from_map(&env_map)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::read_local_media_config_for_test;

    #[test]
    fn read_local_media_config_from_env_allows_missing_root() {
        let cfg = read_local_media_config_for_test(&[]).expect("config should parse");
        assert!(cfg.root.is_none());
        assert_eq!(cfg.max_remux_jobs, 2);
        assert_eq!(cfg.max_transcode_jobs, 1);
        assert_eq!(cfg.max_poster_jobs, 2);
        assert!(cfg.enabled);
        assert!(cfg.auto_download_ffmpeg);
        assert_eq!(cfg.cache_dir, PathBuf::from("tmp/local-media-cache"));
    }

    #[test]
    fn read_local_media_config_from_env_accepts_explicit_paths() {
        let cfg = read_local_media_config_for_test(&[
            ("STATICFLOW_LOCAL_MEDIA_ROOT", "/tmp/media"),
            ("STATICFLOW_LOCAL_MEDIA_CACHE_DIR", "/tmp/cache"),
            ("STATICFLOW_LOCAL_MEDIA_MAX_REMUX_JOBS", "4"),
            ("STATICFLOW_FFMPEG_BIN", "/tmp/bin/ffmpeg"),
            ("STATICFLOW_FFPROBE_BIN", "/tmp/bin/ffprobe"),
        ])
        .expect("config should parse");
        assert_eq!(cfg.root, Some(PathBuf::from("/tmp/media")));
        assert_eq!(cfg.cache_dir, PathBuf::from("/tmp/cache"));
        assert_eq!(cfg.max_remux_jobs, 4);
        assert_eq!(cfg.ffmpeg_bin, Some(PathBuf::from("/tmp/bin/ffmpeg")));
        assert_eq!(cfg.ffprobe_bin, Some(PathBuf::from("/tmp/bin/ffprobe")));
    }

    #[test]
    fn read_local_media_config_from_env_rejects_zero_remux_jobs() {
        let err =
            read_local_media_config_for_test(&[("STATICFLOW_LOCAL_MEDIA_MAX_REMUX_JOBS", "0")])
                .expect_err("zero jobs must be rejected");
        assert!(err
            .to_string()
            .contains("STATICFLOW_LOCAL_MEDIA_MAX_REMUX_JOBS"));
    }

    #[test]
    fn read_local_media_config_from_env_rejects_zero_transcode_jobs() {
        let err =
            read_local_media_config_for_test(&[("STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS", "0")])
                .expect_err("zero jobs must be rejected");
        assert!(err
            .to_string()
            .contains("STATICFLOW_LOCAL_MEDIA_MAX_TRANSCODE_JOBS"));
    }

    #[test]
    fn read_local_media_config_from_env_rejects_zero_poster_jobs() {
        let err =
            read_local_media_config_for_test(&[("STATICFLOW_LOCAL_MEDIA_MAX_POSTER_JOBS", "0")])
                .expect_err("zero poster jobs must be rejected");
        assert!(err
            .to_string()
            .contains("STATICFLOW_LOCAL_MEDIA_MAX_POSTER_JOBS"));
    }
}
