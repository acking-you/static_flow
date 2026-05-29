//! Public-facing storage: public access/usage keys, account-contribution and
//! sponsor rows, and admin review-queue request rows, plus the
//! `PublicSubmissionStore`, `PublicAccessStore`, `PublicCommunityStore`,
//! `PublicUsageStore`, and `AdminReviewQueueStore` impls.

use async_trait::async_trait;

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

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
#[async_trait]
impl AdminReviewQueueStore for PostgresControlRepository {
    async fn get_admin_token_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        self.get_admin_token_request_row(request_id).await
    }

    async fn list_admin_token_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminTokenRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_token_requests",
                "SELECT COUNT(*) FROM llm_token_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminTokenRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin token requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, requested_quota_billable_limit,
                        request_reason, frontend_page_url, status, client_ip, ip_region,
                        admin_note, failure_reason, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_token_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin token requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_token_request_row)
            .collect::<Vec<_>>();
        Ok(AdminTokenRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn get_admin_account_contribution_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn list_admin_account_contribution_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminAccountContributionRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_account_contribution_requests",
                "SELECT COUNT(*) FROM llm_account_contribution_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminAccountContributionRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin account contribution requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, account_name, account_id, id_token, access_token,
                        refresh_token, requester_email, contributor_message, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, imported_account_name, issued_key_id, issued_key_name,
                        created_at_ms, updated_at_ms, processed_at_ms
                     FROM llm_account_contribution_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin account contribution requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_account_contribution_request_row)
            .collect::<Vec<_>>();
        Ok(AdminAccountContributionRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn get_admin_sponsor_request(
        &self,
        request_id: &str,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        self.get_admin_sponsor_request_row(request_id).await
    }

    async fn list_admin_sponsor_requests(
        &self,
        query: AdminReviewQueueQuery,
    ) -> anyhow::Result<AdminSponsorRequestsPage> {
        let total = self
            .count_rows(
                "SELECT COUNT(*) FROM llm_sponsor_requests",
                "SELECT COUNT(*) FROM llm_sponsor_requests WHERE status = $1",
                query.status.as_deref(),
            )
            .await?;
        if total == 0 || query.offset >= total {
            return Ok(AdminSponsorRequestsPage {
                total,
                offset: query.offset,
                limit: query.limit,
                has_more: false,
                requests: Vec::new(),
            });
        }
        let rows = if let Some(status) = query.status.as_deref() {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     WHERE status = $1
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $2 OFFSET $3",
                    &[&status, &(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin sponsor requests by status")?
        } else {
            self.client
                .query(
                    "SELECT
                        request_id, requester_email, sponsor_message, display_name, github_id,
                        frontend_page_url, status, client_ip, ip_region, admin_note,
                        failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
                        processed_at_ms
                     FROM llm_sponsor_requests
                     ORDER BY created_at_ms DESC, request_id DESC
                     LIMIT $1 OFFSET $2",
                    &[&(query.limit as i64), &(query.offset as i64)],
                )
                .await
                .context("list admin sponsor requests")?
        };
        let requests = rows
            .into_iter()
            .map(decode_admin_sponsor_request_row)
            .collect::<Vec<_>>();
        Ok(AdminSponsorRequestsPage {
            total,
            offset: query.offset,
            limit: query.limit,
            has_more: query.offset.saturating_add(requests.len()) < total,
            requests,
        })
    }

    async fn issue_admin_token_request(
        &self,
        request_id: &str,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request_row(request_id).await? else {
            return Ok(None);
        };
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                let created = self.create_admin_key(key).await?;
                (Some(created.id), Some(created.name))
            },
            (None, None) => (None, None),
        };
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'issued',
                     admin_note = $2,
                     failure_reason = NULL,
                     issued_key_id = $3,
                     issued_key_name = $4,
                     updated_at_ms = $5,
                     processed_at_ms = $5
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &action.admin_note,
                    &issued_key_id,
                    &issued_key_name,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("issue postgres admin token request")?;
        self.get_admin_token_request_row(request_id).await
    }

    async fn reject_admin_token_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminTokenRequest>> {
        let Some(current) = self.get_admin_token_request_row(request_id).await? else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)
                .await?;
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_token_requests
                 SET status = 'rejected',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("reject postgres admin token request")?;
        self.get_admin_token_request_row(request_id).await
    }

    async fn issue_admin_account_contribution_request(
        &self,
        request_id: &str,
        account: Option<NewAdminCodexAccount>,
        account_group: Option<NewAdminAccountGroup>,
        key: Option<NewAdminKey>,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self
            .get_admin_account_contribution_request_row(request_id)
            .await?
        else {
            return Ok(None);
        };
        let imported_account_name = match (current.imported_account_name, account) {
            (Some(name), _) => Some(name),
            (None, Some(account)) => {
                let created = self.create_admin_codex_account(account).await?;
                Some(created.name)
            },
            (None, None) => None,
        };
        if let Some(group) = account_group.clone() {
            self.create_admin_account_group(group).await?;
        }
        let (issued_key_id, issued_key_name) = match (current.issued_key_id, key) {
            (Some(id), _) => (Some(id), current.issued_key_name),
            (None, Some(key)) => {
                let created = self.create_admin_key(key).await?;
                if let Some(group) = account_group {
                    self.patch_admin_key(&created.id, AdminKeyPatch {
                        route_strategy: Some(Some("fixed".to_string())),
                        account_group_id: Some(Some(group.id.clone())),
                        updated_at_ms: action.updated_at_ms,
                        ..AdminKeyPatch::default()
                    })
                    .await?;
                }
                (Some(created.id), Some(created.name))
            },
            (None, None) => (None, None),
        };
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'issued',
                     admin_note = $2,
                     failure_reason = NULL,
                     imported_account_name = $3,
                     issued_key_id = $4,
                     issued_key_name = $5,
                     updated_at_ms = $6,
                     processed_at_ms = $6
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &action.admin_note,
                    &imported_account_name,
                    &issued_key_id,
                    &issued_key_name,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("issue postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn validate_admin_account_contribution_request(
        &self,
        request_id: &str,
        account_id: Option<String>,
        id_token: String,
        access_token: String,
        refresh_token: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        if self
            .get_admin_account_contribution_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = $2,
                     account_id = $3,
                     id_token = $4,
                     access_token = $5,
                     refresh_token = $6,
                     admin_note = $7,
                     failure_reason = NULL,
                     updated_at_ms = $8,
                     processed_at_ms = NULL
                 WHERE request_id = $1",
                &[
                    &request_id,
                    &PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
                    &account_id,
                    &id_token,
                    &access_token,
                    &refresh_token,
                    &action.admin_note,
                    &action.updated_at_ms,
                ],
            )
            .await
            .context("validate postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn fail_admin_account_contribution_request(
        &self,
        request_id: &str,
        failure_reason: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        if self
            .get_admin_account_contribution_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'failed',
                     admin_note = $2,
                     failure_reason = $3,
                     updated_at_ms = $4,
                     processed_at_ms = NULL
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &failure_reason, &action.updated_at_ms],
            )
            .await
            .context("fail postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn reject_admin_account_contribution_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        let Some(current) = self
            .get_admin_account_contribution_request_row(request_id)
            .await?
        else {
            return Ok(None);
        };
        if let Some(key_id) = current.issued_key_id.as_deref() {
            self.disable_admin_key_if_present(key_id, action.updated_at_ms)
                .await?;
        }
        if let Some(account_name) = current.imported_account_name.as_deref() {
            self.delete_admin_codex_account(account_name).await?;
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_contribution_requests
                 SET status = 'rejected',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("reject postgres admin account contribution request")?;
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn approve_admin_sponsor_request(
        &self,
        request_id: &str,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminSponsorRequest>> {
        if self
            .get_admin_sponsor_request_row(request_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_sponsor_requests
                 SET status = 'approved',
                     admin_note = $2,
                     failure_reason = NULL,
                     updated_at_ms = $3,
                     processed_at_ms = $3
                 WHERE request_id = $1",
                &[&request_id, &action.admin_note, &action.updated_at_ms],
            )
            .await
            .context("approve postgres sponsor request")?;
        self.get_admin_sponsor_request_row(request_id).await
    }

    async fn delete_admin_sponsor_request(&self, request_id: &str) -> anyhow::Result<bool> {
        self.ensure_connection_alive()?;
        let changed = self
            .client
            .execute("DELETE FROM llm_sponsor_requests WHERE request_id = $1", &[&request_id])
            .await
            .context("delete postgres sponsor request")?;
        Ok(changed > 0)
    }
}
impl PostgresControlRepository {
    pub(crate) async fn list_public_access_keys_rows(
        &self,
    ) -> anyhow::Result<Vec<PublicAccessKey>> {
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
    pub(crate) async fn load_public_usage_key_by_hash(
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
    pub(crate) async fn list_public_account_contributions_rows(
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
    pub(crate) async fn list_public_sponsors_rows(
        &self,
        limit: usize,
    ) -> anyhow::Result<Vec<PublicSponsor>> {
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
    pub(crate) async fn get_admin_token_request_row(
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
    pub(crate) async fn get_admin_account_contribution_request_row(
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
    pub(crate) async fn get_admin_sponsor_request_row(
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
