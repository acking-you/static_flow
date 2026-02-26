mod api;
mod components;
mod config;
pub mod hooks;
mod i18n;
mod media_session;
mod models;
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
