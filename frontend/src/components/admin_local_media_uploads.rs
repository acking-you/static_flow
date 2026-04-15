use std::collections::HashMap;

use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Event, File, HtmlInputElement};
use yew::prelude::*;

const CHUNK_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Properties, PartialEq, Clone)]
pub struct AdminLocalMediaUploadsProps {
    pub current_dir: String,
    pub on_refresh_dir: Callback<()>,
}

#[function_component(AdminLocalMediaUploads)]
pub fn admin_local_media_uploads(props: &AdminLocalMediaUploadsProps) -> Html {
    let tasks = use_state(Vec::<static_flow_media_types::UploadTaskRecord>::new);
    let attached_files = use_state(HashMap::<String, File>::new);
    let active_task_id = use_state(|| None::<String>);
    let error = use_state(|| None::<String>);
    let busy = (*active_task_id).is_some();

    {
        let tasks = tasks.clone();
        let error = error.clone();
        use_effect_with(props.current_dir.clone(), move |dir| {
            let dir = dir.clone();
            error.set(None);
            spawn_local(async move {
                match crate::api::fetch_admin_local_media_upload_tasks(Some(dir.as_str())).await {
                    Ok(response) => tasks.set(response.tasks),
                    Err(err) => error.set(Some(err)),
                }
            });
            || ()
        });
    }

    let on_change = {
        let current_dir = props.current_dir.clone();
        let tasks = tasks.clone();
        let attached_files = attached_files.clone();
        let active_task_id = active_task_id.clone();
        let error = error.clone();
        let on_refresh_dir = props.on_refresh_dir.clone();
        Callback::from(move |event: Event| {
            if (*active_task_id).is_some() {
                return;
            }
            let Some(input) = event.target_dyn_into::<HtmlInputElement>() else {
                return;
            };
            let Some(files) = input.files() else {
                return;
            };
            let selected = (0..files.length())
                .filter_map(|index| files.get(index))
                .collect::<Vec<_>>();
            input.set_value("");
            if selected.is_empty() {
                return;
            }

            let current_dir = current_dir.clone();
            let tasks = tasks.clone();
            let attached_files = attached_files.clone();
            let active_task_id = active_task_id.clone();
            let error = error.clone();
            let on_refresh_dir = on_refresh_dir.clone();
            spawn_local(async move {
                error.set(None);
                for file in selected {
                    match crate::api::create_admin_local_media_upload_task(
                        &static_flow_media_types::CreateUploadTaskRequest {
                            target_dir: current_dir.clone(),
                            source_file_name: file.name(),
                            file_size: file.size() as u64,
                            last_modified_ms: file.last_modified() as i64,
                            mime_type: Some(file.type_()),
                        },
                    )
                    .await
                    {
                        Ok(task) => {
                            upsert_task(&tasks, task.clone());
                            attach_file(&attached_files, &task.task_id, file.clone());
                            active_task_id.set(Some(task.task_id.clone()));

                            // v1 intentionally uploads one file at a time from
                            // the browser. That keeps resume/cancel semantics
                            // simple and avoids multiple concurrent 8 MiB chunk
                            // streams against the same local media service.
                            let result = run_single_upload(task.clone(), file).await;
                            active_task_id.set(None);
                            match result {
                                Ok(updated) => {
                                    if is_terminal_status(updated.status) {
                                        detach_file(&attached_files, &updated.task_id);
                                    }
                                    upsert_task(&tasks, updated.clone());
                                    if matches!(
                                        updated.status,
                                        static_flow_media_types::UploadTaskStatus::Completed
                                    ) {
                                        on_refresh_dir.emit(());
                                    }
                                },
                                Err(err) => {
                                    error.set(Some(err));
                                    if let Ok(latest) =
                                        crate::api::fetch_admin_local_media_upload_task(
                                            &task.task_id,
                                        )
                                        .await
                                    {
                                        if is_terminal_status(latest.status) {
                                            detach_file(&attached_files, &latest.task_id);
                                        }
                                        upsert_task(&tasks, latest);
                                    }
                                },
                            }
                        },
                        Err(err) => error.set(Some(err)),
                    }
                }
            });
        })
    };

    let on_cancel = {
        let tasks = tasks.clone();
        let attached_files = attached_files.clone();
        let error = error.clone();
        Callback::from(move |task_id: String| {
            let tasks = tasks.clone();
            let attached_files = attached_files.clone();
            let error = error.clone();
            spawn_local(async move {
                match crate::api::delete_admin_local_media_upload_task(&task_id).await {
                    Ok(task) => {
                        detach_file(&attached_files, &task.task_id);
                        upsert_task(&tasks, task);
                    },
                    Err(err) => error.set(Some(err)),
                }
            });
        })
    };

    html! {
        <section class="mb-5 rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface)] p-5 shadow-[var(--shadow)]">
            <h2 class="m-0 text-lg font-semibold text-[var(--text)]">{ "Uploads" }</h2>
            <p class="mt-2 text-sm text-[var(--muted)]">
                { format!("Target directory: /{}", display_target_dir(&props.current_dir)) }
            </p>
            if let Some(err) = (*error).clone() {
                <div class="mt-3 rounded-[var(--radius)] border border-red-400/40 bg-red-500/10 p-3 text-sm text-red-700 dark:text-red-200">
                    { err }
                </div>
            }
            <input
                type="file"
                accept="video/*,.mkv,.mp4,.mov,.webm,.m4v,.avi,.mpeg,.mpg,.ts"
                multiple=true
                disabled={busy}
                class="mt-3 block w-full text-sm text-[var(--text)]"
                onchange={on_change}
            />
            <div class="mt-4 space-y-3">
                { for tasks.iter().map(|task| {
                    let has_local_file = attached_files.contains_key(&task.task_id);
                    render_upload_task_card(
                        task,
                        (*active_task_id).as_deref(),
                        has_local_file,
                        on_cancel.clone(),
                    )
                }) }
            </div>
        </section>
    }
}

