//! Disk-backed control-rollup backlog for the API process.

use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use llm_access_core::store::UsageRollupBatch;
use llm_usage_journal::{
    recover_orphan_active_rollup_files, rollup::parse_rollup_sequence_from_file_name,
    JournalConfig, RollupJournalReader, RollupJournalWriter,
};

use crate::usage_journal::journal_config_from_runtime;

/// Durable backlog rooted under the usage journal directory.
#[derive(Debug)]
pub(crate) struct UsageRollupBacklog {
    config: JournalConfig,
    writer: Option<RollupJournalWriter>,
}

/// One claimed rollup backlog file currently being applied.
#[derive(Debug)]
pub(crate) struct ClaimedRollupBacklogFile {
    path: PathBuf,
    sealed_path: PathBuf,
}

/// Result of scanning durable control-rollup backlog files.
#[derive(Debug, Clone, Default)]
pub(crate) struct UsageRollupBacklogLoadReport {
    pub(crate) sealed_file_count: usize,
    pub(crate) loaded_file_count: usize,
    pub(crate) loaded_batch_count: usize,
    pub(crate) quarantined_file_count: usize,
    pub(crate) bad_file_samples: Vec<String>,
}

impl ClaimedRollupBacklogFile {
    /// Current consuming path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

impl UsageRollupBacklog {
    /// Open the disk backlog and restore any abandoned claims.
    pub(crate) fn open(
        usage_journal_dir: PathBuf,
        runtime_config: &llm_access_core::store::AdminRuntimeConfig,
    ) -> anyhow::Result<Self> {
        let root_dir = usage_journal_dir.join("control-rollups");
        let config = journal_config_from_runtime(root_dir, runtime_config);
        create_dirs(&config.root_dir)?;
        let recovery = recover_orphan_active_rollup_files(&config)?;
        if recovery.recovered_files > 0
            || recovery.deleted_empty_files > 0
            || recovery.quarantined_files > 0
        {
            tracing::warn!(
                recovered_files = recovery.recovered_files,
                deleted_empty_files = recovery.deleted_empty_files,
                quarantined_files = recovery.quarantined_files,
                "completed orphan active control rollup backlog recovery"
            );
        }
        restore_consuming_files(&config.root_dir)?;
        let writer = RollupJournalWriter::open(config.clone())?;
        let backlog = Self {
            config,
            writer: Some(writer),
        };
        tracing::info!(
            root = %backlog.config.root_dir.display(),
            sealed_file_count = backlog.sealed_file_count().unwrap_or(0),
            "opened control rollup disk backlog"
        );
        Ok(backlog)
    }

    /// Open a test backlog under a temporary root.
    #[cfg(test)]
    pub(crate) fn open_for_tests(root_dir: PathBuf) -> anyhow::Result<Self> {
        let runtime_config = llm_access_core::store::AdminRuntimeConfig::default();
        let config = journal_config_from_runtime(root_dir, &runtime_config);
        create_dirs(&config.root_dir)?;
        let _ = recover_orphan_active_rollup_files(&config)?;
        restore_consuming_files(&config.root_dir)?;
        let writer = RollupJournalWriter::open(config.clone())?;
        Ok(Self {
            config,
            writer: Some(writer),
        })
    }

    /// Append batches and immediately seal them so the replay path only scans
    /// immutable files.
    pub(crate) fn append_batches(&mut self, batches: &[UsageRollupBatch]) -> anyhow::Result<()> {
        if batches.is_empty() {
            return Ok(());
        }
        let writer = self
            .writer
            .as_mut()
            .ok_or_else(|| anyhow!("rollup backlog writer is not open"))?;
        writer.append_batches(batches)?;
        writer.flush()?;
        self.seal_current_writer()?;
        Ok(())
    }

