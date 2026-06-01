//! StaticFlow frontend SPA entrypoint.

mod api;
mod components;
mod config;
/// Shared hooks that are intentionally reused across multiple page modules.
#[allow(
    missing_docs,
    reason = "The hooks module exposes many app-internal hooks; the public module itself needs \
              docs, but item-level docs would be enforced incrementally."
)]
pub mod hooks;
mod i18n;
mod media_session;
mod models;
/// Music playback context and provider state shared across the application.
#[allow(
    missing_docs,
    reason = "The module is public for app wiring, but its detailed API docs are enforced \
              separately from the root module contract."
)]
pub mod music_context;
mod navigation_context;
mod pages;
mod router;
mod seo;
mod utils;

use yew::prelude::*;

use crate::music_context::MusicPlayerProvider;

#[function_component(App)]
fn app() -> Html {
    html! {
        <MusicPlayerProvider>
            <router::AppRouter />
        </MusicPlayerProvider>
    }
}

fn main() {
    yew::Renderer::<App>::new().render();
}
