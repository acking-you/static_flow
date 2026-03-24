use gloo_timers::callback::Timeout;
use js_sys::Date;
use wasm_bindgen::prelude::*;
use web_sys::{window, HtmlElement, HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        create_admin_llm_gateway_key, delete_admin_llm_gateway_key, fetch_admin_llm_gateway_config,
        fetch_admin_llm_gateway_keys, fetch_admin_llm_gateway_usage_events,
        patch_admin_llm_gateway_key, update_admin_llm_gateway_config, AdminLlmGatewayKeyView,
        AdminLlmGatewayUsageEventView, AdminLlmGatewayUsageEventsQuery, LlmGatewayRuntimeConfig,
    },
    components::pagination::Pagination,
    router::Route,
};

const USAGE_PAGE_SIZE: usize = 20;

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
}

#[function_component(KeyEditorCard)]
fn key_editor_card(props: &KeyEditorCardProps) -> Html {
    let key_item = props.key_item.clone();
    let name = use_state(|| key_item.name.clone());
    let quota = use_state(|| key_item.quota_billable_limit.to_string());
    let public_visible = use_state(|| key_item.public_visible);
    let status = use_state(|| key_item.status.clone());
    let saving = use_state(|| false);
    let feedback = use_state(|| None::<String>);

    {
        // Reset editor controls whenever the parent list refreshes this card.
        let key_item = props.key_item.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        use_effect_with(props.key_item.clone(), move |_| {
            name.set(key_item.name.clone());
            quota.set(key_item.quota_billable_limit.to_string());
            public_visible.set(key_item.public_visible);
            status.set(key_item.status.clone());
            || ()
        });
    }

    let on_save = {
        let key_id = key_item.id.clone();
        let name = name.clone();
        let quota = quota.clone();
        let public_visible = public_visible.clone();
        let status = status.clone();
        let saving = saving.clone();
        let feedback = feedback.clone();
        let on_changed = props.on_changed.clone();
        Callback::from(move |_| {
            let key_id = key_id.clone();
            let name_value = (*name).trim().to_string();
            let quota_value = (*quota).trim().parse::<u64>();
            let public_visible_value = *public_visible;
            let status_value = (*status).clone();
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

    // Usage events are fetched independently so paging and key filters do not
    // need to re-fetch the rest of the admin page chrome.
    let reload_usage = {
        let usage_events = usage_events.clone();
        let usage_total = usage_total.clone();
        let usage_page = usage_page.clone();
        let usage_loading = usage_loading.clone();
        let usage_key_filter = usage_key_filter.clone();
        let load_error = load_error.clone();
        Callback::from(move |requested_page: Option<usize>| {
            let usage_events = usage_events.clone();
            let usage_total = usage_total.clone();
            let usage_page = usage_page.clone();
            let usage_loading = usage_loading.clone();
            let usage_key_filter = usage_key_filter.clone();
            let load_error = load_error.clone();
            let page = requested_page.unwrap_or(*usage_page).max(1);
            let selected_key_id = (*usage_key_filter).clone();
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
                    Ok::<_, String>((cfg, keys_resp.keys, effective_key_filter, usage_resp))
                }
                .await;

                match result {
                    Ok((cfg, key_items, effective_key_filter, usage_resp)) => {
                        ttl_input.set(cfg.auth_cache_ttl_seconds.to_string());
                        config.set(Some(cfg));
                        keys.set(key_items);
                        usage_key_filter.set(effective_key_filter);
                        usage_total.set(usage_resp.total);
                        usage_events.set(usage_resp.events);
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
        use_effect_with((), move |_| {
            reload.emit(());
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
                usage_key_filter.set(target.value());
                usage_page.set(1);
                reload_usage.emit(Some(1));
            }
        })
    };

    let on_usage_page_change = {
        let usage_page = usage_page.clone();
        let reload_usage = reload_usage.clone();
        Callback::from(move |page: usize| {
            usage_page.set(page);
            reload_usage.emit(Some(page));
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
                                    />
                                }) }
                            }
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
                                Callback::from(move |_| reload_usage.emit(None))
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
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2")}
                                value={(*usage_key_filter).clone()}
                                onchange={on_usage_key_filter_change}
                            >
                                <option value="">{ "全部" }</option>
                                { for keys.iter().map(|key_item| html! {
                                    <option value={key_item.id.clone()}>{ key_item.name.clone() }</option>
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
                                    <th class={classes!("py-2", "pr-3")}>{ "URL / Route" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Model" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Status" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Latency" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "IP / 属地" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Tokens" }</th>
                                    <th class={classes!("py-2", "pr-3")}>{ "Headers" }</th>
                                </tr>
                            </thead>
                            <tbody>
                                if usage_events.is_empty() && !*loading && !*usage_loading {
                                    <tr class={classes!("border-t", "border-[var(--border)]")}>
                                        <td colspan="9" class={classes!("py-8", "text-center", "text-[var(--muted)]")}>{ "当前筛选下还没有 usage 事件" }</td>
                                    </tr>
                                } else {
                                    { for usage_events.iter().map(|event| {
                                        let event_for_modal = event.clone();
                                        let header_preview = pretty_headers_json(&event.request_headers_json);
                                        html! {
                                            <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap")}>{ format_ms(event.created_at) }</td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[13rem]")}>
                                                    <div class={classes!("font-semibold", "text-[var(--text)]")}>{ event.key_name.clone() }</div>
                                                    <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>{ event.key_id.clone() }</div>
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
                                                        title="查看请求 headers"
                                                        aria-label="查看请求 headers"
                                                        onclick={{
                                                            let selected_usage_event = selected_usage_event.clone();
                                                            Callback::from(move |_| selected_usage_event.set(Some(event_for_modal.clone())))
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
                                <p class={classes!("m-0", "text-xs", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Header Detail" }</p>
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

                        <div class={classes!("mt-4", "grid", "shrink-0", "gap-3", "lg:grid-cols-4")}>
                            <div class={classes!("rounded-lg", "border", "border-[var(--border)]", "px-3", "py-3")}>
                                <div class={classes!("text-xs", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key ID" }</div>
                                <div class={classes!("mt-1", "font-mono", "text-xs", "break-all")}>{ event.key_id.clone() }</div>
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
