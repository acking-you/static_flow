# llm-access Control-Plane Neon Cutover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the live `llm-access` control plane from shared SQLite to Neon Postgres, cut over both API and usage-worker in one maintenance window, keep SQLite only for rollback, and enforce that `usage-journal` stays on local disk.

**Architecture:** Add an explicit control-backend selection layer, implement a Postgres-backed control repository with the same trait surface currently provided by `SqliteControlRepository`, and wire both services to consume a shared Neon connection env file from `/mnt/llm-access/config/neon.env`. Rollout is a hard switch: no dual write, no replay, no silent fallback to SQLite.

**Tech Stack:** Rust (`llm-access`, `llm-access-store`, `llm-access-migrations`, `llm-access-core`), Postgres/Neon, `tokio-postgres` + TLS, SQLite rollback state, systemd service templates, GCP cloud release scripts.

---

### Task 1: Add explicit control-backend selection and preserve the local journal invariant

**Files:**
- Modify: `llm-access/src/config.rs`
- Modify: `llm-access/src/lib.rs`
- Test: `llm-access/src/config.rs`

- [ ] **Step 1: Create a dedicated worktree for the implementation branch**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow
git worktree add ../static_flow-control-neon -b feat/llm-access-control-neon
cd ../static_flow-control-neon
```

Expected: a clean worktree on branch `feat/llm-access-control-neon`.

- [ ] **Step 2: Write the failing config tests for explicit Postgres control selection**

Add to `llm-access/src/config.rs`:

```rust
#[test]
fn parses_postgres_control_backend_from_env_name() {
    let command = super::CliCommand::parse([
        "llm-access",
        "serve",
        "--state-root",
        "/mnt/llm-access",
        "--postgres-control-database-url-env",
        "LLM_ACCESS_CONTROL_DATABASE_URL",
        "--usage-journal-dir",
        "/var/lib/staticflow/llm-access/usage-journal",
    ])
    .expect("parse serve config");

    let super::CliCommand::Serve(config) = command else {
        panic!("expected serve command");
    };
    assert!(matches!(
        config.storage.control_store,
        super::ControlStoreConfig::Postgres { ref database_url_env }
        if database_url_env == "LLM_ACCESS_CONTROL_DATABASE_URL"
    ));
    assert_eq!(
        config.storage.usage_journal_dir,
        PathBuf::from("/var/lib/staticflow/llm-access/usage-journal")
    );
}

#[test]
fn rejects_sqlite_and_postgres_control_flags_together() {
    let err = super::CliCommand::parse([
        "llm-access",
        "serve",
        "--state-root",
        "/mnt/llm-access",
        "--sqlite-control",
        "/mnt/llm-access/control/llm-access.sqlite3",
        "--postgres-control-database-url-env",
        "LLM_ACCESS_CONTROL_DATABASE_URL",
    ])
    .expect_err("mixed backend flags must fail");

    assert!(err.to_string().contains("exactly one control backend"));
}
```

- [ ] **Step 3: Run the config tests and verify they fail before implementation**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access parses_postgres_control_backend_from_env_name rejects_sqlite_and_postgres_control_flags_together --jobs 4
```

Expected: FAIL because `StorageConfig` only knows `sqlite_control`.

- [ ] **Step 4: Replace `sqlite_control` with an explicit control-backend enum**

Update `llm-access/src/config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlStoreConfig {
    Sqlite { path: PathBuf },
    Postgres { database_url_env: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageConfig {
    pub state_root: PathBuf,
    pub control_store: ControlStoreConfig,
    pub duckdb: PathBuf,
    pub usage_journal_dir: PathBuf,
    pub duckdb_tiered: Option<TieredDuckDbStorageConfig>,
    pub kiro_auths_dir: PathBuf,
    pub codex_auths_dir: PathBuf,
    pub logs_dir: PathBuf,
}
```

In the parser, replace the single `sqlite_control` field with mutually exclusive state:

