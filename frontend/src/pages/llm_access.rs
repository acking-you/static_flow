use gloo_timers::callback::{Interval, Timeout};
use wasm_bindgen::prelude::*;
use web_sys::Element;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_llm_gateway_access, fetch_llm_gateway_status, LlmGatewayAccessResponse,
        LlmGatewayPublicKeyView, LlmGatewayRateLimitStatusResponse, LlmGatewayRateLimitWindowView,
    },
    pages::llm_access_shared::{
        format_ms, format_percent, format_reset_hint, format_window_label, pretty_limit_name,
        resolved_base_url, usage_ratio, REMOTE_COMPACT_ARTICLE_ID,
    },
    router::Route,
};

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

#[derive(Properties, PartialEq)]
struct PublicKeyCardProps {
    key_item: LlmGatewayPublicKeyView,
    on_copy: Callback<(String, String)>,
    on_refresh: Callback<(String, String)>,
    refreshing: bool,
}

#[derive(Properties, PartialEq)]
struct RateLimitWindowPanelProps {
    label: AttrValue,
    eyebrow: AttrValue,
    accent_class: Classes,
    window: LlmGatewayRateLimitWindowView,
}

#[function_component(RateLimitWindowPanel)]
fn rate_limit_window_panel(props: &RateLimitWindowPanelProps) -> Html {
    let width = props.window.remaining_percent.clamp(0.0, 100.0);

    html! {
        <article class={classes!(
            "overflow-hidden",
            "rounded-[1.35rem]",
            "border",
            "border-[color:color-mix(in_srgb,var(--border)_68%,transparent)]",
            "bg-[color:color-mix(in_srgb,var(--surface)_94%,transparent)]",
            "p-5"
        )}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                <div>
                    <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--muted)]")}>
                        { props.eyebrow.clone() }
                    </p>
                    <h3 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                        { props.label.clone() }
                    </h3>
                </div>
                <div class={classes!("text-right")}>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "剩余" }</div>
                    <div class={classes!("mt-2", "text-4xl", "font-black", "tracking-[-0.06em]", "text-[var(--text)]")}>
                        { format_percent(props.window.remaining_percent) }
                    </div>
                </div>
            </div>

            <div class={classes!("mt-5", "h-4", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                <div
                    class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", props.accent_class.clone())}
                    style={format!("width: {width:.2}%;")}
                />
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "sm:grid-cols-3")}>
                <div>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "已用" }</div>
                    <div class={classes!("mt-2", "text-xl", "font-bold", "text-[var(--text)]")}>
                        { format_percent(props.window.used_percent) }
                    </div>
                </div>
                <div>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "窗口" }</div>
                    <div class={classes!("mt-2", "text-xl", "font-bold", "text-[var(--text)]")}>
                        { format_window_label(props.window.window_duration_mins, "unknown") }
                    </div>
                </div>
                <div>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "重置" }</div>
                    <div class={classes!("mt-2", "text-sm", "font-semibold", "leading-7", "text-[var(--text)]")}>
                        { format_reset_hint(props.window.resets_at) }
                    </div>
                </div>
            </div>
        </article>
    }
}

