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
    i18n::{current::tag_detail_page as t, fill_one},
    pages::posts::group_articles_by_year,
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct TagDetailProps {
    pub tag: String,
}

#[function_component(TagDetailPage)]
pub fn tag_detail_page(props: &TagDetailProps) -> Html {
    let normalized = props.tag.trim().to_string();
    let filter_value = if normalized.is_empty() { None } else { Some(normalized) };
    let display_tag = filter_value
        .clone()
        .unwrap_or_else(|| t::UNNAMED.to_string());

    let articles = use_state(Vec::<ArticleListItem>::new);
    let loading = use_state(|| true);

    {
        let articles = articles.clone();
        let tag = filter_value.clone();
        let loading = loading.clone();
        use_effect_with(tag.clone(), move |_| {
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                let tag_ref = tag.as_deref();
                match crate::api::fetch_all_articles(tag_ref, None).await {
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

    let empty_message = if let Some(tag_value) = filter_value.as_ref() {
        fill_one(t::EMPTY_TEMPLATE, tag_value)
    } else {
        t::INVALID_NAME.to_string()
    };

    {
        let tag = filter_value.clone();
        let total_posts = total_posts;
        use_effect_with((tag.clone(), total_posts), move |_| {
            let persist = move || {
                if crate::navigation_context::is_return_armed() {
                    return;
                }
                let mut state = BTreeMap::new();
                if let Some(tag) = tag.as_ref() {
                    state.insert("tag".to_string(), tag.clone());
                }
                crate::navigation_context::save_context_for_current_page(state);
            };

            persist();

            let on_scroll = wasm_bindgen::closure::Closure::wrap(Box::new(move |_: Event| {
                persist();
            })
                as Box<dyn FnMut(_)>);

            if let Some(win) = window() {
                let _ = win
                    .add_event_listener_with_callback("scroll", on_scroll.as_ref().unchecked_ref());
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
        let tag = filter_value.clone();
        let total_posts = total_posts;
        use_effect_with((tag, total_posts), move |_| {
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
            "tag-detail-page",
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
                // Header Section with Editorial Style
                <header class={classes!(
                    "tag-header",
                    "mb-16",
                    "text-center",
                    "relative"
                )}>
                    <div class={classes!(
                        "tag-badge",
                        "inline-flex",
                        "items-center",
                        "gap-2",
                        "px-4",
                        "py-2",
                        "mb-6",
                        "rounded-full",
                        "bg-gradient-to-r",
                        "from-[var(--primary)]/10",
                        "to-purple-500/10",
                        "border",
                        "border-[var(--primary)]/30",
                        "text-[var(--primary)]",
                        "font-medium",
                        "text-sm",
                        "tracking-wider",
                        "uppercase"
                    )}>
                        <i class="fas fa-tag"></i>
                        <span>{ t::ARCHIVE_BADGE }</span>
                    </div>

                    <h1 class={classes!(
                        "tag-title",
                        "text-5xl",
                        "md:text-7xl",
                        "font-bold",
                        "mb-4",
                        "leading-tight"
                    )}>
                        { format!("#{}", display_tag) }
                    </h1>

                    <p class={classes!(
                        "tag-count",
                        "text-xl",
                        "text-[var(--muted)]",
                        "font-light"
                    )}>
                        if total_posts > 0 {
                            { fill_one(t::COLLECTED_COUNT_TEMPLATE, total_posts) }
                        } else {
                            { t::NO_CONTENT }
                        }
                    </p>

                    // Decorative gradient line
                    <div class={classes!(
                        "tag-divider",
                        "w-24",
                        "h-1",
                        "mx-auto",
                        "mt-8",
                        "rounded-full",
                        "bg-gradient-to-r",
                        "from-transparent",
                        "via-[var(--primary)]",
                        "to-transparent",
                        "opacity-50"
                    )}></div>
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
                                    "fa-folder-open",
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
                        render_editorial_timeline(&grouped_by_year)
                    }
                }
            </div>
            <ScrollToTopButton />
        </main>
    }
}

// Editorial-style timeline rendering
fn render_editorial_timeline(grouped_by_year: &[(i32, Vec<ArticleListItem>)]) -> Html {
    html! {
        <div class={classes!("editorial-timeline")}>
            { for grouped_by_year.iter().map(|(year, posts)| {
                html! {
                    <section class={classes!("timeline-year-section", "mb-16")} key={*year}>
                        // Year Header with Decorative Style
                        <div class={classes!(
                            "year-header",
                            "flex",
                            "items-center",
                            "gap-4",
                            "mb-8"
                        )}>
                            <div class={classes!(
                                "year-label",
                                "text-3xl",
                                "md:text-4xl",
                                "font-bold",
                                "text-[var(--text)]",
                                "px-6",
                                "py-2",
                                "rounded-lg",
                                "bg-gradient-to-br",
                                "from-[var(--primary)]/10",
                                "to-purple-500/10",
                                "border-2",
                                "border-[var(--primary)]/30",
                                "shadow-lg"
                            )}>
                                { year }
                            </div>
                            <div class={classes!(
                                "year-line",
                                "flex-1",
                                "h-[2px]",
                                "bg-gradient-to-r",
                                "from-[var(--primary)]/30",
                                "to-transparent",
                                "rounded-full"
                            )}></div>
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
                                render_article_card(article, detail_route)
                            }) }
                        </div>
                    </section>
                }
            }) }
        </div>
    }
}

fn render_article_card(article: &ArticleListItem, route: Route) -> Html {
    html! {
        <Link<Route>
            to={route}
            classes={classes!("article-card-editorial")}
        >
            <article class={classes!(
                "relative",
                "group",
                "p-6",
                "rounded-xl",
                "bg-[var(--surface)]/80",
                "backdrop-blur-sm",
                "border",
                "border-[var(--border)]",
                "transition-all",
                "duration-300",
                "hover:shadow-xl",
                "hover:border-[var(--primary)]/50",
                "hover:-translate-y-1"
            )}>
                // Glow effect on hover
                <div class={classes!(
                    "absolute",
                    "inset-0",
                    "rounded-xl",
                    "bg-gradient-to-br",
                    "from-[var(--primary)]/0",
                    "to-purple-500/0",
                    "opacity-0",
                    "group-hover:opacity-10",
                    "transition-opacity",
                    "duration-300",
                    "pointer-events-none"
                )}></div>

                <div class={classes!("relative", "z-10")}>
                    <h3 class={classes!(
                        "article-card-title",
                        "text-xl",
                        "md:text-2xl",
                        "font-bold",
                        "text-[var(--text)]",
                        "mb-3",
                        "group-hover:text-[var(--primary)]",
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
                            <i class="far fa-folder"></i>
                            { &article.category }
                        </span>
                    </div>
                </div>

                // Arrow indicator
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
                    <i class="fas fa-arrow-right"></i>
                </div>
            </article>
        </Link<Route>>
    }
}
