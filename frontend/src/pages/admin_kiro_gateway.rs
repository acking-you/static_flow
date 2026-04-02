//! Admin UI for managing Kiro accounts, keys, usage, and proxy bindings.

use std::collections::{BTreeMap, HashSet};

use gloo_timers::callback::Timeout;
use wasm_bindgen::prelude::*;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        create_admin_kiro_key, create_admin_kiro_manual_account, delete_admin_kiro_account,
        delete_admin_kiro_key, fetch_admin_kiro_accounts, fetch_admin_kiro_keys,
        fetch_admin_kiro_usage_events, fetch_admin_llm_gateway_proxy_bindings,
        fetch_admin_llm_gateway_proxy_configs, fetch_kiro_models, import_admin_kiro_account,
        patch_admin_kiro_account, patch_admin_kiro_key, refresh_admin_kiro_account_balance,
        AdminLlmGatewayKeyView, AdminLlmGatewayUsageEventView, AdminLlmGatewayUsageEventsQuery,
        AdminUpstreamProxyBindingView, AdminUpstreamProxyConfigView, CreateManualKiroAccountInput,
        KiroAccountView, KiroBalanceView, KiroModelView, PatchAdminLlmGatewayKeyRequest,
        PatchKiroAccountInput,
    },
    pages::llm_access_shared::{
        format_float2, format_ms, format_number_i64, format_number_u64, format_reset_hint,
        kiro_credit_ratio, kiro_key_usage_ratio, MaskedSecretCode,
    },
    router::Route,
};

const TAB_OVERVIEW: &str = "overview";
const TAB_ACCOUNTS: &str = "accounts";
const TAB_KEYS: &str = "keys";
const TAB_USAGE: &str = "usage";

/// Shared Tailwind classes for the dark "Kiro" pill badge.
fn kiro_badge() -> Classes {
    classes!(
        "inline-flex",
        "items-center",
        "rounded-full",
        "bg-slate-900",
        "px-2.5",
        "py-1",
        "font-mono",
        "text-[11px]",
        "font-semibold",
        "uppercase",
        "tracking-[0.16em]",
        "text-emerald-300"
    )
}

/// Render a horizontal tab bar. Each `(id, label)` pair becomes a button;
/// the one matching `active` gets the primary style.
fn render_tab_bar(active: &str, tabs: &[(&str, &str)], on_click: &Callback<String>) -> Html {
    html! {
        <nav class={classes!(
            "flex", "items-center", "gap-1.5", "flex-wrap",
            "rounded-xl", "border", "border-[var(--border)]",
            "bg-[var(--surface)]", "p-1.5"
        )} role="tablist">
            { for tabs.iter().map(|(id, label)| {
                let is_active = active == *id;
                let id_owned = id.to_string();
                let on_click = on_click.clone();
                html! {
                    <button
                        type="button"
                        role="tab"
                        aria-selected={is_active.to_string()}
                        class={classes!(
                            "btn-terminal",
                            if is_active { "btn-terminal-primary" } else { "" }
                        )}
                        onclick={Callback::from(move |_| on_click.emit(id_owned.clone()))}
                    >
                        { *label }
                    </button>
                }
            }) }
        </nav>
    }
}

#[wasm_bindgen(inline_js = r#"
export function copy_text(text) {
    if (navigator.clipboard) {
        navigator.clipboard.writeText(text).catch(function(){});
    }
}
"#)]
extern "C" {
    fn copy_text(text: &str);
}

fn format_timestamp_opt(ts: Option<i64>) -> String {
    ts.map(format_ms).unwrap_or_else(|| "-".to_string())
}

fn format_float4(value: f64) -> String {
    format!("{value:.4}")
}

fn format_cache_summary(account: &KiroAccountView) -> String {
    let status = account.cache.status.trim();
    if status.is_empty() {
        return "cache loading".to_string();
    }
    match account.cache.last_checked_at {
        Some(ts) => format!("cache {status} · checked {}", format_ms(ts)),
        None => format!("cache {status}"),
    }
}

fn kiro_account_proxy_select_value(account: &KiroAccountView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "direct".to_string(),
        "fixed" => account
            .proxy_config_id
            .as_deref()
            .map(|id| format!("fixed:{id}"))
            .unwrap_or_else(|| "inherit".to_string()),
        _ => "inherit".to_string(),
    }
}

