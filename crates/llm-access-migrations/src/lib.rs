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
    SqlMigration {
        version: 3,
        name: "proxy_traffic_rollups",
        sql: include_str!("../migrations/duckdb/0003_proxy_traffic_rollups.sql"),
    },
    SqlMigration {
        version: 4,
        name: "usage_error_classification",
        sql: include_str!("../migrations/duckdb/0004_usage_error_classification.sql"),
    },
    SqlMigration {
        version: 5,
        name: "usage_image_metrics",
        sql: include_str!("../migrations/duckdb/0005_usage_image_metrics.sql"),
    },
    SqlMigration {
        version: 6,
        name: "usage_retry_details",
        sql: include_str!("../migrations/duckdb/0006_usage_retry_details.sql"),
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
    SqlMigration {
        version: 20,
        name: "kiro_protected_content_validation",
        sql: include_str!("../migrations/postgres/0020_kiro_protected_content_validation.sql"),
    },
    SqlMigration {
        version: 21,
        name: "kiro_cctest_text_handling",
        sql: include_str!("../migrations/postgres/0021_kiro_cctest_text_handling.sql"),
    },
    SqlMigration {
        version: 22,
        name: "kiro_cctest_proxy_config",
        sql: include_str!("../migrations/postgres/0022_kiro_cctest_proxy_config.sql"),
    },
    SqlMigration {
        version: 23,
        name: "codex_session_affinity_config",
        sql: include_str!("../migrations/postgres/0023_codex_session_affinity_config.sql"),
    },
    SqlMigration {
        version: 24,
        name: "kiro_cache_snapshot_config",
        sql: include_str!("../migrations/postgres/0024_kiro_cache_snapshot_config.sql"),
    },
    SqlMigration {
        version: 25,
        name: "usage_rollup_applied_batches",
        sql: include_str!("../migrations/postgres/0025_usage_rollup_applied_batches.sql"),
    },
    SqlMigration {
        version: 26,
        name: "kiro_pool_strategy",
        sql: include_str!("../migrations/postgres/0026_kiro_pool_strategy.sql"),
    },
    SqlMigration {
        version: 27,
        name: "proxy_config_traffic_snapshots",
        sql: include_str!("../migrations/postgres/0027_proxy_config_traffic_snapshots.sql"),
    },
    SqlMigration {
        version: 28,
        name: "codex_client_version_0142",
        sql: include_str!("../migrations/postgres/0028_codex_client_version_0142.sql"),
    },
    SqlMigration {
        version: 29,
        name: "codex_strict_session_rejection",
        sql: include_str!("../migrations/postgres/0029_codex_strict_session_rejection.sql"),
    },
    SqlMigration {
        version: 30,
        name: "codex_image_generation_toggle",
        sql: include_str!("../migrations/postgres/0030_codex_image_generation_toggle.sql"),
    },
    SqlMigration {
        version: 31,
        name: "codex_image_key_usage_rollup",
        sql: include_str!("../migrations/postgres/0031_codex_image_key_usage_rollup.sql"),
    },
    SqlMigration {
        version: 32,
        name: "codex_image_direct_toggle",
        sql: include_str!("../migrations/postgres/0032_codex_image_direct_toggle.sql"),
    },
    SqlMigration {
        version: 33,
        name: "anthropic_upstream_pool",
        sql: include_str!("../migrations/postgres/0033_anthropic_upstream_pool.sql"),
    },
    SqlMigration {
        version: 34,
        name: "anthropic_upstream_probe_state",
        sql: include_str!("../migrations/postgres/0034_anthropic_upstream_probe_state.sql"),
    },
    SqlMigration {
        version: 35,
        name: "kiro_model_group_preferences",
        sql: include_str!("../migrations/postgres/0035_kiro_model_group_preferences.sql"),
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

        assert_eq!(migrations.len(), 6);
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
        assert_eq!(migrations[2].version, 3);
        assert_eq!(migrations[2].name, "proxy_traffic_rollups");
        assert!(migrations[2]
            .sql
            .contains("CREATE TABLE IF NOT EXISTS proxy_traffic_rollups_hourly"));
        assert_eq!(migrations[3].version, 4);
        assert_eq!(migrations[3].name, "usage_error_classification");
        assert!(migrations[3]
            .sql
            .contains("ADD COLUMN IF NOT EXISTS error_class"));
        assert!(migrations[3]
            .sql
            .contains("ADD COLUMN IF NOT EXISTS session_blocked"));
        assert_eq!(migrations[4].version, 5);
        assert_eq!(migrations[4].name, "usage_image_metrics");
        assert!(migrations[4]
            .sql
            .contains("ADD COLUMN IF NOT EXISTS response_image_count"));
        assert_eq!(migrations[5].version, 6);
        assert_eq!(migrations[5].name, "usage_retry_details");
        assert!(migrations[5]
            .sql
            .contains("ADD COLUMN IF NOT EXISTS same_account_retry_count"));
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
    fn postgres_migrations_register_every_postgres_file() {
        let migrations = super::postgres_migrations();
        let registered_versions = migrations
            .iter()
            .map(|migration| migration.version)
            .collect::<std::collections::BTreeSet<_>>();
        let migration_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("migrations")
            .join("postgres");
        let file_versions = std::fs::read_dir(&migration_dir)
            .expect("read postgres migrations")
            .map(|entry| {
                let path = entry.expect("migration entry").path();
                let file_name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .expect("utf8 migration file name");
                file_name
                    .split_once('_')
                    .expect("versioned migration file name")
                    .0
                    .parse::<i64>()
                    .expect("numeric migration version")
            })
            .collect::<std::collections::BTreeSet<_>>();

        assert_eq!(registered_versions, file_versions);
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

    #[test]
    fn postgres_migrations_include_kiro_protected_content_validation_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_protected_content_validation")
            .expect("kiro protected content validation migration exists");

        assert_eq!(migration.version, 20);
        assert!(migration
            .sql
            .contains("kiro_protected_content_validation_enabled"));
        assert!(migration.sql.contains("DEFAULT FALSE"));
    }

    #[test]
    fn postgres_migrations_include_kiro_cctest_text_handling() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_cctest_text_handling")
            .expect("kiro cctest text handling migration exists");

        assert_eq!(migration.version, 21);
        assert!(migration.sql.contains("kiro_cctest_text_handling_enabled"));
        assert!(migration.sql.contains("DEFAULT FALSE"));
    }

    #[test]
    fn postgres_migrations_include_kiro_cctest_proxy_config() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "kiro_cctest_proxy_config")
            .expect("kiro cctest proxy config migration exists");

        assert_eq!(migration.version, 22);
        assert!(migration.sql.contains("kiro_cctest_proxy_base_url"));
        assert!(migration.sql.contains("kiro_cctest_proxy_api_key"));
    }

    #[test]
    fn postgres_migrations_include_codex_session_affinity_config() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_session_affinity_config")
            .expect("codex session affinity config migration exists");

        assert_eq!(migration.version, 23);
        assert!(migration.sql.contains("codex_session_affinity_enabled"));
        assert!(migration
            .sql
            .contains("codex_fallback_affinity_prefix_bytes"));
    }

    #[test]
    fn postgres_migrations_include_proxy_config_traffic_snapshots() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "proxy_config_traffic_snapshots")
            .expect("proxy traffic snapshot migration exists");

        assert_eq!(migration.version, 27);
        assert!(migration
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_proxy_config_traffic_snapshots"));
        assert!(migration
            .sql
            .contains("REFERENCES llm_proxy_configs(proxy_config_id) ON DELETE CASCADE"));
        assert!(migration.sql.contains("total_bytes BIGINT NOT NULL"));
    }

    #[test]
    fn postgres_migrations_include_codex_client_version_0142() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_client_version_0142")
            .expect("codex client version migration exists");

        assert_eq!(migration.version, 28);
        assert!(migration.sql.contains("codex_client_version = '0.142.0'"));
    }

    #[test]
    fn postgres_migrations_include_codex_strict_session_rejection() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_strict_session_rejection")
            .expect("codex strict session rejection migration exists");

        assert_eq!(migration.version, 29);
        assert!(migration
            .sql
            .contains("codex_strict_session_rejection_enabled"));
        assert!(migration.sql.contains("DEFAULT FALSE"));
    }

    #[test]
    fn postgres_migrations_include_codex_image_generation_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_image_generation_toggle")
            .expect("codex image generation migration exists");

        assert_eq!(migration.version, 30);
        assert!(migration.sql.contains("codex_image_generation_enabled"));
        assert!(migration.sql.contains("DEFAULT FALSE"));
    }

    #[test]
    fn postgres_migrations_include_codex_image_key_usage_rollup() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_image_key_usage_rollup")
            .expect("codex image key usage rollup migration exists");

        assert_eq!(migration.version, 31);
        assert!(migration.sql.contains("codex_image_usage_tokens"));
        assert!(migration.sql.contains("codex_image_usage_missing_events"));
        assert!(migration.sql.contains("codex_image_last_used_at_ms"));
    }

    #[test]
    fn postgres_migrations_include_codex_image_direct_toggle() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "codex_image_direct_toggle")
            .expect("codex image direct toggle migration exists");

        assert_eq!(migration.version, 32);
        assert!(migration
            .sql
            .contains("codex_image_direct_generation_enabled"));
        assert!(migration.sql.contains("DEFAULT TRUE"));
        assert!(migration.sql.contains("DEFAULT FALSE"));
    }

    #[test]
    fn postgres_migrations_include_anthropic_upstream_pool() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "anthropic_upstream_pool")
            .expect("anthropic upstream pool migration exists");

        assert_eq!(migration.version, 33);
        assert!(migration.sql.contains("kiro_anthropic_upstream_pool_mode"));
        assert!(migration
            .sql
            .contains("CREATE TABLE IF NOT EXISTS llm_anthropic_upstream_channels"));
        assert!(migration
            .sql
            .contains("llm_anthropic_upstream_channel_usage_rollups"));
        assert!(migration.sql.contains("DEFAULT 'disabled'"));
    }

    #[test]
    fn postgres_migrations_include_anthropic_upstream_probe_state() {
        let migrations = super::postgres_migrations();
        let migration = migrations
            .iter()
            .find(|migration| migration.name == "anthropic_upstream_probe_state")
            .expect("anthropic upstream probe state migration exists");

        assert_eq!(migration.version, 34);
        assert!(migration.sql.contains("model_ids JSONB"));
        assert!(migration.sql.contains("last_models_checked_at_ms"));
        assert!(migration.sql.contains("last_test_model"));
    }
}
