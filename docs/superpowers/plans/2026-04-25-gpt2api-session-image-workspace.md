# GPT2API Session Image Workspace Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a ChatGPT-style GPT2API workspace where sessions, messages, image tasks, artifacts, queue progress, admin controls, and optional email notifications are persisted by `gpt2api-rs`.

**Architecture:** `gpt2api-rs` owns product state in SQLite plus filesystem artifacts under its storage root. StaticFlow only serves the standalone `/gpt2api/*` frontend and proxies `/api/gpt2api/*`; compatibility APIs keep their public response shapes while writing through the same session/task storage path.

**Tech Stack:** Rust, Axum, Tokio, Rusqlite, DuckDB, Serde, Tower tests, Vite, TypeScript, React, Playwright smoke checks.

---

## File Map

- Modify: `deps/gpt2api_rs/Cargo.toml`
- Modify: `deps/gpt2api_rs/src/app.rs`
- Modify: `deps/gpt2api_rs/src/config.rs`
- Modify: `deps/gpt2api_rs/src/http/mod.rs`
- Modify: `deps/gpt2api_rs/src/http/admin_api.rs`
- Modify: `deps/gpt2api_rs/src/http/public_api.rs`
- Create: `deps/gpt2api_rs/src/http/product_api.rs`
- Modify: `deps/gpt2api_rs/src/lib.rs`
- Modify: `deps/gpt2api_rs/src/main.rs`
- Modify: `deps/gpt2api_rs/src/models.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Create: `deps/gpt2api_rs/src/tasks.rs`
- Create: `deps/gpt2api_rs/src/notifications.rs`
- Create: `deps/gpt2api_rs/src/storage/artifacts.rs`
- Modify: `deps/gpt2api_rs/src/storage/control.rs`
- Modify: `deps/gpt2api_rs/src/storage/migrations.rs`
- Modify: `deps/gpt2api_rs/src/storage/mod.rs`
- Modify: `deps/gpt2api_rs/tests/storage_bootstrap.rs`
- Create: `deps/gpt2api_rs/tests/product_storage.rs`
- Create: `deps/gpt2api_rs/tests/product_api.rs`
- Create: `deps/gpt2api_rs/tests/image_task_runner.rs`
- Create: `deps/gpt2api_rs/tests/notifications.rs`
- Modify: `deps/gpt2api_rs/tests/public_api.rs`
- Create: `frontend/gpt2api-app/package.json`
- Create: `frontend/gpt2api-app/package-lock.json`
- Create: `frontend/gpt2api-app/index.html`
- Create: `frontend/gpt2api-app/tsconfig.json`
- Create: `frontend/gpt2api-app/vite.config.ts`
- Create: `frontend/gpt2api-app/src/api.ts`
- Create: `frontend/gpt2api-app/src/App.tsx`
- Create: `frontend/gpt2api-app/src/main.tsx`
- Create: `frontend/gpt2api-app/src/styles.css`
- Create: `frontend/gpt2api-app/src/types.ts`
- Create: `frontend/gpt2api-app/src/components/Composer.tsx`
- Create: `frontend/gpt2api-app/src/components/PendingImageCard.tsx`
- Create: `frontend/gpt2api-app/src/components/SessionSidebar.tsx`
- Create: `frontend/gpt2api-app/src/components/AdminPanel.tsx`
- Modify: `frontend/.gitignore`
- Modify: `scripts/build_frontend_selfhosted.sh`
- Create: `scripts/build_gpt2api_frontend.sh`
- Modify: `backend/src/routes.rs`
- Modify: `backend/src/handlers.rs`

## Implementation Notes

- Treat `deps/gpt2api_rs` as a submodule. Commit inside that repository first, then update the parent submodule pointer.
- Keep service-admin token auth for existing operational endpoints such as account import. Add API-key admin role support only where product admins need it.
- Do not expose `secret_plaintext` to product-admin API responses. Existing service-admin responses keep their current behavior.
- The image queue unit is one `image_tasks` row. Synchronous OpenAI-compatible endpoints submit a task and wait for completion; web endpoints return immediately with task state.
- Store generated images as files. SQLite stores metadata and enforces ownership. No route serves the artifact directory directly.
- Email is disabled unless SMTP env config is complete and the key enables notification with a syntactically valid email.

### Task 1: Product Schema And Domain Models

**Files:**
- Modify: `deps/gpt2api_rs/src/models.rs`
- Modify: `deps/gpt2api_rs/src/storage/migrations.rs`
- Modify: `deps/gpt2api_rs/src/storage/control.rs`
- Modify: `deps/gpt2api_rs/src/storage/mod.rs`
- Modify: `deps/gpt2api_rs/tests/storage_bootstrap.rs`
- Create: `deps/gpt2api_rs/tests/product_storage.rs`

- [ ] **Step 1: Add failing storage tests**

Create `deps/gpt2api_rs/tests/product_storage.rs` with these tests:

```rust
//! Product storage tests for sessions, image tasks, artifacts, and signed links.

use gpt2api_rs::{
    config::ResolvedPaths,
    models::{ImageTaskStatus, MessageStatus, SessionSource},
    storage::Storage,
};
use tempfile::tempdir;

#[tokio::test]
async fn bootstrap_adds_product_tables_and_defaults() {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");

    let tables = storage.control.list_table_names().await.expect("table names");
    for expected in [
        "api_keys",
        "sessions",
        "messages",
        "image_tasks",
        "task_events",
        "image_artifacts",
        "signed_links",
        "runtime_config",
    ] {
        assert!(tables.iter().any(|name| name == expected), "missing table {expected}");
    }

    let key = storage.control.get_api_key("default").await.expect("key read").expect("default key");
    assert_eq!(key.role.as_str(), "user");
    assert_eq!(key.notification_email, None);
    assert!(!key.notification_enabled);

    let config = storage.control.get_runtime_config().await.expect("runtime config");
    assert_eq!(config.global_image_concurrency, 1);
    assert_eq!(config.signed_link_ttl_seconds, 604_800);
    assert_eq!(config.queue_eta_window_size, 20);
}

#[tokio::test]
async fn session_and_message_crud_is_key_scoped() {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");

    let session = storage
        .control
        .create_session("key-a", "First image", SessionSource::Web)
        .await
        .expect("session created");
    assert_eq!(session.key_id, "key-a");

    let user_message = storage
        .control
        .append_message(
            &session.id,
            "key-a",
            "user",
            serde_json::json!({"type":"text","text":"draw a lake"}),
            MessageStatus::Done,
        )
        .await
        .expect("message created");
    assert_eq!(user_message.session_id, session.id);

    assert!(storage
        .control
        .get_session_for_key(&session.id, "key-a")
        .await
        .expect("own session lookup")
        .is_some());
    assert!(storage
        .control
        .get_session_for_key(&session.id, "key-b")
        .await
        .expect("other session lookup")
        .is_none());
}

#[tokio::test]
async fn signed_link_hash_validation_rejects_expired_and_revoked_links() {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");

    let link = storage
        .control
        .create_signed_link("image_task", "task-1", 100, 10)
        .await
        .expect("link created");
    assert!(storage
        .control
        .resolve_signed_link(&link.plaintext_token, 50)
        .await
        .expect("valid link lookup")
        .is_some());
    assert!(storage
        .control
        .resolve_signed_link(&link.plaintext_token, 200)
        .await
        .expect("expired link lookup")
        .is_none());

    storage.control.revoke_signed_link(&link.id, 60).await.expect("revoke link");
    assert!(storage
        .control
        .resolve_signed_link(&link.plaintext_token, 61)
        .await
        .expect("revoked link lookup")
        .is_none());
}
```

- [ ] **Step 2: Run tests and confirm the expected failures**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_storage
```

Expected: compile failures for missing `SessionSource`, `MessageStatus`, `ImageTaskStatus`, runtime config helpers, session helpers, and signed-link helpers.

- [ ] **Step 3: Extend models with role, notification, session, message, task, artifact, signed-link, and runtime config records**

In `deps/gpt2api_rs/src/models.rs`, add these public types and extend `ApiKeyRecord`:

