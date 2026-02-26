use std::collections::BTreeMap;

use gloo_timers::callback::Timeout;
use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::{window, Event, HtmlInputElement, HtmlTextAreaElement};
use yew::prelude::*;
use yew_router::prelude::{use_location, Link};

use crate::{
    api::{self, ArticleRequestItem},
    components::{
        article_card::ArticleCard,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        pagination::Pagination,
        raw_html::RawHtml,
        scroll_to_top_button::ScrollToTopButton,
    },
    i18n::current::latest_articles_page as t,
    i18n::current::article_request as ar_t,
    router::Route,
    utils::markdown_to_html,
};

const PAGE_SIZE: usize = 12;
const REQUEST_PAGE_SIZE: usize = 12;

#[derive(Properties, PartialEq)]
struct RequestCardProps {
    pub req: ArticleRequestItem,
    pub on_follow_up: Callback<ArticleRequestItem>,
    #[prop_or_default]
    pub parent_summary: Option<AttrValue>,
}

#[function_component(RequestCard)]
fn request_card(props: &RequestCardProps) -> Html {
    let req = &props.req;
    let show_modal = use_state(|| false);

    let (status_class, status_dot) = match req.status.as_str() {
        "done" => (
            "bg-emerald-50 text-emerald-700 ring-emerald-600/20 dark:bg-emerald-500/10 dark:text-emerald-300 dark:ring-emerald-500/25",
            "bg-emerald-500",
        ),
        "running" => (
            "bg-sky-50 text-sky-700 ring-sky-600/20 dark:bg-sky-500/10 dark:text-sky-300 dark:ring-sky-500/25",
            "bg-sky-500 animate-pulse",
        ),
        "failed" => (
            "bg-red-50 text-red-700 ring-red-600/20 dark:bg-red-500/10 dark:text-red-300 dark:ring-red-500/25",
            "bg-red-500",
        ),
        "rejected" => (
            "bg-gray-50 text-gray-600 ring-gray-500/20 dark:bg-gray-500/10 dark:text-gray-400 dark:ring-gray-500/25",
            "bg-gray-400",
        ),
        _ => (
            "bg-amber-50 text-amber-700 ring-amber-600/20 dark:bg-amber-500/10 dark:text-amber-300 dark:ring-amber-500/25",
            "bg-amber-500",
        ),
    };
    let status_text = match req.status.as_str() {
        "pending" => ar_t::STATUS_PENDING,
        "approved" => ar_t::STATUS_APPROVED,
        "running" => ar_t::STATUS_RUNNING,
        "done" => ar_t::STATUS_DONE,
        "failed" => ar_t::STATUS_FAILED,
        _ => &req.status,
    };

    let open_modal = {
        let show_modal = show_modal.clone();
        Callback::from(move |_: MouseEvent| show_modal.set(true))
    };
    let close_modal = {
        let show_modal = show_modal.clone();
        Callback::from(move |_: MouseEvent| show_modal.set(false))
    };
    let stop_bubble = Callback::from(|e: MouseEvent| e.stop_propagation());

    let on_follow_up_click = {
        let cb = props.on_follow_up.clone();
        let req = req.clone();
        Callback::from(move |_: MouseEvent| cb.emit(req.clone()))
    };

    let has_ai_reply = req.status == "done" && req.ai_reply.is_some();

    // Render the detail modal
    let modal_html = if *show_modal {
        let ai_rendered = req.ai_reply.as_deref().map(|r| markdown_to_html(r));
        html! {
            <div class="fixed inset-0 z-[200] flex items-center justify-center bg-black/50 backdrop-blur-sm p-4"
                 onclick={close_modal.clone()}>
                <div class="relative w-full max-w-2xl max-h-[85vh] flex flex-col \
                            bg-white dark:bg-[var(--surface)] \
                            rounded-2xl shadow-2xl border border-gray-200 dark:border-[var(--border)] \
                            overflow-hidden"
                     onclick={stop_bubble.clone()}>
                    // Header
                    <div class="flex items-center justify-between px-5 py-3.5 \
                                border-b border-gray-100 dark:border-[var(--border)] shrink-0">
                        <h3 class="text-base font-semibold text-gray-900 dark:text-[var(--text)]">
                            {ar_t::DETAIL_MODAL_TITLE}
                        </h3>
                        <button onclick={close_modal.clone()}
                            class="rounded-lg p-1.5 text-gray-400 hover:text-gray-600 \
                                   dark:text-[var(--muted)] dark:hover:text-[var(--text)] \
                                   hover:bg-gray-100 dark:hover:bg-white/5 transition-colors">
                            <svg class="w-5 h-5" xmlns="http://www.w3.org/2000/svg" fill="none"
                                 viewBox="0 0 24 24" stroke-width="2" stroke="currentColor">
                                <path stroke-linecap="round" stroke-linejoin="round" d="M6 18 18 6M6 6l12 12" />
                            </svg>
                        </button>
                    </div>
                    // Body
                    <div class="flex-1 overflow-y-auto px-5 py-4 space-y-4">
                        // Status + nickname row
                        <div class="flex items-center gap-2 flex-wrap">
                            <span class={format!("inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 \
                                                   text-xs font-medium ring-1 ring-inset {status_class}")}>
                                <span class={format!("w-1.5 h-1.5 rounded-full {status_dot}")} />
                                {status_text}
                            </span>
                            if req.parent_request_id.is_some() {
                                <span class="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] \
                                             font-medium bg-violet-50 text-violet-700 ring-1 ring-inset ring-violet-700/10 \
                                             dark:bg-violet-500/10 dark:text-violet-300 dark:ring-violet-500/25">
                                    {ar_t::FOLLOW_UP_BADGE}
                                </span>
                            }
                            <span class="text-xs text-gray-500 dark:text-[var(--muted)] ml-auto">{&req.nickname}</span>
                        </div>
                        if let Some(ref summary) = props.parent_summary {
                            <div class="flex items-center gap-1.5 text-xs text-violet-600 dark:text-violet-400 \
                                        bg-violet-50 dark:bg-violet-500/5 rounded-lg px-3 py-2">
                                <svg class="w-3.5 h-3.5 shrink-0" xmlns="http://www.w3.org/2000/svg" fill="none"
                                     viewBox="0 0 24 24" stroke-width="2" stroke="currentColor">
                                    <path stroke-linecap="round" stroke-linejoin="round"
                                          d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3" />
                                </svg>
                                <span>{summary}</span>
                            </div>
                        }
                        // URL
                        <div>
                            <p class="text-[11px] font-medium text-gray-400 dark:text-[var(--muted)] uppercase tracking-wider mb-1">
                                {ar_t::LABEL_URL}
                            </p>
                            <a href={req.article_url.clone()} target="_blank" rel="noopener noreferrer"
                                class="text-sm text-blue-600 dark:text-[var(--primary)] hover:underline break-all">
                                {&req.article_url}
                            </a>
                        </div>
                        // Title hint
                        if let Some(ref hint) = req.title_hint {
                            <div>
                                <p class="text-[11px] font-medium text-gray-400 dark:text-[var(--muted)] uppercase tracking-wider mb-1">
                                    {ar_t::LABEL_TITLE}
                                </p>
                                <p class="text-sm text-gray-800 dark:text-[var(--text)]">{hint}</p>
                            </div>
                        }
                        // Request message
                        <div>
                            <p class="text-[11px] font-medium text-gray-400 dark:text-[var(--muted)] uppercase tracking-wider mb-1">
                                {ar_t::LABEL_MESSAGE}
                            </p>
                            <p class="text-sm text-gray-700 dark:text-[var(--text)] whitespace-pre-wrap">{&req.request_message}</p>
                        </div>
                        // AI reply
                        if let Some(ref rendered) = ai_rendered {
                            <div>
                                <p class="text-[11px] font-medium text-gray-400 dark:text-[var(--muted)] uppercase tracking-wider mb-1">
                                    {ar_t::LABEL_AI_REPLY}
                                </p>
                                <div class="rounded-xl bg-gray-50 dark:bg-[var(--surface-alt)] \
                                            border border-gray-100 dark:border-[var(--border)] \
                                            p-4 text-sm prose prose-sm prose-gray dark:prose-invert max-w-none">
                                    <RawHtml html={AttrValue::from(rendered.clone())} />
                                </div>
                            </div>
                        }
                        // Ingested article link
                        if req.status == "done" {
                            if let Some(ref aid) = req.ingested_article_id {
                                <div>
                                    <Link<Route> to={Route::ArticleDetail { id: aid.clone() }}
                                        classes="inline-flex items-center gap-1 text-sm text-blue-600 \
                                                 dark:text-[var(--primary)] hover:underline font-medium">
                                        {ar_t::VIEW_ARTICLE}
                                    </Link<Route>>
                                </div>
                            }
                        }
                        // Region
                        <p class="text-[11px] text-gray-400 dark:text-[var(--muted)]">
                            {ar_t::LABEL_REGION}{": "}{&req.ip_region}
                        </p>
                    </div>
                </div>
            </div>
        }
    } else {
        Html::default()
    };

    // Card body — compact summary
    html! {
        <>
        <div class="bg-white dark:bg-[var(--surface)] border border-gray-200 dark:border-[var(--border)] \
                    rounded-xl p-4 flex flex-col gap-2 shadow-sm hover:shadow-md transition-shadow">
            // Row 1: status + badges + nickname
            <div class="flex items-center justify-between gap-2">
                <div class="flex items-center gap-1.5">
                    <span class={format!("inline-flex items-center gap-1.5 rounded-full px-2.5 py-1 \
                                           text-xs font-medium ring-1 ring-inset {status_class}")}>
                        <span class={format!("w-1.5 h-1.5 rounded-full {status_dot}")} />
                        {status_text}
                    </span>
                    if req.parent_request_id.is_some() {
                        <span class="inline-flex items-center rounded-full px-2 py-0.5 text-[10px] \
                                     font-medium bg-violet-50 text-violet-700 ring-1 ring-inset ring-violet-700/10 \
                                     dark:bg-violet-500/10 dark:text-violet-300 dark:ring-violet-500/25">
                            {ar_t::FOLLOW_UP_BADGE}
                        </span>
                    }
                </div>
                <span class="text-xs text-gray-500 dark:text-[var(--muted)]">{&req.nickname}</span>
            </div>
            // Parent reference
            if let Some(ref summary) = props.parent_summary {
                <div class="flex items-center gap-1 text-[10px] text-violet-600 dark:text-violet-400 \
                            bg-violet-50 dark:bg-violet-500/5 rounded-md px-2 py-1">
                    <svg class="w-3 h-3 shrink-0" xmlns="http://www.w3.org/2000/svg" fill="none"
                         viewBox="0 0 24 24" stroke-width="2" stroke="currentColor">
                        <path stroke-linecap="round" stroke-linejoin="round"
                              d="M9 15 3 9m0 0 6-6M3 9h12a6 6 0 0 1 0 12h-3" />
                    </svg>
                    <span class="truncate">{summary}</span>
                </div>
            }
            // URL — full, no truncation
            <a href={req.article_url.clone()} target="_blank" rel="noopener noreferrer"
                class="text-sm font-medium text-blue-600 dark:text-[var(--primary)] hover:underline break-all leading-snug">
                {&req.article_url}
            </a>
            // Title hint — full
            if let Some(ref hint) = req.title_hint {
                <p class="text-xs text-gray-500 dark:text-[var(--muted)]">{hint}</p>
            }
            // Request message — full
            <p class="text-sm text-gray-700 dark:text-[var(--text)] whitespace-pre-wrap">{&req.request_message}</p>
            // Ingested article link
            if req.status == "done" {
                if let Some(ref aid) = req.ingested_article_id {
                    <Link<Route> to={Route::ArticleDetail { id: aid.clone() }}
                        classes="text-xs text-blue-600 dark:text-[var(--primary)] hover:underline font-medium">
                        {ar_t::VIEW_ARTICLE}
                    </Link<Route>>
                }
            }
            // Action row: detail button + follow-up
            <div class="flex items-center gap-2 mt-1">
                if has_ai_reply {
                    <button onclick={open_modal}
                        class="inline-flex items-center gap-1 text-xs font-medium \
                               text-blue-600 dark:text-[var(--primary)] \
                               hover:bg-blue-50 dark:hover:bg-[var(--primary)]/10 \
                               rounded-md px-2 py-1 transition-colors">
                        <svg class="w-3.5 h-3.5" xmlns="http://www.w3.org/2000/svg" fill="none"
                             viewBox="0 0 24 24" stroke-width="2" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round"
                                  d="M19.5 14.25v-2.625a3.375 3.375 0 0 0-3.375-3.375h-1.5A1.125 1.125 0 0 1 \
                                     13.5 7.125v-1.5a3.375 3.375 0 0 0-3.375-3.375H8.25m0 12.75h7.5m-7.5 3H12M10.5 \
                                     2.25H5.625c-.621 0-1.125.504-1.125 1.125v17.25c0 .621.504 1.125 1.125 \
                                     1.125h12.75c.621 0 1.125-.504 1.125-1.125V11.25a9 9 0 0 0-9-9Z" />
                        </svg>
                        {ar_t::VIEW_DETAIL_BTN}
                    </button>
                }
                if req.status == "done" {
                    <button onclick={on_follow_up_click}
                        class="inline-flex items-center gap-1 text-xs font-medium \
                               text-blue-600 dark:text-[var(--primary)] \
                               hover:bg-blue-50 dark:hover:bg-[var(--primary)]/10 \
                               rounded-md px-2 py-1 transition-colors">
                        {ar_t::FOLLOW_UP_BTN}
                    </button>
                }
            </div>
            // Region
            <span class="text-[10px] text-gray-400 dark:text-[var(--muted)]">{&req.ip_region}</span>
        </div>
        {modal_html}
        </>
    }
}

