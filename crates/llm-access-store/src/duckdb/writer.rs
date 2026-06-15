//! Usage-event write path: detail rows/blobs/store + hot/persistent
//! writers and the insert executors.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};

use anyhow::{anyhow, Context};

use super::{
    has_external_detail_payloads, insert_usage_event_sql,
    sql::insert_usage_event_detail_sql,
    util::{gunzip_json_bytes, gzip_json_bytes, sha256_hex, utc_date_parts},
    DuckDbUsageConnectionConfig, DuckDbUsageRepository, DuckDbUsageWriter, HotUsageWriter,
    PersistentUsageWriter, UsageEventDetailBlob, UsageEventDetailObjectRef,
    UsageEventDetailPackWrite, UsageEventDetailRow, UsageEventDetailStore, UsageEventRow,
};

#[cfg(feature = "duckdb-runtime")]
impl UsageEventDetailRow {
    fn from_usage_event_row(row: &UsageEventRow) -> Self {
        Self {
            event_id: row.event_id.clone(),
            request_headers_json: row.request_headers_json.clone(),
            routing_diagnostics_json: row.routing_diagnostics_json.clone(),
            last_message_content: row.last_message_content.clone(),
            client_request_body_json: row.client_request_body_json.clone(),
            upstream_request_body_json: row.upstream_request_body_json.clone(),
            full_request_json: row.full_request_json.clone(),
            error_message: row.error_message.clone(),
            error_body: row.error_body.clone(),
            response_body: row.response_body.clone(),
        }
    }

    fn has_external_payloads(&self) -> bool {
        has_external_detail_payloads(
            self.client_request_body_json.as_deref(),
            self.upstream_request_body_json.as_deref(),
            self.full_request_json.as_deref(),
            self.error_body.as_deref(),
            self.response_body.as_deref(),
        )
    }
}
#[cfg(feature = "duckdb-runtime")]
impl UsageEventDetailBlob {
    fn from_detail_row(row: &UsageEventDetailRow) -> Self {
        Self {
            request_headers_json: row.request_headers_json.clone(),
            routing_diagnostics_json: row.routing_diagnostics_json.clone(),
            last_message_content: row.last_message_content.clone(),
            client_request_body_json: row.client_request_body_json.clone(),
            upstream_request_body_json: row.upstream_request_body_json.clone(),
            full_request_json: row.full_request_json.clone(),
            error_message: row.error_message.clone(),
            error_body: row.error_body.clone(),
            response_body: row.response_body.clone(),
        }
    }

