use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::window;
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
};

#[function_component(LatestArticlesPage)]
pub fn latest_articles_page() -> Html {
    let route_location = use_location();
    let articles = use_state(|| Vec::<ArticleListItem>::new());
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
        let current_page_num = current_page_num;
        Callback::from(move |_| {
            if let Some(win) = window() {
                if let Some(storage) = win.session_storage().ok().flatten() {
                    let _ = storage.set_item("home_articles_page", &current_page_num.to_string());
                    let scroll_top = win.scroll_y().unwrap_or(0.0);
                    let _ =
                        storage.set_item("home_articles_scroll", &(scroll_top as i32).to_string());
                }
            }
        })
    };

    {
        let go_to_page_cb = go_to_page.clone();
        let location_dep = route_location.clone();
        use_effect_with(location_dep, move |_| {
            if let Some(storage) = window().and_then(|w| w.session_storage().ok().flatten()) {
                let has_saved_data = storage
                    .get_item("home_articles_page")
                    .ok()
                    .flatten()
                    .is_some()
                    || storage
                        .get_item("home_articles_scroll")
                        .ok()
                        .flatten()
                        .is_some();

                if has_saved_data {
                    if let Some(saved_page) = storage.get_item("home_articles_page").ok().flatten()
                    {
                        if let Ok(page_num) = saved_page.parse::<usize>() {
                            go_to_page_cb.emit(page_num);
                        }
                    }

                    let scroll_pos = storage
                        .get_item("home_articles_scroll")
                        .ok()
                        .flatten()
                        .and_then(|v| v.parse::<i32>().ok())
                        .unwrap_or(0);

                    if let Some(win) = window() {
                        let callback = wasm_bindgen::closure::Closure::once(move || {
                            if scroll_pos > 0 {
                                if let Some(win) = window() {
                                    let _ = win.scroll_to_with_x_and_y(0.0, scroll_pos as f64);
                                }
                            }
                            if let Some(storage) =
                                window().and_then(|w| w.session_storage().ok().flatten())
                            {
                                let _ = storage.remove_item("home_articles_scroll");
                                let _ = storage.remove_item("home_articles_page");
                            }
                        });
                        let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                            callback.as_ref().unchecked_ref(),
                            100,
                        );
                        callback.forget();
                    }
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

    let article_grid = if *loading {
        html! {
            <div class={classes!("flex", "items-center", "justify-center", "min-h-[400px]") }>
                <LoadingSpinner size={SpinnerSize::Large} />
            </div>
        }
    } else if visible_articles.is_empty() {
        html! { <p class={classes!("text-center", "text-[var(--muted)]")}>{ "暂无文章" }</p> }
    } else {
        html! {
            <>
                <div class={classes!(
                    "grid",
                    "gap-[var(--space-card-gap)]",
                    "mt-[var(--space-lg)]",
                    "mb-0",
                    "grid-cols-[repeat(auto-fit,minmax(min(320px,100%),1fr))]",
                    "lg:grid-cols-[repeat(3,minmax(0,1fr))]",
                    "md:grid-cols-[repeat(2,minmax(0,1fr))]",
                    "max-[767px]:grid-cols-1"
                )}>
                    { for visible_articles.iter().map(|article| {
                        html! { <ArticleCard article={article.clone()} on_before_navigate={Some(save_scroll_position.clone())} /> }
                    }) }
                </div>
                { pagination_controls }
            </>
        }
    };

    html! {
        <main class={classes!(
            "mt-[var(--space-lg)]",
            "pt-10",
            "pb-16",
            "bg-[var(--bg)]"
        ) }>
            <div class={classes!("w-full", "max-w-[80rem]", "mx-auto", "px-[clamp(1rem,4vw,2.5rem)]") }>
                <section class={classes!(
                    "flex",
                    "flex-col",
                    "gap-[var(--space-md)]",
                    "mb-[var(--space-lg)]",
                    "relative",
                    "z-10",
                    "mt-0"
                )} aria-label="文章列表">
                    <div class={classes!("bg-[var(--surface)]", "border", "border-[var(--border)]", "rounded-[calc(var(--radius)-4px)]", "p-5", "px-6", "shadow-[var(--shadow-sm)]", "transition-[var(--transition-base)]")}
                    >
                        <div>
                            <h2 class={classes!("m-0", "text-[1.4rem]", "font-semibold")}>{ "最新文章" }</h2>
                            <p class={classes!("m-0", "text-[var(--muted)]", "text-[0.95rem]")}>{ "甄选近期发布的内容，持续更新" }</p>
                        </div>
                    </div>
                    { article_grid }
                </section>
            </div>
            <ScrollToTopButton />
        </main>
    }
}
