//! Versioned SQL migrations for the standalone LLM access service.

use anyhow::{Context, Result};
use sqlx_core::{query, query_scalar, raw_sql};

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
    SqlMigration {
        version: 11,
        name: "proxy_config_node_overrides",
        sql: include_str!("../migrations/postgres/0011_proxy_config_node_overrides.sql"),
    },
    SqlMigration {
        version: 12,
        name: "proxy_config_endpoint_checks",
        sql: include_str!("../migrations/postgres/0012_proxy_config_endpoint_checks.sql"),
    },
    SqlMigration {
        version: 13,
        name: "kiro_remote_media_resolution",
        sql: include_str!("../migrations/postgres/0013_kiro_remote_media_resolution.sql"),
    },
    SqlMigration {
        version: 14,
        name: "codex_fast_toggle",
        sql: include_str!("../migrations/postgres/0014_codex_fast_toggle.sql"),
    },
    SqlMigration {
        version: 15,
        name: "usage_catalog",
        sql: include_str!("../migrations/postgres/0015_usage_catalog.sql"),
    },
    SqlMigration {
        version: 16,
        name: "usage_catalog_segment_filters",
        sql: include_str!("../migrations/postgres/0016_usage_catalog_segment_filters.sql"),
    },
    SqlMigration {
        version: 17,
        name: "kiro_latency_routing_toggle",
        sql: include_str!("../migrations/postgres/0017_kiro_latency_routing_toggle.sql"),
    },
    SqlMigration {
        version: 18,
        name: "kiro_context_usage_threshold",
        sql: include_str!("../migrations/postgres/0018_kiro_context_usage_threshold.sql"),
    },
    SqlMigration {
        version: 19,
        name: "kiro_compact_trigger",
        sql: include_str!("../migrations/postgres/0019_kiro_compact_trigger.sql"),
    },
];

/// Return target DuckDB migrations in execution order.
pub fn duckdb_migrations() -> &'static [SqlMigration] {
    DUCKDB_MIGRATIONS
}

/// Return target Postgres migrations in execution order.
pub fn postgres_migrations() -> &'static [SqlMigration] {
    POSTGRES_MIGRATIONS
}

/// Return all DuckDB target schema SQL as one executable script.
pub fn duckdb_schema_sql() -> String {
    DUCKDB_MIGRATIONS
        .iter()
        .map(|migration| migration.sql)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Run pending target Postgres migrations and record applied versions.
pub async fn run_postgres_migrations(pool: &sqlx_postgres::PgPool) -> Result<()> {
    raw_sql::raw_sql(
        "CREATE TABLE IF NOT EXISTS llm_access_schema_migrations (
            version BIGINT PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at_ms BIGINT NOT NULL CHECK (applied_at_ms >= 0)
        );",
    )
    .execute(pool)
    .await
    .context("failed to initialize postgres migration metadata")?;

    for migration in POSTGRES_MIGRATIONS {
        let already_applied = query_scalar::query_scalar::<_, bool>(
            "SELECT EXISTS(
                SELECT 1 FROM llm_access_schema_migrations WHERE version = $1
            )",
        )
        .bind(migration.version)
        .fetch_one(pool)
        .await
        .with_context(|| format!("failed to inspect migration {}", migration.version))?;
        if already_applied {
            continue;
        }

        let mut tx = pool
            .begin()
            .await
            .with_context(|| format!("failed to begin migration {}", migration.version))?;
        raw_sql::raw_sql(migration.sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("failed to run migration {}", migration.version))?;
        query::query(
            "INSERT INTO llm_access_schema_migrations (version, name, applied_at_ms)
             VALUES ($1, $2, (EXTRACT(EPOCH FROM clock_timestamp()) * 1000)::bigint)",
        )
        .bind(migration.version)
        .bind(migration.name)
        .execute(&mut *tx)
        .await
        .with_context(|| format!("failed to record migration {}", migration.version))?;
        tx.commit()
            .await
            .with_context(|| format!("failed to commit migration {}", migration.version))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
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
    fn postgres_migrations_are_file_backed_and_versioned() {
        let migrations = super::postgres_migrations();

        assert_eq!(migrations[0].version, 1);
        assert_eq!(migrations[0].name, "init");
        assert!(migrations[0]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_keys"));
        assert!(migrations
            .iter()
            .any(|migration| migration.sql.contains("llm_runtime_config")));
    }

    #[test]
    fn postgres_migrations_include_proxy_node_overrides() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "proxy_config_node_overrides")
            .expect("proxy node override migration exists");

        assert_eq!(migration.version, 11);
        assert!(migration.sql.contains("llm_proxy_config_node_overrides"));
        assert!(migration
            .sql
            .contains("PRIMARY KEY (proxy_config_id, node_id)"));
        assert!(migration
            .sql
            .contains("REFERENCES llm_proxy_configs(proxy_config_id)"));
    }

    #[test]
    fn postgres_migrations_include_kiro_remote_media_resolution_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_remote_media_resolution")
            .expect("kiro remote media migration exists");

        assert_eq!(migration.version, 13);
        assert!(migration
            .sql
            .contains("kiro_remote_media_resolution_enabled"));
    }

    #[test]
    fn postgres_migrations_include_codex_fast_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_fast_toggle")
            .expect("codex fast migration exists");

        assert_eq!(migration.version, 14);
        assert!(migration.sql.contains("codex_fast_enabled"));
        assert!(migration.sql.contains("ADD COLUMN IF NOT EXISTS"));
    }

    #[test]
    fn postgres_migrations_include_usage_catalog_tables() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "usage_catalog")
            .expect("usage catalog migration exists");

        assert_eq!(migration.version, 15);
        assert!(migration
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_usage_segments"));
        assert!(migration
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_usage_segment_events"));
        assert!(migration
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_usage_segment_key_rollups"));
    }

    #[test]
    fn postgres_migrations_include_kiro_latency_routing_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_latency_routing_toggle")
            .expect("kiro latency routing migration exists");

        assert_eq!(migration.version, 17);
        assert!(migration.sql.contains("kiro_latency_routing_enabled"));
        assert!(migration.sql.contains("DEFAULT TRUE"));
    }

    #[test]
    fn postgres_migrations_include_kiro_context_usage_threshold() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_context_usage_threshold")
            .expect("kiro context usage threshold migration exists");

        assert_eq!(migration.version, 18);
        assert!(migration
            .sql
            .contains("kiro_context_usage_min_request_tokens"));
        assert!(migration.sql.contains("DEFAULT 15000"));
    }
}
