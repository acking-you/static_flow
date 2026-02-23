use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::HtmlAudioElement;
use yew::prelude::*;

use super::icons::{Icon, IconName};

#[derive(Properties, PartialEq)]
pub struct AudioPlayerProps {
    pub src: AttrValue,
    #[prop_or_default]
    pub on_time_update: Option<Callback<f64>>,
}

#[function_component(AudioPlayer)]
pub fn audio_player(props: &AudioPlayerProps) -> Html {
    let audio_ref = use_node_ref();
    let playing = use_state(|| false);
    let current_time = use_state(|| 0.0_f64);
    let duration = use_state(|| 0.0_f64);
    let volume = use_state(|| 1.0_f64);
    let buffered_end = use_state(|| 0.0_f64);
    let seeking = use_state(|| false);

    // Register audio event listeners
    {
        let audio_ref = audio_ref.clone();
        let playing = playing.clone();
        let current_time = current_time.clone();
        let duration = duration.clone();
        let buffered_end = buffered_end.clone();
        let on_time_update = props.on_time_update.clone();
        let seeking = seeking.clone();

        use_effect_with(props.src.clone(), move |_| {
            let audio: Option<HtmlAudioElement> = audio_ref
                .cast::<HtmlAudioElement>();
            let closures: Vec<Closure<dyn FnMut()>> = Vec::new();
            let closures = std::rc::Rc::new(std::cell::RefCell::new(closures));

            if let Some(audio) = audio {
                // timeupdate
                let ct = current_time.clone();
                let cb = on_time_update.clone();
                let seeking_c = seeking.clone();
                let c1 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || {
                        if !*seeking_c {
                            let t = audio.current_time();
                            ct.set(t);
                            if let Some(ref cb) = cb {
                                cb.emit(t);
                            }
                        }
                    }
                });
                let _ = audio.add_event_listener_with_callback(
                    "timeupdate", c1.as_ref().unchecked_ref());
                closures.borrow_mut().push(c1);

                // loadedmetadata
                let dur = duration.clone();
                let c2 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || { dur.set(audio.duration()); }
                });
                let _ = audio.add_event_listener_with_callback(
                    "loadedmetadata", c2.as_ref().unchecked_ref());
                closures.borrow_mut().push(c2);

                // ended
                let pl = playing.clone();
                let c3 = Closure::<dyn FnMut()>::new(move || { pl.set(false); });
                let _ = audio.add_event_listener_with_callback(
                    "ended", c3.as_ref().unchecked_ref());
                closures.borrow_mut().push(c3);

                // progress (buffered)
                let buf = buffered_end.clone();
                let c4 = Closure::<dyn FnMut()>::new({
                    let audio = audio.clone();
                    move || {
                        let b = audio.buffered();
                        if b.length() > 0 {
                            if let Ok(end) = b.end(b.length() - 1) {
                                buf.set(end);
                            }
                        }
                    }
                });
                let _ = audio.add_event_listener_with_callback(
                    "progress", c4.as_ref().unchecked_ref());
                closures.borrow_mut().push(c4);
            }

            // prevent closures from being dropped
            move || { drop(closures); }
        });
    }

    let toggle_play = {
        let audio_ref = audio_ref.clone();
        let playing = playing.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                if *playing {
                    let _ = audio.pause();
                    playing.set(false);
                } else {
                    let _ = audio.play();
                    playing.set(true);
                }
            }
        })
    };

    let on_seek = {
        let audio_ref = audio_ref.clone();
        let current_time = current_time.clone();
        let seeking = seeking.clone();
        let on_time_update = props.on_time_update.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                if let Ok(v) = input.value().parse::<f64>() {
                    seeking.set(false);
                    current_time.set(v);
                    if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                        audio.set_current_time(v);
                    }
                    if let Some(ref cb) = on_time_update {
                        cb.emit(v);
                    }
                }
            }
        })
    };

    let on_seek_start = {
        let seeking = seeking.clone();
        Callback::from(move |_: MouseEvent| { seeking.set(true); })
    };

    let on_volume = {
        let audio_ref = audio_ref.clone();
        let volume = volume.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
                if let Ok(v) = input.value().parse::<f64>() {
                    volume.set(v);
                    if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                        audio.set_volume(v);
                    }
                }
            }
        })
    };

    let toggle_mute = {
        let audio_ref = audio_ref.clone();
        let volume = volume.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(audio) = audio_ref.cast::<HtmlAudioElement>() {
                if *volume > 0.0 {
                    audio.set_volume(0.0);
                    volume.set(0.0);
                } else {
                    audio.set_volume(1.0);
                    volume.set(1.0);
                }
            }
        })
    };

    let dur = *duration;
    let ct = *current_time;
    let buf = *buffered_end;
    let progress_pct = if dur > 0.0 { (ct / dur) * 100.0 } else { 0.0 };
    let buffered_pct = if dur > 0.0 { (buf / dur) * 100.0 } else { 0.0 };

    html! {
        <div class="w-full">
            <audio ref={audio_ref.clone()} preload="metadata" src={props.src.clone()} />

            // Progress bar
            <div class="relative w-full h-2 group mb-3">
                // Buffered track
                <div class="absolute inset-0 rounded-full bg-[var(--border)] overflow-hidden">
                    <div class="h-full bg-[var(--muted)]/30 transition-all duration-300"
                        style={format!("width: {}%", buffered_pct)} />
                </div>
                // Played track
                <div class="absolute inset-0 rounded-full overflow-hidden pointer-events-none">
                    <div class="h-full bg-[var(--primary)] transition-all"
                        style={format!("width: {}%", progress_pct)} />
                </div>
                // Range input
                <input type="range"
                    min="0" max={dur.to_string()} step="0.1"
                    value={ct.to_string()}
                    oninput={on_seek}
                    onmousedown={on_seek_start}
                    class="absolute inset-0 w-full h-full opacity-0 cursor-pointer"
                    aria-label="Seek"
                />
            </div>

            // Controls row
            <div class="flex items-center gap-3">
                // Play/Pause
                <button onclick={toggle_play} type="button"
                    class="w-10 h-10 rounded-full bg-[var(--primary)] text-white flex items-center justify-center hover:opacity-90 transition-opacity shrink-0"
                    aria-label={if *playing { "Pause" } else { "Play" }}>
                    <Icon name={if *playing { IconName::Pause } else { IconName::Play }} size={18} color="white" />
                </button>

                // Time display
                <span class="text-xs text-[var(--muted)] tabular-nums whitespace-nowrap min-w-[80px]">
                    {format!("{} / {}", format_time(ct), format_time(dur))}
                </span>

                <div class="flex-1" />

                // Volume
                <button onclick={toggle_mute} type="button"
                    class="text-[var(--muted)] hover:text-[var(--text)] transition-colors"
                    aria-label="Toggle mute">
                    <Icon name={if *volume > 0.0 { IconName::Volume2 } else { IconName::VolumeX }} size={18} />
                </button>
                <div class="w-20 hidden sm:block">
                    <input type="range" min="0" max="1" step="0.01"
                        value={volume.to_string()}
                        oninput={on_volume}
                        class="w-full h-1 rounded-full appearance-none bg-[var(--border)] accent-[var(--primary)] cursor-pointer"
                        aria-label="Volume"
                    />
                </div>
            </div>
        </div>
    }
}

fn format_time(secs: f64) -> String {
    if secs.is_nan() || secs.is_infinite() {
        return "00:00".to_string();
    }
    let total = secs as u64;
    let m = total / 60;
    let s = total % 60;
    format!("{:02}:{:02}", m, s)
}
