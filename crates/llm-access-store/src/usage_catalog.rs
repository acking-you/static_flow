//! Postgres-backed archived usage catalog metadata for tiered analytics.

use std::{collections::HashSet, path::PathBuf};

use anyhow::{anyhow, Context};
use llm_access_core::store::UsageEventTotals;
use native_tls::TlsConnector;
use postgres::{types::ToSql, Client};
use postgres_native_tls::MakeTlsConnector;
use serde::{Deserialize, Serialize};

use crate::{
    request_cache::{RequestCache, RequestCacheConfig},
    KeyUsageRollupSummary,
};

/// Archived segment metadata loaded from the catalog store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UsageCatalogSegment {
    /// Archived DuckDB file path.
    pub archive_path: PathBuf,
    /// Earliest event timestamp in this segment.
    pub start_ms: Option<i64>,
    /// Latest event timestamp in this segment.
    pub end_ms: Option<i64>,
    /// Archived event count for this segment.
    pub row_count: usize,
}

/// One catalog-level field filter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageCatalogFieldFilter {
    /// Indexed field name.
    pub field_name: UsageCatalogFieldName,
    /// Exact field value matched by the query.
    pub field_value: String,
}

/// One catalog query over archived segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageCatalogQuery {
    /// Inclusive lower bound on event time.
    pub start_ms: Option<i64>,
    /// Exclusive upper bound on event time.
    pub end_ms: Option<i64>,
    /// Optional key scope.
    pub key_id: Option<String>,
    /// Optional provider scope.
    pub provider_type: Option<String>,
    /// Optional indexed field filters.
    pub field_filters: Vec<UsageCatalogFieldFilter>,
}

/// One indexed field supported by the archived usage catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) enum UsageCatalogFieldName {
    Model,
    AccountName,
    Endpoint,
    StatusCode,
    StatusKind,
}

impl UsageCatalogFieldName {
    pub(crate) fn as_storage_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::AccountName => "account_name",
            Self::Endpoint => "endpoint",
            Self::StatusCode => "status_code",
            Self::StatusKind => "status_kind",
        }
    }
}

/// One archived segment plus its pre-aggregated matching totals.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UsageCatalogSegmentMatch {
    /// Archived segment metadata.
    pub segment: UsageCatalogSegment,
    /// Matching catalog totals for the whole segment when the query shape is
    /// exactly supported by catalog rollups.
    pub matching_totals: Option<UsageCatalogSegmentTotals>,
}

/// Serializable segment totals carried through the catalog cache layer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UsageCatalogSegmentTotals {
    /// Matching event count.
    pub event_count: usize,
    /// Total uncached input tokens across matching rows.
    pub input_uncached_tokens: u64,
    /// Total cached input tokens across matching rows.
    pub input_cached_tokens: u64,
    /// Total output tokens across matching rows.
    pub output_tokens: u64,
    /// Total billable tokens across matching rows.
    pub billable_tokens: u64,
}

impl From<UsageCatalogSegmentTotals> for UsageEventTotals {
    fn from(value: UsageCatalogSegmentTotals) -> Self {
        Self {
            event_count: value.event_count,
            input_uncached_tokens: value.input_uncached_tokens,
            input_cached_tokens: value.input_cached_tokens,
            output_tokens: value.output_tokens,
            billable_tokens: value.billable_tokens,
        }
    }
}

/// One expired archived segment selected for retention pruning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageCatalogRetentionSegment {
    /// Stable segment identifier.
    pub segment_id: String,
    /// Archived DuckDB path.
    pub archive_path: PathBuf,
}

/// Immutable segment row written into the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageCatalogSegmentRecord {
    /// Stable segment identifier.
    pub segment_id: String,
    /// Archived DuckDB path.
    pub archive_path: PathBuf,
    /// Earliest event timestamp in this segment.
    pub start_ms: Option<i64>,
    /// Latest event timestamp in this segment.
    pub end_ms: Option<i64>,
    /// Archived event count.
    pub row_count: usize,
    /// Total uncached input tokens across the segment.
    pub input_uncached_tokens: i64,
    /// Total cached input tokens across the segment.
    pub input_cached_tokens: i64,
    /// Total output tokens across the segment.
    pub output_tokens: i64,
    /// Total billable tokens across the segment.
    pub billable_tokens: i64,
    /// Archived DuckDB size in bytes.
    pub size_bytes: u64,
    /// Segment seal timestamp.
    pub sealed_at_ms: i64,
}

/// Per-key rollup row written into the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageCatalogKeyRollupRecord {
    /// API key id.
    pub key_id: String,
    /// Provider family.
    pub provider_type: String,
    /// Matching event count in this segment.
    pub row_count: usize,
    /// Total uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Total cached input tokens.
    pub input_cached_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total billable tokens.
    pub billable_tokens: i64,
    /// Total credit usage as a decimal string.
    pub credit_total: String,
    /// Events missing provider credit usage.
    pub credit_missing_events: i64,
    /// Earliest usage time for this key in the segment.
    pub first_used_at_ms: Option<i64>,
    /// Latest usage time for this key in the segment.
    pub last_used_at_ms: Option<i64>,
}

