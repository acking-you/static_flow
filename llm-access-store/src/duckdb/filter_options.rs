//! Usage filter-option discovery across active and archived segments.

use std::{
    collections::{BTreeSet, HashSet},
    path::Path,
};

use anyhow::Context;
use llm_access_core::store::{
    UsageEventQuery, UsageEventStatusKind, UsageEventTotals, UsageFilterOptions,
};

use super::{
    query::archived_segments_for_query,
    sql::{duckdb_table_columns, usage_event_filter_column_sql},
    DuckDbUsageRepository, TieredDuckDbUsageConfig, TieredUsageCatalogBackend,
    UsageFilterOptionField,
};
use crate::usage_catalog::UsageCatalogFieldName;

#[cfg(feature = "duckdb-runtime")]
impl UsageFilterOptionField {
    fn catalog_field_name(self) -> UsageCatalogFieldName {
        match self {
            Self::Model => UsageCatalogFieldName::Model,
            Self::Account => UsageCatalogFieldName::AccountName,
            Self::Endpoint => UsageCatalogFieldName::Endpoint,
        }
    }
}
#[cfg(feature = "duckdb-runtime")]
pub fn list_usage_filter_options_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageFilterOptions> {
    fetch_usage_filter_options_from_conn(conn, query)
}
#[cfg(feature = "duckdb-runtime")]
pub fn list_usage_filter_options_from_tiered(
    _config: &TieredDuckDbUsageConfig,
    catalog_backend: &TieredUsageCatalogBackend,
    active_path: &Path,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageFilterOptions> {
    let mut options = UsageFilterOptions::default();
    if query.source.includes_hot() {
        let conn = DuckDbUsageRepository::open_read_only_conn(active_path)?;
        options = merge_usage_filter_options(
            options,
            fetch_usage_filter_options_from_conn(&conn, query)?,
        );
    }
    if query.source.includes_archive() {
        let mut archived_options = UsageFilterOptions::default();
        let mut missing_fields = Vec::new();
        for field in [
            UsageFilterOptionField::Model,
            UsageFilterOptionField::Account,
            UsageFilterOptionField::Endpoint,
        ] {
            match catalog_backend
                .archived_filter_option_values(query, field.catalog_field_name())?
            {
                Some(values) => {
                    assign_usage_filter_option_values(&mut archived_options, field, values)
                },
                None => missing_fields.push(field),
            }
        }
        if !missing_fields.is_empty() {
            let archived_paths = archived_segments_for_query(catalog_backend, query)?
                .into_iter()
                .map(|segment| segment.archive_path)
                .collect::<Vec<_>>();
            for archived_path in archived_paths {
                let conn = DuckDbUsageRepository::open_read_only_conn(&archived_path)?;
                let scanned = fetch_usage_filter_options_from_conn(&conn, query)?;
                merge_missing_usage_filter_options(&mut archived_options, scanned, &missing_fields);
            }
        }
        options = merge_usage_filter_options(options, archived_options);
    }
    Ok(options)
}
#[cfg(feature = "duckdb-runtime")]
fn fetch_usage_filter_options_from_conn(
    conn: &duckdb::Connection,
    query: &UsageEventQuery,
) -> anyhow::Result<UsageFilterOptions> {
    let columns = duckdb_table_columns(conn, "usage_events")?;
    let sql = usage_filter_options_sql(&columns, "e");
    let mut stmt = conn
        .prepare(&sql)
        .context("prepare duckdb usage filter options query")?;
    let rows = stmt
        .query_map(
            duckdb::params![
                query.key_id.as_deref(),
                query.provider_type.as_deref(),
                query.start_ms,
                query.end_ms,
                query.model.as_deref(),
                query.account_name.as_deref(),
                query.endpoint.as_deref(),
                query.status_code,
                query.status_kind.map(UsageEventStatusKind::as_query_value)
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .context("query duckdb usage filter options")?;
    let mut models = BTreeSet::new();
    let mut accounts = BTreeSet::new();
    let mut endpoints = BTreeSet::new();
    for row in rows {
        let (field_name, value) = row.context("read duckdb usage filter option row")?;
        if value.is_empty() {
            continue;
        }
        match field_name.as_str() {
            "model" => {
                models.insert(value);
            },
            "account_name" => {
                accounts.insert(value);
            },
            "endpoint" => {
                endpoints.insert(value);
            },
            _ => {},
        }
    }
    Ok(UsageFilterOptions {
        models: models.into_iter().collect(),
        accounts: accounts.into_iter().collect(),
        endpoints: endpoints.into_iter().collect(),
    })
}
#[cfg(feature = "duckdb-runtime")]
fn usage_filter_options_sql(columns: &HashSet<String>, table_alias: &str) -> String {
    let model_sql =
        usage_event_filter_column_sql(columns, table_alias, "model", "CAST(NULL AS VARCHAR)");
    let account_sql = usage_event_filter_column_sql(
        columns,
        table_alias,
        "account_name",
        "CAST(NULL AS VARCHAR)",
    );
    let endpoint_sql =
        usage_event_filter_column_sql(columns, table_alias, "endpoint", "CAST(NULL AS VARCHAR)");
    let model_where_sql =
        usage_filter_options_where_sql(columns, table_alias, UsageFilterOptionField::Model);
    let account_where_sql =
        usage_filter_options_where_sql(columns, table_alias, UsageFilterOptionField::Account);
    let endpoint_where_sql =
        usage_filter_options_where_sql(columns, table_alias, UsageFilterOptionField::Endpoint);
    format!(
        "SELECT field_name, value
         FROM (
            SELECT 'model' AS field_name, {model_sql} AS value
            FROM usage_events {table_alias}
            {model_where_sql}
            UNION
            SELECT 'account_name' AS field_name, {account_sql} AS value
            FROM usage_events {table_alias}
            {account_where_sql}
            UNION
            SELECT 'endpoint' AS field_name, {endpoint_sql} AS value
            FROM usage_events {table_alias}
            {endpoint_where_sql}
         ) values_by_field
         WHERE value IS NOT NULL AND length(trim(value)) > 0
         ORDER BY field_name, value"
    )
}
#[cfg(feature = "duckdb-runtime")]
fn usage_filter_options_where_sql(
    columns: &HashSet<String>,
    table_alias: &str,
    cleared_field: UsageFilterOptionField,
) -> String {
    let model_sql =
        usage_event_filter_column_sql(columns, table_alias, "model", "CAST(NULL AS VARCHAR)");
    let account_name_sql = usage_event_filter_column_sql(
        columns,
        table_alias,
        "account_name",
        "CAST(NULL AS VARCHAR)",
    );
    let endpoint_sql =
        usage_event_filter_column_sql(columns, table_alias, "endpoint", "CAST(NULL AS VARCHAR)");
    let status_code_sql =
        usage_event_filter_column_sql(columns, table_alias, "status_code", "CAST(NULL AS INTEGER)");
    let model_predicate = match cleared_field {
        UsageFilterOptionField::Model => "TRUE".to_string(),
        _ => format!("(?5 IS NULL OR {model_sql} = ?5)"),
    };
    let account_predicate = match cleared_field {
        UsageFilterOptionField::Account => "TRUE".to_string(),
        _ => format!("(?6 IS NULL OR {account_name_sql} = ?6)"),
    };
    let endpoint_predicate = match cleared_field {
        UsageFilterOptionField::Endpoint => "TRUE".to_string(),
        _ => format!("(?7 IS NULL OR {endpoint_sql} = ?7)"),
    };
    format!(
        "WHERE (?1 IS NULL OR {table_alias}.key_id = ?1)
      AND (?2 IS NULL OR {table_alias}.provider_type = ?2)
      AND (?3 IS NULL OR {table_alias}.created_at_ms >= ?3)
      AND (?4 IS NULL OR {table_alias}.created_at_ms < ?4)
      AND {model_predicate}
      AND {account_predicate}
      AND {endpoint_predicate}
      AND (?8 IS NULL OR {status_code_sql} = ?8)
      AND (?9 IS NULL
           OR (?9 = 'ok' AND {status_code_sql} = 200)
           OR (?9 = 'non_ok' AND {status_code_sql} <> 200))"
    )
}
#[cfg(feature = "duckdb-runtime")]
fn merge_usage_filter_options(
    mut base: UsageFilterOptions,
    added: UsageFilterOptions,
) -> UsageFilterOptions {
    base.models.extend(added.models);
    base.accounts.extend(added.accounts);
    base.endpoints.extend(added.endpoints);
    base.models.sort();
    base.models.dedup();
    base.accounts.sort();
    base.accounts.dedup();
    base.endpoints.sort();
    base.endpoints.dedup();
    base
}
#[cfg(feature = "duckdb-runtime")]
fn assign_usage_filter_option_values(
    options: &mut UsageFilterOptions,
    field: UsageFilterOptionField,
    mut values: Vec<String>,
) {
    values.sort();
    values.dedup();
    match field {
        UsageFilterOptionField::Model => options.models = values,
        UsageFilterOptionField::Account => options.accounts = values,
        UsageFilterOptionField::Endpoint => options.endpoints = values,
    }
}
#[cfg(feature = "duckdb-runtime")]
fn merge_missing_usage_filter_options(
    target: &mut UsageFilterOptions,
    added: UsageFilterOptions,
    missing_fields: &[UsageFilterOptionField],
) {
    for field in missing_fields {
        match field {
            UsageFilterOptionField::Model => target.models.extend(added.models.clone()),
            UsageFilterOptionField::Account => target.accounts.extend(added.accounts.clone()),
            UsageFilterOptionField::Endpoint => target.endpoints.extend(added.endpoints.clone()),
        }
    }
    target.models.sort();
    target.models.dedup();
    target.accounts.sort();
    target.accounts.dedup();
    target.endpoints.sort();
    target.endpoints.dedup();
}
#[cfg(feature = "duckdb-runtime")]
pub fn merge_usage_event_totals(target: &mut UsageEventTotals, added: &UsageEventTotals) {
    target.event_count = target.event_count.saturating_add(added.event_count);
    target.input_uncached_tokens = target
        .input_uncached_tokens
        .saturating_add(added.input_uncached_tokens);
    target.input_cached_tokens = target
        .input_cached_tokens
        .saturating_add(added.input_cached_tokens);
    target.output_tokens = target.output_tokens.saturating_add(added.output_tokens);
    target.billable_tokens = target.billable_tokens.saturating_add(added.billable_tokens);
}
