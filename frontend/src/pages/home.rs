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
        (
            IconName::FileText,
            total_articles.to_string(),
            "文章".to_string(),
            Some(Route::LatestArticles),
        ),
        (IconName::Hash, "12".to_string(), "标签".to_string(), Some(Route::Tags)),
        (IconName::Folder, "5".to_string(), "分类".to_string(), Some(Route::Categories)),
    ];

    let social_button_class = classes!(
        "inline-flex",
        "justify-center",
        "items-center",
        "w-12",
        "h-12",
        "rounded-[1.1rem]",
        "border",
        "border-[var(--border)]",
        "bg-[var(--surface)]",
        "text-[var(--text)]",
        "shadow-[var(--shadow-sm)]",
        "transition-all",
        "duration-300",
        "ease-[var(--ease-spring)]",
        "relative",
        "overflow-hidden",
        "hover:-translate-y-1",
        "hover:scale-110",
        "hover:border-transparent",
        "hover:bg-[var(--primary)]",
        "hover:text-white",
        "hover:shadow-[0_18px_40px_rgba(var(--primary-rgb),0.35)]",
        "focus-visible:outline-none",
        "focus-visible:ring-2",
        "focus-visible:ring-[var(--primary)]",
        "focus-visible:ring-offset-2",
        "focus-visible:ring-offset-[var(--bg)]",
        "active:scale-95"
    );

    let tech_chip_class = classes!(
        "group",
        "inline-flex",
        "items-center",
        "gap-3",
        "rounded-[1.1rem]",
        "border",
        "border-[var(--border)]",
        "bg-[var(--surface)]",
        "text-[var(--text)]",
        "px-4",
        "py-2.5",
        "shadow-[var(--shadow-sm)]",
        "transition-all",
        "duration-300",
        "ease-[var(--ease-spring)]",
        "hover:-translate-y-1",
        "hover:scale-105",
        "hover:border-transparent",
        "hover:bg-[var(--primary)]",
        "hover:text-white",
        "hover:shadow-[0_18px_40px_rgba(var(--primary-rgb),0.35)]",
        "focus-visible:outline-none",
        "focus-visible:ring-2",
        "focus-visible:ring-[var(--primary)]",
        "focus-visible:ring-offset-2",
        "focus-visible:ring-offset-[var(--bg)]",
        "active:scale-95"
    );

    let tech_icon_wrapper_class = classes!(
        "flex",
        "items-center",
        "justify-center",
        "w-9",
        "h-9",
        "rounded-full",
        "bg-[rgba(var(--surface-rgb),0.95)]",
        "shadow-inner",
        "transition-all",
        "duration-300",
        "ease-[var(--ease-spring)]",
        "group-hover:bg-white/90"
    );

    let tech_label_class = classes!(
        "text-sm",
        "font-semibold",
        "whitespace-nowrap",
        "transition-all",
        "duration-300",
        "opacity-90",
        "group-hover:opacity-100",
        "group-hover:text-white"
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
            "bg-[linear-gradient(rgba(29,158,216,0.05)_1px,transparent_1px),linear-gradient(90deg,rgba(29,158,216,0.05)_1px,transparent_1px)]",
            "bg-[size:40px_40px]",
            "dark:bg-[var(--surface-alt)]",
            "dark:bg-[radial-gradient(circle_at_center,rgba(53,179,255,0.12)_2px,transparent_2px)]",
            "overflow-x-hidden",
            "pb-2"
        )}>
            <div class={classes!("w-full", "pb-4") }>
                <section class={classes!(
                        "relative",
                        "mb-0",
                        "py-[var(--space-2xl)]",
                        "px-4",
                        "pb-24",
                        "max-[767px]:pb-16",
                        "rounded-[calc(var(--radius)*1.5)]",
                        "overflow-hidden",
                        "bg-[var(--bg)]",
                        "bg-[linear-gradient(rgba(29,158,216,0.08)_1px,_transparent_1px),linear-gradient(90deg,_rgba(29,158,216,0.08)_1px,_transparent_1px)]",
                        "bg-[size:40px_40px]",
                        "dark:bg-[var(--surface-alt)]",
                        "dark:bg-[radial-gradient(circle_at_center,rgba(53,179,255,0.15)_2px,_transparent_2px)]"
                    )}>
                        <div class={classes!(
                            "pointer-events-none",
                            "absolute",
                            "w-[220px]",
                            "h-[220px]",
                            "-top-16",
                            "-right-10",
                            "bg-[radial-gradient(circle,rgba(29,158,216,0.25)_0%,transparent_60%)]",
                            "blur-[8px]"
                        )} />
                        <svg class={classes!("pointer-events-none", "w-full", "h-[120px]", "absolute", "left-0", "top-0", "-translate-y-[60px]")} viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                            <path d="M0,40 C240,120 360,0 720,60 C1080,120 1200,20 1440,60 L1440,0 L0,0 Z" fill="rgba(29, 158, 216, 0.08)" />
                        </svg>
                        <div class={classes!(
                            "w-full",
                            "max-w-[80rem]",
                            "mx-auto",
                            "px-[clamp(1rem,4vw,2.5rem)]"
                        )}>
                            <div class={classes!("py-[var(--space-xl)]", "px-0", "pb-[var(--space-md)]", "flex", "flex-col", "items-center", "text-center", "gap-4")}>
                                <div
                                    class={classes!("flex", "justify-center") }
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
                                <h1 class={classes!("my-4", "mx-0", "mb-[0.35rem]", "text-[1.9rem]", "font-semibold", "tracking-[0.03em]", "text-[#8a919c]", "text-center", "dark:text-[#dcdcdc]")}>
                                    { "学习如逆水行舟，不进则退！" }
                                </h1>
                                <p class={classes!("m-0", "text-[var(--muted)]", "text-[1.1rem]", "max-w-[38rem]", "text-center")}>
                                    { "本地优先的写作实验室,记录 Rust · 自动化 · 创作思考。" }
                                </p>
                                <div class={classes!(
                                    "mt-6",
                                    "flex",
                                    "flex-wrap",
                                    "items-center",
                                    "justify-center",
                                    "gap-3"
                                )}>
                                    <Link<Route>
                                        to={Route::LatestArticles}
                                        classes={classes!(
                                            "group",
                                            "inline-flex",
                                            "items-center",
                                            "justify-center",
                                            "gap-2",
                                            "rounded-full",
                                            "px-6",
                                            "py-3",
                                            "text-base",
                                            "font-semibold",
                                            "text-white",
                                            "bg-[var(--primary)]",
                                            "border",
                                            "border-[var(--primary)]",
                                            "shadow-[0_18px_35px_rgba(15,23,42,0.25)]",
                                            "transition-all",
                                            "duration-300",
                                            "ease-[var(--ease-spring)]",
                                            "hover:-translate-y-1",
                                            "hover:bg-[var(--link)]",
                                            "focus-visible:outline-none",
                                            "focus-visible:ring-2",
                                            "focus-visible:ring-[var(--link)]",
                                            "focus-visible:ring-offset-2",
                                            "focus-visible:ring-offset-[var(--surface)]",
                                            "no-underline"
                                        )}
                                    >
                                        <span>{ "查看文章" }</span>
                                        <i class={classes!(
                                            "fas",
                                            "fa-arrow-down",
                                            "text-sm",
                                            "transition-transform",
                                            "duration-300",
                                            "group-hover:translate-y-1"
                                        )} aria-hidden="true"></i>
                                    </Link<Route>>
                                    <Link<Route>
                                        to={Route::Posts}
                                        classes={classes!(
                                            "inline-flex",
                                            "items-center",
                                            "gap-2",
                                            "rounded-full",
                                            "px-6",
                                            "py-3",
                                            "text-base",
                                            "font-medium",
                                            "text-[var(--text)]",
                                            "border",
                                            "border-[var(--border)]",
                                            "bg-[rgba(var(--surface-rgb),0.8)]",
                                            "backdrop-blur",
                                            "shadow-[var(--shadow-sm)]",
                                            "transition-all",
                                            "duration-300",
                                            "ease-[var(--ease-spring)]",
                                            "hover:text-[var(--primary)]",
                                            "hover:border-[var(--primary)]",
                                            "hover:-translate-y-1",
                                            "hover:shadow-[var(--shadow)]",
                                            "focus-visible:outline-none",
                                            "focus-visible:ring-2",
                                            "focus-visible:ring-[var(--primary)]",
                                            "focus-visible:ring-offset-2",
                                            "focus-visible:ring-offset-[var(--bg)]",
                                            "no-underline"
                                        )}
                                    >
                                        <span>{ "文章归档" }</span>
                                        <i class={classes!("fas", "fa-arrow-up-right-from-square", "text-sm") } aria-hidden="true"></i>
                                    </Link<Route>>
                                </div>
                                <div class={classes!("flex", "justify-center", "items-center", "gap-4", "mt-6", "text-center")} aria-label="社交链接">
                                    <a
                                        href="https://github.com/ACking-you"
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        aria-label="GitHub"
                                        class={social_button_class.clone()}
                                    >
                                        <i class={classes!("fa-brands", "fa-github-alt")} aria-hidden="true"></i>
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
                                            width="22"
                                            height="22"
                                        >
                                            <path
                                                fill="currentColor"
                                                d="M17.813 4.653h.854c1.51.054 2.769.578 3.773 1.574 1.004.995 1.524 2.249 1.56 3.76v7.36c-.036 1.51-.556 2.769-1.56 3.773s-2.262 1.524-3.773 1.56H5.333c-1.51-.036-2.769-.556-3.773-1.56S.036 18.858 0 17.347v-7.36c.036-1.511.556-2.765 1.56-3.76 1.004-.996 2.262-1.52 3.773-1.574h.774l-1.174-1.12a1.234 1.234 0 0 1-.373-.906c0-.356.124-.658.373-.907l.027-.027c.267-.249.573-.373.92-.373.347 0 .653.124.92.373L9.653 4.44c.071.071.134.142.187.213h4.267a.836.836 0 0 1 .16-.213l2.853-2.747c.267-.249.573-.373.92-.373.347 0 .662.151.929.4.267.249.391.551.391.907 0 .355-.124.657-.373.906zM5.333 7.24c-.746.018-1.373.276-1.88.773-.506.498-.769 1.13-.786 1.894v7.52c.017.764.28 1.395.786 1.893.507.498 1.134.756 1.88.773h13.334c.746-.017 1.373-.275 1.88-.773.506-.498.769-1.129.786-1.893v-7.52c-.017-.765-.28-1.396-.786-1.894-.507-.497-1.134-.755-1.88-.773zM8 11.107c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c0-.373.129-.689.386-.947.258-.257.574-.386.947-.386zm8 0c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c.017-.391.15-.711.4-.96.249-.249.56-.373.933-.373Z"
                                            />
                                        </svg>
                                        <span class={classes!("sr-only")}>{ "Bilibili" }</span>
                                    </a>
                                </div>
                            </div>
                            <div class={classes!("mt-8", "grid", "grid-cols-[repeat(auto-fit,minmax(180px,1fr))]", "gap-4", "w-full", "md:grid-cols-[repeat(auto-fit,minmax(140px,1fr))]", "max-[480px]:grid-cols-1")}>
                                { for stats.into_iter().map(|(icon, value, label, route)| html! {
                                    <StatsCard icon={icon} value={value} label={label} route={route} />
                                }) }
                            </div>
                            <div class={classes!("mt-8", "w-full", "text-center")}>
                                <p class={classes!("m-0", "mb-[0.65rem]", "text-[0.95rem]", "tracking-[0.05em]", "uppercase", "text-[var(--muted)]")}>{ "技术栈" }</p>
                                <div class={classes!("flex", "flex-wrap", "justify-center", "gap-3")}>
                                    { for tech_stack.iter().map(|(logo, name, href)| html! {
                                        <a
                                            class={tech_chip_class.clone()}
                                            href={(*href).to_string()}
                                            target="_blank"
                                            rel="noopener noreferrer"
                                            title={*name}
                                            aria-label={(*name).to_string()}
                                        >
                                            <span class={tech_icon_wrapper_class.clone()}>
                                                <img
                                                    src={logo.clone()}
                                                    alt={*name}
                                                    class={classes!(
                                                        "object-contain",
                                                        "w-5",
                                                        "h-5",
                                                        "transition-transform",
                                                        "duration-300",
                                                        "group-hover:scale-110"
                                                    )}
                                                    loading="lazy"
                                                />
                                            </span>
                                            <span class={tech_label_class.clone()}>{ *name }</span>
                                        </a>
                                    }) }
                                </div>
                            </div>
                        </div>
                        <svg class={classes!("pointer-events-none", "w-full", "h-[120px]", "absolute", "left-0", "bottom-0", "translate-y-[60px]")} viewBox="0 0 1440 120" preserveAspectRatio="none" aria-hidden="true">
                            <path d="M0,80 C200,20 320,120 720,60 C1120,0 1240,80 1440,40 L1440,120 L0,120 Z" fill="var(--bg)" />
                        </svg>
                    </section>
                </div>
        </div>
    }
}
