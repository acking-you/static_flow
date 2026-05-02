# llm-access Compact DuckDB Archive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make tiered `llm-access` DuckDB archival rewrite pending segments into compact local DuckDB files before uploading immutable archive files to JuiceFS.

**Architecture:** Keep request-time writes unchanged: active segments still roll over to local pending files and a new active file opens immediately. Replace the sealer's direct `fs::copy(pending, archive)` path with a local DuckDB logical rewrite, then copy the compacted file through an uploading temp path before catalog publication. Serialize sealer work with one process-local lock so the 2c8g cloud host does not run multiple DuckDB compactions at once.

**Tech Stack:** Rust, `duckdb` crate, `rusqlite`, existing `llm-access-store` tiered DuckDB repository, systemd deployment templates.

---

## Scope

This plan only changes `llm-access` usage analytics archival. It does not change provider request dispatch, SQLite control storage, Caddy routing, JuiceFS mount units, or frontend API contracts.

## File Structure

- Modify `llm-access-store/src/duckdb.rs`
  - Add compacting directory helpers.
  - Add one global sealer lock.
  - Replace direct pending-to-archive copy with compact-then-archive.
  - Add focused tests in the existing test module.
- Modify `deployment-examples/systemd/llm-access.service.template`
  - Keep the current active/archive/catalog envs.
  - Add a short comment documenting that compact work is derived from the active directory.
- Modify `docs/superpowers/specs/2026-05-02-llm-access-tiered-usage-duckdb-design.md`
  - Only if implementation reveals a small correction to the already-approved design.

## Task 1: Prove Archive Publication Rewrites The Segment

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Write the failing test**

Add this test near `duckdb_tiered_repository_rolls_over_without_blocking_active_appends`:

```rust
#[cfg(feature = "duckdb-runtime")]
#[test]
fn duckdb_tiered_publish_rewrites_segment_with_current_schema() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-compact-publish",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create compact publish test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        catalog_dir: root.join("catalog"),
        rollover_bytes: 1,
    };
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");

    let pending_path = root.join("pending-source.duckdb");
    {
        let conn = duckdb::Connection::open(&pending_path).expect("open pending source");
        crate::initialize_duckdb_target(&conn).expect("initialize pending source");
        let mut writer = super::DuckDbUsageWriter::new(conn).expect("open pending writer");
        let mut event = test_usage_event();
        event.event_id = "compact-publish-event".to_string();
        event.created_at_ms = 1_700_000_000_000;
        writer
            .insert_usage_events(&[super::UsageEventRow::from_usage_event(&event)])
            .expect("insert pending event");
    }
    {
        let conn = duckdb::Connection::open(&pending_path).expect("reopen pending source");
        conn.execute_batch(
            "
            CREATE INDEX IF NOT EXISTS idx_usage_events_created_date
                ON usage_events(created_at_ms);
            CHECKPOINT;
            ",
        )
        .expect("create legacy source index");
    }

    super::publish_pending_segment(&config, &pending_path, "usage-compact-test-000001")
        .expect("publish compacted segment");

    let archive_path = config.archive_dir.join("usage-compact-test-000001.duckdb");
    assert!(archive_path.exists(), "archived compact segment should exist");
    assert!(
        !pending_path.exists(),
        "pending segment should be removed only after catalog publication"
    );
    assert!(
        !super::tiered_compacting_dir(&config).join("usage-compact-test-000001.tmp.duckdb").exists(),
        "local compact temp file should be removed after publication"
    );
    assert!(
        !config
            .archive_dir
            .join("usage-compact-test-000001.uploading.duckdb")
            .exists(),
        "uploading archive temp file should not remain after publication"
    );

    let archived = super::DuckDbUsageRepository::open_read_only_conn(&archive_path)
        .expect("open archived compact segment");
    let indexes = archived
        .prepare("SELECT index_name FROM duckdb_indexes() ORDER BY index_name")
        .expect("prepare index query")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query indexes")
        .collect::<Result<Vec<_>, _>>()
        .expect("read indexes");
    assert!(
        indexes.is_empty(),
        "archive should be rewritten with current schema and no legacy explicit indexes: {indexes:?}"
    );

    let count: i64 = archived
        .query_row("SELECT CAST(count(*) AS BIGINT) FROM usage_events", [], |row| row.get(0))
        .expect("count archived rows");
    assert_eq!(count, 1);

    std::fs::remove_dir_all(&root).expect("cleanup compact publish test directory");
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run:

```bash
df -h /mnt/wsl/data4tb
pgrep -af 'cargo|rustc|trunk|ld|lld|mold' || true
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
  cargo test -p llm-access-store duckdb_tiered_publish_rewrites_segment_with_current_schema --jobs 1 --features duckdb-runtime -- --nocapture
```

Expected: FAIL because the current implementation directly copies the pending file and preserves `idx_usage_events_created_date` in the archive.

## Task 2: Add Compact Directory And Temp Path Helpers

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Add helper functions**

Add helpers near `tiered_pending_dir`:

```rust
#[cfg(feature = "duckdb-runtime")]
fn tiered_compacting_dir(config: &TieredDuckDbUsageConfig) -> PathBuf {
    config.active_dir.join("compacting")
}