/// One per-segment rollup for one indexed field value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageCatalogFieldRollupRecord {
    /// Optional key scope.
    pub key_id: Option<String>,
    /// Optional provider scope.
    pub provider_type: Option<String>,
    /// Indexed field name.
    pub field_name: UsageCatalogFieldName,
    /// Exact field value.
    pub field_value: String,
    /// Matching event count in this segment.
    pub row_count: usize,
    /// Total uncached input tokens across matching rows.
    pub input_uncached_tokens: i64,
    /// Total cached input tokens across matching rows.
    pub input_cached_tokens: i64,
    /// Total output tokens across matching rows.
    pub output_tokens: i64,
    /// Total billable tokens across matching rows.
    pub billable_tokens: i64,
    /// Earliest usage time for this field value in the segment.
    pub first_used_at_ms: Option<i64>,
    /// Latest usage time for this field value in the segment.
    pub last_used_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogRollupsLookup {
    generation: i64,
    rollups: Vec<KeyUsageRollupSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogSegmentMatchesLookup {
    generation: i64,
    segments: Vec<UsageCatalogSegmentMatch>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogEventLocatorLookup {
    generation: i64,
    segment: Option<UsageCatalogSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogFilterOptionsLookup {
    generation: i64,
    options: Vec<String>,
}

/// Postgres-backed catalog store for archived usage segments.
#[derive(Debug, Clone)]
pub(crate) struct PostgresUsageCatalog {
    database_url: String,
    request_cache: Option<RequestCache>,
}

impl PostgresUsageCatalog {
    /// Build a Postgres-backed usage catalog handle.
    pub(crate) fn new(
        database_url: &str,
        request_cache_config: Option<RequestCacheConfig>,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            database_url: database_url.to_string(),
            request_cache: request_cache_config.map(RequestCache::new).transpose()?,
        })
    }

    /// Return true when no archived catalog rows exist yet.
    pub(crate) fn is_empty(&self) -> anyhow::Result<bool> {
        self.with_client("catalog emptiness check", |client| {
            let row = client
                .query_one("SELECT COUNT(*) FROM llm_usage_segments", &[])
                .context("query archived usage segment count")?;
            let count: i64 = row.get(0);
            Ok(count == 0)
        })
    }

    /// Return the newest sealed segment sequence from the catalog.
    pub(crate) fn next_sequence(&self) -> anyhow::Result<u64> {
        self.with_client("catalog sequence lookup", |client| {
            let row = client
                .query_opt(
                    "SELECT segment_id
                     FROM llm_usage_segments
                     ORDER BY sealed_at_ms DESC, segment_id DESC
                     LIMIT 1",
                    &[],
                )
                .context("query latest archived segment id")?;
            Ok(row
                .and_then(|row| parse_sequence_from_segment_id(&row.get::<_, String>(0)))
                .unwrap_or(0))
        })
    }

    /// Return the archived DuckDB path for one segment id.
    pub(crate) fn archive_path_for_segment(
        &self,
        segment_id: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        self.with_client("segment archive lookup", |client| {
            let row = client
                .query_opt(
                    "SELECT archive_path
                     FROM llm_usage_segments
                     WHERE segment_id = $1
                     LIMIT 1",
                    &[&segment_id],
                )
                .context("query archived segment path")?;
            Ok(row.map(|row| PathBuf::from(row.get::<_, String>(0))))
        })
    }

    /// Upsert one archived segment, its rollups, and event locators.
    pub(crate) fn publish_segment(
        &self,
        segment: &UsageCatalogSegmentRecord,
        rollups: &[UsageCatalogKeyRollupRecord],
        field_rollups: &[UsageCatalogFieldRollupRecord],
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        self.with_client("segment publication", |client| {
            let mut tx = client
                .transaction()
                .context("begin usage catalog transaction")?;
            tx.execute(
                "INSERT INTO llm_usage_segments (
                    segment_id, archive_path, state, start_ms, end_ms, row_count,
                    input_uncached_tokens, input_cached_tokens, output_tokens,
                    billable_tokens, size_bytes, sealed_at_ms
                 ) VALUES ($1, $2, 'archived', $3, $4, $5, $6, $7, $8, $9, $10, $11)
                 ON CONFLICT (segment_id) DO UPDATE
                 SET archive_path = EXCLUDED.archive_path,
                     state = EXCLUDED.state,
                     start_ms = EXCLUDED.start_ms,
                     end_ms = EXCLUDED.end_ms,
                     row_count = EXCLUDED.row_count,
                     input_uncached_tokens = EXCLUDED.input_uncached_tokens,
                     input_cached_tokens = EXCLUDED.input_cached_tokens,
                     output_tokens = EXCLUDED.output_tokens,
                     billable_tokens = EXCLUDED.billable_tokens,
                     size_bytes = EXCLUDED.size_bytes,
                     sealed_at_ms = EXCLUDED.sealed_at_ms",
                &[
                    &segment.segment_id,
                    &segment.archive_path.to_string_lossy().to_string(),
                    &segment.start_ms,
                    &segment.end_ms,
                    &usize_to_i64(segment.row_count),
                    &segment.input_uncached_tokens,
                    &segment.input_cached_tokens,
                    &segment.output_tokens,
                    &segment.billable_tokens,
                    &u64_to_i64(segment.size_bytes),
                    &segment.sealed_at_ms,
                ],
            )
            .context("upsert archived usage segment")?;
            tx.execute("DELETE FROM llm_usage_segment_events WHERE segment_id = $1", &[
                &segment.segment_id
            ])
            .context("delete archived segment event locators")?;
            tx.execute("DELETE FROM llm_usage_segment_key_rollups WHERE segment_id = $1", &[
                &segment.segment_id,
            ])
            .context("delete archived segment key rollups")?;
            tx.execute("DELETE FROM llm_usage_segment_field_rollups WHERE segment_id = $1", &[
                &segment.segment_id,
            ])
            .context("delete archived segment field rollups")?;
            insert_rollups(&mut tx, &segment.segment_id, rollups)?;
            insert_field_rollups(&mut tx, &segment.segment_id, field_rollups)?;
            insert_event_locators(&mut tx, &segment.segment_id, event_ids)?;
            tx.commit().context("commit usage catalog transaction")?;
            Ok(())
        })?;
        self.bump_generation();
        Ok(())
    }

    /// Aggregate archived key rollups from Postgres with Redis read-through.
    pub(crate) fn archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let generation = self.current_generation();
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_rollups_key();
            match cache.get_json_blocking::<CachedUsageCatalogRollupsLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.rollups),
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache usage-catalog rollup read failed; falling back to postgres"
                ),
            }
            let rollups = self.load_archived_key_usage_rollups()?;
            let payload = CachedUsageCatalogRollupsLookup {
                generation,
                rollups: rollups.clone(),
            };
            if let Err(err) =
                cache.set_json_blocking(&cache_key, &payload, cache.usage_catalog_rollups_ttl())
            {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache usage-catalog rollup write failed"
                );
            }
            return Ok(rollups);
        }
        self.load_archived_key_usage_rollups()
    }

    /// Remove expired archived segments from the catalog.
    pub(crate) fn delete_expired_segments(
        &self,
        cutoff_ms: i64,
    ) -> anyhow::Result<Vec<UsageCatalogRetentionSegment>> {
        let deleted = self.with_client("usage retention prune", |client| {
            let rows = client
                .query(
                    "SELECT segment_id, archive_path
                     FROM llm_usage_segments
                     WHERE state = 'archived'
                       AND end_ms IS NOT NULL
                       AND end_ms < $1
                     ORDER BY end_ms ASC, segment_id ASC",
                    &[&cutoff_ms],
                )
                .context("query expired archived usage segments")?;
            let candidates = rows
                .into_iter()
                .map(|row| UsageCatalogRetentionSegment {
                    segment_id: row.get(0),
                    archive_path: PathBuf::from(row.get::<_, String>(1)),
                })
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Ok(candidates);
            }
            let segment_ids = candidates
                .iter()
                .map(|candidate| candidate.segment_id.as_str())
                .collect::<Vec<_>>();
            client
                .execute("DELETE FROM llm_usage_segments WHERE segment_id = ANY($1)", &[
                    &segment_ids,
                ])
                .context("delete expired archived usage segments")?;
            Ok(candidates)
        })?;
        if !deleted.is_empty() {
            self.bump_generation();
        }
        Ok(deleted)
    }

    /// Return all archived DuckDB paths tracked by the catalog.
    pub(crate) fn archived_paths(&self) -> anyhow::Result<HashSet<PathBuf>> {
        self.with_client("orphan prune path lookup", |client| {
            let rows = client
                .query(
                    "SELECT archive_path
                     FROM llm_usage_segments
                     WHERE state = 'archived'",
                    &[],
                )
                .context("query archived duckdb paths")?;
            Ok(rows
                .into_iter()
                .map(|row| PathBuf::from(row.get::<_, String>(0)))
                .collect())
        })
    }

    /// Return archived segment paths still missing field-rollup rows.
    pub(crate) fn archived_paths_missing_field_rollups(&self) -> anyhow::Result<Vec<PathBuf>> {
        self.with_client("field rollup backfill lookup", |client| {
            let rows = client
                .query(
                    "SELECT s.archive_path
                     FROM llm_usage_segments s
                     LEFT JOIN llm_usage_segment_field_rollups f
                       ON f.segment_id = s.segment_id
                     WHERE s.state = 'archived'
                     GROUP BY s.segment_id, s.archive_path
                     HAVING COUNT(f.segment_id) = 0
                     ORDER BY s.segment_id ASC",
                    &[],
                )
                .context("query archived segments missing field rollups")?;
            Ok(rows
                .into_iter()
                .map(|row| PathBuf::from(row.get::<_, String>(0)))
                .collect())
        })
    }

    /// Return archived segments plus exact whole-segment catalog totals when
    /// the query shape is supported by segment-level rollups.
    pub(crate) fn archived_segment_matches_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let generation = self.current_generation();
        let query_fingerprint = usage_catalog_query_fingerprint(query);
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_filtered_segments_key(&query_fingerprint);
            match cache.get_json_blocking::<CachedUsageCatalogSegmentMatchesLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.segments),
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache filtered archived segment read failed; falling back to postgres"
                ),
            }
            let segments = self.load_archived_segment_matches_for_query(query)?;
            let payload = CachedUsageCatalogSegmentMatchesLookup {
                generation,
                segments: segments.clone(),
            };
            if let Err(err) = cache.set_json_blocking(
                &cache_key,
                &payload,
                cache.usage_catalog_filtered_segments_ttl(&query_fingerprint),
            ) {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache filtered archived segment write failed"
                );
            }
            return Ok(segments);
        }
        self.load_archived_segment_matches_for_query(query)
    }

    /// Return exact archive filter-option values from the catalog when the
    /// query has no remaining cross-dimension filters after self-clearing.
    pub(crate) fn archived_filter_option_values(
        &self,
        query: &UsageCatalogQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if !query.field_filters.is_empty() {
            return Ok(None);
        }
        let generation = self.current_generation();
        let query_fingerprint = format!(
            "{}:field:{}",
            usage_catalog_query_fingerprint(query),
            field_name.as_storage_str()
        );
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_filter_options_key(&query_fingerprint);
            match cache.get_json_blocking::<CachedUsageCatalogFilterOptionsLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => {
                    return Ok(Some(lookup.options))
                },
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache archived filter-options read failed; falling back to postgres"
                ),
            }
            let options = self.load_archived_filter_option_values(query, field_name)?;
            let payload = CachedUsageCatalogFilterOptionsLookup {
                generation,
                options: options.clone(),
            };
            if let Err(err) = cache.set_json_blocking(
                &cache_key,
                &payload,
                cache.usage_catalog_filter_options_ttl(&query_fingerprint),
            ) {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache archived filter-options write failed"
                );
            }
            return Ok(Some(options));
        }
        self.load_archived_filter_option_values(query, field_name)
            .map(Some)
    }

    /// Return the archived segment that contains one event id.
    pub(crate) fn locate_archived_segment(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<UsageCatalogSegment>> {
        let generation = self.current_generation();
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_event_locator_key(event_id);
            match cache.get_json_blocking::<CachedUsageCatalogEventLocatorLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.segment),
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache usage event locator read failed; falling back to postgres"
                ),
            }
            let segment = self.load_archived_segment_locator(event_id)?;
            let payload = CachedUsageCatalogEventLocatorLookup {
                generation,
                segment: segment.clone(),
            };
            if let Err(err) = cache.set_json_blocking(
                &cache_key,
                &payload,
                cache.usage_catalog_event_locator_ttl(event_id),
            ) {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache usage event locator write failed"
                );
            }
            return Ok(segment);
        }
        self.load_archived_segment_locator(event_id)
    }

    fn load_archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        self.with_client("archived key usage rollups", |client| {
            let rows = client
                .query(&archived_key_usage_rollups_sql(), &[])
                .context("query archived key usage rollups")?;
            Ok(rows
                .into_iter()
                .map(|row| KeyUsageRollupSummary {
                    key_id: row.get(0),
                    input_uncached_tokens: row.get(1),
                    input_cached_tokens: row.get(2),
                    output_tokens: row.get(3),
                    billable_tokens: row.get(4),
                    credit_total: row.get(5),
                    credit_missing_events: row.get(6),
                    last_used_at_ms: row.get(7),
                })
                .collect())
        })
    }

    fn load_archived_segment_matches_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let sql = match query.field_filters.len() {
            0 => filtered_archived_segments_sql_for_scope_only(),
            1 => filtered_archived_segments_sql_for_single_field(),
            _ => filtered_archived_segments_sql_for_multi_field(query.field_filters.len()),
        };
        let query = query.clone();
        let key_id = query.key_id.as_deref();
        let provider_type = query.provider_type.as_deref();
        self.with_client("filtered segment lookup", move |client| {
            let field_names = query
                .field_filters
                .iter()
                .map(|filter| filter.field_name.as_storage_str())
                .collect::<Vec<_>>();
            let field_values = query
                .field_filters
                .iter()
                .map(|filter| filter.field_value.as_str())
                .collect::<Vec<_>>();
            let mut params: Vec<&(dyn ToSql + Sync)> =
                vec![&query.start_ms, &query.end_ms, &key_id, &provider_type];
            for index in 0..field_names.len() {
                params.push(&field_names[index]);
                params.push(&field_values[index]);
            }
            let rows = client
                .query(&sql, &params)
                .context("query filtered archived segments")?;
            rows.into_iter().map(decode_segment_match_row).collect()
        })
    }

    fn load_archived_filter_option_values(
        &self,
        query: &UsageCatalogQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Vec<String>> {
        let query = query.clone();
        self.with_client("archived filter-options lookup", move |client| {
            let rows = client
                .query(&archived_filter_option_values_sql(), &[
                    &query.start_ms,
                    &query.end_ms,
                    &query.key_id.as_deref(),
                    &query.provider_type.as_deref(),
                    &field_name.as_storage_str(),
                ])
                .context("query archived filter-options values")?;
            Ok(rows
                .into_iter()
                .map(|row| row.get::<_, String>(0))
                .filter(|value| !value.is_empty())
                .collect())
        })
    }

    fn load_archived_segment_locator(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<UsageCatalogSegment>> {
        self.with_client("event locator", |client| {
            let row = client
                .query_opt(
                    "SELECT s.archive_path, s.start_ms, s.end_ms, s.row_count
                     FROM llm_usage_segment_events e
                     JOIN llm_usage_segments s ON s.segment_id = e.segment_id
                     WHERE e.event_id = $1 AND s.state = 'archived'",
                    &[&event_id],
                )
                .context("query archived event locator")?;
            row.map(decode_segment_row).transpose()
        })
    }

    fn current_generation(&self) -> i64 {
        let Some(cache) = self.request_cache.as_ref() else {
            return 0;
        };
        let key = cache.usage_catalog_generation_key();
        match cache.get_i64_blocking(&key) {
            Ok(Some(value)) => value,
            Ok(None) => 0,
            Err(err) => {
                tracing::warn!(
                    key = %key,
                    error = %err,
                    "request cache usage-catalog generation read failed"
                );
                0
            },
        }
    }

    fn bump_generation(&self) {
        let Some(cache) = self.request_cache.as_ref() else {
            return;
        };
        let key = cache.usage_catalog_generation_key();
        if let Err(err) = cache.incr_blocking(&key) {
            tracing::warn!(
                key = %key,
                error = %err,
                "request cache usage-catalog generation bump failed"
            );
        }
    }

    fn with_client<T>(
        &self,
        purpose: &str,
        action: impl FnOnce(&mut Client) -> anyhow::Result<T> + Send,
    ) -> anyhow::Result<T>
    where
        T: Send,
    {
        let database_url = self.database_url.clone();
        let purpose = purpose.to_string();
        let panic_purpose = purpose.clone();
        std::thread::scope(|scope| {
            let handle = scope.spawn(move || {
                let native_tls = TlsConnector::builder()
                    .build()
                    .context("build native tls connector for usage catalog")?;
                let tls = MakeTlsConnector::new(native_tls);
                let mut client = Client::connect(&database_url, tls)
                    .with_context(|| format!("connect postgres usage catalog for {purpose}"))?;
                action(&mut client)
            });
            match handle.join() {
                Ok(result) => result,
                Err(_) => Err(anyhow!(
                    "postgres usage catalog worker thread panicked for {panic_purpose}"
                )),
            }
        })
    }
}

fn sum_bigint_sql(expr: &str) -> String {
    format!("COALESCE(SUM({expr}), 0)::BIGINT")
}

fn usage_catalog_query_fingerprint(query: &UsageCatalogQuery) -> String {
    let mut filters = query.field_filters.clone();
    filters.sort_by(|left, right| {
        left.field_name
            .as_storage_str()
            .cmp(right.field_name.as_storage_str())
            .then_with(|| left.field_value.cmp(&right.field_value))
    });
    let filters_key = if filters.is_empty() {
        "-".to_string()
    } else {
        filters
            .into_iter()
            .map(|filter| format!("{}={}", filter.field_name.as_storage_str(), filter.field_value))
            .collect::<Vec<_>>()
            .join("|")
    };
    format!(
        "start:{}:end:{}:key:{}:provider:{}:filters:{}",
        option_i64_key(query.start_ms),
        option_i64_key(query.end_ms),
        option_str_key(query.key_id.as_deref()),
        option_str_key(query.provider_type.as_deref()),
        filters_key,
    )
}

fn segment_time_overlap_sql(alias: &str) -> String {
    format!(
        "($1::BIGINT IS NULL OR {alias}.end_ms IS NULL OR {alias}.end_ms >= $1)
         AND ($2::BIGINT IS NULL OR {alias}.start_ms IS NULL OR {alias}.start_ms < $2)"
    )
}

fn scoped_time_overlap_sql(alias: &str) -> String {
    format!(
        "($1::BIGINT IS NULL OR {alias}.last_used_at_ms IS NULL OR {alias}.last_used_at_ms >= $1)
         AND ($2::BIGINT IS NULL OR {alias}.first_used_at_ms IS NULL OR {alias}.first_used_at_ms < \
         $2)"
    )
}

fn field_rollup_scope_sql(alias: &str) -> String {
    format!(
        "((($3::TEXT IS NULL AND $4::TEXT IS NULL)
            AND {alias}.key_id = ''
            AND {alias}.provider_type = '')
          OR (($3::TEXT IS NOT NULL OR $4::TEXT IS NOT NULL)
              AND ({alias}.key_id <> '' OR {alias}.provider_type <> '')
              AND ($3::TEXT IS NULL OR {alias}.key_id = $3)
              AND ($4::TEXT IS NULL OR {alias}.provider_type = $4)))"
    )
}

