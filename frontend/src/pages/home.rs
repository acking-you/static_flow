use web_sys::console;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    components::{icons::IconName, stats_card::StatsCard},
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
    let on_avatar_enter = {
        let avatar_hovered = avatar_hovered.clone();
        Callback::from(move |_| avatar_hovered.set(true))
    };
    let on_avatar_leave = {
        let avatar_hovered = avatar_hovered.clone();
        Callback::from(move |_| avatar_hovered.set(false))
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
        "bg-[var(--surface)]",
        "overflow-hidden",
        "transition-[var(--transition-base)]",
        "shadow-[0_15px_35px_rgba(0,0,0,0.15)]",
        "no-underline",
        "text-inherit",
        "hero-avatar-trigger",
        if *avatar_hovered { "hero-avatar-trigger--hovered" } else { "" }
    );

    let avatar_image_class = classes!(
        "w-full",
        "h-full",
        "object-cover",
        "rounded-[inherit]",
        "block",
        "hero-avatar",
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
                                        <Link<Route>
                                            to={Route::Posts}
                                            classes={classes!("inline-flex", "w-full", "h-full", "justify-center", "items-center")}
                                        >
                                            <img src={crate::config::asset_path("static/avatar.jpg")} alt="作者头像" loading="lazy" class={avatar_image_class.clone()} />
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
                                    <span class="terminal-content">{ "地球Online | Lv.0 → ∞ | \"Talk is cheap, show me the code.\" - Linus. Coding just for fun!" }</span>
                                </div>

                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "$ " }</span>
                                    <span class="terminal-content">{ "cat ./README.md" }</span>
                                </div>
                                <div class="terminal-line">
                                    <span class="terminal-prompt">{ "> " }</span>
                                    <span class="terminal-content">{ "全栈 Rust 小站，玩 WASM 玩 AI。代码、生活、随想，想记就记。" }</span>
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
                                            <img
                                                src={logo.clone()}
                                                alt={*name}
                                                class="command-item-icon"
                                                loading="lazy"
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