fn sanitize_kiro_auto_account_names(names: &[String], available_names: &[String]) -> Vec<String> {
    let valid_names = available_names
        .iter()
        .map(|name| name.as_str())
        .collect::<HashSet<_>>();
    let mut sanitized = names
        .iter()
        .filter(|name| valid_names.contains(name.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    sanitized.sort();
    sanitized.dedup();
    sanitized
}

fn sanitize_kiro_fixed_account_name(value: Option<&str>, available_names: &[String]) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if available_names.iter().any(|name| name == value) {
        value.to_string()
    } else {
        String::new()
    }
}

fn kiro_key_route_summary(
    route_strategy: &str,
    fixed_account_name: &str,
    auto_account_names: &[String],
) -> String {
    if route_strategy == "fixed" {
        format!(
            "绑定: {}",
            if fixed_account_name.is_empty() { "未选择" } else { fixed_account_name }
        )
    } else if auto_account_names.is_empty() {
        "全账号池自动择优；如果某个账号不可用，会继续尝试其他账号。".to_string()
    } else {
        format!(
            "仅在这些账号中自动择优: {}；如果子集里没有可用账号，请求会直接报错。",
            auto_account_names.join(", ")
        )
    }
}

fn build_kiro_route_patch_fields(
    route_strategy: &str,
    fixed_account_name: &str,
    auto_account_names: &[String],
) -> (String, String, Vec<String>) {
    if route_strategy == "fixed" {
        return ("fixed".to_string(), fixed_account_name.trim().to_string(), Vec::new());
    }
    if auto_account_names.is_empty() {
        return (String::new(), String::new(), Vec::new());
    }
    ("auto".to_string(), String::new(), auto_account_names.to_vec())
}

#[derive(Properties, PartialEq)]
struct KiroAccountCardProps {
    account: KiroAccountView,
    proxy_configs: Vec<AdminUpstreamProxyConfigView>,
    on_reload: Callback<()>,
    flash: UseStateHandle<Option<String>>,
    notify: Callback<(String, bool)>,
    error: UseStateHandle<Option<String>>,
}

#[function_component(KiroAccountCard)]
fn kiro_account_card(props: &KiroAccountCardProps) -> Html {
    let expanded = use_state(|| false);
    let scheduler_max = use_state(|| props.account.kiro_channel_max_concurrency.to_string());
    let scheduler_min = use_state(|| props.account.kiro_channel_min_start_interval_ms.to_string());
    let selected_proxy = use_state(|| kiro_account_proxy_select_value(&props.account));
    let feedback = use_state(|| None::<String>);
    let busy = use_state(|| false);

    {
        let account = props.account.clone();
        let scheduler_max = scheduler_max.clone();
        let scheduler_min = scheduler_min.clone();
        let selected_proxy = selected_proxy.clone();
        use_effect_with(props.account.clone(), move |_| {
            scheduler_max.set(account.kiro_channel_max_concurrency.to_string());
            scheduler_min.set(account.kiro_channel_min_start_interval_ms.to_string());
            selected_proxy.set(kiro_account_proxy_select_value(&account));
            || ()
        });
    }

    let on_refresh_cache = {
        let account_name = props.account.name.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                busy.set(true);
                error.set(None);
                match refresh_admin_kiro_account_balance(&account_name).await {
                    Ok(_) => {
                        feedback.set(Some("Cache refreshed.".to_string()));
                        let message = format!("Refreshed cached balance for `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((
                            format!(
                                "Failed to refresh cached balance for `{account_name}`.\n{err}"
                            ),
                            true,
                        ));
                    },
                }
                busy.set(false);
            });
        })
    };

    let on_delete_account = {
        let account_name = props.account.name.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                busy.set(true);
                error.set(None);
                match delete_admin_kiro_account(&account_name).await {
                    Ok(_) => {
                        feedback.set(Some("Deleted.".to_string()));
                        let message = format!("Deleted `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to delete `{account_name}`.\n{err}"), true));
                    },
                }
                busy.set(false);
            });
        })
    };

    let on_save_scheduler = {
        let account_name = props.account.name.clone();
        let scheduler_max = scheduler_max.clone();
        let scheduler_min = scheduler_min.clone();
        let selected_proxy = selected_proxy.clone();
        let flash = props.flash.clone();
        let notify = props.notify.clone();
        let error = props.error.clone();
        let feedback = feedback.clone();
        let busy = busy.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let account_name = account_name.clone();
            let scheduler_max = scheduler_max.clone();
            let scheduler_min = scheduler_min.clone();
            let selected_proxy = selected_proxy.clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let feedback = feedback.clone();
            let busy = busy.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_max = match (*scheduler_max).trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Max concurrency must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                let parsed_min = match (*scheduler_min).trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Min start interval must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                busy.set(true);
                error.set(None);
                let (proxy_mode, proxy_config_id) = if *selected_proxy == "direct" {
                    (Some("direct".to_string()), None)
                } else if let Some(proxy_config_id) = (*selected_proxy).strip_prefix("fixed:") {
                    (Some("fixed".to_string()), Some(proxy_config_id.to_string()))
                } else {
                    (Some("inherit".to_string()), None)
                };
                match patch_admin_kiro_account(&account_name, &PatchKiroAccountInput {
                    kiro_channel_max_concurrency: Some(parsed_max),
                    kiro_channel_min_start_interval_ms: Some(parsed_min),
                    proxy_mode,
                    proxy_config_id,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Account settings saved.".to_string()));
                        let message = format!("Updated account settings for `{account_name}`.");
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((
                            format!(
                                "Failed to update account settings for `{account_name}`.\n{err}"
                            ),
                            true,
                        ));
                    },
                }
                busy.set(false);
            });
        })
    };

    let toggle_expanded = {
        let expanded = expanded.clone();
        Callback::from(move |_| expanded.set(!*expanded))
    };

    let account = props.account.clone();
    let email = account.email.clone().unwrap_or_else(|| "-".to_string());
    let expires_at = account
        .expires_at
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let profile_arn = account
        .profile_arn
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let machine_id = account
        .machine_id
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let region = account.region.clone().unwrap_or_else(|| "-".to_string());
    let auth_region = account
        .auth_region
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let api_region = account
        .api_region
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let proxy_url = account.proxy_url.clone().unwrap_or_else(|| "-".to_string());
    let effective_proxy_url = account
        .effective_proxy_url
        .clone()
        .unwrap_or_else(|| "direct".to_string());
    let source = account.source.clone().unwrap_or_else(|| "-".to_string());
    let source_db_path = account
        .source_db_path
        .clone()
        .unwrap_or_else(|| "-".to_string());
    let last_imported = format_timestamp_opt(account.last_imported_at);

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h3 class={classes!("m-0", "text-lg", "font-semibold")}>{ account.name.clone() }</h3>
                        if account.disabled {
                            <span class={classes!("inline-flex", "items-center", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-2.5", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-amber-700", "dark:text-amber-200")}>
                                { "disabled" }
                            </span>
                        }
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                        { format!("{} · provider {} · refresh {}", account.auth_method, account.provider.clone().unwrap_or_else(|| "-".to_string()), if account.has_refresh_token { "present" } else { "missing" }) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format_cache_summary(&account) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!(
                            "scheduler {} in-flight · {} ms spacing",
                            account.kiro_channel_max_concurrency,
                            account.kiro_channel_min_start_interval_ms
                        ) }
                    </p>
                    if let Some(cache_error) = account.cache.error_message.clone() {
                        <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-amber-700", "dark:text-amber-200")}>
                            { cache_error }
                        </p>
                    }
                </div>
                <div class={classes!("flex", "gap-2", "flex-wrap")}>
                    <button type="button" class={classes!("btn-terminal")} onclick={on_refresh_cache.clone()} disabled={*busy}>
                        { "Refresh Cache" }
                    </button>
                    <button type="button" class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")} onclick={on_delete_account.clone()} disabled={*busy}>
                        { "Delete" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota Snapshot" }</div>
                if let Some(balance) = account.balance.clone() {
                    { quota_progress_bar(&balance, account.subscription_title.clone()) }
                } else {
                    <p class={classes!("mt-3", "mb-0", "text-sm", "text-[var(--muted)]")}>{ "Balance not loaded yet." }</p>
                }
            </div>

            <button
                type="button"
                class={classes!("mt-3", "btn-terminal", "text-xs")}
                onclick={toggle_expanded}
            >
                { if *expanded { "收起详情 ▲" } else { "展开详情 ▼" } }
            </button>

            if *expanded {
                <div class={classes!("mt-3", "grid", "gap-4", "lg:grid-cols-3")}>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Identity" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "email: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ email }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "expires_at: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ expires_at }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "profileArn: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ profile_arn }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "machineId: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ machine_id }</dd></div>
                        </dl>
                    </div>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Regions / Proxy" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "region: " }</dt><dd class={classes!("inline", "font-mono")}>{ region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "auth_region: " }</dt><dd class={classes!("inline", "font-mono")}>{ auth_region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "api_region: " }</dt><dd class={classes!("inline", "font-mono")}>{ api_region }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "effective_proxy: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ format!("{} · {}", account.effective_proxy_source, effective_proxy_url) }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "effective_proxy_config: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ account.effective_proxy_config_name.clone().unwrap_or_else(|| "-".to_string()) }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "legacy_proxy_url: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ proxy_url }</dd></div>
                        </dl>
                    </div>
                    <div class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Source" }</div>
                        <dl class={classes!("mt-3", "space-y-2", "text-sm")}>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "source: " }</dt><dd class={classes!("inline", "font-mono")}>{ source }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "source_db_path: " }</dt><dd class={classes!("inline", "font-mono", "break-all")}>{ source_db_path }</dd></div>
                            <div><dt class={classes!("inline", "text-[var(--muted)]")}>{ "last_imported_at: " }</dt><dd class={classes!("inline", "font-mono")}>{ last_imported }</dd></div>
                        </dl>
                    </div>
                </div>
                <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                    <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Scheduler / Proxy" }</div>
                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-3")}>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Max Concurrency" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*scheduler_max).clone()}
                                oninput={{
                                    let scheduler_max = scheduler_max.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        scheduler_max.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Min Start Interval Ms" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*scheduler_min).clone()}
                                oninput={{
                                    let scheduler_min = scheduler_min.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        scheduler_min.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Proxy Mode" }</div>
                            <select
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-sm")}
                                value={(*selected_proxy).clone()}
                                onchange={{
                                    let selected_proxy = selected_proxy.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlSelectElement = event.target_unchecked_into();
                                        selected_proxy.set(input.value());
                                    })
                                }}
                            >
                                <option value="inherit" selected={*selected_proxy == "inherit"}>{ "Inherit Provider Proxy" }</option>
                                <option value="direct" selected={*selected_proxy == "direct"}>{ "Direct / No Proxy" }</option>
                                { for props.proxy_configs.iter().map(|proxy_config| {
                                    let option_value = format!("fixed:{}", proxy_config.id);
                                    html! {
                                        <option value={option_value.clone()} selected={*selected_proxy == option_value}>
                                            { format!("Fixed · {} · {}", proxy_config.name, proxy_config.proxy_url) }
                                        </option>
                                    }
                                }) }
                            </select>
                        </label>
                    </div>
                    <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap")}>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save_scheduler} disabled={*busy}>
                            { if *busy { "Saving..." } else { "Save Account Settings" } }
                        </button>
                        <span class={classes!("text-xs", "text-[var(--muted)]")}>
                            { "并发、起步间隔和账号级 proxy 选择一起保存。未单独指定时，这个账号默认继承 Kiro provider 级代理绑定。" }
                        </span>
                    </div>
                </div>
            }

            if let Some(message) = (*feedback).clone() {
                <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ message }</div>
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct KiroKeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    available_models: Vec<KiroModelView>,
    accounts: Vec<KiroAccountView>,
    on_reload: Callback<()>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(KiroKeyEditorCard)]