fn archived_key_usage_rollups_sql() -> String {
    format!(
        "SELECT
            key_id,
            {},
            {},
            {},
            {},
            COALESCE(SUM((credit_total)::numeric), 0)::text,
            {},
            MAX(last_used_at_ms)
         FROM llm_usage_segment_key_rollups
         GROUP BY key_id",
        sum_bigint_sql("input_uncached_tokens"),
        sum_bigint_sql("input_cached_tokens"),
        sum_bigint_sql("output_tokens"),
        sum_bigint_sql("billable_tokens"),
        sum_bigint_sql("credit_missing_events"),
    )
}

fn filtered_archived_segments_sql_for_scope_only() -> String {
    let matching_row_count = sum_bigint_sql("r.row_count");
    let input_uncached_tokens = sum_bigint_sql("r.input_uncached_tokens");
    let input_cached_tokens = sum_bigint_sql("r.input_cached_tokens");
    let output_tokens = sum_bigint_sql("r.output_tokens");
    let billable_tokens = sum_bigint_sql("r.billable_tokens");
    format!(
        "SELECT
            s.archive_path,
            s.start_ms,
            s.end_ms,
            s.row_count,
            CASE
                WHEN $3::TEXT IS NULL AND $4::TEXT IS NULL THEN s.row_count::BIGINT
                ELSE {matching_row_count}
            END AS matching_row_count,
            CASE
                WHEN $3::TEXT IS NULL AND $4::TEXT IS NULL THEN s.input_uncached_tokens
                ELSE {input_uncached_tokens}
            END AS input_uncached_tokens,
            CASE
                WHEN $3::TEXT IS NULL AND $4::TEXT IS NULL THEN s.input_cached_tokens
                ELSE {input_cached_tokens}
            END AS input_cached_tokens,
            CASE
                WHEN $3::TEXT IS NULL AND $4::TEXT IS NULL THEN s.output_tokens
                ELSE {output_tokens}
            END AS output_tokens,
            CASE
                WHEN $3::TEXT IS NULL AND $4::TEXT IS NULL THEN s.billable_tokens
                ELSE {billable_tokens}
            END AS billable_tokens
         FROM llm_usage_segments s
         LEFT JOIN llm_usage_segment_key_rollups r
           ON r.segment_id = s.segment_id
          AND ($3::TEXT IS NULL OR r.key_id = $3)
          AND ($4::TEXT IS NULL OR r.provider_type = $4)
          AND {scoped_time_overlap}
         WHERE s.state = 'archived'
           AND {segment_time_overlap}
         GROUP BY
            s.segment_id, s.archive_path, s.start_ms, s.end_ms, s.row_count,
            s.input_uncached_tokens, s.input_cached_tokens, s.output_tokens,
            s.billable_tokens
         HAVING ($3::TEXT IS NULL AND $4::TEXT IS NULL) OR {matching_row_count} > 0
         ORDER BY COALESCE(s.end_ms, 0) DESC, s.segment_id DESC",
        scoped_time_overlap = scoped_time_overlap_sql("r"),
        segment_time_overlap = segment_time_overlap_sql("s"),
    )
}

