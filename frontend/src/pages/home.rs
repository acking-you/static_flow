use static_flow_shared::ArticleListItem;
use wasm_bindgen::JsCast;
use web_sys::{window, Element, TouchEvent};
use yew::prelude::*;
use yew_router::prelude::{use_location, Link};

use crate::{
    components::{
        article_card::ArticleCard,
        icons::IconName,
        loading_spinner::{LoadingSpinner, SpinnerSize},
        pagination::Pagination,
        stats_card::StatsCard,
        tooltip::{TooltipIconButton, TooltipPosition},
    },
    hooks::use_pagination,
    router::Route,
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum CurrentPage {
    Hero,
    Articles,
}

#[function_component(HomePage)]
pub fn home_page() -> Html {
    // Check URL hash on mount to restore state
    let initial_page = if let Some(win) = window() {
        let location = win.location();
        if let Ok(hash) = location.hash() {
            if hash == "#articles" {
                CurrentPage::Articles
            } else {
                CurrentPage::Hero
            }
        } else {
            CurrentPage::Hero
        }
    } else {
        CurrentPage::Hero
    };

    let current_page = use_state(|| initial_page);
    let route_location = use_location();
    let articles_scroll_ref = use_node_ref();

    // 手势滑动状态
    let touch_start_x = use_state(|| 0.0f64);
    let touch_start_y = use_state(|| 0.0f64);
    let touch_offset_x = use_state(|| 0.0f64);
    let is_swiping = use_state(|| false);
    let slider_ref = use_node_ref();

    let articles = use_state(|| Vec::<ArticleListItem>::new());
    let loading = use_state(|| true);

    let (visible_articles, current_page_num, total_pages, go_to_page) =
        use_pagination((*articles).clone(), 12);

    // Fetch articles
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
                        web_sys::console::error_1(
                            &format!("Failed to fetch articles: {}", e).into(),
                        );
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let switch_to_articles = {
        let current_page = current_page.clone();
        Callback::from(move |_: MouseEvent| {
            current_page.set(CurrentPage::Articles);
            // Update URL hash to create history entry
            if let Some(win) = window() {
                let location = win.location();
                let _ = location.set_hash("articles");
            }
        })
    };

    let switch_to_hero = {
        let current_page = current_page.clone();
        Callback::from(move |_: MouseEvent| {
            current_page.set(CurrentPage::Hero);
            // Clear URL hash
            if let Some(win) = window() {
                if let Ok(history) = win.history() {
                    // Use replaceState to avoid creating extra history entry
                    let home_path = crate::config::route_path("/");
                    let _ =
                        history.replace_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&home_path));
                }
            }
        })
    };

    let scroll_to_top = {
        let articles_scroll_ref = articles_scroll_ref.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let Some(container) = articles_scroll_ref.cast::<Element>() {
                container.set_scroll_top(0);
            }
        })
    };

    // 触摸开始：记录初始位置
    let on_touch_start = {
        let touch_start_x = touch_start_x.clone();
        let touch_start_y = touch_start_y.clone();
        let is_swiping = is_swiping.clone();
        Callback::from(move |e: TouchEvent| {
            if let Some(touch) = e.touches().item(0) {
                touch_start_x.set(touch.client_x() as f64);
                touch_start_y.set(touch.client_y() as f64);
                is_swiping.set(false);
            }
        })
    };

    // 触摸移动：计算偏移并实时更新
    let on_touch_move = {
        let touch_start_x = touch_start_x.clone();
        let touch_start_y = touch_start_y.clone();
        let touch_offset_x = touch_offset_x.clone();
        let is_swiping = is_swiping.clone();
        let slider_ref = slider_ref.clone();
        let current_page = current_page.clone();
        Callback::from(move |e: TouchEvent| {
            if let Some(touch) = e.touches().item(0) {
                let delta_x = touch.client_x() as f64 - *touch_start_x;
                let delta_y = (touch.client_y() as f64 - *touch_start_y).abs();

                // 只有横向滑动超过阈值且纵向偏移不大时才处理
                if delta_x.abs() > 10.0 && delta_y < 30.0 {
                    if !*is_swiping {
                        is_swiping.set(true);
                    }

                    // 边界限制
                    let offset = match *current_page {
                        CurrentPage::Hero if delta_x > 0.0 => 0.0, // 主页不能右滑
                        CurrentPage::Articles if delta_x < 0.0 => 0.0, // 文章页不能左滑
                        _ => delta_x,
                    };

                    touch_offset_x.set(offset);

                    // 实时更新 transform
                    if let Some(slider) = slider_ref.cast::<web_sys::HtmlElement>() {
                        let base_offset = match *current_page {
                            CurrentPage::Hero => 0.0,
                            CurrentPage::Articles => -100.0,
                        };
                        let percent_offset = (offset / slider.client_width() as f64) * 100.0;
                        let total_offset = base_offset + percent_offset;
                        let _ = slider.style().set_property(
                            "transform",
                            &format!("translateX({}%)", total_offset),
                        );
                    }

                    e.prevent_default();
                }
            }
        })
    };

    // 触摸结束：判断是否切换页面
    let on_touch_end = {
        let touch_offset_x = touch_offset_x.clone();
        let is_swiping = is_swiping.clone();
        let current_page = current_page.clone();
        let slider_ref = slider_ref.clone();
        Callback::from(move |_: TouchEvent| {
            if *is_swiping {
                let threshold = 50.0; // 滑动阈值（像素）
                let offset = *touch_offset_x;

                // 判断是否触发切换
                let should_switch = match *current_page {
                    CurrentPage::Hero => offset < -threshold, // 左滑切换到文章页
                    CurrentPage::Articles => offset > threshold, // 右滑切换到主页
                };

                if should_switch {
                    // 触发页面切换
                    if matches!(*current_page, CurrentPage::Hero) {
                        current_page.set(CurrentPage::Articles);
                        if let Some(win) = window() {
                            let location = win.location();
                            let _ = location.set_hash("articles");
                        }
                    } else {
                        current_page.set(CurrentPage::Hero);
                        if let Some(win) = window() {
                            if let Ok(history) = win.history() {
                                let home_path = crate::config::route_path("/");
                                let _ = history.replace_state_with_url(
                                    &wasm_bindgen::JsValue::NULL,
                                    "",
                                    Some(&home_path),
                                );
                            }
                        }
                    }
                }

                // 重置状态并移除 inline transform
                if let Some(slider) = slider_ref.cast::<web_sys::HtmlElement>() {
                    let _ = slider.style().remove_property("transform");
                }
                touch_offset_x.set(0.0);
                is_swiping.set(false);
            }
        })
    };

    let is_articles_page = matches!(*current_page, CurrentPage::Articles);

    // Save scroll position and page number before navigating to article detail
    let save_scroll_position = {
        let articles_scroll_ref = articles_scroll_ref.clone();
        let page_num = current_page_num;
        Callback::from(move |_| {
            if let Some(storage) = window().and_then(|w| w.session_storage().ok().flatten()) {
                // Save current page number
                let _ = storage.set_item("home_articles_page", &page_num.to_string());

                // Save scroll position
                if let Some(container) = articles_scroll_ref.cast::<Element>() {
                    let scroll_top = container.scroll_top();
                    let _ = storage.set_item("home_articles_scroll", &scroll_top.to_string());
                }
            }
        })
    };

    // Restore scroll position and page number when switching to articles view or
    // returning from article detail
    {
        let current_page = current_page.clone();
        let articles_scroll_ref = articles_scroll_ref.clone();
        let location_dep = route_location.clone();
        let go_to_page_cb = go_to_page.clone();
        use_effect_with((*current_page, location_dep), move |(page, _location)| {
            if matches!(*page, CurrentPage::Articles) {
                if let Some(storage) = window().and_then(|w| w.session_storage().ok().flatten()) {
                    let has_saved_data = storage
                        .get_item("home_articles_page")
                        .ok()
                        .flatten()
                        .is_some()
                        || storage
                            .get_item("home_articles_scroll")
                            .ok()
                            .flatten()
                            .is_some();

                    if has_saved_data {
                        // Restore page number first
                        if let Some(saved_page) =
                            storage.get_item("home_articles_page").ok().flatten()
                        {
                            if let Ok(page_num) = saved_page.parse::<usize>() {
                                go_to_page_cb.emit(page_num);
                            }
                        }

                        // Then restore scroll position
                        let scroll_pos = storage
                            .get_item("home_articles_scroll")
                            .ok()
                            .flatten()
                            .and_then(|v| v.parse::<i32>().ok())
                            .unwrap_or(0);

                        // Delay restore to ensure DOM is ready
                        let container_ref = articles_scroll_ref.clone();
                        if let Some(win) = window() {
                            let callback = wasm_bindgen::closure::Closure::once(move || {
                                if scroll_pos > 0 {
                                    if let Some(container) = container_ref.cast::<Element>() {
                                        container.set_scroll_top(scroll_pos);
                                    }
                                }
                                // Always clear saved data after restoration attempt
                                if let Some(storage) =
                                    window().and_then(|w| w.session_storage().ok().flatten())
                                {
                                    let _ = storage.remove_item("home_articles_scroll");
                                    let _ = storage.remove_item("home_articles_page");
                                }
                            });
                            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                                callback.as_ref().unchecked_ref(),
                                100, // 100ms delay
                            );
                            callback.forget();
                        }
                    }
                }
            }
            || ()
        });
    }

    let total_articles = articles.len();
    let stats = vec![
        (IconName::FileText, total_articles.to_string(), "文章".to_string(), Some(Route::Posts)),
        (IconName::Hash, "12".to_string(), "标签".to_string(), Some(Route::Tags)),
        (IconName::Folder, "5".to_string(), "分类".to_string(), Some(Route::Categories)),
    ];

    let tech_stack = vec![
        (
            crate::config::asset_path("static/logos/rust.svg"),
            "Rust",
            "https://doc.rust-lang.org/book",
        ),
        (
            crate::config::asset_path("static/logos/yew.svg"),
            "Yew",
            "https://yew.rs/docs/getting-started/introduction",
        ),
        (
            crate::config::asset_path("static/logos/tailwind.svg"),
            "Tailwind",
            "https://tailwindcss.com/docs",
        ),
        (
            crate::config::asset_path("static/logos/lancedb.png"),
            "LanceDB",
            "https://lancedb.com/docs/",
        ),
        (
            crate::config::asset_path("static/logos/wasm.ico"),
            "WebAssembly",
            "https://webassembly.org/getting-started/developers-guide",
        ),
    ];

    let pagination_controls = if total_pages > 1 {
        html! {
            <div class="mt-10 flex justify-center">
                <Pagination
                    current_page={current_page_num}
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
                        html! { <ArticleCard article={article.clone()} on_before_navigate={Some(save_scroll_position.clone())} /> }
                    }) }
                </div>
                { pagination_controls }
            </>
        }
    };

    // Page slider class
    let slider_class = if is_articles_page {
        "home-page-slider home-page-slider--articles"
    } else {
        "home-page-slider home-page-slider--hero"
    };

    html! {
        <div class="home-page-viewport">
            <div
                class={slider_class}
                ref={slider_ref}
                ontouchstart={on_touch_start}
                ontouchmove={on_touch_move}
                ontouchend={on_touch_end.clone()}
                ontouchcancel={on_touch_end}
            >
                // Hero Page
                <div class="home-page">
                    <div class="home-page-scroll">
                        <section class="hero-section">
                            <svg class="hero-wave hero-wave-top" viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                                <path d="M0,40 C240,120 360,0 720,60 C1080,120 1200,20 1440,60 L1440,0 L0,0 Z" fill="rgba(29, 158, 216, 0.08)" />
                            </svg>
                            <div class="container">
                                <div class="home-profile">
                                    <div class="home-avatar">
                                        <Link<Route>
                                            to={Route::Posts}
                                            classes={classes!("home-avatar-link")}
                                        >
                                            <img src={crate::config::asset_path("static/avatar.jpg")} alt="作者头像" loading="lazy" />
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
                                    { for stats.into_iter().map(|(icon, value, label, route)| html! {
                                        <StatsCard icon={icon} value={value} label={label} route={route} />
                                    }) }
                                </div>
                                <div class="tech-stack">
                                    <p class="tech-stack-title">{ "技术栈" }</p>
                                    <div class="tech-stack-tags">
                                        { for tech_stack.iter().map(|(logo, name, href)| html! {
                                            <a class="tech-tag" href={(*href).to_string()} target="_blank" rel="noopener noreferrer" title={*name}>
                                                <img src={logo.clone()} alt={*name} class="tech-logo" loading="lazy" />
                                                <span class="tech-tag-name">{ *name }</span>
                                            </a>
                                        }) }
                                    </div>
                                </div>
                            </div>
                            <svg class="hero-wave hero-wave-bottom" viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                                <path d="M0,80 C200,20 320,120 720,60 C1120,0 1240,80 1440,40 L1440,120 L0,120 Z" fill="var(--bg)" />
                            </svg>
                        </section>
                    </div>
                </div>

                // Articles Page
                <div class="home-page">
                    <div class="home-page-scroll" ref={articles_scroll_ref}>
                        <section class="article-list-section" aria-label="文章列表">
                            <div class="container">
                                <div class="content">
                                    <div class="section-title-with-bg">
                                        <h2>{ "最新文章" }</h2>
                                        <p>{ "甄选近期发布的内容，持续更新" }</p>
                                    </div>
                                    { article_grid }
                                </div>
                            </div>
                        </section>
                    </div>
                </div>
            </div>

            // Navigation Buttons - 回到主页在左侧，查看文章在右侧
            if is_articles_page {
                <div class="home-nav-button-container home-nav-button-container--left">
                    <TooltipIconButton
                        icon={IconName::ChevronLeft}
                        tooltip="回到主页"
                        position={TooltipPosition::Right}
                        onclick={switch_to_hero}
                        size={24}
                        class="home-nav-button home-nav-button--back"
                    />
                </div>
            } else {
                <div class="home-nav-button-container home-nav-button-container--right">
                    <TooltipIconButton
                        icon={IconName::ChevronRight}
                        tooltip="查看文章"
                        position={TooltipPosition::Left}
                        onclick={switch_to_articles}
                        size={24}
                        class="home-nav-button home-nav-button--forward"
                    />
                </div>
            }

            // Scroll to Top Button (only on articles page)
            if is_articles_page {
                <div class="scroll-to-top">
                    <TooltipIconButton
                        icon={IconName::ArrowUp}
                        tooltip="回到顶部"
                        position={TooltipPosition::Top}
                        onclick={scroll_to_top}
                        size={20}
                    />
                </div>
            }
        </div>
    }
}
