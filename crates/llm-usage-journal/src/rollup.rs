//! Journal writer and reader for durable control-plane usage rollups.

use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{Cursor, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context, Result};
use llm_access_core::store::UsageRollupBatch;
use sha2::{Digest, Sha256};

use crate::{
    config::JournalConfig,
    reader::{JournalFileSummary, JournalStreamReport},
    wire::{
        decode_journal_rollup_batch_block, BlockHeaderV1, FileFooterV1, FileHeaderV1,
        JournalRollupBatchBlockV1, JournalRollupBatchV1, FORMAT_VERSION_V1, ROLLUP_FILE_MAGIC_V1,
        SCHEMA_VERSION_V1,
    },
    writer::{block_crc32c, write_file_header, write_record, BLOCK_TAG, FOOTER_TAG},
};

/// Summary of rollup active-file recovery before opening a producer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RollupActiveRecoveryReport {
    /// Non-empty active files recovered into sealed files.
    pub recovered_files: u64,
    /// Empty active files deleted.
    pub deleted_empty_files: u64,
    /// Corrupt active files moved to `bad/`.
    pub quarantined_files: u64,
}

/// Writer for active control-rollup journal files.
#[derive(Debug)]
pub struct RollupJournalWriter {
    config: JournalConfig,
    active_path: PathBuf,
    file: File,
    file_sequence: u64,
    created_at_ms: i64,
    block_sequence: u64,
    pending_batches: Vec<JournalRollupBatchV1>,
    pending_uncompressed_bytes: usize,
    batch_count: u64,
    min_created_at_ms: Option<i64>,
    max_created_at_ms: Option<i64>,
    uncompressed_bytes: u64,
    compressed_bytes: u64,
}