```rust
let mut sqlite_control = None;
let mut postgres_control_database_url_env = None;

"--sqlite-control" => {
    sqlite_control = Some(PathBuf::from(
        args.next().ok_or_else(|| anyhow!("--sqlite-control requires a path"))?,
    ));
}
"--postgres-control-database-url-env" => {
    postgres_control_database_url_env = Some(
        args.next()
            .ok_or_else(|| anyhow!("--postgres-control-database-url-env requires an env name"))?
            .to_string_lossy()
            .to_string(),
    );
}
```

Resolve exactly one backend:

```rust
let control_store = match (sqlite_control, postgres_control_database_url_env) {
    (Some(path), None) => ControlStoreConfig::Sqlite { path },
    (None, Some(database_url_env)) => ControlStoreConfig::Postgres { database_url_env },
    _ => return Err(anyhow!("exactly one control backend must be configured")),
};
```

Update `llm-access/src/lib.rs` bootstrap validation so it only initializes SQLite when the backend is `Sqlite`:

```rust
match &config.control_store {
    ControlStoreConfig::Sqlite { path } => {
        llm_access_store::initialize_sqlite_target_path(path)?;
    }
    ControlStoreConfig::Postgres { .. } => {}
}
```

- [ ] **Step 5: Re-run the config tests plus the existing config test module**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access config --jobs 4
```

Expected: PASS.

- [ ] **Step 6: Commit the config/backend selection work**

```bash
git add llm-access/src/config.rs llm-access/src/lib.rs
git commit -m "refactor(llm-access): add explicit control backend selection"
```

### Task 2: Add Postgres migrations and bootstrap helpers for the control schema

**Files:**
- Modify: `Cargo.toml`
- Modify: `llm-access-migrations/Cargo.toml`
- Modify: `llm-access-migrations/src/lib.rs`
- Create: `llm-access-migrations/migrations/postgres/0001_init.sql`
- Create: `llm-access-migrations/migrations/postgres/0002_followups.sql`
- Modify: `llm-access-store/Cargo.toml`
- Modify: `llm-access-store/src/lib.rs`
- Test: `llm-access-migrations/src/lib.rs`

- [ ] **Step 1: Add a failing migration test for Postgres migration enumeration**

Append to `llm-access-migrations/src/lib.rs` tests:

```rust
#[test]
fn postgres_migrations_are_file_backed_and_versioned() {
    let migrations = super::postgres_migrations();

    assert_eq!(migrations[0].version, 1);
    assert_eq!(migrations[0].name, "init");
    assert!(migrations[0].sql.contains("CREATE TABLE IF NOT EXISTS llm_keys"));
    assert!(migrations.iter().any(|migration| migration.sql.contains("llm_runtime_config")));
}
```

- [ ] **Step 2: Run the migration test and verify it fails**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access-migrations postgres_migrations_are_file_backed_and_versioned --jobs 4
```

Expected: FAIL because there is no Postgres migration list yet.

- [ ] **Step 3: Add Postgres dependencies and migration registration**

Update workspace and crate dependencies:

```toml
# Cargo.toml
[workspace.dependencies]
tokio-postgres = "0.7"
native-tls = "0.2"
postgres-native-tls = "0.5"
```

```toml
# llm-access-migrations/Cargo.toml
[dependencies]
anyhow = { workspace = true }
rusqlite = { version = "0.37", features = ["bundled"] }
tokio-postgres = { workspace = true }
```

Register Postgres migrations in `llm-access-migrations/src/lib.rs`:

```rust
const POSTGRES_MIGRATIONS: &[SqlMigration] = &[
    SqlMigration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/postgres/0001_init.sql"),
    },
    SqlMigration {
        version: 2,
        name: "followups",
        sql: include_str!("../migrations/postgres/0002_followups.sql"),
    },
];

pub fn postgres_migrations() -> &'static [SqlMigration] {
    POSTGRES_MIGRATIONS
}
```

Implement the runner:

```rust
pub async fn run_postgres_migrations(client: &tokio_postgres::Client) -> Result<()> {
    client
        .batch_execute(
            "CREATE TABLE IF NOT EXISTS llm_access_schema_migrations (
                version BIGINT PRIMARY KEY,
                name TEXT NOT NULL,
                applied_at_ms BIGINT NOT NULL CHECK (applied_at_ms >= 0)
            );",
        )
        .await
        .context("failed to initialize postgres migration metadata")?;

    for migration in POSTGRES_MIGRATIONS {
        let row = client
            .query_one(
                "SELECT EXISTS(
                    SELECT 1 FROM llm_access_schema_migrations WHERE version = $1
                )",
                &[&migration.version],
            )
            .await
            .with_context(|| format!("failed to inspect migration {}", migration.version))?;
        let already_applied: bool = row.get(0);
        if already_applied {
            continue;
        }
        let tx = client.transaction().await?;
        tx.batch_execute(migration.sql).await?;
        tx.execute(
            "INSERT INTO llm_access_schema_migrations (version, name, applied_at_ms)
             VALUES ($1, $2, (extract(epoch from now()) * 1000)::bigint)",
            &[&migration.version, &migration.name],
        )
        .await?;
        tx.commit().await?;
    }
    Ok(())
}
```

- [ ] **Step 4: Add the first Postgres schema files**

Create `llm-access-migrations/migrations/postgres/0001_init.sql` by porting the existing SQLite baseline:

```sql
CREATE TABLE IF NOT EXISTS llm_keys (
    key_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    protocol_family TEXT NOT NULL CHECK (protocol_family IN ('openai', 'anthropic')),
    public_visible BOOLEAN NOT NULL,
    quota_billable_limit BIGINT NOT NULL CHECK (quota_billable_limit >= 0),
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_keys_provider_status
    ON llm_keys(provider_type, status);
```

Add the remaining baseline control tables in the same file.

Create `0002_followups.sql` for the follow-up columns that are currently spread
across SQLite migrations, for example:

```sql
ALTER TABLE llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_full_request_logging_enabled BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE llm_runtime_config
    ADD COLUMN IF NOT EXISTS usage_analytics_retention_days BIGINT NOT NULL DEFAULT 7;
```

- [ ] **Step 5: Add store bootstrap helpers for Postgres initialization**

Update `llm-access-store/src/lib.rs`:

```rust
pub async fn initialize_postgres_target(database_url: &str) -> anyhow::Result<()> {
    let tls = postgres_native_tls::MakeTlsConnector::new(
        native_tls::TlsConnector::new().context("build native tls connector")?,
    );
    let (client, connection) = tokio_postgres::connect(database_url, tls)
        .await
        .context("connect postgres for initialization")?;
    tokio::spawn(async move {
        let _ = connection.await;
    });
    llm_access_migrations::run_postgres_migrations(&client).await
}
```

- [ ] **Step 6: Re-run migration tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access-migrations --jobs 4
```

Expected: PASS.

- [ ] **Step 7: Commit the migration/bootstrap layer**

```bash
git add Cargo.toml llm-access-migrations/Cargo.toml llm-access-migrations/src/lib.rs \
  llm-access-migrations/migrations/postgres/0001_init.sql \
  llm-access-migrations/migrations/postgres/0002_followups.sql \
  llm-access-store/Cargo.toml llm-access-store/src/lib.rs
