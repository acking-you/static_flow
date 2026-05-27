//! Postgres-backed archived usage catalog metadata for tiered analytics.

use std::{collections::HashSet, path::PathBuf};

use anyhow::{anyhow, Context};
use native_tls::TlsConnector;
use postgres::Client;
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

/// One archived segment plus its pre-aggregated matching row count.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UsageCatalogSegmentCount {
    /// Archived segment metadata.
    pub segment: UsageCatalogSegment,
    /// Matching catalog row count for the filter.
    pub matching_row_count: usize,
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Archived DuckDB size in bytes.
    pub size_bytes: u64,
    /// Segment seal timestamp.
    pub sealed_at_ms: i64,
}

/// Per-key rollup row written into the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Latest usage time for this key in the segment.
    pub last_used_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogRollupsLookup {
    generation: i64,
    rollups: Vec<KeyUsageRollupSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogSegmentsLookup {
    generation: i64,
    segments: Vec<UsageCatalogSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogFilteredSegmentsLookup {
    generation: i64,
    segments: Vec<UsageCatalogSegmentCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CachedUsageCatalogEventLocatorLookup {
    generation: i64,
    segment: Option<UsageCatalogSegment>,
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
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        self.with_client("segment publication", |client| {
            let mut tx = client
                .transaction()
                .context("begin usage catalog transaction")?;
            tx.execute(
                "INSERT INTO llm_usage_segments (
                    segment_id, archive_path, state, start_ms, end_ms, row_count, size_bytes, \
                 sealed_at_ms
                 ) VALUES ($1, $2, 'archived', $3, $4, $5, $6, $7)
                 ON CONFLICT (segment_id) DO UPDATE
                 SET archive_path = EXCLUDED.archive_path,
                     state = EXCLUDED.state,
                     start_ms = EXCLUDED.start_ms,
                     end_ms = EXCLUDED.end_ms,
                     row_count = EXCLUDED.row_count,
                     size_bytes = EXCLUDED.size_bytes,
                     sealed_at_ms = EXCLUDED.sealed_at_ms",
                &[
                    &segment.segment_id,
                    &segment.archive_path.to_string_lossy().to_string(),
                    &segment.start_ms,
                    &segment.end_ms,
                    &usize_to_i64(segment.row_count),
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
            insert_rollups(&mut tx, &segment.segment_id, rollups)?;
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

    /// Return archived segments intersecting the requested time window.
    pub(crate) fn archived_segments_for_query(
        &self,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
    ) -> anyhow::Result<Vec<UsageCatalogSegment>> {
        let generation = self.current_generation();
        let query_fingerprint =
            format!("start:{}:end:{}", option_i64_key(start_ms), option_i64_key(end_ms));
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_segments_key(&query_fingerprint);
            match cache.get_json_blocking::<CachedUsageCatalogSegmentsLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.segments),
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache archived segment read failed; falling back to postgres"
                ),
            }
            let segments = self.load_archived_segments_for_query(start_ms, end_ms)?;
            let payload = CachedUsageCatalogSegmentsLookup {
                generation,
                segments: segments.clone(),
            };
            if let Err(err) = cache.set_json_blocking(
                &cache_key,
                &payload,
                cache.usage_catalog_segments_ttl(&query_fingerprint),
            ) {
                tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache archived segment write failed"
                );
            }
            return Ok(segments);
        }
        self.load_archived_segments_for_query(start_ms, end_ms)
    }

    /// Return archived segments plus pre-aggregated matching row counts.
    pub(crate) fn archived_segments_with_catalog_counts(
        &self,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        key_id: Option<&str>,
        provider_type: Option<&str>,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentCount>> {
        let generation = self.current_generation();
        let query_fingerprint = format!(
            "start:{}:end:{}:key:{}:provider:{}",
            option_i64_key(start_ms),
            option_i64_key(end_ms),
            option_str_key(key_id),
            option_str_key(provider_type)
        );
        if let Some(cache) = self.request_cache.as_ref() {
            let cache_key = cache.usage_catalog_filtered_segments_key(&query_fingerprint);
            match cache.get_json_blocking::<CachedUsageCatalogFilteredSegmentsLookup>(&cache_key) {
                Ok(Some(lookup)) if lookup.generation == generation => return Ok(lookup.segments),
                Ok(Some(_)) | Ok(None) => {},
                Err(err) => tracing::warn!(
                    key = %cache_key,
                    error = %err,
                    "request cache filtered archived segment read failed; falling back to postgres"
                ),
            }
            let segments = self.load_archived_segments_with_catalog_counts(
                start_ms,
                end_ms,
                key_id,
                provider_type,
            )?;
            let payload = CachedUsageCatalogFilteredSegmentsLookup {
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
        self.load_archived_segments_with_catalog_counts(start_ms, end_ms, key_id, provider_type)
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
                .query(
                    "SELECT
                        key_id,
                        COALESCE(SUM(input_uncached_tokens), 0),
                        COALESCE(SUM(input_cached_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(billable_tokens), 0),
                        COALESCE(SUM((credit_total)::numeric), 0)::text,
                        COALESCE(SUM(credit_missing_events), 0),
                        MAX(last_used_at_ms)
                     FROM llm_usage_segment_key_rollups
                     GROUP BY key_id",
                    &[],
                )
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

    fn load_archived_segments_for_query(
        &self,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
    ) -> anyhow::Result<Vec<UsageCatalogSegment>> {
        self.with_client("segment lookup", |client| {
            let rows = client
                .query(
                    "SELECT archive_path, start_ms, end_ms, row_count
                     FROM llm_usage_segments
                     WHERE state = 'archived'
                       AND ($1::BIGINT IS NULL OR end_ms IS NULL OR end_ms >= $1)
                       AND ($2::BIGINT IS NULL OR start_ms IS NULL OR start_ms < $2)
                     ORDER BY COALESCE(end_ms, 0) DESC, segment_id DESC",
                    &[&start_ms, &end_ms],
                )
                .context("query archived segments")?;
            rows.into_iter().map(decode_segment_row).collect()
        })
    }

    fn load_archived_segments_with_catalog_counts(
        &self,
        start_ms: Option<i64>,
        end_ms: Option<i64>,
        key_id: Option<&str>,
        provider_type: Option<&str>,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentCount>> {
        self.with_client("filtered segment lookup", |client| {
            let rows = client
                .query(
                    "SELECT
                        s.archive_path,
                        s.start_ms,
                        s.end_ms,
                        s.row_count,
                        COALESCE(SUM(r.row_count), 0) AS matching_row_count
                     FROM llm_usage_segments s
                     JOIN llm_usage_segment_key_rollups r ON r.segment_id = s.segment_id
                     WHERE s.state = 'archived'
                       AND ($1::BIGINT IS NULL OR s.end_ms IS NULL OR s.end_ms >= $1)
                       AND ($2::BIGINT IS NULL OR s.start_ms IS NULL OR s.start_ms < $2)
                       AND ($3::TEXT IS NULL OR r.key_id = $3)
                       AND ($4::TEXT IS NULL OR r.provider_type = $4)
                     GROUP BY s.segment_id, s.archive_path, s.start_ms, s.end_ms, s.row_count
                     HAVING COALESCE(SUM(r.row_count), 0) > 0
                     ORDER BY COALESCE(s.end_ms, 0) DESC, s.segment_id DESC",
                    &[&start_ms, &end_ms, &key_id, &provider_type],
                )
                .context("query filtered archived segments")?;
            rows.into_iter()
                .map(|row| {
                    Ok(UsageCatalogSegmentCount {
                        segment: UsageCatalogSegment {
                            archive_path: PathBuf::from(row.get::<_, String>(0)),
                            start_ms: row.get(1),
                            end_ms: row.get(2),
                            row_count: i64_to_usize(row.get(3))?,
                        },
                        matching_row_count: i64_to_usize(row.get(4))?,
                    })
                })
                .collect()
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
    let last_used_at_ms = rollups
        .iter()
        .map(|rollup| rollup.last_used_at_ms)
        .collect::<Vec<_>>();
    tx.execute(
        "INSERT INTO llm_usage_segment_key_rollups (
            segment_id, key_id, provider_type, row_count, input_uncached_tokens,
            input_cached_tokens, output_tokens, billable_tokens, credit_total,
            credit_missing_events, last_used_at_ms
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
            $11::BIGINT[]
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
            &last_used_at_ms,
        ],
    )
    .context("insert archived segment rollups")?;
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
