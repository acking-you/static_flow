use gloo_timers::callback::Interval;
use web_sys::HtmlSelectElement;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_admin_llm_gateway_usage_metrics, AdminLlmGatewayUsageMetricsDimensionView,
        AdminLlmGatewayUsageMetricsQuery, AdminLlmGatewayUsageMetricsResponse,
        AdminLlmGatewayUsageMetricsStatusCodeView,
    },
    components::loading_spinner::{LoadingSpinner, SpinnerSize},
    pages::llm_access_shared::format_ms,
    router::Route,
};

const WINDOW_15M: &str = "15m";
const WINDOW_1H: &str = "1h";
const WINDOW_6H: &str = "6h";
const WINDOW_24H: &str = "24h";
const SOURCE_ALL: &str = "all";
const SOURCE_HOT: &str = "hot";
const SOURCE_ARCHIVE: &str = "archive";

fn format_metric_ms(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0} ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_metric_i64_ms(value: Option<i64>) -> String {
    value
        .map(|value| format!("{value} ms"))
        .unwrap_or_else(|| "-".to_string())
}

fn format_ratio(numerator: u64, denominator: u64) -> String {
    if denominator == 0 {
        "-".to_string()
    } else {
        format!("{:.1}%", numerator as f64 * 100.0 / denominator as f64)
    }
}

fn provider_query_value(selection: &str) -> Option<String> {
    let value = selection.trim();
    if value.is_empty() || value == "all" {
        None
    } else {
        Some(value.to_string())
    }
}

fn source_label(value: &str) -> String {
    match value {
        "All" | SOURCE_ALL => "all".to_string(),
        "Hot" | SOURCE_HOT => "hot".to_string(),
        "Archive" | SOURCE_ARCHIVE => "archive".to_string(),
        other => other.to_ascii_lowercase(),
    }
}

#[derive(Properties, PartialEq)]
struct SummaryCardProps {
    label: AttrValue,
    value: AttrValue,
    detail: AttrValue,
}

#[function_component(SummaryCard)]
fn summary_card(props: &SummaryCardProps) -> Html {
    html! {
        <div class={classes!(
            "rounded-[var(--radius)]",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "px-4",
            "py-3",
            "shadow-[var(--shadow)]"
        )}>
            <div class={classes!("text-xs", "uppercase", "tracking-[0.08em]", "text-[var(--muted)]")}>
                { props.label.clone() }
            </div>
            <div class={classes!("mt-2", "text-2xl", "font-semibold", "text-[var(--foreground)]")}>
                { props.value.clone() }
            </div>
            <div class={classes!("mt-1", "text-sm", "text-[var(--muted)]")}>
                { props.detail.clone() }
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct MetricsTableProps {
    title: AttrValue,
    caption: AttrValue,
    rows: Vec<AdminLlmGatewayUsageMetricsDimensionView>,
}

#[function_component(MetricsTable)]
fn metrics_table(props: &MetricsTableProps) -> Html {
    html! {
        <section class={classes!(
            "rounded-[var(--radius)]",
            "border",
            "border-[var(--border)]",
            "bg-[var(--surface)]",
            "shadow-[var(--shadow)]",
            "overflow-hidden"
        )}>
            <div class={classes!("border-b", "border-[var(--border)]", "px-4", "py-3")}>
                <div class={classes!("text-sm", "font-semibold", "text-[var(--foreground)]")}>
                    { props.title.clone() }
                </div>
                <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]")}>
                    { props.caption.clone() }
                </div>
            </div>
            <div class={classes!("overflow-x-auto")}>
                <table class={classes!("min-w-full", "text-sm")}>
                    <thead class={classes!("bg-[var(--surface-alt)]", "text-[var(--muted)]")}>
                        <tr>
                            <th class={classes!("px-4", "py-2", "text-left", "font-medium")}>{ "对象" }</th>
                            <th class={classes!("px-3", "py-2", "text-right", "font-medium")}>{ "请求" }</th>
                            <th class={classes!("px-3", "py-2", "text-right", "font-medium")}>{ "非 200" }</th>
                            <th class={classes!("px-3", "py-2", "text-right", "font-medium")}>{ "首字均值" }</th>
                            <th class={classes!("px-3", "py-2", "text-right", "font-medium")}>{ "路由等待" }</th>
                            <th class={classes!("px-3", "py-2", "text-right", "font-medium")}>{ "Failover" }</th>
                            <th class={classes!("px-4", "py-2", "text-right", "font-medium")}>{ "断流" }</th>
                        </tr>
                    </thead>
                    <tbody>
                        if props.rows.is_empty() {
                            <tr>
                                <td colspan="7" class={classes!("px-4", "py-6", "text-center", "text-[var(--muted)]")}>
                                    { "当前窗口内没有数据。" }
                                </td>
                            </tr>
                        } else {
                            { for props.rows.iter().map(|row| {
                                let secondary = row
                                    .proxy_source
                                    .as_deref()
                                    .or(row.proxy_url.as_deref())
                                    .or(row.proxy_config_id.as_deref())
                                    .unwrap_or("");
                                html! {
                                    <tr class={classes!("border-t", "border-[var(--border)]", "align-top")}>
                                        <td class={classes!("px-4", "py-3")}>
                                            <div class={classes!("font-medium", "text-[var(--foreground)]")}>{ row.label.clone() }</div>
                                            if !secondary.is_empty() {
                                                <div class={classes!("mt-1", "text-xs", "text-[var(--muted)]", "break-all")}>
                                                    { secondary }
                                                </div>
                                            }
                                        </td>
                                        <td class={classes!("px-3", "py-3", "text-right")}>{ row.request_count }</td>
                                        <td class={classes!("px-3", "py-3", "text-right")}>
                                            <div>{ row.non_ok_count }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format_ratio(row.non_ok_count, row.request_count) }
                                            </div>
                                        </td>
                                        <td class={classes!("px-3", "py-3", "text-right")}>
                                            <div>{ format_metric_ms(row.avg_first_token_ms) }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("max {}", format_metric_i64_ms(row.max_first_token_ms)) }
                                            </div>
                                        </td>
                                        <td class={classes!("px-3", "py-3", "text-right")}>
                                            <div>{ format_metric_ms(row.avg_routing_wait_ms) }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("max {}", format_metric_i64_ms(row.max_routing_wait_ms)) }
                                            </div>
                                        </td>
                                        <td class={classes!("px-3", "py-3", "text-right")}>
                                            <div>{ row.failover_request_count }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format!("sum {}", row.total_quota_failovers) }
                                            </div>
                                        </td>
                                        <td class={classes!("px-4", "py-3", "text-right")}>
                                            <div>{ row.downstream_disconnect_count }</div>
                                            <div class={classes!("text-xs", "text-[var(--muted)]")}>
                                                { format_ratio(row.downstream_disconnect_count, row.request_count) }
                                            </div>
                                        </td>
                                    </tr>
                                }
                            }) }
                        }
                    </tbody>
                </table>
            </div>
        </section>
    }
}