impl RollupJournalWriter {
    /// Open a control-rollup journal writer rooted at the configured directory.
    pub fn open(config: JournalConfig) -> Result<Self> {
        create_journal_dirs(&config.root_dir)?;
        let file_sequence = next_rollup_sequence(&config.root_dir)?;
        let active_path = config
            .root_dir
            .join("active")
            .join(format!("rollup-{file_sequence:012}.open"));
        let mut file = File::create(&active_path).with_context(|| {
            format!("failed to create rollup journal `{}`", active_path.display())
        })?;
        let created_at_ms = now_ms();
        let header = FileHeaderV1 {
            magic: *ROLLUP_FILE_MAGIC_V1,
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
            pending_batches: Vec::new(),
            pending_uncompressed_bytes: 0,
            batch_count: 0,
            min_created_at_ms: None,
            max_created_at_ms: None,
            uncompressed_bytes: 0,
            compressed_bytes: 0,
        })
    }

    /// Append rollup batches to the active file, flushing blocks as thresholds
    /// are reached.
    pub fn append_batches(&mut self, batches: &[UsageRollupBatch]) -> Result<()> {
        for batch in batches {
            if self.pending_batches.len() >= self.config.block_max_events.max(1) {
                self.flush_pending_block()?;
            }
            let batch = JournalRollupBatchV1::from_rollup_batch(batch);
            self.pending_uncompressed_bytes = self
                .pending_uncompressed_bytes
                .saturating_add(postcard::to_allocvec(&batch)?.len());
            self.pending_batches.push(batch);
            if self.pending_uncompressed_bytes >= self.config.block_target_uncompressed_bytes.max(1)
            {
                self.flush_pending_block()?;
            }
        }
        Ok(())
    }

    /// Flush pending batches into a block.
    pub fn flush(&mut self) -> Result<()> {
        self.flush_pending_block()?;
        if self.config.fsync_interval_ms == 0 {
            self.file.sync_data().with_context(|| {
                format!("failed to sync rollup journal `{}`", self.active_path.display())
            })?;
        }
        Ok(())
    }

    /// Current active rollup journal file sequence.
    pub fn active_file_sequence(&self) -> u64 {
        self.file_sequence
    }

    /// Current active rollup journal file path.
    pub fn active_path(&self) -> &Path {
        &self.active_path
    }

    /// Current active rollup journal file size.
    pub fn active_file_bytes(&self) -> Result<u64> {
        Ok(fs::metadata(&self.active_path)
            .with_context(|| {
                format!("failed to stat rollup journal `{}`", self.active_path.display())
            })?
            .len())
    }

    /// Return true once this writer has persisted at least one batch block.
    pub fn has_written_batches(&self) -> bool {
        self.batch_count > 0
    }

    /// Seal the current file and return its sealed path.
    pub fn seal_current_file(mut self) -> Result<PathBuf> {
        self.flush_pending_block()?;
        let footer = FileFooterV1 {
            file_sequence: self.file_sequence,
            created_at_ms: self.created_at_ms,
            sealed_at_ms: now_ms(),
            event_count: self.batch_count,
            block_count: self.block_sequence,
            min_created_at_ms: self.min_created_at_ms,
            max_created_at_ms: self.max_created_at_ms,
            uncompressed_bytes: self.uncompressed_bytes,
            compressed_bytes: self.compressed_bytes,
        };
        write_record(&mut self.file, FOOTER_TAG, &footer)?;
        self.file.sync_all().with_context(|| {
            format!("failed to sync rollup journal `{}`", self.active_path.display())
        })?;
        drop(self.file);

        let sealed_path = self
            .config
            .root_dir
            .join("sealed")
            .join(format!("rollup-{:012}.journal", self.file_sequence));
        fs::rename(&self.active_path, &sealed_path).with_context(|| {
            format!(
                "failed to seal rollup journal `{}` to `{}`",
                self.active_path.display(),
                sealed_path.display()
            )
        })?;
        Ok(sealed_path)
    }

    fn flush_pending_block(&mut self) -> Result<()> {
        if self.pending_batches.is_empty() {
            return Ok(());
        }
        let block = JournalRollupBatchBlockV1 {
            batches: std::mem::take(&mut self.pending_batches),
        };
        self.pending_uncompressed_bytes = 0;
        let uncompressed = postcard::to_allocvec(&block)?;
        let compressed =
            zstd::stream::encode_all(Cursor::new(&uncompressed), self.config.zstd_level)
                .context("failed to zstd-compress rollup journal block")?;
        let batch_count = block.batches.len() as u32;
        let min_created_at_ms = block
            .batches
            .iter()
            .map(|batch| batch.created_at_ms)
            .min()
            .ok_or_else(|| anyhow!("rollup journal block unexpectedly has no batches"))?;
        let max_created_at_ms = block
            .batches
            .iter()
            .map(|batch| batch.created_at_ms)
            .max()
            .ok_or_else(|| anyhow!("rollup journal block unexpectedly has no batches"))?;
        let mut header = BlockHeaderV1 {
            block_sequence: self.block_sequence,
            event_count: batch_count,
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
            .context("failed to write rollup journal block payload")?;

        self.block_sequence = self.block_sequence.saturating_add(1);
        self.batch_count = self.batch_count.saturating_add(u64::from(batch_count));
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

/// Reader for one control-rollup journal file.
#[derive(Debug)]
pub struct RollupJournalReader {
    path: PathBuf,
}

/// Streaming reader for one control-rollup journal file.
#[derive(Debug)]
pub struct RollupJournalBatchStream {
    path: PathBuf,
    file: File,
    total_compressed_bytes: u64,
    bytes_read: u64,
    hasher: Sha256,
    finished: bool,
    footer: Option<FileFooterV1>,
    decoded_batches: VecDeque<UsageRollupBatch>,
}

impl RollupJournalReader {
    /// Open a rollup journal file for reading.
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Read the sealed footer and validate block headers and CRCs without
    /// materializing all decoded batches in memory.
    pub fn scan_summary(&self) -> Result<JournalFileSummary> {
        let mut file = File::open(&self.path)
            .with_context(|| format!("failed to open rollup journal `{}`", self.path.display()))?;
        let total_compressed_bytes = file
            .metadata()
            .with_context(|| format!("failed to stat rollup journal `{}`", self.path.display()))?
            .len();
        let _header = read_file_header(&mut file)?;
        loop {
            let tag = read_record_tag(&mut file, &self.path)?;
            if tag == *FOOTER_TAG {
                let footer: FileFooterV1 = read_record_payload(&mut file)?;
                return Ok(JournalFileSummary {
                    footer,
                    total_compressed_bytes,
                });
            }
            if tag != *BLOCK_TAG {
                return Err(anyhow!(
                    "unexpected rollup journal record tag `{}`",
                    String::from_utf8_lossy(&tag)
                ));
            }
            let header: BlockHeaderV1 = read_record_payload(&mut file)?;
            let compressed = read_exact_bytes(
                &mut file,
                header.compressed_len as usize,
                "failed to read rollup journal block payload",
            )?;
            validate_block(&header, &compressed)?;
        }
    }

    /// Open a streaming rollup batch reader that validates and decodes one
    /// block at a time.
    pub fn stream_batches(&self) -> Result<RollupJournalBatchStream> {
        let file = File::open(&self.path)
            .with_context(|| format!("failed to open rollup journal `{}`", self.path.display()))?;
        let total_compressed_bytes = file
            .metadata()
            .with_context(|| format!("failed to stat rollup journal `{}`", self.path.display()))?
            .len();
        let mut stream = RollupJournalBatchStream {
            path: self.path.clone(),
            file,
            total_compressed_bytes,
            bytes_read: 0,
            hasher: Sha256::new(),
            finished: false,
            footer: None,
            decoded_batches: VecDeque::new(),
        };
        let _header = read_file_header_hashed(&mut stream)?;
        Ok(stream)
    }

    /// Read and validate all rollup batches in the file.
    pub fn read_all_batches(&self) -> Result<Vec<UsageRollupBatch>> {
        let mut stream = self.stream_batches()?;
        let mut batches = Vec::new();
        while let Some(batch) = stream.next_batch()? {
            batches.push(batch);
        }
        let _ = stream.finish()?;
        Ok(batches)
    }
}

impl RollupJournalBatchStream {
    /// Decode the next rollup batch. Returns `None` once the sealed footer has
    /// been consumed.
    pub fn next_batch(&mut self) -> Result<Option<UsageRollupBatch>> {
        if let Some(batch) = self.decoded_batches.pop_front() {
            return Ok(Some(batch));
        }
        if self.finished {
            return Ok(None);
        }
        let tag = read_record_tag_hashed(self)?;
        if tag == *FOOTER_TAG {
            let footer: FileFooterV1 = read_record_payload_hashed(self)?;
            self.finished = true;
            self.footer = Some(footer);
            return Ok(None);
        }
        if tag != *BLOCK_TAG {
            return Err(anyhow!(
                "unexpected rollup journal record tag `{}`",
                String::from_utf8_lossy(&tag)
            ));
        }
        let header: BlockHeaderV1 = read_record_payload_hashed(self)?;
        let compressed = self.read_exact_bytes(
            header.compressed_len as usize,
            "failed to read rollup journal block payload",
        )?;
        validate_block(&header, &compressed)?;
        let decoded =
            zstd::stream::decode_all(Cursor::new(&compressed)).context("zstd decode failed")?;
        if decoded.len() != header.uncompressed_len as usize {
            return Err(anyhow!(
                "rollup journal block length mismatch: expected {}, got {}",
                header.uncompressed_len,
                decoded.len()
            ));
        }
        let block = decode_journal_rollup_batch_block(&decoded)?;
        if block.batches.is_empty() {
            return Err(anyhow!("rollup journal block unexpectedly has no batches"));
        }
        self.decoded_batches = block
            .batches
            .into_iter()
            .map(JournalRollupBatchV1::into_rollup_batch)
            .collect();
        Ok(self.decoded_batches.pop_front())
    }

    /// Finish a fully consumed stream and return footer metadata plus the
    /// complete file digest.
    pub fn finish(self) -> Result<JournalStreamReport> {
        if !self.finished {
            return Err(anyhow!(
                "rollup journal `{}` stream was not fully consumed",
                self.path.display()
            ));
        }
        let footer = self
            .footer
            .ok_or_else(|| anyhow!("rollup journal stream finished without footer"))?;
        Ok(JournalStreamReport {
            footer,
            file_digest_hex: format!("{:x}", self.hasher.finalize()),
            total_compressed_bytes: self.total_compressed_bytes,
        })
    }

    /// Return the total file bytes known from metadata.
    pub fn total_compressed_bytes(&self) -> u64 {
        self.total_compressed_bytes
    }

    /// Return bytes consumed from the rollup journal stream so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    fn read_exact_tracked(&mut self, buf: &mut [u8], context: &'static str) -> Result<()> {
        self.file.read_exact(buf).context(context)?;
        self.bytes_read = self.bytes_read.saturating_add(buf.len() as u64);
        self.hasher.update(buf);
        Ok(())
    }

    fn read_exact_bytes(&mut self, len: usize, context: &'static str) -> Result<Vec<u8>> {
        let mut bytes = vec![0u8; len];
        self.read_exact_tracked(&mut bytes, context)?;
        Ok(bytes)
    }
}

fn create_journal_dirs(root: &Path) -> Result<()> {
    for name in ["active", "sealed", "consuming"] {
        let path = root.join(name);
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create rollup journal dir `{}`", path.display()))?;
    }
    Ok(())
}

fn next_rollup_sequence(root: &Path) -> Result<u64> {
    let mut max_sequence = 0u64;
    for dir_name in ["active", "sealed", "consuming", "bad"] {
        let dir = root.join(dir_name);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to list rollup journal dir `{}`", dir.display()))?
        {
            let entry = entry.with_context(|| {
                format!("failed to read rollup journal dir entry `{}`", dir.display())
            })?;
            let Some(file_name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(sequence) = parse_rollup_sequence_from_file_name(&file_name) {
                max_sequence = max_sequence.max(sequence);
            }
        }
    }
    Ok(max_sequence.saturating_add(1))
}

/// Parse the numeric sequence from a rollup journal file name.
pub fn parse_rollup_sequence_from_file_name(file_name: &str) -> Option<u64> {
    let suffix = file_name.strip_prefix("rollup-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        return None;
    }
    digits.parse().ok()
}

/// Recover orphaned active rollup files after an API-process crash.
pub fn recover_orphan_active_rollup_files(
    config: &JournalConfig,
) -> Result<RollupActiveRecoveryReport> {
    create_journal_dirs(&config.root_dir)?;
    fs::create_dir_all(config.root_dir.join("bad")).with_context(|| {
        format!(
            "failed to create rollup journal bad dir `{}`",
            config.root_dir.join("bad").display()
        )
    })?;
    let active_dir = config.root_dir.join("active");
    let mut report = RollupActiveRecoveryReport::default();
    for entry in fs::read_dir(&active_dir)
        .with_context(|| format!("failed to list rollup active dir `{}`", active_dir.display()))?
    {
        let entry = entry.with_context(|| {
            format!("failed to read rollup active dir entry `{}`", active_dir.display())
        })?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(sequence) = parse_rollup_sequence_from_file_name(file_name) else {
            continue;
        };
        match recover_one_active_rollup_file(config, &path, sequence) {
            Ok(ActiveRollupRecoveryOutcome::Recovered) => {
                report.recovered_files = report.recovered_files.saturating_add(1);
            },
            Ok(ActiveRollupRecoveryOutcome::DeletedEmpty) => {
                report.deleted_empty_files = report.deleted_empty_files.saturating_add(1);
            },
            Ok(ActiveRollupRecoveryOutcome::Quarantined) => {
                report.quarantined_files = report.quarantined_files.saturating_add(1);
            },
            Err(err) => {
                report.quarantined_files = report.quarantined_files.saturating_add(1);
                tracing::error!(
                    path = %path.display(),
                    "failed to recover active rollup journal; quarantining: {err:#}"
                );
                quarantine_active_rollup_file(config, &path, sequence)?;
            },
        }
    }
    Ok(report)
}

enum ActiveRollupRecoveryOutcome {
    Recovered,
    DeletedEmpty,
    Quarantined,
}

fn recover_one_active_rollup_file(
    config: &JournalConfig,
    path: &Path,
    sequence: u64,
) -> Result<ActiveRollupRecoveryOutcome> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read active rollup journal `{}`", path.display()))?;
    let Some(scan) = scan_active_rollup_bytes(&bytes)? else {
        fs::remove_file(path).with_context(|| {
            format!("failed to delete empty active rollup journal `{}`", path.display())
        })?;
        return Ok(ActiveRollupRecoveryOutcome::DeletedEmpty);
    };
    if scan.file_sequence != sequence {
        anyhow::bail!(
            "active rollup journal sequence mismatch: file name {}, header {}",
            sequence,
            scan.file_sequence
        );
    }
    if scan.batch_count == 0 {
        fs::remove_file(path).with_context(|| {
            format!("failed to delete empty active rollup journal `{}`", path.display())
        })?;
        return Ok(ActiveRollupRecoveryOutcome::DeletedEmpty);
    }

    let mut file = OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("failed to open active rollup journal `{}`", path.display()))?;
    file.set_len(scan.valid_len).with_context(|| {
        format!("failed to truncate active rollup journal `{}` to valid prefix", path.display())
    })?;
    file.seek(SeekFrom::End(0))
        .context("failed to seek active rollup journal end")?;
    if !scan.has_footer {
        write_record(&mut file, FOOTER_TAG, &scan.footer)?;
    }
    file.sync_all().with_context(|| {
        format!("failed to sync recovered active rollup journal `{}`", path.display())
    })?;
    drop(file);

    let sealed_path = config
        .root_dir
        .join("sealed")
        .join(format!("rollup-{sequence:012}.journal"));
    if sealed_path.exists() {
        tracing::error!(
            path = %path.display(),
            sealed_path = %sealed_path.display(),
            "cannot recover active rollup journal because sealed target exists; quarantining"
        );
        quarantine_active_rollup_file(config, path, sequence)?;
        return Ok(ActiveRollupRecoveryOutcome::Quarantined);
    }
    fs::rename(path, &sealed_path).with_context(|| {
        format!(
            "failed to recover active rollup journal `{}` to `{}`",
            path.display(),
            sealed_path.display()
        )
    })?;
    Ok(ActiveRollupRecoveryOutcome::Recovered)
}

