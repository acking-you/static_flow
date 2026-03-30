# Homepage Page-Swipe Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the homepage into two horizontally swipeable pages (system_info.sh and explore.sh) with CSS scroll-snap, PC mouse drag enhancement, and a floating acrylic glass page indicator.

**Architecture:** Use CSS `scroll-snap-type: x mandatory` for native touch swipe on mobile. Add a lightweight mouse drag layer for PC. A floating `PageDots` indicator (inline Yew component) syncs with scroll position via `scroll` event. All visual styles go in `input.css` using existing CSS variables for dark/light mode.

**Tech Stack:** Yew (Rust WASM), Tailwind CSS v4, CSS scroll-snap, `web_sys` DOM APIs for scroll/mouse events.

---

### Task 1: Add CSS styles for page slider, slides, and dots

**Files:**
- Modify: `frontend/input.css` (append inside `@layer components { ... }`)

- [ ] **Step 1: Find the insertion point**

Open `frontend/input.css` and locate the end of the `@layer components { }` block. Add the new styles before the closing `}`.

- [ ] **Step 2: Add page slider and slide styles**

```css
  /* ==========================================
     Page Slider — horizontal scroll-snap
     ========================================== */
  .page-slider {
    display: flex;
    overflow-x: auto;
    scroll-snap-type: x mandatory;
    scroll-behavior: smooth;
    scrollbar-width: none;
    -webkit-overflow-scrolling: touch;
    cursor: grab;
  }
  .page-slider::-webkit-scrollbar {
    display: none;
  }
  .page-slider.dragging {
    cursor: grabbing;
    scroll-behavior: auto;
    user-select: none;
  }

  .page-slide {
    min-width: 100%;
    scroll-snap-align: start;
    scroll-snap-stop: always;
    flex-shrink: 0;
    transition: opacity 0.5s var(--ease-spring),
                transform 0.5s var(--ease-spring);
  }
  .page-slide.inactive {
    opacity: 0.6;
    transform: scale(0.97);
  }
```

- [ ] **Step 3: Add page dots indicator styles**

```css
  /* ==========================================
     Page Dots — floating acrylic pill indicator
     ========================================== */
  .page-dots {
    position: fixed;
    bottom: 1.5rem;
    left: 50%;
    transform: translateX(-50%);
    z-index: 50;
    display: flex;
    align-items: center;
    gap: 0.625rem;
    padding: 0.5rem 0.875rem;
    border-radius: 9999px;
    background: rgba(var(--surface-rgb), 0.72);
    backdrop-filter: blur(var(--acrylic-blur)) saturate(var(--acrylic-saturate));
    border: 1px solid rgba(var(--primary-rgb), 0.15);
    box-shadow: var(--shadow-4);
  }

  .page-dot {
    width: 1.75rem;
    height: 0.375rem;
    border-radius: 9999px;
    background: var(--muted);
    opacity: 0.35;
    transition: all 0.4s var(--ease-spring);
    cursor: pointer;
  }

  .page-dot.active {
    background: var(--primary);
    opacity: 1;
    box-shadow: 0 0 8px rgba(var(--primary-rgb), 0.5);
    animation: dot-pulse 2s ease-in-out infinite;
  }

  @keyframes dot-pulse {
    0%, 100% { box-shadow: 0 0 8px rgba(var(--primary-rgb), 0.5); }
    50% { box-shadow: 0 0 14px rgba(var(--primary-rgb), 0.8); }
  }

  /* Dark mode enhancements */
  [data-theme="dark"] .page-dots,
  [theme="dark"] .page-dots {
    border-color: rgba(var(--primary-rgb), 0.25);
    box-shadow: var(--shadow-4), 0 0 12px rgba(var(--primary-rgb), 0.1);
  }

  [data-theme="dark"] .page-dot.active,
  [theme="dark"] .page-dot.active {
    box-shadow: 0 0 10px rgba(var(--primary-rgb), 0.7);
  }

  /* Responsive */
  @media (max-width: 767px) {
    .page-dots {
      bottom: 1rem;
    }
  }
```

- [ ] **Step 4: Verify CSS parses**

Run: `cd frontend && npx tailwindcss -i input.css -o /dev/null 2>&1 | head -10`
Expected: no parse errors.

- [ ] **Step 5: Commit**

```bash
git add frontend/input.css
git commit -m "feat: add page-slider and page-dots CSS styles"
```

---

### Task 2: Restructure home.rs HTML into two slides + add PageDots + event handlers

**Files:**
- Modify: `frontend/src/pages/home.rs`

This is the main task. It restructures the HTML, adds the `active_page` state, scroll/mouse event handlers, and the inline `PageDots` component.

- [ ] **Step 1: Add new imports**

At the top of `frontend/src/pages/home.rs`, add `web_sys` imports needed for DOM manipulation. Change the existing import block to:

