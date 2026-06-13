//! Reading progress bar: a 2px primary line under the sticky header that
//! fills as the reader moves through the article body.
//!
//! The scroll handler writes the bar width straight through a `NodeRef`
//! (no `use_state`), so scrolling never re-renders the component tree.

use gloo_events::EventListener;
use gloo_utils::{document, window};
use wasm_bindgen::JsCast;
use web_sys::HtmlElement;
use yew::prelude::*;

/// Selector for the article body the bar measures against.
const ARTICLE_SELECTOR: &str = ".article-content";

/// Fixed progress line mounted by the article detail page.
#[function_component(ReadingProgress)]
pub fn reading_progress() -> Html {
    let bar_ref = use_node_ref();

    {
        let bar_ref = bar_ref.clone();
        use_effect_with((), move |_| {
            let update = move || {
                let Some(bar) = bar_ref.cast::<HtmlElement>() else {
                    return;
                };
                let Some(article) = document()
                    .query_selector(ARTICLE_SELECTOR)
                    .ok()
                    .flatten()
                    .and_then(|el| el.dyn_into::<HtmlElement>().ok())
                else {
                    let _ = bar.style().set_property("width", "0%");
                    return;
                };
                let rect = article.get_bounding_client_rect();
                let viewport = window()
                    .inner_height()
                    .ok()
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let total = rect.height() - viewport;
                if total <= 0.0 {
                    // Article fits in the viewport: no progress to report.
                    let _ = bar.style().set_property("width", "0%");
                    return;
                }
                let progress = ((-rect.top()) / total).clamp(0.0, 1.0);
                let _ = bar
                    .style()
                    .set_property("width", &format!("{:.2}%", progress * 100.0));
            };
            update();
            let on_scroll = EventListener::new(&window(), "scroll", move |_| update());
            move || drop(on_scroll)
        });
    }

    html! {
        <div class={classes!("reading-progress-track")} aria-hidden="true">
            <div ref={bar_ref} class={classes!("reading-progress-bar")}></div>
        </div>
    }
}