    /// Stream pending batches from sealed files one file at a time. Unreadable
    /// files are quarantined so one corrupt backlog cannot block startup.
    pub(crate) fn for_each_pending_batch(
        &self,
        mut on_batch: impl FnMut(&UsageRollupBatch) -> anyhow::Result<()>,
    ) -> anyhow::Result<UsageRollupBacklogLoadReport> {
        let mut report = UsageRollupBacklogLoadReport::default();
        let paths = self.pending_sealed_paths()?;
        report.sealed_file_count = paths.len();
        for path in paths {
            let path_display = path.display().to_string();
            let batches = match RollupJournalReader::open(&path)
                .and_then(|reader| reader.read_all_batches())
            {
                Ok(batches) => batches,
                Err(err) => {
                    let bad_path = self.quarantine_sealed_path(&path)?;
                    report.quarantined_file_count = report.quarantined_file_count.saturating_add(1);
                    push_sample(&mut report.bad_file_samples, bad_path.display().to_string());
                    tracing::error!(
                        path = %path_display,
                        bad_path = %bad_path.display(),
                        "quarantined unreadable control rollup backlog file during startup scan: \
                         {err:#}"
                    );
                    continue;
                },
            };
            for batch in &batches {
                on_batch(batch)?;
            }
            report.loaded_file_count = report.loaded_file_count.saturating_add(1);
            report.loaded_batch_count = report.loaded_batch_count.saturating_add(batches.len());
        }
        Ok(report)
    }

    /// Return all pending batches from sealed backlog files.
    #[cfg(test)]
    pub(crate) fn read_all_pending_batches(&self) -> anyhow::Result<Vec<UsageRollupBatch>> {
        let mut batches = Vec::new();
        let _ = self.for_each_pending_batch(|batch| {
            batches.push(batch.clone());
            Ok(())
        })?;
        Ok(batches)
    }

    /// Claim the oldest sealed backlog file for application.
    pub(crate) fn claim_next(&self) -> anyhow::Result<Option<ClaimedRollupBacklogFile>> {
        let Some(sealed_path) = self.pending_sealed_paths()?.into_iter().next() else {
            return Ok(None);
        };
        let file_name = sealed_path
            .file_name()
            .ok_or_else(|| anyhow!("sealed rollup backlog path has no file name"))?;
        let consuming_path = self.config.root_dir.join("consuming").join(file_name);
        fs::rename(&sealed_path, &consuming_path).with_context(|| {
            format!(
                "failed to claim rollup backlog `{}` to `{}`",
                sealed_path.display(),
                consuming_path.display()
            )
        })?;
        tracing::warn!(
            path = %consuming_path.display(),
            sealed_path = %sealed_path.display(),
            "claimed control rollup backlog file for replay"
        );
        Ok(Some(ClaimedRollupBacklogFile {
            path: consuming_path,
            sealed_path,
        }))
    }

    /// Read batches from a claimed backlog file.
    pub(crate) fn read_claim(
        &self,
        claim: &ClaimedRollupBacklogFile,
    ) -> anyhow::Result<Vec<UsageRollupBatch>> {
        RollupJournalReader::open(&claim.path)?.read_all_batches()
    }

    /// Mark a claim as applied by deleting its consuming file.
    pub(crate) fn complete_claim(&self, claim: ClaimedRollupBacklogFile) -> anyhow::Result<()> {
        fs::remove_file(&claim.path).with_context(|| {
            format!("failed to delete applied rollup backlog `{}`", claim.path.display())
        })
    }

    /// Return a failed claim to the sealed queue.
    pub(crate) fn restore_claim(&self, claim: ClaimedRollupBacklogFile) -> anyhow::Result<()> {
        if claim.sealed_path.exists() {
            return Err(anyhow!(
                "cannot restore rollup backlog claim `{}` because sealed target `{}` exists",
                claim.path.display(),
                claim.sealed_path.display()
            ));
        }
        fs::rename(&claim.path, &claim.sealed_path).with_context(|| {
            format!(
                "failed to restore rollup backlog `{}` to `{}`",
                claim.path.display(),
                claim.sealed_path.display()
            )
        })?;
        tracing::warn!(
            path = %claim.path.display(),
            sealed_path = %claim.sealed_path.display(),
            "restored control rollup backlog claim after retryable failure"
        );
        Ok(())
    }

