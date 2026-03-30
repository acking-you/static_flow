# Homepage Redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Restructure the StaticFlow homepage from a terminal-only showcase into a content discovery entry point with article and music previews, while preserving the terminal hero aesthetic.

**Architecture:** Simplify the Hero terminal (remove tabs, flatten CTA), extract LLM Access into an independent banner, move Stats up, add two new content preview sections (articles + music), compress Tech Stack to a logo strip, and relocate Social to the footer zone. Also replace the global `ImageWithLoading` pulse skeleton with a centered spinner.

**Tech Stack:** Yew (Rust WASM), Tailwind CSS, existing `ArticleCard` component, existing `LoadingSpinner` component, existing API endpoints (`fetch_articles`, `fetch_songs`, `fetch_site_stats`, `fetch_images_page`).

---

### Task 1: Update ImageWithLoading — replace pulse with spinner

**Files:**
- Modify: `frontend/src/components/image_with_loading.rs:1-88`

This is a cross-cutting change that benefits all pages. Do it first so all subsequent work uses the new loading state.

- [ ] **Step 1: Add LoadingSpinner import**

In `frontend/src/components/image_with_loading.rs`, add the import at the top:

```rust
use crate::components::loading_spinner::{LoadingSpinner, SpinnerSize};
```

The existing import block is just `use yew::prelude::*;` — add the new import after it.

- [ ] **Step 2: Replace the pulse skeleton with LoadingSpinner**

Replace the loading placeholder (lines 60-74) from:

```rust
if !*image_loaded {
    html! {
        <div class={classes!(
            "absolute",
            "inset-0",
            "bg-gradient-to-br",
            "from-[var(--surface-alt)]",
            "to-[var(--surface)]",
            "animate-pulse",
            "pointer-events-none"
        )} />
    }
} else {
    html! {}
}
```

To:

```rust
if !*image_loaded {
    html! {
        <div class={classes!(
            "absolute",
            "inset-0",
            "flex",
            "items-center",
            "justify-center",
            "pointer-events-none"
        )}>
            <LoadingSpinner size={SpinnerSize::Small} />
        </div>
    }
} else {
    html! {}
}
```

- [ ] **Step 3: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -30`
Expected: no errors related to `image_with_loading.rs`

- [ ] **Step 4: Format changed file**

Run: `rustfmt frontend/src/components/image_with_loading.rs`

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/image_with_loading.rs
git commit -m "refactor: replace ImageWithLoading pulse skeleton with LoadingSpinner"
```

---

### Task 2: Add new i18n keys

**Files:**
- Modify: `frontend/src/i18n/zh_cn.rs:65-118` (the `pub mod home` block)

- [ ] **Step 1: Add new constants to the `home` module**

In `frontend/src/i18n/zh_cn.rs`, inside `pub mod home { ... }`, add these constants before the closing `}`:

```rust
    // Homepage redesign — new section titles
    pub const CMD_SHOW_RECENT_ARTICLES: &str = "ls ./recent-articles";
    pub const CMD_SHOW_RECENT_MUSIC: &str = "ls ./recent-music";
    pub const CMD_SHOW_TECH_STACK: &str = "cat ./tech-stack";
    pub const BTN_VIEW_ALL_ARTICLES: &str = "查看全部 →";
    pub const BTN_VIEW_ALL_MUSIC: &str = "查看全部 →";
    pub const BTN_IMAGE: &str = "图片";
```

Note: `CMD_SHOW_SOCIAL` already exists as `pub const CMD_SHOW_SOCIAL: &str = "cat ./social_links.json";` — reuse it.

