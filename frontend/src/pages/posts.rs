use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use yew::prelude::*;
use yew_router::prelude::{use_location, Link};

use crate::{
    models::{get_mock_articles, ArticleListItem},
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
        .and_then(|loc| loc.query::<PostsQuery>().ok())
        .unwrap_or_default()
        .normalized();

    let articles = use_memo((), |_| {
        let mut list = get_mock_articles();
        list.sort_by(|a, b| b.date.cmp(&a.date));
        list
    });

    let mut filtered = (*articles).clone();
    if let Some(tag) = query.tag.as_ref() {
        let tag_lower = tag.to_lowercase();
        filtered.retain(|article| article.tags.iter().any(|t| t.to_lowercase() == tag_lower));
    }
    if let Some(category) = query.category.as_ref() {
        filtered.retain(|article| article.category.eq_ignore_ascii_case(category));
    }

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
                            render_timeline(&grouped_by_year)
                        }
                    }
                </div>
            </div>
        </main>
    }
}

pub(crate) fn render_timeline(grouped_by_year: &[(i32, Vec<ArticleListItem>)]) -> Html {
    html! {
        <>
            { for grouped_by_year.iter().map(|(year, posts)| {
                let year_value = *year;
                html! {
                    <>
                        <h3 class="group-title">{ year_value }</h3>
                        <div class="timeline">
                            { for posts.iter().cloned().map(|article| {
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
