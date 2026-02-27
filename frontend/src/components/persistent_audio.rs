use std::collections::HashSet;

use wasm_bindgen::{prelude::*, JsCast};
use web_sys::HtmlAudioElement;
use yew::prelude::*;

use crate::{
    api, media_session,
    music_context::{MusicAction, MusicPlayerContext, NextSongMode},
};

/// Call `audio.play()` and silently swallow the rejected Promise.
/// If the browser blocks playback (autoplay policy), dispatch `Pause`
/// so the UI stays in sync.
fn try_play(audio: &HtmlAudioElement, ctx: Option<&MusicPlayerContext>) {
    if let Ok(promise) = audio.play() {
        let ctx = ctx.cloned();
        let cb = Closure::once(move |_: JsValue| {
            // play() was rejected — sync UI back to paused
            if let Some(c) = ctx {
                c.dispatch(MusicAction::Pause);
            }
        });
        let _ = promise.catch(&cb);
        cb.forget();
    }
}

/// Pick next song from playlist order, semantic candidates, or random.
pub(crate) async fn resolve_next_song(
    ctx: &MusicPlayerContext,
) -> Option<(api::SongDetail, String)> {
    // If there's forward history, reducer handles it
    if let Some(idx) = ctx.history_index {
        if idx + 1 < ctx.history.len() {
            return None;
        }
    }

    match ctx.next_mode {
        NextSongMode::PlaylistSequential => pick_playlist_next(ctx).await,
        NextSongMode::Semantic => pick_backend_next(ctx, api::NextSongResolveMode::Semantic).await,
        NextSongMode::Random => pick_backend_next(ctx, api::NextSongResolveMode::Random).await,
    }
}

/// Resolve what the next song preview card should show.
/// Different from `resolve_next_song`, this includes forward history.
pub(crate) async fn preview_next_song(
    ctx: &MusicPlayerContext,
) -> Option<(api::SongDetail, String)> {
    if let Some(idx) = ctx.history_index {
        if idx + 1 < ctx.history.len() {
            let (id, song) = ctx.history[idx + 1].clone();
            return Some((song, id));
        }
    }

    match ctx.next_mode {
        NextSongMode::PlaylistSequential => pick_playlist_next(ctx).await,
        NextSongMode::Semantic => pick_backend_next(ctx, api::NextSongResolveMode::Semantic).await,
        NextSongMode::Random => pick_backend_next(ctx, api::NextSongResolveMode::Random).await,
    }
}

async fn pick_playlist_next(ctx: &MusicPlayerContext) -> Option<(api::SongDetail, String)> {
    if ctx.playlist_ids.is_empty() {
        return None;
    }

    let next_id = match ctx.song_id.as_deref() {
        Some(current_id) => ctx
            .playlist_ids
            .iter()
            .position(|id| id == current_id)
            .and_then(|idx| ctx.playlist_ids.get(idx + 1).cloned())
            .or_else(|| ctx.playlist_ids.first().cloned()),
        None => ctx.playlist_ids.first().cloned(),
    }?;

    if let Ok(Some(detail)) = api::fetch_song_detail(&next_id).await {
        Some((detail, next_id))
    } else {
        None
    }
}

fn collect_recent_song_ids(ctx: &MusicPlayerContext, max: usize) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut ids = Vec::new();
    for (id, _) in ctx.history.iter().rev() {
        let normalized = id.trim();
        if normalized.is_empty() {
            continue;
        }
        if !seen.insert(normalized.to_string()) {
            continue;
        }
        ids.push(normalized.to_string());
        if ids.len() >= max {
            break;
        }
    }
    ids
}

async fn pick_backend_next(
    ctx: &MusicPlayerContext,
    mode: api::NextSongResolveMode,
) -> Option<(api::SongDetail, String)> {
    let recent_ids = collect_recent_song_ids(ctx, 10);
    let current_song_id = ctx.song_id.as_deref();
    if let Ok(Some(detail)) = api::fetch_next_song(mode, current_song_id, &recent_ids).await {
        let id = detail.id.clone();
        return Some((detail, id));
    }
    None
}

