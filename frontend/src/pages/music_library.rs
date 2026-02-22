use yew::prelude::*;
use yew_router::prelude::*;

use crate::api;
use crate::router::Route;

#[function_component(MusicLibraryPage)]
pub fn music_library_page() -> Html {
    let songs = use_state(|| Vec::new());
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);

    {
        let songs = songs.clone();
        let loading = loading.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            wasm_bindgen_futures::spawn_local(async move {
                match api::fetch_songs(Some(20), None).await {
                    Ok(list) => {
                        songs.set(list);
                        loading.set(false);
                    }
                    Err(e) => {
                        error.set(Some(e));
                        loading.set(false);
                    }
                }
            });
            || ()
        });
    }

    html! {
        <div class="max-w-7xl mx-auto px-4 py-8">
            <div class="mb-8">
                <h1 class="text-3xl font-bold text-[var(--text-primary)] mb-2">
                    {"Music Library"}
                </h1>
                <p class="text-[var(--text-secondary)]">
                    {"Explore and play the music collection"}
                </p>
            </div>

            if *loading {
                <div class="flex justify-center py-20">
                    <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--accent)]"></div>
                </div>
            } else if let Some(err) = (*error).as_ref() {
                <div class="text-center py-20 text-red-500">
                    {format!("Failed to load: {}", err)}
                </div>
            } else if songs.is_empty() {
                <div class="text-center py-20 text-[var(--text-secondary)]">
                    {"No music yet"}
                </div>
            } else {
                <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-4">
                    { for songs.iter().map(|song| {
                        let cover_url = api::song_cover_url(song.cover_image.as_deref());
                        let duration = format_duration(song.duration_ms);
                        let id = song.id.clone();
                        html! {
                            <Link<Route> to={Route::MusicPlayer { id }}>
                                <div class="group rounded-xl overflow-hidden bg-[var(--surface)] border border-[var(--border)] hover:border-[var(--accent)] transition-all duration-200 hover:shadow-lg cursor-pointer">
                                    <div class="aspect-square bg-[var(--surface-alt)] relative overflow-hidden">
                                        if cover_url.is_empty() {
                                            <div class="w-full h-full flex items-center justify-center text-[var(--text-tertiary)]">
                                                <svg xmlns="http://www.w3.org/2000/svg" class="w-16 h-16 opacity-30" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5">
                                                    <path d="M9 18V5l12-2v13"/>
                                                    <circle cx="6" cy="18" r="3"/>
                                                    <circle cx="18" cy="16" r="3"/>
                                                </svg>
                                            </div>
                                        } else {
                                            <img src={cover_url} alt={song.title.clone()}
                                                class="w-full h-full object-cover group-hover:scale-105 transition-transform duration-300" />
                                        }
                                        <div class="absolute bottom-2 right-2 bg-black/60 text-white text-xs px-2 py-0.5 rounded">
                                            {&duration}
                                        </div>
                                    </div>
                                    <div class="p-3">
                                        <h3 class="text-sm font-medium text-[var(--text-primary)] truncate">
                                            {&song.title}
                                        </h3>
                                        <p class="text-xs text-[var(--text-secondary)] truncate mt-0.5">
                                            {&song.artist}
                                        </p>
                                    </div>
                                </div>
                            </Link<Route>>
                        }
                    })}
                </div>
            }
        </div>
    }
}

fn format_duration(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}", minutes, seconds)
}
