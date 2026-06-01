//! Codex accounts: account view, paged listing/summary/query types, sort
//! modes, in-memory query filtering, refresh targets, create/patch payloads,
//! and the import-job summary/item/detail types.

use serde::{Deserialize, Serialize};

use super::{KEY_STATUS_ACTIVE, KEY_STATUS_DISABLED};

/// Admin-facing Codex account summary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminCodexAccount {
    /// Account display name.
    pub name: String,
    /// Runtime status.
    pub status: String,
    /// Upstream account id.
    pub account_id: Option<String>,
    /// Upstream plan type, when known.
    pub plan_type: Option<String>,
    /// Manual routing tier override used for weighted auto routing.
    pub route_weight_tier: String,
    /// Primary rate-limit remaining percentage, when known.
    pub primary_remaining_percent: Option<f64>,
    /// Secondary rate-limit remaining percentage, when known.
    pub secondary_remaining_percent: Option<f64>,
    /// Whether GPT-5.3 Codex is mapped to Spark for this account.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auto_refresh_enabled: bool,
    /// Per-account request concurrency cap.
    pub request_max_concurrency: Option<u64>,
    /// Per-account request pacing interval.
    pub request_min_start_interval_ms: Option<u64>,
    /// Proxy selection mode.
    pub proxy_mode: String,
    /// Fixed proxy config id when proxy mode is fixed.
    pub proxy_config_id: Option<String>,
    /// Effective proxy source.
    pub effective_proxy_source: String,
    /// Effective proxy URL.
    pub effective_proxy_url: Option<String>,
    /// Effective proxy config name.
    pub effective_proxy_config_name: Option<String>,
    /// Last auth refresh timestamp.
    pub last_refresh: Option<i64>,
    /// Current access token expiry timestamp in Unix milliseconds.
    pub access_token_expires_at: Option<i64>,
    /// Last auth refresh error, if any.
    pub auth_refresh_error_message: Option<String>,
    /// Last usage refresh attempt timestamp.
    pub last_usage_checked_at: Option<i64>,
    /// Last successful usage refresh timestamp.
    pub last_usage_success_at: Option<i64>,
    /// Last usage refresh error.
    pub usage_error_message: Option<String>,
}

/// Page of admin Codex accounts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdminCodexAccountsPage {
    /// Page rows.
    pub accounts: Vec<AdminCodexAccount>,
    /// Full aggregate over all Codex accounts.
    pub summary: AdminAccountsSummary,
    /// Total rows matching the query before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Full aggregate for admin account inventories.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountsSummary {
    /// Total account rows.
    pub total: usize,
    /// Active account count.
    pub active_count: usize,
    /// Disabled account count.
    pub disabled_count: usize,
    /// Unavailable account count.
    pub unavailable_count: usize,
}

/// Admin Codex account list query shared by paginated inventory screens.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminCodexAccountPageQuery {
    /// Optional case-insensitive search query.
    pub search: Option<String>,
    /// Whether disabled rows should be excluded.
    pub active_only: bool,
    /// Whether only unhealthy rows should be returned.
    pub unhealthy_only: bool,
    /// Sort mode applied before pagination.
    pub sort: AdminCodexAccountSortMode,
}

/// Supported admin Codex account list sort modes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AdminCodexAccountSortMode {
    /// Default created-at descending order.
    #[default]
    Newest,
    /// Primary remaining percentage ascending.
    PrimaryAsc,
    /// Primary remaining percentage descending.
    PrimaryDesc,
    /// Secondary remaining percentage ascending.
    SecondaryAsc,
    /// Secondary remaining percentage descending.
    SecondaryDesc,
}

pub fn summarize_admin_accounts(accounts: &[AdminCodexAccount]) -> AdminAccountsSummary {
    let mut summary = AdminAccountsSummary::default();
    for account in accounts {
        summary.total += 1;
        match account.status.as_str() {
            KEY_STATUS_ACTIVE => summary.active_count += 1,
            KEY_STATUS_DISABLED => summary.disabled_count += 1,
            "unavailable" => summary.unavailable_count += 1,
            _ => {},
        }
    }
    summary
}

fn admin_codex_account_matches_query(
    account: &AdminCodexAccount,
    query: &AdminCodexAccountPageQuery,
) -> bool {
    if query.active_only && account.status == KEY_STATUS_DISABLED {
        return false;
    }
    if query.unhealthy_only
        && account.status != KEY_STATUS_DISABLED
        && account.auth_refresh_error_message.is_none()
        && account.usage_error_message.is_none()
    {
        return false;
    }
    let Some(search) = query
        .search
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return true;
    };
    let search = search.to_ascii_lowercase();
    account.name.to_ascii_lowercase().contains(&search)
        || account.status.to_ascii_lowercase().contains(&search)
        || account
            .plan_type
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(&search)
        || account
            .account_id
            .as_deref()
            .unwrap_or("")
            .to_ascii_lowercase()
            .contains(&search)
        || account
            .route_weight_tier
            .to_ascii_lowercase()
            .contains(&search)
}

