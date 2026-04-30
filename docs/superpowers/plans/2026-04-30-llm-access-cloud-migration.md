# LLM Access Cloud Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move all high-frequency LLM access traffic from the home upstream path to a cloud `llm-access` service while keeping the rest of StaticFlow local.

**Architecture:** Keep the existing public Caddy entrypoint, split only LLM paths to a cloud-local `llm-access` process, and leave all non-LLM paths on the existing pb-mapper path back to local StaticFlow. `llm-access` stores runtime control state in SQLite, append-heavy usage analytics in DuckDB, and keeps all state under a JuiceFS mount so the VM can be replaced without losing service data.

**Tech Stack:** Rust/Axum, SQLite via `rusqlite`, DuckDB with feature-gated runtime support, StaticFlow `LlmGatewayStore` source CDC, Caddy, systemd, JuiceFS, targeted `cargo test`, `cargo clippy --jobs 1`, and per-file `rustfmt`.

---

## File Structure

- Modify: `llm-access/src/lib.rs`
  - Replace placeholder handlers with a configurable service runtime, mount guard, provider router, and health/version endpoints.
- Create: `llm-access/src/config.rs`
  - Parse CLI/env configuration for bind address, SQLite path, DuckDB path, auth roots, mount root, and source CDC path.
- Create: `llm-access/src/runtime.rs`
  - Own `LlmAccessRuntime`, HTTP clients, account pools, store handles, and startup validation.
- Create: `llm-access/src/routes.rs`
  - Register OpenAI-compatible, Claude-compatible, health, version, and admin/control routes.
- Create: `llm-access/src/provider.rs`
  - Host request-forwarding logic copied or extracted from existing backend LLM gateway modules.
- Create: `llm-access/src/usage.rs`
  - Convert provider completion metadata into SQLite rollups and DuckDB usage rows.
- Modify: `llm-access/Cargo.toml`
  - Add dependencies needed by the extracted provider runtime.
- Modify: `llm-access-store/src/lib.rs`
  - Add typed SQLite repositories and optional DuckDB initialization/writer entrypoints.
- Create: `llm-access-store/src/sqlite.rs`
  - Implement SQLite control-plane reads/writes.
- Create: `llm-access-store/src/duckdb.rs`
  - Implement DuckDB usage writer behind the existing `duckdb-runtime` feature.
- Modify: `llm-access-migrator/src/lib.rs`
  - Extend replay from key-only to all LLM entities.
- Create: `llm-access-migrator/src/snapshot.rs`
  - Export source LanceDB LLM state and import it into target SQLite/DuckDB.
- Create: `llm-access-migrator/src/duckdb_usage.rs`
  - Convert source usage events into target DuckDB usage rows.
- Modify: `llm-access-migrator/Cargo.toml`
  - Add dependencies required for snapshot export from existing StaticFlow shared store.
- Create: `deployment-examples/systemd/llm-access.service.template`
  - Cloud systemd service template gated on the JuiceFS mount.
- Create: `deployment-examples/systemd/llm-access-juicefs.mount.template`
  - Example mount unit for `/mnt/llm-access`.
- Create: `deployment-examples/caddy/llm-access-path-split.Caddyfile`
  - Caddy route split example that sends LLM paths to `127.0.0.1:19080`.
- Create: `scripts/render_llm_access_cloud_bundle.sh`
  - Render deployment templates into a staging directory.
- Create: `scripts/test_llm_access_cloud_bundle.sh`
  - Validate rendered Caddy/systemd templates without touching live services.
- Modify: `docs/llm-access-cdc-storage-design.zh.md`
  - Update current-state notes after provider runtime, snapshot import, and deployment artifacts land.

## Safety Rules For Execution

- Do not restart local `sf-gateway`, local backend slots, cloud Caddy, cloud pb-mapper, or any live backend service during implementation tasks.
- Do not deploy or cut traffic until the canary task explicitly says to do so and the user approves.
- Before every Rust build/check, run:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\\+\\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
```

- If another Rust build/link process is active, wait or ask before starting another one.
- Use `--jobs 1` for every cargo command.
- Do not run `cargo fmt` at workspace root. Format only touched Rust files with `rustfmt path/to/file.rs`.
- Keep each task in a separate commit unless the user asks for a different commit strategy.

---

### Task 1: Lock Down `llm-access` Configuration And Mount Safety

**Files:**
- Modify: `llm-access/src/lib.rs`
- Create: `llm-access/src/config.rs`
- Create: `llm-access/src/runtime.rs`
- Modify: `llm-access/Cargo.toml`
- Test: `llm-access/src/config.rs`
- Test: `llm-access/src/runtime.rs`

- [ ] **Step 1: Write failing tests for config parsing**

Create `llm-access/src/config.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn parses_serve_config_with_state_root_and_duckdb_path() {
        let command = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--bind",
            "127.0.0.1:19080",
            "--state-root",
            "/mnt/llm-access",
            "--sqlite-control",
            "/mnt/llm-access/control/llm-access.sqlite3",
            "--duckdb",
            "/mnt/llm-access/analytics/usage.duckdb",
        ])
        .expect("parse serve command");

        let super::CliCommand::Serve(config) = command else {
            panic!("expected serve command");
        };

        assert_eq!(config.bind_addr.to_string(), "127.0.0.1:19080");
        assert_eq!(config.storage.state_root, PathBuf::from("/mnt/llm-access"));
        assert_eq!(
            config.storage.sqlite_control,
            PathBuf::from("/mnt/llm-access/control/llm-access.sqlite3")
        );
        assert_eq!(
            config.storage.duckdb,
            PathBuf::from("/mnt/llm-access/analytics/usage.duckdb")
        );
        assert_eq!(
            config.storage.kiro_auths_dir,
            PathBuf::from("/mnt/llm-access/auths/kiro")
        );
        assert_eq!(
            config.storage.codex_auths_dir,
            PathBuf::from("/mnt/llm-access/auths/codex")
        );
    }

    #[test]
    fn rejects_state_paths_outside_state_root() {
        let err = super::CliCommand::parse([
            "llm-access",
            "serve",
            "--state-root",
            "/mnt/llm-access",
            "--sqlite-control",
            "/tmp/llm-access.sqlite3",
            "--duckdb",
            "/mnt/llm-access/analytics/usage.duckdb",
        ])
        .expect_err("sqlite outside state root must fail");

        assert!(err.to_string().contains("must live under --state-root"));
    }
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test -p llm-access --jobs 1 parses_serve_config_with_state_root_and_duckdb_path -- --nocapture
cargo test -p llm-access --jobs 1 rejects_state_paths_outside_state_root -- --nocapture
```

Expected:

- The tests fail because `llm-access/src/config.rs` is not wired into the crate and the new config fields do not exist.

- [ ] **Step 3: Implement the config module**

Replace CLI/config ownership in `llm-access/src/lib.rs` by moving it into `llm-access/src/config.rs`.

Add this shape to `llm-access/src/config.rs`:

```rust
use std::{ffi::OsString, net::SocketAddr, path::{Path, PathBuf}};