struct ActiveRollupScan {
    file_sequence: u64,
    batch_count: u64,
    valid_len: u64,
    has_footer: bool,
    footer: FileFooterV1,
}

fn scan_active_rollup_bytes(bytes: &[u8]) -> Result<Option<ActiveRollupScan>> {
    if bytes.is_empty() {
        return Ok(None);
    }
    let mut offset = 0usize;
    if bytes.len() < ROLLUP_FILE_MAGIC_V1.len() + 4 {
        return Ok(None);
    }
    let magic = &bytes[..ROLLUP_FILE_MAGIC_V1.len()];
    if magic != ROLLUP_FILE_MAGIC_V1 {
        anyhow::bail!("invalid active rollup journal magic");
    }
    offset += ROLLUP_FILE_MAGIC_V1.len();
    let Some(header_bytes) = read_len_prefixed_slice(bytes, &mut offset) else {
        return Ok(None);
    };
    let header: FileHeaderV1 = postcard::from_bytes(header_bytes)?;
    let mut valid_len = offset;
    let mut batch_count = 0u64;
    let mut block_count = 0u64;
    let mut min_created_at_ms = None::<i64>;
    let mut max_created_at_ms = None::<i64>;
    let mut uncompressed_bytes = 0u64;
    let mut compressed_bytes = 0u64;
    let mut has_footer = false;
    let mut footer = None::<FileFooterV1>;

    while offset < bytes.len() {
        if bytes.len().saturating_sub(offset) < 8 {
            break;
        }
        let tag: [u8; 4] = bytes[offset..offset + 4]
            .try_into()
            .expect("slice length checked");
        offset += 4;
        let Some(record_bytes) = read_len_prefixed_slice(bytes, &mut offset) else {
            break;
        };
        if tag == *FOOTER_TAG {
            let decoded: FileFooterV1 = postcard::from_bytes(record_bytes)?;
            footer = Some(decoded);
            has_footer = true;
            valid_len = offset;
            break;
        }
        if tag != *BLOCK_TAG {
            anyhow::bail!(
                "unexpected active rollup journal record tag `{}`",
                String::from_utf8_lossy(&tag)
            );
        }
        let block_header: BlockHeaderV1 = postcard::from_bytes(record_bytes)?;
        let compressed_len = block_header.compressed_len as usize;
        if bytes.len().saturating_sub(offset) < compressed_len {
            break;
        }
        let compressed = &bytes[offset..offset + compressed_len];
        validate_block(&block_header, compressed)?;
        offset += compressed_len;
        valid_len = offset;
        batch_count = batch_count.saturating_add(u64::from(block_header.event_count));
        block_count = block_count.saturating_add(1);
        min_created_at_ms =
            Some(min_created_at_ms.map_or(block_header.min_created_at_ms, |current| {
                current.min(block_header.min_created_at_ms)
            }));
        max_created_at_ms =
            Some(max_created_at_ms.map_or(block_header.max_created_at_ms, |current| {
                current.max(block_header.max_created_at_ms)
            }));
        uncompressed_bytes = uncompressed_bytes.saturating_add(block_header.uncompressed_len);
        compressed_bytes = compressed_bytes.saturating_add(block_header.compressed_len);
    }

    if batch_count == 0 {
        return Ok(None);
    }
    let footer = footer.unwrap_or(FileFooterV1 {
        file_sequence: header.file_sequence,
        created_at_ms: header.created_at_ms,
        sealed_at_ms: now_ms(),
        event_count: batch_count,
        block_count,
        min_created_at_ms,
        max_created_at_ms,
        uncompressed_bytes,
        compressed_bytes,
    });
    Ok(Some(ActiveRollupScan {
        file_sequence: header.file_sequence,
        batch_count,
        valid_len: valid_len as u64,
        has_footer,
        footer,
    }))
}

