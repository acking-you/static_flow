use std::{
    fs::{self, OpenOptions},
    io::{BufWriter, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use serde::Serialize;
use sha2::{Digest, Sha256};

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
    /// SHA-256 hash of the prompt.
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
}

impl ImageLogWriter {
    /// Creates a writer and ensures the log directory exists.
    pub fn new(config: ImageLogConfig) -> anyhow::Result<Self> {
        fs::create_dir_all(&config.log_dir).with_context(|| {
            format!("create codex image log directory `{}`", config.log_dir.display())
        })?;
        Ok(Self {
            config,
            active_started_at_ms: now_ms(),
        })
    }

    /// Appends one redacted event as a JSON line.
    pub fn append(&mut self, event: &ImageLogEvent) -> anyhow::Result<()> {
        self.rotate_if_needed()?;
        let active_path = self.active_path();
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)
            .with_context(|| format!("open codex image log `{}`", active_path.display()))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer(&mut writer, event).context("serialize codex image log event")?;
        writer
            .write_all(b"\n")
            .context("write codex image log newline")?;
        writer.flush().context("flush codex image log event")?;
        Ok(())
    }

    fn rotate_if_needed(&mut self) -> anyhow::Result<()> {
        let active_path = self.active_path();
        let size_exceeded = active_path
            .metadata()
            .map(|metadata| metadata.len() >= self.config.max_file_bytes.max(1))
            .unwrap_or(false);
        let age_exceeded = now_ms().saturating_sub(self.active_started_at_ms)
            >= self.config.max_file_age_ms.max(1);
        if !size_exceeded && !age_exceeded {
            return Ok(());
        }
        if active_path.exists() {
            let sealed_path = self
                .config
                .log_dir
                .join(format!("codex-image-{}.jsonl", now_ms()));
            fs::rename(&active_path, &sealed_path).with_context(|| {
                format!(
                    "rotate codex image log `{}` to `{}`",
                    active_path.display(),
                    sealed_path.display()
                )
            })?;
        }
        self.active_started_at_ms = now_ms();
        self.prune_sealed_logs()
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
}

/// Builds a redacted log event that never stores raw prompts or base64 images.
pub fn build_image_log_event(input: ImageLogInput<'_>) -> ImageLogEvent {
    ImageLogEvent {
        schema_version: 1,
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

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