```rust
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{console, HtmlElement, ScrollBehavior, ScrollToOptions};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{self, SongListItem},
    components::{
        article_card::ArticleCard,
        icons::{Icon, IconName},
        image_with_loading::ImageWithLoading,
    },
    i18n::current::{common as common_text, home as t},
    models::ArticleListItem,
    router::Route,
};
```

Key changes: added `wasm_bindgen::prelude::*`, `web_sys::HtmlElement`.

- [ ] **Step 2: Add `active_page` state and slider ref**

Inside `home_page()`, after the existing state declarations (after `let songs_loaded = ...`), add:

```rust
    let active_page = use_state(|| 0usize);
    let slider_ref = use_node_ref();
```

- [ ] **Step 3: Add scroll event handler for page tracking**

After the `use_effect_with` data fetching block (after `|| ()`), add a new effect that attaches a scroll listener:

```rust
    // Scroll listener to track active page
    {
        let slider_ref = slider_ref.clone();
        let active_page = active_page.clone();
        use_effect_with((), move |_| {
            let slider_ref = slider_ref.clone();
            let active_page = active_page.clone();
            let raf_id = std::rc::Rc::new(std::cell::Cell::new(0i32));

            let closure = {
                let slider_ref = slider_ref.clone();
                let active_page = active_page.clone();
                let raf_id = raf_id.clone();
                Closure::<dyn Fn()>::new(move || {
                    if let Some(el) = slider_ref.cast::<HtmlElement>() {
                        let scroll_left = el.scroll_left() as f64;
                        let width = el.client_width() as f64;
                        if width > 0.0 {
                            let page = ((scroll_left / width) + 0.5) as usize;
                            if page != *active_page {
                                active_page.set(page);
                            }
                        }
                    }
                    raf_id.set(0);
                })
            };

            let scroll_closure = {
                let raf_id = raf_id.clone();
                let closure_ref = std::rc::Rc::new(closure);
                let closure_for_scroll = closure_ref.clone();
                Closure::<dyn Fn()>::new(move || {
                    if raf_id.get() == 0 {
                        if let Some(win) = web_sys::window() {
                            if let Ok(id) = win.request_animation_frame(
                                closure_for_scroll.as_ref().unchecked_ref(),
                            ) {
                                raf_id.set(id);
                            }
                        }
                    }
                })
            };

            if let Some(el) = slider_ref.cast::<HtmlElement>() {
                let _ = el.add_event_listener_with_callback(
                    "scroll",
                    scroll_closure.as_ref().unchecked_ref(),
                );
            }

            // Prevent closure from being dropped
            scroll_closure.forget();

            || ()
        });
    }
```

- [ ] **Step 4: Add mouse drag handlers for PC**

After the scroll listener effect, add mouse drag state and callbacks:

```rust
    // Mouse drag for PC
    let is_dragging = use_state(|| false);
    let drag_start_x = use_state(|| 0i32);
    let drag_start_scroll = use_state(|| 0i32);
    let drag_moved = use_state(|| false);

    let on_mouse_down = {
        let slider_ref = slider_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_start_x = drag_start_x.clone();
        let drag_start_scroll = drag_start_scroll.clone();
        let drag_moved = drag_moved.clone();
        Callback::from(move |e: MouseEvent| {
            if let Some(el) = slider_ref.cast::<HtmlElement>() {
                is_dragging.set(true);
                drag_start_x.set(e.client_x());
                drag_start_scroll.set(el.scroll_left());
                drag_moved.set(false);
                let _ = el.class_list().add_1("dragging");
            }
        })
    };

    let on_mouse_move = {
        let slider_ref = slider_ref.clone();
        let is_dragging = is_dragging.clone();
        let drag_start_x = drag_start_x.clone();
        let drag_start_scroll = drag_start_scroll.clone();
        let drag_moved = drag_moved.clone();
        Callback::from(move |e: MouseEvent| {
            if !*is_dragging {
                return;
            }
            let dx = e.client_x() - *drag_start_x;
            if dx.abs() > 5 {
                drag_moved.set(true);
            }
            if let Some(el) = slider_ref.cast::<HtmlElement>() {
                el.set_scroll_left(*drag_start_scroll - dx);
            }
        })
    };

    let on_mouse_up = {
        let slider_ref = slider_ref.clone();
        let is_dragging = is_dragging.clone();
        Callback::from(move |_: MouseEvent| {
            if *is_dragging {
                is_dragging.set(false);
                if let Some(el) = slider_ref.cast::<HtmlElement>() {
                    let _ = el.class_list().remove_1("dragging");
                }
            }
        })
    };

    let on_mouse_leave = on_mouse_up.clone();
```

- [ ] **Step 5: Add dot click handler**