```rust
/// Product role assigned to a downstream API key.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyRole {
    /// Normal user scoped to their own sessions and artifacts.
    User,
    /// Product administrator with cross-key visibility and queue controls.
    Admin,
}

impl ApiKeyRole {
    /// Returns the stable SQLite representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Admin => "admin",
        }
    }

    /// Parses the stable SQLite representation.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "user" => Some(Self::User),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// Durable session source.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionSource {
    /// Created by the standalone web UI.
    Web,
    /// Created by OpenAI-compatible API calls.
    Api,
}

impl SessionSource {
    /// Returns the stable SQLite representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Api => "api",
        }
    }
}

/// Persisted session row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionRecord {
    /// Stable session id.
    pub id: String,
    /// Owning API-key id.
    pub key_id: String,
    /// Human-readable title.
    pub title: String,
    /// Session source.
    pub source: SessionSource,
    /// Active or archived.
    pub status: String,
    /// Creation epoch seconds.
    pub created_at: i64,
    /// Update epoch seconds.
    pub updated_at: i64,
    /// Last message epoch seconds.
    pub last_message_at: Option<i64>,
}

/// Message lifecycle status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    /// Created but no assistant content yet.
    Pending,
    /// Text stream is currently being forwarded.
    Streaming,
    /// Message is complete.
    Done,
    /// Message failed.
    Failed,
}

impl MessageStatus {
    /// Returns the stable SQLite representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Streaming => "streaming",
            Self::Done => "done",
            Self::Failed => "failed",
        }
    }
}

/// Persisted message row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageRecord {
    /// Stable message id.
    pub id: String,
    /// Parent session id.
    pub session_id: String,
    /// Owning API-key id.
    pub key_id: String,
    /// Conversation role.
    pub role: String,
    /// Structured content JSON string.
    pub content_json: String,
    /// Message status.
    pub status: MessageStatus,
    /// Creation epoch seconds.
    pub created_at: i64,
    /// Update epoch seconds.
    pub updated_at: i64,
}

/// Image task lifecycle status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ImageTaskStatus {
    /// Waiting in the global queue.
    Queued,
    /// Claimed by a worker.
    Running,
    /// Artifacts were written successfully.
    Succeeded,
    /// Upstream or local processing failed.
    Failed,
    /// Cancelled before execution.
    Cancelled,
}

impl ImageTaskStatus {
    /// Returns the stable SQLite representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Persisted image task row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageTaskRecord {
    /// Stable task id.
    pub id: String,
    /// Parent session id.
    pub session_id: String,
    /// Assistant message id updated by this task.
    pub message_id: String,
    /// Owning API-key id.
    pub key_id: String,
    /// Task status.
    pub status: ImageTaskStatus,
    /// generation or edit.
    pub mode: String,
    /// Original user prompt.
    pub prompt: String,
    /// Requested model.
    pub model: String,
    /// Requested image count.
    pub n: i64,
    /// Original request JSON.
    pub request_json: String,
    /// UI phase such as queued, allocating, running, saving, done, failed.
    pub phase: String,
    /// Queue entry epoch seconds.
    pub queue_entered_at: i64,
    /// Start epoch seconds.
    pub started_at: Option<i64>,
    /// Finish epoch seconds.
    pub finished_at: Option<i64>,
    /// Last known number of queued tasks ahead.
    pub position_snapshot: Option<i64>,
    /// Approximate wait in milliseconds.
    pub estimated_start_after_ms: Option<i64>,
    /// Stable error code.
    pub error_code: Option<String>,
    /// Human-readable error summary.
    pub error_message: Option<String>,
}

/// Generated image artifact metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageArtifactRecord {
    /// Stable artifact id.
    pub id: String,
    /// Parent task id.
    pub task_id: String,
    /// Parent session id.
    pub session_id: String,
    /// Parent message id.
    pub message_id: String,
    /// Owning API-key id.
    pub key_id: String,
    /// Relative filesystem path under the service root.
    pub relative_path: String,
    /// MIME type.
    pub mime_type: String,
    /// Hex SHA-256 of the file.
    pub sha256: String,
    /// File size in bytes.
    pub size_bytes: i64,
    /// Optional image width.
    pub width: Option<i64>,
    /// Optional image height.
    pub height: Option<i64>,
    /// Upstream revised prompt.
    pub revised_prompt: Option<String>,
    /// Creation epoch seconds.
    pub created_at: i64,
}

/// Runtime configuration stored in SQLite.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeConfigRecord {
    /// Minimum account refresh interval.
    pub refresh_min_seconds: i64,
    /// Maximum account refresh interval.
    pub refresh_max_seconds: i64,
    /// Refresh jitter.
    pub refresh_jitter_seconds: i64,
    /// Default per-key/account concurrency.
    pub default_request_max_concurrency: i64,
    /// Default start interval in milliseconds.
    pub default_request_min_start_interval_ms: i64,
    /// Usage outbox flush batch size.
    pub event_flush_batch_size: i64,
    /// Usage outbox flush interval.
    pub event_flush_interval_seconds: i64,
    /// Global image task concurrency.
    pub global_image_concurrency: i64,
    /// Signed link TTL in seconds.
    pub signed_link_ttl_seconds: i64,
    /// ETA averaging window size.
    pub queue_eta_window_size: i64,
}
```

Extend `ApiKeyRecord` and `ApiKeyRecord::minimal` with:

```rust
/// Product role.
pub role: ApiKeyRole,
/// Optional default notification email.
pub notification_email: Option<String>,
/// Whether completion email notifications are enabled.
pub notification_enabled: bool,
```

- [ ] **Step 4: Add additive migrations**

In `deps/gpt2api_rs/src/storage/migrations.rs`, extend `bootstrap_control_schema`:

```rust
ensure_api_key_product_columns(conn)?;
ensure_runtime_config_product_columns(conn)?;
bootstrap_product_tables(conn)?;
```

Add these helpers:

```rust
fn ensure_api_key_product_columns(conn: &SqliteConnection) -> Result<()> {
    let columns = table_columns(conn, "api_keys")?;
    if !columns.iter().any(|column| column == "role") {
        conn.execute_batch("ALTER TABLE api_keys ADD COLUMN role TEXT NOT NULL DEFAULT 'user'")?;
    }
    if !columns.iter().any(|column| column == "notification_email") {
        conn.execute_batch("ALTER TABLE api_keys ADD COLUMN notification_email TEXT")?;
    }
    if !columns.iter().any(|column| column == "notification_enabled") {
        conn.execute_batch(
            "ALTER TABLE api_keys ADD COLUMN notification_enabled INTEGER NOT NULL DEFAULT 0",
        )?;
    }
    Ok(())
}

fn ensure_runtime_config_product_columns(conn: &SqliteConnection) -> Result<()> {
    let columns = table_columns(conn, "runtime_config")?;
    if !columns.iter().any(|column| column == "global_image_concurrency") {
        conn.execute_batch(
            "ALTER TABLE runtime_config ADD COLUMN global_image_concurrency INTEGER NOT NULL DEFAULT 1",
        )?;
    }
    if !columns.iter().any(|column| column == "signed_link_ttl_seconds") {
        conn.execute_batch(
            "ALTER TABLE runtime_config ADD COLUMN signed_link_ttl_seconds INTEGER NOT NULL DEFAULT 604800",
        )?;
    }
    if !columns.iter().any(|column| column == "queue_eta_window_size") {
        conn.execute_batch(
            "ALTER TABLE runtime_config ADD COLUMN queue_eta_window_size INTEGER NOT NULL DEFAULT 20",
        )?;
    }
    Ok(())
}

fn bootstrap_product_tables(conn: &SqliteConnection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY NOT NULL,
            key_id TEXT NOT NULL,
            title TEXT NOT NULL,
            source TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            last_message_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_key_updated ON sessions(key_id, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_source_updated ON sessions(source, updated_at);
        CREATE INDEX IF NOT EXISTS idx_sessions_status_updated ON sessions(status, updated_at);

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            role TEXT NOT NULL,
            content_json TEXT NOT NULL,
            status TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_messages_session_created ON messages(session_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_messages_key_created ON messages(key_id, created_at);

        CREATE TABLE IF NOT EXISTS image_tasks (
            id TEXT PRIMARY KEY NOT NULL,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            status TEXT NOT NULL,
            mode TEXT NOT NULL,
            prompt TEXT NOT NULL,
            model TEXT NOT NULL,
            n INTEGER NOT NULL,
            request_json TEXT NOT NULL,
            phase TEXT NOT NULL,
            queue_entered_at INTEGER NOT NULL,
            started_at INTEGER,
            finished_at INTEGER,
            position_snapshot INTEGER,
            estimated_start_after_ms INTEGER,
            error_code TEXT,
            error_message TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_image_tasks_status_queue ON image_tasks(status, queue_entered_at);
        CREATE INDEX IF NOT EXISTS idx_image_tasks_key_queue ON image_tasks(key_id, queue_entered_at);
        CREATE INDEX IF NOT EXISTS idx_image_tasks_session_queue ON image_tasks(session_id, queue_entered_at);

        CREATE TABLE IF NOT EXISTS task_events (
            id TEXT PRIMARY KEY NOT NULL,
            task_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            event_kind TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_task_events_task_created ON task_events(task_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_task_events_session_created ON task_events(session_id, created_at);

        CREATE TABLE IF NOT EXISTS image_artifacts (
            id TEXT PRIMARY KEY NOT NULL,
            task_id TEXT NOT NULL,
            session_id TEXT NOT NULL,
            message_id TEXT NOT NULL,
            key_id TEXT NOT NULL,
            relative_path TEXT NOT NULL,
            mime_type TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            size_bytes INTEGER NOT NULL,
            width INTEGER,
            height INTEGER,
            revised_prompt TEXT,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_image_artifacts_task_created ON image_artifacts(task_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_image_artifacts_session_created ON image_artifacts(session_id, created_at);
        CREATE INDEX IF NOT EXISTS idx_image_artifacts_key_created ON image_artifacts(key_id, created_at);

        CREATE TABLE IF NOT EXISTS signed_links (
            id TEXT PRIMARY KEY NOT NULL,
            token_hash TEXT NOT NULL UNIQUE,
            scope TEXT NOT NULL,
            scope_id TEXT NOT NULL,
            expires_at INTEGER NOT NULL,
            revoked_at INTEGER,
            created_at INTEGER NOT NULL,
            used_at INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_signed_links_scope ON signed_links(scope, scope_id);
        "#,
    )?;
    Ok(())
}
```