    fn into_detail_row(self, event_id: String) -> UsageEventDetailRow {
        UsageEventDetailRow {
            event_id,
            request_headers_json: self.request_headers_json,
            routing_diagnostics_json: self.routing_diagnostics_json,
            last_message_content: self.last_message_content,
            client_request_body_json: self.client_request_body_json,
            upstream_request_body_json: self.upstream_request_body_json,
            full_request_json: self.full_request_json,
            error_message: self.error_message,
            error_body: self.error_body,
            response_body: self.response_body,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
impl UsageEventDetailStore {
    pub(super) fn from_dir(path: &Path) -> anyhow::Result<Option<Self>> {
        if path.as_os_str().is_empty() {
            return Ok(None);
        }
        if !path.is_absolute() {
            return Err(anyhow!(
                "usage details dir `{}` must be an absolute local filesystem path",
                path.display()
            ));
        }
        fs::create_dir_all(path).with_context(|| {
            format!("failed to create usage details directory `{}`", path.display())
        })?;
        Ok(Some(Self {
            root_dir: path.to_path_buf(),
        }))
    }

    fn pack_relative_path_for_rows(&self, rows: &[UsageEventRow], pack_bytes: &[u8]) -> String {
        let first = rows
            .iter()
            .find(|row| row.detail_object_payload_present)
            .or_else(|| rows.first())
            .expect("detail pack rows should not be empty");
        let (year, month, day) = utc_date_parts(first.created_at_ms);
        let pack_hash = sha256_hex(pack_bytes);
        format!(
            "packs/{}/{year:04}/{month:02}/{day:02}/{}-{}.detailpack-v1",
            first.provider_type,
            first.event_id,
            &pack_hash[..16]
        )
    }

    pub(super) fn prepare_pack(
        &self,
        rows: &mut [UsageEventRow],
    ) -> anyhow::Result<Option<UsageEventDetailPackWrite>> {
        let mut pack_bytes = Vec::new();
        let mut packed = Vec::new();
        let mut seen = BTreeMap::<String, (i64, i64, String)>::new();
        for (index, row) in rows.iter_mut().enumerate() {
            let detail = UsageEventDetailRow::from_usage_event_row(row);
            let has_external_payloads = detail.has_external_payloads();
            row.detail_object_payload_present = has_external_payloads;
            if !has_external_payloads {
                row.detail_object_path = None;
                row.detail_object_offset = None;
                row.detail_object_length = None;
                row.detail_object_sha256 = None;
                continue;
            }
            let blob = UsageEventDetailBlob::from_detail_row(&detail);
            let encoded = gzip_json_bytes(&blob)
                .with_context(|| format!("failed to encode usage detail `{}`", row.event_id))?;
            let compressed_sha = sha256_hex(&encoded);
            let (offset, length, sha256) =
                if let Some((offset, length, sha256)) = seen.get(&compressed_sha).cloned() {
                    (offset, length, sha256)
                } else {
                    let offset = i64::try_from(pack_bytes.len())
                        .context("usage detail pack offset exceeds i64")?;
                    let length = i64::try_from(encoded.len())
                        .context("usage detail pack member length exceeds i64")?;
                    pack_bytes.extend_from_slice(&encoded);
                    seen.insert(compressed_sha.clone(), (offset, length, compressed_sha.clone()));
                    (offset, length, compressed_sha)
                };
            packed.push((index, offset, length, sha256));
        }
        if packed.is_empty() {
            return Ok(None);
        }
        let relative_path = self.pack_relative_path_for_rows(rows, &pack_bytes);
        for (index, offset, length, sha256) in packed {
            rows[index].detail_object_path = Some(relative_path.clone());
            rows[index].detail_object_offset = Some(offset);
            rows[index].detail_object_length = Some(length);
            rows[index].detail_object_sha256 = Some(sha256);
        }
        Ok(Some(UsageEventDetailPackWrite {
            relative_path,
            bytes: pack_bytes,
        }))
    }

    async fn put_pack(&self, pack: UsageEventDetailPackWrite) -> anyhow::Result<()> {
        let pack_path = self.root_dir.join(&pack.relative_path);
        if let Some(parent) = pack_path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create usage detail pack parent directory `{}`",
                    parent.display()
                )
            })?;
        }
        fs::write(&pack_path, pack.bytes).with_context(|| {
            format!("failed to write usage detail pack `{}`", pack_path.display())
        })?;
        Ok(())
    }

    pub(super) async fn get_row_for_ref(
        &self,
        event_id: &str,
        detail_ref: &UsageEventDetailObjectRef,
    ) -> anyhow::Result<Option<UsageEventDetailRow>> {
        let pack_path = self.root_dir.join(&detail_ref.relative_path);
        let mut file = match fs::File::open(&pack_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to open usage detail pack `{}`", pack_path.display())
                })
            },
        };
        let range_len = detail_ref
            .byte_range
            .end
            .checked_sub(detail_ref.byte_range.start)
            .ok_or_else(|| anyhow!("usage detail pack byte range is invalid"))?;
        let mut bytes =
            vec![0_u8; usize::try_from(range_len).context("detail byte range too large")?];
        file.seek(SeekFrom::Start(detail_ref.byte_range.start))
            .with_context(|| {
                format!("failed to seek usage detail pack `{}`", pack_path.display())
            })?;
        file.read_exact(&mut bytes).with_context(|| {
            format!("failed to read usage detail pack `{}`", pack_path.display())
        })?;
        let actual_sha = sha256_hex(&bytes);
        if actual_sha != detail_ref.sha256 {
            return Err(anyhow!(
                "usage detail pack member hash mismatch for event `{event_id}` in `{}`",
                pack_path.display()
            ));
        }
        let blob: UsageEventDetailBlob = gunzip_json_bytes(&bytes).with_context(|| {
            format!("failed to decode usage detail pack member `{}`", pack_path.display())
        })?;
        Ok(Some(blob.into_detail_row(event_id.to_string())))
    }
}
#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageWriter {
    /// Create a writer from an opened DuckDB connection.
    pub fn new(conn: duckdb::Connection) -> anyhow::Result<Self> {
        crate::initialize_duckdb_target(&conn)?;
        Ok(Self {
            conn,
        })
    }

    /// Insert one usage event row.
    pub fn insert_usage_event(&mut self, row: &UsageEventRow) -> anyhow::Result<()> {
        self.insert_usage_events(std::slice::from_ref(row))
            .map(|_| ())
    }

    fn insert_usage_event_summaries(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let inserted_rows = {
            let mut inserted_rows = Vec::new();
            let mut summary_stmt = tx.prepare(insert_usage_event_sql())?;
            for row in rows {
                if execute_usage_event_insert(&mut summary_stmt, row)? > 0 {
                    inserted_rows.push(row);
                }
            }
            inserted_rows
        };
        let inserted_count = inserted_rows.len();
        {
            upsert_proxy_traffic_rollups(&tx, &inserted_rows)?;
        }
        tx.commit()?;
        Ok(inserted_count)
    }

    /// Insert a batch of usage event rows in one transaction.
    pub fn insert_usage_events(&mut self, rows: &[UsageEventRow]) -> anyhow::Result<usize> {
        if rows.is_empty() {
            return Ok(0);
        }
        let tx = self.conn.transaction()?;
        let inserted_rows = {
            let mut inserted_rows = Vec::new();
            let mut summary_stmt = tx.prepare(insert_usage_event_sql())?;
            let mut detail_stmt = tx.prepare(insert_usage_event_detail_sql())?;
            for row in rows {
                if execute_usage_event_insert(&mut summary_stmt, row)? > 0 {
                    inserted_rows.push(row);
                }
                execute_usage_event_detail_insert(&mut detail_stmt, row)?;
            }
            inserted_rows
        };
        let inserted_count = inserted_rows.len();
        {
            upsert_proxy_traffic_rollups(&tx, &inserted_rows)?;
        }
        tx.commit()?;
        Ok(inserted_count)
    }

    /// Insert only the summary projection for a batch of usage events.
    pub fn insert_usage_event_summaries_only(
        &mut self,
        rows: &[UsageEventRow],
    ) -> anyhow::Result<usize> {
        self.insert_usage_event_summaries(rows)
    }
}
#[cfg(feature = "duckdb-runtime")]
impl HotUsageWriter {
    pub(super) fn open(
        duckdb_path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
        detail_store: Option<Arc<UsageEventDetailStore>>,
    ) -> anyhow::Result<Self> {
        let summary =
            DuckDbUsageWriter::new(DuckDbUsageRepository::open_conn_with_connection_config(
                duckdb_path,
                connection_config,
            )?)?;
        Ok(Self {
            summary,
            detail_store,
        })
    }

