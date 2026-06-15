#[cfg(feature = "duckdb-runtime")]
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        KiroLatencyRankingQuery, ProxyTrafficQuery, UsageAnalyticsStore, UsageEventQuery,
        UsageEventSink, UsageEventSource, UsageEventStatusKind, UsageFilterOptions,
        UsageMetricsQuery,
    },
    usage::{UsageEvent, UsageStreamDetails, UsageTiming},
};

#[cfg(feature = "duckdb-runtime")]
fn test_usage_event() -> UsageEvent {
    UsageEvent {
        event_id: "duckdb-test-event".to_string(),
        created_at_ms: 1_700_000_000_000,
        provider_type: ProviderType::Kiro,
        protocol_family: ProtocolFamily::Anthropic,
        key_id: "key-duckdb".to_string(),
        key_name: "DuckDB Key".to_string(),
        account_name: Some("kiro-account".to_string()),
        account_group_id_at_event: Some("group-duckdb".to_string()),
        route_strategy_at_event: Some(RouteStrategy::Auto),
        request_method: "POST".to_string(),
        request_url: "https://example.test/api/kiro-gateway/cc/v1/messages".to_string(),
        endpoint: "/cc/v1/messages".to_string(),
        model: Some("claude-sonnet-4-5".to_string()),
        mapped_model: Some("claude-sonnet-4-5".to_string()),
        status_code: 200,
        request_body_bytes: Some(1234),
        quota_failover_count: 2,
        routing_diagnostics_json: Some(r#"{"route":"auto"}"#.to_string()),
        input_uncached_tokens: 10,
        input_cached_tokens: 20,
        output_tokens: 30,
        billable_tokens: 40,
        credit_usage: Some("0.5".to_string()),
        usage_missing: false,
        credit_usage_missing: false,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: r#"{"host":["example.test"]}"#.to_string(),
        last_message_content: Some("hello".to_string()),
        client_request_body_json: Some(r#"{"model":"claude-sonnet-4-5"}"#.to_string()),
        upstream_request_body_json: Some(r#"{"conversationState":{}}"#.to_string()),
        full_request_json: Some(r#"{"model":"claude-sonnet-4-5"}"#.to_string()),
        error_message: None,
        error_body: None,
        response_body: None,
        timing: UsageTiming {
            latency_ms: Some(55),
            routing_wait_ms: Some(5),
            upstream_headers_ms: Some(11),
            post_headers_body_ms: Some(22),
            request_body_read_ms: Some(3),
            request_json_parse_ms: Some(4),
            pre_handler_ms: Some(7),
            first_sse_write_ms: Some(33),
            stream_finish_ms: Some(44),
        },
        stream: UsageStreamDetails {
            stream_completed_cleanly: Some(true),
            downstream_disconnect: Some(false),
            final_event_type: Some("message_stop".to_string()),
            bytes_streamed: Some(2048),
        },
    }
}

#[cfg(feature = "duckdb-runtime")]
fn assert_usage_event_round_trips(actual: &UsageEvent, expected: &UsageEvent) {
    let actual_credit = actual
        .credit_usage
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok());
    let expected_credit = expected
        .credit_usage
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok());
    assert_eq!(actual_credit, expected_credit);

    let mut actual_without_decimal_format = actual.clone();
    actual_without_decimal_format.credit_usage = expected.credit_usage.clone();
    assert_eq!(actual_without_decimal_format, expected.clone());
}

#[cfg(feature = "duckdb-runtime")]
fn assert_usage_event_summary_round_trips(actual: &UsageEvent, expected: &UsageEvent) {
    let mut expected_summary = expected.clone();
    expected_summary.request_headers_json = "{}".to_string();
    expected_summary.routing_diagnostics_json = None;
    expected_summary.last_message_content = None;
    expected_summary.client_request_body_json = None;
    expected_summary.upstream_request_body_json = None;
    expected_summary.full_request_json = None;
    expected_summary.error_message = None;
    expected_summary.error_body = None;
    assert_usage_event_round_trips(actual, &expected_summary);
}

#[cfg(feature = "duckdb-runtime")]
fn assert_usage_event_light_detail_round_trips(actual: &UsageEvent, expected: &UsageEvent) {
    let mut expected_summary = expected.clone();
    expected_summary.client_request_body_json = None;
    expected_summary.upstream_request_body_json = None;
    expected_summary.full_request_json = None;
    expected_summary.error_message = None;
    expected_summary.error_body = None;
    assert_usage_event_round_trips(actual, &expected_summary);
}

#[cfg(feature = "duckdb-runtime")]
fn assert_usage_event_detail_payloads(actual: &UsageEvent, expected: &UsageEvent) {
    assert_eq!(actual.request_headers_json, expected.request_headers_json);
    assert_eq!(actual.routing_diagnostics_json, expected.routing_diagnostics_json);
    assert_eq!(actual.last_message_content, expected.last_message_content);
    assert_eq!(actual.client_request_body_json, expected.client_request_body_json);
    assert_eq!(actual.upstream_request_body_json, expected.upstream_request_body_json);
    assert_eq!(actual.full_request_json, expected.full_request_json);
    assert_eq!(actual.error_message, expected.error_message);
    assert_eq!(actual.error_body, expected.error_body);
}

#[cfg(feature = "duckdb-runtime")]
fn details_store_dir(root: &std::path::Path) -> std::path::PathBuf {
    root.join("usage-details")
}

#[cfg(feature = "duckdb-runtime")]
fn legacy_details_store_object_path(
    root: &std::path::Path,
    event: &UsageEvent,
) -> std::path::PathBuf {
    let ts = chrono::DateTime::from_timestamp_millis(event.created_at_ms)
        .expect("valid usage event timestamp");
    details_store_dir(root)
        .join(event.provider_type.as_storage_str())
        .join(ts.format("%Y").to_string())
        .join(ts.format("%m").to_string())
        .join(ts.format("%d").to_string())
        .join(format!("{}.json.gz", event.event_id))
}

#[cfg(feature = "duckdb-runtime")]
fn archived_segment_path_for_timestamp(
    config: &super::TieredDuckDbUsageConfig,
    segment_id: &str,
    timestamp_ms: i64,
) -> std::path::PathBuf {
    super::archive_segment_path_for_timestamp(config, segment_id, timestamp_ms)
}

#[cfg(feature = "duckdb-runtime")]
fn test_catalog_backend(
    config: &super::TieredDuckDbUsageConfig,
) -> super::TieredUsageCatalogBackend {
    super::TieredUsageCatalogBackend::Test(std::sync::Arc::new(
        super::TestTieredUsageCatalog::open(super::test_catalog_state_path(config))
            .expect("open test usage catalog"),
    ))
}

#[cfg(feature = "duckdb-runtime")]
fn create_legacy_usage_archive_without_stream_columns(path: &std::path::Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create legacy archive parent directory");
    }
    let conn = duckdb::Connection::open(path).expect("open legacy archive");
    conn.execute_batch(
        r#"
        CREATE TABLE usage_events (
            source_seq BIGINT NOT NULL,
            source_event_id VARCHAR NOT NULL,
            event_id VARCHAR PRIMARY KEY,
            created_at_ms BIGINT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL,
            created_date DATE NOT NULL,
            created_hour TIMESTAMP NOT NULL,
            provider_type VARCHAR NOT NULL,
            protocol_family VARCHAR NOT NULL,
            key_id VARCHAR NOT NULL,
            key_name VARCHAR NOT NULL,
            key_status_at_event VARCHAR NOT NULL,
            account_name VARCHAR,
            account_group_id_at_event VARCHAR,
            route_strategy_at_event VARCHAR,
            request_method VARCHAR NOT NULL DEFAULT 'POST',
            request_url VARCHAR NOT NULL DEFAULT '',
            endpoint VARCHAR NOT NULL,
            model VARCHAR,
            mapped_model VARCHAR,
            status_code INTEGER NOT NULL,
            latency_ms INTEGER,
            routing_wait_ms INTEGER,
            upstream_headers_ms INTEGER,
            post_headers_body_ms INTEGER,
            request_body_read_ms INTEGER,
            request_json_parse_ms INTEGER,
            pre_handler_ms INTEGER,
            first_sse_write_ms INTEGER,
            stream_finish_ms INTEGER,
            request_body_bytes BIGINT,
            quota_failover_count BIGINT NOT NULL DEFAULT 0,
            routing_diagnostics_json VARCHAR,
            input_uncached_tokens BIGINT NOT NULL,
            input_cached_tokens BIGINT NOT NULL,
            output_tokens BIGINT NOT NULL,
            billable_tokens BIGINT NOT NULL,
            credit_usage DECIMAL(24, 12),
            usage_missing BOOLEAN NOT NULL,
            credit_usage_missing BOOLEAN NOT NULL,
            client_ip VARCHAR,
            ip_region VARCHAR,
            request_headers_json VARCHAR NOT NULL DEFAULT '{}',
            last_message_content VARCHAR,
            client_request_body_json VARCHAR,
            upstream_request_body_json VARCHAR,
            full_request_json VARCHAR
        );
        INSERT INTO usage_events (
            source_seq, source_event_id, event_id, created_at_ms, created_at,
            created_date, created_hour, provider_type, protocol_family, key_id,
            key_name, key_status_at_event, account_name, account_group_id_at_event,
            route_strategy_at_event, request_method, request_url, endpoint, model,
            mapped_model, status_code, latency_ms, routing_wait_ms,
            upstream_headers_ms, post_headers_body_ms, request_body_read_ms,
            request_json_parse_ms, pre_handler_ms, first_sse_write_ms,
            stream_finish_ms, request_body_bytes, quota_failover_count,
            routing_diagnostics_json, input_uncached_tokens, input_cached_tokens,
            output_tokens, billable_tokens, credit_usage, usage_missing,
            credit_usage_missing, client_ip, ip_region, request_headers_json,
            last_message_content, client_request_body_json, upstream_request_body_json,
            full_request_json
        ) VALUES (
            0, 'legacy-source-event', 'legacy-archive-event', 1700000000000,
            to_timestamp(1700000000), CAST(to_timestamp(1700000000) AS DATE),
            date_trunc('hour', to_timestamp(1700000000)), 'kiro', 'anthropic',
            'key-duckdb', 'DuckDB Key', 'active', 'kiro-account', 'group-duckdb',
            'auto', 'POST', 'https://example.test/api/kiro-gateway/cc/v1/messages',
            '/cc/v1/messages', 'claude-sonnet-4-5', 'claude-sonnet-4-5',
            200, 55, 5, 11, 22, 3, 4, 7, 33, 44, 1234, 2,
            '{"route":"legacy"}', 10, 20, 30, 40, 0.5, false, false,
            '127.0.0.1', 'local', '{"host":["example.test"]}', 'hello',
            '{"model":"claude-sonnet-4-5"}', '{"conversationState":{}}',
            '{"model":"claude-sonnet-4-5"}'
        );
        CHECKPOINT;
        "#,
    )
    .expect("create legacy archive schema");
}

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
        "account_group_id_at_event",
        "route_strategy_at_event",
        "endpoint",
        "status_code",
        "upstream_headers_ms",
        "post_headers_body_ms",
        "first_sse_write_ms",
        "stream_finish_ms",
        "stream_completed_cleanly",
        "downstream_disconnect",
        "final_event_type",
        "bytes_streamed",
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

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_persists_usage_events_with_default_feature() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-duckdb-repository", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
    let mut event = test_usage_event();
    event.routing_diagnostics_json = Some(r#"{"route":"diagnostic"}"#.to_string());
    event.last_message_content = Some("x".repeat(4096));

    repo.append_usage_event(&event)
        .await
        .expect("append duckdb usage event");

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(event.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list duckdb usage events");
    assert_eq!(page.total, 1);
    assert_eq!(page.events.len(), 1);
    assert_usage_event_summary_round_trips(&page.events[0], &event);
    assert_eq!(page.events[0].request_headers_json, "{}");
    assert_eq!(page.events[0].routing_diagnostics_json, None);
    assert_eq!(page.events[0].last_message_content, None);
    assert_eq!(page.events[0].client_request_body_json, None);
    assert_eq!(page.events[0].upstream_request_body_json, None);
    assert_eq!(page.events[0].full_request_json, None);

    let detail = repo
        .get_usage_event(&event.event_id)
        .await
        .expect("get duckdb usage event")
        .expect("duckdb usage event exists");
    assert_usage_event_round_trips(&detail, &event);

    let chart = repo
        .usage_chart_points(&event.key_id, event.created_at_ms, 60_000, 1)
        .await
        .expect("query duckdb usage chart");
    assert_eq!(chart.len(), 1);
    assert_eq!(chart[0].bucket_start_ms, event.created_at_ms);
    assert_eq!(chart[0].tokens, 40);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_single_repository_keeps_writer_open_between_appends() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-duckdb-single-writer", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "single-writer-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    repo.append_usage_event(&first)
        .await
        .expect("append first usage event");

    let wal_path = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Single {
            state, ..
        } => {
            let state = state.lock().expect("lock single duckdb state");
            assert!(
                state.writer.is_some(),
                "single-file repository should keep the writer open after append"
            );
            super::duckdb_wal_path(&state.path)
        },
        _ => panic!("expected single repository"),
    };
    assert!(wal_path.exists(), "single-file WAL should remain present while the writer stays open");

    let mut second = test_usage_event();
    second.event_id = "single-writer-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    repo.append_usage_event(&second)
        .await
        .expect("append second usage event");

    match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Single {
            state, ..
        } => {
            let state = state.lock().expect("lock single duckdb state");
            assert!(
                state.writer.is_some(),
                "single-file repository should reuse the persistent writer"
            );
        },
        _ => panic!("expected single repository"),
    }
    assert!(wal_path.exists(), "single-file WAL should still be present after the second append");

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn duckdb_usage_connection_config_formats_runtime_limits() {
    let config = super::DuckDbUsageConnectionConfig {
        memory_limit_mib: 1024,
        checkpoint_threshold_mib: 32,
    };
    let sql = super::duckdb_usage_connection_sql(&config, "/tmp/staticflow-duckdb");

    assert!(sql.contains("SET memory_limit='1024MB'"));
    assert!(sql.contains("SET checkpoint_threshold='32MB'"));
    assert!(sql.contains("SET TimeZone='UTC'"));
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn duckdb_compact_connection_config_uses_runtime_memory_limit() {
    let sql = super::duckdb_compact_connection_sql(
        super::DuckDbUsageConnectionConfig {
            memory_limit_mib: 2048,
            checkpoint_threshold_mib: 16,
        },
        "/tmp/staticflow-duckdb-compact",
    );

    assert!(sql.contains("SET memory_limit='2048MB'"));
    assert!(sql.contains("SET max_temp_directory_size='8GB'"));
    assert!(sql.contains("SET TimeZone='UTC'"));
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn tiered_active_writer_uses_runtime_checkpoint_threshold_directly() {
    let config = super::DuckDbUsageConnectionConfig {
        memory_limit_mib: 1024,
        checkpoint_threshold_mib: 8,
    };
    assert_eq!(config.memory_limit_mib, 1024);
    assert_eq!(config.checkpoint_threshold_mib, 8);
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn tiered_usage_detail_store_rejects_non_file_backends() {
    let err = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: std::env::temp_dir().join("llm-access-active-reject-remote"),
        archive_dir: std::env::temp_dir().join("llm-access-archive-reject-remote"),
        rollover_bytes: u64::MAX,
        details_dir: Some(std::path::PathBuf::from("s3://should-not-work")),
    })
    .expect_err("non-local details dir must fail");

    assert!(err.to_string().contains("local filesystem path"));
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn tiered_usage_detail_prune_removes_only_expired_day_buckets() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-detail-retention-buckets", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create detail retention test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    };
    let now_ms = 1_700_864_000_000;
    let day_ms = 86_400_000;
    let expired_day = details_store_dir(&root)
        .join("packs/kiro")
        .join(super::archive_segment_bucket_dir(now_ms - 8 * day_ms));
    let retained_day = details_store_dir(&root)
        .join("packs/kiro")
        .join(super::archive_segment_bucket_dir(now_ms - 2 * day_ms));
    std::fs::create_dir_all(&expired_day).expect("create expired detail day");
    std::fs::create_dir_all(&retained_day).expect("create retained detail day");
    std::fs::write(expired_day.join("expired.detailpack-v1"), b"expired")
        .expect("write expired detail pack");
    std::fs::write(retained_day.join("retained.detailpack-v1"), b"retained")
        .expect("write retained detail pack");

    let (deleted_files, deleted_dirs) = super::prune_expired_detail_day_buckets(
        &config,
        super::usage_analytics_retention_cutoff_ms(now_ms, 7),
    )
    .expect("prune detail day buckets");

    assert_eq!(deleted_files, 1);
    assert!(deleted_dirs >= 1);
    assert!(!expired_day.exists());
    assert!(retained_day.exists());

    std::fs::remove_dir_all(&root).expect("cleanup detail retention test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_persists_usage_event_batches() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-duckdb-batch-repository", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
    let mut first = test_usage_event();
    first.event_id = "batch-first".to_string();
    let mut second = test_usage_event();
    second.event_id = "batch-second".to_string();
    second.created_at_ms = second.created_at_ms.saturating_add(1);

    repo.append_usage_events(&[first.clone(), second.clone()])
        .await
        .expect("append duckdb usage event batch");

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list duckdb usage events");
    assert_eq!(page.total, 2);
    assert_eq!(page.events.len(), 2);
    assert_usage_event_summary_round_trips(&page.events[0], &second);
    assert_usage_event_summary_round_trips(&page.events[1], &first);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_append_usage_events_ignores_segment_local_duplicates() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-dedup-batch-repository", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    let mut existing = test_usage_event();
    existing.event_id = "dedup-existing".to_string();
    repo.append_usage_event(&existing)
        .await
        .expect("append existing event");

    let mut first_new = test_usage_event();
    first_new.event_id = "dedup-new-first".to_string();
    first_new.created_at_ms = first_new.created_at_ms.saturating_add(1);
    let mut second_new = test_usage_event();
    second_new.event_id = "dedup-new-second".to_string();
    second_new.created_at_ms = second_new.created_at_ms.saturating_add(2);

    repo.append_usage_events(&[
        existing.clone(),
        first_new.clone(),
        first_new.clone(),
        second_new.clone(),
    ])
    .await
    .expect("append deduplicated batch");

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(existing.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list deduplicated page");
    assert_eq!(page.total, 3);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_separates_detail_payloads_from_usage_fact_rows() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-detail-split", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");
    let mut event = test_usage_event();
    event.client_request_body_json = None;
    event.upstream_request_body_json = None;
    event.full_request_json = None;

    repo.append_usage_event(&event)
        .await
        .expect("append duckdb usage event");

    let db_path = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => state.lock().expect("lock tiered state").active_path.clone(),
        _ => panic!("expected tiered repository"),
    };

    let conn =
        super::DuckDbUsageRepository::open_read_only_conn(&db_path).expect("open read-only db");
    let fact_row = conn
        .query_row(
            "SELECT request_headers_json, routing_diagnostics_json, last_message_content,
                    detail_object_payload_present
             FROM usage_events WHERE event_id = ?1",
            [&event.event_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .expect("read fact row");
    assert_eq!(fact_row.0, event.request_headers_json);
    assert_eq!(fact_row.1, event.routing_diagnostics_json);
    assert_eq!(fact_row.2, event.last_message_content);
    assert!(!fact_row.3);
    assert!(!legacy_details_store_object_path(&root, &event).exists());

    let detail = repo
        .get_usage_event(&event.event_id)
        .await
        .expect("get usage event detail")
        .expect("usage event exists");
    assert_usage_event_light_detail_round_trips(&detail, &event);

    let mut heavy = event.clone();
    heavy.event_id = "duckdb-test-event-heavy".to_string();
    heavy.client_request_body_json = Some(r#"{"client":true}"#.to_string());
    heavy.upstream_request_body_json = Some(r#"{"upstream":true}"#.to_string());
    heavy.full_request_json = Some(r#"{"full":true}"#.to_string());
    repo.append_usage_event(&heavy)
        .await
        .expect("append heavy duckdb usage event");
    let conn =
        super::DuckDbUsageRepository::open_read_only_conn(&db_path).expect("reopen read-only db");
    let detail_pack_path = conn
        .query_row(
            "SELECT detail_object_path FROM usage_events WHERE event_id = ?1",
            [&heavy.event_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .expect("read heavy detail pack path")
        .expect("heavy event detail pack path");
    assert!(root.join("usage-details").join(detail_pack_path).exists());
    assert!(!legacy_details_store_object_path(&root, &heavy).exists());

    let heavy_detail = repo
        .get_usage_event(&heavy.event_id)
        .await
        .expect("get heavy usage event detail")
        .expect("heavy usage event exists");
    assert_usage_event_round_trips(&heavy_detail, &heavy);
    assert_usage_event_detail_payloads(&heavy_detail, &heavy);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_writes_heavy_detail_payloads_into_shared_pack() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-detail-pack", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "duckdb-test-pack-first".to_string();
    first.client_request_body_json = Some(r#"{"client":1}"#.to_string());
    first.upstream_request_body_json = Some(r#"{"upstream":1}"#.to_string());
    first.full_request_json = Some(r#"{"full":1}"#.to_string());
    let mut second = test_usage_event();
    second.event_id = "duckdb-test-pack-second".to_string();
    second.client_request_body_json = Some(r#"{"client":2}"#.to_string());
    second.upstream_request_body_json = Some(r#"{"upstream":2}"#.to_string());
    second.full_request_json = Some(r#"{"full":2}"#.to_string());

    repo.append_usage_events(&[first.clone(), second.clone()])
        .await
        .expect("append packed detail events");

    let db_path = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => state.lock().expect("lock tiered state").active_path.clone(),
        _ => panic!("expected tiered repository"),
    };
    let conn =
        super::DuckDbUsageRepository::open_read_only_conn(&db_path).expect("open read-only db");
    let detail_refs = conn
        .prepare(
            "SELECT detail_object_path, detail_object_offset, detail_object_length,
                    detail_object_sha256
             FROM usage_events
             WHERE event_id IN (?1, ?2)
             ORDER BY event_id",
        )
        .expect("prepare detail refs")
        .query_map([&first.event_id, &second.event_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .expect("query detail refs")
        .collect::<Result<Vec<_>, _>>()
        .expect("collect detail refs");
    assert_eq!(detail_refs.len(), 2);
    let first_ref = &detail_refs[0];
    let second_ref = &detail_refs[1];
    assert_eq!(first_ref.0, second_ref.0);
    assert_ne!(first_ref.1, second_ref.1);
    assert!(first_ref.2.expect("first length") > 0);
    assert!(second_ref.2.expect("second length") > 0);
    assert!(first_ref
        .3
        .as_deref()
        .is_some_and(|value| !value.is_empty()));
    assert!(second_ref
        .3
        .as_deref()
        .is_some_and(|value| !value.is_empty()));
    let pack_path = root
        .join("usage-details")
        .join(first_ref.0.as_deref().expect("detail pack path"));
    assert!(pack_path.exists(), "detail pack should exist at {}", pack_path.display());
    assert!(!legacy_details_store_object_path(&root, &first).exists());
    assert!(!legacy_details_store_object_path(&root, &second).exists());

    let first_detail = repo
        .get_usage_event(&first.event_id)
        .await
        .expect("get first detail")
        .expect("first event exists");
    let second_detail = repo
        .get_usage_event(&second.event_id)
        .await
        .expect("get second detail")
        .expect("second event exists");
    assert_usage_event_detail_payloads(&first_detail, &first);
    assert_usage_event_detail_payloads(&second_detail, &second);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_returns_empty_payloads_when_external_detail_pack_is_missing() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-missing-detail-pack", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");
    let mut event = test_usage_event();
    event.event_id = "duckdb-test-missing-pack".to_string();
    event.client_request_body_json = Some(r#"{"client":true}"#.to_string());
    event.upstream_request_body_json = Some(r#"{"upstream":true}"#.to_string());
    event.full_request_json = Some(r#"{"full":true}"#.to_string());

    repo.append_usage_event(&event)
        .await
        .expect("append packed detail event");
    std::fs::remove_dir_all(root.join("usage-details")).expect("remove detail pack directory");

    let detail = repo
        .get_usage_event(&event.event_id)
        .await
        .expect("get detail after pack deletion")
        .expect("event exists");
    assert_usage_event_light_detail_round_trips(&detail, &event);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_uses_detail_ref_even_when_payload_present_flag_is_false() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-false-detail-flag", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");
    let mut event = test_usage_event();
    event.event_id = "duckdb-test-false-detail-flag".to_string();
    event.client_request_body_json = Some(r#"{"client":true}"#.to_string());
    event.upstream_request_body_json = Some(r#"{"upstream":true}"#.to_string());
    event.full_request_json = Some(r#"{"full":true}"#.to_string());

    repo.append_usage_event(&event)
        .await
        .expect("append packed detail event");

    let db_path = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => state.lock().expect("lock tiered state").active_path.clone(),
        _ => panic!("expected tiered repository"),
    };
    let conn =
        duckdb::Connection::open(&db_path).expect("open active duckdb for detail flag update");
    conn.execute(
        "UPDATE usage_events
         SET detail_object_payload_present = false
         WHERE event_id = ?1",
        [&event.event_id],
    )
    .expect("force false detail payload flag");
    conn.execute_batch("CHECKPOINT;")
        .expect("checkpoint active duckdb after detail flag update");

    let detail = repo
        .get_usage_event(&event.event_id)
        .await
        .expect("get detail after false detail payload flag")
        .expect("event exists");
    assert_usage_event_detail_payloads(&detail, &event);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_round_trips_error_payloads_in_usage_detail() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-error-detail", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");
    let mut event = test_usage_event();
    event.client_request_body_json = None;
    event.upstream_request_body_json = None;
    event.full_request_json = None;
    event.error_message = Some(
        "400 Bedrock error message: A text block must be included when using documents."
            .to_string(),
    );
    event.error_body = Some(
        r#"{"error":{"message":"A text block must be included when using documents."}}"#
            .to_string(),
    );

    repo.append_usage_event(&event)
        .await
        .expect("append duckdb usage event");

    let detail = repo
        .get_usage_event(&event.event_id)
        .await
        .expect("get usage event detail")
        .expect("usage event detail exists");
    assert_usage_event_detail_payloads(&detail, &event);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn usage_detail_store_recomputes_payload_present_from_payloads() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-detail-pack-flag-recompute", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create detail pack flag recompute directory");
    let detail_store = super::UsageEventDetailStore::from_dir(&details_store_dir(&root))
        .expect("open detail store")
        .expect("detail store configured");
    let mut event = test_usage_event();
    event.event_id = "duckdb-test-detail-pack-flag-recompute".to_string();
    event.client_request_body_json = Some(r#"{"client":true}"#.to_string());
    event.upstream_request_body_json = Some(r#"{"upstream":true}"#.to_string());
    event.full_request_json = Some(r#"{"full":true}"#.to_string());
    let mut row = super::UsageEventRow::from_usage_event(&event);
    row.detail_object_payload_present = false;

    let pack = detail_store
        .prepare_pack(std::slice::from_mut(&mut row))
        .expect("prepare detail pack")
        .expect("detail pack should be written");

    assert!(row.detail_object_payload_present);
    assert_eq!(row.detail_object_path.as_deref(), Some(pack.relative_path.as_str()));
    assert!(row.detail_object_offset.is_some());
    assert!(row.detail_object_length.is_some());
    assert!(row
        .detail_object_sha256
        .as_deref()
        .is_some_and(|value| !value.is_empty()));

    std::fs::remove_dir_all(&root).expect("cleanup detail pack flag recompute directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_summarizes_key_usage_rollups() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-key-rollups", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
    let mut first = test_usage_event();
    first.event_id = "rollup-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    first.credit_usage = Some("0.5".to_string());
    let mut second = test_usage_event();
    second.event_id = "rollup-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    second.credit_usage = Some("0.25".to_string());
    second.credit_usage_missing = true;

    repo.append_usage_events(&[first.clone(), second.clone()])
        .await
        .expect("append duckdb usage event batch");

    let rollups = repo
        .key_usage_rollups()
        .await
        .expect("summarize key usage rollups");

    assert_eq!(rollups.len(), 1);
    assert_eq!(rollups[0].key_id, first.key_id);
    assert_eq!(rollups[0].input_uncached_tokens, 20);
    assert_eq!(rollups[0].input_cached_tokens, 40);
    assert_eq!(rollups[0].output_tokens, 60);
    assert_eq!(rollups[0].billable_tokens, 80);
    assert_eq!(rollups[0].credit_total, "0.75");
    assert_eq!(rollups[0].credit_missing_events, 1);
    assert_eq!(rollups[0].last_used_at_ms, Some(second.created_at_ms));

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_lists_usage_events_newest_first_from_append_order() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-append-order", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");
    let mut first = test_usage_event();
    first.event_id = "append-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    let mut second = test_usage_event();
    second.event_id = "append-second".to_string();
    second.created_at_ms = 1_700_000_060_000;

    repo.append_usage_event(&first)
        .await
        .expect("append first duckdb usage event");
    repo.append_usage_event(&second)
        .await
        .expect("append second duckdb usage event");

    let first_page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 1,
            offset: 0,
        })
        .await
        .expect("list first page");
    assert_eq!(first_page.total, 2);
    assert_eq!(first_page.offset, 0);
    assert_eq!(first_page.limit, 1);
    assert!(first_page.has_more);
    assert_eq!(first_page.events[0].event_id, second.event_id);

    let second_page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 1,
            offset: 1,
        })
        .await
        .expect("list second page");
    assert_eq!(second_page.total, 2);
    assert_eq!(second_page.offset, 1);
    assert_eq!(second_page.limit, 1);
    assert!(!second_page.has_more);
    assert_eq!(second_page.events[0].event_id, first.event_id);

    let time_filtered_page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: Some(second.created_at_ms),
            end_ms: Some(second.created_at_ms.saturating_add(1)),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list time-filtered page");
    assert_eq!(time_filtered_page.total, 1);
    assert_eq!(time_filtered_page.events.len(), 1);
    assert_eq!(time_filtered_page.events[0].event_id, second.event_id);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_repository_clamps_online_usage_event_pages() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-online-page-clamp", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    for index in 0..25 {
        let mut event = test_usage_event();
        event.event_id = format!("online-clamp-{index:02}");
        event.created_at_ms = 1_700_000_000_000 + i64::from(index);
        repo.append_usage_event(&event)
            .await
            .expect("append duckdb usage event");
    }

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: None,
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 500,
            offset: 1_000,
        })
        .await
        .expect("list clamped page");

    assert_eq!(page.limit, super::USAGE_EVENT_PAGE_MAX_LIMIT);
    assert_eq!(page.offset, 1_000);
    assert_eq!(page.total, 25);
    assert!(page.events.is_empty());
    assert!(!page.has_more);

    let first_page = repo
        .list_usage_events(UsageEventQuery {
            key_id: None,
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 500,
            offset: 0,
        })
        .await
        .expect("list first clamped page");
    assert_eq!(first_page.limit, super::USAGE_EVENT_PAGE_MAX_LIMIT);
    assert_eq!(first_page.total, 25);
    assert_eq!(first_page.events.len(), 25);
    assert!(!first_page.has_more);
    assert_eq!(first_page.events[0].event_id, "online-clamp-24");

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn list_usage_events_supports_offsets_beyond_two_hundred() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-usage-online-offset", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    for index in 0..260 {
        let mut event = test_usage_event();
        event.event_id = format!("offset-page-{index:03}");
        event.created_at_ms = 1_700_100_000_000 + i64::from(index);
        repo.append_usage_event(&event)
            .await
            .expect("append duckdb usage event");
    }

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: None,
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 20,
            offset: 220,
        })
        .await
        .expect("list usage page after offset two hundred");

    assert_eq!(page.total, 260);
    assert_eq!(page.offset, 220);
    assert_eq!(page.limit, 20);
    assert_eq!(page.events.len(), 20);
    assert!(page.has_more);
    assert_eq!(page.events[0].event_id, "offset-page-039");
    assert_eq!(page.events[19].event_id, "offset-page-020");

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn list_usage_events_returns_full_totals_for_filtered_result_set() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-usage-filtered-totals", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "filtered-totals-1".to_string();
    first.created_at_ms = 1_700_200_000_000;
    first.provider_type = ProviderType::Codex;
    first.protocol_family = ProtocolFamily::OpenAi;
    first.key_id = "key-filtered".to_string();
    first.account_name = Some("account-a".to_string());
    first.endpoint = "/v1/responses".to_string();
    first.model = Some("gpt-5.4".to_string());
    first.status_code = 200;
    first.input_uncached_tokens = 11;
    first.input_cached_tokens = 7;
    first.output_tokens = 5;
    first.billable_tokens = 16;
    repo.append_usage_event(&first)
        .await
        .expect("append first filtered event");

    let mut second = first.clone();
    second.event_id = "filtered-totals-2".to_string();
    second.created_at_ms += 1_000;
    second.input_uncached_tokens = 19;
    second.input_cached_tokens = 13;
    second.output_tokens = 17;
    second.billable_tokens = 36;
    repo.append_usage_event(&second)
        .await
        .expect("append second filtered event");

    let mut non_matching = first.clone();
    non_matching.event_id = "filtered-totals-3".to_string();
    non_matching.created_at_ms += 2_000;
    non_matching.account_name = Some("account-b".to_string());
    non_matching.endpoint = "/v1/chat/completions".to_string();
    non_matching.model = Some("gpt-5.5".to_string());
    non_matching.status_code = 524;
    non_matching.input_uncached_tokens = 100;
    non_matching.input_cached_tokens = 100;
    non_matching.output_tokens = 100;
    non_matching.billable_tokens = 200;
    repo.append_usage_event(&non_matching)
        .await
        .expect("append non matching event");

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some("key-filtered".to_string()),
            provider_type: Some("codex".to_string()),
            model: Some("gpt-5.4".to_string()),
            account_name: Some("account-a".to_string()),
            endpoint: Some("/v1/responses".to_string()),
            status_code: Some(200),
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 1,
            offset: 0,
        })
        .await
        .expect("list filtered usage page");

    assert_eq!(page.total, 2);
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].event_id, second.event_id);
    assert_eq!(page.totals.event_count, 2);
    assert_eq!(
        page.totals.input_uncached_tokens,
        (first.input_uncached_tokens + second.input_uncached_tokens) as u64
    );
    assert_eq!(
        page.totals.input_cached_tokens,
        (first.input_cached_tokens + second.input_cached_tokens) as u64
    );
    assert_eq!(page.totals.output_tokens, (first.output_tokens + second.output_tokens) as u64);
    assert_eq!(
        page.totals.billable_tokens,
        (first.billable_tokens + second.billable_tokens) as u64
    );

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn list_usage_events_supports_status_kind_buckets() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-usage-status-kind", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    let mut ok_event = test_usage_event();
    ok_event.event_id = "status-kind-ok".to_string();
    ok_event.created_at_ms = 1_700_300_000_000;
    ok_event.key_id = "status-kind-key".to_string();
    ok_event.status_code = 200;
    ok_event.billable_tokens = 10;
    repo.append_usage_event(&ok_event)
        .await
        .expect("append ok usage event");

    let mut error_event = ok_event.clone();
    error_event.event_id = "status-kind-error".to_string();
    error_event.created_at_ms += 1_000;
    error_event.status_code = 524;
    error_event.billable_tokens = 25;
    repo.append_usage_event(&error_event)
        .await
        .expect("append non-ok usage event");

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some("status-kind-key".to_string()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: Some(UsageEventStatusKind::NonOk),
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list usage events filtered by status kind");

    assert_eq!(page.total, 1);
    assert_eq!(page.events.len(), 1);
    assert_eq!(page.events[0].event_id, error_event.event_id);
    assert_eq!(page.events[0].status_code, 524);
    assert_eq!(page.totals.event_count, 1);
    assert_eq!(page.totals.billable_tokens, error_event.billable_tokens as u64);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn list_usage_filter_options_respects_scope_but_not_self_filter() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-usage-filter-options-scope", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "filter-options-first".to_string();
    first.created_at_ms = 1_700_400_000_000;
    first.key_id = "filter-options-key".to_string();
    first.model = Some("gpt-5.4".to_string());
    first.account_name = Some("account-a".to_string());
    first.endpoint = "/v1/responses".to_string();
    first.status_code = 200;
    repo.append_usage_event(&first)
        .await
        .expect("append first filter-options event");

    let mut second = first.clone();
    second.event_id = "filter-options-second".to_string();
    second.created_at_ms += 1_000;
    second.model = Some("gpt-5.5".to_string());
    second.account_name = Some("account-b".to_string());
    second.endpoint = "/v1/chat/completions".to_string();
    second.status_code = 524;
    repo.append_usage_event(&second)
        .await
        .expect("append second filter-options event");

    let options = repo
        .list_usage_filter_options(UsageEventQuery {
            key_id: Some("filter-options-key".to_string()),
            provider_type: None,
            model: Some("gpt-5.4".to_string()),
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 20,
            offset: 0,
        })
        .await
        .expect("list usage filter options");

    assert_eq!(options.models, vec!["gpt-5.4".to_string(), "gpt-5.5".to_string()]);
    assert_eq!(options.accounts, vec!["account-a".to_string()]);
    assert_eq!(options.endpoints, vec!["/v1/responses".to_string()]);

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn tiered_usage_page_plan_skips_whole_sources_and_fetches_only_page_rows() {
    let plan = super::plan_tiered_usage_page_fetches([50, 80, 80], 55, 20);

    assert_eq!(plan, vec![super::TieredUsagePageFetch {
        partition_index: 1,
        local_newest_offset: 5,
        limit: 20,
    }]);

    let cross_partition_plan = super::plan_tiered_usage_page_fetches([5, 10], 3, 10);
    assert_eq!(cross_partition_plan, vec![
        super::TieredUsagePageFetch {
            partition_index: 0,
            local_newest_offset: 3,
            limit: 2,
        },
        super::TieredUsagePageFetch {
            partition_index: 1,
            local_newest_offset: 0,
            limit: 8,
        },
    ]);
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_rolls_over_without_blocking_active_appends() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-rollover", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");
    let mut first = test_usage_event();
    first.event_id = "tiered-archived-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    first.last_message_content = Some("archived detail".repeat(128));
    first.client_request_body_json = None;
    first.upstream_request_body_json = None;
    first.full_request_json = None;
    let mut second = test_usage_event();
    second.event_id = "tiered-active-second".to_string();
    second.created_at_ms = 1_700_000_060_000;

    repo.append_usage_event(&first)
        .await
        .expect("append first tiered usage event");
    repo.append_usage_event(&second)
        .await
        .expect("append second tiered usage event after rollover");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list tiered usage events");
    assert_eq!(page.total, 2);
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].event_id, second.event_id);
    assert_eq!(page.events[1].event_id, first.event_id);

    let second_page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 1,
            offset: 1,
        })
        .await
        .expect("list second tiered usage page");
    assert_eq!(second_page.total, 2);
    assert_eq!(second_page.events.len(), 1);
    assert_eq!(second_page.events[0].event_id, first.event_id);
    assert!(!second_page.has_more);

    let archived_detail = repo
        .get_usage_event(&first.event_id)
        .await
        .expect("get archived tiered usage event")
        .expect("archived tiered event exists");
    assert_usage_event_light_detail_round_trips(&archived_detail, &first);
    assert!(!legacy_details_store_object_path(&root, &first).exists());

    std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_preserves_heavy_detail_payloads_after_rollover() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-heavy-rollover", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-heavy-archived-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    first.client_request_body_json = Some(r#"{"client":1}"#.to_string());
    first.upstream_request_body_json = Some(r#"{"upstream":1}"#.to_string());
    first.full_request_json = Some(r#"{"full":1}"#.to_string());

    let mut second = test_usage_event();
    second.event_id = "tiered-heavy-active-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    second.client_request_body_json = None;
    second.upstream_request_body_json = None;
    second.full_request_json = None;

    repo.append_usage_event(&first)
        .await
        .expect("append first tiered heavy usage event");
    repo.append_usage_event(&second)
        .await
        .expect("append second tiered usage event after rollover");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;

    let archived_detail = repo
        .get_usage_event(&first.event_id)
        .await
        .expect("get archived tiered heavy usage event")
        .expect("archived tiered heavy event exists");
    assert_usage_event_round_trips(&archived_detail, &first);
    assert_usage_event_detail_payloads(&archived_detail, &first);

    std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_reads_legacy_archives_without_stream_columns() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-legacy-archive-schema", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("archive")).expect("create archive dir");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    };
    std::fs::create_dir_all(&config.active_dir).expect("create active dir");
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");
    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-legacy-archive-000001",
        1_700_000_000_000,
    );
    create_legacy_usage_archive_without_stream_columns(&archive_path);
    let stats = super::collect_segment_stats(&archive_path).expect("collect legacy stats");
    let size_bytes = std::fs::metadata(&archive_path)
        .expect("legacy archive metadata")
        .len();
    let catalog_backend = test_catalog_backend(&config);
    super::publish_segment_catalog(
        &catalog_backend,
        "usage-legacy-archive-000001",
        &archive_path,
        &stats,
        size_bytes,
    )
    .expect("publish legacy catalog");

    let repo =
        super::DuckDbUsageRepository::open_tiered(config).expect("open tiered duckdb usage db");
    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some("key-duckdb".to_string()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list legacy archive usage events");

    assert_eq!(page.total, 1);
    assert_eq!(page.events[0].event_id, "legacy-archive-event");
    assert_eq!(page.events[0].stream.stream_completed_cleanly, None);
    assert_eq!(page.events[0].stream.downstream_disconnect, None);
    assert_eq!(page.events[0].stream.final_event_type, None);
    assert_eq!(page.events[0].stream.bytes_streamed, None);

    let detail = repo
        .get_usage_event("legacy-archive-event")
        .await
        .expect("get legacy archive detail")
        .expect("legacy archive event exists");
    assert_eq!(detail.stream.stream_completed_cleanly, None);
    assert_eq!(detail.stream.downstream_disconnect, None);
    assert_eq!(detail.stream.final_event_type, None);
    assert_eq!(detail.stream.bytes_streamed, None);

    std::fs::remove_dir_all(&root).expect("cleanup legacy archive test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn tiered_usage_filter_options_include_archived_segments() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-tiered-filter-options-archive",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-filter-options-first".to_string();
    first.created_at_ms = 1_700_500_000_000;
    first.key_id = "tiered-filter-options-key".to_string();
    first.model = Some("gpt-5.4".to_string());
    first.account_name = Some("archived-account-a".to_string());
    first.endpoint = "/v1/responses".to_string();
    repo.append_usage_event(&first)
        .await
        .expect("append first archived candidate");

    let mut second = first.clone();
    second.event_id = "tiered-filter-options-second".to_string();
    second.created_at_ms += 1_000;
    second.model = Some("gpt-5.5".to_string());
    second.account_name = Some("archived-account-b".to_string());
    second.endpoint = "/v1/chat/completions".to_string();
    repo.append_usage_event(&second)
        .await
        .expect("append second archived candidate");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    let options = wait_for_usage_filter_options(
        &repo,
        UsageEventQuery {
            key_id: Some("tiered-filter-options-key".to_string()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: None,
            end_ms: None,
            limit: 20,
            offset: 0,
        },
        |options| {
            options.models.len() >= 2 && options.accounts.len() >= 2 && options.endpoints.len() >= 2
        },
    )
    .await;

    assert_eq!(options.models, vec!["gpt-5.4".to_string(), "gpt-5.5".to_string()]);
    assert_eq!(options.accounts, vec![
        "archived-account-a".to_string(),
        "archived-account-b".to_string()
    ]);
    assert_eq!(options.endpoints, vec![
        "/v1/chat/completions".to_string(),
        "/v1/responses".to_string()
    ]);

    std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_reads_legacy_embedded_detail_rows_without_detail_packs() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-legacy-embedded-detail", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("archive")).expect("create archive dir");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    };
    std::fs::create_dir_all(&config.active_dir).expect("create active dir");
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");
    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-legacy-detail-000001",
        1_700_000_000_000,
    );
    create_legacy_usage_archive_without_stream_columns(&archive_path);
    let stats = super::collect_segment_stats(&archive_path).expect("collect legacy stats");
    let size_bytes = std::fs::metadata(&archive_path)
        .expect("legacy archive metadata")
        .len();
    let catalog_backend = test_catalog_backend(&config);
    super::publish_segment_catalog(
        &catalog_backend,
        "usage-legacy-detail-000001",
        &archive_path,
        &stats,
        size_bytes,
    )
    .expect("publish legacy catalog");

    let repo =
        super::DuckDbUsageRepository::open_tiered(config).expect("open tiered duckdb usage db");
    let detail = repo
        .get_usage_event("legacy-archive-event")
        .await
        .expect("get legacy archive detail")
        .expect("legacy archive event exists");
    assert_eq!(detail.request_headers_json, r#"{"host":["example.test"]}"#);
    assert_eq!(detail.routing_diagnostics_json.as_deref(), Some(r#"{"route":"legacy"}"#));
    assert_eq!(detail.last_message_content.as_deref(), Some("hello"));
    assert_eq!(
        detail.client_request_body_json.as_deref(),
        Some(r#"{"model":"claude-sonnet-4-5"}"#)
    );
    assert_eq!(detail.upstream_request_body_json.as_deref(), Some(r#"{"conversationState":{}}"#));
    assert_eq!(detail.full_request_json.as_deref(), Some(r#"{"model":"claude-sonnet-4-5"}"#));

    std::fs::remove_dir_all(&root).expect("cleanup legacy embedded detail test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_skips_nonmatching_archives_before_partial_time_counts() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-skip-nonmatching-archives", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("archive")).expect("create archive dir");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    };
    std::fs::create_dir_all(&config.active_dir).expect("create active dir");
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");
    let catalog_backend = test_catalog_backend(&config);
    catalog_backend
        .publish_segment(
            &crate::usage_catalog::UsageCatalogSegmentRecord {
                segment_id: "usage-nonmatching-000001".to_string(),
                archive_path: config.archive_dir.join("missing-nonmatching.duckdb"),
                start_ms: Some(1_700_000_000_000_i64),
                end_ms: Some(1_700_000_100_000_i64),
                row_count: 1,
                input_uncached_tokens: 1,
                input_cached_tokens: 0,
                output_tokens: 1,
                billable_tokens: 2,
                size_bytes: 1,
                sealed_at_ms: 1_700_000_100_000_i64,
            },
            &[crate::usage_catalog::UsageCatalogKeyRollupRecord {
                key_id: "other-key".to_string(),
                provider_type: "kiro".to_string(),
                row_count: 1,
                input_uncached_tokens: 1,
                input_cached_tokens: 0,
                output_tokens: 1,
                billable_tokens: 2,
                credit_total: "0".to_string(),
                credit_missing_events: 0,
                first_used_at_ms: Some(1_700_000_050_000_i64),
                last_used_at_ms: Some(1_700_000_050_000_i64),
            }],
            &[],
            &[],
        )
        .expect("insert nonmatching test catalog segment");

    let repo =
        super::DuckDbUsageRepository::open_tiered(config).expect("open tiered duckdb usage db");
    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some("key-duckdb".to_string()),
            provider_type: Some("kiro".to_string()),
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: Some(1_700_000_010_000),
            end_ms: Some(1_700_000_020_000),
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list should skip nonmatching archive");

    assert_eq!(page.total, 0);
    assert!(page.events.is_empty());

    std::fs::remove_dir_all(&root).expect("cleanup skip nonmatching archive test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn tiered_archive_totals_limit_zero_can_come_from_catalog_without_opening_segments() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-catalog-only-totals", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered catalog-only totals test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-catalog-only-first".to_string();
    first.created_at_ms = 1_700_600_000_000;
    first.model = Some("gpt-5.4".to_string());
    repo.append_usage_event(&first)
        .await
        .expect("append first archived event");

    let mut second = first.clone();
    second.event_id = "tiered-catalog-only-second".to_string();
    second.created_at_ms += 1_000;
    second.model = Some("gpt-5.5".to_string());
    repo.append_usage_event(&second)
        .await
        .expect("append second archived event");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    remove_archived_duckdb_files(&root.join("archive"));

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: None,
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: None,
            end_ms: None,
            limit: 0,
            offset: 0,
        })
        .await
        .expect("list catalog-only archive totals");

    assert_eq!(page.total, 2);
    assert_eq!(page.totals.event_count, 2);
    assert_eq!(page.totals.input_uncached_tokens, 20);
    assert_eq!(page.totals.input_cached_tokens, 40);
    assert_eq!(page.totals.output_tokens, 60);
    assert_eq!(page.totals.billable_tokens, 80);
    assert!(page.events.is_empty());

    std::fs::remove_dir_all(&root).expect("cleanup tiered catalog-only totals test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn tiered_archive_model_filter_limit_zero_can_come_from_catalog_without_opening_segments() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-tiered-catalog-only-model-filter",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered catalog-only model-filter test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-catalog-model-first".to_string();
    first.created_at_ms = 1_700_610_000_000;
    first.key_id = "tiered-catalog-model-key".to_string();
    first.model = Some("gpt-5.4".to_string());
    repo.append_usage_event(&first)
        .await
        .expect("append first archived model-filter event");

    let mut second = first.clone();
    second.event_id = "tiered-catalog-model-second".to_string();
    second.created_at_ms += 1_000;
    second.model = Some("gpt-5.5".to_string());
    repo.append_usage_event(&second)
        .await
        .expect("append second archived model-filter event");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    remove_archived_duckdb_files(&root.join("archive"));

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some("tiered-catalog-model-key".to_string()),
            provider_type: Some("kiro".to_string()),
            model: Some("gpt-5.4".to_string()),
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: None,
            end_ms: None,
            limit: 0,
            offset: 0,
        })
        .await
        .expect("list catalog-only archive model-filter totals");

    assert_eq!(page.total, 1);
    assert_eq!(page.totals.event_count, 1);
    assert_eq!(page.totals.input_uncached_tokens, 10);
    assert_eq!(page.totals.input_cached_tokens, 20);
    assert_eq!(page.totals.output_tokens, 30);
    assert_eq!(page.totals.billable_tokens, 40);
    assert!(page.events.is_empty());

    std::fs::remove_dir_all(&root)
        .expect("cleanup tiered catalog-only model-filter test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn tiered_usage_filter_options_can_come_from_catalog_without_opening_archives() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-tiered-catalog-only-filter-options",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root)
        .expect("create tiered catalog-only filter-options test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-catalog-filter-options-first".to_string();
    first.created_at_ms = 1_700_620_000_000;
    first.key_id = "tiered-catalog-filter-options-key".to_string();
    first.model = Some("gpt-5.4".to_string());
    first.account_name = Some("catalog-account-a".to_string());
    first.endpoint = "/v1/responses".to_string();
    repo.append_usage_event(&first)
        .await
        .expect("append first catalog filter-options event");

    let mut second = first.clone();
    second.event_id = "tiered-catalog-filter-options-second".to_string();
    second.created_at_ms += 1_000;
    second.model = Some("gpt-5.5".to_string());
    second.account_name = Some("catalog-account-b".to_string());
    second.endpoint = "/v1/chat/completions".to_string();
    repo.append_usage_event(&second)
        .await
        .expect("append second catalog filter-options event");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    remove_archived_duckdb_files(&root.join("archive"));

    let options = repo
        .list_usage_filter_options(UsageEventQuery {
            key_id: Some("tiered-catalog-filter-options-key".to_string()),
            provider_type: Some("kiro".to_string()),
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: None,
            end_ms: None,
            limit: 20,
            offset: 0,
        })
        .await
        .expect("list catalog-only filter options");

    assert_eq!(options.models, vec!["gpt-5.4".to_string(), "gpt-5.5".to_string()]);
    assert_eq!(options.accounts, vec![
        "catalog-account-a".to_string(),
        "catalog-account-b".to_string()
    ]);
    assert_eq!(options.endpoints, vec![
        "/v1/chat/completions".to_string(),
        "/v1/responses".to_string()
    ]);

    std::fs::remove_dir_all(&root)
        .expect("cleanup tiered catalog-only filter-options test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn tiered_open_refreshes_missing_catalog_field_rollups() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-tiered-catalog-refresh-rollups",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("archive")).expect("create archive dir");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    };
    std::fs::create_dir_all(&config.active_dir).expect("create active dir");
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");

    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-legacy-refresh-000001",
        1_700_000_000_000,
    );
    create_legacy_usage_archive_without_stream_columns(&archive_path);
    let stats = super::collect_segment_stats(&archive_path).expect("collect legacy stats");
    let event_ids =
        super::collect_segment_event_ids(&archive_path).expect("collect legacy event ids");
    let size_bytes = std::fs::metadata(&archive_path)
        .expect("legacy archive metadata")
        .len();
    let catalog_backend = test_catalog_backend(&config);
    catalog_backend
        .publish_segment(
            &crate::usage_catalog::UsageCatalogSegmentRecord {
                segment_id: "usage-legacy-refresh-000001".to_string(),
                archive_path: archive_path.clone(),
                start_ms: stats.start_ms,
                end_ms: stats.end_ms,
                row_count: stats.row_count,
                input_uncached_tokens: stats.input_uncached_tokens,
                input_cached_tokens: stats.input_cached_tokens,
                output_tokens: stats.output_tokens,
                billable_tokens: stats.billable_tokens,
                size_bytes,
                sealed_at_ms: 1_700_000_000_000,
            },
            &stats
                .rollups
                .iter()
                .map(|rollup| crate::usage_catalog::UsageCatalogKeyRollupRecord {
                    key_id: rollup.key_id.clone(),
                    provider_type: rollup.provider_type.clone(),
                    row_count: rollup.row_count,
                    input_uncached_tokens: rollup.input_uncached_tokens,
                    input_cached_tokens: rollup.input_cached_tokens,
                    output_tokens: rollup.output_tokens,
                    billable_tokens: rollup.billable_tokens,
                    credit_total: rollup.credit_total.clone(),
                    credit_missing_events: rollup.credit_missing_events,
                    first_used_at_ms: rollup.first_used_at_ms,
                    last_used_at_ms: rollup.last_used_at_ms,
                })
                .collect::<Vec<_>>(),
            &[],
            &event_ids,
        )
        .expect("publish legacy catalog without field rollups");

    let repo = super::DuckDbUsageRepository::open_tiered(config.clone())
        .expect("open tiered duckdb usage db");
    std::fs::remove_file(&archive_path).expect("remove archive after refresh");

    let options = repo
        .list_usage_filter_options(UsageEventQuery {
            key_id: None,
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::Archive,
            start_ms: Some(1_699_999_000_000),
            end_ms: Some(1_700_001_000_000),
            limit: 20,
            offset: 0,
        })
        .await
        .expect("list filter options after catalog refresh");

    assert_eq!(options.models, vec!["claude-sonnet-4-5".to_string()]);
    assert_eq!(options.accounts, vec!["kiro-account".to_string()]);
    assert_eq!(options.endpoints, vec!["/cc/v1/messages".to_string()]);

    std::fs::remove_dir_all(&root).expect("cleanup tiered catalog refresh test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_append_usage_events_allows_archived_duplicates() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-dedup-archived", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut archived = test_usage_event();
    archived.event_id = "tiered-dedup-archived".to_string();
    archived.created_at_ms = 1_700_000_000_000;
    repo.append_usage_event(&archived)
        .await
        .expect("append archived event");

    let mut active = test_usage_event();
    active.event_id = "tiered-dedup-active".to_string();
    active.created_at_ms = 1_700_000_060_000;
    repo.append_usage_event(&active)
        .await
        .expect("append active event");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    wait_for_tiered_usage_event(&repo, &archived.event_id).await;
    wait_for_tiered_usage_event(&repo, &active.event_id).await;

    let mut fresh = test_usage_event();
    fresh.event_id = "tiered-dedup-fresh".to_string();
    fresh.created_at_ms = 1_700_000_120_000;
    repo.append_usage_events(&[archived.clone(), active.clone(), fresh.clone(), fresh.clone()])
        .await
        .expect("append deduplicated tiered batch");
    wait_for_tiered_usage_event(&repo, &fresh.event_id).await;

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(archived.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list tiered deduplicated page");
    assert_eq!(page.total, 5);

    std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_retention_prunes_expired_archived_segments() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-retention-prune", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered retention test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");
    let now_ms = 1_700_864_000_000;
    let day_ms = 86_400_000;
    let mut expired = test_usage_event();
    expired.event_id = "expired-retention-event".to_string();
    expired.created_at_ms = now_ms - 8 * day_ms;
    let mut retained = test_usage_event();
    retained.event_id = "retained-retention-event".to_string();
    retained.created_at_ms = now_ms - 2 * day_ms;

    repo.append_usage_event(&expired)
        .await
        .expect("append expired event");
    repo.append_usage_event(&retained)
        .await
        .expect("append retained event");
    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    wait_for_usage_event_present(&repo, &retained.event_id).await;

    let report = repo
        .prune_usage_analytics(now_ms, 7)
        .await
        .expect("prune expired usage analytics");

    assert_eq!(report.deleted_segments, 1);
    assert_eq!(report.deleted_files, 1);
    assert!(repo
        .get_usage_event(&expired.event_id)
        .await
        .expect("lookup expired event")
        .is_none());
    assert!(repo
        .get_usage_event(&retained.event_id)
        .await
        .expect("lookup retained event")
        .is_some());
    wait_for_archived_duckdb_file_count(&root.join("archive"), 1).await;

    std::fs::remove_dir_all(&root).expect("cleanup tiered retention test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_retention_discards_expired_active_segment() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-retention-active-prune", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create active retention test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");
    let now_ms = 1_700_864_000_000;
    let day_ms = 86_400_000;
    let mut expired = test_usage_event();
    expired.event_id = "expired-active-retention-event".to_string();
    expired.created_at_ms = now_ms - 8 * day_ms;

    repo.append_usage_event(&expired)
        .await
        .expect("append expired active event");
    assert!(repo
        .get_usage_event(&expired.event_id)
        .await
        .expect("lookup expired active event before prune")
        .is_some());
    assert_eq!(duckdb_file_count(&root.join("active")), 1);

    let report = repo
        .prune_usage_analytics(now_ms, 7)
        .await
        .expect("prune expired active usage analytics");

    assert_eq!(report.deleted_segments, 0);
    assert_eq!(report.deleted_files, 1);
    assert!(repo
        .get_usage_event(&expired.event_id)
        .await
        .expect("lookup expired active event after prune")
        .is_none());
    assert_eq!(duckdb_file_count(&root.join("active")), 1);
    assert_eq!(duckdb_file_count(&root.join("archive")), 0);

    std::fs::remove_dir_all(&root).expect("cleanup active retention test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_rolls_over_existing_oversized_active_before_append() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-pre-rollover", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered pre-rollover test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: None,
    };
    let repo = super::DuckDbUsageRepository::open_tiered(config.clone())
        .expect("open tiered duckdb usage db");
    let mut first = test_usage_event();
    first.event_id = "tiered-existing-active-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    repo.append_usage_event(&first)
        .await
        .expect("append existing active event");
    drop(repo);

    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        rollover_bytes: 1,
        ..config
    })
    .expect("reopen tiered duckdb usage db with smaller rollover threshold");
    let mut second = test_usage_event();
    second.event_id = "tiered-new-active-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    repo.append_usage_event(&second)
        .await
        .expect("append should pre-rollover existing active segment");

    wait_for_archived_duckdb_file_count(&root.join("archive"), 2).await;
    let archived = duckdb_file_count(&root.join("archive"));
    assert_eq!(
        archived, 2,
        "pre-rollover should archive the existing active separately from the new append"
    );

    let page = repo
        .list_usage_events(UsageEventQuery {
            key_id: Some(first.key_id.clone()),
            provider_type: None,
            model: None,
            account_name: None,
            endpoint: None,
            status_code: None,
            status_kind: None,
            source: UsageEventSource::All,
            start_ms: None,
            end_ms: None,
            limit: 10,
            offset: 0,
        })
        .await
        .expect("list tiered usage events");
    assert_eq!(page.total, 2);
    assert_eq!(page.events.len(), 2);
    assert_eq!(page.events[0].event_id, second.event_id);
    assert_eq!(page.events[1].event_id, first.event_id);

    std::fs::remove_dir_all(&root).expect("cleanup tiered pre-rollover test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_keeps_active_writer_open_between_appends() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-active-writer-open", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered active writer test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: u64::MAX,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-active-writer-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    repo.append_usage_event(&first)
        .await
        .expect("append first tiered usage event");

    let (active_path, wal_path) = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => {
            let state = state.lock().expect("lock tiered duckdb state");
            assert!(
                state.active_writer.is_some(),
                "tiered repository should keep the active writer open after append"
            );
            let active_path = state.active_path.clone();
            let wal_path = super::duckdb_wal_path(&active_path);
            (active_path, wal_path)
        },
        _ => panic!("expected tiered repository"),
    };
    assert!(
        wal_path.exists(),
        "active WAL should remain present while the active writer stays open"
    );

    let mut second = test_usage_event();
    second.event_id = "tiered-active-writer-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    repo.append_usage_event(&second)
        .await
        .expect("append second tiered usage event");

    match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => {
            let state = state.lock().expect("lock tiered duckdb state");
            assert_eq!(state.active_path, active_path);
            assert!(
                state.active_writer.is_some(),
                "tiered repository should still hold the same active writer after reuse"
            );
        },
        _ => panic!("expected tiered repository"),
    }
    assert!(wal_path.exists(), "active WAL should still be present after the second append");

    std::fs::remove_dir_all(&root).expect("cleanup tiered active writer test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_repository_rollover_leaves_fresh_active_without_writer() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-tiered-rollover-drops-writer",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered rollover writer test directory");
    let repo = super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    })
    .expect("open tiered duckdb usage db");

    let mut first = test_usage_event();
    first.event_id = "tiered-rollover-drops-writer-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    repo.append_usage_event(&first)
        .await
        .expect("append first tiered usage event");

    let first_active_path = match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => {
            let state = state.lock().expect("lock tiered duckdb state");
            assert!(
                state.active_writer.is_none(),
                "rollover should drop the active writer after checkpointing the old segment"
            );
            state.active_path.clone()
        },
        _ => panic!("expected tiered repository"),
    };

    let mut second = test_usage_event();
    second.event_id = "tiered-rollover-drops-writer-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    repo.append_usage_event(&second)
        .await
        .expect("append second tiered usage event");

    match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => {
            let state = state.lock().expect("lock tiered duckdb state");
            assert_ne!(
                state.active_path, first_active_path,
                "rollover should switch the repository to a fresh active path"
            );
            assert!(
                state.active_writer.is_none(),
                "fresh active path should not retain the rolled-over writer handle"
            );
        },
        _ => panic!("expected tiered repository"),
    }

    std::fs::remove_dir_all(&root).expect("cleanup tiered rollover writer test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_publish_rewrites_segment_with_current_schema() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-compact-publish", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create compact publish test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
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

    let catalog_backend = test_catalog_backend(&config);
    super::publish_pending_segment_async(
        &config,
        &catalog_backend,
        &pending_path,
        "usage-compact-test-000001",
        super::DuckDbUsageConnectionConfig::default(),
    )
    .await
    .expect("publish compacted segment");

    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-compact-test-000001",
        1_700_000_000_000,
    );
    assert!(archive_path.exists(), "archived compact segment should exist");
    assert!(
        !pending_path.exists(),
        "pending segment should be removed only after catalog publication"
    );
    assert!(
        !super::compacting_segment_path(&config, "usage-compact-test-000001").exists(),
        "local compact temp file should be removed after publication"
    );
    assert!(
        !super::uploading_archive_segment_path_from_archive_path(&archive_path).exists(),
        "uploading archive temp file should not remain after publication"
    );
    let stale_compact_path = super::compacting_segment_path(&config, "usage-compact-test-000001");
    std::fs::write(&stale_compact_path, b"stale compact retry")
        .expect("write stale compact retry file");
    super::publish_pending_segment_async(
        &config,
        &catalog_backend,
        &pending_path,
        "usage-compact-test-000001",
        super::DuckDbUsageConnectionConfig::default(),
    )
    .await
    .expect("published segment finalization is idempotent");
    assert!(
        !stale_compact_path.exists(),
        "idempotent finalization should remove stale compact retry files"
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
        "archive should be rewritten with current schema and no legacy explicit indexes: \
         {indexes:?}"
    );

    let count: i64 = archived
        .query_row("SELECT CAST(count(*) AS BIGINT) FROM usage_events", [], |row| row.get(0))
        .expect("count archived rows");
    assert_eq!(count, 1);

    std::fs::remove_dir_all(&root).expect("cleanup compact publish test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_publish_handles_reordered_pending_usage_event_columns() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-compact-reordered", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create reordered compact test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: None,
    };
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");

    let pending_path = root.join("pending-reordered.duckdb");
    {
        let conn = duckdb::Connection::open(&pending_path).expect("open pending source");
        crate::initialize_duckdb_target(&conn).expect("initialize pending source");
        let mut writer = super::DuckDbUsageWriter::new(conn).expect("open pending writer");
        let mut event = test_usage_event();
        event.event_id = "compact-reordered-event".to_string();
        event.client_ip = "unknown".to_string();
        writer
            .insert_usage_events(&[super::UsageEventRow::from_usage_event(&event)])
            .expect("insert pending event");
    }
    {
        let conn = duckdb::Connection::open(&pending_path).expect("reopen pending source");
        conn.execute_batch(
            "
            CREATE TABLE usage_events_reordered AS
            SELECT
                source_seq, source_event_id, event_id, created_at_ms, created_at,
                created_date, created_hour, provider_type, protocol_family, key_id,
                key_name, key_status_at_event, account_name, account_group_id_at_event,
                route_strategy_at_event, request_method, request_url, endpoint, model,
                mapped_model, status_code, latency_ms, routing_wait_ms,
                upstream_headers_ms, post_headers_body_ms, request_body_read_ms,
                request_json_parse_ms, pre_handler_ms, first_sse_write_ms,
                stream_finish_ms, stream_completed_cleanly, downstream_disconnect,
                final_event_type, client_ip, bytes_streamed, request_body_bytes,
                quota_failover_count, routing_diagnostics_json,
                input_uncached_tokens, input_cached_tokens, output_tokens,
                billable_tokens, credit_usage, usage_missing, credit_usage_missing,
                ip_region, request_headers_json, last_message_content,
                detail_object_payload_present
            FROM usage_events;
            DROP TABLE usage_events;
            ALTER TABLE usage_events_reordered RENAME TO usage_events;
            CHECKPOINT;
            ",
        )
        .expect("reorder pending source usage_events columns");
    }

    let catalog_backend = test_catalog_backend(&config);
    super::publish_pending_segment_async(
        &config,
        &catalog_backend,
        &pending_path,
        "usage-reordered-test-000001",
        super::DuckDbUsageConnectionConfig::default(),
    )
    .await
    .expect("publish compacted segment with reordered usage_events");

    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-reordered-test-000001",
        1_700_000_000_000,
    );
    let archived = super::DuckDbUsageRepository::open_read_only_conn(&archive_path)
        .expect("open archived reordered segment");
    let row = archived
        .query_row(
            "SELECT client_ip, request_body_bytes FROM usage_events WHERE event_id = ?1",
            ["compact-reordered-event"],
            |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .expect("read archived reordered event");
    assert_eq!(row.0.as_deref(), Some("unknown"));
    assert_eq!(row.1, Some(1234));

    std::fs::remove_dir_all(&root).expect("cleanup reordered compact publish test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn duckdb_tiered_publish_drops_legacy_wide_detail_payloads_without_pack_index() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-legacy-pending-detail-backfill",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create legacy pending compact test directory");
    let config = super::TieredDuckDbUsageConfig {
        active_dir: root.join("active"),
        archive_dir: root.join("archive"),
        rollover_bytes: 1,
        details_dir: Some(details_store_dir(&root)),
    };
    super::initialize_tiered_catalog(&config).expect("initialize tiered catalog");

    let pending_path = root.join("pending-legacy-wide.duckdb");
    create_legacy_usage_archive_without_stream_columns(&pending_path);

    let catalog_backend = test_catalog_backend(&config);
    super::publish_pending_segment_async(
        &config,
        &catalog_backend,
        &pending_path,
        "usage-legacy-pending-000001",
        super::DuckDbUsageConnectionConfig::default(),
    )
    .await
    .expect("publish compacted legacy pending segment");

    let archive_path = archived_segment_path_for_timestamp(
        &config,
        "usage-legacy-pending-000001",
        1_700_000_000_000,
    );
    let archived = super::DuckDbUsageRepository::open_read_only_conn(&archive_path)
        .expect("open archived legacy pending segment");
    let fact_row = archived
        .query_row(
            "SELECT request_headers_json, routing_diagnostics_json, last_message_content,
                    detail_object_payload_present
             FROM usage_events WHERE event_id = 'legacy-archive-event'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, bool>(3)?,
                ))
            },
        )
        .expect("read archived fact row");
    assert_eq!(fact_row.0, r#"{"host":["example.test"]}"#);
    assert_eq!(fact_row.1, Some(r#"{"route":"legacy"}"#.to_string()));
    assert_eq!(fact_row.2, Some("hello".to_string()));
    assert!(fact_row.3);

    let repo = super::DuckDbUsageRepository::open_tiered(config.clone())
        .expect("open tiered duckdb usage db");
    let detail = repo
        .get_usage_event("legacy-archive-event")
        .await
        .expect("get legacy event detail")
        .expect("legacy event exists");
    assert_eq!(detail.request_headers_json, r#"{"host":["example.test"]}"#);
    assert_eq!(detail.routing_diagnostics_json.as_deref(), Some(r#"{"route":"legacy"}"#));
    assert_eq!(detail.last_message_content.as_deref(), Some("hello"));
    assert_eq!(detail.client_request_body_json, None);
    assert_eq!(detail.upstream_request_body_json, None);
    assert_eq!(detail.full_request_json, None);

    std::fs::remove_dir_all(&root).expect("cleanup legacy pending compact test directory");
}

#[cfg(feature = "duckdb-runtime")]
async fn wait_for_archived_duckdb_file_count(archive_dir: &std::path::Path, expected: usize) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let archived = duckdb_file_count(archive_dir);
        let catalog_segments = test_catalog_segment_count(archive_dir);
        if archived >= expected && catalog_segments >= expected {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for archived duckdb segment");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "duckdb-runtime")]
async fn wait_for_usage_event_present(repo: &super::DuckDbUsageRepository, event_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let event = repo
            .get_usage_event(event_id)
            .await
            .expect("query usage event while waiting");
        if event.is_some() {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for usage event `{event_id}`");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "duckdb-runtime")]
async fn wait_for_usage_filter_options<F>(
    repo: &super::DuckDbUsageRepository,
    query: UsageEventQuery,
    predicate: F,
) -> UsageFilterOptions
where
    F: Fn(&UsageFilterOptions) -> bool,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let options = repo
            .list_usage_filter_options(query.clone())
            .await
            .expect("query usage filter options while waiting");
        if predicate(&options) {
            return options;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for usage filter options to converge");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "duckdb-runtime")]
fn duckdb_file_count(dir: &std::path::Path) -> usize {
    let mut files = Vec::new();
    super::collect_files_recursive(dir, &mut files)
        .expect("collect recursive duckdb files for test count");
    files
        .into_iter()
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("duckdb"))
        .count()
}

#[cfg(feature = "duckdb-runtime")]
fn test_catalog_segment_count(archive_dir: &std::path::Path) -> usize {
    let state_path = archive_dir.join(".test-usage-catalog.json");
    let Ok(bytes) = std::fs::read(&state_path) else {
        return 0;
    };
    serde_json::from_slice::<super::TestTieredUsageCatalogState>(&bytes)
        .map(|state| state.segments.len())
        .unwrap_or(0)
}

#[cfg(feature = "duckdb-runtime")]
fn remove_archived_duckdb_files(archive_dir: &std::path::Path) {
    let mut files = Vec::new();
    super::collect_files_recursive(archive_dir, &mut files)
        .expect("collect archived duckdb files for removal");
    for path in files {
        if path.extension().and_then(|ext| ext.to_str()) == Some("duckdb") {
            std::fs::remove_file(&path).unwrap_or_else(|err| {
                panic!("remove archived duckdb file {}: {err}", path.display())
            });
        }
    }
}

#[cfg(feature = "duckdb-runtime")]
async fn wait_for_tiered_usage_event(repo: &super::DuckDbUsageRepository, event_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if repo
            .get_usage_event(event_id)
            .await
            .expect("query tiered usage event while waiting")
            .is_some()
        {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for tiered usage event {event_id}");
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn usage_metrics_snapshot_tracks_proxy_and_error_hotspots() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-metrics-snapshot", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create metrics test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open usage repo");

    let mut ok_event = test_usage_event();
    ok_event.event_id = "metrics-ok".to_string();
    ok_event.account_name = Some("acct-fast".to_string());
    ok_event.created_at_ms = 1_700_000_100_000;
    ok_event.timing.first_sse_write_ms = Some(120);
    ok_event.quota_failover_count = 0;

    let mut slow_error_event = test_usage_event();
    slow_error_event.event_id = "metrics-error".to_string();
    slow_error_event.account_name = Some("acct-slow".to_string());
    slow_error_event.created_at_ms = 1_700_000_101_000;
    slow_error_event.status_code = 524;
    slow_error_event.timing.first_sse_write_ms = Some(980);
    slow_error_event.timing.routing_wait_ms = Some(240);
    slow_error_event.quota_failover_count = 3;
    slow_error_event.stream.downstream_disconnect = Some(true);

    let ok_row = super::UsageEventRow::from_usage_event(&ok_event).with_proxy_attribution(Some(
        &crate::postgres::UsageProxyAttribution {
            provider_type: "kiro".to_string(),
            account_name: "acct-fast".to_string(),
            proxy_source: "fixed".to_string(),
            proxy_config_id: Some("proxy-sg-fast".to_string()),
            proxy_config_name: Some("sg-fast".to_string()),
            proxy_url: Some("http://127.0.0.1:11129".to_string()),
        },
    ));
    let slow_error_row = super::UsageEventRow::from_usage_event(&slow_error_event)
        .with_proxy_attribution(Some(&crate::postgres::UsageProxyAttribution {
            provider_type: "kiro".to_string(),
            account_name: "acct-slow".to_string(),
            proxy_source: "binding".to_string(),
            proxy_config_id: Some("proxy-us-slow".to_string()),
            proxy_config_name: Some("us-slow".to_string()),
            proxy_url: Some("http://127.0.0.1:11118".to_string()),
        }));

    repo.append_usage_event_rows_owned(vec![ok_row, slow_error_row])
        .await
        .expect("append usage rows");

    let snapshot = repo
        .usage_metrics_snapshot(UsageMetricsQuery {
            provider_type: Some("kiro".to_string()),
            source: UsageEventSource::Hot,
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_200_000,
            top_limit: 10,
        })
        .await
        .expect("fetch usage metrics snapshot");

    assert_eq!(snapshot.summary.total_requests, 2);
    assert_eq!(snapshot.summary.non_ok_requests, 1);
    assert_eq!(snapshot.summary.failover_request_count, 1);
    assert_eq!(snapshot.summary.total_quota_failovers, 3);
    assert_eq!(snapshot.summary.downstream_disconnect_count, 1);
    assert_eq!(
        snapshot
            .top_first_token_accounts
            .first()
            .map(|row| row.label.as_str()),
        Some("acct-slow")
    );
    assert_eq!(
        snapshot
            .top_non_ok_accounts
            .first()
            .map(|row| row.label.as_str()),
        Some("acct-slow")
    );
    assert_eq!(
        snapshot
            .top_first_token_proxies
            .first()
            .and_then(|row| row.proxy_config_name.as_deref()),
        Some("us-slow")
    );
    assert_eq!(
        snapshot
            .non_ok_status_codes
            .first()
            .map(|row| row.status_code),
        Some(524)
    );
    let ranking = repo
        .kiro_latency_ranking_snapshot(KiroLatencyRankingQuery {
            source: UsageEventSource::Hot,
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_200_000,
        })
        .await
        .expect("fetch kiro latency ranking snapshot");
    assert_eq!(ranking.first_token_samples, 2);
    assert_eq!(ranking.accounts.len(), 2);
    assert_eq!(ranking.proxies.len(), 2);
    assert_eq!(
        ranking
            .accounts
            .first()
            .and_then(|row| row.account_name.as_deref()),
        Some("acct-fast")
    );

    std::fs::remove_dir_all(&root).expect("cleanup metrics test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn proxy_traffic_snapshot_groups_request_and_response_bytes_by_proxy() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-proxy-traffic", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create proxy traffic test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open usage repo");

    let mut first = test_usage_event();
    first.event_id = "proxy-traffic-first".to_string();
    first.created_at_ms = 1_700_000_000_000;
    first.request_body_bytes = Some(1_024);
    first.stream.bytes_streamed = Some(4_096);

    let mut second = test_usage_event();
    second.event_id = "proxy-traffic-second".to_string();
    second.created_at_ms = 1_700_000_060_000;
    second.request_body_bytes = Some(512);
    second.stream.bytes_streamed = Some(2_048);

    let mut other_proxy = test_usage_event();
    other_proxy.event_id = "proxy-traffic-other".to_string();
    other_proxy.created_at_ms = 1_700_000_090_000;
    other_proxy.request_body_bytes = Some(10_000);
    other_proxy.stream.bytes_streamed = Some(10_000);

    let proxy = crate::postgres::UsageProxyAttribution {
        provider_type: "kiro".to_string(),
        account_name: "kiro-account".to_string(),
        proxy_source: "fixed".to_string(),
        proxy_config_id: Some("proxy-main".to_string()),
        proxy_config_name: Some("Proxy Main".to_string()),
        proxy_url: Some("http://127.0.0.1:18080".to_string()),
    };
    let other = crate::postgres::UsageProxyAttribution {
        provider_type: "kiro".to_string(),
        account_name: "kiro-account".to_string(),
        proxy_source: "fixed".to_string(),
        proxy_config_id: Some("proxy-other".to_string()),
        proxy_config_name: Some("Proxy Other".to_string()),
        proxy_url: Some("http://127.0.0.1:18081".to_string()),
    };

    repo.append_usage_event_rows_owned(vec![
        super::UsageEventRow::from_usage_event(&first).with_proxy_attribution(Some(&proxy)),
        super::UsageEventRow::from_usage_event(&second).with_proxy_attribution(Some(&proxy)),
        super::UsageEventRow::from_usage_event(&other_proxy).with_proxy_attribution(Some(&other)),
    ])
    .await
    .expect("append usage rows");

    let snapshot = repo
        .proxy_traffic_snapshot(ProxyTrafficQuery {
            proxy_config_id: Some("proxy-main".to_string()),
            provider_type: Some("kiro".to_string()),
            source: UsageEventSource::Hot,
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_120_000,
            bucket_ms: 60_000,
        })
        .await
        .expect("fetch proxy traffic snapshot");

    assert_eq!(snapshot.totals.event_count, 2);
    assert_eq!(snapshot.totals.request_bytes, 1_536);
    assert_eq!(snapshot.totals.response_bytes, 6_144);
    assert_eq!(snapshot.totals.total_bytes, 7_680);
    assert_eq!(snapshot.points.len(), 2);
    assert_eq!(snapshot.points[0].bucket_start_ms, 1_700_000_000_000);
    assert_eq!(snapshot.points[0].total_bytes, 5_120);
    assert_eq!(snapshot.points[1].bucket_start_ms, 1_700_000_060_000);
    assert_eq!(snapshot.points[1].total_bytes, 2_560);
    assert_eq!(snapshot.proxies.len(), 1);
    assert_eq!(snapshot.proxies[0].proxy_config_id.as_deref(), Some("proxy-main"));
    assert_eq!(snapshot.proxies[0].totals.total_bytes, 7_680);

    std::fs::remove_dir_all(&root).expect("cleanup proxy traffic test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test]
async fn proxy_traffic_rollup_does_not_double_count_replayed_events() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-proxy-traffic-dedupe", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create proxy traffic dedupe test directory");
    let db_path = root.join("usage.duckdb");
    let repo = super::DuckDbUsageRepository::open_path(&db_path).expect("open usage repo");

    let mut event = test_usage_event();
    event.event_id = "proxy-traffic-replayed".to_string();
    event.created_at_ms = 1_700_000_000_000;
    event.request_body_bytes = Some(256);
    event.stream.bytes_streamed = Some(768);
    let proxy = crate::postgres::UsageProxyAttribution {
        provider_type: "kiro".to_string(),
        account_name: "kiro-account".to_string(),
        proxy_source: "fixed".to_string(),
        proxy_config_id: Some("proxy-dedupe".to_string()),
        proxy_config_name: Some("Proxy Dedupe".to_string()),
        proxy_url: Some("http://127.0.0.1:18082".to_string()),
    };
    let row = super::UsageEventRow::from_usage_event(&event).with_proxy_attribution(Some(&proxy));

    repo.append_usage_event_rows_owned(vec![row.clone()])
        .await
        .expect("append usage row once");
    repo.append_usage_event_rows_owned(vec![row])
        .await
        .expect("replay usage row");

    let snapshot = repo
        .proxy_traffic_snapshot(ProxyTrafficQuery {
            proxy_config_id: Some("proxy-dedupe".to_string()),
            provider_type: None,
            source: UsageEventSource::Hot,
            start_ms: 1_700_000_000_000,
            end_ms: 1_700_000_060_000,
            bucket_ms: 60_000,
        })
        .await
        .expect("fetch proxy traffic snapshot");

    assert_eq!(snapshot.totals.event_count, 1);
    assert_eq!(snapshot.totals.request_bytes, 256);
    assert_eq!(snapshot.totals.response_bytes, 768);
    assert_eq!(snapshot.totals.total_bytes, 1_024);

    std::fs::remove_dir_all(&root).expect("cleanup proxy traffic dedupe test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn proxy_traffic_migration_backfill_collapses_metadata_drift() {
    let root = std::env::temp_dir().join(format!(
        "llm-access-duckdb-test-{}-proxy-traffic-backfill-drift",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create proxy traffic backfill test directory");
    let db_path = root.join("usage.duckdb");
    let conn = duckdb::Connection::open(&db_path).expect("open duckdb");
    let migrations = llm_access_migrations::duckdb_migrations();
    conn.execute_batch(migrations[0].sql)
        .expect("initialize v1 schema");
    conn.execute_batch(
        r#"
        INSERT INTO usage_events (
            source_seq,
            source_event_id,
            event_id,
            created_at_ms,
            created_at,
            created_date,
            created_hour,
            provider_type,
            protocol_family,
            key_id,
            key_name,
            key_status_at_event,
            endpoint,
            status_code,
            request_body_bytes,
            bytes_streamed,
            input_uncached_tokens,
            input_cached_tokens,
            output_tokens,
            billable_tokens,
            usage_missing,
            credit_usage_missing,
            proxy_source_at_event,
            proxy_config_id_at_event,
            proxy_config_name_at_event,
            proxy_url_at_event
        ) VALUES
            (
                1,
                'source-proxy-drift-1',
                'proxy-drift-1',
                1700000000000,
                to_timestamp(1700000000),
                CAST(to_timestamp(1700000000) AS DATE),
                date_trunc('hour', to_timestamp(1700000000)),
                'kiro',
                'anthropic',
                'key-1',
                'Key 1',
                'active',
                '/cc/v1/messages',
                200,
                100,
                400,
                0,
                0,
                0,
                0,
                false,
                false,
                'fixed',
                'proxy-drift',
                'Proxy Old',
                'http://127.0.0.1:18080'
            ),
            (
                2,
                'source-proxy-drift-2',
                'proxy-drift-2',
                1700000600000,
                to_timestamp(1700000600),
                CAST(to_timestamp(1700000600) AS DATE),
                date_trunc('hour', to_timestamp(1700000600)),
                'kiro',
                'anthropic',
                'key-1',
                'Key 1',
                'active',
                '/cc/v1/messages',
                200,
                200,
                800,
                0,
                0,
                0,
                0,
                false,
                false,
                'binding',
                'proxy-drift',
                'Proxy New',
                'http://127.0.0.1:18081'
            );
        "#,
    )
    .expect("insert drifted usage events");

    conn.execute_batch(migrations[2].sql)
        .expect("backfill proxy traffic rollups with drifted metadata");

    let row = conn
        .query_row(
            "SELECT
                count(*),
                max(request_count),
                max(request_bytes),
                max(response_bytes),
                max(total_bytes),
                max(proxy_source),
                max(proxy_config_name),
                max(proxy_url)
             FROM proxy_traffic_rollups_hourly",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, String>(7)?,
                ))
            },
        )
        .expect("query backfilled proxy traffic rollup");

    assert_eq!(row.0, 1);
    assert_eq!(row.1, 2);
    assert_eq!(row.2, 300);
    assert_eq!(row.3, 1_200);
    assert_eq!(row.4, 1_500);
    assert_eq!(row.5, "binding");
    assert_eq!(row.6, "Proxy New");
    assert_eq!(row.7, "http://127.0.0.1:18081");

    std::fs::remove_dir_all(&root).expect("cleanup proxy traffic backfill test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[test]
fn duckdb_initialization_drops_legacy_usage_art_indexes() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-drop-indexes", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create duckdb test directory");
    let db_path = root.join("usage.duckdb");
    let conn = duckdb::Connection::open(&db_path).expect("open duckdb");

    crate::initialize_duckdb_target(&conn).expect("initialize duckdb");
    conn.execute_batch(
        "
        CREATE UNIQUE INDEX IF NOT EXISTS idx_usage_events_source_event_id
            ON usage_events(source_event_id);
        CREATE INDEX IF NOT EXISTS idx_usage_events_source_seq
            ON usage_events(source_seq);
        CREATE INDEX IF NOT EXISTS idx_usage_events_created_date
            ON usage_events(created_date);
        CREATE INDEX IF NOT EXISTS idx_usage_events_key_date
            ON usage_events(key_id, created_date);
        CREATE INDEX IF NOT EXISTS idx_usage_events_provider_date
            ON usage_events(provider_type, created_date);
        ",
    )
    .expect("create legacy indexes");

    crate::initialize_duckdb_target(&conn).expect("reinitialize duckdb");
    let mut stmt = conn
        .prepare("SELECT index_name FROM duckdb_indexes() ORDER BY index_name")
        .expect("prepare index query");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query indexes");
    let indexes = rows.collect::<Result<Vec<_>, _>>().expect("read indexes");

    assert!(
        indexes.is_empty(),
        "only implicit primary key constraints should remain, found explicit indexes: {indexes:?}"
    );

    std::fs::remove_dir_all(&root).expect("cleanup duckdb test directory");
}

#[cfg(feature = "duckdb-runtime")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duckdb_tiered_append_serializes_retention_via_write_gate() {
    let root = std::env::temp_dir()
        .join(format!("llm-access-duckdb-test-{}-tiered-write-gate", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("create tiered duckdb test directory");
    // Large rollover threshold so the append's own size-based rollover never
    // fires — this test only exercises append-vs-retention serialization.
    let repo = std::sync::Arc::new(
        super::DuckDbUsageRepository::open_tiered(super::TieredDuckDbUsageConfig {
            active_dir: root.join("active"),
            archive_dir: root.join("archive"),
            rollover_bytes: 1 << 30,
            details_dir: Some(details_store_dir(&root)),
        })
        .expect("open tiered duckdb usage db"),
    );

    // Install the seam so the spawned append parks — still holding the write
    // gate — right after acquiring it.
    let (reached_tx, reached_rx) = tokio::sync::oneshot::channel::<()>();
    let (proceed_tx, proceed_rx) = tokio::sync::oneshot::channel::<()>();
    match repo.inner.as_ref() {
        super::DuckDbUsageRepositoryInner::Tiered {
            state, ..
        } => {
            let mut state = state.lock().expect("lock tiered state");
            state.append_seam = Some(super::AppendSeam {
                reached: reached_tx,
                proceed: proceed_rx,
            });
        },
        _ => panic!("expected tiered repository"),
    }

    let append_repo = std::sync::Arc::clone(&repo);
    let append_task = tokio::spawn(async move {
        let mut event = test_usage_event();
        event.event_id = "write-gate-inflight".to_string();
        append_repo
            .append_usage_event(&event)
            .await
            .expect("append parks at seam then completes");
    });

    // Wait until the append is parked while holding the gate.
    reached_rx.await.expect("append reached the seam");

    // Retention must block on the write gate while the append holds it.
    let blocked = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        repo.prune_usage_analytics(1_700_000_000_000, 30),
    )
    .await;
    assert!(blocked.is_err(), "retention must block on the write gate while an append holds it");

    // Release the parked append; it finishes and drops the gate.
    proceed_tx.send(()).expect("signal append to proceed");
    append_task.await.expect("append task joined");

    // With the gate released, retention must no longer block.
    let unblocked = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        repo.prune_usage_analytics(1_700_000_000_000, 30),
    )
    .await;
    assert!(unblocked.is_ok(), "retention must proceed once the append releases the gate");

    std::fs::remove_dir_all(&root).expect("cleanup tiered duckdb test directory");
}