#[function_component(PersistentAudio)]
pub fn persistent_audio() -> Html {
    let ctx = use_context::<MusicPlayerContext>();
    let audio_ref = use_node_ref();
    let prev_song_id = use_state(|| None::<String>);

    let ctx = match ctx {
        Some(c) => c,
        None => return html! {},
    };

    // Sync src when song_id changes
    {
        let audio_ref = audio_ref.clone();
        let ctx = ctx.clone();
        let prev_song_id = prev_song_id.clone();
        use_effect_with(ctx.song_id.clone(), move |song_id| {
            if *song_id != *prev_song_id {
                prev_song_id.set(song_id.clone());
                if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                    if let Some(id) = song_id {
                        let url = api::song_audio_url(id);
                        audio.set_src(&url);
                        try_play(&audio, Some(&ctx));
                    } else {
                        audio.set_src("");
                        let _ = audio.pause();
                    }
                }
                // Sync Media Session metadata
                if let Some(song) = &ctx.current_song {
                    let cover = api::song_cover_url(song.cover_image.as_deref());
                    media_session::set_media_metadata(
                        &song.title,
                        &song.artist,
                        &song.album,
                        &cover,
                    );
                }
            }
            || ()
        });
    }

    // Sync play/pause state
    {
        let audio_ref = audio_ref.clone();
        let ctx_for_sync = ctx.clone();
        let playing = ctx.playing;
        let visible = ctx.visible;
        use_effect_with((playing, visible), move |(playing, visible)| {
            if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                if *playing && *visible {
                    try_play(&audio, Some(&ctx_for_sync));
                } else {
                    let _ = audio.pause();
                }
            }
            || ()
        });
    }

    // Sync volume
    {
        let audio_ref = audio_ref.clone();
        let volume = ctx.volume;
        use_effect_with(volume, move |vol| {
            if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                audio.set_volume(*vol);
            }
            || ()
        });
    }

    // Sync Media Session playback state
    {
        let playing = ctx.playing;
        use_effect_with(playing, move |playing| {
            media_session::set_playback_state(*playing);
            || ()
        });
    }

    // Register event listeners
    {
        let audio_ref = audio_ref.clone();
        let ctx = ctx.clone();
        use_effect_with((), move |_| {
            let audio: Option<HtmlAudioElement> = audio_ref.cast::<HtmlAudioElement>();
            let closures: Vec<Closure<dyn FnMut()>> = Vec::new();
            let closures = std::rc::Rc::new(std::cell::RefCell::new(closures));

            if let Some(audio) = audio {
                let ctx_c = ctx.clone();
                let c1 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || {
                        ctx_c.dispatch(MusicAction::SetTime(audio.current_time()));
                    }
                });
                let _ = audio
                    .add_event_listener_with_callback("timeupdate", c1.as_ref().unchecked_ref());
                closures.borrow_mut().push(c1);

                let ctx_c = ctx.clone();
                let c2 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || {
                        ctx_c.dispatch(MusicAction::SetDuration(audio.duration()));
                    }
                });
                let _ = audio.add_event_listener_with_callback(
                    "loadedmetadata",
                    c2.as_ref().unchecked_ref(),
                );
                closures.borrow_mut().push(c2);

                // ended → auto-next
                let ctx_c = ctx.clone();
                let c3 = Closure::<dyn FnMut()>::new(move || {
                    let ctx_inner = ctx_c.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let fallback = resolve_next_song(&ctx_inner).await;
                        ctx_inner.dispatch(MusicAction::PlayNext {
                            fallback,
                        });
                    });
                });
                let _ =
                    audio.add_event_listener_with_callback("ended", c3.as_ref().unchecked_ref());
                closures.borrow_mut().push(c3);

                // Register Media Session action handlers
                {
                    let ctx_play = ctx.clone();
                    let ctx_pause = ctx.clone();
                    let ctx_prev = ctx.clone();
                    let ctx_next = ctx.clone();
                    let ms_closures = media_session::register_media_session_handlers(
                        audio.clone(),
                        // on_play: audio.play() is called synchronously inside the handler
                        // to preserve user-gesture context. TogglePlay syncs UI state.
                        // Safe because OS only sends "play" when state is paused.
                        move || ctx_play.dispatch(MusicAction::TogglePlay),
                        move || ctx_pause.dispatch(MusicAction::Pause),
                        move || ctx_prev.dispatch(MusicAction::PlayPrev),
                        move || {
                            let ctx_inner = ctx_next.clone();
                            wasm_bindgen_futures::spawn_local(async move {
                                let fallback = resolve_next_song(&ctx_inner).await;
                                ctx_inner.dispatch(MusicAction::PlayNext {
                                    fallback,
                                });
                            });
                        },
                    );
                    closures.borrow_mut().extend(ms_closures);
                }
            }

            move || {
                drop(closures);
            }
        });
    }

    html! {
        <audio ref={audio_ref} preload="metadata" style="display:none;" />
    }
}