    pub(super) async fn insert_usage_events(
        &mut self,
        rows: &[UsageEventRow],
    ) -> anyhow::Result<usize> {
        if let Some(detail_store) = &self.detail_store {
            let mut rows = rows.to_vec();
            let pack = detail_store.prepare_pack(&mut rows)?;
            let inserted_count = self.summary.insert_usage_event_summaries(&rows)?;
            if let Some(pack) = pack {
                detail_store.put_pack(pack).await?;
            }
            return Ok(inserted_count);
        }
        self.summary.insert_usage_event_summaries(rows)
    }
}
#[cfg(feature = "duckdb-runtime")]
impl PersistentUsageWriter {
    pub(super) fn open(
        path: &Path,
        connection_config: DuckDbUsageConnectionConfig,
        detail_store: Option<Arc<UsageEventDetailStore>>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            writer: HotUsageWriter::open(path, connection_config, detail_store)?,
            connection_config,
        })
    }
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProxyTrafficRollupKey {
    bucket_hour_ms: i64,
    provider_type: String,
    proxy_key: String,
    proxy_source: Option<String>,
    proxy_config_id: Option<String>,
    proxy_config_name: Option<String>,
    proxy_url: Option<String>,
}

#[cfg(feature = "duckdb-runtime")]
#[derive(Debug, Clone, Copy, Default)]
struct ProxyTrafficRollupDelta {
    request_count: i64,
    request_bytes: i64,
    response_bytes: i64,
    total_bytes: i64,
}