fn codex_account_primary_pct(account: &AdminCodexAccount) -> f64 {
    account.primary_remaining_percent.unwrap_or(100.0)
}

fn codex_account_secondary_pct(account: &AdminCodexAccount) -> f64 {
    account.secondary_remaining_percent.unwrap_or(100.0)
}

pub fn apply_admin_codex_account_query(
    accounts: &mut Vec<AdminCodexAccount>,
    query: &AdminCodexAccountPageQuery,
) {
    accounts.retain(|account| admin_codex_account_matches_query(account, query));
    match query.sort {
        AdminCodexAccountSortMode::Newest => {},
        AdminCodexAccountSortMode::PrimaryAsc => accounts.sort_by(|a, b| {
            codex_account_primary_pct(a)
                .partial_cmp(&codex_account_primary_pct(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.name.cmp(&a.name))
        }),
        AdminCodexAccountSortMode::PrimaryDesc => accounts.sort_by(|a, b| {
            codex_account_primary_pct(b)
                .partial_cmp(&codex_account_primary_pct(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.name.cmp(&a.name))
        }),
        AdminCodexAccountSortMode::SecondaryAsc => accounts.sort_by(|a, b| {
            codex_account_secondary_pct(a)
                .partial_cmp(&codex_account_secondary_pct(b))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.name.cmp(&a.name))
        }),
        AdminCodexAccountSortMode::SecondaryDesc => accounts.sort_by(|a, b| {
            codex_account_secondary_pct(b)
                .partial_cmp(&codex_account_secondary_pct(a))
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.name.cmp(&a.name))
        }),
    }
}

/// Minimal Codex account projection used by background status refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexStatusRefreshTarget {
    /// Account display name.
    pub name: String,
    /// Runtime status.
    pub status: String,
}

/// New imported Codex account row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexAccount {
    /// Account display name.
    pub name: String,
    /// Upstream account id.
    pub account_id: Option<String>,
    /// Persisted auth JSON.
    pub auth_json: String,
    /// Whether GPT-5.3 Codex is mapped to Spark for this account.
    pub map_gpt53_codex_to_spark: bool,
    /// Whether this account may participate in automatic auth refresh.
    pub auto_refresh_enabled: bool,
    /// Manual routing tier override used for weighted auto routing.
    pub route_weight_tier: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one Codex account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminCodexAccountPatch {
    /// New runtime status.
    pub status: Option<String>,
    /// New GPT-5.3 Codex Spark mapping toggle.
    pub map_gpt53_codex_to_spark: Option<bool>,
    /// New automatic auth refresh toggle.
    pub auto_refresh_enabled: Option<bool>,
    /// New routing weight tier override.
    pub route_weight_tier: Option<String>,
    /// New proxy selection mode.
    pub proxy_mode: Option<String>,
    /// New proxy config id.
    pub proxy_config_id: Option<Option<String>>,
    /// New per-account request concurrency cap.
    pub request_max_concurrency: Option<Option<u64>>,
    /// New per-account request pacing interval.
    pub request_min_start_interval_ms: Option<Option<u64>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Admin-facing summary for one Codex batch import job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobSummary {
    /// Batch job id.
    pub job_id: String,
    /// Provider type.
    pub provider_type: String,
    /// Import source type.
    pub source_type: String,
    /// Whether refresh validation runs before import.
    pub validate_before_import: bool,
    /// Current batch status.
    pub status: String,
    /// Total queued item count.
    pub total_count: usize,
    /// Number of terminal items.
    pub completed_count: usize,
    /// Number of imported items.
    pub succeeded_count: usize,
    /// Number of skipped items.
    pub skipped_count: usize,
    /// Number of failed/conflict items.
    pub failed_count: usize,
    /// Batch-level failure reason when the worker aborts early.
    pub batch_error_message: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Last update timestamp.
    pub updated_at_ms: i64,
    /// Finish timestamp once terminal.
    pub finished_at_ms: Option<i64>,
}

/// Admin-facing result row for one Codex batch import item.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobItem {
    /// Zero-based item index within the batch.
    pub item_index: usize,
    /// Requested account name.
    pub requested_name: String,
    /// Requested upstream account id when present.
    pub requested_account_id: Option<String>,
    /// Current item status.
    pub status: String,
    /// Terminal error message when the item fails.
    pub error_message: Option<String>,
    /// Imported account name when successful.
    pub imported_account_name: Option<String>,
    /// Final upstream account id after validation/import.
    pub final_account_id: Option<String>,
    /// Validation timestamp when refresh validation succeeds.
    pub validated_at_ms: Option<i64>,
    /// Import timestamp when the account row is created.
    pub imported_at_ms: Option<i64>,
}

/// Full admin-facing detail for one Codex batch import job.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminCodexImportJobDetail {
    /// Job summary row.
    pub summary: AdminCodexImportJobSummary,
    /// Per-item states ordered by item index.
    pub items: Vec<AdminCodexImportJobItem>,
}

