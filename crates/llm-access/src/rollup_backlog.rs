//! Disk-backed control-rollup backlog for the API process.

use std::{
    fs,
    path::{Path, PathBuf},
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
        Ok(Self {
            config,
            writer: Some(writer),
        })
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

    /// Return all pending batches from sealed backlog files.
    pub(crate) fn read_all_pending_batches(&self) -> anyhow::Result<Vec<UsageRollupBatch>> {
        let mut batches = Vec::new();
        for path in self.pending_sealed_paths()? {
            batches.extend(RollupJournalReader::open(&path)?.read_all_batches()?);
        }
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
        })
    }

    /// Count currently sealed backlog files.
    pub(crate) fn sealed_file_count(&self) -> anyhow::Result<usize> {
        Ok(self.pending_sealed_paths()?.len())
    }

    fn seal_current_writer(&mut self) -> anyhow::Result<()> {
        let old_writer = self
            .writer
            .take()
            .ok_or_else(|| anyhow!("rollup backlog writer is not open"))?;
        let sealed = old_writer.seal_current_file()?;
        tracing::error!(
            path = %sealed.display(),
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
    for name in ["active", "sealed", "consuming"] {
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
            return Err(anyhow!(
                "cannot restore rollup backlog consuming file `{}` because sealed target exists",
                path.display()
            ));
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