#[cfg(feature = "duckdb-runtime")]
fn upsert_proxy_traffic_rollups(
    tx: &duckdb::Transaction<'_>,
    rows: &[&UsageEventRow],
) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut rollups = HashMap::<ProxyTrafficRollupKey, ProxyTrafficRollupDelta>::new();
    for row in rows {
        let request_bytes = non_negative_i64(row.request_body_bytes);
        let response_bytes = non_negative_i64(row.bytes_streamed);
        let key = ProxyTrafficRollupKey {
            bucket_hour_ms: bucket_hour_ms(row.created_at_ms),
            provider_type: row.provider_type.clone(),
            proxy_key: proxy_traffic_key(
                row.proxy_config_id_at_event.as_deref(),
                row.proxy_url_at_event.as_deref(),
                row.proxy_source_at_event.as_deref(),
            ),
            proxy_source: normalize_optional_string(row.proxy_source_at_event.as_deref()),
            proxy_config_id: normalize_optional_string(row.proxy_config_id_at_event.as_deref()),
            proxy_config_name: normalize_optional_string(row.proxy_config_name_at_event.as_deref()),
            proxy_url: normalize_optional_string(row.proxy_url_at_event.as_deref()),
        };
        let delta = rollups.entry(key).or_default();
        delta.request_count = delta.request_count.saturating_add(1);
        delta.request_bytes = delta.request_bytes.saturating_add(request_bytes);
        delta.response_bytes = delta.response_bytes.saturating_add(response_bytes);
        delta.total_bytes = delta
            .total_bytes
            .saturating_add(request_bytes.saturating_add(response_bytes));
    }
    let mut stmt = tx.prepare(
        "INSERT INTO proxy_traffic_rollups_hourly (
            bucket_hour,
            provider_type,
            proxy_key,
            proxy_source,
            proxy_config_id,
            proxy_config_name,
            proxy_url,
            request_count,
            request_bytes,
            response_bytes,
            total_bytes
         ) VALUES (
            date_trunc('hour', to_timestamp(?1 / 1000.0)),
            ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
         )
         ON CONFLICT (bucket_hour, provider_type, proxy_key) DO UPDATE SET
            request_count = proxy_traffic_rollups_hourly.request_count + excluded.request_count,
            request_bytes = proxy_traffic_rollups_hourly.request_bytes + excluded.request_bytes,
            response_bytes = proxy_traffic_rollups_hourly.response_bytes + excluded.response_bytes,
            total_bytes = proxy_traffic_rollups_hourly.total_bytes + excluded.total_bytes,
            proxy_source = COALESCE(excluded.proxy_source, \
         proxy_traffic_rollups_hourly.proxy_source),
            proxy_config_id = COALESCE(excluded.proxy_config_id, \
         proxy_traffic_rollups_hourly.proxy_config_id),
            proxy_config_name = COALESCE(excluded.proxy_config_name, \
         proxy_traffic_rollups_hourly.proxy_config_name),
            proxy_url = COALESCE(excluded.proxy_url, proxy_traffic_rollups_hourly.proxy_url)",
    )?;
    for (key, delta) in rollups {
        stmt.execute(duckdb::params![
            key.bucket_hour_ms,
            &key.provider_type,
            &key.proxy_key,
            key.proxy_source.as_deref(),
            key.proxy_config_id.as_deref(),
            key.proxy_config_name.as_deref(),
            key.proxy_url.as_deref(),
            delta.request_count,
            delta.request_bytes,
            delta.response_bytes,
            delta.total_bytes,
        ])?;
    }
    Ok(())
}

