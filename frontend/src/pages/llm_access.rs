use gloo_timers::callback::{Interval, Timeout};
use wasm_bindgen::prelude::*;
use web_sys::{Element, HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_llm_gateway_access, fetch_llm_gateway_status, submit_llm_gateway_token_request,
        LlmGatewayAccessResponse, LlmGatewayPublicKeyView, LlmGatewayRateLimitStatusResponse,
        LlmGatewayRateLimitWindowView,
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
    accent_class: Classes,
    window: LlmGatewayRateLimitWindowView,
}

// PLACEHOLDER_RATE_LIMIT_PANEL

#[function_component(RateLimitWindowPanel)]
fn rate_limit_window_panel(props: &RateLimitWindowPanelProps) -> Html {
    let width = props.window.remaining_percent.clamp(0.0, 100.0);

    html! {
        <article class={classes!(
            "overflow-hidden",
            "rounded-xl",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "p-4"
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <h3 class={classes!("m-0", "text-sm", "font-bold", "text-[var(--text)]")}>
                    { props.label.clone() }
                </h3>
                <span class={classes!("text-2xl", "font-black", "tracking-tight", "text-[var(--text)]")}>
                    { format_percent(props.window.remaining_percent) }
                </span>
            </div>

            <div class={classes!("mt-3", "h-2.5", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                <div
                    class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", props.accent_class.clone())}
                    style={format!("width: {width:.2}%;")}
                />
            </div>

            <div class={classes!("mt-2", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]")}>
                <span>{ format!("已用 {}", format_percent(props.window.used_percent)) }</span>
                <span>{ format_window_label(props.window.window_duration_mins, "unknown") }</span>
                <span>{ format_reset_hint(props.window.resets_at) }</span>
            </div>
        </article>
    }
}

// PLACEHOLDER_PUBLIC_KEY_CARD

#[function_component(PublicKeyCard)]
fn public_key_card(props: &PublicKeyCardProps) -> Html {
    let key_item = props.key_item.clone();
    let usage_percent = (usage_ratio(&key_item) * 100.0).round() as i32;

    html! {
        <article class={classes!(
            "group",
            "overflow-hidden",
            "rounded-xl",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "p-5",
            "transition-all",
            "duration-200",
            "hover:-translate-y-0.5",
            "hover:shadow-[0_8px_24px_rgba(0,0,0,0.08)]"
        )}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-3")}>
                <h3 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                    { key_item.name.clone() }
                </h3>
                <button
                    type="button"
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
            </div>

            <div class={classes!("mt-3", "rounded-lg", "bg-slate-950", "px-3", "py-3", "text-emerald-200")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <code class={classes!("min-w-0", "flex-1", "break-all", "text-xs")}>
                        { key_item.secret.clone() }
                    </code>
                    <button
                        class={classes!("btn-terminal", "btn-terminal-primary", "!text-xs")}
                        onclick={{
                            let on_copy = props.on_copy.clone();
                            let secret = key_item.secret.clone();
                            Callback::from(move |_| on_copy.emit(("Key".to_string(), secret.clone())))
                        }}
                    >
                        { "复制" }
                    </button>
                </div>
            </div>

            <div class={classes!("mt-4", "grid", "gap-3", "grid-cols-2")}>
                <div>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-black", "text-[var(--text)]")}>
                        { key_item.remaining_billable }
                    </div>
                </div>
                <div>
                    <div class={classes!("text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                    <div class={classes!("mt-1", "text-2xl", "font-black", "text-[var(--text)]")}>
                        { key_item.quota_billable_limit }
                    </div>
                </div>
            </div>

            <div class={classes!("mt-4")}>
                <div class={classes!("flex", "items-center", "justify-between", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>
                    <span>{ "用量" }</span>
                    <span>{ format!("{usage_percent}%") }</span>
                </div>
                <div class={classes!("mt-1.5", "h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                    <div
                        class={classes!("h-full", "rounded-full", "bg-[linear-gradient(90deg,#0f766e,#2563eb)]", "transition-[width]", "duration-300")}
                        style={format!("width: {}%;", usage_percent.clamp(0, 100))}
                    />
                </div>
                <div class={classes!("mt-2", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]")}>
                    <span>{ format!("输入 {}", key_item.usage_input_uncached_tokens) }</span>
                    <span>{ format!("缓存 {}", key_item.usage_input_cached_tokens) }</span>
                    <span>{ format!("输出 {}", key_item.usage_output_tokens) }</span>
                    if let Some(ts) = key_item.last_used_at {
                        <span class={classes!("ml-auto")}>{ format_ms(ts) }</span>
                    }
                </div>
            </div>
        </article>
    }
}

// PLACEHOLDER_MAIN_COMPONENT

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
    let wish_section_ref = use_node_ref();
    let wish_quota = use_state(String::new);
    let wish_reason = use_state(String::new);
    let wish_email = use_state(String::new);
    let wish_submitting = use_state(|| false);
    let wish_feedback = use_state(|| None::<(String, bool)>);

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

    let show_toast = {
        let toast = toast.clone();
        let toast_timeout = toast_timeout.clone();
        move |msg: String, is_error: bool, duration_ms: u32| {
            toast.set(Some((msg, is_error)));
            toast_timeout.borrow_mut().take();
            let toast = toast.clone();
            let clear_handle = toast_timeout.clone();
            let timeout = Timeout::new(duration_ms, move || {
                toast.set(None);
                clear_handle.borrow_mut().take();
            });
            *toast_timeout.borrow_mut() = Some(timeout);
        }
    };

    let on_copy = {
        let show_toast = show_toast.clone();
        Callback::from(move |(label, value): (String, String)| {
            copy_text(&value);
            show_toast(format!("已复制{}", label), false, 1800);
        })
    };

    let on_refresh_key = {
        let access = access.clone();
        let refreshing_key = refreshing_key.clone();
        let show_toast = show_toast.clone();
        Callback::from(move |(key_id, key_name): (String, String)| {
            refreshing_key.set(Some(key_id));
            let access = access.clone();
            let refreshing_key = refreshing_key.clone();
            let show_toast = show_toast.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_access().await {
                    Ok(data) => {
                        access.set(Some(data));
                        show_toast(format!("已刷新 {}", key_name), false, 2200);
                    },
                    Err(err) => {
                        show_toast(format!("刷新失败：{}", err), true, 2200);
                    },
                }
                refreshing_key.set(None);
            });
        })
    };

    let on_refresh_status = {
        let rate_limit_status = rate_limit_status.clone();
        let status_error = status_error.clone();
        let refreshing_status = refreshing_status.clone();
        let show_toast = show_toast.clone();
        Callback::from(move |_| {
            refreshing_status.set(true);
            let rate_limit_status = rate_limit_status.clone();
            let status_error = status_error.clone();
            let refreshing_status = refreshing_status.clone();
            let show_toast = show_toast.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        rate_limit_status.set(Some(data));
                        status_error.set(None);
                        show_toast("已刷新限额快照".to_string(), false, 2200);
                    },
                    Err(err) => {
                        status_error.set(Some(err.clone()));
                        show_toast(format!("刷新失败：{}", err), true, 2200);
                    },
                }
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

    let on_scroll_to_wish = {
        let wish_section_ref = wish_section_ref.clone();
        Callback::from(move |_| {
            if let Some(section) = wish_section_ref.cast::<Element>() {
                section.scroll_into_view();
            }
        })
    };

    let on_submit_token_wish = {
        let wish_quota = wish_quota.clone();
        let wish_reason = wish_reason.clone();
        let wish_email = wish_email.clone();
        let wish_submitting = wish_submitting.clone();
        let wish_feedback = wish_feedback.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            let quota_raw = (*wish_quota).trim().to_string();
            let reason = (*wish_reason).trim().to_string();
            let email = (*wish_email).trim().to_string();
            let Ok(quota) = quota_raw.parse::<u64>() else {
                wish_feedback.set(Some(("所需 token 量必须是正整数".to_string(), true)));
                return;
            };
            if quota == 0 || reason.is_empty() || email.is_empty() {
                wish_feedback.set(Some(("token 量、缘由和邮箱都必须填写".to_string(), true)));
                return;
            }

            let frontend_page_url =
                web_sys::window().and_then(|window| window.location().href().ok());
            let wish_quota = wish_quota.clone();
            let wish_reason = wish_reason.clone();
            let wish_email = wish_email.clone();
            let wish_submitting = wish_submitting.clone();
            let wish_feedback = wish_feedback.clone();
            wish_submitting.set(true);
            wish_feedback.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match submit_llm_gateway_token_request(
                    quota,
                    &reason,
                    &email,
                    frontend_page_url.as_deref(),
                )
                .await
                {
                    Ok(_) => {
                        wish_quota.set(String::new());
                        wish_reason.set(String::new());
                        wish_email.set(String::new());
                        wish_feedback.set(Some((
                            "许愿已提交，审核通过后才会创建 token 并发送到你的邮箱。".to_string(),
                            false,
                        )));
                    },
                    Err(err) => {
                        wish_feedback.set(Some((err, true)));
                    },
                }
                wish_submitting.set(false);
            });
        })
    };

    // PLACEHOLDER_CONTENT_RENDER

    let content = if *loading {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                { "正在读取公开 Key" }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class={classes!("mt-10", "rounded-xl", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
                { err }
            </div>
        }
    } else if let Some(access) = (*access).clone() {
        let base_url = resolved_base_url(&access);

        // --- Status view ---
        let status_view = if *status_loading {
            html! {
                <div class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-12", "text-center", "text-[var(--muted)]")}>
                    { "正在读取限额快照" }
                </div>
            }
        } else if let Some(status) = (*rate_limit_status).clone() {
            let effective_status_error = (*status_error)
                .clone()
                .or_else(|| status.error_message.clone());

            // Group buckets by account_name. Buckets without account_name go
            // into a single "legacy" group so the rendering stays backward
            // compatible with the pre-multi-account era.
            let mut account_groups: Vec<(
                Option<String>,
                Vec<crate::api::LlmGatewayRateLimitBucketView>,
            )> = Vec::new();
            {
                let mut seen_order: Vec<Option<String>> = Vec::new();
                let mut map: std::collections::HashMap<
                    Option<String>,
                    Vec<crate::api::LlmGatewayRateLimitBucketView>,
                > = std::collections::HashMap::new();
                for bucket in status.buckets.iter() {
                    let key = bucket.account_name.clone();
                    if !map.contains_key(&key) {
                        seen_order.push(key.clone());
                    }
                    map.entry(key).or_default().push(bucket.clone());
                }
                for key in seen_order {
                    if let Some(buckets) = map.remove(&key) {
                        account_groups.push((key, buckets));
                    }
                }
            }

            html! {
                <section
                    ref={status_section_ref.clone()}
                    class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}
                >
                    // Header
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                            { "Codex 限额状态" }
                        </h2>
                        <div class={classes!("flex", "items-center", "gap-3")}>
                            <span class={classes!(
                                "inline-flex", "items-center", "gap-1.5",
                                "rounded-full", "border", "border-[var(--border)]",
                                "bg-[var(--surface-alt)]", "px-3", "py-1",
                                "text-xs", "font-semibold", "uppercase", "tracking-wider",
                                match status.status.as_str() {
                                    "ready" => "text-emerald-600",
                                    "degraded" => "text-amber-600",
                                    "error" => "text-red-600",
                                    _ => "text-[var(--muted)]",
                                }
                            )}>
                                <span class={classes!("inline-block", "h-2", "w-2", "rounded-full", match status.status.as_str() {
                                    "ready" => "bg-emerald-500",
                                    "degraded" => "bg-amber-500",
                                    "error" => "bg-red-500",
                                    _ => "bg-slate-400",
                                })} />
                                { status.status.clone() }
                            </span>
                            <button
                                type="button"
                                class={classes!("btn-terminal")}
                                onclick={on_refresh_status.clone()}
                                disabled={*refreshing_status}
                            >
                                <i class={classes!("fas", if *refreshing_status { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            </button>
                        </div>
                    </div>

                    // PLACEHOLDER_STATUS_BODY

                    // Render buckets grouped by account
                    { for account_groups.iter().map(|(account_name, group_buckets)| {
                        let primary_bucket = group_buckets
                            .iter()
                            .find(|b| b.is_primary)
                            .cloned()
                            .or_else(|| group_buckets.first().cloned());
                        let additional_buckets: Vec<_> = group_buckets
                            .iter()
                            .filter(|b| !b.is_primary)
                            .cloned()
                            .collect();
                        let group_label = account_name
                            .as_deref()
                            .unwrap_or("default");
                        let show_account_header = account_groups.len() > 1 || account_name.is_some();
                        html! {
                            <div class={classes!("mt-4")}>
                                if show_account_header {
                                    <div class={classes!("flex", "items-center", "gap-2", "mb-2")}>
                                        <span class={classes!(
                                            "inline-flex", "items-center",
                                            "rounded-full", "border", "border-[var(--border)]",
                                            "bg-[var(--surface-alt)]", "px-3", "py-1",
                                            "text-xs", "font-bold", "uppercase", "tracking-wider",
                                            "text-[var(--primary)]"
                                        )}>
                                            { group_label }
                                        </span>
                                    </div>
                                }
                                if let Some(primary_bucket) = primary_bucket.clone() {
                                    <div>
                                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                                            <span class={classes!("text-sm", "font-bold", "text-[var(--text)]")}>
                                                { pretty_limit_name(&primary_bucket.display_name) }
                                            </span>
                                            <div class={classes!("flex", "items-center", "gap-3")}>
                                                if let Some(credits) = primary_bucket.credits.clone() {
                                                    <span class={classes!("text-xs", "font-semibold", "text-[var(--muted)]")}>
                                                        { if !credits.has_credits { "Credits: N/A" } else if credits.unlimited { "Credits: ∞" } else { "Credits: ✓" } }
                                                    </span>
                                                }
                                                if let Some(ref plan) = primary_bucket.plan_type {
                                                    <span class={classes!("text-xs", "font-semibold", "text-[var(--muted)]")}>
                                                        { plan.clone() }
                                                    </span>
                                                }
                                            </div>
                                        </div>
                                        <div class={classes!("mt-3", "grid", "gap-3", "sm:grid-cols-2")}>
                                            if let Some(primary_window) = primary_bucket.primary.clone() {
                                                <RateLimitWindowPanel
                                                    label={"5h 窗口"}
                                                    accent_class={classes!("bg-[linear-gradient(90deg,#0f766e,#14b8a6)]")}
                                                    window={primary_window}
                                                />
                                            }
                                            if let Some(secondary_window) = primary_bucket.secondary.clone() {
                                                <RateLimitWindowPanel
                                                    label={"Weekly 窗口"}
                                                    accent_class={classes!("bg-[linear-gradient(90deg,#2563eb,#7c3aed)]")}
                                                    window={secondary_window}
                                                />
                                            }
                                        </div>
                                    </div>
                                }
                                if !additional_buckets.is_empty() {
                                    <div class={classes!("mt-3")}>
                                        <h3 class={classes!("m-0", "text-sm", "font-bold", "text-[var(--text)]")}>
                                            { format!("其他 Buckets ({})", additional_buckets.len()) }
                                        </h3>
                                        <div class={classes!("mt-3", "space-y-2")}>
                                            { for additional_buckets.iter().map(|bucket| {
                                                let bp = bucket.primary.clone();
                                                let bs = bucket.secondary.clone();
                                                html! {
                                                    <div class={classes!("flex", "items-center", "justify-between", "gap-4", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-4", "py-3", "flex-wrap")}>
                                                        <span class={classes!("text-sm", "font-semibold", "text-[var(--text)]")}>
                                                            { pretty_limit_name(&bucket.display_name) }
                                                        </span>
                                                        <div class={classes!("flex", "items-center", "gap-4", "text-sm")}>
                                                            if let Some(p) = bp {
                                                                <span class={classes!("font-bold", "text-[var(--text)]")}>
                                                                    { format!("5h {}", format_percent(p.remaining_percent)) }
                                                                </span>
                                                            }
                                                            if let Some(s) = bs {
                                                                <span class={classes!("font-bold", "text-[var(--text)]")}>
                                                                    { format!("wk {}", format_percent(s.remaining_percent)) }
                                                                </span>
                                                            }
                                                        </div>
                                                    </div>
                                                }
                                            }) }
                                        </div>
                                    </div>
                                }
                            </div>
                        }
                    }) }

                    // Snapshot meta (compact)
                    <div class={classes!("mt-4", "flex", "items-center", "gap-4", "text-xs", "text-[var(--muted)]", "flex-wrap")}>
                        <span>{ format!("每 {}s 刷新", status.refresh_interval_seconds) }</span>
                        if let Some(ts) = status.last_success_at {
                            <span>{ format!("上次成功 {}", format_ms(ts)) }</span>
                        }
                    </div>

                    if let Some(error_message) = effective_status_error {
                        <div class={classes!("mt-3", "llm-access-notice")}>
                            { error_message }
                        </div>
                    }
                </section>
            }
        } else if let Some(err) = (*status_error).clone() {
            html! {
                <div class={classes!("rounded-xl", "border", "border-red-400/35", "bg-red-500/8", "px-5", "py-5", "text-sm", "text-red-700", "dark:text-red-200")}>
                    { err }
                </div>
            }
        } else {
            Html::default()
        };

        // PLACEHOLDER_FINAL_HTML

        html! {
            <>
                // Page header
                <section class={classes!("mt-8", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div>
                            <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                                <h1 class={classes!("m-0", "text-2xl", "font-bold", "text-[var(--text)]")}>
                                    { "🦞 LLM Gateway" }
                                </h1>
                                <span class={classes!("rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                                    { format!("{} keys", access.keys.len()) }
                                </span>
                            </div>
                            <div class={classes!("mt-2", "flex", "items-center", "gap-2", "text-sm", "text-[var(--muted)]")}>
                                <code class={classes!("break-all", "text-[var(--text)]")}>{ base_url.clone() }</code>
                            </div>
                        </div>
                        <div class={classes!("flex", "items-center", "gap-2")}>
                            <button
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                onclick={{
                                    let on_copy = on_copy.clone();
                                    let base_url = base_url.clone();
                                    Callback::from(move |_| on_copy.emit(("Base URL".to_string(), base_url.clone())))
                                }}
                            >
                                <i class="fas fa-copy"></i>
                                { "复制 URL" }
                            </button>
                            <Link<Route>
                                to={Route::LlmAccessGuide}
                                classes={classes!("btn-terminal")}
                            >
                                <i class="fas fa-book"></i>
                                { "接入帮助" }
                            </Link<Route>>
                            <Link<Route>
                                to={Route::AdminLlmGateway}
                                classes={classes!("btn-terminal")}
                            >
                                <i class="fas fa-sliders"></i>
                                { "Admin" }
                            </Link<Route>>
                        </div>
                    </div>
                </section>

                // Notice bar (remote compact warning)
                <div class={classes!("mt-4", "llm-access-notice")}>
                    { "接 Codex 请确认中转站保留了 remote compact — " }
                    <Link<Route>
                        to={Route::LlmAccessGuide}
                        classes={classes!("underline", "text-[var(--primary)]")}
                    >
                        { "接入帮助" }
                    </Link<Route>>
                    { " · " }
                    <Link<Route>
                        to={Route::ArticleDetail {
                            id: REMOTE_COMPACT_ARTICLE_ID.to_string(),
                        }}
                        classes={classes!("underline", "text-[var(--primary)]")}
                    >
                        { "深潜文章" }
                    </Link<Route>>
                </div>

                // Keys section
                <section class={classes!("mt-6")}>
                    <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                        { "公开 Key" }
                    </h2>
                    if access.keys.is_empty() {
                        <div class={classes!("mt-3", "rounded-xl", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-10", "text-center", "text-[var(--muted)]")}>
                            { "当前没有公开放出的 Key" }
                        </div>
                    } else {
                        <div class={classes!("mt-3", "grid", "gap-4", "lg:grid-cols-2")}>
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

                // Status section
                <section class={classes!("mt-6")}>
                    { status_view }
                </section>

                // Token wish section
                <section
                    ref={wish_section_ref.clone()}
                    class={classes!("mt-6", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}
                >
                    <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "text-lg", "font-bold", "text-[var(--text)]")}>
                                { "许愿 Token" }
                            </h2>
                            <p class={classes!("mt-2", "m-0", "text-sm", "leading-6", "text-[var(--muted)]")}>
                                { "如果当前公开 key 不够用，可以在这里提交额度申请。只有 admin 审核通过后，系统才会创建新 token，并把它发到你填写的邮箱。" }
                            </p>
                        </div>
                        <div class={classes!("rounded-full", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-1", "text-xs", "font-semibold", "text-[var(--muted)]")}>
                            { "邮箱必填" }
                        </div>
                    </div>

                    <form class={classes!("mt-5", "grid", "gap-4")} onsubmit={on_submit_token_wish}>
                        <div class={classes!("grid", "gap-4", "lg:grid-cols-2")}>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "所需 token 量" }</span>
                                <input
                                    type="number"
                                    min="1"
                                    step="1"
                                    placeholder="例如 500000"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-[var(--text)]")}
                                    value={(*wish_quota).clone()}
                                    oninput={{
                                        let wish_quota = wish_quota.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                wish_quota.set(target.value());
                                            }
                                        })
                                    }}
                                    required=true
                                />
                            </label>
                            <label class={classes!("text-sm")}>
                                <span class={classes!("text-[var(--muted)]")}>{ "邮箱" }</span>
                                <input
                                    type="email"
                                    placeholder="you@example.com"
                                    class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-[var(--text)]")}
                                    value={(*wish_email).clone()}
                                    oninput={{
                                        let wish_email = wish_email.clone();
                                        Callback::from(move |event: InputEvent| {
                                            if let Some(target) = event.target_dyn_into::<HtmlInputElement>() {
                                                wish_email.set(target.value());
                                            }
                                        })
                                    }}
                                    required=true
                                />
                            </label>
                        </div>

                        <label class={classes!("text-sm")}>
                            <span class={classes!("text-[var(--muted)]")}>{ "缘由" }</span>
                            <textarea
                                rows="4"
                                placeholder="说清楚你准备用这些 token 做什么、为什么需要这个量。"
                                class={classes!("mt-1", "w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-[var(--text)]", "resize-y")}
                                value={(*wish_reason).clone()}
                                oninput={{
                                    let wish_reason = wish_reason.clone();
                                    Callback::from(move |event: InputEvent| {
                                        if let Some(target) = event.target_dyn_into::<HtmlTextAreaElement>() {
                                            wish_reason.set(target.value());
                                        }
                                    })
                                }}
                                required=true
                            />
                        </label>

                        <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                            <p class={classes!("m-0", "text-xs", "leading-6", "text-[var(--muted)]")}>
                                { "提交后不会立刻发 token。管理员会先在后台审核，审核通过时才会创建 key 并发送邮件。" }
                            </p>
                            <button
                                type="submit"
                                class={classes!("btn-terminal", "btn-terminal-primary")}
                                disabled={*wish_submitting}
                            >
                                <i class={classes!("fas", if *wish_submitting { "fa-spinner animate-spin" } else { "fa-paper-plane" })}></i>
                                { if *wish_submitting { "提交中..." } else { "提交许愿" } }
                            </button>
                        </div>

                        if let Some((message, is_error)) = (*wish_feedback).clone() {
                            <div class={classes!(
                                "rounded-lg", "border", "px-4", "py-3", "text-sm",
                                if is_error {
                                    classes!("border-red-400/35", "bg-red-500/8", "text-red-700", "dark:text-red-200")
                                } else {
                                    classes!("border-emerald-400/35", "bg-emerald-500/8", "text-emerald-700", "dark:text-emerald-200")
                                }
                            )}>
                                { message }
                            </div>
                        }
                    </form>
                </section>
            </>
        }
    } else {
        Html::default()
    };

    html! {
        <main class={classes!("relative", "min-h-screen", "bg-[var(--bg)]")}>
            <div class={classes!("relative", "mx-auto", "max-w-5xl", "px-4", "pb-16", "pt-8", "lg:px-6")}>
                { content }
            </div>

            <button
                type="button"
                class={classes!(
                    "fixed", "bottom-24", "left-5", "z-[85]",
                    "btn-terminal",
                    "!rounded-full", "!px-4", "!py-2.5",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.15)]"
                )}
                onclick={on_scroll_to_wish}
            >
                <i class="fas fa-envelope-open-text"></i>
                { "许愿 Token" }
            </button>

            <button
                type="button"
                class={classes!(
                    "fixed", "bottom-24", "right-5", "z-[85]",
                    "btn-terminal", "btn-terminal-primary",
                    "!rounded-full", "!px-4", "!py-2.5",
                    "shadow-[0_8px_24px_rgba(0,0,0,0.15)]"
                )}
                onclick={on_scroll_to_status}
            >
                <span class={classes!("relative", "flex", "h-2", "w-2")}>
                    <span class={classes!("absolute", "inline-flex", "h-full", "w-full", "animate-ping", "rounded-full", "bg-white/60")}></span>
                    <span class={classes!("relative", "inline-flex", "h-2", "w-2", "rounded-full", "bg-white")}></span>
                </span>
                { "限额状态" }
            </button>

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