async fn run_single_upload(
    task: static_flow_media_types::UploadTaskRecord,
    file: File,
) -> Result<static_flow_media_types::UploadTaskRecord, String> {
    let mut offset = task.uploaded_bytes;
    while offset < task.file_size {
        let end = (offset + CHUNK_BYTES).min(task.file_size);
        // Slice from the browser `File` on demand so resume always starts from
        // the uploaded byte count acknowledged by the service.
        let blob = file
            .slice_with_f64_and_f64(offset as f64, end as f64)
            .map_err(|err| format!("{err:?}"))?;
        let js_value = JsFuture::from(blob.array_buffer())
            .await
            .map_err(|err| format!("{err:?}"))?;
        let bytes = js_sys::Uint8Array::new(&js_value).to_vec();
        let updated =
            crate::api::append_admin_local_media_upload_chunk(&task.task_id, offset, bytes).await?;
        offset = updated.uploaded_bytes;
        if is_terminal_status(updated.status) {
            return Ok(updated);
        }
    }
    crate::api::fetch_admin_local_media_upload_task(&task.task_id).await
}

fn upsert_task(
    tasks: &UseStateHandle<Vec<static_flow_media_types::UploadTaskRecord>>,
    task: static_flow_media_types::UploadTaskRecord,
) {
    let mut next = (**tasks).clone();
    next.retain(|row| row.task_id != task.task_id);
    next.push(task);
    next.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    tasks.set(next);
}

fn attach_file(attached_files: &UseStateHandle<HashMap<String, File>>, task_id: &str, file: File) {
    let mut next = (**attached_files).clone();
    next.insert(task_id.to_string(), file);
    attached_files.set(next);
}

fn detach_file(attached_files: &UseStateHandle<HashMap<String, File>>, task_id: &str) {
    let mut next = (**attached_files).clone();
    next.remove(task_id);
    attached_files.set(next);
}

