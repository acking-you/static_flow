//! Read-only preview helpers for active producer journal files.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

use crate::{
    wire::{
        decode_journal_usage_batch, BlockHeaderV1, FileHeaderV1, JournalUsageBatchV1,
        JournalUsageEventV1, FILE_MAGIC_V1,
    },
    writer::{block_crc32c, BLOCK_TAG, FOOTER_TAG},
};

/// Preview result for one producer journal file.
#[derive(Debug, Clone, PartialEq)]
pub struct JournalPreviewReport {
    /// File path on disk.
    pub path: PathBuf,
    /// File sequence parsed from the header.
    pub file_sequence: u64,
    /// Bytes successfully scanned, including validated block payloads.
    pub bytes_scanned: u64,
    /// Number of complete blocks decoded.
    pub complete_blocks: u64,
    /// True when the file ended with a partial trailing record.
    pub truncated_tail: bool,
    /// Most recent events retained by the preview limit.
    pub events: Vec<JournalUsageEventV1>,
    /// Total decoded events across complete blocks.
    pub total_events: usize,
}

/// Best-effort reader for a producer-side active journal file.
///
/// Unlike [`crate::reader::JournalReader`], this reader does not require a
/// sealed footer. It stops at the first incomplete trailing record and returns
/// all previously validated blocks.
#[derive(Debug)]
pub struct JournalPreviewReader {
    path: PathBuf,
}

impl JournalPreviewReader {
    /// Open an active producer journal file for preview reads.
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Read the most recent `event_limit` events from a producer journal file.
    pub fn read_last_events(&self, event_limit: usize) -> Result<JournalPreviewReport> {
        self.read_recent_events_page(event_limit, 0)
    }

    /// Read a recent events page from a producer journal file.
    ///
    /// `offset` counts backwards from the newest event: `offset=0` returns the
    /// newest page, `offset=limit` returns the next older page.
    pub fn read_recent_events_page(
        &self,
        event_limit: usize,
        offset: usize,
    ) -> Result<JournalPreviewReport> {
        let mut file = File::open(&self.path)
            .with_context(|| format!("failed to open journal preview `{}`", self.path.display()))?;
        let header = read_file_header(&mut file)?;
        let mut bytes_scanned = 8_u64;
        let mut complete_blocks = 0_u64;
        let mut truncated_tail = false;
        let mut all_events = Vec::new();

        loop {
            match read_record_tag_preview(&mut file)? {
                PreviewTag::CleanEof => break,
                PreviewTag::Truncated => {
                    truncated_tail = true;
                    break;
                },
                PreviewTag::Tag(tag) if tag == *FOOTER_TAG => break,
                PreviewTag::Tag(tag) if tag == *BLOCK_TAG => {
                    let Some(header_bytes) = read_len_prefixed_payload_preview(&mut file)? else {
                        truncated_tail = true;
                        break;
                    };
                    bytes_scanned = bytes_scanned
                        .saturating_add(4)
                        .saturating_add(4)
                        .saturating_add(header_bytes.len() as u64);
                    let block_header: BlockHeaderV1 = postcard::from_bytes(&header_bytes)
                        .context("failed to decode preview block header")?;
                    let Some(compressed) =
                        read_exact_bytes_preview(&mut file, block_header.compressed_len as usize)?
                    else {
                        truncated_tail = true;
                        break;
                    };
                    bytes_scanned = bytes_scanned.saturating_add(block_header.compressed_len);
                    let actual_crc = block_crc32c(&block_header, &compressed)?;
                    if actual_crc != block_header.crc32c {
                        return Err(anyhow!(
                            "usage journal preview block crc mismatch: expected {}, got {}",
                            block_header.crc32c,
                            actual_crc
                        ));
                    }
                    let decoded = zstd::stream::decode_all(std::io::Cursor::new(&compressed))
                        .context("failed to decode preview block payload")?;
                    if decoded.len() != block_header.uncompressed_len as usize {
                        return Err(anyhow!(
                            "usage journal preview block length mismatch: expected {}, got {}",
                            block_header.uncompressed_len,
                            decoded.len()
                        ));
                    }
                    let batch: JournalUsageBatchV1 = decode_journal_usage_batch(&decoded)?;
                    complete_blocks = complete_blocks.saturating_add(1);
                    all_events.extend(batch.events);
                },
                PreviewTag::Tag(tag) => {
                    return Err(anyhow!(
                        "unexpected usage journal preview record tag `{}`",
                        String::from_utf8_lossy(&tag)
                    ));
                },
            }
        }

        let total_events = all_events.len();
        let end = total_events.saturating_sub(offset);
        let start = end.saturating_sub(event_limit);
        let events = if start >= end { Vec::new() } else { all_events.drain(start..end).collect() };

        Ok(JournalPreviewReport {
            path: self.path.clone(),
            file_sequence: header.file_sequence,
            bytes_scanned,
            complete_blocks,
            truncated_tail,
            events,
            total_events,
        })
    }
}

