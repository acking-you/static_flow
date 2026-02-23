use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::components::icons::{Icon, IconName};
use crate::components::synced_lyrics::SyncedLyrics;
use crate::components::persistent_audio::resolve_next_song;
use crate::music_context::{MusicAction, MusicPlayerContext, NextSongMode};
use crate::router::Route;

#[derive(Properties, Clone, PartialEq)]
pub struct Props {
    pub id: String,
}

#[function_component(MusicPlayerPage)]
pub fn music_player_page(props: &Props) -> Html {
    let id = props.id.clone();
    let song = use_state(|| None::<api::SongDetail>);
    let lyrics = use_state(|| None::<api::SongLyrics>);
    let comments = use_state(|| Vec::<api::MusicCommentItem>::new());
    let song_loading = use_state(|| true);
    let lyrics_loading = use_state(|| true);
    let comments_loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let nickname = use_state(String::new);
    let comment_text = use_state(String::new);
    let submitting = use_state(|| false);
    let submit_error = use_state(|| None::<String>);
    let player_ctx = use_context::<MusicPlayerContext>();
    let navigator = use_navigator();
    let current_time = player_ctx.as_ref().map(|c| c.current_time).unwrap_or(0.0);

    // Navigate when global song_id changes (prev/next/auto-next)
    {
        let navigator = navigator.clone();
        let id = id.clone();
        let ctx_song_id = player_ctx.as_ref().and_then(|c| c.song_id.clone());
        use_effect_with(ctx_song_id, move |ctx_song_id| {
            if let Some(ref new_id) = ctx_song_id {
                if *new_id != id {
                    if let Some(ref nav) = navigator {
                        nav.replace(&Route::MusicPlayer { id: new_id.clone() });
                    }
                }
            }
            || ()
        });
    }

    // Fetch song detail (highest priority)
    // Fix: reuse global context data on remount; fire-and-forget track_song_play
    {
        let id = id.clone();
        let song = song.clone();
        let song_loading = song_loading.clone();
        let error = error.clone();
        let player_ctx = player_ctx.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            // If global context already has this song, skip fetch
            let ctx_hit = player_ctx.as_ref().and_then(|c| {
                if c.song_id.as_deref() == Some(id.as_str()) {
                    c.current_song.clone()
                } else {
                    None
                }
            });
            if let Some(cached) = ctx_hit {
                song.set(Some(cached));
                song_loading.set(false);
                // fire-and-forget play tracking
                let id2 = id.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    let _ = api::track_song_play(&id2).await;
                });
            } else {
                song_loading.set(true);
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_song_detail(&id).await {
                        Ok(Some(d)) => {
                            if let Some(ref ctx) = player_ctx {
                                ctx.dispatch(MusicAction::PlaySong { song: d.clone(), id: id.clone() });
                            }
                            song.set(Some(d));
                        }
                        Ok(None) => { error.set(Some("Song not found".to_string())); }
                        Err(e) => { error.set(Some(e)); }
                    }
                    song_loading.set(false);
                    // fire-and-forget play tracking
                    let id2 = id.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        let _ = api::track_song_play(&id2).await;
                    });
                });
            }
            || ()
        });
    }

    // Fetch lyrics (independent)
    {
        let id = id.clone();
        let lyrics = lyrics.clone();
        let lyrics_loading = lyrics_loading.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            lyrics_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(Some(l)) = api::fetch_song_lyrics(&id).await {
                    lyrics.set(Some(l));
                } else {
                    lyrics.set(None);
                }
                lyrics_loading.set(false);
            });
            || ()
        });
    }

    // Fetch comments (independent)
    {
        let id = id.clone();
        let comments = comments.clone();
        let comments_loading = comments_loading.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            comments_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                if let Ok(c) = api::fetch_music_comments(&id, Some(50), None).await {
                    comments.set(c);
                }
                comments_loading.set(false);
            });
            || ()
        });
    }

    let on_submit_comment = {
        let id = id.clone(); let nickname = nickname.clone(); let comment_text = comment_text.clone();
        let comments = comments.clone(); let submitting = submitting.clone(); let submit_error = submit_error.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let nick = (*nickname).clone(); let text = (*comment_text).clone();
            if nick.trim().is_empty() || text.trim().is_empty() { return; }
            let id = id.clone(); let nickname = nickname.clone(); let comment_text = comment_text.clone();
            let comments = comments.clone(); let submitting = submitting.clone(); let submit_error = submit_error.clone();
            submitting.set(true); submit_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match api::submit_music_comment(&id, &nick, &text).await {
                    Ok(nc) => { let mut l = (*comments).clone(); l.insert(0, nc); comments.set(l); nickname.set(String::new()); comment_text.set(String::new()); }
                    Err(e) => { submit_error.set(Some(e)); }
                }
                submitting.set(false);
            });
        })
    };
    let on_nickname_input = { let nickname = nickname.clone(); Callback::from(move |e: InputEvent| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) { nickname.set(input.value()); }
    })};
    let on_comment_input = { let comment_text = comment_text.clone(); Callback::from(move |e: InputEvent| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) { comment_text.set(input.value()); }
    })};

    let on_toggle_play = { let ctx = player_ctx.clone(); Callback::from(move |_: MouseEvent| {
        if let Some(ref c) = ctx { c.dispatch(MusicAction::TogglePlay); }
    })};
    let can_prev = player_ctx.as_ref().map(|c| c.history_index.map(|i| i > 0).unwrap_or(false)).unwrap_or(false);
    let on_prev = { let ctx = player_ctx.clone(); Callback::from(move |_: MouseEvent| {
        if let Some(ref c) = ctx { c.dispatch(MusicAction::PlayPrev); }
    })};
    let on_next = { let ctx = player_ctx.clone(); Callback::from(move |_: MouseEvent| {
        if let Some(ref c) = ctx { let c2 = c.clone(); wasm_bindgen_futures::spawn_local(async move {
            let fallback = resolve_next_song(&c2).await; c2.dispatch(MusicAction::PlayNext { fallback });
        }); }
    })};
    let is_semantic = player_ctx.as_ref().map(|c| c.next_mode == NextSongMode::Semantic).unwrap_or(false);
    let on_toggle_mode = { let ctx = player_ctx.clone(); Callback::from(move |_: MouseEvent| {
        if let Some(ref c) = ctx {
            let m = if c.next_mode == NextSongMode::Semantic { NextSongMode::Random } else { NextSongMode::Semantic };
            c.dispatch(MusicAction::SetNextMode(m));
        }
    })};
    let candidates = player_ctx.as_ref().map(|c| c.candidates.clone()).unwrap_or_default();
    let on_seek = { let ctx = player_ctx.clone(); Callback::from(move |e: InputEvent| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            if let Ok(v) = input.value().parse::<f64>() {
                if let Some(ref c) = ctx { c.dispatch(MusicAction::SetTime(v)); }
                if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                    if let Ok(Some(el)) = doc.query_selector("audio") {
                        if let Some(audio) = el.dyn_ref::<web_sys::HtmlAudioElement>() { audio.set_current_time(v); }
                    }
                }
            }
        }
    })};
    let on_volume = { let ctx = player_ctx.clone(); Callback::from(move |e: InputEvent| {
        if let Some(input) = e.target().and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok()) {
            if let Ok(v) = input.value().parse::<f64>() { if let Some(ref c) = ctx { c.dispatch(MusicAction::SetVolume(v)); } }
        }
    })};
    let on_toggle_mute = { let ctx = player_ctx.clone(); let cv = player_ctx.as_ref().map(|c| c.volume).unwrap_or(1.0);
        Callback::from(move |_: MouseEvent| { if let Some(ref c) = ctx { c.dispatch(MusicAction::SetVolume(if cv > 0.0 { 0.0 } else { 1.0 })); } })
    };
    let on_minimize = { let ctx = player_ctx.clone(); let navigator = navigator.clone();
        Callback::from(move |_: MouseEvent| {
            if let Some(ref c) = ctx { c.dispatch(MusicAction::Minimize); }
            if let Some(ref nav) = navigator { nav.push(&Route::MediaAudio); }
        })
    };

    let on_back = Callback::from(|_: MouseEvent| {
        if let Some(w) = web_sys::window() {
            let _ = w.history().map(|h| h.back());
        }
    });

    if *song_loading { return html! { <div class="flex justify-center py-20"><div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--primary)]" /></div> }; }
    if let Some(err) = (*error).as_ref() { return html! { <div class="text-center py-20 text-red-500">{format!("Error: {}", err)}</div> }; }
    let detail = match (*song).as_ref() { Some(d) => d, None => { return html! { <div class="text-center py-20 text-[var(--muted)]">{"Song not found"}</div> }; } };

    let cover_url = api::song_cover_url(detail.cover_image.as_deref());
    let audio_url = api::song_audio_url(&id);
    let duration_str = format_duration(detail.duration_ms);
    let lyrics_lrc = (*lyrics).as_ref().and_then(|l| l.lyrics_lrc.clone());
    let lyrics_trans = (*lyrics).as_ref().and_then(|l| l.lyrics_translation.clone());
    let playing = player_ctx.as_ref().map(|c| c.playing).unwrap_or(false);
    let duration = player_ctx.as_ref().map(|c| c.duration).unwrap_or(0.0);
    let volume = player_ctx.as_ref().map(|c| c.volume).unwrap_or(1.0);
    let progress_pct = if duration > 0.0 { (current_time / duration) * 100.0 } else { 0.0 };

    let artist_name = detail.artist.clone();
    let album_name = detail.album.clone();
    let on_artist_nav = {
        let navigator = navigator.clone();
        let artist = artist_name.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let Some(ref nav) = navigator {
                let q = std::collections::HashMap::from([("artist", artist.as_str())]);
                let _ = nav.push_with_query(&Route::MediaAudio, &q);
            }
        })
    };
    let on_album_nav = {
        let navigator = navigator.clone();
        let album = album_name.clone();
        Callback::from(move |e: MouseEvent| {
            e.prevent_default();
            if let Some(ref nav) = navigator {
                let q = std::collections::HashMap::from([("album", album.as_str())]);
                let _ = nav.push_with_query(&Route::MediaAudio, &q);
            }
        })
    };

    html! {
        <div class="max-w-3xl mx-auto px-4 py-8">
            // Back button
            <button onclick={on_back} type="button"
                class="flex items-center gap-1.5 text-sm text-[var(--muted)] hover:text-[var(--text)] transition-colors mb-4">
                <Icon name={IconName::ArrowLeft} size={16} />
                {"Back"}
            </button>

            <div class="flex flex-col items-center mb-8">
                <div class="w-64 h-64 sm:w-72 sm:h-72 rounded-2xl overflow-hidden liquid-glass shadow-[var(--shadow-8)] mb-6 bg-[var(--surface-alt)]">
                    if cover_url.is_empty() {
                        <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                            <Icon name={IconName::Music} size={64} class={classes!("opacity-30")} />
                        </div>
                    } else {
                        <img src={cover_url} alt={detail.title.clone()} class="w-full h-full object-cover" />
                    }
                </div>
                <h1 class="text-2xl sm:text-3xl font-bold text-[var(--text)] text-center mb-1" style="font-family: 'Fraunces', serif;">{&detail.title}</h1>
                <div class="flex items-center gap-2 text-[var(--muted)] text-sm mb-1">
                    <a href="#" onclick={on_artist_nav}
                        class="hover:text-[var(--primary)] transition-colors cursor-pointer">{&detail.artist}</a>
                    if !detail.album.is_empty() {
                        <span class="text-[var(--border)]">{"·"}</span>
                        <a href="#" onclick={on_album_nav}
                            class="hover:text-[var(--primary)] transition-colors cursor-pointer">{&detail.album}</a>
                    }
                </div>
                <p class="text-xs text-[var(--muted)]/70">{format!("{} | {} | {}kbps", duration_str, &detail.format, detail.bitrate / 1000)}</p>
            </div>

            // Player controls
            <div class="mb-8 w-full">
                <div class="relative w-full h-2 group mb-3">
                    <div class="absolute inset-0 rounded-full bg-[var(--border)] overflow-hidden">
                        <div class="h-full bg-[var(--primary)] transition-all" style={format!("width: {}%", progress_pct)} />
                    </div>
                    <input type="range" min="0" max={duration.to_string()} step="0.1" value={current_time.to_string()} oninput={on_seek}
                        class="absolute inset-0 w-full h-full opacity-0 cursor-pointer" aria-label="Seek" />
                </div>
                <div class="flex items-center gap-3">
                    <button onclick={on_toggle_mode} type="button"
                        class={classes!("transition-colors", if is_semantic { "text-[var(--primary)]" } else { "text-[var(--muted)] hover:text-[var(--text)]" })}
                        aria-label="Toggle next-song mode" title={if is_semantic { "Mode: Semantic" } else { "Mode: Random" }}>
                        <Icon name={IconName::Shuffle} size={18} />
                    </button>
                    <button onclick={on_prev} type="button" disabled={!can_prev}
                        class="text-[var(--muted)] hover:text-[var(--text)] transition-colors disabled:opacity-30 disabled:cursor-not-allowed" aria-label="Previous song">
                        <Icon name={IconName::SkipBack} size={18} />
                    </button>
                    <button onclick={on_toggle_play} type="button"
                        class="w-10 h-10 rounded-full bg-[var(--primary)] text-white flex items-center justify-center hover:opacity-90 transition-opacity shrink-0"
                        aria-label={if playing { "Pause" } else { "Play" }}>
                        <Icon name={if playing { IconName::Pause } else { IconName::Play }} size={18} color="white" />
                    </button>
                    <button onclick={on_next} type="button" class="text-[var(--muted)] hover:text-[var(--text)] transition-colors" aria-label="Next song">
                        <Icon name={IconName::SkipForward} size={18} />
                    </button>
                    <span class="text-xs text-[var(--muted)] tabular-nums whitespace-nowrap min-w-[80px]">
                        {format!("{} / {}", format_time(current_time), format_time(duration))}
                    </span>
                    <div class="flex-1" />
                    <a href={audio_url} download={format!("{}.{}", detail.title, detail.format)}
                        class="text-[var(--muted)] hover:text-[var(--text)] transition-colors" aria-label="Download"
                        onclick={Callback::from(|e: MouseEvent| e.stop_propagation())}>
                        <Icon name={IconName::Download} size={18} />
                    </a>
                    <button onclick={on_minimize} type="button" class="text-[var(--muted)] hover:text-[var(--text)] transition-colors" aria-label="Minimize player">
                        <Icon name={IconName::Minimize2} size={18} />
                    </button>
                    <button onclick={on_toggle_mute.clone()} type="button" class="text-[var(--muted)] hover:text-[var(--text)] transition-colors" aria-label="Toggle mute">
                        <Icon name={if volume > 0.0 { IconName::Volume2 } else { IconName::VolumeX }} size={18} />
                    </button>
                    <div class="w-20 max-sm:hidden">
                        <input type="range" min="0" max="1" step="0.01" value={volume.to_string()} oninput={on_volume.clone()}
                            class="w-full h-1 rounded-full appearance-none bg-[var(--border)] accent-[var(--primary)] cursor-pointer" aria-label="Volume" />
                    </div>
                </div>
                // Mobile-only volume row
                <div class="flex items-center gap-2 mt-3 sm:hidden">
                    <button onclick={on_toggle_mute.clone()} type="button"
                        class="text-[var(--muted)] hover:text-[var(--text)] transition-colors shrink-0" aria-label="Toggle mute">
                        <Icon name={if volume > 0.0 { IconName::Volume2 } else { IconName::VolumeX }} size={16} />
                    </button>
                    <input type="range" min="0" max="1" step="0.01" value={volume.to_string()} oninput={on_volume}
                        class="flex-1 h-1.5 rounded-full appearance-none bg-[var(--border)] accent-[var(--primary)] cursor-pointer" aria-label="Volume" />
                    <span class="text-xs text-[var(--muted)] w-8 text-right tabular-nums">
                        {format!("{:.0}%", volume * 100.0)}
                    </span>
                </div>
            </div>

            // Semantic candidates (Up Next)
            if is_semantic && !candidates.is_empty() {
                <div class="mb-8">
                    <h2 class="text-sm font-semibold text-[var(--muted)] mb-3">{"Up Next (Semantic)"}</h2>
                    <div class="grid grid-cols-2 sm:grid-cols-4 gap-3">
                        { for candidates.iter().map(|c| render_candidate_card(c)) }
                    </div>
                </div>
            }

            if *lyrics_loading {
                <div class="mb-8 bg-[var(--surface)] border border-[var(--border)] rounded-xl p-4 space-y-2">
                    <div class="h-4 w-20 bg-[var(--border)] rounded animate-pulse" />
                    <div class="h-3 w-3/4 bg-[var(--border)] rounded animate-pulse" />
                    <div class="h-3 w-2/3 bg-[var(--border)] rounded animate-pulse" />
                    <div class="h-3 w-4/5 bg-[var(--border)] rounded animate-pulse" />
                </div>
            } else if lyrics_lrc.is_some() {
                <div class="mb-8 bg-[var(--surface)] border border-[var(--border)] rounded-xl overflow-hidden">
                    <h2 class="text-sm font-semibold text-[var(--muted)] px-4 pt-3 pb-1">{"Lyrics"}</h2>
                    <SyncedLyrics lyrics_lrc={lyrics_lrc.map(AttrValue::from)} lyrics_translation={lyrics_trans.map(AttrValue::from)} current_time={current_time} />
                </div>
            }

            // Comments
            <div>
                if *comments_loading {
                    <div class="space-y-3">
                        <div class="h-5 w-32 bg-[var(--border)] rounded animate-pulse mb-4" />
                        <div class="bg-[var(--surface)] border border-[var(--border)] rounded-lg p-3 space-y-2">
                            <div class="h-3 w-24 bg-[var(--border)] rounded animate-pulse" />
                            <div class="h-3 w-3/4 bg-[var(--border)] rounded animate-pulse" />
                        </div>
                        <div class="bg-[var(--surface)] border border-[var(--border)] rounded-lg p-3 space-y-2">
                            <div class="h-3 w-20 bg-[var(--border)] rounded animate-pulse" />
                            <div class="h-3 w-2/3 bg-[var(--border)] rounded animate-pulse" />
                        </div>
                    </div>
                } else {
                <h2 class="text-lg font-semibold text-[var(--text)] mb-4">{format!("Comments ({})", comments.len())}</h2>
                <form onsubmit={on_submit_comment} class="mb-6 bg-[var(--surface)] border border-[var(--border)] rounded-xl p-4">
                    <div class="flex gap-3 mb-3">
                        <input type="text" placeholder="Nickname" value={(*nickname).clone()} oninput={on_nickname_input}
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:border-[var(--primary)]" />
                    </div>
                    <div class="flex gap-3">
                        <input type="text" placeholder="Write a comment..." value={(*comment_text).clone()} oninput={on_comment_input}
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:border-[var(--primary)]" />
                        <button type="submit" disabled={*submitting}
                            class="px-4 py-2 rounded-lg bg-[var(--primary)] text-white text-sm font-medium hover:opacity-90 disabled:opacity-50 transition-opacity">
                            if *submitting { {"..."} } else { {"Send"} }
                        </button>
                    </div>
                    if let Some(err) = (*submit_error).as_ref() { <p class="text-xs text-red-500 mt-2">{err}</p> }
                </form>
                if comments.is_empty() {
                    <p class="text-sm text-[var(--muted)] text-center py-8">{"No comments yet. Be the first!"}</p>
                } else {
                    <div class="space-y-3">
                        { for comments.iter().map(|c| { let ts = format_timestamp(c.created_at); html! {
                            <div class="bg-[var(--surface)] border border-[var(--border)] rounded-lg p-3">
                                <div class="flex items-center gap-2 mb-1">
                                    <span class="text-sm font-medium text-[var(--text)]">{&c.nickname}</span>
                                    if let Some(region) = c.ip_region.as_ref() { <span class="text-xs text-[var(--muted)]">{region}</span> }
                                    <span class="text-xs text-[var(--muted)] ml-auto">{ts}</span>
                                </div>
                                <p class="text-sm text-[var(--muted)]">{&c.comment_text}</p>
                            </div>
                        }}) }
                    </div>
                }
                } // comments_loading else
            </div>
        </div>
    }
}

