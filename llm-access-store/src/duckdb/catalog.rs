//! Tiered usage-catalog backend (Postgres + in-memory test catalog) and
//! catalog<->usage query translation.

use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::PathBuf,
    sync::Mutex,
};

use anyhow::{anyhow, Context};
use llm_access_core::store::UsageEventQuery;

use super::{
    append::merge_key_rollup,
    segment::{
        parse_sequence_from_segment_id, segment_matches_time_window, sort_archived_segments,
    },
    util::i64_to_u64,
    ArchivedUsageSegment, TestTieredUsageCatalog, TestTieredUsageCatalogState,
    TieredUsageCatalogBackend,
};
use crate::{
    usage_catalog::{
        UsageCatalogFieldFilter, UsageCatalogFieldName, UsageCatalogFieldRollupRecord,
        UsageCatalogKeyRollupRecord, UsageCatalogQuery, UsageCatalogRetentionSegment,
        UsageCatalogSegment, UsageCatalogSegmentMatch, UsageCatalogSegmentRecord,
        UsageCatalogSegmentTotals,
    },
    KeyUsageRollupSummary,
};

#[cfg(feature = "duckdb-runtime")]
impl TieredUsageCatalogBackend {
    pub(super) fn is_empty(&self) -> anyhow::Result<bool> {
        match self {
            Self::Postgres(catalog) => catalog.is_empty(),
            Self::Test(catalog) => catalog.is_empty(),
        }
    }

    pub(super) fn next_sequence(&self) -> anyhow::Result<u64> {
        match self {
            Self::Postgres(catalog) => catalog.next_sequence(),
            Self::Test(catalog) => catalog.next_sequence(),
        }
    }

