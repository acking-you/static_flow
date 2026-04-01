use web_sys::{HtmlInputElement, KeyboardEvent};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_public_llm_gateway_usage, PublicLlmGatewayUsageLookupRequest,
        PublicLlmGatewayUsageLookupResponse,
    },
    components::{pagination::Pagination, token_usage_trend_chart::TokenUsageTrendChart},
    pages::llm_access_shared::{format_ms, format_number_i64, format_number_u64},
    router::Route,
};

const PUBLIC_USAGE_PAGE_LIMIT: usize = 50;

fn provider_badge_label(provider_type: &str) -> String {
    let trimmed = provider_type.trim();
    if trimmed.is_empty() {
        return "unknown".to_string();
    }
    let mut chars = trimmed.chars();
    match chars.next() {
        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
        None => "unknown".to_string(),
    }
}

fn normalize_public_lookup_error(raw: &str) -> String {
    let normalized = raw.trim();
    if normalized.is_empty() {
        return "查询失败，请稍后再试".to_string();
    }
    if normalized.contains("queryable key not found") {
        return "未找到可查询的 key".to_string();
    }
    if normalized.contains("api_key is required") {
        return "请输入要查询的 key".to_string();
    }
    normalized.to_string()
}

#[function_component(LlmAccessUsagePage)]
pub fn llm_access_usage_page() -> Html {
    let key_input = use_state(String::new);
    let active_key = use_state(|| None::<String>);
    let lookup = use_state(|| None::<PublicLlmGatewayUsageLookupResponse>);
    let loading = use_state(|| false);
    let error = use_state(|| None::<String>);
    let current_page = use_state(|| 1usize);
    let show_key = use_state(|| false);

    let perform_lookup = {
        let active_key = active_key.clone();
        let lookup = lookup.clone();
        let loading = loading.clone();
        let error = error.clone();
        let current_page = current_page.clone();
        Callback::from(move |(api_key, page, clear_existing): (String, usize, bool)| {
            let trimmed = api_key.trim().to_string();
            if trimmed.is_empty() {
                error.set(Some("请输入要查询的 key".to_string()));
                return;
            }

            if clear_existing {
                lookup.set(None);
            }
            active_key.set(Some(trimmed.clone()));
            current_page.set(page);
            loading.set(true);
            error.set(None);

            let lookup = lookup.clone();
            let loading = loading.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let request = PublicLlmGatewayUsageLookupRequest {
                    api_key: trimmed,
                    limit: Some(PUBLIC_USAGE_PAGE_LIMIT),
                    offset: Some((page.saturating_sub(1)).saturating_mul(PUBLIC_USAGE_PAGE_LIMIT)),
                };
                match fetch_public_llm_gateway_usage(&request).await {
                    Ok(response) => lookup.set(Some(response)),
                    Err(err) => error.set(Some(normalize_public_lookup_error(&err))),
                }
                loading.set(false);
            });
        })
    };

    let on_submit = {
        let key_input = key_input.clone();
        let perform_lookup = perform_lookup.clone();
        Callback::from(move |_| {
            perform_lookup.emit(((*key_input).clone(), 1, true));
        })
    };

    let on_submit_click = {
        let on_submit = on_submit.clone();
        Callback::from(move |_| on_submit.emit(()))
    };

    let on_keydown = {
        let on_submit = on_submit.clone();
        Callback::from(move |event: KeyboardEvent| {
            if event.key() == "Enter" {
                event.prevent_default();
                on_submit.emit(());
            }
        })
    };

    let on_page_change = {
        let active_key = active_key.clone();
        let perform_lookup = perform_lookup.clone();
        Callback::from(move |page: usize| {
            if let Some(api_key) = (*active_key).clone() {
                perform_lookup.emit((api_key, page, false));
            }
        })
    };

    let on_refresh = {
        let active_key = active_key.clone();
        let current_page = current_page.clone();
        let perform_lookup = perform_lookup.clone();
        Callback::from(move |_| {
            if let Some(api_key) = (*active_key).clone() {
                perform_lookup.emit((api_key, *current_page, false));
            }
        })
    };

    let total_pages = (*lookup).as_ref().map_or(1, |response| {
        let total = response.total.max(1);
        let limit = response.limit.max(1);
        total.div_ceil(limit)
    });

    html! {
        <div class={classes!("mt-8", "space-y-6")}>
            <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                    <div class={classes!("max-w-3xl")}>
                        <div class={classes!("flex", "items-center", "gap-3", "flex-wrap")}>
                            <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>
                                { "Key Usage Lookup" }
                            </h1>
                            <span class={classes!("rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-0.5", "font-mono", "text-[11px]", "font-semibold", "text-[var(--muted)]")}>
                                { "公开查询页" }
                            </span>
                        </div>
                        <p class={classes!("mt-3", "mb-0", "max-w-3xl", "text-sm", "leading-7", "text-[var(--muted)]")}>
                            { "输入你的公开 gateway key 后，可以查看该 key 的总额度、分页 usage 日志，以及最近 24 小时 token 用量趋势。" }
                        </p>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <Link<Route> to={Route::LlmAccess} classes={classes!("btn-terminal")}>
                            <i class="fas fa-arrow-left"></i>
                            { "返回 LLM Access" }
                        </Link<Route>>
                        <Link<Route> to={Route::LlmAccessGuide} classes={classes!("btn-terminal")}>
                            <i class="fas fa-book"></i>
                            { "接入帮助" }
                        </Link<Route>>
                    </div>
                </div>

                <div class={classes!("mt-5", "grid", "gap-3", "lg:grid-cols-[minmax(0,1fr)_auto]")}>
                    <div class={classes!("flex", "items-stretch", "gap-2", "rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "p-2")}>
                        <input
                            type={if *show_key { "text" } else { "password" }}
                            class={classes!("min-w-0", "flex-1", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-3", "font-mono", "text-sm", "text-[var(--text)]")}
                            placeholder="sfk_..."
                            value={(*key_input).clone()}
                            oninput={{
                                let key_input = key_input.clone();
                                Callback::from(move |event: InputEvent| {
                                    let value = event.target_unchecked_into::<HtmlInputElement>().value();
                                    key_input.set(value);
                                })
                            }}
                            onkeydown={on_keydown}
                            autocomplete="off"
                            autocapitalize="off"
                            spellcheck="false"
                        />
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={{
                                let show_key = show_key.clone();
                                Callback::from(move |_| show_key.set(!*show_key))
                            }}
                            title={if *show_key { "隐藏" } else { "显示" }}
                            aria-label={if *show_key { "隐藏" } else { "显示" }}
                        >
                            <i class={classes!("fas", if *show_key { "fa-eye-slash" } else { "fa-eye" })}></i>
                        </button>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2")}>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "btn-terminal-primary")}
                            onclick={on_submit_click}
                            disabled={*loading}
                        >
                            <i class={classes!("fas", if *loading { "fa-spinner animate-spin" } else { "fa-magnifying-glass" })}></i>
                            { "查询" }
                        </button>
                        <button
                            type="button"
                            class={classes!("btn-terminal")}
                            onclick={on_refresh}
                            disabled={*loading || (*active_key).is_none()}
                        >
                            <i class={classes!("fas", "fa-rotate-right")}></i>
                        </button>
                    </div>
                </div>

                if let Some(error_message) = (*error).clone() {
                    <div class={classes!("mt-4", "rounded-lg", "border", "border-red-400/35", "bg-red-500/8", "px-4", "py-3", "text-sm", "text-red-700", "dark:text-red-200")}>
                        { error_message }
                    </div>
                }
            </section>

            if let Some(response) = (*lookup).clone() {
                <section class={classes!("grid", "gap-4", "lg:grid-cols-4")}>
                    <article class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "Key" }</div>
                        <div class={classes!("mt-2", "text-lg", "font-bold", "text-[var(--text)]")}>{ response.key.name.clone() }</div>
                        <div class={classes!("mt-2", "inline-flex", "rounded-full", "bg-[var(--surface-alt)]", "px-2.5", "py-1", "font-mono", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-[var(--muted)]")}>
                            { provider_badge_label(&response.key.provider_type) }
                        </div>
                    </article>
                    <article class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "总额度" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>{ format_number_u64(response.key.quota_billable_limit) }</div>
                    </article>
                    <article class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "已用 Billable" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>{ format_number_u64(response.key.usage_billable_tokens) }</div>
                    </article>
                    <article class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
                        <div class={classes!("font-mono", "text-[11px]", "uppercase", "tracking-widest", "text-[var(--muted)]")}>{ "剩余" }</div>
                        <div class={classes!("mt-2", "font-mono", "text-2xl", "font-black", "text-[var(--text)]")}>{ format_number_i64(response.key.remaining_billable) }</div>
                    </article>
                </section>

                <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>
                                { "最近 24h Token 用量" }
                            </h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { "统计口径：uncached input + output，不计 cached input。" }
                            </p>
                        </div>
                        if let Some(last_used_at) = response.key.last_used_at {
                            <span class={classes!("font-mono", "text-xs", "text-[var(--muted)]")}>
                                { format!("last used {}", format_ms(last_used_at)) }
                            </span>
                        }
                    </div>
                    <TokenUsageTrendChart
                        points={response.chart_points.clone()}
                        empty_text={"最近 24 小时还没有 token 使用记录".to_string()}
                        class={classes!("mt-4")}
                    />
                </section>

                <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                    <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                        <div>
                            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>
                                { "Usage 日志" }
                            </h2>
                            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                                { format!("共 {} 条，当前第 {} 页", response.total, *current_page) }
                            </p>
                        </div>
                        <div class={classes!("grid", "gap-1", "font-mono", "text-xs", "text-[var(--muted)]")}>
                            <span>{ format!("Uncached {}", format_number_u64(response.key.usage_input_uncached_tokens)) }</span>
                            <span>{ format!("Cached {}", format_number_u64(response.key.usage_input_cached_tokens)) }</span>
                            <span>{ format!("Out {}", format_number_u64(response.key.usage_output_tokens)) }</span>
                        </div>
                    </div>

                    if response.events.is_empty() {
                        <div class={classes!("mt-4", "rounded-lg", "border", "border-dashed", "border-[var(--border)]", "px-5", "py-10", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                            { "当前 key 还没有 usage 日志" }
                        </div>
                    } else {
                        <div class={classes!("mt-4", "overflow-x-auto")}>
                            <table class={classes!("min-w-full", "border-collapse", "text-left", "text-sm")}>
                                <thead class={classes!("border-b", "border-[var(--border)]", "font-mono", "text-[11px]", "uppercase", "tracking-[0.12em]", "text-[var(--muted)]")}>
                                    <tr>
                                        <th class={classes!("py-2", "pr-3")}>{ "时间" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "账号" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "请求" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "模型 / 状态" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "延迟" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "IP / 地区" }</th>
                                        <th class={classes!("py-2", "pr-3")}>{ "Tokens" }</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    { for response.events.iter().map(|event| {
                                        html! {
                                            <tr key={event.id.clone()} class={classes!("border-b", "border-[var(--border)]", "align-top")}>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap", "font-mono", "text-xs")}>
                                                    { format_ms(event.created_at) }
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[10rem]")}>
                                                    <span class={classes!("inline-flex", "rounded-full", "border", "border-emerald-500/20", "bg-emerald-500/10", "px-2.5", "py-1", "text-xs", "font-semibold", "text-emerald-700", "dark:text-emerald-200")}>
                                                        { event.account_name.clone().unwrap_or_else(|| "legacy auth".to_string()) }
                                                    </span>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[22rem]")}>
                                                    <div class={classes!("flex", "items-start", "gap-2")}>
                                                        <span class={classes!("inline-flex", "rounded-full", "border", "border-sky-500/20", "bg-sky-500/10", "px-2", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-sky-700", "dark:text-sky-200")}>
                                                            { event.request_method.clone() }
                                                        </span>
                                                        <div class={classes!("min-w-0", "flex-1")}>
                                                            <div class={classes!("truncate")} title={event.request_url.clone()}>{ event.request_url.clone() }</div>
                                                            <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                                                { format!("upstream {}", event.endpoint) }
                                                            </div>
                                                        </div>
                                                    </div>
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[11rem]")}>
                                                    <div>{ event.model.clone().unwrap_or_else(|| "-".to_string()) }</div>
                                                    <div class={classes!("mt-1", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                                        { format!("status {}", event.status_code) }
                                                    </div>
                                                    if event.usage_missing {
                                                        <div class={classes!("mt-2", "inline-flex", "rounded-full", "border", "border-amber-500/20", "bg-amber-500/10", "px-2", "py-1", "text-[11px]", "font-semibold", "uppercase", "tracking-[0.12em]", "text-amber-700", "dark:text-amber-200")}>
                                                            { "usage missing" }
                                                        </div>
                                                    }
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap", "font-mono", "text-xs")}>
                                                    { format!("{} ms", event.latency_ms) }
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "whitespace-nowrap", "font-mono", "text-xs")}>
                                                    { format!("{}/{}", event.client_ip, event.ip_region) }
                                                </td>
                                                <td class={classes!("py-3", "pr-3", "min-w-[12rem]")}>
                                                    <div class={classes!("grid", "gap-1", "font-mono", "text-xs", "text-[var(--muted)]")}>
                                                        <span>{ format!("Uncached {}", format_number_u64(event.input_uncached_tokens)) }</span>
                                                        <span>{ format!("Cached {}", format_number_u64(event.input_cached_tokens)) }</span>
                                                        <span>{ format!("Out {}", format_number_u64(event.output_tokens)) }</span>
                                                        <span class={classes!("font-semibold", "text-[var(--text)]")}>{ format!("Billable {}", format_number_u64(event.billable_tokens)) }</span>
                                                    </div>
                                                </td>
                                            </tr>
                                        }
                                    }) }
                                </tbody>
                            </table>
                        </div>
                    }

                    <div class={classes!("mt-5")}>
                        <Pagination current_page={*current_page} total_pages={total_pages} on_page_change={on_page_change} />
                    </div>
                </section>
            } else if *loading {
                <section class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-5", "py-12", "text-center", "font-mono", "text-sm", "text-[var(--muted)]")}>
                    { "> loading usage..." }
                </section>
            }
        </div>
    }
}
