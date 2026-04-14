# Admin Local Media Upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add resumable admin-only uploads to `/admin/local-media`, storing files into the currently browsed directory, persisting upload task state on disk, and allowing browser-restart recovery by re-selecting the same local file.

**Architecture:** `static-flow-media` becomes the upload source of truth with disk-backed task metadata plus staged partial bytes under the media root. `static-flow-backend` remains an authenticated reverse proxy, and the frontend becomes a task observer plus chunk sender rather than the owner of upload truth.

**Tech Stack:** Axum, Tokio async filesystem IO, reqwest proxying, Yew, gloo-net/web-sys fetch APIs, serde JSON, Rust integration/unit tests.

---

### Task 1: Define Shared Upload Protocol Types

**Files:**
- Modify: `media-types/src/lib.rs`
- Test: `media-types/src/lib.rs`

- [ ] **Step 1: Write the failing protocol-shape tests**

Add focused serde round-trip tests near the bottom of `media-types/src/lib.rs`:

```rust
#[cfg(test)]
mod upload_type_tests {
    use super::*;

    #[test]
    fn upload_task_status_uses_snake_case_wire_values() {
        assert_eq!(
            serde_json::to_string(&UploadTaskStatus::Partial).expect("serialize"),
            "\"partial\""
        );
    }

    #[test]
    fn create_or_resume_request_round_trips() {
        let request = CreateUploadTaskRequest {
            target_dir: "movies/demo".to_string(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 42,
            last_modified_ms: 1_713_000_000_000,
            mime_type: Some("video/mp4".to_string()),
        };
        let value = serde_json::to_value(&request).expect("serialize");
        let decoded: CreateUploadTaskRequest =
            serde_json::from_value(value).expect("deserialize");
        assert_eq!(decoded.target_dir, "movies/demo");
        assert_eq!(decoded.source_file_name, "clip.mp4");
        assert_eq!(decoded.file_size, 42);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-media-types upload_type_tests -- --nocapture`

Expected: FAIL because `UploadTaskStatus` and `CreateUploadTaskRequest` do not exist yet.

- [ ] **Step 3: Write minimal shared protocol types**