fn render_candidate_card(c: &api::SongSearchResult) -> Html {
    let cover = api::song_cover_url(c.cover_image.as_deref());
    let cid = c.id.clone();
    html! {
        <Link<Route> to={Route::MusicPlayer { id: cid }}
            classes="group bg-[var(--surface)] border border-[var(--border)] rounded-lg overflow-hidden flex flex-col \
                     transition-all duration-200 hover:border-[var(--primary)] hover:shadow-[var(--shadow-4)]">
            <div class="aspect-square bg-[var(--surface-alt)] relative overflow-hidden">
                if cover.is_empty() {
                    <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                        <Icon name={IconName::Music} size={32} class={classes!("opacity-30")} />
                    </div>
                } else {
                    <img src={cover} alt={c.title.clone()} loading="lazy"
                        class="w-full h-full object-cover transition-transform duration-300 group-hover:scale-105" />
                }
                <div class="absolute inset-0 bg-black/0 group-hover:bg-black/20 transition-all duration-200 \
                            flex items-center justify-center opacity-0 group-hover:opacity-100">
                    <div class="w-8 h-8 rounded-full bg-white/90 flex items-center justify-center shadow">
                        <Icon name={IconName::Play} size={14} color="#000" />
                    </div>
                </div>
            </div>
            <div class="p-2">
                <p class="text-xs font-semibold text-[var(--text)] truncate" style="font-family: 'Fraunces', serif;">{&c.title}</p>
                <p class="text-[10px] text-[var(--muted)] truncate">{&c.artist}</p>
            </div>
        </Link<Route>>
    }
}

fn format_duration(ms: u64) -> String { let s = ms / 1000; format!("{:02}:{:02}", s / 60, s % 60) }
fn format_time(secs: f64) -> String { if secs.is_nan() || secs.is_infinite() { return "00:00".to_string(); } let t = secs as u64; format!("{:02}:{:02}", t / 60, t % 60) }
fn format_timestamp(epoch_ms: i64) -> String {
    let secs = epoch_ms / 1000;
    let hours_ago = (js_sys::Date::now() as i64 / 1000 - secs) / 3600;
    if hours_ago < 1 { "just now".to_string() }
    else if hours_ago < 24 { format!("{}h ago", hours_ago) }
    else { let d = hours_ago / 24; if d < 30 { format!("{}d ago", d) } else { format!("day {}", secs / 86400) } }
}