Update `rebuild_api_keys_table` so the rebuilt table preserves `role`, `notification_email`, and `notification_enabled` when present and defaults them otherwise.

- [ ] **Step 5: Add control DB helpers**

In `deps/gpt2api_rs/src/storage/control.rs`, update `api_key_from_row`, every API-key SELECT, and `upsert_api_key` to include the three new key fields. Then add helper methods matching the tests:

```rust
pub async fn get_runtime_config(&self) -> Result<RuntimeConfigRecord>;
pub async fn update_runtime_config_product_fields(
    &self,
    global_image_concurrency: i64,
    signed_link_ttl_seconds: i64,
    queue_eta_window_size: i64,
) -> Result<RuntimeConfigRecord>;
pub async fn create_session(
    &self,
    key_id: &str,
    title: &str,
    source: SessionSource,
) -> Result<SessionRecord>;
pub async fn get_session_for_key(
    &self,
    session_id: &str,
    key_id: &str,
) -> Result<Option<SessionRecord>>;
pub async fn get_session_for_admin(&self, session_id: &str) -> Result<Option<SessionRecord>>;
pub async fn list_sessions_for_key(
    &self,
    key_id: &str,
    limit: u64,
    cursor_updated_before: Option<i64>,
) -> Result<Vec<SessionRecord>>;
pub async fn search_sessions_for_admin(
    &self,
    key_id: Option<&str>,
    query: Option<&str>,
    limit: u64,
    cursor_updated_before: Option<i64>,
) -> Result<Vec<SessionRecord>>;
pub async fn append_message(
    &self,
    session_id: &str,
    key_id: &str,
    role: &str,
    content: serde_json::Value,
    status: MessageStatus,
) -> Result<MessageRecord>;
pub async fn update_message_content_status(
    &self,
    message_id: &str,
    content: serde_json::Value,
    status: MessageStatus,
) -> Result<()>;
pub async fn list_messages_for_session(&self, session_id: &str) -> Result<Vec<MessageRecord>>;
pub async fn create_signed_link(
    &self,
    scope: &str,
    scope_id: &str,
    created_at: i64,
    ttl_seconds: i64,
) -> Result<CreatedSignedLink>;
pub async fn resolve_signed_link(
    &self,
    plaintext_token: &str,
    now: i64,
) -> Result<Option<SignedLinkRecord>>;
pub async fn revoke_signed_link(&self, link_id: &str, revoked_at: i64) -> Result<()>;
```

Use `tokio::task::spawn_blocking` consistently, matching existing `ControlDb` methods.

- [ ] **Step 6: Run storage tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_storage --test storage_bootstrap
```

Expected: all tests pass.

- [ ] **Step 7: Commit schema and storage foundation in the submodule**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add src/models.rs src/storage/migrations.rs src/storage/control.rs src/storage/mod.rs tests/storage_bootstrap.rs tests/product_storage.rs
git commit -m "feat: add product session storage"
```

### Task 2: Product Auth, Session, Message, And Admin-Key APIs

**Files:**
- Create: `deps/gpt2api_rs/src/http/product_api.rs`
- Modify: `deps/gpt2api_rs/src/http/mod.rs`
- Modify: `deps/gpt2api_rs/src/app.rs`
- Modify: `deps/gpt2api_rs/src/lib.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/src/http/admin_api.rs`
- Create: `deps/gpt2api_rs/tests/product_api.rs`

- [ ] **Step 1: Add failing API tests**

Create `deps/gpt2api_rs/tests/product_api.rs`:

```rust
//! Product API tests for key login, sessions, and role checks.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use gpt2api_rs::{
    app::build_router,
    config::ResolvedPaths,
    models::{ApiKeyRecord, ApiKeyRole},
    service::AppService,
    storage::Storage,
    upstream::chatgpt::ChatgptUpstreamClient,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tempfile::{tempdir, TempDir};
use tower::ServiceExt;

async fn build_app() -> (TempDir, axum::Router, Storage) {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");
    seed_key(&storage, "user-a", "User A", "sk-user-a", ApiKeyRole::User).await;
    seed_key(&storage, "user-b", "User B", "sk-user-b", ApiKeyRole::User).await;
    seed_key(&storage, "admin-a", "Admin A", "sk-admin-a", ApiKeyRole::Admin).await;
    let service = Arc::new(
        AppService::new(storage.clone(), "service-admin".to_string(), ChatgptUpstreamClient::default())
            .await
            .expect("service"),
    );
    (temp, build_router(service), storage)
}

async fn seed_key(storage: &Storage, id: &str, name: &str, secret: &str, role: ApiKeyRole) {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    let secret_hash = format!("{:x}", hasher.finalize());
    storage
        .control
        .upsert_api_key(&ApiKeyRecord {
            id: id.to_string(),
            name: name.to_string(),
            secret_hash,
            secret_plaintext: Some(secret.to_string()),
            status: "active".to_string(),
            quota_total_calls: 100,
            quota_used_calls: 0,
            route_strategy: "auto".to_string(),
            account_group_id: None,
            request_max_concurrency: None,
            request_min_start_interval_ms: None,
            role,
            notification_email: None,
            notification_enabled: false,
        })
        .await
        .expect("seed key");
}

async fn json_request(app: axum::Router, method: &str, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| json!({}));
    (status, value)
}

#[tokio::test]
async fn auth_verify_returns_role_and_notification_settings() {
    let (_temp, app, _storage) = build_app().await;
    let (status, value) = json_request(app, "POST", "/auth/verify", "sk-admin-a", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["ok"], true);
    assert_eq!(value["key"]["id"], "admin-a");
    assert_eq!(value["key"]["role"], "admin");
    assert_eq!(value["key"]["notification_enabled"], false);
}

#[tokio::test]
async fn user_sessions_are_scoped_to_own_key() {
    let (_temp, app, _storage) = build_app().await;
    let (created_status, created) = json_request(
        app.clone(),
        "POST",
        "/sessions",
        "sk-user-a",
        json!({"title":"Lake image"}),
    )
    .await;
    assert_eq!(created_status, StatusCode::OK);
    let session_id = created["session"]["id"].as_str().expect("session id");

    let (own_status, _own) =
        json_request(app.clone(), "GET", &format!("/sessions/{session_id}"), "sk-user-a", json!({})).await;
    assert_eq!(own_status, StatusCode::OK);

    let (other_status, _other) =
        json_request(app, "GET", &format!("/sessions/{session_id}"), "sk-user-b", json!({})).await;
    assert_eq!(other_status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn admin_key_can_search_all_sessions_without_service_admin_token() {
    let (_temp, app, _storage) = build_app().await;
    let (_created_status, _created) =
        json_request(app.clone(), "POST", "/sessions", "sk-user-a", json!({"title":"Lake image"})).await;

    let (status, value) =
        json_request(app, "GET", "/admin/sessions?q=Lake&limit=20", "sk-admin-a", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["items"].as_array().expect("items").len(), 1);
}

#[tokio::test]
async fn normal_key_cannot_use_admin_product_api() {
    let (_temp, app, _storage) = build_app().await;
    let (status, _value) =
        json_request(app, "GET", "/admin/sessions?limit=20", "sk-user-a", json!({})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run tests and confirm expected failures**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api
```

Expected: missing `/auth/verify`, `/sessions`, `/sessions/:id`, and admin-key session handlers.

- [ ] **Step 3: Add product API handlers**

Create `deps/gpt2api_rs/src/http/product_api.rs` with handlers for:

```rust
pub async fn verify_auth(...);
pub async fn get_me(...);
pub async fn update_my_notification(...);
pub async fn list_sessions(...);
pub async fn create_session(...);
pub async fn get_session(...);
pub async fn patch_session(...);
pub async fn list_admin_sessions(...);
```

Use these response shapes:

```json
{
  "ok": true,
  "key": {
    "id": "admin-a",
    "name": "Admin A",
    "status": "active",
    "role": "admin",
    "quota_total_calls": 100,
    "quota_used_calls": 0,
    "route_strategy": "auto",
    "notification_email": null,
    "notification_enabled": false
  }
}
```

