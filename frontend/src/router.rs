use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::{footer::Footer, header::Header, spotlight::Spotlight},
    music_context::{MusicAction, MusicPlayerContext},
    pages,
};

fn is_article_detail_path(path: &str) -> bool {
    path.contains("/posts/")
}

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
    #[at("/posts/:id/raw/:lang")]
    ArticleRaw { id: String, lang: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/posts/:id/raw/:lang")]
    ArticleRaw { id: String, lang: String },

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

    #[cfg(not(feature = "mock"))]
    #[at("/admin")]
    Admin,
    #[cfg(feature = "mock")]
    #[at("/static_flow/admin")]
    Admin,

    #[cfg(not(feature = "mock"))]
    #[at("/admin/comments/runs/:task_id")]
    AdminCommentRuns { task_id: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/admin/comments/runs/:task_id")]
    AdminCommentRuns { task_id: String },

    #[cfg(not(feature = "mock"))]
    #[at("/admin/music-wishes/runs/:wish_id")]
    AdminMusicWishRuns { wish_id: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/admin/music-wishes/runs/:wish_id")]
    AdminMusicWishRuns { wish_id: String },

    #[cfg(not(feature = "mock"))]
    #[at("/admin/article-requests/runs/:request_id")]
    AdminArticleRequestRuns { request_id: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/admin/article-requests/runs/:request_id")]
    AdminArticleRequestRuns { request_id: String },

    #[cfg(not(feature = "mock"))]
    #[at("/media/video")]
    MediaVideo,
    #[cfg(feature = "mock")]
    #[at("/static_flow/media/video")]
    MediaVideo,

    #[cfg(not(feature = "mock"))]
    #[at("/media/audio")]
    MediaAudio,
    #[cfg(feature = "mock")]
    #[at("/static_flow/media/audio")]
    MediaAudio,

    #[cfg(not(feature = "mock"))]
    #[at("/media/audio/:id")]
    MusicPlayer { id: String },
    #[cfg(feature = "mock")]
    #[at("/static_flow/media/audio/:id")]
    MusicPlayer { id: String },

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
        Route::ArticleRaw {
            id,
            lang,
        } => {
            html! { <pages::article_raw::ArticleRawPage id={id} lang={lang} /> }
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
        Route::Admin => html! { <pages::admin::AdminPage /> },
        Route::AdminCommentRuns {
            task_id,
        } => {
            html! { <pages::admin_ai_stream::AdminCommentRunsPage task_id={task_id} /> }
        },
        Route::AdminMusicWishRuns {
            wish_id,
        } => {
            html! { <pages::admin_music_wish_stream::AdminMusicWishRunsPage wish_id={wish_id} /> }
        },
        Route::AdminArticleRequestRuns {
            request_id,
        } => {
            html! { <pages::admin_article_request_stream::AdminArticleRequestRunsPage request_id={request_id} /> }
        },
        Route::MediaVideo => html! { <pages::coming_soon::ComingSoonPage feature={"video"} /> },
        Route::MediaAudio => html! { <pages::music_library::MusicLibraryPage /> },
        Route::MusicPlayer {
            id,
        } => html! { <pages::music_player::MusicPlayerPage id={id} /> },
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
    let route = use_route::<Route>();

    // Auto-minimize player when navigating away from MusicPlayer page
    {
        let route = route.clone();
        let player_ctx = use_context::<MusicPlayerContext>();
        use_effect_with(route.clone(), move |route| {
            if let Some(ref ctx) = player_ctx {
                let is_player = matches!(route, Some(Route::MusicPlayer { .. }));
                if ctx.visible && !ctx.minimized && !is_player {
                    ctx.dispatch(MusicAction::Minimize);
                }
            }
            || ()
        });
    }

    {
        let route = route.clone();
        use_effect_with(route.clone(), move |active_route| {
            crate::seo::apply_route_seo(active_route.as_ref());
            || ()
        });
    }

    // 判断是否在文章详情页（不显示Spotlight）
    let show_spotlight = location
        .as_ref()
        .map(|loc| !is_article_detail_path(loc.path()))
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
            <crate::components::persistent_audio::PersistentAudio />
            <crate::components::mini_player::MiniPlayer />
        </div>
    }
}
