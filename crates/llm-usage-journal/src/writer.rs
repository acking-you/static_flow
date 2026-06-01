//! Journal file writer.

use std::{
    fs::{self, File},
    io::{Cursor, Write},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use llm_access_core::usage::UsageEvent;

use crate::{
    config::JournalConfig,
    wire::{
        BlockHeaderV1, FileFooterV1, FileHeaderV1, JournalUsageBatchV1, JournalUsageEventV1,
        FILE_MAGIC_V1, FORMAT_VERSION_V1, SCHEMA_VERSION_V1,
    },
    writer_state::JournalWriterState,
};

pub(crate) const BLOCK_TAG: &[u8; 4] = b"BLK1";
pub(crate) const FOOTER_TAG: &[u8; 4] = b"FTR1";

/// Writer for active usage journal files.
#[derive(Debug)]
pub struct JournalWriter {
    config: JournalConfig,
    active_path: PathBuf,
    file: File,
    file_sequence: u64,
    created_at_ms: i64,
    block_sequence: u64,
    pending_events: Vec<JournalUsageEventV1>,
    event_count: u64,
    min_created_at_ms: Option<i64>,
    max_created_at_ms: Option<i64>,
    uncompressed_bytes: u64,
    compressed_bytes: u64,
}

impl JournalWriter {
    /// Open a journal writer rooted at the configured directory.
    pub fn open(config: JournalConfig) -> Result<Self> {
        fs::create_dir_all(config.root_dir.join("active")).with_context(|| {
            format!(
                "failed to create journal active dir `{}`",
                config.root_dir.join("active").display()
            )
        })?;
        fs::create_dir_all(config.root_dir.join("sealed")).with_context(|| {
            format!(
                "failed to create journal sealed dir `{}`",
                config.root_dir.join("sealed").display()
            )
        })?;
        fs::create_dir_all(config.root_dir.join("consuming")).with_context(|| {
            format!(
                "failed to create journal consuming dir `{}`",
                config.root_dir.join("consuming").display()
            )
        })?;

        let mut writer_state = JournalWriterState::open(&config.root_dir)?;
        let file_sequence = writer_state.allocate_next_file_sequence(&config.root_dir)?;
        let active_path = config
            .root_dir
            .join("active")
            .join(format!("usage-{file_sequence:012}.open"));
        let mut file = File::create(&active_path)
            .with_context(|| format!("failed to create journal `{}`", active_path.display()))?;
        let created_at_ms = now_ms();
        let header = FileHeaderV1 {
            magic: *FILE_MAGIC_V1,
            format_version: FORMAT_VERSION_V1,
            schema_version: SCHEMA_VERSION_V1,
            file_sequence,
            created_at_ms,
            writer_id: format!("pid-{}", std::process::id()),
            compression: "zstd".to_string(),
        };
        write_file_header(&mut file, &header)?;

        Ok(Self {
            config,
            active_path,
            file,
            file_sequence,
            created_at_ms,
            block_sequence: 0,
            pending_events: Vec::new(),
            event_count: 0,
            min_created_at_ms: None,
            max_created_at_ms: None,
            uncompressed_bytes: 0,
            compressed_bytes: 0,
        })
    }

    /// Append usage events to the active file, flushing blocks as thresholds
    /// are reached.
    pub fn append_events(&mut self, events: &[UsageEvent]) -> Result<()> {
        for event in events {
            if self.pending_events.len() >= self.config.block_max_events.max(1) {
                self.flush_pending_block()?;
            }
            self.pending_events
                .push(JournalUsageEventV1::from_usage_event(event));
            if self.pending_uncompressed_len()?
                >= self.config.block_target_uncompressed_bytes.max(1)
            {
                self.flush_pending_block()?;
            }
        }
        Ok(())
    }

    /// Flush pending events into a block.
    pub fn flush(&mut self) -> Result<()> {
        self.flush_pending_block()?;
        if self.config.fsync_interval_ms == 0 {
            self.file.sync_data().with_context(|| {
                format!("failed to sync journal `{}`", self.active_path.display())
            })?;
        }
        Ok(())
    }

    /// Current active journal file sequence.
    pub fn active_file_sequence(&self) -> u64 {
        self.file_sequence
    }

    /// Current active journal file path.
    pub fn active_path(&self) -> &std::path::Path {
        &self.active_path
    }

    /// Current active journal creation timestamp.
    pub fn created_at_ms(&self) -> i64 {
        self.created_at_ms
    }

    /// Current active journal file size.
    pub fn active_file_bytes(&self) -> Result<u64> {
        Ok(fs::metadata(&self.active_path)
            .with_context(|| format!("failed to stat journal `{}`", self.active_path.display()))?
            .len())
    }

    /// Return true once this writer has persisted at least one event block.
    pub fn has_written_events(&self) -> bool {
        self.event_count > 0
    }

    /// Seal the current file and return its sealed path.
    pub fn seal_current_file(mut self) -> Result<PathBuf> {
        self.flush_pending_block()?;
        let footer = FileFooterV1 {
            file_sequence: self.file_sequence,
            created_at_ms: self.created_at_ms,
            sealed_at_ms: now_ms(),
            event_count: self.event_count,
            block_count: self.block_sequence,
            min_created_at_ms: self.min_created_at_ms,
            max_created_at_ms: self.max_created_at_ms,
            uncompressed_bytes: self.uncompressed_bytes,
            compressed_bytes: self.compressed_bytes,
        };
        write_record(&mut self.file, FOOTER_TAG, &footer)?;
        self.file
            .sync_all()
            .with_context(|| format!("failed to sync journal `{}`", self.active_path.display()))?;
        drop(self.file);

        let sealed_path = self
            .config
            .root_dir
            .join("sealed")
            .join(format!("usage-{:012}.journal", self.file_sequence));
        fs::rename(&self.active_path, &sealed_path).with_context(|| {
            format!(
                "failed to seal journal `{}` to `{}`",
                self.active_path.display(),
                sealed_path.display()
            )
        })?;
        Ok(sealed_path)
    }

    fn pending_uncompressed_len(&self) -> Result<usize> {
        let batch = JournalUsageBatchV1 {
            events: self.pending_events.clone(),
        };
        Ok(postcard::to_allocvec(&batch)?.len())
    }

    fn flush_pending_block(&mut self) -> Result<()> {
        if self.pending_events.is_empty() {
            return Ok(());
        }
        let batch = JournalUsageBatchV1 {
            events: std::mem::take(&mut self.pending_events),
        };
        let uncompressed = postcard::to_allocvec(&batch)?;
        let compressed =
            zstd::stream::encode_all(Cursor::new(&uncompressed), self.config.zstd_level)
                .context("failed to zstd-compress usage journal block")?;
        let event_count = batch.events.len() as u32;
        let min_created_at_ms = batch
            .events
            .iter()
            .map(|event| event.created_at_ms)
            .min()
            .ok_or_else(|| anyhow!("usage journal block unexpectedly has no events"))?;
        let max_created_at_ms = batch
            .events
            .iter()
            .map(|event| event.created_at_ms)
            .max()
            .ok_or_else(|| anyhow!("usage journal block unexpectedly has no events"))?;
        let mut header = BlockHeaderV1 {
            block_sequence: self.block_sequence,
            event_count,
            min_created_at_ms,
            max_created_at_ms,
            uncompressed_len: uncompressed.len() as u64,
            compressed_len: compressed.len() as u64,
            crc32c: 0,
        };
        header.crc32c = block_crc32c(&header, &compressed)?;
        write_record(&mut self.file, BLOCK_TAG, &header)?;
        self.file
            .write_all(&compressed)
            .context("failed to write journal block payload")?;

        self.block_sequence = self.block_sequence.saturating_add(1);
        self.event_count = self.event_count.saturating_add(u64::from(event_count));
        self.min_created_at_ms = Some(
            self.min_created_at_ms
                .map_or(min_created_at_ms, |current| current.min(min_created_at_ms)),
        );
        self.max_created_at_ms = Some(
            self.max_created_at_ms
                .map_or(max_created_at_ms, |current| current.max(max_created_at_ms)),
        );
        self.uncompressed_bytes = self
            .uncompressed_bytes
            .saturating_add(uncompressed.len() as u64);
        self.compressed_bytes = self
            .compressed_bytes
            .saturating_add(compressed.len() as u64);
        Ok(())
    }
}

pub(crate) fn block_crc32c(header: &BlockHeaderV1, compressed_payload: &[u8]) -> Result<u32> {
    let mut header_without_crc = header.clone();
    header_without_crc.crc32c = 0;
    let mut bytes = postcard::to_allocvec(&header_without_crc)?;
    bytes.extend_from_slice(compressed_payload);
    Ok(crc32c::crc32c(&bytes))
}

pub(crate) fn write_record<T: serde::Serialize>(
    file: &mut File,
    tag: &[u8; 4],
    value: &T,
) -> Result<()> {
    let bytes = postcard::to_allocvec(value)?;
    file.write_all(tag)
        .with_context(|| format!("failed to write journal tag `{}`", tag_label(tag)))?;
    file.write_all(&(bytes.len() as u32).to_le_bytes())
        .context("failed to write journal record length")?;
    file.write_all(&bytes)
        .context("failed to write journal record bytes")?;
    Ok(())
}

pub(crate) fn write_file_header(file: &mut File, header: &FileHeaderV1) -> Result<()> {
    file.write_all(FILE_MAGIC_V1)
        .context("failed to write journal magic")?;
    let bytes = postcard::to_allocvec(header)?;
    file.write_all(&(bytes.len() as u32).to_le_bytes())
        .context("failed to write journal header length")?;
    file.write_all(&bytes)
        .context("failed to write journal header bytes")?;
    Ok(())
}

pub(crate) fn parse_sequence_from_file_name(file_name: &str) -> Option<u64> {
    let suffix = file_name.strip_prefix("usage-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn tag_label(tag: &[u8; 4]) -> String {
    String::from_utf8_lossy(tag).into_owned()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };

    use super::JournalWriter;
    use crate::{reader::JournalReader, retention, JournalConfig};

    #[test]
    fn writer_seals_file_with_valid_footer_and_reader_streams_batches_and_summary() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = JournalConfig::new(dir.path().to_path_buf());
        let mut writer = JournalWriter::open(config).expect("open writer");
        writer
            .append_events(&[test_usage_event("evt-journal-1")])
            .expect("append");
        let sealed = writer.seal_current_file().expect("seal");
        let reader = JournalReader::open(&sealed).expect("open reader");
        let summary = reader.scan_summary().expect("scan summary");
        assert_eq!(summary.footer.block_count, 1);
        assert_eq!(summary.footer.event_count, 1);
        assert!(summary.total_compressed_bytes > 0);

        let mut stream = reader.stream_batches().expect("stream");
        let batch = stream
            .next_batch()
            .expect("read batch")
            .expect("batch present");
        assert_eq!(batch.events[0].event_id, "evt-journal-1");
        assert!(stream.next_batch().expect("read footer").is_none());
        let report = stream.finish().expect("finish stream");
        assert_eq!(report.footer.block_count, 1);
        assert_eq!(report.footer.event_count, 1);
        assert_eq!(report.total_compressed_bytes, summary.total_compressed_bytes);
        assert_eq!(report.file_digest_hex.len(), 64);
    }

    #[test]
    fn reader_rejects_corrupted_block_crc() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = JournalConfig::new(dir.path().to_path_buf());
        let mut writer = JournalWriter::open(config).expect("open writer");
        writer
            .append_events(&[test_usage_event("evt-corrupt")])
            .expect("append");
        let sealed = writer.seal_current_file().expect("seal");
        corrupt_one_payload_byte(&sealed);

        let err = JournalReader::open(&sealed)
            .and_then(|reader| reader.read_all_batches())
            .expect_err("crc must fail");
        assert!(err.to_string().contains("crc"));
    }

    #[test]
    fn retention_deletes_oldest_sealed_file_but_keeps_active_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = JournalConfig {
            max_files: 1,
            ..JournalConfig::new(dir.path().to_path_buf())
        };
        create_file(dir.path(), "sealed/usage-000000000001.journal", b"old");
        create_file(dir.path(), "sealed/usage-000000000002.journal", b"new");
        create_file(dir.path(), "active/usage-000000000003.open", b"active");

        let report = retention::enforce_retention(&config).expect("retention");

        assert_eq!(report.deleted_files, 1);
        assert!(!dir
            .path()
            .join("sealed/usage-000000000001.journal")
            .exists());
        assert!(dir.path().join("active/usage-000000000003.open").exists());
    }

    fn corrupt_one_payload_byte(path: &Path) {
        let mut bytes = fs::read(path).expect("read sealed");
        let block_tag = bytes
            .windows(super::BLOCK_TAG.len())
            .position(|window| window == super::BLOCK_TAG)
            .expect("block tag");
        let header_len_start = block_tag + super::BLOCK_TAG.len();
        let header_len = u32::from_le_bytes(
            bytes[header_len_start..header_len_start + 4]
                .try_into()
                .expect("header len"),
        ) as usize;
        let payload_start = header_len_start + 4 + header_len;
        bytes[payload_start] ^= 0x01;
        fs::write(path, bytes).expect("write corrupt");
    }

    fn create_file(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
        fs::write(path, bytes).expect("write file");
    }

    fn test_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-1".to_string(),
            key_name: "for-yangshu".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: Some("group-1".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            mapped_model: Some("claude-opus-4-7".to_string()),
            status_code: 200,
            request_body_bytes: Some(17),
            quota_failover_count: 0,
            routing_diagnostics_json: Some("{\"route\":\"fixed\"}".to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.12".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"user-agent\":\"test\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"model\":\"m\"}".to_string()),
            upstream_request_body_json: Some("{\"upstream\":true}".to_string()),
            full_request_json: Some("{\"model\":\"m\"}".to_string()),
            error_message: None,
            error_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
