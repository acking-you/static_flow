use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::{footer::Footer, header::Header},
    pages,
};

#[derive(Routable, Clone, PartialEq, Debug)]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/posts")]
    Posts,
    #[at("/posts/:id")]
    ArticleDetail { id: String },
    #[at("/tags")]
    Tags,
    #[at("/tags/:tag")]
    TagDetail { tag: String },
    #[at("/categories")]
    Categories,
    #[at("/categories/:category")]
    CategoryDetail { category: String },
    #[not_found]
    #[at("/404")]
    NotFound,
}

fn switch(route: Route) -> Html {
    match route {
        Route::Home => html! { <pages::home::HomePage /> },
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
        Route::NotFound => html! { <pages::not_found::NotFoundPage /> },
    }
}

#[function_component(AppRouter)]
pub fn app_router() -> Html {
    html! {
        <BrowserRouter>
            <div class="app-shell">
                <Header />
                <div class="app-content">
                    <Switch<Route> render={switch} />
                </div>
                <Footer />
            </div>
        </BrowserRouter>
    }
}
