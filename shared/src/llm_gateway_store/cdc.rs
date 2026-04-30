//! SQLite-backed CDC outbox for LLM gateway writes.

use std::{
    env,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use rand::RngCore;
use rusqlite::{params, Connection};
use serde::Serialize;

use super::types::now_ms;

const OUTBOX_ENV: &str = "STATICFLOW_LLM_CDC_OUTBOX";
const SOURCE_INSTANCE_ENV: &str = "STATICFLOW_LLM_CDC_SOURCE_INSTANCE";

const OUTBOX_SCHEMA: &str = include_str!("cdc_outbox.sql");

/// One LLM CDC entity name stored in the source outbox.
#[derive(Debug, Clone, Copy)]
pub(super) enum LlmGatewayCdcEntity {
    /// `llm_gateway_keys`.
    Key,
    /// `llm_gateway_runtime_config`.
    RuntimeConfig,
    /// `llm_gateway_account_groups`.
    AccountGroup,
    /// `llm_gateway_proxy_configs`.
    ProxyConfig,
    /// `llm_gateway_proxy_bindings`.
    ProxyBinding,
    /// `llm_gateway_token_requests`.
    TokenRequest,
    /// `llm_gateway_account_contribution_requests`.
    AccountContributionRequest,
    /// `gpt2api_account_contribution_requests`.
    Gpt2ApiAccountContributionRequest,
    /// `llm_gateway_sponsor_requests`.
    SponsorRequest,
    /// `llm_gateway_usage_events`.
    UsageEvent,
}

impl LlmGatewayCdcEntity {
    fn as_str(self) -> &'static str {
        match self {
            Self::Key => "key",
            Self::RuntimeConfig => "runtime_config",
            Self::AccountGroup => "account_group",
            Self::ProxyConfig => "proxy_config",
            Self::ProxyBinding => "proxy_binding",
            Self::TokenRequest => "token_request",
            Self::AccountContributionRequest => "account_contribution_request",
            Self::Gpt2ApiAccountContributionRequest => "gpt2api_account_contribution_request",
            Self::SponsorRequest => "sponsor_request",
            Self::UsageEvent => "usage_event",
        }
    }
}

/// CDC operation recorded in the source outbox.
#[derive(Debug, Clone, Copy)]
pub(super) enum LlmGatewayCdcOperation {
    /// Append-only fact event.
    Append,
    /// Current-state insert or update.
    Upsert,
    /// Current-state deletion.
    Delete,
}

impl LlmGatewayCdcOperation {
    fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }
}

/// Opened source CDC outbox.
#[derive(Clone)]
pub(super) struct LlmGatewayCdcOutbox {
    conn: Arc<Mutex<Connection>>,
    source_instance: Arc<str>,
}

impl LlmGatewayCdcOutbox {
    /// Open the configured source outbox if `STATICFLOW_LLM_CDC_OUTBOX` is set.
    pub(super) fn from_env() -> Result<Option<Self>> {
        let Some(path) = env::var_os(OUTBOX_ENV)
            .map(PathBuf::from)
            .filter(|path| !path.as_os_str().is_empty())
        else {
            return Ok(None);
        };
        let source_instance = env::var(SOURCE_INSTANCE_ENV)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "static-flow-local".to_string());
        Self::open(path, source_instance).map(Some)
    }

    /// Open one source outbox database.
    pub(super) fn open(path: impl AsRef<Path>, source_instance: String) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create cdc outbox parent directory `{}`", parent.display())
            })?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("failed to open cdc outbox `{}`", path.display()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .context("failed to enable cdc outbox foreign keys")?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .context("failed to enable cdc outbox WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")
            .context("failed to set cdc outbox synchronous mode")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .context("failed to configure cdc outbox busy timeout")?;
        conn.execute_batch(OUTBOX_SCHEMA)
            .context("failed to initialize cdc outbox schema")?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            source_instance: Arc::from(source_instance),
        })
    }

    /// Record one row mutation after the LanceDB write has committed.
    pub(super) fn record<T: Serialize + ?Sized>(
        &self,
        entity: LlmGatewayCdcEntity,
        op: LlmGatewayCdcOperation,
        primary_key: &str,
        payload: &T,
    ) -> Result<()> {
        let payload_json =
            serde_json::to_string(payload).context("failed to serialize llm cdc payload")?;
        let committed_at_ms = now_ms();
        let event_id = new_event_id(committed_at_ms);
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cdc outbox mutex poisoned"))?;
        conn.execute(
            "INSERT INTO cdc_outbox (
                event_id, source_instance, entity, op, primary_key, schema_version,
                payload_json, created_at_ms, committed_at_ms
            ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
            params![
                event_id,
                self.source_instance.as_ref(),
                entity.as_str(),
                op.as_str(),
                primary_key,
                payload_json,
                committed_at_ms,
                committed_at_ms,
            ],
        )
        .context("failed to insert llm cdc outbox event")?;
        Ok(())
    }

    /// Record an append-only batch in one SQLite transaction.
    ///
    /// Event ids are derived from entity, operation, and primary key so retries
    /// of the same append batch do not create duplicate CDC events.
    pub(super) fn record_append_batch<T: Serialize>(
        &self,
        entity: LlmGatewayCdcEntity,
        records: &[T],
        primary_key: impl Fn(&T) -> &str,
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("cdc outbox mutex poisoned"))?;
        let tx = conn
            .transaction()
            .context("failed to begin llm cdc outbox transaction")?;
        {
            let mut insert = tx
                .prepare(
                    "INSERT OR IGNORE INTO cdc_outbox (
                        event_id, source_instance, entity, op, primary_key, schema_version,
                        payload_json, created_at_ms, committed_at_ms
                    ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8)",
                )
                .context("failed to prepare llm cdc outbox insert")?;
            for record in records {
                let primary_key = primary_key(record);
                let payload_json =
                    serde_json::to_string(record).context("failed to serialize llm cdc payload")?;
                let committed_at_ms = now_ms();
                let event_id =
                    idempotent_append_event_id(self.source_instance.as_ref(), entity, primary_key);
                insert
                    .execute(params![
                        event_id,
                        self.source_instance.as_ref(),
                        entity.as_str(),
                        LlmGatewayCdcOperation::Append.as_str(),
                        primary_key,
                        payload_json,
                        committed_at_ms,
                        committed_at_ms,
                    ])
                    .context("failed to insert llm cdc outbox batch event")?;
            }
        }
        tx.commit()
            .context("failed to commit llm cdc outbox transaction")?;
        Ok(())
    }
}

