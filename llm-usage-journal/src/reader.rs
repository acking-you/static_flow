//! Journal file reader.

use std::{
    fs::File,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Result};

use crate::{
    wire::{BlockHeaderV1, FileFooterV1, FileHeaderV1, JournalUsageBatchV1, FILE_MAGIC_V1},
    writer::{block_crc32c, BLOCK_TAG, FOOTER_TAG},
};

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

    /// Read and validate all usage batches in the file.
    pub fn read_all_batches(&self) -> Result<Vec<JournalUsageBatchV1>> {
        let mut file = File::open(&self.path)
            .with_context(|| format!("failed to open journal `{}`", self.path.display()))?;
        let _header = read_file_header(&mut file)?;
        let mut batches = Vec::new();
        loop {
            let mut tag = [0u8; 4];
            match file.read_exact(&mut tag) {
                Ok(()) => {},
                Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(anyhow!(
                        "usage journal `{}` ended before footer",
                        self.path.display()
                    ));
                },
                Err(err) => return Err(err).context("failed to read journal record tag"),
            }
            if &tag == FOOTER_TAG {
                let _footer: FileFooterV1 = read_record_payload(&mut file)?;
                break;
            }
            if &tag != BLOCK_TAG {
                return Err(anyhow!(
                    "unexpected usage journal record tag `{}`",
                    String::from_utf8_lossy(&tag)
                ));
            }
            let header: BlockHeaderV1 = read_record_payload(&mut file)?;
            let mut compressed = vec![0u8; header.compressed_len as usize];
            file.read_exact(&mut compressed)
                .context("failed to read journal block payload")?;
            let actual_crc = block_crc32c(&header, &compressed)?;
            if actual_crc != header.crc32c {
                return Err(anyhow!(
                    "usage journal block crc mismatch: expected {}, got {}",
                    header.crc32c,
                    actual_crc
                ));
            }
            let decoded =
                zstd::stream::decode_all(Cursor::new(&compressed)).context("zstd decode failed")?;
            if decoded.len() != header.uncompressed_len as usize {
                return Err(anyhow!(
                    "usage journal block length mismatch: expected {}, got {}",
                    header.uncompressed_len,
                    decoded.len()
                ));
            }
            let batch: JournalUsageBatchV1 = postcard::from_bytes(&decoded)?;
            batches.push(batch);
        }
        Ok(batches)
    }
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

fn read_record_payload<T: serde::de::DeserializeOwned>(file: &mut File) -> Result<T> {
    read_len_prefixed_payload(file)
        .and_then(|bytes| postcard::from_bytes(&bytes).map_err(anyhow::Error::from))
}

fn read_len_prefixed_payload(file: &mut File) -> Result<Vec<u8>> {
    let mut len = [0u8; 4];
    file.read_exact(&mut len)
        .context("failed to read journal record length")?;
    let len = u32::from_le_bytes(len) as usize;
    let mut bytes = vec![0u8; len];
    file.read_exact(&mut bytes)
        .context("failed to read journal record payload")?;
    Ok(bytes)
}
