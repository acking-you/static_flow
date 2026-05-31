//! Proxy-config base/node-override/endpoint-check reads, binding
//! resolution, and the `AdminProxyStore` impl.

use std::collections::BTreeMap;

use anyhow::Context;
use async_trait::async_trait;
use llm_access_core::store::{
    self as core_store, default_proxy_bindings, AdminLegacyKiroProxyMigration, AdminProxyBinding,
    AdminProxyConfig, AdminProxyConfigPatch, AdminProxyEndpointCheckUpdate, AdminProxyStore,
    NewAdminProxyConfig,
};

use super::{
    decode::{decode_admin_proxy_config_row, decode_proxy_endpoint_check_row},
    now_ms,
    proxy_support::{
        apply_proxy_config_node_override, apply_proxy_endpoint_checks,
        clear_legacy_kiro_proxy_json, legacy_proxy_json_string,
    },
    PostgresControlRepository, ProviderProxyResolutionContext, ProxyConfigNodeOverride,
    ProxyEndpointCheckRow,
};
use crate::records::KiroAccountRecord;

impl PostgresControlRepository {
    async fn list_admin_proxy_config_base_rows(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 ORDER BY created_at_ms DESC, proxy_config_id DESC",
                &[],
            )
            .await
            .context("list admin proxy configs")?;
        Ok(rows
            .into_iter()
            .map(decode_admin_proxy_config_row)
            .collect())
    }

    pub(super) async fn list_admin_proxy_configs_rows(
        &self,
    ) -> anyhow::Result<Vec<AdminProxyConfig>> {
        let mut proxies = self.list_admin_proxy_config_base_rows().await?;
        if self.proxy_scope.can_edit_slot_metadata() {
            for proxy in &mut proxies {
                self.apply_proxy_scope_metadata(proxy, "core", false);
            }
            self.apply_proxy_endpoint_checks_to_configs(&mut proxies)
                .await?;
            return Ok(proxies);
        }
        let overrides = self.list_proxy_config_node_overrides().await?;
        for proxy in &mut proxies {
            if let Some(override_row) = overrides.get(&proxy.id) {
                apply_proxy_config_node_override(proxy, override_row);
                self.apply_proxy_scope_metadata(proxy, "node_override", true);
            } else {
                self.apply_proxy_scope_metadata(proxy, "core", false);
            }
        }
        self.apply_proxy_endpoint_checks_to_configs(&mut proxies)
            .await?;
        Ok(proxies)
    }

    async fn get_admin_proxy_config_base_row(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_configs
                 WHERE proxy_config_id = $1",
                &[&proxy_id],
            )
            .await
            .context("load admin proxy config")?;
        Ok(row.map(decode_admin_proxy_config_row))
    }

    async fn get_admin_proxy_config_row(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(mut proxy) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if self.proxy_scope.can_edit_slot_metadata() {
            self.apply_proxy_scope_metadata(&mut proxy, "core", false);
            self.apply_proxy_endpoint_checks_to_config(&mut proxy)
                .await?;
            return Ok(Some(proxy));
        }
        match self.get_proxy_config_node_override(proxy_id).await? {
            Some(override_row) => {
                apply_proxy_config_node_override(&mut proxy, &override_row);
                self.apply_proxy_scope_metadata(&mut proxy, "node_override", true);
            },
            None => self.apply_proxy_scope_metadata(&mut proxy, "core", false),
        }
        self.apply_proxy_endpoint_checks_to_config(&mut proxy)
            .await?;
        Ok(Some(proxy))
    }

    async fn list_proxy_endpoint_check_rows(&self) -> anyhow::Result<Vec<ProxyEndpointCheckRow>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, provider_type, target_url, reachable, status_code,
                    latency_ms, error_message, checked_at_ms
                 FROM llm_proxy_config_endpoint_checks
                 WHERE node_id = $1",
                &[&self.proxy_scope.node_id],
            )
            .await
            .context("list postgres proxy endpoint checks")?;
        Ok(rows
            .into_iter()
            .map(decode_proxy_endpoint_check_row)
            .collect())
    }

    async fn apply_proxy_endpoint_checks_to_configs(
        &self,
        proxies: &mut [AdminProxyConfig],
    ) -> anyhow::Result<()> {
        let mut by_proxy_id = BTreeMap::<String, Vec<ProxyEndpointCheckRow>>::new();
        for row in self.list_proxy_endpoint_check_rows().await? {
            by_proxy_id
                .entry(row.proxy_config_id.clone())
                .or_default()
                .push(row);
        }
        for proxy in proxies {
            if let Some(rows) = by_proxy_id.get(&proxy.id) {
                apply_proxy_endpoint_checks(proxy, rows);
            }
        }
        Ok(())
    }

    async fn apply_proxy_endpoint_checks_to_config(
        &self,
        proxy: &mut AdminProxyConfig,
    ) -> anyhow::Result<()> {
        let rows = self
            .list_proxy_endpoint_check_rows()
            .await?
            .into_iter()
            .filter(|row| row.proxy_config_id == proxy.id)
            .collect::<Vec<_>>();
        apply_proxy_endpoint_checks(proxy, &rows);
        Ok(())
    }

    async fn list_proxy_config_node_overrides(
        &self,
    ) -> anyhow::Result<BTreeMap<String, ProxyConfigNodeOverride>> {
        self.ensure_connection_alive()?;
        let rows = self
            .client
            .query(
                "SELECT proxy_config_id, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_config_node_overrides
                 WHERE node_id = $1",
                &[&self.proxy_scope.node_id],
            )
            .await
            .context("list postgres proxy config node overrides")?;
        Ok(rows
            .into_iter()
            .map(|row| {
                (row.get::<_, String>(0), ProxyConfigNodeOverride {
                    proxy_url: row.get(1),
                    proxy_username: row.get(2),
                    proxy_password: row.get(3),
                    status: row.get(4),
                    created_at_ms: row.get(5),
                    updated_at_ms: row.get(6),
                })
            })
            .collect())
    }

    async fn get_proxy_config_node_override(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<ProxyConfigNodeOverride>> {
        self.ensure_connection_alive()?;
        let row = self
            .client
            .query_opt(
                "SELECT proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 FROM llm_proxy_config_node_overrides
                 WHERE proxy_config_id = $1 AND node_id = $2",
                &[&proxy_id, &self.proxy_scope.node_id],
            )
            .await
            .context("load postgres proxy config node override")?;
        Ok(row.map(|row| ProxyConfigNodeOverride {
            proxy_url: row.get(0),
            proxy_username: row.get(1),
            proxy_password: row.get(2),
            status: row.get(3),
            created_at_ms: row.get(4),
            updated_at_ms: row.get(5),
        }))
    }

    async fn patch_admin_proxy_config_node_override(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if patch.name.is_some() {
            anyhow::bail!("proxy slot names can only be changed on the core node");
        }
        let Some(base) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        let existing = self.get_proxy_config_node_override(proxy_id).await?;
        let proxy_url = patch
            .proxy_url
            .or_else(|| existing.as_ref().map(|row| row.proxy_url.clone()))
            .unwrap_or(base.proxy_url);
        let proxy_username = match patch.proxy_username {
            Some(value) => value,
            None => existing
                .as_ref()
                .map(|row| row.proxy_username.clone())
                .unwrap_or(base.proxy_username),
        };
        let proxy_password = match patch.proxy_password {
            Some(value) => value,
            None => existing
                .as_ref()
                .map(|row| row.proxy_password.clone())
                .unwrap_or(base.proxy_password),
        };
        let status = patch
            .status
            .or_else(|| existing.as_ref().map(|row| row.status.clone()))
            .unwrap_or(base.status);
        let created_at_ms = existing
            .as_ref()
            .map(|row| row.created_at_ms)
            .unwrap_or(patch.updated_at_ms);
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_config_node_overrides (
                    proxy_config_id, node_id, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
                 ON CONFLICT (proxy_config_id, node_id) DO UPDATE SET
                    proxy_url = EXCLUDED.proxy_url,
                    proxy_username = EXCLUDED.proxy_username,
                    proxy_password = EXCLUDED.proxy_password,
                    status = EXCLUDED.status,
                    updated_at_ms = EXCLUDED.updated_at_ms",
                &[
                    &proxy_id,
                    &self.proxy_scope.node_id,
                    &proxy_url,
                    &proxy_username,
                    &proxy_password,
                    &status,
                    &created_at_ms,
                    &patch.updated_at_ms,
                ],
            )
            .await
            .context("patch postgres proxy config node override")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(proxy_id).await
    }

    fn apply_proxy_scope_metadata(
        &self,
        proxy: &mut AdminProxyConfig,
        effective_source: &str,
        has_node_override: bool,
    ) {
        proxy.scope_node_id = self.proxy_scope.scope_node_id();
        proxy.effective_source = effective_source.to_string();
        proxy.has_node_override = has_node_override;
        proxy.can_edit_slot_metadata = self.proxy_scope.can_edit_slot_metadata();
    }

    pub(super) async fn load_admin_proxy_binding_row(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<AdminProxyBinding> {
        let proxy_configs_by_id = self
            .list_admin_proxy_configs_rows()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        self.load_admin_proxy_binding_from_configs(provider_type, &proxy_configs_by_id)
            .await
    }

    pub(super) async fn load_admin_proxy_binding_from_configs(
        &self,
        provider_type: &str,
        proxy_configs_by_id: &BTreeMap<String, AdminProxyConfig>,
    ) -> anyhow::Result<AdminProxyBinding> {
        self.ensure_connection_alive()?;
        let binding = self
            .client
            .query_opt(
                "SELECT provider_type, proxy_config_id, updated_at_ms
                 FROM llm_proxy_bindings
                 WHERE provider_type = $1",
                &[&provider_type],
            )
            .await
            .context("load proxy binding row")?;
        let Some(row) = binding else {
            return Ok(default_proxy_bindings()
                .into_iter()
                .find(|binding| binding.provider_type == provider_type)
                .unwrap_or_else(|| AdminProxyBinding {
                    provider_type: provider_type.to_string(),
                    effective_source: "none".to_string(),
                    bound_proxy_config_id: None,
                    effective_proxy_config_name: None,
                    effective_proxy_url: None,
                    effective_proxy_username: None,
                    effective_proxy_password: None,
                    binding_updated_at: None,
                    error_message: None,
                }));
        };
        let provider_type: String = row.get(0);
        let proxy_config_id: String = row.get(1);
        let updated_at_ms: i64 = row.get(2);
        let Some(proxy) = proxy_configs_by_id.get(&proxy_config_id).cloned() else {
            return Ok(AdminProxyBinding {
                provider_type,
                effective_source: "invalid".to_string(),
                bound_proxy_config_id: Some(proxy_config_id),
                effective_proxy_config_name: None,
                effective_proxy_url: None,
                effective_proxy_username: None,
                effective_proxy_password: None,
                binding_updated_at: Some(updated_at_ms),
                error_message: Some("bound proxy config is missing".to_string()),
            });
        };
        if proxy.status != core_store::KEY_STATUS_ACTIVE {
            return Ok(AdminProxyBinding {
                provider_type,
                effective_source: "invalid".to_string(),
                bound_proxy_config_id: Some(proxy.id),
                effective_proxy_config_name: Some(proxy.name),
                effective_proxy_url: None,
                effective_proxy_username: None,
                effective_proxy_password: None,
                binding_updated_at: Some(updated_at_ms),
                error_message: Some("bound proxy config is disabled".to_string()),
            });
        }
        Ok(AdminProxyBinding {
            provider_type,
            effective_source: "binding".to_string(),
            bound_proxy_config_id: Some(proxy.id),
            effective_proxy_config_name: Some(proxy.name),
            effective_proxy_url: Some(proxy.proxy_url),
            effective_proxy_username: proxy.proxy_username,
            effective_proxy_password: proxy.proxy_password,
            binding_updated_at: Some(updated_at_ms),
            error_message: None,
        })
    }

    pub(super) async fn load_provider_proxy_resolution_context(
        &self,
        provider_type: &str,
    ) -> anyhow::Result<ProviderProxyResolutionContext> {
        let proxy_configs_by_id = self
            .load_admin_proxy_configs_cached()
            .await?
            .into_iter()
            .map(|proxy| (proxy.id.clone(), proxy))
            .collect::<BTreeMap<_, _>>();
        let binding = self.load_admin_proxy_binding_cached(provider_type).await?;
        Ok(ProviderProxyResolutionContext {
            proxy_configs_by_id,
            binding,
        })
    }
}
#[async_trait]
impl AdminProxyStore for PostgresControlRepository {
    async fn list_admin_proxy_configs(&self) -> anyhow::Result<Vec<AdminProxyConfig>> {
        self.load_admin_proxy_configs_cached().await
    }

    async fn get_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn create_admin_proxy_config(
        &self,
        proxy: NewAdminProxyConfig,
    ) -> anyhow::Result<AdminProxyConfig> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            anyhow::bail!("proxy slots can only be created on the core node");
        }
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_configs (
                    proxy_config_id, name, proxy_url, proxy_username, proxy_password,
                    status, created_at_ms, updated_at_ms
                ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
                &[
                    &proxy.id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    &core_store::KEY_STATUS_ACTIVE,
                    &proxy.created_at_ms,
                    &proxy.created_at_ms,
                ],
            )
            .await
            .context("create postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(&proxy.id)
            .await?
            .context("created postgres proxy config disappeared")
    }

    async fn patch_admin_proxy_config(
        &self,
        proxy_id: &str,
        patch: AdminProxyConfigPatch,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            return self
                .patch_admin_proxy_config_node_override(proxy_id, patch)
                .await;
        }
        let Some(mut proxy) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if let Some(name) = patch.name.as_ref() {
            proxy.name = name.clone();
        }
        if let Some(proxy_url) = patch.proxy_url.as_ref() {
            proxy.proxy_url = proxy_url.clone();
        }
        if let Some(proxy_username) = patch.proxy_username.as_ref() {
            proxy.proxy_username = proxy_username.clone();
        }
        if let Some(proxy_password) = patch.proxy_password.as_ref() {
            proxy.proxy_password = proxy_password.clone();
        }
        if let Some(status) = patch.status.as_ref() {
            proxy.status = status.clone();
        }
        proxy.updated_at = patch.updated_at_ms;
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "UPDATE llm_proxy_configs
                 SET name = $2, proxy_url = $3, proxy_username = $4,
                     proxy_password = $5, status = $6, updated_at_ms = $7
                 WHERE proxy_config_id = $1",
                &[
                    &proxy_id,
                    &proxy.name,
                    &proxy.proxy_url,
                    &proxy.proxy_username,
                    &proxy.proxy_password,
                    &proxy.status,
                    &proxy.updated_at,
                ],
            )
            .await
            .context("patch postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn record_admin_proxy_endpoint_check(
        &self,
        update: AdminProxyEndpointCheckUpdate,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if update.provider_type != core_store::PROVIDER_CODEX
            && update.provider_type != core_store::PROVIDER_KIRO
        {
            anyhow::bail!("unsupported proxy endpoint check provider `{}`", update.provider_type);
        }
        if self
            .get_admin_proxy_config_base_row(&update.proxy_config_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let status_code = update.status_code.map(i32::from);
        self.ensure_connection_alive()?;
        self.client
            .execute(
                "INSERT INTO llm_proxy_config_endpoint_checks (
                    proxy_config_id, node_id, provider_type, target_url, reachable,
                    status_code, latency_ms, error_message, checked_at_ms
                 ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
                 ON CONFLICT (proxy_config_id, node_id, provider_type) DO UPDATE SET
                    target_url = EXCLUDED.target_url,
                    reachable = EXCLUDED.reachable,
                    status_code = EXCLUDED.status_code,
                    latency_ms = EXCLUDED.latency_ms,
                    error_message = EXCLUDED.error_message,
                    checked_at_ms = EXCLUDED.checked_at_ms",
                &[
                    &update.proxy_config_id,
                    &self.proxy_scope.node_id,
                    &update.provider_type,
                    &update.target_url,
                    &update.reachable,
                    &status_code,
                    &update.latency_ms,
                    &update.error_message,
                    &update.checked_at_ms,
                ],
            )
            .await
            .context("record postgres proxy endpoint check")?;
        self.invalidate_proxy_metadata_cache().await;
        self.get_admin_proxy_config_row(&update.proxy_config_id)
            .await
    }

    async fn delete_admin_proxy_config(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        if !self.proxy_scope.can_edit_slot_metadata() {
            anyhow::bail!("proxy slots can only be deleted on the core node");
        }
        let Some(proxy) = self.get_admin_proxy_config_row(proxy_id).await? else {
            return Ok(None);
        };
        self.ensure_connection_alive()?;
        self.client
            .execute("DELETE FROM llm_proxy_configs WHERE proxy_config_id = $1", &[&proxy_id])
            .await
            .context("delete postgres proxy config")?;
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
            .await;
        self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
            .await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(Some(proxy))
    }

    async fn reset_admin_proxy_config_override(
        &self,
        proxy_id: &str,
    ) -> anyhow::Result<Option<AdminProxyConfig>> {
        let Some(_) = self.get_admin_proxy_config_base_row(proxy_id).await? else {
            return Ok(None);
        };
        if !self.proxy_scope.can_edit_slot_metadata() {
            self.ensure_connection_alive()?;
            self.client
                .execute(
                    "DELETE FROM llm_proxy_config_node_overrides
                     WHERE proxy_config_id = $1 AND node_id = $2",
                    &[&proxy_id, &self.proxy_scope.node_id],
                )
                .await
                .context("reset postgres proxy config node override")?;
            self.invalidate_proxy_metadata_cache().await;
            self.invalidate_all_account_views_for_provider(core_store::PROVIDER_CODEX)
                .await;
            self.invalidate_all_account_views_for_provider(core_store::PROVIDER_KIRO)
                .await;
            self.bump_dispatch_generation(core_store::PROVIDER_CODEX)
                .await;
            self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
                .await;
        }
        self.get_admin_proxy_config_row(proxy_id).await
    }

    async fn list_admin_proxy_bindings(&self) -> anyhow::Result<Vec<AdminProxyBinding>> {
        let mut bindings = Vec::new();
        for provider in [core_store::PROVIDER_CODEX, core_store::PROVIDER_KIRO] {
            bindings.push(self.load_admin_proxy_binding_cached(provider).await?);
        }
        Ok(bindings)
    }

    async fn update_admin_proxy_binding(
        &self,
        provider_type: &str,
        proxy_config_id: Option<String>,
    ) -> anyhow::Result<AdminProxyBinding> {
        self.ensure_connection_alive()?;
        match proxy_config_id {
            Some(proxy_config_id) => {
                self.client
                    .execute(
                        "INSERT INTO llm_proxy_bindings (
                            provider_type, proxy_config_id, updated_at_ms
                        ) VALUES ($1, $2, $3)
                        ON CONFLICT(provider_type) DO UPDATE SET
                            proxy_config_id = EXCLUDED.proxy_config_id,
                            updated_at_ms = EXCLUDED.updated_at_ms",
                        &[&provider_type, &proxy_config_id, &now_ms()],
                    )
                    .await
                    .context("upsert postgres proxy binding")?;
            },
            None => {
                self.client
                    .execute("DELETE FROM llm_proxy_bindings WHERE provider_type = $1", &[
                        &provider_type,
                    ])
                    .await
                    .context("delete postgres proxy binding")?;
            },
        }
        self.invalidate_proxy_metadata_cache().await;
        self.invalidate_all_account_views_for_provider(provider_type)
            .await;
        self.bump_dispatch_generation(provider_type).await;
        self.load_admin_proxy_binding_cached(provider_type).await
    }

    async fn import_legacy_kiro_proxy_configs(
        &self,
    ) -> anyhow::Result<AdminLegacyKiroProxyMigration> {
        let mut tuples_to_accounts =
            BTreeMap::<(String, Option<String>, Option<String>), Vec<KiroAccountRecord>>::new();
        for account in self.list_kiro_accounts_rows().await? {
            let auth_json = serde_json::from_str::<serde_json::Value>(&account.auth_json)
                .context("parse postgres kiro auth json for legacy proxy migration")?;
            let Some(proxy_url) = legacy_proxy_json_string(&auth_json, &["proxyUrl", "proxy_url"])
            else {
                continue;
            };
            let proxy_username =
                legacy_proxy_json_string(&auth_json, &["proxyUsername", "proxy_username"]);
            let proxy_password =
                legacy_proxy_json_string(&auth_json, &["proxyPassword", "proxy_password"]);
            tuples_to_accounts
                .entry((proxy_url, proxy_username, proxy_password))
                .or_default()
                .push(account);
        }

        if tuples_to_accounts.is_empty() {
            return Ok(AdminLegacyKiroProxyMigration {
                created_configs: Vec::new(),
                reused_configs: Vec::new(),
                migrated_account_names: Vec::new(),
            });
        }

        let mut existing_by_tuple =
            BTreeMap::<(String, Option<String>, Option<String>), AdminProxyConfig>::new();
        for proxy in self.list_admin_proxy_configs().await? {
            existing_by_tuple.insert(
                (
                    proxy.proxy_url.clone(),
                    proxy.proxy_username.clone(),
                    proxy.proxy_password.clone(),
                ),
                proxy,
            );
        }

        let mut created_configs = Vec::new();
        let mut reused_configs = Vec::new();
        let mut migrated_account_names = Vec::new();
        for (index, (tuple, mut accounts)) in tuples_to_accounts.into_iter().enumerate() {
            let proxy = if let Some(proxy) = existing_by_tuple.get(&tuple).cloned() {
                reused_configs.push(proxy.clone());
                proxy
            } else {
                let now = now_ms();
                let base = format!("llm-proxy-legacy-{}-{}", now, index + 1);
                let mut suffix = 0usize;
                let proxy_id = loop {
                    let candidate =
                        if suffix == 0 { base.clone() } else { format!("{base}-{suffix}") };
                    if !existing_by_tuple
                        .values()
                        .any(|proxy| proxy.id == candidate)
                    {
                        break candidate;
                    }
                    suffix += 1;
                    if suffix >= 1_000 {
                        anyhow::bail!("failed to allocate postgres legacy proxy config id");
                    }
                };
                let proxy = NewAdminProxyConfig {
                    id: proxy_id,
                    name: format!("legacy-kiro-{}", index + 1),
                    proxy_url: tuple.0.clone(),
                    proxy_username: tuple.1.clone(),
                    proxy_password: tuple.2.clone(),
                    created_at_ms: now,
                };
                let created = self.create_admin_proxy_config(proxy).await?;
                existing_by_tuple.insert(tuple.clone(), created.clone());
                created_configs.push(created.clone());
                created
            };

            accounts.sort_by_cached_key(|account| account.account_name.to_ascii_lowercase());
            for mut account in accounts {
                account.proxy_config_id = Some(proxy.id.clone());
                account.updated_at_ms = now_ms();
                account.auth_json = clear_legacy_kiro_proxy_json(&account.auth_json, &proxy.id)?;
                self.upsert_kiro_account(&account).await?;
                self.invalidate_account_cache(core_store::PROVIDER_KIRO, &account.account_name)
                    .await;
                migrated_account_names.push(account.account_name);
            }
        }
        migrated_account_names.sort();
        migrated_account_names.dedup();
        self.invalidate_proxy_metadata_cache().await;
        self.bump_dispatch_generation(core_store::PROVIDER_KIRO)
            .await;
        Ok(AdminLegacyKiroProxyMigration {
            created_configs,
            reused_configs,
            migrated_account_names,
        })
    }
}