For session detail responses, return:

```json
{
  "session": {},
  "messages": [],
  "tasks": [],
  "artifacts": []
}
```

- [ ] **Step 4: Add role-aware service helpers**

In `deps/gpt2api_rs/src/service.rs`, add:

```rust
pub async fn authenticate_product_key(
    &self,
    bearer: &str,
) -> std::result::Result<ApiKeyRecord, PublicAuthFailure> {
    self.authenticate_public_key(bearer).await
}

pub fn is_product_admin(&self, key: &ApiKeyRecord) -> bool {
    key.role == ApiKeyRole::Admin
}

pub async fn create_web_session(&self, key: &ApiKeyRecord, title: Option<&str>) -> Result<SessionRecord>;
pub async fn get_session_detail_for_key(&self, key: &ApiKeyRecord, session_id: &str) -> Result<Option<SessionDetail>>;
pub async fn get_session_detail_for_admin(&self, session_id: &str) -> Result<Option<SessionDetail>>;
```

`SessionDetail` belongs in `models.rs` and contains `session`, `messages`, `tasks`, and `artifacts`.

- [ ] **Step 5: Wire routes**

In `deps/gpt2api_rs/src/http/mod.rs`:

```rust
pub mod product_api;
```

In `deps/gpt2api_rs/src/app.rs`, add product routes before existing service-admin routes:

```rust
.route("/auth/verify", post(product_api::verify_auth))
.route("/me", get(product_api::get_me))
.route("/me/notification", patch(product_api::update_my_notification))
.route("/sessions", get(product_api::list_sessions).post(product_api::create_session))
.route("/sessions/:session_id", get(product_api::get_session).patch(product_api::patch_session))
.route("/admin/sessions", get(product_api::list_admin_sessions))
```

Keep `/auth/login` as an alias of key verification by returning the same key fields.

- [ ] **Step 6: Update key admin create/update fields without breaking service-admin token behavior**

In `deps/gpt2api_rs/src/http/admin_api.rs`, add `role`, `notification_email`, and `notification_enabled` to `CreateKeyRequest` and `UpdateKeyRequest`. Existing service-admin token auth remains accepted. Product-admin key auth is accepted for key listing and key patching, but product-admin responses must use a serializer that omits `secret_plaintext`.

- [ ] **Step 7: Run API tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api --test admin_api
```

Expected: product API tests pass and existing admin API tests still pass.

- [ ] **Step 8: Commit product API foundation in the submodule**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add src/http/product_api.rs src/http/mod.rs src/app.rs src/lib.rs src/service.rs src/http/admin_api.rs src/models.rs tests/product_api.rs tests/admin_api.rs
git commit -m "feat: add product session APIs"
```

### Task 3: Artifact Store And Image Task Runner

**Files:**
- Modify: `deps/gpt2api_rs/src/config.rs`
- Modify: `deps/gpt2api_rs/src/storage/mod.rs`
- Create: `deps/gpt2api_rs/src/storage/artifacts.rs`
- Create: `deps/gpt2api_rs/src/tasks.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/src/main.rs`
- Modify: `deps/gpt2api_rs/src/lib.rs`
- Create: `deps/gpt2api_rs/tests/image_task_runner.rs`

- [ ] **Step 1: Add failing runner tests**

Create `deps/gpt2api_rs/tests/image_task_runner.rs`:

```rust
//! Image task runner tests.

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use gpt2api_rs::{
    config::ResolvedPaths,
    models::{ImageTaskStatus, SessionSource},
    storage::Storage,
};
use tempfile::tempdir;

const ONE_PIXEL_PNG: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D',
    b'R', 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00,
    0x1f, 0x15, 0xc4, 0x89,
];

#[tokio::test]
async fn artifact_store_writes_under_key_session_message_path() {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");
    let session = storage
        .control
        .create_session("key-a", "Image", SessionSource::Web)
        .await
        .expect("session");
    let message = storage
        .control
        .append_message(
            &session.id,
            "key-a",
            "assistant",
            serde_json::json!({"blocks":[]}),
            gpt2api_rs::models::MessageStatus::Pending,
        )
        .await
        .expect("message");

    let item = gpt2api_rs::upstream::chatgpt::GeneratedImageItem {
        b64_json: BASE64.encode(ONE_PIXEL_PNG),
        revised_prompt: "lake".to_string(),
    };
    let artifact = storage
        .artifacts
        .write_generated_image("task-1", &session.id, &message.id, "key-a", &item, 0)
        .await
        .expect("artifact written");

    assert!(artifact.relative_path.starts_with(&format!(
        "artifacts/images/key-a/{}/{}/",
        session.id, message.id
    )));
    assert!(temp.path().join(&artifact.relative_path).is_file());
    assert_eq!(artifact.mime_type, "image/png");
    assert_eq!(artifact.width, Some(1));
    assert_eq!(artifact.height, Some(1));
}

#[tokio::test]
async fn queued_task_position_counts_tasks_ahead() {
    let temp = tempdir().expect("tempdir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");
    let first = storage
        .control
        .create_image_task("session-1", "message-1", "key-a", "generation", "one", "gpt-image-1", 1, serde_json::json!({}))
        .await
        .expect("first task");
    let second = storage
        .control
        .create_image_task("session-1", "message-2", "key-a", "generation", "two", "gpt-image-1", 1, serde_json::json!({}))
        .await
        .expect("second task");

    let snapshot = storage.control.queue_snapshot_for_task(&second.id).await.expect("snapshot");
    assert_eq!(snapshot.position_ahead, 1);
    assert_eq!(snapshot.task.id, second.id);

    let claimed = storage.control.claim_next_image_task(1, 123).await.expect("claim").expect("task");
    assert_eq!(claimed.id, first.id);
    assert_eq!(claimed.status, ImageTaskStatus::Running);
}
```

- [ ] **Step 2: Run tests and confirm expected failures**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test image_task_runner
```

Expected: missing artifact store, image task helpers, and queue snapshot helpers.

- [ ] **Step 3: Extend resolved paths**

In `deps/gpt2api_rs/src/config.rs`, add:

```rust
/// Directory containing generated image artifacts.
pub image_artifacts_dir: PathBuf,
```

Initialize it in `ResolvedPaths::new`:

```rust
image_artifacts_dir: root.join("artifacts").join("images"),
```

- [ ] **Step 4: Add artifact storage facade**

Create `deps/gpt2api_rs/src/storage/artifacts.rs`:

```rust
//! Filesystem-backed generated image artifact storage.

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use uuid::Uuid;

use crate::{
    models::ImageArtifactRecord,
    upstream::chatgpt::GeneratedImageItem,
};

/// Filesystem artifact store rooted at the service storage directory.
#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
    images_dir: PathBuf,
}

impl ArtifactStore {
    /// Creates a new artifact store.
    #[must_use]
    pub fn new(root: PathBuf, images_dir: PathBuf) -> Self {
        Self { root, images_dir }
    }

    /// Writes one generated image and returns its metadata.
    pub async fn write_generated_image(
        &self,
        task_id: &str,
        session_id: &str,
        message_id: &str,
        key_id: &str,
        item: &GeneratedImageItem,
        index: usize,
    ) -> Result<ImageArtifactRecord> {
        let bytes = BASE64.decode(item.b64_json.as_bytes()).context("invalid image base64")?;
        let image_id = format!("img_{}", Uuid::new_v4().simple());
        let file_name = if index == 0 {
            format!("{image_id}.png")
        } else {
            format!("{image_id}_{index}.png")
        };
        let relative_path = PathBuf::from("artifacts")
            .join("images")
            .join(key_id)
            .join(session_id)
            .join(message_id)
            .join(file_name);
        let full_path = self.root.join(&relative_path);
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::File::create(&full_path).await?;
        file.write_all(&bytes).await?;
        file.flush().await?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let (width, height) = detect_png_or_jpeg_dimensions(&bytes);
        Ok(ImageArtifactRecord {
            id: image_id,
            task_id: task_id.to_string(),
            session_id: session_id.to_string(),
            message_id: message_id.to_string(),
            key_id: key_id.to_string(),
            relative_path: relative_path.to_string_lossy().replace('\\', "/"),
            mime_type: "image/png".to_string(),
            sha256: format!("{:x}", hasher.finalize()),
            size_bytes: bytes.len() as i64,
            width,
            height,
            revised_prompt: Some(item.revised_prompt.clone()),
            created_at: crate::service::unix_timestamp_secs(),
        })
    }

