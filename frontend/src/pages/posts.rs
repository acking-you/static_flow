use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::{use_location, Link};
use static_flow_shared::ArticleListItem;

use crate::router::Route;

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
        .and_then(|loc| loc.query::<PostsQuery>().ok())
        .unwrap_or_default()
        .normalized();

    let articles = use_state(|| Vec::<ArticleListItem>::new());
    let expanded_years = use_state(|| HashMap::<i32, bool>::new());

    {
        let articles = articles.clone();
        let tag = query.tag.clone();
        let category = query.category.clone();

        use_effect_with((tag.clone(), category.clone()), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                let tag_ref = tag.as_deref();
                let category_ref = category.as_deref();

                match crate::api::fetch_articles(tag_ref, category_ref).await {
                    Ok(data) => articles.set(data),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch articles: {}", e).into());
                    }
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
            "当前筛选下暂无文章，换个标签或分类试试？".to_string()
        } else {
            "暂时还没有文章，敬请期待。".to_string()
        }
    } else if query.has_filters() {
        format!("共找到 {} 篇文章匹配当前筛选。", total_posts)
    } else {
        format!("现在共有 {} 篇文章，按年份倒序排列。", total_posts)
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

    html! {
        <main class="main posts-page">
            <div class="container">
                <div class="page archive">
                    <p class="page-kicker">{ "Posts" }</p>
                    <h1 class="single-title">{ "文章时间线" }</h1>
                    <p class="page-description">{ description }</p>

                    {
                        if query.has_filters() {
                            html! {
                                <div class="post-filter-bar">
                                    <span class="filter-chip">
                                        <i class="fas fa-filter" aria-hidden="true"></i>
                                        { format!("当前筛选：{}", filter_label) }
                                    </span>
                                    <Link<Route> to={Route::Posts} classes={classes!("btn", "btn-soft")}>
                                        { "清除筛选" }
                                    </Link<Route>>
                                </div>
                            }
                        } else {
                            html! {}
                        }
                    }

                    {
                        if grouped_by_year.is_empty() {
                            html! {
                                <p class="timeline-empty">{ "暂无文章可展示。" }</p>
                            }
                        } else {
                            render_expandable_timeline(&grouped_by_year, &*expanded_years, &toggle_year)
                        }
                    }
                </div>
            </div>
        </main>
    }
}

pub(crate) fn render_timeline(grouped_by_year: &[(i32, Vec<ArticleListItem>)]) -> Html {
    render_timeline_with_state(grouped_by_year, None, None)
}

pub(crate) fn render_expandable_timeline(
    grouped_by_year: &[(i32, Vec<ArticleListItem>)],
    expanded_years: &HashMap<i32, bool>,
    toggle_year: &Callback<i32>,
) -> Html {
    render_timeline_with_state(grouped_by_year, Some(expanded_years), Some(toggle_year))
}

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
                        <h3 class="group-title">{ year_value }</h3>
                        <div class="timeline">
                            { for posts.iter().take(visible_count).cloned().map(|article| {
                                let detail_route = Route::ArticleDetail { id: article.id.clone() };
                                html! {
                                    <div class="circle">
                                        <div class="item">
                                            <Link<Route> to={detail_route} classes={classes!("item-link")}>
                                                { article.title.clone() }
                                            </Link<Route>>
                                        </div>
                                        <div class="item">
                                            <span class="item-date">
                                                { format!("Published on {}", format_month_day(&article.date)) }
                                            </span>
                                        </div>
                                    </div>
                                }
                            }) }
                        </div>
                        {
                            if should_collapse {
                                let button_label = if is_expanded {
                                    "收起".to_string()
                                } else {
                                    format!("展开剩余 {} 篇", remaining)
                                };
                                if let Some(toggle_cb) = toggle_year {
                                    let toggle_cb = toggle_cb.clone();
                                    let year_for_toggle = year_value;
                                    let onclick = Callback::from(move |_| toggle_cb.emit(year_for_toggle));
                                    html! {
                                        <button
                                            type="button"
                                            class={classes!("btn", "btn-soft", "btn-expand")}
                                            {onclick}
                                            aria-expanded={is_expanded.to_string()}
                                            aria-label={format!("切换 {year_value} 年文章折叠状态")}
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
