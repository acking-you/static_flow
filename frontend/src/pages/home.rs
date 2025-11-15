use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{components::article_card::ArticleCard, router::Route};
use static_flow_shared::ArticleListItem;

#[function_component(HomePage)]
pub fn home_page() -> Html {
    let articles = use_state(|| Vec::<ArticleListItem>::new());

    {
        let articles = articles.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_articles(None, None).await {
                    Ok(data) => articles.set(data),
                    Err(e) => {
                        web_sys::console::error_1(&format!("Failed to fetch articles: {}", e).into());
                    }
                }
            });
            || ()
        });
    }

    html! {
        <main class="main">
            <div class="container">
                <section class="home-profile">
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
                </section>

                <section class="summary-card" aria-label="文章列表">
                    { for articles.iter().map(|article| {
                        let id = article.id.clone();
                        html! {
                            <ArticleCard
                                key={id}
                                article={article.clone()}
                            />
                        }
                    }) }
                </section>
            </div>
        </main>
    }
}
