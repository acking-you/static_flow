use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::{footer::Footer, header::Header, spotlight::Spotlight},
    pages,
};

#[derive(Routable, Clone, PartialEq, Debug)]
pub enum Route {
    #[cfg(not(feature = "mock"))]
    #[at("/")]
    Home,
    #[cfg(feature = "mock")]
    #[at("/static_flow/")]
    Home,

    #[cfg(not(feature = "mock"))]
    #[at("/latest")]
    LatestArticles,
    #[cfg(feature = "mock")]
    #[at("/static_flow/latest")]
    LatestArticles,

    #[cfg(not(feature = "mock"))]
    #[at("/posts")]
    Posts,
    #[cfg(feature = "mock")]
    #[at("/static_flow/posts")]
    Posts,

    #[cfg(not(feature = "mock"))]
    #[at("/posts/:id")]
    ArticleDetail { id: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/posts/:id")]
    ArticleDetail { id: String },

    #[cfg(not(feature = "mock"))]
    #[at("/tags")]
    Tags,
    #[cfg(feature = "mock")]
    #[at("/static_flow/tags")]
    Tags,

    #[cfg(not(feature = "mock"))]
    #[at("/tags/:tag")]
    TagDetail { tag: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/tags/:tag")]
    TagDetail { tag: String },

    #[cfg(not(feature = "mock"))]
    #[at("/categories")]
    Categories,
    #[cfg(feature = "mock")]
    #[at("/static_flow/categories")]
    Categories,

    #[cfg(not(feature = "mock"))]
    #[at("/categories/:category")]
    CategoryDetail { category: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/categories/:category")]
    CategoryDetail { category: String },

    #[cfg(not(feature = "mock"))]
    #[at("/search")]
    Search,
    #[cfg(feature = "mock")]
    #[at("/static_flow/search")]
    Search,

    #[not_found]
    #[cfg(not(feature = "mock"))]
    #[at("/404")]
    NotFound,
    #[not_found]
    #[cfg(feature = "mock")]
    #[at("/static_flow/404")]
    NotFound,
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! { <pages::home::HomePage /> },
        Route::LatestArticles => html! { <pages::latest_articles::LatestArticlesPage /> },
        Route::Posts => html! { <pages::PostsPage /> },
        Route::ArticleDetail {
            id,
        } => {
            html! { <pages::article_detail::ArticleDetailPage id={id} /> }
        },
        Route::Tags => html! { <pages::tags::TagsPage /> },
        Route::TagDetail {
            tag,
        } => {
            html! { <pages::tag_detail::TagDetailPage tag={tag} /> }
        },
        Route::Categories => html! { <pages::categories::CategoriesPage /> },
        Route::CategoryDetail {
            category,
        } => {
            html! { <pages::category_detail::CategoryDetailPage category={category} /> }
        },
        Route::Search => html! { <pages::search::SearchPage /> },
        Route::NotFound => html! { <pages::not_found::NotFoundPage /> },
    }
}

#[function_component(AppRouter)]
pub fn app_router() -> Html {
    html! {
        <BrowserRouter>
            <AppRouterInner />
        </BrowserRouter>
    }
}

#[function_component(AppRouterInner)]
fn app_router_inner() -> Html {
    let location = use_location();

    // 判断是否在文章详情页（不显示Spotlight）
    let show_spotlight = location
        .as_ref()
        .map(|loc| !loc.path().contains("/posts/"))
        .unwrap_or(true);

    html! {
        <div class="flex flex-col bg-[var(--bg)]" style="min-height: 100vh; min-height: 100svh;">
            if show_spotlight {
                <Spotlight />
            }
            <Header />
            <div class="flex-1 pt-[var(--space-sm)]">
                <Switch<Route> render={switch} />
            </div>
            <Footer />
        </div>
    }
}