fn filtered_archived_segments_sql_for_single_field() -> String {
    let matching_row_count = sum_bigint_sql("f.row_count");
    let input_uncached_tokens = sum_bigint_sql("f.input_uncached_tokens");
    let input_cached_tokens = sum_bigint_sql("f.input_cached_tokens");
    let output_tokens = sum_bigint_sql("f.output_tokens");
    let billable_tokens = sum_bigint_sql("f.billable_tokens");
    format!(
        "SELECT
            s.archive_path,
            s.start_ms,
            s.end_ms,
            s.row_count,
            {matching_row_count} AS matching_row_count,
            {input_uncached_tokens} AS input_uncached_tokens,
            {input_cached_tokens} AS input_cached_tokens,
            {output_tokens} AS output_tokens,
            {billable_tokens} AS billable_tokens
         FROM llm_usage_segments s
         JOIN llm_usage_segment_field_rollups f
           ON f.segment_id = s.segment_id
         WHERE s.state = 'archived'
           AND {segment_time_overlap}
           AND {field_scope}
           AND {field_time_overlap}
           AND f.field_name = $5
           AND f.field_value = $6
         GROUP BY s.segment_id, s.archive_path, s.start_ms, s.end_ms, s.row_count
         HAVING {matching_row_count} > 0
         ORDER BY COALESCE(s.end_ms, 0) DESC, s.segment_id DESC",
        segment_time_overlap = segment_time_overlap_sql("s"),
        field_scope = field_rollup_scope_sql("f"),
        field_time_overlap = scoped_time_overlap_sql("f"),
    )
}