    /// Move an unreadable claimed file out of the replay queue.
    pub(crate) fn quarantine_claim(
        &self,
        claim: ClaimedRollupBacklogFile,
    ) -> anyhow::Result<PathBuf> {
        self.quarantine_path(&claim.path)
    }

    /// Count currently sealed backlog files.
    pub(crate) fn sealed_file_count(&self) -> anyhow::Result<usize> {
        Ok(self.pending_sealed_paths()?.len())
    }

    fn quarantine_sealed_path(&self, path: &Path) -> anyhow::Result<PathBuf> {
        self.quarantine_path(path)
    }

    fn quarantine_path(&self, path: &Path) -> anyhow::Result<PathBuf> {
        let bad_path = unique_bad_path(&self.config.root_dir, path)?;
        fs::rename(path, &bad_path).with_context(|| {
            format!(
                "failed to quarantine rollup backlog `{}` to `{}`",
                path.display(),
                bad_path.display()
            )
        })?;
        Ok(bad_path)
    }

    fn seal_current_writer(&mut self) -> anyhow::Result<()> {
        let old_writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("rollup backlog writer is not open"))?;
        let sealed = old_writer.seal_current_file()?;
        let sealed_file_count = self.sealed_file_count().unwrap_or(0);
        tracing::error!(
            path = %sealed.display(),
            sealed_file_count,
            "persisted failed control rollup batch to disk backlog"
        );
        self.writer = Some(RollupJournalWriter::open(self.config.clone())?);
        Ok(())
    }

    fn pending_sealed_paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        let sealed_dir = self.config.root_dir.join("sealed");
        let mut paths = Vec::new();
        for entry in fs::read_dir(&sealed_dir).with_context(|| {
            format!("failed to list rollup backlog dir `{}`", sealed_dir.display())
        })? {
            let entry = entry.with_context(|| {
                format!("failed to read rollup backlog dir entry `{}`", sealed_dir.display())
            })?;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if parse_rollup_sequence_from_file_name(file_name).is_some() {
                paths.push(path);
            }
        }
        paths.sort_by_key(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .and_then(parse_rollup_sequence_from_file_name)
                .unwrap_or(u64::MAX)
        });
        Ok(paths)
    }
}

fn create_dirs(root: &Path) -> anyhow::Result<()> {
    for name in ["active", "sealed", "consuming", "bad"] {
        let path = root.join(name);
        fs::create_dir_all(&path)
            .with_context(|| format!("failed to create rollup backlog dir `{}`", path.display()))?;
    }
    Ok(())
}

fn restore_consuming_files(root: &Path) -> anyhow::Result<()> {
    let consuming_dir = root.join("consuming");
    let sealed_dir = root.join("sealed");
    for entry in fs::read_dir(&consuming_dir).with_context(|| {
        format!("failed to list rollup backlog consuming dir `{}`", consuming_dir.display())
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read rollup backlog consuming dir entry `{}`",
                consuming_dir.display()
            )
        })?;
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if parse_rollup_sequence_from_file_name(file_name).is_none() {
            continue;
        }
        let sealed_path = sealed_dir.join(file_name);
        if sealed_path.exists() {
            let bad_path = unique_bad_path(root, &path)?;
            fs::rename(&path, &bad_path).with_context(|| {
                format!(
                    "failed to quarantine duplicate rollup backlog consuming file `{}` to `{}`",
                    path.display(),
                    bad_path.display()
                )
            })?;
            tracing::error!(
                path = %path.display(),
                sealed_path = %sealed_path.display(),
                bad_path = %bad_path.display(),
                "quarantined abandoned control rollup backlog claim because sealed target already \
                 exists"
            );
            continue;
        }
        fs::rename(&path, &sealed_path).with_context(|| {
            format!(
                "failed to restore rollup backlog consuming file `{}` to `{}`",
                path.display(),
                sealed_path.display()
            )
        })?;
        tracing::warn!(
            path = %sealed_path.display(),
            "restored abandoned control rollup backlog claim"
        );
    }
    Ok(())
}

