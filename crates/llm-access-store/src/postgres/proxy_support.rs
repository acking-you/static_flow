//! Proxy config transforms: admin<->provider conversion, node-override and
//! endpoint-check application, and legacy-proxy JSON cleanup.

use anyhow::Context;
use llm_access_core::store::{self as core_store, AdminProxyConfig, ProviderProxyConfig};

use super::{ProviderProxyResolutionContext, ProxyConfigNodeOverride, ProxyEndpointCheckRow};

fn provider_proxy_from_admin_proxy(proxy: AdminProxyConfig) -> ProviderProxyConfig {
    ProviderProxyConfig {
        proxy_url: proxy.proxy_url,
        proxy_username: proxy.proxy_username,
        proxy_password: proxy.proxy_password,
    }
}

pub fn apply_proxy_config_node_override(
    proxy: &mut AdminProxyConfig,
    override_row: &ProxyConfigNodeOverride,
) {
    proxy.proxy_url = override_row.proxy_url.clone();
    proxy.proxy_username = override_row.proxy_username.clone();
    proxy.proxy_password = override_row.proxy_password.clone();
    proxy.status = override_row.status.clone();
    proxy.updated_at = override_row.updated_at_ms;
}

pub fn apply_proxy_endpoint_checks(proxy: &mut AdminProxyConfig, rows: &[ProxyEndpointCheckRow]) {
    proxy.latest_codex_check = None;
    proxy.latest_kiro_check = None;
    for row in rows {
        match row.provider_type.as_str() {
            core_store::PROVIDER_CODEX => proxy.latest_codex_check = Some(row.check.clone()),
            core_store::PROVIDER_KIRO => proxy.latest_kiro_check = Some(row.check.clone()),
            _ => {},
        }
    }
}

pub fn legacy_proxy_json_string(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| value.get(*key))
        .find_map(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub fn clear_legacy_kiro_proxy_json(
    auth_json: &str,
    proxy_config_id: &str,
) -> anyhow::Result<String> {
    let mut value = serde_json::from_str::<serde_json::Value>(auth_json)
        .context("parse postgres kiro auth json for legacy proxy cleanup")?;
    if let Some(object) = value.as_object_mut() {
        for key in [
            "proxyUrl",
            "proxy_url",
            "proxyUsername",
            "proxy_username",
            "proxyPassword",
            "proxy_password",
        ] {
            object.remove(key);
        }
        object.insert("proxyMode".to_string(), serde_json::Value::String("fixed".to_string()));
        object.insert(
            "proxyConfigId".to_string(),
            serde_json::Value::String(proxy_config_id.to_string()),
        );
    }
    serde_json::to_string(&value).context("serialize postgres kiro auth json after proxy cleanup")
}

pub fn resolve_provider_proxy_config_from_context(
    proxy_mode: &str,
    proxy_config_id: Option<&str>,
    context: &ProviderProxyResolutionContext,
) -> anyhow::Result<Option<ProviderProxyConfig>> {
    match proxy_mode {
        "none" | "direct" => Ok(None),
        "fixed" => {
            let Some(proxy_id) = proxy_config_id else {
                anyhow::bail!("fixed proxy mode requires proxy_config_id");
            };
            let Some(proxy) = context.proxy_configs_by_id.get(proxy_id).cloned() else {
                anyhow::bail!("fixed proxy config `{proxy_id}` is missing");
            };
            if proxy.status != core_store::KEY_STATUS_ACTIVE {
                anyhow::bail!("fixed proxy config `{}` is disabled", proxy.name);
            }
            Ok(Some(provider_proxy_from_admin_proxy(proxy)))
        },
        _ => {
            if let Some(message) = context.binding.error_message.clone() {
                anyhow::bail!("provider proxy binding is invalid: {message}");
            }
            match context.binding.effective_proxy_url.clone() {
                Some(proxy_url) => Ok(Some(ProviderProxyConfig {
                    proxy_url,
                    proxy_username: context.binding.effective_proxy_username.clone(),
                    proxy_password: context.binding.effective_proxy_password.clone(),
                })),
                None => Ok(None),
            }
        },
    }
}