- [ ] **Step 2: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -30`
Expected: warnings about unused constants (they'll be used in Task 3) but no errors.

- [ ] **Step 3: Format changed file**

Run: `rustfmt frontend/src/i18n/zh_cn.rs`

- [ ] **Step 4: Commit**

```bash
git add frontend/src/i18n/zh_cn.rs
git commit -m "feat: add i18n keys for homepage redesign sections"
```

---

### Task 3: Rewrite home.rs — Hero simplification + data fetching

**Files:**
- Modify: `frontend/src/pages/home.rs:1-791` (full rewrite)

This is the largest task. We rewrite the entire `home.rs` file. The `GithubWrappedSelector` component and its helpers (`WrappedYear`, `get_wrapped_years`) remain unchanged at the bottom of the file.

- [ ] **Step 1: Update imports**

Replace the current import block (lines 1-13) with:

```rust
use wasm_bindgen::JsCast;
use web_sys::console;
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{self, SongListItem},
    components::{
        article_card::ArticleCard,
        icons::{Icon, IconName},
        image_with_loading::ImageWithLoading,
        loading_spinner::{LoadingSpinner, SpinnerSize},
    },
    i18n::current::{common as common_text, home as t},
    models::ArticleListItem,
    router::Route,
};
```

Key additions: `api::{self, SongListItem}`, `ArticleCard`, `LoadingSpinner`, `SpinnerSize`, `ArticleListItem`.

- [ ] **Step 2: Remove HomeTab enum**

Delete the `HomeTab` enum (lines 15-19):

```rust
// DELETE THIS:
#[derive(Clone, Copy, PartialEq, Eq)]
enum HomeTab {
    Navigation,
    Social,
}
```

- [ ] **Step 3: Add a duration format helper**

Add this helper function above `HomePage` (or below the imports):

```rust
fn format_duration_short(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}
```

- [ ] **Step 4: Rewrite state declarations in HomePage**

Replace the state block (lines 23-28) with expanded state that includes articles and songs:

```rust
#[function_component(HomePage)]
pub fn home_page() -> Html {
    let total_articles = use_state(|| 0usize);
    let total_tags = use_state(|| 0usize);
    let total_categories = use_state(|| 0usize);
    let total_music = use_state(|| 0usize);
    let total_images = use_state(|| 0usize);
    let stats_loaded = use_state(|| false);
    let recent_articles = use_state(Vec::<ArticleListItem>::new);
    let articles_loaded = use_state(|| false);
    let recent_songs = use_state(Vec::<SongListItem>::new);
    let songs_loaded = use_state(|| false);
```

- [ ] **Step 5: Rewrite the use_effect_with data fetching block**

Replace the existing `use_effect_with` block (lines 30-72) with one that fires 4 parallel fetches:

```rust
    {
        let total_articles = total_articles.clone();
        let total_tags = total_tags.clone();
        let total_categories = total_categories.clone();
        let total_music = total_music.clone();
        let total_images = total_images.clone();
        let stats_loaded = stats_loaded.clone();
        let recent_articles = recent_articles.clone();
        let articles_loaded = articles_loaded.clone();
        let recent_songs = recent_songs.clone();
        let songs_loaded = songs_loaded.clone();
        use_effect_with((), move |_| {
            // Stats
            {
                let total_articles = total_articles.clone();
                let total_tags = total_tags.clone();
                let total_categories = total_categories.clone();
                let stats_loaded = stats_loaded.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_site_stats().await {
                        Ok(stats) => {
                            total_articles.set(stats.total_articles);
                            total_tags.set(stats.total_tags);
                            total_categories.set(stats.total_categories);
                        }
                        Err(e) => {
                            console::error_1(
                                &format!("Failed to fetch home stats: {e}").into(),
                            );
                        }
                    }
                    stats_loaded.set(true);
                });
            }
            // Images count
            {
                let total_images = total_images.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_images_page(Some(1), Some(0)).await {
                        Ok(resp) => total_images.set(resp.total),
                        Err(e) => {
                            console::error_1(
                                &format!("Failed to fetch image stats: {e}").into(),
                            );
                        }
                    }
                });
            }
            // Songs (shared: total for stats + list for preview)
            {
                let total_music = total_music.clone();
                let recent_songs = recent_songs.clone();
                let songs_loaded = songs_loaded.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_songs(Some(4), Some(0), None, None, None).await {
                        Ok(resp) => {
                            total_music.set(resp.total);
                            recent_songs.set(resp.songs);
                        }
                        Err(e) => {
                            console::error_1(
                                &format!("Failed to fetch songs: {e}").into(),
                            );
                        }
                    }
                    songs_loaded.set(true);
                });
            }
            // Recent articles
            {
                let recent_articles = recent_articles.clone();
                let articles_loaded = articles_loaded.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_articles(None, None, Some(4), Some(0)).await {
                        Ok(page) => {
                            recent_articles.set(page.articles);
                        }
                        Err(e) => {
                            console::error_1(
                                &format!("Failed to fetch recent articles: {e}").into(),
                            );
                        }
                    }
                    articles_loaded.set(true);
                });
            }
            || ()
        });
    }