git commit -m "feat(llm-access): add postgres control migrations"
```

### Task 3: Implement the Postgres control repository for read paths and startup-critical flows

**Files:**
- Create: `llm-access-store/src/postgres.rs`
- Modify: `llm-access-store/src/lib.rs`
- Modify: `llm-access-store/src/repository.rs`
- Test: `llm-access-store/src/postgres.rs`

- [ ] **Step 1: Add a failing integration test for runtime config and key lookup**

Create `llm-access-store/src/postgres.rs` test module with an env-gated integration test:

```rust
#[tokio::test]
async fn postgres_repository_reads_runtime_config_and_authenticates_key() {
    let database_url = std::env::var("TEST_POSTGRES_URL")
        .expect("TEST_POSTGRES_URL must point to an isolated Postgres database");
    let repo = super::PostgresControlRepository::connect(&database_url)
        .await
        .expect("connect postgres repository");

    let config = repo.get_admin_runtime_config().await.expect("runtime config");
    assert_eq!(config.id.as_str(), "default");

    let key = repo
        .lookup_authenticated_key("secret")
        .await
        .expect("lookup result")
        .expect("key must exist");
    assert_eq!(key.name, "external");
}
```

- [ ] **Step 2: Run the failing read-path test against an isolated Postgres DB**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access-store postgres_repository_reads_runtime_config_and_authenticates_key --jobs 4
```

Expected: FAIL because `PostgresControlRepository` does not exist yet.

- [ ] **Step 3: Add the Postgres repository shell with async connection bootstrap**

Create `llm-access-store/src/postgres.rs`:

```rust
pub struct PostgresControlRepository {
    client: tokio_postgres::Client,
    _connection_task: tokio::task::JoinHandle<()>,
}

impl PostgresControlRepository {
    pub async fn connect(database_url: &str) -> anyhow::Result<Self> {
        let tls = postgres_native_tls::MakeTlsConnector::new(
            native_tls::TlsConnector::new().context("build native tls connector")?,
        );
        let (client, connection) = tokio_postgres::connect(database_url, tls)
            .await
            .context("connect postgres control repository")?;
        let connection_task = tokio::spawn(async move {
            if let Err(err) = connection.await {
                tracing::error!("postgres control connection terminated: {err:#}");
            }
        });
        llm_access_migrations::run_postgres_migrations(&client).await?;
        Ok(Self {
            client,
            _connection_task: connection_task,
        })
    }
}
```

- [ ] **Step 4: Implement startup-critical trait reads first**

In `llm-access-store/src/postgres.rs`, implement these traits before any write-path work:

```rust
#[async_trait]
impl AdminConfigStore for PostgresControlRepository {
    async fn get_admin_runtime_config(&self) -> anyhow::Result<AdminRuntimeConfig> {
        let row = self
            .client
            .query_one("SELECT * FROM llm_runtime_config WHERE id = 'default'", &[])
            .await?;
        decode_runtime_config_row(&row)
    }
}

#[async_trait]
impl PublicAccessStore for PostgresControlRepository {
    async fn lookup_authenticated_key(
        &self,
        secret: &str,
    ) -> anyhow::Result<Option<AuthenticatedKey>> {
        let key_hash = sha256_secret(secret);
        let row = self
            .client
            .query_opt(
                "SELECT k.*, r.*
                 FROM llm_keys k
                 LEFT JOIN llm_key_route_config r ON r.key_id = k.key_id
                 WHERE k.key_hash = $1 AND k.status = 'active'",
                &[&key_hash],
            )
            .await?;
        row.map(decode_authenticated_key_row).transpose()
    }
}
```

Also implement the read side of:

- `ProviderRouteStore`
- `PublicStatusStore`
- `AdminCodexAccountStore`
- `AdminKiroAccountStore`
- `AdminProxyStore`
- `AdminAccountGroupStore`