use anyhow::{anyhow, Context};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    pub state_root: PathBuf,
    pub sqlite_control: PathBuf,
    pub duckdb: PathBuf,
    pub kiro_auths_dir: PathBuf,
    pub codex_auths_dir: PathBuf,
    pub cdc_dir: PathBuf,
    pub logs_dir: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServeConfig {
    pub bind_addr: SocketAddr,
    pub storage: StorageConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliCommand {
    Init(StorageConfig),
    Serve(ServeConfig),
}

impl CliCommand {
    pub fn parse<I, S>(args: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        let mut args = args.into_iter().map(Into::into);
        let _program = args.next();
        let command = args.next().ok_or_else(usage_error)?;
        match command.to_string_lossy().as_ref() {
            "init" => Ok(Self::Init(parse_storage_args(args)?)),
            "serve" => {
                let (bind_addr, storage) = parse_serve_args(args)?;
                Ok(Self::Serve(ServeConfig { bind_addr, storage }))
            },
            _ => Err(usage_error()),
        }
    }
}

fn parse_serve_args<I>(args: I) -> anyhow::Result<(SocketAddr, StorageConfig)>
where
    I: IntoIterator<Item = OsString>,
{
    let mut bind_addr = None;
    let mut rest = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--bind" => {
                let value = args.next().ok_or_else(|| anyhow!("--bind requires an address"))?;
                bind_addr = Some(
                    value
                        .to_string_lossy()
                        .parse()
                        .context("failed to parse --bind address")?,
                );
            },
            _ => rest.push(arg),
        }
    }
    Ok((
        bind_addr.unwrap_or_else(|| "127.0.0.1:19080".parse().expect("valid bind addr")),
        parse_storage_args(rest)?,
    ))
}

fn parse_storage_args<I>(args: I) -> anyhow::Result<StorageConfig>
where
    I: IntoIterator<Item = OsString>,
{
    let mut state_root = None;
    let mut sqlite_control = None;
    let mut duckdb = None;
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--state-root" => {
                state_root = Some(PathBuf::from(
                    args.next().ok_or_else(|| anyhow!("--state-root requires a path"))?,
                ));
            },
            "--sqlite-control" => {
                sqlite_control = Some(PathBuf::from(
                    args.next().ok_or_else(|| anyhow!("--sqlite-control requires a path"))?,
                ));
            },
            "--duckdb" => {
                duckdb = Some(PathBuf::from(
                    args.next().ok_or_else(|| anyhow!("--duckdb requires a path"))?,
                ));
            },
            _ => return Err(usage_error()),
        }
    }
    let state_root = state_root.ok_or_else(usage_error)?;
    let sqlite_control = sqlite_control.ok_or_else(usage_error)?;
    let duckdb = duckdb.ok_or_else(usage_error)?;
    ensure_under_root(&state_root, &sqlite_control)?;
    ensure_under_root(&state_root, &duckdb)?;
    Ok(StorageConfig {
        kiro_auths_dir: state_root.join("auths/kiro"),
        codex_auths_dir: state_root.join("auths/codex"),
        cdc_dir: state_root.join("cdc"),
        logs_dir: state_root.join("logs"),
        state_root,
        sqlite_control,
        duckdb,
    })
}

fn ensure_under_root(root: &Path, path: &Path) -> anyhow::Result<()> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(anyhow!(
            "`{}` must live under --state-root `{}`",
            path.display(),
            root.display()
        ))
    }
}

fn usage_error() -> anyhow::Error {
    anyhow!(
        "usage: llm-access init --state-root <path> --sqlite-control <path> --duckdb <path>\nusage: \
         llm-access serve [--bind <addr>] --state-root <path> --sqlite-control <path> --duckdb <path>"
    )
}
```

Update `llm-access/src/lib.rs` to declare and use the module:

```rust
pub mod config;
pub mod runtime;

use config::{CliCommand, ServeConfig, StorageConfig};
```

- [ ] **Step 4: Add mount/state root validation**

Create `llm-access/src/runtime.rs`:

```rust
use anyhow::{anyhow, Context};

use crate::config::StorageConfig;

