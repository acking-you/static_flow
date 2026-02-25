use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api,
    components::{
        icons::{Icon, IconName},
        image_with_loading::ImageWithLoading,
        pagination::Pagination,
    },
    i18n::current::music_wish as wish_t,
    music_context::{MusicAction, MusicPlayerContext},
    router::Route,
};

const PAGE_SIZE: usize = 20;
const RANDOM_RECOMMEND_LIMIT: usize = 10;
const WISH_PAGE_SIZE: usize = 12;

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
struct MusicLibraryQuery {
    artist: Option<String>,
    album: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LibraryDisplayMode {
    RandomRecommended,
    AllSongs,
}

#[function_component(MusicLibraryPage)]
pub fn music_library_page() -> Html {
    let location = use_location();
    let query_string = location
        .as_ref()
        .map(|l| l.query_str().to_string())
        .unwrap_or_default();

    // Initialize filter state directly from URL to avoid double-fetch on mount
    let initial_query = location
        .as_ref()
        .and_then(|loc| loc.query::<MusicLibraryQuery>().ok())
        .unwrap_or(MusicLibraryQuery {
            artist: None,
            album: None,
        });

    let page_songs = use_state(Vec::<api::SongListItem>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let random_songs = use_state(Vec::<api::SongListItem>::new);
    let random_loading = use_state(|| true);
    let random_error = use_state(|| None::<String>);
    let random_refresh_tick = use_state(|| 0_u64);
    let display_mode = use_state(|| LibraryDisplayMode::RandomRecommended);
    let active_artist = use_state(|| initial_query.artist.clone());
    let active_album = use_state(|| initial_query.album.clone());
    let current_page = use_state(|| 1_usize);
    let total = use_state(|| 0_usize);
    let all_songs_request_seq = use_mut_ref(|| 0_u64);
    let random_request_seq = use_mut_ref(|| 0_u64);
    let player_ctx = use_context::<MusicPlayerContext>();

    // Wish board state
    let wishes = use_state(Vec::<api::MusicWishItem>::new);
    let wish_loading = use_state(|| false);
    let wish_list_error = use_state(|| None::<String>);
    let wish_page = use_state(|| 1_usize);
    let wish_total = use_state(|| 0_usize);
    let wish_refresh_tick = use_state(|| 0_u64);
    let wish_request_seq = use_mut_ref(|| 0_u64);
    let wish_form_song = use_state(String::new);
    let wish_form_artist = use_state(String::new);
    let wish_form_message = use_state(String::new);
    let wish_form_nickname = use_state(String::new);
    let wish_form_email = use_state(String::new);
    let wish_submitting = use_state(|| false);
    let wish_submit_msg = use_state(|| None::<String>);
    let wish_submit_err = use_state(|| None::<String>);

    // Hero search state
    let hero_query = use_state(String::new);
    let hero_focused = use_state(|| false);

    let on_hero_input = {
        let hero_query = hero_query.clone();
        Callback::from(move |e: InputEvent| {
            if let Some(input) = e.target_dyn_into::<web_sys::HtmlInputElement>() {
                hero_query.set(input.value());
            }
        })
    };

    let do_hero_search = {
        let hero_query = hero_query.clone();
        move || {
            let q = hero_query.trim().to_string();
            if q.is_empty() {
                return;
            }
            let encoded = urlencoding::encode(&q);
            let url = crate::config::route_path(&format!("/search?q={encoded}&mode=music"));
            if let Some(window) = web_sys::window() {
                if let Ok(history) = window.history() {
                    let _ =
                        history.push_state_with_url(&wasm_bindgen::JsValue::NULL, "", Some(&url));
                    if let Ok(event) = web_sys::Event::new("popstate") {
                        let _ = window.dispatch_event(&event);
                    }
                }
            }
        }
    };

    let on_hero_search = {
        let do_search = do_hero_search.clone();
        Callback::from(move |_: MouseEvent| do_search())
    };

    let on_hero_keypress = {
        let do_search = do_hero_search;
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                do_search();
            }
        })
    };

    let on_hero_focus = {
        let hero_focused = hero_focused.clone();
        Callback::from(move |_: FocusEvent| hero_focused.set(true))
    };

    let on_hero_blur = {
        let hero_focused = hero_focused.clone();
        Callback::from(move |_: FocusEvent| hero_focused.set(false))
    };

    // Sync URL query params → state on subsequent navigation
    {
        let active_artist = active_artist.clone();
        let active_album = active_album.clone();
        let current_page = current_page.clone();
        let display_mode = display_mode.clone();
        let location = location.clone();
        use_effect_with(query_string, move |_| {
            if let Some(ref loc) = location {
                if let Ok(q) = loc.query::<MusicLibraryQuery>() {
                    let has_filter = q.artist.is_some() || q.album.is_some();
                    active_artist.set(q.artist);
                    active_album.set(q.album);
                    if has_filter {
                        display_mode.set(LibraryDisplayMode::AllSongs);
                    }
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
        let all_songs_request_seq = all_songs_request_seq.clone();
        let deps =
            (*display_mode, (*active_artist).clone(), (*active_album).clone(), *current_page);
        use_effect_with(deps, move |deps| {
            let request_id = {
                let mut seq = all_songs_request_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            let (mode, artist, album, page) = deps.clone();
            if mode != LibraryDisplayMode::AllSongs {
                loading.set(false);
                error.set(None);
            } else {
                let offset = (page - 1) * PAGE_SIZE;
                loading.set(true);
                error.set(None);
                let all_songs_request_seq = all_songs_request_seq.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_songs(
                        Some(PAGE_SIZE),
                        Some(offset),
                        artist.as_deref(),
                        album.as_deref(),
                        None,
                    )
                    .await
                    {
                        Ok(resp) => {
                            if *all_songs_request_seq.borrow() != request_id {
                                return;
                            }
                            total.set(resp.total);
                            page_songs.set(resp.songs);
                        },
                        Err(e) => {
                            if *all_songs_request_seq.borrow() != request_id {
                                return;
                            }
                            error.set(Some(e));
                        },
                    }
                    if *all_songs_request_seq.borrow() != request_id {
                        return;
                    }
                    loading.set(false);
                });
            }
            || ()
        });
    }

    // Fetch default random recommendations.
    {
        let display_mode = *display_mode;
        let random_songs = random_songs.clone();
        let random_loading = random_loading.clone();
        let random_error = random_error.clone();
        let random_request_seq = random_request_seq.clone();
        let refresh_tick = *random_refresh_tick;
        use_effect_with((display_mode, refresh_tick), move |(mode, _)| {
            let request_id = {
                let mut seq = random_request_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            if *mode != LibraryDisplayMode::RandomRecommended {
                random_loading.set(false);
                random_error.set(None);
            } else {
                random_loading.set(true);
                random_error.set(None);
                let random_request_seq = random_request_seq.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    match api::fetch_random_recommended_songs(Some(RANDOM_RECOMMEND_LIMIT), &[])
                        .await
                    {
                        Ok(songs) => {
                            if *random_request_seq.borrow() != request_id {
                                return;
                            }
                            random_songs.set(songs);
                        },
                        Err(err) => {
                            if *random_request_seq.borrow() != request_id {
                                return;
                            }
                            random_error.set(Some(err));
                        },
                    }
                    if *random_request_seq.borrow() != request_id {
                        return;
                    }
                    random_loading.set(false);
                });
            }
            || ()
        });
    }

    let total_val = *total;
    let total_pages = if total_val == 0 { 1 } else { total_val.div_ceil(PAGE_SIZE) };
    let wish_total_val = *wish_total;
    let wish_total_pages = if wish_total_val == 0 {
        1
    } else {
        wish_total_val.div_ceil(WISH_PAGE_SIZE)
    };

    let on_artist_click = {
        let active_artist = active_artist.clone();
        let current_page = current_page.clone();
        let display_mode = display_mode.clone();
        move |artist: String| {
            let active_artist = active_artist.clone();
            let current_page = current_page.clone();
            let display_mode = display_mode.clone();
            Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                active_artist.set(Some(artist.clone()));
                current_page.set(1);
                display_mode.set(LibraryDisplayMode::AllSongs);
            })
        }
    };

    let on_album_click = {
        let active_album = active_album.clone();
        let current_page = current_page.clone();
        let display_mode = display_mode.clone();
        move |album: String| {
            let active_album = active_album.clone();
            let current_page = current_page.clone();
            let display_mode = display_mode.clone();
            Callback::from(move |e: MouseEvent| {
                e.prevent_default();
                active_album.set(Some(album.clone()));
                current_page.set(1);
                display_mode.set(LibraryDisplayMode::AllSongs);
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
        Callback::from(move |page: usize| {
            current_page.set(page);
        })
    };

    let on_wish_page_change = {
        let wish_page = wish_page.clone();
        Callback::from(move |page: usize| {
            wish_page.set(page);
        })
    };

    let on_switch_random = {
        let display_mode = display_mode.clone();
        Callback::from(move |_: MouseEvent| {
            display_mode.set(LibraryDisplayMode::RandomRecommended);
        })
    };

    let on_switch_all = {
        let display_mode = display_mode.clone();
        Callback::from(move |_: MouseEvent| {
            display_mode.set(LibraryDisplayMode::AllSongs);
        })
    };

    let on_refresh_random = {
        let random_refresh_tick = random_refresh_tick.clone();
        Callback::from(move |_: MouseEvent| {
            random_refresh_tick.set(*random_refresh_tick + 1);
        })
    };

    // Keep global playlist synced with current Music Library page results.
    {
        let player_ctx = player_ctx.clone();
        let mode = *display_mode;
        let active_artist = (*active_artist).clone();
        let active_album = (*active_album).clone();
        let current_page = *current_page;
        let random_tick = *random_refresh_tick;
        let ids: Vec<String> = if mode == LibraryDisplayMode::RandomRecommended {
            random_songs.iter().map(|song| song.id.clone()).collect()
        } else {
            page_songs.iter().map(|song| song.id.clone()).collect()
        };
        use_effect_with(
            (
                ids.clone(),
                mode,
                active_artist.clone(),
                active_album.clone(),
                current_page,
                random_tick,
            ),
            move |(ids, mode, artist, album, page, tick)| {
                if let Some(ctx) = player_ctx.as_ref() {
                    let source = if *mode == LibraryDisplayMode::RandomRecommended {
                        format!("music-library:random:tick={tick}")
                    } else {
                        format!(
                            "music-library:all:artist={}:album={}:page={}",
                            artist.clone().unwrap_or_default(),
                            album.clone().unwrap_or_default(),
                            page
                        )
                    };
                    ctx.dispatch(MusicAction::SetPlaylist {
                        source,
                        ids: ids.clone(),
                    });
                }
                || ()
            },
        );
    }

    // Fetch wishes when page changes or manual refresh is triggered.
    {
        let wishes = wishes.clone();
        let wish_loading = wish_loading.clone();
        let wish_list_error = wish_list_error.clone();
        let wish_total = wish_total.clone();
        let wish_request_seq = wish_request_seq.clone();
        let deps = (*wish_page, *wish_refresh_tick);
        use_effect_with(deps, move |(page, _)| {
            let request_id = {
                let mut seq = wish_request_seq.borrow_mut();
                *seq += 1;
                *seq
            };
            let wishes = wishes.clone();
            let wish_loading = wish_loading.clone();
            let wish_list_error = wish_list_error.clone();
            let wish_total = wish_total.clone();
            let wish_request_seq = wish_request_seq.clone();
            let offset = ((*page).saturating_sub(1)) * WISH_PAGE_SIZE;
            wish_loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match api::fetch_music_wishes(Some(WISH_PAGE_SIZE), Some(offset)).await {
                    Ok(resp) => {
                        if *wish_request_seq.borrow() != request_id {
                            return;
                        }
                        wishes.set(resp.wishes);
                        wish_total.set(resp.total);
                        wish_list_error.set(None);
                    },
                    Err(err) => {
                        if *wish_request_seq.borrow() != request_id {
                            return;
                        }
                        wish_list_error.set(Some(err));
                    },
                }
                if *wish_request_seq.borrow() != request_id {
                    return;
                }
                wish_loading.set(false);
            });
            || ()
        });
    }

    // Manual refresh wishes callback
    let on_refresh_wishes = {
        let wish_refresh_tick = wish_refresh_tick.clone();
        Callback::from(move |_: MouseEvent| {
            wish_refresh_tick.set(*wish_refresh_tick + 1);
        })
    };

    let on_wish_submit = {
        let wish_form_song = wish_form_song.clone();
        let wish_form_artist = wish_form_artist.clone();
        let wish_form_message = wish_form_message.clone();
        let wish_form_nickname = wish_form_nickname.clone();
        let wish_form_email = wish_form_email.clone();
        let wish_submitting = wish_submitting.clone();
        let wish_submit_msg = wish_submit_msg.clone();
        let wish_submit_err = wish_submit_err.clone();
        let wish_page = wish_page.clone();
        let wish_refresh_tick = wish_refresh_tick.clone();
        Callback::from(move |e: SubmitEvent| {
            e.prevent_default();
            let song = (*wish_form_song).trim().to_string();
            let artist = (*wish_form_artist).trim().to_string();
            let message = (*wish_form_message).trim().to_string();
            let nickname = (*wish_form_nickname).trim().to_string();
            let email = (*wish_form_email).trim().to_string();
            if song.is_empty() || message.is_empty() {
                return;
            }
            let artist_opt = if artist.is_empty() { None } else { Some(artist.clone()) };
            let nickname_opt = if nickname.is_empty() { None } else { Some(nickname.clone()) };
            let email_opt = if email.is_empty() { None } else { Some(email.clone()) };
            let frontend_page_url = web_sys::window().and_then(|w| w.location().href().ok());
            let wish_submitting = wish_submitting.clone();
            let wish_submit_msg = wish_submit_msg.clone();
            let wish_submit_err = wish_submit_err.clone();
            let wish_page = wish_page.clone();
            let wish_refresh_tick = wish_refresh_tick.clone();
            let wish_form_song = wish_form_song.clone();
            let wish_form_artist = wish_form_artist.clone();
            let wish_form_message = wish_form_message.clone();
            let wish_form_email = wish_form_email.clone();
            wish_submitting.set(true);
            wish_submit_msg.set(None);
            wish_submit_err.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                match api::submit_music_wish(
                    &song,
                    artist_opt.as_deref(),
                    &message,
                    nickname_opt.as_deref(),
                    email_opt.as_deref(),
                    frontend_page_url.as_deref(),
                )
                .await
                {
                    Ok(_) => {
                        wish_submit_msg.set(Some(wish_t::SUBMIT_SUCCESS.to_string()));
                        wish_form_song.set(String::new());
                        wish_form_artist.set(String::new());
                        wish_form_message.set(String::new());
                        wish_form_email.set(String::new());
                        wish_page.set(1);
                        wish_refresh_tick.set(*wish_refresh_tick + 1);
                    },
                    Err(e) => {
                        wish_submit_err.set(Some(e));
                    },
                }
                wish_submitting.set(false);
            });
        })
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

            <div class="mb-5 flex flex-wrap items-center gap-2">
                <button type="button"
                    onclick={on_switch_random}
                    class={classes!(
                        "px-4", "py-2", "rounded-lg", "text-sm", "font-medium", "transition-colors",
                        if *display_mode == LibraryDisplayMode::RandomRecommended {
                            "bg-[var(--primary)] text-white"
                        } else {
                            "bg-[var(--surface)] border border-[var(--border)] text-[var(--text)] hover:bg-[var(--surface-alt)]"
                        }
                    )}>
                    {"Random 10 Picks"}
                </button>
                <button type="button"
                    onclick={on_switch_all}
                    class={classes!(
                        "px-4", "py-2", "rounded-lg", "text-sm", "font-medium", "transition-colors",
                        if *display_mode == LibraryDisplayMode::AllSongs {
                            "bg-[var(--primary)] text-white"
                        } else {
                            "bg-[var(--surface)] border border-[var(--border)] text-[var(--text)] hover:bg-[var(--surface-alt)]"
                        }
                    )}>
                    {"View All Songs"}
                </button>
                if *display_mode == LibraryDisplayMode::RandomRecommended {
                    <button type="button"
                        onclick={on_refresh_random}
                        class="px-3 py-2 rounded-lg text-xs font-medium bg-[var(--surface)] border border-[var(--border)] text-[var(--muted)] hover:text-[var(--text)] hover:bg-[var(--surface-alt)] transition-colors">
                        {"Refresh Random"}
                    </button>
                }
            </div>

            // Hero search box
            <div class={classes!(
                "music-search-hero",
                hero_focused.then_some("focused"),
            )}>
                <div class="music-search-hero-inner">
                    <i class="fas fa-music music-search-icon" />
                    <input type="text"
                        placeholder="搜索歌曲、歌手或专辑..."
                        class="music-search-input"
                        value={(*hero_query).clone()}
                        oninput={on_hero_input}
                        onkeypress={on_hero_keypress}
                        onfocus={on_hero_focus}
                        onblur={on_hero_blur}
                    />
                    <button class="music-search-btn" onclick={on_hero_search} type="button">
                        <i class="fas fa-search" />
                    </button>
                </div>
            </div>

            if *display_mode == LibraryDisplayMode::AllSongs && (active_artist.is_some() || active_album.is_some()) {
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

            if *display_mode == LibraryDisplayMode::RandomRecommended {
                if *random_loading {
                    <div class="flex justify-center py-20">
                        <div class="animate-spin rounded-full h-8 w-8 border-b-2 border-[var(--primary)]" />
                    </div>
                } else if let Some(ref err) = *random_error {
                    <div class="text-center py-20 text-red-500">
                        {format!("Failed to load random picks: {}", err)}
                    </div>
                } else if random_songs.is_empty() {
                    <div class="text-center py-20 text-[var(--muted)]">
                        {"No random picks available"}
                    </div>
                } else {
                    <p class="text-xs text-[var(--muted)] mb-4">
                        {"Showing 10 random recommendations. Click \"Refresh Random\" for a different set."}
                    </p>
                    <div class="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 xl:grid-cols-5 gap-5">
                        { for random_songs.iter().map(|song| {
                            render_song_card(song, &on_artist_click, &on_album_click)
                        })}
                    </div>
                }
            } else {
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
            }

            // Wish board section
            <div class="mt-16 border-t border-[var(--border)] pt-10">
                <h2 class="text-2xl font-bold text-[var(--text)] mb-1" style="font-family: 'Fraunces', serif;">
                    {wish_t::SECTION_TITLE}
                </h2>
                <p class="text-[var(--muted)] text-sm mb-6">{wish_t::SECTION_SUBTITLE}</p>

                <form onsubmit={on_wish_submit}
                    class="bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl p-5 mb-8 \
                           grid grid-cols-1 sm:grid-cols-2 gap-4">
                    <div>
                        <label class="block text-xs text-[var(--muted)] mb-1">{wish_t::SONG_NAME_LABEL}</label>
                        <input type="text" placeholder={wish_t::SONG_NAME_PLACEHOLDER}
                            value={(*wish_form_song).clone()}
                            oninput={let s = wish_form_song.clone(); Callback::from(move |e: InputEvent| {
                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                s.set(input.value());
                            })}
                            class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                   text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]"
                            required=true />
                    </div>
                    <div>
                        <label class="block text-xs text-[var(--muted)] mb-1">{wish_t::ARTIST_LABEL}</label>
                        <input type="text" placeholder={wish_t::ARTIST_PLACEHOLDER}
                            value={(*wish_form_artist).clone()}
                            oninput={let s = wish_form_artist.clone(); Callback::from(move |e: InputEvent| {
                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                s.set(input.value());
                            })}
                            class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                   text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                    </div>
                    <div class="sm:col-span-2">
                        <label class="block text-xs text-[var(--muted)] mb-1">{wish_t::MESSAGE_LABEL}</label>
                        <textarea placeholder={wish_t::MESSAGE_PLACEHOLDER}
                            value={(*wish_form_message).clone()}
                            oninput={let s = wish_form_message.clone(); Callback::from(move |e: InputEvent| {
                                let input: web_sys::HtmlTextAreaElement = e.target_unchecked_into();
                                s.set(input.value());
                            })}
                            rows="3"
                            class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                   text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)] resize-none"
                            required=true />
                    </div>
                    <div>
                        <label class="block text-xs text-[var(--muted)] mb-1">{wish_t::NICKNAME_LABEL}</label>
                        <input type="text" placeholder={wish_t::NICKNAME_PLACEHOLDER}
                            value={(*wish_form_nickname).clone()}
                            oninput={let s = wish_form_nickname.clone(); Callback::from(move |e: InputEvent| {
                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                s.set(input.value());
                            })}
                            class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                   text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                    </div>
                    <div>
                        <label class="block text-xs text-[var(--muted)] mb-1">{wish_t::EMAIL_LABEL}</label>
                        <input type="email" placeholder={wish_t::EMAIL_PLACEHOLDER}
                            value={(*wish_form_email).clone()}
                            oninput={let s = wish_form_email.clone(); Callback::from(move |e: InputEvent| {
                                let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                s.set(input.value());
                            })}
                            class="w-full px-3 py-2 rounded-lg bg-[var(--surface-alt)] border border-[var(--border)] \
                                   text-[var(--text)] text-sm focus:outline-none focus:border-[var(--primary)]" />
                        <p class="mt-1 text-[11px] text-[var(--muted)]">{wish_t::EMAIL_HELP_TEXT}</p>
                    </div>
                    <div class="flex items-end">
                        <button type="submit" disabled={*wish_submitting}
                            class="px-5 py-2 rounded-lg bg-[var(--primary)] text-white text-sm font-medium \
                                   hover:opacity-90 transition-opacity disabled:opacity-50">
                            {if *wish_submitting { wish_t::SUBMITTING } else { wish_t::SUBMIT_BTN }}
                        </button>
                    </div>
                    if let Some(ref msg) = *wish_submit_msg {
                        <div class="sm:col-span-2 text-green-500 text-sm">{msg}</div>
                    }
                    if let Some(ref err) = *wish_submit_err {
                        <div class="sm:col-span-2 text-red-500 text-sm">{err}</div>
                    }
                </form>

                // Refresh button
                <div class="flex justify-end mb-4">
                    <button
                        onclick={on_refresh_wishes}
                        disabled={*wish_loading}
                        class="inline-flex items-center gap-1.5 px-4 py-2 rounded-lg \
                               border border-[var(--border)] bg-[var(--surface)] \
                               text-[var(--text)] text-sm font-medium \
                               hover:bg-[var(--surface-alt)] transition-colors \
                               disabled:opacity-50 disabled:cursor-not-allowed"
                    >
                        <svg class={if *wish_loading { "w-4 h-4 animate-spin" } else { "w-4 h-4" }}
                             xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24"
                             stroke-width="2" stroke="currentColor">
                            <path stroke-linecap="round" stroke-linejoin="round"
                                  d="M16.023 9.348h4.992v-.001M2.985 19.644v-4.992m0 0h4.992m-4.993 0 \
                                     3.181 3.183a8.25 8.25 0 0 0 13.803-3.7M4.031 9.865a8.25 8.25 0 0 1 \
                                     13.803-3.7l3.181 3.182" />
                        </svg>
                        {if *wish_loading { wish_t::REFRESHING } else { wish_t::REFRESH_BTN }}
                    </button>
                </div>

                if *wish_loading && wishes.is_empty() {
                    <div class="flex justify-center py-8">
                        <div class="animate-spin rounded-full h-6 w-6 border-b-2 border-[var(--primary)]" />
                    </div>
                } else if let Some(err) = (*wish_list_error).clone() {
                    <p class="text-center text-red-500 py-8">{format!("Failed to load wishes: {err}")}</p>
                } else if wishes.is_empty() {
                    <p class="text-center text-[var(--muted)] py-8">{wish_t::EMPTY_LIST}</p>
                } else {
                    <>
                        if *wish_loading {
                            <div class="mb-3 inline-flex items-center gap-2 text-xs text-[var(--muted)]">
                                <div class="animate-spin rounded-full h-4 w-4 border-b-2 border-[var(--primary)]" />
                                <span>{"Loading wishes..."}</span>
                            </div>
                        }
                        <div class="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
                            { for wishes.iter().map(render_wish_card) }
                        </div>
                    </>
                }
                if wish_total_pages > 1 {
                    <div class="flex justify-center mt-6">
                        <Pagination
                            current_page={*wish_page}
                            total_pages={wish_total_pages}
                            on_page_change={on_wish_page_change.clone()}
                        />
                    </div>
                }
            </div>
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
                        <ImageWithLoading
                            src={cover_url}
                            alt={song.title.clone()}
                            loading={Some(AttrValue::from("lazy"))}
                            referrerpolicy={Some(AttrValue::from("no-referrer"))}
                            class="w-full h-full object-cover transition-transform duration-500 ease-out group-hover:scale-105"
                            container_class={classes!("w-full", "h-full")}
                        />
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

