use wasm_bindgen::JsCast;
use web_sys::console;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{icons::IconName, image_with_loading::ImageWithLoading, stats_card::StatsCard},
    router::Route,
};

#[function_component(HomePage)]
pub fn home_page() -> Html {
    let total_articles = use_state(|| 0usize);

    {
        let total_articles = total_articles.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match crate::api::fetch_articles(None, None).await {
                    Ok(data) => total_articles.set(data.len()),
                    Err(e) => {
                        console::error_1(&format!("Failed to fetch articles: {}", e).into());
                    },
                }
            });
            || ()
        });
    }

    let stats = vec![
        (IconName::FileText, total_articles.to_string(), "文章".to_string(), Some(Route::Posts)),
        (IconName::Hash, "12".to_string(), "标签".to_string(), Some(Route::Tags)),
        (IconName::Folder, "5".to_string(), "分类".to_string(), Some(Route::Categories)),
    ];

    let social_button_class = classes!(
        "btn-fluent-icon",
        "border",
        "border-[var(--border)]",
        "hover:bg-[var(--surface-alt)]",
        "hover:text-[var(--primary)]",
        "transition-all",
        "duration-100",
        "ease-[var(--ease-snap)]"
    );

    let tech_chip_class = classes!(
        "group",
        "inline-flex",
        "items-center",
        "gap-3",
        "relative",
        "overflow-hidden",
        "rounded-lg",
        "border",
        "border-[var(--border)]",
        "bg-[var(--surface)]",
        "text-[var(--text)]",
        "px-4",
        "py-3",
        "shadow-[var(--shadow-2)]",
        "liquid-glass-subtle",
        "shimmer-hover",
        "transform-gpu",
        "transition-all",
        "duration-200",
        "ease-[var(--ease-snap)]",
        "hover:bg-[var(--surface-alt)]",
        "hover:text-[var(--primary)]",
        "hover:shadow-[var(--shadow-4)]",
        "hover:scale-105"
    );

    let tech_icon_wrapper_class = classes!(
        "flex",
        "items-center",
        "justify-center",
        "w-9",
        "h-9",
        "rounded",
        "bg-[var(--surface-alt)]",
        "text-[var(--primary)]",
        "transition-all",
        "duration-150",
        "ease-[var(--ease-snap)]"
    );

    let tech_label_class = classes!(
        "text-sm",
        "font-semibold",
        "whitespace-nowrap",
        "text-[var(--text)]",
        "transition-colors",
        "duration-150",
        "opacity-90",
        "group-hover:opacity-100",
        "group-hover:text-[var(--primary)]"
    );

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

    let avatar_hovered = use_state(|| false);
    let avatar_loaded = use_state(|| false);

    let on_avatar_enter = {
        let avatar_hovered = avatar_hovered.clone();
        Callback::from(move |_| avatar_hovered.set(true))
    };
    let on_avatar_leave = {
        let avatar_hovered = avatar_hovered.clone();
        Callback::from(move |_| avatar_hovered.set(false))
    };
    let on_avatar_load = {
        let avatar_loaded = avatar_loaded.clone();
        Callback::from(move |_: Event| avatar_loaded.set(true))
    };

    let avatar_container_class = classes!(
        "inline-flex",
        "justify-center",
        "items-center",
        "w-[140px]",
        "h-[140px]",
        "rounded-full",
        "border-[3px]",
        "border-[var(--surface)]",
        "overflow-hidden",
        "transition-[var(--transition-base)]",
        "shadow-[0_15px_35px_rgba(0,0,0,0.15)]",
        "no-underline",
        "text-inherit",
        "hero-avatar-trigger",
        "relative",
        if !*avatar_loaded { "bg-[var(--surface)]" } else { "bg-transparent" },
        if *avatar_hovered { "hero-avatar-trigger--hovered" } else { "" }
    );

    let avatar_image_class = classes!(
        "w-full",
        "h-full",
        "object-cover",
        "rounded-[inherit]",
        "block",
        "hero-avatar",
        "transition-opacity",
        "duration-500",
        if *avatar_loaded { "opacity-100" } else { "opacity-0" },
        if *avatar_hovered { "hero-avatar--spinning" } else { "" }
    );

    html! {
        <div class={classes!(
            "relative",
            "w-full",
            "min-h-screen",
            "bg-[var(--bg)]",
            "overflow-x-hidden",
            "pb-8"
        )}>
            <div class={classes!("w-full", "pb-6") }>
                <section class={classes!(
                        "relative",
                        "py-20",
                        "md:py-24",
                        "px-4",
                        "max-[767px]:pb-16",
                        "max-w-5xl",
                        "mx-auto"
                    )}>
                        <div class={classes!(
                            "w-full",
                            "mx-auto",
                            "px-[clamp(1rem,4vw,2rem)]"
                        )}>
                            // Terminal Hero - Terminal Style Interface
                            <div class="terminal-hero">
                                // Terminal Header with macOS-style dots
                                <div class="terminal-header">
                                    <span class="terminal-dot terminal-dot-red"></span>
                                    <span class="terminal-dot terminal-dot-yellow"></span>
                                    <span class="terminal-dot terminal-dot-green"></span>
                                    <span class="terminal-title">{ "system_info.sh" }</span>
                                </div>

                                // Avatar displayed as command output
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "cat ./profile/avatar.jpg" }</span>
                                </div>
                                <div
                                    class={classes!("flex", "justify-center", "my-6")}
                                    onmouseover={on_avatar_enter.clone()}
                                    onmouseout={on_avatar_leave.clone()}
                                >
                                    <div class={avatar_container_class.clone()}>
                                        {
                                            if !*avatar_loaded {
                                                html! {
                                                    <div class={classes!(
                                                        "absolute",
                                                        "inset-0",
                                                        "rounded-full",
                                                        "bg-gradient-to-br",
                                                        "from-[var(--surface-alt)]",
                                                        "to-[var(--surface)]",
                                                        "animate-pulse"
                                                    )} />
                                                }
                                            } else {
                                                html! {}
                                            }
                                        }
                                        <Link<Route>
                                            to={Route::Posts}
                                            classes={classes!("inline-flex", "w-full", "h-full", "justify-center", "items-center")}
                                        >
                                            <img
                                                src={crate::config::asset_path("static/avatar.jpg")}
                                                alt="作者头像"
                                                loading="eager"
                                                onload={on_avatar_load}
                                                class={avatar_image_class.clone()}
                                            />
                                            <span class={classes!("sr-only")}>{ "前往文章列表" }</span>
                                        </Link<Route>>
                                    </div>
                                </div>

                                // Introduction as terminal commands
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "echo $MOTTO" }</span>
                                </div>
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "> " }</span>
                                    <span class="terminal-content">{ "El Psy Kongroo | 世界线收束中... | Rustacean | Database 练习生，痴迷一切底层黑魔法" }</span>
                                </div>

                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "cat ./README.md" }</span>
                                </div>
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "> " }</span>
                                    <span class="terminal-content">{ "不造 agent，只写 skill。借 AI 之力高效学习创作，顺便记录技术与生活。" }</span>
                                </div>

                                // Quick navigation buttons styled as terminal commands
                                <div class="terminal-line" style="margin-top: 1.5rem;">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "ls -l ./navigation/" }</span>
                                </div>
                                <div class={classes!(
                                    "flex",
                                    "flex-wrap",
                                    "gap-3",
                                    "mt-4",
                                    "ml-8"
                                )}>
                                    <Link<Route>
                                        to={Route::LatestArticles}
                                        classes={classes!("btn-fluent-primary", "!px-6", "!py-2.5", "!text-sm")}
                                    >
                                        <i class="fas fa-arrow-right mr-2"></i>
                                        { "查看文章" }
                                    </Link<Route>>
                                    <Link<Route>
                                        to={Route::Posts}
                                        classes={classes!("btn-fluent-secondary", "!px-6", "!py-2.5", "!text-sm")}
                                    >
                                        <i class="fas fa-archive mr-2"></i>
                                        { "文章归档" }
                                    </Link<Route>>
                                </div>

                                // Social links as terminal output
                                <div class="terminal-line" style="margin-top: 1.5rem;">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "cat ./social_links.json" }</span>
                                </div>
                                <div class={classes!("flex", "gap-3", "mt-3", "ml-8")}>
                                    <a
                                        href="https://github.com/ACking-you"
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        aria-label="GitHub"
                                        class={social_button_class.clone()}
                                    >
                                        <i class={classes!("fa-brands", "fa-github-alt", "text-lg")} aria-hidden="true"></i>
                                        <span class={classes!("sr-only")}>{ "GitHub" }</span>
                                    </a>
                                    <a
                                        href="https://space.bilibili.com/24264499"
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        aria-label="Bilibili"
                                        class={social_button_class.clone()}
                                    >
                                        <svg
                                            viewBox="0 0 24 24"
                                            role="img"
                                            aria-hidden="true"
                                            focusable="false"
                                            width="20"
                                            height="20"
                                        >
                                            <path
                                                fill="currentColor"
                                                d="M17.813 4.653h.854c1.51.054 2.769.578 3.773 1.574 1.004.995 1.524 2.249 1.56 3.76v7.36c-.036 1.51-.556 2.769-1.56 3.773s-2.262 1.524-3.773 1.56H5.333c-1.51-.036-2.769-.556-3.773-1.56S.036 18.858 0 17.347v-7.36c.036-1.511.556-2.765 1.56-3.76 1.004-.996 2.262-1.52 3.773-1.574h.774l-1.174-1.12a1.234 1.234 0 0 1-.373-.906c0-.356.124-.658.373-.907l.027-.027c.267-.249.573-.373.92-.373.347 0 .653.124.92.373L9.653 4.44c.071.071.134.142.187.213h4.267a.836.836 0 0 1 .16-.213l2.853-2.747c.267-.249.573-.373.92-.373.347 0 .662.151.929.4.267.249.391.551.391.907 0 .355-.124.657-.373.906zM5.333 7.24c-.746.018-1.373.276-1.88.773-.506.498-.769 1.13-.786 1.894v7.52c.017.764.28 1.395.786 1.893.507.498 1.134.756 1.88.773h13.334c.746-.017 1.373-.275 1.88-.773.506-.498.769-1.129.786-1.893v-7.52c-.017-.765-.28-1.396-.786-1.894-.507-.497-1.134-.755-1.88-.773zM8 11.107c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c0-.373.129-.689.386-.947.258-.257.574-.386.947-.386zm8 0c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c.017-.391.15-.711.4-.96.249-.249.56-.373.933-.373Z"
                                            />
                                        </svg>
                                        <span class={classes!("sr-only")}>{ "Bilibili" }</span>
                                    </a>
                                </div>

                                // GitHub Wrapped - Featured Entry with Year Selector
                                <div class="terminal-line" style="margin-top: 1.5rem;">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "./scripts/github-wrapped.sh --list-years" }</span>
                                </div>
                                <GithubWrappedSelector />

                                // Blinking cursor at the end
                                <div class="terminal-line" style="margin-top: 1.5rem;">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-cursor"></span>
                                </div>
                            </div>

                            // System Info Panels (Stats as system metrics)
                            <div class={classes!("mt-12", "w-full")}>
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "cat /proc/system/stats" }</span>
                                </div>
                                <div class={classes!(
                                    "mt-4",
                                    "grid",
                                    "gap-5",
                                    "grid-cols-1",
                                    "md:grid-cols-3",
                                    "w-full"
                                )}>
                                    { for stats.into_iter().map(|(icon, value, label, route)| {
                                        let panel_content = html! {
                                            <div class="system-panel">
                                                <div class="system-panel-label">{ label.clone() }</div>
                                                <div class="system-panel-value">{ value.clone() }</div>
                                                <div class="system-panel-unit">{ "total" }</div>
                                            </div>
                                        };

                                        if let Some(r) = route {
                                            html! {
                                                <Link<Route> to={r}>
                                                    { panel_content }
                                                </Link<Route>>
                                            }
                                        } else {
                                            panel_content
                                        }
                                    }) }
                                </div>
                            </div>

                            // Tech Stack as Command List
                            <div class={classes!("mt-12", "w-full")}>
                                <div class="command-list">
                                    <div class="command-list-header">
                                        <span class="terminal-prompt">{ "$ " }</span>
                                        <span class="command-list-title">{ "POWERED BY" }</span>
                                    </div>
                                    { for tech_stack.iter().map(|(logo, name, href)| html! {
                                        <a
                                            class="command-item"
                                            href={(*href).to_string()}
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            title={*name}
                                            aria-label={(*name).to_string()}
                                        >
                                            <ImageWithLoading
                                                src={logo.clone()}
                                                alt={*name}
                                                loading={Some(AttrValue::from("lazy"))}
                                                class="command-item-icon"
                                                container_class={classes!("inline-flex")}
                                            />
                                            <span class="command-item-name">{ *name }</span>
                                            <span class="command-item-arrow">{ "→" }</span>
                                        </a>
                                    }) }
                                </div>
                            </div>
                        </div>
                    </section>
                </div>
        </div>
    }
}

