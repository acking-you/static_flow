use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::components::audio_player::AudioPlayer;
use crate::components::icons::{Icon, IconName};
use crate::components::synced_lyrics::SyncedLyrics;
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
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let nickname = use_state(String::new);
    let comment_text = use_state(String::new);
    let submitting = use_state(|| false);
    let submit_error = use_state(|| None::<String>);
    let current_time = use_state(|| 0.0_f64);

    // Fetch song detail + lyrics + comments + track play
    {
        let id = id.clone();
        let song = song.clone();
        let lyrics = lyrics.clone();
        let comments = comments.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with(id.clone(), move |id| {
            let id = id.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match api::fetch_song_detail(&id).await {
                    Ok(Some(d)) => { song.set(Some(d)); }
                    Ok(None) => { error.set(Some("Song not found".to_string())); }
                    Err(e) => { error.set(Some(e)); }
                }
                if let Ok(Some(l)) = api::fetch_song_lyrics(&id).await {
                    lyrics.set(Some(l));
                }
                if let Ok(c) = api::fetch_music_comments(&id, Some(50), None).await {
                    comments.set(c);
                }
                let _ = api::track_song_play(&id).await;
                loading.set(false);
            });
            || ()
        });
    }

    // Comment submit handler
    let on_submit_comment = {
        let id = id.clone();
        let nickname = nickname.clone();
        let comment_text = comment_text.clone();
        let comments = comments.clone();
        let submitting = submitting.clone();
        let submit_error = submit_error.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let nick = (*nickname).clone();
            let text = (*comment_text).clone();
            if nick.trim().is_empty() || text.trim().is_empty() { return; }
            let id = id.clone();
            let nickname = nickname.clone();
            let comment_text = comment_text.clone();
            let comments = comments.clone();
            let submitting = submitting.clone();
            let submit_error = submit_error.clone();
            submitting.set(true);
            submit_error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match api::submit_music_comment(&id, &nick, &text).await {
                    Ok(new_comment) => {
                        let mut list = (*comments).clone();
                        list.insert(0, new_comment);
                        comments.set(list);
                        nickname.set(String::new());
                        comment_text.set(String::new());
                    }
                    Err(e) => { submit_error.set(Some(e)); }
                }
                submitting.set(false);
            });
        })
    };

    let on_nickname_input = {
        let nickname = nickname.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                nickname.set(input.value());
            }
        })
    };

    let on_comment_input = {
        let comment_text = comment_text.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target().and_then(|t| t.dyn_into::<HtmlInputElement>().ok()) {
                comment_text.set(input.value());
            }
        })
    };

    let on_time_update = {
        let current_time = current_time.clone();
        Callback::from(move |t: f64| { current_time.set(t); })
    };

    if *loading {
        return html! {
            <div class="flex justify-center py-20">
                <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--primary)]" />
            </div>
        };
    }

    if let Some(err) = (*error).as_ref() {
        return html! {
            <div class="text-center py-20 text-red-500">
                {format!("Error: {}", err)}
            </div>
        };
    }

    let detail = match (*song).as_ref() {
        Some(d) => d,
        None => {
            return html! {
                <div class="text-center py-20 text-[var(--muted)]">{"Song not found"}</div>
            };
        }
    };

    let cover_url = api::song_cover_url(detail.cover_image.as_deref());
    let audio_url = api::song_audio_url(&id);
    let duration_str = format_duration(detail.duration_ms);

    let lyrics_lrc = (*lyrics).as_ref().and_then(|l| l.lyrics_lrc.clone());
    let lyrics_trans = (*lyrics).as_ref().and_then(|l| l.lyrics_translation.clone());

    html! {
        <div class="max-w-3xl mx-auto px-4 py-8">
            // Cover + Info (immersive layout)
            <div class="flex flex-col items-center mb-8">
                // Large cover
                <div class="w-64 h-64 sm:w-72 sm:h-72 rounded-2xl overflow-hidden liquid-glass \
                            shadow-[var(--shadow-8)] mb-6 bg-[var(--surface-alt)]">
                    if cover_url.is_empty() {
                        <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                            <Icon name={IconName::Music} size={64} class={classes!("opacity-30")} />
                        </div>
                    } else {
                        <img src={cover_url} alt={detail.title.clone()}
                            class="w-full h-full object-cover" />
                    }
                </div>

                // Song info
                <h1 class="text-2xl sm:text-3xl font-bold text-[var(--text)] text-center mb-1"
                    style="font-family: 'Fraunces', serif;">
                    {&detail.title}
                </h1>
                <div class="flex items-center gap-2 text-[var(--muted)] text-sm mb-1">
                    <Link<Route>
                        to={Route::MediaAudio}
                        classes="hover:text-[var(--primary)] transition-colors">
                        {&detail.artist}
                    </Link<Route>>
                    if !detail.album.is_empty() {
                        <span class="text-[var(--border)]">{"·"}</span>
                        <Link<Route>
                            to={Route::MediaAudio}
                            classes="hover:text-[var(--primary)] transition-colors">
                            {&detail.album}
                        </Link<Route>>
                    }
                </div>
                <p class="text-xs text-[var(--muted)]/70">
                    {format!("{} | {} | {}kbps", duration_str, &detail.format, detail.bitrate / 1000)}
                </p>
            </div>

            // Audio Player
            <div class="mb-8">
                <AudioPlayer src={audio_url} on_time_update={on_time_update} />
            </div>

            // Synced Lyrics
            if lyrics_lrc.is_some() {
                <div class="mb-8 bg-[var(--surface)] border border-[var(--border)] rounded-xl overflow-hidden">
                    <h2 class="text-sm font-semibold text-[var(--muted)] px-4 pt-3 pb-1">{"Lyrics"}</h2>
                    <SyncedLyrics
                        lyrics_lrc={lyrics_lrc.map(AttrValue::from)}
                        lyrics_translation={lyrics_trans.map(AttrValue::from)}
                        current_time={*current_time}
                    />
                </div>
            }

            // Comments section
            <div>
                <h2 class="text-lg font-semibold text-[var(--text)] mb-4">
                    {format!("Comments ({})", comments.len())}
                </h2>

                // Submit form
                <form onsubmit={on_submit_comment}
                    class="mb-6 bg-[var(--surface)] border border-[var(--border)] rounded-xl p-4">
                    <div class="flex gap-3 mb-3">
                        <input type="text"
                            placeholder="Nickname"
                            value={(*nickname).clone()}
                            oninput={on_nickname_input}
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm \
                                   text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:border-[var(--primary)]"
                        />
                    </div>
                    <div class="flex gap-3">
                        <input type="text"
                            placeholder="Write a comment..."
                            value={(*comment_text).clone()}
                            oninput={on_comment_input}
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm \
                                   text-[var(--text)] placeholder-[var(--muted)] focus:outline-none focus:border-[var(--primary)]"
                        />
                        <button type="submit" disabled={*submitting}
                            class="px-4 py-2 rounded-lg bg-[var(--primary)] text-white text-sm font-medium \
                                   hover:opacity-90 disabled:opacity-50 transition-opacity">
                            if *submitting { {"..."} } else { {"Send"} }
                        </button>
                    </div>
                    if let Some(err) = (*submit_error).as_ref() {
                        <p class="text-xs text-red-500 mt-2">{err}</p>
                    }
                </form>

                // Comment list
                if comments.is_empty() {
                    <p class="text-sm text-[var(--muted)] text-center py-8">
                        {"No comments yet. Be the first!"}
                    </p>
                } else {
                    <div class="space-y-3">
                        { for comments.iter().map(|c| {
                            let time_str = format_timestamp(c.created_at);
                            html! {
                                <div class="bg-[var(--surface)] border border-[var(--border)] rounded-lg p-3">
                                    <div class="flex items-center gap-2 mb-1">
                                        <span class="text-sm font-medium text-[var(--text)]">
                                            {&c.nickname}
                                        </span>
                                        if let Some(region) = c.ip_region.as_ref() {
                                            <span class="text-xs text-[var(--muted)]">{region}</span>
                                        }
                                        <span class="text-xs text-[var(--muted)] ml-auto">
                                            {time_str}
                                        </span>
                                    </div>
                                    <p class="text-sm text-[var(--muted)]">{&c.comment_text}</p>
                                </div>
                            }
                        })}
                    </div>
                }
            </div>
        </div>
    }
}

fn format_duration(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}

fn format_timestamp(epoch_ms: i64) -> String {
    let secs = epoch_ms / 1000;
    let hours_ago = (js_sys::Date::now() as i64 / 1000 - secs) / 3600;
    if hours_ago < 1 {
        "just now".to_string()
    } else if hours_ago < 24 {
        format!("{}h ago", hours_ago)
    } else {
        let days = hours_ago / 24;
        if days < 30 {
            format!("{}d ago", days)
        } else {
            let days_since_epoch = secs / 86400;
            format!("day {}", days_since_epoch)
        }
    }
}
