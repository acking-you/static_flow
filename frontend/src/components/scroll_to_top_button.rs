use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::window;
use yew::prelude::*;

use crate::components::{
    icons::IconName,
    tooltip::{TooltipIconButton, TooltipPosition},
};

#[function_component(ScrollToTopButton)]
pub fn scroll_to_top_button() -> Html {
    let show = use_state(|| false);

    // 监听滚动事件
    {
        let show = show.clone();
        use_effect_with((), move |_| {
            let window = window().expect("no global `window` exists");

            let closure = {
                let show = show.clone();
                let window = window.clone();
                Closure::wrap(Box::new(move || {
                    let scroll_y = window.scroll_y().unwrap_or(0.0);
                    // 滚动超过 400px 显示按钮
                    show.set(scroll_y > 400.0);
                }) as Box<dyn Fn()>)
            };

            window
                .add_event_listener_with_callback("scroll", closure.as_ref().unchecked_ref())
                .unwrap();

            let cleanup = move || {
                let _ = window.remove_event_listener_with_callback(
                    "scroll",
                    closure.as_ref().unchecked_ref(),
                );
                drop(closure);
            };

            move || cleanup()
        });
    }

    let onclick = Callback::from(|_: MouseEvent| {
        if let Some(window) = window() {
            let options = web_sys::ScrollToOptions::new();
            options.set_behavior(web_sys::ScrollBehavior::Smooth);
            options.set_top(0.0);
            options.set_left(0.0);

            let _ = window.scroll_with_scroll_to_options(&options);
        }
    });

    if *show {
        html! {
            <div class="scroll-to-top">
                <TooltipIconButton
                    icon={IconName::ArrowUp}
                    tooltip="回到顶部"
                    position={TooltipPosition::Top}
                    onclick={onclick}
                    size={20}
                />
            </div>
        }
    } else {
        html! {}
    }
}
