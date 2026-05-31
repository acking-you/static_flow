//! Public access/community/usage/submission reads + admin request-row
//! loads, and the `Public*Store`/submission trait impls.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    AdminAccountContributionRequest, AdminSponsorRequest, AdminTokenRequest,
    NewPublicAccountContributionRequest, NewPublicSponsorRequest, NewPublicTokenRequest,
    PublicAccessKey, PublicAccessStore, PublicAccountContribution, PublicCommunityStore,
    PublicSponsor, PublicSubmissionStore, PublicUsageLookupKey, PublicUsageStore,
    DEFAULT_AUTH_CACHE_TTL_SECONDS, PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
    PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT, PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
    PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
};

use super::{
    decode::{
        decode_admin_account_contribution_request_row, decode_admin_sponsor_request_row,
        decode_admin_token_request_row, decode_public_usage_lookup_row,
    },
    hash_bearer_secret, now_ms, PostgresControlRepository,
};

impl PostgresControlRepository {
    async fn list_public_access_keys_rows(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    k.key_id,
                    k.name,
                    k.secret,
                    k.quota_billable_limit,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    u.last_used_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.status = 'active' AND k.public_visible = TRUE
                 ORDER BY lower(k.name)",
                &[],
            )
            .await
            .context("list public access keys")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicAccessKey {
                key_id: row.get(0),
                key_name: row.get(1),
                secret: row.get(2),
                quota_billable_limit: row.get::<_, i64>(3).max(0) as u64,
                usage_input_uncached_tokens: row.get::<_, i64>(4).max(0) as u64,
                usage_input_cached_tokens: row.get::<_, i64>(5).max(0) as u64,
                usage_output_tokens: row.get::<_, i64>(6).max(0) as u64,
                usage_billable_tokens: row.get::<_, i64>(7).max(0) as u64,
                last_used_at_ms: row.get(8),
            })
            .collect())
    }

    async fn load_public_usage_key_by_hash(
        &self,
        key_hash: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    k.key_id,
                    k.name,
                    k.provider_type,
                    k.status,
                    k.public_visible,
                    k.quota_billable_limit,
                    COALESCE(u.input_uncached_tokens, 0),
                    COALESCE(u.input_cached_tokens, 0),
                    COALESCE(u.output_tokens, 0),
                    COALESCE(u.billable_tokens, 0),
                    COALESCE(u.credit_total, '0'),
                    COALESCE(u.credit_missing_events, 0),
                    u.last_used_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_hash = $1",
                &[&key_hash],
            )
            .await
            .context("load public usage key by hash")?;
        row.map(decode_public_usage_lookup_row).transpose()
    }

    async fn list_public_account_contributions_rows(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    request_id,
                    COALESCE(imported_account_name, account_name),
                    contributor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE status = 'issued'
                   AND show_on_public_wall = TRUE
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT $1",
                &[&(limit.max(1) as i64)],
            )
            .await
            .context("list public account contributions")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicAccountContribution {
                request_id: row.get(0),
                account_name: row.get(1),
                contributor_message: row.get(2),
                github_id: row.get(3),
                processed_at_ms: row.get(4),
            })
            .collect())
    }

    async fn list_public_sponsors_rows(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    request_id,
                    display_name,
                    sponsor_message,
                    github_id,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE status = 'approved'
                 ORDER BY COALESCE(processed_at_ms, created_at_ms) DESC
                 LIMIT $1",
                &[&(limit.max(1) as i64)],
            )
            .await
            .context("list public sponsors")?;
        Ok(rows
            .into_iter()
            .map(|row| PublicSponsor {
                request_id: row.get(0),
                display_name: row.get(1),
                sponsor_message: row.get(2),
                github_id: row.get(3),
                processed_at_ms: row.get(4),
            })
            .collect())
    }

    pub(super) async fn get_admin_token_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, requester_email, requested_quota_billable_limit,
                    request_reason, frontend_page_url, status, client_ip, ip_region,
                    admin_note, failure_reason, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_token_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin token request")?;
        Ok(row.map(decode_admin_token_request_row))
    }

    pub(super) async fn get_admin_account_contribution_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, account_name, account_id, id_token, access_token,
                    refresh_token, requester_email, contributor_message, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, imported_account_name, issued_key_id, issued_key_name,
                    created_at_ms, updated_at_ms, processed_at_ms
                 FROM llm_account_contribution_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin account contribution request")?;
        Ok(row.map(decode_admin_account_contribution_request_row))
    }

    pub(super) async fn get_admin_sponsor_request_row(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                 FROM llm_sponsor_requests
                 WHERE request_id = $1",
                &[&request_id],
            )
            .await
            .context("load admin sponsor request")?;
        Ok(row.map(decode_admin_sponsor_request_row))
    }
}
#[async_trait]
impl PublicSubmissionStore for PostgresControlRepository {
    async fn create_public_token_request(
        &self,
        request: NewPublicTokenRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_token_requests (
                    request_id, requester_email, requested_quota_billable_limit, request_reason,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, issued_key_id, issued_key_name, created_at_ms,
                    updated_at_ms, processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, NULL, NULL, NULL, NULL, $10, $10, NULL
                )",
                &[
                    &request.request_id,
                    &request.requester_email,
                    &(request.requested_quota_billable_limit as i64),
                    &request.request_reason,
                    &request.frontend_page_url,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public token request")?;
        Ok(())
    }

    async fn create_public_account_contribution_request(
        &self,
        request: NewPublicAccountContributionRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_account_contribution_requests (
                    request_id, account_name, account_id, id_token, access_token, refresh_token,
                    requester_email, contributor_message, github_id, frontend_page_url,
                    show_on_public_wall, status, fingerprint, client_ip, ip_region,
                    admin_note, failure_reason, imported_account_name, issued_key_id,
                    issued_key_name, created_at_ms, updated_at_ms, processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14,
                    $15, NULL, NULL, NULL, NULL, NULL, $16, $16, NULL
                )",
                &[
                    &request.request_id,
                    &request.account_name,
                    &request.account_id,
                    &request.id_token,
                    &request.access_token,
                    &request.refresh_token,
                    &request.requester_email,
                    &request.contributor_message,
                    &request.github_id,
                    &request.frontend_page_url,
                    &request.show_on_public_wall,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public account contribution request")?;
        Ok(())
    }

    async fn public_account_contribution_name_exists(
        &self,
        account_name: &str,
    ) -> anyhow::Result<bool> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_one(
                "SELECT EXISTS(
                    SELECT 1 FROM llm_codex_accounts WHERE account_name = $1
                    UNION ALL
                    SELECT 1 FROM llm_account_contribution_requests
                     WHERE account_name = $1
                       AND status IN ($2, $3, 'issued')
                )",
                &[
                    &account_name,
                    &PUBLIC_TOKEN_REQUEST_STATUS_PENDING,
                    &PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
                ],
            )
            .await
            .context("check postgres public account contribution name")?;
        Ok(row.get(0))
    }