fn read_len_prefixed_slice<'a>(bytes: &'a [u8], offset: &mut usize) -> Option<&'a [u8]> {
    if bytes.len().saturating_sub(*offset) < 4 {
        return None;
    }
    let len = u32::from_le_bytes(
        bytes[*offset..*offset + 4]
            .try_into()
            .expect("slice length checked"),
    ) as usize;
    *offset += 4;
    if bytes.len().saturating_sub(*offset) < len {
        return None;
    }
    let payload = &bytes[*offset..*offset + len];
    *offset += len;
    Some(payload)
}

fn quarantine_active_rollup_file(config: &JournalConfig, path: &Path, sequence: u64) -> Result<()> {
    let bad_path = config
        .root_dir
        .join("bad")
        .join(format!("rollup-{sequence:012}.bad"));
    fs::rename(path, &bad_path).with_context(|| {
        format!(
            "failed to quarantine active rollup journal `{}` to `{}`",
            path.display(),
            bad_path.display()
        )
    })
}

fn validate_block(header: &BlockHeaderV1, compressed: &[u8]) -> Result<()> {
    let actual_crc = block_crc32c(header, compressed)?;
    if actual_crc != header.crc32c {
        return Err(anyhow!(
            "rollup journal block crc mismatch: expected {}, got {}",
            header.crc32c,
            actual_crc
        ));
    }
    Ok(())
}

