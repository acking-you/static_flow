use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlAudioElement;
use yew::prelude::*;

use crate::api;
use crate::music_context::{MusicAction, MusicPlayerContext, NextSongMode};

/// Pick next song from pre-fetched candidates (semantic) or random.
pub(crate) async fn resolve_next_song(ctx: &MusicPlayerContext) -> Option<(api::SongDetail, String)> {
    // If there's forward history, reducer handles it
    if let Some(idx) = ctx.history_index {
        if idx + 1 < ctx.history.len() {
            return None;
        }
    }

    // Collect recent 3 song IDs to avoid repeats
    let recent_ids: Vec<&str> = ctx.history.iter()
        .rev()
        .take(3)
        .map(|(id, _)| id.as_str())
        .collect();

    match ctx.next_mode {
        NextSongMode::Semantic => {
            // Pick from pre-fetched candidates, excluding recent 3
            let filtered: Vec<_> = ctx.candidates.iter()
                .filter(|c| !recent_ids.contains(&c.id.as_str()))
                .collect();
            if !filtered.is_empty() {
                let idx = (js_sys::Math::random() * filtered.len() as f64) as usize;
                let pick = &filtered[idx.min(filtered.len() - 1)];
                if let Ok(Some(detail)) = api::fetch_song_detail(&pick.id).await {
                    return Some((detail, pick.id.clone()));
                }
            }
            // Fallback to random if no candidates
            pick_random_song(&recent_ids).await
        }
        NextSongMode::Random => pick_random_song(&recent_ids).await,
    }
}

async fn pick_random_song(recent_ids: &[&str]) -> Option<(api::SongDetail, String)> {
    if let Ok(resp) = api::fetch_songs(Some(20), None, None, None, Some("random")).await {
        let candidates: Vec<_> = resp.songs.into_iter()
            .filter(|s| !recent_ids.contains(&s.id.as_str()))
            .collect();
        if !candidates.is_empty() {
            let idx = (js_sys::Math::random() * candidates.len() as f64) as usize;
            let pick = &candidates[idx.min(candidates.len() - 1)];
            if let Ok(Some(detail)) = api::fetch_song_detail(&pick.id).await {
                return Some((detail, pick.id.clone()));
            }
        }
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
                        let _ = audio.play();
                    } else {
                        audio.set_src("");
                        let _ = audio.pause();
                    }
                }
            }
            || ()
        });
    }

    // Fetch semantic candidates when song or mode changes
    {
        let ctx = ctx.clone();
        let song_id = ctx.song_id.clone();
        let next_mode = ctx.next_mode.clone();
        use_effect_with((song_id, next_mode), move |(song_id, next_mode)| {
            if *next_mode == NextSongMode::Semantic {
                if let Some(id) = song_id.clone() {
                    let ctx = ctx.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match api::fetch_related_songs(&id).await {
                            Ok(related) => {
                                let filtered: Vec<_> = related
                                    .into_iter()
                                    .filter(|r| r.id != id)
                                    .take(4)
                                    .collect();
                                ctx.dispatch(MusicAction::SetCandidates(filtered));
                            }
                            Err(_) => {
                                ctx.dispatch(MusicAction::SetCandidates(vec![]));
                            }
                        }
                    });
                }
            } else {
                ctx.dispatch(MusicAction::SetCandidates(vec![]));
            }
            || ()
        });
    }

    // Sync play/pause state
    {
        let audio_ref = audio_ref.clone();
        let playing = ctx.playing;
        let visible = ctx.visible;
        use_effect_with((playing, visible), move |(playing, visible)| {
            if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                if *playing && *visible {
                    let _ = audio.play();
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
                let _ = audio.add_event_listener_with_callback(
                    "timeupdate", c1.as_ref().unchecked_ref(),
                );
                closures.borrow_mut().push(c1);

                let ctx_c = ctx.clone();
                let c2 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || {
                        ctx_c.dispatch(MusicAction::SetDuration(audio.duration()));
                    }
                });
                let _ = audio.add_event_listener_with_callback(
                    "loadedmetadata", c2.as_ref().unchecked_ref(),
                );
                closures.borrow_mut().push(c2);

                // ended → auto-next
                let ctx_c = ctx.clone();
                let c3 = Closure::<dyn FnMut()>::new(move || {
                    let ctx_inner = ctx_c.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let fallback = resolve_next_song(&ctx_inner).await;
                        ctx_inner.dispatch(MusicAction::PlayNext { fallback });
                    });
                });
                let _ = audio.add_event_listener_with_callback(
                    "ended", c3.as_ref().unchecked_ref(),
                );
                closures.borrow_mut().push(c3);
            }

            move || { drop(closures); }
        });
    }

    html! {
        <audio ref={audio_ref} preload="metadata" style="display:none;" />
    }
}