fn render_status_code_distribution(rows: &[AdminLlmGatewayUsageMetricsStatusCodeView]) -> Html {
    if rows.is_empty() {
        return html! {
            <div class={classes!("text-sm", "text-[var(--muted)]")}>{ "当前窗口没有非 200 状态码。" }</div>
        };
    }
    html! {
        <div class={classes!("flex", "flex-wrap", "gap-2")}>
            { for rows.iter().map(|row| html! {
                <div class={classes!(
                    "rounded-full",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface-alt)]",
                    "px-3",
                    "py-1.5",
                    "text-sm"
                )}>
                    <span class={classes!("font-semibold", "text-[var(--foreground)]")}>{ row.status_code }</span>
                    <span class={classes!("ml-2", "text-[var(--muted)]")}>{ row.request_count }</span>
                </div>
            }) }
        </div>
    }
}

#[function_component(AdminLlmGatewayMonitorPage)]
pub fn admin_llm_gateway_monitor_page() -> Html {
    let window = use_state(|| WINDOW_1H.to_string());
    let source = use_state(|| SOURCE_ALL.to_string());
    let provider = use_state(|| "all".to_string());
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let snapshot = use_state(AdminLlmGatewayUsageMetricsResponse::default);
    let refresh_tick = use_state(|| 0_u64);

    {
        let refresh_tick = refresh_tick.clone();
        use_effect_with((), move |_| {
            let interval = Interval::new(15_000, move || {
                refresh_tick.set((*refresh_tick).saturating_add(1));
            });
            move || drop(interval)
        });
    }

    {
        let window_value = (*window).clone();
        let source_value = (*source).clone();
        let provider_value = (*provider).clone();
        let refresh_value = *refresh_tick;
        let loading = loading.clone();
        let error = error.clone();
        let snapshot = snapshot.clone();
        use_effect_with(
            (window_value, source_value, provider_value, refresh_value),
            move |(window_value, source_value, provider_value, _)| {
                loading.set(true);
                error.set(None);
                let loading = loading.clone();
                let error = error.clone();
                let snapshot = snapshot.clone();
                let provider_query = provider_query_value(provider_value);
                let query = AdminLlmGatewayUsageMetricsQuery {
                    provider_type: provider_query,
                    source: Some(source_value.clone()),
                    window: Some(window_value.clone()),
                    top_limit: Some(10),
                };
                wasm_bindgen_futures::spawn_local(async move {
                    match fetch_admin_llm_gateway_usage_metrics(&query).await {
                        Ok(response) => {
                            snapshot.set(response);
                            loading.set(false);
                        },
                        Err(err_msg) => {
                            error.set(Some(err_msg));
                            loading.set(false);
                        },
                    }
                });
                || ()
            },
        );
    }

    let on_window_change = {
        let window = window.clone();
        Callback::from(move |event: Event| {
            let input: HtmlSelectElement = event.target_unchecked_into();
            window.set(input.value());
        })
    };
    let on_source_change = {
        let source = source.clone();
        Callback::from(move |event: Event| {
            let input: HtmlSelectElement = event.target_unchecked_into();
            source.set(input.value());
        })
    };
    let on_provider_change = {
        let provider = provider.clone();
        Callback::from(move |event: Event| {
            let input: HtmlSelectElement = event.target_unchecked_into();
            provider.set(input.value());
        })
    };
    let on_refresh_click = {
        let refresh_tick = refresh_tick.clone();
        Callback::from(move |_| refresh_tick.set((*refresh_tick).saturating_add(1)))
    };

    let snapshot_value = (*snapshot).clone();
    let provider_badge = snapshot_value
        .provider_type
        .clone()
        .unwrap_or_else(|| "all providers".to_string());
    let source_badge = source_label(&snapshot_value.source);
    let error_rate =
        format_ratio(snapshot_value.summary.non_ok_requests, snapshot_value.summary.total_requests);

    html! {
        <main class={classes!("container", "py-8")}>
            <section class={classes!(
                "rounded-[var(--radius)]",
                "border",
                "border-[var(--border)]",
                "bg-[var(--surface)]",
                "shadow-[var(--shadow)]",
                "p-5"
            )}>
                <div class={classes!("flex", "items-start", "justify-between", "gap-4", "flex-wrap")}>
                    <div>
                        <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                            <Link<Route> to={Route::AdminLlmGateway} classes={classes!("btn-fluent-secondary")}>
                                { "返回 LLM Gateway" }
                            </Link<Route>>
                            <Link<Route> to={Route::Admin} classes={classes!("btn-fluent-secondary")}>
                                { "返回 Admin" }
                            </Link<Route>>
                        </div>
                        <h1 class={classes!("mt-4", "mb-1", "text-2xl", "font-semibold")}>
                            { "LLM Gateway 运行监控" }
                        </h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]", "max-w-3xl")}>
                            { "聚焦最近窗口的首字延迟、异常请求、路由等待、配额 failover 与断流分布。代理归因来自 worker 消费期写入的事件快照，不走前端近似推断。" }
                        </p>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <label class={classes!("text-sm", "text-[var(--muted)]")}>
                            { "窗口" }
                            <select class={classes!("ml-2", "rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")} value={(*window).clone()} onchange={on_window_change}>
                                <option value={WINDOW_15M}>{ "15m" }</option>
                                <option value={WINDOW_1H}>{ "1h" }</option>
                                <option value={WINDOW_6H}>{ "6h" }</option>
                                <option value={WINDOW_24H}>{ "24h" }</option>
                            </select>
                        </label>
                        <label class={classes!("text-sm", "text-[var(--muted)]")}>
                            { "数据源" }
                            <select class={classes!("ml-2", "rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")} value={(*source).clone()} onchange={on_source_change}>
                                <option value={SOURCE_ALL}>{ "all" }</option>
                                <option value={SOURCE_HOT}>{ "hot" }</option>
                                <option value={SOURCE_ARCHIVE}>{ "archive" }</option>
                            </select>
                        </label>
                        <label class={classes!("text-sm", "text-[var(--muted)]")}>
                            { "Provider" }
                            <select class={classes!("ml-2", "rounded-md", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2")} value={(*provider).clone()} onchange={on_provider_change}>
                                <option value="all">{ "all" }</option>
                                <option value="codex">{ "codex" }</option>
                                <option value="kiro">{ "kiro" }</option>
                            </select>
                        </label>
                        <button class={classes!("btn-fluent-primary")} onclick={on_refresh_click}>
                            { if *loading { "刷新中..." } else { "立即刷新" } }
                        </button>
                    </div>
                </div>

                <div class={classes!("mt-4", "flex", "flex-wrap", "gap-2", "text-xs", "text-[var(--muted)]")}>
                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-1")}>
                        { format!("provider: {provider_badge}") }
                    </span>
                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-1")}>
                        { format!("source: {source_badge}") }
                    </span>
                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-1")}>
                        { format!("window: {} → {}", format_ms(snapshot_value.start_ms), format_ms(snapshot_value.end_ms)) }
                    </span>
                    <span class={classes!("rounded-full", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-1")}>
                        { format!("generated: {}", format_ms(snapshot_value.generated_at_ms)) }
                    </span>
                </div>

                if let Some(error_text) = (*error).clone() {
                    <div class={classes!(
                        "mt-4",
                        "rounded-[var(--radius)]",
                        "border",
                        "border-red-400/40",
                        "bg-red-500/10",
                        "px-4",
                        "py-3",
                        "text-sm",
                        "text-red-700",
                        "dark:text-red-200"
                    )}>
                        { error_text }
                    </div>
                }
            </section>

            if *loading && snapshot_value.generated_at_ms == 0 {
                <LoadingSpinner size={SpinnerSize::Large} />
            } else {
                <section class={classes!("mt-5", "grid", "gap-4", "md:grid-cols-2", "xl:grid-cols-4")}>
                    <SummaryCard
                        label="请求总量"
                        value={snapshot_value.summary.total_requests.to_string()}
                        detail={format!("非 200 {} / {}", snapshot_value.summary.non_ok_requests, error_rate)}
                    />
                    <SummaryCard
                        label="首字延迟"
                        value={format_metric_ms(snapshot_value.summary.avg_first_token_ms)}
                        detail={format!("max {}", format_metric_i64_ms(snapshot_value.summary.max_first_token_ms))}
                    />
                    <SummaryCard
                        label="整体延迟"
                        value={format_metric_ms(snapshot_value.summary.avg_latency_ms)}
                        detail={format!("routing {}", format_metric_ms(snapshot_value.summary.avg_routing_wait_ms))}
                    />
                    <SummaryCard
                        label="Failover"
                        value={snapshot_value.summary.failover_request_count.to_string()}
                        detail={format!("sum {}", snapshot_value.summary.total_quota_failovers)}
                    />
                    <SummaryCard
                        label="断流请求"
                        value={snapshot_value.summary.downstream_disconnect_count.to_string()}
                        detail={format_ratio(snapshot_value.summary.downstream_disconnect_count, snapshot_value.summary.total_requests)}
                    />
                    <SummaryCard
                        label="缺失 usage"
                        value={snapshot_value.summary.usage_missing_count.to_string()}
                        detail={format_ratio(snapshot_value.summary.usage_missing_count, snapshot_value.summary.total_requests)}
                    />
                    <SummaryCard
                        label="缺失 credit"
                        value={snapshot_value.summary.credit_usage_missing_count.to_string()}
                        detail={format_ratio(snapshot_value.summary.credit_usage_missing_count, snapshot_value.summary.total_requests)}
                    />
                    <SummaryCard
                        label="参与对象"
                        value={format!("{} / {}", snapshot_value.summary.distinct_accounts, snapshot_value.summary.distinct_proxies)}
                        detail="accounts / proxies"
                    />
                </section>

                <section class={classes!(
                    "mt-5",
                    "rounded-[var(--radius)]",
                    "border",
                    "border-[var(--border)]",
                    "bg-[var(--surface)]",
                    "shadow-[var(--shadow)]",
                    "p-4"
                )}>
                    <div class={classes!("text-sm", "font-semibold", "text-[var(--foreground)]")}>{ "非 200 状态码分布" }</div>
                    <div class={classes!("mt-3")}>
                        { render_status_code_distribution(&snapshot_value.non_ok_status_codes) }
                    </div>
                </section>

                <section class={classes!("mt-5", "grid", "gap-4", "xl:grid-cols-2")}>
                    <MetricsTable title="首字延迟 Top 账号" caption="按首字均值降序。" rows={snapshot_value.top_first_token_accounts.clone()} />
                    <MetricsTable title="首字延迟 Top 代理" caption="按首字均值降序。" rows={snapshot_value.top_first_token_proxies.clone()} />
                    <MetricsTable title="异常请求 Top 账号" caption="按非 200 请求量降序。" rows={snapshot_value.top_non_ok_accounts.clone()} />
                    <MetricsTable title="异常请求 Top 代理" caption="按非 200 请求量降序。" rows={snapshot_value.top_non_ok_proxies.clone()} />
                    <MetricsTable title="路由等待 Top 账号" caption="按路由等待均值降序。" rows={snapshot_value.top_routing_wait_accounts.clone()} />
                    <MetricsTable title="路由等待 Top 代理" caption="按路由等待均值降序。" rows={snapshot_value.top_routing_wait_proxies.clone()} />
                    <MetricsTable title="Failover Top 账号" caption="按触发 failover 的请求数降序。" rows={snapshot_value.top_failover_accounts.clone()} />
                    <MetricsTable title="Failover Top 代理" caption="按触发 failover 的请求数降序。" rows={snapshot_value.top_failover_proxies.clone()} />
                    <MetricsTable title="断流 Top 账号" caption="按 downstream disconnect 请求数降序。" rows={snapshot_value.top_disconnect_accounts.clone()} />
                    <MetricsTable title="断流 Top 代理" caption="按 downstream disconnect 请求数降序。" rows={snapshot_value.top_disconnect_proxies.clone()} />
                </section>
            }
        </main>
    }
}
