use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_llm_gateway_status, LlmGatewayPublicAccountStatusView, LlmGatewayRateLimitBucketView,
        LlmGatewayRateLimitStatusResponse, LlmGatewayRateLimitWindowView,
    },
    components::pagination::Pagination,
    pages::llm_access_shared::{
        format_ms, format_percent, format_reset_hint, format_window_label, pretty_limit_name,
    },
    router::Route,
};

const PAGE_SIZE: usize = 6;


#[derive(Clone, PartialEq)]
struct AccountEntry {
    name: String,
    summary: Option<LlmGatewayPublicAccountStatusView>,
    buckets: Vec<LlmGatewayRateLimitBucketView>,
}

fn build_account_entries(status: &LlmGatewayRateLimitStatusResponse) -> Vec<AccountEntry> {
    let mut seen_order: Vec<Option<String>> = Vec::new();
    let mut bucket_map: std::collections::HashMap<
        Option<String>,
        Vec<LlmGatewayRateLimitBucketView>,
    > = std::collections::HashMap::new();
    for bucket in status.buckets.iter() {
        let key = bucket.account_name.clone();
        if !bucket_map.contains_key(&key) {
            seen_order.push(key.clone());
        }
        bucket_map.entry(key).or_default().push(bucket.clone());
    }
    let mut entries = Vec::new();
    if !status.accounts.is_empty() {
        for account in status.accounts.iter() {
            let key = Some(account.name.clone());
            entries.push(AccountEntry {
                name: account.name.clone(),
                summary: Some(account.clone()),
                buckets: bucket_map.remove(&key).unwrap_or_default(),
            });
        }
        for key in seen_order {
            if let Some(buckets) = bucket_map.remove(&key) {
                entries.push(AccountEntry {
                    name: key.unwrap_or_else(|| "default".to_string()),
                    summary: None,
                    buckets,
                });
            }
        }
    } else {
        for key in seen_order {
            if let Some(buckets) = bucket_map.remove(&key) {
                entries.push(AccountEntry {
                    name: key.unwrap_or_else(|| "default".to_string()),
                    summary: None,
                    buckets,
                });
            }
        }
    }
    entries
}

fn matches_filter(entry: &AccountEntry, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if entry.name.to_lowercase().contains(&q) {
        return true;
    }
    if let Some(ref summary) = entry.summary {
        if summary.status.to_lowercase().contains(&q) {
            return true;
        }
        if let Some(ref plan) = summary.plan_type {
            if plan.to_lowercase().contains(&q) {
                return true;
            }
        }
    }
    for bucket in &entry.buckets {
        if bucket.display_name.to_lowercase().contains(&q) {
            return true;
        }
        if let Some(ref plan) = bucket.plan_type {
            if plan.to_lowercase().contains(&q) {
                return true;
            }
        }
    }
    false
}

fn status_dot_class(status: &str) -> &'static str {
    match status {
        "active" => "bg-emerald-500",
        "unavailable" => "bg-amber-500",
        "ready" => "bg-emerald-500",
        "degraded" => "bg-amber-500",
        "error" => "bg-red-500",
        _ => "bg-slate-400",
    }
}

fn status_text_class(status: &str) -> &'static str {
    match status {
        "active" | "ready" => "text-emerald-600",
        "unavailable" | "degraded" => "text-amber-600",
        "error" => "text-red-600",
        _ => "text-[var(--muted)]",
    }
}

const ACCENT_BORDERS: &[&str] = &[
    "border-l-4 border-l-teal-500/70",
    "border-l-4 border-l-violet-500/70",
    "border-l-4 border-l-amber-500/70",
    "border-l-4 border-l-sky-500/70",
    "border-l-4 border-l-rose-500/70",
];

#[derive(Clone, Copy, PartialEq)]
enum SortMode {
    None,
    Primary5hAsc,
    Primary5hDesc,
    WeeklyAsc,
    WeeklyDesc,
}

fn primary_remaining(entry: &AccountEntry) -> f64 {
    entry
        .summary
        .as_ref()
        .and_then(|s| s.primary_remaining_percent)
        .or_else(|| {
            let bucket = entry
                .buckets
                .iter()
                .find(|b| b.is_primary)
                .or(entry.buckets.first())?;
            bucket.primary.as_ref().map(|w| w.remaining_percent)
        })
        .unwrap_or(100.0)
}

