//! Recovery helpers for orphan producer-side active journal files.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};

use crate::{
    config::JournalConfig,
    wire::{BlockHeaderV1, FileFooterV1, FileHeaderV1, FILE_MAGIC_V1},
    writer::{
        block_crc32c, parse_sequence_from_file_name, write_file_header, write_record, BLOCK_TAG,
        FOOTER_TAG,
    },
    writer_state::JournalWriterState,
};

/// Summary of one orphan-active recovery pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActiveRecoveryReport {
    /// Number of orphan active files recovered into sealed files.
    pub recovered_files: u64,
    /// Number of empty header-only orphan files deleted.
    pub deleted_empty_files: u64,
    /// Number of corrupt orphan files moved aside or deleted.
    pub quarantined_files: u64,
}

/// Recover orphan active files under one journal root.
pub fn recover_orphan_active_files(config: &JournalConfig) -> Result<ActiveRecoveryReport> {
    let active_dir = config.root_dir.join("active");
    if !active_dir.exists() {
        return Ok(ActiveRecoveryReport::default());
    }
    fs::create_dir_all(config.root_dir.join("sealed")).with_context(|| {
        format!(
            "failed to create recovery sealed dir `{}`",
            config.root_dir.join("sealed").display()
        )
    })?;
    fs::create_dir_all(config.root_dir.join("bad")).with_context(|| {
        format!("failed to create recovery bad dir `{}`", config.root_dir.join("bad").display())
    })?;

    let mut active_paths = Vec::new();
    for entry in fs::read_dir(&active_dir)
        .with_context(|| format!("failed to read active journal dir `{}`", active_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat active journal `{}`", path.display()))?;
        if !metadata.is_file() {
            continue;
        }
        active_paths.push(path);
    }
    active_paths.sort_by(|left, right| {
        let left_name = left
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        let right_name = right
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_default();
        parse_sequence_from_file_name(&left_name)
            .cmp(&parse_sequence_from_file_name(&right_name))
            .then_with(|| left_name.cmp(&right_name))
    });

    let mut writer_state = JournalWriterState::open(&config.root_dir)?;
    let mut report = ActiveRecoveryReport::default();
    for path in active_paths {
        match recover_one_active_file(&path, config, &mut writer_state) {
            Ok(RecoveryOutcome::Recovered {
                sealed_path,
            }) => {
                report.recovered_files = report.recovered_files.saturating_add(1);
                tracing::info!(
                    path = %path.display(),
                    sealed_path = %sealed_path.display(),
                    "recovered orphan active usage journal into sealed backlog"
                );
            },
            Ok(RecoveryOutcome::DeletedEmpty) => {
                report.deleted_empty_files = report.deleted_empty_files.saturating_add(1);
            },
            Ok(RecoveryOutcome::Quarantined {
                bad_path,
            }) => {
                report.quarantined_files = report.quarantined_files.saturating_add(1);
                tracing::warn!(
                    path = %path.display(),
                    bad_path = %bad_path.display(),
                    "quarantined corrupt orphan active usage journal"
                );
            },
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to recover orphan active journal `{}`", path.display())
                });
            },
        }
    }
    Ok(report)
}

enum RecoveryOutcome {
    Recovered { sealed_path: PathBuf },
    DeletedEmpty,
    Quarantined { bad_path: PathBuf },
}

fn recover_one_active_file(
    source_path: &Path,
    config: &JournalConfig,
    writer_state: &mut JournalWriterState,
) -> Result<RecoveryOutcome> {
    match try_recover_one_active_file(source_path, config, writer_state) {
        Ok(outcome) => Ok(outcome),
        Err(err) => quarantine_or_delete_active_file(source_path, config, err),
    }
}

