use wasm_bindgen::JsCast;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api;

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
    let nickname = use_state(|| String::new());
    let comment_text = use_state(|| String::new());
    let submitting = use_state(|| false);
    let submit_error = use_state(|| None::<String>);

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
                    Ok(Some(d)) => {
                        song.set(Some(d));
                    }
                    Ok(None) => {
                        error.set(Some("Song not found".to_string()));
                    }
                    Err(e) => {
                        error.set(Some(e));
                    }
                }
                if let Ok(Some(l)) = api::fetch_song_lyrics(&id).await {
                    lyrics.set(Some(l));
                }
                if let Ok(c) = api::fetch_music_comments(&id, Some(50), None).await {
                    comments.set(c);
                }
                // Track play count (fire-and-forget)
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
            if nick.trim().is_empty() || text.trim().is_empty() {
                return;
            }
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
                    Err(e) => {
                        submit_error.set(Some(e));
                    }
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

    if *loading {
        return html! {
            <div class="flex justify-center py-20">
                <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--accent)]"></div>
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
                <div class="text-center py-20 text-[var(--text-secondary)]">
                    {"Song not found"}
                </div>
            };
        }
    };

    let cover_url = api::song_cover_url(detail.cover_image.as_deref());
    let audio_url = api::song_audio_url(&id);
    let duration_str = format_duration(detail.duration_ms);

    // Parse lyrics lines for display
    let lyrics_lines: Vec<String> = (*lyrics)
        .as_ref()
        .and_then(|l| l.lyrics_lrc.as_ref())
        .map(|lrc| parse_lrc_lines(lrc))
        .unwrap_or_default();

    html! {
        <div class="max-w-4xl mx-auto px-4 py-8">
            // Song info + player
            <div class="flex flex-col md:flex-row gap-6 mb-8">
                // Cover
                <div class="w-full md:w-64 flex-shrink-0">
                    <div class="aspect-square rounded-xl overflow-hidden bg-[var(--surface-alt)]">
                        if cover_url.is_empty() {
                            <div class="w-full h-full flex items-center justify-center text-[var(--text-tertiary)]">
                                <svg xmlns="http://www.w3.org/2000/svg" class="w-20 h-20 opacity-30" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                                    <path d="M9 18V5l12-2v13"/>
                                    <circle cx="6" cy="18" r="3"/>
                                    <circle cx="18" cy="16" r="3"/>
                                </svg>
                            </div>
                        } else {
                            <img src={cover_url} alt={detail.title.clone()}
                                class="w-full h-full object-cover" />
                        }
                    </div>
                </div>

                // Info + audio
                <div class="flex-1 flex flex-col justify-between">
                    <div>
                        <h1 class="text-2xl font-bold text-[var(--text-primary)] mb-1">
                            {&detail.title}
                        </h1>
                        <p class="text-[var(--text-secondary)] mb-1">{&detail.artist}</p>
                        if !detail.album.is_empty() {
                            <p class="text-sm text-[var(--text-tertiary)] mb-1">
                                {format!("Album: {}", &detail.album)}
                            </p>
                        }
                        <p class="text-xs text-[var(--text-tertiary)]">
                            {format!("{} | {} | {}kbps", duration_str, &detail.format, detail.bitrate / 1000)}
                        </p>
                    </div>
                    <div class="mt-4">
                        <audio controls=true class="w-full" preload="metadata" src={audio_url}>
                            {"Your browser does not support the audio element."}
                        </audio>
                    </div>
                </div>
            </div>

            // Lyrics section
            if !lyrics_lines.is_empty() {
                <div class="mb-8">
                    <h2 class="text-lg font-semibold text-[var(--text-primary)] mb-3">{"Lyrics"}</h2>
                    <div class="bg-[var(--surface)] border border-[var(--border)] rounded-xl p-4 max-h-80 overflow-y-auto">
                        { for lyrics_lines.iter().map(|line| {
                            if line.trim().is_empty() {
                                html! { <div class="h-4"></div> }
                            } else {
                                html! {
                                    <p class="text-sm text-[var(--text-secondary)] leading-7">
                                        {line}
                                    </p>
                                }
                            }
                        })}
                    </div>
                </div>
            }

            // Comments section
            <div>
                <h2 class="text-lg font-semibold text-[var(--text-primary)] mb-4">
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
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm text-[var(--text-primary)] placeholder-[var(--text-tertiary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                    </div>
                    <div class="flex gap-3">
                        <input type="text"
                            placeholder="Write a comment..."
                            value={(*comment_text).clone()}
                            oninput={on_comment_input}
                            class="flex-1 px-3 py-2 rounded-lg bg-[var(--bg)] border border-[var(--border)] text-sm text-[var(--text-primary)] placeholder-[var(--text-tertiary)] focus:outline-none focus:border-[var(--accent)]"
                        />
                        <button type="submit"
                            disabled={*submitting}
                            class="px-4 py-2 rounded-lg bg-[var(--accent)] text-white text-sm font-medium hover:opacity-90 disabled:opacity-50 transition-opacity">
                            if *submitting { {"..."} } else { {"Send"} }
                        </button>
                    </div>
                    if let Some(err) = (*submit_error).as_ref() {
                        <p class="text-xs text-red-500 mt-2">{err}</p>
                    }
                </form>

                // Comment list
                if comments.is_empty() {
                    <p class="text-sm text-[var(--text-tertiary)] text-center py-8">
                        {"No comments yet. Be the first!"}
                    </p>
                } else {
                    <div class="space-y-3">
                        { for comments.iter().map(|c| {
                            let time_str = format_timestamp(c.created_at);
                            html! {
                                <div class="bg-[var(--surface)] border border-[var(--border)] rounded-lg p-3">
                                    <div class="flex items-center gap-2 mb-1">
                                        <span class="text-sm font-medium text-[var(--text-primary)]">
                                            {&c.nickname}
                                        </span>
                                        if let Some(region) = c.ip_region.as_ref() {
                                            <span class="text-xs text-[var(--text-tertiary)]">
                                                {region}
                                            </span>
                                        }
                                        <span class="text-xs text-[var(--text-tertiary)] ml-auto">
                                            {time_str}
                                        </span>
                                    </div>
                                    <p class="text-sm text-[var(--text-secondary)]">
                                        {&c.comment_text}
                                    </p>
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

/// Parse LRC format lyrics, stripping timestamp tags and returning text lines.
fn parse_lrc_lines(lrc: &str) -> Vec<String> {
    lrc.lines()
        .map(|line| {
            let mut s = line;
            // Strip all [xx:xx.xx] timestamp tags
            while let Some(start) = s.find('[') {
                if let Some(end) = s[start..].find(']') {
                    let tag = &s[start + 1..start + end];
                    // Check if it looks like a timestamp (contains ':')
                    if tag.contains(':') && tag.len() < 12 {
                        s = &s[start + end + 1..];
                        continue;
                    }
                }
                break;
            }
            s.to_string()
        })
        .collect()
}

fn format_timestamp(epoch_ms: i64) -> String {
    // Simple date formatting from epoch ms
    let secs = epoch_ms / 1000;
    let days_since_epoch = secs / 86400;
    // Approximate: just show relative or raw
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
            // Fallback: show days since epoch as rough date
            format!("day {}", days_since_epoch)
        }
    }
}
