//! Usage-event append path into the tiered store, key-rollup
//! aggregation, and detail publish.

use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
    sync::{Arc, Mutex},
};

use anyhow::{anyhow, Context};
use llm_access_core::usage::UsageEvent;

use super::{
    connection::connection_config_snapshot,
    retention::{duckdb_wal_path, rollover_active_segment},
    DuckDbUsageRepository, PersistentUsageWriter, SharedDuckDbUsageConnectionConfig,
    TieredDuckDbUsageConfig, TieredDuckDbUsageState, TieredUsageCatalogBackend, UsageEventRow,
};
use crate::KeyUsageRollupSummary;

#[cfg(feature = "duckdb-runtime")]
pub fn key_usage_rollups_from_path(path: &Path) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let conn = DuckDbUsageRepository::open_read_only_conn(path)?;
    key_usage_rollups_from_conn(&conn)
}
#[cfg(feature = "duckdb-runtime")]
fn key_usage_rollups_from_conn(
    conn: &duckdb::Connection,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut stmt = conn
        .prepare(
            "SELECT
                key_id,
                CAST(COALESCE(sum(input_uncached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(input_cached_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(output_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(billable_tokens), 0) AS BIGINT),
                CAST(COALESCE(sum(COALESCE(try_cast(credit_usage AS DOUBLE), 0)), 0) AS VARCHAR),
                CAST(COALESCE(sum(CASE WHEN credit_usage_missing THEN 1 ELSE 0 END), 0) AS BIGINT),
                max(created_at_ms)
             FROM usage_events
             GROUP BY key_id",
        )
        .context("prepare duckdb key usage rollup query")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(KeyUsageRollupSummary {
                key_id: row.get(0)?,
                input_uncached_tokens: row.get(1)?,
                input_cached_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                billable_tokens: row.get(4)?,
                credit_total: row.get(5)?,
                credit_missing_events: row.get(6)?,
                last_used_at_ms: row.get(7)?,
            })
        })
        .context("query duckdb key usage rollups")?;
    rows.collect::<Result<Vec<_>, _>>()
        .context("collect duckdb key usage rollups")
}
#[cfg(feature = "duckdb-runtime")]
pub fn key_usage_rollups_from_tiered(
    _config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    catalog_backend: &TieredUsageCatalogBackend,
) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
    let mut combined = BTreeMap::<String, KeyUsageRollupSummary>::new();
    {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        let conn = DuckDbUsageRepository::open_read_only_conn(&state.active_path)?;
        for rollup in key_usage_rollups_from_conn(&conn)? {
            merge_key_rollup(&mut combined, rollup);
        }
    }
    for rollup in catalog_backend.archived_key_usage_rollups()? {
        merge_key_rollup(&mut combined, rollup);
    }
    Ok(combined.into_values().collect())
}
#[cfg(feature = "duckdb-runtime")]
pub fn merge_key_rollup(
    combined: &mut BTreeMap<String, KeyUsageRollupSummary>,
    rollup: KeyUsageRollupSummary,
) {
    let entry = combined
        .entry(rollup.key_id.clone())
        .or_insert_with(|| KeyUsageRollupSummary {
            key_id: rollup.key_id.clone(),
            input_uncached_tokens: 0,
            input_cached_tokens: 0,
            output_tokens: 0,
            billable_tokens: 0,
            credit_total: "0".to_string(),
            credit_missing_events: 0,
            last_used_at_ms: None,
        });
    entry.input_uncached_tokens = entry
        .input_uncached_tokens
        .saturating_add(rollup.input_uncached_tokens);
    entry.input_cached_tokens = entry
        .input_cached_tokens
        .saturating_add(rollup.input_cached_tokens);
    entry.output_tokens = entry.output_tokens.saturating_add(rollup.output_tokens);
    entry.billable_tokens = entry.billable_tokens.saturating_add(rollup.billable_tokens);
    let current_credit = entry.credit_total.parse::<f64>().unwrap_or(0.0);
    let added_credit = rollup.credit_total.parse::<f64>().unwrap_or(0.0);
    entry.credit_total = (current_credit + added_credit).to_string();
    entry.credit_missing_events = entry
        .credit_missing_events
        .saturating_add(rollup.credit_missing_events);
    entry.last_used_at_ms = match (entry.last_used_at_ms, rollup.last_used_at_ms) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (None, Some(right)) => Some(right),
        (left, None) => left,
    };
}
#[cfg(feature = "duckdb-runtime")]
pub async fn append_usage_events_to_tiered(
    config: &TieredDuckDbUsageConfig,
    state: &Mutex<TieredDuckDbUsageState>,
    connection_config: &SharedDuckDbUsageConnectionConfig,
    catalog_backend: &Arc<TieredUsageCatalogBackend>,
    rows: &[UsageEventRow],
) -> anyhow::Result<()> {
    // Serialize against retention's active-segment rollover/discard (see
    // `TieredDuckDbUsageState::write_gate`): hold the gate for the whole append
    // so a concurrent retention cycle cannot delete/roll the active segment
    // while this append holds its writer across the insert `.await` — which
    // would orphan the writer onto a deleted segment and lose its rows.
    let write_gate = {
        let state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        Arc::clone(&state.write_gate)
    };
    let _write_guard = write_gate.lock_owned().await;
    #[cfg(test)]
    {
        let seam = {
            let mut state = state
                .lock()
                .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
            state.append_seam.take()
        };
        if let Some(seam) = seam {
            let _ = seam.reached.send(());
            let _ = seam.proceed.await;
        }
    }
    let connection_config_snapshot = connection_config_snapshot(connection_config);
    let mut writer = {
        let mut state = state
            .lock()
            .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
        if state.active_has_rows
            && active_segment_disk_bytes(&state.active_path) >= config.rollover_bytes.max(1)
        {
            rollover_active_segment(
                config,
                &mut state,
                connection_config_snapshot,
                Arc::clone(catalog_backend),
            )?;
        }
        let should_reopen = state
            .active_writer
            .as_ref()
            .map(|writer| writer.connection_config != connection_config_snapshot)
            .unwrap_or(true);
        if should_reopen {
            state.active_writer = Some(PersistentUsageWriter::open(
                &state.active_path,
                connection_config_snapshot,
                state.detail_store.clone(),
            )?);
        }
        state
            .active_writer
            .take()
            .ok_or_else(|| anyhow!("tiered active writer missing after initialization"))?
    };
    writer.writer.insert_usage_events(rows).await?;
    let mut state = state
        .lock()
        .map_err(|_| anyhow!("tiered duckdb state lock poisoned"))?;
    state.active_has_rows = true;
    state.active_writer = Some(writer);
    if active_segment_disk_bytes(&state.active_path) >= config.rollover_bytes.max(1) {
        rollover_active_segment(
            config,
            &mut state,
            connection_config_snapshot,
            Arc::clone(catalog_backend),
        )?;
    }
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub async fn publish_pending_segment_details_if_configured(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
) -> anyhow::Result<()> {
    let _ = (config, pending_path);
    Ok(())
}
#[cfg(feature = "duckdb-runtime")]
pub fn dedupe_usage_events_owned(events: Vec<UsageEvent>) -> Vec<UsageEvent> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(events.len());
    for event in events {
        if seen.insert(event.event_id.clone()) {
            deduped.push(event);
        }
    }
    deduped
}
#[cfg(feature = "duckdb-runtime")]
fn active_segment_disk_bytes(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
        + fs::metadata(duckdb_wal_path(path))
            .map(|meta| meta.len())
            .unwrap_or(0)
}
