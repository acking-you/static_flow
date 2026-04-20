use std::collections::{BTreeMap, HashSet};

use gloo_timers::callback::Timeout;
use js_sys::Date;
use wasm_bindgen::prelude::*;
use web_sys::{window, HtmlElement, HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        admin_approve_and_issue_llm_gateway_account_contribution_request,
        admin_approve_and_issue_llm_gateway_token_request,
        admin_approve_llm_gateway_sponsor_request,
        admin_reject_llm_gateway_account_contribution_request,
        admin_reject_llm_gateway_token_request, check_admin_llm_gateway_proxy_config,
        create_admin_llm_gateway_account_group, create_admin_llm_gateway_key,
        create_admin_llm_gateway_proxy_config, delete_admin_llm_gateway_account,
        delete_admin_llm_gateway_account_group, delete_admin_llm_gateway_key,
        delete_admin_llm_gateway_proxy_config, delete_admin_llm_gateway_sponsor_request,
        fetch_admin_llm_gateway_account_contribution_requests,
        fetch_admin_llm_gateway_account_groups, fetch_admin_llm_gateway_accounts,
        fetch_admin_llm_gateway_config, fetch_admin_llm_gateway_keys,
        fetch_admin_llm_gateway_proxy_bindings, fetch_admin_llm_gateway_proxy_configs,
        fetch_admin_llm_gateway_sponsor_requests, fetch_admin_llm_gateway_token_requests,
        fetch_admin_llm_gateway_usage_event_detail, fetch_admin_llm_gateway_usage_events,
        import_admin_legacy_kiro_proxy_configs, import_admin_llm_gateway_account,
        patch_admin_llm_gateway_account, patch_admin_llm_gateway_account_group,
        patch_admin_llm_gateway_key, patch_admin_llm_gateway_proxy_config,
        refresh_admin_llm_gateway_account, update_admin_llm_gateway_config,
        update_admin_llm_gateway_proxy_binding, AccountSummaryView, AdminAccountGroupView,
        AdminLlmGatewayAccountContributionRequestView,
        AdminLlmGatewayAccountContributionRequestsQuery, AdminLlmGatewayKeyView,
        AdminLlmGatewaySponsorRequestView, AdminLlmGatewaySponsorRequestsQuery,
        AdminLlmGatewayTokenRequestView, AdminLlmGatewayTokenRequestsQuery,
        AdminLlmGatewayUsageEventDetailView, AdminLlmGatewayUsageEventView,
        AdminLlmGatewayUsageEventsQuery, AdminUpstreamProxyBindingView,
        AdminUpstreamProxyCheckResponse, AdminUpstreamProxyCheckTargetView,
        AdminUpstreamProxyConfigView, CreateAdminAccountGroupInput,
        CreateAdminUpstreamProxyConfigInput, LlmGatewayRuntimeConfig, PatchAdminAccountGroupInput,
        PatchAdminLlmGatewayAccountInput, PatchAdminLlmGatewayKeyRequest,
        PatchAdminUpstreamProxyConfigInput,
    },
    components::pagination::Pagination,
    pages::llm_access_shared::{format_number_i64, format_number_u64, MaskedSecretCode},
    router::Route,
};

const USAGE_PAGE_SIZE: usize = 20;
const TOKEN_REQUEST_PAGE_SIZE: usize = 20;
const ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE: usize = 20;
const SPONSOR_REQUEST_PAGE_SIZE: usize = 20;

const TAB_OVERVIEW: &str = "overview";
const TAB_KEYS: &str = "keys";
const TAB_GROUPS: &str = "groups";
const TAB_ACCOUNTS: &str = "accounts";
const TAB_USAGE: &str = "usage";
const TAB_REQUESTS: &str = "requests";
const TAB_SETTINGS: &str = "settings";

/// Render a horizontal tab bar with an optional numeric badge on one tab.
/// `badge_tab` is `Some((tab_id, count))` to show a pending-count pill.
fn render_tab_bar(
    active: &str,
    tabs: &[(&str, &str)],
    on_click: &Callback<String>,
    badge_tab: Option<(&str, usize)>,
) -> Html {
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
                let badge_count = badge_tab
                    .filter(|(bid, count)| *bid == *id && *count > 0)
                    .map(|(_, count)| count);
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
                        if let Some(count) = badge_count {
                            <span class={classes!(
                                "ml-1.5", "inline-flex", "items-center", "justify-center",
                                "min-w-[1.25rem]", "h-5", "rounded-full",
                                "bg-amber-500", "text-white", "text-[10px]", "font-bold"
                            )}>
                                { count }
                            </span>
                        }
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

fn format_ms(ts_ms: i64) -> String {
    let d = Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64));
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        d.get_full_year(),
        d.get_month() + 1,
        d.get_date(),
        d.get_hours(),
        d.get_minutes(),
        d.get_seconds(),
    )
}

fn account_proxy_select_value(account: &AccountSummaryView) -> String {
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

fn account_configured_proxy_label(account: &AccountSummaryView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "configured: direct".to_string(),
        "fixed" => account
            .effective_proxy_config_name
            .as_deref()
            .map(|name| format!("configured: fixed ({name})"))
            .or_else(|| {
                account
                    .proxy_config_id
                    .as_deref()
                    .map(|id| format!("configured: fixed ({id})"))
            })
            .unwrap_or_else(|| "configured: fixed".to_string()),
        _ => "configured: inherit provider".to_string(),
    }
}

fn format_latency_ms(latency_ms: i32) -> String {
    format!("{} ms", latency_ms.max(0))
}

fn format_credit4(value: f64) -> String {
    format!("{value:.4}")
}

fn key_credit_display(key_item: &AdminLlmGatewayKeyView) -> String {
    if key_item.usage_credit_total > 0.0 || key_item.usage_credit_missing_events > 0 {
        format_credit4(key_item.usage_credit_total)
    } else {
        "-".to_string()
    }
}

fn sanitize_auto_account_names(names: &[String], accounts: &[AccountSummaryView]) -> Vec<String> {
    let valid_names = accounts
        .iter()
        .map(|account| account.name.as_str())
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

fn sanitize_account_group_id(
    value: Option<&str>,
    groups: &[AdminAccountGroupView],
    _allow_empty: bool,
) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return String::new();
    };
    if groups.iter().any(|group| group.id == value) {
        value.to_string()
    } else {
        String::new()
    }
}

fn group_name_for_id(groups: &[AdminAccountGroupView], group_id: &str) -> String {
    groups
        .iter()
        .find(|group| group.id == group_id)
        .map(|group| group.name.clone())
        .unwrap_or_else(|| group_id.to_string())
}

fn format_proxy_check_target_line(target: &AdminUpstreamProxyCheckTargetView) -> String {
    if target.reachable {
        format!(
            "{}: {} in {} ms",
            target.target,
            target
                .status_code
                .map(|status| status.to_string())
                .unwrap_or_else(|| "ok".to_string()),
            target.latency_ms.max(0)
        )
    } else {
        format!(
            "{}: {}",
            target.target,
            target
                .error_message
                .clone()
                .unwrap_or_else(|| "request failed".to_string())
        )
    }
}

fn format_proxy_check_message(result: &AdminUpstreamProxyCheckResponse) -> String {
    let mut lines = vec![if result.ok {
        format!(
            "{} 代理检查成功：{}",
            result.provider_type.to_uppercase(),
            result.proxy_config_name
        )
    } else {
        format!(
            "{} 代理检查失败：{}",
            result.provider_type.to_uppercase(),
            result.proxy_config_name
        )
    }];
    lines.push(format!("使用认证：{}", result.auth_label));
    lines.extend(result.targets.iter().map(format_proxy_check_target_line));
    lines.join("\n")
}

fn preview_text(text: &str, max_chars: usize) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return "-".to_string();
    }
    let total_chars = trimmed.chars().count();
    if total_chars <= max_chars {
        trimmed.to_string()
    } else {
        let prefix = trimmed.chars().take(max_chars).collect::<String>();
        format!("{prefix}...")
    }
}

fn is_gpt_pro_account(plan_type: Option<&str>) -> bool {
    plan_type.map(str::trim).is_some_and(|plan| {
        let normalized = plan.to_ascii_lowercase();
        normalized == "pro" || normalized == "gpt pro"
    })
}

// Render a compact status pill that matches the current key state.
fn key_status_badge(status: &str) -> Classes {
    let base = classes!(
        "inline-flex",
        "items-center",
        "rounded-full",
        "px-2.5",
        "py-1",
        "text-xs",
        "font-semibold",
        "uppercase",
        "tracking-[0.16em]"
    );
    match status {
        "active" => {
            classes!(base, "bg-emerald-500/12", "text-emerald-700", "dark:text-emerald-200")
        },
        "disabled" => classes!(base, "bg-slate-500/14", "text-slate-700", "dark:text-slate-200"),
        _ => classes!(base, "bg-[var(--surface-alt)]", "text-[var(--muted)]"),
    }
}

// Keep copy affordances visually small so dense diagnostics tables stay
// readable.
fn copy_icon_button(text: &str, on_copy: &Callback<(String, String)>) -> Html {
    let value = text.to_string();
    let on_copy = on_copy.clone();
    html! {
        <button
            type="button"
            class={classes!(
                "inline-flex",
                "h-8",
                "w-8",
                "items-center",
                "justify-center",
                "rounded-full",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "text-[var(--muted)]",
                "transition-colors",
                "hover:text-[var(--primary)]",
                "hover:bg-[var(--surface-alt)]"
            )}
            title="复制"
            aria-label="复制"
            onclick={Callback::from(move |_| on_copy.emit(("".to_string(), value.clone())))}
        >
            <i class={classes!("fas", "fa-copy", "text-xs")} />
        </button>
    }
}

fn copyable_token_preview(label: &str, value: &str, on_copy: &Callback<(String, String)>) -> Html {
    html! {
        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    { label }
                </div>
                { copy_icon_button(value, on_copy) }
            </div>
            <code class={classes!("mt-2", "block", "break-all", "text-xs", "text-[var(--text)]")}>
                { preview_text(value, 96) }
            </code>
        </div>
    }
}

// Reformat stored header JSON before showing it in the modal dialog.
fn pretty_headers_json(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

fn usage_last_message_preview(event: &AdminLlmGatewayUsageEventView) -> String {
    event
        .last_message_content
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "-".to_string())
}

fn usage_last_message_table_preview(event: &AdminLlmGatewayUsageEventView) -> String {
    let preview = usage_last_message_preview(event);
    if preview == "-" {
        return preview;
    }
    let single_line = preview.split_whitespace().collect::<Vec<_>>().join(" ");
    preview_text(&single_line, 120)
}

fn pretty_json_text(raw: &str) -> String {
    serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| raw.to_string())
}

#[derive(Properties, PartialEq)]
struct KeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    on_changed: Callback<()>,
    on_refresh: Callback<(String, String)>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
    refreshing: bool,
    accounts: Vec<AccountSummaryView>,
    account_groups: Vec<AdminAccountGroupView>,
}