fn filtered_archived_segments_sql_for_multi_field(filter_count: usize) -> String {
    let mut exists_sql = Vec::new();
    for index in 0..filter_count {
        let alias = format!("f{index}");
        let field_param = 5 + index * 2;
        let value_param = field_param + 1;
        exists_sql.push(format!(
            "EXISTS (
                SELECT 1
                FROM llm_usage_segment_field_rollups {alias}
                WHERE {alias}.segment_id = s.segment_id
                  AND {field_scope}
                  AND {field_time_overlap}
                  AND {alias}.field_name = ${field_param}
                  AND {alias}.field_value = ${value_param}
            )",
            field_scope = field_rollup_scope_sql(&alias),
            field_time_overlap = scoped_time_overlap_sql(&alias),
        ));
    }
    let exists_sql = if exists_sql.is_empty() {
        "TRUE".to_string()
    } else {
        exists_sql.join("\n           AND ")
    };
    format!(
        "SELECT
            s.archive_path,
            s.start_ms,
            s.end_ms,
            s.row_count,
            NULL::BIGINT AS matching_row_count,
            NULL::BIGINT AS input_uncached_tokens,
            NULL::BIGINT AS input_cached_tokens,
            NULL::BIGINT AS output_tokens,
            NULL::BIGINT AS billable_tokens
         FROM llm_usage_segments s
         WHERE s.state = 'archived'
           AND {segment_time_overlap}
           AND {exists_sql}
         ORDER BY COALESCE(s.end_ms, 0) DESC, s.segment_id DESC",
        segment_time_overlap = segment_time_overlap_sql("s"),
    )
}