fn is_terminal_status(status: static_flow_media_types::UploadTaskStatus) -> bool {
    matches!(
        status,
        static_flow_media_types::UploadTaskStatus::Completed
            | static_flow_media_types::UploadTaskStatus::Failed
            | static_flow_media_types::UploadTaskStatus::Canceled
    )
}

fn display_target_dir(dir: &str) -> String {
    if dir.is_empty() {
        String::new()
    } else {
        dir.to_string()
    }
}

fn upload_progress_percent(task: &static_flow_media_types::UploadTaskRecord) -> f64 {
    if task.file_size == 0 {
        0.0
    } else {
        (task.uploaded_bytes as f64 / task.file_size as f64) * 100.0
    }
}

fn upload_status_label(
    task: &static_flow_media_types::UploadTaskRecord,
    is_active: bool,
    has_local_file: bool,
) -> String {
    if is_active {
        return "Sending".to_string();
    }
    if matches!(task.status, static_flow_media_types::UploadTaskStatus::Partial) && !has_local_file
    {
        return "Re-select the same file to resume".to_string();
    }
    format!("{:?}", task.status)
}

fn render_upload_task_card(
    task: &static_flow_media_types::UploadTaskRecord,
    active_task_id: Option<&str>,
    has_local_file: bool,
    on_cancel: Callback<String>,
) -> Html {
    let progress = upload_progress_percent(task);
    let is_active = active_task_id == Some(task.task_id.as_str());
    let cancel = {
        let task_id = task.task_id.clone();
        let on_cancel = on_cancel.clone();
        Callback::from(move |_| on_cancel.emit(task_id.clone()))
    };

    html! {
        <div class="rounded-[var(--radius)] border border-[var(--border)] bg-[var(--surface-alt)] p-3">
            <div class="text-sm font-semibold text-[var(--text)] break-all">{ task.target_relative_path.clone() }</div>
            <div class="mt-2 h-2 overflow-hidden rounded-full bg-[var(--surface)]">
                <div class="h-full bg-sky-500" style={format!("width: {:.2}%;", progress)}></div>
            </div>
            <div class="mt-2 flex items-center justify-between gap-3 text-xs text-[var(--muted)]">
                <span>{ format!("{} / {} bytes ({:.1}%)", task.uploaded_bytes, task.file_size, progress) }</span>
                <span>{ upload_status_label(task, is_active, has_local_file) }</span>
            </div>
            if let Some(err) = task.error.clone() {
                <div class="mt-2 text-xs text-red-700 dark:text-red-200">{ err }</div>
            }
            if !matches!(
                task.status,
                static_flow_media_types::UploadTaskStatus::Completed
                    | static_flow_media_types::UploadTaskStatus::Canceled
            ) {
                <button type="button" class="btn-fluent-secondary mt-3" onclick={cancel}>
                    { "Cancel" }
                </button>
            }
        </div>
    }
}

#[cfg(test)]
mod tests {
    use static_flow_media_types::{UploadTaskRecord, UploadTaskStatus};

    use super::{upload_progress_percent, upload_status_label};

    fn sample_task() -> UploadTaskRecord {
        UploadTaskRecord {
            task_id: "task-1".to_string(),
            resume_key: "resume".to_string(),
            status: UploadTaskStatus::Partial,
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            target_file_name: "clip.mp4".to_string(),
            target_relative_path: "clip.mp4".to_string(),
            file_size: 8,
            uploaded_bytes: 4,
            last_modified_ms: 1,
            mime_type: None,
            error: None,
            created_at_ms: 1,
            updated_at_ms: 1,
        }
    }

    #[test]
    fn upload_progress_percent_handles_zero_size() {
        let mut task = sample_task();
        task.file_size = 0;
        task.uploaded_bytes = 0;
        assert_eq!(upload_progress_percent(&task), 0.0);
    }

    #[test]
    fn partial_task_without_local_file_requests_resume_selection() {
        let task = sample_task();
        assert_eq!(upload_status_label(&task, false, false), "Re-select the same file to resume");
    }
}
