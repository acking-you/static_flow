use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::HtmlElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    components::icons::{Icon, IconName},
    router::Route,
};

#[derive(Properties, PartialEq, Clone)]
pub struct StatsCardProps {
    pub icon: IconName,
    pub value: String,
    pub label: String, // 添加文字标签用于tooltip
    #[prop_or_default]
    pub route: Option<Route>,
}

#[function_component(StatsCard)]
pub fn stats_card(props: &StatsCardProps) -> Html {
    let card_ref = use_node_ref();
    const MAX_TILT: f64 = 12.0;
    const MAGNETIC_RADIUS: f64 = 150.0;
    const MAGNETIC_MAX_OFFSET: f64 = 15.0;

    let card_classes = classes!(
        "group",
        "relative",
        "bg-[var(--surface)]",
        "border-t", "border-r", "border-b", "border-[var(--border)]",
        "border-l-[4px]",
        "border-l-[var(--primary)]",
        "rounded-lg",
        "p-6",
        "flex",
        "items-center",
        "gap-4",
        "shadow-[var(--shadow-2)]",
        "overflow-hidden",
        "transform-gpu",
        "card-3d-container",
        "liquid-card",
        "stats-card",
        "text-[var(--text)]",
        "no-underline"
    );

    let icon_classes = classes!(
        "flex",
        "items-center",
        "justify-center",
        "w-12",
        "h-12",
        "shrink-0",
        "rounded-lg",
        "bg-[var(--surface-alt)]",
        "text-[var(--primary)]"
    );

    let value_classes = classes!("block", "text-3xl", "text-[var(--text)]", "font-bold", "leading-none");

    let label_classes = classes!(
        "text-sm",
        "text-[var(--muted)]"
    );

    // Ripple interaction with mouse position
    let on_card_click = {
        let card_ref = card_ref.clone();
        Callback::from(move |e: MouseEvent| {
            if let Some(card) = card_ref.cast::<web_sys::Element>() {
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    let rect = card.get_bounding_client_rect();
                    let x = e.client_x() as f64 - rect.left();
                    let y = e.client_y() as f64 - rect.top();
                    if let Ok(span) = doc.create_element("span") {
                        let _ = span.set_attribute("class", "stats-ripple");
                        let _ = span.set_attribute(
                            "style",
                            &format!("--ripple-x:{}px; --ripple-y:{}px;", x, y),
                        );
                        let _ = card.append_child(&span);
                        if let Some(win) = web_sys::window() {
                            let span_clone = span.clone();
                            let timeout_closure = Closure::once(move || {
                                let _ = span_clone.remove();
                            });
                            let _ = win.set_timeout_with_callback_and_timeout_and_arguments_0(
                                timeout_closure.as_ref().unchecked_ref(),
                                650,
                            );
                            timeout_closure.forget();
                        }
                    }
                }
            }
        })
    };

    let on_mouse_move = {
        let card_ref = card_ref.clone();
        Callback::from(move |e: MouseEvent| {
            if let Some(element) = card_ref.cast::<web_sys::Element>() {
                let rect = element.get_bounding_client_rect();
                let width = rect.width();
                let height = rect.height();

                if width <= 0.0 || height <= 0.0 {
                    return;
                }

                let rel_x = ((e.client_x() as f64 - rect.left()) / width).clamp(0.0, 1.0);
                let rel_y = ((e.client_y() as f64 - rect.top()) / height).clamp(0.0, 1.0);
                let offset_x = (rel_x - 0.5) * 30.0;
                let offset_y = (rel_y - 0.5) * 30.0;
                if let Some(html_element) = element.dyn_ref::<HtmlElement>() {
                    let center_x = rect.left() + width / 2.0;
                    let center_y = rect.top() + height / 2.0;
                    let norm_x =
                        ((e.client_x() as f64 - center_x) / (width / 2.0)).clamp(-1.0, 1.0);
                    let norm_y =
                        ((e.client_y() as f64 - center_y) / (height / 2.0)).clamp(-1.0, 1.0);
                    let tilt_x = (-norm_y * MAX_TILT).clamp(-MAX_TILT, MAX_TILT);
                    let tilt_y = (norm_x * MAX_TILT).clamp(-MAX_TILT, MAX_TILT);
                    let dx = e.client_x() as f64 - center_x;
                    let dy = e.client_y() as f64 - center_y;
                    let distance = (dx * dx + dy * dy).sqrt();
                    let (magnetic_x, magnetic_y) = if distance < MAGNETIC_RADIUS && distance > 0.0
                    {
                        let influence = (1.0 - distance / MAGNETIC_RADIUS).clamp(0.0, 1.0);
                        let dir_x = dx / distance;
                        let dir_y = dy / distance;
                        let offset_x = dir_x * MAGNETIC_MAX_OFFSET * influence;
                        let offset_y = dir_y * MAGNETIC_MAX_OFFSET * influence;
                        (offset_x, offset_y)
                    } else {
                        (0.0, 0.0)
                    };
                    let style = html_element.style();
                    let _ = style.set_property("--morph-x", &format!("{offset_x}px"));
                    let _ = style.set_property("--morph-y", &format!("{offset_y}px"));
                    let _ = style.set_property("--tilt-x", &format!("{tilt_x:.2}deg"));
                    let _ = style.set_property("--tilt-y", &format!("{tilt_y:.2}deg"));
                    let _ = style.set_property("--magnetic-x", &format!("{magnetic_x:.2}px"));
                    let _ = style.set_property("--magnetic-y", &format!("{magnetic_y:.2}px"));
                }
            }
        })
    };

    let on_mouse_leave = {
        let card_ref = card_ref.clone();
        Callback::from(move |_| {
            if let Some(element) = card_ref.cast::<web_sys::Element>() {
                if let Some(html_element) = element.dyn_ref::<HtmlElement>() {
                    let style = html_element.style();
                    let _ = style.set_property("--morph-x", "0px");
                    let _ = style.set_property("--morph-y", "0px");
                    let _ = style.set_property("--tilt-x", "0deg");
                    let _ = style.set_property("--tilt-y", "0deg");
                    let _ = style.set_property("--magnetic-x", "0px");
                    let _ = style.set_property("--magnetic-y", "0px");
                }
            }
        })
    };

    let content = html! {
        <>
            <span class={icon_classes} aria-hidden="true">
                <Icon name={props.icon} size={28} />
            </span>
            <div class={classes!("flex", "flex-col", "gap-1", "items-start", "min-w-0")}>
                <strong class={value_classes}>{ props.value.clone() }</strong>
                <span class={label_classes}>{ props.label.clone() }</span>
            </div>
        </>
    };

    if let Some(route) = &props.route {
        html! {
            <div
                ref={card_ref}
                class={card_classes}
                onclick={on_card_click}
                onmousemove={on_mouse_move.clone()}
                onmouseleave={on_mouse_leave.clone()}
            >
                <Link<Route> to={route.clone()} classes="contents">
                    { content }
                </Link<Route>>
            </div>
        }
    } else {
        html! {
            <div
                class={card_classes}
                role="status"
                title={props.label.clone()}
                ref={card_ref}
                onclick={on_card_click}
                onmousemove={on_mouse_move}
                onmouseleave={on_mouse_leave}
            >
                { content }
            </div>
        }
    }
}
