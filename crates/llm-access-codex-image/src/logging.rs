use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    time::UNIX_EPOCH,
};

use anyhow::Context;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::util::now_ms;

/// Upstream outcome fields recorded in the image request log.
#[derive(Debug, Clone, Copy)]
pub struct UpstreamLogInput<'a> {
    /// Upstream HTTP status code, when a response was received.
    pub status: Option<u16>,
    /// Request duration in milliseconds.
    pub duration_ms: u64,
    /// Number of account failovers before the final outcome.
    pub failover_count: u64,
    /// Stable error class, when the request failed.
    pub error_class: Option<&'a str>,
    /// Number of images returned by upstream.
    pub response_image_count: Option<u64>,
    /// Total returned image payload bytes after decoding known response fields.
    pub response_image_bytes: Option<u64>,
    /// Usage token count if upstream exposed it.
    pub usage_tokens: Option<u64>,
    /// Whether usage metadata was absent.
    pub usage_missing: bool,
}

/// Input used to build a redacted image request log event.
#[derive(Debug, Clone, Copy)]
pub struct ImageLogInput<'a> {
    /// Gateway entrypoint mode.
    pub gateway_mode: &'a str,
    /// Stable request id for correlating logs.
    pub request_id: &'a str,
    /// Authenticated key id.
    pub key_id: &'a str,
    /// Authenticated key name.
    pub key_name: &'a str,
    /// Selected Codex account name.
    pub account_name: Option<&'a str>,
    /// Normalized endpoint name.
    pub endpoint: &'a str,
    /// Raw prompt, hashed but not stored.
    pub prompt: &'a str,
    /// Requested image size.
    pub size: Option<&'a str>,
    /// Requested image quality.
    pub quality: Option<&'a str>,
    /// Requested image count.
    pub n: u64,
    /// Edit input image sources, counted but not stored.
    pub input_images: &'a [&'a str],
    /// Upstream outcome.
    pub upstream: UpstreamLogInput<'a>,
}

/// Redacted structured image request log event.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImageLogEvent {
    /// Schema version for future log readers.
    pub schema_version: u64,
    /// Gateway entrypoint mode.
    pub gateway_mode: String,
    /// Stable request id for correlating logs.
    pub request_id: String,
    /// Authenticated key id.
    pub key_id: String,
    /// Authenticated key name.
    pub key_name: String,
    /// Selected Codex account name.
    pub account_name: Option<String>,
    /// Normalized endpoint name.
    pub endpoint: String,
    /// SHA-256 correlation hash of the prompt; not a secrecy boundary.
    pub prompt_hash: String,
    /// Requested image size.
    pub size: Option<String>,
    /// Requested image quality.
    pub quality: Option<String>,
    /// Requested image count.
    pub n: u64,
    /// Number of edit input images.
    pub input_image_count: u64,
    /// Number of data-url edit input images.
    pub input_data_image_count: u64,
    /// Upstream HTTP status code, when available.
    pub upstream_status: Option<u16>,
    /// Request duration in milliseconds.
    pub duration_ms: u64,
    /// Number of account failovers before the final outcome.
    pub failover_count: u64,
    /// Stable error class, when the request failed.
    pub error_class: Option<String>,
    /// Number of images returned by upstream.
    pub response_image_count: Option<u64>,
    /// Total returned image payload bytes after decoding known response fields.
    pub response_image_bytes: Option<u64>,
    /// Usage token count if upstream exposed it.
    pub usage_tokens: Option<u64>,
    /// Whether usage metadata was absent.
    pub usage_missing: bool,
}

/// Runtime configuration for the image JSONL log writer.
#[derive(Debug, Clone)]
pub struct ImageLogConfig {
    /// Directory containing active and sealed log files.
    pub log_dir: PathBuf,
    /// Maximum active file bytes before rollover.
    pub max_file_bytes: u64,
    /// Maximum active file age before rollover.
    pub max_file_age_ms: u64,
    /// Maximum sealed files retained.
    pub max_files: usize,
}

/// Process-local JSONL image log writer.
#[derive(Debug)]
pub struct ImageLogWriter {
    config: ImageLogConfig,
    active_started_at_ms: u64,
    active_writer: Option<BufWriter<File>>,
    active_bytes: u64,
    sealed_sequence: u64,
}

