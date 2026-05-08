//! Persistent producer-side sequence allocation for usage journal files.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::writer::parse_sequence_from_file_name;

/// Persistent writer state stored under one journal root.
pub struct JournalWriterState {
    conn: Connection,
}

impl JournalWriterState {
    /// Open the writer-state database under one journal root.
    pub fn open(root_dir: &Path) -> Result<Self> {
        fs::create_dir_all(root_dir)
            .with_context(|| format!("failed to create journal root `{}`", root_dir.display()))?;
        let path = root_dir.join("writer-state.sqlite3");
        let conn = Connection::open(&path)
            .with_context(|| format!("failed to open writer state `{}`", path.display()))?;
        initialize_writer_state(&conn)?;
        Ok(Self {
            conn,
        })
    }

    /// Allocate the next globally monotonic file sequence for this journal
    /// root.
    pub fn allocate_next_file_sequence(&mut self, root_dir: &Path) -> Result<u64> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("begin writer-state transaction")?;
        let current = tx
            .query_row(
                "SELECT next_file_sequence
                 FROM writer_state
                 WHERE id = 'current'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .context("load writer-state next sequence")?
            .map(|value| value.max(0) as u64);
        let next_file_sequence = match current {
            Some(value) => value,
            None => {
                let seeded = seed_next_file_sequence(root_dir)?;
                tx.execute(
                    "INSERT INTO writer_state (id, next_file_sequence)
                     VALUES ('current', ?1)",
                    params![seeded as i64],
                )
                .context("seed writer-state next sequence")?;
                seeded
            },
        };
        let following = next_file_sequence.saturating_add(1);
        tx.execute(
            "UPDATE writer_state
             SET next_file_sequence = ?1
             WHERE id = 'current'",
            params![following as i64],
        )
        .context("advance writer-state next sequence")?;
        tx.commit().context("commit writer-state transaction")?;
        Ok(next_file_sequence)
    }
}

fn initialize_writer_state(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS writer_state (
            id TEXT PRIMARY KEY CHECK (id = 'current'),
            next_file_sequence INTEGER NOT NULL
         ) STRICT, WITHOUT ROWID;",
    )
    .context("initialize writer-state schema")?;
    Ok(())
}

fn seed_next_file_sequence(root_dir: &Path) -> Result<u64> {
    let max_existing = max_existing_file_sequence(root_dir)?;
    let max_consumed = max_consumed_file_sequence(root_dir)?;
    let max_seen = match (max_existing, max_consumed) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    };
    Ok(max_seen.map_or(0, |value| value.saturating_add(1)))
}

fn max_existing_file_sequence(root_dir: &Path) -> Result<Option<u64>> {
    let mut max_sequence = None;
    for subdir in ["active", "sealed", "consuming", "bad"] {
        let dir = root_dir.join(subdir);
        if !dir.exists() {
            continue;
        }
        for entry in fs::read_dir(&dir)
            .with_context(|| format!("failed to read journal dir `{}`", dir.display()))?
        {
            let entry = entry?;
            let Some(sequence) =
                parse_sequence_from_file_name(&entry.file_name().to_string_lossy())
            else {
                continue;
            };
            max_sequence =
                Some(max_sequence.map_or(sequence, |current: u64| current.max(sequence)));
        }
    }
    Ok(max_sequence)
}

fn max_consumed_file_sequence(root_dir: &Path) -> Result<Option<u64>> {
    let path = root_dir.join("consumer-state.sqlite3");
    if !path.exists() {
        return Ok(None);
    }
    let conn = Connection::open(&path)
        .with_context(|| format!("failed to open consumer state `{}`", path.display()))?;
    let consumed_table_exists: bool = conn
        .query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM sqlite_master
                WHERE type = 'table' AND name = 'consumed_files'
             )",
            [],
            |row| row.get(0),
        )
        .context("check consumed_files table existence")?;
    if !consumed_table_exists {
        return Ok(None);
    }
    conn.query_row("SELECT MAX(file_sequence) FROM consumed_files", [], |row| {
        row.get::<_, Option<i64>>(0)
    })
    .context("load max consumed journal file sequence")
    .map(|value| value.map(|sequence| sequence.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::JournalWriterState;

    #[test]
    fn writer_state_starts_from_zero_for_empty_root() {
        let root = tempdir().expect("tempdir");
        let mut state = JournalWriterState::open(root.path()).expect("open writer state");

        assert_eq!(
            state
                .allocate_next_file_sequence(root.path())
                .expect("allocate first sequence"),
            0
        );
        assert_eq!(
            state
                .allocate_next_file_sequence(root.path())
                .expect("allocate second sequence"),
            1
        );
    }

    #[test]
    fn writer_state_seeds_from_consumed_files_when_present() {
        let root = tempdir().expect("tempdir");
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

        let mut state = JournalWriterState::open(root.path()).expect("open writer state");
        assert_eq!(
            state
                .allocate_next_file_sequence(root.path())
                .expect("allocate sequence"),
            235
        );
    }

    #[test]
    fn writer_state_seeds_from_existing_journal_files() {
        let root = tempdir().expect("tempdir");
        fs::create_dir_all(root.path().join("sealed")).expect("sealed dir");
        fs::write(root.path().join("sealed/usage-000000000017.journal"), b"x")
            .expect("sealed file");

        let mut state = JournalWriterState::open(root.path()).expect("open writer state");
        assert_eq!(
            state
                .allocate_next_file_sequence(root.path())
                .expect("allocate sequence"),
            18
        );
    }
}
