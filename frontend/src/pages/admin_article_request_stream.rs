use js_sys::Date;
use wasm_bindgen::{closure::Closure, JsCast, JsValue};
use web_sys::{Event, EventSource, MessageEvent};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        build_admin_article_request_ai_stream_url, fetch_admin_article_request_ai_output,
        ArticleRequestAiRunChunk, ArticleRequestAiRunRecord,
    },
    router::Route,
};

type NamedEventClosure = Closure<dyn FnMut(MessageEvent)>;
type ErrorClosure = Closure<dyn FnMut(Event)>;

struct StreamHandle {
    source: EventSource,
    _on_chunk: NamedEventClosure,
    _on_done: NamedEventClosure,
    _on_stream_error: NamedEventClosure,
    _on_error: ErrorClosure,
}

impl Drop for StreamHandle {
    fn drop(&mut self) {
        self.source.close();
    }
}

#[derive(Properties, Clone, PartialEq)]
pub struct Props {
    pub request_id: String,
}

fn format_ms(ts_ms: i64) -> String {
    Date::new(&JsValue::from_f64(ts_ms as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| ts_ms.to_string())
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string()
}

#[function_component(AdminArticleRequestRunsPage)]
pub fn admin_article_request_runs_page(props: &Props) -> Html {
    let load_error = use_state(|| None::<String>);
    let loading = use_state(|| false);
    let runs = use_state(Vec::<ArticleRequestAiRunRecord>::new);
    let stream_chunks = use_state(Vec::<ArticleRequestAiRunChunk>::new);
    let stream_status = use_state(|| "idle".to_string());
    let stream_error = use_state(|| None::<String>);
    let stream_ref = use_mut_ref(|| None::<StreamHandle>);

    let request_id = props.request_id.clone();

    // Load AI output on mount
    {
        let request_id = request_id.clone();
        let load_error = load_error.clone();
        let loading = loading.clone();
        let runs = runs.clone();
        let stream_chunks = stream_chunks.clone();
        let stream_status = stream_status.clone();
        use_effect_with(request_id.clone(), move |id| {
            let id = id.clone();
            let load_error = load_error;
            let loading = loading;
            let runs = runs;
            let stream_chunks = stream_chunks;
            let stream_status = stream_status;
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_article_request_ai_output(&id).await {
                    Ok(data) => {
                        if let Some(latest) = data.runs.last() {
                            stream_status.set(latest.status.clone());
                        }
                        stream_chunks.set(data.chunks);
                        runs.set(data.runs);
                        load_error.set(None);
                    },
                    Err(err) => {
                        load_error.set(Some(format!("Failed to load AI runs: {}", err)));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    // SSE connection for live streaming (named events: chunk, done, error)
    {
        let request_id = request_id.clone();
        let stream_chunks = stream_chunks.clone();
        let stream_status = stream_status.clone();
        let stream_error = stream_error.clone();
        let stream_ref = stream_ref.clone();
        use_effect_with(request_id.clone(), move |wid| {
            if let Some(handle) = stream_ref.borrow_mut().take() {
                drop(handle);
            }

            let stream_url = build_admin_article_request_ai_stream_url(wid);
            if let Ok(source) = EventSource::new(&stream_url) {
                stream_status.set("streaming".to_string());

                let on_chunk = {
                    let stream_chunks = stream_chunks.clone();
                    Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                        let Some(text) = event.data().as_string() else { return };
                        let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) else {
                            return;
                        };
                        let stream = val["stream"].as_str().unwrap_or("stdout").to_string();
                        let batch_index = val["batch_index"].as_i64().unwrap_or(0) as i32;
                        let content = val["content"].as_str().unwrap_or("").to_string();
                        let mut next = (*stream_chunks).clone();
                        if !next
                            .iter()
                            .any(|c| c.batch_index == batch_index && c.stream == stream)
                        {
                            next.push(ArticleRequestAiRunChunk {
                                chunk_id: format!("live-{}-{}", stream, batch_index),
                                run_id: String::new(),
                                request_id: String::new(),
                                stream,
                                batch_index,
                                content,
                                created_at: 0,
                            });
                            next.sort_by_key(|c| c.batch_index);
                            stream_chunks.set(next);
                        }
                    })
                };

                let on_done = {
                    let stream_status = stream_status.clone();
                    Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                        let status = event
                            .data()
                            .as_string()
                            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                            .and_then(|v| v["status"].as_str().map(String::from))
                            .unwrap_or_else(|| "done".to_string());
                        stream_status.set(status);
                    })
                };

                let on_stream_error = {
                    let stream_status = stream_status.clone();
                    let stream_error = stream_error.clone();
                    Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                        let reason = event
                            .data()
                            .as_string()
                            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                            .and_then(|v| v["failure_reason"].as_str().map(String::from))
                            .unwrap_or_else(|| "Unknown error".to_string());
                        stream_status.set("error".to_string());
                        stream_error.set(Some(reason));
                    })
                };

                let on_error = {
                    let stream_status = stream_status.clone();
                    let stream_error = stream_error.clone();
                    let source_ref = source.clone();
                    Closure::<dyn FnMut(Event)>::new(move |_| {
                        // Guard: ignore onerror if we already reached a terminal state
                        let current = (*stream_status).clone();
                        if current == "success" || current == "done" || current == "error" {
                            return;
                        }
                        // readyState: 0=CONNECTING (browser auto-reconnecting), 2=CLOSED
                        if source_ref.ready_state() == 2 {
                            stream_status.set("disconnected".to_string());
                            stream_error.set(Some("Stream connection closed".to_string()));
                        }
                        // CONNECTING (0) → browser is retrying, don't overwrite
                        // status
                    })
                };

                let _ = source
                    .add_event_listener_with_callback("chunk", on_chunk.as_ref().unchecked_ref());
                let _ = source
                    .add_event_listener_with_callback("done", on_done.as_ref().unchecked_ref());
                let _ = source.add_event_listener_with_callback(
                    "error",
                    on_stream_error.as_ref().unchecked_ref(),
                );
                source.set_onerror(Some(on_error.as_ref().unchecked_ref()));

                *stream_ref.borrow_mut() = Some(StreamHandle {
                    source,
                    _on_chunk: on_chunk,
                    _on_done: on_done,
                    _on_stream_error: on_stream_error,
                    _on_error: on_error,
                });
            } else {
                stream_error.set(Some("Failed to open EventSource".to_string()));
            }

            let stream_ref = stream_ref.clone();
            move || {
                stream_ref.borrow_mut().take();
            }
        });
    }

    let chunk_rows = {
        let mut rows = (*stream_chunks).clone();
        rows.sort_by_key(|c| c.batch_index);
        rows
    };

    html! {
        <main class={classes!("container", "py-8")}>
            <section class={classes!(
                "bg-[var(--surface)]", "border", "border-[var(--border)]",
                "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "mb-5"
            )}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold")}>{ "Article Request AI Stream" }</h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { format!("request_id={}", request_id) }
                        </p>
                    </div>
                    <Link<Route> to={Route::Admin} classes={classes!("btn-fluent-secondary")}>
                        { "Back to Admin" }
                    </Link<Route>>
                </div>
                if let Some(err) = (*load_error).clone() {
                    <p class={classes!("mt-3", "m-0", "text-sm", "text-red-700", "dark:text-red-200")}>{ err }</p>
                }
                if let Some(err) = (*stream_error).clone() {
                    <p class={classes!("mt-2", "m-0", "text-sm", "text-red-700", "dark:text-red-200")}>{ err }</p>
                }
                <p class={classes!("mt-2", "m-0", "text-sm", "text-[var(--muted)]")}>
                    { format!("stream_status={}", *stream_status) }
                </p>
            </section>

            if *loading {
                <section class={classes!("text-sm", "text-[var(--muted)]")}>{ "Loading..." }</section>
            } else if !runs.is_empty() {
                <section class={classes!(
                    "bg-[var(--surface)]", "border", "border-[var(--border)]",
                    "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5", "mb-5"
                )}>
                    <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>
                        { format!("Runs ({})", runs.len()) }
                    </h2>
                    <div class={classes!("flex", "gap-2", "flex-wrap")}>
                        { for (*runs).iter().map(|run| {
                            html! {
                                <span class={classes!(
                                    "inline-flex", "items-center", "rounded-full",
                                    "px-2", "py-0.5", "text-xs", "font-semibold",
                                    "bg-[var(--surface-alt)]", "text-[var(--muted)]"
                                )}>
                                    { format!("{} · {}", run.status, &run.run_id[..8.min(run.run_id.len())]) }
                                </span>
                            }
                        }) }
                    </div>
                </section>
            }

            <section class={classes!(
                "bg-[var(--surface)]", "border", "border-[var(--border)]",
                "rounded-[var(--radius)]", "shadow-[var(--shadow)]", "p-5"
            )}>
                <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>
                    { format!("Stream Chunks ({})", chunk_rows.len()) }
                </h2>
                if chunk_rows.is_empty() {
                    <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "No chunks yet." }</p>
                } else {
                    <ul class={classes!("m-0", "p-0", "list-none", "flex", "flex-col", "gap-2")}>
                        { for chunk_rows.into_iter().map(|chunk| {
                            let badge = if chunk.stream == "stderr" {
                                classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-red-500/15", "text-red-700", "dark:text-red-200")
                            } else {
                                classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-sky-500/15", "text-sky-700", "dark:text-sky-200")
                            };
                            html! {
                                <li class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <div class={classes!("mb-2", "flex", "items-center", "gap-2", "flex-wrap")}>
                                        <span class={badge}>{ chunk.stream.clone() }</span>
                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("batch={}", chunk.batch_index) }</span>
                                        if chunk.created_at > 0 {
                                            <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format_ms(chunk.created_at) }</span>
                                        }
                                    </div>
                                    <pre class={classes!("m-0", "text-xs", "whitespace-pre-wrap", "break-words", "font-mono")}>
                                        { chunk.content.clone() }
                                    </pre>
                                </li>
                            }
                        }) }
                    </ul>
                }
            </section>
        </main>
    }
}
