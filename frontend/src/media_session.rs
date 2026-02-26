use wasm_bindgen::prelude::*;
use web_sys::HtmlAudioElement;

fn get_media_session() -> Option<JsValue> {
    let nav = js_sys::Reflect::get(&js_sys::global(), &"navigator".into()).ok()?;
    if nav.is_undefined() || nav.is_null() {
        return None;
    }
    let ms = js_sys::Reflect::get(&nav, &"mediaSession".into()).ok()?;
    if ms.is_undefined() || ms.is_null() {
        return None;
    }
    Some(ms)
}

/// Resolve a potentially-relative cover URL to absolute for OS notifications.
fn to_absolute_url(url: &str) -> String {
    if url.is_empty() || url.starts_with("http://") || url.starts_with("https://") {
        return url.to_string();
    }
    // Relative path — prepend origin
    if let Some(win) = web_sys::window() {
        if let Ok(origin) = win.location().origin() {
            return format!("{}{}", origin, url);
        }
    }
    url.to_string()
}

/// Sync song metadata to navigator.mediaSession.metadata
pub fn set_media_metadata(title: &str, artist: &str, album: &str, cover_url: &str) {
    let Some(ms) = get_media_session() else {
        return;
    };

    let artwork = js_sys::Array::new();
    if !cover_url.is_empty() {
        let abs_url = to_absolute_url(cover_url);
        let entry = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&entry, &"src".into(), &abs_url.into());
        let _ = js_sys::Reflect::set(&entry, &"sizes".into(), &"512x512".into());
        let _ = js_sys::Reflect::set(&entry, &"type".into(), &"image/jpeg".into());
        artwork.push(&entry);
    }

    let init = js_sys::Object::new();
    let _ = js_sys::Reflect::set(&init, &"title".into(), &title.into());
    let _ = js_sys::Reflect::set(&init, &"artist".into(), &artist.into());
    let _ = js_sys::Reflect::set(&init, &"album".into(), &album.into());
    let _ = js_sys::Reflect::set(&init, &"artwork".into(), &artwork);

    // new MediaMetadata(init)
    if let Ok(ctor) = js_sys::Reflect::get(&js_sys::global(), &"MediaMetadata".into()) {
        if let Ok(metadata) = js_sys::Reflect::construct(ctor.unchecked_ref(), &{
            let args = js_sys::Array::new();
            args.push(&init);
            args
        }) {
            let _ = js_sys::Reflect::set(&ms, &"metadata".into(), &metadata);
        }
    }
}

/// Register play/pause/previoustrack/nexttrack action handlers.
///
/// `audio` is the real `<audio>` element — handlers call `.play()` / `.pause()`
/// directly so the browser's user-gesture context is preserved (critical on mobile).
pub fn register_media_session_handlers(
    audio: HtmlAudioElement,
    on_play: impl Fn() + 'static,
    on_pause: impl Fn() + 'static,
    on_prev: impl Fn() + 'static,
    on_next: impl Fn() + 'static,
) -> Vec<Closure<dyn FnMut()>> {
    let Some(ms) = get_media_session() else {
        return Vec::new();
    };

    let mut closures = Vec::with_capacity(4);

    // play — must call audio.play() synchronously inside the handler
    {
        let audio = audio.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let _ = audio.play();
            on_play();
        });
        set_action(&ms, "play", &closure);
        closures.push(closure);
    }
    // pause
    {
        let audio = audio.clone();
        let closure = Closure::<dyn FnMut()>::new(move || {
            let _ = audio.pause();
            on_pause();
        });
        set_action(&ms, "pause", &closure);
        closures.push(closure);
    }
    // previoustrack
    {
        let closure = Closure::<dyn FnMut()>::new(move || on_prev());
        set_action(&ms, "previoustrack", &closure);
        closures.push(closure);
    }
    // nexttrack
    {
        let closure = Closure::<dyn FnMut()>::new(move || on_next());
        set_action(&ms, "nexttrack", &closure);
        closures.push(closure);
    }

    closures
}

fn set_action(ms: &JsValue, action: &str, closure: &Closure<dyn FnMut()>) {
    if let Ok(set_handler) = js_sys::Reflect::get(ms, &"setActionHandler".into()) {
        if let Some(func) = set_handler.dyn_ref::<js_sys::Function>() {
            let _ = func.call2(ms, &action.into(), closure.as_ref().unchecked_ref());
        }
    }
}

/// Sync playback state to navigator.mediaSession.playbackState
pub fn set_playback_state(playing: bool) {
    let Some(ms) = get_media_session() else {
        return;
    };
    let state: JsValue = if playing { "playing" } else { "paused" }.into();
    let _ = js_sys::Reflect::set(&ms, &"playbackState".into(), &state);
}