#[function_component(PublicKeyCard)]
fn public_key_card(props: &PublicKeyCardProps) -> Html {
    let key_item = props.key_item.clone();
    let usage_percent = (usage_ratio(&key_item) * 100.0).round() as i32;

    html! {
        <article class={classes!(
            "group",
            "overflow-hidden",
            "rounded-[1.45rem]",
            "border",
            "border-[color:color-mix(in_srgb,var(--border)_72%,transparent)]",
            "bg-[color:color-mix(in_srgb,var(--surface)_92%,transparent)]",
            "p-5",
            "shadow-[0_24px_70px_rgba(15,23,42,0.10)]",
            "transition-all",
            "duration-200",
            "hover:-translate-y-1",
            "hover:shadow-[0_32px_90px_rgba(15,23,42,0.16)]"
        )}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-4")}>
                <div class={classes!("min-w-0", "flex-1")}>
                    <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>
                        { "Public Key" }
                    </p>
                    <h3 class={classes!("mt-3", "text-2xl", "font-black", "tracking-[-0.04em]", "text-[var(--text)]")}>
                        { key_item.name.clone() }
                    </h3>
                    <p class={classes!("mt-2", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                        { "直接复制这把 Key 就能开始接入，刷新按钮会回读当前实时剩余额度" }
                    </p>
                </div>

                <button
                    type="button"
                    class={classes!(
                        "inline-flex",
                        "h-11",
                        "w-11",
                        "items-center",
                        "justify-center",
                        "rounded-2xl",
                        "border",
                        "border-[var(--border)]",
                        "bg-[var(--surface)]",
                        "text-[var(--muted)]",
                        "transition-colors",
                        "hover:bg-[var(--surface-alt)]",
                        "hover:text-[var(--primary)]",
                        "disabled:cursor-not-allowed",
                        "disabled:opacity-50"
                    )}
                    title="刷新当前 Key 的实时额度"
                    aria-label="刷新当前 Key 的实时额度"
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
            </div>

            <div class={classes!("mt-5", "rounded-[1.2rem]", "bg-slate-950", "px-4", "py-4", "text-emerald-200")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                    <code class={classes!("min-w-0", "flex-1", "break-all", "text-sm")}>
                        { key_item.secret.clone() }
                    </code>
                    <button
                        class={classes!("btn-fluent-primary", "!px-4", "!py-2", "!text-sm")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("公开 Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制 Key" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-5", "grid", "gap-3", "sm:grid-cols-2", "xl:grid-cols-4")}>
                <div class={classes!("rounded-[1.05rem]", "border", "border-[var(--border)]", "px-4", "py-4")}>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "剩余额度" }</div>
                    <div class={classes!("mt-2", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                        { key_item.remaining_billable }
                    </div>
                </div>
                <div class={classes!("rounded-[1.05rem]", "border", "border-[var(--border)]", "px-4", "py-4")}>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "总额度" }</div>
                    <div class={classes!("mt-2", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                        { key_item.quota_billable_limit }
                    </div>
                </div>
                <div class={classes!("rounded-[1.05rem]", "border", "border-[var(--border)]", "px-4", "py-4")}>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "已用 billable" }</div>
                    <div class={classes!("mt-2", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                        { (key_item.quota_billable_limit as i64 - key_item.remaining_billable).max(0) }
                    </div>
                </div>
                <div class={classes!("rounded-[1.05rem]", "border", "border-[var(--border)]", "px-4", "py-4")}>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "上次使用" }</div>
                    <div class={classes!("mt-2", "text-sm", "font-semibold", "leading-7", "text-[var(--text)]")}>
                        { key_item.last_used_at.map(format_ms).unwrap_or_else(|| "尚未产生调用".to_string()) }
                    </div>
                </div>
            </div>

            <div class={classes!("mt-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                    <span>{ "额度进度" }</span>
                    <span>{ format!("{usage_percent}% used") }</span>
                </div>
                <div class={classes!("mt-2", "h-3", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                    <div
                        class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                        style={format!("width: {}%;", usage_percent.clamp(0, 100))}
                    />
                </div>
                <div class={classes!("mt-3", "grid", "gap-2", "sm:grid-cols-3", "text-xs", "text-[var(--muted)]")}>
                    <span>{ format!("未缓存输入 {}", key_item.usage_input_uncached_tokens) }</span>
                    <span>{ format!("缓存命中 {}", key_item.usage_input_cached_tokens) }</span>
                    <span>{ format!("输出 {}", key_item.usage_output_tokens) }</span>
                </div>
            </div>
        </article>
    }
}

#[function_component(LlmAccessPage)]
pub fn llm_access_page() -> Html {
    let access = use_state(|| None::<LlmGatewayAccessResponse>);
    let rate_limit_status = use_state(|| None::<LlmGatewayRateLimitStatusResponse>);
    let loading = use_state(|| true);
    let status_loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let status_error = use_state(|| None::<String>);
    let toast = use_state(|| None::<(String, bool)>);
    let toast_timeout = use_mut_ref(|| None::<Timeout>);
    let refreshing_key = use_state(|| None::<String>);
    let refreshing_status = use_state(|| false);
    let status_section_ref = use_node_ref();

    {
        let access = access.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        access.set(Some(data));
                        error.set(None);
                    },
                    Err(err) => {
                        access.set(None);
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    {
        let rate_limit_status = rate_limit_status.clone();
        let status_loading = status_loading.clone();
        let status_error = status_error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        rate_limit_status.set(Some(data));
                        status_error.set(None);
                    },
                    Err(err) => {
                        rate_limit_status.set(None);
                        status_error.set(Some(err));
                    },
                }
                status_loading.set(false);
            });
            || ()
        });
    }

    {
        let rate_limit_status = rate_limit_status.clone();
        let status_error = status_error.clone();
        use_effect_with((), move |_| {
            let interval = Interval::new(30_000, move || {
                let rate_limit_status = rate_limit_status.clone();
                let status_error = status_error.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_llm_gateway_status().await {
                        Ok(data) => {
                            rate_limit_status.set(Some(data));
                            status_error.set(None);
                        },
                        Err(err) => {
                            status_error.set(Some(err));
                        },
                    }
                });
            });
            move || drop(interval)
        });
    }

    let page_shell = classes!(
        "relative",
        "min-h-screen",
        "overflow-hidden",
        "bg-[radial-gradient(circle_at_top_left,rgba(15,118,110,0.18),transparent_32%),\
         radial-gradient(circle_at_top_right,rgba(37,99,235,0.18),transparent_28%),\
         linear-gradient(180deg,var(--bg),color-mix(in_srgb,var(--bg)_84%,var(--surface-alt)))]"
    );

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

    let on_refresh_key = {
        let access = access.clone();
        let refreshing_key = refreshing_key.clone();
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |(key_id, key_name): (String, String)| {
            refreshing_key.set(Some(key_id));
            let access = access.clone();
            let refreshing_key = refreshing_key.clone();
            let toast = toast.clone();
            let toast_timeout = toast_timeout.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        access.set(Some(data));
                        toast.set(Some((format!("已刷新 {} 的实时额度", key_name), false)));
                    },
                    Err(err) => {
                        toast.set(Some((format!("刷新失败：{}", err), true)));
                    },
                }
                toast_timeout.borrow_mut().take();
                let toast = toast.clone();
                let clear_handle = toast_timeout.clone();
                let timeout = Timeout::new(2200, move || {
                    toast.set(None);
                    clear_handle.borrow_mut().take();
                });
                *toast_timeout.borrow_mut() = Some(timeout);
                refreshing_key.set(None);
            });
        })
    };

    let on_refresh_status = {
        let rate_limit_status = rate_limit_status.clone();
        let status_error = status_error.clone();
        let refreshing_status = refreshing_status.clone();
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        Callback::from(move |_| {
            refreshing_status.set(true);
            let rate_limit_status = rate_limit_status.clone();
            let status_error = status_error.clone();
            let refreshing_status = refreshing_status.clone();
            let toast = toast.clone();
            let toast_timeout = toast_timeout.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        rate_limit_status.set(Some(data));
                        status_error.set(None);
                        toast.set(Some(("已读取最新限额快照".to_string(), false)));
                    },
                    Err(err) => {
                        status_error.set(Some(err.clone()));
                        toast.set(Some((format!("读取限额快照失败：{}", err), true)));
                    },
                }
                toast_timeout.borrow_mut().take();
                let toast = toast.clone();
                let clear_handle = toast_timeout.clone();
                let timeout = Timeout::new(2200, move || {
                    toast.set(None);
                    clear_handle.borrow_mut().take();
                });
                *toast_timeout.borrow_mut() = Some(timeout);
                refreshing_status.set(false);
            });
        })
    };

    let on_scroll_to_status = {
        let status_section_ref = status_section_ref.clone();
        Callback::from(move |_| {
            if let Some(section) = status_section_ref.cast::<Element>() {
                section.scroll_into_view();
            }
        })
    };

    let content = if *loading {
        html! {
            <div class={classes!("mt-10", "rounded-[1.25rem]", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                { "正在读取当前公开可用的 Key" }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-[1.25rem]", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
                { err }
            </div>
        }
    } else if let Some(access) = (*access).clone() {
        let base_url = resolved_base_url(&access);
        let status_view = if *status_loading {
            html! {
                <div class={classes!("rounded-[1.45rem]", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                    { "正在读取 Codex 周限额与 5h 限额快照" }
                </div>
            }
        } else if let Some(status) = (*rate_limit_status).clone() {
            let effective_status_error = (*status_error)
                .clone()
                .or_else(|| status.error_message.clone());
            let primary_bucket = status
                .buckets
                .iter()
                .find(|bucket| bucket.is_primary)
                .cloned()
                .or_else(|| status.buckets.first().cloned());
            let additional_buckets = status
                .buckets
                .iter()
                .filter(|bucket| !bucket.is_primary)
                .cloned()
                .collect::<Vec<_>>();

            html! {
                <section
                    ref={status_section_ref.clone()}
                    class={classes!(
                        "relative",
                        "overflow-hidden",
                        "rounded-[1.8rem]",
                        "border",
                        "border-[color:color-mix(in_srgb,var(--border)_70%,transparent)]",
                        "bg-[linear-gradient(180deg,color-mix(in_srgb,var(--surface)_94%,transparent),color-mix(in_srgb,var(--surface-alt)_88%,transparent))]",
                        "p-6",
                        "shadow-[0_28px_80px_rgba(15,23,42,0.10)]",
                        "lg:p-8"
                    )}
                >
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>
                                { "Live Codex Status" }
                            </p>
                            <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                { "当前周限额与 5h 限额快照" }
                            </h2>
                            <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { "这里展示的是后台每 1 分钟自动刷新到内存里的 Codex 官方限额状态，前端读取的是缓存快照，不会每次都去打 upstream" }
                            </p>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                            <div class={classes!(
                                "inline-flex",
                                "items-center",
                                "gap-2",
                                "rounded-full",
                                "border",
                                "border-[var(--border)]",
                                "bg-[var(--surface)]",
                                "px-4",
                                "py-2",
                                "text-xs",
                                "font-semibold",
                                "uppercase",
                                "tracking-[0.16em]",
                                match status.status.as_str() {
                                    "ready" => "text-emerald-600",
                                    "degraded" => "text-amber-600",
                                    "error" => "text-red-600",
                                    _ => "text-[var(--muted)]",
                                }
                            )}>
                                <span class={classes!("inline-block", "h-2.5", "w-2.5", "rounded-full", match status.status.as_str() {
                                    "ready" => "bg-emerald-500",
                                    "degraded" => "bg-amber-500",
                                    "error" => "bg-red-500",
                                    _ => "bg-slate-400",
                                })} />
                                { match status.status.as_str() {
                                    "ready" => "cache ready",
                                    "degraded" => "cache degraded",
                                    "error" => "cache error",
                                    _ => "cache loading",
                                } }
                            </div>
                            <button
                                type="button"
                                class={classes!("btn-fluent-secondary", "!px-4", "!py-3")}
                                onclick={on_refresh_status.clone()}
                                disabled={*refreshing_status}
                            >
                                <i class={classes!("fas", if *refreshing_status { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                                <span>{ "读取缓存快照" }</span>
                            </button>
                        </div>
                    </div>

                    <div class={classes!("mt-5", "grid", "gap-4", "xl:grid-cols-[minmax(0,1.25fr)_minmax(0,0.75fr)]")}>
                        <div class={classes!("space-y-4")}>
                            if let Some(primary_bucket) = primary_bucket.clone() {
                                <section class={classes!("rounded-[1.4rem]", "border", "border-[var(--border)]", "bg-[color:color-mix(in_srgb,var(--surface)_92%,transparent)]", "p-5")}>
                                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                                        <div class={classes!("max-w-2xl")}>
                                            <p class={classes!("m-0", "text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                                                { "Primary Bucket" }
                                            </p>
                                            <h3 class={classes!("mt-3", "text-2xl", "font-black", "tracking-[-0.04em]", "text-[var(--text)]")}>
                                                { pretty_limit_name(&primary_bucket.display_name) }
                                            </h3>
                                            <p class={classes!("mt-2", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                                {
                                                    primary_bucket.plan_type
                                                        .clone()
                                                        .map(|plan| format!("当前账号计划 {}", plan))
                                                        .unwrap_or_else(|| "当前账号计划暂时未返回".to_string())
                                                }
                                            </p>
                                        </div>
                                        if let Some(credits) = primary_bucket.credits.clone() {
                                            <div class={classes!("rounded-[1.15rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-4", "py-4", "min-w-[15rem]")}>
                                                <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Credits" }</div>
                                                <div class={classes!("mt-2", "text-xl", "font-bold", "text-[var(--text)]")}>
                                                    {
                                                        if !credits.has_credits {
                                                            "未启用".to_string()
                                                        } else if credits.unlimited {
                                                            "Unlimited".to_string()
                                                        } else {
                                                            credits.balance.unwrap_or_else(|| "可用".to_string())
                                                        }
                                                    }
                                                </div>
                                            </div>
                                        }
                                    </div>

                                    <div class={classes!("mt-5", "grid", "gap-4", "xl:grid-cols-2")}>
                                        if let Some(primary_window) = primary_bucket.primary.clone() {
                                            <RateLimitWindowPanel
                                                label={"5h 窗口"}
                                                eyebrow={"Fast feedback"}
                                                accent_class={classes!("bg-[linear-gradient(90deg,#0f766e,#14b8a6)]")}
                                                window={primary_window}
                                            />
                                        }
                                        if let Some(secondary_window) = primary_bucket.secondary.clone() {
                                            <RateLimitWindowPanel
                                                label={"Weekly 窗口"}
                                                eyebrow={"Long runway"}
                                                accent_class={classes!("bg-[linear-gradient(90deg,#2563eb,#7c3aed)]")}
                                                window={secondary_window}
                                            />
                                        }
                                    </div>
                                </section>
                            } else {
                                <div class={classes!("rounded-[1.4rem]", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-10", "text-center", "text-[var(--muted)]")}>
                                    { "当前还没有读到 Codex 限额快照" }
                                </div>
                            }

                            if !additional_buckets.is_empty() {
                                <section class={classes!("rounded-[1.4rem]", "border", "border-[var(--border)]", "bg-[color:color-mix(in_srgb,var(--surface)_90%,transparent)]", "p-5")}>
                                    <div class={classes!("flex", "items-end", "justify-between", "gap-3", "flex-wrap")}>
                                        <div>
                                            <p class={classes!("m-0", "text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Additional Buckets" }</p>
                                            <h3 class={classes!("mt-3", "text-2xl", "font-black", "tracking-[-0.04em]", "text-[var(--text)]")}>{ "其他 metered limit" }</h3>
                                        </div>
                                        <span class={classes!("text-xs", "font-semibold", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>
                                            { format!("{} buckets", additional_buckets.len()) }
                                        </span>
                                    </div>
                                    <div class={classes!("mt-5", "space-y-3")}>
                                        { for additional_buckets.iter().map(|bucket| {
                                            let bucket_primary = bucket.primary.clone();
                                            let bucket_secondary = bucket.secondary.clone();
                                            html! {
                                                <div class={classes!("rounded-[1.1rem]", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-4", "py-4")}>
                                                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                                                        <div>
                                                            <div class={classes!("text-sm", "font-bold", "text-[var(--text)]")}>
                                                                { pretty_limit_name(&bucket.display_name) }
                                                            </div>
                                                            <div class={classes!("mt-1", "text-xs", "uppercase", "tracking-[0.16em]", "text-[var(--muted)]")}>
                                                                { bucket.limit_id.clone() }
                                                            </div>
                                                        </div>
                                                        <div class={classes!("grid", "gap-3", "sm:grid-cols-2")}>
                                                            if let Some(primary) = bucket_primary {
                                                                <div class={classes!("min-w-[11rem]")}>
                                                                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                                                                        { format_window_label(primary.window_duration_mins, "5h") }
                                                                    </div>
                                                                    <div class={classes!("mt-2", "text-lg", "font-black", "text-[var(--text)]")}>
                                                                        { format_percent(primary.remaining_percent) }
                                                                    </div>
                                                                    <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                                        { format_reset_hint(primary.resets_at) }
                                                                    </div>
                                                                </div>
                                                            }
                                                            if let Some(secondary) = bucket_secondary {
                                                                <div class={classes!("min-w-[11rem]")}>
                                                                    <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>
                                                                        { format_window_label(secondary.window_duration_mins, "weekly") }
                                                                    </div>
                                                                    <div class={classes!("mt-2", "text-lg", "font-black", "text-[var(--text)]")}>
                                                                        { format_percent(secondary.remaining_percent) }
                                                                    </div>
                                                                    <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                                                                        { format_reset_hint(secondary.resets_at) }
                                                                    </div>
                                                                </div>
                                                            }
                                                        </div>
                                                    </div>
                                                </div>
                                            }
                                        }) }
                                    </div>
                                </section>
                            }
                        </div>

                        <aside class={classes!("space-y-4")}>
                            <div class={classes!("rounded-[1.4rem]", "border", "border-[var(--border)]", "bg-[color:color-mix(in_srgb,var(--surface)_92%,transparent)]", "p-5")}>
                                <p class={classes!("m-0", "text-[11px]", "uppercase", "tracking-[0.18em]", "text-[var(--muted)]")}>{ "Snapshot Meta" }</p>
                                <div class={classes!("mt-4", "space-y-4", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.16em]")}>{ "上次检查" }</div>
                                        <div class={classes!("mt-1", "font-semibold", "text-[var(--text)]")}>
                                            { status.last_checked_at.map(format_ms).unwrap_or_else(|| "尚未检查".to_string()) }
                                        </div>
                                    </div>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.16em]")}>{ "上次成功" }</div>
                                        <div class={classes!("mt-1", "font-semibold", "text-[var(--text)]")}>
                                            { status.last_success_at.map(format_ms).unwrap_or_else(|| "尚未成功".to_string()) }
                                        </div>
                                    </div>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.16em]")}>{ "自动刷新" }</div>
                                        <div class={classes!("mt-1", "font-semibold", "text-[var(--text)]")}>
                                            { format!("每 {} 秒刷新一次", status.refresh_interval_seconds) }
                                        </div>
                                    </div>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.16em]")}>{ "来源" }</div>
                                        <div class={classes!("mt-1", "break-all", "font-semibold", "text-[var(--text)]")}>
                                            { status.source_url.clone() }
                                        </div>
                                    </div>
                                </div>
                            </div>

                            if let Some(error_message) = effective_status_error {
                                <div class={classes!("rounded-[1.4rem]", "border", "border-amber-400/30", "bg-amber-500/8", "p-5")}>
                                    <p class={classes!("m-0", "text-[11px]", "uppercase", "tracking-[0.18em]", "text-amber-700", "dark:text-amber-300")}>{ "Refresh Note" }</p>
                                    <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-amber-800", "dark:text-amber-200")}>
                                        { error_message }
                                    </p>
                                </div>
                            }
                        </aside>
                    </div>
                </section>
            }
        } else if let Some(err) = (*status_error).clone() {
            html! {
                <div class={classes!("rounded-[1.45rem]", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "leading-7", "text-red-700", "dark:text-red-200")}>
                    { err }
                </div>
            }
        } else {
            Html::default()
        };

        html! {
            <>
                <section class={classes!(
                    "llm-access-hero",
                    "mt-8",
                    "overflow-hidden",
                    "rounded-[1.8rem]",
                    "border",
                    "border-emerald-500/15",
                    "bg-[linear-gradient(140deg,rgba(236,253,245,0.98),rgba(239,246,255,0.96))]",
                    "p-6",
                    "text-slate-900",
                    "shadow-[0_30px_90px_rgba(16,185,129,0.10)]",
                    "lg:p-8"
                )}>
                    <div class={classes!("grid", "gap-8", "xl:grid-cols-[minmax(0,1.15fr)_minmax(0,0.85fr)]", "xl:items-end")}>
                        <div class={classes!("max-w-4xl")}>
                            <p class={classes!("llm-access-hero-eyebrow", "m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.26em]", "text-emerald-700")}>
                                { "Public Key Hall" }
                            </p>
                            <h1 class={classes!("llm-access-hero-title", "mt-4", "text-4xl", "font-black", "tracking-[-0.06em]", "text-slate-950", "sm:text-[3.5rem]")}>
                                { "站长大大 GPT Pro 用不完 顺手放点免费 Key 给大家养龙虾🦞 和跑 Codex" }
                            </h1>
                            <div class={classes!("mt-6", "flex", "flex-wrap", "gap-3")}>
                                <button
                                    class={classes!("btn-fluent-primary")}
                                    onclick={{
                                        let on_copy = on_copy.clone();
                                        let base_url = base_url.clone();
                                        Callback::from(move |_| on_copy.emit((" /v1 Base URL".to_string(), base_url.clone())))
                                    }}
                                >
                                    { "复制 /v1 Base URL" }
                                </button>
                                <Link<Route>
                                    to={Route::LlmAccessGuide}
                                    classes={classes!(
                                        "llm-access-hero-secondary-btn",
                                        "btn-fluent-secondary",
                                        "!border-slate-900/10",
                                        "!bg-white/68",
                                        "!text-slate-900",
                                        "hover:!bg-white/92"
                                    )}
                                >
                                    { "三步接入帮助页" }
                                </Link<Route>>
                            </div>
                        </div>

                        <div class={classes!("space-y-4")}>
                            <div class={classes!("rounded-[1.35rem]", "border", "border-slate-900/10", "bg-white/62", "p-5", "backdrop-blur-xl")}>
                                <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-sky-700")}>{ "Gateway Entry" }</p>
                                <div class={classes!("mt-3", "break-all", "text-lg", "font-black", "tracking-[-0.03em]", "text-slate-950")}>
                                    { base_url.clone() }
                                </div>
                                <div class={classes!("mt-3", "text-sm", "font-semibold", "text-slate-700")}>
                                    { "已经带 /v1 直接用" }
                                </div>
                            </div>
                            <div class={classes!("rounded-[1.35rem]", "border", "border-slate-900/10", "bg-white/62", "p-5", "backdrop-blur-xl")}>
                                <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-emerald-700")}>{ "Live Inventory" }</p>
                                <div class={classes!("mt-3", "grid", "gap-3", "sm:grid-cols-3")}>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-slate-500")}>{ "公开 Key" }</div>
                                        <div class={classes!("mt-2", "text-3xl", "font-black", "tracking-[-0.05em]", "text-slate-950")}>{ access.keys.len() }</div>
                                    </div>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-slate-500")}>{ "鉴权缓存 TTL" }</div>
                                        <div class={classes!("mt-2", "text-3xl", "font-black", "tracking-[-0.05em]", "text-slate-950")}>{ access.auth_cache_ttl_seconds }</div>
                                    </div>
                                    <div>
                                        <div class={classes!("text-[11px]", "uppercase", "tracking-[0.18em]", "text-slate-500")}>{ "额度刷新" }</div>
                                        <div class={classes!("mt-2", "text-sm", "font-semibold", "leading-7", "text-slate-800")}>{ "单卡实时" }</div>
                                    </div>
                                </div>
                            </div>
                        </div>
                    </div>
                </section>

                <section class={classes!(
                    "llm-access-warning",
                    "mt-8",
                    "overflow-hidden",
                    "rounded-[1.75rem]",
                    "border",
                    "border-amber-500/20",
                    "bg-[linear-gradient(135deg,rgba(255,247,237,0.98),rgba(254,249,195,0.92))]",
                    "p-6",
                    "text-slate-900",
                    "shadow-[0_24px_80px_rgba(245,158,11,0.12)]",
                    "lg:p-8"
                )}>
                    <div class={classes!("grid", "gap-6", "xl:grid-cols-[minmax(0,1.12fr)_minmax(0,0.88fr)]", "xl:items-end")}>
                        <div class={classes!("max-w-4xl")}>
                            <p class={classes!("llm-access-warning-eyebrow", "m-0", "text-xs", "font-semibold", "uppercase", "tracking-[0.26em]", "text-amber-700")}>
                                { "Remote Compact Matters" }
                            </p>
                            <h2 class={classes!("llm-access-warning-title", "mt-4", "text-4xl", "font-black", "tracking-[-0.06em]", "text-slate-950", "sm:text-[3.15rem]")}>
                                { "接 Codex 必须保住 remote compact" }
                            </h2>
                            <p class={classes!("mt-4", "m-0", "max-w-3xl", "text-sm", "font-semibold", "leading-7", "text-slate-700/90")}>
                                { "很多中转站看起来能接进来 其实已经把 remote compact 藏没了 这会直接拖垮 Codex 的长任务续接和连续使用体验" }
                            </p>
                        </div>

                        <div class={classes!("llm-access-warning-card", "rounded-[1.35rem]", "border", "border-slate-900/10", "bg-white/68", "p-5", "backdrop-blur-xl", "space-y-3")}>
                            <Link<Route>
                                to={Route::LlmAccessGuide}
                                classes={classes!(
                                    "inline-flex", "w-full", "items-center", "justify-center", "rounded-full",
                                    "border", "border-slate-900/10", "bg-slate-950", "px-5", "py-3",
                                    "text-sm", "font-semibold", "text-white", "transition-colors", "hover:bg-slate-800"
                                )}
                            >
                                { "先去看三步接入帮助页" }
                            </Link<Route>>
                            <Link<Route>
                                to={Route::ArticleDetail {
                                    id: REMOTE_COMPACT_ARTICLE_ID.to_string(),
                                }}
                                classes={classes!(
                                    "llm-access-warning-link",
                                    "inline-flex", "w-full", "items-center", "justify-center", "rounded-full",
                                    "border", "border-amber-500/24", "bg-amber-500/10", "px-5", "py-3",
                                    "text-sm", "font-semibold", "text-amber-800", "transition-colors", "hover:bg-amber-500/16"
                                )}
                            >
                                { "再看 compact 深潜文章" }
                            </Link<Route>>
                        </div>
                    </div>
                </section>

                <section class={classes!("mt-10", "space-y-4")}>
                    { status_view }
                </section>

                <section class={classes!("mt-10", "space-y-4")}>
                    <div class={classes!("flex", "items-end", "justify-between", "gap-4", "flex-wrap")}>
                        <div class={classes!("max-w-3xl")}>
                            <p class={classes!("m-0", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.22em]", "text-[var(--primary)]")}>
                                { "Current Public Keys" }
                            </p>
                            <h2 class={classes!("mt-3", "text-3xl", "font-black", "tracking-[-0.05em]", "text-[var(--text)]")}>
                                { "当前公开可用的 Key" }
                            </h2>
                            <p class={classes!("mt-3", "m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { "这里不再塞 provider 配置和示例代码，只留下你此刻真正要复制的 Key 仓位" }
                            </p>
                        </div>
                        <Link<Route>
                            to={Route::LlmAccessGuide}
                            classes={classes!("btn-fluent-secondary", "!px-6", "!py-3")}
                        >
                            { "不会接？去帮助页" }
                        </Link<Route>>
                    </div>

                    if access.keys.is_empty() {
                        <div class={classes!("rounded-[1.35rem]", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                            { "当前还没有公开放出的 Key" }
                        </div>
                    } else {
                        <div class={classes!("grid", "gap-5", "2xl:grid-cols-2")}>
                            { for access.keys.iter().map(|key_item| html! {
                                <PublicKeyCard
                                    key={key_item.id.clone()}
                                    key_item={key_item.clone()}
                                    on_copy={on_copy.clone()}
                                    on_refresh={on_refresh_key.clone()}
                                    refreshing={(*refreshing_key).as_deref() == Some(key_item.id.as_str())}
                                />
                            }) }
                        </div>
                    }
                </section>
            </>
        }
    } else {
        Html::default()
    };

    html! {
        <main class={page_shell}>
            <div class={classes!("relative", "mx-auto", "max-w-[96rem]", "px-4", "pb-16", "pt-8", "lg:px-6", "lg:pt-10")}>
                { content }
            </div>

            <button
                type="button"
                class={classes!(
                    "fixed",
                    "bottom-24",
                    "right-5",
                    "z-[85]",
                    "inline-flex",
                    "items-center",
                    "gap-3",
                    "rounded-full",
                    "border",
                    "border-sky-500/20",
                    "bg-slate-950/92",
                    "px-5",
                    "py-3",
                    "text-sm",
                    "font-semibold",
                    "text-white",
                    "shadow-[0_20px_50px_rgba(15,23,42,0.28)]",
                    "backdrop-blur-xl",
                    "transition-transform",
                    "duration-200",
                    "hover:-translate-y-1"
                )}
                onclick={on_scroll_to_status}
            >
                <span class={classes!("relative", "flex", "h-3", "w-3")}>
                    <span class={classes!("absolute", "inline-flex", "h-full", "w-full", "animate-ping", "rounded-full", "bg-sky-400/60")}></span>
                    <span class={classes!("relative", "inline-flex", "h-3", "w-3", "rounded-full", "bg-sky-300")}></span>
                </span>
                <span>{ "查看 Codex 限额状态" }</span>
            </button>

            if let Some((message, is_error)) = (*toast).clone() {
                <div class={classes!(
                    "fixed",
                    "bottom-5",
                    "right-5",
                    "z-[90]",
                    "rounded-full",
                    "border",
                    "px-4",
                    "py-3",
                    "text-sm",
                    "font-semibold",
                    "shadow-[0_18px_42px_rgba(15,23,42,0.24)]",
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
