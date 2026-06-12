//! Global toast notifications: a context-backed stack rendered top-right.
//!
//! Mount [`ToastProvider`] once near the app root, then call
//! [`use_toast`] in any page or component:
//!
//! ```ignore
//! let toast = use_toast();
//! toast.success("已保存");
//! toast.error("保存失败，请重试");
//! ```
//!
//! The provider exposes only the reducer dispatcher (stable identity), so
//! pushing a toast re-renders the viewport alone, never the consumers.

use std::rc::Rc;

use yew::prelude::*;
use yew_hooks::prelude::use_timeout;

use crate::i18n::current::toast as t;

/// Visual flavor of one toast message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    /// Confirmation of a completed user action.
    Success,
    /// A failed user action; sticks around longer.
    Error,
    /// Neutral informational notice.
    Info,
}

/// One queued toast.
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    /// Monotonic id used as the render key and dismiss handle.
    pub id: u64,
    /// Visual flavor.
    pub kind: ToastKind,
    /// Already-localized message text.
    pub message: String,
}

/// Reducer actions for the toast stack.
pub enum ToastAction {
    /// Append a toast (oldest is dropped beyond the stack cap).
    Push(ToastKind, String),
    /// Remove a toast by id (timeout or manual close).
    Dismiss(u64),
}

/// Newest-last toast stack; capped so a burst cannot flood the screen.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToastList {
    /// Live toasts, oldest first.
    pub items: Vec<Toast>,
    next_id: u64,
}

/// Maximum simultaneously visible toasts.
const TOAST_STACK_CAP: usize = 4;
/// Auto-dismiss delay for success/info toasts.
const TOAST_SUCCESS_MS: u32 = 4_000;
/// Auto-dismiss delay for error toasts (longer: users must read these).
const TOAST_ERROR_MS: u32 = 6_000;

impl Reducible for ToastList {
    type Action = ToastAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            ToastAction::Push(kind, message) => {
                next.items.push(Toast {
                    id: next.next_id,
                    kind,
                    message,
                });
                next.next_id += 1;
                if next.items.len() > TOAST_STACK_CAP {
                    let overflow = next.items.len() - TOAST_STACK_CAP;
                    next.items.drain(..overflow);
                }
            },
            ToastAction::Dismiss(id) => {
                next.items.retain(|toast| toast.id != id);
            },
        }
        Rc::new(next)
    }
}

/// Handle pages use to push toasts; clones share the same dispatcher.
#[derive(Clone, PartialEq)]
pub struct ToastHandle {
    dispatcher: UseReducerDispatcher<ToastList>,
}

impl ToastHandle {
    /// Show a success toast (auto-dismisses).
    pub fn success(&self, message: impl Into<String>) {
        self.dispatcher
            .dispatch(ToastAction::Push(ToastKind::Success, message.into()));
    }

    /// Show an error toast (auto-dismisses, longer delay).
    pub fn error(&self, message: impl Into<String>) {
        self.dispatcher
            .dispatch(ToastAction::Push(ToastKind::Error, message.into()));
    }

    /// Show a neutral info toast (auto-dismisses).
    pub fn info(&self, message: impl Into<String>) {
        self.dispatcher
            .dispatch(ToastAction::Push(ToastKind::Info, message.into()));
    }
}

/// Access the toast handle; panics if [`ToastProvider`] is not mounted.
#[hook]
pub fn use_toast() -> ToastHandle {
    use_context::<ToastHandle>().expect("ToastProvider must wrap the app")
}

/// Props for [`ToastProvider`].
#[derive(Properties, PartialEq)]
pub struct ToastProviderProps {
    /// Subtree that gains access to [`use_toast`].
    pub children: Html,
}

/// Mounts the toast context and the fixed viewport that renders the stack.
#[function_component(ToastProvider)]
pub fn toast_provider(props: &ToastProviderProps) -> Html {
    let state = use_reducer(ToastList::default);
    let handle = ToastHandle {
        dispatcher: state.dispatcher(),
    };
    let on_dismiss = {
        let dispatcher = state.dispatcher();
        Callback::from(move |id: u64| dispatcher.dispatch(ToastAction::Dismiss(id)))
    };
    html! {
        <ContextProvider<ToastHandle> context={handle}>
            { props.children.clone() }
            <div class={classes!("toast-viewport")} aria-live="polite">
                { for state.items.iter().map(|toast| html! {
                    <ToastItem
                        key={toast.id}
                        toast={toast.clone()}
                        on_dismiss={on_dismiss.clone()}
                    />
                }) }
            </div>
        </ContextProvider<ToastHandle>>
    }
}

/// Props for one rendered toast.
#[derive(Properties, PartialEq)]
struct ToastItemProps {
    toast: Toast,
    on_dismiss: Callback<u64>,
}

/// A single toast row: kind icon, message, close button, auto-dismiss timer.
#[function_component(ToastItem)]
fn toast_item(props: &ToastItemProps) -> Html {
    let id = props.toast.id;
    let delay = match props.toast.kind {
        ToastKind::Error => TOAST_ERROR_MS,
        ToastKind::Success | ToastKind::Info => TOAST_SUCCESS_MS,
    };
    // Arms on mount; the hook cancels the timer when the item unmounts.
    let _auto_timeout = {
        let on_dismiss = props.on_dismiss.clone();
        use_timeout(move || on_dismiss.emit(id), delay)
    };

    let (kind_class, icon, role) = match props.toast.kind {
        ToastKind::Success => ("toast--success", "fa-circle-check", "status"),
        ToastKind::Error => ("toast--error", "fa-circle-exclamation", "alert"),
        ToastKind::Info => ("toast--info", "fa-circle-info", "status"),
    };
    let on_close = {
        let on_dismiss = props.on_dismiss.clone();
        Callback::from(move |_| on_dismiss.emit(id))
    };
    html! {
        <div class={classes!("toast", kind_class)} role={role}>
            <i class={classes!("fas", icon, "toast-icon")} aria-hidden="true"></i>
            <span class={classes!("toast-message")}>{ &props.toast.message }</span>
            <button
                type="button"
                class={classes!("toast-close")}
                aria-label={t::CLOSE}
                onclick={on_close}
            >
                <i class="fas fa-xmark" aria-hidden="true"></i>
            </button>
        </div>
    }
}
