//! Usage event journal sink for the API process.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use llm_access_core::{
    store::{AdminRuntimeConfig, UsageEventSink},
    usage::UsageEvent,
};
use llm_usage_journal::{
    recover_orphan_active_files, retention, JournalConfig, JournalStatusSnapshot, JournalWriter,
};

/// Writer abstraction used to keep diagnostic write failures non-fatal.
pub(crate) trait JournalUsageWriter: Send {
    /// Append a batch of usage events.
    fn append_usage_events(&mut self, events: &[UsageEvent]) -> anyhow::Result<()>;

    /// Run timer-driven maintenance such as idle rollover.
    fn maintain(&mut self) -> anyhow::Result<()>;

    /// Build a status snapshot.
    fn status_snapshot(&self, write_failures_total: u64) -> anyhow::Result<JournalStatusSnapshot>;
}

/// Usage-event sink that writes compact local journal files.
pub(crate) struct JournalUsageEventSink {
    writer: Mutex<Box<dyn JournalUsageWriter>>,
    write_failures_total: AtomicU64,
}

impl JournalUsageEventSink {
    /// Open a journal sink using the current runtime settings.
    pub(crate) fn open(
        root_dir: PathBuf,
        runtime_config: &AdminRuntimeConfig,
    ) -> anyhow::Result<Self> {
        Ok(Self::from_writer(DiskJournalUsageWriter::open(root_dir, runtime_config)?))
    }

    #[cfg(test)]
    pub(crate) fn open_for_tests(root_dir: PathBuf) -> anyhow::Result<Self> {
        Self::open(root_dir, &AdminRuntimeConfig::default())
    }

    pub(crate) fn from_writer<W>(writer: W) -> Self
    where
        W: JournalUsageWriter + 'static,
    {
        Self {
            writer: Mutex::new(Box::new(writer)),
            write_failures_total: AtomicU64::new(0),
        }
    }

    /// Return producer-side journal status.
    pub(crate) fn status_snapshot(&self) -> anyhow::Result<JournalStatusSnapshot> {
        let writer = self
            .writer
            .lock()
            .map_err(|_| anyhow!("usage journal writer mutex poisoned"))?;
        writer.status_snapshot(self.write_failures_total.load(Ordering::Relaxed))
    }

    /// Run producer-side maintenance without requiring new usage events.
    pub(crate) fn maintain(&self) {
        let result = self
            .writer
            .lock()
            .map_err(|_| anyhow!("usage journal writer mutex poisoned"))
            .and_then(|mut writer| writer.maintain());
        if let Err(err) = result {
            self.write_failures_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!("llm access usage journal maintenance failed: {err:#}");
        }
    }
}

#[async_trait]
impl UsageEventSink for JournalUsageEventSink {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.append_usage_events(std::slice::from_ref(event)).await
    }

    async fn append_usage_events(&self, events: &[UsageEvent]) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let result = self
            .writer
            .lock()
            .map_err(|_| anyhow!("usage journal writer mutex poisoned"))
            .and_then(|mut writer| writer.append_usage_events(events));
        if let Err(err) = result {
            self.write_failures_total.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                count = events.len(),
                "dropped llm access usage journal events after rollup persistence: {err:#}"
            );
        }
        Ok(())
    }
}

struct DiskJournalUsageWriter {
    enabled: bool,
    config: JournalConfig,
    writer: JournalWriter,
}

impl DiskJournalUsageWriter {
    fn open(root_dir: PathBuf, runtime_config: &AdminRuntimeConfig) -> anyhow::Result<Self> {
        let config = journal_config_from_runtime(root_dir, runtime_config);
        let recovery = recover_orphan_active_files(&config)?;
        if recovery.recovered_files > 0
            || recovery.deleted_empty_files > 0
            || recovery.quarantined_files > 0
        {
            tracing::info!(
                recovered_files = recovery.recovered_files,
                deleted_empty_files = recovery.deleted_empty_files,
                quarantined_files = recovery.quarantined_files,
                "completed orphan active usage journal recovery before opening producer"
            );
        }
        Ok(Self {
            enabled: runtime_config.usage_journal_enabled,
            writer: JournalWriter::open(config.clone())?,
            config,
        })
    }