```

- [ ] **Step 6: Verify compilation of state + fetch changes**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -40`
Expected: may have warnings about unused variables (the HTML isn't written yet), but no errors in the fetch logic.

- [ ] **Step 7: Commit progress**

```bash
git add frontend/src/pages/home.rs
git commit -m "feat(home): rewrite state and data fetching for homepage redesign"
```

---

### Task 4: Rewrite home.rs — HTML template (Hero + Banner + Stats)

**Files:**
- Modify: `frontend/src/pages/home.rs` (continuing from Task 3)

This task rewrites the HTML `html! { ... }` block. We'll build it in two tasks: this one covers Hero + Banner + Stats, the next covers content sections + footer.

- [ ] **Step 1: Remove old variable declarations that are no longer needed**

Delete these blocks from the function body (they were used by the old tab/social/tech-chip UI):

- `active_home_tab` state and `tab_nav`/`tab_social` callbacks (lines 102-110)
- `social_button_class` (lines 121-130)
- `_tech_chip_class` (lines 132-157) — already prefixed with `_`
- `_tech_icon_wrapper_class` (lines 159-171) — already prefixed with `_`
- `_tech_label_class` (lines 173-183) — already prefixed with `_`

Keep: `stats` vec, `staticflow_search_href`, `on_staticflow_search_click`, `tech_stack` array, `avatar_*` state/callbacks/classes.

- [ ] **Step 2: Write the stats vec (unchanged but repositioned)**

The `stats` vec stays the same as current code (lines 74-100). No changes needed.

- [ ] **Step 3: Write the HTML — outer wrapper + Hero terminal**

Replace the entire `html! { ... }` block (lines 262-615) with the new structure. Start with the outer wrapper and Hero:

```rust
    html! {
        <div class={classes!(
            "relative",
            "w-full",
            "min-h-screen",
            "bg-[var(--bg)]",
            "overflow-x-hidden",
            "pb-8"
        )}>
            <div class={classes!("w-full", "pb-6")}>
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
                        // ── Section 1: Hero Terminal ──
                        <div class="terminal-hero">
                            <div class="terminal-header">
                                <span class="terminal-dot terminal-dot-red"></span>
                                <span class="terminal-dot terminal-dot-yellow"></span>
                                <span class="terminal-dot terminal-dot-green"></span>
                                <span class="terminal-title">{ t::TERMINAL_TITLE }</span>
                            </div>

                            // Avatar
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_AVATAR }</span>
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
                                            alt={t::AVATAR_ALT}
                                            loading="eager"
                                            onload={on_avatar_load}
                                            class={avatar_image_class.clone()}
                                        />
                                        <span class={classes!("sr-only")}>{ t::AVATAR_LINK_SR }</span>
                                    </Link<Route>>
                                </div>
                            </div>

                            // Motto
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_MOTTO }</span>
                            </div>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                                <span class="terminal-content">{ t::MOTTO }</span>
                            </div>

                            // README
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_README }</span>
                            </div>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                                <span class="terminal-content">{ t::INTRO }</span>
                            </div>

                            // Open source
                            <div class="terminal-line" style="margin-top: 0.5rem;">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_OUTPUT }</span>
                                <span class="terminal-content">
                                    { t::OPEN_SOURCE_INLINE }
                                    { " " }
                                    <a href="https://github.com/acking-you/static_flow"
                                       target="_blank" rel="noopener noreferrer"
                                       class={classes!("underline", "text-[var(--primary)]", "font-semibold")}>
                                        { t::OPEN_SOURCE_GITHUB_CTA }
                                    </a>
                                </span>
                            </div>

                            // CTA buttons — flat, no tabs
                            <div class="terminal-line" style="margin-top: 1.5rem;">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_NAVIGATION }</span>
                            </div>
                            <div class={classes!("flex", "flex-wrap", "gap-2", "mt-3", "ml-8")}>
                                <Link<Route>
                                    to={Route::LatestArticles}
                                    classes={classes!("btn-terminal", "btn-terminal-primary")}
                                >
                                    <i class="fas fa-arrow-right"></i>
                                    { t::BTN_VIEW_ARTICLES }
                                </Link<Route>>
                                <Link<Route>
                                    to={Route::Posts}
                                    classes={classes!("btn-terminal")}
                                >
                                    <i class="fas fa-archive"></i>
                                    { t::BTN_ARCHIVE }
                                </Link<Route>>
                                <Link<Route>
                                    to={Route::MediaAudio}
                                    classes={classes!("btn-terminal", "btn-terminal-accent")}
                                >
                                    <i class="fas fa-headphones"></i>
                                    { t::BTN_MEDIA_AUDIO }
                                </Link<Route>>
                                <Link<Route>
                                    to={Route::MediaImage}
                                    classes={classes!("btn-terminal")}
                                >
                                    <i class="fas fa-image"></i>
                                    { t::BTN_IMAGE }
                                </Link<Route>>
                                <a
                                    href={staticflow_search_href.clone()}
                                    onclick={on_staticflow_search_click}
                                    class={classes!("btn-fluent-search-hero", "no-underline")}
                                >
                                    <i class="fas fa-search"></i>
                                    { t::BTN_SEARCH_STATICFLOW }
                                </a>
                                <Link<Route>
                                    to={Route::Admin}
                                    classes={classes!("btn-terminal")}
                                >
                                    <i class="fas fa-sliders"></i>
                                    { "Admin" }
                                </Link<Route>>
                            </div>

                            // Blinking cursor
                            <div class="terminal-line" style="margin-top: 1.5rem;">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-cursor"></span>
                            </div>
                        </div>

                        // ── Section 2: LLM/Kiro Access Banner ──
                        <div class={classes!(
                            "mt-8",
                            "w-full",
                            "bg-[var(--surface)]",
                            "border",
                            "border-[var(--border)]",
                            "border-l-[4px]",
                            "border-l-[var(--primary)]",
                            "rounded-lg",
                            "shadow-[var(--shadow-2)]",
                            "px-5",
                            "py-4",
                            "flex",
                            "flex-col",
                            "md:flex-row",
                            "md:items-center",
                            "md:justify-between",
                            "gap-3"
                        )}>
                            <p class={classes!("m-0", "text-sm", "leading-7", "text-[var(--muted)]")}>
                                { t::LLM_ACCESS_HINT }
                            </p>
                            <div class={classes!("flex", "flex-wrap", "gap-2", "shrink-0")}>
                                <Link<Route>
                                    to={Route::LlmAccess}
                                    classes={classes!("btn-terminal", "btn-terminal-accent")}
                                >
                                    <i class="fas fa-key"></i>
                                    { t::BTN_LLM_ACCESS }
                                </Link<Route>>
                                <Link<Route>
                                    to={Route::KiroAccess}
                                    classes={classes!("btn-terminal")}
                                >
                                    <i class="fas fa-bolt"></i>
                                    { "Kiro Access" }
                                </Link<Route>>
                            </div>
                        </div>

                        // ── Section 3: Stats Bar ──
                        <div class={classes!("mt-8", "w-full")}>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_STATS }</span>
                            </div>
                            <div class={classes!(
                                "mt-4", "grid", "gap-3", "grid-cols-2",
                                "sm:grid-cols-3", "lg:grid-cols-5", "w-full"
                            )}>
                                { for stats.into_iter().map(|(icon, value, label, route)| {
                                    let panel_content = html! {
                                        <div class="system-panel-compact">
                                            <div class={classes!(
                                                "inline-flex", "h-10", "w-10", "items-center", "justify-center",
                                                "rounded-lg", "border", "border-[var(--border)]",
                                                "bg-[var(--surface-alt)]", "text-[var(--primary)]"
                                            )}>
                                                <Icon name={icon} size={20} />
                                            </div>
                                            <div class={classes!("text-[1.75rem]", "font-bold", "leading-none", "text-[var(--primary)]")}>
                                                if *stats_loaded {
                                                    { value.clone() }
                                                } else {
                                                    <div class="h-7 w-10 rounded bg-[var(--surface-alt)] animate-pulse inline-block" />
                                                }
                                            </div>
                                            <div class={classes!("text-[0.72rem]", "uppercase", "tracking-[0.15em]", "text-[var(--muted)]")}>{ label.clone() }</div>
                                        </div>
                                    };
                                    if let Some(r) = route {
                                        html! {
                                            <Link<Route> to={r} classes={classes!("no-underline")}>
                                                { panel_content }
                                            </Link<Route>>
                                        }
                                    } else {
                                        panel_content
                                    }
                                }) }
                            </div>
                        </div>

                        // SECTIONS_4_5_6_7_PLACEHOLDER

                    </div>
                </section>
            </div>
        </div>
    }
}
```

- [ ] **Step 4: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -40`
Expected: compiles (placeholder comment is fine — sections 4-7 will be added next task).

- [ ] **Step 5: Commit**

```bash
git add frontend/src/pages/home.rs
git commit -m "feat(home): rewrite Hero + Banner + Stats sections"
```

---

### Task 5: Rewrite home.rs — HTML template (Recent Articles + Recent Music)

**Files:**
- Modify: `frontend/src/pages/home.rs` (continuing — replace `// SECTIONS_4_5_6_7_PLACEHOLDER`)

- [ ] **Step 1: Add Section 4 — Recent Articles**

Replace `// SECTIONS_4_5_6_7_PLACEHOLDER` with:

```rust
                        // ── Section 4: Recent Articles ──
                        <div class={classes!("mt-12", "w-full")}>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_RECENT_ARTICLES }</span>
                            </div>
                            <div class={classes!(
                                "mt-4", "grid", "gap-[var(--space-card-gap)]",
                                "grid-cols-1", "md:grid-cols-2", "w-full"
                            )}>
                                if *articles_loaded {
                                    { for recent_articles.iter().map(|article| html! {
                                        <ArticleCard article={article.clone()} />
                                    }) }
                                } else {
                                    { for (0..4).map(|_| html! {
                                        <div class={classes!(
                                            "bg-[var(--surface)]",
                                            "border",
                                            "border-[var(--border)]",
                                            "rounded-xl",
                                            "overflow-hidden",
                                            "animate-pulse"
                                        )}>
                                            <div class="h-48 bg-[var(--surface-alt)]" />
                                            <div class="p-4 space-y-3">
                                                <div class="h-5 w-3/4 rounded bg-[var(--surface-alt)]" />
                                                <div class="h-4 w-full rounded bg-[var(--surface-alt)]" />
                                                <div class="h-4 w-1/2 rounded bg-[var(--surface-alt)]" />
                                            </div>
                                        </div>
                                    }) }
                                }
                            </div>
                            if *articles_loaded && !recent_articles.is_empty() {
                                <div class={classes!("mt-4", "flex", "justify-end")}>
                                    <Link<Route>
                                        to={Route::LatestArticles}
                                        classes={classes!("btn-terminal")}
                                    >
                                        { t::BTN_VIEW_ALL_ARTICLES }
                                    </Link<Route>>
                                </div>
                            }
                        </div>

                        // SECTIONS_5_6_7_PLACEHOLDER
```

- [ ] **Step 2: Add Section 5 — Recent Music**

Replace `// SECTIONS_5_6_7_PLACEHOLDER` with:

```rust
                        // ── Section 5: Recent Music ──
                        <div class={classes!("mt-12", "w-full")}>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_RECENT_MUSIC }</span>
                            </div>
                            <div class={classes!(
                                "mt-4", "grid", "gap-3",
                                "grid-cols-2", "md:grid-cols-4", "w-full"
                            )}>
                                if *songs_loaded {
                                    { for recent_songs.iter().map(|song| {
                                        let cover_url = api::song_cover_url(song.cover_image.as_deref());
                                        let id = song.id.clone();
                                        let has_cover = song.cover_image.as_ref().is_some_and(|c| !c.is_empty());
                                        let duration = format_duration_short(song.duration_ms);
                                        html! {
                                            <Link<Route>
                                                to={Route::MusicPlayer { id }}
                                                classes={classes!(
                                                    "group",
                                                    "bg-[var(--surface)]",
                                                    "border",
                                                    "border-[var(--border)]",
                                                    "rounded-xl",
                                                    "overflow-hidden",
                                                    "flex",
                                                    "flex-col",
                                                    "transition-all",
                                                    "duration-200",
                                                    "hover:border-[var(--primary)]",
                                                    "hover:shadow-[var(--shadow-4)]",
                                                    "no-underline",
                                                    "text-inherit"
                                                )}
                                            >
                                                <div class={classes!(
                                                    "relative",
                                                    "w-full",
                                                    "aspect-square",
                                                    "bg-[var(--surface-alt)]",
                                                    "overflow-hidden"
                                                )}>
                                                    if has_cover {
                                                        <ImageWithLoading
                                                            src={cover_url}
                                                            alt={song.title.clone()}
                                                            class={classes!("w-full", "h-full", "object-cover")}
                                                            container_class={classes!("w-full", "h-full")}
                                                        />
                                                    } else {
                                                        <div class={classes!(
                                                            "w-full",
                                                            "h-full",
                                                            "flex",
                                                            "items-center",
                                                            "justify-center",
                                                            "text-[var(--muted)]"
                                                        )}>
                                                            <Icon name={IconName::Music} size={48} />
                                                        </div>
                                                    }
                                                </div>
                                                <div class={classes!("p-3", "flex", "flex-col", "gap-1", "min-w-0")}>
                                                    <div class={classes!(
                                                        "text-sm",
                                                        "font-semibold",
                                                        "truncate",
                                                        "text-[var(--text)]",
                                                        "group-hover:text-[var(--primary)]"
                                                    )}>
                                                        { &song.title }
                                                    </div>
                                                    <div class={classes!(
                                                        "text-xs",
                                                        "text-[var(--muted)]",
                                                        "truncate"
                                                    )}>
                                                        { &song.artist }
                                                    </div>
                                                    <div class={classes!(
                                                        "text-xs",
                                                        "text-[var(--muted)]"
                                                    )}>
                                                        { duration }
                                                    </div>
                                                </div>
                                            </Link<Route>>
                                        }
                                    }) }
                                } else {
                                    { for (0..4).map(|_| html! {
                                        <div class={classes!(
                                            "bg-[var(--surface)]",
                                            "border",
                                            "border-[var(--border)]",
                                            "rounded-xl",
                                            "overflow-hidden",
                                            "animate-pulse"
                                        )}>
                                            <div class="aspect-square bg-[var(--surface-alt)]" />
                                            <div class="p-3 space-y-2">
                                                <div class="h-4 w-3/4 rounded bg-[var(--surface-alt)]" />
                                                <div class="h-3 w-1/2 rounded bg-[var(--surface-alt)]" />
                                            </div>
                                        </div>
                                    }) }
                                }
                            </div>
                            if *songs_loaded && !recent_songs.is_empty() {
                                <div class={classes!("mt-4", "flex", "justify-end")}>
                                    <Link<Route>
                                        to={Route::MediaAudio}
                                        classes={classes!("btn-terminal")}
                                    >
                                        { t::BTN_VIEW_ALL_MUSIC }
                                    </Link<Route>>
                                </div>
                            }
                        </div>

                        // SECTIONS_6_7_PLACEHOLDER
```

- [ ] **Step 3: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -40`
Expected: compiles with no errors.

- [ ] **Step 4: Commit**

```bash
git add frontend/src/pages/home.rs
git commit -m "feat(home): add Recent Articles and Recent Music sections"
```

---

### Task 6: Rewrite home.rs — HTML template (Tech Stack + Social + Footer)

**Files:**
- Modify: `frontend/src/pages/home.rs` (continuing — replace `// SECTIONS_6_7_PLACEHOLDER`)

- [ ] **Step 1: Add Section 6 — Tech Stack logo strip + Section 7 — Social**

Replace `// SECTIONS_6_7_PLACEHOLDER` with:

```rust
                        // ── Section 6: Tech Stack Logo Strip ──
                        <div class={classes!("mt-12", "w-full")}>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_TECH_STACK }</span>
                            </div>
                            <div class={classes!(
                                "mt-4",
                                "flex",
                                "flex-wrap",
                                "gap-3",
                                "items-center"
                            )}>
                                { for tech_stack.iter().map(|(logo, name, href)| html! {
                                    <a
                                        href={(*href).to_string()}
                                        target="_blank"
                                        rel="noopener noreferrer"
                                        title={*name}
                                        aria-label={(*name).to_string()}
                                        class={classes!(
                                            "inline-flex",
                                            "items-center",
                                            "gap-2",
                                            "px-3",
                                            "py-2",
                                            "rounded-lg",
                                            "border",
                                            "border-[var(--border)]",
                                            "bg-[var(--surface)]",
                                            "text-[var(--text)]",
                                            "text-sm",
                                            "no-underline",
                                            "transition-all",
                                            "duration-150",
                                            "hover:border-[var(--primary)]",
                                            "hover:text-[var(--primary)]",
                                            "hover:shadow-[var(--shadow-2)]"
                                        )}
                                    >
                                        <ImageWithLoading
                                            src={logo.clone()}
                                            alt={*name}
                                            loading={Some(AttrValue::from("lazy"))}
                                            class={classes!("w-5", "h-5", "object-contain")}
                                            container_class={classes!("inline-flex", "w-5", "h-5")}
                                        />
                                        <span>{ *name }</span>
                                    </a>
                                }) }
                            </div>
                        </div>

                        // ── Section 7: Social + GitHub Wrapped ──
                        <div class={classes!("mt-12", "w-full")}>
                            <div class="terminal-line">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_SOCIAL }</span>
                            </div>
                            <div class={classes!("flex", "items-center", "gap-3", "mt-3")}>
                                <a
                                    href="https://github.com/ACking-you"
                                    target="_blank" rel="noopener noreferrer"
                                    aria-label={common_text::GITHUB}
                                    class={classes!(
                                        "btn-fluent-icon",
                                        "border",
                                        "border-[var(--border)]",
                                        "hover:bg-[var(--surface-alt)]",
                                        "hover:text-[var(--primary)]",
                                        "transition-all",
                                        "duration-100",
                                        "ease-[var(--ease-snap)]"
                                    )}
                                >
                                    <i class={classes!("fa-brands", "fa-github-alt", "text-lg")} aria-hidden="true"></i>
                                    <span class={classes!("sr-only")}>{ common_text::GITHUB }</span>
                                </a>
                                <a
                                    href="https://space.bilibili.com/24264499"
                                    target="_blank" rel="noopener noreferrer"
                                    aria-label={common_text::BILIBILI}
                                    class={classes!(
                                        "btn-fluent-icon",
                                        "border",
                                        "border-[var(--border)]",
                                        "hover:bg-[var(--surface-alt)]",
                                        "hover:text-[var(--primary)]",
                                        "transition-all",
                                        "duration-100",
                                        "ease-[var(--ease-snap)]"
                                    )}
                                >
                                    <svg viewBox="0 0 24 24" role="img" aria-hidden="true" focusable="false" width="20" height="20">
                                        <path
                                            fill="currentColor"
                                            d="M17.813 4.653h.854c1.51.054 2.769.578 3.773 1.574 1.004.995 1.524 2.249 1.56 3.76v7.36c-.036 1.51-.556 2.769-1.56 3.773s-2.262 1.524-3.773 1.56H5.333c-1.51-.036-2.769-.556-3.773-1.56S.036 18.858 0 17.347v-7.36c.036-1.511.556-2.765 1.56-3.76 1.004-.996 2.262-1.52 3.773-1.574h.774l-1.174-1.12a1.234 1.234 0 0 1-.373-.906c0-.356.124-.658.373-.907l.027-.027c.267-.249.573-.373.92-.373.347 0 .653.124.92.373L9.653 4.44c.071.071.134.142.187.213h4.267a.836.836 0 0 1 .16-.213l2.853-2.747c.267-.249.573-.373.92-.373.347 0 .662.151.929.4.267.249.391.551.391.907 0 .355-.124.657-.373.906zM5.333 7.24c-.746.018-1.373.276-1.88.773-.506.498-.769 1.13-.786 1.894v7.52c.017.764.28 1.395.786 1.893.507.498 1.134.756 1.88.773h13.334c.746-.017 1.373-.275 1.88-.773.506-.498.769-1.129.786-1.893v-7.52c-.017-.765-.28-1.396-.786-1.894-.507-.497-1.134-.755-1.88-.773zM8 11.107c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c0-.373.129-.689.386-.947.258-.257.574-.386.947-.386zm8 0c.373 0 .684.124.933.373.25.249.383.569.4.96v1.173c-.017.391-.15.711-.4.96-.249.25-.56.374-.933.374s-.684-.125-.933-.374c-.25-.249-.383-.569-.4-.96V12.44c.017-.391.15-.711.4-.96.249-.249.56-.373.933-.373Z"
                                        />
                                    </svg>
                                    <span class={classes!("sr-only")}>{ common_text::BILIBILI }</span>
                                </a>
                            </div>
                            // GitHub Wrapped
                            <div class="terminal-line" style="margin-top: 1.5rem;">
                                <span class="terminal-prompt">{ common_text::TERMINAL_PROMPT_CMD }</span>
                                <span class="terminal-content">{ t::CMD_SHOW_WRAPPED }</span>
                            </div>
                            <GithubWrappedSelector />
                        </div>
```

Note: The `GithubWrappedSelector` component, `WrappedYear` struct, `get_wrapped_years()` function, and `github_wrapped_selector()` function component (lines 618-790 in the original file) remain completely unchanged. They stay at the bottom of `home.rs`.

- [ ] **Step 2: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -40`
Expected: compiles with zero errors and zero warnings.

- [ ] **Step 3: Format the file**

Run: `rustfmt frontend/src/pages/home.rs`

- [ ] **Step 4: Commit**

```bash
git add frontend/src/pages/home.rs
git commit -m "feat(home): add Tech Stack strip and Social footer sections"
```

---

### Task 7: Final verification and cleanup

**Files:**
- All changed files: `frontend/src/pages/home.rs`, `frontend/src/components/image_with_loading.rs`, `frontend/src/i18n/zh_cn.rs`

- [ ] **Step 1: Run full clippy check**

Run: `cargo clippy -p static-flow-frontend 2>&1`
Expected: zero errors, zero warnings.

- [ ] **Step 2: Format all changed files**

Run:
```bash
rustfmt frontend/src/pages/home.rs
rustfmt frontend/src/components/image_with_loading.rs
rustfmt frontend/src/i18n/zh_cn.rs
```

- [ ] **Step 3: Verify WASM build succeeds**

Run: `cd frontend && trunk build 2>&1 | tail -20`
Expected: build succeeds with no errors.

- [ ] **Step 4: Review diff for any leftover dead code**

Run: `git diff --stat HEAD~6` (or however many commits since Task 1)
Verify: no unintended files changed, no leftover `_tech_chip_class` or `HomeTab` references.

- [ ] **Step 5: Final commit if any formatting changes**

```bash
git add -u
git commit -m "chore: format homepage redesign files"
```
