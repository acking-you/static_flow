//! Versioned SQL migrations for the standalone LLM access service.

use anyhow::{Context, Result};

/// One embedded SQL migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SqlMigration {
    /// Monotonic schema version.
    pub version: i64,
    /// Human-readable migration name.
    pub name: &'static str,
    /// SQL body.
    pub sql: &'static str,
}

const SQLITE_MIGRATIONS: &[SqlMigration] = &[
    SqlMigration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/sqlite/0001_init.sql"),
    },
    SqlMigration {
        version: 2,
        name: "codex_status_cache",
        sql: include_str!("../migrations/sqlite/0002_codex_status_cache.sql"),
    },
    SqlMigration {
        version: 3,
        name: "kiro_full_request_logging",
        sql: include_str!("../migrations/sqlite/0003_kiro_full_request_logging.sql"),
    },
    SqlMigration {
        version: 4,
        name: "duckdb_usage_runtime_settings",
        sql: include_str!("../migrations/sqlite/0004_duckdb_usage_runtime_settings.sql"),
    },
    SqlMigration {
        version: 5,
        name: "account_contribution_validated_status",
        sql: include_str!("../migrations/sqlite/0005_account_contribution_validated_status.sql"),
    },
    SqlMigration {
        version: 6,
        name: "codex_account_import_jobs",
        sql: include_str!("../migrations/sqlite/0006_codex_account_import_jobs.sql"),
    },
    SqlMigration {
        version: 7,
        name: "usage_journal_runtime_settings",
        sql: include_str!("../migrations/sqlite/0007_usage_journal_runtime_settings.sql"),
    },
];

const DUCKDB_MIGRATIONS: &[SqlMigration] = &[
    SqlMigration {
        version: 1,
        name: "init",
        sql: include_str!("../migrations/duckdb/0001_init.sql"),
    },
    SqlMigration {
        version: 2,
        name: "drop_explicit_art_indexes",
        sql: include_str!("../migrations/duckdb/0002_drop_explicit_art_indexes.sql"),
    },
];

/// Return target SQLite migrations in execution order.
pub fn sqlite_migrations() -> &'static [SqlMigration] {
    SQLITE_MIGRATIONS
}

/// Return target DuckDB migrations in execution order.
pub fn duckdb_migrations() -> &'static [SqlMigration] {
    DUCKDB_MIGRATIONS
}