    fn should_seal(&self) -> anyhow::Result<bool> {
        if !self.writer.has_written_events() {
            return Ok(false);
        }
        let active_file_bytes = self.writer.active_file_bytes()?;
        let age_ms = now_ms().saturating_sub(self.writer.created_at_ms());
        Ok(active_file_bytes >= self.config.max_file_bytes
            || age_ms as u64 >= self.config.max_file_age_ms)
    }

    fn seal_current_writer(&mut self) -> anyhow::Result<()> {
        let old_writer =
            std::mem::replace(&mut self.writer, JournalWriter::open(self.config.clone())?);
        let sealed = old_writer.seal_current_file()?;
        tracing::debug!(path = %sealed.display(), "sealed llm access usage journal");
        let _report = retention::enforce_retention(&self.config)?;
        Ok(())
    }
}

impl JournalUsageWriter for DiskJournalUsageWriter {
    fn append_usage_events(&mut self, events: &[UsageEvent]) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.writer.append_events(events)?;
        self.writer.flush()?;
        if self.should_seal()? {
            self.seal_current_writer()?;
        }
        Ok(())
    }

    fn maintain(&mut self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if self.should_seal()? {
            self.seal_current_writer()?;
        }
        Ok(())
    }

    fn status_snapshot(&self, write_failures_total: u64) -> anyhow::Result<JournalStatusSnapshot> {
        let sealed = sealed_stats(&self.config.root_dir)?;
        Ok(JournalStatusSnapshot {
            journal_enabled: self.enabled,
            journal_root: self.config.root_dir.display().to_string(),
            active_file_sequence: Some(self.writer.active_file_sequence()),
            active_file_bytes: self.writer.active_file_bytes().unwrap_or(0),
            sealed_file_count: sealed.file_count,
            sealed_bytes: sealed.bytes,
            oldest_sealed_age_ms: sealed.oldest_age_ms,
            dropped_files_total: 0,
            dropped_unconsumed_files_total: 0,
            write_failures_total,
        })
    }
}

fn journal_config_from_runtime(
    root_dir: PathBuf,
    runtime_config: &AdminRuntimeConfig,
) -> JournalConfig {
    JournalConfig {
        root_dir,
        max_file_bytes: runtime_config.usage_journal_max_file_bytes.max(1),
        max_file_age_ms: runtime_config.usage_journal_max_file_age_ms.max(1),
        max_files: usize::try_from(runtime_config.usage_journal_max_files.max(1))
            .unwrap_or(usize::MAX),
        block_target_uncompressed_bytes: usize::try_from(
            runtime_config
                .usage_journal_block_target_uncompressed_bytes
                .max(1),
        )
        .unwrap_or(usize::MAX),
        block_max_events: usize::try_from(runtime_config.usage_journal_block_max_events.max(1))
            .unwrap_or(usize::MAX),
        fsync_interval_ms: runtime_config.usage_journal_fsync_interval_ms,
        zstd_level: i32::try_from(runtime_config.usage_journal_zstd_level).unwrap_or(3),
        consumer_lease_ms: runtime_config.usage_journal_consumer_lease_ms.max(1),
        delete_bad_files: runtime_config.usage_journal_delete_bad_files,
    }
}

#[derive(Default)]
struct SealedJournalStats {
    file_count: u64,
    bytes: u64,
    oldest_age_ms: Option<i64>,
}

