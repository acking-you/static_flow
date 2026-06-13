//! Accessible modal dialogs: focus trap, Escape/backdrop close, scroll lock.
//!
//! [`Modal`] is the generic shell (portal-rendered into `<body>`);
//! [`ConfirmModal`] is the opinionated confirm/cancel dialog that replaces
//! `window.confirm()` for destructive admin actions.

use gloo_events::EventListener;
use gloo_utils::{body, document};
use wasm_bindgen::JsCast;
use web_sys::{HtmlElement, KeyboardEvent};
use yew::prelude::*;

use crate::i18n::current::modal as t;

/// Selector matching everything focusable inside the dialog panel.
const FOCUSABLE: &str = "a[href], button:not([disabled]), input:not([disabled]), \
                         select:not([disabled]), textarea:not([disabled]), \
                         [tabindex]:not([tabindex='-1'])";

/// Props for [`Modal`].
#[derive(Properties, PartialEq)]
pub struct ModalProps {
    /// Whether the dialog is shown.
    pub open: bool,
    /// Title rendered in the dialog header and referenced by aria-labelledby.
    pub title: AttrValue,
    /// Invoked on Escape, backdrop click, or the close button.
    pub on_close: Callback<()>,
    /// Dialog body.
    pub children: Html,
}

/// Generic dialog shell: portal into `<body>`, focus trap, Escape close,
/// backdrop click close, body scroll lock, focus restore on close.
#[function_component(Modal)]
pub fn modal(props: &ModalProps) -> Html {
    let panel_ref = use_node_ref();
    let backdrop_ref = use_node_ref();

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
                        "Escape" => on_close.emit(()),
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
                let body_overflow = body()
                    .style()
                    .get_property_value("overflow")
                    .unwrap_or_default();
                let _ = body().style().set_property("overflow", "hidden");
                cleanup = Some(Box::new(move || {
                    drop(keydown);
                    let _ = body().style().set_property("overflow", &body_overflow);
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

    if !props.open {
        return Html::default();
    }

    let on_backdrop_click = {
        let on_close = props.on_close.clone();
        let backdrop_ref = backdrop_ref.clone();
        Callback::from(move |event: MouseEvent| {
            let target = event.target();
            let backdrop = backdrop_ref.get();
            if let (Some(target), Some(backdrop)) = (target, backdrop) {
                if target.loose_eq(&backdrop) {
                    on_close.emit(());
                }
            }
        })
    };
    let on_close_btn = {
        let on_close = props.on_close.clone();
        Callback::from(move |_| on_close.emit(()))
    };

    create_portal(
        html! {
            <div
                ref={backdrop_ref}
                class={classes!("modal-backdrop")}
                onclick={on_backdrop_click}
            >
                <div
                    ref={panel_ref}
                    class={classes!("modal-panel")}
                    role="dialog"
                    aria-modal="true"
                    aria-labelledby="modal-title"
                    tabindex="-1"
                >
                    <div class={classes!("modal-header")}>
                        <h2 id="modal-title" class={classes!("modal-title")}>{ &props.title }</h2>
                        <button
                            type="button"
                            class={classes!("modal-close")}
                            aria-label={t::CLOSE}
                            onclick={on_close_btn}
                        >
                            <i class="fas fa-xmark" aria-hidden="true"></i>
                        </button>
                    </div>
                    <div class={classes!("modal-body")}>
                        { props.children.clone() }
                    </div>
                </div>
            </div>
        },
        body().into(),
    )
}

/// Props for [`ConfirmModal`].
#[derive(Properties, PartialEq)]
pub struct ConfirmModalProps {
    /// Whether the dialog is shown.
    pub open: bool,
    /// Dialog title.
    pub title: AttrValue,
    /// Explanatory message above the buttons.
    pub message: AttrValue,
    /// Confirm button label; defaults to the localized "confirm".
    #[prop_or_default]
    pub confirm_label: Option<AttrValue>,
    /// Render the confirm button in the destructive (red) style.
    #[prop_or(false)]
    pub danger: bool,
    /// Disables both buttons while the confirmed action runs.
    #[prop_or(false)]
    pub busy: bool,
    /// Invoked when the user confirms.
    pub on_confirm: Callback<()>,
    /// Invoked on cancel, Escape, or backdrop click.
    pub on_cancel: Callback<()>,
}

/// Confirm/cancel dialog for destructive or consequential actions.
#[function_component(ConfirmModal)]
pub fn confirm_modal(props: &ConfirmModalProps) -> Html {
    let on_confirm = {
        let on_confirm = props.on_confirm.clone();
        Callback::from(move |_| on_confirm.emit(()))
    };
    let on_cancel_btn = {
        let on_cancel = props.on_cancel.clone();
        Callback::from(move |_| on_cancel.emit(()))
    };
    let confirm_label = props
        .confirm_label
        .clone()
        .unwrap_or_else(|| AttrValue::from(t::CONFIRM));
    let confirm_class = if props.danger {
        classes!("modal-btn", "modal-btn--danger")
    } else {
        classes!("modal-btn", "modal-btn--primary")
    };
    html! {
        <Modal open={props.open} title={props.title.clone()} on_close={props.on_cancel.clone()}>
            <p class={classes!("modal-message")}>{ &props.message }</p>
            <div class={classes!("modal-actions")}>
                <button
                    type="button"
                    class={classes!("modal-btn", "modal-btn--ghost")}
                    disabled={props.busy}
                    onclick={on_cancel_btn}
                >
                    { t::CANCEL }
                </button>
                <button
                    type="button"
                    class={confirm_class}
                    disabled={props.busy}
                    onclick={on_confirm}
                >
                    if props.busy {
                        <i class="fas fa-spinner fa-spin" aria-hidden="true"></i>
                    }
                    { confirm_label }
                </button>
            </div>
        </Modal>
    }
}