fn try_recover_one_active_file(
    source_path: &Path,
    config: &JournalConfig,
    writer_state: &mut JournalWriterState,
) -> Result<RecoveryOutcome> {
    let mut source = File::open(source_path)
        .with_context(|| format!("failed to open `{}`", source_path.display()))?;
    let source_header = read_file_header(&mut source)?;
    let mut output = None::<RecoveredOutput>;
    let mut block_count = 0u64;
    let mut event_count = 0u64;
    let mut min_created_at_ms = None;
    let mut max_created_at_ms = None;
    let mut uncompressed_bytes = 0u64;
    let mut compressed_bytes = 0u64;

    let result = loop {
        match read_record_tag(&mut source, source_path)? {
            NextTag::CleanEof => {
                if block_count == 0 {
                    fs::remove_file(source_path).with_context(|| {
                        format!("failed to delete empty orphan active `{}`", source_path.display())
                    })?;
                    break Ok(RecoveryOutcome::DeletedEmpty);
                }
                let output = output.take().ok_or_else(|| {
                    anyhow!("missing recovered output for `{}`", source_path.display())
                })?;
                let sealed_path =
                    finalize_recovered_output(output, source_path, RecoveredFooterSummary {
                        block_count,
                        event_count,
                        min_created_at_ms,
                        max_created_at_ms,
                        uncompressed_bytes,
                        compressed_bytes,
                        created_at_ms: source_header.created_at_ms,
                    })?;
                break Ok(RecoveryOutcome::Recovered {
                    sealed_path,
                });
            },
            NextTag::Tag(tag) if tag == *BLOCK_TAG => {
                let block_header_bytes =
                    read_len_prefixed_payload(&mut source, "failed to read active block header")?;
                let block_header: BlockHeaderV1 = postcard::from_bytes(&block_header_bytes)
                    .context("failed to decode active block header")?;
                let compressed_payload = read_exact_bytes(
                    &mut source,
                    block_header.compressed_len as usize,
                    "failed to read active block payload",
                )?;
                let actual_crc = block_crc32c(&block_header, &compressed_payload)?;
                if actual_crc != block_header.crc32c {
                    anyhow::bail!(
                        "orphan active block crc mismatch: expected {}, got {}",
                        block_header.crc32c,
                        actual_crc
                    );
                }
                if output.is_none() {
                    output = Some(create_recovered_output(
                        source_path,
                        config,
                        writer_state,
                        &source_header,
                    )?);
                }
                let output = output.as_mut().ok_or_else(|| {
                    anyhow!("missing recovered output for `{}`", source_path.display())
                })?;
                write_raw_record(
                    &mut output.file,
                    BLOCK_TAG,
                    &block_header_bytes,
                    &compressed_payload,
                )?;
                block_count = block_count.saturating_add(1);
                event_count = event_count.saturating_add(block_header.event_count as u64);
                min_created_at_ms = Some(
                    min_created_at_ms.map_or(block_header.min_created_at_ms, |current: i64| {
                        current.min(block_header.min_created_at_ms)
                    }),
                );
                max_created_at_ms = Some(
                    max_created_at_ms.map_or(block_header.max_created_at_ms, |current: i64| {
                        current.max(block_header.max_created_at_ms)
                    }),
                );
                uncompressed_bytes =
                    uncompressed_bytes.saturating_add(block_header.uncompressed_len);
                compressed_bytes = compressed_bytes.saturating_add(block_header.compressed_len);
            },
            NextTag::Tag(tag) if tag == *FOOTER_TAG => {
                let _footer_bytes =
                    read_len_prefixed_payload(&mut source, "failed to read active footer")?;
                if block_count == 0 {
                    fs::remove_file(source_path).with_context(|| {
                        format!(
                            "failed to delete empty active-with-footer `{}`",
                            source_path.display()
                        )
                    })?;
                    break Ok(RecoveryOutcome::DeletedEmpty);
                }
                let output = output.take().ok_or_else(|| {
                    anyhow!("missing recovered output for `{}`", source_path.display())
                })?;
                let sealed_path =
                    finalize_recovered_output(output, source_path, RecoveredFooterSummary {
                        block_count,
                        event_count,
                        min_created_at_ms,
                        max_created_at_ms,
                        uncompressed_bytes,
                        compressed_bytes,
                        created_at_ms: source_header.created_at_ms,
                    })?;
                break Ok(RecoveryOutcome::Recovered {
                    sealed_path,
                });
            },
            NextTag::Tag(tag) => {
                anyhow::bail!(
                    "unexpected orphan active record tag `{}` in `{}`",
                    String::from_utf8_lossy(&tag),
                    source_path.display()
                );
            },
        }
    };
    if result.is_err() {
        if let Some(output) = output {
            let _ = fs::remove_file(output.temp_path);
        }
    }
    result
}

struct RecoveredOutput {
    file: File,
    temp_path: PathBuf,
    sealed_path: PathBuf,
    sequence: u64,
}

struct RecoveredFooterSummary {
    block_count: u64,
    event_count: u64,
    min_created_at_ms: Option<i64>,
    max_created_at_ms: Option<i64>,
    uncompressed_bytes: u64,
    compressed_bytes: u64,
    created_at_ms: i64,
}

