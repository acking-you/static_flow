use js_sys::Date;
use wasm_bindgen::{closure::Closure, JsCast};
use web_sys::{Event, EventSource, MessageEvent};
use yew::prelude::*;
use yew_router::prelude::Link;

use crate::{
    api::{
        build_admin_comment_ai_stream_url, fetch_admin_comment_task_ai_output,
        AdminCommentAiRunChunk, AdminCommentAiStreamEvent, AdminCommentTaskAiOutputResponse,
    },
    router::Route,
};

#[derive(Properties, Clone, PartialEq)]
pub struct AdminCommentRunsProps {
    pub task_id: String,
}

fn format_ms(ts_ms: i64) -> String {
    Date::new(&wasm_bindgen::JsValue::from_f64(ts_ms as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| ts_ms.to_string())
        .replace('T', " ")
        .trim_end_matches('Z')
        .to_string()
}

#[function_component(AdminCommentRunsPage)]
pub fn admin_comment_runs_page(props: &AdminCommentRunsProps) -> Html {
    let load_error = use_state(|| None::<String>);
    let loading = use_state(|| false);
    let output = use_state(|| None::<AdminCommentTaskAiOutputResponse>);
    let selected_run_id = use_state(|| None::<String>);
    let stream_chunks = use_state(Vec::<AdminCommentAiRunChunk>::new);
    let stream_status = use_state(|| "idle".to_string());
    let stream_error = use_state(|| None::<String>);
    let stream_ref = use_mut_ref(|| {
        None::<(
            EventSource,
            Closure<dyn FnMut(MessageEvent)>,
            Closure<dyn FnMut(Event)>,
        )>
    });

    let task_id = props.task_id.clone();

    {
        let task_id = task_id.clone();
        let load_error = load_error.clone();
        let loading = loading.clone();
        let output = output.clone();
        let selected_run_id = selected_run_id.clone();
        let stream_chunks = stream_chunks.clone();
        let stream_status = stream_status.clone();
        let stream_error = stream_error.clone();
        use_effect_with(task_id.clone(), move |id| {
            let id = id.clone();
            let load_error = load_error.clone();
            let loading = loading.clone();
            let output = output.clone();
            let selected_run_id = selected_run_id.clone();
            let stream_chunks = stream_chunks.clone();
            let stream_status = stream_status.clone();
            let stream_error = stream_error.clone();
            loading.set(true);
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_comment_task_ai_output(&id, None, Some(2000)).await {
                    Ok(data) => {
                        selected_run_id.set(data.selected_run_id.clone());
                        stream_chunks.set(data.chunks.clone());
                        stream_status.set(
                            data.runs
                                .iter()
                                .find(|item| Some(item.run_id.as_str()) == data.selected_run_id.as_deref())
                                .map(|item| item.status.clone())
                                .unwrap_or_else(|| "idle".to_string()),
                        );
                        stream_error.set(None);
                        output.set(Some(data));
                        load_error.set(None);
                    },
                    Err(err) => {
                        output.set(None);
                        stream_chunks.set(vec![]);
                        load_error.set(Some(format!("Failed to load AI runs: {}", err)));
                    },
                }
                loading.set(false);
            });
            || ()
        });
    }

    let on_select_run = {
        let task_id = task_id.clone();
        let output = output.clone();
        let selected_run_id = selected_run_id.clone();
        let stream_chunks = stream_chunks.clone();
        let stream_status = stream_status.clone();
        let stream_error = stream_error.clone();
        let load_error = load_error.clone();
        Callback::from(move |run_id: String| {
            let task_id = task_id.clone();
            let output = output.clone();
            let selected_run_id = selected_run_id.clone();
            let stream_chunks = stream_chunks.clone();
            let stream_status = stream_status.clone();
            let stream_error = stream_error.clone();
            let load_error = load_error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match fetch_admin_comment_task_ai_output(&task_id, Some(&run_id), Some(2000)).await {
                    Ok(data) => {
                        selected_run_id.set(Some(run_id.clone()));
                        stream_chunks.set(data.chunks.clone());
                        stream_status.set(
                            data.runs
                                .iter()
                                .find(|item| item.run_id == run_id)
                                .map(|item| item.status.clone())
                                .unwrap_or_else(|| "idle".to_string()),
                        );
                        stream_error.set(None);
                        output.set(Some(data));
                        load_error.set(None);
                    },
                    Err(err) => {
                        load_error.set(Some(format!("Failed to switch run: {}", err)));
                    },
                }
            });
        })
    };

    {
        let task_id = task_id.clone();
        let selected_run_id = selected_run_id.clone();
        let stream_chunks = stream_chunks.clone();
        let stream_status = stream_status.clone();
        let stream_error = stream_error.clone();
        let stream_ref = stream_ref.clone();
        use_effect_with((task_id.clone(), (*selected_run_id).clone()), move |(task_id, run_id)| {
            if let Some((source, _, _)) = stream_ref.borrow_mut().take() {
                source.close();
            }

            if let Some(run_id) = run_id.clone() {
                let stream_url = build_admin_comment_ai_stream_url(task_id, Some(&run_id), None);
                match EventSource::new(&stream_url) {
                    Ok(source) => {
                        stream_status.set("streaming".to_string());

                        let stream_chunks_setter = stream_chunks.clone();
                        let stream_status_setter = stream_status.clone();
                        let stream_error_setter = stream_error.clone();
                        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                            let Some(payload_text) = event.data().as_string() else {
                                return;
                            };
                            let parsed = serde_json::from_str::<AdminCommentAiStreamEvent>(&payload_text);
                            let Ok(payload) = parsed else {
                                return;
                            };
                            match payload.event_type.as_str() {
                                "chunk" => {
                                    if let Some(chunk) = payload.chunk {
                                        let mut next = (*stream_chunks_setter).clone();
                                        if !next.iter().any(|item| item.chunk_id == chunk.chunk_id) {
                                            next.push(chunk);
                                            next.sort_by(|left, right| left.batch_index.cmp(&right.batch_index));
                                            stream_chunks_setter.set(next);
                                        }
                                    }
                                },
                                "done" => {
                                    stream_status_setter.set(
                                        payload.run_status.unwrap_or_else(|| "done".to_string()),
                                    );
                                },
                                "error" => {
                                    stream_status_setter.set("error".to_string());
                                    stream_error_setter.set(Some("Stream returned error event".to_string()));
                                },
                                _ => {},
                            }
                        });
                        source.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));

                        let stream_status_setter = stream_status.clone();
                        let stream_error_setter = stream_error.clone();
                        let onerror = Closure::<dyn FnMut(Event)>::new(move |_| {
                            stream_status_setter.set("error".to_string());
                            stream_error_setter.set(Some("Stream connection closed".to_string()));
                        });
                        source.set_onerror(Some(onerror.as_ref().unchecked_ref()));

                        *stream_ref.borrow_mut() = Some((source, onmessage, onerror));
                    },
                    Err(err) => {
                        stream_status.set("error".to_string());
                        stream_error.set(Some(format!("Failed to open stream: {:?}", err)));
                    },
                }
            } else {
                stream_status.set("idle".to_string());
            }

            let stream_ref = stream_ref.clone();
            move || {
                if let Some((source, _, _)) = stream_ref.borrow_mut().take() {
                    source.close();
                }
            }
        });
    }

    let chunk_rows = {
        let mut rows = (*stream_chunks).clone();
        rows.sort_by(|left, right| left.batch_index.cmp(&right.batch_index));
        rows
    };

    html! {
        <main class={classes!("container", "py-8")}>
            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5",
                "mb-5"
            )}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <div>
                        <h1 class={classes!("m-0", "text-xl", "font-semibold")}>{ "Comment AI Stream" }</h1>
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>
                            { format!("task_id={}", task_id) }
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
            } else if let Some(data) = (*output).clone() {
                <section class={classes!(
                    "bg-[var(--surface)]",
                    "border",
                    "border-[var(--border)]",
                    "rounded-[var(--radius)]",
                    "shadow-[var(--shadow)]",
                    "p-5",
                    "mb-5"
                )}>
                    <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>{ "Runs" }</h2>
                    if data.runs.is_empty() {
                        <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "No runs." }</p>
                    } else {
                        <div class={classes!("flex", "gap-2", "flex-wrap")}>
                            { for data.runs.iter().map(|run| {
                                let run_id = run.run_id.clone();
                                let selected = Some(run_id.clone()) == *selected_run_id;
                                let click = {
                                    let on_select_run = on_select_run.clone();
                                    let run_id = run_id.clone();
                                    Callback::from(move |_| on_select_run.emit(run_id.clone()))
                                };
                                html! {
                                    <button
                                        class={if selected { classes!("btn-fluent-primary", "!px-2", "!py-1", "!text-xs") } else { classes!("btn-fluent-secondary", "!px-2", "!py-1", "!text-xs") }}
                                        onclick={click}
                                    >
                                        { format!("{} · {}", run.status, run.run_id) }
                                    </button>
                                }
                            }) }
                        </div>
                    }
                </section>
            } else {
                <section class={classes!("text-sm", "text-[var(--muted)]")}>{ "No data." }</section>
            }

            <section class={classes!(
                "bg-[var(--surface)]",
                "border",
                "border-[var(--border)]",
                "rounded-[var(--radius)]",
                "shadow-[var(--shadow)]",
                "p-5"
            )}>
                <h2 class={classes!("m-0", "mb-3", "text-lg", "font-semibold")}>
                    { format!("Stream Chunks ({})", chunk_rows.len()) }
                </h2>
                if chunk_rows.is_empty() {
                    <p class={classes!("m-0", "text-sm", "text-[var(--muted)]")}>{ "No chunks yet." }</p>
                } else {
                    <ul class={classes!("m-0", "p-0", "list-none", "flex", "flex-col", "gap-2")}>
                        { for chunk_rows.into_iter().map(|chunk| {
                            let stream_badge = if chunk.stream == "stderr" {
                                classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-red-500/15", "text-red-700", "dark:text-red-200")
                            } else {
                                classes!("inline-flex", "rounded-full", "px-2", "py-0.5", "text-xs", "font-semibold", "bg-sky-500/15", "text-sky-700", "dark:text-sky-200")
                            };
                            html! {
                                <li class={classes!("rounded-[var(--radius)]", "border", "border-[var(--border)]", "p-3")}>
                                    <div class={classes!("mb-2", "flex", "items-center", "gap-2", "flex-wrap")}>
                                        <span class={stream_badge}>{ chunk.stream.clone() }</span>
                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format!("batch={}", chunk.batch_index) }</span>
                                        <span class={classes!("text-xs", "text-[var(--muted)]")}>{ format_ms(chunk.created_at) }</span>
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