- [ ] **Step 5: Re-run the read-path integration test**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access-store postgres_repository_reads_runtime_config_and_authenticates_key --jobs 4
```

Expected: PASS.

- [ ] **Step 6: Commit the read-path repository layer**

```bash
git add llm-access-store/src/postgres.rs llm-access-store/src/lib.rs llm-access-store/src/repository.rs
git commit -m "feat(llm-access): add postgres control read repository"
```

### Task 4: Implement Postgres mutation paths and trait parity required for live traffic

**Files:**
- Modify: `llm-access-store/src/postgres.rs`
- Test: `llm-access-store/src/postgres.rs`
- Test: `llm-access/src/provider.rs`

- [ ] **Step 1: Add failing tests for rollups and public submission writes**

Add to `llm-access-store/src/postgres.rs`:

```rust
#[tokio::test]
async fn postgres_repository_updates_key_usage_rollups() {
    let database_url = std::env::var("TEST_POSTGRES_URL")
        .expect("TEST_POSTGRES_URL must point to an isolated Postgres database");
    let repo = super::PostgresControlRepository::connect(&database_url)
        .await
        .expect("connect postgres repository");

    repo.apply_usage_rollup(KeyUsageRollupUpdate {
        key_id: "key-1".to_string(),
        input_uncached_tokens_delta: 10,
        input_cached_tokens_delta: 2,
        output_tokens_delta: 5,
        billable_tokens_delta: 15,
        credit_total_delta: Some("1.25".to_string()),
        credit_missing_events_delta: 0,
        used_at_ms: Some(1_700_000_000_001),
    })
    .await
    .expect("apply rollup");

    let summary = repo.key_usage_rollup("key-1").await.expect("summary");
    assert_eq!(summary.expect("row").billable_tokens, 15);
}
```

Add a public-submission write test:

```rust
#[tokio::test]
async fn postgres_repository_creates_account_contribution_request() {
    let database_url = std::env::var("TEST_POSTGRES_URL")
        .expect("TEST_POSTGRES_URL must point to an isolated Postgres database");
    let repo = super::PostgresControlRepository::connect(&database_url)
        .await
        .expect("connect postgres repository");

    let request = NewPublicAccountContributionRequest {
        account_name: "acct-1".to_string(),
        account_id: Some("acct-id-1".to_string()),
        id_token: "id-token".to_string(),
        access_token: "access-token".to_string(),
        refresh_token: "refresh-token".to_string(),
        requester_email: "user@example.com".to_string(),
        contributor_message: "hello".to_string(),
        github_id: None,
        frontend_page_url: None,
        fingerprint: "fp".to_string(),
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
    };

    let created = repo
        .create_public_account_contribution_request(request)
        .await
        .expect("create request");
    assert_eq!(created.status, "pending");
}
```

- [ ] **Step 2: Run the failing mutation tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access-store postgres_repository_updates_key_usage_rollups postgres_repository_creates_account_contribution_request --jobs 4
```

Expected: FAIL because mutation trait implementations are incomplete.

- [ ] **Step 3: Implement transactional mutation paths in the Postgres repository**

In `llm-access-store/src/postgres.rs`, implement:

- `ControlStore`
- `AdminKeyStore`
- `PublicSubmissionStore`
- `AdminReviewQueueStore`
- `PublicUsageStore`

For rollups, use atomic `INSERT ... ON CONFLICT ... DO UPDATE`:

```rust
self.client
    .execute(
        "INSERT INTO llm_key_usage_rollups (
            key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
            billable_tokens, credit_total, credit_missing_events, last_used_at_ms, updated_at_ms
         ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
         ON CONFLICT (key_id) DO UPDATE SET
            input_uncached_tokens = llm_key_usage_rollups.input_uncached_tokens + EXCLUDED.input_uncached_tokens,
            input_cached_tokens = llm_key_usage_rollups.input_cached_tokens + EXCLUDED.input_cached_tokens,
            output_tokens = llm_key_usage_rollups.output_tokens + EXCLUDED.output_tokens,
            billable_tokens = llm_key_usage_rollups.billable_tokens + EXCLUDED.billable_tokens,
            credit_total = ((llm_key_usage_rollups.credit_total)::numeric + (EXCLUDED.credit_total)::numeric)::text,
            credit_missing_events = llm_key_usage_rollups.credit_missing_events + EXCLUDED.credit_missing_events,
            last_used_at_ms = GREATEST(llm_key_usage_rollups.last_used_at_ms, EXCLUDED.last_used_at_ms),
            updated_at_ms = EXCLUDED.updated_at_ms",
        &[...],
    )
    .await?;
```

For public/admin workflows, port each SQLite mutation into explicit Postgres SQL
with the same validation semantics, not looser semantics.

- [ ] **Step 4: Add one provider-level regression test that still passes through the new store**