fn kiro_key_editor_card(props: &KiroKeyEditorCardProps) -> Html {
    let available_account_names = props
        .accounts
        .iter()
        .map(|account| account.name.clone())
        .collect::<Vec<_>>();
    let name = use_state(|| props.key_item.name.clone());
    let quota = use_state(|| props.key_item.quota_billable_limit.to_string());
    let status = use_state(|| props.key_item.status.clone());
    let route_strategy = use_state(|| {
        props
            .key_item
            .route_strategy
            .clone()
            .unwrap_or_else(|| "auto".to_string())
    });
    let fixed_account_name = use_state(|| {
        sanitize_kiro_fixed_account_name(
            props.key_item.fixed_account_name.as_deref(),
            &available_account_names,
        )
    });
    let auto_account_names = use_state(|| {
        sanitize_kiro_auto_account_names(
            props.key_item.auto_account_names.as_deref().unwrap_or(&[]),
            &available_account_names,
        )
    });
    let model_name_map = use_state(|| props.key_item.model_name_map.clone().unwrap_or_default());
    let kiro_request_validation_enabled =
        use_state(|| props.key_item.kiro_request_validation_enabled);
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let key_item = props.key_item.clone();
        let name = name.clone();
        let quota = quota.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let fixed_account_name = fixed_account_name.clone();
        let auto_account_names = auto_account_names.clone();
        let model_name_map = model_name_map.clone();
        let kiro_request_validation_enabled = kiro_request_validation_enabled.clone();
        let available_account_names = available_account_names.clone();
        use_effect_with(props.key_item.clone(), move |_| {
            name.set(key_item.name.clone());
            quota.set(key_item.quota_billable_limit.to_string());
            status.set(key_item.status.clone());
            route_strategy.set(
                key_item
                    .route_strategy
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
            fixed_account_name.set(sanitize_kiro_fixed_account_name(
                key_item.fixed_account_name.as_deref(),
                &available_account_names,
            ));
            auto_account_names.set(sanitize_kiro_auto_account_names(
                key_item.auto_account_names.as_deref().unwrap_or(&[]),
                &available_account_names,
            ));
            model_name_map.set(key_item.model_name_map.clone().unwrap_or_default());
            kiro_request_validation_enabled.set(key_item.kiro_request_validation_enabled);
            || ()
        });
    }

    {
        let fixed_account_name = fixed_account_name.clone();
        let auto_account_names = auto_account_names.clone();
        use_effect_with(available_account_names.clone(), move |available_account_names| {
            fixed_account_name.set(sanitize_kiro_fixed_account_name(
                Some((*fixed_account_name).as_str()),
                available_account_names,
            ));
            auto_account_names.set(sanitize_kiro_auto_account_names(
                (*auto_account_names).as_slice(),
                available_account_names,
            ));
            || ()
        });
    }

    let on_save = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let name = name.clone();
        let quota = quota.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let fixed_account_name = fixed_account_name.clone();
        let auto_account_names = auto_account_names.clone();
        let model_name_map = model_name_map.clone();
        let kiro_request_validation_enabled = kiro_request_validation_enabled.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let name_value = (*name).clone();
            let quota_value = (*quota).clone();
            let status_value = (*status).clone();
            let route_strategy_value = (*route_strategy).clone();
            let fixed_account_name_value = (*fixed_account_name).clone();
            let auto_account_names_value = (*auto_account_names).clone();
            let (route_strategy_payload, fixed_account_name_payload, auto_account_names_payload) =
                build_kiro_route_patch_fields(
                    &route_strategy_value,
                    &fixed_account_name_value,
                    auto_account_names_value.as_slice(),
                );
            let model_name_map_value = (*model_name_map).clone();
            let kiro_request_validation_enabled_value = *kiro_request_validation_enabled;
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota_value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        feedback.set(Some(message.clone()));
                        on_flash.emit((message, true));
                        return;
                    },
                };
                saving.set(true);
                feedback.set(None);
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(name_value.trim()),
                    status: Some(status_value.trim()),
                    public_visible: None,
                    quota_billable_limit: Some(parsed_quota),
                    route_strategy: Some(route_strategy_payload.as_str()),
                    fixed_account_name: Some(fixed_account_name_payload.as_str()),
                    auto_account_names: Some(auto_account_names_payload.as_slice()),
                    model_name_map: Some(&model_name_map_value),
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    kiro_request_validation_enabled: Some(kiro_request_validation_enabled_value),
                    request_max_concurrency_unlimited: false,
                    request_min_start_interval_ms_unlimited: false,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Saved.".to_string()));
                        on_flash.emit((format!("Saved Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash
                            .emit((format!("Failed to save Kiro key `{key_name}`.\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_disable = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let name = name.clone();
        let quota = quota.clone();
        let route_strategy = route_strategy.clone();
        let fixed_account_name = fixed_account_name.clone();
        let auto_account_names = auto_account_names.clone();
        let model_name_map = model_name_map.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let name_value = (*name).clone();
            let quota_value = (*quota).clone();
            let route_strategy_value = (*route_strategy).clone();
            let fixed_account_name_value = (*fixed_account_name).clone();
            let auto_account_names_value = (*auto_account_names).clone();
            let (route_strategy_payload, fixed_account_name_payload, auto_account_names_payload) =
                build_kiro_route_patch_fields(
                    &route_strategy_value,
                    &fixed_account_name_value,
                    auto_account_names_value.as_slice(),
                );
            let model_name_map_value = (*model_name_map).clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota_value.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        feedback.set(Some(message.clone()));
                        on_flash.emit((message, true));
                        return;
                    },
                };
                saving.set(true);
                feedback.set(None);
                match patch_admin_kiro_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(name_value.trim()),
                    status: Some("disabled"),
                    public_visible: None,
                    quota_billable_limit: Some(parsed_quota),
                    route_strategy: Some(route_strategy_payload.as_str()),
                    fixed_account_name: Some(fixed_account_name_payload.as_str()),
                    auto_account_names: Some(auto_account_names_payload.as_slice()),
                    model_name_map: Some(&model_name_map_value),
                    request_max_concurrency: None,
                    request_min_start_interval_ms: None,
                    kiro_request_validation_enabled: None,
                    request_max_concurrency_unlimited: false,
                    request_min_start_interval_ms_unlimited: false,
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("Disabled.".to_string()));
                        on_flash.emit((format!("Disabled Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("Failed to disable Kiro key `{key_name}`.\n{err}"),
                            true,
                        ));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let key_id = props.key_item.id.clone();
        let key_name = props.key_item.name.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_reload = props.on_reload.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                feedback.set(None);
                match delete_admin_kiro_key(&key_id).await {
                    Ok(_) => {
                        feedback.set(Some("Deleted.".to_string()));
                        on_flash.emit((format!("Deleted Kiro key `{key_name}`."), false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("Failed to delete Kiro key `{key_name}`.\n{err}"),
                            true,
                        ));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_reset_model_map = {
        let model_name_map = model_name_map.clone();
        Callback::from(move |_| model_name_map.set(BTreeMap::new()))
    };

    let toggle_auto_account_name = {
        let auto_account_names = auto_account_names.clone();
        Callback::from(move |account_name: String| {
            let mut names = (*auto_account_names).clone();
            if let Some(idx) = names.iter().position(|name| name == &account_name) {
                names.remove(idx);
            } else {
                names.push(account_name);
                names.sort();
                names.dedup();
            }
            auto_account_names.set(names);
        })
    };

    let route_summary = kiro_key_route_summary(
        (*route_strategy).as_str(),
        (*fixed_account_name).as_str(),
        (*auto_account_names).as_slice(),
    );

    let key_ratio = kiro_key_usage_ratio(
        props.key_item.remaining_billable,
        props.key_item.quota_billable_limit,
    );
    let key_pct = (key_ratio * 100.0).round() as i32;
    let mapping_overrides_preview = if (*model_name_map).is_empty() {
        "identity map".to_string()
    } else {
        (*model_name_map)
            .iter()
            .map(|(source, target)| format!("{source} -> {target}"))
            .collect::<Vec<_>>()
            .join(" · ")
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.key_item.name.clone() }</h3>
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("{} · remaining {}", props.key_item.status, format_number_i64(props.key_item.remaining_billable)) }
                    </p>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("credits {}", format_float4(props.key_item.usage_credit_total)) }
                        if props.key_item.usage_credit_missing_events > 0 {
                            { format!(" · partial ({} missing)", props.key_item.usage_credit_missing_events) }
                        }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("created {} · used {}", format_ms(props.key_item.created_at), format_timestamp_opt(props.key_item.last_used_at)) }
                    </span>
                    <button
                        type="button"
                        class={classes!("btn-terminal", "text-xs")}
                        onclick={{
                            let on_reload = props.on_reload.clone();
                            Callback::from(move |_| on_reload.emit(()))
                        }}
                    >
                        { "Refresh" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    <span>{ "用量" }</span>
                    <span>{ format!("{key_pct}%") }</span>
                </div>
                <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                    <div class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                         style={format!("width: {}%;", key_pct.clamp(0, 100))} />
                </div>
                <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                    <span>{ format!("remaining {}", format_number_i64(props.key_item.remaining_billable)) }</span>
                    <span>{ format!("limit {}", format_number_u64(props.key_item.quota_billable_limit)) }</span>
                </div>
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2")}>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Secret" }</div>
                    <MaskedSecretCode
                        value={props.key_item.secret.clone()}
                        copy_label={"Kiro Key"}
                        on_copy={props.on_copy.clone()}
                        code_class={classes!("leading-6", "text-[var(--text)]")}
                    />
                </div>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Name" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                name.set(input.value());
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota" }</div>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                        value={(*quota).clone()}
                        oninput={{
                            let quota = quota.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                quota.set(input.value());
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Status" }</div>
                    <select
                        key={format!("kiro-key-status-{}", props.key_item.id)}
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                        value={(*status).clone()}
                        onchange={{
                            let status = status.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlSelectElement = event.target_unchecked_into();
                                status.set(input.value());
                            })
                        }}
                    >
                        <option value="active">{ "active" }</option>
                        <option value="disabled">{ "disabled" }</option>
                    </select>
                </label>
                <label class={classes!("md:col-span-2", "flex", "cursor-pointer", "items-start", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*kiro_request_validation_enabled}
                        onchange={{
                            let kiro_request_validation_enabled =
                                kiro_request_validation_enabled.clone();
                            Callback::from(move |event: Event| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                kiro_request_validation_enabled.set(input.checked());
                            })
                        }}
                    />
                    <span>
                        <strong>{ "请求合法性校验" }</strong>
                        <span class={classes!("block", "mt-1", "text-xs", "text-[var(--muted)]")}>
                            { "开启时会在转发前拦截明显坏掉的 Anthropic message 结构。空文本占位块现在会自动忽略；如果某个客户端仍被误伤，可以按 key 关闭这层校验。" }
                        </span>
                    </span>
                </label>
                <div class={classes!("flex", "items-center", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}>
                    <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                        { "private" }
                    </span>
                    <span>{ "Kiro key 不会在公开页面暴露。" }</span>
                </div>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3", "text-sm", "text-[var(--muted)]", "space-y-2")}>
                    <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                        <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                            <span>{ "路由" }</span>
                                <select
                                    key={format!("{}-route-{}", props.key_item.id, (*route_strategy).clone())}
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                                    onchange={{
                                        let route_strategy = route_strategy.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                route_strategy.set(target.value());
                                            }
                                        })
                                    }}
                                >
                                <option value="auto" selected={*route_strategy == "auto"}>{ "自动 (按额度)" }</option>
                                <option value="fixed" selected={*route_strategy == "fixed"}>{ "绑定账号" }</option>
                            </select>
                        </label>
                        if *route_strategy == "fixed" {
                            <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                <span>{ "账号" }</span>
                                <select
                                    key={format!("{}-fixed-{}", props.key_item.id, (*fixed_account_name).clone())}
                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                                    onchange={{
                                        let fixed_account_name = fixed_account_name.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                fixed_account_name.set(target.value());
                                            }
                                        })
                                    }}
                                >
                                    <option value="" selected={(*fixed_account_name).is_empty()}>{ "-- 选择 --" }</option>
                                    { for props.accounts.iter().map(|account| html! {
                                        <option value={account.name.clone()} selected={*fixed_account_name == account.name}>{ account.name.clone() }</option>
                                    }) }
                                </select>
                            </label>
                        } else {
                            <div class={classes!("w-full", "space-y-2")}>
                                <div class={classes!("text-xs", "text-[var(--muted)]")}>{ "自动候选账号（可选，空则走全池）" }</div>
                                if props.accounts.is_empty() {
                                    <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]")}>
                                        { "当前没有可供绑定的账号。" }
                                    </div>
                                } else {
                                    <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                                        { for props.accounts.iter().map(|account| {
                                            let account_name = account.name.clone();
                                            let checked = auto_account_names.iter().any(|name| name == &account.name);
                                            let toggle_auto_account_name = toggle_auto_account_name.clone();
                                            html! {
                                                <label class={classes!(
                                                    "flex", "cursor-pointer", "items-start", "gap-2", "rounded-lg", "border", "px-3", "py-2",
                                                    if checked {
                                                        "border-sky-500/30 bg-sky-500/8"
                                                    } else {
                                                        "border-[var(--border)] bg-[var(--surface-alt)]"
                                                    }
                                                )}>
                                                    <input
                                                        type="checkbox"
                                                        checked={checked}
                                                        onchange={Callback::from(move |_| toggle_auto_account_name.emit(account_name.clone()))}
                                                    />
                                                    <span class={classes!("font-medium")}>{ account.name.clone() }</span>
                                                </label>
                                            }
                                        }) }
                                    </div>
                                }
                            </div>
                        }
                    </div>
                    <div class={classes!("text-xs", "text-[var(--muted)]")}>{ route_summary }</div>
                </div>
                <div class={classes!("md:col-span-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Model Mapping" }</div>
                            <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                { "默认是 source -> source；这里只保存覆盖项。你可以把 Haiku 改写到 Sonnet 或 Opus。" }
                            </div>
                        </div>
                        <button type="button" class={classes!("btn-terminal", "text-xs")} onclick={on_reset_model_map}>
                            { "Reset To Identity" }
                        </button>
                    </div>
                    if props.available_models.is_empty() {
                        <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ "当前没有加载到可用模型目录。" }</div>
                    } else {
                        <div class={classes!("mt-3", "space-y-2")}>
                            { for props.available_models.iter().map(|source_model| {
                                let source_id = source_model.id.clone();
                                let current_target = (*model_name_map)
                                    .get(&source_id)
                                    .cloned()
                                    .unwrap_or_else(|| source_id.clone());
                                let model_name_map = model_name_map.clone();
                                let target_models = props.available_models.clone();
                                html! {
                                    <div class={classes!("grid", "gap-2", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-3", "lg:grid-cols-[minmax(0,1fr)_minmax(18rem,24rem)]")}>
                                        <div>
                                            <div class={classes!("text-sm", "font-semibold", "text-[var(--text)]")}>{ source_model.display_name.clone() }</div>
                                            <div class={classes!("mt-1", "font-mono", "text-[11px]", "break-all", "text-[var(--muted)]")}>{ source_model.id.clone() }</div>
                                        </div>
                                        <label class={classes!("text-sm")}>
                                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Map To" }</div>
                                            <select
                                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                                value={current_target}
                                                onchange={Callback::from(move |event: Event| {
                                                    let input: HtmlSelectElement = event.target_unchecked_into();
                                                    let selected = input.value();
                                                    let mut next = (*model_name_map).clone();
                                                    if selected == source_id {
                                                        next.remove(&source_id);
                                                    } else {
                                                        next.insert(source_id.clone(), selected);
                                                    }
                                                    model_name_map.set(next);
                                                })}
                                            >
                                                { for target_models.iter().map(|target_model| html! {
                                                    <option value={target_model.id.clone()}>
                                                        { format!("{} · {}", target_model.display_name, target_model.id) }
                                                    </option>
                                                }) }
                                            </select>
                                        </label>
                                    </div>
                                }
                            }) }
                        </div>
                        <div class={classes!("mt-3", "font-mono", "text-[11px]", "text-[var(--muted)]", "break-words")}>
                            { format!("overrides: {}", mapping_overrides_preview) }
                        </div>
                    }
                </div>
            </div>

            <div class={classes!("mt-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save}>
                    { if *saving { "Saving..." } else { "Save" } }
                </button>
                <button type="button" class={classes!("btn-terminal")} onclick={on_disable}>
                    { "Disable" }
                </button>
                <button
                    type="button"
                    class={classes!("btn-terminal", "text-red-600", "dark:text-red-400")}
                    onclick={on_delete}
                >
                    { "Delete" }
                </button>
            </div>

            if let Some(message) = (*feedback).clone() {
                <div class={classes!("mt-3", "text-sm", "text-[var(--muted)]")}>{ message }</div>
            }
        </article>
    }
}

#[function_component(AdminKiroGatewayPage)]
/// Render the Kiro-specific admin surface.
///
/// This page owns the full CRUD workflow for Kiro accounts and private keys,
/// plus usage inspection and provider-level proxy context.
pub fn admin_kiro_gateway_page() -> Html {
    let accounts = use_state(Vec::<KiroAccountView>::new);
    let keys = use_state(Vec::<AdminLlmGatewayKeyView>::new);
    let kiro_models = use_state(Vec::<KiroModelView>::new);
    let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventView>::new);
    let usage_loading = use_state(|| false);
    let usage_error = use_state(|| None::<String>);
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let proxy_bindings = use_state(Vec::<AdminUpstreamProxyBindingView>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let flash = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let notify = {
        let flash = flash.clone();
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(message, is_error): (String, bool)| {
            flash.set(Some(message.clone()));
            toast.set(Some((message, is_error)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(2600, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        })
    };
    let refresh_tick = use_state(|| 0u32);
    let active_tab = use_state(|| TAB_OVERVIEW.to_string());
    let on_tab_click = {
        let active_tab = active_tab.clone();
        Callback::from(move |tab: String| active_tab.set(tab))
    };
    let manual_form_expanded = use_state(|| false);

    let import_name = use_state(|| "default".to_string());
    let import_sqlite_path = use_state(String::new);
    let import_scheduler_max = use_state(|| "1".to_string());
    let import_scheduler_min = use_state(|| "0".to_string());

    let manual_name = use_state(String::new);
    let manual_auth_method = use_state(|| "social".to_string());
    let manual_access_token = use_state(String::new);
    let manual_refresh_token = use_state(String::new);
    let manual_profile_arn = use_state(String::new);
    let manual_expires_at = use_state(String::new);
    let manual_client_id = use_state(String::new);
    let manual_client_secret = use_state(String::new);
    let manual_region = use_state(|| "us-east-1".to_string());
    let manual_auth_region = use_state(|| "us-east-1".to_string());
    let manual_api_region = use_state(|| "us-east-1".to_string());
    let manual_machine_id = use_state(String::new);
    let manual_provider = use_state(String::new);
    let manual_email = use_state(String::new);
    let manual_subscription_title = use_state(String::new);
    let manual_scheduler_max = use_state(|| "1".to_string());
    let manual_scheduler_min = use_state(|| "0".to_string());
    let manual_disabled = use_state(|| false);

    let new_key_name = use_state(|| "kiro-private".to_string());
    let new_key_quota = use_state(|| "1000000".to_string());

    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_loading = usage_loading.clone();
        let usage_error = usage_error.clone();
        Callback::from(move |_| {
            let usage_events = usage_events.clone();
            let usage_loading = usage_loading.clone();
            let usage_error = usage_error.clone();
            usage_loading.set(true);
            usage_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_kiro_usage_events(&AdminLlmGatewayUsageEventsQuery {
                    key_id: None,
                    limit: Some(5),
                    offset: Some(0),
                })
                .await
                {
                    Ok(usage_resp) => usage_events.set(usage_resp.events),
                    Err(err) => usage_error.set(Some(err)),
                }
                usage_loading.set(false);
            });
        })
    };

    {
        let accounts = accounts.clone();
        let keys = keys.clone();
        let kiro_models = kiro_models.clone();
        let proxy_configs = proxy_configs.clone();
        let proxy_bindings = proxy_bindings.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with(*refresh_tick, move |_| {
            let accounts = accounts.clone();
            let keys = keys.clone();
            let kiro_models = kiro_models.clone();
            let proxy_configs = proxy_configs.clone();
            let proxy_bindings = proxy_bindings.clone();
            let loading = loading.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                loading.set(true);
                error.set(None);
                let (
                    accounts_result,
                    keys_result,
                    models_result,
                    proxy_configs_result,
                    proxy_bindings_result,
                ) = futures::join!(
                    fetch_admin_kiro_accounts(),
                    fetch_admin_kiro_keys(),
                    fetch_kiro_models(),
                    fetch_admin_llm_gateway_proxy_configs(),
                    fetch_admin_llm_gateway_proxy_bindings(),
                );
                match (
                    accounts_result,
                    keys_result,
                    models_result,
                    proxy_configs_result,
                    proxy_bindings_result,
                ) {
                    (
                        Ok(accounts_resp),
                        Ok(keys_resp),
                        Ok(models_resp),
                        Ok(proxy_configs_resp),
                        Ok(proxy_bindings_resp),
                    ) => {
                        accounts.set(accounts_resp.accounts);
                        keys.set(keys_resp.keys);
                        kiro_models.set(models_resp.data);
                        proxy_configs.set(proxy_configs_resp.proxy_configs);
                        proxy_bindings.set(proxy_bindings_resp.bindings);
                    },
                    (Err(err), _, _, _, _)
                    | (_, Err(err), _, _, _)
                    | (_, _, Err(err), _, _)
                    | (_, _, _, Err(err), _)
                    | (_, _, _, _, Err(err)) => {
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    {
        let reload_usage = reload_usage.clone();
        use_effect_with(*refresh_tick, move |_| {
            reload_usage.emit(());
            || ()
        });
    }

    let on_reload = {
        let refresh_tick = refresh_tick.clone();
        Callback::from(move |_| refresh_tick.set(refresh_tick.wrapping_add(1)))
    };

    let on_copy = {
        let notify = notify.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            notify.emit((format!("Copied {} to clipboard.", label), false));
        })
    };

    let on_import_local = {
        let import_name = import_name.clone();
        let import_sqlite_path = import_sqlite_path.clone();
        let import_scheduler_max = import_scheduler_max.clone();
        let import_scheduler_min = import_scheduler_min.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let import_name = (*import_name).clone();
            let import_sqlite_path = (*import_sqlite_path).clone();
            let import_scheduler_max = (*import_scheduler_max).clone();
            let import_scheduler_min = (*import_scheduler_min).clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_max = match import_scheduler_max.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Import max concurrency must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                let parsed_min = match import_scheduler_min.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message =
                            "Import min start interval must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                error.set(None);
                match import_admin_kiro_account(
                    Some(import_name.as_str()),
                    if import_sqlite_path.trim().is_empty() {
                        None
                    } else {
                        Some(import_sqlite_path.as_str())
                    },
                    Some(parsed_max),
                    Some(parsed_min),
                )
                .await
                {
                    Ok(account) => {
                        let message = format!("Imported local Kiro auth `{}`.", account.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to import local Kiro auth.\n{err}"), true));
                    },
                }
            });
        })
    };

    let on_create_manual = {
        let manual_name = manual_name.clone();
        let manual_auth_method = manual_auth_method.clone();
        let manual_access_token = manual_access_token.clone();
        let manual_refresh_token = manual_refresh_token.clone();
        let manual_profile_arn = manual_profile_arn.clone();
        let manual_expires_at = manual_expires_at.clone();
        let manual_client_id = manual_client_id.clone();
        let manual_client_secret = manual_client_secret.clone();
        let manual_region = manual_region.clone();
        let manual_auth_region = manual_auth_region.clone();
        let manual_api_region = manual_api_region.clone();
        let manual_machine_id = manual_machine_id.clone();
        let manual_provider = manual_provider.clone();
        let manual_email = manual_email.clone();
        let manual_subscription_title = manual_subscription_title.clone();
        let manual_scheduler_max = manual_scheduler_max.clone();
        let manual_scheduler_min = manual_scheduler_min.clone();
        let manual_disabled = manual_disabled.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            let parsed_max = match (*manual_scheduler_max).trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    let message =
                        "Manual account max concurrency must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                },
            };
            let parsed_min = match (*manual_scheduler_min).trim().parse::<u64>() {
                Ok(value) => value,
                Err(_) => {
                    let message =
                        "Manual account min start interval must be a valid integer.".to_string();
                    error.set(Some(message.clone()));
                    notify.emit((message, true));
                    return;
                },
            };
            let input = CreateManualKiroAccountInput {
                name: (*manual_name).trim().to_string(),
                access_token: normalized_str_option(&manual_access_token),
                refresh_token: normalized_str_option(&manual_refresh_token),
                profile_arn: normalized_str_option(&manual_profile_arn),
                expires_at: normalized_str_option(&manual_expires_at),
                auth_method: normalized_str_option(&manual_auth_method),
                client_id: normalized_str_option(&manual_client_id),
                client_secret: normalized_str_option(&manual_client_secret),
                region: normalized_str_option(&manual_region),
                auth_region: normalized_str_option(&manual_auth_region),
                api_region: normalized_str_option(&manual_api_region),
                machine_id: normalized_str_option(&manual_machine_id),
                provider: normalized_str_option(&manual_provider),
                email: normalized_str_option(&manual_email),
                subscription_title: normalized_str_option(&manual_subscription_title),
                kiro_channel_max_concurrency: Some(parsed_max),
                kiro_channel_min_start_interval_ms: Some(parsed_min),
                disabled: *manual_disabled,
            };
            wasm_bindgen_futures::spawn_local(async move {
                error.set(None);
                match create_admin_kiro_manual_account(&input).await {
                    Ok(account) => {
                        let message = format!("Saved manual Kiro account `{}`.", account.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to save manual Kiro account.\n{err}"), true));
                    },
                }
            });
        })
    };

    let on_create_key = {
        let new_key_name = new_key_name.clone();
        let new_key_quota = new_key_quota.clone();
        let flash = flash.clone();
        let notify = notify.clone();
        let error = error.clone();
        let on_reload = on_reload.clone();
        Callback::from(move |_| {
            let name = (*new_key_name).clone();
            let quota = (*new_key_quota).clone();
            let flash = flash.clone();
            let notify = notify.clone();
            let error = error.clone();
            let on_reload = on_reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let parsed_quota = match quota.trim().parse::<u64>() {
                    Ok(value) => value,
                    Err(_) => {
                        let message = "Quota must be a valid integer.".to_string();
                        error.set(Some(message.clone()));
                        notify.emit((message, true));
                        return;
                    },
                };
                error.set(None);
                match create_admin_kiro_key(name.trim(), parsed_quota).await {
                    Ok(key) => {
                        let message = format!("Created Kiro key `{}`.", key.name);
                        flash.set(Some(message.clone()));
                        notify.emit((message, false));
                        on_reload.emit(());
                    },
                    Err(err) => {
                        error.set(Some(err.clone()));
                        notify.emit((format!("Failed to create Kiro key.\n{err}"), true));
                    },
                }
            });
        })
    };

    let disabled_account_count = accounts.iter().filter(|a| a.disabled).count();
    let active_key_count = keys.iter().filter(|k| k.status == "active").count();

    html! {
        <main class={classes!(
            "min-h-screen",
            "bg-[var(--bg)]",
            "px-4",
            "py-8",
            "lg:px-6",
            "lg:py-10"
        )}>
            <div class={classes!("mx-auto", "max-w-6xl", "space-y-4")}>

            // ── Header (always visible) ──
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-4", "flex-wrap")}>
                    <div class={classes!("flex", "items-center", "gap-3")}>
                        <span class={kiro_badge()}>
                            { "Kiro" }
                        </span>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>{ "Gateway Admin" }</h1>
                    </div>
                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                        <Link<Route> to={Route::KiroAccess} classes={classes!("btn-terminal")}>{ "Kiro Access" }</Link<Route>>
                        <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>{ "LLM Access" }</Link<Route>>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "btn-terminal-primary")}
                            onclick={{
                                let on_reload = on_reload.clone();
                                Callback::from(move |_| on_reload.emit(()))
                            }}
                        >
                            { if *loading { "Loading..." } else { "Refresh" } }
                        </button>
                    </div>
                </div>
                if let Some(message) = (*flash).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "bg-emerald-500/10", "px-3", "py-2", "text-sm", "text-emerald-700", "dark:text-emerald-200")}>
                        { message }
                    </div>
                }
                if let Some(err) = (*error).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "bg-red-500/10", "px-3", "py-2", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { err }
                    </div>
                }

                <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2", "xl:grid-cols-4")}>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Accounts" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ accounts.len() }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Disabled" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", if disabled_account_count > 0 { "text-amber-600" } else { "" })}>{ disabled_account_count }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Keys" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ keys.len() }</div>
                    </div>
                    <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Active Keys" }</div>
                        <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ active_key_count }</div>
                    </div>
                </div>
            </section>

            // ── Tab Bar (always visible) ──
            { render_tab_bar(&active_tab, &[
                (TAB_OVERVIEW, "Overview"),
                (TAB_ACCOUNTS, "Accounts"),
                (TAB_KEYS, "Keys"),
                (TAB_USAGE, "Usage"),
            ], &on_tab_click) }

            // ── Overview Tab ──
            if *active_tab == TAB_OVERVIEW {
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Effective Upstream Proxy" }</h2>
                {
                    if let Some(binding) = proxy_bindings.iter().find(|item| item.provider_type == "kiro") {
                        html! {
                            <div class={classes!("mt-4", "space-y-2", "text-sm")}>
                                <div class={classes!("font-mono", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>
                                    { format!("source: {}", binding.effective_source) }
                                </div>
                                <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                                    <div class={classes!("font-mono", "text-xs", "break-all")}>
                                        { binding.effective_proxy_url.clone().unwrap_or_else(|| "-".to_string()) }
                                    </div>
                                    if let Some(name) = binding.effective_proxy_config_name.as_deref() {
                                        <div class={classes!("mt-2", "text-xs", "text-[var(--muted)]")}>{ format!("config: {}", name) }</div>
                                    }
                                    if let Some(error_message) = binding.error_message.as_deref() {
                                        <div class={classes!("mt-2", "text-xs", "text-red-600", "dark:text-red-300")}>{ error_message }</div>
                                    }
                                </div>
                                <p class={classes!("m-0", "text-xs", "text-[var(--muted)]")}>
                                    { "这里是 Kiro 的默认 provider 级代理。账号没有单独指定时继承它；账号改成 direct/fixed 之后，会覆盖这里的默认值。" }
                                </p>
                            </div>
                        }
                    } else {
                        html! {
                            <p class={classes!("mt-4", "text-sm", "text-[var(--muted)]")}>
                                { "当前还没有拿到 Kiro provider 代理绑定状态。" }
                            </p>
                        }
                    }
                }
            </section>
            } // end TAB_OVERVIEW

            // ── Accounts Tab ──
            if *active_tab == TAB_ACCOUNTS {
            <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Import Local Kiro CLI Auth" }</h2>
                    <div class={classes!("mt-4", "space-y-3")}>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Account Name" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*import_name).clone()}
                                oninput={{
                                    let import_name = import_name.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        import_name.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "SQLite Path Override" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                placeholder="~/.local/share/kiro-cli/data.sqlite3"
                                value={(*import_sqlite_path).clone()}
                                oninput={{
                                    let import_sqlite_path = import_sqlite_path.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        import_sqlite_path.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("block", "text-sm")}>
                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Max Concurrency" }</div>
                                <input
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                    value={(*import_scheduler_max).clone()}
                                    oninput={{
                                        let import_scheduler_max = import_scheduler_max.clone();
                                        Callback::from(move |event: InputEvent| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            import_scheduler_max.set(input.value());
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("block", "text-sm")}>
                                <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Min Start Interval Ms" }</div>
                                <input
                                    class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-sm")}
                                    value={(*import_scheduler_min).clone()}
                                    oninput={{
                                        let import_scheduler_min = import_scheduler_min.clone();
                                        Callback::from(move |event: InputEvent| {
                                            let input: HtmlInputElement = event.target_unchecked_into();
                                            import_scheduler_min.set(input.value());
                                        })
                                    }}
                                />
                            </label>
                        </div>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_import_local}>
                            { "Import Local Auth" }
                        </button>
                    </div>
                </article>

                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Manual Kiro Account" }</h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { "手动填写必要或完整字段，保存成单独 JSON 文件。适合已有 refresh token / profileArn / IDC 凭据的场景。" }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "text-xs")}
                            onclick={{
                                let manual_form_expanded = manual_form_expanded.clone();
                                Callback::from(move |_| manual_form_expanded.set(!*manual_form_expanded))
                            }}
                        >
                            { if *manual_form_expanded { "收起 ▲" } else { "展开 ▼" } }
                        </button>
                    </div>
                    if *manual_form_expanded {
                    <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-2")}>
                        { text_input("Name", &manual_name, None) }
                        <label class={classes!("text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Auth Method" }</div>
                            <select
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*manual_auth_method).clone()}
                                onchange={{
                                    let manual_auth_method = manual_auth_method.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlSelectElement = event.target_unchecked_into();
                                        manual_auth_method.set(input.value());
                                    })
                                }}
                            >
                                <option value="social">{ "social" }</option>
                                <option value="idc">{ "idc" }</option>
                            </select>
                        </label>
                        { text_input("Refresh Token", &manual_refresh_token, Some("lg:col-span-2")) }
                        { text_input("Access Token", &manual_access_token, Some("lg:col-span-2")) }
                        { text_input("Profile ARN", &manual_profile_arn, Some("lg:col-span-2")) }
                        { text_input("Expires At (RFC3339)", &manual_expires_at, None) }
                        { text_input("Provider", &manual_provider, None) }
                        { text_input("Email", &manual_email, None) }
                        { text_input("Subscription Title", &manual_subscription_title, None) }
                        { text_input("Client ID", &manual_client_id, None) }
                        { text_input("Client Secret", &manual_client_secret, None) }
                        { text_input("Region", &manual_region, None) }
                        { text_input("Auth Region", &manual_auth_region, None) }
                        { text_input("API Region", &manual_api_region, None) }
                        { text_input("Machine ID", &manual_machine_id, None) }
                        { text_input("Max Concurrency", &manual_scheduler_max, None) }
                        { text_input("Min Start Interval Ms", &manual_scheduler_min, None) }
                    </div>
                    <div class={classes!("mt-4", "flex", "items-center", "gap-4", "flex-wrap", "text-sm", "text-[var(--muted)]")}>
                        <label class={classes!("inline-flex", "items-center", "gap-2")}>
                            <input
                                type="checkbox"
                                checked={*manual_disabled}
                                onchange={{
                                    let manual_disabled = manual_disabled.clone();
                                    Callback::from(move |event: Event| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        manual_disabled.set(input.checked());
                                    })
                                }}
                            />
                            { "disabled" }
                        </label>
                    </div>
                    <button type="button" class={classes!("mt-4", "btn-terminal", "btn-terminal-primary")} onclick={on_create_manual}>
                        { "Save Manual Account" }
                    </button>
                    } // end manual_form_expanded
                </article>
            </section>

            <section>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Kiro Accounts" }</h2>
                    </div>
                </div>
                <div class={classes!("mt-4", "grid", "gap-4", "xl:grid-cols-2")}>
                    {
                        if (*accounts).is_empty() {
                            html! {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                                    { "当前还没有导入任何 Kiro 账号。可以从上面的 SQLite 导入，或者手动填写字段生成一个账号文件。" }
                                </div>
                            }
                        } else {
                            html! {
                                for (*accounts).iter().map(|account| html! {
                                    <KiroAccountCard
                                        key={account.name.clone()}
                                        account={account.clone()}
                                        proxy_configs={(*proxy_configs).clone()}
                                        on_reload={on_reload.clone()}
                                        flash={flash.clone()}
                                        notify={notify.clone()}
                                        error={error.clone()}
                                    />
                                })
                            }
                        }
                    }
                </div>
            </section>
            } // end TAB_ACCOUNTS

            // ── Keys Tab ──
            if *active_tab == TAB_KEYS {
            <section>
                <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Kiro Key" }</h2>
                    <div class={classes!("mt-4", "space-y-3")}>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Key Name" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                                value={(*new_key_name).clone()}
                                oninput={{
                                    let new_key_name = new_key_name.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        new_key_name.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <label class={classes!("block", "text-sm")}>
                            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Quota" }</div>
                            <input
                                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                                value={(*new_key_quota).clone()}
                                oninput={{
                                    let new_key_quota = new_key_quota.clone();
                                    Callback::from(move |event: InputEvent| {
                                        let input: HtmlInputElement = event.target_unchecked_into();
                                        new_key_quota.set(input.value());
                                    })
                                }}
                            />
                        </label>
                        <button type="button" class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create_key}>
                            { "Create Kiro Key" }
                        </button>
                    </div>
                </article>
            </section>

            <section>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Kiro Key Inventory" }</h2>
                    </div>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let on_reload = on_reload.clone();
                            Callback::from(move |_| on_reload.emit(()))
                        }}
                    >
                        { if *loading { "Refreshing..." } else { "Refresh" } }
                    </button>
                </div>
                <div class={classes!("mt-4", "grid", "gap-4", "xl:grid-cols-2")}>
                    {
                        if (*keys).is_empty() {
                            html! {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                                    { "还没有 Kiro key。先创建一个，然后把 base URL 和 key 发给 Claude Code 或 Anthropic SDK 使用。" }
                                </div>
                            }
                        } else {
                            html! {
                                for (*keys).iter().map(|key_item| html! {
                                    <KiroKeyEditorCard
                                        key={key_item.id.clone()}
                                        key_item={key_item.clone()}
                                        available_models={(*kiro_models).clone()}
                                        accounts={(*accounts).clone()}
                                        on_reload={on_reload.clone()}
                                        on_copy={on_copy.clone()}
                                        on_flash={notify.clone()}
                                    />
                                })
                            }
                        }
                    }
                </div>
            </section>
            } // end TAB_KEYS

            // ── Usage Tab ──
            if *active_tab == TAB_USAGE {
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Recent Usage" }</h2>
                    <Link<Route> to={Route::AdminLlmGateway} classes={classes!("btn-terminal")}>
                        { "查看完整记录" }
                    </Link<Route>>
                </div>
                if *usage_loading {
                    <div class={classes!("mt-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                        <i class={classes!("fas", "fa-spinner", "animate-spin")} />
                        <span>{ "加载中" }</span>
                    </div>
                } else if let Some(err) = (*usage_error).clone() {
                    <div class={classes!("mt-3", "rounded-lg", "bg-red-500/10", "px-3", "py-2", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { err }
                    </div>
                } else if (*usage_events).is_empty() {
                    <div class={classes!("mt-3", "font-mono", "text-sm", "text-[var(--muted)]")}>{ "暂无记录" }</div>
                } else {
                    <div class={classes!("mt-3", "space-y-2")}>
                        { for (*usage_events).iter().take(5).map(|event| {
                            let credit_text = event.credit_usage
                                .map(|c| format!("{c:.4}"))
                                .unwrap_or_else(|| "-".to_string());
                            html! {
                                <div class={classes!("flex", "items-center", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono", "text-xs", "flex-wrap")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ format_ms(event.created_at) }</span>
                                    <span class={classes!("font-semibold", "text-[var(--text)]")}>{ event.key_name.clone() }</span>
                                    <span class={classes!("text-[var(--muted)]")}>{ event.model.clone().unwrap_or_else(|| "-".to_string()) }</span>
                                    <span class={classes!("ml-auto", "text-[var(--text)]")}>{ format!("credit {credit_text}") }</span>
                                </div>
                            }
                        }) }
                    </div>
                }
            </section>
            } // end TAB_USAGE

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed", "bottom-5", "right-5", "z-[90]",
                    "max-w-[min(34rem,calc(100vw-2.5rem))]",
                    "rounded-xl", "border", "px-4", "py-3",
                    "text-sm", "font-semibold", "leading-5", "whitespace-pre-wrap",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.15)]",
                    if is_error {
                        classes!("border-red-400/35", "bg-red-500/92", "text-white")
                    } else {
                        classes!("border-emerald-400/35", "bg-emerald-500/92", "text-white")
                    }
                )}>
                    { message }
                </div>
            }
            </div>
        </main>
    }
}

