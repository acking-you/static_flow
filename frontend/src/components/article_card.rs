use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{models::ArticleListItem, router::Route};

#[derive(Properties, PartialEq, Clone)]
pub struct ArticleCardProps {
    pub article: ArticleListItem,
}

#[function_component(ArticleCard)]
pub fn article_card(props: &ArticleCardProps) -> Html {
    let article = props.article.clone();
    let detail_route = Route::ArticleDetail {
        id: article.id.clone(),
    };

    html! {
        <article class="article-card">
            {
                if let Some(image) = article.featured_image.as_ref() {
                    html! {
                        <Link<Route> to={detail_route.clone()} classes={classes!("featured-image")}>
                            <img src={image.clone()} alt={article.title.clone()} loading="lazy" />
                        </Link<Route>>
                    }
                } else {
                    html! {}
                }
            }
            <h3 class="article-title">
                <Link<Route> to={detail_route.clone()} classes={classes!("article-title-link")}>
                    { &article.title }
                </Link<Route>>
            </h3>
            <div class="post-meta">
                <span class="post-meta-item">
                    <i class="fas fa-user-circle" aria-hidden="true"></i>
                    { &article.author }
                </span>
                <span class="post-meta-item">
                    <i class="far fa-calendar-alt" aria-hidden="true"></i>
                    { &article.date }
                </span>
                <Link<Route>
                    to={Route::CategoryDetail { category: article.category.clone() }}
                    classes={classes!("post-meta-item", "post-category")}
                >
                    <i class="far fa-folder" aria-hidden="true"></i>
                    { &article.category }
                </Link<Route>>
            </div>
            <p class="article-excerpt">{ &article.summary }</p>
            <div class="post-footer">
                <ul class="post-tags">
                    { for article.tags.iter().map(|tag| {
                        let tag_route = Route::TagDetail { tag: tag.clone() };
                        let tag_label = format!("#{}", tag);
                        html! {
                            <li>
                                <Link<Route>
                                    to={tag_route}
                                    classes={classes!("tag-pill")}
                                >
                                    { tag_label }
                                </Link<Route>>
                            </li>
                        }
                    }) }
                </ul>
            </div>
        </article>
    }
}
