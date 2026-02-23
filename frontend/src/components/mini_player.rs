use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::components::icons::{Icon, IconName};
use crate::components::persistent_audio::resolve_next_song;
use crate::music_context::{MusicAction, MusicPlayerContext};
use crate::router::Route;

#[function_component(MiniPlayer)]
pub fn mini_player() -> Html {
    let ctx = use_context::<MusicPlayerContext>();
    let navigator = use_navigator();

    let ctx = match ctx {
        Some(c) => c,
        None => return html! {},
    };

    if !ctx.visible || ctx.current_song.is_none() {
        return html! {};
    }

    let show = ctx.minimized;
    let song = ctx.current_song.as_ref().unwrap();

    let cover_url = api::song_cover_url(song.cover_image.as_deref());
    let title = song.title.clone();
    let artist = song.artist.clone();
    let playing = ctx.playing;

    let on_toggle_play = {
        let ctx = ctx.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            ctx.dispatch(MusicAction::TogglePlay);
        })
    };

    let can_prev = ctx.history_index.map(|i| i > 0).unwrap_or(false);
    let on_prev = {
        let ctx = ctx.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            ctx.dispatch(MusicAction::PlayPrev);
        })
    };

    let on_next = {
        let ctx = ctx.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            let c2 = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let fallback = resolve_next_song(&c2).await;
                c2.dispatch(MusicAction::PlayNext { fallback });
            });
        })
    };

    let on_close = {
        let ctx = ctx.clone();
        Callback::from(move |e: MouseEvent| {
            e.stop_propagation();
            ctx.dispatch(MusicAction::Close);
        })
    };

    let on_expand = {
        let ctx = ctx.clone();
        let navigator = navigator.clone();
        let song_id = ctx.song_id.clone();
        Callback::from(move |_: MouseEvent| {
            ctx.dispatch(MusicAction::Expand);
            if let (Some(nav), Some(id)) = (navigator.as_ref(), song_id.as_ref()) {
                nav.push(&Route::MusicPlayer { id: id.clone() });
            }
        })
    };

    let vis_class = if show {
        "translate-y-0 opacity-100 pointer-events-auto"
    } else {
        "translate-y-4 opacity-0 pointer-events-none"
    };

    let container_class = format!(
        "fixed bottom-4 right-4 z-[70] flex items-center gap-3 \
         bg-[var(--surface)] liquid-glass border border-[var(--border)] \
         rounded-xl shadow-[var(--shadow-8)] px-3 py-2 cursor-pointer \
         hover:border-[var(--primary)] transition-all duration-300 \
         max-w-[280px] select-none {vis_class}"
    );

    html! {
        <div onclick={on_expand}
            class={container_class}
            style="backdrop-filter: blur(12px);">
            // Cover thumbnail
            <div class="w-10 h-10 rounded-lg overflow-hidden shrink-0 bg-[var(--surface-alt)]">
                if cover_url.is_empty() {
                    <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                        <Icon name={IconName::Music} size={16} class={classes!("opacity-40")} />
                    </div>
                } else {
                    <img src={cover_url} alt={title.clone()} referrerpolicy="no-referrer"
                        class="w-full h-full object-cover" />
                }
            </div>

            // Song info
            <div class="flex-1 min-w-0">
                <p class="text-xs font-semibold text-[var(--text)] truncate"
                   style="font-family: 'Fraunces', serif;">
                    {title}
                </p>
                <p class="text-[10px] text-[var(--muted)] truncate">
                    {artist}
                </p>
            </div>

            // Prev button
            <button onclick={on_prev} type="button" disabled={!can_prev}
                class="w-6 h-6 rounded-full flex items-center justify-center \
                       text-[var(--muted)] hover:text-[var(--text)] \
                       transition-all shrink-0 disabled:opacity-30 disabled:cursor-not-allowed"
                aria-label="Previous song">
                <Icon name={IconName::SkipBack} size={12} />
            </button>

            // Play/Pause button
            <button onclick={on_toggle_play} type="button"
                class="w-8 h-8 rounded-full bg-[var(--primary)] text-white \
                       flex items-center justify-center hover:opacity-90 \
                       transition-opacity shrink-0"
                aria-label={if playing { "Pause" } else { "Play" }}>
                <Icon name={if playing { IconName::Pause } else { IconName::Play }} size={14} color="white" />
            </button>

            // Next button
            <button onclick={on_next} type="button"
                class="w-6 h-6 rounded-full flex items-center justify-center \
                       text-[var(--muted)] hover:text-[var(--text)] \
                       transition-all shrink-0"
                aria-label="Next song">
                <Icon name={IconName::SkipForward} size={12} />
            </button>

            // Close button
            <button onclick={on_close} type="button"
                class="w-6 h-6 rounded-full flex items-center justify-center \
                       text-[var(--muted)] hover:text-[var(--text)] \
                       hover:bg-[var(--surface-alt)] transition-all shrink-0"
                aria-label="Close player">
                <Icon name={IconName::X} size={12} />
            </button>
        </div>
    }
}
