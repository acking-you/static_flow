use std::collections::BTreeMap;

use gloo_timers::callback::Timeout;
use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::{window, Event};
use yew::prelude::*;
use yew_router::prelude::use_location;

use crate::{
    components::{
        article_card::ArticleCard,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        pagination::Pagination,
        scroll_to_top_button::ScrollToTopButton,
    },
    hooks::use_pagination,
    i18n::current::latest_articles_page as t,
};

#[function_component(LatestArticlesPage)]
pub fn latest_articles_page() -> Html {
    let route_location = use_location();
    let articles = use_state(Vec::<ArticleListItem>::new);
    let loading = use_state(|| true);

    let (visible_articles, current_page_num, total_pages, go_to_page) =
        use_pagination((*articles).clone(), 12);

    {
        let articles = articles.clone();
        let loading = loading.clone();
        use_effect_with((), move |_| {
            loading.set(true);
            let articles = articles.clone();
            let loading = loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_articles(None, None).await {
                    Ok(data) => articles.set(data),
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch articles: {}", e).into(),
                        );
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let save_scroll_position = {
        let location = route_location.clone();
        let current_page_num = current_page_num;
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
        let current_page_num = current_page_num;
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
        let go_to_page_cb = go_to_page.clone();
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
                        go_to_page_cb.emit(page_num);
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
                    } else if visible_articles.is_empty() {
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
                                    { for visible_articles.iter().map(|article| {
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
            <ScrollToTopButton />
        </main>
    }
}