    /// Reads an artifact by metadata relative path.
    pub async fn read_artifact(&self, artifact: &ImageArtifactRecord) -> Result<Vec<u8>> {
        let path = self.root.join(&artifact.relative_path);
        let canonical_root = self.root.canonicalize().context("canonical root")?;
        let canonical_path = path.canonicalize().context("canonical artifact path")?;
        anyhow::ensure!(canonical_path.starts_with(canonical_root), "artifact path escaped root");
        Ok(tokio::fs::read(canonical_path).await?)
    }
}
```

Add a private `detect_png_or_jpeg_dimensions` function using the existing PNG/JPEG header logic from `upstream/chatgpt.rs`.

- [ ] **Step 5: Wire artifact store**

In `deps/gpt2api_rs/src/storage/mod.rs`:

```rust
pub mod artifacts;

pub struct Storage {
    pub control: control::ControlDb,
    pub events: events::EventStore,
    pub artifacts: artifacts::ArtifactStore,
}
```

Inside `Storage::open`, create `paths.image_artifacts_dir` and initialize the artifact store.

- [ ] **Step 6: Add image task DB helpers**

In `deps/gpt2api_rs/src/storage/control.rs`, add:

```rust
pub async fn create_image_task(
    &self,
    session_id: &str,
    message_id: &str,
    key_id: &str,
    mode: &str,
    prompt: &str,
    model: &str,
    n: i64,
    request_json: serde_json::Value,
) -> Result<ImageTaskRecord>;
pub async fn get_image_task(&self, task_id: &str) -> Result<Option<ImageTaskRecord>>;
pub async fn get_image_task_for_key(
    &self,
    task_id: &str,
    key_id: &str,
) -> Result<Option<ImageTaskRecord>>;
pub async fn list_tasks_for_session(&self, session_id: &str) -> Result<Vec<ImageTaskRecord>>;
pub async fn list_artifacts_for_session(&self, session_id: &str) -> Result<Vec<ImageArtifactRecord>>;
pub async fn insert_image_artifact(&self, artifact: &ImageArtifactRecord) -> Result<()>;
pub async fn get_image_artifact(&self, artifact_id: &str) -> Result<Option<ImageArtifactRecord>>;
pub async fn claim_next_image_task(&self, global_limit: i64, started_at: i64) -> Result<Option<ImageTaskRecord>>;
pub async fn mark_image_task_phase(&self, task_id: &str, phase: &str, payload: serde_json::Value) -> Result<()>;
pub async fn mark_image_task_succeeded(
    &self,
    task_id: &str,
    finished_at: i64,
    artifact_ids: &[String],
) -> Result<()>;
pub async fn mark_image_task_failed(
    &self,
    task_id: &str,
    finished_at: i64,
    error_code: &str,
    error_message: &str,
) -> Result<()>;
pub async fn cancel_queued_image_task(&self, task_id: &str, key_id: Option<&str>) -> Result<bool>;
pub async fn queue_snapshot_for_task(&self, task_id: &str) -> Result<QueueSnapshot>;
pub async fn queue_snapshot_admin(&self) -> Result<AdminQueueSnapshot>;
```

`claim_next_image_task` must run in one SQLite transaction:

```sql
SELECT COUNT(*) FROM image_tasks WHERE status = 'running';
SELECT id FROM image_tasks WHERE status = 'queued' ORDER BY queue_entered_at ASC LIMIT 1;
UPDATE image_tasks SET status = 'running', phase = 'allocating', started_at = ?2 WHERE id = ?1 AND status = 'queued';
```

If running count is already at the configured global limit, return `Ok(None)`.

- [ ] **Step 7: Add task runner**

Create `deps/gpt2api_rs/src/tasks.rs`:

```rust
//! Background image task runner and queue snapshots.

use anyhow::Result;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

use crate::{models::ImageTaskRecord, service::AppService};

/// Background worker that drains queued image tasks.
#[derive(Debug)]
pub struct ImageTaskRunner {
    service: Arc<AppService>,
}

impl ImageTaskRunner {
    /// Creates a runner bound to the application service.
    #[must_use]
    pub fn new(service: Arc<AppService>) -> Self {
        Self { service }
    }

    /// Spawns the queue-drain loop.
    pub fn spawn(self, mut shutdown_rx: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        if changed.is_err() || *shutdown_rx.borrow() {
                            return;
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(500)) => {
                        let _ = self.drain_once().await;
                    }
                }
            }
        })
    }

    /// Claims and executes tasks until no global queue slot is available.
    pub async fn drain_once(&self) -> Result<()> {
        loop {
            let config = self.service.storage().control.get_runtime_config().await?;
            let Some(task) = self
                .service
                .storage()
                .control
                .claim_next_image_task(config.global_image_concurrency, crate::service::unix_timestamp_secs())
                .await?
            else {
                return Ok(());
            };
            let service = Arc::clone(&self.service);
            tokio::spawn(async move {
                let _ = service.execute_claimed_image_task(task).await;
            });
        }
    }
}
```

In `service.rs`, add `execute_claimed_image_task(task: ImageTaskRecord)`. It must:

1. load the key by `task.key_id`
2. mark phase `allocating`
3. acquire key/account leases using existing scheduler methods
4. mark phase `running`
5. call existing upstream image generation/edit path without directly settling quota inside the old method
6. write artifacts
7. update assistant message content in place
8. settle usage exactly once on success
9. mark task succeeded or failed
10. trigger notification send on success

- [ ] **Step 8: Spawn runner**

In `deps/gpt2api_rs/src/main.rs`, after the existing worker spawns:

```rust
std::mem::drop(gpt2api_rs::tasks::ImageTaskRunner::new(Arc::clone(&service)).spawn(shutdown_rx.clone()));
```

In `deps/gpt2api_rs/src/lib.rs`:

```rust
pub mod tasks;
```

- [ ] **Step 9: Run runner tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test image_task_runner --test product_storage
```

Expected: all tests pass.

- [ ] **Step 10: Commit task runner and artifact store**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add src/config.rs src/storage/mod.rs src/storage/artifacts.rs src/storage/control.rs src/tasks.rs src/service.rs src/main.rs src/lib.rs tests/image_task_runner.rs tests/product_storage.rs
git commit -m "feat: add image task runner and artifacts"
```

### Task 4: Product Message API, SSE Progress, And Task Cancellation

**Files:**
- Modify: `deps/gpt2api_rs/src/http/product_api.rs`
- Modify: `deps/gpt2api_rs/src/app.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/src/storage/control.rs`
- Modify: `deps/gpt2api_rs/Cargo.toml`
- Modify: `deps/gpt2api_rs/tests/product_api.rs`

- [ ] **Step 1: Add failing product message tests**

Append to `deps/gpt2api_rs/tests/product_api.rs`:

```rust
#[tokio::test]
async fn image_message_creates_pending_assistant_message_and_queued_task() {
    let (_temp, app, _storage) = build_app().await;
    let (_status, created) =
        json_request(app.clone(), "POST", "/sessions", "sk-user-a", json!({"title":"Image"})).await;
    let session_id = created["session"]["id"].as_str().expect("session id");

    let (status, value) = json_request(
        app,
        "POST",
        &format!("/sessions/{session_id}/messages"),
        "sk-user-a",
        json!({"kind":"image_generation","prompt":"draw a lake","model":"gpt-image-1","n":1}),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["task"]["status"], "queued");
    assert_eq!(value["assistant_message"]["status"], "pending");
    assert_eq!(value["queue"]["position_ahead"], 0);
}

#[tokio::test]
async fn queued_task_can_be_cancelled_by_owner() {
    let (_temp, app, _storage) = build_app().await;
    let (_status, created) =
        json_request(app.clone(), "POST", "/sessions", "sk-user-a", json!({"title":"Image"})).await;
    let session_id = created["session"]["id"].as_str().expect("session id");
    let (_status, submitted) = json_request(
        app.clone(),
        "POST",
        &format!("/sessions/{session_id}/messages"),
        "sk-user-a",
        json!({"kind":"image_generation","prompt":"draw a lake","model":"gpt-image-1","n":1}),
    )
    .await;
    let task_id = submitted["task"]["id"].as_str().expect("task id");

    let (status, value) =
        json_request(app, "POST", &format!("/tasks/{task_id}/cancel"), "sk-user-a", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["cancelled"], true);
}
```

- [ ] **Step 2: Run tests and confirm expected failures**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api image_message_creates_pending_assistant_message_and_queued_task queued_task_can_be_cancelled_by_owner
```

Expected: missing message submission and cancel handlers.

- [ ] **Step 3: Add message submission endpoint**

In `product_api.rs`, add:

```rust
pub async fn create_message(
    Path(session_id): Path<String>,
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
    Json(body): Json<CreateMessageRequest>,
) -> Result<Json<Value>, AppError>;
```

Supported request bodies:

```json
{"kind":"text","text":"hello","model":"gpt-5"}
{"kind":"image_generation","prompt":"draw a lake","model":"gpt-image-1","n":1}
```

For edit uploads, add a multipart handler:

```rust
pub async fn create_edit_message(
    Path(session_id): Path<String>,
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Json<Value>, AppError>;
```

Use route `/sessions/:session_id/messages/edit` for multipart image edits.