pub fn validate_state_root(config: &StorageConfig) -> anyhow::Result<()> {
    let metadata = std::fs::metadata(&config.state_root)
        .with_context(|| format!("state root `{}` is not accessible", config.state_root.display()))?;
    if !metadata.is_dir() {
        return Err(anyhow!("state root `{}` is not a directory", config.state_root.display()));
    }
    for dir in [
        &config.kiro_auths_dir,
        &config.codex_auths_dir,
        &config.cdc_dir,
        &config.logs_dir,
    ] {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("failed to create `{}`", dir.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    #[test]
    fn validate_state_root_creates_expected_subdirectories() {
        let root = std::env::temp_dir().join(format!(
            "llm-access-state-root-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create root");
        let config = crate::config::StorageConfig {
            state_root: root.clone(),
            sqlite_control: root.join("control/llm-access.sqlite3"),
            duckdb: root.join("analytics/usage.duckdb"),
            kiro_auths_dir: root.join("auths/kiro"),
            codex_auths_dir: root.join("auths/codex"),
            cdc_dir: root.join("cdc"),
            logs_dir: root.join("logs"),
        };

        super::validate_state_root(&config).expect("validate root");

        assert!(PathBuf::from(&config.kiro_auths_dir).is_dir());
        assert!(PathBuf::from(&config.codex_auths_dir).is_dir());
        assert!(PathBuf::from(&config.cdc_dir).is_dir());
        assert!(PathBuf::from(&config.logs_dir).is_dir());
        std::fs::remove_dir_all(&root).expect("cleanup");
    }
}
```

- [ ] **Step 5: Wire bootstrap through the mount guard**

Update `bootstrap_storage` in `llm-access/src/lib.rs`:

```rust
pub fn bootstrap_storage(config: &StorageConfig) -> anyhow::Result<()> {
    runtime::validate_state_root(config)?;
    llm_access_store::initialize_sqlite_target_path(&config.sqlite_control)?;
    llm_access_store::write_duckdb_schema_file(config.duckdb.with_extension("schema.sql"))?;
    Ok(())
}
```

- [ ] **Step 6: Run tests and format touched files**

Run:

```bash
rustfmt llm-access/src/lib.rs llm-access/src/config.rs llm-access/src/runtime.rs
cargo test -p llm-access --jobs 1 -- --nocapture
cargo clippy -p llm-access --jobs 1 -- -D warnings
```

Expected:

- All `llm-access` tests pass.
- Clippy reports zero warnings.

- [ ] **Step 7: Commit**

```bash
git add llm-access/src/lib.rs llm-access/src/config.rs llm-access/src/runtime.rs llm-access/Cargo.toml Cargo.lock
git commit -m "feat: add llm access runtime config guard"
```

---

### Task 2: Add Typed SQLite Repositories For Control-Plane State

**Files:**
- Create: `llm-access-store/src/sqlite.rs`
- Modify: `llm-access-store/src/lib.rs`
- Test: `llm-access-store/src/sqlite.rs`

- [ ] **Step 1: Write failing repository tests**

Create `llm-access-store/src/sqlite.rs` with tests first:

```rust
#[cfg(test)]
mod tests {
    #[test]
    fn key_repository_round_trips_key_route_and_rollup() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        let key = super::KeyRecord {
            key_id: "key-1".to_string(),
            name: "primary".to_string(),
            secret: "sk-test".to_string(),
            key_hash: "hash".to_string(),
            status: "active".to_string(),
            provider_type: "kiro".to_string(),
            protocol_family: "anthropic".to_string(),
            public_visible: true,
            quota_billable_limit: 1000,
            created_at_ms: 10,
            updated_at_ms: 20,
        };
        let route = super::KeyRouteConfig {
            key_id: "key-1".to_string(),
            route_strategy: Some("auto".to_string()),
            fixed_account_name: None,
            auto_account_names_json: Some(r#"["a","b"]"#.to_string()),
            account_group_id: Some("group-1".to_string()),
            model_name_map_json: None,
        };
        let rollup = super::KeyUsageRollup {
            key_id: "key-1".to_string(),
            input_uncached_tokens: 11,
            input_cached_tokens: 22,
            output_tokens: 33,
            billable_tokens: 44,
            credit_total: 55.5,
            credit_missing_events: 1,
            last_used_at_ms: Some(30),
            updated_at_ms: 40,
        };

        repo.upsert_key_bundle(&key, &route, &rollup).expect("upsert key");
        let loaded = repo.get_key("key-1").expect("load key").expect("key exists");

        assert_eq!(loaded.key.name, "primary");
        assert_eq!(loaded.route.account_group_id.as_deref(), Some("group-1"));
        assert_eq!(loaded.rollup.output_tokens, 33);
    }

    #[test]
    fn runtime_config_repository_upserts_single_named_record() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");
        crate::initialize_sqlite_target(&conn).expect("init schema");
        let repo = super::SqliteControlStore::new(conn);

        repo.upsert_runtime_config("default", r#"{"codex_client_version":"0.124.0"}"#, 100)
            .expect("upsert config");
        repo.upsert_runtime_config("default", r#"{"codex_client_version":"0.125.0"}"#, 200)
            .expect("upsert config");

        let value = repo.get_runtime_config_json("default").expect("load config");
        assert_eq!(value.as_deref(), Some(r#"{"codex_client_version":"0.125.0"}"#));
    }
}
```

- [ ] **Step 2: Run focused tests and verify they fail**

Run:

```bash
cargo test -p llm-access-store --jobs 1 key_repository_round_trips_key_route_and_rollup -- --nocapture
cargo test -p llm-access-store --jobs 1 runtime_config_repository_upserts_single_named_record -- --nocapture
```

Expected:

- The tests fail because `SqliteControlStore`, `KeyRecord`, `KeyRouteConfig`, and `KeyUsageRollup` are not implemented.

- [ ] **Step 3: Implement typed records and repository methods**

Add to `llm-access-store/src/sqlite.rs`:

```rust
use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};

pub struct SqliteControlStore {
    conn: Connection,
}

pub struct KeyBundle {
    pub key: KeyRecord,
    pub route: KeyRouteConfig,
    pub rollup: KeyUsageRollup,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyRecord {
    pub key_id: String,
    pub name: String,
    pub secret: String,
    pub key_hash: String,
    pub status: String,
    pub provider_type: String,
    pub protocol_family: String,
    pub public_visible: bool,
    pub quota_billable_limit: i64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyRouteConfig {
    pub key_id: String,
    pub route_strategy: Option<String>,
    pub fixed_account_name: Option<String>,
    pub auto_account_names_json: Option<String>,
    pub account_group_id: Option<String>,
    pub model_name_map_json: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyUsageRollup {
    pub key_id: String,
    pub input_uncached_tokens: i64,
    pub input_cached_tokens: i64,
    pub output_tokens: i64,
    pub billable_tokens: i64,
    pub credit_total: f64,
    pub credit_missing_events: i64,
    pub last_used_at_ms: Option<i64>,
    pub updated_at_ms: i64,
}

impl SqliteControlStore {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn upsert_key_bundle(
        &self,
        key: &KeyRecord,
        route: &KeyRouteConfig,
        rollup: &KeyUsageRollup,
    ) -> anyhow::Result<()> {
        let tx = self.conn.unchecked_transaction().context("begin key bundle tx")?;
        tx.execute(
            "INSERT INTO llm_keys (
                key_id, name, secret, key_hash, status, provider_type, protocol_family,
                public_visible, quota_billable_limit, created_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(key_id) DO UPDATE SET
                name = excluded.name,
                secret = excluded.secret,
                key_hash = excluded.key_hash,
                status = excluded.status,
                provider_type = excluded.provider_type,
                protocol_family = excluded.protocol_family,
                public_visible = excluded.public_visible,
                quota_billable_limit = excluded.quota_billable_limit,
                created_at_ms = excluded.created_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![
                key.key_id,
                key.name,
                key.secret,
                key.key_hash,
                key.status,
                key.provider_type,
                key.protocol_family,
                key.public_visible as i64,
                key.quota_billable_limit,
                key.created_at_ms,
                key.updated_at_ms,
            ],
        )
        .context("upsert llm key")?;
        tx.execute(
            "INSERT INTO llm_key_route_config (
                key_id, route_strategy, fixed_account_name, auto_account_names_json,
                account_group_id, model_name_map_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(key_id) DO UPDATE SET
                route_strategy = excluded.route_strategy,
                fixed_account_name = excluded.fixed_account_name,
                auto_account_names_json = excluded.auto_account_names_json,
                account_group_id = excluded.account_group_id,
                model_name_map_json = excluded.model_name_map_json",
            params![
                route.key_id,
                route.route_strategy,
                route.fixed_account_name,
                route.auto_account_names_json,
                route.account_group_id,
                route.model_name_map_json,
            ],
        )
        .context("upsert key route config")?;
        tx.execute(
            "INSERT INTO llm_key_usage_rollups (
                key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_total, credit_missing_events, last_used_at_ms, updated_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(key_id) DO UPDATE SET
                input_uncached_tokens = excluded.input_uncached_tokens,
                input_cached_tokens = excluded.input_cached_tokens,
                output_tokens = excluded.output_tokens,
                billable_tokens = excluded.billable_tokens,
                credit_total = excluded.credit_total,
                credit_missing_events = excluded.credit_missing_events,
                last_used_at_ms = excluded.last_used_at_ms,
                updated_at_ms = excluded.updated_at_ms",
            params![
                rollup.key_id,
                rollup.input_uncached_tokens,
                rollup.input_cached_tokens,
                rollup.output_tokens,
                rollup.billable_tokens,
                rollup.credit_total.to_string(),
                rollup.credit_missing_events,
                rollup.last_used_at_ms,
                rollup.updated_at_ms,
            ],
        )
        .context("upsert key usage rollup")?;
        tx.commit().context("commit key bundle tx")?;
        Ok(())
    }

    pub fn get_key(&self, key_id: &str) -> anyhow::Result<Option<KeyBundle>> {
        self.conn
            .query_row(
                "SELECT
                    k.key_id, k.name, k.secret, k.key_hash, k.status, k.provider_type,
                    k.protocol_family, k.public_visible, k.quota_billable_limit,
                    k.created_at_ms, k.updated_at_ms,
                    r.route_strategy, r.fixed_account_name, r.auto_account_names_json,
                    r.account_group_id, r.model_name_map_json,
                    u.input_uncached_tokens, u.input_cached_tokens, u.output_tokens,
                    u.billable_tokens, u.credit_total, u.credit_missing_events,
                    u.last_used_at_ms, u.updated_at_ms
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 LEFT JOIN llm_key_usage_rollups u ON u.key_id = k.key_id
                 WHERE k.key_id = ?1",
                [key_id],
                |row| {
                    let key_id: String = row.get(0)?;
                    Ok(KeyBundle {
                        key: KeyRecord {
                            key_id: key_id.clone(),
                            name: row.get(1)?,
                            secret: row.get(2)?,
                            key_hash: row.get(3)?,
                            status: row.get(4)?,
                            provider_type: row.get(5)?,
                            protocol_family: row.get(6)?,
                            public_visible: row.get::<_, i64>(7)? != 0,
                            quota_billable_limit: row.get(8)?,
                            created_at_ms: row.get(9)?,
                            updated_at_ms: row.get(10)?,
                        },
                        route: KeyRouteConfig {
                            key_id: key_id.clone(),
                            route_strategy: row.get(11)?,
                            fixed_account_name: row.get(12)?,
                            auto_account_names_json: row.get(13)?,
                            account_group_id: row.get(14)?,
                            model_name_map_json: row.get(15)?,
                        },
                        rollup: KeyUsageRollup {
                            key_id,
                            input_uncached_tokens: row.get(16)?,
                            input_cached_tokens: row.get(17)?,
                            output_tokens: row.get(18)?,
                            billable_tokens: row.get(19)?,
                            credit_total: row.get::<_, String>(20)?.parse().unwrap_or(0.0),
                            credit_missing_events: row.get(21)?,
                            last_used_at_ms: row.get(22)?,
                            updated_at_ms: row.get(23)?,
                        },
                    })
                },
            )
            .optional()
            .context("load key bundle")
    }

    pub fn upsert_runtime_config(
        &self,
        name: &str,
        config_json: &str,
        updated_at_ms: i64,
    ) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO llm_runtime_config (name, config_json, updated_at_ms)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
                config_json = excluded.config_json,
                updated_at_ms = excluded.updated_at_ms",
            params![name, config_json, updated_at_ms],
        )?;
        Ok(())
    }

    pub fn get_runtime_config_json(&self, name: &str) -> anyhow::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT config_json FROM llm_runtime_config WHERE name = ?1",
                [name],
                |row| row.get(0),
            )
            .optional()
            .context("load runtime config")
    }
}
```

Update `llm-access-store/src/lib.rs`:

```rust
pub mod sqlite;
```

- [ ] **Step 4: Run tests and format touched files**

```bash
rustfmt llm-access-store/src/lib.rs llm-access-store/src/sqlite.rs
cargo test -p llm-access-store --jobs 1 -- --nocapture
cargo clippy -p llm-access-store --jobs 1 -- -D warnings
```

Expected:

- All `llm-access-store` tests pass.
- Clippy reports zero warnings.

- [ ] **Step 5: Commit**

```bash
git add llm-access-store/src/lib.rs llm-access-store/src/sqlite.rs
git commit -m "feat: add llm access sqlite control store"
```

---

### Task 3: Add DuckDB Usage Writer Behind A Feature Gate

**Files:**
- Create: `llm-access-store/src/duckdb.rs`
- Modify: `llm-access-store/src/lib.rs`
- Modify: `llm-access-store/Cargo.toml`
- Test: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Write failing DuckDB schema-level tests that do not compile DuckDB by default**

Create `llm-access-store/src/duckdb.rs`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct UsageEventRow {
    pub event_id: String,
    pub request_id: String,
    pub created_at_ms: i64,
    pub key_id: String,
    pub key_name: String,
    pub provider_type: String,
    pub protocol_family: String,
    pub account_name: Option<String>,
    pub account_group_id_at_event: Option<String>,
    pub route_strategy_at_event: Option<String>,
    pub endpoint: String,
    pub model: String,
    pub mapped_model: Option<String>,
    pub status_code: i64,
    pub stream: bool,
    pub upstream_headers_ms: Option<i64>,
    pub first_sse_ms: Option<i64>,
    pub stream_finish_ms: Option<i64>,
    pub input_uncached_tokens: i64,
    pub input_cached_tokens: i64,
    pub output_tokens: i64,
    pub billable_tokens: i64,
    pub credit_total: Option<f64>,
}

pub fn insert_usage_event_sql() -> &'static str {
    "INSERT INTO usage_events (
        event_id, request_id, created_at_ms, key_id, key_name, provider_type,
        protocol_family, account_name, account_group_id_at_event, route_strategy_at_event,
        endpoint, model, mapped_model, status_code, stream, upstream_headers_ms,
        first_sse_ms, stream_finish_ms, input_uncached_tokens, input_cached_tokens,
        output_tokens, billable_tokens, credit_total
     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
}

#[cfg(test)]
mod tests {
    #[test]
    fn usage_insert_sql_targets_wide_fact_table_without_runtime_joins() {
        let sql = super::insert_usage_event_sql();

        assert!(sql.starts_with("INSERT INTO usage_events"));
        assert!(sql.contains("key_name"));
        assert!(sql.contains("account_group_id_at_event"));
        assert!(sql.contains("route_strategy_at_event"));
        assert!(!sql.to_ascii_lowercase().contains(" join "));
    }
}
```

- [ ] **Step 2: Run the default feature test and verify it passes without DuckDB runtime**

Run:

```bash
cargo test -p llm-access-store --jobs 1 usage_insert_sql_targets_wide_fact_table_without_runtime_joins -- --nocapture
```

Expected:

- The test passes without enabling `duckdb-runtime`.

- [ ] **Step 3: Add the optional runtime writer**

Append to `llm-access-store/src/duckdb.rs`:

```rust
#[cfg(feature = "duckdb-runtime")]
pub struct DuckDbUsageWriter {
    conn: duckdb::Connection,
}

#[cfg(feature = "duckdb-runtime")]
impl DuckDbUsageWriter {
    pub fn new(conn: duckdb::Connection) -> anyhow::Result<Self> {
        crate::initialize_duckdb_target(&conn)?;
        Ok(Self { conn })
    }

    pub fn insert_usage_event(&self, row: &UsageEventRow) -> anyhow::Result<()> {
        self.conn.execute(
            insert_usage_event_sql(),
            duckdb::params![
                row.event_id,
                row.request_id,
                row.created_at_ms,
                row.key_id,
                row.key_name,
                row.provider_type,
                row.protocol_family,
                row.account_name,
                row.account_group_id_at_event,
                row.route_strategy_at_event,
                row.endpoint,
                row.model,
                row.mapped_model,
                row.status_code,
                row.stream,
                row.upstream_headers_ms,
                row.first_sse_ms,
                row.stream_finish_ms,
                row.input_uncached_tokens,
                row.input_cached_tokens,
                row.output_tokens,
                row.billable_tokens,
                row.credit_total.map(|value| value.to_string()),
            ],
        )?;
        Ok(())
    }
}
```

Update `llm-access-store/src/lib.rs`:

```rust
pub mod duckdb;
```

- [ ] **Step 4: Run tests and clippy without DuckDB runtime**

```bash
rustfmt llm-access-store/src/lib.rs llm-access-store/src/duckdb.rs
cargo test -p llm-access-store --jobs 1 -- --nocapture
cargo clippy -p llm-access-store --jobs 1 -- -D warnings
```

Expected:

- Default feature tests pass.
- DuckDB runtime is not compiled in this command.

- [ ] **Step 5: Commit**

```bash
git add llm-access-store/src/lib.rs llm-access-store/src/duckdb.rs llm-access-store/Cargo.toml Cargo.lock
git commit -m "feat: add llm access duckdb usage writer"
```

---

### Task 4: Extend CDC Replay To All Control-Plane Entities

**Files:**
- Modify: `llm-access-migrator/src/lib.rs`
- Test: `llm-access-migrator/src/lib.rs`

- [ ] **Step 1: Add failing replay tests for runtime config, account group, proxy config, and request queues**

Add tests to `llm-access-migrator/src/lib.rs`:

```rust
#[test]
fn replays_runtime_config_and_account_group_events() {
    let source = rusqlite::Connection::open_in_memory().expect("source");
    let target = rusqlite::Connection::open_in_memory().expect("target");
    source.execute_batch(include_str!("../../shared/src/llm_gateway_store/cdc_outbox.sql"))
        .expect("source schema");
    llm_access_store::initialize_sqlite_target(&target).expect("target schema");

    insert_source_event(
        &source,
        1,
        "runtime-config-1",
        "runtime_config",
        "upsert",
        "default",
        r#"{"name":"default","config_json":"{\"codex_client_version\":\"0.124.0\"}","updated_at":100}"#,
    );
    insert_source_event(
        &source,
        2,
        "group-1",
        "account_group",
        "upsert",
        "group-a",
        r#"{"id":"group-a","name":"Group A","provider_type":"kiro","enabled":true,"updated_at":200}"#,
    );

    let stats = replay_source_outbox_to_sqlite_target(
        &source,
        &target,
        &ReplayOptions { consumer_name: "test", max_events: 10 },
    )
    .expect("replay");

    assert_eq!(stats.applied_events, 2);
    let runtime_count: i64 = target
        .query_row("SELECT count(*) FROM llm_runtime_config", [], |row| row.get(0))
        .expect("runtime count");
    let group_count: i64 = target
        .query_row("SELECT count(*) FROM llm_account_groups", [], |row| row.get(0))
        .expect("group count");
    assert_eq!(runtime_count, 1);
    assert_eq!(group_count, 1);
}
```

- [ ] **Step 2: Run focused test and verify it fails**

```bash
cargo test -p llm-access-migrator --jobs 1 replays_runtime_config_and_account_group_events -- --nocapture
```

Expected:

- The test fails with `unsupported replay event`.

- [ ] **Step 3: Implement entity-specific apply functions**

Add match arms in `apply_event`:

```rust
("runtime_config", "upsert") => apply_runtime_config_upsert(conn, event),
("account_group", "upsert") => apply_account_group_upsert(conn, event),
("account_group", "delete") => delete_by_id(conn, "llm_account_groups", "group_id", &event.primary_key),
("proxy_config", "upsert") => apply_proxy_config_upsert(conn, event),
("proxy_config", "delete") => delete_by_id(conn, "llm_proxy_configs", "proxy_id", &event.primary_key),
("proxy_binding", "upsert") => apply_proxy_binding_upsert(conn, event),
("proxy_binding", "delete") => delete_by_id(conn, "llm_proxy_bindings", "binding_id", &event.primary_key),
("token_request", "upsert") => apply_json_request_upsert(conn, "llm_token_requests", "request_id", event),
("account_contribution_request", "upsert") => {
    apply_json_request_upsert(conn, "llm_account_contribution_requests", "request_id", event)
},
("gpt2api_account_contribution_request", "upsert") => {
    apply_json_request_upsert(conn, "gpt2api_account_contribution_requests", "request_id", event)
},
("sponsor_request", "upsert") => {
    apply_json_request_upsert(conn, "llm_sponsor_requests", "request_id", event)
},
("sponsor_request", "delete") => {
    delete_by_id(conn, "llm_sponsor_requests", "request_id", &event.primary_key)
},
```

Implement the helpers with explicit SQL per target table. Keep the helper table names private and only call them from hard-coded match arms:

```rust
fn delete_by_id(conn: &Connection, table: &str, column: &str, value: &str) -> Result<()> {
    let sql = match (table, column) {
        ("llm_account_groups", "group_id") => "DELETE FROM llm_account_groups WHERE group_id = ?1",
        ("llm_proxy_configs", "proxy_id") => "DELETE FROM llm_proxy_configs WHERE proxy_id = ?1",
        ("llm_proxy_bindings", "binding_id") => "DELETE FROM llm_proxy_bindings WHERE binding_id = ?1",
        ("llm_sponsor_requests", "request_id") => "DELETE FROM llm_sponsor_requests WHERE request_id = ?1",
        _ => bail!("unsupported delete target table={table} column={column}"),
    };
    conn.execute(sql, [value]).map(|_| ()).context("delete replayed row")
}
```

- [ ] **Step 4: Run migrator tests**

```bash
rustfmt llm-access-migrator/src/lib.rs
cargo test -p llm-access-migrator --jobs 1 -- --nocapture
cargo clippy -p llm-access-migrator --jobs 1 -- -D warnings
```

Expected:

- Key replay tests still pass.
- New control-plane replay tests pass.
- Unsupported entities still fail loudly instead of being silently skipped.

- [ ] **Step 5: Commit**

```bash
git add llm-access-migrator/src/lib.rs
git commit -m "feat: replay llm access control cdc events"
```

---

### Task 5: Add Snapshot Export And Import

**Files:**
- Create: `llm-access-migrator/src/snapshot.rs`
- Modify: `llm-access-migrator/src/lib.rs`
- Modify: `llm-access-migrator/Cargo.toml`
- Test: `llm-access-migrator/src/snapshot.rs`

- [ ] **Step 1: Add snapshot manifest tests**

Create `llm-access-migrator/src/snapshot.rs` with tests first:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SnapshotManifest {
    pub source_db_path: String,
    pub exported_at_ms: i64,
    pub cdc_high_water_seq: i64,
    pub keys: usize,
    pub usage_events: usize,
}

pub fn manifest_json(manifest: &SnapshotManifest) -> anyhow::Result<String> {
    Ok(serde_json::to_string_pretty(manifest)?)
}

#[cfg(test)]
mod tests {
    #[test]
    fn snapshot_manifest_records_cdc_high_water_mark() {
        let manifest = super::SnapshotManifest {
            source_db_path: "/data/lancedb".to_string(),
            exported_at_ms: 1000,
            cdc_high_water_seq: 42,
            keys: 3,
            usage_events: 9,
        };

        let json = super::manifest_json(&manifest).expect("manifest json");

        assert!(json.contains("\"cdc_high_water_seq\": 42"));
        assert!(json.contains("\"keys\": 3"));
        assert!(json.contains("\"usage_events\": 9"));
    }
}
```

- [ ] **Step 2: Run the manifest test**

```bash
cargo test -p llm-access-migrator --jobs 1 snapshot_manifest_records_cdc_high_water_mark -- --nocapture
```

Expected:

- The test passes after adding the initial manifest-only code.

- [ ] **Step 3: Add export/import entrypoints with explicit contracts**

Add to `llm-access-migrator/src/snapshot.rs`:

```rust
use std::path::Path;

pub struct SnapshotExportOptions<'a> {
    pub source_lancedb_path: &'a Path,
    pub source_cdc_sqlite_path: &'a Path,
    pub output_dir: &'a Path,
}

pub struct SnapshotImportOptions<'a> {
    pub snapshot_dir: &'a Path,
    pub target_sqlite_path: &'a Path,
    pub target_duckdb_path: &'a Path,
}

pub fn export_snapshot(options: &SnapshotExportOptions<'_>) -> anyhow::Result<SnapshotManifest> {
    std::fs::create_dir_all(options.output_dir)?;
    let cdc_high_water_seq = read_source_cdc_high_water_seq(options.source_cdc_sqlite_path)?;
    let manifest = SnapshotManifest {
        source_db_path: options.source_lancedb_path.display().to_string(),
        exported_at_ms: unix_ms(),
        cdc_high_water_seq,
        keys: 0,
        usage_events: 0,
    };
    std::fs::write(options.output_dir.join("manifest.json"), manifest_json(&manifest)?)?;
    Ok(manifest)
}

pub fn import_snapshot(options: &SnapshotImportOptions<'_>) -> anyhow::Result<SnapshotManifest> {
    let manifest_path = options.snapshot_dir.join("manifest.json");
    let manifest: SnapshotManifest =
        serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
    llm_access_store::initialize_sqlite_target_path(options.target_sqlite_path)?;
    llm_access_store::write_duckdb_schema_file(options.target_duckdb_path.with_extension("schema.sql"))?;
    Ok(manifest)
}

fn read_source_cdc_high_water_seq(path: &Path) -> anyhow::Result<i64> {
    let conn = rusqlite::Connection::open(path)?;
    let seq = conn
        .query_row("SELECT COALESCE(MAX(seq), 0) FROM cdc_outbox", [], |row| row.get(0))
        .unwrap_or(0);
    Ok(seq)
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
```

Expose the module from `llm-access-migrator/src/lib.rs`:

```rust
pub mod snapshot;
```

Add dependencies in `llm-access-migrator/Cargo.toml`:

```toml
serde = { workspace = true, features = ["derive"] }
```

- [ ] **Step 4: Extend export to real LanceDB LLM tables**

Use existing shared store APIs instead of direct LanceDB file parsing. Add a focused source reader in `snapshot.rs` that calls `static-flow-shared` LLM store APIs for:

```text
llm_gateway_keys
llm_gateway_runtime_config
llm_gateway_account_groups
llm_gateway_proxy_configs
llm_gateway_proxy_bindings
llm_gateway_token_requests
llm_gateway_account_contribution_requests
gpt2api_account_contribution_requests
llm_gateway_sponsor_requests
llm_gateway_usage_events
```

Write snapshot files as JSONL under:

```text
snapshot/keys.jsonl
snapshot/runtime_config.jsonl
snapshot/account_groups.jsonl
snapshot/proxy_configs.jsonl
snapshot/proxy_bindings.jsonl
snapshot/token_requests.jsonl
snapshot/account_contribution_requests.jsonl
snapshot/gpt2api_account_contribution_requests.jsonl
snapshot/sponsor_requests.jsonl
snapshot/usage_events.jsonl
```

Do not add ad hoc string parsing of LanceDB files.

- [ ] **Step 5: Run tests and clippy**

```bash
rustfmt llm-access-migrator/src/lib.rs llm-access-migrator/src/snapshot.rs
cargo test -p llm-access-migrator --jobs 1 -- --nocapture
cargo clippy -p llm-access-migrator --jobs 1 -- -D warnings
```

Expected:

- Snapshot manifest tests pass.
- Existing replay tests pass.
- Clippy reports zero warnings.

- [ ] **Step 6: Commit**

```bash
git add llm-access-migrator/src/lib.rs llm-access-migrator/src/snapshot.rs llm-access-migrator/Cargo.toml Cargo.lock
git commit -m "feat: add llm access snapshot manifest"
```

---

### Task 6: Wire Provider Runtime Into `llm-access`

**Files:**
- Create: `llm-access/src/routes.rs`
- Create: `llm-access/src/provider.rs`
- Create: `llm-access/src/usage.rs`
- Modify: `llm-access/src/lib.rs`
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/Cargo.toml`
- Test: `llm-access/src/routes.rs`
- Test: `llm-access/src/provider.rs`

- [ ] **Step 1: Write route ownership tests**

Create `llm-access/src/routes.rs`:

```rust
pub fn is_llm_access_path(path: &str) -> bool {
    path == "/healthz"
        || path == "/version"
        || path.starts_with("/v1/")
        || path.starts_with("/cc/v1/")
        || path.starts_with("/api/llm-gateway/")
        || path.starts_with("/api/kiro-gateway/")
        || path.starts_with("/api/codex-gateway/")
        || path.starts_with("/api/llm-access/")
}

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

- [ ] **Step 2: Run route tests**

```bash
cargo test -p llm-access --jobs 1 recognizes_public_llm_provider_paths -- --nocapture
cargo test -p llm-access --jobs 1 leaves_non_llm_staticflow_paths_on_local_backend -- --nocapture
```

Expected:

- Tests pass after the new route classifier is added.

- [ ] **Step 3: Add explicit 401 behavior for missing API keys**

Create `llm-access/src/provider.rs`:

```rust
use axum::{http::StatusCode, response::{IntoResponse, Response}};

pub async fn provider_entry() -> Response {
    (StatusCode::UNAUTHORIZED, "missing bearer token").into_response()
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn provider_entry_rejects_missing_bearer_token() {
        let response = super::provider_entry().await;

        assert_eq!(response.status(), axum::http::StatusCode::UNAUTHORIZED);
    }
}
```

- [ ] **Step 4: Register provider routes**

Update `llm-access/src/lib.rs`:

```rust
pub mod routes;
pub mod provider;
pub mod usage;
```

Replace placeholder `not_implemented` route registrations with:

```rust
.route("/v1/chat/completions", post(provider::provider_entry))
.route("/v1/responses", post(provider::provider_entry))
.route("/v1/models", get(provider::provider_entry))
.route("/cc/v1/messages", post(provider::provider_entry))
.route("/api/llm-gateway/*path", post(provider::provider_entry).get(provider::provider_entry))
.route("/api/kiro-gateway/*path", post(provider::provider_entry).get(provider::provider_entry))
.route("/api/codex-gateway/*path", post(provider::provider_entry).get(provider::provider_entry))
.route("/api/llm-access/*path", post(provider::provider_entry).get(provider::provider_entry))
```

- [ ] **Step 5: Extract existing backend provider logic in small pieces**

Move or copy provider-only logic from these backend files into `llm-access/src/provider.rs` and helper modules, keeping backend-owned article/music/comment code out:

```text
backend/src/llm_gateway/request.rs
backend/src/llm_gateway/response.rs
backend/src/llm_gateway/models.rs
backend/src/llm_gateway/accounts.rs
backend/src/llm_gateway/runtime.rs
backend/src/llm_gateway/token_refresh.rs
backend/src/kiro_gateway/provider.rs
backend/src/kiro_gateway/runtime.rs
backend/src/kiro_gateway/scheduler.rs
backend/src/kiro_gateway/token.rs
backend/src/kiro_gateway/wire.rs
backend/src/upstream_proxy.rs
backend/src/geoip.rs
```

The first working provider target is:

```text
POST /v1/chat/completions
POST /v1/responses
POST /cc/v1/messages
GET  /v1/models
```

Keep unsupported legacy paths returning 501 until their extracted runtime is tested.

- [ ] **Step 6: Run route/provider tests**

```bash
rustfmt llm-access/src/lib.rs llm-access/src/routes.rs llm-access/src/provider.rs llm-access/src/usage.rs llm-access/src/runtime.rs
cargo test -p llm-access --jobs 1 -- --nocapture
cargo clippy -p llm-access --jobs 1 -- -D warnings
```

Expected:

- New route ownership tests pass.
- Missing-token test passes.
- Extracted provider tests from backend pass in the new crate before Caddy canary.

- [ ] **Step 7: Commit**

```bash
git add llm-access/src llm-access/Cargo.toml Cargo.lock
git commit -m "feat: wire llm access provider routes"
```

---

### Task 7: Add Deployment Templates For JuiceFS, systemd, And Caddy Split

**Files:**
- Create: `deployment-examples/systemd/llm-access.service.template`
- Create: `deployment-examples/systemd/llm-access-juicefs.mount.template`
- Create: `deployment-examples/caddy/llm-access-path-split.Caddyfile`
- Create: `scripts/render_llm_access_cloud_bundle.sh`
- Create: `scripts/test_llm_access_cloud_bundle.sh`

- [ ] **Step 1: Write the template validation script**

Create `scripts/test_llm_access_cloud_bundle.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:-"$ROOT_DIR/tmp/llm-access-cloud-bundle-test"}"

rm -rf "$OUT_DIR"
"$ROOT_DIR/scripts/render_llm_access_cloud_bundle.sh" "$OUT_DIR"

test -s "$OUT_DIR/llm-access.service"
test -s "$OUT_DIR/mnt-llm\\x2daccess.mount"
test -s "$OUT_DIR/Caddyfile"

grep -F 'ExecStart=/usr/local/bin/llm-access serve' "$OUT_DIR/llm-access.service"
grep -F 'RequiresMountsFor=/mnt/llm-access' "$OUT_DIR/llm-access.service"
grep -F 'handle_path /v1/*' "$OUT_DIR/Caddyfile"
grep -F 'reverse_proxy 127.0.0.1:19080' "$OUT_DIR/Caddyfile"
grep -F 'reverse_proxy 127.0.0.1:39080' "$OUT_DIR/Caddyfile"
```

- [ ] **Step 2: Run the validation script and verify it fails**

```bash
bash scripts/test_llm_access_cloud_bundle.sh
```

Expected:

- It fails because the render script and templates do not exist yet.

- [ ] **Step 3: Add systemd service template**

Create `deployment-examples/systemd/llm-access.service.template`:

```ini
[Unit]
Description=StaticFlow LLM Access Service
After=network-online.target mnt-llm\x2daccess.mount
Wants=network-online.target
RequiresMountsFor=/mnt/llm-access

[Service]
Type=simple
User=llm-access
Group=llm-access
EnvironmentFile=/etc/llm-access/llm-access.env
WorkingDirectory=/mnt/llm-access
ExecStart=/usr/local/bin/llm-access serve --bind ${LLM_ACCESS_BIND_ADDR} --state-root ${LLM_ACCESS_STATE_ROOT} --sqlite-control ${LLM_ACCESS_SQLITE_CONTROL} --duckdb ${LLM_ACCESS_DUCKDB}
Restart=always
RestartSec=3
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=full
ReadWritePaths=/mnt/llm-access

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 4: Add JuiceFS mount template**

Create `deployment-examples/systemd/llm-access-juicefs.mount.template`:

```ini
[Unit]
Description=JuiceFS mount for llm-access state
After=network-online.target
Wants=network-online.target

[Mount]
What=${JUICEFS_META_URL}
Where=/mnt/llm-access
Type=juicefs
Options=_netdev,allow_other,cache-dir=/var/cache/juicefs/llm-access,writeback,attr-cache=1,entry-cache=1,dir-entry-cache=1

[Install]
WantedBy=multi-user.target
```

- [ ] **Step 5: Add Caddy path split template**

Create `deployment-examples/caddy/llm-access-path-split.Caddyfile`:

```caddyfile
ackingliu.top, www.ackingliu.top {
        @health path /_caddy_health
        respond @health "ok" 200

        @admin path /admin*
        respond @admin "forbidden" 403

        handle_path /v1/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle_path /cc/v1/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle_path /api/llm-gateway/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle_path /api/kiro-gateway/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle_path /api/codex-gateway/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle_path /api/llm-access/* {
                reverse_proxy 127.0.0.1:19080
        }

        handle {
                reverse_proxy 127.0.0.1:39080 {
                        header_up X-Real-IP {remote_host}
                        header_up X-Forwarded-For {remote_host}
                        header_up X-Forwarded-Proto {scheme}
                        header_up X-Forwarded-Host {host}
                }
        }
}
```

- [ ] **Step 6: Add render script**

Create `scripts/render_llm_access_cloud_bundle.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${1:?usage: scripts/render_llm_access_cloud_bundle.sh <out-dir>}"

mkdir -p "$OUT_DIR"
cp "$ROOT_DIR/deployment-examples/systemd/llm-access.service.template" "$OUT_DIR/llm-access.service"
cp "$ROOT_DIR/deployment-examples/systemd/llm-access-juicefs.mount.template" "$OUT_DIR/mnt-llm\\x2daccess.mount"
cp "$ROOT_DIR/deployment-examples/caddy/llm-access-path-split.Caddyfile" "$OUT_DIR/Caddyfile"
```

- [ ] **Step 7: Run validation**

```bash
chmod +x scripts/render_llm_access_cloud_bundle.sh scripts/test_llm_access_cloud_bundle.sh
bash scripts/test_llm_access_cloud_bundle.sh
```

Expected:

- The script exits 0.
- Rendered Caddy includes LLM routes to `19080` and fallback to `39080`.

- [ ] **Step 8: Commit**

```bash
git add deployment-examples/systemd/llm-access.service.template deployment-examples/systemd/llm-access-juicefs.mount.template deployment-examples/caddy/llm-access-path-split.Caddyfile scripts/render_llm_access_cloud_bundle.sh scripts/test_llm_access_cloud_bundle.sh
git commit -m "feat: add llm access cloud deployment templates"
```

---

### Task 8: Add Canary Verification Script

**Files:**
- Create: `scripts/verify_llm_access_cloud_canary.sh`
- Test: `scripts/verify_llm_access_cloud_canary.sh`

- [ ] **Step 1: Add a read-only verification script**

Create `scripts/verify_llm_access_cloud_canary.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

BASE_URL="${BASE_URL:-https://ackingliu.top}"
LLM_HEALTH_URL="${LLM_HEALTH_URL:-$BASE_URL/healthz}"
STATICFLOW_HEALTH_URL="${STATICFLOW_HEALTH_URL:-$BASE_URL/api/healthz}"

curl_common=(
  -o /dev/null
  -sS
  -w 'code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n'
)

echo "[llm-access] health"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$LLM_HEALTH_URL"

echo "[staticflow] health"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$STATICFLOW_HEALTH_URL"

echo "[routing] non-llm article API should still be reachable"
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl "${curl_common[@]}" "$BASE_URL/api/articles"
```

- [ ] **Step 2: Validate script syntax without running live cutover**

```bash
bash -n scripts/verify_llm_access_cloud_canary.sh
chmod +x scripts/verify_llm_access_cloud_canary.sh
```

Expected:

- Shell syntax validation passes.

- [ ] **Step 3: Commit**

```bash
git add scripts/verify_llm_access_cloud_canary.sh
git commit -m "test: add llm access cloud canary checks"
```

---

### Task 9: Update Documentation And Run Final Local Verification

**Files:**
- Modify: `docs/llm-access-cdc-storage-design.zh.md`
- Modify: `docs/superpowers/specs/2026-04-30-llm-access-cloud-migration-design.md`

- [ ] **Step 1: Update docs with current implementation status**

Update `docs/llm-access-cdc-storage-design.zh.md` so the "当前工具入口" section describes:

```text
llm-access init --state-root /mnt/llm-access --sqlite-control /mnt/llm-access/control/llm-access.sqlite3 --duckdb /mnt/llm-access/analytics/usage.duckdb
llm-access serve --bind 127.0.0.1:19080 --state-root /mnt/llm-access --sqlite-control /mnt/llm-access/control/llm-access.sqlite3 --duckdb /mnt/llm-access/analytics/usage.duckdb
```

Also add a note that deployment templates exist under:

```text
deployment-examples/systemd/llm-access.service.template
deployment-examples/systemd/llm-access-juicefs.mount.template
deployment-examples/caddy/llm-access-path-split.Caddyfile
```

- [ ] **Step 2: Run final local checks**

Run build pressure check first:

```bash
pgrep -af 'cargo|rustc|trunk|ld|lld|mold|c\\+\\+' || true
ps -eo pid,ppid,stat,rss,etime,cmd --sort=-rss | head -40
```

Then run:

```bash
cargo test -p llm-access-migrations -p llm-access-store -p llm-access-migrator -p llm-access --jobs 1 -- --nocapture
cargo clippy -p llm-access-migrations -p llm-access-store -p llm-access-migrator -p llm-access --jobs 1 -- -D warnings
bash scripts/test_llm_access_cloud_bundle.sh
bash -n scripts/verify_llm_access_cloud_canary.sh
git diff --check
```

Expected:

- All tests pass.
- Clippy reports zero warnings.
- Template validation passes.
- Shell syntax validation passes.
- `git diff --check` reports no whitespace errors.

- [ ] **Step 3: Commit docs**

```bash
git add docs/llm-access-cdc-storage-design.zh.md docs/superpowers/specs/2026-04-30-llm-access-cloud-migration-design.md
git commit -m "docs: update llm access cloud migration status"
```

---

## Execution Notes

- The first useful cutover milestone is not "all admin pages perfect"; it is "LLM streaming traffic no longer leaves through the home uplink."
- Keep Caddy route split conservative. If a route is not clearly LLM-related, leave it on local StaticFlow.
- The cloud service should fail fast when the JuiceFS mount is absent. A service that silently starts on an empty local directory is worse than a failed service.
- Do not run more than one active `llm-access` writer against the same SQLite/DuckDB files.
- Do not cut production traffic inside this implementation plan without an explicit user approval checkpoint after canary verification.
