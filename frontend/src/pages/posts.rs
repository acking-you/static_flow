use std::collections::{BTreeMap, HashMap};

use gloo_timers::callback::Timeout;
use serde::{Deserialize, Serialize};
use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::{window, Event};
use yew::prelude::*;
use yew_router::prelude::{use_location, Link};

use crate::{
    components::scroll_to_top_button::ScrollToTopButton,
    i18n::{current::posts_page as t, fill_one},
    router::Route,
};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PostsQuery {
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
}

impl PostsQuery {
    fn trim_field(field: Option<String>) -> Option<String> {
        field.and_then(|raw| {
            let trimmed = raw.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            }
        })
    }

    pub fn normalized(mut self) -> Self {
        self.tag = Self::trim_field(self.tag.take());
        self.category = Self::trim_field(self.category.take());
        self
    }

    pub fn has_filters(&self) -> bool {
        self.tag.is_some() || self.category.is_some()
    }
}

#[function_component(PostsPage)]
pub fn posts_page() -> Html {
    let location = use_location();
    let query = location
        .as_ref()
        .and_then(|loc| loc.query::<PostsQuery>().ok())
        .unwrap_or_default()
        .normalized();

    let articles = use_state(Vec::<ArticleListItem>::new);
    let expanded_years = use_state(HashMap::<i32, bool>::new);

    {
        let articles = articles.clone();
        let tag = query.tag.clone();
        let category = query.category.clone();

        use_effect_with((tag.clone(), category.clone()), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let tag_ref = tag.as_deref();
                let category_ref = category.as_deref();

                match crate::api::fetch_all_articles(tag_ref, category_ref).await {
                    Ok(data) => articles.set(data),
                    Err(e) => {
                        web_sys::console::error_1(
                            &format!("Failed to fetch articles: {}", e).into(),
                        );
                    },
                }
            });
            || ()
        });
    }

    let filtered = (*articles).clone();

    let total_posts = filtered.len();
    let grouped_by_year = group_articles_by_year(&filtered);

    let filter_label = match (&query.tag, &query.category) {
        (Some(tag), Some(category)) => format!("#{tag} · {category}"),
        (Some(tag), None) => format!("#{tag}"),
        (None, Some(category)) => category.clone(),
        (None, None) => String::new(),
    };

    let description = if total_posts == 0 {
        if query.has_filters() {
            t::DESC_EMPTY_FILTERED.to_string()
        } else {
            t::DESC_EMPTY_ALL.to_string()
        }
    } else if query.has_filters() {
        fill_one(t::DESC_FILTERED_TEMPLATE, total_posts)
    } else {
        fill_one(t::DESC_ALL_TEMPLATE, total_posts)
    };

    let toggle_year = {
        let expanded_years = expanded_years.clone();
        Callback::from(move |year: i32| {
            expanded_years.set({
                let mut map = (*expanded_years).clone();
                let next = !map.get(&year).copied().unwrap_or(false);
                map.insert(year, next);
                map
            });
        })
    };

    let expanded_years_serialized = {
        let mut years = (*expanded_years)
            .iter()
            .filter_map(|(year, expanded)| if *expanded { Some(*year) } else { None })
            .collect::<Vec<_>>();
        years.sort_unstable();
        years
            .into_iter()
            .map(|year| year.to_string())
            .collect::<Vec<_>>()
            .join(",")
    };

    {
        let location_dep = location.clone();
        let tag = query.tag.clone();
        let category = query.category.clone();
        let expanded_years_serialized = expanded_years_serialized.clone();
        let total_posts = total_posts;
        use_effect_with(
            (
                location_dep.clone(),
                tag.clone(),
                category.clone(),
                expanded_years_serialized.clone(),
                total_posts,
            ),
            move |_| {
                let persist = move || {
                    if crate::navigation_context::is_return_armed() {
                        return;
                    }
                    let mut state = std::collections::BTreeMap::new();
                    if let Some(value) = tag.as_ref() {
                        state.insert("tag".to_string(), value.clone());
                    }
                    if let Some(value) = category.as_ref() {
                        state.insert("category".to_string(), value.clone());
                    }
                    if !expanded_years_serialized.is_empty() {
                        state.insert(
                            "expanded_years".to_string(),
                            expanded_years_serialized.clone(),
                        );
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
            },
        );
    }

    {
        let location_dep = location.clone();
        let expanded_years = expanded_years.clone();
        let total_posts = total_posts;
        use_effect_with((location_dep, total_posts), move |_| {
            if total_posts > 0 {
                if let Some(context) =
                    crate::navigation_context::pop_context_if_armed_for_current_page()
                {
                    if let Some(raw) = context.page_state.get("expanded_years") {
                        let mut restored = HashMap::<i32, bool>::new();
                        for value in raw.split(',') {
                            if let Ok(year) = value.parse::<i32>() {
                                restored.insert(year, true);
                            }
                        }
                        if !restored.is_empty() {
                            expanded_years.set(restored);
                        }
                    }
                    let scroll_y = context.scroll_y.max(0.0);
                    Timeout::new(150, move || {
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
                        "leading-tight",
                        "tag-title"
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
                        { description }
                    </p>

                    // Filter badge if active
                    {
                        if query.has_filters() {
                            html! {
                                <div class={classes!(
                                    "flex",
                                    "flex-wrap",
                                    "gap-3",
                                    "items-center",
                                    "justify-center",
                                    "mt-8"
                                )}>
                                    <span class={classes!(
                                        "tag-badge",
                                        "inline-flex",
                                        "items-center",
                                        "gap-2",
                                        "px-6",
                                        "py-3",
                                        "bg-gradient-to-r",
                                        "from-[var(--primary)]",
                                        "to-[var(--link)]",
                                        "text-white",
                                        "rounded-full",
                                        "shadow-[var(--shadow-8)]",
                                        "text-sm",
                                        "font-semibold",
                                        "tracking-wide"
                                    )}>
                                        <i class={classes!("fas", "fa-filter")}></i>
                                        { filter_label }
                                    </span>
                                    <Link<Route> to={Route::Posts} classes={classes!(
                                        "btn-fluent-secondary",
                                        "!rounded-full"
                                    )}>
                                        <i class={classes!("fas", "fa-times", "mr-1")}></i>
                                        { t::FILTER_CLEAR }
                                    </Link<Route>>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }
                </div>

                // Article Timeline Section
                {
                    if grouped_by_year.is_empty() {
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
                            <div class={classes!("editorial-timeline")}>
                                { for grouped_by_year.iter().map(|(year, posts)| {
                                    let year_value = *year;
                                    let total_count = posts.len();
                                    let should_collapse = total_count > 20;
                                    let is_expanded = (*expanded_years).get(&year_value).copied().unwrap_or(false);
                                    let visible_count = if should_collapse && !is_expanded {
                                        total_count.min(10)
                                    } else {
                                        total_count
                                    };
                                    let remaining = total_count.saturating_sub(visible_count);

                                    html! {
                                        <div class={classes!("timeline-year-section", "mb-16")}>
                                            // Year Header
                                            <div class={classes!(
                                                "flex",
                                                "items-center",
                                                "gap-4",
                                                "mb-8"
                                            )}>
                                                <div class={classes!(
                                                    "year-label",
                                                    "text-4xl",
                                                    "md:text-5xl",
                                                    "font-bold",
                                                    "text-[var(--text)]",
                                                    "tracking-tight"
                                                )}
                                                style="font-family: 'Fraunces', serif;">
                                                    { year_value }
                                                </div>
                                                <div class={classes!(
                                                    "flex-1",
                                                    "h-[2px]",
                                                    "bg-gradient-to-r",
                                                    "from-[var(--border)]",
                                                    "to-transparent"
                                                )}></div>
                                                <div class={classes!(
                                                    "px-4",
                                                    "py-1",
                                                    "rounded-full",
                                                    "bg-[var(--surface-alt)]",
                                                    "text-[var(--muted)]",
                                                    "text-sm",
                                                    "font-medium"
                                                )}>
                                                    { fill_one(t::YEAR_COUNT_TEMPLATE, total_count) }
                                                </div>
                                            </div>

                                            // Articles Grid
                                            <div class={classes!(
                                                "articles-grid",
                                                "grid",
                                                "grid-cols-1",
                                                "md:grid-cols-2",
                                                "lg:grid-cols-3",
                                                "gap-6"
                                            )}>
                                                { for posts.iter().take(visible_count).map(|article| {
                                                    let detail_route = Route::ArticleDetail { id: article.id.clone() };
                                                    html! {
                                                        <Link<Route>
                                                            to={detail_route}
                                                            classes={classes!("article-card-editorial", "block")}
                                                        >
                                                            <article class={classes!(
                                                                "article-card",
                                                                "bg-[var(--surface)]",
                                                                "liquid-glass",
                                                                "rounded-xl",
                                                                "border",
                                                                "border-[var(--border)]",
                                                                "p-6",
                                                                "h-full",
                                                                "flex",
                                                                "flex-col",
                                                                "gap-3",
                                                                "transition-all",
                                                                "duration-300"
                                                            )}>
                                                                // Date badge
                                                                <time class={classes!(
                                                                    "text-xs",
                                                                    "tracking-[0.2em]",
                                                                    "uppercase",
                                                                    "text-[var(--muted)]",
                                                                    "font-semibold"
                                                                )}>
                                                                    { format_month_day(&article.date) }
                                                                </time>

                                                                // Title
                                                                <h3 class={classes!(
                                                                    "article-card-title",
                                                                    "text-xl",
                                                                    "font-bold",
                                                                    "text-[var(--text)]",
                                                                    "leading-tight",
                                                                    "line-clamp-2"
                                                                )}
                                                                style="font-family: 'Fraunces', serif;">
                                                                    { article.title.clone() }
                                                                </h3>

                                                                // Summary if exists
                                                                {
                                                                    if !article.summary.is_empty() {
                                                                        html! {
                                                                            <p class={classes!(
                                                                                "text-sm",
                                                                                "text-[var(--muted)]",
                                                                                "leading-relaxed",
                                                                                "line-clamp-3",
                                                                                "flex-1"
                                                                            )}>
                                                                                { &article.summary }
                                                                            </p>
                                                                        }
                                                                    } else {
                                                                        html! {}
                                                                    }
                                                                }

                                                                // Tags
                                                                {
                                                                    if !article.tags.is_empty() {
                                                                        html! {
                                                                            <div class={classes!(
                                                                                "flex",
                                                                                "flex-wrap",
                                                                                "gap-2",
                                                                                "mt-auto"
                                                                            )}>
                                                                                { for article.tags.iter().take(3).map(|tag| {
                                                                                    html! {
                                                                                        <span class={classes!(
                                                                                            "text-xs",
                                                                                            "px-2",
                                                                                            "py-1",
                                                                                            "rounded",
                                                                                            "bg-[var(--surface-alt)]",
                                                                                            "text-[var(--muted)]",
                                                                                            "border",
                                                                                            "border-[var(--border)]"
                                                                                        )}>
                                                                                            { format!("#{}", tag) }
                                                                                        </span>
                                                                                    }
                                                                                }) }
                                                                            </div>
                                                                        }
                                                                    } else {
                                                                        html! {}
                                                                    }
                                                                }
                                                            </article>
                                                        </Link<Route>>
                                                    }
                                                }) }
                                            </div>

                                            // Expand/Collapse Button
                                            {
                                                if should_collapse {
                                                    let button_label = if is_expanded {
                                                        t::COLLAPSE.to_string()
                                                    } else {
                                                        fill_one(t::EXPAND_REMAINING_TEMPLATE, remaining)
                                                    };
                                                    let toggle_cb = toggle_year.clone();
                                                    let year_for_toggle = year_value;
                                                    let onclick = Callback::from(move |_| toggle_cb.emit(year_for_toggle));
                                                    html! {
                                                        <div class={classes!("text-center", "mt-8")}>
                                                            <button
                                                                type="button"
                                                                class={classes!(
                                                                    "btn-fluent-ghost",
                                                                    "!rounded-full",
                                                                    "px-8"
                                                                )}
                                                                {onclick}
                                                                aria-expanded={is_expanded.to_string()}
                                                            >
                                                                { button_label }
                                                                <i class={classes!(
                                                                    "fas",
                                                                    if is_expanded { "fa-chevron-up" } else { "fa-chevron-down" },
                                                                    "ml-2"
                                                                )}></i>
                                                            </button>
                                                        </div>
                                                    }
                                                } else {
                                                    Html::default()
                                                }
                                            }
                                        </div>
                                    }
                                }) }
                            </div>
                        }
                    }
                }
            </div>
            <ScrollToTopButton />
        </main>
    }
}

#[allow(dead_code)]
pub(crate) fn render_timeline(grouped_by_year: &[(i32, Vec<ArticleListItem>)]) -> Html {
    render_timeline_with_state(grouped_by_year, None, None)
}

#[allow(dead_code)]
pub(crate) fn render_expandable_timeline(
    grouped_by_year: &[(i32, Vec<ArticleListItem>)],
    expanded_years: &HashMap<i32, bool>,
    toggle_year: &Callback<i32>,
) -> Html {
    render_timeline_with_state(grouped_by_year, Some(expanded_years), Some(toggle_year))
}

#[allow(dead_code)]
fn render_timeline_with_state(
    grouped_by_year: &[(i32, Vec<ArticleListItem>)],
    expanded_years: Option<&HashMap<i32, bool>>,
    toggle_year: Option<&Callback<i32>>,
) -> Html {
    html! {
        <>
            { for grouped_by_year.iter().map(|(year, posts)| {
                let year_value = *year;
                let total_count = posts.len();
                let collapse_enabled = expanded_years.is_some() && toggle_year.is_some();
                let should_collapse = collapse_enabled && total_count > 20;
                let is_expanded = if collapse_enabled {
                    expanded_years
                        .and_then(|map| map.get(&year_value).copied())
                        .unwrap_or(false)
                } else {
                    true
                };
                let visible_count = if should_collapse && !is_expanded {
                    total_count.min(10)
                } else {
                    total_count
                };
                        let remaining = total_count.saturating_sub(visible_count);
                        html! {
                            <>
                                <h3 class={classes!(
                                    "mt-10",
                                    "mb-5",
                                    "text-2xl",
                                    "font-bold",
                                    "text-[var(--text)]"
                                )}>{ year_value }</h3>
                        <div class={classes!("timeline")}>
                            { for posts.iter().take(visible_count).map(|article| {
                                let detail_route = Route::ArticleDetail { id: article.id.clone() };
                                html! {
                                    <div class={classes!("circle")}>
                                        <div class={classes!(
                                            "m-0",
                                            "leading-relaxed",
                                            "pl-[calc(var(--timeline-offset)+1rem)]"
                                        )}>
                                            <Link<Route> to={detail_route} classes={classes!(
                                                "text-[1.1rem]",
                                                "font-semibold",
                                                "text-[var(--text)]",
                                                "transition-colors",
                                                "duration-200",
                                                "hover:text-[var(--primary)]"
                                            )}>
                                                { article.title.clone() }
                                            </Link<Route>>
                                        </div>
                                        <div class={classes!(
                                            "m-0",
                                            "leading-relaxed",
                                            "pl-[calc(var(--timeline-offset)+1rem)]"
                                        )}>
                                            <span class={classes!(
                                                "inline-block",
                                                "mt-1",
                                                "text-[0.9rem]",
                                                "tracking-[0.2em]",
                                                "uppercase",
                                                "text-[var(--muted)]"
                                            )}>
                                                { fill_one(t::PUBLISHED_ON_TEMPLATE, format_month_day(&article.date)) }
                                            </span>
                                        </div>
                                    </div>
                                }
                            }) }
                        </div>
                        {
                            if should_collapse {
                                let button_label = if is_expanded {
                                    t::COLLAPSE.to_string()
                                } else {
                                    fill_one(t::EXPAND_REMAINING_TEMPLATE, remaining)
                                };
                                if let Some(toggle_cb) = toggle_year {
                                    let toggle_cb = toggle_cb.clone();
                                    let year_for_toggle = year_value;
                                    let onclick = Callback::from(move |_| toggle_cb.emit(year_for_toggle));
                                    html! {
                                        <button
                                            type="button"
                                            class={classes!("btn-fluent-ghost", "mt-3")}
                                            {onclick}
                                            aria-expanded={is_expanded.to_string()}
                                            aria-label={fill_one(t::YEAR_TOGGLE_ARIA_TEMPLATE, year_value)}
                                        >
                                            { button_label }
                                        </button>
                                    }
                                } else {
                                    Html::default()
                                }
                            } else {
                                Html::default()
                            }
                        }
                    </>
                }
            }) }
        </>
    }
}

pub(crate) fn group_articles_by_year(
    articles: &[ArticleListItem],
) -> Vec<(i32, Vec<ArticleListItem>)> {
    let mut map: BTreeMap<i32, Vec<ArticleListItem>> = BTreeMap::new();
    for article in articles {
        if let Some(year) = extract_year(&article.date) {
            map.entry(year).or_default().push(article.clone());
        }
    }

    for posts in map.values_mut() {
        posts.sort_by(|a, b| b.date.cmp(&a.date));
    }

    map.into_iter().rev().collect()
}

fn extract_year(date: &str) -> Option<i32> {
    date.split('-').next()?.parse().ok()
}

pub(crate) fn format_month_day(date: &str) -> String {
    let mut parts = date.split('-');
    let _ = parts.next();
    match (parts.next(), parts.next()) {
        (Some(month), Some(day)) => format!("{month}-{day}"),
        _ => date.to_string(),
    }
}