- [ ] **Step 4: Add service submission methods**

In `service.rs`, add:

```rust
pub async fn submit_text_message(
    &self,
    key: &ApiKeyRecord,
    session_id: &str,
    text: &str,
    model: &str,
) -> Result<SessionDetail>;

pub async fn submit_image_generation_message(
    &self,
    key: &ApiKeyRecord,
    session_id: &str,
    prompt: &str,
    model: &str,
    n: usize,
) -> Result<ImageSubmissionResult>;

pub async fn submit_image_edit_message(
    &self,
    key: &ApiKeyRecord,
    session_id: &str,
    prompt: &str,
    model: &str,
    n: usize,
    edit_input: ImageEditInput,
) -> Result<ImageSubmissionResult>;
```

`ImageSubmissionResult` contains `user_message`, `assistant_message`, `task`, and `queue`.

- [ ] **Step 5: Add SSE route**

Add dependency in `deps/gpt2api_rs/Cargo.toml`:

```toml
tokio-stream = { version = "0.1", features = ["sync"] }
```

Add route:

```rust
.route("/sessions/:session_id/events", get(product_api::session_events))
```

The SSE handler sends a snapshot first, then emits `task_event` frames every time new `task_events` rows appear for that session:

```text
event: snapshot
data: {"session":{},"messages":[],"tasks":[],"artifacts":[]}

event: task_event
data: {"task_id":"...","event_kind":"phase","payload":{}}
```

Polling every 750 ms is acceptable for v1 because the source of truth is SQLite and reconnect recovers state via `GET /sessions/:id`.

- [ ] **Step 6: Add task read and cancel endpoints**

Routes:

```rust
.route("/tasks/:task_id", get(product_api::get_task))
.route("/tasks/:task_id/cancel", post(product_api::cancel_task))
```

Owner keys can cancel only their own queued tasks. Admin keys can cancel queued tasks for any key.

- [ ] **Step 7: Run product API tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api
```

Expected: all product API tests pass.

- [ ] **Step 8: Commit message and SSE APIs**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add Cargo.toml Cargo.lock src/http/product_api.rs src/app.rs src/service.rs src/storage/control.rs tests/product_api.rs
git commit -m "feat: add workspace message and progress APIs"
```

### Task 5: OpenAI-Compatible APIs Write Session History

**Files:**
- Modify: `deps/gpt2api_rs/src/http/public_api.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/tests/public_api.rs`

- [ ] **Step 1: Add failing compatibility history tests**

Append to `deps/gpt2api_rs/tests/public_api.rs`:

```rust
#[tokio::test]
async fn compatible_image_generation_writes_api_session_history() {
    let (_temp, app) = build_test_app().await;
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/images/generations")
                .header("authorization", "Bearer admin-secret")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"gpt-image-1","prompt":"draw a lake","n":1}"#))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);

    let sessions = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/sessions?limit=20")
                .header("authorization", "Bearer admin-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("sessions response");
    assert_eq!(sessions.status(), StatusCode::OK);
    let bytes = to_bytes(sessions.into_body(), 1024 * 1024).await.expect("body");
    let value: Value = serde_json::from_slice(&bytes).expect("json");
    assert!(value["items"].as_array().expect("items").iter().any(|item| item["source"] == "api"));
}
```

- [ ] **Step 2: Run the failing test**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test public_api compatible_image_generation_writes_api_session_history
```

Expected: session history is missing.

- [ ] **Step 3: Add API session resolution**

In `public_api.rs`, read optional header `x-gpt2api-session-id`. In `service.rs`, add:

```rust
pub async fn resolve_api_session(
    &self,
    key: &ApiKeyRecord,
    supplied_session_id: Option<&str>,
) -> Result<SessionRecord>;
```

Behavior:

- If `supplied_session_id` is present and belongs to the key, use it.
- If absent, create or reuse the newest active `source = api` session titled `API Requests`.
- If supplied id belongs to another key, return `bad_request`.

- [ ] **Step 4: Route synchronous image compatibility through task storage**

Replace direct calls to `generate_images_for_key` and `edit_images_for_key` in compatibility handlers with:

```rust
let session = service.resolve_api_session(&key, extract_session_header(&headers).as_deref()).await?;
let submission = service
    .submit_image_generation_message(&key, &session.id, body.prompt.trim(), body.model.trim(), n)
    .await?;
let completed = service.wait_for_image_task(&submission.task.id, std::time::Duration::from_secs(180)).await?;
Ok(Json(service.build_image_generation_response(&completed)).into_response())
```

For image edits, call `submit_image_edit_message`.

- [ ] **Step 5: Write text chat history**

For non-streaming `/v1/chat/completions` and `/v1/responses`, wrap the existing text execution:

```rust
let session = service.resolve_api_session(&key, extract_session_header(&headers).as_deref()).await?;
let user_message = service.append_api_user_text_message(&key, &session.id, &prompt, model, endpoint).await?;
match service.complete_text_for_key(&key, &prompt, model, endpoint).await {
    Ok(result) => {
        service.append_api_assistant_text_message(&key, &session.id, &user_message.id, &result).await?;
        Ok(...)
    }
    Err(error) => {
        service.append_api_failed_assistant_message(&key, &session.id, &user_message.id, &error.to_string()).await?;
        Err(...)
    }
}
```

For streaming text, return the same SSE shape but wrap the upstream body stream so the adapter accumulates parsed text chunks and updates the assistant message when the stream ends. If the stream errors, mark the assistant message failed.

- [ ] **Step 6: Run compatibility tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test public_api
```

Expected: existing public API tests and history test pass.

- [ ] **Step 7: Commit compatibility history**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add src/http/public_api.rs src/service.rs tests/public_api.rs
git commit -m "feat: persist compatible API history"
```

### Task 6: Signed Links And Email Notifications

**Files:**
- Modify: `deps/gpt2api_rs/Cargo.toml`
- Modify: `deps/gpt2api_rs/src/config.rs`
- Create: `deps/gpt2api_rs/src/notifications.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/src/http/product_api.rs`
- Modify: `deps/gpt2api_rs/src/app.rs`
- Modify: `deps/gpt2api_rs/src/lib.rs`
- Create: `deps/gpt2api_rs/tests/notifications.rs`

- [ ] **Step 1: Add failing notification tests**

Create `deps/gpt2api_rs/tests/notifications.rs`:

```rust
//! Notification and share-link tests.

use gpt2api_rs::notifications::{is_valid_notification_email, render_image_done_email};

#[test]
fn notification_email_validation_is_syntax_only() {
    assert!(is_valid_notification_email("user@example.com"));
    assert!(is_valid_notification_email("user.name+tag@example.co"));
    assert!(!is_valid_notification_email(""));
    assert!(!is_valid_notification_email("missing-at.example.com"));
    assert!(!is_valid_notification_email("user@"));
}

#[test]
fn image_done_email_contains_prompt_and_signed_link_without_key_secret() {
    let rendered = render_image_done_email(
        "Lake session",
        "draw a lake",
        "gpt-image-1",
        1,
        "https://example.com/gpt2api/share/token-123",
    );
    assert!(rendered.subject.contains("Lake session"));
    assert!(rendered.text_body.contains("draw a lake"));
    assert!(rendered.text_body.contains("https://example.com/gpt2api/share/token-123"));
    assert!(!rendered.text_body.contains("sk-"));
}
```

- [ ] **Step 2: Run tests and confirm expected failures**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test notifications
```

Expected: missing notification module.

- [ ] **Step 3: Add email dependencies**

In `deps/gpt2api_rs/Cargo.toml`:

```toml
lettre = { version = "0.11", default-features = false, features = ["builder", "smtp-transport", "tokio1-rustls-tls"] }
```

- [ ] **Step 4: Add notification config**

In `deps/gpt2api_rs/src/config.rs`, add:

```rust
/// SMTP configuration for optional image completion emails.
#[derive(Debug, Clone, Default)]
pub struct SmtpConfig {
    /// Public base URL used in email links.
    pub public_base_url: Option<String>,
    /// SMTP host.
    pub host: Option<String>,
    /// SMTP port.
    pub port: u16,
    /// SMTP username.
    pub username: Option<String>,
    /// SMTP password.
    pub password: Option<String>,
    /// Sender email address.
    pub from: Option<String>,
}

impl SmtpConfig {
    /// Reads SMTP config from environment variables.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            public_base_url: std::env::var("GPT2API_PUBLIC_BASE_URL").ok(),
            host: std::env::var("GPT2API_SMTP_HOST").ok(),
            port: std::env::var("GPT2API_SMTP_PORT").ok().and_then(|value| value.parse().ok()).unwrap_or(587),
            username: std::env::var("GPT2API_SMTP_USERNAME").ok(),
            password: std::env::var("GPT2API_SMTP_PASSWORD").ok(),
            from: std::env::var("GPT2API_SMTP_FROM").ok(),
        }
    }
}
```