fn archived_filter_option_values_sql() -> String {
    format!(
        "SELECT DISTINCT f.field_value
         FROM llm_usage_segment_field_rollups f
         JOIN llm_usage_segments s ON s.segment_id = f.segment_id
         WHERE s.state = 'archived'
           AND {segment_time_overlap}
           AND {field_scope}
           AND {field_time_overlap}
           AND f.field_name = $5
         ORDER BY f.field_value",
        segment_time_overlap = segment_time_overlap_sql("s"),
        field_scope = field_rollup_scope_sql("f"),
        field_time_overlap = scoped_time_overlap_sql("f"),
    )
}

fn insert_rollups(
    tx: &mut postgres::Transaction<'_>,
    segment_id: &str,
    rollups: &[UsageCatalogKeyRollupRecord],
) -> anyhow::Result<()> {
    if rollups.is_empty() {
        return Ok(());
    }
    let key_ids = rollups
        .iter()
        .map(|rollup| rollup.key_id.clone())
        .collect::<Vec<_>>();
    let provider_types = rollups
        .iter()
        .map(|rollup| rollup.provider_type.clone())
        .collect::<Vec<_>>();
    let row_counts = rollups
        .iter()
        .map(|rollup| usize_to_i64(rollup.row_count))
        .collect::<Vec<_>>();
    let input_uncached_tokens = rollups
        .iter()
        .map(|rollup| rollup.input_uncached_tokens)
        .collect::<Vec<_>>();
    let input_cached_tokens = rollups
        .iter()
        .map(|rollup| rollup.input_cached_tokens)
        .collect::<Vec<_>>();
    let output_tokens = rollups
        .iter()
        .map(|rollup| rollup.output_tokens)
        .collect::<Vec<_>>();
    let billable_tokens = rollups
        .iter()
        .map(|rollup| rollup.billable_tokens)
        .collect::<Vec<_>>();
    let credit_totals = rollups
        .iter()
        .map(|rollup| normalize_credit_total(&rollup.credit_total))
        .collect::<Vec<_>>();
    let credit_missing_events = rollups
        .iter()
        .map(|rollup| rollup.credit_missing_events)
        .collect::<Vec<_>>();
    let first_used_at_ms = rollups
        .iter()
        .map(|rollup| rollup.first_used_at_ms)
        .collect::<Vec<_>>();
    let last_used_at_ms = rollups
        .iter()
        .map(|rollup| rollup.last_used_at_ms)
        .collect::<Vec<_>>();
    tx.execute(
        "INSERT INTO llm_usage_segment_key_rollups (
            segment_id, key_id, provider_type, row_count, input_uncached_tokens,
            input_cached_tokens, output_tokens, billable_tokens, credit_total,
            credit_missing_events, first_used_at_ms, last_used_at_ms
         )
         SELECT
            $1,
            data.key_id,
            data.provider_type,
            data.row_count,
            data.input_uncached_tokens,
            data.input_cached_tokens,
            data.output_tokens,
            data.billable_tokens,
            data.credit_total,
            data.credit_missing_events,
            data.first_used_at_ms,
            data.last_used_at_ms
         FROM UNNEST(
            $2::TEXT[],
            $3::TEXT[],
            $4::BIGINT[],
            $5::BIGINT[],
            $6::BIGINT[],
            $7::BIGINT[],
            $8::BIGINT[],
            $9::TEXT[],
            $10::BIGINT[],
            $11::BIGINT[],
            $12::BIGINT[]
         ) AS data(
            key_id,
            provider_type,
            row_count,
            input_uncached_tokens,
            input_cached_tokens,
            output_tokens,
            billable_tokens,
            credit_total,
            credit_missing_events,
            first_used_at_ms,
            last_used_at_ms
         )
         ON CONFLICT (segment_id, key_id, provider_type) DO UPDATE
         SET row_count = EXCLUDED.row_count,
             input_uncached_tokens = EXCLUDED.input_uncached_tokens,
             input_cached_tokens = EXCLUDED.input_cached_tokens,
             output_tokens = EXCLUDED.output_tokens,
             billable_tokens = EXCLUDED.billable_tokens,
             credit_total = EXCLUDED.credit_total,
             credit_missing_events = EXCLUDED.credit_missing_events,
             first_used_at_ms = EXCLUDED.first_used_at_ms,
             last_used_at_ms = EXCLUDED.last_used_at_ms",
        &[
            &segment_id,
            &key_ids,
            &provider_types,
            &row_counts,
            &input_uncached_tokens,
            &input_cached_tokens,
            &output_tokens,
            &billable_tokens,
            &credit_totals,
            &credit_missing_events,
            &first_used_at_ms,
            &last_used_at_ms,
        ],
    )
    .context("insert archived segment rollups")?;
    Ok(())
}

