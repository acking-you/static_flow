use std::{cell::RefCell, rc::Rc};

use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{HtmlElement, MouseEvent};
use yew::{prelude::*, use_effect_with, use_mut_ref};

type AnimationClosure = Closure<dyn FnMut()>;
type SharedAnimationClosure = Rc<RefCell<Option<AnimationClosure>>>;

#[function_component(Spotlight)]
pub fn spotlight() -> Html {
    let spotlight_ref = use_node_ref();
    let target_position = use_mut_ref(|| (0.0f64, 0.0f64));
    let current_position = use_mut_ref(|| (0.0f64, 0.0f64));
    let raf_handle = use_mut_ref(|| Option::<i32>::None);

    {
        let spotlight_ref = spotlight_ref.clone();
        let target_position = target_position.clone();
        let current_position = current_position.clone();
        let raf_handle = raf_handle.clone();

        use_effect_with((), move |_| {
            // Store cleanup closures in Option to allow conditional initialization
            let window_opt = web_sys::window();
            let mouse_closure_opt: Option<Closure<dyn FnMut(MouseEvent)>> =
                window_opt.as_ref().map(|window| {
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

                    {
                        let mut target = target_position.borrow_mut();
                        *target = (start_x, start_y);
                    }
                    {
                        let mut current = current_position.borrow_mut();
                        *current = (start_x, start_y);
                    }

                    if let Some(el) = spotlight_ref.cast::<HtmlElement>() {
                        let _ = el.style().set_property("left", &format!("{start_x:.2}px"));
                        let _ = el.style().set_property("top", &format!("{start_y:.2}px"));
                    }

                    let mouse_move_closure: Closure<dyn FnMut(MouseEvent)> = {
                        let target_position = target_position.clone();
                        Closure::wrap(Box::new(move |event: MouseEvent| {
                            let mut target = target_position.borrow_mut();
                            target.0 = event.client_x() as f64;
                            target.1 = event.client_y() as f64;
                        }) as Box<dyn FnMut(MouseEvent)>)
                    };

                    let _ = window.add_event_listener_with_callback(
                        "mousemove",
                        mouse_move_closure.as_ref().unchecked_ref(),
                    );

                    let animation_cb: SharedAnimationClosure = Rc::new(RefCell::new(None));
                    let animation_cb_clone = animation_cb.clone();
                    let spotlight_for_animation = spotlight_ref.clone();
                    let target_for_animation = target_position.clone();
                    let current_for_animation = current_position.clone();
                    let raf_for_animation = raf_handle.clone();
                    let window_for_animation = window.clone();

                    let animation = Closure::wrap(Box::new(move || {
                        let (target_x, target_y) = *target_for_animation.borrow();
                        let mut current = current_for_animation.borrow_mut();
                        current.0 += (target_x - current.0) * 0.15;
                        current.1 += (target_y - current.1) * 0.15;

                        if let Some(el) = spotlight_for_animation.cast::<HtmlElement>() {
                            let _ = el
                                .style()
                                .set_property("left", &format!("{:.2}px", current.0));
                            let _ = el
                                .style()
                                .set_property("top", &format!("{:.2}px", current.1));
                        }

                        if let Some(cb) = animation_cb_clone.borrow().as_ref() {
                            if let Ok(id) = window_for_animation
                                .request_animation_frame(cb.as_ref().unchecked_ref())
                            {
                                *raf_for_animation.borrow_mut() = Some(id);
                            }
                        }
                    }) as Box<dyn FnMut()>);

                    *animation_cb.borrow_mut() = Some(animation);

                    if let Some(cb) = animation_cb.borrow().as_ref() {
                        if let Ok(id) = window.request_animation_frame(cb.as_ref().unchecked_ref())
                        {
                            *raf_handle.borrow_mut() = Some(id);
                        }
                    }

                    mouse_move_closure
                });

            move || {
                if let (Some(window), Some(mouse_closure)) =
                    (window_opt.as_ref(), mouse_closure_opt.as_ref())
                {
                    if let Some(id) = *raf_handle.borrow() {
                        let _ = window.cancel_animation_frame(id);
                    }
                    let _ = window.remove_event_listener_with_callback(
                        "mousemove",
                        mouse_closure.as_ref().unchecked_ref(),
                    );
                }
            }
        });
    }

    html! {
        <div ref={spotlight_ref} class="spotlight" aria-hidden="true"></div>
    }
}