- [ ] **Step 5: Add notification module**

Create `deps/gpt2api_rs/src/notifications.rs` with:

```rust
//! Optional email notifications for completed image tasks.

/// Rendered email content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEmail {
    /// Email subject.
    pub subject: String,
    /// Plain text body.
    pub text_body: String,
}

/// Performs syntax-only email validation.
#[must_use]
pub fn is_valid_notification_email(email: &str) -> bool {
    let value = email.trim();
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

/// Renders the image completion email.
#[must_use]
pub fn render_image_done_email(
    session_title: &str,
    prompt: &str,
    model: &str,
    image_count: usize,
    signed_link: &str,
) -> RenderedEmail {
    RenderedEmail {
        subject: format!("GPT2API image ready: {session_title}"),
        text_body: format!(
            "Your image generation is complete.\n\nSession: {session_title}\nModel: {model}\nImages: {image_count}\nPrompt:\n{prompt}\n\nView result:\n{signed_link}\n"
        ),
    }
}
```

Add async sender logic behind `SmtpConfig` using `lettre`. If SMTP config is incomplete, return `NotificationOutcome::Skipped`.

- [ ] **Step 6: Add share-link APIs**

Routes:

```rust
.route("/share/:token", get(product_api::get_share))
.route("/share/:token/artifacts/:artifact_id", get(product_api::get_shared_artifact))
```

`GET /share/:token` returns:

```json
{
  "scope": "image_task",
  "session": {},
  "task": {},
  "messages": [],
  "artifacts": []
}
```

`GET /share/:token/artifacts/:artifact_id` streams the image only when the artifact belongs to the signed link scope.

- [ ] **Step 7: Trigger notifications after successful image tasks**

At the end of `execute_claimed_image_task`, after artifacts and signed link are written, call:

```rust
self.send_image_done_notification(&key, &task, &session, &artifacts, &signed_link).await
```

Email send failure must record a `task_events` row with `event_kind = "notification_failed"` and must not change task success.

- [ ] **Step 8: Run notification tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test notifications --test product_api
```

Expected: all notification and product API tests pass.

- [ ] **Step 9: Commit notifications and share links**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add Cargo.toml Cargo.lock src/config.rs src/notifications.rs src/service.rs src/http/product_api.rs src/app.rs src/lib.rs tests/notifications.rs tests/product_api.rs
git commit -m "feat: add image notifications and signed links"
```

### Task 7: Admin Queue, Runtime Config, And Cross-Key Inspection

**Files:**
- Modify: `deps/gpt2api_rs/src/http/admin_api.rs`
- Modify: `deps/gpt2api_rs/src/http/product_api.rs`
- Modify: `deps/gpt2api_rs/src/service.rs`
- Modify: `deps/gpt2api_rs/src/storage/control.rs`
- Modify: `deps/gpt2api_rs/tests/admin_api.rs`
- Modify: `deps/gpt2api_rs/tests/product_api.rs`

- [ ] **Step 1: Add failing admin queue tests**

Append to `deps/gpt2api_rs/tests/product_api.rs`:

```rust
#[tokio::test]
async fn admin_key_can_update_global_queue_concurrency() {
    let (_temp, app, _storage) = build_app().await;
    let (status, value) = json_request(
        app.clone(),
        "PATCH",
        "/admin/queue/config",
        "sk-admin-a",
        json!({"global_image_concurrency":2}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(value["config"]["global_image_concurrency"], 2);

    let (user_status, _user_value) = json_request(
        app,
        "PATCH",
        "/admin/queue/config",
        "sk-user-a",
        json!({"global_image_concurrency":3}),
    )
    .await;
    assert_eq!(user_status, StatusCode::FORBIDDEN);
}
```

- [ ] **Step 2: Run test and confirm expected failure**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api admin_key_can_update_global_queue_concurrency
```

Expected: missing queue config handler.

- [ ] **Step 3: Add admin queue endpoints**

Routes:

```rust
.route("/admin/queue", get(product_api::admin_queue))
.route("/admin/queue/config", patch(product_api::patch_admin_queue_config))
.route("/admin/tasks/:task_id/cancel", post(product_api::admin_cancel_task))
```

`PATCH /admin/queue/config` accepts:

```json
{"global_image_concurrency":2}
```

Reject values outside `1..=16` with `400`.

- [ ] **Step 4: Add admin key management fields**

Product-admin `PATCH /admin/keys/:id` accepts:

```json
{
  "role": "admin",
  "notification_email": "user@example.com",
  "notification_enabled": true,
  "quota_total_calls": 100,
  "route_strategy": "auto",
  "request_max_concurrency": 1,
  "request_min_start_interval_ms": 0
}
```

Rules:

- product-admin may update role, notification, quota, routing, and scheduler fields
- product-admin response omits `secret_plaintext`
- service-admin token keeps access to existing import/account operations
- no endpoint returns another key's raw secret to product-admin callers

- [ ] **Step 5: Run admin tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test --test product_api --test admin_api
```

Expected: product-admin queue tests pass and service-admin tests keep passing.

- [ ] **Step 6: Commit admin queue controls**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
git add src/http/admin_api.rs src/http/product_api.rs src/service.rs src/storage/control.rs tests/admin_api.rs tests/product_api.rs
git commit -m "feat: add product admin queue controls"
```

### Task 8: Standalone React Workspace Frontend

**Files:**
- Create: `frontend/gpt2api-app/package.json`
- Create: `frontend/gpt2api-app/package-lock.json`
- Create: `frontend/gpt2api-app/index.html`
- Create: `frontend/gpt2api-app/tsconfig.json`
- Create: `frontend/gpt2api-app/vite.config.ts`
- Create: `frontend/gpt2api-app/src/api.ts`
- Create: `frontend/gpt2api-app/src/App.tsx`
- Create: `frontend/gpt2api-app/src/main.tsx`
- Create: `frontend/gpt2api-app/src/styles.css`
- Create: `frontend/gpt2api-app/src/types.ts`
- Create: `frontend/gpt2api-app/src/components/Composer.tsx`
- Create: `frontend/gpt2api-app/src/components/PendingImageCard.tsx`
- Create: `frontend/gpt2api-app/src/components/SessionSidebar.tsx`
- Create: `frontend/gpt2api-app/src/components/AdminPanel.tsx`
- Modify: `frontend/.gitignore`
- Create: `scripts/build_gpt2api_frontend.sh`
- Modify: `scripts/build_frontend_selfhosted.sh`

- [ ] **Step 1: Scaffold Vite React app**

Create `frontend/gpt2api-app/package.json`:

```json
{
  "name": "gpt2api-workspace",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "build": "tsc -b && vite build",
    "dev": "vite --host 127.0.0.1"
  },
  "dependencies": {
    "@vitejs/plugin-react": "^5.0.0",
    "vite": "^7.0.0",
    "typescript": "^5.6.0",
    "react": "^19.0.0",
    "react-dom": "^19.0.0",
    "lucide-react": "^0.468.0"
  },
  "devDependencies": {}
}
```

Create `frontend/gpt2api-app/vite.config.ts`:

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  base: "/static/gpt2api/",
  build: {
    outDir: "../static/gpt2api",
    emptyOutDir: true,
  },
  server: {
    proxy: {
      "/api/gpt2api": "http://127.0.0.1:39180",
    },
  },
});
```

- [ ] **Step 2: Add frontend types and API client**

Create `frontend/gpt2api-app/src/types.ts` with session, message, task, artifact, queue, key, and share-link response types that match the product API response fields.

Create `frontend/gpt2api-app/src/api.ts`:

```ts
const API_BASE = "/api/gpt2api";

export function authHeaders(key: string): HeadersInit {
  return {
    authorization: `Bearer ${key}`,
    "content-type": "application/json",
  };
}

export async function fetchJson<T>(path: string, key: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(`${API_BASE}${path}`, {
    ...init,
    headers: {
      ...authHeaders(key),
      ...(init.headers || {}),
    },
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function openSessionEventStream(sessionId: string, key: string): Promise<ReadableStream<Uint8Array>> {
  const response = await fetch(`${API_BASE}/sessions/${encodeURIComponent(sessionId)}/events`, {
    headers: authHeaders(key),
  });
  if (!response.ok || !response.body) {
    throw new Error(`event stream failed: HTTP ${response.status}`);
  }
  return response.body;
}
```

Parse the returned SSE byte stream in React so the API key stays in the `Authorization` header instead of the URL.

- [ ] **Step 3: Build ChatGPT-style layout**

In `App.tsx`, implement:

- login screen
- left session sidebar with search and new chat
- center message stream
- bottom composer with `Chat`, `Image`, and `Edit` modes
- right panel for queue details, notification settings, and admin tabs
- share view for `/gpt2api/share/:token`

Use no local business history. Store only the current API key in `localStorage`.

- [ ] **Step 4: Implement pending image card**

