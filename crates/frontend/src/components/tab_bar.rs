//! Shared admin tab bar.
//!
//! Replaces the duplicated `render_tab_bar` helpers previously defined in
//! `pages/admin_llm_gateway.rs` and `pages/admin_kiro_gateway.rs`. Behavior is
//! identical to the LLM-gateway version (which is a superset: it supports an
//! optional badge count on a single tab). Call with `badge = None` for the
//! Kiro variant.
//!
//! Keyboard: ArrowLeft / ArrowRight rotate through tabs, Home / End jump to
//! the first / last tab — matches the WAI-ARIA tab pattern so keyboard-only
//! users can navigate without reaching for the mouse.

use wasm_bindgen::JsCast;
use web_sys::KeyboardEvent;
use yew::prelude::*;

/// Render a horizontal tab bar.
///
/// * `active` — id of the currently active tab.
/// * `tabs` — ordered `(id, label)` pairs.
/// * `on_select` — fires with the clicked (or keyboard-rotated) tab id.
/// * `badge` — optional `(tab_id, count)`; shows a pending-count pill on that
///   tab when `count > 0`.
pub fn render_tab_bar(
    active: &str,
    tabs: &[(&str, &str)],
    on_select: &Callback<String>,
    badge: Option<(&str, usize)>,
) -> Html {
    // Snapshot of ids, cheap to clone into the keyboard handler closure.
    let tab_ids: Vec<String> = tabs.iter().map(|(id, _)| (*id).to_string()).collect();

    html! {
        <nav class={classes!(
            "flex", "items-center", "gap-1.5", "flex-wrap",
            "rounded-xl", "border", "border-[var(--border)]",
            "bg-[var(--surface)]", "p-1.5"
        )} role="tablist">
            { for tabs.iter().enumerate().map(|(index, (id, label))| {
                let is_active = active == *id;
                let id_owned = id.to_string();
                let on_select_click = on_select.clone();
                let on_select_key = on_select.clone();
                let tab_ids_for_key = tab_ids.clone();
                let badge_count = badge
                    .filter(|(bid, count)| *bid == *id && *count > 0)
                    .map(|(_, count)| count);
                let onkeydown = Callback::from(move |e: KeyboardEvent| {
                    let total = tab_ids_for_key.len();
                    if total == 0 {
                        return;
                    }
                    let target = match e.key().as_str() {
                        "ArrowRight" => (index + 1) % total,
                        "ArrowLeft" => (index + total - 1) % total,
                        "Home" => 0,
                        "End" => total - 1,
                        _ => return,
                    };
                    e.prevent_default();
                    let target_id = tab_ids_for_key[target].clone();
                    on_select_key.emit(target_id.clone());
                    // Move real DOM focus to the rotated-to tab so the roving
                    // tabindex stays in sync (focus() ignores the momentary
                    // tabindex=-1; the re-render flips it back to 0).
                    if let Some(el) = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.get_element_by_id(&format!("sf-admin-tab-{target_id}")))
                        .and_then(|el| el.dyn_into::<web_sys::HtmlElement>().ok())
                    {
                        let _ = el.focus();
                    }
                });
                html! {
                    <button
                        type="button"
                        role="tab"
                        id={format!("sf-admin-tab-{id}")}
                        aria-selected={is_active.to_string()}
                        tabindex={if is_active { "0" } else { "-1" }}
                        class={classes!(
                            "btn-terminal",
                            if is_active { "btn-terminal-primary" } else { "" }
                        )}
                        onclick={Callback::from(move |_| on_select_click.emit(id_owned.clone()))}
                        onkeydown={onkeydown}
                    >
                        { *label }
                        if let Some(count) = badge_count {
                            <span class={classes!(
                                "ml-1.5", "inline-flex", "items-center", "justify-center",
                                "min-w-[1.25rem]", "h-5", "rounded-full",
                                "bg-amber-500", "text-white", "text-[10px]", "font-bold"
                            )}>
                                { count }
                            </span>
                        }
                    </button>
                }
            }) }
        </nav>
    }
}
