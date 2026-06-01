//! Public-facing access and community contracts: public access/usage-lookup
//! keys, account-contribution/sponsor views, public submission payloads, and
//! the admin-side review-queue request/page/query/action types.

use serde::{Deserialize, Serialize};

/// Public-safe key summary used by the unauthenticated access endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccessKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Plaintext public key secret.
    pub secret: String,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated billable tokens.
    pub usage_billable_tokens: u64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
}

/// Public usage lookup key and current rollup state.
#[derive(Debug, Clone, PartialEq)]
pub struct PublicUsageLookupKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type.
    pub provider_type: String,
    /// Key status.
    pub status: String,
    /// Whether this key is public-visible.
    pub public_visible: bool,
    /// Billable quota limit.
    pub quota_billable_limit: u64,
    /// Accumulated uncached input tokens.
    pub usage_input_uncached_tokens: u64,
    /// Accumulated cached input tokens.
    pub usage_input_cached_tokens: u64,
    /// Accumulated output tokens.
    pub usage_output_tokens: u64,
    /// Accumulated billable tokens.
    pub usage_billable_tokens: u64,
    /// Accumulated credit usage.
    pub usage_credit_total: f64,
    /// Number of events missing credit usage.
    pub usage_credit_missing_events: u64,
    /// Last usage timestamp.
    pub last_used_at_ms: Option<i64>,
}

/// Public thank-you card for an approved account contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicAccountContribution {
    /// Request id.
    pub request_id: String,
    /// Imported account display name.
    pub account_name: String,
    /// Contributor-supplied message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval/issuance timestamp.
    pub processed_at_ms: Option<i64>,
}

/// Public thank-you card for an approved sponsor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicSponsor {
    /// Request id.
    pub request_id: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Sponsor-supplied message.
    pub sponsor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Approval timestamp.
    pub processed_at_ms: Option<i64>,
}

/// New public token request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicTokenRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Requested billable quota.
    pub requested_quota_billable_limit: u64,
    /// Requester explanation.
    pub request_reason: String,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// New public Codex account contribution request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicAccountContributionRequest {
    /// Stable request id.
    pub request_id: String,
    /// Proposed account display name.
    pub account_name: String,
    /// Optional upstream account id.
    pub account_id: Option<String>,
    /// Upstream id token.
    pub id_token: String,
    /// Upstream access token.
    pub access_token: String,
    /// Upstream refresh token.
    pub refresh_token: String,
    /// Requester email address.
    pub requester_email: String,
    /// Contributor message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Whether this contribution should be shown on the public thank-you wall.
    pub show_on_public_wall: bool,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// New public sponsor request after input normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPublicSponsorRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Sponsor message.
    pub sponsor_message: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Normalized client fingerprint.
    pub fingerprint: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Creation timestamp.
    pub created_at_ms: i64,
}

/// Admin-facing projection of one token request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminTokenRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Requested billable quota.
    pub requested_quota_billable_limit: u64,
    /// Requester explanation.
    pub request_reason: String,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Issued key id when the request has produced a key.
    pub issued_key_id: Option<String>,
    /// Issued key name when the request has produced a key.
    pub issued_key_name: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for token requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminTokenRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminTokenRequest>,
}

/// Admin-facing projection of one account contribution request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountContributionRequest {
    /// Stable request id.
    pub request_id: String,
    /// Proposed account display name.
    pub account_name: String,
    /// Optional upstream account id.
    pub account_id: Option<String>,
    /// Upstream id token.
    pub id_token: String,
    /// Upstream access token.
    pub access_token: String,
    /// Upstream refresh token.
    pub refresh_token: String,
    /// Requester email address.
    pub requester_email: String,
    /// Contributor message.
    pub contributor_message: String,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Imported account name after approval.
    pub imported_account_name: Option<String>,
    /// Issued key id after approval.
    pub issued_key_id: Option<String>,
    /// Issued key name after approval.
    pub issued_key_name: Option<String>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for account contribution requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminAccountContributionRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminAccountContributionRequest>,
}

/// Admin-facing projection of one sponsor request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSponsorRequest {
    /// Stable request id.
    pub request_id: String,
    /// Requester email address.
    pub requester_email: String,
    /// Sponsor message.
    pub sponsor_message: String,
    /// Optional display name.
    pub display_name: Option<String>,
    /// Optional GitHub id.
    pub github_id: Option<String>,
    /// Optional frontend page URL.
    pub frontend_page_url: Option<String>,
    /// Request status.
    pub status: String,
    /// Normalized client IP.
    pub client_ip: String,
    /// Client IP region when known.
    pub ip_region: String,
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Optional failure reason.
    pub failure_reason: Option<String>,
    /// Payment email timestamp in Unix milliseconds.
    pub payment_email_sent_at: Option<i64>,
    /// Creation timestamp in Unix milliseconds.
    pub created_at: i64,
    /// Update timestamp in Unix milliseconds.
    pub updated_at: i64,
    /// Processing timestamp in Unix milliseconds.
    pub processed_at: Option<i64>,
}

/// Paginated admin response for sponsor requests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminSponsorRequestsPage {
    /// Total rows matching the filter.
    pub total: usize,
    /// Page offset.
    pub offset: usize,
    /// Page limit.
    pub limit: usize,
    /// Whether a later page exists.
    pub has_more: bool,
    /// Current page rows.
    pub requests: Vec<AdminSponsorRequest>,
}

/// Normalized admin review-queue pagination query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminReviewQueueQuery {
    /// Optional status filter.
    pub status: Option<String>,
    /// Page limit.
    pub limit: usize,
    /// Page offset.
    pub offset: usize,
}

/// Admin action metadata applied to one review queue item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdminReviewQueueAction {
    /// Optional admin note.
    pub admin_note: Option<String>,
    /// Update timestamp in Unix milliseconds.
    pub updated_at_ms: i64,
}

impl PublicAccessKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        let limit = i64::try_from(self.quota_billable_limit).unwrap_or(i64::MAX);
        let used = i64::try_from(self.usage_billable_tokens).unwrap_or(i64::MAX);
        limit.saturating_sub(used)
    }
}

impl PublicUsageLookupKey {
    /// Remaining billable token budget available to this key.
    pub fn remaining_billable(&self) -> i64 {
        let limit = i64::try_from(self.quota_billable_limit).unwrap_or(i64::MAX);
        let used = i64::try_from(self.usage_billable_tokens).unwrap_or(i64::MAX);
        limit.saturating_sub(used)
    }
}