#[function_component(LatestArticlesPage)]
pub fn latest_articles_page() -> Html {
    let route_location = use_location();
    let articles = use_state(Vec::<ArticleListItem>::new);
    let loading = use_state(|| true);
    let current_page = use_state(|| 1_usize);
    let total = use_state(|| 0_usize);
    let fetch_seq = use_mut_ref(|| 0_u64);

    // Article request state
    let ar_requests = use_state(Vec::<ArticleRequestItem>::new);
    let ar_loading = use_state(|| false);
    let ar_page = use_state(|| 1_usize);
    let ar_total = use_state(|| 0_usize);
    let ar_list_error = use_state(|| None::<String>);
    let ar_form_url = use_state(String::new);
    let ar_form_title = use_state(String::new);
    let ar_form_message = use_state(String::new);
    let ar_form_nickname = use_state(String::new);
    let ar_form_email = use_state(String::new);
    let ar_submitting = use_state(|| false);
    let ar_submit_msg = use_state(|| None::<String>);
    let ar_submit_err = use_state(|| None::<String>);
    let ar_refresh_seq = use_state(|| 0_u32);
    let ar_parent_request = use_state(|| None::<ArticleRequestItem>);

    let total_pages = {
        let t = *total;
        if t == 0 {
            1
        } else {
            t.div_ceil(PAGE_SIZE)
        }
    };
    let current_page_num = *current_page;

    {
        let articles = articles.clone();
        let loading = loading.clone();
        let total = total.clone();
        let fetch_seq = fetch_seq.clone();
        let page = *current_page;
        use_effect_with(page, move |page| {
            let offset = (*page - 1) * PAGE_SIZE;
            let request_id = {
                let mut seq = fetch_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            loading.set(true);
            let articles = articles.clone();
            let loading = loading.clone();
            let total = total.clone();
            let fetch_seq = fetch_seq.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_articles(None, None, Some(PAGE_SIZE), Some(offset)).await {
                    Ok(data) => {
                        if *fetch_seq.borrow() != request_id {
                            return;
                        }
                        total.set(data.total);
                        articles.set(data.articles);
                    },
                    Err(e) => {
                        if *fetch_seq.borrow() != request_id {
                            return;
                        }
                        web_sys::console::error_1(
                            &format!("Failed to fetch articles: {}", e).into(),
                        );
                    },
                }
                if *fetch_seq.borrow() != request_id {
                    return;
                }
                loading.set(false);
            });
            || ()
        });
    }

    let save_scroll_position = {
        let location = route_location.clone();
        Callback::from(move |_| {
            if crate::navigation_context::is_return_armed() {
                return;
            }
            let mut state = BTreeMap::new();
            state.insert("page".to_string(), current_page_num.to_string());
            if let Some(loc) = location.as_ref() {
                state.insert("location".to_string(), loc.path().to_string());
            }
            crate::navigation_context::save_context_for_current_page(state);
        })
    };

    {
        let location = route_location.clone();
        use_effect_with((location, current_page_num), move |_| {
            let mut on_scroll_opt: Option<wasm_bindgen::closure::Closure<dyn FnMut(Event)>> = None;

            if !crate::navigation_context::is_return_armed() {
                let persist = move || {
                    let mut state = BTreeMap::new();
                    state.insert("page".to_string(), current_page_num.to_string());
                    crate::navigation_context::save_context_for_current_page(state);
                };

                persist();

                let on_scroll = wasm_bindgen::closure::Closure::wrap(Box::new(move |_: Event| {
                    if crate::navigation_context::is_return_armed() {
                        return;
                    }
                    let mut state = BTreeMap::new();
                    state.insert("page".to_string(), current_page_num.to_string());
                    crate::navigation_context::save_context_for_current_page(state);
                })
                    as Box<dyn FnMut(_)>);

                if let Some(win) = window() {
                    let _ = win.add_event_listener_with_callback(
                        "scroll",
                        on_scroll.as_ref().unchecked_ref(),
                    );
                }

                on_scroll_opt = Some(on_scroll);
            }

            move || {
                if let Some(on_scroll) = on_scroll_opt {
                    if let Some(win) = window() {
                        let _ = win.remove_event_listener_with_callback(
                            "scroll",
                            on_scroll.as_ref().unchecked_ref(),
                        );
                    }
                }
            }
        });
    }

    {
        let current_page = current_page.clone();
        let location_dep = route_location.clone();
        let article_len = articles.len();
        use_effect_with((location_dep, article_len), move |_| {
            if article_len > 0 {
                if let Some(context) =
                    crate::navigation_context::pop_context_if_armed_for_current_page()
                {
                    if let Some(page_num) = context
                        .page_state
                        .get("page")
                        .and_then(|raw| raw.parse::<usize>().ok())
                    {
                        current_page.set(page_num);
                    }

                    let scroll_y = context.scroll_y.max(0.0);
                    Timeout::new(140, move || {
                        if let Some(win) = window() {
                            win.scroll_to_with_x_and_y(0.0, scroll_y);
                        }
                    })
                    .forget();
                }
            }
            || ()
        });
    }

    let go_to_page = {
        let current_page = current_page.clone();
        Callback::from(move |page: usize| {
            current_page.set(page);
        })
    };

    let pagination_controls = if total_pages > 1 {
        html! {
            <div class={classes!("mt-10", "flex", "justify-center")}>
                <Pagination
                    current_page={current_page_num}
                    total_pages={total_pages}
                    on_page_change={go_to_page.clone()}
                />
            </div>
        }
    } else {
        Html::default()
    };

    // Article request: fetch on page change
    let ar_total_pages = {
        let t = *ar_total;
        if t == 0 { 1 } else { t.div_ceil(REQUEST_PAGE_SIZE) }
    };
    {
        let ar_requests = ar_requests.clone();
        let ar_loading = ar_loading.clone();
        let ar_total = ar_total.clone();
        let ar_list_error = ar_list_error.clone();
        let page = *ar_page;
        let seq = *ar_refresh_seq;
        use_effect_with((page, seq), move |(page, _seq)| {
            let offset = (*page - 1) * REQUEST_PAGE_SIZE;
            ar_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::fetch_article_requests(Some(REQUEST_PAGE_SIZE), Some(offset)).await {
                    Ok(data) => {
                        ar_total.set(data.total);
                        ar_requests.set(data.requests);
                        ar_list_error.set(None);
                    },
                    Err(e) => {
                        ar_list_error.set(Some(e));
                    },
                }
                ar_loading.set(false);
            });
            || ()
        });
    }

    let on_ar_page_change = {
        let ar_page = ar_page.clone();
        Callback::from(move |page: usize| ar_page.set(page))
    };

    let on_ar_refresh = {
        let ar_refresh_seq = ar_refresh_seq.clone();
        Callback::from(move |_: MouseEvent| {
            ar_refresh_seq.set(*ar_refresh_seq + 1);
        })
    };

    let on_follow_up = {
        let ar_parent_request = ar_parent_request.clone();
        let ar_form_url = ar_form_url.clone();
        Callback::from(move |req: ArticleRequestItem| {
            ar_form_url.set(req.article_url.clone());
            ar_parent_request.set(Some(req));
            // Scroll to form
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                if let Some(el) = doc.get_element_by_id("article-request-section") {
                    el.scroll_into_view();
                }
            }
        })
    };

    let on_cancel_follow_up = {
        let ar_parent_request = ar_parent_request.clone();
        let ar_form_url = ar_form_url.clone();
        Callback::from(move |_: MouseEvent| {
            ar_parent_request.set(None);
            ar_form_url.set(String::new());
        })
    };

    let has_parent = ar_parent_request.is_some();

    let on_ar_submit = {
        let ar_form_url = ar_form_url.clone();
        let ar_form_title = ar_form_title.clone();
        let ar_form_message = ar_form_message.clone();
        let ar_form_nickname = ar_form_nickname.clone();
        let ar_form_email = ar_form_email.clone();
        let ar_submitting = ar_submitting.clone();
        let ar_submit_msg = ar_submit_msg.clone();
        let ar_submit_err = ar_submit_err.clone();
        let ar_page = ar_page.clone();
        let ar_parent_request = ar_parent_request.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let url = (*ar_form_url).clone();
            let title = (*ar_form_title).clone();
            let message = (*ar_form_message).clone();
            let nickname = (*ar_form_nickname).clone();
            let email = (*ar_form_email).clone();
            let parent_id = (*ar_parent_request).as_ref().map(|r| r.request_id.clone());
            let ar_submitting = ar_submitting.clone();
            let ar_submit_msg = ar_submit_msg.clone();
            let ar_submit_err = ar_submit_err.clone();
            let ar_form_url = ar_form_url.clone();
            let ar_form_title = ar_form_title.clone();
            let ar_form_message = ar_form_message.clone();
            let ar_page = ar_page.clone();
            let ar_parent_request = ar_parent_request.clone();
            ar_submitting.set(true);
            ar_submit_msg.set(None);
            ar_submit_err.set(None);
            let frontend_page_url = web_sys::window()
                .and_then(|w| w.location().href().ok());
            wasm_bindgen_futures::spawn_local(async move {
                let title_opt = if title.trim().is_empty() { None } else { Some(title.trim()) };
                let nick_opt = if nickname.trim().is_empty() { None } else { Some(nickname.trim()) };
                let email_opt = if email.trim().is_empty() { None } else { Some(email.trim()) };
                match api::submit_article_request(
                    &url,
                    title_opt,
                    &message,
                    nick_opt,
                    email_opt,
                    frontend_page_url.as_deref(),
                    parent_id.as_deref(),
                ).await {
                    Ok(_) => {
                        ar_submit_msg.set(Some(ar_t::SUBMIT_SUCCESS.to_string()));
                        ar_form_url.set(String::new());
                        ar_form_title.set(String::new());
                        ar_form_message.set(String::new());
                        ar_page.set(1);
                        ar_parent_request.set(None);
                    },
                    Err(err) => {
                        ar_submit_err.set(Some(err));
                    },
                }
                ar_submitting.set(false);
            });
        })
    };

    let scroll_to_request_section = Callback::from(|_: MouseEvent| {
        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            if let Some(el) = doc.get_element_by_id("article-request-section") {
                el.scroll_into_view();
            }
        }
    });

    html! {
        <main class={classes!(
            "mt-[var(--header-height-mobile)]",
            "md:mt-[var(--header-height-desktop)]",
            "pb-20"
        )}>
            <div class={classes!("container")}>
                // Hero Section with Editorial Style
                <div class={classes!(
                    "text-center",
                    "py-16",
                    "md:py-24",
                    "px-4",
                    "relative",
                    "overflow-hidden"
                )}>
                    <p class={classes!(
                        "text-sm",
                        "tracking-[0.4em]",
                        "uppercase",
                        "text-[var(--muted)]",
                        "mb-6",
                        "font-semibold"
                    )}>{ t::HERO_INDEX }</p>

                    <h1 class={classes!(
                        "text-5xl",
                        "md:text-7xl",
                        "font-bold",
                        "mb-6",
                        "leading-tight"
                    )}
                    style="font-family: 'Fraunces', serif;">
                        { t::HERO_TITLE }
                    </h1>

                    <p class={classes!(
                        "text-lg",
                        "md:text-xl",
                        "text-[var(--muted)]",
                        "max-w-2xl",
                        "mx-auto",
                        "leading-relaxed"
                    )}>
                        { t::HERO_DESC }
                    </p>
                </div>

                // Article Grid with Editorial Style
                {
                    if *loading {
                        html! {
                            <div class={classes!("flex", "items-center", "justify-center", "min-h-[400px]")}>
                                <LoadingSpinner size={SpinnerSize::Large} />
                            </div>
                        }
                    } else if articles.is_empty() {
                        html! {
                            <div class={classes!(
                                "empty-state",
                                "text-center",
                                "py-20",
                                "px-4",
                                "bg-[var(--surface)]",
                                "liquid-glass",
                                "rounded-2xl",
                                "border",
                                "border-[var(--border)]"
                            )}>
                                <i class={classes!("fas", "fa-inbox", "text-6xl", "text-[var(--muted)]", "mb-6")}></i>
                                <p class={classes!("text-xl", "text-[var(--muted)]")}>
                                    { t::EMPTY }
                                </p>
                            </div>
                        }
                    } else {
                        html! {
                            <>
                                <div class={classes!(
                                    "articles-grid",
                                    "grid",
                                    "grid-cols-1",
                                    "md:grid-cols-2",
                                    "lg:grid-cols-3",
                                    "gap-6",
                                    "mb-12"
                                )}>
                                    { for articles.iter().map(|article| {
                                        html! {
                                            <ArticleCard
                                                key={article.id.clone()}
                                                article={article.clone()}
                                                on_before_navigate={Some(save_scroll_position.clone())}
                                            />
                                        }
                                    }) }
                                </div>
                                { pagination_controls }
                            </>
                        }
                    }
                }
            </div>

            // Article request section
            <div class="container" id="article-request-section">
                <div class="mt-16 border-t border-[var(--border)] pt-10">
                    <h2 class="text-2xl font-bold text-[var(--text)] mb-1" style="font-family: 'Fraunces', serif;">
                        {ar_t::SECTION_TITLE}
                    </h2>
                    <p class="text-[var(--muted)] text-sm mb-6">{ar_t::SECTION_SUBTITLE}</p>

                    <form onsubmit={on_ar_submit}
                        class="bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl p-5 mb-8 \
                               grid grid-cols-1 sm:grid-cols-2 gap-4">
                        if has_parent {
                            <div class="sm:col-span-2 flex items-center justify-between gap-2 \
                                        bg-[var(--primary)]/10 rounded-lg px-3 py-2">
                                <span class="text-xs text-[var(--primary)] font-medium">
                                    {ar_t::FOLLOW_UP_INDICATOR}
                                </span>
                                <button type="button" onclick={on_cancel_follow_up.clone()}
                                    class="text-xs text-[var(--muted)] hover:text-[var(--text)] transition-colors">
                                    {ar_t::CANCEL_FOLLOW_UP}
                                </button>
                            </div>
                        }
                        <div class="sm:col-span-2">
                            <label class="block text-xs text-[var(--muted)] mb-1">{ar_t::URL_LABEL}</label>
                            <input type="url" placeholder={ar_t::URL_PLACEHOLDER}
                                value={(*ar_form_url).clone()}
                                oninput={let s = ar_form_url.clone(); Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    s.set(input.value());
                                })}
                                class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                       text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]"
                                required={!has_parent} />
                        </div>
                        <div>
                            <label class="block text-xs text-[var(--muted)] mb-1">{ar_t::TITLE_HINT_LABEL}</label>
                            <input type="text" placeholder={ar_t::TITLE_HINT_PLACEHOLDER}
                                value={(*ar_form_title).clone()}
                                oninput={let s = ar_form_title.clone(); Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    s.set(input.value());
                                })}
                                class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                       text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                        </div>
                        <div>
                            <label class="block text-xs text-[var(--muted)] mb-1">{ar_t::NICKNAME_LABEL}</label>
                            <input type="text" placeholder={ar_t::NICKNAME_PLACEHOLDER}
                                value={(*ar_form_nickname).clone()}
                                oninput={let s = ar_form_nickname.clone(); Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    s.set(input.value());
                                })}
                                class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                       text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                        </div>
                        <div class="sm:col-span-2">
                            <label class="block text-xs text-[var(--muted)] mb-1">{ar_t::MESSAGE_LABEL}</label>
                            <textarea placeholder={ar_t::MESSAGE_PLACEHOLDER}
                                value={(*ar_form_message).clone()}
                                oninput={let s = ar_form_message.clone(); Callback::from(move |e: InputEvent| {
                                    let input: HtmlTextAreaElement = e.target_unchecked_into();
                                    s.set(input.value());
                                })}
                                rows="3"
                                class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                       text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)] resize-none"
                                required=true />
                        </div>
                        <div>
                            <label class="block text-xs text-[var(--muted)] mb-1">{ar_t::EMAIL_LABEL}</label>
                            <input type="email" placeholder={ar_t::EMAIL_PLACEHOLDER}
                                value={(*ar_form_email).clone()}
                                oninput={let s = ar_form_email.clone(); Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    s.set(input.value());
                                })}
                                class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                       text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                            <p class="mt-1 text-[11px] text-[var(--muted)]">{ar_t::EMAIL_HELP_TEXT}</p>
                        </div>
                        <div class="flex items-end">
                            <button type="submit" disabled={*ar_submitting}
                                class="px-5 py-2 rounded-lg bg-[var(--primary)] text-white text-sm font-medium \
                                       hover:opacity-90 transition-opacity disabled:opacity-50">
                                {if *ar_submitting { ar_t::SUBMITTING } else { ar_t::SUBMIT_BTN }}
                            </button>
                        </div>
                        if let Some(ref msg) = *ar_submit_msg {
                            <div class="sm:col-span-2 text-green-500 text-sm">{msg}</div>
                        }
                        if let Some(ref err) = *ar_submit_err {
                            <div class="sm:col-span-2 text-red-500 text-sm">{err}</div>
                        }
                    </form>

                    // Refresh button
                    <div class="flex justify-end mb-4">
                        <button
                            onclick={on_ar_refresh}
                            disabled={*ar_loading}
                            class="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg \
                                   border border-[var(--border)] bg-[var(--surface)] \
                                   text-[var(--text)] text-sm font-medium \
                                   hover:bg-[var(--surface-alt)] transition-colors \
                                   disabled:opacity-50 disabled:cursor-not-allowed"
                        >
                            <svg class={if *ar_loading { "w-4 h-4 animate-spin" } else { "w-4 h-4" }}
                                 xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24"
                                 stroke-width="2" stroke="currentColor">
                                <path stroke-linecap="round" stroke-linejoin="round"
                                      d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 \
                                         3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 \
                                         13.803-3.7l3.181 3.182" />
                            </svg>
                            {if *ar_loading { ar_t::REFRESHING } else { ar_t::REFRESH_BTN }}
                        </button>
                    </div>

                    if *ar_loading && ar_requests.is_empty() {
                        <div class="flex justify-center py-8">
                            <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-[var(--primary)]" />
                        </div>
                    } else if let Some(err) = (*ar_list_error).clone() {
                        <p class="text-center text-red-500 py-8">{format!("Failed to load requests: {err}")}</p>
                    } else if ar_requests.is_empty() {
                        <p class="text-center text-[var(--muted)] py-8">{ar_t::EMPTY_LIST}</p>
                    } else {
                        <>
                            if *ar_loading {
                                <div class="mb-3 inline-flex items-center gap-2 text-xs text-[var(--muted)]">
                                    <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-[var(--primary)]" />
                                    <span>{"Loading..."}</span>
                                </div>
                            }
                            <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
                                { for ar_requests.iter().map(|req| {
                                    let key = req.request_id.clone();
                                    let parent_summary = req.parent_request_id.as_ref().and_then(|pid| {
                                        ar_requests.iter().find(|r| r.request_id == *pid).map(|parent| {
                                            let label = parent.title_hint.as_deref()
                                                .filter(|s| !s.is_empty())
                                                .unwrap_or(&parent.article_url);
                                            let display: String = if label.chars().count() > 40 {
                                                format!("{}…", label.chars().take(39).collect::<String>())
                                            } else {
                                                label.to_string()
                                            };
                                            AttrValue::from(format!("{} {}", ar_t::FOLLOW_UP_REF_PREFIX, display))
                                        })
                                    });
                                    html! {
                                        <RequestCard
                                            key={key}
                                            req={req.clone()}
                                            on_follow_up={on_follow_up.clone()}
                                            parent_summary={parent_summary}
                                        />
                                    }
                                }) }
                            </div>
                        </>
                    }
                    if ar_total_pages > 1 {
                        <div class="flex justify-center mt-6">
                            <Pagination
                                current_page={*ar_page}
                                total_pages={ar_total_pages}
                                on_page_change={on_ar_page_change.clone()}
                            />
                        </div>
                    }
                </div>
            </div>

            // Fixed nav button (left bottom) — icon with tooltip
            <button
                onclick={scroll_to_request_section}
                title={ar_t::NAV_BTN}
                class="group fixed left-4 bottom-20 z-50 w-10 h-10 rounded-full \
                       bg-[var(--primary)] text-white shadow-lg \
                       hover:scale-110 hover:shadow-xl active:scale-95 \
                       transition-all duration-200 flex items-center justify-center"
            >
                <svg xmlns="http://www.w3.org/2000/svg" class="w-5 h-5" fill="none"
                     viewBox="0 0 24 24" stroke-width="2" stroke="currentColor">
                    <path stroke-linecap="round" stroke-linejoin="round"
                          d="M12 4.5v15m7.5-7.5h-15" />
                </svg>
                <span class="pointer-events-none absolute left-full ml-2 px-2 py-1 \
                             rounded bg-[var(--surface)] text-[var(--text)] text-xs \
                             border border-[var(--border)] shadow-md whitespace-nowrap \
                             opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                    {ar_t::NAV_BTN}
                </span>
            </button>

            <ScrollToTopButton />
        </main>
    }
}