In `PendingImageCard.tsx`, render these phases:

```ts
const phaseLabels: Record<string, string> = {
  queued: "Queued",
  allocating: "Starting",
  running: "Running",
  saving: "Finishing",
  done: "Done",
  failed: "Failed",
  cancelled: "Cancelled",
};
```

The card must show:

- queue position
- tasks ahead
- approximate ETA
- elapsed time
- activity log
- cancel button while queued

It must update from SSE events and recover from `GET /sessions/:id` on refresh.

- [ ] **Step 5: Add admin panel**

In `AdminPanel.tsx`, implement tabs:

- `All sessions`: `GET /admin/sessions?key_id=&q=&limit=`
- `Queue`: `GET /admin/queue`, `PATCH /admin/queue/config`, `POST /admin/tasks/:id/cancel`
- `Keys`: `GET /admin/keys`, `PATCH /admin/keys/:id`

Hide the panel for `role !== "admin"`. Treat hidden UI only as presentation; backend role checks remain the authority.

- [ ] **Step 6: Add build script**

Create `scripts/build_gpt2api_frontend.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
APP_DIR="$ROOT_DIR/frontend/gpt2api-app"
NPM_CACHE_DIR="${NPM_CACHE_DIR:-$ROOT_DIR/tmp/npm-cache}"

cd "$APP_DIR"
mkdir -p "$NPM_CACHE_DIR"
if [[ ! -d node_modules ]]; then
  NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm install
fi
NPM_CONFIG_CACHE="$NPM_CACHE_DIR" npm run build
```

Make it executable:

```bash
chmod +x scripts/build_gpt2api_frontend.sh
```

- [ ] **Step 7: Wire self-hosted build**

In `scripts/build_frontend_selfhosted.sh`, before the Trunk build step, run:

```bash
log "Building standalone GPT2API frontend..."
"$ROOT_DIR/scripts/build_gpt2api_frontend.sh"
```

This writes `frontend/static/gpt2api`, and Trunk copies it into `frontend/dist/static/gpt2api`.

In `frontend/.gitignore`, add:

```gitignore
/static/gpt2api/
/gpt2api-app/node_modules/
/gpt2api-app/dist/
```

- [ ] **Step 8: Build frontend**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
./scripts/build_gpt2api_frontend.sh
./scripts/build_frontend_selfhosted.sh --skip-npm
```

Expected:

- `frontend/static/gpt2api/index.html` exists
- `frontend/dist/static/gpt2api/index.html` exists
- generated JS does not contain `http://localhost:3000/api`

- [ ] **Step 9: Commit frontend workspace**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
git add frontend/gpt2api-app frontend/.gitignore scripts/build_gpt2api_frontend.sh scripts/build_frontend_selfhosted.sh
git commit -m "feat: add gpt2api workspace frontend"
```

### Task 9: StaticFlow Route Shell And Share Routes

**Files:**
- Modify: `backend/src/routes.rs`
- Modify: `backend/src/handlers.rs`

- [ ] **Step 1: Add route expectations**

Extend the existing handler test in `backend/src/handlers.rs` to assert the standalone entry path remains:

```rust
#[test]
fn gpt2api_frontend_index_path_points_to_static_entry() {
    let path = gpt2api_frontend_index_path(PathBuf::from("/tmp/frontend/dist").as_path());
    assert_eq!(path, PathBuf::from("/tmp/frontend/dist/static/gpt2api/index.html"));
}
```

This test already exists; keep it unchanged.

- [ ] **Step 2: Expand frontend shell routes**

In `backend/src/routes.rs`, replace the three explicit GPT2API public frontend routes with wildcard shell routes:

```rust
let gpt2api_frontend_router = Router::new()
    .route("/gpt2api", get(handlers::serve_gpt2api_frontend))
    .route("/gpt2api/*path", get(handlers::serve_gpt2api_frontend))
    .route("/static_flow/gpt2api", get(handlers::serve_gpt2api_frontend))
    .route("/static_flow/gpt2api/*path", get(handlers::serve_gpt2api_frontend))
    .with_state(spa_state.clone());
```

This covers `/gpt2api/login`, `/gpt2api/chat`, `/gpt2api/share/:token`, and future nested frontend routes.

- [ ] **Step 3: Run backend route tests**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
cargo test -p static-flow-backend gpt2api_frontend_index_path_points_to_static_entry
```

Expected: test passes.

- [ ] **Step 4: Commit StaticFlow shell route**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
git add backend/src/routes.rs backend/src/handlers.rs
git commit -m "feat: serve gpt2api workspace shell routes"
```

### Task 10: Full Verification And Release Startup

**Files:**
- Update only files required by failures found during this task.

- [ ] **Step 1: Format changed Rust files only**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo fmt
```

Run StaticFlow formatting only for touched parent Rust files:

```bash
cd /home/ts_user/rust_pro/static_flow
rustfmt backend/src/routes.rs backend/src/handlers.rs
```

Do not run `cargo fmt` at the StaticFlow workspace root.

- [ ] **Step 2: Run gpt2api-rs tests and clippy**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
cargo test
cargo clippy --all-targets --all-features -- -D warnings
cargo build --release
```

Expected: all pass.

- [ ] **Step 3: Run StaticFlow affected tests and clippy**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
cargo test -p static-flow-backend gpt2api_frontend_index_path_points_to_static_entry
cargo clippy -p static-flow-backend --all-targets --all-features -- -D warnings
```

Expected: all pass.

- [ ] **Step 4: Build frontend**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
./scripts/build_frontend_selfhosted.sh --skip-npm
```

Expected: self-hosted frontend build succeeds and `frontend/dist/static/gpt2api/index.html` exists.

- [ ] **Step 5: Start release gpt2api-rs with production-like storage**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs
tmux kill-session -t gpt2api-rs 2>/dev/null || true
tmux new-session -d -s gpt2api-rs -- \
  ./target/release/gpt2api-rs serve \
    --listen 127.0.0.1:8787 \
    --storage-dir /mnt/wsl/data4tb/static-flow-data/gpt2api-rs \
    --admin-token "$GPT2API_ADMIN_TOKEN"
```

Expected:

```bash
curl -sS http://127.0.0.1:8787/healthz
```

returns a healthy response.

- [ ] **Step 6: Smoke product API**

Run with a real downstream key:

```bash
curl -sS -X POST \
  -H "Authorization: Bearer $GPT2API_KEY" \
  http://127.0.0.1:8787/auth/verify

curl -sS -X POST \
  -H "Authorization: Bearer $GPT2API_KEY" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:8787/sessions \
  -d '{"title":"Smoke image"}'
```

Expected: key verification returns role and notification fields; session creation returns a session id.

- [ ] **Step 7: Smoke StaticFlow proxy and frontend shell**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
curl -sS -o /dev/null -w 'code=%{http_code}\n' http://127.0.0.1:39180/gpt2api/login
curl -sS -o /dev/null -w 'code=%{http_code}\n' http://127.0.0.1:39180/gpt2api/share/test-token
curl -sS -X POST -H "Authorization: Bearer $GPT2API_KEY" -o /dev/null -w 'code=%{http_code}\n' http://127.0.0.1:39180/api/gpt2api/auth/verify
```

Expected: frontend shell routes return `200`; API verify returns `200` for a valid key.

- [ ] **Step 8: Browser verification**

Use Playwright against `http://127.0.0.1:39180/gpt2api/login`:

- login succeeds
- session list loads from server
- new chat creates a session
- image task shows queued/running/saving/done state changes
- refresh recovers the pending card
- admin key shows queue controls
- normal key does not show admin controls and admin APIs return `403`

- [ ] **Step 9: Parent repository submodule pointer commit**

After the submodule commits are pushed or confirmed local, update the parent pointer and commit parent-side changes:

```bash
cd /home/ts_user/rust_pro/static_flow
git add deps/gpt2api_rs backend/src/routes.rs backend/src/handlers.rs frontend/gpt2api-app frontend/.gitignore scripts/build_gpt2api_frontend.sh scripts/build_frontend_selfhosted.sh
git commit -m "feat: add gpt2api session image workspace"
```

## Self-Review Checklist

- Spec coverage: sessions, messages, prompts, artifacts, email links, admin queue controls, ETA/progress, API compatibility, and StaticFlow shell routing are each covered by a task.
- Backward compatibility: existing `/v1/images/generations`, `/v1/images/edits`, `/v1/chat/completions`, `/v1/responses`, service-admin account operations, and StaticFlow `/api/gpt2api/*` proxy behavior remain in place.
- Security: product admin role does not expose raw key secrets; artifact files are read only through authorized APIs; signed links store token hashes only.
- Performance: queue scans use status/time indexes; artifact bytes live on disk, not in SQLite; SSE v1 uses lightweight polling with snapshot recovery.
- Verification: affected Rust tests, clippy, release build, self-hosted frontend build, API smoke, and browser smoke are listed with exact commands.
