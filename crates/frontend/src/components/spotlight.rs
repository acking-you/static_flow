//! Global cursor-following glow, visible only in dark mode.
//!
//! A single fixed-position element trails the pointer with an eased follow
//! (rAF lerp toward the live cursor target). It is invisible in light mode
//! and under `prefers-reduced-motion` via CSS.
//!
//! The animation loop is demand-driven, not always-on: the rAF only spins
//! while the glow is in dark mode *and* still catching up to the pointer.
//! When it settles (or the theme leaves dark) the loop stops itself, so an
//! idle page — and every light-mode page — costs nothing beyond a cheap
//! `mousemove` listener that stashes the target coordinates. Mount once
//! near the app root.

use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{HtmlElement, MouseEvent};
use yew::{prelude::*, use_effect_with, use_mut_ref};

/// Eased follow factor per frame (0..1). Lower = a longer, smoother trail.
const FOLLOW_EASING: f64 = 0.15;
/// Sub-pixel threshold at which the glow is "caught up" and the loop parks.
const SETTLE_EPSILON: f64 = 0.5;

type TickClosure = Closure<dyn FnMut()>;
type SharedTick = Rc<RefCell<Option<TickClosure>>>;

/// True when the app theme switch has put the document into dark mode.
fn is_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.document_element())
        .and_then(|el| el.get_attribute("data-theme"))
        .is_some_and(|theme| theme == "dark")
}

/// Renders the cursor-following spotlight overlay (dark mode only).
#[function_component(Spotlight)]
pub fn spotlight() -> Html {
    let spotlight_ref = use_node_ref();
    let target = use_mut_ref(|| (0.0_f64, 0.0_f64));
    let current = use_mut_ref(|| (0.0_f64, 0.0_f64));
    let running = use_mut_ref(|| false);
    let raf = use_mut_ref(|| Option::<i32>::None);

    {
        let spotlight_ref = spotlight_ref.clone();
        let target = target.clone();
        let current = current.clone();
        let running = running.clone();
        let raf = raf.clone();

        use_effect_with((), move |_| {
            let Some(window) = web_sys::window() else {
                return Box::new(|| {}) as Box<dyn FnOnce()>;
            };

            // Seed both target and current at the viewport centre so the glow
            // starts settled instead of sliding in from (0,0).
            let start_x = window
                .inner_width()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
            let start_y = window
                .inner_height()
                .ok()
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0)
                / 2.0;
            *target.borrow_mut() = (start_x, start_y);
            *current.borrow_mut() = (start_x, start_y);
            if let Some(el) = spotlight_ref.cast::<HtmlElement>() {
                let _ = el.style().set_property("left", &format!("{start_x:.2}px"));
                let _ = el.style().set_property("top", &format!("{start_y:.2}px"));
            }

            // Self-referential rAF tick: lerp toward the target, then either
            // reschedule (still moving, still dark) or park the loop.
            let tick: SharedTick = Rc::new(RefCell::new(None));
            {
                let tick_inner = tick.clone();
                let spotlight_ref = spotlight_ref.clone();
                let target = target.clone();
                let current = current.clone();
                let running = running.clone();
                let raf = raf.clone();
                let window = window.clone();
                *tick.borrow_mut() = Some(Closure::wrap(Box::new(move || {
                    let (tx, ty) = *target.borrow();
                    let (cx, cy) = {
                        let mut cur = current.borrow_mut();
                        cur.0 += (tx - cur.0) * FOLLOW_EASING;
                        cur.1 += (ty - cur.1) * FOLLOW_EASING;
                        *cur
                    };
                    if let Some(el) = spotlight_ref.cast::<HtmlElement>() {
                        let _ = el.style().set_property("left", &format!("{cx:.2}px"));
                        let _ = el.style().set_property("top", &format!("{cy:.2}px"));
                    }

                    let settled =
                        (tx - cx).abs() < SETTLE_EPSILON && (ty - cy).abs() < SETTLE_EPSILON;
                    if settled || !is_dark() {
                        *running.borrow_mut() = false;
                        *raf.borrow_mut() = None;
                        return;
                    }
                    if let Some(cb) = tick_inner.borrow().as_ref() {
                        *raf.borrow_mut() = window
                            .request_animation_frame(cb.as_ref().unchecked_ref())
                            .ok();
                    }
                }) as Box<dyn FnMut()>));
            }

            // mousemove only stashes the target; it kicks the rAF awake on
            // demand, and only when the glow would actually be visible.
            let mouse = {
                let target = target.clone();
                let running = running.clone();
                let raf = raf.clone();
                let tick = tick.clone();
                let window = window.clone();
                Closure::wrap(Box::new(move |event: MouseEvent| {
                    *target.borrow_mut() = (event.client_x() as f64, event.client_y() as f64);
                    if !*running.borrow() && is_dark() {
                        if let Some(cb) = tick.borrow().as_ref() {
                            *running.borrow_mut() = true;
                            *raf.borrow_mut() = window
                                .request_animation_frame(cb.as_ref().unchecked_ref())
                                .ok();
                        }
                    }
                }) as Box<dyn FnMut(MouseEvent)>)
            };
            let _ = window
                .add_event_listener_with_callback("mousemove", mouse.as_ref().unchecked_ref());

            Box::new(move || {
                if let Some(id) = raf.borrow_mut().take() {
                    let _ = window.cancel_animation_frame(id);
                }
                let _ = window.remove_event_listener_with_callback(
                    "mousemove",
                    mouse.as_ref().unchecked_ref(),
                );
                drop(mouse);
                drop(tick);
            }) as Box<dyn FnOnce()>
        });
    }

    html! {
        <div ref={spotlight_ref} class="spotlight" aria-hidden="true"></div>
    }
}