#[cfg(feature = "duckdb-runtime")]
fn compacting_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    tiered_compacting_dir(config).join(format!("{segment_id}.tmp.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn archive_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    config.archive_dir.join(format!("{segment_id}.duckdb"))
}

#[cfg(feature = "duckdb-runtime")]
fn uploading_archive_segment_path(config: &TieredDuckDbUsageConfig, segment_id: &str) -> PathBuf {
    config.archive_dir.join(format!("{segment_id}.uploading.duckdb"))
}
```

- [ ] **Step 2: Create compacting directory on startup**

In `DuckDbUsageRepository::open_tiered`, after pending directory creation, add:

```rust
fs::create_dir_all(tiered_compacting_dir(&config)).with_context(|| {
    format!(
        "failed to create compacting duckdb directory `{}`",
        tiered_compacting_dir(&config).display()
    )
})?;
```

- [ ] **Step 3: Run the focused test**

Run the same command from Task 1. Expected: still FAIL with the legacy index assertion, because publication still directly copies the source.

## Task 3: Implement Local DuckDB Logical Rewrite

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Add file cleanup helper**

Add near the path helpers:

```rust
#[cfg(feature = "duckdb-runtime")]
fn remove_file_if_exists(path: &Path) -> anyhow::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("failed to remove file `{}`", path.display())),
    }
}
```

- [ ] **Step 2: Add SQL copy helper**

Add this string-literal helper near the path helpers:

```rust
#[cfg(feature = "duckdb-runtime")]
fn duckdb_string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
```

Add this compact helper near `checkpoint_duckdb_path`:

```rust
#[cfg(feature = "duckdb-runtime")]
fn compact_pending_segment_to_local_file(
    config: &TieredDuckDbUsageConfig,
    pending_path: &Path,
    segment_id: &str,
) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(tiered_compacting_dir(config)).with_context(|| {
        format!(
            "failed to create compacting duckdb directory `{}`",
            tiered_compacting_dir(config).display()
        )
    })?;
    let compact_path = compacting_segment_path(config, segment_id);
    remove_file_if_exists(&compact_path)?;

    let conn = DuckDbUsageRepository::open_conn(&compact_path)?;
    conn.execute_batch("SET memory_limit='768MB';")
        .context("failed to set duckdb compact memory limit")?;
    crate::initialize_duckdb_target(&conn)?;
    let attach_sql = format!(
        "ATTACH DATABASE {} AS pending_segment (READ_ONLY);",
        duckdb_string_literal(&pending_path.to_string_lossy())
    );
    conn.execute_batch(&attach_sql).with_context(|| {
        format!(
            "failed to attach pending duckdb segment `{}`",
            pending_path.display()
        )
    })?;
    conn.execute_batch(
        "
        INSERT INTO usage_events SELECT * FROM pending_segment.usage_events;
        INSERT INTO usage_event_details SELECT * FROM pending_segment.usage_event_details;
        INSERT INTO usage_rollups_hourly SELECT * FROM pending_segment.usage_rollups_hourly;
        INSERT INTO usage_rollups_daily SELECT * FROM pending_segment.usage_rollups_daily;
        DETACH pending_segment;
        CHECKPOINT;
        ",
    )
    .with_context(|| {
        format!(
            "failed to compact pending duckdb segment `{}` into `{}`",
            pending_path.display(),
            compact_path.display()
        )
    })?;
    Ok(compact_path)
}
```

- [ ] **Step 3: Add validation helper**

Add:

```rust
#[cfg(feature = "duckdb-runtime")]
fn validate_compacted_segment_matches_source(source: &Path, compacted: &Path) -> anyhow::Result<SegmentStats> {
    let source_stats = collect_segment_stats(source)?;
    let compacted_stats = collect_segment_stats(compacted)?;
    if source_stats.row_count != compacted_stats.row_count
        || source_stats.start_ms != compacted_stats.start_ms
        || source_stats.end_ms != compacted_stats.end_ms
    {
        return Err(anyhow!(
            "compacted duckdb segment mismatch: source rows={} start={:?} end={:?}, compacted rows={} start={:?} end={:?}",
            source_stats.row_count,
            source_stats.start_ms,
            source_stats.end_ms,
            compacted_stats.row_count,
            compacted_stats.start_ms,
            compacted_stats.end_ms
        ));
    }
    Ok(compacted_stats)
}
```

- [ ] **Step 4: Run the focused test**

Run the same command from Task 1. Expected: still FAIL until `publish_pending_segment` uses the helper.

## Task 4: Replace Direct Archive Copy With Compact-Then-Archive

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Replace `publish_pending_segment` body**

Replace the current direct copy body with:

```rust
fs::create_dir_all(&config.archive_dir).with_context(|| {
    format!("failed to create archive directory `{}`", config.archive_dir.display())
})?;
let compact_path = compact_pending_segment_to_local_file(config, pending_path, segment_id)?;
let stats = validate_compacted_segment_matches_source(pending_path, &compact_path)?;
let uploading_path = uploading_archive_segment_path(config, segment_id);
let archive_path = archive_segment_path(config, segment_id);
remove_file_if_exists(&uploading_path)?;
fs::copy(&compact_path, &uploading_path).with_context(|| {
    format!(
        "failed to copy compacted duckdb segment `{}` to uploading archive `{}`",
        compact_path.display(),
        uploading_path.display()
    )
})?;
fs::rename(&uploading_path, &archive_path).with_context(|| {
    format!(
        "failed to publish uploading archive `{}` to `{}`",
        uploading_path.display(),
        archive_path.display()
    )
})?;
let size_bytes = fs::metadata(&archive_path)
    .with_context(|| format!("failed to stat archived segment `{}`", archive_path.display()))?
    .len();