fn sealed_stats(root_dir: &Path) -> anyhow::Result<SealedJournalStats> {
    let sealed_dir = root_dir.join("sealed");
    if !sealed_dir.exists() {
        return Ok(SealedJournalStats::default());
    }
    let mut stats = SealedJournalStats::default();
    for entry in fs::read_dir(&sealed_dir)
        .with_context(|| format!("failed to read sealed journal dir `{}`", sealed_dir.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        stats.file_count = stats.file_count.saturating_add(1);
        stats.bytes = stats.bytes.saturating_add(metadata.len());
        if let Ok(modified) = metadata.modified() {
            if let Ok(duration) = modified.duration_since(UNIX_EPOCH) {
                let age_ms = now_ms().saturating_sub(duration.as_millis() as i64);
                stats.oldest_age_ms = Some(
                    stats
                        .oldest_age_ms
                        .map_or(age_ms, |oldest| oldest.max(age_ms)),
                );
            }
        }
    }
    Ok(stats)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use llm_access_core::{
        provider::{ProtocolFamily, ProviderType},
        store::{AdminRuntimeConfig, UsageEventSink},
        usage::{UsageEvent, UsageStreamDetails, UsageTiming},
    };

    use super::{JournalUsageEventSink, JournalUsageWriter};

    #[tokio::test]
    async fn journal_sink_writes_event_without_duckdb() {
        let root = tempfile::tempdir().expect("tempdir");
        let sink =
            JournalUsageEventSink::open_for_tests(root.path().to_path_buf()).expect("open sink");

        sink.append_usage_event(&test_usage_event("evt-api-journal"))
            .await
            .expect("append");

        let status = sink.status_snapshot().expect("status");
        assert_eq!(status.active_file_sequence, Some(0));
        assert_eq!(status.write_failures_total, 0);
    }

    #[tokio::test]
    async fn journal_sink_drops_diagnostic_event_on_write_failure() {
        let sink = JournalUsageEventSink::from_writer(FailingJournalWriter);

        sink.append_usage_event(&test_usage_event("evt-drop"))
            .await
            .expect("diagnostic drop is non-fatal");

        let status = sink.status_snapshot().expect("status");
        assert_eq!(status.write_failures_total, 1);
    }

    #[tokio::test]
    async fn journal_sink_startup_recovers_orphan_active_files() {
        let root = tempfile::tempdir().expect("tempdir");
        let config = llm_usage_journal::JournalConfig::new(root.path().to_path_buf());
        let mut writer = llm_usage_journal::JournalWriter::open(config.clone()).expect("writer 0");
        writer
            .append_events(&[test_usage_event("evt-sealed")])
            .expect("append sealed");
        writer.seal_current_file().expect("seal writer 0");

        let mut orphan_writer =
            llm_usage_journal::JournalWriter::open(config).expect("writer orphan");
        orphan_writer
            .append_events(&[test_usage_event("evt-orphan-active")])
            .expect("append orphan");
        orphan_writer.flush().expect("flush orphan");
        drop(orphan_writer);
        std::fs::remove_file(root.path().join("writer-state.sqlite3")).expect("drop writer state");
        let conn =
            rusqlite::Connection::open(root.path().join("consumer-state.sqlite3")).expect("sqlite");
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

        let sink =
            JournalUsageEventSink::open_for_tests(root.path().to_path_buf()).expect("open sink");
        let status = sink.status_snapshot().expect("status");

        assert_eq!(status.active_file_sequence, Some(236));
        let active_entries = std::fs::read_dir(root.path().join("active"))
            .expect("read active dir")
            .count();
        assert_eq!(active_entries, 1);
        let sealed_entries = std::fs::read_dir(root.path().join("sealed"))
            .expect("read sealed dir")
            .count();
        assert_eq!(sealed_entries, 2);
    }

    #[tokio::test]
    async fn journal_sink_maintenance_seals_aged_active_file_without_new_events() {
        let root = tempfile::tempdir().expect("tempdir");
        let runtime_config = AdminRuntimeConfig {
            usage_journal_max_file_age_ms: 1,
            ..AdminRuntimeConfig::default()
        };
        let sink = JournalUsageEventSink::open(root.path().to_path_buf(), &runtime_config)
            .expect("open sink");

        sink.append_usage_event(&test_usage_event("evt-aged"))
            .await
            .expect("append");
        std::thread::sleep(std::time::Duration::from_millis(10));
        sink.maintain();

        let status = sink.status_snapshot().expect("status");
        assert_eq!(status.sealed_file_count, 1);
        assert_eq!(status.active_file_sequence, Some(1));
    }

    struct FailingJournalWriter;

    impl JournalUsageWriter for FailingJournalWriter {
        fn append_usage_events(&mut self, _events: &[UsageEvent]) -> anyhow::Result<()> {
            anyhow::bail!("intentional journal failure")
        }

        fn maintain(&mut self) -> anyhow::Result<()> {
            Ok(())
        }

        fn status_snapshot(
            &self,
            write_failures_total: u64,
        ) -> anyhow::Result<llm_usage_journal::JournalStatusSnapshot> {
            Ok(llm_usage_journal::JournalStatusSnapshot {
                write_failures_total,
                ..llm_usage_journal::JournalStatusSnapshot::default()
            })
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
            error_message: None,
            error_body: None,
            timing: UsageTiming::default(),
            stream: UsageStreamDetails::default(),
        }
    }
}