    pub(super) fn archive_path_for_segment(
        &self,
        segment_id: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archive_path_for_segment(segment_id),
            Self::Test(catalog) => catalog.archive_path_for_segment(segment_id),
        }
    }

    pub(super) fn publish_segment(
        &self,
        segment: &UsageCatalogSegmentRecord,
        rollups: &[UsageCatalogKeyRollupRecord],
        field_rollups: &[UsageCatalogFieldRollupRecord],
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        match self {
            Self::Postgres(catalog) => {
                catalog.publish_segment(segment, rollups, field_rollups, event_ids)
            },
            Self::Test(catalog) => {
                catalog.publish_segment(segment, rollups, field_rollups, event_ids)
            },
        }
    }

    pub(super) fn archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_key_usage_rollups(),
            Self::Test(catalog) => catalog.archived_key_usage_rollups(),
        }
    }

    pub(super) fn delete_expired_segments(
        &self,
        cutoff_ms: i64,
    ) -> anyhow::Result<Vec<UsageCatalogRetentionSegment>> {
        match self {
            Self::Postgres(catalog) => catalog.delete_expired_segments(cutoff_ms),
            Self::Test(catalog) => catalog.delete_expired_segments(cutoff_ms),
        }
    }

    pub(super) fn archived_paths(&self) -> anyhow::Result<HashSet<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_paths(),
            Self::Test(catalog) => catalog.archived_paths(),
        }
    }

    pub(super) fn archived_paths_missing_field_rollups(&self) -> anyhow::Result<Vec<PathBuf>> {
        match self {
            Self::Postgres(catalog) => catalog.archived_paths_missing_field_rollups(),
            Self::Test(catalog) => catalog.archived_paths_missing_field_rollups(),
        }
    }

    pub(super) fn archived_segments_for_query(
        &self,
        query: &UsageEventQuery,
    ) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
        let catalog_query = catalog_query_from_usage_query(query);
        match self {
            Self::Postgres(catalog) => catalog
                .archived_segment_matches_for_query(&catalog_query)
                .map(|segments| {
                    segments
                        .into_iter()
                        .map(|segment| segment.segment.into())
                        .collect()
                }),
            Self::Test(catalog) => catalog.archived_segments_for_query(&catalog_query),
        }
    }

    pub(super) fn archived_segment_matches_for_query(
        &self,
        query: &UsageEventQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let catalog_query = catalog_query_from_usage_query(query);
        match self {
            Self::Postgres(catalog) => catalog.archived_segment_matches_for_query(&catalog_query),
            Self::Test(catalog) => catalog.archived_segment_matches_for_query(&catalog_query),
        }
    }

    pub(super) fn archived_filter_option_values(
        &self,
        query: &UsageEventQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Option<Vec<String>>> {
        let catalog_query = catalog_filter_options_query_from_usage_query(query, field_name);
        match self {
            Self::Postgres(catalog) => {
                catalog.archived_filter_option_values(&catalog_query, field_name)
            },
            Self::Test(catalog) => {
                catalog.archived_filter_option_values(&catalog_query, field_name)
            },
        }
    }

    pub(super) fn locate_archived_segment(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<ArchivedUsageSegment>> {
        match self {
            Self::Postgres(catalog) => catalog
                .locate_archived_segment(event_id)
                .map(|segment| segment.map(Into::into)),
            Self::Test(catalog) => catalog.locate_archived_segment(event_id),
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
impl TestTieredUsageCatalog {
    pub(super) fn open(path: PathBuf) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "failed to create test usage catalog parent directory `{}`",
                    parent.display()
                )
            })?;
        }
        let state = if path.exists() {
            let bytes = fs::read(&path).with_context(|| {
                format!("failed to read test usage catalog state `{}`", path.display())
            })?;
            serde_json::from_slice::<TestTieredUsageCatalogState>(&bytes).with_context(|| {
                format!("failed to deserialize test usage catalog state `{}`", path.display())
            })?
        } else {
            TestTieredUsageCatalogState::default()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
        })
    }

    pub(super) fn lock(
        &self,
    ) -> anyhow::Result<std::sync::MutexGuard<'_, TestTieredUsageCatalogState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("test tiered usage catalog lock poisoned"))
    }

    fn persist(&self, state: &TestTieredUsageCatalogState) -> anyhow::Result<()> {
        let bytes = serde_json::to_vec(state).context("serialize test usage catalog state")?;
        let temp_path = self.path.with_extension("json.tmp");
        fs::write(&temp_path, bytes).with_context(|| {
            format!("failed to write test usage catalog temp state `{}`", temp_path.display())
        })?;
        fs::rename(&temp_path, &self.path).with_context(|| {
            format!("failed to replace test usage catalog state `{}`", self.path.display())
        })?;
        Ok(())
    }

    pub(super) fn is_empty(&self) -> anyhow::Result<bool> {
        Ok(self.lock()?.segments.is_empty())
    }

    pub(super) fn next_sequence(&self) -> anyhow::Result<u64> {
        Ok(self
            .lock()?
            .segments
            .keys()
            .filter_map(|segment_id| parse_sequence_from_segment_id(segment_id))
            .max()
            .unwrap_or(0))
    }

    pub(super) fn archive_path_for_segment(
        &self,
        segment_id: &str,
    ) -> anyhow::Result<Option<PathBuf>> {
        Ok(self
            .lock()?
            .segments
            .get(segment_id)
            .map(|segment| segment.archive_path.clone()))
    }

    pub(super) fn publish_segment(
        &self,
        segment: &UsageCatalogSegmentRecord,
        rollups: &[UsageCatalogKeyRollupRecord],
        field_rollups: &[UsageCatalogFieldRollupRecord],
        event_ids: &[String],
    ) -> anyhow::Result<()> {
        let mut state = self.lock()?;
        state
            .segments
            .insert(segment.segment_id.clone(), segment.clone());
        state
            .segment_rollups
            .insert(segment.segment_id.clone(), rollups.to_vec());
        state
            .segment_field_rollups
            .insert(segment.segment_id.clone(), field_rollups.to_vec());
        state
            .event_locators
            .retain(|_, current_segment_id| current_segment_id != &segment.segment_id);
        for event_id in event_ids {
            state
                .event_locators
                .insert(event_id.clone(), segment.segment_id.clone());
        }
        self.persist(&state)?;
        Ok(())
    }

    pub(super) fn archived_key_usage_rollups(&self) -> anyhow::Result<Vec<KeyUsageRollupSummary>> {
        let state = self.lock()?;
        let mut combined = BTreeMap::<String, KeyUsageRollupSummary>::new();
        for rollup in state.segment_rollups.values().flatten() {
            merge_key_rollup(&mut combined, KeyUsageRollupSummary {
                key_id: rollup.key_id.clone(),
                input_uncached_tokens: rollup.input_uncached_tokens,
                input_cached_tokens: rollup.input_cached_tokens,
                output_tokens: rollup.output_tokens,
                billable_tokens: rollup.billable_tokens,
                credit_total: rollup.credit_total.clone(),
                credit_missing_events: rollup.credit_missing_events,
                last_used_at_ms: rollup.last_used_at_ms,
            });
        }
        Ok(combined.into_values().collect())
    }

    pub(super) fn delete_expired_segments(
        &self,
        cutoff_ms: i64,
    ) -> anyhow::Result<Vec<UsageCatalogRetentionSegment>> {
        let mut state = self.lock()?;
        let mut deleted = state
            .segments
            .iter()
            .filter(|(_, segment)| segment.end_ms.is_some_and(|end_ms| end_ms < cutoff_ms))
            .map(|(segment_id, segment)| UsageCatalogRetentionSegment {
                segment_id: segment_id.clone(),
                archive_path: segment.archive_path.clone(),
            })
            .collect::<Vec<_>>();
        deleted.sort_by(|left, right| left.segment_id.cmp(&right.segment_id));
        if deleted.is_empty() {
            return Ok(deleted);
        }
        let deleted_ids = deleted
            .iter()
            .map(|segment| segment.segment_id.as_str())
            .collect::<HashSet<_>>();
        state
            .segments
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .segment_rollups
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .segment_field_rollups
            .retain(|segment_id, _| !deleted_ids.contains(segment_id.as_str()));
        state
            .event_locators
            .retain(|_, segment_id| !deleted_ids.contains(segment_id.as_str()));
        self.persist(&state)?;
        Ok(deleted)
    }

    pub(super) fn archived_paths(&self) -> anyhow::Result<HashSet<PathBuf>> {
        Ok(self
            .lock()?
            .segments
            .values()
            .map(|segment| segment.archive_path.clone())
            .collect())
    }

    pub(super) fn archived_paths_missing_field_rollups(&self) -> anyhow::Result<Vec<PathBuf>> {
        let state = self.lock()?;
        let mut paths = state
            .segments
            .iter()
            .filter(|(segment_id, _)| {
                state
                    .segment_field_rollups
                    .get(*segment_id)
                    .is_none_or(|rollups| rollups.is_empty())
            })
            .map(|(_, segment)| segment.archive_path.clone())
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    pub(super) fn archived_segments_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<ArchivedUsageSegment>> {
        let state = self.lock()?;
        let mut segments = state
            .segments
            .values()
            .filter(|segment| segment_matches_time_window(segment, query.start_ms, query.end_ms))
            .filter(|segment| {
                test_catalog_segment_matches_query(&state, &segment.segment_id, query)
            })
            .map(archived_segment_from_record)
            .collect::<Vec<_>>();
        sort_archived_segments(&mut segments);
        Ok(segments)
    }

    pub(super) fn archived_segment_matches_for_query(
        &self,
        query: &UsageCatalogQuery,
    ) -> anyhow::Result<Vec<UsageCatalogSegmentMatch>> {
        let state = self.lock()?;
        let mut segments = Vec::new();
        for (segment_id, segment) in state.segments.iter().filter(|(_, segment)| {
            segment_matches_time_window(segment, query.start_ms, query.end_ms)
        }) {
            if !test_catalog_segment_matches_query(&state, segment_id, query) {
                continue;
            }
            let matching_totals =
                test_catalog_segment_totals_for_query(&state, segment_id, segment, query);
            if query.field_filters.len() > 1 || matching_totals.is_some() {
                segments.push(UsageCatalogSegmentMatch {
                    segment: UsageCatalogSegment {
                        archive_path: segment.archive_path.clone(),
                        start_ms: segment.start_ms,
                        end_ms: segment.end_ms,
                        row_count: segment.row_count,
                    },
                    matching_totals,
                });
            }
        }
        segments.sort_by(|left, right| {
            right
                .segment
                .end_ms
                .unwrap_or_default()
                .cmp(&left.segment.end_ms.unwrap_or_default())
                .then_with(|| right.segment.archive_path.cmp(&left.segment.archive_path))
        });
        Ok(segments)
    }

    pub(super) fn archived_filter_option_values(
        &self,
        query: &UsageCatalogQuery,
        field_name: UsageCatalogFieldName,
    ) -> anyhow::Result<Option<Vec<String>>> {
        if !query.field_filters.is_empty() {
            return Ok(None);
        }
        let state = self.lock()?;
        let mut values = BTreeSet::new();
        for (segment_id, _segment) in state.segments.iter().filter(|(_, segment)| {
            segment_matches_time_window(segment, query.start_ms, query.end_ms)
        }) {
            let Some(rollups) = state.segment_field_rollups.get(segment_id) else {
                continue;
            };
            for rollup in rollups {
                if rollup.field_name != field_name {
                    continue;
                }
                if !test_field_rollup_matches_scope(rollup, query) {
                    continue;
                }
                if !test_rollup_matches_time(
                    rollup.first_used_at_ms,
                    rollup.last_used_at_ms,
                    query.start_ms,
                    query.end_ms,
                ) {
                    continue;
                }
                values.insert(rollup.field_value.clone());
            }
        }
        Ok(Some(values.into_iter().collect()))
    }

    pub(super) fn locate_archived_segment(
        &self,
        event_id: &str,
    ) -> anyhow::Result<Option<ArchivedUsageSegment>> {
        let state = self.lock()?;
        let Some(segment_id) = state.event_locators.get(event_id) else {
            return Ok(None);
        };
        Ok(state
            .segments
            .get(segment_id)
            .map(archived_segment_from_record))
    }
}
#[cfg(feature = "duckdb-runtime")]
impl From<UsageCatalogSegment> for ArchivedUsageSegment {
    fn from(value: UsageCatalogSegment) -> Self {
        Self {
            archive_path: value.archive_path,
            start_ms: value.start_ms,
            end_ms: value.end_ms,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
fn archived_segment_from_record(record: &UsageCatalogSegmentRecord) -> ArchivedUsageSegment {
    ArchivedUsageSegment {
        archive_path: record.archive_path.clone(),
        start_ms: record.start_ms,
        end_ms: record.end_ms,
    }
}
#[cfg(feature = "duckdb-runtime")]
fn catalog_query_from_usage_query(query: &UsageEventQuery) -> UsageCatalogQuery {
    let mut field_filters = Vec::new();
    if let Some(model) = query.model.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::Model,
            field_value: model.clone(),
        });
    }
    if let Some(account_name) = query.account_name.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::AccountName,
            field_value: account_name.clone(),
        });
    }
    if let Some(endpoint) = query.endpoint.as_ref() {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::Endpoint,
            field_value: endpoint.clone(),
        });
    }
    if let Some(status_code) = query.status_code {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::StatusCode,
            field_value: status_code.to_string(),
        });
    }
    if let Some(status_kind) = query.status_kind {
        field_filters.push(UsageCatalogFieldFilter {
            field_name: UsageCatalogFieldName::StatusKind,
            field_value: status_kind.as_query_value().to_string(),
        });
    }
    UsageCatalogQuery {
        start_ms: query.start_ms,
        end_ms: query.end_ms,
        key_id: query.key_id.clone(),
        provider_type: query.provider_type.clone(),
        field_filters,
    }
}
#[cfg(feature = "duckdb-runtime")]
fn catalog_filter_options_query_from_usage_query(
    query: &UsageEventQuery,
    field_name: UsageCatalogFieldName,
) -> UsageCatalogQuery {
    catalog_query_from_usage_query(&usage_filter_options_query_for_catalog_field(query, field_name))
}
#[cfg(feature = "duckdb-runtime")]
fn usage_filter_options_query_for_catalog_field(
    query: &UsageEventQuery,
    field_name: UsageCatalogFieldName,
) -> UsageEventQuery {
    let mut scoped = query.clone();
    match field_name {
        UsageCatalogFieldName::Model => scoped.model = None,
        UsageCatalogFieldName::AccountName => scoped.account_name = None,
        UsageCatalogFieldName::Endpoint => scoped.endpoint = None,
        UsageCatalogFieldName::StatusCode => scoped.status_code = None,
        UsageCatalogFieldName::StatusKind => scoped.status_kind = None,
    }
    scoped.limit = 1;
    scoped.offset = 0;
    scoped
}
#[cfg(feature = "duckdb-runtime")]
fn catalog_query_has_exact_totals(query: &UsageCatalogQuery) -> bool {
    query.field_filters.len() <= 1
}
#[cfg(feature = "duckdb-runtime")]
fn test_catalog_segment_matches_query(
    state: &TestTieredUsageCatalogState,
    segment_id: &str,
    query: &UsageCatalogQuery,
) -> bool {
    if query.field_filters.is_empty() {
        if query.key_id.is_none() && query.provider_type.is_none() {
            return true;
        }
        return state
            .segment_rollups
            .get(segment_id)
            .into_iter()
            .flatten()
            .any(|rollup| {
                test_key_rollup_matches_scope(rollup, query)
                    && test_rollup_matches_time(
                        rollup.first_used_at_ms,
                        rollup.last_used_at_ms,
                        query.start_ms,
                        query.end_ms,
                    )
            });
    }
    let Some(field_rollups) = state.segment_field_rollups.get(segment_id) else {
        return false;
    };
    query.field_filters.iter().all(|filter| {
        field_rollups.iter().any(|rollup| {
            rollup.field_name == filter.field_name
                && rollup.field_value == filter.field_value
                && test_field_rollup_matches_scope(rollup, query)
                && test_rollup_matches_time(
                    rollup.first_used_at_ms,
                    rollup.last_used_at_ms,
                    query.start_ms,
                    query.end_ms,
                )
        })
    })
}
#[cfg(feature = "duckdb-runtime")]
fn test_catalog_segment_totals_for_query(
    state: &TestTieredUsageCatalogState,
    segment_id: &str,
    segment: &UsageCatalogSegmentRecord,
    query: &UsageCatalogQuery,
) -> Option<UsageCatalogSegmentTotals> {
    if !catalog_query_has_exact_totals(query) {
        return None;
    }
    if query.field_filters.is_empty() {
        if query.key_id.is_none() && query.provider_type.is_none() {
            return Some(UsageCatalogSegmentTotals {
                event_count: segment.row_count,
                input_uncached_tokens: i64_to_u64(segment.input_uncached_tokens),
                input_cached_tokens: i64_to_u64(segment.input_cached_tokens),
                output_tokens: i64_to_u64(segment.output_tokens),
                billable_tokens: i64_to_u64(segment.billable_tokens),
            });
        }
        let totals = state
            .segment_rollups
            .get(segment_id)
            .into_iter()
            .flatten()
            .filter(|rollup| test_key_rollup_matches_scope(rollup, query))
            .fold(
                UsageCatalogSegmentTotals {
                    event_count: 0,
                    input_uncached_tokens: 0,
                    input_cached_tokens: 0,
                    output_tokens: 0,
                    billable_tokens: 0,
                },
                |mut totals, rollup| {
                    totals.event_count = totals.event_count.saturating_add(rollup.row_count);
                    totals.input_uncached_tokens = totals
                        .input_uncached_tokens
                        .saturating_add(i64_to_u64(rollup.input_uncached_tokens));
                    totals.input_cached_tokens = totals
                        .input_cached_tokens
                        .saturating_add(i64_to_u64(rollup.input_cached_tokens));
                    totals.output_tokens = totals
                        .output_tokens
                        .saturating_add(i64_to_u64(rollup.output_tokens));
                    totals.billable_tokens = totals
                        .billable_tokens
                        .saturating_add(i64_to_u64(rollup.billable_tokens));
                    totals
                },
            );
        return Some(totals);
    }
    let filter = query.field_filters.first()?;
    let totals = state
        .segment_field_rollups
        .get(segment_id)
        .into_iter()
        .flatten()
        .filter(|rollup| {
            rollup.field_name == filter.field_name
                && rollup.field_value == filter.field_value
                && test_field_rollup_matches_scope(rollup, query)
        })
        .fold(
            UsageCatalogSegmentTotals {
                event_count: 0,
                input_uncached_tokens: 0,
                input_cached_tokens: 0,
                output_tokens: 0,
                billable_tokens: 0,
            },
            |mut totals, rollup| {
                totals.event_count = totals.event_count.saturating_add(rollup.row_count);
                totals.input_uncached_tokens = totals
                    .input_uncached_tokens
                    .saturating_add(i64_to_u64(rollup.input_uncached_tokens));
                totals.input_cached_tokens = totals
                    .input_cached_tokens
                    .saturating_add(i64_to_u64(rollup.input_cached_tokens));
                totals.output_tokens = totals
                    .output_tokens
                    .saturating_add(i64_to_u64(rollup.output_tokens));
                totals.billable_tokens = totals
                    .billable_tokens
                    .saturating_add(i64_to_u64(rollup.billable_tokens));
                totals
            },
        );
    Some(totals)
}
#[cfg(feature = "duckdb-runtime")]
fn test_key_rollup_matches_scope(
    rollup: &UsageCatalogKeyRollupRecord,
    query: &UsageCatalogQuery,
) -> bool {
    query
        .key_id
        .as_deref()
        .is_none_or(|key_id| rollup.key_id == key_id)
        && query
            .provider_type
            .as_deref()
            .is_none_or(|provider_type| rollup.provider_type == provider_type)
}
#[cfg(feature = "duckdb-runtime")]
fn test_field_rollup_matches_scope(
    rollup: &UsageCatalogFieldRollupRecord,
    query: &UsageCatalogQuery,
) -> bool {
    match (query.key_id.as_deref(), query.provider_type.as_deref()) {
        (None, None) => rollup.key_id.is_none() && rollup.provider_type.is_none(),
        (key_id, provider_type) => {
            key_id.is_none_or(|key_id| rollup.key_id.as_deref() == Some(key_id))
                && provider_type.is_none_or(|provider_type| {
                    rollup.provider_type.as_deref() == Some(provider_type)
                })
        },
    }
}
#[cfg(feature = "duckdb-runtime")]
fn test_rollup_matches_time(
    first_used_at_ms: Option<i64>,
    last_used_at_ms: Option<i64>,
    start_ms: Option<i64>,
    end_ms: Option<i64>,
) -> bool {
    (start_ms.is_none() || last_used_at_ms.is_none() || last_used_at_ms >= start_ms)
        && (end_ms.is_none() || first_used_at_ms.is_none() || first_used_at_ms < end_ms)
}