#[function_component(KeyEditorCard)]
fn key_editor_card(props: &KeyEditorCardProps) -> Html {
    let key_item = props.key_item.clone();
    let key_name_for_actions = key_item.name.clone();
    let name = use_state(|| key_item.name.clone());
    let quota = use_state(|| key_item.quota_billable_limit.to_string());
    let public_visible = use_state(|| key_item.public_visible);
    let status = use_state(|| key_item.status.clone());
    let route_strategy = use_state(|| {
        key_item
            .route_strategy
            .clone()
            .unwrap_or_else(|| "auto".to_string())
    });
    let account_group_id = use_state(|| {
        sanitize_account_group_id(key_item.account_group_id.as_deref(), &props.account_groups, true)
    });
    let request_max_concurrency = use_state(|| {
        key_item
            .request_max_concurrency
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let request_min_start_interval_ms = use_state(|| {
        key_item
            .request_min_start_interval_ms
            .map(|value| value.to_string())
            .unwrap_or_default()
    });
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        // Reset editor controls whenever the parent list refreshes this card.
        let key_item = props.key_item.clone();
        let account_groups = props.account_groups.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let request_max_concurrency = request_max_concurrency.clone();
        let request_min_start_interval_ms = request_min_start_interval_ms.clone();
        use_effect_with((props.key_item.clone(), props.account_groups.clone()), move |_| {
            name.set(key_item.name.clone());
            quota.set(key_item.quota_billable_limit.to_string());
            public_visible.set(key_item.public_visible);
            status.set(key_item.status.clone());
            route_strategy.set(
                key_item
                    .route_strategy
                    .clone()
                    .unwrap_or_else(|| "auto".to_string()),
            );
            account_group_id.set(sanitize_account_group_id(
                key_item.account_group_id.as_deref(),
                &account_groups,
                true,
            ));
            request_max_concurrency.set(
                key_item
                    .request_max_concurrency
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            request_min_start_interval_ms.set(
                key_item
                    .request_min_start_interval_ms
                    .map(|value| value.to_string())
                    .unwrap_or_default(),
            );
            || ()
        });
    }

    if key_item.provider_type == "kiro" {
        return html! {
            <article class={classes!(
                "rounded-xl",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "p-4"
            )}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "bg-slate-900", "px-2.5", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]", "text-emerald-300")}>
                            { "Kiro Key" }
                        </span>
                        <h3 class={classes!("m-0", "text-base", "font-bold")}>{ key_item.name.clone() }</h3>
                    </div>
                    <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal")}>
                        { "前往 /admin/kiro-gateway" }
                    </Link<Route>>
                </div>

                <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-2", "text-xs", "text-emerald-200")}>
                    <MaskedSecretCode
                        value={key_item.secret.clone()}
                        copy_label={"Kiro Key"}
                        on_copy={props.on_copy.clone()}
                        code_class={classes!("text-emerald-200")}
                    />
                </div>

                <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap", "text-xs", "text-[var(--muted)]")}>
                    <span>{ format!("status {}", key_item.status) }</span>
                    <span>{ format!("created {}", format_ms(key_item.created_at)) }</span>
                    <button
                        class={classes!("btn-terminal", "ml-auto")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("Kiro Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制" }
                    </button>
                </div>
            </article>
        };
    }

    let on_save = {
        let key_id = key_item.id.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let account_group_id = account_group_id.clone();
        let request_max_concurrency = request_max_concurrency.clone();
        let request_min_start_interval_ms = request_min_start_interval_ms.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_changed = props.on_changed.clone();
        let key_name_for_actions = key_name_for_actions.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let key_name = key_name_for_actions.clone();
            let name_value = (*name).trim().to_string();
            let quota_value = (*quota).trim().parse::<u64>();
            let public_visible_value = *public_visible;
            let status_value = (*status).clone();
            let route_strategy_value = (*route_strategy).clone();
            let account_group_id_value = (*account_group_id).clone();
            let request_max_concurrency_value = (*request_max_concurrency).trim().to_string();
            let request_min_start_interval_ms_value =
                (*request_min_start_interval_ms).trim().to_string();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if *saving {
                    return;
                }
                let Ok(quota_value) = quota_value else {
                    let message = "额度必须是正整数".to_string();
                    feedback.set(Some(message.clone()));
                    on_flash.emit((message, true));
                    return;
                };
                let request_max_concurrency_value = if request_max_concurrency_value.is_empty() {
                    None
                } else {
                    match request_max_concurrency_value.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "并发上限必须是整数，留空表示不限制".to_string();
                            feedback.set(Some(message.clone()));
                            on_flash.emit((message, true));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms_value =
                    if request_min_start_interval_ms_value.is_empty() {
                        None
                    } else {
                        match request_min_start_interval_ms_value.parse::<u64>() {
                            Ok(value) => Some(value),
                            Err(_) => {
                                let message = "请求间隔必须是整数毫秒，留空表示不限制".to_string();
                                feedback.set(Some(message.clone()));
                                on_flash.emit((message, true));
                                return;
                            },
                        }
                    };
                saving.set(true);
                match patch_admin_llm_gateway_key(&key_id, PatchAdminLlmGatewayKeyRequest {
                    name: Some(&name_value),
                    status: Some(&status_value),
                    public_visible: Some(public_visible_value),
                    quota_billable_limit: Some(quota_value),
                    route_strategy: Some(&route_strategy_value),
                    account_group_id: Some(&account_group_id_value),
                    fixed_account_name: None,
                    auto_account_names: None,
                    model_name_map: None,
                    request_max_concurrency: request_max_concurrency_value,
                    request_min_start_interval_ms: request_min_start_interval_ms_value,
                    kiro_request_validation_enabled: None,
                    kiro_cache_estimation_enabled: None,
                    kiro_cache_policy_override_json: None,
                    kiro_billable_model_multipliers_override_json: None,
                    request_max_concurrency_unlimited: request_max_concurrency_value.is_none(),
                    request_min_start_interval_ms_unlimited: request_min_start_interval_ms_value
                        .is_none(),
                })
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("已保存".to_string()));
                        on_flash.emit((format!("已保存 key `{}`", key_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存 key `{}` 失败\n{err}", key_name), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let key_id = key_item.id.clone();
        let on_changed = props.on_changed.clone();
        let feedback = feedback.clone();
        let saving = saving.clone();
        let on_flash = props.on_flash.clone();
        let key_name_for_actions = key_name_for_actions.clone();
        Callback::from(move |_| {
            let Some(window) = window() else {
                return;
            };
            if !window
                .confirm_with_message("确认删除这个 API key？")
                .ok()
                .unwrap_or(false)
            {
                return;
            }
            let key_id = key_id.clone();
            let key_name = key_name_for_actions.clone();
            let feedback = feedback.clone();
            let saving = saving.clone();
            let on_flash = on_flash.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_key(&key_id).await {
                    Ok(_) => {
                        feedback.set(Some("已删除".to_string()));
                        on_flash.emit((format!("已删除 key `{}`", key_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("删除 key `{}` 失败\n{err}", key_name), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let fixed_route_groups = props
        .account_groups
        .iter()
        .filter(|group| group.account_names.len() == 1)
        .cloned()
        .collect::<Vec<_>>();
    let current_route_summary = if *route_strategy == "fixed" {
        if (*account_group_id).is_empty() {
            "固定组：未选择".to_string()
        } else {
            format!(
                "固定组：{}",
                group_name_for_id(&props.account_groups, (*account_group_id).as_str())
            )
        }
    } else if (*account_group_id).is_empty() {
        "自动：全账号池".to_string()
    } else {
        format!("自动：{}", group_name_for_id(&props.account_groups, (*account_group_id).as_str()))
    };

    html! {
        <article class={classes!(
            "rounded-xl",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "p-4"
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    <div class={key_status_badge(&key_item.status)}>{ key_item.status.clone() }</div>
                    <h3 class={classes!("m-0", "text-base", "font-bold")}>{ key_item.name.clone() }</h3>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format_ms(key_item.created_at) }</span>
                </div>
                <div class={classes!("flex", "gap-2")}>
                    <button
                        class={classes!("btn-terminal")}
                        title="刷新额度"
                        aria-label="刷新额度"
                        onclick={{
                            let on_refresh = props.on_refresh.clone();
                            let key_id = key_item.id.clone();
                            let key_name = key_item.name.clone();
                            Callback::from(move |_| on_refresh.emit((key_id.clone(), key_name.clone())))
                        }}
                        disabled={props.refreshing}
                    >
                        <i class={classes!("fas", if props.refreshing { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                    </button>
                    <button
                        class={classes!("btn-terminal")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制" }
                    </button>
                    <button class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")} onclick={on_delete} disabled={*saving}>
                        { "删除" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-2", "text-xs", "text-emerald-200")}>
                <MaskedSecretCode
                    value={key_item.secret.clone()}
                    copy_label={"Key"}
                    on_copy={props.on_copy.clone()}
                    code_class={classes!("text-emerald-200")}
                />
            </div>

            <div class={classes!("mt-3", "grid", "gap-3", "xl:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "名称" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    name.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "额度上限" }</span>
                    <input
                        type="number"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*quota).clone()}
                        oninput={{
                            let quota = quota.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    quota.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-3", "grid", "gap-3", "xl:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "并发上限" }</span>
                    <input
                        type="number"
                        placeholder="留空表示不限制"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*request_max_concurrency).clone()}
                        oninput={{
                            let request_max_concurrency = request_max_concurrency.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    request_max_concurrency.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "请求起始间隔 ms" }</span>
                    <input
                        type="number"
                        placeholder="留空表示不限制"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*request_min_start_interval_ms).clone()}
                        oninput={{
                            let request_min_start_interval_ms = request_min_start_interval_ms.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    request_min_start_interval_ms.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap")}>
                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                    <input
                        type="checkbox"
                        checked={*public_visible}
                        onchange={{
                            let public_visible = public_visible.clone();
                            Callback::from(move |event: Event| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    public_visible.set(target.checked());
                                }
                            })
                        }}
                    />
                    <span>{ "公开" }</span>
                </label>
                <select
                    key={format!("{}-status-{}", key_item.id, (*status).clone())}
                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                    onchange={{
                        let status = status.clone();
                        Callback::from(move |event: Event| {
                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                status.set(target.value());
                            }
                        })
                    }}
                >
                    <option value="active" selected={*status == "active"}>{ "active" }</option>
                    <option value="disabled" selected={*status == "disabled"}>{ "disabled" }</option>
                </select>
                <button class={classes!("btn-terminal", "btn-terminal-primary", "ml-auto")} onclick={on_save} disabled={*saving}>
                    { if *saving { "保存中..." } else { "保存" } }
                </button>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-3", "flex-wrap")}>
                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "路由" }</span>
                    <select
                        key={format!("{}-route-{}", key_item.id, (*route_strategy).clone())}
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
                        <span class={classes!("text-[var(--muted)]")}>{ "单账号组" }</span>
                        <select
                            key={format!("{}-group-fixed-{}", key_item.id, (*account_group_id).clone())}
                            class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                            onchange={{
                                let account_group_id = account_group_id.clone();
                                Callback::from(move |event: Event| {
                                    if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                        account_group_id.set(target.value());
                                    }
                                })
                            }}
                        >
                            <option value="" selected={(*account_group_id).is_empty()}>{ "-- 选择组 --" }</option>
                            { for fixed_route_groups.iter().map(|group| html! {
                                <option value={group.id.clone()} selected={*account_group_id == group.id}>{ format!("{} ({})", group.name, group.account_names.join(", ")) }</option>
                            }) }
                        </select>
                    </label>
                } else {
                    <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                        <span class={classes!("text-[var(--muted)]")}>{ "账号组" }</span>
                        <select
                            key={format!("{}-group-auto-{}", key_item.id, (*account_group_id).clone())}
                            class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-1.5", "text-sm")}
                            onchange={{
                                let account_group_id = account_group_id.clone();
                                Callback::from(move |event: Event| {
                                    if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                        account_group_id.set(target.value());
                                    }
                                })
                            }}
                        >
                            <option value="" selected={(*account_group_id).is_empty()}>{ "全账号池" }</option>
                            { for props.account_groups.iter().map(|group| html! {
                                <option value={group.id.clone()} selected={*account_group_id == group.id}>{ format!("{} ({} 个账号)", group.name, group.account_names.len()) }</option>
                            }) }
                        </select>
                    </label>
                }
                <span class={classes!("text-xs", "text-[var(--muted)]")}>
                    { current_route_summary }
                </span>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]")}>
                <span>{ format!("剩余 {}", format_number_i64(key_item.remaining_billable)) }</span>
                <span>{ format!("输入 {}", format_number_u64(key_item.usage_input_uncached_tokens)) }</span>
                <span>{ format!("缓存 {}", format_number_u64(key_item.usage_input_cached_tokens)) }</span>
                <span>{ format!("输出 {}", format_number_u64(key_item.usage_output_tokens)) }</span>
                <span>{ format!(
                    "并发 {}",
                    key_item.request_max_concurrency.map(|value| value.to_string()).unwrap_or_else(|| "∞".to_string())
                ) }</span>
                <span>{ format!(
                    "间隔 {}ms",
                    key_item.request_min_start_interval_ms.map(|value| value.to_string()).unwrap_or_else(|| "∞".to_string())
                ) }</span>
                <span>{ format!("Credit {}", key_credit_display(&key_item)) }</span>
                if key_item.usage_credit_missing_events > 0 {
                    <span>{ format!("partial {}", key_item.usage_credit_missing_events) }</span>
                }
            </div>

            if let Some(feedback) = (*feedback).clone() {
                <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct AccountGroupEditorCardProps {
    group_item: AdminAccountGroupView,
    accounts: Vec<AccountSummaryView>,
    on_changed: Callback<()>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(AccountGroupEditorCard)]
fn account_group_editor_card(props: &AccountGroupEditorCardProps) -> Html {
    let name = use_state(|| props.group_item.name.clone());
    let account_names =
        use_state(|| sanitize_auto_account_names(&props.group_item.account_names, &props.accounts));
    let expanded = use_state(|| false);
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let group_item = props.group_item.clone();
        let accounts = props.accounts.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        use_effect_with((props.group_item.clone(), props.accounts.clone()), move |_| {
            name.set(group_item.name.clone());
            account_names.set(sanitize_auto_account_names(&group_item.account_names, &accounts));
            || ()
        });
    }

    let on_toggle_account = {
        let account_names = account_names.clone();
        Callback::from(move |account_name: String| {
            let mut names = (*account_names).clone();
            if let Some(index) = names.iter().position(|name| name == &account_name) {
                names.remove(index);
            } else {
                names.push(account_name);
                names.sort();
            }
            account_names.set(names);
        })
    };

    let on_save = {
        let group_id = props.group_item.id.clone();
        let name = name.clone();
        let account_names = account_names.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        let on_changed = props.on_changed.clone();
        Callback::from(move |_| {
            if *saving {
                return;
            }
            let group_id = group_id.clone();
            let name_value = (*name).trim().to_string();
            let account_names_value = (*account_names).clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match patch_admin_llm_gateway_account_group(
                    &group_id,
                    PatchAdminAccountGroupInput {
                        name: Some(&name_value),
                        account_names: Some(account_names_value.as_slice()),
                    },
                )
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("已保存".to_string()));
                        on_flash.emit((format!("已保存账号组 `{}`", name_value), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存账号组失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let group_id = props.group_item.id.clone();
        let group_name = props.group_item.name.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        let saving = saving.clone();
        Callback::from(move |_| {
            let Some(window) = window() else {
                return;
            };
            if !window
                .confirm_with_message("确认删除这个账号组？")
                .ok()
                .unwrap_or(false)
            {
                return;
            }
            let group_id = group_id.clone();
            let group_name = group_name.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            let saving = saving.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_account_group(&group_id).await {
                    Ok(_) => {
                        on_flash.emit((format!("已删除账号组 `{}`", group_name), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        on_flash.emit((format!("删除账号组失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <h3 class={classes!("m-0", "text-base", "font-bold")}>{ props.group_item.name.clone() }</h3>
                    <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                        {
                            if props.group_item.account_names.is_empty() {
                                "没有成员账号".to_string()
                            } else {
                                format!("成员: {}", props.group_item.account_names.join(", "))
                            }
                        }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("{} 个账号", props.group_item.account_names.len()) }</span>
                    <button
                        type="button"
                        class={classes!("btn-terminal")}
                        onclick={{
                            let expanded = expanded.clone();
                            Callback::from(move |_| expanded.set(!*expanded))
                        }}
                    >
                        { if *expanded { "收起 ▲" } else { "展开 ▼" } }
                    </button>
                    <button class={classes!("btn-terminal", "text-red-600", "dark:text-red-300")} onclick={on_delete} disabled={*saving}>
                        { "删除" }
                    </button>
                </div>
            </div>

            if *expanded {
                <label class={classes!("mt-3", "block", "text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "组名" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    name.set(target.value());
                                }
                            })
                        }}
                    />
                </label>

                <div class={classes!("mt-3", "space-y-2")}>
                    <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "成员账号" }</div>
                    <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                        { for props.accounts.iter().map(|account| {
                            let checked = account_names.iter().any(|name| name == &account.name);
                            let account_name = account.name.clone();
                            let on_toggle_account = on_toggle_account.clone();
                            html! {
                                <label class={classes!(
                                    "flex", "cursor-pointer", "items-center", "gap-3", "rounded-lg", "border", "px-3", "py-2.5",
                                    if checked {
                                        "border-sky-500/30 bg-sky-500/8"
                                    } else {
                                        "border-[var(--border)] bg-[var(--surface-alt)]"
                                    }
                                )}>
                                    <input
                                        type="checkbox"
                                        checked={checked}
                                        onchange={Callback::from(move |_| on_toggle_account.emit(account_name.clone()))}
                                    />
                                    <div class={classes!("min-w-0", "flex-1")}>
                                        <div class={classes!("font-semibold", "text-[var(--text)]")}>{ account.name.clone() }</div>
                                        <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                            { format!(
                                                "5h {} / wk {}",
                                                account.primary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string()),
                                                account.secondary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string())
                                            ) }
                                        </div>
                                    </div>
                                </label>
                            }
                        }) }
                    </div>
                </div>

                <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3")}>
                    <span class={classes!("text-xs", "text-[var(--muted)]")}>
                        { format!("当前成员: {}", if account_names.is_empty() { "无".to_string() } else { account_names.join(", ") }) }
                    </span>
                    <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save} disabled={*saving}>
                        { if *saving { "保存中..." } else { "保存账号组" } }
                    </button>
                </div>

                if let Some(feedback) = (*feedback).clone() {
                    <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
                }
            }
        </article>
    }
}

#[derive(Properties, PartialEq)]
struct ProxyConfigEditorCardProps {
    proxy_config: AdminUpstreamProxyConfigView,
    on_changed: Callback<()>,
    on_copy: Callback<(String, String)>,
    on_flash: Callback<(String, bool)>,
}

#[function_component(ProxyConfigEditorCard)]
fn proxy_config_editor_card(props: &ProxyConfigEditorCardProps) -> Html {
    let proxy_config = props.proxy_config.clone();
    let name = use_state(|| proxy_config.name.clone());
    let proxy_url = use_state(|| proxy_config.proxy_url.clone());
    let proxy_username = use_state(|| proxy_config.proxy_username.clone().unwrap_or_default());
    let proxy_password = use_state(|| proxy_config.proxy_password.clone().unwrap_or_default());
    let status = use_state(|| proxy_config.status.clone());
    let saving = use_state(|| false);
    let checking = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        let proxy_config = props.proxy_config.clone();
        let name = name.clone();
        let proxy_url = proxy_url.clone();
        let proxy_username = proxy_username.clone();
        let proxy_password = proxy_password.clone();
        let status = status.clone();
        use_effect_with(props.proxy_config.clone(), move |_| {
            name.set(proxy_config.name.clone());
            proxy_url.set(proxy_config.proxy_url.clone());
            proxy_username.set(proxy_config.proxy_username.clone().unwrap_or_default());
            proxy_password.set(proxy_config.proxy_password.clone().unwrap_or_default());
            status.set(proxy_config.status.clone());
            || ()
        });
    }

    let on_save = {
        let proxy_id = proxy_config.id.clone();
        let name = name.clone();
        let proxy_url = proxy_url.clone();
        let proxy_username = proxy_username.clone();
        let proxy_password = proxy_password.clone();
        let status = status.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |_| {
            let proxy_id = proxy_id.clone();
            let input = PatchAdminUpstreamProxyConfigInput {
                name: Some((*name).trim().to_string()),
                proxy_url: Some((*proxy_url).trim().to_string()),
                proxy_username: {
                    let value = (*proxy_username).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                proxy_password: {
                    let value = (*proxy_password).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                status: Some((*status).trim().to_string()),
            };
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match patch_admin_llm_gateway_proxy_config(&proxy_id, &input).await {
                    Ok(_) => {
                        feedback.set(Some("Saved.".to_string()));
                        on_flash.emit(("已保存代理配置".to_string(), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("保存代理配置失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_delete = {
        let proxy_id = proxy_config.id.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |_| {
            let proxy_id = proxy_id.clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_proxy_config(&proxy_id).await {
                    Ok(_) => {
                        on_flash.emit(("已删除代理配置".to_string(), false));
                        on_changed.emit(());
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((format!("删除代理配置失败\n{err}"), true));
                    },
                }
                saving.set(false);
            });
        })
    };

    let on_check_provider = {
        let proxy_id = proxy_config.id.clone();
        let checking = checking.clone();
        let feedback = feedback.clone();
        let on_flash = props.on_flash.clone();
        Callback::from(move |provider_type: String| {
            let proxy_id = proxy_id.clone();
            let checking = checking.clone();
            let feedback = feedback.clone();
            let on_flash = on_flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if *checking {
                    return;
                }
                checking.set(true);
                match check_admin_llm_gateway_proxy_config(&proxy_id, &provider_type).await {
                    Ok(result) => {
                        let message = format_proxy_check_message(&result);
                        feedback.set(Some(if result.ok {
                            format!("{} 检查完成", provider_type.to_uppercase())
                        } else {
                            format!("{} 检查失败", provider_type.to_uppercase())
                        }));
                        on_flash.emit((message, !result.ok));
                    },
                    Err(err) => {
                        feedback.set(Some(err.clone()));
                        on_flash.emit((
                            format!("{} 代理检查失败\n{err}", provider_type.to_uppercase()),
                            true,
                        ));
                    },
                }
                checking.set(false);
            });
        })
    };

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ props.proxy_config.name.clone() }</h3>
                        <span class={classes!("inline-flex", "items-center", "rounded-full", "px-2.5", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.16em]",
                            if props.proxy_config.status == "active" { "bg-emerald-500/12 text-emerald-700 dark:text-emerald-200" } else { "bg-slate-500/12 text-slate-700 dark:text-slate-200" })}>
                            { props.proxy_config.status.clone() }
                        </span>
                    </div>
                    <p class={classes!("mt-2", "mb-0", "text-xs", "font-mono", "text-[var(--muted)]")}>
                        { format!("created {} · updated {}", format_ms(props.proxy_config.created_at), format_ms(props.proxy_config.updated_at)) }
                    </p>
                </div>
                <div class={classes!("flex", "items-center", "gap-2")}>
                    { copy_icon_button(&props.proxy_config.proxy_url, &props.on_copy) }
                </div>
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2")}>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Name" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={(*name).clone()}
                        oninput={{
                            let name = name.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    name.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Status" }</span>
                    <select
                        key={format!("proxy-config-status-{}-{}", proxy_config.id, (*status).clone())}
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={(*status).clone()}
                        onchange={{
                            let status = status.clone();
                            Callback::from(move |event: Event| {
                                if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                    status.set(target.value());
                                }
                            })
                        }}
                    >
                        <option value="active" selected={*status == "active"}>{ "active" }</option>
                        <option value="disabled" selected={*status == "disabled"}>{ "disabled" }</option>
                    </select>
                </label>
                <label class={classes!("text-sm", "md:col-span-2")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy URL" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "font-mono")}
                        value={(*proxy_url).clone()}
                        oninput={{
                            let proxy_url = proxy_url.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    proxy_url.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy Username" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={(*proxy_username).clone()}
                        oninput={{
                            let proxy_username = proxy_username.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    proxy_username.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
                <label class={classes!("text-sm")}>
                    <span class={classes!("text-[var(--muted)]")}>{ "Proxy Password" }</span>
                    <input
                        type="text"
                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")}
                        value={(*proxy_password).clone()}
                        oninput={{
                            let proxy_password = proxy_password.clone();
                            Callback::from(move |event: InputEvent| {
                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                    proxy_password.set(target.value());
                                }
                            })
                        }}
                    />
                </label>
            </div>

            <div class={classes!("mt-4", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-3")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                    <div class={classes!("min-w-0")}>
                        <div class={classes!("text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>{ "Visible Credentials" }</div>
                        <code class={classes!("mt-2", "block", "break-all", "font-mono", "text-xs")}>
                            { format!("{} @ {}", props.proxy_config.proxy_username.clone().unwrap_or_else(|| "-".to_string()), props.proxy_config.proxy_url.clone()) }
                        </code>
                        if let Some(password) = props.proxy_config.proxy_password.as_deref() {
                            <code class={classes!("mt-1", "block", "break-all", "font-mono", "text-xs")}>
                                { password }
                            </code>
                        }
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        { copy_icon_button(&props.proxy_config.proxy_url, &props.on_copy) }
                        if let Some(username) = props.proxy_config.proxy_username.as_deref() {
                            { copy_icon_button(username, &props.on_copy) }
                        }
                        if let Some(password) = props.proxy_config.proxy_password.as_deref() {
                            { copy_icon_button(password, &props.on_copy) }
                        }
                    </div>
                </div>
            </div>

            <div class={classes!("mt-4", "flex", "items-center", "gap-2", "flex-wrap")}>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit("codex".to_string()))
                    }}
                    disabled={*saving || *checking}
                >
                    { if *checking { "检查中..." } else { "检查 Codex" } }
                </button>
                <button
                    class={classes!("btn-terminal")}
                    onclick={{
                        let on_check_provider = on_check_provider.clone();
                        Callback::from(move |_| on_check_provider.emit("kiro".to_string()))
                    }}
                    disabled={*saving || *checking}
                >
                    { if *checking { "检查中..." } else { "检查 Kiro" } }
                </button>
                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_save.clone()} disabled={*saving}>
                    { if *saving { "保存中..." } else { "保存" } }
                </button>
                <button class={classes!("btn-terminal", "text-red-600", "dark:text-red-400")} onclick={on_delete} disabled={*saving}>
                    { "删除" }
                </button>
            </div>

            if let Some(feedback) = (*feedback).clone() {
                <p class={classes!("mt-2", "m-0", "text-xs", "text-[var(--muted)]")}>{ feedback }</p>
            }
        </article>
    }
}

#[function_component(AdminLlmGatewayPage)]
pub fn admin_llm_gateway_page() -> Html {
    let config = use_state(|| None::<LlmGatewayRuntimeConfig>);
    let keys = use_state(Vec::<AdminLlmGatewayKeyView>::new);
    let account_groups = use_state(Vec::<AdminAccountGroupView>::new);
    let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventView>::new);
    let usage_total = use_state(|| 0_usize);
    let usage_page = use_state(|| 1_usize);
    let usage_current_rpm = use_state(|| 0_u32);
    let usage_current_in_flight = use_state(|| 0_u32);
    let usage_loading = use_state(|| false);
    let usage_error = use_state(|| None::<String>);
    let usage_key_filter = use_state(String::new);
    let token_requests = use_state(Vec::<AdminLlmGatewayTokenRequestView>::new);
    let token_request_total = use_state(|| 0_usize);
    let token_request_page = use_state(|| 1_usize);
    let token_request_loading = use_state(|| false);
    let token_request_status_filter = use_state(String::new);
    let token_request_action_inflight = use_state(HashSet::<String>::new);
    let account_contribution_requests =
        use_state(Vec::<AdminLlmGatewayAccountContributionRequestView>::new);
    let account_contribution_request_total = use_state(|| 0_usize);
    let account_contribution_request_page = use_state(|| 1_usize);
    let account_contribution_request_loading = use_state(|| false);
    let account_contribution_request_status_filter = use_state(String::new);
    let account_contribution_request_action_inflight = use_state(HashSet::<String>::new);
    let sponsor_requests = use_state(Vec::<AdminLlmGatewaySponsorRequestView>::new);
    let sponsor_request_total = use_state(|| 0_usize);
    let sponsor_request_page = use_state(|| 1_usize);
    let sponsor_request_loading = use_state(|| false);
    let sponsor_request_status_filter = use_state(String::new);
    let sponsor_request_action_inflight = use_state(HashSet::<String>::new);
    let selected_usage_event = use_state(|| None::<AdminLlmGatewayUsageEventDetailView>);
    let usage_detail_loading = use_state(|| false);
    let usage_scroll_top_ref = use_node_ref();
    let usage_scroll_bottom_ref = use_node_ref();
    let usage_scroll_width = use_state(|| 1_i32);
    let loading = use_state(|| true);
    let load_error = use_state(|| None::<String>);
    let ttl_input = use_state(|| "60".to_string());
    let max_request_body_input = use_state(|| (8 * 1024 * 1024_u64).to_string());
    let account_failure_retry_limit_input = use_state(|| "3".to_string());
    let codex_refresh_min_input = use_state(|| "240".to_string());
    let codex_refresh_max_input = use_state(|| "300".to_string());
    let codex_account_jitter_max_input = use_state(|| "10".to_string());
    let kiro_refresh_min_input = use_state(|| "240".to_string());
    let kiro_refresh_max_input = use_state(|| "300".to_string());
    let kiro_account_jitter_max_input = use_state(|| "10".to_string());
    let usage_flush_batch_size_input = use_state(|| "256".to_string());
    let usage_flush_interval_input = use_state(|| "15".to_string());
    let usage_flush_max_buffer_bytes_input = use_state(|| (8 * 1024 * 1024_u64).to_string());
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let proxy_bindings = use_state(Vec::<AdminUpstreamProxyBindingView>::new);
    let create_proxy_name = use_state(|| "shared-upstream".to_string());
    let create_proxy_url = use_state(|| "http://127.0.0.1:11111".to_string());
    let create_proxy_username = use_state(String::new);
    let create_proxy_password = use_state(String::new);
    let creating_proxy = use_state(|| false);
    let codex_proxy_binding_input = use_state(String::new);
    let kiro_proxy_binding_input = use_state(String::new);
    let saving_proxy_binding_provider = use_state(|| None::<String>);
    let migrating_legacy_kiro_proxy = use_state(|| false);
    let saving_runtime_config = use_state(|| false);
    let create_name = use_state(String::new);
    let create_quota = use_state(|| "100000".to_string());
    let create_public = use_state(|| true);
    let create_request_max_concurrency = use_state(String::new);
    let create_request_min_start_interval_ms = use_state(String::new);
    let creating = use_state(|| false);
    let create_account_group_name = use_state(String::new);
    let create_account_group_account_names = use_state(Vec::<String>::new);
    let creating_account_group = use_state(|| false);
    let account_group_form_expanded = use_state(|| false);
    let refreshing_key_id = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let flash = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(message, is_error): (String, bool)| {
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
    let open_usage_detail = {
        let selected_usage_event = selected_usage_event.clone();
        let usage_detail_loading = usage_detail_loading.clone();
        let flash = flash.clone();
        Callback::from(move |event_id: String| {
            let selected_usage_event = selected_usage_event.clone();
            let usage_detail_loading = usage_detail_loading.clone();
            let flash = flash.clone();
            selected_usage_event.set(None);
            usage_detail_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_usage_event_detail(&event_id).await {
                    Ok(detail) => selected_usage_event.set(Some(detail)),
                    Err(err) => flash.emit((err, true)),
                }
                usage_detail_loading.set(false);
            });
        })
    };
    let accounts = use_state(Vec::<AccountSummaryView>::new);
    let import_name = use_state(String::new);
    let import_id_token = use_state(String::new);
    let import_access_token = use_state(String::new);
    let import_refresh_token = use_state(String::new);
    let import_account_id = use_state(String::new);
    let importing = use_state(|| false);
    let account_action_inflight = use_state(HashSet::<String>::new);
    let account_proxy_inputs = use_state(BTreeMap::<String, String>::new);
    let account_request_max_inputs = use_state(BTreeMap::<String, String>::new);
    let account_request_min_inputs = use_state(BTreeMap::<String, String>::new);
    let show_import_form = use_state(|| false);
    let active_tab = use_state(|| TAB_OVERVIEW.to_string());
    let on_tab_click = {
        let active_tab = active_tab.clone();
        Callback::from(move |tab: String| active_tab.set(tab))
    };

    // Usage events are fetched independently so paging and key filters do not
    // need to re-fetch the rest of the admin page chrome.
    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_total = usage_total.clone();
        let usage_page = usage_page.clone();
        let usage_current_rpm = usage_current_rpm.clone();
        let usage_current_in_flight = usage_current_in_flight.clone();
        let usage_loading = usage_loading.clone();
        let usage_error = usage_error.clone();
        let usage_key_filter = usage_key_filter.clone();
        Callback::from(move |(requested_page, override_key_id): (Option<usize>, Option<String>)| {
            let usage_events = usage_events.clone();
            let usage_total = usage_total.clone();
            let usage_page = usage_page.clone();
            let usage_current_rpm = usage_current_rpm.clone();
            let usage_current_in_flight = usage_current_in_flight.clone();
            let usage_loading = usage_loading.clone();
            let usage_error = usage_error.clone();
            let usage_key_filter = usage_key_filter.clone();
            let page = requested_page.unwrap_or(*usage_page).max(1);
            let selected_key_id = override_key_id.unwrap_or_else(|| (*usage_key_filter).clone());
            usage_loading.set(true);
            usage_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayUsageEventsQuery {
                    key_id: (!selected_key_id.is_empty()).then_some(selected_key_id),
                    limit: Some(USAGE_PAGE_SIZE),
                    offset: Some((page - 1) * USAGE_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_usage_events(&query).await {
                    Ok(resp) => {
                        usage_total.set(resp.total);
                        usage_current_rpm.set(resp.current_rpm);
                        usage_current_in_flight.set(resp.current_in_flight);
                        usage_events.set(resp.events);
                        usage_page.set(page);
                    },
                    Err(err) => {
                        usage_current_rpm.set(0);
                        usage_current_in_flight.set(0);
                        usage_error.set(Some(err));
                    },
                }
                usage_loading.set(false);
            });
        })
    };

    let reload_token_requests = {
        let token_requests = token_requests.clone();
        let token_request_total = token_request_total.clone();
        let token_request_page = token_request_page.clone();
        let token_request_loading = token_request_loading.clone();
        let token_request_status_filter = token_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let token_requests = token_requests.clone();
            let token_request_total = token_request_total.clone();
            let token_request_page = token_request_page.clone();
            let token_request_loading = token_request_loading.clone();
            let token_request_status_filter = token_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*token_request_page).max(1);
            let selected_status =
                override_status.unwrap_or_else(|| (*token_request_status_filter).clone());
            token_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayTokenRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(TOKEN_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * TOKEN_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_token_requests(&query).await {
                    Ok(resp) => {
                        token_request_total.set(resp.total);
                        token_requests.set(resp.requests);
                        token_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                token_request_loading.set(false);
            });
        })
    };

    let reload_account_contribution_requests = {
        let account_contribution_requests = account_contribution_requests.clone();
        let account_contribution_request_total = account_contribution_request_total.clone();
        let account_contribution_request_page = account_contribution_request_page.clone();
        let account_contribution_request_loading = account_contribution_request_loading.clone();
        let account_contribution_request_status_filter =
            account_contribution_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let account_contribution_requests = account_contribution_requests.clone();
            let account_contribution_request_total = account_contribution_request_total.clone();
            let account_contribution_request_page = account_contribution_request_page.clone();
            let account_contribution_request_loading = account_contribution_request_loading.clone();
            let account_contribution_request_status_filter =
                account_contribution_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page
                .unwrap_or(*account_contribution_request_page)
                .max(1);
            let selected_status = override_status
                .unwrap_or_else(|| (*account_contribution_request_status_filter).clone());
            account_contribution_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayAccountContributionRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_account_contribution_requests(&query).await {
                    Ok(resp) => {
                        account_contribution_request_total.set(resp.total);
                        account_contribution_requests.set(resp.requests);
                        account_contribution_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                account_contribution_request_loading.set(false);
            });
        })
    };

    let reload_sponsor_requests = {
        let sponsor_requests = sponsor_requests.clone();
        let sponsor_request_total = sponsor_request_total.clone();
        let sponsor_request_page = sponsor_request_page.clone();
        let sponsor_request_loading = sponsor_request_loading.clone();
        let sponsor_request_status_filter = sponsor_request_status_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_status): (Option<usize>, Option<String>)| {
            let sponsor_requests = sponsor_requests.clone();
            let sponsor_request_total = sponsor_request_total.clone();
            let sponsor_request_page = sponsor_request_page.clone();
            let sponsor_request_loading = sponsor_request_loading.clone();
            let sponsor_request_status_filter = sponsor_request_status_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*sponsor_request_page).max(1);
            let selected_status =
                override_status.unwrap_or_else(|| (*sponsor_request_status_filter).clone());
            sponsor_request_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewaySponsorRequestsQuery {
                    status: (!selected_status.is_empty()).then_some(selected_status),
                    limit: Some(SPONSOR_REQUEST_PAGE_SIZE),
                    offset: Some((page - 1) * SPONSOR_REQUEST_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_sponsor_requests(&query).await {
                    Ok(resp) => {
                        sponsor_request_total.set(resp.total);
                        sponsor_requests.set(resp.requests);
                        sponsor_request_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                sponsor_request_loading.set(false);
            });
        })
    };

    // This reload keeps the inventory, runtime config, and the current usage
    // page in sync after any admin write operation.
    let reload = {
        let config = config.clone();
        let keys = keys.clone();
        let proxy_configs = proxy_configs.clone();
        let proxy_bindings = proxy_bindings.clone();
        let account_groups = account_groups.clone();
        let loading = loading.clone();
        let load_error = load_error.clone();
        let ttl_input = ttl_input.clone();
        let max_request_body_input = max_request_body_input.clone();
        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
        let codex_refresh_min_input = codex_refresh_min_input.clone();
        let codex_refresh_max_input = codex_refresh_max_input.clone();
        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
        let usage_flush_interval_input = usage_flush_interval_input.clone();
        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
        let usage_page = usage_page.clone();
        let usage_key_filter = usage_key_filter.clone();
        let accounts = accounts.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let account_request_max_inputs = account_request_max_inputs.clone();
        let account_request_min_inputs = account_request_min_inputs.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |_| {
            let config = config.clone();
            let keys = keys.clone();
            let proxy_configs = proxy_configs.clone();
            let proxy_bindings = proxy_bindings.clone();
            let account_groups = account_groups.clone();
            let loading = loading.clone();
            let load_error = load_error.clone();
            let ttl_input = ttl_input.clone();
            let max_request_body_input = max_request_body_input.clone();
            let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
            let codex_refresh_min_input = codex_refresh_min_input.clone();
            let codex_refresh_max_input = codex_refresh_max_input.clone();
            let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
            let kiro_refresh_min_input = kiro_refresh_min_input.clone();
            let kiro_refresh_max_input = kiro_refresh_max_input.clone();
            let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
            let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
            let usage_flush_interval_input = usage_flush_interval_input.clone();
            let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
            let codex_proxy_binding_input = codex_proxy_binding_input.clone();
            let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
            let usage_page = usage_page.clone();
            let usage_key_filter = usage_key_filter.clone();
            let accounts = accounts.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let account_request_max_inputs = account_request_max_inputs.clone();
            let account_request_min_inputs = account_request_min_inputs.clone();
            let reload_usage = reload_usage.clone();
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let current_key_filter = (*usage_key_filter).clone();
                let current_page = (*usage_page).max(1);
                let result = async {
                    let (
                        cfg_result,
                        keys_result,
                        account_groups_result,
                        proxy_configs_result,
                        proxy_bindings_result,
                        accounts_result,
                    ) = futures::join!(
                        fetch_admin_llm_gateway_config(),
                        fetch_admin_llm_gateway_keys(),
                        fetch_admin_llm_gateway_account_groups(),
                        fetch_admin_llm_gateway_proxy_configs(),
                        fetch_admin_llm_gateway_proxy_bindings(),
                        fetch_admin_llm_gateway_accounts(),
                    );
                    let cfg = cfg_result?;
                    let keys_resp = keys_result?;
                    let account_groups_resp = account_groups_result?;
                    let proxy_configs_resp = proxy_configs_result?;
                    let proxy_bindings_resp = proxy_bindings_result?;
                    let accounts_resp = accounts_result?;
                    let effective_key_filter = if current_key_filter.is_empty()
                        || keys_resp
                            .keys
                            .iter()
                            .any(|item| item.id == current_key_filter)
                    {
                        current_key_filter
                    } else {
                        String::new()
                    };
                    Ok::<_, String>((
                        cfg,
                        keys_resp.keys,
                        account_groups_resp.groups,
                        proxy_configs_resp.proxy_configs,
                        proxy_bindings_resp.bindings,
                        effective_key_filter,
                        accounts_resp,
                    ))
                }
                .await;

                match result {
                    Ok((
                        cfg,
                        key_items,
                        account_group_items,
                        proxy_config_items,
                        proxy_binding_items,
                        effective_key_filter,
                        accounts_resp,
                    )) => {
                        let usage_filter_for_reload = effective_key_filter.clone();
                        ttl_input.set(cfg.auth_cache_ttl_seconds.to_string());
                        max_request_body_input.set(cfg.max_request_body_bytes.to_string());
                        account_failure_retry_limit_input
                            .set(cfg.account_failure_retry_limit.to_string());
                        codex_refresh_min_input
                            .set(cfg.codex_status_refresh_min_interval_seconds.to_string());
                        codex_refresh_max_input
                            .set(cfg.codex_status_refresh_max_interval_seconds.to_string());
                        codex_account_jitter_max_input
                            .set(cfg.codex_status_account_jitter_max_seconds.to_string());
                        kiro_refresh_min_input
                            .set(cfg.kiro_status_refresh_min_interval_seconds.to_string());
                        kiro_refresh_max_input
                            .set(cfg.kiro_status_refresh_max_interval_seconds.to_string());
                        kiro_account_jitter_max_input
                            .set(cfg.kiro_status_account_jitter_max_seconds.to_string());
                        usage_flush_batch_size_input
                            .set(cfg.usage_event_flush_batch_size.to_string());
                        usage_flush_interval_input
                            .set(cfg.usage_event_flush_interval_seconds.to_string());
                        usage_flush_max_buffer_bytes_input
                            .set(cfg.usage_event_flush_max_buffer_bytes.to_string());
                        config.set(Some(cfg));
                        keys.set(key_items);
                        account_groups.set(account_group_items);
                        let codex_bound = proxy_binding_items
                            .iter()
                            .find(|item| item.provider_type == "codex")
                            .and_then(|item| item.bound_proxy_config_id.clone())
                            .unwrap_or_default();
                        let kiro_bound = proxy_binding_items
                            .iter()
                            .find(|item| item.provider_type == "kiro")
                            .and_then(|item| item.bound_proxy_config_id.clone())
                            .unwrap_or_default();
                        proxy_configs.set(proxy_config_items);
                        proxy_bindings.set(proxy_binding_items);
                        codex_proxy_binding_input.set(codex_bound);
                        kiro_proxy_binding_input.set(kiro_bound);
                        usage_key_filter.set(effective_key_filter);
                        let next_proxy_inputs = accounts_resp
                            .accounts
                            .iter()
                            .map(|account| {
                                (account.name.clone(), account_proxy_select_value(account))
                            })
                            .collect::<BTreeMap<_, _>>();
                        let next_request_max_inputs = accounts_resp
                            .accounts
                            .iter()
                            .map(|account| {
                                (
                                    account.name.clone(),
                                    account
                                        .request_max_concurrency
                                        .map(|value| value.to_string())
                                        .unwrap_or_default(),
                                )
                            })
                            .collect::<BTreeMap<_, _>>();
                        let next_request_min_inputs = accounts_resp
                            .accounts
                            .iter()
                            .map(|account| {
                                (
                                    account.name.clone(),
                                    account
                                        .request_min_start_interval_ms
                                        .map(|value| value.to_string())
                                        .unwrap_or_default(),
                                )
                            })
                            .collect::<BTreeMap<_, _>>();
                        accounts.set(accounts_resp.accounts);
                        account_proxy_inputs.set(next_proxy_inputs);
                        account_request_max_inputs.set(next_request_max_inputs);
                        account_request_min_inputs.set(next_request_min_inputs);
                        load_error.set(None);
                        reload_usage.emit((Some(current_page), Some(usage_filter_for_reload)));
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    {
        let reload = reload.clone();
        let reload_token_requests = reload_token_requests.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        use_effect_with((), move |_| {
            reload.emit(());
            reload_token_requests.emit((Some(1), Some(String::new())));
            reload_account_contribution_requests.emit((Some(1), Some(String::new())));
            reload_sponsor_requests.emit((Some(1), Some(String::new())));
            || ()
        });
    }

    let on_save_runtime_config = {
        let config = config.clone();
        let ttl_input = ttl_input.clone();
        let max_request_body_input = max_request_body_input.clone();
        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
        let codex_refresh_min_input = codex_refresh_min_input.clone();
        let codex_refresh_max_input = codex_refresh_max_input.clone();
        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
        let usage_flush_interval_input = usage_flush_interval_input.clone();
        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
        let saving_runtime_config = saving_runtime_config.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let config = config.clone();
            let ttl = (*ttl_input).trim().parse::<u64>();
            let max_request_body_bytes = (*max_request_body_input).trim().parse::<u64>();
            let account_failure_retry_limit =
                (*account_failure_retry_limit_input).trim().parse::<u64>();
            let codex_status_refresh_min_interval_seconds =
                (*codex_refresh_min_input).trim().parse::<u64>();
            let codex_status_refresh_max_interval_seconds =
                (*codex_refresh_max_input).trim().parse::<u64>();
            let codex_status_account_jitter_max_seconds =
                (*codex_account_jitter_max_input).trim().parse::<u64>();
            let kiro_status_refresh_min_interval_seconds =
                (*kiro_refresh_min_input).trim().parse::<u64>();
            let kiro_status_refresh_max_interval_seconds =
                (*kiro_refresh_max_input).trim().parse::<u64>();
            let kiro_status_account_jitter_max_seconds =
                (*kiro_account_jitter_max_input).trim().parse::<u64>();
            let usage_event_flush_batch_size =
                (*usage_flush_batch_size_input).trim().parse::<u64>();
            let usage_event_flush_interval_seconds =
                (*usage_flush_interval_input).trim().parse::<u64>();
            let usage_event_flush_max_buffer_bytes =
                (*usage_flush_max_buffer_bytes_input).trim().parse::<u64>();
            let saving_runtime_config = saving_runtime_config.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(ttl) = ttl else {
                    load_error.set(Some("TTL 必须是正整数".to_string()));
                    return;
                };
                let Ok(max_request_body_bytes) = max_request_body_bytes else {
                    load_error.set(Some("请求体上限必须是正整数".to_string()));
                    return;
                };
                let Ok(account_failure_retry_limit) = account_failure_retry_limit else {
                    load_error.set(Some("账号失败重试次数必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_status_refresh_min_interval_seconds) =
                    codex_status_refresh_min_interval_seconds
                else {
                    load_error.set(Some("Codex 最小轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_status_refresh_max_interval_seconds) =
                    codex_status_refresh_max_interval_seconds
                else {
                    load_error.set(Some("Codex 最大轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(codex_status_account_jitter_max_seconds) =
                    codex_status_account_jitter_max_seconds
                else {
                    load_error.set(Some("Codex 单账号抖动上限必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_refresh_min_interval_seconds) =
                    kiro_status_refresh_min_interval_seconds
                else {
                    load_error.set(Some("Kiro 最小轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_refresh_max_interval_seconds) =
                    kiro_status_refresh_max_interval_seconds
                else {
                    load_error.set(Some("Kiro 最大轮询间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(kiro_status_account_jitter_max_seconds) =
                    kiro_status_account_jitter_max_seconds
                else {
                    load_error.set(Some("Kiro 单账号抖动上限必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_batch_size) = usage_event_flush_batch_size else {
                    load_error.set(Some("usage flush 批大小必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_interval_seconds) = usage_event_flush_interval_seconds
                else {
                    load_error.set(Some("usage flush 间隔必须是非负整数".to_string()));
                    return;
                };
                let Ok(usage_event_flush_max_buffer_bytes) = usage_event_flush_max_buffer_bytes
                else {
                    load_error.set(Some("usage flush 缓冲上限必须是非负整数".to_string()));
                    return;
                };
                let runtime_config = LlmGatewayRuntimeConfig {
                    auth_cache_ttl_seconds: ttl,
                    max_request_body_bytes,
                    account_failure_retry_limit,
                    codex_status_refresh_min_interval_seconds,
                    codex_status_refresh_max_interval_seconds,
                    codex_status_account_jitter_max_seconds,
                    kiro_status_refresh_min_interval_seconds,
                    kiro_status_refresh_max_interval_seconds,
                    kiro_status_account_jitter_max_seconds,
                    usage_event_flush_batch_size,
                    usage_event_flush_interval_seconds,
                    usage_event_flush_max_buffer_bytes,
                    kiro_cache_kmodels_json: config
                        .as_ref()
                        .map(|current| current.kiro_cache_kmodels_json.clone())
                        .unwrap_or_default(),
                    kiro_billable_model_multipliers_json: config
                        .as_ref()
                        .map(|current| current.kiro_billable_model_multipliers_json.clone())
                        .unwrap_or_else(|| "{}".to_string()),
                    kiro_cache_policy_json: config
                        .as_ref()
                        .map(|current| current.kiro_cache_policy_json.clone())
                        .unwrap_or_default(),
                    kiro_prefix_cache_mode: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_mode.clone())
                        .unwrap_or_else(|| "prefix_tree".to_string()),
                    kiro_prefix_cache_max_tokens: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_max_tokens)
                        .unwrap_or(4_000_000),
                    kiro_prefix_cache_entry_ttl_seconds: config
                        .as_ref()
                        .map(|current| current.kiro_prefix_cache_entry_ttl_seconds)
                        .unwrap_or(21_600),
                    kiro_conversation_anchor_max_entries: config
                        .as_ref()
                        .map(|current| current.kiro_conversation_anchor_max_entries)
                        .unwrap_or(20_000),
                    kiro_conversation_anchor_ttl_seconds: config
                        .as_ref()
                        .map(|current| current.kiro_conversation_anchor_ttl_seconds)
                        .unwrap_or(86_400),
                };
                saving_runtime_config.set(true);
                match update_admin_llm_gateway_config(&runtime_config).await {
                    Ok(_) => {
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_runtime_config.set(false);
            });
        })
    };

    let on_create_proxy_config = {
        let create_proxy_name = create_proxy_name.clone();
        let create_proxy_url = create_proxy_url.clone();
        let create_proxy_username = create_proxy_username.clone();
        let create_proxy_password = create_proxy_password.clone();
        let creating_proxy = creating_proxy.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let input = CreateAdminUpstreamProxyConfigInput {
                name: (*create_proxy_name).trim().to_string(),
                proxy_url: (*create_proxy_url).trim().to_string(),
                proxy_username: {
                    let value = (*create_proxy_username).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
                proxy_password: {
                    let value = (*create_proxy_password).trim().to_string();
                    if value.is_empty() {
                        None
                    } else {
                        Some(value)
                    }
                },
            };
            let create_proxy_name = create_proxy_name.clone();
            let create_proxy_username = create_proxy_username.clone();
            let create_proxy_password = create_proxy_password.clone();
            let creating_proxy = creating_proxy.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                creating_proxy.set(true);
                match create_admin_llm_gateway_proxy_config(&input).await {
                    Ok(_) => {
                        create_proxy_name.set(String::new());
                        create_proxy_username.set(String::new());
                        create_proxy_password.set(String::new());
                        load_error.set(None);
                        flash.emit(("已创建代理配置".to_string(), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建代理配置失败\n{err}"), true));
                    },
                }
                creating_proxy.set(false);
            });
        })
    };

    let on_save_proxy_binding = {
        let proxy_bindings = proxy_bindings.clone();
        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
        let saving_proxy_binding_provider = saving_proxy_binding_provider.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        Callback::from(move |provider_type: String| {
            let proxy_config_id = match provider_type.as_str() {
                "codex" => (*codex_proxy_binding_input).clone(),
                "kiro" => (*kiro_proxy_binding_input).clone(),
                _ => String::new(),
            };
            let proxy_bindings = proxy_bindings.clone();
            let codex_proxy_binding_input = codex_proxy_binding_input.clone();
            let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
            let saving_proxy_binding_provider = saving_proxy_binding_provider.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving_proxy_binding_provider.set(Some(provider_type.clone()));
                match update_admin_llm_gateway_proxy_binding(
                    &provider_type,
                    if proxy_config_id.trim().is_empty() {
                        None
                    } else {
                        Some(proxy_config_id.trim())
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*proxy_bindings).clone();
                        if let Some(existing) = items
                            .iter_mut()
                            .find(|item| item.provider_type == updated.provider_type)
                        {
                            *existing = updated.clone();
                        } else {
                            items.push(updated.clone());
                            items.sort_by(|left, right| {
                                left.provider_type.cmp(&right.provider_type)
                            });
                        }
                        proxy_bindings.set(items);
                        let bound_value = updated.bound_proxy_config_id.clone().unwrap_or_default();
                        match provider_type.as_str() {
                            "codex" => codex_proxy_binding_input.set(bound_value),
                            "kiro" => kiro_proxy_binding_input.set(bound_value),
                            _ => {},
                        }
                        load_error.set(None);
                        flash.emit((
                            format!("已更新 {} 代理绑定", provider_type.to_uppercase()),
                            false,
                        ));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((
                            format!("保存 {} 代理绑定失败\n{err}", provider_type.to_uppercase()),
                            true,
                        ));
                    },
                }
                saving_proxy_binding_provider.set(None);
            });
        })
    };

    let on_import_legacy_kiro_proxy = {
        let migrating_legacy_kiro_proxy = migrating_legacy_kiro_proxy.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let migrating_legacy_kiro_proxy = migrating_legacy_kiro_proxy.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                migrating_legacy_kiro_proxy.set(true);
                match import_admin_legacy_kiro_proxy_configs().await {
                    Ok(_) => {
                        load_error.set(None);
                        flash.emit(("已导入 legacy Kiro 代理配置".to_string(), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("导入 legacy Kiro 代理配置失败\n{err}"), true));
                    },
                }
                migrating_legacy_kiro_proxy.set(false);
            });
        })
    };

    let on_create = {
        let create_name = create_name.clone();
        let create_quota = create_quota.clone();
        let create_public = create_public.clone();
        let create_request_max_concurrency = create_request_max_concurrency.clone();
        let create_request_min_start_interval_ms = create_request_min_start_interval_ms.clone();
        let creating = creating.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let reload = reload.clone();
        let usage_page = usage_page.clone();
        Callback::from(move |_| {
            let name = (*create_name).trim().to_string();
            let quota = (*create_quota).trim().parse::<u64>();
            let public_visible = *create_public;
            let request_max_concurrency = (*create_request_max_concurrency).trim().to_string();
            let request_min_start_interval_ms =
                (*create_request_min_start_interval_ms).trim().to_string();
            let creating = creating.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let reload = reload.clone();
            let create_name = create_name.clone();
            let create_request_max_concurrency = create_request_max_concurrency.clone();
            let create_request_min_start_interval_ms = create_request_min_start_interval_ms.clone();
            let usage_page = usage_page.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(quota) = quota else {
                    let message = "主额度必须是正整数".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                };
                let request_max_concurrency = if request_max_concurrency.is_empty() {
                    None
                } else {
                    match request_max_concurrency.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "并发上限必须是整数，留空表示不限制".to_string();
                            load_error.set(Some(message.clone()));
                            flash.emit((message, true));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms = if request_min_start_interval_ms.is_empty() {
                    None
                } else {
                    match request_min_start_interval_ms.parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            let message = "请求间隔必须是整数毫秒，留空表示不限制".to_string();
                            load_error.set(Some(message.clone()));
                            flash.emit((message, true));
                            return;
                        },
                    }
                };
                creating.set(true);
                match create_admin_llm_gateway_key(
                    &name,
                    quota,
                    public_visible,
                    request_max_concurrency,
                    request_min_start_interval_ms,
                )
                .await
                {
                    Ok(_) => {
                        create_name.set(String::new());
                        create_request_max_concurrency.set(String::new());
                        create_request_min_start_interval_ms.set(String::new());
                        usage_page.set(1);
                        load_error.set(None);
                        flash.emit((format!("已创建 key `{}`", name), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建 key `{}` 失败\n{err}", name), true));
                    },
                }
                creating.set(false);
            });
        })
    };

    let on_toggle_create_account_group_member = {
        let create_account_group_account_names = create_account_group_account_names.clone();
        Callback::from(move |account_name: String| {
            let mut names = (*create_account_group_account_names).clone();
            if let Some(index) = names.iter().position(|name| name == &account_name) {
                names.remove(index);
            } else {
                names.push(account_name);
                names.sort();
                names.dedup();
            }
            create_account_group_account_names.set(names);
        })
    };

    let on_create_account_group = {
        let create_account_group_name = create_account_group_name.clone();
        let create_account_group_account_names = create_account_group_account_names.clone();
        let creating_account_group = creating_account_group.clone();
        let flash = flash.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            if *creating_account_group {
                return;
            }
            let group_name = (*create_account_group_name).trim().to_string();
            let account_names = (*create_account_group_account_names).clone();
            let create_account_group_name = create_account_group_name.clone();
            let create_account_group_account_names = create_account_group_account_names.clone();
            let creating_account_group = creating_account_group.clone();
            let flash = flash.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if group_name.is_empty() {
                    let message = "账号组名称不能为空".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                }
                if account_names.is_empty() {
                    let message = "账号组至少需要选择一个账号".to_string();
                    load_error.set(Some(message.clone()));
                    flash.emit((message, true));
                    return;
                }
                creating_account_group.set(true);
                match create_admin_llm_gateway_account_group(CreateAdminAccountGroupInput {
                    name: &group_name,
                    account_names: account_names.as_slice(),
                })
                .await
                {
                    Ok(_) => {
                        create_account_group_name.set(String::new());
                        create_account_group_account_names.set(Vec::new());
                        load_error.set(None);
                        flash.emit((format!("已创建账号组 `{group_name}`"), false));
                        reload.emit(());
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("创建账号组失败\n{err}"), true));
                    },
                }
                creating_account_group.set(false);
            });
        })
    };

    // A per-card refresh avoids reloading unrelated state while re-reading the
    // latest counters for a single key.
    let on_refresh_key = {
        let keys = keys.clone();
        let load_error = load_error.clone();
        let flash = flash.clone();
        let refreshing_key_id = refreshing_key_id.clone();
        Callback::from(move |(key_id, key_name): (String, String)| {
            refreshing_key_id.set(Some(key_id.clone()));
            let keys = keys.clone();
            let load_error = load_error.clone();
            let flash = flash.clone();
            let refreshing_key_id = refreshing_key_id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_keys().await {
                    Ok(resp) => {
                        keys.set(resp.keys);
                        load_error.set(None);
                        flash.emit((format!("已刷新 key `{}`", key_name), false));
                    },
                    Err(err) => {
                        load_error.set(Some(err.clone()));
                        flash.emit((format!("刷新 key `{}` 失败\n{err}", key_name), true));
                    },
                }
                refreshing_key_id.set(None);
            });
        })
    };

    let on_usage_key_filter_change = {
        let usage_key_filter = usage_key_filter.clone();
        let usage_page = usage_page.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let selected_key_id = target.value();
                usage_key_filter.set(selected_key_id.clone());
                usage_page.set(1);
                reload_usage.emit((Some(1), Some(selected_key_id)));
            }
        })
    };

    let on_usage_page_change = {
        let usage_page = usage_page.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |page: usize| {
            usage_page.set(page);
            reload_usage.emit((Some(page), None));
        })
    };

    let on_usage_scroll_top = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |_| {
            let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() else {
                return;
            };
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let left = top.scroll_left();
            if bottom.scroll_left() != left {
                bottom.set_scroll_left(left);
            }
        })
    };

    let on_usage_scroll_bottom = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |_| {
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() else {
                return;
            };
            let left = bottom.scroll_left();
            if top.scroll_left() != left {
                top.set_scroll_left(left);
            }
        })
    };

    // Programmatically scroll the usage table left/right by `delta` pixels,
    // keeping the top mirror scrollbar in sync.
    let scroll_usage_table_by = {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        Callback::from(move |delta: i32| {
            let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() else {
                return;
            };
            let next_left = (bottom.scroll_left() + delta).max(0);
            bottom.set_scroll_left(next_left);
            if let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() {
                top.set_scroll_left(next_left);
            }
        })
    };

    {
        let usage_scroll_top_ref = usage_scroll_top_ref.clone();
        let usage_scroll_bottom_ref = usage_scroll_bottom_ref.clone();
        let usage_scroll_width = usage_scroll_width.clone();
        let event_count = usage_events.len();
        let usage_loading_flag = *usage_loading;
        let usage_page_value = *usage_page;
        use_effect_with((event_count, usage_loading_flag, usage_page_value), move |_| {
            if let Some(bottom) = usage_scroll_bottom_ref.cast::<HtmlElement>() {
                let measured_width = bottom.scroll_width().max(bottom.client_width()).max(1);
                usage_scroll_width.set(measured_width);
                if let Some(top) = usage_scroll_top_ref.cast::<HtmlElement>() {
                    top.set_scroll_left(bottom.scroll_left());
                }
            }
            || ()
        });
    }

    let usage_total_pages = (*usage_total).max(1).div_ceil(USAGE_PAGE_SIZE);
    let token_request_total_pages = (*token_request_total)
        .max(1)
        .div_ceil(TOKEN_REQUEST_PAGE_SIZE);
    let account_contribution_request_total_pages = (*account_contribution_request_total)
        .max(1)
        .div_ceil(ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE);
    let sponsor_request_total_pages = (*sponsor_request_total)
        .max(1)
        .div_ceil(SPONSOR_REQUEST_PAGE_SIZE);

    let on_token_request_status_filter_change = {
        let token_request_status_filter = token_request_status_filter.clone();
        let token_request_page = token_request_page.clone();
        let reload_token_requests = reload_token_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                token_request_status_filter.set(status.clone());
                token_request_page.set(1);
                reload_token_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_token_request_page_change = {
        let token_request_page = token_request_page.clone();
        let reload_token_requests = reload_token_requests.clone();
        Callback::from(move |page: usize| {
            token_request_page.set(page);
            reload_token_requests.emit((Some(page), None));
        })
    };

    let on_approve_token_request = {
        let token_request_action_inflight = token_request_action_inflight.clone();
        let token_requests = token_requests.clone();
        let reload = reload.clone();
        let reload_token_requests = reload_token_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let token_request_action_inflight = token_request_action_inflight.clone();
            let token_requests = token_requests.clone();
            let reload = reload.clone();
            let reload_token_requests = reload_token_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*token_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                token_request_action_inflight.set(inflight);

                match admin_approve_and_issue_llm_gateway_token_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*token_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        token_requests.set(list);
                        load_error.set(None);
                        reload.emit(());
                        reload_token_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*token_request_action_inflight).clone();
                inflight.remove(&request_id);
                token_request_action_inflight.set(inflight);
            });
        })
    };

    let on_reject_token_request = {
        let token_request_action_inflight = token_request_action_inflight.clone();
        let token_requests = token_requests.clone();
        let reload_token_requests = reload_token_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let token_request_action_inflight = token_request_action_inflight.clone();
            let token_requests = token_requests.clone();
            let reload_token_requests = reload_token_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*token_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                token_request_action_inflight.set(inflight);

                match admin_reject_llm_gateway_token_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*token_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        token_requests.set(list);
                        load_error.set(None);
                        reload_token_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*token_request_action_inflight).clone();
                inflight.remove(&request_id);
                token_request_action_inflight.set(inflight);
            });
        })
    };

    let on_account_contribution_status_filter_change = {
        let account_contribution_request_status_filter =
            account_contribution_request_status_filter.clone();
        let account_contribution_request_page = account_contribution_request_page.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                account_contribution_request_status_filter.set(status.clone());
                account_contribution_request_page.set(1);
                reload_account_contribution_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_account_contribution_page_change = {
        let account_contribution_request_page = account_contribution_request_page.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        Callback::from(move |page: usize| {
            account_contribution_request_page.set(page);
            reload_account_contribution_requests.emit((Some(page), None));
        })
    };

    let on_approve_account_contribution_request = {
        let account_contribution_request_action_inflight =
            account_contribution_request_action_inflight.clone();
        let account_contribution_requests = account_contribution_requests.clone();
        let reload = reload.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let account_contribution_request_action_inflight =
                account_contribution_request_action_inflight.clone();
            let account_contribution_requests = account_contribution_requests.clone();
            let reload = reload.clone();
            let reload_account_contribution_requests = reload_account_contribution_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                account_contribution_request_action_inflight.set(inflight);

                match admin_approve_and_issue_llm_gateway_account_contribution_request(
                    &request_id,
                    None,
                )
                .await
                {
                    Ok(updated) => {
                        let mut list = (*account_contribution_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        account_contribution_requests.set(list);
                        load_error.set(None);
                        reload.emit(());
                        reload_account_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.remove(&request_id);
                account_contribution_request_action_inflight.set(inflight);
            });
        })
    };

    let on_reject_account_contribution_request = {
        let account_contribution_request_action_inflight =
            account_contribution_request_action_inflight.clone();
        let account_contribution_requests = account_contribution_requests.clone();
        let reload_account_contribution_requests = reload_account_contribution_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let account_contribution_request_action_inflight =
                account_contribution_request_action_inflight.clone();
            let account_contribution_requests = account_contribution_requests.clone();
            let reload_account_contribution_requests = reload_account_contribution_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                account_contribution_request_action_inflight.set(inflight);

                match admin_reject_llm_gateway_account_contribution_request(&request_id, None).await
                {
                    Ok(updated) => {
                        let mut list = (*account_contribution_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        account_contribution_requests.set(list);
                        load_error.set(None);
                        reload_account_contribution_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_contribution_request_action_inflight).clone();
                inflight.remove(&request_id);
                account_contribution_request_action_inflight.set(inflight);
            });
        })
    };

    let on_sponsor_request_status_filter_change = {
        let sponsor_request_status_filter = sponsor_request_status_filter.clone();
        let sponsor_request_page = sponsor_request_page.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                let status = target.value();
                sponsor_request_status_filter.set(status.clone());
                sponsor_request_page.set(1);
                reload_sponsor_requests.emit((Some(1), Some(status)));
            }
        })
    };

    let on_sponsor_request_page_change = {
        let sponsor_request_page = sponsor_request_page.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        Callback::from(move |page: usize| {
            sponsor_request_page.set(page);
            reload_sponsor_requests.emit((Some(page), None));
        })
    };

    let on_approve_sponsor_request = {
        let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
        let sponsor_requests = sponsor_requests.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
            let sponsor_requests = sponsor_requests.clone();
            let reload_sponsor_requests = reload_sponsor_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                sponsor_request_action_inflight.set(inflight);

                match admin_approve_llm_gateway_sponsor_request(&request_id, None).await {
                    Ok(updated) => {
                        let mut list = (*sponsor_requests).clone();
                        if let Some(item) = list
                            .iter_mut()
                            .find(|item| item.request_id == updated.request_id)
                        {
                            *item = updated;
                        }
                        sponsor_requests.set(list);
                        load_error.set(None);
                        reload_sponsor_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.remove(&request_id);
                sponsor_request_action_inflight.set(inflight);
            });
        })
    };

    let on_delete_sponsor_request = {
        let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
        let sponsor_requests = sponsor_requests.clone();
        let sponsor_request_total = sponsor_request_total.clone();
        let reload_sponsor_requests = reload_sponsor_requests.clone();
        let load_error = load_error.clone();
        Callback::from(move |request_id: String| {
            let Some(browser) = window() else {
                return;
            };
            if !browser
                .confirm_with_message("确认删除这条 Sponsor 请求？")
                .ok()
                .unwrap_or(false)
            {
                return;
            }

            let sponsor_request_action_inflight = sponsor_request_action_inflight.clone();
            let sponsor_requests = sponsor_requests.clone();
            let sponsor_request_total = sponsor_request_total.clone();
            let reload_sponsor_requests = reload_sponsor_requests.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.insert(request_id.clone());
                sponsor_request_action_inflight.set(inflight);

                match delete_admin_llm_gateway_sponsor_request(&request_id).await {
                    Ok(_) => {
                        let filtered = (*sponsor_requests)
                            .iter()
                            .filter(|item| item.request_id != request_id)
                            .cloned()
                            .collect::<Vec<_>>();
                        sponsor_requests.set(filtered);
                        sponsor_request_total.set((*sponsor_request_total).saturating_sub(1));
                        load_error.set(None);
                        reload_sponsor_requests.emit((None, None));
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*sponsor_request_action_inflight).clone();
                inflight.remove(&request_id);
                sponsor_request_action_inflight.set(inflight);
            });
        })
    };

    let on_toggle_account_spark_mapping = {
        let account_action_inflight = account_action_inflight.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |(account_name, enabled): (String, bool)| {
            let account_action_inflight = account_action_inflight.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        map_gpt53_codex_to_spark: Some(enabled),
                        proxy_mode: None,
                        proxy_config_id: None,
                        request_max_concurrency: None,
                        request_min_start_interval_ms: None,
                        request_max_concurrency_unlimited: false,
                        request_min_start_interval_ms_unlimited: false,
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated;
                        }
                        accounts.set(items);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_save_account_settings = {
        let account_action_inflight = account_action_inflight.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let account_request_max_inputs = account_request_max_inputs.clone();
        let account_request_min_inputs = account_request_min_inputs.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let account_request_max_inputs = account_request_max_inputs.clone();
            let account_request_min_inputs = account_request_min_inputs.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let selection = (*account_proxy_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_else(|| "inherit".to_string());
                let request_max_raw = (*account_request_max_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_default();
                let request_min_raw = (*account_request_min_inputs)
                    .get(&account_name)
                    .cloned()
                    .unwrap_or_default();
                let (proxy_mode, proxy_config_id) = if selection == "direct" {
                    (Some("direct".to_string()), None)
                } else if let Some(proxy_config_id) = selection.strip_prefix("fixed:") {
                    (Some("fixed".to_string()), Some(proxy_config_id.to_string()))
                } else {
                    (Some("inherit".to_string()), None)
                };
                let request_max_concurrency = if request_max_raw.trim().is_empty() {
                    None
                } else {
                    match request_max_raw.trim().parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            load_error
                                .set(Some("账号并发上限必须是整数，留空表示不限制".to_string()));
                            return;
                        },
                    }
                };
                let request_min_start_interval_ms = if request_min_raw.trim().is_empty() {
                    None
                } else {
                    match request_min_raw.trim().parse::<u64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            load_error.set(Some(
                                "账号请求起始间隔必须是整数毫秒，留空表示不限制".to_string(),
                            ));
                            return;
                        },
                    }
                };

                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match patch_admin_llm_gateway_account(
                    &account_name,
                    &PatchAdminLlmGatewayAccountInput {
                        map_gpt53_codex_to_spark: None,
                        proxy_mode,
                        proxy_config_id,
                        request_max_concurrency,
                        request_min_start_interval_ms,
                        request_max_concurrency_unlimited: request_max_concurrency.is_none(),
                        request_min_start_interval_ms_unlimited: request_min_start_interval_ms
                            .is_none(),
                    },
                )
                .await
                {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated.clone();
                        }
                        accounts.set(items);

                        let mut next_inputs = (*account_proxy_inputs).clone();
                        next_inputs
                            .insert(updated.name.clone(), account_proxy_select_value(&updated));
                        account_proxy_inputs.set(next_inputs);
                        let mut next_request_max_inputs = (*account_request_max_inputs).clone();
                        next_request_max_inputs.insert(
                            updated.name.clone(),
                            updated
                                .request_max_concurrency
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        account_request_max_inputs.set(next_request_max_inputs);
                        let mut next_request_min_inputs = (*account_request_min_inputs).clone();
                        next_request_min_inputs.insert(
                            updated.name.clone(),
                            updated
                                .request_min_start_interval_ms
                                .map(|value| value.to_string())
                                .unwrap_or_default(),
                        );
                        account_request_min_inputs.set(next_request_min_inputs);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_refresh_account = {
        let account_action_inflight = account_action_inflight.clone();
        let account_proxy_inputs = account_proxy_inputs.clone();
        let accounts = accounts.clone();
        let load_error = load_error.clone();
        Callback::from(move |account_name: String| {
            let account_action_inflight = account_action_inflight.clone();
            let account_proxy_inputs = account_proxy_inputs.clone();
            let accounts = accounts.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut inflight = (*account_action_inflight).clone();
                inflight.insert(account_name.clone());
                account_action_inflight.set(inflight);

                match refresh_admin_llm_gateway_account(&account_name).await {
                    Ok(updated) => {
                        let mut items = (*accounts).clone();
                        if let Some(item) = items.iter_mut().find(|item| item.name == updated.name)
                        {
                            *item = updated.clone();
                        }
                        accounts.set(items);

                        let mut next_inputs = (*account_proxy_inputs).clone();
                        next_inputs
                            .insert(updated.name.clone(), account_proxy_select_value(&updated));
                        account_proxy_inputs.set(next_inputs);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
                }

                let mut inflight = (*account_action_inflight).clone();
                inflight.remove(&account_name);
                account_action_inflight.set(inflight);
            });
        })
    };

    let on_copy = {
        let flash = flash.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            flash.emit((format!("已复制{}", label), false));
        })
    };

    let on_import_account = {
        let import_name = import_name.clone();
        let import_id_token = import_id_token.clone();
        let import_access_token = import_access_token.clone();
        let import_refresh_token = import_refresh_token.clone();
        let import_account_id = import_account_id.clone();
        let importing = importing.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let name = (*import_name).trim().to_string();
            let id_token = (*import_id_token).trim().to_string();
            let access_token = (*import_access_token).trim().to_string();
            let refresh_token = (*import_refresh_token).trim().to_string();
            let account_id = {
                let v = (*import_account_id).trim().to_string();
                if v.is_empty() {
                    None
                } else {
                    Some(v)
                }
            };
            let importing = importing.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            let import_name = import_name.clone();
            let import_id_token = import_id_token.clone();
            let import_access_token = import_access_token.clone();
            let import_refresh_token = import_refresh_token.clone();
            let import_account_id = import_account_id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                importing.set(true);
                match import_admin_llm_gateway_account(
                    &name,
                    &id_token,
                    &access_token,
                    &refresh_token,
                    account_id.as_deref(),
                )
                .await
                {
                    Ok(_) => {
                        import_name.set(String::new());
                        import_id_token.set(String::new());
                        import_access_token.set(String::new());
                        import_refresh_token.set(String::new());
                        import_account_id.set(String::new());
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                importing.set(false);
            });
        })
    };

    let on_delete_account = {
        let reload = reload.clone();
        let load_error = load_error.clone();
        Callback::from(move |name: String| {
            let confirmed = window()
                .and_then(|w| {
                    w.confirm_with_message(&format!("确认删除账号 {} ？", name))
                        .ok()
                })
                .unwrap_or(false);
            if !confirmed {
                return;
            }
            let reload = reload.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match delete_admin_llm_gateway_account(&name).await {
                    Ok(_) => reload.emit(()),
                    Err(err) => load_error.set(Some(err)),
                }
            });
        })
    };

    let total_remaining: i64 = keys.iter().map(|item| item.remaining_billable).sum();
    let public_visible_count = keys.iter().filter(|item| item.public_visible).count();
    let active_key_count = keys.iter().filter(|item| item.status == "active").count();
    let total_quota: u64 = keys.iter().map(|k| k.quota_billable_limit).sum();
    let total_used: u64 = keys
        .iter()
        .map(|k| {
            k.usage_input_uncached_tokens + k.usage_input_cached_tokens + k.usage_output_tokens
        })
        .sum();
    let credit_keys_present = keys
        .iter()
        .any(|item| item.usage_credit_total > 0.0 || item.usage_credit_missing_events > 0);
    let total_credit_used: f64 = keys.iter().map(|item| item.usage_credit_total).sum();
    let total_credit_missing_events: u64 = keys
        .iter()
        .map(|item| item.usage_credit_missing_events)
        .sum();
    // Derive usage percentage from quota and remaining (billable-token basis).
    let usage_percent = if total_quota > 0 {
        let used = total_quota as f64 - (total_remaining.max(0) as f64);
        (used / total_quota as f64 * 100.0)
            .clamp(0.0, 100.0)
            .round() as u64
    } else {
        0
    };
    let pending_token_requests = token_requests
        .iter()
        .filter(|r| r.status == "pending")
        .count();
    let pending_contribution_requests = account_contribution_requests
        .iter()
        .filter(|r| r.status == "pending")
        .count();
    let pending_sponsor_requests = sponsor_requests
        .iter()
        .filter(|r| r.status == "submitted" || r.status == "payment_email_sent")
        .count();
    let total_pending =
        pending_token_requests + pending_contribution_requests + pending_sponsor_requests;
    // Build the full-screen modal for a selected usage event (request detail,
    // headers, last message, copy buttons). Rendered outside the tab flow so
    // it overlays the entire viewport.
    let usage_detail_modal = if *usage_detail_loading {
        Some(html! {
            <div class={classes!(
                "fixed",
                "inset-0",
                "z-[90]",
                "flex",
                "items-center",
                "justify-center",
                "bg-slate-950/58",
                "backdrop-blur-sm",
                "px-4",
                "py-8"
            )}>
                <div class={classes!(
                    "rounded-xl",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "px-5",
                    "py-4",
                    "text-sm",
                    "text-[var(--muted)]",
                    "shadow-[0_16px_48px_rgba(0,0,0,0.2)]"
                )}>
                    { "正在加载请求详情..." }
                </div>
            </div>
        })
    } else {
        (*selected_usage_event).clone().map(|event| {
        let request_detail_summary = format!(
            "{} {} · {} / {} · key {} · account {} · status {} · model {} · route {} · latency {}",
            event.request_method,
            event.request_url,
            event.client_ip,
            event.ip_region,
            event.key_name,
            event.account_name
                .clone()
                .unwrap_or_else(|| "legacy auth".to_string()),
            event.status_code,
            event.model.clone().unwrap_or_else(|| "-".to_string()),
            event.endpoint,
            format_latency_ms(event.latency_ms),
        );
        let last_message_for_copy = event
            .last_message_content
            .clone()
            .unwrap_or_else(|| "-".to_string());
        let headers_json_for_copy = pretty_headers_json(&event.request_headers_json);
        let client_request_json_for_copy = event
            .client_request_body_json
            .as_deref()
            .map(pretty_json_text);
        let full_request_json_for_copy = event
            .full_request_json
            .as_deref()
            .map(pretty_json_text);
        let upstream_request_json_for_copy = event
            .upstream_request_body_json
            .as_deref()
            .map(pretty_json_text);
        html! {
            <div
                class={classes!(
                    "fixed",
                    "inset-0",
                    "z-[90]",
                    "flex",
                    "items-start",
                    "sm:items-center",
                    "justify-center",
                    "overflow-y-auto",
                    "bg-slate-950/58",
                    "backdrop-blur-sm",
                    "px-4",
                    "py-8"
                )}
                onclick={{
                    let selected_usage_event = selected_usage_event.clone();
                    Callback::from(move |_| selected_usage_event.set(None))
                }}
            >
                <div
                    class={classes!(
                        "w-full",
                        "mx-auto",
                        "flex",
                        "max-h-[92vh]",
                        "max-w-4xl",
                        "flex-col",
                        "overflow-y-auto",
                        "rounded-xl",
                        "border",
                        "border-[var(--border)]",
                        "bg-[var(--surface)]",
                        "p-5",
                        "shadow-[0_16px_48px_rgba(0,0,0,0.2)]"
                    )}
                    onclick={Callback::from(|event: MouseEvent| event.stop_propagation())}
                >
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap", "shrink-0")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Request Detail" }</p>
                            <h2 class={classes!("mt-3", "text-2xl", "font-black", "tracking-[-0.03em]")}>{ event.key_name.clone() }</h2>
                            <p class={classes!("mt-2", "m-0", "break-all", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { format!("{} {} · {} / {}", event.request_method, event.request_url, event.client_ip, event.ip_region) }
                            </p>
                        </div>
                        <div class={classes!("flex", "gap-2", "flex-wrap")}>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let request_detail_summary = request_detail_summary.clone();
                                    Callback::from(move |_| on_copy.emit(("Request Summary".to_string(), request_detail_summary.clone())))
                                }}
                            >
                                { "复制摘要" }
                            </button>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let headers_json_for_copy = headers_json_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Headers".to_string(), headers_json_for_copy.clone())))
                                }}
                            >
                                { "复制 Headers" }
                            </button>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={{
                                    let selected_usage_event = selected_usage_event.clone();
                                    Callback::from(move |_| selected_usage_event.set(None))
                                }}
                            >
                                { "关闭" }
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-6")}>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key ID" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-xs", "break-all")}>{ event.key_id.clone() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Account" }</div>
                            <div class={classes!("mt-1", "text-sm")}>{ event.account_name.clone().unwrap_or_else(|| "legacy auth".to_string()) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Status / Model" }</div>
                            <div class={classes!("mt-1", "text-sm")}>{ format!("{} · {}", event.status_code, event.model.clone().unwrap_or_else(|| "-".to_string())) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Route" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-xs", "break-all")}>{ event.endpoint.clone() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Latency" }</div>
                            <div class={classes!("mt-1", "text-sm", "font-semibold")}>{ format_latency_ms(event.latency_ms) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Credit" }</div>
                            <div class={classes!("mt-1", "text-sm", "font-semibold")}>
                                { event.credit_usage.map(format_credit4).unwrap_or_else(|| "-".to_string()) }
                            </div>
                            if event.credit_usage_missing {
                                <div class={classes!("mt-1", "text-xs", "text-amber-700", "dark:text-amber-200")}>{ "missing" }</div>
                            }
                        </div>
                    </div>

                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Last Message" }</div>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let last_message_for_copy = last_message_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Last Message".to_string(), last_message_for_copy.clone())))
                                }}
                            >
                                { "复制 Last Message" }
                            </button>
                        </div>
                        <pre class={classes!(
                            "max-h-[40vh]",
                            "overflow-x-auto",
                            "overflow-y-auto",
                            "rounded-lg",
                            "bg-slate-950",
                            "p-3",
                            "text-xs",
                            "leading-6",
                            "text-amber-100",
                            "whitespace-pre-wrap",
                            "break-words"
                        )}>
                            { last_message_for_copy }
                        </pre>
                    </div>

                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Headers" }</div>
                            <button
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let headers_json_for_copy = headers_json_for_copy.clone();
                                    Callback::from(move |_| on_copy.emit(("Headers".to_string(), headers_json_for_copy.clone())))
                                }}
                            >
                                { "复制 Headers" }
                            </button>
                        </div>
                        <pre class={classes!(
                            "max-h-[42vh]",
                            "overflow-x-auto",
                            "overflow-y-auto",
                            "rounded-lg",
                            "bg-slate-950",
                            "p-3",
                            "text-xs",
                            "leading-6",
                            "text-emerald-200",
                            "whitespace-pre-wrap",
                            "break-words"
                        )}>
                            { headers_json_for_copy }
                        </pre>
                    </div>

                    if let Some(client_request_json_for_copy) = client_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Client Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let client_request_json_for_copy = client_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Client Request".to_string(), client_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Client Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-sky-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { client_request_json_for_copy }
                            </pre>
                        </div>
                    }

                    if let Some(full_request_json_for_copy) = full_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Full Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let full_request_json_for_copy = full_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Full Request".to_string(), full_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Full Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-cyan-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { full_request_json_for_copy }
                            </pre>
                        </div>
                    }

                    if let Some(upstream_request_json_for_copy) = upstream_request_json_for_copy {
                        <div class={classes!("mt-4")}>
                            <div class={classes!("mb-2", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Upstream Request" }</div>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let upstream_request_json_for_copy = upstream_request_json_for_copy.clone();
                                        Callback::from(move |_| on_copy.emit(("Upstream Request".to_string(), upstream_request_json_for_copy.clone())))
                                    }}
                                >
                                    { "复制 Upstream Request" }
                                </button>
                            </div>
                            <pre class={classes!(
                                "max-h-[42vh]",
                                "overflow-x-auto",
                                "overflow-y-auto",
                                "rounded-lg",
                                "bg-slate-950",
                                "p-3",
                                "text-xs",
                                "leading-6",
                                "text-fuchsia-100",
                                "whitespace-pre-wrap",
                                "break-words"
                            )}>
                                { upstream_request_json_for_copy }
                            </pre>
                        </div>
                    }
                </div>
            </div>
        }
        })
    };

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
                <section class={classes!(
                    "rounded-xl",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "p-5"
                )}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold")}>
                            { "LLM Gateway Admin" }
                        </h1>
                        <div class={classes!("flex", "gap-2", "flex-wrap")}>
                            <Link<Route> to={Route::Admin} classes={classes!("btn-terminal")}>{ "Admin 首页" }</Link<Route>>
                            <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal", "btn-terminal-primary")}>{ "公共页" }</Link<Route>>
                        </div>
                    </div>

                    if let Some(err) = (*load_error).clone() {
                        <div class={classes!("mt-4", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                            { err }
                        </div>
                    }
                </section>

                // ── Tab Bar (always visible) ──
                { render_tab_bar(&active_tab, &[
                    (TAB_OVERVIEW, "Overview"),
                    (TAB_KEYS, "Keys"),
                    (TAB_GROUPS, "Groups"),
                    (TAB_ACCOUNTS, "Accounts"),
                    (TAB_USAGE, "Usage"),
                    (TAB_REQUESTS, "Requests"),
                    (TAB_SETTINGS, "Settings"),
                ], &on_tab_click, Some((TAB_REQUESTS, total_pending))) }

                // ── Overview Tab ──
                if *active_tab == TAB_OVERVIEW {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Dashboard" }</h2>
                        <button
                            class={classes!("btn-terminal")}
                            title="刷新 Dashboard"
                            aria-label="刷新 Dashboard"
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>
                    <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2", "xl:grid-cols-4")}>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key 总数" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ keys.len() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "公开 / Active" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format!("{} / {}", public_visible_count, active_key_count) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余额度" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_i64(total_remaining) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(total_quota) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已用量" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(total_used) }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("flex", "items-center", "justify-between")}>
                                <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "使用率" }</div>
                                <div class={classes!("font-mono", "text-sm", "font-bold", "text-[var(--text)]")}>{ format!("{}%", usage_percent) }</div>
                            </div>
                            <div class={classes!("mt-2", "h-2", "w-full", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                                <div
                                    class={classes!(
                                        "h-full", "rounded-full",
                                        "transition-all", "duration-700", "ease-out",
                                        if usage_percent >= 90 { "bg-red-500" }
                                        else if usage_percent >= 70 { "bg-amber-500" }
                                        else { "bg-emerald-500" }
                                    )}
                                    style={format!("width: {}%", usage_percent)}
                                />
                            </div>
                            <div class={classes!("mt-1.5", "flex", "justify-between", "font-mono", "text-[10px]", "text-[var(--muted)]")}>
                                <span>{ format!("剩余 {}", format_number_i64(total_remaining)) }</span>
                                <span>{ format!("总计 {}", format_number_u64(total_quota)) }</span>
                            </div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Credit 已记录" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>
                                { if credit_keys_present { format_credit4(total_credit_used) } else { "-".to_string() } }
                            </div>
                            if total_credit_missing_events > 0 {
                                <div class={classes!("mt-1", "text-xs", "text-amber-700", "dark:text-amber-200")}>
                                    { format!("partial · {} events missing", total_credit_missing_events) }
                                </div>
                            }
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "待审核" }</div>
                            <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black", if total_pending > 0 { "text-amber-600" } else { "" })}>{ total_pending }</div>
                        </div>
                    </div>
                </section>
                } // end TAB_OVERVIEW

                // ── Settings Tab ──
                if *active_tab == TAB_SETTINGS {
                <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Runtime Config" }</h2>
                                <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                    { "This page owns gateway-wide runtime defaults and llm usage maintenance cadence. Kiro cache simulation, prefix-tree capacity, anchor settings, and per-account scheduler overrides are managed from the Kiro Gateway page." }
                                </p>
                            </div>
                            <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal", "btn-terminal-secondary")}>
                                { "Open Kiro Gateway" }
                            </Link<Route>>
                        </div>
                        <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-2", "xl:grid-cols-3")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "auth_cache_ttl_seconds" }</span>
                                <input
                                    type="number"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*ttl_input).clone()}
                                    oninput={{
                                        let ttl_input = ttl_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                ttl_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "max_request_body_bytes" }</span>
                                <input
                                    type="number"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*max_request_body_input).clone()}
                                    oninput={{
                                        let max_request_body_input = max_request_body_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                max_request_body_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "account_failure_retry_limit" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*account_failure_retry_limit_input).clone()}
                                    oninput={{
                                        let account_failure_retry_limit_input = account_failure_retry_limit_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                account_failure_retry_limit_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_refresh_min_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_refresh_min_input).clone()}
                                    oninput={{
                                        let codex_refresh_min_input = codex_refresh_min_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_refresh_min_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_refresh_max_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_refresh_max_input).clone()}
                                    oninput={{
                                        let codex_refresh_max_input = codex_refresh_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_refresh_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "codex_status_account_jitter_max_seconds" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*codex_account_jitter_max_input).clone()}
                                    oninput={{
                                        let codex_account_jitter_max_input = codex_account_jitter_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                codex_account_jitter_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_refresh_min_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_refresh_min_input).clone()}
                                    oninput={{
                                        let kiro_refresh_min_input = kiro_refresh_min_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_refresh_min_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_refresh_max_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="240"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_refresh_max_input).clone()}
                                    oninput={{
                                        let kiro_refresh_max_input = kiro_refresh_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_refresh_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "kiro_status_account_jitter_max_seconds" }</span>
                                <input
                                    type="number"
                                    min="0"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*kiro_account_jitter_max_input).clone()}
                                    oninput={{
                                        let kiro_account_jitter_max_input = kiro_account_jitter_max_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                kiro_account_jitter_max_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_batch_size" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_batch_size_input).clone()}
                                    oninput={{
                                        let usage_flush_batch_size_input = usage_flush_batch_size_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_batch_size_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_interval_seconds" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_interval_input).clone()}
                                    oninput={{
                                        let usage_flush_interval_input = usage_flush_interval_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_interval_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "usage_event_flush_max_buffer_bytes" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*usage_flush_max_buffer_bytes_input).clone()}
                                    oninput={{
                                        let usage_flush_max_buffer_bytes_input = usage_flush_max_buffer_bytes_input.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                usage_flush_max_buffer_bytes_input.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--bg)]", "px-3", "py-2", "text-xs", "text-[var(--muted)]", "md:col-span-2", "xl:col-span-3")}>
                                <p class={classes!("m-0")}>
                                    { "默认轮询窗口：Codex / Kiro 都是 240-300 秒；每个账号请求之间插入 0-10 秒随机抖动。" }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "默认 usage flush：256 条、15 秒、8 MiB。提高阈值能显著降低 version churn，但会增加短时缓冲占用。" }
                                </p>
                                <p class={classes!("m-0", "mt-1")}>
                                    { "llm usage 表现在和其他表共用 /admin 里的 Storage Maintenance 配置：scan interval、fragment threshold、prune 窗口和 worker 数都只有一套。" }
                                </p>
                            </div>
                            <div class={classes!("flex", "items-end", "md:col-span-2", "xl:col-span-3")}>
                                <button class={classes!("btn-terminal", "btn-terminal-primary", "w-full", "md:w-auto")} onclick={on_save_runtime_config} disabled={*saving_runtime_config}>
                                    { if *saving_runtime_config { "保存中..." } else { "保存" } }
                                </button>
                            </div>
                        </div>
                        if let Some(cfg) = (*config).clone() {
                            <div class={classes!("mt-3", "space-y-1", "text-xs", "text-[var(--muted)]")}>
                                <p class={classes!("m-0")}>
                                    { format!("当前 TTL：{} 秒", cfg.auth_cache_ttl_seconds) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!("当前请求体上限：{} bytes", format_number_u64(cfg.max_request_body_bytes)) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!("当前账号失败重试次数：{}", cfg.account_failure_retry_limit) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 Codex 轮询窗口：{}-{} 秒，单账号抖动上限：{} 秒",
                                        cfg.codex_status_refresh_min_interval_seconds,
                                        cfg.codex_status_refresh_max_interval_seconds,
                                        cfg.codex_status_account_jitter_max_seconds
                                    ) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 Kiro 轮询窗口：{}-{} 秒，单账号抖动上限：{} 秒",
                                        cfg.kiro_status_refresh_min_interval_seconds,
                                        cfg.kiro_status_refresh_max_interval_seconds,
                                        cfg.kiro_status_account_jitter_max_seconds
                                    ) }
                                </p>
                                <p class={classes!("m-0")}>
                                    { format!(
                                        "当前 usage flush：{} 条 / {} 秒 / {} bytes",
                                        cfg.usage_event_flush_batch_size,
                                        cfg.usage_event_flush_interval_seconds,
                                        format_number_u64(cfg.usage_event_flush_max_buffer_bytes)
                                    ) }
                                </p>
                            </div>
                        }
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Create Key" }</h2>
                        <div class={classes!("mt-3", "grid", "gap-3")}>
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "名称" }</span>
                                    <input
                                        type="text"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_name).clone()}
                                        oninput={{
                                            let create_name = create_name.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_name.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "主额度上限" }</span>
                                    <input
                                        type="number"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_quota).clone()}
                                        oninput={{
                                            let create_quota = create_quota.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_quota.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                            </div>
                            <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "并发上限" }</span>
                                    <input
                                        type="number"
                                        placeholder="留空表示不限制"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_request_max_concurrency).clone()}
                                        oninput={{
                                            let create_request_max_concurrency = create_request_max_concurrency.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_request_max_concurrency.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "请求起始间隔 ms" }</span>
                                    <input
                                        type="number"
                                        placeholder="留空表示不限制"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_request_min_start_interval_ms).clone()}
                                        oninput={{
                                            let create_request_min_start_interval_ms = create_request_min_start_interval_ms.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_request_min_start_interval_ms.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                            </div>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <label class={classes!("flex", "items-center", "gap-2", "text-sm")}>
                                    <input
                                        type="checkbox"
                                        checked={*create_public}
                                        onchange={{
                                            let create_public = create_public.clone();
                                            Callback::from(move |event: Event| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_public.set(target.checked());
                                                }
                                            })
                                        }}
                                    />
                                    <span>{ "公开" }</span>
                                </label>
                                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create} disabled={*creating}>
                                    { if *creating { "创建中..." } else { "创建" } }
                                </button>
                            </div>
                        </div>
                    </section>
                </section>

                <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Provider Proxy Bindings" }</h2>
                            <button class={classes!("btn-terminal")} onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}>
                                { if *loading { "刷新中..." } else { "刷新" } }
                            </button>
                        </div>
                        <div class={classes!("mt-4", "grid", "gap-4")}>
                            {
                                for ["codex", "kiro"].iter().map(|provider| {
                                    let binding = proxy_bindings.iter().find(|item| item.provider_type == *provider).cloned();
                                    let selected_value = if *provider == "codex" {
                                        (*codex_proxy_binding_input).clone()
                                    } else {
                                        (*kiro_proxy_binding_input).clone()
                                    };
                                    let on_change = if *provider == "codex" {
                                        let codex_proxy_binding_input = codex_proxy_binding_input.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                codex_proxy_binding_input.set(target.value());
                                            }
                                        })
                                    } else {
                                        let kiro_proxy_binding_input = kiro_proxy_binding_input.clone();
                                        Callback::from(move |event: Event| {
                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                kiro_proxy_binding_input.set(target.value());
                                            }
                                        })
                                    };
                                    let provider_name = (*provider).to_string();
                                    let select_key = format!(
                                        "provider-proxy-binding-{}-{}",
                                        provider_name,
                                        selected_value.clone()
                                    );
                                    html! {
                                        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                                <div>
                                                    <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ provider_name.to_uppercase() }</div>
                                                    <div class={classes!("mt-1", "text-sm", "text-[var(--muted)]")}>
                                                        {
                                                            binding.as_ref()
                                                                .map(|item| format!("{} · {}", item.effective_source, item.effective_proxy_url.clone().unwrap_or_else(|| "-".to_string())))
                                                                .unwrap_or_else(|| "loading".to_string())
                                                        }
                                                    </div>
                                                </div>
                                                <button
                                                    class={classes!("btn-terminal", "btn-terminal-primary")}
                                                    onclick={{
                                                        let on_save_proxy_binding = on_save_proxy_binding.clone();
                                                        let provider_name = provider_name.clone();
                                                        Callback::from(move |_| on_save_proxy_binding.emit(provider_name.clone()))
                                                    }}
                                                    disabled={(*saving_proxy_binding_provider).as_deref() == Some(provider_name.as_str())}
                                                >
                                                    {
                                                        if (*saving_proxy_binding_provider).as_deref() == Some(provider_name.as_str()) {
                                                            "保存中..."
                                                        } else {
                                                            "保存绑定"
                                                        }
                                                    }
                                                </button>
                                            </div>
                                            <label class={classes!("mt-4", "block", "text-sm")}>
                                                <span class={classes!("text-[var(--muted)]")}>{ "绑定到代理配置" }</span>
                                                <select
                                                    key={select_key}
                                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                                    value={selected_value.clone()}
                                                    onchange={on_change}
                                                >
                                                    <option value="" selected={selected_value.is_empty()}>{ "Env fallback" }</option>
                                                    { for proxy_configs.iter().map(|proxy_config| html! {
                                                        <option value={proxy_config.id.clone()} selected={selected_value == proxy_config.id}>
                                                            { format!("{} · {}", proxy_config.name, proxy_config.proxy_url) }
                                                        </option>
                                                    }) }
                                                </select>
                                            </label>
                                            if let Some(binding) = binding {
                                                <div class={classes!("mt-3", "space-y-1", "text-xs", "text-[var(--muted)]")}>
                                                    <p class={classes!("m-0")}>
                                                        { format!("effective_source: {}", binding.effective_source) }
                                                    </p>
                                                    <p class={classes!("m-0", "font-mono", "break-all")}>
                                                        { format!("effective_proxy_url: {}", binding.effective_proxy_url.unwrap_or_else(|| "-".to_string())) }
                                                    </p>
                                                    if let Some(error_message) = binding.error_message {
                                                        <p class={classes!("m-0", "text-red-600", "dark:text-red-300")}>
                                                            { format!("error: {}", error_message) }
                                                        </p>
                                                    }
                                                </div>
                                            }
                                        </article>
                                    }
                                })
                            }
                        </div>
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                            <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                <div>
                                    <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "Legacy Kiro Proxy Migration" }</h3>
                                    <p class={classes!("mt-2", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                        { "扫描 ~/.static-flow/auths/kiro/*.json 中遗留的账号级代理字段，导入为共享代理配置，把对应账号切到 fixed 选择，并清掉旧字段。" }
                                    </p>
                                </div>
                                <button class={classes!("btn-terminal")} onclick={on_import_legacy_kiro_proxy} disabled={*migrating_legacy_kiro_proxy}>
                                    { if *migrating_legacy_kiro_proxy { "导入中..." } else { "导入 Legacy Kiro Proxy" } }
                                </button>
                            </div>
                        </div>
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Proxy Config Inventory" }</h2>
                        <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Name" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_name).clone()}
                                    oninput={{
                                        let create_proxy_name = create_proxy_name.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_name.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm", "md:col-span-2")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy URL" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono")}
                                    value={(*create_proxy_url).clone()}
                                    oninput={{
                                        let create_proxy_url = create_proxy_url.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_url.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy Username" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_username).clone()}
                                    oninput={{
                                        let create_proxy_username = create_proxy_username.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_username.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "Proxy Password" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*create_proxy_password).clone()}
                                    oninput={{
                                        let create_proxy_password = create_proxy_password.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                create_proxy_password.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <div class={classes!("md:col-span-2")}>
                                <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create_proxy_config} disabled={*creating_proxy}>
                                    { if *creating_proxy { "创建中..." } else { "创建代理配置" } }
                                </button>
                            </div>
                        </div>
                        <div class={classes!("mt-5", "grid", "gap-4")}>
                            if (*proxy_configs).is_empty() && !*loading {
                                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                    { "当前还没有可复用的代理配置。" }
                                </div>
                            } else {
                                { for proxy_configs.iter().map(|proxy_config| html! {
                                    <ProxyConfigEditorCard
                                        key={proxy_config.id.clone()}
                                        proxy_config={proxy_config.clone()}
                                        on_changed={reload.clone()}
                                        on_copy={on_copy.clone()}
                                        on_flash={flash.clone()}
                                    />
                                }) }
                            }
                        </div>
                    </section>
                </section>
                } // end TAB_SETTINGS

                // ── Keys Tab ──
                if *active_tab == TAB_KEYS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Key Inventory" }</h2>
                        <button class={classes!("btn-terminal")} onclick={{
                            let reload = reload.clone();
                            Callback::from(move |_| reload.emit(()))
                        }}>
                            { if *loading { "刷新中..." } else { "刷新" } }
                        </button>
                    </div>
                    <div class={classes!("mt-5", "grid", "gap-4", "2xl:grid-cols-2")}>
                        if keys.is_empty() && !*loading {
                            <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                { "当前还没有可管理的 key。" }
                            </div>
                        } else {
                            { for keys.iter().map(|key_item| html! {
                                <KeyEditorCard
                                    key={key_item.id.clone()}
                                    key_item={key_item.clone()}
                                    on_changed={reload.clone()}
                                    on_refresh={on_refresh_key.clone()}
                                    on_copy={on_copy.clone()}
                                    on_flash={flash.clone()}
                                    refreshing={(*refreshing_key_id).as_deref() == Some(key_item.id.as_str())}
                                    accounts={(*accounts).clone()}
                                    account_groups={(*account_groups).clone()}
                                />
                            }) }
                        }
                    </div>
                </section>
                } // end TAB_KEYS

                if *active_tab == TAB_GROUPS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Account Groups" }</h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { "先为账号分组，再让 key 选择组而不是直接勾账号。固定路由请选择单账号组；自动路由可以选任意组，留空则继续使用全账号池。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            { if *loading { "刷新中..." } else { "刷新账号组" } }
                        </button>
                    </div>

                    <div class={classes!("mt-4", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-4")}>
                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <div>
                                <h3 class={classes!("m-0", "text-sm", "font-semibold")}>{ "创建账号组" }</h3>
                                <p class={classes!("mt-1", "mb-0", "text-xs", "text-[var(--muted)]")}>
                                    { "默认收起，只在需要新增轮询号池时展开。" }
                                </p>
                            </div>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={{
                                    let account_group_form_expanded = account_group_form_expanded.clone();
                                    Callback::from(move |_| account_group_form_expanded.set(!*account_group_form_expanded))
                                }}
                            >
                                { if *account_group_form_expanded { "收起 ▲" } else { "展开 ▼" } }
                            </button>
                        </div>
                        if *account_group_form_expanded {
                            <div class={classes!("mt-4", "grid", "gap-3")}>
                                <label class={classes!("text-sm")}>
                                    <span class={classes!("text-[var(--muted)]")}>{ "组名" }</span>
                                    <input
                                        type="text"
                                        class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                        value={(*create_account_group_name).clone()}
                                        oninput={{
                                            let create_account_group_name = create_account_group_name.clone();
                                            Callback::from(move |event: InputEvent| {
                                                if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                    create_account_group_name.set(target.value());
                                                }
                                            })
                                        }}
                                    />
                                </label>
                                <div class={classes!("space-y-2")}>
                                    <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "成员账号" }</div>
                                    if accounts.is_empty() {
                                        <div class={classes!("rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-3", "py-3", "text-xs", "text-[var(--muted)]")}>
                                            { "当前没有可加入账号组的账号。" }
                                        </div>
                                    } else {
                                        <div class={classes!("grid", "gap-2", "xl:grid-cols-2")}>
                                            { for accounts.iter().map(|account| {
                                                let checked = create_account_group_account_names.iter().any(|name| name == &account.name);
                                                let account_name = account.name.clone();
                                                let on_toggle_create_account_group_member =
                                                    on_toggle_create_account_group_member.clone();
                                                html! {
                                                    <label class={classes!(
                                                        "flex", "cursor-pointer", "items-center", "gap-3", "rounded-lg", "border", "px-3", "py-2.5",
                                                        if checked {
                                                            "border-sky-500/30 bg-sky-500/8"
                                                        } else {
                                                            "border-[var(--border)] bg-[var(--surface)]"
                                                        }
                                                    )}>
                                                        <input
                                                            type="checkbox"
                                                            checked={checked}
                                                            onchange={Callback::from(move |_| {
                                                                on_toggle_create_account_group_member.emit(account_name.clone())
                                                            })}
                                                        />
                                                        <div class={classes!("min-w-0", "flex-1")}>
                                                            <div class={classes!("font-semibold", "text-[var(--text)]")}>{ account.name.clone() }</div>
                                                            <div class={classes!("mt-1", "font-mono", "text-[11px]", "text-[var(--muted)]")}>
                                                                { format!(
                                                                    "5h {} / wk {}",
                                                                    account.primary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string()),
                                                                    account.secondary_remaining_percent.map(|value| format!("{value:.0}%")).unwrap_or_else(|| "-".to_string())
                                                                ) }
                                                            </div>
                                                        </div>
                                                    </label>
                                                }
                                            }) }
                                        </div>
                                    }
                                </div>
                                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                    <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                        { format!(
                                            "当前成员: {}",
                                            if create_account_group_account_names.is_empty() {
                                                "无".to_string()
                                            } else {
                                                create_account_group_account_names.join(", ")
                                            }
                                        ) }
                                    </span>
                                    <button
                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                        onclick={on_create_account_group}
                                        disabled={*creating_account_group}
                                    >
                                        { if *creating_account_group { "创建中..." } else { "创建账号组" } }
                                    </button>
                                </div>
                            </div>
                        }
                    </div>

                    <div class={classes!("mt-5", "grid", "gap-4", "2xl:grid-cols-2")}>
                        if account_groups.is_empty() && !*loading {
                            <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                                { "当前还没有账号组。" }
                            </div>
                        } else {
                            { for account_groups.iter().map(|group_item| html! {
                                <AccountGroupEditorCard
                                    key={group_item.id.clone()}
                                    group_item={group_item.clone()}
                                    accounts={(*accounts).clone()}
                                    on_changed={reload.clone()}
                                    on_flash={flash.clone()}
                                />
                            }) }
                        }
                    </div>
                </section>
                } // end TAB_GROUPS

                // ── Accounts Tab ──
                if *active_tab == TAB_ACCOUNTS {
                // === Codex Accounts ===
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Codex Accounts" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { format!("已导入 {} 个账号。这里会显示账号状态、usage 刷新健康度和账号级 proxy 配置。", accounts.len()) }
                            </p>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload = reload.clone();
                                Callback::from(move |_| reload.emit(()))
                            }}
                            disabled={*loading}
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            { if *loading { "刷新中..." } else { "刷新列表" } }
                        </button>
                    </div>

                    // Import form toggle
                    <div class={classes!("mt-3")}>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let show_import_form = show_import_form.clone();
                                Callback::from(move |_| show_import_form.set(!*show_import_form))
                            }}
                        >
                            <i class={classes!("fas", if *show_import_form { "fa-chevron-up" } else { "fa-plus" })}></i>
                            { if *show_import_form { "收起导入表单" } else { "导入账号" } }
                        </button>
                    </div>

                    if *show_import_form {
                    // Import form
                    <div class={classes!("mt-3", "grid", "gap-3")}>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "名称 (唯一)" }</span>
                                <input
                                    type="text"
                                    placeholder="my-pro-account"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*import_name).clone()}
                                    oninput={{
                                        let import_name = import_name.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                import_name.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "account_id (可选)" }</span>
                                <input
                                    type="text"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                    value={(*import_account_id).clone()}
                                    oninput={{
                                        let import_account_id = import_account_id.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                import_account_id.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                        </div>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "access_token" }</span>
                            <textarea
                                rows="2"
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                value={(*import_access_token).clone()}
                                oninput={{
                                    let import_access_token = import_access_token.clone();
                                    Callback::from(move |event: InputEvent| {
                                        if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                            import_access_token.set(target.value());
                                        }
                                    })
                                }}
                            />
                        </label>
                        <div class={classes!("grid", "gap-3", "md:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "id_token" }</span>
                                <textarea
                                    rows="2"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                    value={(*import_id_token).clone()}
                                    oninput={{
                                        let import_id_token = import_id_token.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                                import_id_token.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "refresh_token" }</span>
                                <textarea
                                    rows="2"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "font-mono", "text-xs")}
                                    value={(*import_refresh_token).clone()}
                                    oninput={{
                                        let import_refresh_token = import_refresh_token.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<web_sys::HtmlTextAreaElement>() {
                                                import_refresh_token.set(target.value());
                                            }
                                        })
                                    }}
                                />
                            </label>
                        </div>
                        <div class={classes!("flex", "justify-end")}>
                            <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_import_account} disabled={*importing}>
                                { if *importing { "导入验证中..." } else { "导入账号" } }
                            </button>
                        </div>
                    </div>
                    } // end show_import_form

                    // Account list
                    if !accounts.is_empty() {
                        <div class={classes!("mt-4", "space-y-2")}>
                            { for accounts.iter().map(|acc| {
                                let acc_name_for_toggle = acc.name.clone();
                                let acc_name_for_delete = acc.name.clone();
                                let acc_name_for_refresh = acc.name.clone();
                                let acc_name_for_proxy_change = acc.name.clone();
                                let acc_name_for_settings_save = acc.name.clone();
                                let acc_name_for_request_max_change = acc.name.clone();
                                let acc_name_for_request_min_change = acc.name.clone();
                                let acc_name = acc.name.clone();
                                let acc_status = acc.status.clone();
                                let acc_plan_type = acc.plan_type.clone();
                                let acc_account_id = acc.account_id.clone();
                                let spark_mapping_enabled = acc.map_gpt53_codex_to_spark;
                                let selected_proxy_value = (*account_proxy_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| account_proxy_select_value(acc));
                                let selected_request_max_value = (*account_request_max_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        acc.request_max_concurrency
                                            .map(|value| value.to_string())
                                            .unwrap_or_default()
                                    });
                                let selected_request_min_value = (*account_request_min_inputs)
                                    .get(&acc_name)
                                    .cloned()
                                    .unwrap_or_else(|| {
                                        acc.request_min_start_interval_ms
                                            .map(|value| value.to_string())
                                            .unwrap_or_default()
                                    });
                                let configured_proxy_line = account_configured_proxy_label(acc);
                                let effective_proxy_line = format!(
                                    "effective: {} · {}",
                                    acc.effective_proxy_source,
                                    acc.effective_proxy_url.clone().unwrap_or_else(|| "direct".to_string())
                                );
                                let scheduler_line = format!(
                                    "scheduler: concurrency {} · start interval {}",
                                    acc.request_max_concurrency
                                        .map(|value| value.to_string())
                                        .unwrap_or_else(|| "∞".to_string()),
                                    acc.request_min_start_interval_ms
                                        .map(|value| format!("{} ms", value))
                                        .unwrap_or_else(|| "∞".to_string())
                                );
                                let last_refresh_line = acc
                                    .last_refresh
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let last_usage_checked_line = acc
                                    .last_usage_checked_at
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let last_usage_success_line = acc
                                    .last_usage_success_at
                                    .map(format_ms)
                                    .unwrap_or_else(|| "-".to_string());
                                let on_delete = on_delete_account.clone();
                                let on_refresh_account = on_refresh_account.clone();
                                let on_toggle_account_spark_mapping =
                                    on_toggle_account_spark_mapping.clone();
                                let on_save_account_settings = on_save_account_settings.clone();
                                let primary_pct = acc.primary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let secondary_pct = acc.secondary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let is_pro = is_gpt_pro_account(acc_plan_type.as_deref());
                                let show_spark_toggle = is_pro || spark_mapping_enabled;
                                let account_busy =
                                    (*account_action_inflight).contains(&acc_name);
                                html! {
                                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "px-4", "py-3", "flex-wrap")}>
                                        <div class={classes!("flex", "items-center", "gap-3")}>
                                            <div class={key_status_badge(&acc_status)}>{ acc_status.clone() }</div>
                                            <div>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("font-bold")}>{ acc_name.clone() }</span>
                                                    if let Some(ref plan_type) = acc_plan_type {
                                                        <span class={classes!("rounded-full", "bg-sky-500/12", "px-2.5", "py-1", "text-xs", "font-semibold", "text-sky-700", "dark:text-sky-200")}>
                                                            { plan_type.clone() }
                                                        </span>
                                                    }
                                                    if let Some(ref aid) = acc_account_id {
                                                        <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ aid.clone() }</span>
                                                    }
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "font-mono", "text-[var(--muted)]")}>
                                                    { configured_proxy_line.clone() }
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "font-mono", "text-[var(--muted)]")}>
                                                    { effective_proxy_line.clone() }
                                                    if let Some(proxy_name) = acc.effective_proxy_config_name.as_deref() {
                                                        { format!(" · {}", proxy_name) }
                                                    }
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "font-mono", "text-[var(--muted)]")}>
                                                    { scheduler_line.clone() }
                                                </div>
                                                <div class={classes!("mt-1", "text-xs", "font-mono", "text-[var(--muted)]", "flex", "gap-3", "flex-wrap")}>
                                                    <span>{ format!("token refresh {}", last_refresh_line) }</span>
                                                    <span>{ format!("usage checked {}", last_usage_checked_line) }</span>
                                                    <span>{ format!("usage success {}", last_usage_success_line) }</span>
                                                </div>
                                                if let Some(usage_error) = acc.usage_error_message.as_deref() {
                                                    <div class={classes!("mt-2", "max-w-3xl", "text-xs", "leading-5", "text-amber-700", "dark:text-amber-300")}>
                                                        { format!("usage refresh error: {}", usage_error) }
                                                    </div>
                                                }
                                            </div>
                                        </div>
                                        <div class={classes!("flex", "items-center", "gap-3", "flex-wrap", "justify-end")}>
                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("5h {} / wk {}", primary_pct, secondary_pct) }
                                            </span>
                                            <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                <input
                                                    type="number"
                                                    class={classes!("w-28", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-2", "text-xs")}
                                                    placeholder="账号并发"
                                                    value={selected_request_max_value.clone()}
                                                    oninput={{
                                                        let account_request_max_inputs = account_request_max_inputs.clone();
                                                        Callback::from(move |event: InputEvent| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                                let mut next = (*account_request_max_inputs).clone();
                                                                next.insert(acc_name_for_request_max_change.clone(), target.value());
                                                                account_request_max_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                />
                                                <input
                                                    type="number"
                                                    class={classes!("w-32", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-2.5", "py-2", "text-xs")}
                                                    placeholder="账号间隔 ms"
                                                    value={selected_request_min_value.clone()}
                                                    oninput={{
                                                        let account_request_min_inputs = account_request_min_inputs.clone();
                                                        Callback::from(move |event: InputEvent| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                                let mut next = (*account_request_min_inputs).clone();
                                                                next.insert(acc_name_for_request_min_change.clone(), target.value());
                                                                account_request_min_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                />
                                                <select
                                                    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-xs")}
                                                    value={selected_proxy_value.clone()}
                                                    onchange={{
                                                        let account_proxy_inputs = account_proxy_inputs.clone();
                                                        Callback::from(move |event: Event| {
                                                            if let Some(target) = event.target_dyn_into::<HtmlSelectElement>() {
                                                                let mut next = (*account_proxy_inputs).clone();
                                                                next.insert(acc_name_for_proxy_change.clone(), target.value());
                                                                account_proxy_inputs.set(next);
                                                            }
                                                        })
                                                    }}
                                                >
                                                    <option value="inherit" selected={selected_proxy_value == "inherit"}>{ "继承 Provider Proxy" }</option>
                                                    <option value="direct" selected={selected_proxy_value == "direct"}>{ "Direct / 不走代理" }</option>
                                                    { for proxy_configs.iter().map(|proxy_config| {
                                                        let option_value = format!("fixed:{}", proxy_config.id);
                                                        html! {
                                                            <option value={option_value.clone()} selected={selected_proxy_value == option_value}>
                                                                { format!("固定到 {} · {}", proxy_config.name, proxy_config.proxy_url) }
                                                            </option>
                                                        }
                                                    }) }
                                                </select>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_save_account_settings.emit(acc_name_for_settings_save.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "处理中..." } else { "保存设置" } }
                                                </button>
                                                <button
                                                    class={classes!("btn-terminal")}
                                                    onclick={Callback::from(move |_| on_refresh_account.emit(acc_name_for_refresh.clone()))}
                                                    disabled={account_busy}
                                                >
                                                    { if account_busy { "处理中..." } else { "刷新状态" } }
                                                </button>
                                            </div>
                                            if show_spark_toggle {
                                                <button
                                                    class={classes!(
                                                        "btn-terminal",
                                                        if spark_mapping_enabled {
                                                            "btn-terminal-primary"
                                                        } else {
                                                            ""
                                                        }
                                                    )}
                                                    onclick={Callback::from(move |_| {
                                                        on_toggle_account_spark_mapping.emit((
                                                            acc_name_for_toggle.clone(),
                                                            !spark_mapping_enabled,
                                                        ))
                                                    })}
                                                    disabled={account_busy}
                                                    title="把客户端请求的 gpt-5.3-codex 映射到该账号上游的 gpt-5.3-codex-spark"
                                                >
                                                    {
                                                        if account_busy {
                                                            "切换中..."
                                                        } else if spark_mapping_enabled {
                                                            "Spark 映射已开"
                                                        } else {
                                                            "启用 Spark 映射"
                                                        }
                                                    }
                                                </button>
                                            }
                                            <button
                                                class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                onclick={Callback::from(move |_| on_delete.emit(acc_name_for_delete.clone()))}
                                            >
                                                { "删除" }
                                            </button>
                                        </div>
                                    </div>
                                }
                            }) }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-6", "text-sm", "text-[var(--muted)]")}>
                            { "当前还没有导入任何 Codex 账号。可以先导入账号，或者点击上方“刷新列表”确认后端是否已加载本地账号文件。" }
                        </div>
                    }
                </section>
                } // end TAB_ACCOUNTS

                // ── Usage Tab ──
                if *active_tab == TAB_USAGE {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Usage Events" }</h2>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                { format!("RPM {}", *usage_current_rpm) }
                            </span>
                            <span class={classes!("rounded-full", "border", "border-[var(--border)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                { format!("In Flight {}", *usage_current_in_flight) }
                            </span>
                            <button
                                class={classes!("btn-terminal")}
                                title="刷新事件"
                                aria-label="刷新事件"
                                onclick={{
                                    let reload_usage = reload_usage.clone();
                                    Callback::from(move |_| reload_usage.emit((None, None)))
                                }}
                                disabled={*usage_loading}
                            >
                                <i class={classes!("fas", if *usage_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_auto_auto]", "items-end")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "筛选 Key" }</span>
                            <select
                                key={format!("usage-filter-{}", (*usage_key_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_usage_key_filter_change}
                            >
                                <option value="" selected={(*usage_key_filter).is_empty()}>{ "全部" }</option>
                                { for keys.iter().map(|key_item| html! {
                                    <option
                                        value={key_item.id.clone()}
                                        selected={(*usage_key_filter).as_str() == key_item.id.as_str()}
                                    >
                                        { key_item.name.clone() }
                                    </option>
                                }) }
                            </select>
                        </label>
                        <span class={classes!("text-sm", "font-semibold", "text-[var(--muted)]")}>
                            { format!("{} 条", *usage_total) }
                        </span>
                        <span class={classes!("text-sm", "font-semibold", "text-[var(--muted)]")}>
                            { format!("第 {} 页", *usage_page) }
                        </span>
                    </div>

                    if *usage_loading {
                        <div class={classes!("mt-3", "inline-flex", "items-center", "gap-2", "text-xs", "text-[var(--muted)]")}>
                            <i class={classes!("fas", "fa-spinner", "animate-spin")} />
                            <span>{ "加载中" }</span>
                        </div>
                    }
                    if let Some(err) = (*usage_error).clone() {
                        <div class={classes!("mt-3", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                            { err }
                        </div>
                    }

                    <div class={classes!("mt-3", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div class={classes!("text-xs", "text-[var(--muted)]")}>
                            { "列较多，可用按钮或下方滚动条随时左右查看。" }
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                title="向左滚动"
                                aria-label="向左滚动"
                                onclick={{
                                    let scroll_usage_table_by = scroll_usage_table_by.clone();
                                    Callback::from(move |_| scroll_usage_table_by.emit(-320))
                                }}
                            >
                                <i class={classes!("fas", "fa-arrow-left")} />
                            </button>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                title="向右滚动"
                                aria-label="向右滚动"
                                onclick={{
                                    let scroll_usage_table_by = scroll_usage_table_by.clone();
                                    Callback::from(move |_| scroll_usage_table_by.emit(320))
                                }}
                            >
                                <i class={classes!("fas", "fa-arrow-right")} />
                            </button>
                        </div>
                    </div>

                    <div
                        ref={usage_scroll_top_ref}
                        class={classes!("mt-3", "overflow-x-auto", "overflow-y-hidden", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-2", "py-2")}
                        onscroll={on_usage_scroll_top}
                    >
                        <div
                            class={classes!("h-3", "rounded-full", "bg-[linear-gradient(90deg,rgba(37,99,235,0.18),rgba(16,185,129,0.22))]")}
                            style={format!("width: {}px;", (*usage_scroll_width).max(1))}
                        />
                    </div>

                    <div
                        ref={usage_scroll_bottom_ref}
                        class={classes!("mt-4", "overflow-x-auto", "rounded-xl", "border", "border-[var(--border)]")}
                        onscroll={on_usage_scroll_bottom}
                    >
                        <table class={classes!("min-w-[110rem]", "w-full", "text-sm")}>
                            <thead>
                                <tr class={classes!("text-left", "text-[var(--muted)]")}>
                                    <th class={classes!("py-2", "pr-3")}>{ "时间" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Key" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "号池" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "URL / Route" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Model" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Latency" }</th>
                                <th class={classes!("py-2", "pr-3")}>{ "IP / 属地" }</th>
                                <th class={classes!("py-2", "pr-3")}>{ "Tokens" }</th>
                                <th class={classes!("py-2", "pr-3")}>{ "Credit" }</th>
                                <th class={classes!("py-2", "pr-3")}>{ "最后一条内容" }</th>
                                <th class={classes!("py-2", "pr-3")}>{ "Headers" }</th>
                            </tr>
                        </thead>
                            <tbody>
                                if usage_events.is_empty() && !*loading && !*usage_loading && (*usage_error).is_none() {
                                    <tr class={classes!("border-t", "border-[var(--border)]")}>
                                        <td colspan="12" class={classes!("py-8", "text-center", "text-[var(--muted)]")}>{ "当前筛选下还没有 usage 事件" }</td>
                                    </tr>
                                } else {
                                    { for usage_events.iter().map(|event| {
                                        let event_id_for_detail = event.id.clone();
                                        let event_id_for_message = event.id.clone();
                                        let header_preview = "按需加载".to_string();
                                        let account_label = event.account_name.clone().unwrap_or_else(|| "legacy auth".to_string());
                                        let last_message_preview = usage_last_message_table_preview(event);
                                        html! {
                                            <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap")}>{ format_ms(event.created_at) }</td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[13rem]")}>
                                                    <div class={classes!("font-semibold", "text-[var(--text)]")}>{ event.key_name.clone() }</div>
                                                    <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>{ event.key_id.clone() }</div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[10rem]")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2.5", "py-1", "text-xs", "font-semibold", "text-emerald-700", "dark:text-emerald-200")}>
                                                        { account_label }
                                                    </span>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[22rem]")}>
                                                    <div class={classes!("flex", "items-start", "gap-2")}>
                                                        <span class={classes!("inline-flex", "rounded-full", "border", "border-sky-500/20", "bg-sky-500/10", "px-2", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-sky-700", "dark:text-sky-200")}>
                                                            { event.request_method.clone() }
                                                        </span>
                                                        <div class={classes!("min-w-0", "flex-1")}>
                                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                                <span class={classes!("truncate")} title={event.request_url.clone()}>{ event.request_url.clone() }</span>
                                                                { copy_icon_button(&event.request_url, &on_copy) }
                                                            </div>
                                                            <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                                                { format!("upstream {}", event.endpoint) }
                                                            </div>
                                                        </div>
                                                    </div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[11rem]")}>
                                                    <div>{ event.model.clone().unwrap_or_else(|| "-".to_string()) }</div>
                                                    if event.usage_missing {
                                                        <div class={classes!("mt-2", "inline-flex", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-2", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-amber-700", "dark:text-amber-200")}>
                                                            { "usage missing" }
                                                        </div>
                                                    }
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap")}>{ event.status_code }</td>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-violet-500/20", "bg-violet-500/10", "px-2.5", "py-1", "text-xs", "font-semibold", "text-violet-700", "dark:text-violet-200")}>
                                                        { format_latency_ms(event.latency_ms) }
                                                    </span>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[14rem]")}>
                                                    <div class={classes!("flex", "items-center", "gap-2")}>
                                                        <span>{ format!("{}/{}", event.client_ip, event.ip_region) }</span>
                                                        { copy_icon_button(&format!("{}/{}", event.client_ip, event.ip_region), &on_copy) }
                                                    </div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[12rem]")}>
                                                    <div class={classes!("grid", "gap-1", "text-xs", "text-[var(--muted)]")}>
                                                        <span>{ format!("Uncached {}", format_number_u64(event.input_uncached_tokens)) }</span>
                                                        <span>{ format!("Cached {}", format_number_u64(event.input_cached_tokens)) }</span>
                                                        <span>{ format!("Out {}", format_number_u64(event.output_tokens)) }</span>
                                                        <span class={classes!("font-semibold", "text-[var(--text)]")}>{ format!("Billable {}", format_number_u64(event.billable_tokens)) }</span>
                                                    </div>
                                                </td>
                                            <td class={classes!("py-3", "pr-3", "min-w-[10rem]")}>
                                                <div class={classes!("grid", "gap-1", "text-xs", "font-mono")}>
                                                    <span class={classes!("font-semibold", "text-[var(--text)]")}>
                                                        { event.credit_usage.map(format_credit4).unwrap_or_else(|| "-".to_string()) }
                                                    </span>
                                                    if event.credit_usage_missing {
                                                        <span class={classes!("text-amber-700", "dark:text-amber-200")}>{ "missing" }</span>
                                                    }
                                                </div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[18rem]")}>
                                                <div class={classes!("max-w-[18rem]", "overflow-hidden", "whitespace-normal", "break-words", "text-xs", "leading-5", "text-[var(--muted)]")}>
                                                    { last_message_preview }
                                                </div>
                                                    <button
                                                        type="button"
                                                        class={classes!(
                                                            "mt-2",
                                                            "inline-flex",
                                                            "items-center",
                                                            "gap-2",
                                                            "rounded-lg",
                                                            "border",
                                                            "border-[var(--border)]",
                                                            "bg-[var(--surface)]",
                                                            "px-2.5",
                                                            "py-1.5",
                                                            "text-[11px]",
                                                            "font-semibold",
                                                            "text-[var(--muted)]",
                                                            "transition-colors",
                                                            "hover:text-[var(--primary)]",
                                                            "hover:bg-[var(--surface-alt)]"
                                                        )}
                                                        title="查看最后一条内容全文"
                                                        aria-label="查看最后一条内容全文"
                                                        onclick={{
                                                            let open_usage_detail = open_usage_detail.clone();
                                                            Callback::from(move |_| open_usage_detail.emit(event_id_for_message.clone()))
                                                        }}
                                                    >
                                                        <i class={classes!("fas", "fa-expand")} />
                                                        <span>{ "查看全文" }</span>
                                                    </button>
                                                </td>
                                                <td class={classes!("py-3", "pr-3")}>
                                                    <button
                                                        type="button"
                                                        class={classes!(
                                                            "inline-flex",
                                                            "h-9",
                                                            "w-9",
                                                            "items-center",
                                                            "justify-center",
                                                            "rounded-xl",
                                                            "border",
                                                            "border-[var(--border)]",
                                                            "bg-[var(--surface)]",
                                                            "text-[var(--muted)]",
                                                            "transition-colors",
                                                            "hover:text-[var(--primary)]",
                                                            "hover:bg-[var(--surface-alt)]"
                                                        )}
                                                        title="查看请求详情"
                                                        aria-label="查看请求详情"
                                                        onclick={{
                                                            let open_usage_detail = open_usage_detail.clone();
                                                            Callback::from(move |_| open_usage_detail.emit(event_id_for_detail.clone()))
                                                        }}
                                                    >
                                                        <i class={classes!("fas", "fa-bars-staggered")}></i>
                                                    </button>
                                                    <div class={classes!("mt-2", "max-w-[12rem]", "truncate", "font-mono", "text-[11px]", "text-[var(--muted)]")} title={header_preview.clone()}>
                                                        { header_preview }
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }) }
                                }
                            </tbody>
                        </table>
                    </div>

                    <div class={classes!("mt-5")}>
                        <Pagination current_page={*usage_page} total_pages={usage_total_pages} on_page_change={on_usage_page_change} />
                    </div>
                </section>
                } // end TAB_USAGE

                // ── Requests Tab ──
                if *active_tab == TAB_REQUESTS {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Token Wishes" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { "只有在这里审核通过后，系统才会真正创建 key 并通过邮件发给申请人。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_token_requests = reload_token_requests.clone();
                                Callback::from(move |_| reload_token_requests.emit((None, None)))
                            }}
                            disabled={*token_request_loading}
                        >
                            <i class={classes!("fas", if *token_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("token-request-filter-{}", (*token_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_token_request_status_filter_change}
                            >
                                <option value="" selected={(*token_request_status_filter).is_empty()}>{ "全部" }</option>
                                <option value="pending" selected={*token_request_status_filter == "pending"}>{ "pending" }</option>
                                <option value="failed" selected={*token_request_status_filter == "failed"}>{ "failed" }</option>
                                <option value="issued" selected={*token_request_status_filter == "issued"}>{ "issued" }</option>
                                <option value="rejected" selected={*token_request_status_filter == "rejected"}>{ "rejected" }</option>
                            </select>
                        </label>
                    </div>

                    if token_requests.is_empty() && !*token_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有 token 许愿。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for token_requests.iter().map(|item| {
                                let request_id = item.request_id.clone();
                                let approve_request_id = item.request_id.clone();
                                let reject_request_id = item.request_id.clone();
                                let approve_cb = on_approve_token_request.clone();
                                let reject_cb = on_reject_token_request.clone();
                                let action_busy = token_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                    "pending" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                    "failed" => classes!("bg-red-500/10", "text-red-700", "dark:text-red-200", "border-red-500/20"),
                                    "issued" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    "rejected" => classes!("bg-slate-500/10", "text-slate-700", "dark:text-slate-200", "border-slate-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class)}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.requester_email.clone() }</span>
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right")}>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "申请 token" }</div>
                                                <div class={classes!("mt-1", "font-mono", "text-2xl", "font-black")}>{ format_number_u64(item.requested_quota_billable_limit) }</div>
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "缘由" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.request_reason.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(issued_key_name) = item.issued_key_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已发放 Key" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>
                                                            { format!("{} ({})", issued_key_name, item.issued_key_id.clone().unwrap_or_default()) }
                                                        </div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未处理".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                                        onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { if action_busy { "处理中..." } else { "批准并发放" } }
                                                    </button>
                                                }
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                        onclick={Callback::from(move |_| reject_cb.emit(reject_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { "拒绝" }
                                                    </button>
                                                }
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination current_page={*token_request_page} total_pages={token_request_total_pages} on_page_change={on_token_request_page_change} />
                    </div>
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Account Contributions" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { "公开页提交的 Codex 账号贡献申请会先进入这里；只有审核通过后，系统才会导入账号并发放绑定该账号路由的 token。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_account_contribution_requests = reload_account_contribution_requests.clone();
                                Callback::from(move |_| reload_account_contribution_requests.emit((None, None)))
                            }}
                            disabled={*account_contribution_request_loading}
                        >
                            <i class={classes!("fas", if *account_contribution_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("account-contribution-filter-{}", (*account_contribution_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_account_contribution_status_filter_change}
                            >
                                <option value="" selected={(*account_contribution_request_status_filter).is_empty()}>{ "全部" }</option>
                                <option value="pending" selected={*account_contribution_request_status_filter == "pending"}>{ "pending" }</option>
                                <option value="failed" selected={*account_contribution_request_status_filter == "failed"}>{ "failed" }</option>
                                <option value="issued" selected={*account_contribution_request_status_filter == "issued"}>{ "issued" }</option>
                                <option value="rejected" selected={*account_contribution_request_status_filter == "rejected"}>{ "rejected" }</option>
                            </select>
                        </label>
                    </div>

                    if account_contribution_requests.is_empty() && !*account_contribution_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有账号贡献申请。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for account_contribution_requests.iter().map(|item| {
                                let request_id = item.request_id.clone();
                                let approve_request_id = item.request_id.clone();
                                let reject_request_id = item.request_id.clone();
                                let approve_cb = on_approve_account_contribution_request.clone();
                                let reject_cb = on_reject_account_contribution_request.clone();
                                let on_copy = on_copy.clone();
                                let action_busy =
                                    account_contribution_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                    "pending" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                    "failed" => classes!("bg-red-500/10", "text-red-700", "dark:text-red-200", "border-red-500/20"),
                                    "issued" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    "rejected" => classes!("bg-slate-500/10", "text-slate-700", "dark:text-slate-200", "border-slate-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class.clone())}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.account_name.clone() }</span>
                                                    <span class={classes!("text-xs", "text-[var(--muted)]")}>{ item.requester_email.clone() }</span>
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right", "space-y-1")}>
                                                if let Some(github_id) = item.github_id.clone() {
                                                    <div class={classes!("text-sm", "font-semibold")}>{ format!("@{}", github_id) }</div>
                                                }
                                                if let Some(account_id) = item.account_id.clone() {
                                                    <div class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ account_id }</div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "留言" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.contributor_message.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(imported_account_name) = item.imported_account_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已导入账号" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>{ imported_account_name }</div>
                                                    </div>
                                                }
                                                if let Some(issued_key_name) = item.issued_key_name.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已发放 Key" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>
                                                            { format!("{} ({})", issued_key_name, item.issued_key_id.clone().unwrap_or_default()) }
                                                        </div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-3")}>
                                            { copyable_token_preview("access_token", &item.access_token, &on_copy) }
                                            { copyable_token_preview("id_token", &item.id_token, &on_copy) }
                                            { copyable_token_preview("refresh_token", &item.refresh_token, &on_copy) }
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未处理".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                                        onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { if action_busy { "处理中..." } else { "批准并导入" } }
                                                    </button>
                                                }
                                                if item.status == "pending" || item.status == "failed" {
                                                    <button
                                                        class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                        onclick={Callback::from(move |_| reject_cb.emit(reject_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { "拒绝" }
                                                    </button>
                                                }
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination
                            current_page={*account_contribution_request_page}
                            total_pages={account_contribution_request_total_pages}
                            on_page_change={on_account_contribution_page_change}
                        />
                    </div>
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>{ "Sponsors" }</h2>
                            <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { "这批请求是“先填邮箱，再发付款说明邮件”的人工确认流。你确认对方已经按邮件说明完成赞助后，再在这里标记通过。" }
                            </p>
                        </div>
                        <button
                            class={classes!("btn-terminal")}
                            onclick={{
                                let reload_sponsor_requests = reload_sponsor_requests.clone();
                                Callback::from(move |_| reload_sponsor_requests.emit((None, None)))
                            }}
                            disabled={*sponsor_request_loading}
                        >
                            <i class={classes!("fas", if *sponsor_request_loading { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                        </button>
                    </div>

                    <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,16rem)_auto]")}>
                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "状态" }</span>
                            <select
                                key={format!("sponsor-filter-{}", (*sponsor_request_status_filter).clone())}
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                onchange={on_sponsor_request_status_filter_change}
                            >
                                <option value="" selected={(*sponsor_request_status_filter).is_empty()}>{ "全部" }</option>
                                <option value="submitted" selected={*sponsor_request_status_filter == "submitted"}>{ "submitted" }</option>
                                <option value="payment_email_sent" selected={*sponsor_request_status_filter == "payment_email_sent"}>{ "payment_email_sent" }</option>
                                <option value="approved" selected={*sponsor_request_status_filter == "approved"}>{ "approved" }</option>
                            </select>
                        </label>
                    </div>

                    if sponsor_requests.is_empty() && !*sponsor_request_loading {
                        <div class={classes!("mt-4", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-4", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前筛选下还没有 Sponsor 请求。" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "space-y-3")}>
                            { for sponsor_requests.iter().map(|item| {
                                let request_id = item.request_id.clone();
                                let approve_request_id = item.request_id.clone();
                                let delete_request_id = item.request_id.clone();
                                let approve_cb = on_approve_sponsor_request.clone();
                                let delete_cb = on_delete_sponsor_request.clone();
                                let action_busy = sponsor_request_action_inflight.contains(&request_id);
                                let status_class = match item.status.as_str() {
                                    "submitted" => classes!("bg-amber-500/10", "text-amber-700", "dark:text-amber-200", "border-amber-500/20"),
                                    "payment_email_sent" => classes!("bg-sky-500/10", "text-sky-700", "dark:text-sky-200", "border-sky-500/20"),
                                    "approved" => classes!("bg-emerald-500/10", "text-emerald-700", "dark:text-emerald-200", "border-emerald-500/20"),
                                    _ => classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]", "border-[var(--border)]"),
                                };
                                html! {
                                    <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                                        <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("min-w-0", "space-y-1")}>
                                                <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "px-2.5", "py-1", "text-xs", "font-semibold", status_class.clone())}>
                                                        { item.status.clone() }
                                                    </span>
                                                    <span class={classes!("font-semibold")}>{ item.requester_email.clone() }</span>
                                                    <span class={classes!("text-xs", "font-mono", "text-[var(--muted)]")}>{ item.request_id.clone() }</span>
                                                </div>
                                                <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                    { format!("{} / {} · created {}", item.client_ip, item.ip_region, format_ms(item.created_at)) }
                                                </div>
                                            </div>
                                            <div class={classes!("text-right", "space-y-1")}>
                                                if let Some(display_name) = item.display_name.clone() {
                                                    <div class={classes!("text-sm", "font-semibold")}>{ display_name }</div>
                                                }
                                                if let Some(github_id) = item.github_id.clone() {
                                                    <div class={classes!("text-xs", "font-semibold", "text-[var(--muted)]")}>{ format!("@{}", github_id) }</div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "grid", "gap-3", "xl:grid-cols-[minmax(0,1fr)_minmax(0,1fr)]")}>
                                            <div>
                                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "留言" }</div>
                                                <div class={classes!("mt-2", "whitespace-pre-wrap", "break-words", "text-sm", "leading-6", "text-[var(--text)]")}>
                                                    { item.sponsor_message.clone() }
                                                </div>
                                            </div>
                                            <div class={classes!("space-y-2", "text-sm")}>
                                                if let Some(frontend_page_url) = item.frontend_page_url.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "页面" }</div>
                                                        <div class={classes!("mt-1", "break-all", "text-[var(--text)]")}>{ frontend_page_url }</div>
                                                    </div>
                                                }
                                                if let Some(payment_email_sent_at) = item.payment_email_sent_at {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "付款说明邮件" }</div>
                                                        <div class={classes!("mt-1", "text-[var(--text)]")}>{ format_ms(payment_email_sent_at) }</div>
                                                    </div>
                                                }
                                                if let Some(admin_note) = item.admin_note.clone() {
                                                    <div>
                                                        <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Admin Note" }</div>
                                                        <div class={classes!("mt-1", "whitespace-pre-wrap", "break-words", "text-[var(--text)]")}>{ admin_note }</div>
                                                    </div>
                                                }
                                                if let Some(failure_reason) = item.failure_reason.clone() {
                                                    <div class={classes!("rounded-lg", "border", "border-red-400/25", "bg-red-500/8", "px-3", "py-2", "text-red-700", "dark:text-red-200")}>
                                                        { failure_reason }
                                                    </div>
                                                }
                                            </div>
                                        </div>

                                        <div class={classes!("mt-4", "flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { item.processed_at.map(format_ms).map(|value| format!("processed {}", value)).unwrap_or_else(|| "尚未确认".to_string()) }
                                            </div>
                                            <div class={classes!("flex", "items-center", "gap-2")}>
                                                if item.status != "approved" {
                                                    <button
                                                        class={classes!("btn-terminal", "btn-terminal-primary")}
                                                        onclick={Callback::from(move |_| approve_cb.emit(approve_request_id.clone()))}
                                                        disabled={action_busy}
                                                    >
                                                        { if action_busy { "处理中..." } else { "标记已确认" } }
                                                    </button>
                                                }
                                                <button
                                                    class={classes!("btn-terminal", "!text-red-600", "dark:!text-red-300")}
                                                    onclick={Callback::from(move |_| delete_cb.emit(delete_request_id.clone()))}
                                                    disabled={action_busy}
                                                >
                                                    { "删除" }
                                                </button>
                                            </div>
                                        </div>
                                    </article>
                                }
                            }) }
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination
                            current_page={*sponsor_request_page}
                            total_pages={sponsor_request_total_pages}
                            on_page_change={on_sponsor_request_page_change}
                        />
                    </div>
                </section>
                } // end TAB_REQUESTS

            </div>

            { usage_detail_modal.unwrap_or_default() }

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
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_last_message_preview_prefers_summary_content() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("hello".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_preview(&event), "hello");
    }

    #[test]
    fn usage_last_message_preview_falls_back_for_blank_content() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("   ".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_preview(&event), "-");
    }

    #[test]
    fn usage_last_message_table_preview_collapses_whitespace_and_truncates() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some(
                "first line\n\nsecond   line with   extra spaces and a very long suffix that \
                 should be truncated in the table preview because it keeps going with more and \
                 more text until the shortened variant must end with ellipsis"
                    .to_string(),
            ),
            ..AdminLlmGatewayUsageEventView::default()
        };

        let preview = usage_last_message_table_preview(&event);

        assert!(!preview.contains('\n'));
        assert!(preview.contains("first line second line with extra spaces"));
        assert!(preview.ends_with("..."));
        assert!(preview.chars().count() <= 123);
    }

    #[test]
    fn usage_last_message_table_preview_keeps_short_single_line_text() {
        let event = AdminLlmGatewayUsageEventView {
            last_message_content: Some("short text".to_string()),
            ..AdminLlmGatewayUsageEventView::default()
        };

        assert_eq!(usage_last_message_table_preview(&event), "short text");
    }
}
