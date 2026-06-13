//! Right-side slide-out panel for record detail / edit views.
//!
//! Admin detail editors used to render inline *below* the table, so selecting a
//! row pushed the table out of view and the editor drowned the list on mobile.
//! A drawer keeps the table in place and overlays the detail, with a backdrop
//! to dismiss. Always mounted so it can animate; gated by `open`.

use yew::prelude::*;

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
    let open_class = props.open.then_some("admin-drawer-root--open");
    html! {
        <div
            class={classes!("admin-drawer-root", open_class)}
            aria-hidden={(!props.open).to_string()}
        >
            <div class={classes!("admin-drawer-backdrop")} onclick={props.on_close.clone()} />
            <aside class={classes!("admin-drawer", props.class.clone())} role="dialog" aria-modal="true">
                <header class={classes!("admin-drawer__header")}>
                    <h3 class={classes!("admin-drawer__title")}>{ props.title.clone() }</h3>
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
