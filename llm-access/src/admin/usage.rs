//! Usage event/metric/filter endpoints, usage-journal status/preview
//! inspection, usage-worker progress, and usage query proxying with activity
//! overlay.

#[allow(unused_imports, reason = "submodule inherits parent facade imports via glob")]
use super::*;

pub(crate) async fn list_llm_gateway_usage_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_list_query(&state, &uri).await
}
pub(crate) async fn get_llm_gateway_usage_filter_options(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}
pub(crate) async fn get_llm_gateway_usage_metrics(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}
pub(crate) async fn get_llm_gateway_usage_event(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(event_id): Path<String>,
) -> Response {
    let _ = event_id;
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}
pub(crate) async fn get_usage_journal_status(
    State(state): State<HttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let mut journal = match producer_journal_status(&state) {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!("failed to load usage journal producer status: {err:#}");
            return internal_error("Failed to load usage journal status").into_response();
        },
    };
    journal.journal_enabled = config.usage_journal_enabled;
    if journal.journal_root.is_empty() {
        journal.journal_root = state
            .usage_journal_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
    let activity = state.request_activity.snapshot(None);
    let now = now_ms();
    let files = match journal_file_lists(&state) {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!("failed to load usage journal file lists: {err:#}");
            return internal_error("Failed to load usage journal file lists").into_response();
        },
    };
    let worker = usage_worker_status(&config.usage_query_base_url, now).await;
    let partitioned = partition_usage_journal_files(&journal, &files, &worker);
    let cluster = match state.cluster_state.as_ref() {
        Some(cluster_state) => {
            let snapshot = cluster_state.snapshot().await;
            Some(AdminClusterNodeStatusView {
                node_id: snapshot.node.node_id,
                node_class: snapshot.node.node_class,
                runtime_role: snapshot.runtime_role,
                primary_node_id: snapshot
                    .primary
                    .as_ref()
                    .map(|primary| primary.node_id.clone()),
                usage_query_mode: snapshot.usage_query_mode,
                primary_worker_base_url: snapshot
                    .primary
                    .as_ref()
                    .and_then(|primary| primary.worker_base_url.clone()),
            })
        },
        None => None,
    };
    Json(AdminUsageJournalStatusResponse {
        cluster,
        journal_enabled: journal.journal_enabled,
        journal_root: journal.journal_root,
        current_rpm: activity.rpm,
        current_in_flight: activity.in_flight,
        active_file_sequence: journal.active_file_sequence,
        active_file_bytes: journal.active_file_bytes,
        sealed_file_count: journal.sealed_file_count,
        sealed_bytes: journal.sealed_bytes,
        oldest_sealed_age_ms: journal.oldest_sealed_age_ms,
        dropped_files_total: journal.dropped_files_total,
        dropped_unconsumed_files_total: journal.dropped_unconsumed_files_total,
        write_failures_total: journal.write_failures_total,
        usage_query_base_url: config.usage_query_base_url,
        producer_current_file: partitioned.producer_current_file,
        orphan_active_files: partitioned.orphan_active_files,
        current_consuming_file: partitioned.current_consuming_file,
        orphan_consuming_files: partitioned.orphan_consuming_files,
        active_files: files.active.into_iter().map(journal_file_view).collect(),
        sealed_files: files.sealed.into_iter().map(journal_file_view).collect(),
        consuming_files: files.consuming.into_iter().map(journal_file_view).collect(),
        bad_files: files.bad.into_iter().map(journal_file_view).collect(),
        worker,
        generated_at: now,
    })
    .into_response()
}
pub(crate) async fn get_usage_journal_preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<AdminUsageJournalPreviewQuery>,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let mut journal = match producer_journal_status(&state) {
        Ok(status) => status,
        Err(err) => {
            tracing::warn!("failed to load usage journal producer status: {err:#}");
            return internal_error("Failed to load usage journal status").into_response();
        },
    };
    journal.journal_enabled = config.usage_journal_enabled;
    if journal.journal_root.is_empty() {
        journal.journal_root = state
            .usage_journal_dir
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
    }
    let files = match journal_file_lists(&state) {
        Ok(files) => files,
        Err(err) => {
            tracing::warn!("failed to load usage journal file lists: {err:#}");
            return internal_error("Failed to load usage journal file lists").into_response();
        },
    };
    let producer_current_file = producer_current_journal_file(&journal, &files.active);
    let limit = query.limit.unwrap_or(50).clamp(1, 200);
    let offset = query.offset.unwrap_or(0);
    let preview = if let Some(file) = producer_current_file.as_ref() {
        match JournalPreviewReader::open(FsPath::new(&file.path))
            .and_then(|reader| reader.read_recent_events_page(limit, offset))
        {
            Ok(report) => Some(admin_usage_journal_preview_view(report)),
            Err(err) => {
                tracing::warn!(
                    path = %file.path,
                    "failed to preview producer usage journal file: {err:#}"
                );
                return internal_error("Failed to preview usage journal producer file")
                    .into_response();
            },
        }
    } else {
        None
    };
    let total = preview.as_ref().map(|view| view.total_events).unwrap_or(0);
    let has_more = total > offset.saturating_add(limit);
    Json(AdminUsageJournalPreviewResponse {
        journal_root: journal.journal_root,
        producer_current_file,
        preview,
        limit,
        offset,
        total,
        has_more,
        generated_at: now_ms(),
    })
    .into_response()
}
pub(crate) async fn list_admin_kiro_usage_events(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
) -> Response {
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_list_query(&state, &uri).await
}
pub(crate) async fn get_admin_kiro_usage_event(
    State(state): State<HttpState>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    Path(event_id): Path<String>,
) -> Response {
    let _ = event_id;
    if let Err(response) = ensure_admin_access(&headers) {
        return response.into_response();
    }
    let _permit = match acquire_admin_usage_query_permit(&state) {
        Ok(permit) => permit,
        Err(response) => return response.into_response(),
    };
    proxy_usage_query(&state, &uri).await
}
pub(crate) fn acquire_admin_usage_query_permit(
    state: &HttpState,
) -> Result<OwnedSemaphorePermit, AdminHttpError> {
    std::sync::Arc::clone(&state.admin_usage_query_gate)
        .try_acquire_owned()
        .map_err(|_| too_many_requests("Another admin usage query is already running"))
}
pub(crate) fn producer_journal_status(state: &HttpState) -> anyhow::Result<JournalStatusSnapshot> {
    #[cfg(any(feature = "duckdb-runtime", feature = "duckdb-bundled"))]
    if let Some(sink) = &state.usage_journal_sink {
        return sink.status_snapshot();
    }
    inspect_journal_dir(state.usage_journal_dir.as_deref())
}
pub(crate) fn journal_file_lists(state: &HttpState) -> anyhow::Result<JournalFileListsSnapshot> {
    let Some(root) = state.usage_journal_dir.as_deref() else {
        return Ok(JournalFileListsSnapshot::default());
    };
    collect_journal_file_lists(root)
}
pub(crate) fn inspect_journal_dir(root: Option<&FsPath>) -> anyhow::Result<JournalStatusSnapshot> {
    let Some(root) = root else {
        return Ok(JournalStatusSnapshot::default());
    };
    let active = active_journal_stats(&root.join("active"))?;
    let sealed = sealed_journal_stats(&root.join("sealed"))?;
    Ok(JournalStatusSnapshot {
        journal_enabled: true,
        journal_root: root.display().to_string(),
        active_file_sequence: active.file_sequence,
        active_file_bytes: active.bytes,
        sealed_file_count: sealed.file_count,
        sealed_bytes: sealed.bytes,
        oldest_sealed_age_ms: sealed.oldest_age_ms,
        dropped_files_total: 0,
        dropped_unconsumed_files_total: 0,
        write_failures_total: 0,
    })
}
pub(crate) fn active_journal_stats(dir: &FsPath) -> anyhow::Result<ActiveJournalStats> {
    if !dir.exists() {
        return Ok(ActiveJournalStats::default());
    }
    let mut stats = ActiveJournalStats::default();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read active journal dir `{}`", dir.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        let Some(sequence) = journal_file_sequence(&entry.path()) else {
            continue;
        };
        if stats
            .file_sequence
            .is_none_or(|current| sequence >= current)
        {
            stats.file_sequence = Some(sequence);
            stats.bytes = metadata.len();
        }
    }
    Ok(stats)
}
pub(crate) fn sealed_journal_stats(dir: &FsPath) -> anyhow::Result<JournalDirStats> {
    if !dir.exists() {
        return Ok(JournalDirStats::default());
    }
    let mut stats = JournalDirStats::default();
    for entry in fs::read_dir(dir)
        .with_context(|| format!("failed to read sealed journal dir `{}`", dir.display()))?
    {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            continue;
        }
        stats.file_count = stats.file_count.saturating_add(1);
        stats.bytes = stats.bytes.saturating_add(metadata.len());
        if let Some(age_ms) = file_age_ms(&metadata) {
            stats.oldest_age_ms = Some(
                stats
                    .oldest_age_ms
                    .map_or(age_ms, |current| current.max(age_ms)),
            );
        }
    }
    Ok(stats)
}
pub(crate) fn journal_file_sequence(path: &FsPath) -> Option<u64> {
    let file_name = path.file_name()?.to_string_lossy();
    let suffix = file_name.strip_prefix("usage-")?;
    let digits = suffix
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}
pub(crate) fn journal_file_view(file: JournalFileSnapshot) -> AdminUsageJournalFileView {
    AdminUsageJournalFileView {
        file_name: file.file_name,
        path: file.path,
        sequence: file.sequence,
        bytes: file.bytes,
        age_ms: file.age_ms,
    }
}
pub(crate) fn admin_usage_journal_preview_view(
    report: JournalPreviewReport,
) -> AdminUsageJournalPreviewFileView {
    AdminUsageJournalPreviewFileView {
        path: report.path.display().to_string(),
        file_sequence: report.file_sequence,
        bytes_scanned: report.bytes_scanned,
        complete_blocks: report.complete_blocks,
        truncated_tail: report.truncated_tail,
        total_events: report.total_events,
        events: report
            .events
            .into_iter()
            .map(|event| AdminUsageJournalPreviewEventView {
                event_id: event.event_id,
                created_at_ms: event.created_at_ms,
                provider_type: event.provider_type,
                protocol_family: event.protocol_family,
                key_id: event.key_id,
                key_name: event.key_name,
                account_name: event.account_name,
                request_method: event.request_method,
                endpoint: event.endpoint,
                model: event.model,
                mapped_model: event.mapped_model,
                status_code: event.status_code,
                input_uncached_tokens: event.input_uncached_tokens,
                input_cached_tokens: event.input_cached_tokens,
                output_tokens: event.output_tokens,
                billable_tokens: event.billable_tokens,
                usage_missing: event.usage_missing,
                credit_usage_missing: event.credit_usage_missing,
                last_message_content: event.last_message_content,
                final_event_type: event.stream.final_event_type,
                stream_completed_cleanly: event.stream.stream_completed_cleanly,
                downstream_disconnect: event.stream.downstream_disconnect,
                bytes_streamed: event.stream.bytes_streamed,
                latency_ms: event.timing.latency_ms,
                first_sse_write_ms: event.timing.first_sse_write_ms,
            })
            .collect(),
    }
}
pub(crate) fn partition_usage_journal_files(
    journal: &JournalStatusSnapshot,
    files: &JournalFileListsSnapshot,
    worker: &AdminUsageWorkerProgressView,
) -> PartitionedUsageJournalFiles {
    let producer_current_file = producer_current_journal_file(journal, &files.active);
    let orphan_active_files = files
        .active
        .iter()
        .filter(|file| file.sequence != journal.active_file_sequence)
        .cloned()
        .map(journal_file_view)
        .collect();
    let current_consuming_file = worker_current_journal_file(worker, &files.consuming);
    let orphan_consuming_files = files
        .consuming
        .iter()
        .filter(|file| {
            !matches_worker_current_file(
                file,
                worker.current_file_sequence,
                worker.current_file_path.as_deref(),
            )
        })
        .cloned()
        .map(journal_file_view)
        .collect();
    PartitionedUsageJournalFiles {
        producer_current_file,
        orphan_active_files,
        current_consuming_file,
        orphan_consuming_files,
    }
}
pub(crate) fn producer_current_journal_file(
    journal: &JournalStatusSnapshot,
    active_files: &[JournalFileSnapshot],
) -> Option<AdminUsageJournalFileView> {
    let sequence = journal.active_file_sequence?;
    if let Some(file) = active_files
        .iter()
        .find(|file| file.sequence == Some(sequence))
    {
        return Some(journal_file_view(file.clone()));
    }
    Some(AdminUsageJournalFileView {
        file_name: format!("usage-{sequence:012}.open"),
        path: FsPath::new(&journal.journal_root)
            .join("active")
            .join(format!("usage-{sequence:012}.open"))
            .display()
            .to_string(),
        sequence: Some(sequence),
        bytes: journal.active_file_bytes,
        age_ms: None,
    })
}
pub(crate) fn worker_current_journal_file(
    worker: &AdminUsageWorkerProgressView,
    consuming_files: &[JournalFileSnapshot],
) -> Option<AdminUsageJournalFileView> {
    let matched = consuming_files
        .iter()
        .find(|file| {
            matches_worker_current_file(
                file,
                worker.current_file_sequence,
                worker.current_file_path.as_deref(),
            )
        })
        .cloned();
    match matched {
        Some(file) => Some(journal_file_view(file)),
        None if worker.current_file_sequence.is_some() || worker.current_file_path.is_some() => {
            let sequence = worker.current_file_sequence;
            let file_name = worker
                .current_file_path
                .as_deref()
                .and_then(|path| FsPath::new(path).file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| {
                    sequence
                        .map(|seq| format!("usage-{seq:012}.journal"))
                        .unwrap_or_else(|| "current-consuming-file".to_string())
                });
            Some(AdminUsageJournalFileView {
                file_name,
                path: worker.current_file_path.clone().unwrap_or_default(),
                sequence,
                bytes: worker.total_compressed_bytes,
                age_ms: None,
            })
        },
        None => None,
    }
}
pub(crate) fn matches_worker_current_file(
    file: &JournalFileSnapshot,
    current_sequence: Option<u64>,
    current_path: Option<&str>,
) -> bool {
    file.sequence == current_sequence || current_path.is_some_and(|path| file.path == path)
}
pub(crate) fn file_age_ms(metadata: &fs::Metadata) -> Option<i64> {
    let modified = metadata.modified().ok()?;
    let modified_ms = modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis()
        .min(i64::MAX as u128) as i64;
    Some(now_ms().saturating_sub(modified_ms))
}
pub(crate) async fn usage_worker_status(base_url: &str, now: i64) -> AdminUsageWorkerProgressView {
    let url = format!("{}/admin/llm-access/usage-worker/status", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(err) => return unreachable_worker_view(now, err.to_string()),
    };
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(err) => return unreachable_worker_view(now, err.to_string()),
    };
    if !response.status().is_success() {
        return unreachable_worker_view(now, format!("worker returned {}", response.status()));
    }
    match response.json::<WorkerStatusSnapshot>().await {
        Ok(status) => worker_progress_view(status.progress, status.process_memory, now),
        Err(err) => unreachable_worker_view(now, err.to_string()),
    }
}
pub(crate) fn worker_progress_view(
    progress: WorkerProgressSnapshot,
    process_memory: ProcessMemoryStats,
    now: i64,
) -> AdminUsageWorkerProgressView {
    AdminUsageWorkerProgressView {
        state: progress.state,
        current_file_path: progress.current_file_path,
        current_file_sequence: progress.current_file_sequence,
        processed_blocks: progress.processed_blocks,
        total_blocks: progress.total_blocks,
        processed_events: progress.processed_events,
        total_events: progress.total_events,
        processed_compressed_bytes: progress.processed_compressed_bytes,
        total_compressed_bytes: progress.total_compressed_bytes,
        progress_percent: progress.progress_percent,
        import_rate_events_per_second: progress.import_rate_events_per_second,
        heartbeat_age_ms: progress
            .heartbeat_at_ms
            .map(|heartbeat| now.saturating_sub(heartbeat)),
        last_successful_file_sequence: progress.last_successful_file_sequence,
        last_successful_import_at_ms: progress.last_successful_import_at_ms,
        last_error: progress.last_error,
        last_error_at_ms: progress.last_error_at_ms,
        process_memory,
    }
}
pub(crate) fn unreachable_worker_view(now: i64, error: String) -> AdminUsageWorkerProgressView {
    worker_progress_view(
        WorkerProgressSnapshot {
            state: "unreachable".to_string(),
            last_error: Some(error),
            last_error_at_ms: Some(now),
            ..WorkerProgressSnapshot::default()
        },
        ProcessMemoryStats::default(),
        now,
    )
}
pub(crate) async fn proxy_usage_list_query(state: &HttpState, uri: &Uri) -> Response {
    let activity_key_id = usage_activity_key_id_from_uri(uri);
    let activity = state.request_activity.snapshot(activity_key_id.as_deref());
    proxy_usage_query_with_activity(state, uri, Some(activity)).await
}
pub(crate) async fn proxy_usage_query(state: &HttpState, uri: &Uri) -> Response {
    proxy_usage_query_with_activity(state, uri, None).await
}
pub(crate) async fn proxy_usage_query_with_activity(
    state: &HttpState,
    uri: &Uri,
    activity: Option<RequestActivitySnapshot>,
) -> Response {
    let config = match state.admin_config_store.get_admin_runtime_config().await {
        Ok(config) => config,
        Err(_) => return internal_error("Failed to load llm gateway config").into_response(),
    };
    let base = config.usage_query_base_url.trim_end_matches('/');
    let path_and_query = uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(uri.path());
    let url = format!("{base}{path_and_query}");
    let response = match reqwest::Client::new().get(&url).send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(url = %url, "usage worker query proxy failed: {err:#}");
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "Usage worker is unavailable".to_string(),
                    code: StatusCode::SERVICE_UNAVAILABLE.as_u16(),
                }),
            )
                .into_response();
        },
    };
    let status = StatusCode::from_u16(response.status().as_u16())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = match response.bytes().await {
        Ok(bytes) => bytes,
        Err(err) => {
            tracing::warn!(url = %url, "failed to read usage worker response: {err:#}");
            return internal_error("Failed to read usage worker response").into_response();
        },
    };
    let body = match activity.filter(|_| status.is_success()) {
        Some(activity) => overlay_usage_activity_response_body(bytes.as_ref(), activity)
            .map(Body::from)
            .unwrap_or_else(|| Body::from(bytes)),
        None => Body::from(bytes),
    };
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder
        .body(body)
        .unwrap_or_else(|_| internal_error("Failed to build usage worker response").into_response())
}
pub(crate) fn usage_activity_key_id_from_uri(uri: &Uri) -> Option<String> {
    url::form_urlencoded::parse(uri.query()?.as_bytes()).find_map(|(name, value)| {
        (name == "key_id")
            .then(|| normalize_optional_string(&value))
            .flatten()
    })
}
pub(crate) fn overlay_usage_activity_response_body(
    body: &[u8],
    activity: RequestActivitySnapshot,
) -> Option<Vec<u8>> {
    let mut value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let object = value.as_object_mut()?;
    if !object.contains_key("events") || !object.contains_key("total") {
        return None;
    }
    object.insert("current_rpm".to_string(), serde_json::Value::from(activity.rpm));
    object.insert("current_in_flight".to_string(), serde_json::Value::from(activity.in_flight));
    serde_json::to_vec(&value).ok()
}
