//! Admin account groups: group view, paged listing, option view, and
//! create/patch payloads.

use serde::{Deserialize, Serialize};

/// Admin-facing projection of one reusable account group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroup {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Account names included in the group.
    pub account_names: Vec<String>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Update timestamp.
    pub updated_at: i64,
}

/// Page of admin account groups.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroupsPage {
    /// Page rows.
    pub groups: Vec<AdminAccountGroup>,
    /// Total rows matching the provider before pagination.
    pub total: usize,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
    /// Whether another page is available.
    pub has_more: bool,
}

/// Lightweight reusable account-group projection for routing selectors.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountGroupOption {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Total accounts in this group.
    pub account_count: usize,
    /// The lone account name when this group contains exactly one account.
    pub single_account_name: Option<String>,
}

/// New reusable account group row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAdminAccountGroup {
    /// Group id.
    pub id: String,
    /// Provider type.
    pub provider_type: String,
    /// Human-readable group name.
    pub name: String,
    /// Account names included in the group.
    pub account_names: Vec<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Patch for one reusable account group.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdminAccountGroupPatch {
    /// New group name.
    pub name: Option<String>,
    /// Replacement account list.
    pub account_names: Option<Vec<String>>,
    /// Update timestamp.
    pub updated_at_ms: i64,
}
