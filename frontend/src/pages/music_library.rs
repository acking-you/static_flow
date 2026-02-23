use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::components::icons::{Icon, IconName};
use crate::components::pagination::Pagination;
use crate::router::Route;

const PAGE_SIZE: usize = 20;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct MusicLibraryQuery {
    artist: Option<String>,
    album: Option<String>,
}

#[function_component(MusicLibraryPage)]
pub fn music_library_page() -> Html {
    let location = use_location();
    let query_string = location.as_ref().map(|l| l.query_str().to_string()).unwrap_or_default();

    // Initialize filter state directly from URL to avoid double-fetch on mount
    let initial_query = location.as_ref()
        .and_then(|loc| loc.query::<MusicLibraryQuery>().ok())
        .unwrap_or(MusicLibraryQuery { artist: None, album: None });

    let page_songs = use_state(Vec::<api::SongListItem>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let active_artist = use_state(|| initial_query.artist.clone());
    let active_album = use_state(|| initial_query.album.clone());
    let current_page = use_state(|| 1_usize);
    let total = use_state(|| 0_usize);

    // Sync URL query params → state on subsequent navigation
    {
        let active_artist = active_artist.clone();
        let active_album = active_album.clone();
        let current_page = current_page.clone();
        let location = location.clone();
        use_effect_with(query_string, move |_| {
            if let Some(ref loc) = location {
                if let Ok(q) = loc.query::<MusicLibraryQuery>() {
                    active_artist.set(q.artist);
                    active_album.set(q.album);
                    current_page.set(1);
                }
            }
            || ()
        });
    }

    // Fetch one page of songs when filter or page changes
    {
        let page_songs = page_songs.clone();
        let loading = loading.clone();
        let error = error.clone();
        let total = total.clone();
        let deps = (
            (*active_artist).clone(),
            (*active_album).clone(),
            *current_page,
        );
        use_effect_with(deps, move |deps| {
            let (artist, album, page) = deps.clone();
            let offset = (page - 1) * PAGE_SIZE;
            loading.set(true);
            error.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match api::fetch_songs(
                    Some(PAGE_SIZE), Some(offset),
                    artist.as_deref(), album.as_deref(), None,
                ).await {
                    Ok(resp) => {
                        total.set(resp.total);
                        page_songs.set(resp.songs);
                    }
                    Err(e) => {
                        error.set(Some(e));
                    }
                }
                loading.set(false);
            });
            || ()
        });
    }

    let total_val = *total;
    let total_pages = if total_val == 0 { 1 } else { (total_val + PAGE_SIZE - 1) / PAGE_SIZE };

    let on_artist_click = {
        let active_artist = active_artist.clone();
        let current_page = current_page.clone();
        move |artist: String| {
            let active_artist = active_artist.clone();
            let current_page = current_page.clone();
            Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                active_artist.set(Some(artist.clone()));
                current_page.set(1);
            })
        }
    };

    let on_album_click = {
        let active_album = active_album.clone();
        let current_page = current_page.clone();
        move |album: String| {
            let active_album = active_album.clone();
            let current_page = current_page.clone();
            Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                active_album.set(Some(album.clone()));
                current_page.set(1);
            })
        }
    };

    let clear_artist = {
        let active_artist = active_artist.clone();
        let current_page = current_page.clone();
        Callback::from(move |_: MouseEvent| {
            active_artist.set(None);
            current_page.set(1);
        })
    };

    let clear_album = {
        let active_album = active_album.clone();
        let current_page = current_page.clone();
        Callback::from(move |_: MouseEvent| {
            active_album.set(None);
            current_page.set(1);
        })
    };

    let on_page_change = {
        let current_page = current_page.clone();
        Callback::from(move |page: usize| { current_page.set(page); })
    };

    html! {
        <div class="max-w-7xl mx-auto px-4 py-8">
            <div class="mb-6">
                <h1 class="text-3xl font-bold text-[var(--text)]" style="font-family: 'Fraunces', serif;">
                    {"Music Library"}
                </h1>
                <p class="text-[var(--muted)] mt-1">
                    {"Explore and play the music collection"}
                </p>
            </div>

            if active_artist.is_some() || active_album.is_some() {
                <div class="flex flex-wrap gap-2 mb-4">
                    if let Some(ref artist) = *active_artist {
                        <span class="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-xs \
                                     bg-[var(--primary)]/10 text-[var(--primary)] border border-[var(--primary)]/20">
                            {format!("Artist: {}", artist)}
                            <button onclick={clear_artist.clone()} type="button"
                                class="hover:opacity-70 transition-opacity">
                                <Icon name={IconName::X} size={12} />
                            </button>
                        </span>
                    }
                    if let Some(ref album) = *active_album {
                        <span class="inline-flex items-center gap-1.5 px-3 py-1 rounded-full text-xs \
                                     bg-[var(--primary)]/10 text-[var(--primary)] border border-[var(--primary)]/20">
                            {format!("Album: {}", album)}
                            <button onclick={clear_album.clone()} type="button"
                                class="hover:opacity-70 transition-opacity">
                                <Icon name={IconName::X} size={12} />
                            </button>
                        </span>
                    }
                </div>
            }

            if *loading {
                <div class="flex justify-center py-20">
                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--primary)]" />
                </div>
            } else if let Some(ref err) = *error {
                <div class="text-center py-20 text-red-500">
                    {format!("Failed to load: {}", err)}
                </div>
            } else if page_songs.is_empty() {
                <div class="text-center py-20 text-[var(--muted)]">
                    {"No music found"}
                </div>
            } else {
                <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-5">
                    { for page_songs.iter().map(|song| {
                        render_song_card(song, &on_artist_click, &on_album_click)
                    })}
                </div>
                if total_pages > 1 {
                    <div class="flex justify-center mt-8">
                        <Pagination
                            current_page={*current_page}
                            total_pages={total_pages}
                            on_page_change={on_page_change.clone()}
                        />
                    </div>
                }
            }
        </div>
    }
}