Add compact protocol types in `media-types/src/lib.rs`:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UploadTaskStatus {
    Created,
    Partial,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateUploadTaskRequest {
    pub target_dir: String,
    pub source_file_name: String,
    pub file_size: u64,
    pub last_modified_ms: i64,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadTaskRecord {
    pub task_id: String,
    pub resume_key: String,
    pub status: UploadTaskStatus,
    pub target_dir: String,
    pub source_file_name: String,
    pub target_file_name: String,
    pub target_relative_path: String,
    pub file_size: u64,
    pub uploaded_bytes: u64,
    pub last_modified_ms: i64,
    pub mime_type: Option<String>,
    pub error: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateUploadTaskResponse {
    pub task: UploadTaskRecord,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListUploadTasksQuery {
    #[serde(default)]
    pub dir: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ListUploadTasksResponse {
    pub tasks: Vec<UploadTaskRecord>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadChunkQuery {
    pub offset: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadChunkResponse {
    pub task: UploadTaskRecord,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p static-flow-media-types upload_type_tests -- --nocapture`

Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add media-types/src/lib.rs
git commit -m "feat: add local media upload protocol types"
```

### Task 2: Build Media-Service Upload Persistence Core

**Files:**
- Create: `media-service/src/upload.rs`
- Create: `media-service/src/upload_store.rs`
- Modify: `media-service/src/lib.rs`
- Modify: `media-service/src/state.rs`
- Modify: `media-service/src/config.rs`
- Test: `media-service/src/upload_store.rs`
- Test: `media-service/src/upload.rs`

- [ ] **Step 1: Write the failing media-service persistence tests**

Add tests that pin down create/resume, auto-rename, and chunk offset enforcement:

```rust
#[tokio::test]
async fn create_task_reuses_existing_partial_task() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state = LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());

    let first = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        },
    )
    .await
    .expect("first task");

    let second = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: Some("video/mp4".to_string()),
        },
    )
    .await
    .expect("second task");

    assert_eq!(first.task.task_id, second.task.task_id);
}

#[tokio::test]
async fn create_task_auto_renames_when_destination_exists() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    std::fs::write(root.path().join("clip.mp4"), b"existing").expect("existing file");
    let state = LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());

    let response = create_or_resume_upload_task(
        state,
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: None,
        },
    )
    .await
    .expect("task");

    assert_eq!(response.task.target_file_name, "clip (1).mp4");
}

#[tokio::test]
async fn append_chunk_rejects_wrong_offset() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let state = LocalMediaState::new_for_test(root.path().to_path_buf(), cache.path().to_path_buf());
    let task = create_or_resume_upload_task(
        state.clone(),
        CreateUploadTaskRequest {
            target_dir: String::new(),
            source_file_name: "clip.mp4".to_string(),
            file_size: 10,
            last_modified_ms: 123,
            mime_type: None,
        },
    )
    .await
    .expect("task");

    let err = append_upload_chunk(state, &task.task.task_id, 5, bytes::Bytes::from_static(b"abc"))
        .await
        .expect_err("offset mismatch must fail");

    assert!(err.to_string().contains("offset"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-media create_task_reuses_existing_partial_task -- --nocapture`

Expected: FAIL because upload modules and APIs do not exist yet.

- [ ] **Step 3: Add minimal state/config plumbing for upload storage**

Extend `media-service/src/config.rs` and `media-service/src/state.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalMediaConfig {
    // existing fields...
    pub upload_task_dir_name: String,
}

impl LocalMediaState {
    pub fn upload_metadata_root(&self) -> PathBuf {
        self.cache_dir.join("uploads").join("tasks")
    }

    pub fn upload_staging_root(&self) -> PathBuf {
        self.root_dir.join(&self.config.upload_task_dir_name)
    }
}
```

Initialize the directories during `LocalMediaState::from_env()` and
`LocalMediaState::new_for_test(...)`.

- [ ] **Step 4: Implement disk-backed upload store and service functions**

Create focused modules instead of bloating playback code:

```rust
// media-service/src/upload_store.rs
pub async fn load_task(task_root: &Path, task_id: &str) -> Result<Option<UploadTaskRecord>> { /* ... */ }
pub async fn save_task(task_root: &Path, task: &UploadTaskRecord) -> Result<()> { /* ... */ }
pub async fn list_tasks(task_root: &Path, dir: Option<&str>) -> Result<Vec<UploadTaskRecord>> { /* ... */ }

// media-service/src/upload.rs
pub async fn create_or_resume_upload_task(
    state: Arc<LocalMediaState>,
    request: CreateUploadTaskRequest,
) -> Result<CreateUploadTaskResponse> { /* ... */ }

pub async fn append_upload_chunk(
    state: Arc<LocalMediaState>,
    task_id: &str,
    offset: u64,
    body: bytes::Bytes,
) -> Result<UploadTaskRecord> { /* ... */ }
```

Implementation rules:

- normalize `target_dir` with existing path guard utilities
- derive `resume_key` from `target_dir`, `source_file_name`, `file_size`,
  `last_modified_ms`
- stage bytes under `<media_root>/.static-flow-uploads/<task_id>.part`
- enforce `offset == uploaded_bytes`
- update `uploaded_bytes` after fsync/flush succeeds
- when `uploaded_bytes == file_size`, rename the `.part` file into the target
  directory and mark `completed`

- [ ] **Step 5: Run test to verify it passes**

Run:

```bash
cargo test -p static-flow-media create_task_reuses_existing_partial_task -- --nocapture
cargo test -p static-flow-media create_task_auto_renames_when_destination_exists -- --nocapture
cargo test -p static-flow-media append_chunk_rejects_wrong_offset -- --nocapture
```

Expected: PASS

- [ ] **Step 6: Refactor for restart reconciliation**

Add one more test and then the minimal reconciliation logic:

```rust
#[tokio::test]
async fn load_task_reconciles_uploaded_bytes_from_part_file() {
    // write task.json with uploaded_bytes=2 but .part length=4
    // assert reloaded task reports uploaded_bytes=4
}
```

Implement reconciliation during task load:

```rust
if let Ok(metadata) = tokio::fs::metadata(&part_path).await {
    let actual = metadata.len();
    if actual != task.uploaded_bytes {
        task.uploaded_bytes = actual;
        task.status = if actual == 0 {
            UploadTaskStatus::Created
        } else if actual >= task.file_size {
            UploadTaskStatus::Completed
        } else {
            UploadTaskStatus::Partial
        };
        save_task(task_root, &task).await?;
    }
}
```

- [ ] **Step 7: Run the focused media-service upload tests**

Run: `cargo test -p static-flow-media upload -- --nocapture`

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add media-service/src/lib.rs media-service/src/config.rs media-service/src/state.rs media-service/src/upload.rs media-service/src/upload_store.rs
git commit -m "feat: add media service upload persistence"
```

### Task 3: Expose Upload APIs From Media-Service HTTP Layer

**Files:**
- Modify: `media-service/src/handlers.rs`
- Modify: `media-service/src/routes.rs`
- Test: `media-service/src/routes.rs`

- [ ] **Step 1: Write the failing route-registration and handler tests**

Add one route smoke test and one happy-path chunk append handler test:

```rust
#[tokio::test]
async fn media_router_registers_upload_routes() {
    let root = tempfile::tempdir().expect("root");
    let cache = tempfile::tempdir().expect("cache");
    let response = create_router(LocalMediaState::new_for_test(
        root.path().to_path_buf(),
        cache.path().to_path_buf(),
    ))
    .oneshot(
        Request::builder()
            .method("POST")
            .uri("/internal/local-media/uploads/tasks")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"target_dir":"","source_file_name":"clip.mp4","file_size":3,"last_modified_ms":1,"mime_type":null}"#))
            .expect("request"),
    )
    .await
    .expect("response");

    assert_ne!(response.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-media media_router_registers_upload_routes -- --nocapture`

Expected: FAIL because the routes are not registered yet.

- [ ] **Step 3: Add upload handlers**

Extend `media-service/src/handlers.rs`:

```rust
pub async fn create_upload_task(
    State(state): State<Arc<LocalMediaState>>,
    Json(request): Json<CreateUploadTaskRequest>,
) -> HandlerResult<Json<CreateUploadTaskResponse>> {
    let response = upload::create_or_resume_upload_task(state, request)
        .await
        .map_err(internal_error)?;
    Ok(Json(response))
}

pub async fn append_upload_chunk(
    State(state): State<Arc<LocalMediaState>>,
    Path(task_id): Path<String>,
    Query(query): Query<UploadChunkQuery>,
    body: axum::body::Bytes,
) -> HandlerResult<Json<UploadChunkResponse>> {
    let task = upload::append_upload_chunk(state, &task_id, query.offset, body)
        .await
        .map_err(internal_error)?;
    Ok(Json(UploadChunkResponse { task }))
}
```

Add matching list/get/delete handlers that call the upload module and return
typed JSON.

- [ ] **Step 4: Register the internal routes**

Extend `media-service/src/routes.rs`:

```rust
.route("/internal/local-media/uploads/tasks", post(handlers::create_upload_task).get(handlers::list_upload_tasks))
.route("/internal/local-media/uploads/tasks/:task_id", get(handlers::get_upload_task).delete(handlers::delete_upload_task))
.route("/internal/local-media/uploads/tasks/:task_id/chunks", put(handlers::append_upload_chunk))
```

- [ ] **Step 5: Run the focused handler tests**

Run:

```bash
cargo test -p static-flow-media media_router_registers_upload_routes -- --nocapture
cargo test -p static-flow-media create_task_reuses_existing_partial_task -- --nocapture
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add media-service/src/handlers.rs media-service/src/routes.rs
git commit -m "feat: expose local media upload APIs"
```

### Task 4: Add Backend Proxy Endpoints For Upload APIs

**Files:**
- Modify: `backend/src/media_proxy/handlers.rs`
- Modify: `backend/src/media_proxy/forward.rs`
- Modify: `backend/src/routes.rs`
- Test: `backend/src/media_proxy/forward.rs`
- Test: `backend/src/routes.rs`

- [ ] **Step 1: Write the failing backend proxy tests**

Add route and body-forwarding tests:

```rust
#[tokio::test]
async fn forward_upload_chunk_preserves_body_and_content_type() {
    let upstream = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("PUT"))
        .and(wiremock::matchers::path("/internal/local-media/uploads/tasks/task-1/chunks"))
        .and(wiremock::matchers::query_param("offset", "3"))
        .and(wiremock::matchers::header("content-type", "application/octet-stream"))
        .and(wiremock::matchers::body_bytes(b"abcd"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(
            r#"{"task":{"task_id":"task-1","resume_key":"k","status":"partial","target_dir":"","source_file_name":"clip.mp4","target_file_name":"clip.mp4","target_relative_path":"clip.mp4","file_size":10,"uploaded_bytes":7,"last_modified_ms":1,"mime_type":null,"error":null,"created_at_ms":1,"updated_at_ms":1}}"#,
            "application/json",
        ))
        .mount(&upstream)
        .await;

    // call forward helper and assert status/body
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-backend forward_upload_chunk_preserves_body_and_content_type -- --nocapture`

Expected: FAIL because no upload forward helper exists.

- [ ] **Step 3: Add raw-body forwarding helper**

Extend `backend/src/media_proxy/forward.rs` with a helper that forwards body and
content-type:

```rust
pub async fn forward_upload_chunk_request(
    client: &reqwest::Client,
    base_url: &reqwest::Url,
    task_id: &str,
    offset: u64,
    content_type: Option<&HeaderValue>,
    body: bytes::Bytes,
) -> Result<Response> {
    let relative = format!("internal/local-media/uploads/tasks/{task_id}/chunks");
    let mut upstream = client
        .put(join_internal_url(base_url, &relative)?)
        .query(&[("offset", offset)])
        .body(body);
    if let Some(value) = content_type.cloned() {
        upstream = upstream.header(header::CONTENT_TYPE, value);
    }
    forward(upstream).await
}
```

- [ ] **Step 4: Add proxy handlers and route wiring**

Extend `backend/src/media_proxy/handlers.rs` and `backend/src/routes.rs`:

```rust
.route("/admin/local-media/api/uploads/tasks", post(crate::media_proxy::handlers::create_upload_task).get(crate::media_proxy::handlers::list_upload_tasks))
.route("/admin/local-media/api/uploads/tasks/:task_id", get(crate::media_proxy::handlers::get_upload_task).delete(crate::media_proxy::handlers::delete_upload_task))
.route("/admin/local-media/api/uploads/tasks/:task_id/chunks", put(crate::media_proxy::handlers::append_upload_chunk))
```

Chunk handler shape:

```rust
pub async fn append_upload_chunk(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Query(query): Query<UploadChunkQuery>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> HandlerResult<Response> {
    ensure_admin_access(&state, &headers)?;
    let media_proxy = configured_media_proxy(&state)?;
    forward_upload_chunk_request(
        media_proxy.client(),
        &media_proxy.config().base_url,
        &task_id,
        query.offset,
        headers.get(header::CONTENT_TYPE),
        body,
    )
    .await
    .map_err(bad_gateway)
}
```

- [ ] **Step 5: Run the backend proxy tests**

Run:

```bash
cargo test -p static-flow-backend forward_upload_chunk_preserves_body_and_content_type -- --nocapture
cargo test -p static-flow-backend local_media -- --nocapture
```

Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add backend/src/media_proxy/handlers.rs backend/src/media_proxy/forward.rs backend/src/routes.rs
git commit -m "feat: proxy local media upload endpoints"
```

### Task 5: Add Frontend Upload APIs And Local Media Upload UI

**Files:**
- Create: `frontend/src/components/admin_local_media_uploads.rs`
- Modify: `frontend/src/components/mod.rs`
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/pages/admin_local_media.rs`
- Modify: `frontend/Cargo.toml`
- Test: `frontend/src/api.rs`

- [ ] **Step 1: Write the failing frontend API tests**

Add URL and normalization tests to `frontend/src/api.rs`:

```rust
#[test]
fn local_media_upload_task_url_uses_admin_prefix() {
    assert_eq!(
        build_admin_local_media_upload_tasks_url(),
        "/admin/local-media/api/uploads/tasks"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p static-flow-frontend local_media_upload_task_url_uses_admin_prefix -- --nocapture`

Expected: FAIL because upload API helpers do not exist.

- [ ] **Step 3: Add upload request/response API helpers**

Extend `frontend/src/api.rs`:

```rust
#[cfg(feature = "local-media")]
pub async fn create_admin_local_media_upload_task(
    request: &CreateUploadTaskRequest,
) -> Result<UploadTaskRecord, String> { /* POST /uploads/tasks */ }

#[cfg(feature = "local-media")]
pub async fn fetch_admin_local_media_upload_tasks(
    dir: Option<&str>,
    limit: Option<usize>,
    offset: Option<usize>,
) -> Result<ListUploadTasksResponse, String> { /* GET /uploads/tasks */ }

#[cfg(feature = "local-media")]
pub async fn append_admin_local_media_upload_chunk(
    task_id: &str,
    offset: u64,
    bytes: js_sys::Uint8Array,
) -> Result<UploadTaskRecord, String> { /* PUT raw bytes */ }
```

Use browser fetch directly for the chunk API so `application/octet-stream` body
is explicit and bounded.

- [ ] **Step 4: Create the upload component**

Create `frontend/src/components/admin_local_media_uploads.rs` with a focused
component API:

```rust
#[derive(Properties, PartialEq, Clone)]
pub struct AdminLocalMediaUploadsProps {
    pub current_dir: String,
    pub on_refresh_dir: Callback<()>,
}
```

Core behavior:

- file picker with `multiple=true`
- per-file `create_admin_local_media_upload_task(...)`
- keep `HashMap<String, web_sys::File>` for attached local files
- slice files into `8 * 1024 * 1024` byte chunks using `Blob::slice_with_f64_and_f64`
- send sequential chunks from `uploaded_bytes`
- poll task list for `current_dir`
- show durable status and progress bars

- [ ] **Step 5: Integrate the component into the page**

Update `frontend/src/pages/admin_local_media.rs` to render uploads above the
directory grid:

```rust
<crate::components::admin_local_media_uploads::AdminLocalMediaUploads
    current_dir={current_dir.clone()}
    on_refresh_dir={{
        let open_dir = open_dir.clone();
        let current_dir = current_dir.clone();
        Callback::from(move |_| open_dir.emit(current_dir.clone()))
    }}
/>
```

Also refresh the directory list after a task transitions to `completed`.

- [ ] **Step 6: Export the component and required web APIs**

Update:

```rust
// frontend/src/components/mod.rs
pub mod admin_local_media_uploads;
```

and extend `frontend/Cargo.toml` `web-sys` features with:

```toml
"File", "Blob", "FileList", "Request", "RequestInit", "RequestMode", "Response", "Headers"
```

- [ ] **Step 7: Run the focused frontend tests**

Run:

```bash
cargo test -p static-flow-frontend local_media_upload_task_url_uses_admin_prefix -- --nocapture
cargo test -p static-flow-frontend local_media -- --nocapture
```

Expected: PASS

- [ ] **Step 8: Commit**

```bash
git add frontend/src/api.rs frontend/src/components/mod.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/pages/admin_local_media.rs frontend/Cargo.toml
git commit -m "feat: add admin local media upload UI"
```

### Task 6: Full Verification And Cleanup

**Files:**
- Modify: any files touched above

- [ ] **Step 1: Format only the changed Rust files**

Run:

```bash
rustfmt media-types/src/lib.rs \
  media-service/src/config.rs media-service/src/handlers.rs media-service/src/lib.rs media-service/src/routes.rs media-service/src/state.rs media-service/src/upload.rs media-service/src/upload_store.rs \
  backend/src/media_proxy/forward.rs backend/src/media_proxy/handlers.rs backend/src/routes.rs \
  frontend/src/api.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/components/mod.rs frontend/src/pages/admin_local_media.rs
```

Expected: exit 0

- [ ] **Step 2: Run targeted tests for all affected crates**

Run:

```bash
cargo test -p static-flow-media-types
cargo test -p static-flow-media
cargo test -p static-flow-backend local_media -- --nocapture
cargo test -p static-flow-frontend local_media -- --nocapture
```

Expected: PASS in all affected crates

- [ ] **Step 3: Run clippy on affected crates and fix everything**

Run:

```bash
cargo clippy -p static-flow-media-types -p static-flow-media -p static-flow-backend -p static-flow-frontend --all-targets -- -D warnings
```

Expected: PASS with zero warnings

- [ ] **Step 4: Manual requirement check**

Verify against the spec:

- `/admin/local-media` uploads into the current directory
- repeated file selection resumes from persisted `uploaded_bytes`
- duplicate names auto-rename
- backend remains a proxy
- task metadata survives restart

- [ ] **Step 5: Final commit**

```bash
git add media-types/src/lib.rs media-service/src/config.rs media-service/src/handlers.rs media-service/src/lib.rs media-service/src/routes.rs media-service/src/state.rs media-service/src/upload.rs media-service/src/upload_store.rs backend/src/media_proxy/forward.rs backend/src/media_proxy/handlers.rs backend/src/routes.rs frontend/src/api.rs frontend/src/components/admin_local_media_uploads.rs frontend/src/components/mod.rs frontend/src/pages/admin_local_media.rs frontend/Cargo.toml
git commit -m "feat: add resumable admin local media uploads"
```
