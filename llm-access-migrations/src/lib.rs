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

        assert_eq!(migrations.len(), 3);
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
        assert_eq!(applied_count, 3);

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
    }
}
