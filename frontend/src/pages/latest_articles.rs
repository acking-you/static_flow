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
        scroll_to_top_button::ScrollToTopButton,
    },
    i18n::current::latest_articles_page as t,
    i18n::current::article_request as ar_t,
    router::Route,
};

const PAGE_SIZE: usize = 12;
const REQUEST_PAGE_SIZE: usize = 12;

fn render_request_card(req: &ArticleRequestItem) -> Html {
    let status_class = match req.status.as_str() {
        "done" => "bg-green-500/15 text-green-700 dark:text-green-200",
        "running" => "bg-sky-500/15 text-sky-700 dark:text-sky-200",
        "failed" => "bg-red-500/15 text-red-700 dark:text-red-200",
        "rejected" => "bg-gray-500/15 text-gray-700 dark:text-gray-200",
        _ => "bg-amber-500/15 text-amber-700 dark:text-amber-200",
    };
    let status_text = match req.status.as_str() {
        "pending" => ar_t::STATUS_PENDING,
        "approved" => ar_t::STATUS_APPROVED,
        "running" => ar_t::STATUS_RUNNING,
        "done" => ar_t::STATUS_DONE,
        "failed" => ar_t::STATUS_FAILED,
        _ => &req.status,
    };
    let url_display: String = if req.article_url.chars().count() > 60 {
        format!("{}...", req.article_url.chars().take(57).collect::<String>())
    } else {
        req.article_url.clone()
    };

    html! {
        <div class="bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl p-4 flex flex-col gap-2">
            <div class="flex items-center justify-between gap-2">
                <span class={format!("inline-flex items-center rounded-full px-2 py-0.5 text-xs font-semibold {status_class}")}>
                    {status_text}
                </span>
                <span class="text-xs text-[var(--muted)]">{&req.nickname}</span>
            </div>
            <a href={req.article_url.clone()} target="_blank" rel="noopener noreferrer"
                class="text-sm font-medium text-[var(--primary)] hover:underline truncate">
                {&url_display}
            </a>
            if let Some(ref hint) = req.title_hint {
                <p class="text-xs text-[var(--muted)] truncate">{hint}</p>
            }
            <p class="text-sm text-[var(--text)] line-clamp-2">{&req.request_message}</p>
            if req.status == "done" {
                if let Some(ref aid) = req.ingested_article_id {
                    <Link<Route> to={Route::ArticleDetail { id: aid.clone() }}
                        classes="text-xs text-[var(--primary)] hover:underline">
                        {ar_t::VIEW_ARTICLE}
                    </Link<Route>>
                }
            }
            <span class="text-[10px] text-[var(--muted)]">{&req.ip_region}</span>
        </div>
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
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let url = (*ar_form_url).clone();
            let title = (*ar_form_title).clone();
            let message = (*ar_form_message).clone();
            let nickname = (*ar_form_nickname).clone();
            let email = (*ar_form_email).clone();
            let ar_submitting = ar_submitting.clone();
            let ar_submit_msg = ar_submit_msg.clone();
            let ar_submit_err = ar_submit_err.clone();
            let ar_form_url = ar_form_url.clone();
            let ar_form_title = ar_form_title.clone();
            let ar_form_message = ar_form_message.clone();
            let ar_page = ar_page.clone();
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
                ).await {
                    Ok(_) => {
                        ar_submit_msg.set(Some(ar_t::SUBMIT_SUCCESS.to_string()));
                        ar_form_url.set(String::new());
                        ar_form_title.set(String::new());
                        ar_form_message.set(String::new());
                        ar_page.set(1);
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
                                required=true />
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
                                { for ar_requests.iter().map(render_request_card) }
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