fn render_wish_card(w: &api::MusicWishItem) -> Html {
    let status_class = match w.status.as_str() {
        "pending" => "bg-yellow-500/10 text-yellow-600 border-yellow-500/20",
        "approved" | "running" => "bg-blue-500/10 text-blue-600 border-blue-500/20",
        "done" => "bg-green-500/10 text-green-600 border-green-500/20",
        "failed" => "bg-red-500/10 text-red-600 border-red-500/20",
        _ => "bg-gray-500/10 text-gray-600 border-gray-500/20",
    };
    let status_text = match w.status.as_str() {
        "pending" => wish_t::STATUS_PENDING,
        "approved" => wish_t::STATUS_APPROVED,
        "running" => wish_t::STATUS_RUNNING,
        "done" => wish_t::STATUS_DONE,
        "failed" => wish_t::STATUS_FAILED,
        other => other,
    };

    let ts = format_ts_ms(w.created_at);

    html! {
        <div class="bg-[var(--surface)] liquid-glass border border-[var(--border)] rounded-xl p-4 \
                    flex flex-col gap-2">
            <div class="flex items-start justify-between gap-2">
                <h3 class="text-sm font-semibold text-[var(--text)] truncate">{&w.song_name}</h3>
                <span class={classes!("text-[10px]", "px-2", "py-0.5", "rounded-full", "border",
                    "whitespace-nowrap", "shrink-0", status_class)}>
                    {status_text}
                </span>
            </div>
            if let Some(ref artist) = w.artist_hint {
                <p class="text-xs text-[var(--muted)]">{format!("🎤 {}", artist)}</p>
            }
            <p class="text-xs text-[var(--text)] line-clamp-3">{&w.wish_message}</p>
            <div class="flex items-center justify-between text-[10px] text-[var(--muted)] mt-auto pt-1 \
                        border-t border-[var(--border)]">
                <span>{format!("{} · {}", w.nickname, w.ip_region)}</span>
                <span>{&ts}</span>
            </div>
            if w.status == "done" {
                if let Some(ref song_id) = w.ingested_song_id {
                    <Link<Route> to={Route::MusicPlayer { id: song_id.clone() }}
                        classes="text-xs text-[var(--primary)] hover:underline">
                        {wish_t::LISTEN_NOW}
                    </Link<Route>>
                }
            }
            if let Some(ref reply) = w.ai_reply {
                <div class="mt-2 p-2 bg-[var(--surface-alt)] rounded-lg border border-[var(--border)] \
                            text-xs text-[var(--text)] whitespace-pre-wrap">
                    <span class="text-[10px] text-[var(--muted)] block mb-1">{"🤖 AI"}</span>
                    {reply}
                </div>
            }
        </div>
    }
}

fn format_ts_ms(ms: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(ms as f64));
    let y = date.get_full_year();
    let m = date.get_month() + 1;
    let d = date.get_date();
    let h = date.get_hours();
    let min = date.get_minutes();
    format!("{y:04}-{m:02}-{d:02} {h:02}:{min:02}")
}
