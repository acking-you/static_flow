use std::collections::HashSet;

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
        admin_reject_llm_gateway_account_contribution_request,
        admin_reject_llm_gateway_token_request, create_admin_llm_gateway_key,
        delete_admin_llm_gateway_account, delete_admin_llm_gateway_key,
        fetch_admin_llm_gateway_account_contribution_requests, fetch_admin_llm_gateway_accounts,
        fetch_admin_llm_gateway_config, fetch_admin_llm_gateway_keys,
        fetch_admin_llm_gateway_token_requests, fetch_admin_llm_gateway_usage_events,
        import_admin_llm_gateway_account, patch_admin_llm_gateway_account,
        patch_admin_llm_gateway_key, update_admin_llm_gateway_config, AccountSummaryView,
        AdminLlmGatewayAccountContributionRequestView,
        AdminLlmGatewayAccountContributionRequestsQuery, AdminLlmGatewayKeyView,
        AdminLlmGatewayTokenRequestView, AdminLlmGatewayTokenRequestsQuery,
        AdminLlmGatewayUsageEventView, AdminLlmGatewayUsageEventsQuery, LlmGatewayRuntimeConfig,
    },
    components::pagination::Pagination,
    router::Route,
};

const USAGE_PAGE_SIZE: usize = 20;
const TOKEN_REQUEST_PAGE_SIZE: usize = 20;
const ACCOUNT_CONTRIBUTION_REQUEST_PAGE_SIZE: usize = 20;

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

