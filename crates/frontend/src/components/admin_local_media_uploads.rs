use std::collections::HashMap;

use js_sys::Date;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{Event, File, HtmlInputElement};
use yew::prelude::*;

const CHUNK_BYTES: u64 = static_flow_media_types::LOCAL_MEDIA_UPLOAD_CHUNK_BYTES as u64;

#[derive(Clone, Copy, Debug, PartialEq)]
struct UploadTransferStats {
    last_uploaded_bytes: u64,
    last_observed_at_ms: f64,
    bytes_per_second: Option<f64>,
}

#[derive(Properties, PartialEq, Clone)]
pub struct AdminLocalMediaUploadsProps {
    pub current_dir: String,
    pub on_refresh_dir: Callback<()>,
}

#[function_component(AdminLocalMediaUploads)]
pub fn admin_local_media_uploads(props: &AdminLocalMediaUploadsProps) -> Html {
    let tasks = use_state(Vec::<static_flow_media_types::UploadTaskRecord>::new);
    let attached_files = use_state(HashMap::<String, File>::new);
    let transfer_stats = use_state(HashMap::<String, UploadTransferStats>::new);
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
        let transfer_stats = transfer_stats.clone();
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
            let transfer_stats = transfer_stats.clone();
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
                            begin_upload_transfer(&transfer_stats, &task);
                            active_task_id.set(Some(task.task_id.clone()));

                            // v1 intentionally uploads one file at a time from
                            // the browser. That keeps resume/cancel semantics
                            // simple and avoids multiple concurrent 8 MiB chunk
                            // streams against the same local media service.
                            let result = run_single_upload(
                                task.clone(),
                                file,
                                tasks.clone(),
                                transfer_stats.clone(),
                            )
                            .await;
                            active_task_id.set(None);
                            match result {
                                Ok(updated) => {
                                    clear_upload_transfer(&transfer_stats, &updated.task_id);
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
                                    clear_upload_transfer(&transfer_stats, &task.task_id);
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
        let transfer_stats = transfer_stats.clone();
        let error = error.clone();
        Callback::from(move |task_id: String| {
            let tasks = tasks.clone();
            let attached_files = attached_files.clone();
            let transfer_stats = transfer_stats.clone();
            let error = error.clone();
            spawn_local(async move {
                match crate::api::delete_admin_local_media_upload_task(&task_id).await {
                    Ok(task) => {
                        detach_file(&attached_files, &task.task_id);
                        clear_upload_transfer(&transfer_stats, &task.task_id);
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
                    let task_transfer_stats = transfer_stats.get(&task.task_id).copied();
                    render_upload_task_card(
                        task,
                        (*active_task_id).as_deref(),
                        has_local_file,
                        task_transfer_stats,
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
    tasks: UseStateHandle<Vec<static_flow_media_types::UploadTaskRecord>>,
    transfer_stats: UseStateHandle<HashMap<String, UploadTransferStats>>,
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
        apply_upload_chunk_success(&tasks, &transfer_stats, updated.clone(), Date::now());
        offset = updated.uploaded_bytes;
        if is_terminal_status(updated.status) {
            return Ok(updated);
        }
    }
    let latest = crate::api::fetch_admin_local_media_upload_task(&task.task_id).await?;
    apply_upload_chunk_success(&tasks, &transfer_stats, latest.clone(), Date::now());
    Ok(latest)
}

fn upsert_task(
    tasks: &UseStateHandle<Vec<static_flow_media_types::UploadTaskRecord>>,
    task: static_flow_media_types::UploadTaskRecord,
) {
    tasks.set(upserted_tasks(tasks, task));
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

fn begin_upload_transfer(
    transfer_stats: &UseStateHandle<HashMap<String, UploadTransferStats>>,
    task: &static_flow_media_types::UploadTaskRecord,
) {
    let mut next = (**transfer_stats).clone();
    next.insert(task.task_id.clone(), UploadTransferStats {
        last_uploaded_bytes: task.uploaded_bytes,
        last_observed_at_ms: Date::now(),
        bytes_per_second: None,
    });
    transfer_stats.set(next);
}

fn clear_upload_transfer(
    transfer_stats: &UseStateHandle<HashMap<String, UploadTransferStats>>,
    task_id: &str,
) {
    let mut next = (**transfer_stats).clone();
    next.remove(task_id);
    transfer_stats.set(next);
}

fn apply_upload_chunk_success(
    tasks: &UseStateHandle<Vec<static_flow_media_types::UploadTaskRecord>>,
    transfer_stats: &UseStateHandle<HashMap<String, UploadTransferStats>>,
    updated_task: static_flow_media_types::UploadTaskRecord,
    observed_at_ms: f64,
) {
    let (next_tasks, next_transfer_stats) =
        record_upload_chunk_success(tasks, transfer_stats, updated_task, observed_at_ms);
    tasks.set(next_tasks);
    transfer_stats.set(next_transfer_stats);
}

fn record_upload_chunk_success(
    tasks: &[static_flow_media_types::UploadTaskRecord],
    transfer_stats: &HashMap<String, UploadTransferStats>,
    updated_task: static_flow_media_types::UploadTaskRecord,
    observed_at_ms: f64,
) -> (Vec<static_flow_media_types::UploadTaskRecord>, HashMap<String, UploadTransferStats>) {
    let next_tasks = upserted_tasks(tasks, updated_task.clone());
    let mut next_transfer_stats = transfer_stats.clone();
    if is_terminal_status(updated_task.status) {
        next_transfer_stats.remove(&updated_task.task_id);
    } else {
        let next_stats = observe_upload_transfer(
            next_transfer_stats.get(&updated_task.task_id).copied(),
            updated_task.uploaded_bytes,
            observed_at_ms,
        );
        next_transfer_stats.insert(updated_task.task_id.clone(), next_stats);
    }
    (next_tasks, next_transfer_stats)
}

fn observe_upload_transfer(
    current: Option<UploadTransferStats>,
    uploaded_bytes: u64,
    observed_at_ms: f64,
) -> UploadTransferStats {
    let Some(current) = current else {
        return UploadTransferStats {
            last_uploaded_bytes: uploaded_bytes,
            last_observed_at_ms: observed_at_ms,
            bytes_per_second: None,
        };
    };

    let elapsed_ms = observed_at_ms - current.last_observed_at_ms;
    let delta_bytes = uploaded_bytes.saturating_sub(current.last_uploaded_bytes);
    let bytes_per_second = if elapsed_ms > 0.0 && delta_bytes > 0 {
        Some(delta_bytes as f64 / (elapsed_ms / 1000.0))
    } else {
        current.bytes_per_second
    };

    UploadTransferStats {
        last_uploaded_bytes: uploaded_bytes,
        last_observed_at_ms: observed_at_ms,
        bytes_per_second,
    }
}

fn upserted_tasks(
    tasks: &[static_flow_media_types::UploadTaskRecord],
    task: static_flow_media_types::UploadTaskRecord,
) -> Vec<static_flow_media_types::UploadTaskRecord> {
    let mut next = tasks.to_vec();
    next.retain(|row| row.task_id != task.task_id);
    next.push(task);
    next.sort_by(|left, right| right.updated_at_ms.cmp(&left.updated_at_ms));
    next
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

fn upload_speed_label(
    is_active: bool,
    transfer_stats: Option<UploadTransferStats>,
) -> Option<String> {
    if !is_active {
        return None;
    }
    match transfer_stats.and_then(|stats| stats.bytes_per_second) {
        Some(speed) if speed.is_finite() && speed > 0.0 => {
            Some(format!("Speed: {}", format_rate(speed)))
        },
        _ => Some("Speed: measuring...".to_string()),
    }
}

fn format_bytes(bytes: u64) -> String {
    format_byte_amount(bytes as f64)
}

fn format_rate(bytes_per_second: f64) -> String {
    format!("{}/s", format_byte_amount(bytes_per_second))
}

fn format_byte_amount(bytes: f64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes = bytes.max(0.0);
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes.round() as u64)
    }
}

fn render_upload_task_card(
    task: &static_flow_media_types::UploadTaskRecord,
    active_task_id: Option<&str>,
    has_local_file: bool,
    transfer_stats: Option<UploadTransferStats>,
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
                <div class="h-full bg-sky-500 transition-[width] duration-150" style={format!("width: {:.2}%;", progress)}></div>
            </div>
            <div class="mt-2 flex items-center justify-between gap-3 text-xs text-[var(--muted)]">
                <span>{ format!("{} / {} ({:.1}%)", format_bytes(task.uploaded_bytes), format_bytes(task.file_size), progress) }</span>
                <span>{ upload_status_label(task, is_active, has_local_file) }</span>
            </div>
            if let Some(speed) = upload_speed_label(is_active, transfer_stats) {
                <div class="mt-1 text-xs text-[var(--muted)]">{ speed }</div>
            }
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
    use std::collections::HashMap;

    use static_flow_media_types::{UploadTaskRecord, UploadTaskStatus};

    use super::{
        record_upload_chunk_success, upload_progress_percent, upload_speed_label,
        upload_status_label, UploadTransferStats,
    };

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

    #[test]
    fn record_upload_chunk_success_updates_progress_and_speed() {
        let mut existing = sample_task();
        existing.uploaded_bytes = 0;
        existing.updated_at_ms = 1;

        let mut updated = existing.clone();
        updated.uploaded_bytes = 4;
        updated.updated_at_ms = 2;

        let mut transfer_stats = HashMap::new();
        transfer_stats.insert(existing.task_id.clone(), UploadTransferStats {
            last_uploaded_bytes: 0,
            last_observed_at_ms: 1_000.0,
            bytes_per_second: None,
        });

        let (tasks, next_transfer_stats) =
            record_upload_chunk_success(&[existing], &transfer_stats, updated.clone(), 2_000.0);

        assert_eq!(tasks[0].uploaded_bytes, 4);
        assert_eq!(tasks[0].task_id, updated.task_id);
        assert_eq!(
            next_transfer_stats
                .get(&updated.task_id)
                .and_then(|stats| stats.bytes_per_second),
            Some(4.0)
        );
    }

    #[test]
    fn upload_speed_label_shows_measuring_before_first_chunk() {
        assert_eq!(
            upload_speed_label(
                true,
                Some(UploadTransferStats {
                    last_uploaded_bytes: 0,
                    last_observed_at_ms: 1_000.0,
                    bytes_per_second: None,
                }),
            ),
            Some("Speed: measuring...".to_string())
        );
    }
}