fn read_file_header(file: &mut File) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .context("failed to read rollup journal magic")?;
    if &magic != ROLLUP_FILE_MAGIC_V1 {
        return Err(anyhow!("invalid rollup journal magic"));
    }
    read_len_prefixed_payload(file)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_file_header_hashed(stream: &mut RollupJournalBatchStream) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    stream.read_exact_tracked(&mut magic, "failed to read rollup journal magic")?;
    if &magic != ROLLUP_FILE_MAGIC_V1 {
        return Err(anyhow!("invalid rollup journal magic"));
    }
    read_len_prefixed_payload_hashed(stream)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_record_tag(file: &mut File, path: &Path) -> Result<[u8; 4]> {
    let mut tag = [0u8; 4];
    match file.read_exact(&mut tag) {
        Ok(()) => Ok(tag),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(anyhow!("rollup journal `{}` ended before footer", path.display()))
        },
        Err(err) => Err(err).context("failed to read rollup journal record tag"),
    }
}

fn read_record_tag_hashed(stream: &mut RollupJournalBatchStream) -> Result<[u8; 4]> {
    let mut tag = [0u8; 4];
    match stream.read_exact_tracked(&mut tag, "failed to read rollup journal record tag") {
        Ok(()) => Ok(tag),
        Err(err) => {
            let unexpected_eof = err
                .downcast_ref::<std::io::Error>()
                .map(|io_err| io_err.kind() == std::io::ErrorKind::UnexpectedEof)
                .unwrap_or(false);
            if unexpected_eof {
                Err(anyhow!("rollup journal `{}` ended before footer", stream.path.display()))
            } else {
                Err(err)
            }
        },
    }
}

fn read_record_payload<T: serde::de::DeserializeOwned>(file: &mut File) -> Result<T> {
    read_len_prefixed_payload(file)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_record_payload_hashed<T: serde::de::DeserializeOwned>(
    stream: &mut RollupJournalBatchStream,
) -> Result<T> {
    read_len_prefixed_payload_hashed(stream)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_len_prefixed_payload(file: &mut File) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    file.read_exact(&mut len)
        .context("failed to read rollup journal record length")?;
    let len = u32::from_le_bytes(len) as usize;
    read_exact_bytes(file, len, "failed to read rollup journal record payload")
}

fn read_len_prefixed_payload_hashed(stream: &mut RollupJournalBatchStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact_tracked(&mut len, "failed to read rollup journal record length")?;
    let len = u32::from_le_bytes(len) as usize;
    stream.read_exact_bytes(len, "failed to read rollup journal record payload")
}

fn read_exact_bytes(file: &mut File, len: usize, context: &'static str) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes).context(context)?;
    Ok(bytes)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
