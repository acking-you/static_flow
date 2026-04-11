use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        fetch_admin_kiro_account_statuses, fetch_admin_llm_gateway_proxy_configs,
        AdminKiroAccountStatusesQuery, AdminKiroAccountStatusesResponse,
        AdminUpstreamProxyConfigView,
    },
    components::pagination::Pagination,
    pages::admin_kiro_gateway::KiroAccountCard,
    router::Route,
};

const DEFAULT_STATUS_PAGE_SIZE: usize = 24;
const STATUS_PAGE_SIZE_OPTIONS: [usize; 3] = [12, 24, 48];

fn normalized_admin_kiro_status_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn admin_kiro_status_total_pages(total: usize, page_size: usize) -> usize {
    total.max(1).div_ceil(page_size.max(1))
}

#[function_component(AdminKiroAccountStatusPage)]
pub fn admin_kiro_account_status_page() -> Html {
    let search_input = use_state(String::new);
    let active_prefix = use_state(|| None::<String>);
    let current_page = use_state(|| 1usize);
    let page_size = use_state(|| DEFAULT_STATUS_PAGE_SIZE);
    let response = use_state(|| None::<AdminKiroAccountStatusesResponse>);
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let flash = use_state(|| None::<String>);
    let refresh_tick = use_state(|| 0u64);

    let notify = {
        let flash = flash.clone();
        let error = error.clone();
        Callback::from(move |(message, is_error): (String, bool)| {
            if is_error {
                error.set(Some(message));
                flash.set(None);
            } else {
                flash.set(Some(message));
                error.set(None);
            }
        })
    };

    {
        let active_prefix = active_prefix.clone();
        let current_page = current_page.clone();
        let page_size = page_size.clone();
        let response = response.clone();
        let proxy_configs = proxy_configs.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with(
            ((*active_prefix).clone(), *current_page, *page_size, *refresh_tick),
            move |_| {
                let active_prefix_value = (*active_prefix).clone();
                let current_page_value = *current_page;
                let page_size_value = *page_size;
                let response = response.clone();
                let proxy_configs = proxy_configs.clone();
                let loading = loading.clone();
                let error = error.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    loading.set(true);
                    let query = AdminKiroAccountStatusesQuery {
                        prefix: active_prefix_value,
                        limit: Some(page_size_value),
                        offset: Some(
                            current_page_value
                                .saturating_sub(1)
                                .saturating_mul(page_size_value),
                        ),
                    };
                    let (statuses_result, proxy_configs_result) = futures::join!(
                        fetch_admin_kiro_account_statuses(&query),
                        fetch_admin_llm_gateway_proxy_configs(),
                    );
                    match (statuses_result, proxy_configs_result) {
                        (Ok(statuses), Ok(proxy_config_response)) => {
                            response.set(Some(statuses));
                            proxy_configs.set(proxy_config_response.proxy_configs);
                            error.set(None);
                        },
                        (Err(err), _) | (_, Err(err)) => {
                            error.set(Some(err));
                        },
                    }
                    loading.set(false);
                });
                || ()
            },
        );
    }

    let on_search = {
        let search_input = search_input.clone();
        let active_prefix = active_prefix.clone();
        let current_page = current_page.clone();
        Callback::from(move |_| {
            active_prefix.set(normalized_admin_kiro_status_prefix(&search_input));
            current_page.set(1);
        })
    };

    let on_clear = {
        let search_input = search_input.clone();
        let active_prefix = active_prefix.clone();
        let current_page = current_page.clone();
        Callback::from(move |_| {
            search_input.set(String::new());
            active_prefix.set(None);
            current_page.set(1);
        })
    };

    let on_refresh = {
        let refresh_tick = refresh_tick.clone();
        Callback::from(move |_| refresh_tick.set((*refresh_tick).wrapping_add(1)))
    };
    let on_refresh_click = {
        let on_refresh = on_refresh.clone();
        Callback::from(move |_| on_refresh.emit(()))
    };

    let on_page_change = {
        let current_page = current_page.clone();
        Callback::from(move |page: usize| current_page.set(page))
    };

    let on_page_size_change = {
        let page_size = page_size.clone();
        let current_page = current_page.clone();
        Callback::from(move |event: Event| {
            let input: HtmlSelectElement = event.target_unchecked_into();
            let parsed = input
                .value()
                .parse::<usize>()
                .ok()
                .filter(|value| STATUS_PAGE_SIZE_OPTIONS.contains(value))
                .unwrap_or(DEFAULT_STATUS_PAGE_SIZE);
            page_size.set(parsed);
            current_page.set(1);
        })
    };

    let on_card_reload = {
        let on_refresh = on_refresh.clone();
        Callback::from(move |_| on_refresh.emit(()))
    };

    let total = response.as_ref().as_ref().map_or(0, |value| value.total);
    let effective_limit = response
        .as_ref()
        .as_ref()
        .map_or(*page_size, |value| value.limit.max(1));
    let total_pages = admin_kiro_status_total_pages(total, effective_limit);
    let active_prefix_label = (*active_prefix)
        .clone()
        .unwrap_or_else(|| "all".to_string());

    html! {
        <main class={classes!("container", "py-8", "space-y-5")}>
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "font-mono", "text-xl", "font-bold", "text-[var(--text)]")}>
                            { "Kiro Account Status" }
                        </h1>
                        <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                            { "卡片样式保持不变，这里只负责分页浏览、按前缀检索和刷新状态。" }
                        </p>
                    </div>
                    <div class={classes!("flex", "items-center", "gap-2", "flex-wrap")}>
                        <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal")}>
                            { "Gateway Admin" }
                        </Link<Route>>
                        <button
                            type="button"
                            class={classes!("btn-terminal", "btn-terminal-primary")}
                            onclick={on_refresh_click}
                        >
                            { if *loading { "Refreshing..." } else { "Refresh" } }
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

                <div class={classes!("mt-4", "grid", "gap-3", "lg:grid-cols-[minmax(0,1fr)_auto_auto_auto]")}>
                    <input
                        class={classes!("w-full", "rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm", "font-mono")}
                        placeholder="prefix search"
                        value={(*search_input).clone()}
                        oninput={{
                            let search_input = search_input.clone();
                            Callback::from(move |event: InputEvent| {
                                let input: HtmlInputElement = event.target_unchecked_into();
                                search_input.set(input.value());
                            })
                        }}
                    />
                    <select
                        class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface-alt)]", "px-3", "py-2", "text-sm")}
                        value={(*page_size).to_string()}
                        onchange={on_page_size_change}
                    >
                        { for STATUS_PAGE_SIZE_OPTIONS.iter().map(|value| html! {
                            <option value={value.to_string()}>{ value }</option>
                        }) }
                    </select>
                    <button type="button" class={classes!("btn-terminal")} onclick={on_search}>
                        { "Search" }
                    </button>
                    <button type="button" class={classes!("btn-terminal")} onclick={on_clear}>
                        { "Clear" }
                    </button>
                </div>

                <div class={classes!("mt-3", "font-mono", "text-xs", "text-[var(--muted)]")}>
                    { format!("prefix {} · total {} · page {}/{}", active_prefix_label, total, *current_page, total_pages) }
                </div>
            </section>

            if *loading && response.is_none() {
                <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "font-mono", "text-sm", "text-[var(--muted)]")}>
                    { "Loading Kiro account statuses..." }
                </section>
            } else if response
                .as_ref()
                .as_ref()
                .is_some_and(|value| value.accounts.is_empty())
            {
                <section class={classes!("rounded-xl", "border", "border-dashed", "border-[var(--border)]", "bg-[var(--surface)]", "p-5", "text-sm", "text-[var(--muted)]")}>
                    { "当前筛选条件下没有匹配到任何 Kiro 账号。" }
                </section>
            } else if let Some(status_response) = response.as_ref().as_ref() {
                <>
                    <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                        { for status_response.accounts.iter().map(|account| html! {
                            <KiroAccountCard
                                key={account.name.clone()}
                                account={account.clone()}
                                proxy_configs={(*proxy_configs).clone()}
                                on_reload={on_card_reload.clone()}
                                flash={flash.clone()}
                                notify={notify.clone()}
                                error={error.clone()}
                            />
                        }) }
                    </section>
                    <section class={classes!("flex", "justify-center")}>
                        <Pagination
                            current_page={*current_page}
                            total_pages={total_pages}
                            on_page_change={on_page_change}
                        />
                    </section>
                </>
            }
        </main>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_admin_kiro_status_prefix_trims_empty_input() {
        assert_eq!(normalized_admin_kiro_status_prefix("   "), None);
        assert_eq!(normalized_admin_kiro_status_prefix("  alpha "), Some("alpha".to_string()));
    }

    #[test]
    fn admin_kiro_status_total_pages_never_drops_below_one() {
        assert_eq!(admin_kiro_status_total_pages(0, 24), 1);
        assert_eq!(admin_kiro_status_total_pages(25, 24), 2);
    }
}