fn read_file_header(file: &mut File) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .context("failed to read journal preview magic")?;
    if &magic != FILE_MAGIC_V1 {
        return Err(anyhow!("invalid usage journal preview magic"));
    }
    let payload = read_len_prefixed_payload_preview(file)?
        .ok_or_else(|| anyhow!("usage journal preview ended before file header"))?;
    postcard::from_bytes(&payload).map_err(anyhow::Error::from)
}

enum PreviewTag {
    CleanEof,
    Truncated,
    Tag([u8; 4]),
}

fn read_record_tag_preview(file: &mut File) -> Result<PreviewTag> {
    let mut tag = [0u8; 4];
    match file.read(&mut tag) {
        Ok(0) => Ok(PreviewTag::CleanEof),
        Ok(4) => Ok(PreviewTag::Tag(tag)),
        Ok(_) => Ok(PreviewTag::Truncated),
        Err(err) => Err(err).context("failed to read preview record tag"),
    }
}

fn read_len_prefixed_payload_preview(file: &mut File) -> Result<Option<Vec<u8>>> {
    let Some(len_bytes) = read_exact_bytes_preview(file, 4)? else {
        return Ok(None);
    };
    let len = u32::from_le_bytes(
        len_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("invalid preview payload length prefix"))?,
    ) as usize;
    read_exact_bytes_preview(file, len)
}

fn read_exact_bytes_preview(file: &mut File, len: usize) -> Result<Option<Vec<u8>>> {
    let mut bytes = vec![0u8; len];
    let mut offset = 0usize;
    while offset < len {
        match file.read(&mut bytes[offset..]) {
            Ok(0) => return Ok(None),
            Ok(read) => offset += read,
            Err(err) => return Err(err).context("failed to read preview payload"),
        }
    }
    Ok(Some(bytes))
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write};

    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };
    use tempfile::tempdir;

    use crate::{config::JournalConfig, writer::JournalWriter};

    fn sample_event(event_id: &str, created_at_ms: i64) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: "key-preview".to_string(),
            key_name: "preview".to_string(),
            account_name: Some("preview-account".to_string()),
            account_group_id_at_event: None,
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/chat/completions".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: Some("gpt-5.3-codex".to_string()),
            mapped_model: None,
            status_code: 200,
            request_body_bytes: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 1,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 2,
            credit_usage: None,
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "unknown".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: Some(event_id.to_string()),
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }

    #[test]
    fn preview_reader_returns_complete_blocks_from_active_file_without_footer() {
        let root = tempdir().expect("tempdir");
        let config = JournalConfig::new(root.path().to_path_buf());
        let mut writer = JournalWriter::open(config).expect("writer");
        writer
            .append_events(&[
                sample_event("evt-1", 1),
                sample_event("evt-2", 2),
                sample_event("evt-3", 3),
            ])
            .expect("append events");
        writer.flush().expect("flush");
        let path = writer.active_path().to_path_buf();

        let preview = super::JournalPreviewReader::open(&path)
            .expect("preview open")
            .read_last_events(2)
            .expect("preview read");

        assert_eq!(preview.file_sequence, writer.active_file_sequence());
        assert_eq!(preview.complete_blocks, 1);
        assert!(!preview.truncated_tail);
        assert_eq!(preview.events.len(), 2);
        assert_eq!(preview.events[0].event_id, "evt-2");
        assert_eq!(preview.events[1].event_id, "evt-3");
    }

    #[test]
    fn preview_reader_stops_before_partial_trailing_block() {
        let root = tempdir().expect("tempdir");
        let config = JournalConfig::new(root.path().to_path_buf());
        let mut writer = JournalWriter::open(config).expect("writer");
        writer
            .append_events(&[sample_event("evt-1", 1), sample_event("evt-2", 2)])
            .expect("append events");
        writer.flush().expect("flush");
        let path = writer.active_path().to_path_buf();

        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open active file");
        file.write_all(b"BLK1").expect("write partial tag");
        file.flush().expect("flush partial tag");
        drop(file);

        let preview = super::JournalPreviewReader::open(&path)
            .expect("preview open")
            .read_last_events(10)
            .expect("preview read");

        assert_eq!(preview.complete_blocks, 1);
        assert!(preview.truncated_tail);
        assert_eq!(preview.events.len(), 2);
        assert_eq!(preview.events[0].event_id, "evt-1");
        assert_eq!(preview.events[1].event_id, "evt-2");
    }

    #[test]
    fn preview_reader_pages_back_from_newest_events() {
        let root = tempdir().expect("tempdir");
        let config = JournalConfig::new(root.path().to_path_buf());
        let mut writer = JournalWriter::open(config).expect("writer");
        writer
            .append_events(&[
                sample_event("evt-1", 1),
                sample_event("evt-2", 2),
                sample_event("evt-3", 3),
                sample_event("evt-4", 4),
            ])
            .expect("append events");
        writer.flush().expect("flush");
        let path = writer.active_path().to_path_buf();

        let preview = super::JournalPreviewReader::open(&path)
            .expect("preview open")
            .read_recent_events_page(2, 1)
            .expect("preview read");

        assert_eq!(preview.total_events, 4);
        assert_eq!(preview.events.len(), 2);
        assert_eq!(preview.events[0].event_id, "evt-2");
        assert_eq!(preview.events[1].event_id, "evt-3");
    }
}
