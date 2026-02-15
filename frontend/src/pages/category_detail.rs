use std::collections::BTreeMap;

use gloo_timers::callback::Timeout;
use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::{window, Event};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{
        loading_spinner::{LoadingSpinner, SpinnerSize},
        scroll_to_top_button::ScrollToTopButton,
    },
    i18n::{current::category_detail_page as t, fill_one},
    pages::posts::group_articles_by_year,
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct CategoryDetailProps {
    pub category: String,
}

#[function_component(CategoryDetailPage)]
pub fn category_detail_page(props: &CategoryDetailProps) -> Html {
    let normalized = props.category.trim().to_string();
    let filter_value = if normalized.is_empty() { None } else { Some(normalized) };
    let display_category = filter_value
        .clone()
        .unwrap_or_else(|| t::UNNAMED.to_string());

    let articles = use_state(Vec::<ArticleListItem>::new);
    let loading = use_state(|| true);

    {
        let articles = articles.clone();
        let category = filter_value.clone();
        let loading = loading.clone();
        use_effect_with(category.clone(), move |_| {
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let category_ref = category.as_deref();
                match crate::api::fetch_articles(None, category_ref).await {
                    Ok(data) => {
                        articles.set(data);
                        loading.set(false);
                    },
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch articles: {}", e).into(),
                        );
                        loading.set(false);
                    },
                }
            });
            || ()
        });
    }

    let filtered = (*articles).clone();

    let total_posts = filtered.len();
    let grouped_by_year = group_articles_by_year(&filtered);

    let empty_message = if let Some(category_value) = filter_value.as_ref() {
        fill_one(t::EMPTY_TEMPLATE, category_value)
    } else {
        t::INVALID_NAME.to_string()
    };

    {
        let category = filter_value.clone();
        let total_posts = total_posts;
        use_effect_with((category.clone(), total_posts), move |_| {
            let persist = move || {
                if crate::navigation_context::is_return_armed() {
                    return;
                }
                let mut state = BTreeMap::new();
                if let Some(category) = category.as_ref() {
                    state.insert("category".to_string(), category.clone());
                }
                crate::navigation_context::save_context_for_current_page(state);
            };

            persist();

            let on_scroll = wasm_bindgen::closure::Closure::wrap(Box::new(move |_: Event| {
                persist();
            })
                as Box<dyn FnMut(_)>);

            if let Some(win) = window() {
                let _ = win.add_event_listener_with_callback(
                    "scroll",
                    on_scroll.as_ref().unchecked_ref(),
                );
            }

            move || {
                if let Some(win) = window() {
                    let _ = win.remove_event_listener_with_callback(
                        "scroll",
                        on_scroll.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    {
        let category = filter_value.clone();
        let total_posts = total_posts;
        use_effect_with((category, total_posts), move |_| {
            if total_posts > 0 {
                if let Some(context) =
                    crate::navigation_context::pop_context_if_armed_for_current_page()
                {
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

    html! {
        <main class={classes!(
            "category-detail-page",
            "min-h-screen",
            "pt-24",
            "pb-20",
            "px-4",
            "md:px-8"
        )}>
            <div class={classes!(
                "max-w-5xl",
                "mx-auto"
            )}>
                // Header Section with Structured Style
                <header class={classes!(
                    "category-header",
                    "mb-16",
                    "text-center",
                    "relative"
                )}>
                    <div class={classes!(
                        "category-badge",
                        "inline-flex",
                        "items-center",
                        "gap-2",
                        "px-4",
                        "py-2",
                        "mb-6",
                        "rounded-lg",
                        "bg-gradient-to-r",
                        "from-[var(--primary)]/10",
                        "to-sky-500/10",
                        "border",
                        "border-[var(--primary)]/30",
                        "text-[var(--primary)]",
                        "dark:text-sky-400",
                        "font-medium",
                        "text-sm",
                        "tracking-wider",
                        "uppercase"
                    )}>
                        <i class="fas fa-folder-open"></i>
                        <span>{ t::COLLECTION_BADGE }</span>
                    </div>

                    <h1 class={classes!(
                        "category-title",
                        "text-5xl",
                        "md:text-7xl",
                        "font-bold",
                        "mb-4",
                        "leading-tight"
                    )}>
                        { &display_category }
                    </h1>

                    <p class={classes!(
                        "category-count",
                        "text-xl",
                        "text-[var(--muted)]",
                        "font-light"
                    )}>
                        if total_posts > 0 {
                            { fill_one(t::HIGHLIGHT_COUNT_TEMPLATE, total_posts) }
                        } else {
                            { t::NO_CONTENT }
                        }
                    </p>

                    // Decorative corner brackets
                    <div class={classes!(
                        "category-brackets",
                        "flex",
                        "justify-center",
                        "gap-8",
                        "mt-8"
                    )}>
                        <div class={classes!(
                            "w-12",
                            "h-1",
                            "bg-gradient-to-r",
                            "from-[var(--primary)]",
                            "to-sky-500",
                            "rounded-full"
                        )}></div>
                        <div class={classes!(
                            "w-2",
                            "h-2",
                            "bg-gradient-to-br",
                            "from-[var(--primary)]",
                            "to-sky-500",
                            "rounded-full",
                            "mt-[-2px]"
                        )}></div>
                        <div class={classes!(
                            "w-12",
                            "h-1",
                            "bg-gradient-to-l",
                            "from-[var(--primary)]",
                            "to-sky-500",
                            "rounded-full"
                        )}></div>
                    </div>
                </header>

                // Content Section
                {
                    if *loading {
                        html! {
                            <div class={classes!(
                                "flex",
                                "min-h-[40vh]",
                                "items-center",
                                "justify-center"
                            )}>
                                <LoadingSpinner size={SpinnerSize::Large} />
                            </div>
                        }
                    } else if grouped_by_year.is_empty() {
                        html! {
                            <div class={classes!(
                                "empty-state",
                                "text-center",
                                "py-16",
                                "px-8",
                                "rounded-2xl",
                                "bg-[var(--surface)]/50",
                                "border",
                                "border-[var(--border)]"
                            )}>
                                <i class={classes!(
                                    "fas",
                                    "fa-inbox",
                                    "text-5xl",
                                    "text-[var(--muted)]",
                                    "mb-4"
                                )}></i>
                                <p class={classes!(
                                    "text-lg",
                                    "text-[var(--muted)]"
                                )}>
                                    { empty_message }
                                </p>
                            </div>
                        }
                    } else {
                        render_category_timeline(&grouped_by_year)
                    }
                }
            </div>
            <ScrollToTopButton />
        </main>
    }
}

// Category-style timeline rendering with geometric design
fn render_category_timeline(grouped_by_year: &[(i32, Vec<ArticleListItem>)]) -> Html {
    html! {
        <div class={classes!("category-timeline")}>
            { for grouped_by_year.iter().map(|(year, posts)| {
                html! {
                    <section class={classes!("timeline-year-section", "mb-16")} key={*year}>
                        // Year Header with Geometric Style
                        <div class={classes!(
                            "year-header-geometric",
                            "relative",
                            "mb-8",
                            "pb-4",
                            "border-b-2",
                            "border-gradient-to-r",
                            "from-[var(--primary)]/30",
                            "via-sky-500/50",
                            "to-transparent"
                        )}>
                            <div class={classes!(
                                "flex",
                                "items-center",
                                "gap-4"
                            )}>
                                // Geometric year badge
                                <div class={classes!(
                                    "year-badge-geometric",
                                    "relative",
                                    "px-6",
                                    "py-3",
                                    "bg-gradient-to-br",
                                    "from-[var(--primary)]/10",
                                    "to-sky-500/10",
                                    "border-l-4",
                                    "border-[var(--primary)]",
                                    "rounded-r-lg",
                                    "shadow-lg"
                                )}>
                                    <span class={classes!(
                                        "text-3xl",
                                        "md:text-4xl",
                                        "font-bold",
                                        "text-[var(--text)]"
                                    )}>
                                        { year }
                                    </span>
                                    // Corner accent
                                    <div class={classes!(
                                        "absolute",
                                        "top-0",
                                        "right-0",
                                        "w-2",
                                        "h-2",
                                        "bg-[var(--primary)]",
                                        "rounded-bl-full"
                                    )}></div>
                                </div>

                                <div class={classes!(
                                    "text-sm",
                                    "text-[var(--muted)]",
                                    "font-medium"
                                )}>
                                    { fill_one(t::YEAR_POSTS_TEMPLATE, posts.len()) }
                                </div>
                            </div>
                        </div>

                        // Article Cards Grid
                        <div class={classes!(
                            "articles-grid",
                            "grid",
                            "gap-4",
                            "md:gap-6"
                        )}>
                            { for posts.iter().map(|article| {
                                let detail_route = Route::ArticleDetail { id: article.id.clone() };
                                render_category_card(article, detail_route)
                            }) }
                        </div>
                    </section>
                }
            }) }
        </div>
    }
}

fn render_category_card(article: &ArticleListItem, route: Route) -> Html {
    html! {
        <Link<Route>
            to={route}
            classes={classes!("category-card")}
        >
            <article class={classes!(
                "relative",
                "group",
                "p-6",
                "rounded-xl",
                "bg-[var(--surface)]/90",
                "backdrop-blur-sm",
                "border-l-4",
                "border-[var(--primary)]/50",
                "shadow-md",
                "transition-all",
                "duration-300",
                "hover:shadow-2xl",
                "hover:border-[var(--primary)]",
                "hover:translate-x-2"
            )}>
                // Side accent glow
                <div class={classes!(
                    "absolute",
                    "left-0",
                    "top-0",
                    "bottom-0",
                    "w-1",
                    "bg-gradient-to-b",
                    "from-[var(--primary)]",
                    "to-sky-500",
                    "opacity-0",
                    "group-hover:opacity-100",
                    "transition-opacity",
                    "duration-300",
                    "rounded-l-xl"
                )}></div>

                <div class={classes!("relative", "z-10")}>
                    <h3 class={classes!(
                        "category-card-title",
                        "text-xl",
                        "md:text-2xl",
                        "font-bold",
                        "text-[var(--text)]",
                        "mb-3",
                        "group-hover:text-[var(--primary)]",
                        "dark:group-hover:text-sky-400",
                        "transition-colors",
                        "duration-200"
                    )}>
                        { &article.title }
                    </h3>

                    <div class={classes!(
                        "article-meta",
                        "flex",
                        "items-center",
                        "gap-4",
                        "text-sm",
                        "text-[var(--muted)]"
                    )}>
                        <time class={classes!(
                            "flex",
                            "items-center",
                            "gap-2"
                        )}>
                            <i class="far fa-calendar"></i>
                            { &article.date }
                        </time>

                        <span class={classes!(
                            "flex",
                            "items-center",
                            "gap-2"
                        )}>
                            <i class="far fa-user"></i>
                            { &article.author }
                        </span>
                    </div>
                </div>

                // Chevron indicator
                <div class={classes!(
                    "absolute",
                    "right-6",
                    "top-1/2",
                    "-translate-y-1/2",
                    "text-[var(--primary)]",
                    "opacity-0",
                    "group-hover:opacity-100",
                    "group-hover:translate-x-2",
                    "transition-all",
                    "duration-300"
                )}>
                    <i class="fas fa-chevron-right"></i>
                </div>

                // Corner decoration
                <div class={classes!(
                    "absolute",
                    "bottom-0",
                    "right-0",
                    "w-8",
                    "h-8",
                    "bg-gradient-to-tl",
                    "from-[var(--primary)]/10",
                    "to-transparent",
                    "rounded-tl-full",
                    "opacity-0",
                    "group-hover:opacity-100",
                    "transition-opacity",
                    "duration-300"
                )}></div>
            </article>
        </Link<Route>>
    }
}