In `llm-access/src/provider.rs`, add a store-backed regression test that uses a
real `PostgresControlRepository` when `TEST_POSTGRES_URL` is set and exercises a
request path that touches:

- authenticated key lookup;
- route resolution;
- usage rollup persistence.

Use a minimal fake upstream and assert that the store mutates the key rollup row.

- [ ] **Step 5: Re-run repository and provider mutation tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access-store -p llm-access postgres_repository_ --jobs 4
```

Expected: PASS for the new Postgres repository tests and the provider regression.

- [ ] **Step 6: Commit the mutation/parity repository work**

```bash
git add llm-access-store/src/postgres.rs llm-access/src/provider.rs
git commit -m "feat(llm-access): add postgres control write paths"
```

### Task 5: Wire API and usage-worker bootstrap to Postgres and keep the journal path local

**Files:**
- Modify: `llm-access/src/runtime.rs`
- Modify: `llm-access/src/bin/llm-access-usage-worker.rs`
- Modify: `llm-access/src/lib.rs`
- Modify: `deployment-examples/systemd/llm-access.service.template`
- Modify: `deployment-examples/systemd/llm-access-usage-worker.service.template`
- Modify: `conf/llm-access-cloud-release.env.example`
- Test: `llm-access/src/runtime.rs`

- [ ] **Step 1: Add a failing runtime test that boots from Postgres config**

Add to `llm-access/src/runtime.rs`:

```rust
#[tokio::test]
async fn runtime_bootstraps_from_postgres_control_backend() {
    let database_url = std::env::var("TEST_POSTGRES_URL")
        .expect("TEST_POSTGRES_URL must point to an isolated Postgres database");
    std::env::set_var("LLM_ACCESS_CONTROL_DATABASE_URL", database_url);

    let config = StorageConfig {
        state_root: PathBuf::from("/mnt/llm-access"),
        control_store: ControlStoreConfig::Postgres {
            database_url_env: "LLM_ACCESS_CONTROL_DATABASE_URL".to_string(),
        },
        duckdb: PathBuf::from("/mnt/llm-access/analytics/usage.duckdb"),
        usage_journal_dir: PathBuf::from("/var/lib/staticflow/llm-access/usage-journal"),
        duckdb_tiered: None,
        kiro_auths_dir: PathBuf::from("/mnt/llm-access/auths/kiro"),
        codex_auths_dir: PathBuf::from("/mnt/llm-access/auths/codex"),
        logs_dir: PathBuf::from("/mnt/llm-access/logs"),
    };

    let runtime = super::LlmAccessStores::from_storage_config(&config)
        .await
        .expect("build runtime stores");
    let cfg = runtime.admin_config_store().get_admin_runtime_config().await.expect("config");
    assert_eq!(cfg.id, "default");
}
```

- [ ] **Step 2: Run the failing runtime bootstrap test**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access runtime_bootstraps_from_postgres_control_backend --jobs 4
```

Expected: FAIL because `from_storage_config()` still only opens SQLite.

- [ ] **Step 3: Teach runtime and worker bootstrap to construct the right repository**

In `llm-access/src/runtime.rs`:

```rust
let repository = match &config.control_store {
    ControlStoreConfig::Sqlite { path } => {
        Arc::new(SqliteControlRepository::open_path(path)?)
    }
    ControlStoreConfig::Postgres { database_url_env } => {
        let database_url = std::env::var(database_url_env)
            .with_context(|| format!("missing control database env `{database_url_env}`"))?;
        Arc::new(PostgresControlRepository::connect(&database_url).await?)
    }
};
```

In `llm-access/src/bin/llm-access-usage-worker.rs`, replace direct SQLite open
with the same backend selection helper.

- [ ] **Step 4: Update service templates to use the env-based Postgres backend**

Change `deployment-examples/systemd/llm-access.service.template`:

```ini
Environment=LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal
ExecStart=/usr/local/bin/llm-access serve \
  --bind ${LLM_ACCESS_BIND_ADDR} \
  --state-root ${LLM_ACCESS_STATE_ROOT} \
  --postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL \
  --usage-journal-dir ${LLM_ACCESS_USAGE_JOURNAL_DIR}
```

Change `deployment-examples/systemd/llm-access-usage-worker.service.template`:

```ini
Environment=LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal
ExecStart=/usr/local/bin/llm-access-usage-worker serve \
  --bind 127.0.0.1:19081 \
  --state-root /mnt/llm-access-usage \
  --postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL \
  --usage-journal-dir /var/lib/staticflow/llm-access/usage-journal \
  --duckdb-active-dir /var/lib/staticflow/llm-access/analytics-active \
  --duckdb-archive-dir /mnt/llm-access-usage/analytics/segments \
  --duckdb-catalog-dir /mnt/llm-access-usage/analytics/catalog \
  --duckdb-rollover-bytes 67108864 \
  --usage-details-dir /mnt/llm-access-usage/details
```

Update `conf/llm-access-cloud-release.env.example` to mention:

```bash
LLM_ACCESS_CONTROL_DATABASE_URL_FILE=/mnt/llm-access/config/neon.env
```

- [ ] **Step 5: Re-run runtime bootstrap tests**

Run:

```bash
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
export TEST_POSTGRES_URL='postgresql://...'
cargo test -p llm-access runtime_ --jobs 4
```

Expected: PASS for the new Postgres bootstrap test and existing runtime tests.

- [ ] **Step 6: Commit runtime and unit wiring**

```bash
git add llm-access/src/runtime.rs llm-access/src/bin/llm-access-usage-worker.rs \
  llm-access/src/lib.rs deployment-examples/systemd/llm-access.service.template \
  deployment-examples/systemd/llm-access-usage-worker.service.template \
  conf/llm-access-cloud-release.env.example
git commit -m "feat(llm-access): wire control backend to postgres"
```

### Task 6: Update release tooling, docs, and execute the GCP cutover verification chain

**Files:**
- Modify: `scripts/activate_llm_access_cloud_release.sh`
- Modify: `scripts/prepare_llm_access_cloud_release.sh`
- Modify: `scripts/release_llm_access_cloud_api_only.sh`
- Modify: `scripts/release_llm_access_cloud_worker_only.sh`
- Modify: `docs/ops-runbook.md`
- Test: `scripts/test_llm_access_cloud_bundle.sh`

- [ ] **Step 1: Add a failing bundle test for Postgres control env usage**

Append to `scripts/test_llm_access_cloud_bundle.sh`:

```bash
grep -F -- '--postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL' \
  "$OUT_DIR/llm-access.service"
grep -F -- '--postgres-control-database-url-env LLM_ACCESS_CONTROL_DATABASE_URL' \
  "$OUT_DIR/llm-access-usage-worker.service"
grep -F -- 'LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal' \
  "$OUT_DIR/llm-access.service"
grep -F -- 'LLM_ACCESS_USAGE_JOURNAL_DIR=/var/lib/staticflow/llm-access/usage-journal' \
  "$OUT_DIR/llm-access-usage-worker.service"
```

- [ ] **Step 2: Run the bundle test and verify it fails on the old templates**

Run:

```bash
bash scripts/test_llm_access_cloud_bundle.sh
```

Expected: FAIL because the rendered units still contain `--sqlite-control`.

- [ ] **Step 3: Update activation scripts to stage and verify Neon config**

In `scripts/activate_llm_access_cloud_release.sh`, add explicit checks:

```bash
sudo test -r /mnt/llm-access/config/neon.env \
  || fail "missing shared Neon config /mnt/llm-access/config/neon.env"

sudo grep -q '^LLM_ACCESS_CONTROL_DATABASE_URL=' /mnt/llm-access/config/neon.env \
  || fail "shared Neon config does not define LLM_ACCESS_CONTROL_DATABASE_URL"
```

Also verify the effective journal path remains local by checking the installed
units and live environment before restart.

