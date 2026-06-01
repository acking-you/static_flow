//! Journal configuration.

use std::path::PathBuf;

/// Runtime settings for usage journal writing and retention.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JournalConfig {
    /// Root directory that contains active, sealed, consuming, and bad files.
    pub root_dir: PathBuf,
    /// Maximum compressed file size before sealing.
    pub max_file_bytes: u64,
    /// Maximum file age before sealing.
    pub max_file_age_ms: u64,
    /// Maximum sealed plus stale-consuming files retained.
    pub max_files: usize,
    /// Target uncompressed block payload bytes.
    pub block_target_uncompressed_bytes: usize,
    /// Maximum events per block.
    pub block_max_events: usize,
    /// Fsync interval in milliseconds; zero means every flushed block.
    pub fsync_interval_ms: u64,
    /// zstd compression level.
    pub zstd_level: i32,
    /// Claimed-file lease age before recovery.
    pub consumer_lease_ms: u64,
    /// Whether corrupt files are deleted instead of quarantined.
    pub delete_bad_files: bool,
}

impl JournalConfig {
    /// Build production defaults for a root directory.
    pub fn new(root_dir: PathBuf) -> Self {
        Self {
            root_dir,
            max_file_bytes: 64 * 1024 * 1024,
            max_file_age_ms: 300_000,
            max_files: 128,
            block_target_uncompressed_bytes: 1024 * 1024,
            block_max_events: 1024,
            fsync_interval_ms: 250,
            zstd_level: 3,
            consumer_lease_ms: 300_000,
            delete_bad_files: false,
        }
    }
}
