# LLM Access Full Parity Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a standalone `llm-access` binary that fully replaces StaticFlow's current LLM/Kiro/Codex subsystem without dropping provider, admin, usage, cache, or account-level runtime behavior.

**Architecture:** Extract the current backend-owned LLM logic into focused crates, keep one source of truth for provider behavior, and swap persistence from LanceDB to SQLite plus DuckDB through typed repository traits. The existing StaticFlow backend should become a consumer or proxy of the extracted runtime instead of owning divergent provider logic.

**Tech Stack:** Rust, Axum, Tokio, Reqwest, SQLite via `rusqlite`, DuckDB, existing StaticFlow LLM/Kiro modules, targeted Rust tests with `--jobs 1`, per-file `rustfmt`, no live backend restarts.

---

## Execution Rules

- Do not restart `sf-gateway`, any backend slot, Caddy, pb-mapper, or live production service.
- Keep each task in a separate commit.
- Before every Rust build/check, run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
```

- If another Rust build/link process is active, wait until it exits.
- Use `--jobs 1` for every Cargo command.
- Do not run workspace `cargo fmt`. Format only files changed in the current task with `rustfmt path/to/file.rs`.
- Do not run `cargo fmt` inside `deps/lance` or `deps/lancedb`.
- Do not route real production traffic to `llm-access` until every verification task in this plan is complete.

## File Structure

- Create `llm-access-core/`
  - Shared route inventory, provider/key/account enums, usage event contracts, timing contracts, route decision types, and store traits.
- Create `llm-access-kiro/`
  - Extracted Kiro auth, token refresh, scheduler, status cache, cache simulator, cache policy, Anthropic converter, stream parser, provider dispatch, and usage conversion.
- Create `llm-access-codex/`
  - Extracted Codex auth, token refresh, account pool, model catalog, request normalization, response/SSE conversion, and usage conversion.
- Modify `llm-access-store/`
  - SQLite repositories for control-plane state and DuckDB writer/query APIs for usage analytics.
- Modify `llm-access-migrations/`
  - SQLite and DuckDB schema migrations for the full parity data model.
- Modify `llm-access-migrator/`
  - Snapshot import and CDC replay for all LLM entities and usage events.
- Modify `llm-access/`
  - Axum server, config, runtime wiring, public/provider/admin route registration, health, version, and shutdown.
- Modify `backend/`
  - Replace local provider internals with extracted crate imports where practical, leaving route wiring compatible during transition.
- Modify `frontend/` only if compatibility tests expose schema drift.
- Modify `docs/superpowers/specs/2026-04-30-llm-access-full-parity-design.md`
  - Add implementation status as milestones land.

## Task 1: Add Workspace Crates And Route-Surface Contract

**Files:**
- Modify: `Cargo.toml`
- Create: `llm-access-core/Cargo.toml`
- Create: `llm-access-core/src/lib.rs`
- Create: `llm-access-core/src/routes.rs`
- Create: `llm-access-core/src/provider.rs`
- Create: `llm-access-core/src/usage.rs`
- Modify: `llm-access/Cargo.toml`
- Modify: `llm-access/src/routes.rs`

- [ ] **Step 1: Write route-surface tests**

Create `llm-access-core/src/routes.rs` with this initial contract:

```rust
//! Canonical route surface owned by the standalone LLM access service.

/// HTTP route declaration used by compatibility tests and router wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec {
    /// HTTP method or `ANY` for wildcard proxy routes.
    pub method: &'static str,
    /// Axum-compatible route pattern.
    pub path: &'static str,
}