impl ImageLogWriter {
    /// Creates a writer and ensures the log directory exists.
    pub fn new(config: ImageLogConfig) -> anyhow::Result<Self> {
        fs::create_dir_all(&config.log_dir).with_context(|| {
            format!("create codex image log directory `{}`", config.log_dir.display())
        })?;
        let active_bytes = config
            .log_dir
            .join("codex-image-active.jsonl")
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        Ok(Self {
            config,
            active_started_at_ms: now_ms(),
            active_writer: None,
            active_bytes,
            sealed_sequence: 0,
        })
    }

    /// Appends one redacted event as a JSON line.
    pub fn append(&mut self, event: &ImageLogEvent) -> anyhow::Result<()> {
        self.rotate_if_needed()?;
        let line = serde_json::to_vec(event).context("serialize codex image log event")?;
        let writer = self.active_writer()?;
        writer
            .write_all(&line)
            .context("write codex image log event")?;
        writer
            .write_all(b"\n")
            .context("write codex image log newline")?;
        writer.flush().context("flush codex image log event")?;
        self.active_bytes = self
            .active_bytes
            .saturating_add(u64::try_from(line.len() + 1).unwrap_or(u64::MAX));
        Ok(())
    }

    fn rotate_if_needed(&mut self) -> anyhow::Result<()> {
        let active_path = self.active_path();
        let size_exceeded = self.active_bytes >= self.config.max_file_bytes.max(1);
        let age_exceeded = now_ms().saturating_sub(self.active_started_at_ms)
            >= self.config.max_file_age_ms.max(1);
        if !size_exceeded && !age_exceeded {
            return Ok(());
        }
        if let Some(mut writer) = self.active_writer.take() {
            writer
                .flush()
                .context("flush codex image log before rotation")?;
        }
        if active_path.exists() {
            let sealed_path = self.next_sealed_path();
            fs::rename(&active_path, &sealed_path).with_context(|| {
                format!(
                    "rotate codex image log `{}` to `{}`",
                    active_path.display(),
                    sealed_path.display()
                )
            })?;
        }
        self.active_started_at_ms = now_ms();
        self.active_bytes = 0;
        self.prune_sealed_logs()
    }

    fn active_writer(&mut self) -> anyhow::Result<&mut BufWriter<File>> {
        if self.active_writer.is_none() {
            let active_path = self.active_path();
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&active_path)
                .with_context(|| format!("open codex image log `{}`", active_path.display()))?;
            self.active_writer = Some(BufWriter::new(file));
        }
        Ok(self
            .active_writer
            .as_mut()
            .expect("active writer just opened"))
    }

    fn next_sealed_path(&mut self) -> PathBuf {
        loop {
            self.sealed_sequence = self.sealed_sequence.saturating_add(1);
            let path = self.config.log_dir.join(format!(
                "codex-image-{}-{}.jsonl",
                now_ms(),
                self.sealed_sequence
            ));
            if !path.exists() {
                return path;
            }
        }
    }

    fn prune_sealed_logs(&self) -> anyhow::Result<()> {
        let mut files = fs::read_dir(&self.config.log_dir)
            .with_context(|| {
                format!("read codex image log dir `{}`", self.config.log_dir.display())
            })?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let file_name = entry.file_name();
                let file_name = file_name.to_str()?;
                if file_name.starts_with("codex-image-")
                    && file_name.ends_with(".jsonl")
                    && file_name != "codex-image-active.jsonl"
                {
                    Some(entry)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        if files.len() <= self.config.max_files {
            return Ok(());
        }
        files.sort_by_key(|entry| {
            entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH)
        });
        for entry in files.iter().take(files.len() - self.config.max_files) {
            let _ = fs::remove_file(entry.path());
        }
        Ok(())
    }

    fn active_path(&self) -> PathBuf {
        self.config.log_dir.join("codex-image-active.jsonl")
    }

    #[cfg(test)]
    fn force_active_started_at_ms(&mut self, active_started_at_ms: u64) {
        self.active_started_at_ms = active_started_at_ms;
    }
}