#[derive(Serialize)]
pub(super) struct DeletePayload<'a> {
    pub(super) primary_key: &'a str,
}

fn new_event_id(timestamp_ms: i64) -> String {
    let mut random = [0_u8; 8];
    rand::thread_rng().fill_bytes(&mut random);
    format!("llm-cdc-{timestamp_ms}-{:016x}", u64::from_be_bytes(random))
}

fn idempotent_append_event_id(
    source_instance: &str,
    entity: LlmGatewayCdcEntity,
    primary_key: &str,
) -> String {
    format!("llm-cdc-append-{source_instance}-{}-{primary_key}", entity.as_str())
}

#[cfg(test)]
mod tests {
    use super::{DeletePayload, LlmGatewayCdcEntity, LlmGatewayCdcOperation, LlmGatewayCdcOutbox};

    #[derive(serde::Serialize)]
    struct TestRecord {
        id: String,
    }

    #[test]
    fn records_outbox_events_with_monotonic_seq() {
        let dir = std::env::temp_dir()
            .join(format!("staticflow-cdc-test-{}", super::new_event_id(super::now_ms())));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join("cdc.sqlite3");
        let outbox =
            LlmGatewayCdcOutbox::open(&path, "test-source".to_string()).expect("open outbox");

        outbox
            .record(
                LlmGatewayCdcEntity::Key,
                LlmGatewayCdcOperation::Upsert,
                "key-1",
                &TestRecord {
                    id: "key-1".to_string(),
                },
            )
            .expect("record key");
        outbox
            .record(
                LlmGatewayCdcEntity::Key,
                LlmGatewayCdcOperation::Delete,
                "key-1",
                &DeletePayload {
                    primary_key: "key-1",
                },
            )
            .expect("record delete");
        let append_records = [
            TestRecord {
                id: "usage-1".to_string(),
            },
            TestRecord {
                id: "usage-2".to_string(),
            },
        ];
        outbox
            .record_append_batch(LlmGatewayCdcEntity::UsageEvent, &append_records, |record| {
                record.id.as_str()
            })
            .expect("record usage append batch");
        outbox
            .record_append_batch(LlmGatewayCdcEntity::UsageEvent, &append_records, |record| {
                record.id.as_str()
            })
            .expect("retry usage append batch");

        let conn = rusqlite::Connection::open(&path).expect("open sqlite");
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM cdc_outbox", [], |row| row.get(0))
            .expect("count outbox");
        let max_seq: i64 = conn
            .query_row("SELECT MAX(seq) FROM cdc_outbox", [], |row| row.get(0))
            .expect("read max seq");
        assert_eq!(rows, 4);
        assert_eq!(max_seq, 4);

        let _ = std::fs::remove_dir_all(dir);
    }
}
