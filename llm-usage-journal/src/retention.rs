//! Journal retention helpers.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};

use crate::{config::JournalConfig, writer::parse_sequence_from_file_name};

/// Result of applying journal retention.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetentionReport {
    /// Number of deleted files.
    pub deleted_files: u64,
    /// Total deleted bytes.
    pub deleted_bytes: u64,
    /// Number of deleted files that were not known to be consumed.
    pub deleted_unconsumed_files: u64,
}

/// Delete oldest sealed files until configured retention is satisfied.
pub fn enforce_retention(config: &JournalConfig) -> Result<RetentionReport> {
    let sealed_dir = config.root_dir.join("sealed");
    if !sealed_dir.exists() {
        return Ok(RetentionReport::default());
    }
    let mut files = Vec::new();
    for entry in fs::read_dir(&sealed_dir)
        .with_context(|| format!("failed to read sealed journal dir `{}`", sealed_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        let sequence = parse_sequence_from_file_name(&file_name).unwrap_or(u64::MAX);
        files.push((sequence, path));
    }
    files.sort_by_key(|(sequence, _path)| *sequence);
    let mut report = RetentionReport::default();
    while files.len() > config.max_files {
        let (_sequence, path): (u64, PathBuf) = files.remove(0);
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        fs::remove_file(&path)
            .with_context(|| format!("failed to delete sealed journal `{}`", path.display()))?;
        report.deleted_files = report.deleted_files.saturating_add(1);
        report.deleted_bytes = report.deleted_bytes.saturating_add(bytes);
        report.deleted_unconsumed_files = report.deleted_unconsumed_files.saturating_add(1);
    }
    Ok(report)
}
