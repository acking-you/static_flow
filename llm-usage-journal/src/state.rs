//! Journal consumer state.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};

use crate::status::WorkerProgressSnapshot;

/// Persisted consumer state for one journal root.
pub struct JournalConsumerState {
    conn: Connection,
}

impl JournalConsumerState {
    /// Open the default consumer-state database under a journal root.
    pub fn open(root_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(root_dir)
            .with_context(|| format!("failed to create journal root `{}`", root_dir.display()))?;
        Self::open_path(root_dir.join("consumer-state.sqlite3"))
    }

    /// Open a consumer-state database at an explicit path.
    pub fn open_path(path: PathBuf) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create consumer state dir `{}`", parent.display())
            })?;
        }
        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open consumer state `{}`", path.display()))?;
        initialize_consumer_state(&conn)?;
        Ok(Self {
            conn,
        })
    }

    /// Return true when a file sequence has already been imported.
    pub fn is_consumed(&self, file_sequence: u64) -> Result<bool> {
        let exists = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM consumed_files WHERE file_sequence = ?1)",
                params![file_sequence as i64],
                |row| row.get::<_, bool>(0),
            )
            .context("check consumed journal file")?;
        Ok(exists)
    }

    /// Record a fully imported journal file.
    pub fn record_consumed_file(
        &self,
        file_sequence: u64,
        file_digest: &str,
        event_count: u64,
        imported_at_ms: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO consumed_files (
                    file_sequence, file_digest, event_count, imported_at_ms
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![file_sequence as i64, file_digest, event_count as i64, imported_at_ms,],
            )
            .context("record consumed journal file")?;
        Ok(())
    }

    /// Persist the current worker progress row.
    pub fn update_progress(
        &self,
        progress: &WorkerProgressSnapshot,
        updated_at_ms: i64,
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO worker_progress (
                    id, state, current_file_path, current_file_sequence,
                    processed_blocks, total_blocks, processed_events, total_events,
                    processed_compressed_bytes, total_compressed_bytes,
                    heartbeat_at_ms, last_error, last_error_at_ms, updated_at_ms
                 ) VALUES (
                    'current', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13
                 )
                 ON CONFLICT(id) DO UPDATE SET
                    state = excluded.state,
                    current_file_path = excluded.current_file_path,
                    current_file_sequence = excluded.current_file_sequence,
                    processed_blocks = excluded.processed_blocks,
                    total_blocks = excluded.total_blocks,
                    processed_events = excluded.processed_events,
                    total_events = excluded.total_events,
                    processed_compressed_bytes = excluded.processed_compressed_bytes,
                    total_compressed_bytes = excluded.total_compressed_bytes,
                    heartbeat_at_ms = excluded.heartbeat_at_ms,
                    last_error = excluded.last_error,
                    last_error_at_ms = excluded.last_error_at_ms,
                    updated_at_ms = excluded.updated_at_ms",
                params![
                    progress.state,
                    progress.current_file_path,
                    progress.current_file_sequence.map(|value| value as i64),
                    progress.processed_blocks as i64,
                    progress.total_blocks as i64,
                    progress.processed_events as i64,
                    progress.total_events as i64,
                    progress.processed_compressed_bytes as i64,
                    progress.total_compressed_bytes as i64,
                    progress.heartbeat_at_ms,
                    progress.last_error,
                    progress.last_error_at_ms,
                    updated_at_ms,
                ],
            )
            .context("update usage worker progress")?;
        Ok(())
    }

    /// Load current worker progress.
    pub fn progress_snapshot(&self) -> Result<WorkerProgressSnapshot> {
        self.conn
            .query_row(
                "SELECT state, current_file_path, current_file_sequence,
                    processed_blocks, total_blocks, processed_events, total_events,
                    processed_compressed_bytes, total_compressed_bytes,
                    heartbeat_at_ms, last_error, last_error_at_ms
                 FROM worker_progress
                 WHERE id = 'current'",
                [],
                decode_progress,
            )
            .optional()
            .context("load usage worker progress")
            .map(|progress| progress.unwrap_or_else(idle_progress))
    }
}

fn initialize_consumer_state(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS consumed_files (
            file_sequence INTEGER PRIMARY KEY,
            file_digest TEXT NOT NULL,
            event_count INTEGER NOT NULL,
            imported_at_ms INTEGER NOT NULL
        ) STRICT, WITHOUT ROWID;

        CREATE TABLE IF NOT EXISTS worker_progress (
            id TEXT PRIMARY KEY CHECK (id = 'current'),
            state TEXT NOT NULL,
            current_file_path TEXT,
            current_file_sequence INTEGER,
            processed_blocks INTEGER NOT NULL,
            total_blocks INTEGER NOT NULL,
            processed_events INTEGER NOT NULL,
            total_events INTEGER NOT NULL,
            processed_compressed_bytes INTEGER NOT NULL,
            total_compressed_bytes INTEGER NOT NULL,
            heartbeat_at_ms INTEGER,
            last_error TEXT,
            last_error_at_ms INTEGER,
            updated_at_ms INTEGER NOT NULL
        ) STRICT, WITHOUT ROWID;",
    )
    .context("initialize usage journal consumer state")?;
    Ok(())
}

fn decode_progress(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkerProgressSnapshot> {
    let processed_events = row.get::<_, i64>(5)?.max(0) as u64;
    let total_events = row.get::<_, i64>(6)?.max(0) as u64;
    let progress_percent = if total_events == 0 {
        0.0
    } else {
        (processed_events as f64 / total_events as f64) * 100.0
    };
    Ok(WorkerProgressSnapshot {
        state: row.get(0)?,
        current_file_path: row.get(1)?,
        current_file_sequence: row
            .get::<_, Option<i64>>(2)?
            .map(|value| value.max(0) as u64),
        processed_blocks: row.get::<_, i64>(3)?.max(0) as u64,
        total_blocks: row.get::<_, i64>(4)?.max(0) as u64,
        processed_events,
        total_events,
        processed_compressed_bytes: row.get::<_, i64>(7)?.max(0) as u64,
        total_compressed_bytes: row.get::<_, i64>(8)?.max(0) as u64,
        progress_percent,
        import_rate_events_per_second: 0.0,
        heartbeat_at_ms: row.get(9)?,
        last_successful_file_sequence: None,
        last_successful_import_at_ms: None,
        last_error: row.get(10)?,
        last_error_at_ms: row.get(11)?,
    })
}

fn idle_progress() -> WorkerProgressSnapshot {
    WorkerProgressSnapshot {
        state: "idle".to_string(),
        ..WorkerProgressSnapshot::default()
    }
}
