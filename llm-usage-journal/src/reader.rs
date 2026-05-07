//! Journal file reader.

use std::path::Path;

use anyhow::Result;

/// Reader for one usage journal file.
#[derive(Debug)]
pub struct JournalReader;

impl JournalReader {
    /// Open a journal file for reading.
    pub fn open(_path: &Path) -> Result<Self> {
        Ok(Self)
    }
}
