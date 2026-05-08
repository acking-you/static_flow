//! Read-only inspection helpers for journal roots.

use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result};

use crate::{
    status::{JournalFileListsSnapshot, JournalFileSnapshot},
    writer::parse_sequence_from_file_name,
};

/// Collect file-level snapshots for one journal root.
pub fn collect_journal_file_lists(root: &Path) -> Result<JournalFileListsSnapshot> {
    Ok(JournalFileListsSnapshot {
        active: collect_state_files(root, "active")?,
        sealed: collect_state_files(root, "sealed")?,
        consuming: collect_state_files(root, "consuming")?,
        bad: collect_state_files(root, "bad")?,
    })
}

fn collect_state_files(root: &Path, state: &str) -> Result<Vec<JournalFileSnapshot>> {
    let dir = root.join(state);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut files = Vec::new();
    for entry in fs::read_dir(&dir)
        .with_context(|| format!("failed to read journal dir `{}`", dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to stat journal file `{}`", path.display()))?;
        if !metadata.is_file() {
            continue;
        }

        let file_name = entry.file_name().to_string_lossy().into_owned();
        files.push(JournalFileSnapshot {
            sequence: parse_sequence_from_file_name(&file_name),
            file_name,
            path: path.display().to_string(),
            bytes: metadata.len(),
            age_ms: file_age_ms(&metadata),
        });
    }

    files.sort_by(|left, right| {
        left.sequence
            .cmp(&right.sequence)
            .then_with(|| left.file_name.cmp(&right.file_name))
    });
    Ok(files)
}

fn file_age_ms(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let modified_ms = modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as i64;
    Some(now_ms().saturating_sub(modified_ms))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    #[test]
    fn collect_journal_file_lists_groups_files_by_state() {
        let root = tempdir().expect("tempdir");
        for state in ["active", "sealed", "consuming", "bad"] {
            fs::create_dir_all(root.path().join(state)).expect("state dir");
        }
        fs::write(root.path().join("active/usage-000000000001.open"), b"active")
            .expect("active file");
        fs::write(root.path().join("sealed/usage-000000000002.journal"), b"sealed")
            .expect("sealed file");
        fs::write(root.path().join("consuming/usage-000000000003.journal"), b"consuming")
            .expect("consuming file");
        fs::write(root.path().join("bad/usage-000000000004.journal"), b"bad").expect("bad file");

        let files = super::collect_journal_file_lists(root.path()).expect("file lists");
        assert_eq!(files.active.len(), 1);
        assert_eq!(files.sealed.len(), 1);
        assert_eq!(files.consuming.len(), 1);
        assert_eq!(files.bad.len(), 1);
        assert_eq!(files.sealed[0].sequence, Some(2));
    }
}