/// Builds a redacted log event that never stores raw prompts or base64 images.
pub fn build_image_log_event(input: ImageLogInput<'_>) -> ImageLogEvent {
    ImageLogEvent {
        schema_version: 1,
        gateway_mode: input.gateway_mode.to_string(),
        request_id: input.request_id.to_string(),
        key_id: input.key_id.to_string(),
        key_name: input.key_name.to_string(),
        account_name: input.account_name.map(ToString::to_string),
        endpoint: input.endpoint.to_string(),
        prompt_hash: prompt_hash(input.prompt),
        size: input.size.map(ToString::to_string),
        quality: input.quality.map(ToString::to_string),
        n: input.n,
        input_image_count: input.input_images.len() as u64,
        input_data_image_count: input
            .input_images
            .iter()
            .filter(|source| source.to_ascii_lowercase().starts_with("data:image/"))
            .count() as u64,
        upstream_status: input.upstream.status,
        duration_ms: input.upstream.duration_ms,
        failover_count: input.upstream.failover_count,
        error_class: input.upstream.error_class.map(ToString::to_string),
        response_image_count: input.upstream.response_image_count,
        response_image_bytes: input.upstream.response_image_bytes,
        usage_tokens: input.upstream.usage_tokens,
        usage_missing: input.upstream.usage_missing,
    }
}

fn prompt_hash(prompt: &str) -> String {
    let digest = Sha256::digest(prompt.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use std::{fs, thread, time::Duration};

    use super::*;

    fn test_event(request_id: &str) -> ImageLogEvent {
        build_image_log_event(ImageLogInput {
            gateway_mode: "standalone",
            request_id,
            key_id: "key-1",
            key_name: "Key One",
            account_name: Some("codex-a"),
            endpoint: "generations",
            prompt: "draw a lake",
            size: Some("1024x1024"),
            quality: Some("high"),
            n: 1,
            input_images: &[],
            upstream: UpstreamLogInput {
                status: Some(200),
                duration_ms: 10,
                failover_count: 0,
                error_class: None,
                response_image_count: Some(1),
                response_image_bytes: Some(128),
                usage_tokens: Some(12),
                usage_missing: false,
            },
        })
    }

    fn sealed_logs(dir: &std::path::Path) -> Vec<PathBuf> {
        let mut files = fs::read_dir(dir)
            .expect("read log dir")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| {
                        name.starts_with("codex-image-")
                            && name.ends_with(".jsonl")
                            && name != "codex-image-active.jsonl"
                    })
            })
            .collect::<Vec<_>>();
        files.sort();
        files
    }

    #[test]
    fn image_log_writer_rotates_by_size_and_prunes_sealed_logs() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let mut writer = ImageLogWriter::new(ImageLogConfig {
            log_dir: temp.path().to_path_buf(),
            max_file_bytes: 1,
            max_file_age_ms: u64::MAX,
            max_files: 2,
        })
        .expect("writer");

        writer.append(&test_event("req-1")).expect("append 1");
        thread::sleep(Duration::from_millis(2));
        writer.append(&test_event("req-2")).expect("append 2");
        thread::sleep(Duration::from_millis(2));
        writer.append(&test_event("req-3")).expect("append 3");
        thread::sleep(Duration::from_millis(2));
        writer.append(&test_event("req-4")).expect("append 4");

        let sealed = sealed_logs(temp.path());
        assert_eq!(sealed.len(), 2);
        assert!(temp.path().join("codex-image-active.jsonl").exists());
        let sealed_names = sealed
            .iter()
            .filter_map(|path| path.file_name()?.to_str())
            .collect::<Vec<_>>();
        assert!(sealed_names
            .iter()
            .all(|name| *name != "codex-image-active.jsonl"));
    }

    #[test]
    fn image_log_writer_rotates_by_age() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let mut writer = ImageLogWriter::new(ImageLogConfig {
            log_dir: temp.path().to_path_buf(),
            max_file_bytes: u64::MAX,
            max_file_age_ms: 1,
            max_files: 4,
        })
        .expect("writer");

        writer.append(&test_event("req-1")).expect("append 1");
        writer.force_active_started_at_ms(0);
        writer.append(&test_event("req-2")).expect("append 2");

        assert_eq!(sealed_logs(temp.path()).len(), 1);
        assert!(temp.path().join("codex-image-active.jsonl").exists());
    }
}
