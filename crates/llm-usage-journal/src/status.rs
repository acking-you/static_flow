//! Journal status snapshots.

use serde::{Deserialize, Serialize};

/// Producer-side journal status snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalStatusSnapshot {
    /// Whether the journal writer is enabled.
    pub journal_enabled: bool,
    /// Journal root directory.
    pub journal_root: String,
    /// Current active file sequence.
    pub active_file_sequence: Option<u64>,
    /// Current active file byte size.
    pub active_file_bytes: u64,
    /// Number of sealed files waiting for consumption.
    pub sealed_file_count: u64,
    /// Total sealed file bytes.
    pub sealed_bytes: u64,
    /// Age of the oldest sealed file in milliseconds.
    pub oldest_sealed_age_ms: Option<i64>,
    /// Total deleted journal files.
    pub dropped_files_total: u64,
    /// Total unconsumed deleted journal files.
    pub dropped_unconsumed_files_total: u64,
    /// Total write failures.
    pub write_failures_total: u64,
}

/// One concrete journal file visible under a journal root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalFileSnapshot {
    /// File name such as `usage-000000000123.journal`.
    pub file_name: String,
    /// Full path on disk.
    pub path: String,
    /// Parsed sequence when the name matches the journal naming scheme.
    pub sequence: Option<u64>,
    /// File size in bytes.
    pub bytes: u64,
    /// File age in milliseconds when metadata is available.
    pub age_ms: Option<i64>,
}

/// File-level view of the current journal root.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalFileListsSnapshot {
    /// Files currently being appended by the producer.
    pub active: Vec<JournalFileSnapshot>,
    /// Files sealed and waiting to be consumed.
    pub sealed: Vec<JournalFileSnapshot>,
    /// Files claimed by the worker and in progress.
    pub consuming: Vec<JournalFileSnapshot>,
    /// Files quarantined after a bad read/import.
    pub bad: Vec<JournalFileSnapshot>,
}

/// Worker-side journal consumption progress snapshot.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerProgressSnapshot {
    /// Worker state label.
    pub state: String,
    /// Current claimed file path.
    pub current_file_path: Option<String>,
    /// Current claimed file sequence.
    pub current_file_sequence: Option<u64>,
    /// Processed block count for the current file.
    pub processed_blocks: u64,
    /// Total block count for the current file.
    pub total_blocks: u64,
    /// Processed event count for the current file.
    pub processed_events: u64,
    /// Total event count for the current file.
    pub total_events: u64,
    /// Processed compressed bytes for the current file.
    pub processed_compressed_bytes: u64,
    /// Total compressed bytes for the current file.
    pub total_compressed_bytes: u64,
    /// Progress percent for display.
    pub progress_percent: f64,
    /// Current import rate in events per second.
    pub import_rate_events_per_second: f64,
    /// Last worker heartbeat timestamp in Unix milliseconds.
    pub heartbeat_at_ms: Option<i64>,
    /// Last successfully imported file sequence.
    pub last_successful_file_sequence: Option<u64>,
    /// Last successful import timestamp in Unix milliseconds.
    pub last_successful_import_at_ms: Option<i64>,
    /// Last error message.
    pub last_error: Option<String>,
    /// Last error timestamp in Unix milliseconds.
    pub last_error_at_ms: Option<i64>,
}