fn render_song_card(
    song: &api::SongListItem,
    on_artist_click: &dyn Fn(String) -> Callback<MouseEvent>,
    on_album_click: &dyn Fn(String) -> Callback<MouseEvent>,
) -> Html {
    let cover_url = api::song_cover_url(song.cover_image.as_deref());
    let duration = format_duration(song.duration_ms);
    let id = song.id.clone();
    let artist = song.artist.clone();
    let album = song.album.clone();
    let artist_cb = on_artist_click(artist.clone());
    let album_cb = on_album_click(album.clone());

    html! {
        <div class="group bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl \
                    overflow-hidden flex flex-col transition-all duration-300 ease-out \
                    hover:shadow-[var(--shadow-8)] hover:border-[var(--primary)] hover:-translate-y-2">
            <Link<Route> to={Route::MusicPlayer { id }}>
                <div class="aspect-square bg-[var(--surface-alt)] relative overflow-hidden">
                    if cover_url.is_empty() {
                        <div class="w-full h-full flex items-center justify-center text-[var(--muted)]">
                            <Icon name={IconName::Music} size={48} class={classes!("opacity-30")} />
                        </div>
                    } else {
                        <img src={cover_url} alt={song.title.clone()} loading="lazy" referrerpolicy="no-referrer"
                            class="w-full h-full object-cover transition-transform duration-500 ease-out group-hover:scale-105" />
                    }
                    <div class="absolute inset-0 bg-black/0 group-hover:bg-black/30 transition-all duration-300 \
                                flex items-center justify-center opacity-0 group-hover:opacity-100">
                        <div class="w-12 h-12 rounded-full bg-white/90 flex items-center justify-center shadow-lg">
                            <Icon name={IconName::Play} size={20} color="#000" />
                        </div>
                    </div>
                    <div class="absolute bottom-2 right-2 bg-black/60 text-white text-xs px-2 py-0.5 rounded">
                        {&duration}
                    </div>
                </div>
            </Link<Route>>
            <div class="p-3 flex flex-col gap-1">
                <h3 class="text-sm font-semibold text-[var(--text)] truncate leading-tight"
                    style="font-family: 'Fraunces', serif;">
                    {&song.title}
                </h3>
                <a href="#" onclick={artist_cb}
                    class="text-xs text-[var(--muted)] truncate hover:text-[var(--primary)] transition-colors cursor-pointer">
                    {&song.artist}
                </a>
                if !song.album.is_empty() {
                    <a href="#" onclick={album_cb}
                        class="inline-flex items-center self-start px-2 py-0.5 rounded-full text-[10px] \
                               bg-[var(--surface-alt)] border border-[var(--border)] text-[var(--muted)] \
                               hover:border-[var(--primary)] hover:text-[var(--primary)] transition-all truncate max-w-full">
                        {&song.album}
                    </a>
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