/// GitHub Wrapped year entry
#[derive(Clone)]
struct WrappedYear {
    year: u16,
    is_latest: bool,
}

impl WrappedYear {
    fn url(&self) -> String {
        format!("/standalone/github-wrapped-{}.html", self.year)
    }
}

/// Available GitHub Wrapped years (newest first)
fn get_wrapped_years() -> Vec<WrappedYear> {
    vec![
        WrappedYear { year: 2025, is_latest: true },
        // Add more years here as they become available:
        // WrappedYear { year: 2024, is_latest: false },
    ]
}

#[function_component(GithubWrappedSelector)]
fn github_wrapped_selector() -> Html {
    let expanded = use_state(|| false);
    let years = get_wrapped_years();
    let latest = years.first().cloned();

    let toggle_expand = {
        let expanded = expanded.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            e.stop_propagation();
            expanded.set(!*expanded);
        })
    };

    let close_dropdown = {
        let expanded = expanded.clone();
        Callback::from(move |_| expanded.set(false))
    };

    // Close on outside click
    {
        let expanded = expanded.clone();
        use_effect_with(*expanded, move |is_expanded| {
            let cleanup: Box<dyn FnOnce()> = if *is_expanded {
                let expanded = expanded.clone();
                let closure = wasm_bindgen::closure::Closure::<dyn Fn(web_sys::Event)>::new(move |_: web_sys::Event| {
                    expanded.set(false);
                });

                if let Some(window) = web_sys::window() {
                    let _ = window.add_event_listener_with_callback(
                        "click",
                        closure.as_ref().unchecked_ref(),
                    );
                    let window_clone = window.clone();
                    Box::new(move || {
                        let _ = window_clone.remove_event_listener_with_callback(
                            "click",
                            closure.as_ref().unchecked_ref(),
                        );
                    })
                } else {
                    Box::new(|| {})
                }
            } else {
                Box::new(|| {})
            };
            cleanup
        });
    }

    let Some(latest) = latest else {
        return html! {};
    };

    let has_multiple_years = years.len() > 1;

    html! {
        <div class={classes!("mt-3", "ml-8", "github-wrapped-container")}>
            <div class="github-wrapped-group">
                // Main button - always links to latest year
                <a
                    href={latest.url()}
                    target="_blank"
                    rel="noopener noreferrer"
                    class="github-wrapped-btn"
                >
                    <span class="github-wrapped-badge">{ "NEW" }</span>
                    <i class={classes!("fa-brands", "fa-github", "text-xl")} aria-hidden="true"></i>
                    <span class="github-wrapped-text">
                        <span class="github-wrapped-title">{ format!("{} GitHub Wrapped", latest.year) }</span>
                        <span class="github-wrapped-subtitle">{ "年度代码回顾 →" }</span>
                    </span>
                </a>

                // Expand button (only show if multiple years)
                if has_multiple_years {
                    <button
                        type="button"
                        class={classes!(
                            "github-wrapped-expand",
                            if *expanded { "expanded" } else { "" }
                        )}
                        onclick={toggle_expand}
                        aria-label="查看更多年份"
                        aria-expanded={(*expanded).to_string()}
                    >
                        <i class="fas fa-chevron-down" aria-hidden="true"></i>
                    </button>
                }
            </div>

            // Dropdown with all years
            if has_multiple_years && *expanded {
                <div class="github-wrapped-dropdown" onclick={close_dropdown.reform(|e: MouseEvent| e.stop_propagation())}>
                    <div class="github-wrapped-dropdown-header">
                        { "选择年份" }
                    </div>
                    { for years.iter().map(|y| html! {
                        <a
                            href={y.url()}
                            target="_blank"
                            rel="noopener noreferrer"
                            class={classes!(
                                "github-wrapped-dropdown-item",
                                if y.is_latest { "latest" } else { "" }
                            )}
                        >
                            <i class="fa-brands fa-github" aria-hidden="true"></i>
                            <span>{ format!("{} Wrapped", y.year) }</span>
                            if y.is_latest {
                                <span class="github-wrapped-latest-tag">{ "最新" }</span>
                            }
                        </a>
                    }) }
                </div>
            }
        </div>
    }
}
