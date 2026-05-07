//! Journal consumer worker for llm-access usage analytics.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use llm_access_store::duckdb::DuckDbUsageRepository;
use llm_usage_journal::{JournalConsumerState, JournalReader, WorkerProgressSnapshot};
use sha2::{Digest, Sha256};

/// Usage journal consumer.
pub struct UsageWorker {
    journal_root: PathBuf,
    state: JournalConsumerState,
    duckdb_usage: Arc<DuckDbUsageRepository>,
}

impl UsageWorker {
    /// Create a worker for one journal root and one analytics repository.
    pub fn new(
        journal_root: PathBuf,
        duckdb_usage: Arc<DuckDbUsageRepository>,
    ) -> anyhow::Result<Self> {
        for subdir in ["sealed", "consuming", "bad"] {
            fs::create_dir_all(journal_root.join(subdir)).with_context(|| {
                format!("failed to create journal dir `{}`", journal_root.join(subdir).display())
            })?;
        }
        let state = JournalConsumerState::open(&journal_root)?;
        Ok(Self {
            journal_root,
            state,
            duckdb_usage,
        })
    }

    /// Import one sealed journal file if one is available.
    pub async fn run_one_import(&self) -> anyhow::Result<bool> {
        self.import_next(None, true).await
    }

    #[cfg(test)]
    async fn run_until_first_block_commit(&self) -> anyhow::Result<bool> {
        self.import_next(Some(1), false).await
    }

    /// Return current worker progress.
    pub fn progress_snapshot(&self) -> anyhow::Result<WorkerProgressSnapshot> {
        self.state.progress_snapshot()
    }

    async fn import_next(
        &self,
        max_blocks: Option<usize>,
        finalize_file: bool,
    ) -> anyhow::Result<bool> {
        let Some(claim) = self.claim_oldest_sealed_file()? else {
            self.state.update_progress(&idle_progress(), now_ms())?;
            return Ok(false);
        };
        self.import_claimed_file(claim, max_blocks, finalize_file)
            .await
            .map(|()| true)
    }

    fn claim_oldest_sealed_file(&self) -> anyhow::Result<Option<ClaimedJournalFile>> {
        let sealed_dir = self.journal_root.join("sealed");
        if !sealed_dir.exists() {
            return Ok(None);
        }
        let mut candidates = Vec::new();
        for entry in fs::read_dir(&sealed_dir).with_context(|| {
            format!("failed to read sealed journal dir `{}`", sealed_dir.display())
        })? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(sequence) = parse_journal_sequence(&path) else {
                continue;
            };
            candidates.push((sequence, path));
        }
        candidates.sort_by_key(|(sequence, _path)| *sequence);
        for (sequence, sealed_path) in candidates {
            if self.state.is_consumed(sequence)? {
                fs::remove_file(&sealed_path).with_context(|| {
                    format!("failed to delete already consumed journal `{}`", sealed_path.display())
                })?;
                continue;
            }
            let consuming_path = self.journal_root.join("consuming").join(
                sealed_path
                    .file_name()
                    .ok_or_else(|| anyhow!("journal file has no name"))?,
            );
            fs::rename(&sealed_path, &consuming_path).with_context(|| {
                format!(
                    "failed to claim journal `{}` as `{}`",
                    sealed_path.display(),
                    consuming_path.display()
                )
            })?;
            return Ok(Some(ClaimedJournalFile {
                sequence,
                path: consuming_path,
            }));
        }
        Ok(None)
    }

    async fn import_claimed_file(
        &self,
        claim: ClaimedJournalFile,
        max_blocks: Option<usize>,
        finalize_file: bool,
    ) -> anyhow::Result<()> {
        let file_bytes = fs::read(&claim.path)
            .with_context(|| format!("failed to read journal `{}`", claim.path.display()))?;
        let file_digest = sha256_hex(&file_bytes);
        let total_compressed_bytes = file_bytes.len() as u64;
        let batches = JournalReader::open(&claim.path)?.read_all_batches()?;
        let total_blocks = batches.len() as u64;
        let total_events = batches
            .iter()
            .map(|batch| batch.events.len() as u64)
            .sum::<u64>();
        let mut processed_events = 0u64;
        let block_limit = max_blocks.unwrap_or(usize::MAX);

        for (index, batch) in batches.iter().take(block_limit).enumerate() {
            let events = batch
                .events
                .clone()
                .into_iter()
                .map(|event| event.into_usage_event())
                .collect::<Vec<_>>();
            self.duckdb_usage
                .append_usage_events_if_new(&events)
                .await?;
            processed_events = processed_events.saturating_add(events.len() as u64);
            let processed_blocks = index as u64 + 1;
            let progress = importing_progress(
                &claim,
                processed_blocks,
                total_blocks,
                processed_events,
                total_events,
                total_compressed_bytes,
            );
            self.state.update_progress(&progress, now_ms())?;
        }

        if max_blocks.is_some() {
            return Ok(());
        }

        self.state
            .record_consumed_file(claim.sequence, &file_digest, total_events, now_ms())?;
        if finalize_file {
            fs::remove_file(&claim.path).with_context(|| {
                format!("failed to delete consumed journal `{}`", claim.path.display())
            })?;
        }
        let mut progress = idle_progress();
        progress.last_successful_file_sequence = Some(claim.sequence);
        progress.last_successful_import_at_ms = Some(now_ms());
        self.state.update_progress(&progress, now_ms())?;
        Ok(())
    }
}