fn insert_field_rollups(
    tx: &mut postgres::Transaction<'_>,
    segment_id: &str,
    field_rollups: &[UsageCatalogFieldRollupRecord],
) -> anyhow::Result<()> {
    if field_rollups.is_empty() {
        return Ok(());
    }
    let key_ids = field_rollups
        .iter()
        .map(|rollup| rollup.key_id.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    let provider_types = field_rollups
        .iter()
        .map(|rollup| rollup.provider_type.clone().unwrap_or_default())
        .collect::<Vec<_>>();
    let field_names = field_rollups
        .iter()
        .map(|rollup| rollup.field_name.as_storage_str())
        .collect::<Vec<_>>();
    let field_values = field_rollups
        .iter()
        .map(|rollup| rollup.field_value.clone())
        .collect::<Vec<_>>();
    let row_counts = field_rollups
        .iter()
        .map(|rollup| usize_to_i64(rollup.row_count))
        .collect::<Vec<_>>();
    let input_uncached_tokens = field_rollups
        .iter()
        .map(|rollup| rollup.input_uncached_tokens)
        .collect::<Vec<_>>();
    let input_cached_tokens = field_rollups
        .iter()
        .map(|rollup| rollup.input_cached_tokens)
        .collect::<Vec<_>>();
    let output_tokens = field_rollups
        .iter()
        .map(|rollup| rollup.output_tokens)
        .collect::<Vec<_>>();
    let billable_tokens = field_rollups
        .iter()
        .map(|rollup| rollup.billable_tokens)
        .collect::<Vec<_>>();
    let first_used_at_ms = field_rollups
        .iter()
        .map(|rollup| rollup.first_used_at_ms)
        .collect::<Vec<_>>();
    let last_used_at_ms = field_rollups
        .iter()
        .map(|rollup| rollup.last_used_at_ms)
        .collect::<Vec<_>>();
    tx.execute(
        "INSERT INTO llm_usage_segment_field_rollups (
            segment_id, key_id, provider_type, field_name, field_value, row_count,
            input_uncached_tokens, input_cached_tokens, output_tokens,
            billable_tokens, first_used_at_ms, last_used_at_ms
         )
         SELECT
            $1,
            data.key_id,
            data.provider_type,
            data.field_name,
            data.field_value,
            data.row_count,
            data.input_uncached_tokens,
            data.input_cached_tokens,
            data.output_tokens,
            data.billable_tokens,
            data.first_used_at_ms,
            data.last_used_at_ms
         FROM UNNEST(
            $2::TEXT[],
            $3::TEXT[],
            $4::TEXT[],
            $5::TEXT[],
            $6::BIGINT[],
            $7::BIGINT[],
            $8::BIGINT[],
            $9::BIGINT[],
            $10::BIGINT[],
            $11::BIGINT[],
            $12::BIGINT[]
         ) AS data(
            key_id,
            provider_type,
            field_name,
            field_value,
            row_count,
            input_uncached_tokens,
            input_cached_tokens,
            output_tokens,
            billable_tokens,
            first_used_at_ms,
            last_used_at_ms
         )
         ON CONFLICT (segment_id, key_id, provider_type, field_name, field_value) DO UPDATE
         SET row_count = EXCLUDED.row_count,
             input_uncached_tokens = EXCLUDED.input_uncached_tokens,
             input_cached_tokens = EXCLUDED.input_cached_tokens,
             output_tokens = EXCLUDED.output_tokens,
             billable_tokens = EXCLUDED.billable_tokens,
             first_used_at_ms = EXCLUDED.first_used_at_ms,
             last_used_at_ms = EXCLUDED.last_used_at_ms",
        &[
            &segment_id,
            &key_ids,
            &provider_types,
            &field_names,
            &field_values,
            &row_counts,
            &input_uncached_tokens,
            &input_cached_tokens,
            &output_tokens,
            &billable_tokens,
            &first_used_at_ms,
            &last_used_at_ms,
        ],
    )
    .context("insert archived segment field rollups")?;
    Ok(())
}

