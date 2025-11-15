use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{
        article_card::ArticleCard,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        pagination::Pagination,
        stats_card::StatsCard,
    },
    hooks::{use_pagination, use_scroll_to_top},
    router::Route,
};
use static_flow_shared::ArticleListItem;

#[function_component(HomePage)]
pub fn home_page() -> Html {
    use_scroll_to_top();

    let articles = use_state(|| Vec::<ArticleListItem>::new());
    let loading = use_state(|| true);

    let (visible_articles, current_page, total_pages, go_to_page) =
        use_pagination((*articles).clone(), 12);

    {
        let articles = articles.clone();
        let loading = loading.clone();
        use_effect_with((), move |_| {
            loading.set(true);
            let articles = articles.clone();
            let loading = loading.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_articles(None, None).await {
                    Ok(data) => articles.set(data),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch articles: {}", e).into());
                    }
                }
                loading.set(false);
            });
            || ()
        });
    }

    let total_articles = articles.len();
    let stats = vec![
        ("📝".to_string(), total_articles.to_string(), Some("/posts".to_string())),
        ("🏷️".to_string(), "12".to_string(), Some("/tags".to_string())),
        ("📂".to_string(), "5".to_string(), Some("/categories".to_string())),
    ];

    let tech_stack = vec![
        ("🦀 Rust", "https://doc.rust-lang.org/book"),
        ("⚛️ Yew", "https://yew.rs/docs/getting-started/introduction"),
        ("🎨 Tailwind", "https://tailwindcss.com/docs"),
        ("🔍 Meilisearch", "https://www.meilisearch.com/docs"),
        ("🚀 WebAssembly", "https://webassembly.org/getting-started/developers-guide"),
    ];

    let pagination_controls = if total_pages > 1 {
        html! {
            <div class="mt-10 flex justify-center">
                <Pagination
                    current_page={current_page}
                    total_pages={total_pages}
                    on_page_change={go_to_page.clone()}
                />
            </div>
        }
    } else {
        Html::default()
    };

    let article_grid = if *loading {
        html! {
            <div class="flex items-center justify-center" style="min-height: 400px;">
                <LoadingSpinner size={SpinnerSize::Large} />
            </div>
        }
    } else if visible_articles.is_empty() {
        html! { <p class="text-center text-muted">{ "暂无文章" }</p> }
    } else {
        html! {
            <>
                <div class="summary-card">
                    { for visible_articles.iter().map(|article| {
                        html! { <ArticleCard article={article.clone()} /> }
                    }) }
                </div>
                { pagination_controls }
            </>
        }
    };

    html! {
        <main class="main">
            <div class="container">
                <div class="content page-enter">
                    <section class="hero-section">
                        <svg class="hero-wave hero-wave-top" viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                            <path d="M0,40 C240,120 360,0 720,60 C1080,120 1200,20 1440,60 L1440,0 L0,0 Z" fill="rgba(29, 158, 216, 0.08)" />
                        </svg>
                        <div class="home-profile">
                            <div class="home-avatar">
                                <Link<Route>
                                    to={Route::Posts}
                                    classes={classes!("home-avatar-link")}
                                >
                                    <img src="/static/avatar.jpg" alt="作者头像" loading="lazy" />
                                    <span class="visually-hidden">{ "前往文章列表" }</span>
                                </Link<Route>>
                            </div>
                            <h1 class="home-title">
                                { "学习如逆水行舟，不进则退！" }
                            </h1>
                            <p class="home-subtitle">
                                { "本地优先的写作实验室，记录 Rust · 自动化 · 创作思考。" }
                            </p>
                            <div class="social-links" aria-label="社交链接">
                                <a
                                    href="https://github.com/ACking-you"
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    aria-label="GitHub"
                                >
                                    <i class="fa-brands fa-github-alt" aria-hidden="true"></i>
                                    <span class="visually-hidden">{ "GitHub" }</span>
                                </a>
                                <a
                                    href="https://space.bilibili.com/24264499"
                                    target="_blank"
                                    rel="noopener noreferrer"
                                    aria-label="Bilibili"
                                >
                                    <svg
                                        viewBox="0 0 24 24"
                                        role="img"
                                        aria-hidden="true"
                                        focusable="false"
                                        width="22"
                                        height="22"
                                    >
                                        <path
                                            fill="currentColor"
                                            d="M17.813 4.653h.854c1.51.054 2.769.578 3.773 1.574 1.004.995 1.524 2.249 1.56 3.76v7.36c-.036 1.51-.556 2.769-1.56 3.773s-2.262 1.524-3.773 1.56H5.333c-1.51-.036-2.769-.556-3.773-1.56S.036 18.858 0 17.347v-7.36c.036-1.511.556-2.765 1.56-3.76 1.004-.996 2.262-1.52 3.773-1.574h.774l-1.174-1.12a1.234 1.234 0 0 1-.373-.906c0-.356.124-.658.373-.907l.027-.027c.267-.249.573-.373.92-.373.347 0 .653.124.92.373L9.653 4.44c.071.071.134.142.187.213h4.267a.836.836 0 0 1 .16-.213l2.853-2.747c.267-.249.573-.373.92-.373.347 0 .662.151.929.4.267.249.391.551.391.907 0 .355-.124.657-.373.906zM5.333 7.24c-.746.018-1.373.276-1.88.773-.506.498-.769 1.13-.786 1.894v7.52c.017.764.28 1.395.786 1.893.507.498 1.134.756 1.88.773h13.334c.746-.017 1.373-.275 1.88-.773.506-.498.769-1.129.786-1.893v-7.52c-.017-.765-.28-1.396-.786-1.894-.507-.497-1.134-.755-1.88-.773zM8 11.107c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c0-.373.129-.689.386-.947.258-.257.574-.386.947-.386zm8 0c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c.017-.391.15-.711.4-.96.249-.249.56-.373.933-.373Z"
                                        />
                                    </svg>
                                    <span class="visually-hidden">{ "Bilibili" }</span>
                                </a>
                            </div>
                        </div>
                        <div class="hero-stats-grid">
                            { for stats.into_iter().map(|(icon, value, href)| html! {
                                <StatsCard icon={icon} value={value} href={href} />
                            }) }
                        </div>
                        <div class="tech-stack">
                            <p class="tech-stack-title">{ "技术栈" }</p>
                            <div class="tech-stack-tags">
                                { for tech_stack.iter().map(|(label, href)| html! {
                                    <a class="tech-tag" href={(*href).to_string()} target="_blank" rel="noopener noreferrer">{ *label }</a>
                                }) }
                            </div>
                        </div>
                        <svg class="hero-wave hero-wave-bottom" viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                            <path d="M0,80 C200,20 320,120 720,60 C1120,0 1240,80 1440,40 L1440,120 L0,120 Z" fill="var(--bg)" />
                        </svg>
                    </section>

                    <section class="article-list-section" aria-label="文章列表">
                        <div class="section-title-with-bg">
                            <h2>{ "最新文章" }</h2>
                            <p>{ "甄选近期发布的内容，持续更新" }</p>
                        </div>
                        { article_grid }
                    </section>
                </div>
            </div>
        </main>
    }
}