struct ClaimedJournalFile {
    sequence: u64,
    path: PathBuf,
}

fn importing_progress(
    claim: &ClaimedJournalFile,
    processed_blocks: u64,
    total_blocks: u64,
    processed_events: u64,
    total_events: u64,
    total_compressed_bytes: u64,
) -> WorkerProgressSnapshot {
    let processed_compressed_bytes = if total_blocks == 0 {
        0
    } else {
        total_compressed_bytes.saturating_mul(processed_blocks) / total_blocks
    };
    WorkerProgressSnapshot {
        state: "importing".to_string(),
        current_file_path: Some(claim.path.display().to_string()),
        current_file_sequence: Some(claim.sequence),
        processed_blocks,
        total_blocks,
        processed_events,
        total_events,
        processed_compressed_bytes,
        total_compressed_bytes,
        progress_percent: progress_percent(processed_events, total_events),
        heartbeat_at_ms: Some(now_ms()),
        ..WorkerProgressSnapshot::default()
    }
}

fn idle_progress() -> WorkerProgressSnapshot {
    WorkerProgressSnapshot {
        state: "idle".to_string(),
        heartbeat_at_ms: Some(now_ms()),
        ..WorkerProgressSnapshot::default()
    }
}

fn progress_percent(processed_events: u64, total_events: u64) -> f64 {
    if total_events == 0 {
        0.0
    } else {
        (processed_events as f64 / total_events as f64) * 100.0
    }
}

