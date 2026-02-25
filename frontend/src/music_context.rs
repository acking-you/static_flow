use std::{collections::HashSet, rc::Rc};

use yew::prelude::*;

use crate::api::{SongDetail, SongSearchResult};

#[derive(Debug, Clone, PartialEq)]
pub enum NextSongMode {
    Random,
    Semantic,
    PlaylistSequential,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MusicPlayerState {
    pub current_song: Option<SongDetail>,
    pub song_id: Option<String>,
    pub playing: bool,
    pub current_time: f64,
    pub duration: f64,
    pub volume: f64,
    pub minimized: bool,
    pub visible: bool,
    pub history: Vec<(String, SongDetail)>,
    pub history_index: Option<usize>,
    pub next_mode: NextSongMode,
    pub candidates: Vec<SongSearchResult>,
    pub playlist_ids: Vec<String>,
    pub playlist_source: Option<String>,
    pub lyrics_offset: f64,
}

impl Default for MusicPlayerState {
    fn default() -> Self {
        Self {
            current_song: None,
            song_id: None,
            playing: false,
            current_time: 0.0,
            duration: 0.0,
            volume: 1.0,
            minimized: false,
            visible: false,
            history: Vec::new(),
            history_index: None,
            next_mode: NextSongMode::Random,
            candidates: Vec::new(),
            playlist_ids: Vec::new(),
            playlist_source: None,
            lyrics_offset: 0.0,
        }
    }
}

pub enum MusicAction {
    PlaySong {
        song: SongDetail,
        id: String,
    },
    /// Load song info into context without auto-playing (e.g. page refresh).
    LoadSong {
        song: SongDetail,
        id: String,
    },
    TogglePlay,
    Pause,
    SetTime(f64),
    SetDuration(f64),
    SetVolume(f64),
    Minimize,
    Expand,
    Close,
    PlayPrev,
    PlayNext {
        fallback: Option<(SongDetail, String)>,
    },
    SetNextMode(NextSongMode),
    SetCandidates(Vec<SongSearchResult>),
    SetPlaylist {
        source: String,
        ids: Vec<String>,
    },
    SetLyricsOffset(f64),
}

impl Reducible for MusicPlayerState {
    type Action = MusicAction;

    fn reduce(self: Rc<Self>, action: Self::Action) -> Rc<Self> {
        let mut next = (*self).clone();
        match action {
            MusicAction::PlaySong {
                song,
                id,
            } => {
                let changed = next.song_id.as_deref() != Some(&id);
                if changed {
                    // Truncate forward history and push new entry
                    if let Some(idx) = next.history_index {
                        next.history.truncate(idx + 1);
                    }
                    next.history.push((id.clone(), song.clone()));
                    next.history_index = Some(next.history.len() - 1);
                    next.current_time = 0.0;
                    next.duration = 0.0;
                }
                next.current_song = Some(song);
                next.song_id = Some(id);
                next.playing = true;
                next.visible = true;
                next.minimized = false;
                next.lyrics_offset = 0.0;
            },
            MusicAction::LoadSong {
                song,
                id,
            } => {
                let changed = next.song_id.as_deref() != Some(&id);
                if changed {
                    if let Some(idx) = next.history_index {
                        next.history.truncate(idx + 1);
                    }
                    next.history.push((id.clone(), song.clone()));
                    next.history_index = Some(next.history.len() - 1);
                    next.current_time = 0.0;
                    next.duration = 0.0;
                }
                next.current_song = Some(song);
                next.song_id = Some(id);
                // Don't set playing=true — user must click play
                next.visible = true;
                next.minimized = false;
                next.lyrics_offset = 0.0;
            },
            MusicAction::TogglePlay => {
                next.playing = !next.playing;
            },
            MusicAction::Pause => {
                next.playing = false;
            },
            MusicAction::SetTime(t) => {
                next.current_time = t;
            },
            MusicAction::SetDuration(d) => {
                next.duration = d;
            },
            MusicAction::SetVolume(v) => {
                next.volume = v;
            },
            MusicAction::Minimize => {
                next.minimized = true;
            },
            MusicAction::Expand => {
                next.minimized = false;
            },
            MusicAction::Close => {
                next.playing = false;
                next.visible = false;
                next.minimized = false;
            },
            MusicAction::PlayPrev => {
                if let Some(idx) = next.history_index {
                    if idx > 0 {
                        let new_idx = idx - 1;
                        let (ref id, ref song) = next.history[new_idx];
                        next.song_id = Some(id.clone());
                        next.current_song = Some(song.clone());
                        next.history_index = Some(new_idx);
                        next.playing = true;
                        next.visible = true;
                        next.current_time = 0.0;
                        next.duration = 0.0;
                    }
                }
            },
            MusicAction::PlayNext {
                fallback,
            } => {
                if let Some(idx) = next.history_index {
                    if idx + 1 < next.history.len() {
                        // Forward in history
                        let new_idx = idx + 1;
                        let (ref id, ref song) = next.history[new_idx];
                        next.song_id = Some(id.clone());
                        next.current_song = Some(song.clone());
                        next.history_index = Some(new_idx);
                        next.playing = true;
                        next.visible = true;
                        next.current_time = 0.0;
                        next.duration = 0.0;
                    } else if let Some((song, id)) = fallback {
                        // Push new entry from fallback
                        next.history.push((id.clone(), song.clone()));
                        next.history_index = Some(next.history.len() - 1);
                        next.song_id = Some(id);
                        next.current_song = Some(song);
                        next.playing = true;
                        next.visible = true;
                        next.current_time = 0.0;
                        next.duration = 0.0;
                    }
                } else if let Some((song, id)) = fallback {
                    // No history yet, start fresh
                    next.history.push((id.clone(), song.clone()));
                    next.history_index = Some(0);
                    next.song_id = Some(id);
                    next.current_song = Some(song);
                    next.playing = true;
                    next.visible = true;
                    next.current_time = 0.0;
                    next.duration = 0.0;
                }
            },
            MusicAction::SetNextMode(mode) => {
                if mode != NextSongMode::Semantic {
                    next.candidates.clear();
                }
                next.next_mode = mode;
            },
            MusicAction::SetCandidates(c) => {
                next.candidates = c;
            },
            MusicAction::SetPlaylist {
                source,
                ids,
            } => {
                let mut seen = HashSet::new();
                let mut normalized = Vec::with_capacity(ids.len());
                for id in ids {
                    let trimmed = id.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if seen.insert(trimmed.to_string()) {
                        normalized.push(trimmed.to_string());
                    }
                }
                next.playlist_ids = normalized;
                next.playlist_source = if source.trim().is_empty() { None } else { Some(source) };
            },
            MusicAction::SetLyricsOffset(o) => {
                let limit = if next.duration > 0.0 { next.duration } else { 600.0 };
                next.lyrics_offset = o.clamp(-limit, limit);
            },
        }
        Rc::new(next)
    }
}

pub type MusicPlayerContext = UseReducerHandle<MusicPlayerState>;

#[derive(Properties, PartialEq)]
pub struct MusicPlayerProviderProps {
    pub children: Html,
}

#[function_component(MusicPlayerProvider)]
pub fn music_player_provider(props: &MusicPlayerProviderProps) -> Html {
    let state = use_reducer(MusicPlayerState::default);
    html! {
        <ContextProvider<MusicPlayerContext> context={state}>
            {props.children.clone()}
        </ContextProvider<MusicPlayerContext>>
    }
}