fn format_latency_ms(latency_ms: i32) -> String {
    format!("{} ms", latency_ms.max(0))
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

#[derive(Properties, PartialEq)]
struct KeyEditorCardProps {
    key_item: AdminLlmGatewayKeyView,
    on_changed: Callback<()>,
    on_refresh: Callback<(String, String)>,
    on_copy: Callback<(String, String)>,
    refreshing: bool,
    account_names: Vec<String>,
}

#[function_component(KeyEditorCard)]
fn key_editor_card(props: &KeyEditorCardProps) -> Html {
    let key_item = props.key_item.clone();
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
    let fixed_account_name = use_state(|| key_item.fixed_account_name.clone().unwrap_or_default());
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        // Reset editor controls whenever the parent list refreshes this card.
        let key_item = props.key_item.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let fixed_account_name = fixed_account_name.clone();
        use_effect_with(props.key_item.clone(), move |_| {
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
            fixed_account_name.set(key_item.fixed_account_name.clone().unwrap_or_default());
            || ()
        });
    }

    let on_save = {
        let key_id = key_item.id.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let route_strategy = route_strategy.clone();
        let fixed_account_name = fixed_account_name.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let name_value = (*name).trim().to_string();
            let quota_value = (*quota).trim().parse::<u64>();
            let public_visible_value = *public_visible;
            let status_value = (*status).clone();
            let route_strategy_value = (*route_strategy).clone();
            let fixed_account_name_value = (*fixed_account_name).clone();
            let saving = saving.clone();
            let feedback = feedback.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                if *saving {
                    return;
                }
                let Ok(quota_value) = quota_value else {
                    feedback.set(Some("额度必须是正整数".to_string()));
                    return;
                };
                saving.set(true);
                match patch_admin_llm_gateway_key(
                    &key_id,
                    Some(&name_value),
                    Some(&status_value),
                    Some(public_visible_value),
                    Some(quota_value),
                    Some(&route_strategy_value),
                    Some(&fixed_account_name_value),
                )
                .await
                {
                    Ok(_) => {
                        feedback.set(Some("已保存".to_string()));
                        on_changed.emit(());
                    },
                    Err(err) => feedback.set(Some(err)),
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
            let feedback = feedback.clone();
            let saving = saving.clone();
            let on_changed = on_changed.clone();
            wasm_bindgen_futures::spawn_local(async move {
                saving.set(true);
                match delete_admin_llm_gateway_key(&key_id).await {
                    Ok(_) => {
                        feedback.set(Some("已删除".to_string()));
                        on_changed.emit(());
                    },
                    Err(err) => feedback.set(Some(err)),
                }
                saving.set(false);
            });
        })
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
                <code class={classes!("break-all")}>{ key_item.secret.clone() }</code>
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
                        <span class={classes!("text-[var(--muted)]")}>{ "账号" }</span>
                        <select
                            key={format!("{}-fixed-{}", key_item.id, (*fixed_account_name).clone())}
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
                            { for props.account_names.iter().map(|acc_name| html! {
                                <option value={acc_name.clone()} selected={*fixed_account_name == *acc_name}>{ acc_name.clone() }</option>
                            }) }
                        </select>
                    </label>
                }
                <span class={classes!("text-xs", "text-[var(--muted)]")}>
                    { if *route_strategy == "fixed" {
                        format!("绑定: {}", if (*fixed_account_name).is_empty() { "未选择" } else { &*fixed_account_name })
                    } else {
                        "自动选择剩余额度最多的账号".to_string()
                    }}
                </span>
            </div>

            <div class={classes!("mt-3", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]")}>
                <span>{ format!("剩余 {}", key_item.remaining_billable) }</span>
                <span>{ format!("输入 {}", key_item.usage_input_uncached_tokens) }</span>
                <span>{ format!("缓存 {}", key_item.usage_input_cached_tokens) }</span>
                <span>{ format!("输出 {}", key_item.usage_output_tokens) }</span>
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
    let usage_events = use_state(Vec::<AdminLlmGatewayUsageEventView>::new);
    let usage_total = use_state(|| 0_usize);
    let usage_page = use_state(|| 1_usize);
    let usage_loading = use_state(|| false);
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
    let selected_usage_event = use_state(|| None::<AdminLlmGatewayUsageEventView>);
    let usage_scroll_top_ref = use_node_ref();
    let usage_scroll_bottom_ref = use_node_ref();
    let usage_scroll_width = use_state(|| 1_i32);
    let loading = use_state(|| true);
    let load_error = use_state(|| None::<String>);
    let ttl_input = use_state(|| "60".to_string());
    let saving_ttl = use_state(|| false);
    let create_name = use_state(String::new);
    let create_quota = use_state(|| "100000".to_string());
    let create_public = use_state(|| true);
    let creating = use_state(|| false);
    let refreshing_key_id = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let accounts = use_state(Vec::<AccountSummaryView>::new);
    let import_name = use_state(String::new);
    let import_id_token = use_state(String::new);
    let import_access_token = use_state(String::new);
    let import_refresh_token = use_state(String::new);
    let import_account_id = use_state(String::new);
    let importing = use_state(|| false);
    let account_action_inflight = use_state(HashSet::<String>::new);

    // Usage events are fetched independently so paging and key filters do not
    // need to re-fetch the rest of the admin page chrome.
    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_total = usage_total.clone();
        let usage_page = usage_page.clone();
        let usage_loading = usage_loading.clone();
        let usage_key_filter = usage_key_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |(requested_page, override_key_id): (Option<usize>, Option<String>)| {
            let usage_events = usage_events.clone();
            let usage_total = usage_total.clone();
            let usage_page = usage_page.clone();
            let usage_loading = usage_loading.clone();
            let usage_key_filter = usage_key_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*usage_page).max(1);
            let selected_key_id = override_key_id.unwrap_or_else(|| (*usage_key_filter).clone());
            usage_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminLlmGatewayUsageEventsQuery {
                    key_id: (!selected_key_id.is_empty()).then_some(selected_key_id),
                    limit: Some(USAGE_PAGE_SIZE),
                    offset: Some((page - 1) * USAGE_PAGE_SIZE),
                };
                match fetch_admin_llm_gateway_usage_events(&query).await {
                    Ok(resp) => {
                        usage_total.set(resp.total);
                        usage_events.set(resp.events);
                        usage_page.set(page);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
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

    // This reload keeps the inventory, runtime config, and the current usage
    // page in sync after any admin write operation.
    let reload = {
        let config = config.clone();
        let keys = keys.clone();
        let loading = loading.clone();
        let load_error = load_error.clone();
        let ttl_input = ttl_input.clone();
        let usage_events = usage_events.clone();
        let usage_total = usage_total.clone();
        let usage_page = usage_page.clone();
        let usage_key_filter = usage_key_filter.clone();
        let accounts = accounts.clone();
        Callback::from(move |_| {
            let config = config.clone();
            let keys = keys.clone();
            let loading = loading.clone();
            let load_error = load_error.clone();
            let ttl_input = ttl_input.clone();
            let usage_events = usage_events.clone();
            let usage_total = usage_total.clone();
            let usage_page = usage_page.clone();
            let usage_key_filter = usage_key_filter.clone();
            let accounts = accounts.clone();
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let current_key_filter = (*usage_key_filter).clone();
                let current_page = (*usage_page).max(1);
                let result = async {
                    let cfg = fetch_admin_llm_gateway_config().await?;
                    let keys_resp = fetch_admin_llm_gateway_keys().await?;
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
                    let usage_query = AdminLlmGatewayUsageEventsQuery {
                        key_id: (!effective_key_filter.is_empty())
                            .then_some(effective_key_filter.clone()),
                        limit: Some(USAGE_PAGE_SIZE),
                        offset: Some((current_page - 1) * USAGE_PAGE_SIZE),
                    };
                    let usage_resp = fetch_admin_llm_gateway_usage_events(&usage_query).await?;
                    let accounts_resp = fetch_admin_llm_gateway_accounts().await.ok();
                    Ok::<_, String>((
                        cfg,
                        keys_resp.keys,
                        effective_key_filter,
                        usage_resp,
                        accounts_resp,
                    ))
                }
                .await;

                match result {
                    Ok((cfg, key_items, effective_key_filter, usage_resp, accounts_resp)) => {
                        ttl_input.set(cfg.auth_cache_ttl_seconds.to_string());
                        config.set(Some(cfg));
                        keys.set(key_items);
                        usage_key_filter.set(effective_key_filter);
                        usage_total.set(usage_resp.total);
                        usage_events.set(usage_resp.events);
                        if let Some(acc_resp) = accounts_resp {
                            accounts.set(acc_resp.accounts);
                        }
                        load_error.set(None);
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
        use_effect_with((), move |_| {
            reload.emit(());
            reload_token_requests.emit((Some(1), Some(String::new())));
            reload_account_contribution_requests.emit((Some(1), Some(String::new())));
            || ()
        });
    }

    let on_save_ttl = {
        let ttl_input = ttl_input.clone();
        let saving_ttl = saving_ttl.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        Callback::from(move |_| {
            let ttl = (*ttl_input).trim().parse::<u64>();
            let saving_ttl = saving_ttl.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(ttl) = ttl else {
                    load_error.set(Some("TTL 必须是正整数".to_string()));
                    return;
                };
                saving_ttl.set(true);
                match update_admin_llm_gateway_config(ttl).await {
                    Ok(_) => {
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                saving_ttl.set(false);
            });
        })
    };

    let on_create = {
        let create_name = create_name.clone();
        let create_quota = create_quota.clone();
        let create_public = create_public.clone();
        let creating = creating.clone();
        let load_error = load_error.clone();
        let reload = reload.clone();
        let usage_page = usage_page.clone();
        Callback::from(move |_| {
            let name = (*create_name).trim().to_string();
            let quota = (*create_quota).trim().parse::<u64>();
            let public_visible = *create_public;
            let creating = creating.clone();
            let load_error = load_error.clone();
            let reload = reload.clone();
            let create_name = create_name.clone();
            let usage_page = usage_page.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let Ok(quota) = quota else {
                    load_error.set(Some("主额度必须是正整数".to_string()));
                    return;
                };
                creating.set(true);
                match create_admin_llm_gateway_key(&name, quota, public_visible).await {
                    Ok(_) => {
                        create_name.set(String::new());
                        usage_page.set(1);
                        load_error.set(None);
                        reload.emit(());
                    },
                    Err(err) => load_error.set(Some(err)),
                }
                creating.set(false);
            });
        })
    };

    // A per-card refresh avoids reloading unrelated state while re-reading the
    // latest counters for a single key.
    let on_refresh_key = {
        let keys = keys.clone();
        let load_error = load_error.clone();
        let refreshing_key_id = refreshing_key_id.clone();
        Callback::from(move |(key_id, _key_name): (String, String)| {
            refreshing_key_id.set(Some(key_id.clone()));
            let keys = keys.clone();
            let load_error = load_error.clone();
            let refreshing_key_id = refreshing_key_id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_llm_gateway_keys().await {
                    Ok(resp) => {
                        keys.set(resp.keys);
                        load_error.set(None);
                    },
                    Err(err) => load_error.set(Some(err)),
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

                match patch_admin_llm_gateway_account(&account_name, enabled).await {
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

    let on_copy = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            toast.set(Some((format!("已复制{}", label), false)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(1800, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
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
                        <h1 class={classes!("m-0", "text-2xl", "font-bold")}>
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

                    <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2", "xl:grid-cols-4")}>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key 总数" }</div>
                            <div class={classes!("mt-1", "text-2xl", "font-black")}>{ keys.len() }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "公开" }</div>
                            <div class={classes!("mt-1", "text-2xl", "font-black")}>{ public_visible_count }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Active" }</div>
                            <div class={classes!("mt-1", "text-2xl", "font-black")}>{ active_key_count }</div>
                        </div>
                        <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                            <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余额度" }</div>
                            <div class={classes!("mt-1", "text-2xl", "font-black")}>{ total_remaining }</div>
                        </div>
                    </div>
                </section>

                <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Runtime TTL" }</h2>
                        <div class={classes!("mt-3", "grid", "gap-3", "md:grid-cols-[minmax(0,1fr)_auto]")}>
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
                            <div class={classes!("flex", "items-end")}>
                                <button class={classes!("btn-terminal", "btn-terminal-primary", "w-full", "md:w-auto")} onclick={on_save_ttl} disabled={*saving_ttl}>
                                    { if *saving_ttl { "保存中..." } else { "保存" } }
                                </button>
                            </div>
                        </div>
                        if let Some(cfg) = (*config).clone() {
                            <p class={classes!("mt-3", "m-0", "text-xs", "text-[var(--muted)]")}>
                                { format!("当前生效：{} 秒", cfg.auth_cache_ttl_seconds) }
                            </p>
                        }
                    </section>

                    <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                        <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Create Key" }</h2>
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

                // === Codex Accounts ===
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Codex Accounts" }</h2>
                    <p class={classes!("mt-1", "m-0", "text-xs", "text-[var(--muted)]")}>
                        { format!("已导入 {} 个账号，文件存储在 ~/.static-flow/auths/", accounts.len()) }
                    </p>

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

                    // Account list
                    if !accounts.is_empty() {
                        <div class={classes!("mt-4", "space-y-2")}>
                            { for accounts.iter().map(|acc| {
                                let acc_name_for_toggle = acc.name.clone();
                                let acc_name_for_delete = acc.name.clone();
                                let acc_name = acc.name.clone();
                                let acc_status = acc.status.clone();
                                let acc_plan_type = acc.plan_type.clone();
                                let acc_account_id = acc.account_id.clone();
                                let spark_mapping_enabled = acc.map_gpt53_codex_to_spark;
                                let on_delete = on_delete_account.clone();
                                let on_toggle_account_spark_mapping =
                                    on_toggle_account_spark_mapping.clone();
                                let primary_pct = acc.primary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let secondary_pct = acc.secondary_remaining_percent
                                    .map(|v| format!("{:.0}%", v))
                                    .unwrap_or_else(|| "-".to_string());
                                let is_pro = is_gpt_pro_account(acc_plan_type.as_deref());
                                let show_spark_toggle = is_pro || spark_mapping_enabled;
                                let spark_toggle_inflight =
                                    (*account_action_inflight).contains(&acc_name);
                                html! {
                                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "rounded-lg", "border", "border-[var(--border)]", "px-4", "py-3", "flex-wrap")}>
                                        <div class={classes!("flex", "items-center", "gap-3")}>
                                            <div class={key_status_badge(&acc_status)}>{ acc_status.clone() }</div>
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
                                        <div class={classes!("flex", "items-center", "gap-3")}>
                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("5h {} / wk {}", primary_pct, secondary_pct) }
                                            </span>
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
                                                    disabled={spark_toggle_inflight}
                                                    title="把客户端请求的 gpt-5.3-codex 映射到该账号上游的 gpt-5.3-codex-spark"
                                                >
                                                    {
                                                        if spark_toggle_inflight {
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
                    }
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Account Contributions" }</h2>
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
                            <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Key Inventory" }</h2>
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
                                        refreshing={(*refreshing_key_id).as_deref() == Some(key_item.id.as_str())}
                                        account_names={accounts.iter().map(|a| a.name.clone()).collect::<Vec<_>>()}
                                    />
                                }) }
                            }
                        </div>
                </section>

                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Token Wishes" }</h2>
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
                                                <div class={classes!("mt-1", "text-2xl", "font-black")}>{ item.requested_quota_billable_limit }</div>
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
                        <h2 class={classes!("m-0", "text-lg", "font-bold")}>{ "Usage Events" }</h2>
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

                    <div
                        ref={usage_scroll_top_ref}
                        class={classes!("mt-3", "overflow-x-auto", "overflow-y-hidden", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-1", "py-1")}
                        onscroll={on_usage_scroll_top}
                    >
                        <div
                            class={classes!("h-[1px]")}
                            style={format!("width: {}px;", (*usage_scroll_width).max(1))}
                        />
                    </div>

                    <div
                        ref={usage_scroll_bottom_ref}
                        class={classes!("mt-4", "overflow-x-auto")}
                        onscroll={on_usage_scroll_bottom}
                    >
                        <table class={classes!("w-full", "text-sm")}>
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
                                    <th class={classes!("py-2", "pr-3")}>{ "最后一条内容" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Headers" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                if usage_events.is_empty() && !*loading && !*usage_loading {
                                    <tr class={classes!("border-t", "border-[var(--border)]")}>
                                        <td colspan="11" class={classes!("py-8", "text-center", "text-[var(--muted)]")}>{ "当前筛选下还没有 usage 事件" }</td>
                                    </tr>
                                } else {
                                    { for usage_events.iter().map(|event| {
                                        let event_for_detail_modal = event.clone();
                                        let event_for_message_modal = event.clone();
                                        let header_preview = pretty_headers_json(&event.request_headers_json);
                                        let account_label = event.account_name.clone().unwrap_or_else(|| "legacy auth".to_string());
                                        let last_message_full = event.last_message_content.clone().unwrap_or_else(|| "-".to_string());
                                        let last_message_preview = preview_text(&last_message_full, 120);
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
                                                        <span>{ format!("Uncached {}", event.input_uncached_tokens) }</span>
                                                        <span>{ format!("Cached {}", event.input_cached_tokens) }</span>
                                                        <span>{ format!("Out {}", event.output_tokens) }</span>
                                                        <span class={classes!("font-semibold", "text-[var(--text)]")}>{ format!("Billable {}", event.billable_tokens) }</span>
                                                    </div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[18rem]")}>
                                                    <div class={classes!("max-w-[18rem]", "whitespace-pre-wrap", "break-words", "text-xs", "leading-6", "text-[var(--muted)]")} title={last_message_full.clone()}>
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
                                                            let selected_usage_event = selected_usage_event.clone();
                                                            Callback::from(move |_| selected_usage_event.set(Some(event_for_message_modal.clone())))
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
                                                            let selected_usage_event = selected_usage_event.clone();
                                                            Callback::from(move |_| selected_usage_event.set(Some(event_for_detail_modal.clone())))
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
            </div>

            if let Some(event) = (*selected_usage_event).clone() {
                <div
                    class={classes!(
                        "fixed",
                        "inset-0",
                        "z-[90]",
                        "flex",
                        "items-center",
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
                            "h-[min(90vh,56rem)]",
                            "min-h-0",
                            "max-w-4xl",
                            "flex-col",
                            "overflow-hidden",
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
                            <div class={classes!("flex", "gap-2")}>
                                <button
                                    class={classes!("btn-terminal")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let headers_json = event.request_headers_json.clone();
                                        Callback::from(move |_| on_copy.emit(("Headers".to_string(), headers_json.clone())))
                                    }}
                                >
                                    { "复制 JSON" }
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

                        <div class={classes!("mt-4", "grid", "shrink-0", "gap-3", "lg:grid-cols-5")}>
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
                        </div>

                        <div class={classes!("mt-4", "shrink-0")}>
                            <div class={classes!("mb-2", "text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Last Message" }</div>
                            <pre class={classes!(
                                "max-h-40",
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
                                { event.last_message_content.clone().unwrap_or_else(|| "-".to_string()) }
                            </pre>
                        </div>

                        <div class={classes!("mt-4", "min-h-0", "flex-1", "overflow-hidden")}>
                            <pre class={classes!(
                                "h-full",
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
                                { pretty_headers_json(&event.request_headers_json) }
                            </pre>
                        </div>
                    </div>
                </div>
            }

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed", "bottom-5", "right-5", "z-[90]",
                    "rounded-full", "border", "px-4", "py-3",
                    "text-sm", "font-semibold",
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