/// Public/provider routes that must be handled by `llm-access`.
pub const PUBLIC_PROVIDER_ROUTES: &[RouteSpec] = &[
    RouteSpec { method: "ANY", path: "/api/llm-gateway/v1/*path" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/access" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/model-catalog.json" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/status" },
    RouteSpec { method: "POST", path: "/api/llm-gateway/public-usage/query" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/support-config" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/support-assets/:file_name" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/account-contributions" },
    RouteSpec { method: "GET", path: "/api/llm-gateway/sponsors" },
    RouteSpec { method: "POST", path: "/api/llm-gateway/token-requests/submit" },
    RouteSpec { method: "POST", path: "/api/llm-gateway/account-contribution-requests/submit" },
    RouteSpec { method: "POST", path: "/api/llm-gateway/sponsor-requests/submit" },
    RouteSpec { method: "GET", path: "/api/kiro-gateway/access" },
    RouteSpec { method: "GET", path: "/api/kiro-gateway/v1/models" },
    RouteSpec { method: "POST", path: "/api/kiro-gateway/v1/messages" },
    RouteSpec { method: "POST", path: "/api/kiro-gateway/v1/messages/count_tokens" },
    RouteSpec { method: "POST", path: "/api/kiro-gateway/cc/v1/messages" },
    RouteSpec { method: "POST", path: "/api/kiro-gateway/cc/v1/messages/count_tokens" },
];

/// Admin routes that must keep working for the current frontend.
pub const ADMIN_ROUTES: &[RouteSpec] = &[
    RouteSpec { method: "GET|POST", path: "/admin/llm-gateway/config" },
    RouteSpec { method: "GET|POST", path: "/admin/llm-gateway/proxy-configs" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/proxy-configs/import-legacy-kiro" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/llm-gateway/proxy-configs/:proxy_id" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/proxy-configs/:proxy_id/check/:provider_type" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/proxy-bindings" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/proxy-bindings/:provider_type" },
    RouteSpec { method: "GET|POST", path: "/admin/llm-gateway/account-groups" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/llm-gateway/account-groups/:group_id" },
    RouteSpec { method: "GET|POST", path: "/admin/llm-gateway/keys" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/llm-gateway/keys/:key_id" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/usage" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/usage/:event_id" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/token-requests" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/token-requests/:request_id/approve-and-issue" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/token-requests/:request_id/reject" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/account-contribution-requests" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/account-contribution-requests/:request_id/approve-and-issue" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/account-contribution-requests/:request_id/reject" },
    RouteSpec { method: "GET", path: "/admin/llm-gateway/sponsor-requests" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/sponsor-requests/:request_id/approve" },
    RouteSpec { method: "DELETE", path: "/admin/llm-gateway/sponsor-requests/:request_id" },
    RouteSpec { method: "GET|POST", path: "/admin/llm-gateway/accounts" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/llm-gateway/accounts/:name" },
    RouteSpec { method: "POST", path: "/admin/llm-gateway/accounts/:name/refresh" },
    RouteSpec { method: "GET|POST", path: "/admin/kiro-gateway/account-groups" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/kiro-gateway/account-groups/:group_id" },
    RouteSpec { method: "GET|POST", path: "/admin/kiro-gateway/keys" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/kiro-gateway/keys/:key_id" },
    RouteSpec { method: "GET", path: "/admin/kiro-gateway/usage" },
    RouteSpec { method: "GET", path: "/admin/kiro-gateway/usage/:event_id" },
    RouteSpec { method: "GET", path: "/admin/kiro-gateway/accounts/statuses" },
    RouteSpec { method: "GET|POST", path: "/admin/kiro-gateway/accounts" },
    RouteSpec { method: "POST", path: "/admin/kiro-gateway/accounts/import-local" },
    RouteSpec { method: "PATCH|DELETE", path: "/admin/kiro-gateway/accounts/:name" },
    RouteSpec { method: "GET|POST", path: "/admin/kiro-gateway/accounts/:name/balance" },
];

/// Return whether a path is owned by `llm-access`.
pub fn is_llm_access_path(path: &str) -> bool {
    path.starts_with("/api/llm-gateway/")
        || path.starts_with("/api/kiro-gateway/")
        || path.starts_with("/admin/llm-gateway/")
        || path.starts_with("/admin/kiro-gateway/")
        || path.starts_with("/v1/")
        || path.starts_with("/cc/v1/")
}

#[cfg(test)]
mod tests {
    use super::{is_llm_access_path, ADMIN_ROUTES, PUBLIC_PROVIDER_ROUTES};

    #[test]
    fn route_contract_contains_required_public_provider_paths() {
        let paths = PUBLIC_PROVIDER_ROUTES.iter().map(|route| route.path).collect::<Vec<_>>();
        assert!(paths.contains(&"/api/llm-gateway/v1/*path"));
        assert!(paths.contains(&"/api/kiro-gateway/v1/messages"));
        assert!(paths.contains(&"/api/kiro-gateway/cc/v1/messages"));
        assert!(paths.contains(&"/api/llm-gateway/public-usage/query"));
    }

    #[test]
    fn route_contract_contains_required_admin_paths() {
        let paths = ADMIN_ROUTES.iter().map(|route| route.path).collect::<Vec<_>>();
        assert!(paths.contains(&"/admin/llm-gateway/keys"));
        assert!(paths.contains(&"/admin/llm-gateway/accounts/:name/refresh"));
        assert!(paths.contains(&"/admin/kiro-gateway/keys/:key_id"));
        assert!(paths.contains(&"/admin/kiro-gateway/accounts/:name/balance"));
    }

    #[test]
    fn route_ownership_matches_llm_path_prefixes() {
        assert!(is_llm_access_path("/api/llm-gateway/status"));
        assert!(is_llm_access_path("/api/kiro-gateway/cc/v1/messages"));
        assert!(is_llm_access_path("/admin/kiro-gateway/accounts"));
        assert!(!is_llm_access_path("/api/articles"));
        assert!(!is_llm_access_path("/admin/local-media"));
    }
}
```

- [ ] **Step 2: Add crate scaffold**

Create `llm-access-core/Cargo.toml`:

```toml
[package]
name = "llm-access-core"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
async-trait = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
```

Create `llm-access-core/src/lib.rs`:

```rust
//! Shared contracts for the standalone LLM access service.

pub mod provider;
pub mod routes;
pub mod usage;
```

Create `llm-access-core/src/provider.rs`:

```rust
//! Provider-neutral request and routing contracts.

use serde::{Deserialize, Serialize};

/// LLM provider family used by keys, accounts, usage events, and routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    /// Codex/OpenAI-compatible provider path.
    Codex,
    /// Kiro/Claude-compatible provider path.
    Kiro,
}

/// Client-facing protocol family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    /// OpenAI-compatible API surface.
    OpenAi,
    /// Anthropic/Claude-compatible API surface.
    Anthropic,
}

/// Account routing strategy stored on a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteStrategy {
    /// Let the runtime choose from eligible accounts.
    Auto,
    /// Force a single account.
    Fixed,
}
```

Create `llm-access-core/src/usage.rs`:

```rust
//! Provider-neutral usage event contract.

use serde::{Deserialize, Serialize};

use crate::provider::{ProtocolFamily, ProviderType, RouteStrategy};

/// Timing fields captured by provider handlers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageTiming {
    /// Time from route entry until upstream headers in milliseconds.
    pub upstream_headers_ms: Option<i64>,
    /// Time from upstream headers until upstream body completion in milliseconds.
    pub post_headers_body_ms: Option<i64>,
    /// Time from route entry until first downstream SSE write in milliseconds.
    pub first_sse_write_ms: Option<i64>,
    /// Time from route entry until stream finish in milliseconds.
    pub stream_finish_ms: Option<i64>,
}

/// One normalized usage event before persistence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageEvent {
    /// Stable event id.
    pub event_id: String,
    /// Creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Provider type.
    pub provider_type: ProviderType,
    /// Protocol family.
    pub protocol_family: ProtocolFamily,
    /// Key id at event time.
    pub key_id: String,
    /// Key name at event time.
    pub key_name: String,
    /// Account name used by the upstream request.
    pub account_name: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<RouteStrategy>,
    /// Client-facing endpoint.
    pub endpoint: String,
    /// Client-facing model.
    pub model: Option<String>,
    /// Upstream mapped model.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Credit usage when known.
    pub credit_usage: Option<String>,
    /// Whether normal token usage was unavailable.
    pub usage_missing: bool,
    /// Whether credit usage was unavailable.
    pub credit_usage_missing: bool,
    /// Provider timing fields.
    pub timing: UsageTiming,
}
```

- [ ] **Step 3: Wire workspace membership and dependency**

Modify root `Cargo.toml` members:

```toml
members = [
    "frontend",
    "shared",
    "backend",
    "cli",
    "media-service",
    "media-types",
    "gateway",
    "llm-access",
    "llm-access-core",
    "llm-access-migrations",
    "llm-access-migrator",
    "llm-access-store",
]
```

Modify `llm-access/Cargo.toml`:

```toml
llm-access-core = { path = "../llm-access-core" }
```

- [ ] **Step 4: Delegate path ownership to core**

Replace `llm-access/src/routes.rs` with:

```rust
//! Route ownership helpers for cloud path splitting.

pub use llm_access_core::routes::is_llm_access_path;

#[cfg(test)]
mod tests {
    #[test]
    fn recognizes_public_llm_provider_paths() {
        for path in [
            "/v1/chat/completions",
            "/v1/responses",
            "/v1/models",
            "/cc/v1/messages",
            "/api/llm-gateway/v1/responses",
            "/api/kiro-gateway/v1/messages",
            "/api/codex-gateway/v1/responses",
            "/api/llm-access/status",
        ] {
            assert!(super::is_llm_access_path(path), "{path}");
        }
    }

    #[test]
    fn leaves_non_llm_staticflow_paths_on_local_backend() {
        for path in ["/", "/api/articles", "/api/music/songs", "/admin/local-media"] {
            assert!(!super::is_llm_access_path(path), "{path}");
        }
    }
}
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-core --jobs 1
cargo test -p llm-access --jobs 1 routes::tests -- --nocapture
rustfmt llm-access-core/src/lib.rs llm-access-core/src/routes.rs llm-access-core/src/provider.rs llm-access-core/src/usage.rs llm-access/src/routes.rs
git add Cargo.toml llm-access/Cargo.toml llm-access-core llm-access/src/routes.rs
git commit -m "feat: add llm access core contracts"
```

Expected:

- `cargo test -p llm-access-core --jobs 1` passes.
- `cargo test -p llm-access --jobs 1 routes::tests -- --nocapture` passes.

## Task 2: Complete SQLite Control-Plane Schema And Repositories

**Files:**
- Modify: `llm-access-migrations/migrations/sqlite/0001_init.sql`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/lib.rs`
- Test: `llm-access-store/src/sqlite.rs`

- [ ] **Step 1: Add schema coverage tests**

Append this test module to `llm-access-store/src/sqlite.rs`:

```rust
#[cfg(test)]
mod schema_tests {
    use rusqlite::Connection;

    fn table_names(conn: &Connection) -> Vec<String> {
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("prepare table query");
        stmt.query_map([], |row| row.get::<_, String>(0))
            .expect("query table names")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table names")
    }

    #[test]
    fn sqlite_schema_contains_full_parity_control_tables() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("initialize sqlite");
        let tables = table_names(&conn);

        for required in [
            "llm_keys",
            "llm_key_route_config",
            "llm_key_usage_rollups",
            "llm_runtime_config",
            "llm_account_groups",
            "llm_proxy_configs",
            "llm_proxy_bindings",
            "llm_codex_accounts",
            "llm_kiro_accounts",
            "llm_kiro_status_cache",
            "llm_token_requests",
            "llm_account_contribution_requests",
            "llm_sponsor_requests",
            "cdc_consumer_offsets",
            "cdc_apply_state",
            "cdc_applied_events_recent",
        ] {
            assert!(tables.contains(&required.to_string()), "missing table {required}");
        }
    }

    #[test]
    fn key_lookup_by_hash_is_indexed() {
        let conn = Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("initialize sqlite");
        let mut stmt = conn
            .prepare("PRAGMA index_list('llm_keys')")
            .expect("prepare index query");
        let indexes = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query indexes")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect indexes");
        assert!(indexes.iter().any(|name| name.contains("key_hash") || name.contains("sqlite_autoindex")));
    }
}
```

- [ ] **Step 2: Run tests and verify failure**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-store --jobs 1 sqlite::schema_tests -- --nocapture
```

Expected:

- The first test fails because `llm_codex_accounts`, `llm_kiro_accounts`, and `llm_kiro_status_cache` are not present yet.

- [ ] **Step 3: Add account and status tables**

Append these tables to `llm-access-migrations/migrations/sqlite/0001_init.sql` after `llm_proxy_bindings`:

```sql
CREATE TABLE IF NOT EXISTS llm_codex_accounts (
    account_name TEXT PRIMARY KEY,
    account_id TEXT,
    email TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json TEXT NOT NULL CHECK (json_valid(auth_json)),
    settings_json TEXT NOT NULL CHECK (json_valid(settings_json)),
    last_refresh_at_ms INTEGER CHECK (last_refresh_at_ms IS NULL OR last_refresh_at_ms >= 0),
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_codex_accounts_status
    ON llm_codex_accounts(status);

CREATE TABLE IF NOT EXISTS llm_kiro_accounts (
    account_name TEXT PRIMARY KEY,
    auth_method TEXT NOT NULL,
    account_id TEXT,
    profile_arn TEXT,
    user_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json TEXT NOT NULL CHECK (json_valid(auth_json)),
    max_concurrency INTEGER CHECK (max_concurrency IS NULL OR max_concurrency >= 0),
    min_start_interval_ms INTEGER CHECK (min_start_interval_ms IS NULL OR min_start_interval_ms >= 0),
    proxy_config_id TEXT,
    last_refresh_at_ms INTEGER CHECK (last_refresh_at_ms IS NULL OR last_refresh_at_ms >= 0),
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_status
    ON llm_kiro_accounts(status);

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_user_id
    ON llm_kiro_accounts(user_id);

CREATE TABLE IF NOT EXISTS llm_kiro_status_cache (
    account_name TEXT PRIMARY KEY REFERENCES llm_kiro_accounts(account_name) ON DELETE CASCADE,
    status TEXT NOT NULL,
    balance_json TEXT NOT NULL CHECK (json_valid(balance_json)),
    cache_json TEXT NOT NULL CHECK (json_valid(cache_json)),
    refreshed_at_ms INTEGER NOT NULL CHECK (refreshed_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
    last_error TEXT
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_kiro_status_cache_expires
    ON llm_kiro_status_cache(expires_at_ms);
```

- [ ] **Step 4: Make control records public and complete key route fields**

In `llm-access-store/src/sqlite.rs`, make all fields on `KeyRecord`, `KeyRouteConfig`, `KeyUsageRollup`, and `RuntimeConfigRecord` public. Add the missing route fields so the struct matches the schema:

```rust
/// API key route configuration row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyRouteConfig {
    /// Owning key id.
    pub key_id: String,
    /// Account route strategy.
    pub route_strategy: Option<String>,
    /// Fixed account name for fixed routing.
    pub fixed_account_name: Option<String>,
    /// JSON array of account names for auto routing.
    pub auto_account_names_json: Option<String>,
    /// Account group id selected by the key.
    pub account_group_id: Option<String>,
    /// JSON object mapping public model names to upstream model names.
    pub model_name_map_json: Option<String>,
    /// Optional per-key concurrency cap.
    pub request_max_concurrency: Option<i64>,
    /// Optional per-key pacing interval.
    pub request_min_start_interval_ms: Option<i64>,
    /// Whether Kiro public request validation is enabled.
    pub kiro_request_validation_enabled: bool,
    /// Whether Kiro cache estimation is enabled.
    pub kiro_cache_estimation_enabled: bool,
    /// Whether zero-cache diagnostic capture is enabled.
    pub kiro_zero_cache_debug_enabled: bool,
    /// Optional Kiro cache policy override JSON.
    pub kiro_cache_policy_override_json: Option<String>,
    /// Optional Kiro billable multiplier override JSON.
    pub kiro_billable_model_multipliers_override_json: Option<String>,
}
```

Update `upsert_key_bundle()` and `decode_key_bundle()` to read and write every field in `llm_key_route_config`.

- [ ] **Step 5: Add repository methods for account rows**

Add these structs and methods to `llm-access-store/src/sqlite.rs`:

```rust
/// Codex account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Account email when known.
    pub email: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Persisted settings JSON.
    pub settings_json: String,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}

/// Kiro account control-plane row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KiroAccountRecord {
    /// Account display name.
    pub account_name: String,
    /// Kiro auth method.
    pub auth_method: String,
    /// Upstream account id when known.
    pub account_id: Option<String>,
    /// Kiro profile ARN when known.
    pub profile_arn: Option<String>,
    /// Upstream user id from usage limits when known.
    pub user_id: Option<String>,
    /// Runtime status.
    pub status: String,
    /// Persisted auth payload JSON.
    pub auth_json: String,
    /// Per-account concurrency cap.
    pub max_concurrency: Option<i64>,
    /// Per-account pacing interval.
    pub min_start_interval_ms: Option<i64>,
    /// Optional proxy config id.
    pub proxy_config_id: Option<String>,
    /// Last refresh timestamp.
    pub last_refresh_at_ms: Option<i64>,
    /// Last refresh or runtime error.
    pub last_error: Option<String>,
    /// Creation timestamp.
    pub created_at_ms: i64,
    /// Update timestamp.
    pub updated_at_ms: i64,
}
```

Implement:

```rust
pub fn upsert_codex_account(&self, record: &CodexAccountRecord) -> anyhow::Result<()>;
pub fn list_codex_accounts(&self) -> anyhow::Result<Vec<CodexAccountRecord>>;
pub fn upsert_kiro_account(&self, record: &KiroAccountRecord) -> anyhow::Result<()>;
pub fn list_kiro_accounts(&self) -> anyhow::Result<Vec<KiroAccountRecord>>;
```

Each method must use one upsert statement with `ON CONFLICT DO UPDATE` or one ordered `SELECT` statement with `ORDER BY account_name`.

- [ ] **Step 6: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-store --jobs 1 sqlite::schema_tests -- --nocapture
cargo test -p llm-access-store --jobs 1 sqlite -- --nocapture
rustfmt llm-access-store/src/sqlite.rs llm-access-store/src/lib.rs
git add llm-access-migrations/migrations/sqlite/0001_init.sql llm-access-store/src/sqlite.rs llm-access-store/src/lib.rs
git commit -m "feat: complete llm access sqlite control plane"
```

Expected:

- SQLite schema tests pass.
- Existing `llm-access-store` tests pass.

## Task 3: Align DuckDB Usage Analytics With Existing Usage Events

**Files:**
- Modify: `llm-access-migrations/migrations/duckdb/0001_init.sql`
- Modify: `llm-access-store/src/duckdb.rs`
- Test: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Replace DuckDB row contract with full usage fact fields**

Update `UsageEventRow` in `llm-access-store/src/duckdb.rs` so its fields match `usage_events` in `llm-access-migrations/migrations/duckdb/0001_init.sql`:

```rust
/// One row for the DuckDB `usage_events` wide fact table.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventRow {
    /// Source CDC sequence, or zero for native standalone events.
    pub source_seq: i64,
    /// Source CDC event id, or this event id for native standalone events.
    pub source_event_id: String,
    /// Stable usage event id.
    pub event_id: String,
    /// Event creation timestamp in Unix milliseconds.
    pub created_at_ms: i64,
    /// Provider type at event time.
    pub provider_type: String,
    /// Protocol family at event time.
    pub protocol_family: String,
    /// API key id at event time.
    pub key_id: String,
    /// API key display name at event time.
    pub key_name: String,
    /// Key status captured at event time.
    pub key_status_at_event: String,
    /// Upstream account name at event time.
    pub account_name: Option<String>,
    /// Account group id captured at event time.
    pub account_group_id_at_event: Option<String>,
    /// Route strategy captured at event time.
    pub route_strategy_at_event: Option<String>,
    /// Provider endpoint.
    pub endpoint: String,
    /// Requested model name.
    pub model: Option<String>,
    /// Mapped upstream model name.
    pub mapped_model: Option<String>,
    /// Final HTTP status code.
    pub status_code: i64,
    /// Overall latency in milliseconds.
    pub latency_ms: Option<i64>,
    /// Time waiting for local routing or scheduler.
    pub routing_wait_ms: Option<i64>,
    /// Time until upstream headers.
    pub upstream_headers_ms: Option<i64>,
    /// Time from upstream headers until body completion.
    pub post_headers_body_ms: Option<i64>,
    /// Time until first downstream SSE write.
    pub first_sse_write_ms: Option<i64>,
    /// Time until stream finish.
    pub stream_finish_ms: Option<i64>,
    /// Request body size in bytes.
    pub request_body_bytes: Option<i64>,
    /// Uncached input tokens.
    pub input_uncached_tokens: i64,
    /// Cached input tokens.
    pub input_cached_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Billable tokens.
    pub billable_tokens: i64,
    /// Credit usage when known.
    pub credit_usage: Option<String>,
    /// Whether token usage was unavailable.
    pub usage_missing: bool,
    /// Whether credit usage was unavailable.
    pub credit_usage_missing: bool,
    /// Client IP captured at event time.
    pub client_ip: Option<String>,
    /// IP region captured at event time.
    pub ip_region: Option<String>,
}
```

- [ ] **Step 2: Add SQL shape test**

Replace the existing DuckDB SQL test with:

```rust
#[test]
fn usage_insert_sql_targets_all_fact_columns_without_runtime_joins() {
    let sql = super::insert_usage_event_sql();
    let lower = sql.to_ascii_lowercase();

    assert!(sql.starts_with("INSERT INTO usage_events"));
    for column in [
        "source_seq",
        "source_event_id",
        "event_id",
        "created_at_ms",
        "provider_type",
        "protocol_family",
        "key_id",
        "key_name",
        "key_status_at_event",
        "account_name",
        "endpoint",
        "status_code",
        "upstream_headers_ms",
        "post_headers_body_ms",
        "first_sse_write_ms",
        "stream_finish_ms",
        "input_uncached_tokens",
        "input_cached_tokens",
        "output_tokens",
        "billable_tokens",
        "credit_usage",
        "usage_missing",
        "credit_usage_missing",
    ] {
        assert!(sql.contains(column), "missing column {column}");
    }
    assert!(!lower.contains(" join "));
}
```

- [ ] **Step 3: Update insert SQL and writer**

Update `insert_usage_event_sql()` so the column list exactly matches the `usage_events` table, and update `DuckDbUsageWriter::insert_usage_event()` to bind those fields in the same order. Compute `created_at`, `created_date`, and `created_hour` inside SQL from `created_at_ms`:

```sql
to_timestamp(?4 / 1000.0),
CAST(to_timestamp(?4 / 1000.0) AS DATE),
date_trunc('hour', to_timestamp(?4 / 1000.0))
```

- [ ] **Step 4: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-store --jobs 1 duckdb -- --nocapture
rustfmt llm-access-store/src/duckdb.rs
git add llm-access-store/src/duckdb.rs llm-access-migrations/migrations/duckdb/0001_init.sql
git commit -m "feat: align llm access duckdb usage facts"
```

Expected:

- DuckDB SQL shape test passes.
- No runtime DuckDB feature is required for the default test command.

## Task 4: Extract Kiro Pure Runtime Modules

**Files:**
- Modify: `Cargo.toml`
- Create: `llm-access-kiro/Cargo.toml`
- Create: `llm-access-kiro/src/lib.rs`
- Move: `backend/src/kiro_gateway/scheduler.rs` -> `llm-access-kiro/src/scheduler.rs`
- Move: `backend/src/kiro_gateway/cache_sim.rs` -> `llm-access-kiro/src/cache_sim.rs`
- Move: `backend/src/kiro_gateway/cache_policy.rs` -> `llm-access-kiro/src/cache_policy.rs`
- Move: `backend/src/kiro_gateway/billable_multipliers.rs` -> `llm-access-kiro/src/billable_multipliers.rs`
- Move: `backend/src/kiro_gateway/parser/*` -> `llm-access-kiro/src/parser/*`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/Cargo.toml`

- [ ] **Step 1: Add Kiro crate scaffold**

Create `llm-access-kiro/Cargo.toml`:

```toml
[package]
name = "llm-access-kiro"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
bytes = { workspace = true }
chrono = { workspace = true }
crc = "3.3"
llm-access-core = { path = "../llm-access-core" }
parking_lot = "0.12"
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["test-util"] }
```

Create `llm-access-kiro/src/lib.rs`:

```rust
//! Kiro provider runtime extracted for standalone LLM access.

pub mod billable_multipliers;
pub mod cache_policy;
pub mod cache_sim;
pub mod parser;
pub mod scheduler;
```

Add `"llm-access-kiro"` to workspace members and add this dependency to `backend/Cargo.toml`:

```toml
llm-access-kiro = { path = "../llm-access-kiro" }
```

- [ ] **Step 2: Move pure Kiro files**

Run:

```bash
mkdir -p llm-access-kiro/src/parser
git mv backend/src/kiro_gateway/scheduler.rs llm-access-kiro/src/scheduler.rs
git mv backend/src/kiro_gateway/cache_sim.rs llm-access-kiro/src/cache_sim.rs
git mv backend/src/kiro_gateway/cache_policy.rs llm-access-kiro/src/cache_policy.rs
git mv backend/src/kiro_gateway/billable_multipliers.rs llm-access-kiro/src/billable_multipliers.rs
git mv backend/src/kiro_gateway/parser/crc.rs llm-access-kiro/src/parser/crc.rs
git mv backend/src/kiro_gateway/parser/decoder.rs llm-access-kiro/src/parser/decoder.rs
git mv backend/src/kiro_gateway/parser/error.rs llm-access-kiro/src/parser/error.rs
git mv backend/src/kiro_gateway/parser/frame.rs llm-access-kiro/src/parser/frame.rs
git mv backend/src/kiro_gateway/parser/header.rs llm-access-kiro/src/parser/header.rs
git mv backend/src/kiro_gateway/parser/mod.rs llm-access-kiro/src/parser/mod.rs
```

- [ ] **Step 3: Replace backend modules with re-exports**

Create `backend/src/kiro_gateway/scheduler.rs`:

```rust
//! Re-exported Kiro scheduler from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::scheduler::*;
```

Create `backend/src/kiro_gateway/cache_sim.rs`:

```rust
//! Re-exported Kiro cache simulator from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::cache_sim::*;
```

Create `backend/src/kiro_gateway/cache_policy.rs`:

```rust
//! Re-exported Kiro cache policy helpers from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::cache_policy::*;
```

Create `backend/src/kiro_gateway/billable_multipliers.rs`:

```rust
//! Re-exported Kiro billable multiplier helpers from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::billable_multipliers::*;
```

Create `backend/src/kiro_gateway/parser/mod.rs`:

```rust
//! Re-exported Kiro event stream parser from the standalone LLM access runtime.

pub(crate) use llm_access_kiro::parser::*;
```

- [ ] **Step 4: Fix visibility and crate paths**

In moved files, replace `pub(crate)` items that are used by backend modules with `pub`. Use `llm_access_core` for shared contracts, `llm_access_kiro` for moved Kiro modules, and `static_flow_shared` for existing shared records. Keep test names and assertions intact.

Visibility rule:

```rust
// Before extraction, used outside this crate:
pub(crate) struct KiroRequestScheduler;

// After extraction:
pub struct KiroRequestScheduler;
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-kiro --jobs 1 -- --nocapture
cargo test -p static-flow-backend --jobs 1 kiro_gateway::scheduler -- --nocapture
rustfmt llm-access-kiro/src/lib.rs llm-access-kiro/src/scheduler.rs llm-access-kiro/src/cache_sim.rs llm-access-kiro/src/cache_policy.rs llm-access-kiro/src/billable_multipliers.rs llm-access-kiro/src/parser/*.rs backend/src/kiro_gateway/scheduler.rs backend/src/kiro_gateway/cache_sim.rs backend/src/kiro_gateway/cache_policy.rs backend/src/kiro_gateway/billable_multipliers.rs backend/src/kiro_gateway/parser/mod.rs
git add Cargo.toml backend/Cargo.toml backend/src/kiro_gateway llm-access-kiro
git commit -m "feat: extract kiro pure runtime modules"
```

Expected:

- `llm-access-kiro` pure module tests pass.
- Backend Kiro scheduler tests still pass through re-exported modules.

## Task 5: Extract Kiro Anthropic Conversion And Stream Semantics

**Files:**
- Move: `backend/src/kiro_gateway/anthropic/types.rs` -> `llm-access-kiro/src/anthropic/types.rs`
- Move: `backend/src/kiro_gateway/anthropic/converter.rs` -> `llm-access-kiro/src/anthropic/converter.rs`
- Move: `backend/src/kiro_gateway/anthropic/stream.rs` -> `llm-access-kiro/src/anthropic/stream.rs`
- Move: `backend/src/kiro_gateway/anthropic/websearch.rs` -> `llm-access-kiro/src/anthropic/websearch.rs`
- Modify: `llm-access-kiro/src/lib.rs`
- Create: `backend/src/kiro_gateway/anthropic/types.rs`
- Create: `backend/src/kiro_gateway/anthropic/converter.rs`
- Create: `backend/src/kiro_gateway/anthropic/stream.rs`
- Create: `backend/src/kiro_gateway/anthropic/websearch.rs`

- [ ] **Step 1: Add Anthropic module exports**

Add to `llm-access-kiro/src/lib.rs`:

```rust
pub mod anthropic;
```

Create `llm-access-kiro/src/anthropic/mod.rs`:

```rust
//! Anthropic-compatible Kiro request and stream conversion.

pub mod converter;
pub mod stream;
pub mod types;
pub mod websearch;
```

- [ ] **Step 2: Move converter files**

Run:

```bash
mkdir -p llm-access-kiro/src/anthropic
git mv backend/src/kiro_gateway/anthropic/types.rs llm-access-kiro/src/anthropic/types.rs
git mv backend/src/kiro_gateway/anthropic/converter.rs llm-access-kiro/src/anthropic/converter.rs
git mv backend/src/kiro_gateway/anthropic/stream.rs llm-access-kiro/src/anthropic/stream.rs
git mv backend/src/kiro_gateway/anthropic/websearch.rs llm-access-kiro/src/anthropic/websearch.rs
```

- [ ] **Step 3: Add backend re-export modules**

Create `backend/src/kiro_gateway/anthropic/types.rs`:

```rust
//! Re-exported Anthropic-compatible Kiro types.

pub(crate) use llm_access_kiro::anthropic::types::*;
```

Create `backend/src/kiro_gateway/anthropic/converter.rs`:

```rust
//! Re-exported Anthropic-compatible Kiro converter.

pub(crate) use llm_access_kiro::anthropic::converter::*;
```

Create `backend/src/kiro_gateway/anthropic/stream.rs`:

```rust
//! Re-exported Anthropic-compatible Kiro stream helpers.

pub(crate) use llm_access_kiro::anthropic::stream::*;
```

Create `backend/src/kiro_gateway/anthropic/websearch.rs`:

```rust
//! Re-exported Anthropic-compatible Kiro web-search helpers.

pub(crate) use llm_access_kiro::anthropic::websearch::*;
```

- [ ] **Step 4: Preserve key conversion tests**

Ensure moved converter tests cover these exact assertions:

- `rejects_messages_that_become_empty_after_filtering`: malformed messages reject locally with `invalid_request_error`.
- `ignores_whitespace_only_placeholder_blocks`: whitespace-only placeholder text or thinking blocks are ignored before upstream serialization.
- `preserves_thinking_effort_when_output_config_is_supplied`: user-provided thinking settings are not overwritten except by the documented model-name override path.

If the current test names differ, rename them during the move so these three names are present in `llm-access-kiro/src/anthropic/converter.rs`.

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-kiro --jobs 1 anthropic -- --nocapture
cargo test -p static-flow-backend --jobs 1 kiro_gateway::anthropic -- --nocapture
rustfmt llm-access-kiro/src/lib.rs llm-access-kiro/src/anthropic/*.rs backend/src/kiro_gateway/anthropic/types.rs backend/src/kiro_gateway/anthropic/converter.rs backend/src/kiro_gateway/anthropic/stream.rs backend/src/kiro_gateway/anthropic/websearch.rs
git add backend/src/kiro_gateway/anthropic llm-access-kiro
git commit -m "feat: extract kiro anthropic conversion"
```

Expected:

- Kiro converter and stream tests pass in the new crate.
- Backend tests pass through re-exported modules.

## Task 6: Extract Codex Request, Response, Models, And Instructions

**Files:**
- Modify: `Cargo.toml`
- Create: `llm-access-codex/Cargo.toml`
- Create: `llm-access-codex/src/lib.rs`
- Move: `backend/src/llm_gateway/request.rs` -> `llm-access-codex/src/request.rs`
- Move: `backend/src/llm_gateway/response.rs` -> `llm-access-codex/src/response.rs`
- Move: `backend/src/llm_gateway/models.rs` -> `llm-access-codex/src/models.rs`
- Move: `backend/src/llm_gateway/instructions.rs` -> `llm-access-codex/src/instructions.rs`
- Copy: `backend/src/llm_gateway/codex_default_instructions.md` -> `llm-access-codex/src/codex_default_instructions.md`
- Modify: `backend/src/llm_gateway/request.rs`
- Modify: `backend/src/llm_gateway/response.rs`
- Modify: `backend/src/llm_gateway/models.rs`
- Modify: `backend/src/llm_gateway/instructions.rs`
- Modify: `backend/Cargo.toml`

- [ ] **Step 1: Add Codex crate scaffold**

Create `llm-access-codex/Cargo.toml`:

```toml
[package]
name = "llm-access-codex"
version = "0.1.0"
edition = "2021"
publish = false

[lints]
workspace = true

[dependencies]
anyhow = { workspace = true }
async-stream = "0.3"
bytes = { workspace = true }
eventsource-stream = "0.2"
futures-util = "0.3"
llm-access-core = { path = "../llm-access-core" }
reqwest = { workspace = true, features = ["stream", "multipart"] }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
tokio = { workspace = true }
tokio-stream = { workspace = true }
tracing = { workspace = true }
```

Create `llm-access-codex/src/lib.rs`:

```rust
//! Codex/OpenAI-compatible runtime extracted for standalone LLM access.

pub mod instructions;
pub mod models;
pub mod request;
pub mod response;
```

Add `"llm-access-codex"` to workspace members and add this dependency to `backend/Cargo.toml`:

```toml
llm-access-codex = { path = "../llm-access-codex" }
```

- [ ] **Step 2: Move pure Codex files**

Run:

```bash
git mv backend/src/llm_gateway/request.rs llm-access-codex/src/request.rs
git mv backend/src/llm_gateway/response.rs llm-access-codex/src/response.rs
git mv backend/src/llm_gateway/models.rs llm-access-codex/src/models.rs
git mv backend/src/llm_gateway/instructions.rs llm-access-codex/src/instructions.rs
cp llm-access-codex/src/codex_default_instructions.md /tmp/staticflow-codex-default-instructions.md 2>/dev/null || true
cp backend/src/llm_gateway/codex_default_instructions.md llm-access-codex/src/codex_default_instructions.md
```

- [ ] **Step 3: Add backend re-export modules**

Create `backend/src/llm_gateway/request.rs`:

```rust
//! Re-exported Codex request normalization.

pub(crate) use llm_access_codex::request::*;
```

Create `backend/src/llm_gateway/response.rs`:

```rust
//! Re-exported Codex response conversion.

pub(crate) use llm_access_codex::response::*;
```

Create `backend/src/llm_gateway/models.rs`:

```rust
//! Re-exported Codex model catalog helpers.

pub(crate) use llm_access_codex::models::*;
```

Create `backend/src/llm_gateway/instructions.rs`:

```rust
//! Re-exported Codex default instructions.

pub(crate) use llm_access_codex::instructions::*;
```

- [ ] **Step 4: Fix crate-local imports**

In the moved Codex files, replace backend-local imports with `llm_access_core` types or local crate paths. Keep existing tests in the moved files. For public items consumed by backend route handlers, change `pub(crate)` to `pub`.

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-codex --jobs 1 -- --nocapture
cargo test -p static-flow-backend --jobs 1 llm_gateway::request -- --nocapture
cargo test -p static-flow-backend --jobs 1 llm_gateway::response -- --nocapture
rustfmt llm-access-codex/src/lib.rs llm-access-codex/src/request.rs llm-access-codex/src/response.rs llm-access-codex/src/models.rs llm-access-codex/src/instructions.rs backend/src/llm_gateway/request.rs backend/src/llm_gateway/response.rs backend/src/llm_gateway/models.rs backend/src/llm_gateway/instructions.rs
git add Cargo.toml backend/Cargo.toml backend/src/llm_gateway llm-access-codex
git commit -m "feat: extract codex request runtime"
```

Expected:

- Codex request/response/model tests pass in the new crate.
- Backend tests pass through re-exported modules.

## Task 7: Define Store Traits And Wire SQLite/DuckDB Adapters

**Files:**
- Modify: `llm-access-core/src/provider.rs`
- Modify: `llm-access-core/src/usage.rs`
- Create: `llm-access-core/src/store.rs`
- Modify: `llm-access-core/src/lib.rs`
- Modify: `llm-access-store/src/lib.rs`
- Create: `llm-access-store/src/repository.rs`

- [ ] **Step 1: Add async store trait contract**

Create `llm-access-core/src/store.rs`:

```rust
//! Storage traits consumed by provider runtimes.

use async_trait::async_trait;

use crate::usage::UsageEvent;

/// Key state used on the hot request path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedKey {
    /// Key id.
    pub key_id: String,
    /// Key display name.
    pub key_name: String,
    /// Provider type as snake_case string.
    pub provider_type: String,
    /// Protocol family as snake_case string.
    pub protocol_family: String,
    /// Key status.
    pub status: String,
    /// Billable quota limit.
    pub quota_billable_limit: i64,
    /// Billable usage already consumed.
    pub billable_tokens_used: i64,
}

/// Control-plane queries used by request handlers.
#[async_trait]
pub trait ControlStore: Send + Sync {
    /// Authenticate a bearer secret by hashing it and loading the key state.
    async fn authenticate_bearer_secret(&self, secret: &str) -> anyhow::Result<Option<AuthenticatedKey>>;

    /// Increment usage counters for a key after a usage event is accepted.
    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()>;
}

/// Analytics sink used by provider runtimes.
#[async_trait]
pub trait UsageEventSink: Send + Sync {
    /// Persist one usage event.
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()>;
}
```

Add to `llm-access-core/src/lib.rs`:

```rust
pub mod store;
```

- [ ] **Step 2: Add repository adapter skeleton**

Create `llm-access-store/src/repository.rs`:

```rust
//! Async repository adapters for llm-access runtime traits.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use llm_access_core::{
    store::{AuthenticatedKey, ControlStore, UsageEventSink},
    usage::UsageEvent,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tokio::task;

use crate::sqlite::SqliteControlStore;

/// Thread-safe SQLite control repository.
pub struct SqliteControlRepository {
    inner: Arc<Mutex<SqliteControlStore>>,
}

impl SqliteControlRepository {
    /// Create a repository from an opened SQLite connection.
    pub fn new(conn: Connection) -> Self {
        Self {
            inner: Arc::new(Mutex::new(SqliteControlStore::new(conn))),
        }
    }
}

fn hash_bearer_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[async_trait]
impl ControlStore for SqliteControlRepository {
    async fn authenticate_bearer_secret(&self, secret: &str) -> anyhow::Result<Option<AuthenticatedKey>> {
        let key_hash = hash_bearer_secret(secret);
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner.lock().expect("sqlite control store mutex poisoned");
            store.get_key_by_hash(&key_hash).map(|record| {
                record.map(|bundle| AuthenticatedKey {
                    key_id: bundle.key.key_id,
                    key_name: bundle.key.name,
                    provider_type: bundle.key.provider_type,
                    protocol_family: bundle.key.protocol_family,
                    status: bundle.key.status,
                    quota_billable_limit: bundle.key.quota_billable_limit,
                    billable_tokens_used: bundle.rollup.billable_tokens,
                })
            })
        })
        .await?
    }

    async fn apply_usage_rollup(&self, event: &UsageEvent) -> anyhow::Result<()> {
        let event = event.clone();
        let inner = Arc::clone(&self.inner);
        task::spawn_blocking(move || {
            let store = inner.lock().expect("sqlite control store mutex poisoned");
            store.increment_key_usage_rollup(&event)
        })
        .await?
    }
}

#[async_trait]
impl UsageEventSink for SqliteControlRepository {
    async fn append_usage_event(&self, event: &UsageEvent) -> anyhow::Result<()> {
        self.apply_usage_rollup(event).await
    }
}
```

- [ ] **Step 3: Add missing SQLite methods**

Add these methods to `SqliteControlStore`:

```rust
pub fn get_key_by_hash(&self, key_hash: &str) -> anyhow::Result<Option<KeyBundle>>;

pub fn increment_key_usage_rollup(&self, event: &llm_access_core::usage::UsageEvent) -> anyhow::Result<()>;
```

`get_key_by_hash` should use the same decode path as `get_key`, changing only the `WHERE k.key_hash = ?1` predicate. `increment_key_usage_rollup` should update `llm_key_usage_rollups` by adding token counters and setting `last_used_at_ms` to `event.created_at_ms`.

- [ ] **Step 4: Export adapters and dependencies**

Modify `llm-access-store/Cargo.toml`:

```toml
async-trait = { workspace = true }
llm-access-core = { path = "../llm-access-core" }
sha2 = { workspace = true }
tokio = { workspace = true }
```

Modify `llm-access-store/src/lib.rs`:

```rust
pub mod repository;
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-core --jobs 1
cargo test -p llm-access-store --jobs 1 repository sqlite -- --nocapture
rustfmt llm-access-core/src/store.rs llm-access-core/src/lib.rs llm-access-store/src/repository.rs llm-access-store/src/sqlite.rs llm-access-store/src/lib.rs
git add llm-access-core llm-access-store
git commit -m "feat: add llm access store traits"
```

Expected:

- Store trait crate compiles.
- SQLite repository tests pass.

## Task 8: Wire Real Provider Authentication Into `llm-access`

**Files:**
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/lib.rs`
- Modify: `llm-access/Cargo.toml`
- Test: `llm-access/src/provider.rs`

- [ ] **Step 1: Add provider auth tests**

Replace the current placeholder test in `llm-access/src/provider.rs` with:

```rust
#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use llm_access_core::store::{AuthenticatedKey, ControlStore};
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Default)]
    struct TestStore;

    #[async_trait]
    impl ControlStore for TestStore {
        async fn authenticate_bearer_secret(&self, secret: &str) -> anyhow::Result<Option<AuthenticatedKey>> {
            if secret == "valid-secret" {
                Ok(Some(AuthenticatedKey {
                    key_id: "key-1".to_string(),
                    key_name: "test-key".to_string(),
                    provider_type: "kiro".to_string(),
                    protocol_family: "anthropic".to_string(),
                    status: "active".to_string(),
                    quota_billable_limit: 100,
                    billable_tokens_used: 0,
                }))
            } else {
                Ok(None)
            }
        }

        async fn apply_usage_rollup(&self, _event: &llm_access_core::usage::UsageEvent) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn provider_entry_rejects_missing_bearer_token() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let request = Request::builder()
            .uri("/api/kiro-gateway/v1/messages")
            .body(Body::empty())
            .expect("request");

        let response = super::provider_entry(state, request).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn provider_entry_accepts_known_bearer_token_before_dispatch() {
        let state = super::ProviderState::new(Arc::new(TestStore));
        let request = Request::builder()
            .uri("/api/kiro-gateway/v1/messages")
            .header(header::AUTHORIZATION, "Bearer valid-secret")
            .body(Body::empty())
            .expect("request");

        let response = super::provider_entry(state, request).await;
        assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 2: Implement provider state and bearer extraction**

Replace `llm-access/src/provider.rs` with a handler that accepts `ProviderState` and `Request`. It should:

- return `401` for missing/malformed bearer token
- call `ControlStore::authenticate_bearer_secret`
- return `401` for unknown key
- return `403` for non-active key
- return `501 provider dispatch not wired` after successful auth until Kiro/Codex dispatch tasks land

The successful-auth response must not be `401`; the Task 8 test asserts that behavior.

- [ ] **Step 3: Wire runtime state into router**

Modify `llm-access/src/runtime.rs` to own an `Arc<dyn ControlStore>`.

Modify `llm-access/src/lib.rs` so `router()` accepts runtime state:

```rust
pub fn router(runtime: runtime::LlmAccessRuntime) -> Router {
    let provider_state = provider::ProviderState::new(runtime.control_store());
    Router::new()
        .route("/healthz", get(healthz))
        .route("/version", get(version))
        .route("/v1/chat/completions", post(provider::provider_entry))
        .route("/v1/responses", post(provider::provider_entry))
        .route("/v1/models", get(provider::provider_entry))
        .route("/cc/v1/messages", post(provider::provider_entry))
        .route("/api/llm-gateway/*path", any(provider::provider_entry))
        .route("/api/kiro-gateway/*path", any(provider::provider_entry))
        .route("/api/codex-gateway/*path", any(provider::provider_entry))
        .route("/api/llm-access/*path", any(provider::provider_entry))
        .with_state(provider_state)
}
```

- [ ] **Step 4: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access --jobs 1 provider -- --nocapture
rustfmt llm-access/src/provider.rs llm-access/src/runtime.rs llm-access/src/lib.rs
git add llm-access
git commit -m "feat: authenticate llm access provider requests"
```

Expected:

- Missing bearer token returns `401`.
- Known bearer token reaches dispatch and returns a non-`401` status.

## Task 9: Move Kiro Token, Account, Status, And Provider Runtime

**Files:**
- Move: `backend/src/kiro_gateway/auth_file.rs` -> `llm-access-kiro/src/auth_file.rs`
- Move: `backend/src/kiro_gateway/local_import.rs` -> `llm-access-kiro/src/local_import.rs`
- Move: `backend/src/kiro_gateway/machine_id.rs` -> `llm-access-kiro/src/machine_id.rs`
- Move: `backend/src/kiro_gateway/provider.rs` -> `llm-access-kiro/src/provider.rs`
- Move: `backend/src/kiro_gateway/runtime.rs` -> `llm-access-kiro/src/runtime.rs`
- Move: `backend/src/kiro_gateway/status_cache.rs` -> `llm-access-kiro/src/status_cache.rs`
- Move: `backend/src/kiro_gateway/token.rs` -> `llm-access-kiro/src/token.rs`
- Move: `backend/src/kiro_gateway/wire.rs` -> `llm-access-kiro/src/wire.rs`
- Modify: `llm-access-kiro/src/lib.rs`
- Modify: `backend/src/kiro_gateway/*.rs`

- [ ] **Step 1: Move runtime files**

Run:

```bash
git mv backend/src/kiro_gateway/auth_file.rs llm-access-kiro/src/auth_file.rs
git mv backend/src/kiro_gateway/local_import.rs llm-access-kiro/src/local_import.rs
git mv backend/src/kiro_gateway/machine_id.rs llm-access-kiro/src/machine_id.rs
git mv backend/src/kiro_gateway/provider.rs llm-access-kiro/src/provider.rs
git mv backend/src/kiro_gateway/runtime.rs llm-access-kiro/src/runtime.rs
git mv backend/src/kiro_gateway/status_cache.rs llm-access-kiro/src/status_cache.rs
git mv backend/src/kiro_gateway/token.rs llm-access-kiro/src/token.rs
git mv backend/src/kiro_gateway/wire.rs llm-access-kiro/src/wire.rs
```

- [ ] **Step 2: Export Kiro runtime modules**

Add to `llm-access-kiro/src/lib.rs`:

```rust
pub mod auth_file;
pub mod local_import;
pub mod machine_id;
pub mod provider;
pub mod runtime;
pub mod status_cache;
pub mod token;
pub mod wire;
```

- [ ] **Step 3: Add backend re-export modules**

Create these backend re-export modules:

```rust
// backend/src/kiro_gateway/auth_file.rs
//! Re-exported Kiro auth-file helpers from standalone LLM access.
pub(crate) use llm_access_kiro::auth_file::*;

// backend/src/kiro_gateway/local_import.rs
//! Re-exported Kiro local import helpers from standalone LLM access.
pub(crate) use llm_access_kiro::local_import::*;

// backend/src/kiro_gateway/machine_id.rs
//! Re-exported Kiro machine-id helpers from standalone LLM access.
pub(crate) use llm_access_kiro::machine_id::*;

// backend/src/kiro_gateway/provider.rs
//! Re-exported Kiro provider runtime from standalone LLM access.
pub(crate) use llm_access_kiro::provider::*;

// backend/src/kiro_gateway/runtime.rs
//! Re-exported Kiro runtime state from standalone LLM access.
pub(crate) use llm_access_kiro::runtime::*;

// backend/src/kiro_gateway/status_cache.rs
//! Re-exported Kiro status cache from standalone LLM access.
pub(crate) use llm_access_kiro::status_cache::*;

// backend/src/kiro_gateway/token.rs
//! Re-exported Kiro token manager from standalone LLM access.
pub(crate) use llm_access_kiro::token::*;

// backend/src/kiro_gateway/wire.rs
//! Re-exported Kiro wire protocol helpers from standalone LLM access.
pub(crate) use llm_access_kiro::wire::*;
```

- [ ] **Step 4: Introduce storage trait boundaries**

In `llm-access-kiro/src/runtime.rs`, replace direct `AppState` and LanceDB store dependencies with trait parameters from `llm-access-core::store` and SQLite repository methods. Runtime construction must receive:

```rust
pub struct KiroRuntimeDeps {
    pub control_store: std::sync::Arc<dyn llm_access_core::store::ControlStore>,
    pub usage_sink: std::sync::Arc<dyn llm_access_core::store::UsageEventSink>,
    pub auths_dir: std::path::PathBuf,
    pub http_client: reqwest::Client,
}
```

The backend adapter can wrap its existing `AppState` only during transition. `llm-access` must construct `KiroRuntimeDeps` from SQLite/DuckDB repositories.

- [ ] **Step 5: Preserve Kiro cooldown tests**

Run and keep passing the tests currently proving:

- per-account concurrency
- per-account start interval
- cooldown expiry
- account-scoped 5-minute quota cooldown
- shortest cooldown waiting behavior
- proxy cooldown snapshot

- [ ] **Step 6: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-kiro --jobs 1 runtime scheduler status_cache token provider -- --nocapture
cargo test -p static-flow-backend --jobs 1 kiro_gateway -- --nocapture
rustfmt llm-access-kiro/src/*.rs backend/src/kiro_gateway/*.rs
git add backend/src/kiro_gateway llm-access-kiro
git commit -m "feat: extract kiro provider runtime"
```

Expected:

- `llm-access-kiro` owns Kiro runtime behavior.
- Backend tests still pass through the extracted runtime.

## Task 10: Move Codex Account, Token, Runtime, And Support APIs

**Files:**
- Move: `backend/src/llm_gateway/accounts.rs` -> `llm-access-codex/src/accounts.rs`
- Move: `backend/src/llm_gateway/activity.rs` -> `llm-access-codex/src/activity.rs`
- Move: `backend/src/llm_gateway/runtime.rs` -> `llm-access-codex/src/runtime.rs`
- Move: `backend/src/llm_gateway/support.rs` -> `llm-access-codex/src/support.rs`
- Move: `backend/src/llm_gateway/token_refresh.rs` -> `llm-access-codex/src/token_refresh.rs`
- Move: `backend/src/llm_gateway/types.rs` -> `llm-access-codex/src/types.rs`
- Modify: `llm-access-codex/src/lib.rs`
- Modify: `backend/src/llm_gateway/*.rs`

- [ ] **Step 1: Move runtime files**

Run:

```bash
git mv backend/src/llm_gateway/accounts.rs llm-access-codex/src/accounts.rs
git mv backend/src/llm_gateway/activity.rs llm-access-codex/src/activity.rs
git mv backend/src/llm_gateway/runtime.rs llm-access-codex/src/runtime.rs
git mv backend/src/llm_gateway/support.rs llm-access-codex/src/support.rs
git mv backend/src/llm_gateway/token_refresh.rs llm-access-codex/src/token_refresh.rs
git mv backend/src/llm_gateway/types.rs llm-access-codex/src/types.rs
```

- [ ] **Step 2: Export Codex runtime modules**

Add to `llm-access-codex/src/lib.rs`:

```rust
pub mod accounts;
pub mod activity;
pub mod runtime;
pub mod support;
pub mod token_refresh;
pub mod types;
```

- [ ] **Step 3: Add backend re-export modules**

Create these backend re-export modules:

```rust
// backend/src/llm_gateway/accounts.rs
//! Re-exported Codex account runtime from standalone LLM access.
pub(crate) use llm_access_codex::accounts::*;

// backend/src/llm_gateway/activity.rs
//! Re-exported Codex activity tracker from standalone LLM access.
pub(crate) use llm_access_codex::activity::*;

// backend/src/llm_gateway/runtime.rs
//! Re-exported Codex runtime state from standalone LLM access.
pub(crate) use llm_access_codex::runtime::*;

// backend/src/llm_gateway/support.rs
//! Re-exported LLM support API helpers from standalone LLM access.
pub(crate) use llm_access_codex::support::*;

// backend/src/llm_gateway/token_refresh.rs
//! Re-exported Codex token refresh helpers from standalone LLM access.
pub(crate) use llm_access_codex::token_refresh::*;

// backend/src/llm_gateway/types.rs
//! Re-exported Codex public/admin API types from standalone LLM access.
pub(crate) use llm_access_codex::types::*;
```

- [ ] **Step 4: Introduce Codex runtime dependencies**

In `llm-access-codex/src/runtime.rs`, add:

```rust
pub struct CodexRuntimeDeps {
    pub control_store: std::sync::Arc<dyn llm_access_core::store::ControlStore>,
    pub usage_sink: std::sync::Arc<dyn llm_access_core::store::UsageEventSink>,
    pub auths_dir: std::path::PathBuf,
    pub http_client: reqwest::Client,
}
```

Refactor runtime construction so provider dispatch can run without `AppState` or LanceDB. Keep backend compatibility by implementing a backend adapter that supplies the same trait behavior from the existing store during transition.

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-codex --jobs 1 runtime accounts token_refresh -- --nocapture
cargo test -p static-flow-backend --jobs 1 llm_gateway -- --nocapture
rustfmt llm-access-codex/src/*.rs backend/src/llm_gateway/*.rs
git add backend/src/llm_gateway llm-access-codex
git commit -m "feat: extract codex provider runtime"
```

Expected:

- Codex runtime tests pass in the new crate.
- Backend LLM gateway tests pass through extracted modules.

## Task 11: Implement `llm-access` Public And Provider Routes

**Files:**
- Modify: `llm-access/src/lib.rs`
- Modify: `llm-access/src/provider.rs`
- Create: `llm-access/src/public_api.rs`
- Create: `llm-access/src/admin_api.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/Cargo.toml`

- [ ] **Step 1: Add route registration tests**

Add this test module to `llm-access/src/lib.rs`:

```rust
#[cfg(test)]
mod route_tests {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn health_route_still_works_without_auth() {
        let app = super::test_router();
        let response = app
            .oneshot(Request::builder().uri("/healthz").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn admin_llm_keys_route_is_registered() {
        let app = super::test_router();
        let response = app
            .oneshot(Request::builder().uri("/admin/llm-gateway/keys").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn kiro_messages_route_is_registered() {
        let app = super::test_router();
        let response = app
            .oneshot(Request::builder().method("POST").uri("/api/kiro-gateway/v1/messages").body(Body::empty()).expect("request"))
            .await
            .expect("response");
        assert_ne!(response.status(), StatusCode::NOT_FOUND);
    }
}
```

- [ ] **Step 2: Add public API module**

Create `llm-access/src/public_api.rs`:

```rust
//! Public LLM access API handlers.

use axum::{Json, response::IntoResponse};
use serde::Serialize;

/// Public access payload compatible with StaticFlow frontend expectations.
#[derive(Debug, Serialize)]
pub struct PublicAccessResponse {
    pub gateway_path: String,
    pub model_catalog_path: String,
}

pub async fn get_llm_access() -> impl IntoResponse {
    Json(PublicAccessResponse {
        gateway_path: "/api/llm-gateway/v1".to_string(),
        model_catalog_path: "/api/llm-gateway/model-catalog.json".to_string(),
    })
}

pub async fn get_kiro_access() -> impl IntoResponse {
    Json(PublicAccessResponse {
        gateway_path: "/api/kiro-gateway".to_string(),
        model_catalog_path: "/api/kiro-gateway/v1/models".to_string(),
    })
}
```

- [ ] **Step 3: Add admin API module**

Create `llm-access/src/admin_api.rs`:

```rust
//! Admin LLM access API handlers.

use axum::{Json, response::IntoResponse};
use serde::Serialize;

/// Minimal status payload used while full admin handlers are wired.
#[derive(Debug, Serialize)]
pub struct AdminRouteStatus {
    pub service: &'static str,
    pub route: &'static str,
}

pub async fn route_registered(route: &'static str) -> impl IntoResponse {
    Json(AdminRouteStatus {
        service: "llm-access",
        route,
    })
}
```

- [ ] **Step 4: Register all compatibility routes**

In `llm-access/src/lib.rs`, register every route from `llm-access-core::routes::PUBLIC_PROVIDER_ROUTES` and `ADMIN_ROUTES` with the real handler where available. During this task, routes that are not fully implemented may return `501` or a route status payload, but they must not be `404`.

Provider routes should dispatch through:

```rust
post(provider::provider_entry)
```

Public access routes should dispatch through:

```rust
get(public_api::get_llm_access)
get(public_api::get_kiro_access)
```

- [ ] **Step 5: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access --jobs 1 route_tests -- --nocapture
rustfmt llm-access/src/lib.rs llm-access/src/provider.rs llm-access/src/public_api.rs llm-access/src/admin_api.rs llm-access/src/runtime.rs
git add llm-access
git commit -m "feat: register llm access compatibility routes"
```

Expected:

- All route registration tests pass.
- Compatibility routes return non-404 responses.

## Task 12: Replace Stub Routes With Full Admin Handlers

**Files:**
- Modify: `llm-access/src/admin_api.rs`
- Modify: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/repository.rs`
- Test: `llm-access/src/admin_api.rs`

- [ ] **Step 1: Add admin round-trip tests**

Add tests that perform:

- `POST /admin/llm-gateway/keys` creates a key.
- `GET /admin/llm-gateway/keys` lists the key.
- `PATCH /admin/kiro-gateway/keys/:key_id` updates `route_strategy`, `fixed_account_name`, `auto_account_names`, `model_name_map`, `kiro_cache_estimation_enabled`, and `kiro_zero_cache_debug_enabled`.
- `GET /admin/kiro-gateway/keys` returns the updated fields.

Use the current frontend field names from `frontend/src/api.rs` and `frontend/src/pages/admin_kiro_gateway.rs`; do not invent alternate names.

- [ ] **Step 2: Implement key/group/config/account handlers**

Move handler behavior from `backend/src/llm_gateway.rs` and `backend/src/kiro_gateway/mod.rs` into `llm-access/src/admin_api.rs`, replacing direct LanceDB calls with `llm-access-store` repository calls.

Required handler groups:

- runtime config
- proxy configs
- proxy bindings
- account groups
- keys
- usage list and detail
- token requests
- account contribution requests
- sponsor requests
- Codex accounts
- Kiro account statuses
- Kiro accounts
- Kiro balance refresh

- [ ] **Step 3: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access --jobs 1 admin_api -- --nocapture
cargo test -p llm-access-store --jobs 1 sqlite repository -- --nocapture
rustfmt llm-access/src/admin_api.rs llm-access-store/src/sqlite.rs llm-access-store/src/repository.rs
git add llm-access llm-access-store
git commit -m "feat: implement llm access admin api"
```

Expected:

- Admin key/config/account routes round-trip through SQLite.
- Kiro admin key fields reload with the same field names the frontend expects.

## Task 13: Wire Kiro Provider Dispatch Into `llm-access`

**Files:**
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access-kiro/src/provider.rs`
- Modify: `llm-access-kiro/src/runtime.rs`
- Test: `llm-access-kiro/src/provider.rs`

- [ ] **Step 1: Add fake-upstream Kiro integration tests**

Use `wiremock` to add tests in `llm-access-kiro/src/provider.rs` for:

- successful `/api/kiro-gateway/v1/messages` streaming request
- successful `/api/kiro-gateway/cc/v1/messages` buffered request
- upstream 429 with `5-minute credit limit exceeded` marks only that account cooldown
- upstream 500 retries another eligible account
- malformed public request returns local `400 invalid_request_error`

- [ ] **Step 2: Implement Kiro dispatch**

In `llm-access/src/provider.rs`, branch authenticated requests:

```rust
match (key.provider_type.as_str(), request.uri().path()) {
    ("kiro", path) if path.starts_with("/api/kiro-gateway/") || path.starts_with("/cc/v1/") => {
        runtime.kiro().handle(request, key).await
    }
    ("codex", _) => {
        runtime.codex().handle(request, key).await
    }
    _ => unsupported_provider_response(),
}
```

Implement `KiroRuntime::handle()` in `llm-access-kiro` by reusing the extracted converter, scheduler, token manager, provider dispatch, stream conversion, usage conversion, and usage sink.

- [ ] **Step 3: Preserve `/cc/v1/messages` context-usage buffering**

Add an assertion in the fake-upstream test that the downstream usage event for `/cc/v1/messages` uses the context-usage input tokens after the upstream context usage event arrives, not a pre-stream estimate.

- [ ] **Step 4: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-kiro --jobs 1 provider -- --nocapture
cargo test -p llm-access --jobs 1 provider -- --nocapture
rustfmt llm-access/src/provider.rs llm-access/src/runtime.rs llm-access-kiro/src/provider.rs llm-access-kiro/src/runtime.rs
git add llm-access llm-access-kiro
git commit -m "feat: dispatch kiro requests from llm access"
```

Expected:

- Kiro fake-upstream tests pass.
- `llm-access` provider tests pass.

## Task 14: Wire Codex Provider Dispatch Into `llm-access`

**Files:**
- Modify: `llm-access/src/provider.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access-codex/src/runtime.rs`
- Test: `llm-access-codex/src/runtime.rs`

- [ ] **Step 1: Add fake-upstream Codex integration tests**

Use `wiremock` to add tests for:

- `/api/llm-gateway/v1/responses` success
- `/api/llm-gateway/v1/chat/completions` request normalization
- `/api/llm-gateway/v1/models` key-scoped model output
- malformed messages reject locally with logged error
- account unavailable means no account is selected even if auth files exist

- [ ] **Step 2: Implement Codex dispatch**

Wire `CodexRuntime::handle()` through extracted request normalization, account pool, token refresh, upstream request, response/SSE conversion, and usage sink.

- [ ] **Step 3: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-codex --jobs 1 runtime -- --nocapture
cargo test -p llm-access --jobs 1 provider -- --nocapture
rustfmt llm-access/src/provider.rs llm-access/src/runtime.rs llm-access-codex/src/runtime.rs
git add llm-access llm-access-codex
git commit -m "feat: dispatch codex requests from llm access"
```

Expected:

- Codex fake-upstream tests pass.
- `llm-access` provider dispatch supports both Codex and Kiro keys.

## Task 15: Complete Snapshot Import And CDC Replay

**Files:**
- Modify: `llm-access-migrator/src/lib.rs`
- Modify: `llm-access-migrator/src/snapshot.rs`
- Create: `llm-access-migrator/src/import.rs`
- Modify: `llm-access-migrator/Cargo.toml`
- Test: `llm-access-migrator/src/import.rs`

- [ ] **Step 1: Add import tests**

Create `llm-access-migrator/src/import.rs` tests that:

- import one key, route config, runtime config, account group, proxy config, proxy binding, token request, account contribution request, sponsor request
- import one Kiro usage event into DuckDB
- replay a delete event idempotently
- preserve CDC high-water mark

- [ ] **Step 2: Implement import module**

Implement functions:

```rust
pub fn import_snapshot_manifest(
    manifest_path: &std::path::Path,
    sqlite: &llm_access_store::sqlite::SqliteControlStore,
    duckdb: Option<&llm_access_store::duckdb::DuckDbUsageWriter>,
) -> anyhow::Result<()>;

pub fn replay_cdc_batch(
    events: &[llm_access_migrator::CdcEvent],
    sqlite: &llm_access_store::sqlite::SqliteControlStore,
    duckdb: Option<&llm_access_store::duckdb::DuckDbUsageWriter>,
) -> anyhow::Result<usize>;
```

Each replayed event must be idempotent by `event_id`. Usage events go to DuckDB and update SQLite rollups.

- [ ] **Step 3: Verify and commit**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-migrator --jobs 1 import -- --nocapture
rustfmt llm-access-migrator/src/lib.rs llm-access-migrator/src/snapshot.rs llm-access-migrator/src/import.rs
git add llm-access-migrator
git commit -m "feat: import llm access snapshots"
```

Expected:

- Snapshot import and CDC replay tests pass.

## Task 16: Frontend Compatibility Verification

**Files:**
- Modify only if required: `frontend/src/api.rs`
- Modify only if required: `frontend/src/pages/admin_llm_gateway.rs`
- Modify only if required: `frontend/src/pages/admin_kiro_gateway.rs`
- Create: `scripts/verify_llm_access_frontend_compat.sh`

- [ ] **Step 1: Add compatibility script**

Create `scripts/verify_llm_access_frontend_compat.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${1:-http://127.0.0.1:19080}"

curl -fsS "${BASE_URL}/api/llm-gateway/access" >/dev/null
curl -fsS "${BASE_URL}/api/kiro-gateway/access" >/dev/null
curl -fsS "${BASE_URL}/admin/llm-gateway/keys" >/dev/null
curl -fsS "${BASE_URL}/admin/kiro-gateway/keys" >/dev/null
curl -fsS "${BASE_URL}/admin/kiro-gateway/accounts/statuses" >/dev/null
```

Make it executable:

```bash
chmod +x scripts/verify_llm_access_frontend_compat.sh
```

- [ ] **Step 2: Run frontend API checks**

Run only after confirming no Rust build is active:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p static-flow-frontend --jobs 1 api:: -- --nocapture
```

Expected:

- Existing frontend API tests compile against unchanged response structs.

- [ ] **Step 3: Commit script and any required compatibility fixes**

Run:

```bash
git add scripts/verify_llm_access_frontend_compat.sh frontend/src/api.rs frontend/src/pages/admin_llm_gateway.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "test: add llm access frontend compatibility checks"
```

Expected:

- If frontend files did not need changes, only the script is committed.

## Task 17: Local End-To-End Verification Without Touching Production

**Files:**
- Create: `scripts/start_llm_access_full_parity_local.sh`
- Create: `scripts/verify_llm_access_full_parity_local.sh`
- Modify: `docs/superpowers/specs/2026-04-30-llm-access-full-parity-design.md`

- [ ] **Step 1: Add local start script**

Create `scripts/start_llm_access_full_parity_local.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT="/mnt/wsl/data4tb/static-flow-data/llm-access-local"
BIN="${LLM_ACCESS_BIN:-target/debug/llm-access}"

exec "${BIN}" serve \
  --bind 127.0.0.1:19080 \
  --state-root "${ROOT}" \
  --sqlite-control "${ROOT}/control/llm-access.sqlite3" \
  --duckdb "${ROOT}/analytics/usage.duckdb"
```

- [ ] **Step 2: Add local verification script**

Create `scripts/verify_llm_access_full_parity_local.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${1:-http://127.0.0.1:19080}"

curl -fsS "${BASE_URL}/healthz" | grep -q '"status":"ok"'
curl -fsS "${BASE_URL}/version" | grep -q '"service":"llm-access"'
curl -fsS "${BASE_URL}/api/llm-gateway/access" >/dev/null
curl -fsS "${BASE_URL}/api/kiro-gateway/access" >/dev/null
curl -fsS "${BASE_URL}/admin/llm-gateway/keys" >/dev/null
curl -fsS "${BASE_URL}/admin/kiro-gateway/keys" >/dev/null
```

Make scripts executable:

```bash
chmod +x scripts/start_llm_access_full_parity_local.sh scripts/verify_llm_access_full_parity_local.sh
```

- [ ] **Step 3: Run final local checks**

Run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\+\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
cargo test -p llm-access-core --jobs 1
cargo test -p llm-access-store --jobs 1
cargo test -p llm-access-kiro --jobs 1
cargo test -p llm-access-codex --jobs 1
cargo test -p llm-access --jobs 1
cargo test -p llm-access-migrator --jobs 1
cargo clippy -p llm-access-core -p llm-access-store -p llm-access-kiro -p llm-access-codex -p llm-access -p llm-access-migrator --jobs 1 -- -D warnings
```

Expected:

- All listed tests pass.
- Clippy reports zero warnings for affected crates.

- [ ] **Step 4: Update design status and commit**

Add a status section to `docs/superpowers/specs/2026-04-30-llm-access-full-parity-design.md`:

```markdown
## Implementation Status

- Full parity crates are implemented.
- SQLite control-plane and DuckDB analytics storage are wired.
- Kiro and Codex provider dispatch run from `llm-access`.
- Existing frontend LLM admin/public APIs are compatible.
- Gateway/Caddy traffic splitting remains a separate deployment step.
```

Run:

```bash
git add scripts/start_llm_access_full_parity_local.sh scripts/verify_llm_access_full_parity_local.sh docs/superpowers/specs/2026-04-30-llm-access-full-parity-design.md
git commit -m "test: verify llm access full parity locally"
```

Expected:

- Local full-parity verification scripts are committed.
- Design status reflects completed local implementation.

## Task 18: Re-enable Canary And Cloud Plans

**Files:**
- Modify: `docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md`
- Modify: `docs/superpowers/plans/2026-04-30-pingora-llm-routing-local-canary.md`
- Modify: `docs/superpowers/specs/2026-04-30-llm-access-cloud-migration-design.md`
- Modify: `docs/superpowers/plans/2026-04-30-llm-access-cloud-migration.md`

- [ ] **Step 1: Update deferred status**

Replace the deferred warning in each document with:

```markdown
> Status update: full-parity `llm-access` local implementation is complete.
> This canary or cloud migration plan may proceed after the user explicitly
> approves traffic routing changes.
```

- [ ] **Step 2: Verify no production services were touched**

Run:

```bash
git diff --check
git status --short
```

Expected:

- Only the four plan/spec files are modified.
- No runtime service files or generated artifacts are dirty.

- [ ] **Step 3: Commit**

Run:

```bash
git add docs/superpowers/specs/2026-04-30-pingora-llm-routing-local-canary-design.md docs/superpowers/plans/2026-04-30-pingora-llm-routing-local-canary.md docs/superpowers/specs/2026-04-30-llm-access-cloud-migration-design.md docs/superpowers/plans/2026-04-30-llm-access-cloud-migration.md
git commit -m "docs: unblock llm access canary planning"
```

Expected:

- Canary and cloud migration plans remain separate from implementation.
- Traffic routing still requires explicit user approval.