fn create_recovered_output(
    source_path: &Path,
    config: &JournalConfig,
    writer_state: &mut JournalWriterState,
    source_header: &FileHeaderV1,
) -> Result<RecoveredOutput> {
    let sequence = writer_state.allocate_next_file_sequence(&config.root_dir)?;
    let temp_path = config
        .root_dir
        .join("sealed")
        .join(format!("usage-{sequence:012}.recovering"));
    let sealed_path = config
        .root_dir
        .join("sealed")
        .join(format!("usage-{sequence:012}.journal"));
    let mut file = File::create(&temp_path).with_context(|| {
        format!(
            "failed to create recovered journal temp `{}` from `{}`",
            temp_path.display(),
            source_path.display()
        )
    })?;
    write_file_header(&mut file, &FileHeaderV1 {
        magic: *FILE_MAGIC_V1,
        format_version: source_header.format_version,
        schema_version: source_header.schema_version,
        file_sequence: sequence,
        created_at_ms: source_header.created_at_ms,
        writer_id: source_header.writer_id.clone(),
        compression: source_header.compression.clone(),
    })?;
    Ok(RecoveredOutput {
        file,
        temp_path,
        sealed_path,
        sequence,
    })
}

fn finalize_recovered_output(
    mut output: RecoveredOutput,
    source_path: &Path,
    summary: RecoveredFooterSummary,
) -> Result<PathBuf> {
    let footer = FileFooterV1 {
        file_sequence: output.sequence,
        created_at_ms: summary.created_at_ms,
        sealed_at_ms: now_ms(),
        event_count: summary.event_count,
        block_count: summary.block_count,
        min_created_at_ms: summary.min_created_at_ms,
        max_created_at_ms: summary.max_created_at_ms,
        uncompressed_bytes: summary.uncompressed_bytes,
        compressed_bytes: summary.compressed_bytes,
    };
    write_record(&mut output.file, FOOTER_TAG, &footer)?;
    output.file.sync_all().with_context(|| {
        format!("failed to sync recovered journal temp `{}`", output.temp_path.display())
    })?;
    drop(output.file);
    fs::rename(&output.temp_path, &output.sealed_path).with_context(|| {
        format!(
            "failed to publish recovered journal `{}` as `{}`",
            output.temp_path.display(),
            output.sealed_path.display()
        )
    })?;
    fs::remove_file(source_path).with_context(|| {
        format!("failed to remove recovered orphan active source `{}`", source_path.display())
    })?;
    Ok(output.sealed_path)
}

fn quarantine_or_delete_active_file(
    source_path: &Path,
    config: &JournalConfig,
    err: anyhow::Error,
) -> Result<RecoveryOutcome> {
    if config.delete_bad_files {
        fs::remove_file(source_path).with_context(|| {
            format!("failed to delete corrupt orphan active journal `{}`", source_path.display())
        })?;
        tracing::warn!(
            path = %source_path.display(),
            "deleted corrupt orphan active usage journal during recovery: {err:#}"
        );
        return Ok(RecoveryOutcome::Quarantined {
            bad_path: source_path.to_path_buf(),
        });
    }

    let bad_path = next_bad_path(source_path, &config.root_dir);
    fs::rename(source_path, &bad_path).with_context(|| {
        format!(
            "failed to quarantine corrupt orphan active journal `{}` to `{}`",
            source_path.display(),
            bad_path.display()
        )
    })?;
    tracing::warn!(
        path = %source_path.display(),
        bad_path = %bad_path.display(),
        "quarantined corrupt orphan active usage journal during recovery: {err:#}"
    );
    Ok(RecoveryOutcome::Quarantined {
        bad_path,
    })
}

fn next_bad_path(source_path: &Path, root_dir: &Path) -> PathBuf {
    let file_name = source_path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "unknown-active.open".to_string());
    let timestamp = now_ms().max(0);
    root_dir
        .join("bad")
        .join(format!("{timestamp}-{file_name}"))
}

enum NextTag {
    CleanEof,
    Tag([u8; 4]),
}

fn read_record_tag(file: &mut File, source_path: &Path) -> Result<NextTag> {
    let mut first = [0u8; 1];
    match file.read(&mut first).with_context(|| {
        format!("failed to read orphan active record tag prefix from `{}`", source_path.display())
    })? {
        0 => Ok(NextTag::CleanEof),
        1 => {
            let mut rest = [0u8; 3];
            file.read_exact(&mut rest).with_context(|| {
                format!("orphan active journal `{}` ended mid-record-tag", source_path.display())
            })?;
            Ok(NextTag::Tag([first[0], rest[0], rest[1], rest[2]]))
        },
        _ => unreachable!(),
    }
}

