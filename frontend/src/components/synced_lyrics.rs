use wasm_bindgen::JsCast;
use web_sys::ScrollToOptions;
use yew::prelude::*;

/// A single parsed LRC line with timestamp and text.
#[derive(Clone, Debug)]
struct LrcLine {
    time: f64,
    text: String,
    translation: Option<String>,
}

/// Parse LRC format string into sorted `Vec<LrcLine>`.
/// Supports `[mm:ss.xx]text` and multiple timestamps per line.
fn parse_lrc(lrc: &str, translation: Option<&str>) -> Vec<LrcLine> {
    let mut lines: Vec<LrcLine> = Vec::new();

    for raw in lrc.lines() {
        let raw = raw.trim();
        if raw.is_empty() { continue; }

        // Collect all timestamps from the line
        let mut times = Vec::new();
        let mut rest = raw;
        while rest.starts_with('[') {
            if let Some(end) = rest.find(']') {
                let tag = &rest[1..end];
                if let Some(t) = parse_timestamp(tag) {
                    times.push(t);
                    rest = &rest[end + 1..];
                } else {
                    break; // metadata tag like [ti:...], skip
                }
            } else {
                break;
            }
        }

        let text = rest.trim().to_string();
        if times.is_empty() || text.is_empty() { continue; }

        for t in times {
            lines.push(LrcLine { time: t, text: text.clone(), translation: None });
        }
    }

    lines.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));

    // Merge translation lyrics by nearest timestamp
    if let Some(trans) = translation {
        let mut trans_lines: Vec<(f64, String)> = Vec::new();
        for raw in trans.lines() {
            let raw = raw.trim();
            if raw.is_empty() { continue; }
            let mut rest = raw;
            let mut time = None;
            while rest.starts_with('[') {
                if let Some(end) = rest.find(']') {
                    let tag = &rest[1..end];
                    if let Some(t) = parse_timestamp(tag) {
                        time = Some(t);
                        rest = &rest[end + 1..];
                    } else { break; }
                } else { break; }
            }
            let text = rest.trim().to_string();
            if let Some(t) = time {
                if !text.is_empty() {
                    trans_lines.push((t, text));
                }
            }
        }
        trans_lines.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Match each translation to the closest original line
        for (tt, ttext) in &trans_lines {
            if let Some(best) = lines.iter_mut().min_by(|a, b| {
                (a.time - tt).abs().partial_cmp(&(b.time - tt).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            }) {
                best.translation = Some(ttext.clone());
            }
        }
    }

    lines
}

fn parse_timestamp(tag: &str) -> Option<f64> {
    // Format: mm:ss.xx or mm:ss
    let parts: Vec<&str> = tag.split(':').collect();
    if parts.len() != 2 { return None; }
    let minutes: f64 = parts[0].parse().ok()?;
    let seconds: f64 = parts[1].parse().ok()?;
    Some(minutes * 60.0 + seconds)
}

#[derive(Properties, PartialEq)]
pub struct SyncedLyricsProps {
    #[prop_or_default]
    pub lyrics_lrc: Option<AttrValue>,
    #[prop_or_default]
    pub lyrics_translation: Option<AttrValue>,
    pub current_time: f64,
    #[prop_or_default]
    pub lyrics_offset: f64,
}

#[function_component(SyncedLyrics)]
pub fn synced_lyrics(props: &SyncedLyricsProps) -> Html {
    let container_ref = use_node_ref();
    let prev_idx = use_state(|| usize::MAX);

    let parsed = use_memo(
        (props.lyrics_lrc.clone(), props.lyrics_translation.clone()),
        |(lrc, trans)| {
            match lrc {
                Some(l) => parse_lrc(l.as_str(), trans.as_ref().map(|t| t.as_str())),
                None => Vec::new(),
            }
        },
    );

    if parsed.is_empty() {
        return html! {};
    }

    // Find current line index via binary search
    let ct = props.current_time + props.lyrics_offset;
    let current_idx = parsed.partition_point(|l| l.time <= ct).saturating_sub(1);

    // Auto-scroll to current line
    {
        let container_ref = container_ref.clone();
        let prev_idx = prev_idx.clone();
        let idx = current_idx;
        let time_jump = {
            if *prev_idx != usize::MAX && idx != *prev_idx {
                let prev_t = parsed.get(*prev_idx).map(|l| l.time).unwrap_or(0.0);
                let curr_t = parsed.get(idx).map(|l| l.time).unwrap_or(0.0);
                (curr_t - prev_t).abs() > 2.0
            } else {
                false
            }
        };

        use_effect_with(idx, move |idx| {
            let idx = *idx;
            prev_idx.set(idx);

            if let Some(container) = container_ref.cast::<web_sys::HtmlElement>() {
                let selector = format!("[data-lyric-idx=\"{}\"]", idx);
                if let Ok(Some(el)) = container.query_selector(&selector) {
                    let opts = ScrollToOptions::new();
                    opts.set_top(el.unchecked_ref::<web_sys::HtmlElement>().offset_top() as f64
                        - container.client_height() as f64 / 2.0 + 20.0);
                    if time_jump {
                        opts.set_behavior(web_sys::ScrollBehavior::Instant);
                    } else {
                        opts.set_behavior(web_sys::ScrollBehavior::Smooth);
                    }
                    container.scroll_to_with_scroll_to_options(&opts);
                }
            }
            || ()
        });
    }

    html! {
        <div ref={container_ref}
            class="relative max-h-80 overflow-y-auto scroll-smooth px-4 py-6 space-y-1"
            style="mask-image: linear-gradient(to bottom, transparent 0%, black 10%, black 90%, transparent 100%);">
            { for parsed.iter().enumerate().map(|(i, line)| {
                let is_current = i == current_idx;
                let is_past = i < current_idx;

                let line_class = if is_current {
                    "text-[var(--primary)] font-semibold scale-[1.02] bg-[var(--primary)]/5 \
                     rounded-lg px-2 py-1.5 transition-all duration-300"
                } else if is_past {
                    "text-[var(--muted)]/60 px-2 py-1 transition-all duration-300"
                } else {
                    "text-[var(--text)] text-sm px-2 py-1 transition-all duration-300"
                };

                html! {
                    <div key={i} data-lyric-idx={i.to_string()} class={line_class}>
                        <p class="m-0 leading-relaxed">{&line.text}</p>
                        { if let Some(ref trans) = line.translation {
                            html! {
                                <p class={if is_current {
                                    "m-0 text-xs text-[var(--primary)]/70 mt-0.5"
                                } else {
                                    "m-0 text-xs text-[var(--muted)]/50 mt-0.5"
                                }}>
                                    {trans}
                                </p>
                            }
                        } else {
                            html! {}
                        }}
                    </div>
                }
            })}
        </div>
    }
}