publish_segment_catalog(config, segment_id, &archive_path, &stats, size_bytes)?;
remove_file_if_exists(pending_path)?;
remove_file_if_exists(&compact_path)?;
Ok(())
```

- [ ] **Step 2: Run the focused test**

Run the Task 1 command. Expected: PASS. The archive should have no legacy explicit indexes, the pending file should be gone, and temp files should be gone.

## Task 5: Serialize Background Sealer Work

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`

- [ ] **Step 1: Add global lock**

Near the top-level tiered helpers, add:

```rust
#[cfg(feature = "duckdb-runtime")]
static TIERED_SEGMENT_SEALER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
```

- [ ] **Step 2: Hold the lock in the sealer thread**

Inside `spawn_segment_sealer`, before the retry loop, add:

```rust
let Ok(_sealer_guard) = TIERED_SEGMENT_SEALER_LOCK.lock() else {
    eprintln!(
        "failed to archive llm-access duckdb segment `{segment_id}` from `{}`: sealer lock poisoned",
        pending_path.display()
    );
    return;
};
```

Keep the existing retry loop under that guard.

- [ ] **Step 3: Run rollover test**

Run:

```bash
df -h /mnt/wsl/data4tb
pgrep -af 'cargo|rustc|trunk|ld|lld|mold' || true
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
  cargo test -p llm-access-store duckdb_tiered_repository_rolls_over_without_blocking_active_appends --jobs 1 --features duckdb-runtime -- --nocapture
```

Expected: PASS. Active appends continue while archived files are eventually published.

## Task 6: Document Derived Compact Work Directory

**Files:**
- Modify: `deployment-examples/systemd/llm-access.service.template`

- [ ] **Step 1: Add a service comment**

Add below `LLM_ACCESS_DUCKDB_ACTIVE_DIR`:

```ini
# Compact work files are derived under ${LLM_ACCESS_DUCKDB_ACTIVE_DIR}/compacting
# so DuckDB rewrite work stays on local block storage instead of JuiceFS.
```

- [ ] **Step 2: No runtime command change**

Do not add a new CLI flag unless implementation proves the derived path is insufficient. The approved design uses a derived local compact directory.

## Task 7: Run Formatting And Verification

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`
- Modify: `deployment-examples/systemd/llm-access.service.template`

- [ ] **Step 1: Format only the changed Rust file**

Run:

```bash
rustfmt llm-access-store/src/duckdb.rs
```

Expected: no output.

- [ ] **Step 2: Run focused tests**

Run:

```bash
df -h /mnt/wsl/data4tb
pgrep -af 'cargo|rustc|trunk|ld|lld|mold' || true
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
  cargo test -p llm-access-store duckdb_tiered --jobs 1 --features duckdb-runtime -- --nocapture
```

Expected: all `duckdb_tiered*` tests pass.

- [ ] **Step 3: Run crate clippy**

Run:

```bash
df -h /mnt/wsl/data4tb
pgrep -af 'cargo|rustc|trunk|ld|lld|mold' || true
CARGO_TARGET_DIR=/mnt/wsl/data4tb/static-flow-data/cargo-target/static_flow \
  cargo clippy -p llm-access-store --jobs 1 --features duckdb-runtime -- -D warnings
```

Expected: clippy finishes with zero warnings.

## Task 8: Commit

**Files:**
- Modify: `llm-access-store/src/duckdb.rs`
- Modify: `deployment-examples/systemd/llm-access.service.template`
- Modify: `docs/superpowers/plans/2026-05-02-llm-access-compact-duckdb-archive.md`

- [ ] **Step 1: Review diff**

Run:

```bash
git diff --check
git diff --stat
git diff -- llm-access-store/src/duckdb.rs deployment-examples/systemd/llm-access.service.template
```

Expected: no whitespace errors, and the diff is limited to compact archive logic plus the service comment.

- [ ] **Step 2: Commit implementation**

Run:

```bash
git add llm-access-store/src/duckdb.rs deployment-examples/systemd/llm-access.service.template docs/superpowers/plans/2026-05-02-llm-access-compact-duckdb-archive.md
git commit -m "feat: compact llm-access duckdb archive segments"
```
