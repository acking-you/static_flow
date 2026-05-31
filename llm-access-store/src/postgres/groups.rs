//! Account-group row reads + the `AdminAccountGroupStore` impl.

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    self as core_store, AdminAccountGroup, AdminAccountGroupOption, AdminAccountGroupPatch,
    AdminAccountGroupStore, AdminPageRequest, NewAdminAccountGroup,
};

use super::{decode::decode_admin_account_group_row, PostgresControlRepository};

impl PostgresControlRepository {
    async fn list_admin_account_groups_rows(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY created_at_ms DESC, group_id DESC",
                &[&provider_type],
            )
            .await
            .context("list admin account groups")?;
        rows.into_iter()
            .map(decode_admin_account_group_row)
            .collect::<anyhow::Result<Vec<_>>>()
    }

    async fn list_admin_account_groups_page_rows(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<core_store::AdminAccountGroupsPage> {
        self.ensure_connection_alive()?;
        let total_row = self
            .client
            .query_one(
                "SELECT COUNT(*)::bigint
                 FROM llm_account_groups
                 WHERE provider_type = $1",
                &[&provider_type],
            )
            .await
            .context("count admin account groups")?;
        let total_i64 = total_row.get::<_, i64>(0);
        let total = usize::try_from(total_i64)
            .with_context(|| format!("admin account groups total out of range: {total_i64}"))?;
        let rows = self
            .client
            .query(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY created_at_ms DESC, group_id DESC
                 LIMIT $2 OFFSET $3",
                &[&provider_type, &(page.limit.max(1) as i64), &(page.offset as i64)],
            )
            .await
            .context("list admin account groups page")?;
        let groups = rows
            .into_iter()
            .map(decode_admin_account_group_row)
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(core_store::AdminAccountGroupsPage {
            has_more: page.has_more(groups.len(), total),
            groups,
            total,
            limit: page.limit,
            offset: page.offset,
        })
    }

    pub(super) async fn get_admin_account_group_row(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT group_id, provider_type, name, account_names_json::text,
                    created_at_ms, updated_at_ms
                 FROM llm_account_groups
                 WHERE group_id = $1",
                &[&group_id],
            )
            .await
            .context("load admin account group")?;
        row.map(decode_admin_account_group_row).transpose()
    }
}
#[async_trait]
impl AdminAccountGroupStore for PostgresControlRepository {
    async fn list_admin_account_groups(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroup>> {
        self.list_admin_account_groups_rows(provider_type).await
    }

    async fn list_admin_account_groups_page(
        &self,
        provider_type: &str,
        page: AdminPageRequest,
    ) -> anyhow::Result<core_store::AdminAccountGroupsPage> {
        self.list_admin_account_groups_page_rows(provider_type, page)
            .await
    }

    async fn list_admin_account_group_options(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<Vec<AdminAccountGroupOption>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT
                    group_id,
                    provider_type,
                    name,
                    CASE
                        WHEN jsonb_typeof(account_names_json) = 'array'
                        THEN jsonb_array_length(account_names_json)
                        ELSE 0
                    END,
                    CASE
                        WHEN jsonb_typeof(account_names_json) = 'array'
                             AND jsonb_array_length(account_names_json) = 1
                        THEN account_names_json ->> 0
                        ELSE NULL
                    END
                 FROM llm_account_groups
                 WHERE provider_type = $1
                 ORDER BY lower(name), group_id",
                &[&provider_type],
            )
            .await
            .context("list postgres admin account group options")?;
        Ok(rows
            .into_iter()
            .map(|row| AdminAccountGroupOption {
                id: row.get(0),
                provider_type: row.get(1),
                name: row.get(2),
                account_count: row.get::<_, i32>(3).max(0) as usize,
                single_account_name: row.get(4),
            })
            .collect())
    }

    async fn create_admin_account_group(
        &self,
        group: NewAdminAccountGroup,
    ) -> anyhow::Result<AdminAccountGroup> {
        let account_names_json =
            serde_json::to_string(&group.account_names).context("serialize account group names")?;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_account_groups (
                    group_id, provider_type, name, account_names_json, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4::jsonb, $5, $6)",
                &[
                    &group.id,
                    &group.provider_type,
                    &group.name,
                    &account_names_json,
                    &group.created_at_ms,
                    &group.created_at_ms,
                ],
            )
            .await
            .context("create postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        self.get_admin_account_group_row(&group.id)
            .await?
            .context("created postgres account group disappeared")
    }

    async fn patch_admin_account_group(
        &self,
        group_id: &str,
        patch: AdminAccountGroupPatch,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(mut group) = self.get_admin_account_group_row(group_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            group.name = name.clone();
        }
        if let Some(account_names) = patch.account_names.as_ref() {
            group.account_names = account_names.clone();
        }
        group.updated_at = patch.updated_at_ms;
        let account_names_json =
            serde_json::to_string(&group.account_names).context("serialize account group names")?;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_account_groups
                 SET name = $2, account_names_json = $3::jsonb, updated_at_ms = $4
                 WHERE group_id = $1",
                &[&group_id, &group.name, &account_names_json, &group.updated_at],
            )
            .await
            .context("patch postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        Ok(Some(group))
    }

    async fn delete_admin_account_group(
        &self,
        group_id: &str,
    ) -> anyhow::Result<Option<AdminAccountGroup>> {
        let Some(group) = self.get_admin_account_group_row(group_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_account_groups WHERE group_id = $1", &[&group_id])
            .await
            .context("delete postgres account group")?;
        self.bump_dispatch_generation(&group.provider_type).await;
        Ok(Some(group))
    }
}
