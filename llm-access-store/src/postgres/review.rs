//! Codex import-job loads + the `AdminReviewQueueStore` impl.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    AdminAccountContributionRequest, AdminAccountContributionRequestsPage, AdminAccountGroupStore,
    AdminCodexAccountStore, AdminCodexImportJobItem, AdminCodexImportJobSummary, AdminKeyPatch,
    AdminKeyStore, AdminReviewQueueAction, AdminReviewQueueQuery, AdminReviewQueueStore,
    AdminSponsorRequest, AdminSponsorRequestsPage, AdminTokenRequest, AdminTokenRequestsPage,
    NewAdminAccountGroup, NewAdminCodexAccount, NewAdminKey,
    PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED,
};

use super::{
    decode::{
        decode_admin_account_contribution_request_row, decode_admin_sponsor_request_row,
        decode_admin_token_request_row, decode_codex_import_job_item_row,
        decode_codex_import_job_summary_row,
    },
    PostgresControlRepository,
};

impl PostgresControlRepository {
    pub(super) async fn load_codex_import_job_summary_row(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Option<AdminCodexImportJobSummary>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT
                    job_id, provider_type, source_type, validate_before_import, status,
                    total_count, completed_count, succeeded_count, skipped_count, failed_count,
                    batch_error_message, created_at_ms, updated_at_ms, finished_at_ms
                 FROM llm_account_import_jobs
                 WHERE job_id = $1",
                &[&job_id],
            )
            .await
            .context("load postgres codex import job summary")?;
        Ok(row.map(decode_codex_import_job_summary_row))
    }

    pub(super) async fn load_codex_import_job_items_rows(
        &self,
        job_id: &str,
    ) -> anyhow::Result<Vec<AdminCodexImportJobItem>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    item_index, requested_name, requested_account_id, status,
                    error_message, imported_account_name, final_account_id,
                    validated_at_ms, imported_at_ms
                 FROM llm_account_import_job_items
                 WHERE job_id = $1
                 ORDER BY item_index",
                &[&job_id],
            )
            .await
            .context("load postgres codex import job items")?;
        Ok(rows
            .into_iter()
            .map(decode_codex_import_job_item_row)
            .collect())
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
        self.ensure_connection_alive()?;
        let rows_affected = self
            .client
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
        if rows_affected == 0 {
            return Ok(None);
        }
        self.get_admin_account_contribution_request_row(request_id)
            .await
    }

    async fn fail_admin_account_contribution_request(
        &self,
        request_id: &str,
        failure_reason: String,
        action: AdminReviewQueueAction,
    ) -> anyhow::Result<Option<AdminAccountContributionRequest>> {
        self.ensure_connection_alive()?;
        let rows_affected = self
            .client
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
        if rows_affected == 0 {
            return Ok(None);
        }
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
        self.ensure_connection_alive()?;
        let rows_affected = self
            .client
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
        if rows_affected == 0 {
            return Ok(None);
        }
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