#[cfg(feature = "duckdb-runtime")]
fn non_negative_i64(value: Option<i64>) -> i64 {
    value.unwrap_or(0).max(0)
}

#[cfg(feature = "duckdb-runtime")]
fn bucket_hour_ms(created_at_ms: i64) -> i64 {
    created_at_ms
        .div_euclid(3_600_000)
        .saturating_mul(3_600_000)
}

#[cfg(feature = "duckdb-runtime")]
fn normalize_optional_string(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

#[cfg(feature = "duckdb-runtime")]
fn proxy_traffic_key(
    proxy_config_id: Option<&str>,
    proxy_url: Option<&str>,
    proxy_source: Option<&str>,
) -> String {
    if let Some(value) = normalize_optional_string(proxy_config_id) {
        return format!("proxy:id:{value}");
    }
    if let Some(value) = normalize_optional_string(proxy_url) {
        return format!("proxy:url:{value}");
    }
    if let Some(value) = normalize_optional_string(proxy_source) {
        return format!("proxy:source:{value}");
    }
    "proxy:unknown".to_string()
}

#[cfg(feature = "duckdb-runtime")]
fn execute_usage_event_insert(
    stmt: &mut duckdb::Statement<'_>,
    row: &UsageEventRow,
) -> anyhow::Result<usize> {
    let inserted = stmt.execute(duckdb::params![
        row.source_seq,
        &row.source_event_id,
        &row.event_id,
        row.created_at_ms,
        &row.provider_type,
        &row.protocol_family,
        &row.key_id,
        &row.key_name,
        &row.key_status_at_event,
        row.account_name.as_deref(),
        row.account_group_id_at_event.as_deref(),
        row.route_strategy_at_event.as_deref(),
        &row.request_method,
        &row.request_url,
        &row.endpoint,
        row.model.as_deref(),
        row.mapped_model.as_deref(),
        row.status_code,
        row.latency_ms,
        row.routing_wait_ms,
        row.upstream_headers_ms,
        row.post_headers_body_ms,
        row.request_body_read_ms,
        row.request_json_parse_ms,
        row.pre_handler_ms,
        row.first_sse_write_ms,
        row.stream_finish_ms,
        row.stream_completed_cleanly,
        row.downstream_disconnect,
        row.final_event_type.as_deref(),
        row.bytes_streamed,
        row.request_body_bytes,
        row.quota_failover_count,
        row.input_uncached_tokens,
        row.input_cached_tokens,
        row.output_tokens,
        row.billable_tokens,
        row.credit_usage.as_deref(),
        row.usage_missing,
        row.credit_usage_missing,
        row.client_ip.as_deref(),
        row.ip_region.as_deref(),
        &row.request_headers_json,
        row.routing_diagnostics_json.as_deref(),
        row.last_message_content.as_deref(),
        row.detail_object_payload_present,
        row.detail_object_path.as_deref(),
        row.detail_object_offset,
        row.detail_object_length,
        row.detail_object_sha256.as_deref(),
        row.proxy_source_at_event.as_deref(),
        row.proxy_config_id_at_event.as_deref(),
        row.proxy_config_name_at_event.as_deref(),
        row.proxy_url_at_event.as_deref(),
    ])?;
    Ok(inserted)
}
#[cfg(feature = "duckdb-runtime")]
fn execute_usage_event_detail_insert(
    stmt: &mut duckdb::Statement<'_>,
    row: &UsageEventRow,
) -> anyhow::Result<()> {
    stmt.execute(duckdb::params![
        &row.event_id,
        &row.request_headers_json,
        row.routing_diagnostics_json.as_deref(),
        row.last_message_content.as_deref(),
        row.client_request_body_json.as_deref(),
        row.upstream_request_body_json.as_deref(),
        row.full_request_json.as_deref(),
        row.error_message.as_deref(),
        row.error_body.as_deref(),
        row.response_body.as_deref(),
    ])?;
    Ok(())
}