/// Return all DuckDB target schema SQL as one executable script.
pub fn duckdb_schema_sql() -> String {
    DUCKDB_MIGRATIONS
        .iter()
        .map(|migration| migration.sql)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run pending target SQLite migrations and record applied versions.
pub fn run_sqlite_migrations(conn: &rusqlite::Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("failed to enable sqlite foreign keys")?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .context("failed to enable sqlite WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")
        .context("failed to set sqlite synchronous mode")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .context("failed to configure sqlite busy timeout")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS llm_access_schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at_ms INTEGER NOT NULL CHECK (applied_at_ms >= 0)
        ) STRICT, WITHOUT ROWID;",
    )
    .context("failed to initialize sqlite migration metadata")?;

    for migration in SQLITE_MIGRATIONS {
        let already_applied: bool = conn
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM llm_access_schema_migrations WHERE version = ?1
                )",
                [migration.version],
                |row| row.get(0),
            )
            .with_context(|| format!("failed to inspect migration {}", migration.version))?;
        if already_applied {
            continue;
        }

        let tx = conn
            .unchecked_transaction()
            .with_context(|| format!("failed to begin migration {}", migration.version))?;
        tx.execute_batch(migration.sql)
            .with_context(|| format!("failed to run migration {}", migration.version))?;
        tx.execute(
            "INSERT INTO llm_access_schema_migrations (version, name, applied_at_ms)
             VALUES (?1, ?2, unixepoch('subsec') * 1000)",
            rusqlite::params![migration.version, migration.name],
        )
        .with_context(|| format!("failed to record migration {}", migration.version))?;
        tx.commit()
            .with_context(|| format!("failed to commit migration {}", migration.version))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn sqlite_migrations_are_file_backed_and_versioned() {
        let migrations = super::sqlite_migrations();

        assert_eq!(migrations.len(), 7);
        assert_eq!(migrations[0].version, 1);
        assert_eq!(migrations[0].name, "init");
        assert!(migrations[0]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_keys"));
        assert_eq!(migrations[1].version, 2);
        assert_eq!(migrations[1].name, "codex_status_cache");
        assert!(migrations[1]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_codex_status_cache"));
        assert_eq!(migrations[2].version, 3);
        assert_eq!(migrations[2].name, "kiro_full_request_logging");
        assert!(migrations[2]
            .sql
            .contains("kiro_full_request_logging_enabled"));
        assert_eq!(migrations[3].version, 4);
        assert_eq!(migrations[3].name, "duckdb_usage_runtime_settings");
        assert!(migrations[3].sql.contains("duckdb_usage_memory_limit_mib"));
        assert!(migrations[3]
            .sql
            .contains("duckdb_usage_checkpoint_threshold_mib"));
        assert_eq!(migrations[4].version, 5);
        assert_eq!(migrations[4].name, "account_contribution_validated_status");
        assert!(migrations[4].sql.contains("'validated'"));
        assert_eq!(migrations[5].version, 6);
        assert_eq!(migrations[5].name, "codex_account_import_jobs");
        assert!(migrations[5]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_account_import_jobs"));
        assert_eq!(migrations[6].version, 7);
        assert_eq!(migrations[6].name, "usage_journal_runtime_settings");
        assert!(migrations[6].sql.contains("usage_journal_enabled"));
        assert!(migrations[6].sql.contains("usage_query_base_url"));
    }

    #[test]
    fn duckdb_migrations_drop_legacy_explicit_art_indexes() {
        let migrations = super::duckdb_migrations();

        assert_eq!(migrations.len(), 2);
        assert_eq!(migrations[0].version, 1);
        assert_eq!(migrations[0].name, "init");
        assert!(!migrations[0]
            .sql
            .contains("CREATE INDEX IF NOT EXISTS idx_usage_events"));
        assert!(!migrations[0]
            .sql
            .contains("CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events"));
        assert_eq!(migrations[1].version, 2);
        assert_eq!(migrations[1].name, "drop_explicit_art_indexes");
        assert!(migrations[1]
            .sql
            .contains("DROP INDEX IF EXISTS idx_usage_events_source_event_id"));
        assert!(!super::duckdb_schema_sql().contains("cdc_"));
    }

    #[test]
    fn sqlite_runner_records_applied_versions() {
        let conn = rusqlite::Connection::open_in_memory().expect("open sqlite");

        super::run_sqlite_migrations(&conn).expect("run sqlite migrations");
        super::run_sqlite_migrations(&conn).expect("run sqlite migrations twice");

        let applied_count: i64 = conn
            .query_row("SELECT count(*) FROM llm_access_schema_migrations", [], |row| row.get(0))
            .expect("count migrations");
        assert_eq!(applied_count, 7);

        let full_logging_column_count: i64 = conn
            .query_row(
                "SELECT count(*)
                 FROM pragma_table_info('llm_key_route_config')
                 WHERE name = 'kiro_full_request_logging_enabled'",
                [],
                |row| row.get(0),
            )
            .expect("inspect route config columns");
        assert_eq!(full_logging_column_count, 1);

        let runtime_duckdb_column_count: i64 = conn
            .query_row(
                "SELECT count(*)
                 FROM pragma_table_info('llm_runtime_config')
                 WHERE name IN (
                    'duckdb_usage_memory_limit_mib',
                    'duckdb_usage_checkpoint_threshold_mib'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("inspect runtime config duckdb columns");
        assert_eq!(runtime_duckdb_column_count, 2);

        let runtime_journal_column_count: i64 = conn
            .query_row(
                "SELECT count(*)
                 FROM pragma_table_info('llm_runtime_config')
                 WHERE name IN (
                    'usage_journal_enabled',
                    'usage_journal_max_file_bytes',
                    'usage_journal_max_file_age_ms',
                    'usage_journal_max_files',
                    'usage_journal_block_target_uncompressed_bytes',
                    'usage_journal_block_max_events',
                    'usage_journal_fsync_interval_ms',
                    'usage_journal_zstd_level',
                    'usage_journal_consumer_lease_ms',
                    'usage_journal_delete_bad_files',
                    'usage_query_bind_addr',
                    'usage_query_base_url'
                 )",
                [],
                |row| row.get(0),
            )
            .expect("inspect runtime config journal columns");
        assert_eq!(runtime_journal_column_count, 12);

        conn.execute(
            "INSERT INTO llm_account_contribution_requests (
                request_id, account_name, id_token, access_token, refresh_token,
                requester_email, contributor_message, status, fingerprint, client_ip,
                ip_region, created_at_ms, updated_at_ms
            ) VALUES (
                'llmacct-validated', 'acct-validated', '', 'access', 'refresh',
                '', 'ok', 'validated', 'fp', '127.0.0.1', 'unknown', 1, 1
            )",
            [],
        )
        .expect("validated account contribution status is allowed");

        conn.execute(
            "INSERT INTO llm_account_import_jobs (
                job_id, provider_type, source_type, validate_before_import, status,
                total_count, completed_count, succeeded_count, skipped_count, failed_count,
                created_at_ms, updated_at_ms
            ) VALUES (
                'llm-import-test', 'codex', 'local_json', 1, 'pending',
                1, 0, 0, 0, 0, 1, 1
            )",
            [],
        )
        .expect("codex import job row is allowed");
        conn.execute(
            "INSERT INTO llm_account_import_job_items (
                job_id, item_index, requested_name, requested_account_id, raw_auth_json,
                status, created_at_ms, updated_at_ms
            ) VALUES (
                'llm-import-test', 0, 'codex-a', 'acct-a', '{\"refresh_token\":\"rt\"}',
                'pending', 1, 1
            )",
            [],
        )
        .expect("codex import job item row is allowed");
    }
}