fn normalized_str_option(state: &UseStateHandle<String>) -> Option<String> {
    let value = (**state).trim();
    (!value.is_empty()).then_some(value.to_string())
}

fn text_input(label: &str, state: &UseStateHandle<String>, extra_class: Option<&str>) -> Html {
    let state_handle = state.clone();
    let mut label_classes = classes!("block", "text-sm");
    if let Some(extra_class) = extra_class {
        label_classes.push(extra_class.to_string());
    }
    html! {
        <label class={label_classes}>
            <div class={classes!("mb-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ label }</div>
            <input
                class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                value={(**state).clone()}
                oninput={Callback::from(move |event: InputEvent| {
                    let input: HtmlInputElement = event.target_unchecked_into();
                    state_handle.set(input.value());
                })}
            />
        </label>
    }
}

fn quota_progress_bar(balance: &KiroBalanceView, account_sub_title: Option<String>) -> Html {
    let subscription_title = balance
        .subscription_title
        .clone()
        .unwrap_or_else(|| account_sub_title.unwrap_or_else(|| "-".to_string()));
    let ratio = kiro_credit_ratio(Some(balance.current_usage), Some(balance.usage_limit));
    let pct = (ratio * 100.0).round() as i32;
    html! { <>
        <div class={classes!("mt-3", "grid", "gap-3", "grid-cols-2")}>
            <div>
                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                <div class={classes!("mt-1", "font-mono", "text-xl", "font-black", "text-[var(--text)]")}>
                    { format_float2(balance.remaining) }
                </div>
            </div>
            <div>
                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                <div class={classes!("mt-1", "font-mono", "text-xl", "font-black", "text-[var(--text)]")}>
                    { format_float2(balance.usage_limit) }
                </div>
            </div>
        </div>
        <div class={classes!("mt-3")}>
            <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                <span>{ "用量" }</span>
                <span>{ format!("{pct}%") }</span>
            </div>
            <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface)]")}>
                <div class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                     style={format!("width: {}%;", pct.clamp(0, 100))} />
            </div>
            <div class={classes!("mt-2", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                <span>{ subscription_title }</span>
                <span class={classes!("ml-auto")}>{ format_reset_hint(balance.next_reset_at) }</span>
            </div>
        </div>
    </> }
}

#[cfg(test)]
mod tests {
    use super::{
        build_kiro_route_patch_fields, kiro_key_route_summary, sanitize_kiro_auto_account_names,
        sanitize_kiro_fixed_account_name,
    };

    #[test]
    fn sanitize_kiro_auto_account_names_drops_unknown_and_sorts() {
        let available = vec!["beta".to_string(), "alpha".to_string()];
        let configured = vec![
            "beta".to_string(),
            "missing".to_string(),
            "alpha".to_string(),
            "beta".to_string(),
        ];

        assert_eq!(sanitize_kiro_auto_account_names(&configured, &available), vec![
            "alpha".to_string(),
            "beta".to_string()
        ]);
    }

    #[test]
    fn sanitize_kiro_fixed_account_name_drops_unknown_value() {
        let available = vec!["alpha".to_string(), "beta".to_string()];

        assert_eq!(sanitize_kiro_fixed_account_name(Some("missing"), &available), "");
        assert_eq!(sanitize_kiro_fixed_account_name(Some(" beta "), &available), "beta");
    }

    #[test]
    fn kiro_key_route_summary_uses_full_pool_text_when_subset_is_empty() {
        let summary = kiro_key_route_summary("auto", "", &[]);
        assert!(summary.contains("全账号池自动择优"));
    }

    #[test]
    fn build_kiro_route_patch_fields_uses_default_route_when_auto_subset_is_empty() {
        let (strategy, fixed_account_name, auto_account_names) =
            build_kiro_route_patch_fields("auto", "alpha", &[]);
        assert!(strategy.is_empty());
        assert!(fixed_account_name.is_empty());
        assert!(auto_account_names.is_empty());
    }
}
