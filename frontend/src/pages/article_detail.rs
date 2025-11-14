use wasm_bindgen::{JsCast, JsValue};
use web_sys::window;
use yew::{prelude::*, virtual_dom::AttrValue};
use yew_router::prelude::{use_route, Link};

use crate::{models::get_mock_article_detail, router::Route, utils::markdown_to_html};

#[derive(Properties, Clone, PartialEq)]
pub struct ArticleDetailProps {
    #[prop_or_default]
    pub id: String,
}

#[function_component(ArticleDetailPage)]
pub fn article_detail_page(props: &ArticleDetailProps) -> Html {
    let route = use_route::<Route>();
    let article_id = route
        .as_ref()
        .and_then(|r| match r {
            Route::ArticleDetail {
                id,
            } => Some(id.clone()),
            _ => None,
        })
        .unwrap_or_else(|| props.id.clone());

    let article = {
        let article_id = article_id.clone();
        use_memo(article_id, move |id| get_mock_article_detail(id.as_str()))
    };

    let article_data = (*article).clone();

    // Initialize markdown rendering (syntax highlighting + math formulas) after
    // content is rendered
    use_effect_with(article_id.clone(), |_| {
        if let Some(win) = window() {
            if let Ok(init_fn) =
                js_sys::Reflect::get(&win, &JsValue::from_str("initMarkdownRendering"))
            {
                if let Ok(func) = init_fn.dyn_into::<js_sys::Function>() {
                    let _ = func.call0(&win);
                }
            }
        }
        || ()
    });

    let body = if let Some(article) = article_data {
        let word_count = article
            .content
            .chars()
            .filter(|c| !c.is_whitespace())
            .count();
        let render_html = markdown_to_html(&article.content);
        let content = Html::from_html_unchecked(AttrValue::from(render_html));

        html! {
            <article class="article-detail">
                {
                    if let Some(image) = article.featured_image.clone() {
                        html! {
                            <div class="article-featured">
                                <img src={image} alt={article.title.clone()} loading="lazy" />
                            </div>
                        }
                    } else {
                        html! {}
                    }
                }

                <header class="article-header fade-in">
                    <Link<Route>
                        to={Route::CategoryDetail { category: article.category.clone() }}
                        classes={classes!("article-category")}
                    >
                        { article.category.clone() }
                    </Link<Route>>
                    <h1 class="article-title">
                        { article.title.clone() }
                    </h1>
                    <div class="article-meta" aria-label="文章元信息">
                        <span class="article-meta-item">
                            <i class="fas fa-user-circle" aria-hidden="true"></i>
                            { article.author.clone() }
                        </span>
                        <span class="article-meta-item">
                            <i class="far fa-calendar-alt" aria-hidden="true"></i>
                            { article.date.clone() }
                        </span>
                        <Link<Route>
                            to={Route::CategoryDetail { category: article.category.clone() }}
                            classes={classes!("article-meta-item")}
                        >
                            <i class="far fa-folder-open" aria-hidden="true"></i>
                            { article.category.clone() }
                        </Link<Route>>
                        <span class="article-meta-item">
                            <i class="far fa-file-alt" aria-hidden="true"></i>
                            { format!("{} 字", word_count) }
                        </span>
                        <span class="article-meta-item">
                            <i class="far fa-clock" aria-hidden="true"></i>
                            { format!("约 {} 分钟", article.read_time) }
                        </span>
                    </div>
                </header>

                <section class="article-content" aria-label="文章正文">
                    { content }
                </section>

                <footer class="article-footer">
                    <h2 class="article-footer-title">{ "标签" }</h2>
                    <ul class="article-tags">
                        { for article.tags.iter().cloned().map(|tag| {
                            html! {
                                <li>
                                    <Link<Route>
                                        to={Route::TagDetail { tag: tag.clone() }}
                                        classes={classes!("article-tag-pill")}
                                    >
                                        { format!("#{}", tag) }
                                    </Link<Route>>
                                </li>
                            }
                        }) }
                    </ul>
                </footer>
            </article>
        }
    } else {
        html! {
            <section class="article-detail not-found">
                <div class="article-header fade-in">
                    <p class="article-category">{ "404" }</p>
                    <h1 class="article-title">{ "文章未找到" }</h1>
                    <p class="article-empty">{ "抱歉，没有找到对应的文章，请返回列表重试。" }</p>
                </div>
            </section>
        }
    };

    html! {
        <main class="main">
            <div class="container">
                { body }
            </div>
        </main>
    }
}