    async fn create_public_sponsor_request(
        &self,
        request: NewPublicSponsorRequest,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_sponsor_requests (
                    request_id, requester_email, sponsor_message, display_name, github_id,
                    frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
                    failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                    processed_at_ms
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, NULL, NULL, NULL, $11, $11, NULL
                )",
                &[
                    &request.request_id,
                    &request.requester_email,
                    &request.sponsor_message,
                    &request.display_name,
                    &request.github_id,
                    &request.frontend_page_url,
                    &PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED,
                    &request.fingerprint,
                    &request.client_ip,
                    &request.ip_region,
                    &request.created_at_ms,
                ],
            )
            .await
            .context("create postgres public sponsor request")?;
        Ok(())
    }

    async fn record_public_sponsor_payment_email_result(
        &self,
        request_id: &str,
        sent_at_ms: Option<i64>,
        failure_reason: Option<String>,
    ) -> anyhow::Result<()> {
        self.ensure_connection_alive()?;
        let status = if sent_at_ms.is_some() {
            PUBLIC_SPONSOR_REQUEST_STATUS_PAYMENT_EMAIL_SENT
        } else {
            PUBLIC_SPONSOR_REQUEST_STATUS_SUBMITTED
        };
        let updated_at_ms = sent_at_ms.unwrap_or_else(now_ms);
        self.client
            .execute(
                "UPDATE llm_sponsor_requests
                 SET status = $2,
                     failure_reason = $3,
                     payment_email_sent_at_ms = $4,
                     updated_at_ms = $5
                 WHERE request_id = $1",
                &[&request_id, &status, &failure_reason, &sent_at_ms, &updated_at_ms],
            )
            .await
            .context("record postgres sponsor payment email result")?;
        Ok(())
    }
}
#[async_trait]
impl PublicAccessStore for PostgresControlRepository {
    async fn auth_cache_ttl_seconds(&self) -> anyhow::Result<u64> {
        Ok(self
            .load_runtime_config_record_cached()
            .await?
            .map_or(DEFAULT_AUTH_CACHE_TTL_SECONDS, |record| {
                record.auth_cache_ttl_seconds.max(0) as u64
            }))
    }

    async fn list_public_access_keys(&self) -> anyhow::Result<Vec<PublicAccessKey>> {
        self.list_public_access_keys_rows().await
    }
}
#[async_trait]
impl PublicCommunityStore for PostgresControlRepository {
    async fn list_public_account_contributions(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicAccountContribution>> {
        self.list_public_account_contributions_rows(limit).await
    }

    async fn list_public_sponsors(&self, limit: usize) -> anyhow::Result<Vec<PublicSponsor>> {
        self.list_public_sponsors_rows(limit).await
    }
}
#[async_trait]
impl PublicUsageStore for PostgresControlRepository {
    async fn get_public_usage_key_by_secret(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<PublicUsageLookupKey>> {
        self.load_public_usage_key_by_hash(&hash_bearer_secret(secret))
            .await
    }
}
