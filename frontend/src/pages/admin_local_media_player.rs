use std::path::Path;

use gloo_timers::callback::Interval;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlElement;
use yew::prelude::*;
use yew_router::prelude::*;

use super::admin_local_media::AdminLocalMediaPlayerQuery;
use crate::{
    api::{
        fetch_admin_local_media_job_status, open_admin_local_media_playback,
        LocalMediaPlaybackMode, LocalMediaPlaybackOpenResponse, LocalMediaPlaybackStatus,
    },
    router::Route,
};

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = window, js_name = sfLocalMediaPlayerMount)]
    fn sf_local_media_player_mount(
        element: HtmlElement,
        url: &str,
        mode: &str,
        title: &str,
        storage_key: &str,
    );

    #[wasm_bindgen(js_namespace = window, js_name = sfLocalMediaPlayerUnmount)]
    fn sf_local_media_player_unmount(element: HtmlElement);
}

#[function_component(AdminLocalMediaPlayerPage)]
pub fn admin_local_media_player_page() -> Html {
    let navigator = use_navigator();
    let location = use_location();
    let file = location
        .as_ref()
        .and_then(|loc| loc.query::<AdminLocalMediaPlayerQuery>().ok())
        .and_then(|query| query.file)
        .unwrap_or_default();

    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let playback = use_state(|| None::<LocalMediaPlaybackOpenResponse>);
    let player_host = use_node_ref();

    {
        let loading = loading.clone();
        let error = error.clone();
        let playback = playback.clone();
        let file = file.clone();
        use_effect_with(file.clone(), move |file| {
            let has_file = !file.trim().is_empty();
            if !has_file {
                loading.set(false);
                error.set(Some("Missing file query".to_string()));
            } else {
                loading.set(true);
                error.set(None);
                let file = file.clone();
                spawn_local(async move {
                    match open_admin_local_media_playback(&file).await {
                        Ok(response) => playback.set(Some(response)),
                        Err(err) => error.set(Some(err)),
                    }
                    loading.set(false);
                });
            }
            || ()
        });
    }

    {
        let playback = playback.clone();
        let error = error.clone();
        use_effect_with((*playback).clone(), move |playback_state| {
            let interval = playback_state.clone().and_then(|playback_state| {
                if playback_state.status != LocalMediaPlaybackStatus::Preparing {
                    return None;
                }

                playback_state.job_id.clone().map(|job_id| {
                    Interval::new(1500, move || {
                        let playback = playback.clone();
                        let error = error.clone();
                        let job_id = job_id.clone();
                        spawn_local(async move {
                            match fetch_admin_local_media_job_status(&job_id).await {
                                Ok(job) => {
                                    let next = LocalMediaPlaybackOpenResponse {
                                        status: job.status,
                                        mode: job.mode,
                                        job_id: Some(job.job_id),
                                        player_url: job.player_url,
                                        title: playback
                                            .as_ref()
                                            .as_ref()
                                            .map(|value| value.title.clone())
                                            .unwrap_or_else(|| "Preparing".to_string()),
                                        error: job.error,
                                    };
                                    playback.set(Some(next));
                                },
                                Err(err) => error.set(Some(err)),
                            }
                        });
                    })
                })
            });
            move || drop(interval)
        });
    }

    {
        let player_host = player_host.clone();
        let file = file.clone();
        use_effect_with((*playback).clone(), move |playback_state| {
            let mounted = if let Some(playback_state) = playback_state.clone() {
                if playback_state.status == LocalMediaPlaybackStatus::Ready {
                    if let (Some(player_url), Some(mode), Some(element)) = (
                        playback_state.player_url.clone(),
                        playback_state.mode,
                        player_host.cast::<HtmlElement>(),
                    ) {
                        let storage_key = format!("sf-local-media-progress:{file}");
                        let mode_name = match mode {
                            LocalMediaPlaybackMode::Raw => "raw",
                            LocalMediaPlaybackMode::Hls => "hls",
                        };
                        sf_local_media_player_mount(
                            element.clone(),
                            &player_url,
                            mode_name,
                            &playback_state.title,
                            &storage_key,
                        );
                        Some(element)
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };
            move || {
                if let Some(element) = mounted {
                    sf_local_media_player_unmount(element);
                }
            }
        });
    }

    let back_to_browser = {
        let navigator = navigator.clone();
        let parent_dir = parent_dir(&file);
        Callback::from(move |_| {
            if let Some(nav) = navigator.clone() {
                let _ = nav.push_with_query(
                    &Route::AdminLocalMedia,
                    &super::admin_local_media::AdminLocalMediaQuery {
                        dir: parent_dir.clone(),
                    },
                );
            }
        })
    };

    let body = if *loading {
        html! {
            <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-6 text-sm text-[var(--muted)]">
                { "Opening player..." }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class="rounded-[var(--radius)] border border-red-400/40 bg-red-500/10 p-4 text-sm text-red-700 dark:text-red-200">
                { err }
            </div>
        }
    } else if let Some(playback) = (*playback).clone() {
        match playback.status {
            LocalMediaPlaybackStatus::Preparing => html! {
                <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-6">
                    <div class="text-base font-semibold text-[var(--text)]">{ playback.title }</div>
                    <p class="mt-2 text-sm text-[var(--muted)]">
                        { "The backend is preparing a mobile-friendly playback stream. This avoids buffering the whole file in memory and keeps MKV playback consistent." }
                    </p>
                    if let Some(job_id) = playback.job_id {
                        <div class="mt-3 text-xs text-[var(--muted)] break-all">{ format!("job: {job_id}") }</div>
                    }
                </div>
            },
            LocalMediaPlaybackStatus::Failed => html! {
                <div class="rounded-[var(--radius)] border border-red-400/40 bg-red-500/10 p-4 text-sm text-red-700 dark:text-red-200">
                    { playback.error.unwrap_or_else(|| "Playback preparation failed".to_string()) }
                </div>
            },
            LocalMediaPlaybackStatus::Ready => html! {
                <div class="space-y-4">
                    <div class="overflow-hidden rounded-[var(--radius)] border border-[var(--border)] bg-black shadow-[var(--shadow)]">
                        <div ref={player_host} class="aspect-video w-full"></div>
                    </div>
                    <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-4 text-sm text-[var(--muted)]">
                        { "Long press for 2x, swipe horizontally to seek, and double tap to play/pause on supported mobile browsers." }
                    </div>
                </div>
            },
        }
    } else {
        Html::default()
    };

    html! {
        <main class="container py-8">
            <section class="mb-5 rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-5 shadow-[var(--shadow)]">
                <div class="flex flex-wrap items-start justify-between gap-3">
                    <div>
                        <div class="text-sm text-[var(--muted)]">
                            <Link<Route> to={Route::Admin} classes={classes!("hover:text-[var(--text)]")}>{ "Admin" }</Link<Route>>
                            <span class="mx-2">{ "/" }</span>
                            <button type="button" class="bg-transparent hover:text-[var(--text)]" onclick={back_to_browser.clone()}>{ "Local Media" }</button>
                            <span class="mx-2">{ "/" }</span>
                            <span>{ "Player" }</span>
                        </div>
                        <h1 class="mt-2 text-xl font-semibold text-[var(--text)] break-all">{ file.clone() }</h1>
                        <p class="mt-1 text-sm text-[var(--muted)]">
                            { "This page stays focused on playback. The browser page remains lightweight, while heavy media work is delegated to the backend and disk cache." }
                        </p>
                    </div>
                    <div class="flex items-center gap-2">
                        <button type="button" class="btn-fluent-secondary" onclick={back_to_browser}>
                            <i class="fas fa-arrow-left mr-2" aria-hidden="true"></i>
                            { "Back To Folder" }
                        </button>
                    </div>
                </div>
            </section>
            { body }
        </main>
    }
}

fn parent_dir(file: &str) -> Option<String> {
    let path = Path::new(file);
    let parent = path.parent()?;
    let parts = parent
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}
