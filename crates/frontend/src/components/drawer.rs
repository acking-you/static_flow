//! Right-side slide-out panel for record detail / edit views.
//!
//! Admin detail editors used to render inline *below* the table, so selecting a
//! row pushed the table out of view and the editor drowned the list on mobile.
//! A drawer keeps the table in place and overlays the detail, with a backdrop
//! to dismiss. Always mounted so it can animate; gated by `open`.

use std::sync::atomic::{AtomicUsize, Ordering};

use gloo_events::EventListener;
use gloo_utils::document;
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent};
use yew::prelude::*;

/// Monotonic counter backing per-drawer unique ids (for `aria-labelledby`).
static NEXT_TITLE_ID: AtomicUsize = AtomicUsize::new(0);

/// Selector matching everything focusable inside the drawer panel.
const FOCUSABLE: &str = "a[href], button:not([disabled]), input:not([disabled]), \
                         select:not([disabled]), textarea:not([disabled]), \
                         [tabindex]:not([tabindex='-1'])";

/// Props for [`Drawer`].
#[derive(Properties, PartialEq)]
pub struct DrawerProps {
    /// Whether the drawer is visible.
    pub open: bool,
    /// Fired by the backdrop and the close button.
    pub on_close: Callback<MouseEvent>,
    /// Header title.
    #[prop_or_default]
    pub title: AttrValue,
    /// Extra classes on the panel (e.g. a width override).
    #[prop_or_default]
    pub class: Classes,
    /// Drawer body content.
    pub children: Html,
}

/// A dismissible right-side panel.
#[function_component(Drawer)]
pub fn drawer(props: &DrawerProps) -> Html {
    // Stable unique id for the title, wired to the panel via `aria-labelledby`.
    // (`yew::use_id` lands in a later Yew; generate one ourselves on 0.21.)
    let title_id = use_memo((), |_| {
        let n = NEXT_TITLE_ID.fetch_add(1, Ordering::Relaxed);
        AttrValue::from(format!("admin-drawer-title-{n}"))
    });
    let title_id = (*title_id).clone();
    let panel_ref = use_node_ref();
    let open_class = props.open.then_some("admin-drawer-root--open");

    {
        let panel_ref = panel_ref.clone();
        let on_close = props.on_close.clone();
        use_effect_with(props.open, move |open| {
            let mut cleanup: Option<Box<dyn FnOnce()>> = None;
            if *open {
                let previously_focused = document()
                    .active_element()
                    .and_then(|el| el.dyn_into::<HtmlElement>().ok());
                if let Some(panel) = panel_ref.cast::<HtmlElement>() {
                    let _ = panel.focus();
                }
                let keydown = EventListener::new(&document(), "keydown", move |event| {
                    let Some(event) = event.dyn_ref::<KeyboardEvent>() else {
                        return;
                    };
                    match event.key().as_str() {
                        "Escape" => {
                            if let Ok(synthetic) = web_sys::MouseEvent::new("click") {
                                on_close.emit(synthetic);
                            }
                        },
                        "Tab" => {
                            let Some(panel) = panel_ref.cast::<HtmlElement>() else {
                                return;
                            };
                            let Ok(nodes) = panel.query_selector_all(FOCUSABLE) else {
                                return;
                            };
                            if nodes.length() == 0 {
                                event.prevent_default();
                                return;
                            }
                            let first = nodes.get(0).and_then(|n| n.dyn_into::<HtmlElement>().ok());
                            let last = nodes
                                .get(nodes.length() - 1)
                                .and_then(|n| n.dyn_into::<HtmlElement>().ok());
                            let active = document()
                                .active_element()
                                .and_then(|el| el.dyn_into::<HtmlElement>().ok());
                            let on_first = active.is_some() && active == first;
                            let on_last = active.is_some() && active == last;
                            if event.shift_key() && on_first {
                                event.prevent_default();
                                if let Some(last) = last {
                                    let _ = last.focus();
                                }
                            } else if !event.shift_key() && on_last {
                                event.prevent_default();
                                if let Some(first) = first {
                                    let _ = first.focus();
                                }
                            }
                        },
                        _ => {},
                    }
                });
                cleanup = Some(Box::new(move || {
                    drop(keydown);
                    if let Some(el) = previously_focused {
                        let _ = el.focus();
                    }
                }));
            }
            move || {
                if let Some(cleanup) = cleanup {
                    cleanup();
                }
            }
        });
    }

    html! {
        <div
            class={classes!("admin-drawer-root", open_class)}
            aria-hidden={if props.open { "false" } else { "true" }}
        >
            <div class={classes!("admin-drawer-backdrop")} onclick={props.on_close.clone()} />
            <aside
                ref={panel_ref}
                class={classes!("admin-drawer", props.class.clone())}
                role="dialog"
                aria-modal="true"
                aria-labelledby={title_id.clone()}
                tabindex="-1"
            >
                <header class={classes!("admin-drawer__header")}>
                    <h3 id={title_id} class={classes!("admin-drawer__title")}>{ props.title.clone() }</h3>
                    <button
                        type="button"
                        class={classes!("admin-drawer__close")}
                        onclick={props.on_close.clone()}
                        aria-label="关闭"
                    >
                        <i class="fas fa-xmark" aria-hidden="true"></i>
                    </button>
                </header>
                <div class={classes!("admin-drawer__body")}>
                    { props.children.clone() }
                </div>
            </aside>
        </div>
    }
}