fn weekly_remaining(entry: &AccountEntry) -> f64 {
    entry
        .summary
        .as_ref()
        .and_then(|s| s.secondary_remaining_percent)
        .or_else(|| {
            let bucket = entry
                .buckets
                .iter()
                .find(|b| b.is_primary)
                .or(entry.buckets.first())?;
            bucket.secondary.as_ref().map(|w| w.remaining_percent)
        })
        .unwrap_or(100.0)
}

fn is_unavailable(entry: &AccountEntry) -> bool {
    if let Some(ref summary) = entry.summary {
        if summary.status == "unavailable" || summary.usage_error_message.is_some() {
            return true;
        }
    }
    false
}

fn sort_entries(entries: &mut [AccountEntry], mode: SortMode) {
    match mode {
        SortMode::None => {},
        SortMode::Primary5hAsc => entries.sort_by(|a, b| {
            primary_remaining(a)
                .partial_cmp(&primary_remaining(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::Primary5hDesc => entries.sort_by(|a, b| {
            primary_remaining(b)
                .partial_cmp(&primary_remaining(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::WeeklyAsc => entries.sort_by(|a, b| {
            weekly_remaining(a)
                .partial_cmp(&weekly_remaining(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::WeeklyDesc => entries.sort_by(|a, b| {
            weekly_remaining(b)
                .partial_cmp(&weekly_remaining(a))
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
}

// PLACEHOLDER_WINDOW_BAR

#[derive(Properties, PartialEq)]
struct WindowBarProps {
    label: AttrValue,
    accent: AttrValue,
    window: LlmGatewayRateLimitWindowView,
}

#[function_component(WindowBar)]
fn window_bar(props: &WindowBarProps) -> Html {
    let width = props.window.remaining_percent.clamp(0.0, 100.0);
    html! {
        <div class={classes!("flex-1", "min-w-[140px]")}>
            <div class={classes!("flex", "items-center", "justify-between", "gap-2", "mb-1.5")}>
                <span class={classes!("font-mono", "text-[11px]", "font-semibold", "text-[var(--muted)]", "uppercase", "tracking-wider")}>
                    { props.label.clone() }
                </span>
                <span class={classes!("font-mono", "text-sm", "font-black", "text-[var(--text)]")}>
                    { format_percent(props.window.remaining_percent) }
                </span>
            </div>
            <div class={classes!("h-2", "overflow-hidden", "rounded-full", "bg-[var(--surface-alt)]")}>
                <div
                    class={classes!("h-full", "rounded-full", "transition-[width]", "duration-500", props.accent.to_string())}
                    style={format!("width: {width:.2}%;")}
                />
            </div>
            <div class={classes!("mt-1", "flex", "items-center", "gap-3", "font-mono", "text-[10px]", "text-[var(--muted)]")}>
                <span>{ format!("已用 {}", format_percent(props.window.used_percent)) }</span>
                <span>{ format_window_label(props.window.window_duration_mins, "") }</span>
                <span>{ format_reset_hint(props.window.resets_at) }</span>
            </div>
        </div>
    }
}

// PLACEHOLDER_ACCOUNT_CARD

#[derive(Properties, PartialEq)]
struct AccountCardProps {
    entry: AccountEntry,
    index: usize,
}

#[function_component(AccountCard)]
fn account_card(props: &AccountCardProps) -> Html {
    let entry = &props.entry;
    let accent = ACCENT_BORDERS[props.index % ACCENT_BORDERS.len()];
    let status_str = entry
        .summary
        .as_ref()
        .map(|s| s.status.as_str())
        .unwrap_or("active");
    let plan_type = entry
        .summary
        .as_ref()
        .and_then(|s| s.plan_type.clone())
        .or_else(|| entry.buckets.iter().find_map(|b| b.plan_type.clone()));
    let primary_bucket = entry
        .buckets
        .iter()
        .find(|b| b.is_primary)
        .cloned()
        .or_else(|| entry.buckets.first().cloned());
    let additional_buckets: Vec<_> = entry
        .buckets
        .iter()
        .filter(|b| !b.is_primary)
        .cloned()
        .collect();

    html! {
            <article class={classes!(
                "rounded-xl", "border", "border-[var(--border)]",
                "bg-[var(--surface)]", "p-5", "overflow-hidden",
                "transition-all", "duration-200",
                "hover:shadow-lg", "hover:shadow-black/5",
                accent,
            )}>
                // Header
                <div class={classes!("flex", "items-start", "justify-between", "gap-3", "min-w-0")}>
                    <div class={classes!("flex", "items-start", "gap-2.5", "min-w-0", "flex-1", "flex-wrap")}>
                        <span class={classes!(
                            "font-mono", "text-sm", "font-bold", "text-[var(--text)]",
                            "break-all", "min-w-0",
                        )}>
                            { entry.name.clone() }
                        </span>
                        <span class={classes!(
                            "inline-flex", "items-center", "gap-1.5", "shrink-0",
                            "rounded-full", "px-2", "py-0.5",
                            "font-mono", "text-[10px]", "font-semibold", "uppercase", "tracking-wider",
                            "bg-[var(--surface-alt)]",
                            status_text_class(status_str),
                        )}>
                            <span class={classes!("inline-block", "h-1.5", "w-1.5", "rounded-full", status_dot_class(status_str))} />
                            { status_str }
                        </span>
                    </div>
                    if let Some(ref plan) = plan_type {
                        <span class={classes!(
                            "rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "shrink-0",
                            "font-mono", "text-[10px]", "font-medium", "text-[var(--muted)]",
                        )}>
                            { plan.clone() }
                        </span>
                    }
                </div>

                // Primary bucket windows
                if let Some(ref bucket) = primary_bucket {
                    <div class={classes!("mt-4")}>
                        <div class={classes!("mb-2", "font-mono", "text-xs", "font-semibold", "text-[var(--text)]")}>
                            { pretty_limit_name(&bucket.display_name) }
                        </div>
                        <div class={classes!("flex", "gap-4", "flex-wrap")}>
                            if let Some(ref w) = bucket.primary {
                                <WindowBar
                                    label={"5h"}
                                    accent={"bg-[linear-gradient(90deg,#0f766e,#14b8a6)]"}
                                    window={w.clone()}
                                />
                            }
                            if let Some(ref w) = bucket.secondary {
                                <WindowBar
                                    label={"weekly"}
                                    accent={"bg-[linear-gradient(90deg,#2563eb,#7c3aed)]"}
                                    window={w.clone()}
                                />
                            }
                        </div>
                    </div>
                }

    // PLACEHOLDER_ADDITIONAL_BUCKETS

                // Additional buckets
                if !additional_buckets.is_empty() {
                    <div class={classes!("mt-3", "space-y-2")}>
                        { for additional_buckets.iter().map(|bucket| {
                            html! {
                                <div class={classes!(
                                    "flex", "items-center", "justify-between", "gap-4",
                                    "rounded-lg", "border", "border-[var(--border)]",
                                    "bg-[var(--surface-alt)]", "px-3", "py-2", "flex-wrap",
                                )}>
                                    <span class={classes!("font-mono", "text-xs", "font-semibold", "text-[var(--text)]")}>
                                        { pretty_limit_name(&bucket.display_name) }
                                    </span>
                                    <div class={classes!("flex", "items-center", "gap-3", "font-mono", "text-xs")}>
                                        if let Some(ref p) = bucket.primary {
                                            <span class={classes!("text-[var(--text)]")}>
                                                { format!("5h {}", format_percent(p.remaining_percent)) }
                                            </span>
                                        }
                                        if let Some(ref s) = bucket.secondary {
                                            <span class={classes!("text-[var(--text)]")}>
                                                { format!("wk {}", format_percent(s.remaining_percent)) }
                                            </span>
                                        }
                                    </div>
                                </div>
                            }
                        }) }
                    </div>
                }

                // Timestamps
                if let Some(ref summary) = entry.summary {
                    if summary.last_usage_checked_at.is_some() || summary.last_usage_success_at.is_some() {
                        <div class={classes!("mt-3", "flex", "items-center", "gap-3", "font-mono", "text-[10px]", "text-[var(--muted)]", "flex-wrap")}>
                            if let Some(ts) = summary.last_usage_checked_at {
                                <span>{ format!("checked {}", format_ms(ts)) }</span>
                            }
                            if let Some(ts) = summary.last_usage_success_at {
                                <span>{ format!("ok {}", format_ms(ts)) }</span>
                            }
                        </div>
                    }
                    if let Some(ref err) = summary.usage_error_message {
                        <div class={classes!("mt-2", "font-mono", "text-[11px]", "text-amber-700", "dark:text-amber-200")}>
                            { err.clone() }
                        </div>
                    }
                }
            </article>
        }
}

// PLACEHOLDER_MAIN_COMPONENT

#[function_component(LlmAccessQuotaStatusPage)]
pub fn llm_access_quota_status_page() -> Html {
    let status_data = use_state(|| None::<LlmGatewayRateLimitStatusResponse>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let search_input = use_state(String::new);
    let active_query = use_state(String::new);
    let current_page = use_state(|| 1usize);
    let refreshing = use_state(|| false);
    let sort_mode = use_state(|| SortMode::None);
    let show_unavailable = use_state(|| false);

    {
        let status_data = status_data.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        status_data.set(Some(data));
                        error.set(None);
                    },
                    Err(err) => {
                        error.set(Some(err));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let on_refresh = {
        let status_data = status_data.clone();
        let error = error.clone();
        let refreshing = refreshing.clone();
        Callback::from(move |_: MouseEvent| {
            let status_data = status_data.clone();
            let error = error.clone();
            let refreshing = refreshing.clone();
            refreshing.set(true);
            spawn_local(async move {
                match fetch_llm_gateway_status().await {
                    Ok(data) => {
                        status_data.set(Some(data));
                        error.set(None);
                    },
                    Err(err) => {
                        error.set(Some(err));
                    },
                }
                refreshing.set(false);
            });
        })
    };

    let on_search_input = {
        let search_input = search_input.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(target) = e.target_dyn_into::<HtmlInputElement>() {
                search_input.set(target.value());
            }
        })
    };

    let on_search_submit = {
        let search_input = search_input.clone();
        let active_query = active_query.clone();
        let current_page = current_page.clone();
        Callback::from(move |_: ()| {
            active_query.set((*search_input).clone());
            current_page.set(1);
        })
    };

    let on_search_keydown = {
        let on_search_submit = on_search_submit.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                on_search_submit.emit(());
            }
        })
    };

    let on_clear = {
        let search_input = search_input.clone();
        let active_query = active_query.clone();
        let current_page = current_page.clone();
        Callback::from(move |_: MouseEvent| {
            search_input.set(String::new());
            active_query.set(String::new());
            current_page.set(1);
        })
    };

    // PLACEHOLDER_RENDER_BODY

    let all_entries = (*status_data)
        .as_ref()
        .map(build_account_entries)
        .unwrap_or_default();
    let mut filtered: Vec<_> = all_entries
        .iter()
        .filter(|e| matches_filter(e, &active_query))
        .filter(|e| !*show_unavailable || is_unavailable(e))
        .cloned()
        .collect();
    sort_entries(&mut filtered, *sort_mode);
    let total_pages = filtered.len().max(1).div_ceil(PAGE_SIZE.max(1));
    let page = (*current_page).clamp(1, total_pages);
    let page_entries: Vec<_> = filtered
        .iter()
        .skip((page - 1) * PAGE_SIZE)
        .take(PAGE_SIZE)
        .cloned()
        .collect();

    let on_page_change = {
        let current_page = current_page.clone();
        Callback::from(move |p: usize| current_page.set(p))
    };

    let global_status = (*status_data)
        .as_ref()
        .map(|s| s.status.as_str())
        .unwrap_or("unknown");

    html! {
            <main class={classes!("mx-auto", "max-w-4xl", "px-4", "py-8", "sm:px-6")}>
                // Back link + title
                <div class={classes!("mb-6")}>
                    <Link<Route> to={Route::LlmAccess} classes={classes!(
                        "inline-flex", "items-center", "gap-1.5",
                        "font-mono", "text-xs", "text-[var(--muted)]",
                        "hover:text-[var(--primary)]", "transition-colors",
                    )}>
                        <i class="fas fa-arrow-left text-[10px]"></i>
                        { "返回" }
                    </Link<Route>>
                    <div class={classes!("mt-3", "flex", "items-center", "justify-between", "gap-4", "flex-wrap")}>
                        <div class={classes!("flex", "items-center", "gap-3")}>
                            <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>
                                { "限额状态" }
                            </h1>
                            <span class={classes!(
                                "inline-flex", "items-center", "gap-1.5",
                                "rounded-full", "px-2.5", "py-0.5",
                                "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-wider",
                                "bg-[var(--surface-alt)]",
                                status_text_class(global_status),
                            )}>
                                <span class={classes!("inline-block", "h-1.5", "w-1.5", "rounded-full", status_dot_class(global_status))} />
                                { global_status }
                            </span>
                        </div>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={on_refresh.clone()}
                            disabled={*refreshing}
                        >
                            <i class={classes!("fas", if *refreshing { "fa-spinner animate-spin" } else { "fa-rotate-right" })}></i>
                            <span class={classes!("ml-1.5", "text-xs")}>{ "刷新" }</span>
                        </button>
                    </div>
                </div>

    // PLACEHOLDER_SEARCH_AND_CONTENT

                // Search bar
                <div class={classes!(
                    "mb-6", "flex", "items-center", "gap-3",
                    "rounded-xl", "border", "border-[var(--border)]",
                    "bg-[var(--surface)]", "px-4", "py-3",
                    "focus-within:border-[var(--primary)]", "focus-within:ring-1", "focus-within:ring-[var(--primary)]/30",
                    "transition-all", "duration-200",
                )}>
                    <i class={classes!("fas", "fa-search", "text-sm", "text-[var(--muted)]")}></i>
                    <input
                        type="text"
                        class={classes!(
                            "flex-1", "bg-transparent", "border-none", "outline-none",
                            "font-mono", "text-sm", "text-[var(--text)]",
                            "placeholder:text-[var(--muted)]",
                        )}
                        placeholder="搜索账号名、Plan 类型..."
                        value={(*search_input).clone()}
                        oninput={on_search_input}
                        onkeydown={on_search_keydown}
                    />
                    if !(*active_query).is_empty() {
                        <button
                            type="button"
                            class={classes!(
                                "inline-flex", "items-center", "justify-center",
                                "h-6", "w-6", "rounded-full",
                                "text-[var(--muted)]", "hover:text-[var(--text)]",
                                "hover:bg-[var(--surface-alt)]", "transition-colors",
                            )}
                            onclick={on_clear}
                            title="清除搜索"
                        >
                            <i class="fas fa-times text-xs"></i>
                        </button>
                    }
                    <button
                        type="button"
                        class={classes!("btn-terminal", "text-xs")}
                        onclick={Callback::from(move |_| on_search_submit.emit(()))}
                    >
                        { "搜索" }
                    </button>
                </div>

                // Sort & filter toolbar
                <div class={classes!(
                    "mb-4", "flex", "items-center", "gap-2", "flex-wrap",
                    "font-mono", "text-xs",
                )}>
                    // Unavailable filter toggle
                    <button
                        type="button"
                        class={classes!(
                            "inline-flex", "items-center", "gap-1.5",
                            "rounded-full", "px-3", "py-1.5",
                            "border", "transition-colors",
                            if *show_unavailable {
                                "border-red-400/60 bg-red-500/10 text-red-600 dark:text-red-300"
                            } else {
                                "border-[var(--border)] text-[var(--muted)] hover:border-[var(--primary)]/50 hover:text-[var(--text)]"
                            },
                        )}
                        onclick={{
                            let show_unavailable = show_unavailable.clone();
                            let current_page = current_page.clone();
                            Callback::from(move |_: MouseEvent| {
                                show_unavailable.set(!*show_unavailable);
                                current_page.set(1);
                            })
                        }}
                    >
                        <i class="fas fa-exclamation-triangle text-[10px]"></i>
                        { "不可用" }
                    </button>

                    <span class={classes!("text-[var(--border)]", "mx-1")}>{ "|" }</span>

                    // Sort buttons
                    <span class={classes!("text-[var(--muted)]")}>{ "排序:" }</span>
                    <button
                        type="button"
                        class={classes!(
                            "inline-flex", "items-center", "gap-1",
                            "rounded-full", "px-2.5", "py-1.5",
                            "border", "transition-colors",
                            if *sort_mode == SortMode::Primary5hAsc || *sort_mode == SortMode::Primary5hDesc {
                                "border-teal-400/60 bg-teal-500/10 text-teal-700 dark:text-teal-300"
                            } else {
                                "border-[var(--border)] text-[var(--muted)] hover:border-[var(--primary)]/50 hover:text-[var(--text)]"
                            },
                        )}
                        onclick={{
                            let sort_mode = sort_mode.clone();
                            let current_page = current_page.clone();
                            Callback::from(move |_: MouseEvent| {
                                let next = match *sort_mode {
                                    SortMode::Primary5hAsc => SortMode::Primary5hDesc,
                                    SortMode::Primary5hDesc => SortMode::None,
                                    _ => SortMode::Primary5hAsc,
                                };
                                sort_mode.set(next);
                                current_page.set(1);
                            })
                        }}
                    >
                        { "5h" }
                        { match *sort_mode {
                            SortMode::Primary5hAsc => html! { <i class="fas fa-arrow-up text-[9px]"></i> },
                            SortMode::Primary5hDesc => html! { <i class="fas fa-arrow-down text-[9px]"></i> },
                            _ => Html::default(),
                        }}
                    </button>
                    <button
                        type="button"
                        class={classes!(
                            "inline-flex", "items-center", "gap-1",
                            "rounded-full", "px-2.5", "py-1.5",
                            "border", "transition-colors",
                            if *sort_mode == SortMode::WeeklyAsc || *sort_mode == SortMode::WeeklyDesc {
                                "border-violet-400/60 bg-violet-500/10 text-violet-700 dark:text-violet-300"
                            } else {
                                "border-[var(--border)] text-[var(--muted)] hover:border-[var(--primary)]/50 hover:text-[var(--text)]"
                            },
                        )}
                        onclick={{
                            let sort_mode = sort_mode.clone();
                            let current_page = current_page.clone();
                            Callback::from(move |_: MouseEvent| {
                                let next = match *sort_mode {
                                    SortMode::WeeklyAsc => SortMode::WeeklyDesc,
                                    SortMode::WeeklyDesc => SortMode::None,
                                    _ => SortMode::WeeklyAsc,
                                };
                                sort_mode.set(next);
                                current_page.set(1);
                            })
                        }}
                    >
                        { "周限额" }
                        { match *sort_mode {
                            SortMode::WeeklyAsc => html! { <i class="fas fa-arrow-up text-[9px]"></i> },
                            SortMode::WeeklyDesc => html! { <i class="fas fa-arrow-down text-[9px]"></i> },
                            _ => Html::default(),
                        }}
                    </button>
                </div>

                // Content
                if *loading {
                    <div class={classes!(
                        "rounded-xl", "border", "border-dashed", "border-[var(--border)]",
                        "px-5", "py-16", "text-center",
                        "font-mono", "text-sm", "text-[var(--muted)]",
                    )}>
                        <i class="fas fa-spinner animate-spin mr-2"></i>
                        { "加载中..." }
                    </div>
                } else if let Some(ref err) = *error {
                    <div class={classes!(
                        "rounded-xl", "border", "border-red-400/35", "bg-red-500/8",
                        "px-5", "py-5", "font-mono", "text-sm",
                        "text-red-700", "dark:text-red-200",
                    )}>
                        { err.clone() }
                    </div>
                } else if filtered.is_empty() {
                    <div class={classes!(
                        "rounded-xl", "border", "border-dashed", "border-[var(--border)]",
                        "px-5", "py-16", "text-center",
                        "font-mono", "text-sm", "text-[var(--muted)]",
                    )}>
                        if *show_unavailable && (*active_query).is_empty() {
                            { "当前没有不可用的账号" }
                        } else if (*active_query).is_empty() {
                            { "暂无限额数据" }
                        } else {
                            { format!("未找到匹配 \"{}\" 的账号", *active_query) }
                        }
                    </div>
    // PLACEHOLDER_CARDS_AND_PAGINATION
                } else {
                    <div class={classes!("space-y-4")}>
                        // Summary line
                        <div class={classes!("flex", "items-center", "justify-between", "font-mono", "text-xs", "text-[var(--muted)]")}>
                            <span>{ format!("共 {} 个账号", filtered.len()) }</span>
                            <span>{ format!("{} / {}", page, total_pages) }</span>
                        </div>

                        // Cards grid
                        <div class={classes!("grid", "gap-4", "sm:grid-cols-2")}>
                            { for page_entries.iter().enumerate().map(|(i, entry)| {
                                let global_idx = (page - 1) * PAGE_SIZE + i;
                                html! { <AccountCard entry={entry.clone()} index={global_idx} /> }
                            }) }
                        </div>

                        // Pagination
                        <Pagination
                            current_page={page}
                            total_pages={total_pages}
                            on_page_change={on_page_change}
                        />
                    </div>
                }

                // Footer metadata
                if let Some(ref data) = *status_data {
                    <div class={classes!("mt-6", "flex", "items-center", "gap-4", "font-mono", "text-[11px]", "text-[var(--muted)]", "flex-wrap")}>
                        <span>{ format!("refresh {}s", data.refresh_interval_seconds) }</span>
                        if let Some(ts) = data.last_success_at {
                            <span>{ format!("last_ok {}", format_ms(ts)) }</span>
                        }
                    </div>
                }
            </main>
        }
}