fn read_file_header(file: &mut File) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .context("failed to read orphan active journal magic")?;
    if &magic != FILE_MAGIC_V1 {
        return Err(anyhow!("invalid orphan active journal magic"));
    }
    let bytes = read_len_prefixed_payload(file, "failed to read orphan active header")?;
    postcard::from_bytes(&bytes).context("failed to decode orphan active header")
}

fn read_len_prefixed_payload(file: &mut File, context: &'static str) -> Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    file.read_exact(&mut len_bytes).context(context)?;
    let len = u32::from_le_bytes(len_bytes) as usize;
    read_exact_bytes(file, len, context)
}

fn read_exact_bytes(file: &mut File, len: usize, context: &'static str) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes).context(context)?;
    Ok(bytes)
}

fn write_raw_record(
    file: &mut File,
    tag: &[u8; 4],
    header_bytes: &[u8],
    payload_bytes: &[u8],
) -> Result<()> {
    file.write_all(tag)
        .context("failed to write recovered journal block tag")?;
    file.write_all(&(header_bytes.len() as u32).to_le_bytes())
        .context("failed to write recovered journal block header length")?;
    file.write_all(header_bytes)
        .context("failed to write recovered journal block header bytes")?;
    file.write_all(payload_bytes)
        .context("failed to write recovered journal block payload bytes")?;
    Ok(())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };
    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::recover_orphan_active_files;
    use crate::{JournalConfig, JournalReader, JournalWriter};

    #[test]
    fn recovery_moves_orphan_active_into_new_sealed_sequence_above_consumed_history() {
        let root = tempdir().expect("tempdir");
        let config = JournalConfig::new(root.path().to_path_buf());
        let mut orphan = JournalWriter::open(config.clone()).expect("writer");
        orphan
            .append_events(&[test_usage_event("evt-recovered")])
            .expect("append");
        orphan.flush().expect("flush");
        drop(orphan);
        std::fs::remove_file(root.path().join("writer-state.sqlite3")).expect("drop writer state");

        let conn = Connection::open(root.path().join("consumer-state.sqlite3")).expect("sqlite");
        conn.execute_batch(
            "CREATE TABLE consumed_files (
                file_sequence INTEGER PRIMARY KEY,
                file_digest TEXT NOT NULL,
                event_count INTEGER NOT NULL,
                imported_at_ms INTEGER NOT NULL
             ) STRICT, WITHOUT ROWID;",
        )
        .expect("schema");
        conn.execute(
            "INSERT INTO consumed_files (file_sequence, file_digest, event_count, imported_at_ms)
             VALUES (234, 'digest', 1, 1)",
            [],
        )
        .expect("row");
        drop(conn);

        let report = recover_orphan_active_files(&config).expect("recover");
        assert_eq!(report.recovered_files, 1);
        let sealed_entries = std::fs::read_dir(root.path().join("sealed"))
            .expect("read sealed")
            .map(|entry| entry.expect("entry").path())
            .collect::<Vec<_>>();
        assert_eq!(sealed_entries.len(), 1);
        let sealed_name = sealed_entries[0]
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(sealed_name.starts_with("usage-000000000235"));
        let reader = JournalReader::open(&sealed_entries[0]).expect("reader");
        let summary = reader.scan_summary().expect("summary");
        assert_eq!(summary.footer.file_sequence, 235);
        assert_eq!(summary.footer.event_count, 1);
    }

    #[test]
    fn recovery_deletes_empty_header_only_orphan_active() {
        let root = tempdir().expect("tempdir");
        let config = JournalConfig::new(root.path().to_path_buf());
        let orphan = JournalWriter::open(config.clone()).expect("writer");
        drop(orphan);

        let report = recover_orphan_active_files(&config).expect("recover");
        assert_eq!(report.deleted_empty_files, 1);
        let active_count = std::fs::read_dir(root.path().join("active"))
            .expect("read active")
            .count();
        assert_eq!(active_count, 0);
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
            timing: UsageTiming {
                latency_ms: Some(123),
                routing_wait_ms: Some(1),
                upstream_headers_ms: Some(2),
                post_headers_body_ms: Some(3),
                request_body_read_ms: Some(4),
                request_json_parse_ms: Some(5),
                pre_handler_ms: Some(6),
                first_sse_write_ms: Some(7),
                stream_finish_ms: Some(8),
            },
            stream: UsageStreamDetails {
                stream_completed_cleanly: Some(true),
                downstream_disconnect: Some(false),
                final_event_type: Some("message_stop".to_string()),
                bytes_streamed: Some(9),
            },
        }
    }
}