fn unique_bad_path(root: &Path, source_path: &Path) -> anyhow::Result<PathBuf> {
    let file_name = source_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            anyhow!("rollup backlog path `{}` has no file name", source_path.display())
        })?;
    let bad_dir = root.join("bad");
    fs::create_dir_all(&bad_dir).with_context(|| {
        format!("failed to create rollup backlog bad dir `{}`", bad_dir.display())
    })?;
    let candidate = bad_dir.join(file_name);
    if !candidate.exists() {
        return Ok(candidate);
    }
    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    for suffix in 1..=1024 {
        let candidate = bad_dir.join(format!("{file_name}.bad-{now_ms}-{suffix}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(anyhow!(
        "failed to find unique quarantine path for rollup backlog `{}`",
        source_path.display()
    ))
}

fn push_sample(samples: &mut Vec<String>, value: String) {
    if samples.len() >= 8 || samples.iter().any(|existing| existing == &value) {
        return;
    }
    samples.push(value);
}

#[cfg(test)]
mod tests {
    use llm_access_core::store::{KeyUsageRollupDelta, UsageRollupBatch};

    use super::UsageRollupBacklog;

    fn test_rollup_batch(batch_id: &str) -> UsageRollupBatch {
        UsageRollupBatch {
            batch_id: batch_id.to_string(),
            source_node_id: Some("node-test".to_string()),
            created_at_ms: 1_700_000_000_000,
            source_event_count: 1,
            deltas: vec![KeyUsageRollupDelta {
                key_id: "key-runtime".to_string(),
                input_uncached_tokens: 10,
                input_cached_tokens: 0,
                output_tokens: 2,
                billable_tokens: 12,
                credit_total: 0.0,
                credit_missing_events: 0,
                last_used_at_ms: Some(1_700_000_000_000),
            }],
            last_used_at_ms_counts: Vec::new(),
        }
    }

    #[test]
    fn read_all_pending_batches_quarantines_unreadable_sealed_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let mut backlog =
            UsageRollupBacklog::open_for_tests(root.path().to_path_buf()).expect("open backlog");
        backlog
            .append_batches(&[test_rollup_batch("rollup-readable")])
            .expect("append readable batch");
        let corrupt_path = root.path().join("sealed/rollup-999999999999.journal");
        std::fs::write(&corrupt_path, b"not a rollup journal").expect("write corrupt file");

        let batches = backlog
            .read_all_pending_batches()
            .expect("corrupt sealed files should be quarantined");

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].batch_id, "rollup-readable");
        assert!(!corrupt_path.exists());
        assert!(root.path().join("bad/rollup-999999999999.journal").exists());
    }

    #[test]
    fn quarantine_claim_moves_bad_claim_out_of_replay_queue() {
        let root = tempfile::tempdir().expect("tempdir");
        let backlog =
            UsageRollupBacklog::open_for_tests(root.path().to_path_buf()).expect("open backlog");
        let corrupt_path = root.path().join("sealed/rollup-000000000001.journal");
        std::fs::write(&corrupt_path, b"not a rollup journal").expect("write corrupt file");

        let claim = backlog
            .claim_next()
            .expect("claim")
            .expect("claim should exist");
        assert!(backlog.read_claim(&claim).is_err());
        let bad_path = backlog
            .quarantine_claim(claim)
            .expect("quarantine bad claim");

        assert!(bad_path.exists());
        assert_eq!(backlog.sealed_file_count().expect("sealed count"), 0);
    }
}