fn insert_event_locators(
    tx: &mut postgres::Transaction<'_>,
    segment_id: &str,
    event_ids: &[String],
) -> anyhow::Result<()> {
    const EVENT_LOCATOR_CHUNK_SIZE: usize = 4_096;

    for chunk in event_ids.chunks(EVENT_LOCATOR_CHUNK_SIZE) {
        tx.execute(
            "INSERT INTO llm_usage_segment_events (event_id, segment_id)
             SELECT event_id, $2
             FROM UNNEST($1::TEXT[]) AS event_id
             ON CONFLICT (event_id) DO UPDATE
             SET segment_id = EXCLUDED.segment_id",
            &[&chunk, &segment_id],
        )
        .context("insert archived segment event locators")?;
    }
    Ok(())
}

fn decode_segment_match_row(row: postgres::Row) -> anyhow::Result<UsageCatalogSegmentMatch> {
    let matching_row_count: Option<i64> = row.get(4);
    let matching_totals = match matching_row_count {
        Some(event_count) => Some(UsageCatalogSegmentTotals {
            event_count: i64_to_usize(event_count)?,
            input_uncached_tokens: u64::try_from(row.get::<_, Option<i64>>(5).unwrap_or(0).max(0))
                .unwrap_or(u64::MAX),
            input_cached_tokens: u64::try_from(row.get::<_, Option<i64>>(6).unwrap_or(0).max(0))
                .unwrap_or(u64::MAX),
            output_tokens: u64::try_from(row.get::<_, Option<i64>>(7).unwrap_or(0).max(0))
                .unwrap_or(u64::MAX),
            billable_tokens: u64::try_from(row.get::<_, Option<i64>>(8).unwrap_or(0).max(0))
                .unwrap_or(u64::MAX),
        }),
        None => None,
    };
    Ok(UsageCatalogSegmentMatch {
        segment: UsageCatalogSegment {
            archive_path: PathBuf::from(row.get::<_, String>(0)),
            start_ms: row.get(1),
            end_ms: row.get(2),
            row_count: i64_to_usize(row.get(3))?,
        },
        matching_totals,
    })
}

fn decode_segment_row(row: postgres::Row) -> anyhow::Result<UsageCatalogSegment> {
    Ok(UsageCatalogSegment {
        archive_path: PathBuf::from(row.get::<_, String>(0)),
        start_ms: row.get(1),
        end_ms: row.get(2),
        row_count: i64_to_usize(row.get(3))?,
    })
}

fn option_i64_key(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "-".to_string())
}

fn option_str_key(value: Option<&str>) -> String {
    value
        .map(ToString::to_string)
        .unwrap_or_else(|| "-".to_string())
}

fn normalize_credit_total(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

fn i64_to_usize(value: i64) -> anyhow::Result<usize> {
    usize::try_from(value).with_context(|| format!("catalog value `{value}` exceeds usize"))
}

fn usize_to_i64(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn u64_to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn parse_sequence_from_segment_id(segment_id: &str) -> Option<u64> {
    segment_id
        .rsplit('-')
        .next()
        .and_then(|raw| raw.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    #[test]
    fn postgres_usage_catalog_sum_queries_cast_back_to_bigint() {
        let rollups_sql = super::archived_key_usage_rollups_sql();
        assert!(rollups_sql.contains("COALESCE(SUM(input_uncached_tokens), 0)::BIGINT"));
        assert!(rollups_sql.contains("COALESCE(SUM(billable_tokens), 0)::BIGINT"));
        assert!(rollups_sql.contains("COALESCE(SUM(credit_missing_events), 0)::BIGINT"));

        let filtered_sql = super::filtered_archived_segments_sql_for_scope_only();
        assert!(filtered_sql.contains("COALESCE(SUM(r.row_count), 0)::BIGINT"));
        assert!(filtered_sql.contains("COALESCE(SUM(r.row_count), 0)::BIGINT > 0"));
    }

    #[test]
    fn postgres_usage_catalog_field_rollup_scope_uses_empty_sentinel() {
        let sql = super::field_rollup_scope_sql("f");
        assert!(sql.contains("f.key_id = ''"));
        assert!(sql.contains("f.provider_type = ''"));
        assert!(sql.contains("(f.key_id <> '' OR f.provider_type <> '')"));
    }

    #[test]
    fn parse_sequence_from_segment_id_accepts_current_format() {
        assert_eq!(
            super::parse_sequence_from_segment_id("usage-1700000000000-000000000123"),
            Some(123)
        );
        assert_eq!(super::parse_sequence_from_segment_id("usage-bad"), None);
    }

    #[test]
    fn normalize_credit_total_replaces_empty_string() {
        assert_eq!(super::normalize_credit_total(""), "0");
        assert_eq!(super::normalize_credit_total("  "), "0");
        assert_eq!(super::normalize_credit_total("1.25"), "1.25");
    }
}