fn parse_journal_sequence(path: &Path) -> Option<u64> {
    let file_name = path.file_name()?.to_string_lossy();
    let suffix = file_name.strip_prefix("usage-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Run the import loop until the process is stopped.
pub async fn run_forever(worker: UsageWorker) -> anyhow::Result<()> {
    loop {
        worker.run_one_import().await?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, sync::Arc};

    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::UsageAnalyticsStore,
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };
    use llm_access_store::duckdb::{DuckDbUsageRepository, TieredDuckDbUsageConfig};
    use llm_usage_journal::{JournalConfig, JournalReader, JournalWriter};

    use super::UsageWorker;

    #[tokio::test]
    async fn worker_imports_sealed_journal_and_deletes_file() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-worker-1");

        fixture.run_one_import().await.expect("import");

        assert!(!fixture.sealed_path(0).exists());
        assert!(fixture.duckdb_event_exists("evt-worker-1").await);
    }

    #[tokio::test]
    async fn worker_progress_updates_after_each_committed_block() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_events_in_two_blocks(["evt-progress-1", "evt-progress-2"]);

        fixture
            .run_until_first_block_commit()
            .await
            .expect("first block");
        let progress = fixture.progress_snapshot();

        assert_eq!(progress.state, "importing");
        assert_eq!(progress.processed_blocks, 1);
        assert_eq!(progress.total_blocks, 2);
        assert!(progress.progress_percent > 0.0);
    }

    #[tokio::test]
    async fn worker_retry_does_not_duplicate_event_id() {
        let fixture = UsageWorkerFixture::new();
        fixture.write_sealed_event("evt-idempotent");
        fixture
            .simulate_commit_before_delete()
            .await
            .expect("commit");

        fixture.run_one_import().await.expect("retry");

        assert_eq!(fixture.count_duckdb_event("evt-idempotent").await, 1);
    }

    struct UsageWorkerFixture {
        _temp_dir: tempfile::TempDir,
        journal_root: PathBuf,
        worker: UsageWorker,
        duckdb: Arc<DuckDbUsageRepository>,
    }

    impl UsageWorkerFixture {
        fn new() -> Self {
            let temp_dir = tempfile::tempdir().expect("tempdir");
            let journal_root = temp_dir.path().join("journal");
            let duckdb = Arc::new(
                DuckDbUsageRepository::open_tiered(TieredDuckDbUsageConfig {
                    active_dir: temp_dir.path().join("duckdb-active"),
                    archive_dir: temp_dir.path().join("duckdb-archive"),
                    catalog_dir: temp_dir.path().join("duckdb-catalog"),
                    rollover_bytes: 1024 * 1024 * 1024,
                })
                .expect("open duckdb"),
            );
            let worker = UsageWorker::new(journal_root.clone(), duckdb.clone()).expect("worker");
            Self {
                _temp_dir: temp_dir,
                journal_root,
                worker,
                duckdb,
            }
        }

        fn write_sealed_event(&self, event_id: &str) {
            self.write_sealed_events([event_id], 1024);
        }

        fn write_sealed_events_in_two_blocks<const N: usize>(&self, event_ids: [&str; N]) {
            self.write_sealed_events(event_ids, 1);
        }

        fn write_sealed_events<const N: usize>(
            &self,
            event_ids: [&str; N],
            block_max_events: usize,
        ) {
            let config = JournalConfig {
                block_max_events,
                ..JournalConfig::new(self.journal_root.clone())
            };
            let mut writer = JournalWriter::open(config).expect("writer");
            let events = event_ids
                .into_iter()
                .map(test_usage_event)
                .collect::<Vec<_>>();
            writer.append_events(&events).expect("append");
            writer.seal_current_file().expect("seal");
        }

        async fn run_one_import(&self) -> anyhow::Result<bool> {
            self.worker.run_one_import().await
        }

        async fn run_until_first_block_commit(&self) -> anyhow::Result<bool> {
            self.worker.run_until_first_block_commit().await
        }

        fn progress_snapshot(&self) -> llm_usage_journal::WorkerProgressSnapshot {
            self.worker.progress_snapshot().expect("progress")
        }

        fn sealed_path(&self, sequence: u64) -> PathBuf {
            self.journal_root
                .join("sealed")
                .join(format!("usage-{sequence:012}.journal"))
        }

        async fn duckdb_event_exists(&self, event_id: &str) -> bool {
            self.duckdb
                .get_usage_event(event_id)
                .await
                .expect("query event")
                .is_some()
        }

        async fn count_duckdb_event(&self, event_id: &str) -> usize {
            usize::from(self.duckdb_event_exists(event_id).await)
        }

        async fn simulate_commit_before_delete(&self) -> anyhow::Result<()> {
            let batches = JournalReader::open(&self.sealed_path(0))?.read_all_batches()?;
            for batch in batches {
                let events = batch
                    .events
                    .into_iter()
                    .map(|event| event.into_usage_event())
                    .collect::<Vec<_>>();
                self.duckdb.append_usage_events_if_new(&events).await?;
            }
            Ok(())
        }
    }

    fn test_usage_event(event_id: &str) -> UsageEvent {
        UsageEvent {
            event_id: event_id.to_string(),
            created_at_ms: 1_700_000_000_000,
            provider_type: ProviderType::Kiro,
            protocol_family: ProtocolFamily::Anthropic,
            key_id: "key-1".to_string(),
            key_name: "for-yangshu".to_string(),
            account_name: Some("acct-1".to_string()),
            account_group_id_at_event: Some("group-1".to_string()),
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "/v1/messages".to_string(),
            endpoint: "/v1/messages".to_string(),
            model: Some("claude-opus-4-7".to_string()),
            mapped_model: Some("claude-opus-4-7".to_string()),
            status_code: 200,
            request_body_bytes: Some(17),
            quota_failover_count: 0,
            routing_diagnostics_json: Some("{\"route\":\"fixed\"}".to_string()),
            input_uncached_tokens: 10,
            input_cached_tokens: 20,
            output_tokens: 30,
            billable_tokens: 40,
            credit_usage: Some("0.12".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{\"user-agent\":\"test\"}".to_string(),
            last_message_content: Some("hello".to_string()),
            client_request_body_json: Some("{\"model\":\"m\"}".to_string()),
            upstream_request_body_json: Some("{\"upstream\":true}".to_string()),
            full_request_json: Some("{\"model\":\"m\"}".to_string()),
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