- [ ] **Step 4: Document the new cutover and rollback procedure**

Update `docs/ops-runbook.md` with:

- shared config path `/mnt/llm-access/config/neon.env`;
- control cutover sequence for API + worker;
- explicit statement that SQLite remains rollback-only;
- check commands for:
  - Postgres env presence
  - local journal path
  - live API/worker health

Add a rollback section that restores SQLite by configuration only.

- [ ] **Step 5: Re-run bundle validation and then the required Rust verification**

Run:

```bash
bash scripts/test_llm_access_cloud_bundle.sh
export CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow
cargo test -p llm-access-migrations -p llm-access-store -p llm-access --jobs 4
cargo clippy -p llm-access-migrations -p llm-access-store -p llm-access --jobs 4 -- -D warnings
```

Expected: PASS with zero clippy warnings.

- [ ] **Step 6: Prepare and activate the cloud release on GCP**

Run:

```bash
source .local/llm-access-cloud-release.env
./scripts/prepare_llm_access_cloud_release.sh
ssh -i "$GCP_SSH_KEY" "$GCP_USER@$GCP_HOST" \
  "/home/ts_user/staticflow-llm-access-release/activate_llm_access_cloud_release.sh"
```

Expected: both services restart successfully against Neon config.

- [ ] **Step 7: Execute the live smoke chain**

Run:

```bash
ssh -i "$GCP_SSH_KEY" "$GCP_USER@$GCP_HOST" \
  "curl -fsS http://127.0.0.1:19080/healthz && \
   curl -fsS http://127.0.0.1:19080/version && \
   curl -fsS http://127.0.0.1:19081/admin/llm-access/usage-worker/status"
```

Then verify one live control write and one live read:

```bash
curl -fsS -H 'Host: ackingliu.top' https://ackingliu.top/api/llm-gateway/status
curl -fsS -H 'Host: ackingliu.top' https://ackingliu.top/api/llm-gateway/access
```

Expected:

- local health checks return `200`;
- usage worker returns `idle` or healthy progress JSON;
- public control endpoints return valid JSON;
- no service points `usage-journal` at a JuiceFS path.

- [ ] **Step 8: Commit docs and release-tooling updates**

```bash
git add scripts/activate_llm_access_cloud_release.sh \
  scripts/prepare_llm_access_cloud_release.sh \
  scripts/release_llm_access_cloud_api_only.sh \
  scripts/release_llm_access_cloud_worker_only.sh \
  scripts/test_llm_access_cloud_bundle.sh \
  docs/ops-runbook.md
git commit -m "docs(llm-access): document neon control cutover"
```

## Spec coverage self-check

- Control live truth moves fully from SQLite to Neon: covered by Tasks 1-5.
- API and usage-worker cut over in one maintenance window: covered by Tasks 5-6.
- SQLite remains on disk as rollback-only state: covered by Tasks 1, 5, and 6.
- No dual write and no replay: enforced by Task 6 rollout procedure and rejected alternatives.
- Shared mounted config path `/mnt/llm-access/config/neon.env`: covered by Tasks 5-6.
- `usage-journal` must remain on local disk only: covered by Tasks 1, 5, and 6.

## Final verification checklist

- [ ] `cargo test -p llm-access-migrations -p llm-access-store -p llm-access --jobs 4`
- [ ] `cargo clippy -p llm-access-migrations -p llm-access-store -p llm-access --jobs 4 -- -D warnings`
- [ ] `bash scripts/test_llm_access_cloud_bundle.sh`
- [ ] GCP `llm-access.service` healthy on `127.0.0.1:19080`
- [ ] GCP `llm-access-usage-worker.service` healthy on `127.0.0.1:19081`
- [ ] Shared Neon config present at `/mnt/llm-access/config/neon.env`
- [ ] Live `usage-journal` path is `/var/lib/staticflow/llm-access/usage-journal`
- [ ] SQLite rollback file still exists at `/mnt/llm-access/control/llm-access.sqlite3`