```rust
    let on_dot_click = {
        let slider_ref = slider_ref.clone();
        Callback::from(move |page: usize| {
            if let Some(el) = slider_ref.cast::<HtmlElement>() {
                let width = el.client_width();
                let mut opts = web_sys::ScrollToOptions::new();
                opts.left((page as f64) * (width as f64));
                opts.behavior(web_sys::ScrollBehavior::Smooth);
                el.scroll_to_with_scroll_to_options(&opts);
            }
        })
    };
```

- [ ] **Step 6: Restructure the HTML template**

Replace the entire `html! { ... }` block. The new structure wraps existing content into two slides:

The outer wrapper becomes:
```rust
    html! {
        <div class={classes!(
            "relative",
            "w-full",
            "min-h-screen",
            "bg-[var(--bg)]",
            "overflow-x-hidden"
        )}>
            // Page dots indicator
            <div class="page-dots">
                <div
                    class={if *active_page == 0 { "page-dot active" } else { "page-dot" }}
                    onclick={let cb = on_dot_click.clone(); Callback::from(move |_: MouseEvent| cb.emit(0))}
                />
                <div
                    class={if *active_page == 1 { "page-dot active" } else { "page-dot" }}
                    onclick={let cb = on_dot_click.clone(); Callback::from(move |_: MouseEvent| cb.emit(1))}
                />
            </div>

            // Page slider container
            <div
                class="page-slider"
                ref={slider_ref.clone()}
                onmousedown={on_mouse_down}
                onmousemove={on_mouse_move}
                onmouseup={on_mouse_up}
                onmouseleave={on_mouse_leave}
            >
                // Page 0: system_info.sh
                <div class={if *active_page == 0 { "page-slide" } else { "page-slide inactive" }}>
                    <div class={classes!("w-full", "pb-6")}>
                        <section class={classes!(
                            "relative", "py-20", "md:py-24", "px-4",
                            "max-[767px]:pb-16", "max-w-5xl", "mx-auto"
                        )}>
                            <div class={classes!(
                                "w-full", "mx-auto",
                                "px-[clamp(1rem,4vw,2rem)]"
                            )}>
                                // === EXISTING Section 1 (Hero Terminal) ===
                                // === EXISTING Section 2 (LLM Banner) ===
                                // === EXISTING Section 3 (Stats Bar) ===
                                // (all unchanged — copy from current lines 278-503)
                            </div>
                        </section>
                    </div>
                </div>

                // Page 1: explore.sh
                <div class={if *active_page == 1 { "page-slide" } else { "page-slide inactive" }}>
                    <div class={classes!("w-full", "pb-6")}>
                        <section class={classes!(
                            "relative", "py-20", "md:py-24", "px-4",
                            "max-[767px]:pb-16", "max-w-5xl", "mx-auto"
                        )}>
                            <div class={classes!(
                                "w-full", "mx-auto",
                                "px-[clamp(1rem,4vw,2rem)]"
                            )}>
                                // === EXISTING Sections 4-7 (Explore Terminal) ===
                                // (all unchanged — copy from current lines 505-712)
                            </div>
                        </section>
                    </div>
                </div>
            </div>
        </div>
    }
```

The key structural change: the current single `<section>` containing everything is split into two `<div class="page-slide">` elements, each with its own `<section>`. Page 0 gets Sections 1-3 (Hero + Banner + Stats). Page 1 gets Sections 4-7 (Explore terminal). All inner HTML stays exactly the same — just re-parented.

Remove `"pb-8"` from the outer div (the slides handle their own padding). Remove `"overflow-x-hidden"` from the outer div (the slider handles overflow).

- [ ] **Step 7: Verify compilation**

Run: `cargo clippy -p static-flow-frontend 2>&1 | head -50`
Expected: zero errors. May have warnings about `drag_moved` being unused (it's for future click suppression) — suppress with `let _drag_moved = drag_moved;` or use it.

- [ ] **Step 8: Format**

Run: `rustfmt frontend/src/pages/home.rs`

- [ ] **Step 9: Commit**

```bash
git add frontend/src/pages/home.rs
git commit -m "feat(home): add horizontal page-swipe with scroll-snap and floating dots"
```

---

### Task 3: Final verification and cleanup

**Files:**
- All changed files: `frontend/src/pages/home.rs`, `frontend/input.css`

- [ ] **Step 1: Run full clippy check**

Run: `cargo clippy -p static-flow-frontend 2>&1`
Expected: zero errors, zero warnings.

- [ ] **Step 2: Format all changed files**

```bash
rustfmt frontend/src/pages/home.rs
```

- [ ] **Step 3: Check for dead code**

Grep for any leftover references that should have been removed:
- `"overflow-x-hidden"` should NOT appear in the outer wrapper div (slider handles it)
- `"pb-8"` should NOT appear in the outer wrapper div

- [ ] **Step 4: Final commit if needed**

```bash
git add -u
git commit -m "chore: cleanup page-swipe implementation"
```