/// New batch import job persisted before background execution starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexImportJob {
    /// Batch job id.
    pub job_id: String,
    /// Provider type.
    pub provider_type: String,
    /// Import source type.
    pub source_type: String,
    /// Whether refresh validation runs before import.
    pub validate_before_import: bool,
    /// Submitted items.
    pub items: Vec<NewAdminCodexImportJobItem>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// One submitted item persisted as part of a new batch import job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminCodexImportJobItem {
    /// Requested account name.
    pub requested_name: String,
    /// Requested upstream account id when present.
    pub requested_account_id: Option<String>,
    /// Stored raw auth JSON for background processing.
    pub raw_auth_json: String,
}

/// Terminal update written after processing one batch import item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminCodexImportJobItemResult {
    /// Zero-based item index within the batch.
    pub item_index: usize,
    /// Terminal item status.
    pub status: String,
    /// Terminal error message when the item fails.
    pub error_message: Option<String>,
    /// Imported account name when successful.
    pub imported_account_name: Option<String>,
    /// Final upstream account id after validation/import.
    pub final_account_id: Option<String>,
    /// Validation timestamp when refresh validation succeeds.
    pub validated_at_ms: Option<i64>,
    /// Import timestamp when the account row is created.
    pub imported_at_ms: Option<i64>,
    /// Completed-item counter increment.
    pub completed_delta: usize,
    /// Imported-item counter increment.
    pub succeeded_delta: usize,
    /// Skipped-item counter increment.
    pub skipped_delta: usize,
    /// Failed/conflict-item counter increment.
    pub failed_delta: usize,
    /// Last update timestamp.
    pub updated_at_ms: i64,
}


#[cfg(test)]
mod tests {

    fn sample_admin_codex_account(
        name: &str,
        status: &str,
        plan_type: Option<&str>,
        primary_remaining_percent: Option<f64>,
        auth_refresh_error_message: Option<&str>,
    ) -> super::AdminCodexAccount {
        super::AdminCodexAccount {
            name: name.to_string(),
            status: status.to_string(),
            account_id: Some(format!("acct-{name}")),
            plan_type: plan_type.map(str::to_string),
            route_weight_tier: "auto".to_string(),
            primary_remaining_percent,
            secondary_remaining_percent: None,
            map_gpt53_codex_to_spark: false,
            auto_refresh_enabled: true,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            proxy_mode: "inherit".to_string(),
            proxy_config_id: None,
            effective_proxy_source: "binding".to_string(),
            effective_proxy_url: None,
            effective_proxy_config_name: None,
            last_refresh: None,
            access_token_expires_at: None,
            auth_refresh_error_message: auth_refresh_error_message.map(str::to_string),
            last_usage_checked_at: None,
            last_usage_success_at: None,
            usage_error_message: None,
        }
    }

    #[test]
    fn admin_codex_account_query_supports_search_sort_and_unhealthy_filters() {
        let mut accounts = vec![
            sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
            sample_admin_codex_account("codex-mid", "disabled", None, None, None),
            sample_admin_codex_account(
                "codex-old",
                "active",
                Some("Plus"),
                Some(20.0),
                Some("refresh failed"),
            ),
        ];

        super::apply_admin_codex_account_query(&mut accounts, &super::AdminCodexAccountPageQuery {
            sort: super::AdminCodexAccountSortMode::PrimaryAsc,
            ..super::AdminCodexAccountPageQuery::default()
        });
        assert_eq!(
            accounts
                .iter()
                .map(|account| account.name.as_str())
                .collect::<Vec<_>>(),
            ["codex-old", "codex-new", "codex-mid"]
        );

        let mut searched = vec![
            sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
            sample_admin_codex_account(
                "codex-old",
                "active",
                Some("Plus"),
                Some(20.0),
                Some("refresh failed"),
            ),
        ];
        super::apply_admin_codex_account_query(&mut searched, &super::AdminCodexAccountPageQuery {
            search: Some("plus".to_string()),
            ..super::AdminCodexAccountPageQuery::default()
        });
        assert_eq!(searched.len(), 1);
        assert_eq!(searched[0].name, "codex-old");

        let mut unhealthy = vec![
            sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
            sample_admin_codex_account("codex-mid", "disabled", None, None, None),
            sample_admin_codex_account(
                "codex-old",
                "active",
                Some("Plus"),
                Some(20.0),
                Some("refresh failed"),
            ),
        ];
        super::apply_admin_codex_account_query(
            &mut unhealthy,
            &super::AdminCodexAccountPageQuery {
                unhealthy_only: true,
                ..super::AdminCodexAccountPageQuery::default()
            },
        );
        assert_eq!(
            unhealthy
                .iter()
                .map(|account| account.name.as_str())
                .collect::<Vec<_>>(),
            ["codex-mid", "codex-old"]
        );
    }
}
