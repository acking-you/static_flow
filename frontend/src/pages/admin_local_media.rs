use serde::{Deserialize, Serialize};
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;
use yew_router::prelude::*;

use crate::{
    api::{
        fetch_admin_local_media_list, LocalMediaEntry, LocalMediaEntryKind, LocalMediaListResponse,
    },
    router::Route,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AdminLocalMediaQuery {
    #[serde(default)]
    pub dir: Option<String>,
}

#[function_component(AdminLocalMediaPage)]
pub fn admin_local_media_page() -> Html {
    let navigator = use_navigator();
    let location = use_location();
    let current_dir = location
        .as_ref()
        .and_then(|loc| loc.query::<AdminLocalMediaQuery>().ok())
        .and_then(|query| query.dir)
        .unwrap_or_default();

    let data = use_state(|| None::<LocalMediaListResponse>);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let refresh_nonce = use_state(|| 0u64);

    {
        let data = data.clone();
        let loading = loading.clone();
        let error = error.clone();
        let current_dir = current_dir.clone();
        let refresh_key = *refresh_nonce;
        use_effect_with((current_dir.clone(), refresh_key), move |(dir, _)| {
            loading.set(true);
            error.set(None);
            let dir = dir.clone();
            spawn_local(async move {
                match fetch_admin_local_media_list(
                    if dir.is_empty() { None } else { Some(dir.as_str()) },
                    None,
                    None,
                )
                .await
                {
                    Ok(response) => data.set(Some(response)),
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            });
            || ()
        });
    }

    let open_dir = {
        let navigator = navigator.clone();
        Callback::from(move |dir: String| {
            if let Some(nav) = navigator.clone() {
                let _ = nav.push_with_query(&Route::AdminLocalMedia, &AdminLocalMediaQuery {
                    dir: if dir.is_empty() { None } else { Some(dir) },
                });
            }
        })
    };

    let open_player = {
        let navigator = navigator.clone();
        Callback::from(move |file: String| {
            if let Some(nav) = navigator.clone() {
                let _ = nav.push_with_query(
                    &Route::AdminLocalMediaPlayer,
                    &AdminLocalMediaPlayerQuery {
                        file: Some(file),
                    },
                );
            }
        })
    };

    let refresh_dir = {
        let refresh_nonce = refresh_nonce.clone();
        Callback::from(move |_| {
            refresh_nonce.set(*refresh_nonce + 1);
        })
    };

    let content = if *loading {
        html! {
            <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-6 text-sm text-[var(--muted)]">
                { "Loading local media..." }
            </div>
        }
    } else if let Some(err) = (*error).clone() {
        html! {
            <div class="rounded-[var(--radius)] border border-red-400/40 bg-red-500/10 p-4 text-sm text-red-700 dark:text-red-200">
                { err }
            </div>
        }
    } else if let Some(response) = (*data).clone() {
        if !response.configured {
            html! {
                <div class="rounded-[var(--radius)] border border-amber-400/40 bg-amber-500/10 p-4 text-sm text-amber-700 dark:text-amber-200">
                    { "Local media is enabled in this build, but the backend does not have STATICFLOW_LOCAL_MEDIA_ROOT configured." }
                </div>
            }
        } else if response.entries.is_empty() {
            html! {
                <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-6 text-sm text-[var(--muted)]">
                    { "This directory is empty." }
                </div>
            }
        } else {
            html! {
                <div class="grid grid-cols-1 gap-3 sm:grid-cols-2 xl:grid-cols-3">
                    { for response.entries.iter().map(|entry| {
                        html! {
                            <LocalMediaCard
                                entry={entry.clone()}
                                on_open_dir={open_dir.clone()}
                                on_open_player={open_player.clone()}
                            />
                        }
                    }) }
                </div>
            }
        }
    } else {
        Html::default()
    };

    let breadcrumb = render_breadcrumbs(&current_dir, open_dir.clone());

    html! {
        <main class="container py-8">
            <section class="mb-5 rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-5 shadow-[var(--shadow)]">
                <div class="flex flex-wrap items-start justify-between gap-3">
                    <div>
                        <div class="text-sm text-[var(--muted)]">
                            <Link<Route> to={Route::Admin} classes={classes!("hover:text-[var(--text)]")}>{ "Admin" }</Link<Route>>
                            <span class="mx-2">{ "/" }</span>
                            <span>{ "Local Media" }</span>
                        </div>
                        <h1 class="mt-2 text-xl font-semibold text-[var(--text)]">{ "Local Media Browser" }</h1>
                        <p class="mt-1 text-sm text-[var(--muted)]">
                            { "Browse the configured local media root. Folders stay lightweight; videos jump straight into the dedicated player page." }
                        </p>
                    </div>
                    <button
                        type="button"
                        class="btn-fluent-secondary"
                        onclick={refresh_dir.reform(|_| ())}
                    >
                        <i class="fas fa-rotate-right mr-2" aria-hidden="true"></i>
                        { "Refresh" }
                    </button>
                </div>
                <div class="mt-4 rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface-alt)] p-3">
                    { breadcrumb }
                </div>
            </section>
            <crate::components::admin_local_media_uploads::AdminLocalMediaUploads
                current_dir={current_dir.clone()}
                on_refresh_dir={refresh_dir}
            />
            { content }
        </main>
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AdminLocalMediaPlayerQuery {
    #[serde(default)]
    pub file: Option<String>,
}

#[derive(Properties, PartialEq, Clone)]
struct LocalMediaCardProps {
    entry: LocalMediaEntry,
    on_open_dir: Callback<String>,
    on_open_player: Callback<String>,
}

#[function_component(LocalMediaCard)]
fn local_media_card(props: &LocalMediaCardProps) -> Html {
    let poster_failed = use_state(|| false);
    let entry = props.entry.clone();
    let relative_path = entry.relative_path.clone();
    let click = match entry.kind {
        LocalMediaEntryKind::Directory => {
            let open_dir = props.on_open_dir.clone();
            Callback::from(move |_| open_dir.emit(relative_path.clone()))
        },
        LocalMediaEntryKind::Video => {
            let open_player = props.on_open_player.clone();
            Callback::from(move |_| open_player.emit(relative_path.clone()))
        },
    };
    let action_label = match entry.kind {
        LocalMediaEntryKind::Directory => "Open Folder",
        LocalMediaEntryKind::Video => "Open Player",
    };
    let icon_class = match entry.kind {
        LocalMediaEntryKind::Directory => "fa-folder-tree",
        LocalMediaEntryKind::Video => "fa-circle-play",
    };
    let show_poster =
        entry.kind == LocalMediaEntryKind::Video && entry.poster_url.is_some() && !*poster_failed;
    let poster_error = {
        let poster_failed = poster_failed.clone();
        Callback::from(move |_| poster_failed.set(true))
    };

    html! {
        <button
            type="button"
            onclick={click}
            class="w-full overflow-hidden rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] text-left shadow-[var(--shadow)] transition-transform duration-150 hover:-translate-y-0.5 hover:border-sky-500/40"
        >
            <div class="relative aspect-video w-full overflow-hidden bg-[var(--surface-alt)]">
                if show_poster {
                    <img
                        src={entry.poster_url.clone().unwrap_or_default()}
                        alt={format!("Poster for {}", entry.name)}
                        loading="lazy"
                        class="h-full w-full object-cover"
                        onerror={poster_error}
                    />
                    <div class="pointer-events-none absolute inset-0 bg-gradient-to-t from-black/55 via-black/5 to-transparent"></div>
                } else {
                    <div class="flex h-full w-full items-center justify-center bg-[radial-gradient(circle_at_top,_rgba(14,165,233,0.18),_transparent_55%),linear-gradient(135deg,rgba(15,23,42,0.12),rgba(15,23,42,0.02))] text-[var(--muted)]">
                        <i class={classes!("fas", icon_class, "text-3xl")} aria-hidden="true"></i>
                    </div>
                }
                <span class="absolute right-3 top-3 rounded-full bg-black/60 px-2.5 py-1 text-[10px] font-semibold uppercase tracking-[0.08em] text-white">
                    { action_label }
                </span>
            </div>
            <div class="p-4">
                <div class="flex items-start gap-2 text-sm font-semibold text-[var(--text)]">
                    <i class={classes!("fas", icon_class, "mt-0.5")} aria-hidden="true"></i>
                    <span class="line-clamp-2 break-all">{ entry.name.clone() }</span>
                </div>
                <div class="mt-2 text-xs text-[var(--muted)] break-all">
                    { entry.relative_path.clone() }
                </div>
                <div class="mt-3 flex items-center justify-between gap-3 text-xs text-[var(--muted)]">
                    <span>{ format_entry_meta(entry.size_bytes, entry.extension.as_deref()) }</span>
                    <span>{ format_modified(entry.modified_at_ms) }</span>
                </div>
            </div>
        </button>
    }
}

fn render_breadcrumbs(current_dir: &str, open_dir: Callback<String>) -> Html {
    let segments = current_dir
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        return html! { <span class="text-sm font-medium text-[var(--text)]">{ "/" }</span> };
    }

    let mut built = Vec::new();
    html! {
        <div class="flex flex-wrap items-center gap-2 text-sm">
            <button
                type="button"
                class="rounded-full bg-transparent px-2 py-1 text-[var(--muted)] transition-colors hover:bg-[var(--surface)] hover:text-[var(--text)]"
                onclick={{
                    let open_dir = open_dir.clone();
                    Callback::from(move |_| open_dir.emit(String::new()))
                }}
            >
                { "/" }
            </button>
            { for segments.iter().map(|segment| {
                built.push((*segment).to_string());
                let next_dir = built.join("/");
                let open_dir = open_dir.clone();
                html! {
                    <>
                        <span class="text-[var(--muted)]">{ "/" }</span>
                        <button
                            type="button"
                            class="rounded-full bg-transparent px-2 py-1 text-[var(--muted)] transition-colors hover:bg-[var(--surface)] hover:text-[var(--text)]"
                            onclick={Callback::from(move |_| open_dir.emit(next_dir.clone()))}
                        >
                            { (*segment).to_string() }
                        </button>
                    </>
                }
            }) }
        </div>
    }
}

fn format_entry_meta(size_bytes: Option<u64>, extension: Option<&str>) -> String {
    let size = size_bytes
        .map(format_bytes)
        .unwrap_or_else(|| "Folder".to_string());
    match extension {
        Some(extension) => format!("{}. {}", extension.to_ascii_uppercase(), size),
        None => size,
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}

fn format_modified(timestamp_ms: Option<i64>) -> String {
    timestamp_ms
        .map(|value| js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(value as f64)))
        .map(|date| {
            format!(
                "{:04}-{:02}-{:02}",
                date.get_full_year(),
                date.get_month() + 1,
                date.get_date()
            )
        })
        .unwrap_or_else(|| "Unknown".to_string())
}
