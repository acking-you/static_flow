//! Journal file reader.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};
use sha2::{Digest, Sha256};

use crate::{
    wire::{
        decode_journal_usage_batch, BlockHeaderV1, FileFooterV1, FileHeaderV1, JournalUsageBatchV1,
        FILE_MAGIC_V1,
    },
    writer::{block_crc32c, BLOCK_TAG, FOOTER_TAG},
};

/// One validated journal file summary without decoding all batches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalFileSummary {
    /// Footer written when the file was sealed.
    pub footer: FileFooterV1,
    /// Total on-disk bytes of the journal file.
    pub total_compressed_bytes: u64,
}

/// Final stream report returned after a full streaming read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalStreamReport {
    /// Footer written when the file was sealed.
    pub footer: FileFooterV1,
    /// SHA-256 of the full file bytes.
    pub file_digest_hex: String,
    /// Total on-disk bytes of the journal file.
    pub total_compressed_bytes: u64,
}

/// Streaming reader for one usage journal file.
#[derive(Debug)]
pub struct JournalBatchStream {
    path: PathBuf,
    file: File,
    total_compressed_bytes: u64,
    bytes_read: u64,
    hasher: Sha256,
    finished: bool,
    footer: Option<FileFooterV1>,
}

/// Reader for one usage journal file.
#[derive(Debug)]
pub struct JournalReader {
    path: PathBuf,
}

impl JournalReader {
    /// Open a journal file for reading.
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            path: path.to_path_buf(),
        })
    }

    /// Read the sealed footer and validate block headers and CRCs without
    /// materializing all decoded batches in memory.
    pub fn scan_summary(&self) -> Result<JournalFileSummary> {
        let mut file = File::open(&self.path)
            .with_context(|| format!("failed to open journal `{}`", self.path.display()))?;
        let total_compressed_bytes = file
            .metadata()
            .with_context(|| format!("failed to stat journal `{}`", self.path.display()))?
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
                    "unexpected usage journal record tag `{}`",
                    String::from_utf8_lossy(&tag)
                ));
            }
            let header: BlockHeaderV1 = read_record_payload(&mut file)?;
            let compressed = read_exact_bytes(
                &mut file,
                header.compressed_len as usize,
                "failed to read journal block payload",
            )?;
            validate_block(&header, &compressed)?;
        }
    }

    /// Open a streaming batch reader that validates and decodes one block at a
    /// time.
    pub fn stream_batches(&self) -> Result<JournalBatchStream> {
        let file = File::open(&self.path)
            .with_context(|| format!("failed to open journal `{}`", self.path.display()))?;
        let total_compressed_bytes = file
            .metadata()
            .with_context(|| format!("failed to stat journal `{}`", self.path.display()))?
            .len();
        let mut stream = JournalBatchStream {
            path: self.path.clone(),
            file,
            total_compressed_bytes,
            bytes_read: 0,
            hasher: Sha256::new(),
            finished: false,
            footer: None,
        };
        let _header = read_file_header_hashed(&mut stream)?;
        Ok(stream)
    }

    /// Read and validate all usage batches in the file.
    pub fn read_all_batches(&self) -> Result<Vec<JournalUsageBatchV1>> {
        let mut stream = self.stream_batches()?;
        let mut batches = Vec::new();
        while let Some(batch) = stream.next_batch()? {
            batches.push(batch);
        }
        let _ = stream.finish()?;
        Ok(batches)
    }
}

impl JournalBatchStream {
    /// Decode the next batch block. Returns `None` once the sealed footer has
    /// been consumed.
    pub fn next_batch(&mut self) -> Result<Option<JournalUsageBatchV1>> {
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
                "unexpected usage journal record tag `{}`",
                String::from_utf8_lossy(&tag)
            ));
        }
        let header: BlockHeaderV1 = read_record_payload_hashed(self)?;
        let compressed = self.read_exact_bytes(
            header.compressed_len as usize,
            "failed to read journal block payload",
        )?;
        validate_block(&header, &compressed)?;
        let decoded =
            zstd::stream::decode_all(Cursor::new(&compressed)).context("zstd decode failed")?;
        if decoded.len() != header.uncompressed_len as usize {
            return Err(anyhow!(
                "usage journal block length mismatch: expected {}, got {}",
                header.uncompressed_len,
                decoded.len()
            ));
        }
        let batch: JournalUsageBatchV1 = decode_journal_usage_batch(&decoded)?;
        Ok(Some(batch))
    }

    /// Finish a fully consumed stream and return footer metadata plus the
    /// complete file digest.
    pub fn finish(self) -> Result<JournalStreamReport> {
        if !self.finished {
            return Err(anyhow!(
                "usage journal `{}` stream was not fully consumed",
                self.path.display()
            ));
        }
        let footer = self
            .footer
            .ok_or_else(|| anyhow!("usage journal stream finished without footer"))?;
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

    /// Return bytes consumed from the journal stream so far.
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

fn validate_block(header: &BlockHeaderV1, compressed: &[u8]) -> Result<()> {
    let actual_crc = block_crc32c(header, compressed)?;
    if actual_crc != header.crc32c {
        return Err(anyhow!(
            "usage journal block crc mismatch: expected {}, got {}",
            header.crc32c,
            actual_crc
        ));
    }
    Ok(())
}

fn read_file_header(file: &mut File) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)
        .context("failed to read journal magic")?;
    if &magic != FILE_MAGIC_V1 {
        return Err(anyhow!("invalid usage journal magic"));
    }
    read_len_prefixed_payload(file)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_file_header_hashed(stream: &mut JournalBatchStream) -> Result<FileHeaderV1> {
    let mut magic = [0u8; 8];
    stream.read_exact_tracked(&mut magic, "failed to read journal magic")?;
    if &magic != FILE_MAGIC_V1 {
        return Err(anyhow!("invalid usage journal magic"));
    }
    read_len_prefixed_payload_hashed(stream)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_record_tag(file: &mut File, path: &Path) -> Result<[u8; 4]> {
    let mut tag = [0u8; 4];
    match file.read_exact(&mut tag) {
        Ok(()) => Ok(tag),
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(anyhow!("usage journal `{}` ended before footer", path.display()))
        },
        Err(err) => Err(err).context("failed to read journal record tag"),
    }
}

fn read_record_tag_hashed(stream: &mut JournalBatchStream) -> Result<[u8; 4]> {
    let mut tag = [0u8; 4];
    match stream.read_exact_tracked(&mut tag, "failed to read journal record tag") {
        Ok(()) => Ok(tag),
        Err(err) => {
            let unexpected_eof = err
                .downcast_ref::<std::io::Error>()
                .map(|io_err| io_err.kind() == std::io::ErrorKind::UnexpectedEof)
                .unwrap_or(false);
            if unexpected_eof {
                Err(anyhow!("usage journal `{}` ended before footer", stream.path.display()))
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
    stream: &mut JournalBatchStream,
) -> Result<T> {
    read_len_prefixed_payload_hashed(stream)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_len_prefixed_payload(file: &mut File) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    file.read_exact(&mut len)
        .context("failed to read journal record length")?;
    let len = u32::from_le_bytes(len) as usize;
    read_exact_bytes(file, len, "failed to read journal record payload")
}

fn read_len_prefixed_payload_hashed(stream: &mut JournalBatchStream) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    stream.read_exact_tracked(&mut len, "failed to read journal record length")?;
    let len = u32::from_le_bytes(len) as usize;
    stream.read_exact_bytes(len, "failed to read journal record payload")
}

fn read_exact_bytes(file: &mut File, len: usize, context: &'static str) -> Result<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes).context(context)?;
    Ok(bytes)
}
