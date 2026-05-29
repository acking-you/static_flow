use std::sync::OnceLock;

use anyhow::Context;
use llm_access_core::{
    provider::{ProtocolFamily, ProviderType, RouteStrategy},
    store::{
        AdminCodexAccountPageQuery, AdminCodexAccountSortMode, AdminCodexAccountStore,
        AdminConfigStore, AdminKeyStore, AdminProxyConfigPatch, AdminProxyStore,
        AdminReviewQueueStore, ControlStore, NewAdminProxyConfig,
        NewPublicAccountContributionRequest, PublicSubmissionStore, PublicUsageStore,
        UsageEventSink,
    },
};
use sha2::{Digest, Sha256};

use super::SqlxClient;

static TEST_DB_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

async fn test_db_guard() -> tokio::sync::MutexGuard<'static, ()> {
    TEST_DB_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

async fn reset_test_db(database_url: &str) -> anyhow::Result<()> {
    crate::initialize_postgres_target(database_url)
        .await
        .context("initialize postgres test database")?;
    let client = SqlxClient::connect(database_url)
        .await
        .context("connect postgres test database")?;
    client
        .batch_execute(
            "TRUNCATE TABLE
                    llm_account_import_job_items,
                    llm_account_import_jobs,
                    llm_codex_status_cache,
                    llm_sponsor_requests,
                    gpt2api_account_contribution_requests,
                    llm_account_contribution_requests,
                    llm_token_requests,
                    llm_kiro_status_cache,
                    llm_kiro_accounts,
                    llm_codex_accounts,
                    llm_proxy_config_endpoint_checks,
                    llm_proxy_config_node_overrides,
                    llm_proxy_bindings,
                    llm_proxy_configs,
                    llm_account_groups,
                    llm_runtime_config,
                    llm_usage_segment_events,
                    llm_usage_segment_key_rollups,
                    llm_usage_segments,
                    llm_key_usage_rollups,
                    llm_key_route_config,
                    llm_keys CASCADE",
        )
        .await
        .context("truncate postgres test fixtures")?;
    client.close().await;
    Ok(())
}

async fn seed_test_key_bundle(database_url: &str) -> anyhow::Result<()> {
    let client = SqlxClient::connect(database_url)
        .await
        .context("connect postgres test database")?;
    let key_hash = format!("{:x}", Sha256::digest(b"secret"));
    client
        .execute(
            "INSERT INTO llm_keys (
                    key_id, name, secret, key_hash, status, provider_type, protocol_family,
                    public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                 ) VALUES (
                    'key-1', 'external', 'secret', $1, 'active', 'codex', 'openai',
                    TRUE, 1000, 1700000000000, 1700000000000
                 )",
            &[&key_hash],
        )
        .await
        .context("insert postgres test key row")?;
    client
        .batch_execute(
            "INSERT INTO llm_key_route_config (
                    key_id, route_strategy, fixed_account_name, auto_account_names_json,
                    account_group_id, model_name_map_json, request_max_concurrency,
                    request_min_start_interval_ms, codex_fast_enabled,
                    kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                    kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                    kiro_cache_policy_override_json,
                    kiro_billable_model_multipliers_override_json
                 ) VALUES (
                    'key-1', NULL, NULL, NULL, NULL, NULL, NULL, NULL,
                    TRUE, FALSE, FALSE, FALSE, FALSE, NULL, NULL
                 );
                 INSERT INTO llm_key_usage_rollups (
                    key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                    billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                    updated_at_ms
                 ) VALUES (
                    'key-1', 0, 0, 0, 0, '0', 0, NULL, 1700000000000
                 );",
        )
        .await
        .context("insert postgres test key config rows")?;
    client.close().await;
    Ok(())
}

async fn seed_test_kiro_key_page_fixture(database_url: &str) -> anyhow::Result<()> {
    let client = SqlxClient::connect(database_url)
        .await
        .context("connect postgres test database")?;
    let key_hash_new = format!("{:x}", Sha256::digest(b"kiro-secret-new"));
    let key_hash_mid = format!("{:x}", Sha256::digest(b"kiro-secret-mid"));
    let key_hash_old = format!("{:x}", Sha256::digest(b"kiro-secret-old"));
    client
        .batch_execute(&format!(
            "INSERT INTO llm_keys (
                        key_id, name, secret, key_hash, status, provider_type, protocol_family,
                        public_visible, quota_billable_limit, created_at_ms, updated_at_ms
                     ) VALUES
                        ('kiro-key-new', 'kiro-new', 'kiro-secret-new', '{key_hash_new}', \
             'active', 'kiro', 'anthropic', TRUE, 1000, 300, 300),
                        ('kiro-key-mid', 'kiro-mid', 'kiro-secret-mid', '{key_hash_mid}', \
             'active', 'kiro', 'anthropic', TRUE, 1000, 200, 200),
                        ('kiro-key-old', 'kiro-old', 'kiro-secret-old', '{key_hash_old}', \
             'active', 'kiro', 'anthropic', TRUE, 1000, 100, 100);
                     INSERT INTO llm_key_route_config (
                        key_id, route_strategy, fixed_account_name, auto_account_names_json,
                        account_group_id, model_name_map_json, request_max_concurrency,
                        request_min_start_interval_ms, codex_fast_enabled,
                        kiro_request_validation_enabled, kiro_cache_estimation_enabled,
                        kiro_zero_cache_debug_enabled, kiro_full_request_logging_enabled,
                        kiro_cache_policy_override_json,
                        kiro_billable_model_multipliers_override_json
                     ) VALUES
                        ('kiro-key-new', 'auto', NULL, NULL, NULL, NULL, NULL, NULL, TRUE, TRUE, \
             TRUE, FALSE, FALSE, NULL, NULL),
                        ('kiro-key-mid', 'fixed', 'kiro-a', NULL, 'group-beta', NULL, NULL, NULL, \
             TRUE, TRUE, TRUE, FALSE, FALSE, NULL, NULL),
                        ('kiro-key-old', 'auto', NULL, '[\"kiro-a\", \"kiro-d\", \
             \"kiro-a\"]'::jsonb, NULL, NULL, NULL, NULL, TRUE, TRUE, TRUE, FALSE, FALSE, NULL, \
             NULL);
                     INSERT INTO llm_key_usage_rollups (
                        key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
                        billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
                        updated_at_ms
                     ) VALUES
                        ('kiro-key-new', 0, 0, 0, 0, '0', 0, NULL, 300),
                        ('kiro-key-mid', 0, 0, 0, 0, '0', 0, NULL, 200),
                        ('kiro-key-old', 0, 0, 0, 0, '0', 0, NULL, 100);
                     INSERT INTO llm_account_groups (
                        group_id, provider_type, name, account_names_json, created_at_ms, \
             updated_at_ms
                     ) VALUES
                        ('group-beta', 'kiro', 'group-beta', '[\"kiro-b\", \"kiro-c\", \
             \"kiro-b\"]'::jsonb, 10, 10);
                     INSERT INTO llm_kiro_accounts (
                        account_name, auth_method, account_id, profile_arn, user_id,
                        status, auth_json, max_concurrency, min_start_interval_ms,
                        proxy_config_id, last_refresh_at_ms, last_error, created_at_ms, \
             updated_at_ms
                     ) VALUES
                        ('kiro-a', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
             NULL, NULL, NULL, 10, 10),
                        ('kiro-b', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
             NULL, NULL, NULL, 20, 20),
                        ('kiro-c', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
             NULL, NULL, NULL, 30, 30),
                        ('kiro-d', 'social', NULL, NULL, NULL, 'active', '{{}}'::jsonb, 1, 0, \
             NULL, NULL, NULL, 40, 40);
                     INSERT INTO llm_kiro_status_cache (
                        account_name, status, balance_json, cache_json, refreshed_at_ms,
                        expires_at_ms, last_error
                     ) VALUES
                        ('kiro-a', 'active', \
             '{{\"current_usage\":60.0,\"usage_limit\":100.0,\"remaining\":40.0,\"next_reset_at\":\
             null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, 2, NULL),
                        ('kiro-b', 'active', \
             '{{\"current_usage\":50.0,\"usage_limit\":200.0,\"remaining\":150.0,\"next_reset_at\"\
             :null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, 2, NULL),
                        ('kiro-c', 'active', 'null'::jsonb, '{{}}'::jsonb, 1, 2, NULL),
                        ('kiro-d', 'active', \
             '{{\"current_usage\":210.0,\"usage_limit\":300.0,\"remaining\":90.0,\"next_reset_at\"\
             :null,\"subscription_title\":\"Pro\"}}'::jsonb, '{{}}'::jsonb, 1, 2, NULL);"
        ))
        .await
        .context("seed postgres kiro key page fixture")?;
    client.close().await;
    Ok(())
}

async fn seed_test_codex_account_page_fixture(database_url: &str) -> anyhow::Result<()> {
    let client = SqlxClient::connect(database_url)
        .await
        .context("connect postgres test database")?;
    client
        .batch_execute(
            r#"
                INSERT INTO llm_codex_accounts (
                    account_name, account_id, email, status, auth_json, settings_json,
                    last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
                ) VALUES
                    (
                        'codex-new', 'acct-new', NULL, 'active',
                        '{"access_token":"token-new"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"auto","proxy_mode":"inherit"}'::jsonb,
                        290, NULL, 300, 300
                    ),
                    (
                        'codex-mid', 'acct-mid', NULL, 'disabled',
                        '{"access_token":"token-mid"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"free","proxy_mode":"inherit"}'::jsonb,
                        190, NULL, 200, 200
                    ),
                    (
                        'codex-old', 'acct-old', NULL, 'active',
                        '{"access_token":"token-old"}'::jsonb,
                        '{"auth_refresh_enabled":true,"map_gpt53_codex_to_spark":false,
                          "route_weight_tier":"plus","proxy_mode":"inherit"}'::jsonb,
                        90, 'refresh failed', 100, 100
                    );
                INSERT INTO llm_codex_status_cache (id, snapshot_json, updated_at_ms)
                VALUES (
                    'default',
                    '{
                        "status":"ready",
                        "refresh_interval_seconds":300,
                        "last_checked_at":400,
                        "last_success_at":400,
                        "source_url":"https://chatgpt.com/backend-api/codex/models",
                        "error_message":null,
                        "accounts":[
                            {
                                "name":"codex-new",
                                "status":"active",
                                "plan_type":"Pro",
                                "primary_remaining_percent":70.0,
                                "secondary_remaining_percent":80.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            },
                            {
                                "name":"codex-mid",
                                "status":"active",
                                "plan_type":"Pro",
                                "primary_remaining_percent":55.0,
                                "secondary_remaining_percent":60.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            },
                            {
                                "name":"codex-old",
                                "status":"active",
                                "plan_type":"Plus",
                                "primary_remaining_percent":20.0,
                                "secondary_remaining_percent":10.0,
                                "last_usage_checked_at":400,
                                "last_usage_success_at":400,
                                "usage_error_message":null
                            }
                        ],
                        "buckets":[]
                    }'::jsonb,
                    400
                );
                "#,
        )
        .await
        .context("seed postgres codex account page fixture")?;
    client.close().await;
    Ok(())
}

#[tokio::test]
async fn postgres_repository_reads_runtime_config_and_authenticates_key() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    seed_test_key_bundle(&database_url)
        .await
        .expect("seed postgres test key bundle");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    let config = repo
        .get_admin_runtime_config()
        .await
        .expect("runtime config");
    assert_eq!(config.codex_client_version.as_str(), "0.124.0");

    let key = repo
        .authenticate_bearer_secret("secret")
        .await
        .expect("lookup result")
        .expect("key must exist");
    assert_eq!(key.key_name, "external");
}

#[tokio::test]
async fn postgres_repository_accepts_optional_request_cache_config() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");

    let repo = super::PostgresControlRepository::connect(
        &database_url,
        Some(crate::request_cache::RequestCacheConfig {
            url: "redis://127.0.0.1:6379/0".to_string(),
            key_prefix: "llma:test".to_string(),
        }),
    )
    .await
    .expect("connect postgres repository with request cache");

    assert!(repo.request_cache.is_some());
}

#[tokio::test]
async fn postgres_repository_resolves_proxy_configs_per_node_scope() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    let core_repo = super::PostgresControlRepository::connect_with_proxy_scope(
        &database_url,
        None,
        super::ProxyConfigScope::core(),
    )
    .await
    .expect("connect core postgres repository");
    let edge_repo = super::PostgresControlRepository::connect_with_proxy_scope(
        &database_url,
        None,
        super::ProxyConfigScope::node("edge-a"),
    )
    .await
    .expect("connect edge postgres repository");

    core_repo
        .create_admin_proxy_config(NewAdminProxyConfig {
            id: "proxy-slot-1".to_string(),
            name: "slot 1".to_string(),
            proxy_url: "http://core.proxy:1111".to_string(),
            proxy_username: Some("core-user".to_string()),
            proxy_password: Some("core-pass".to_string()),
            created_at_ms: 100,
        })
        .await
        .expect("create core proxy slot");

    let inherited = edge_repo
        .get_admin_proxy_config("proxy-slot-1")
        .await
        .expect("load inherited edge proxy")
        .expect("edge sees core slot");
    assert_eq!(inherited.proxy_url, "http://core.proxy:1111");
    assert_eq!(inherited.effective_source, "core");
    assert!(!inherited.has_node_override);

    let overridden = edge_repo
        .patch_admin_proxy_config("proxy-slot-1", AdminProxyConfigPatch {
            proxy_url: Some("http://edge.proxy:2222".to_string()),
            proxy_username: Some(Some("edge-user".to_string())),
            proxy_password: Some(Some("edge-pass".to_string())),
            status: Some("active".to_string()),
            updated_at_ms: 200,
            ..AdminProxyConfigPatch::default()
        })
        .await
        .expect("patch edge proxy override")
        .expect("edge proxy slot exists");
    assert_eq!(overridden.proxy_url, "http://edge.proxy:2222");
    assert_eq!(overridden.proxy_username.as_deref(), Some("edge-user"));
    assert_eq!(overridden.effective_source, "node_override");
    assert!(overridden.has_node_override);

    let core_after_override = core_repo
        .get_admin_proxy_config("proxy-slot-1")
        .await
        .expect("load core proxy")
        .expect("core slot exists");
    assert_eq!(core_after_override.proxy_url, "http://core.proxy:1111");
    assert_eq!(core_after_override.effective_source, "core");

    edge_repo
        .update_admin_proxy_binding("codex", Some("proxy-slot-1".to_string()))
        .await
        .expect("bind codex proxy slot");
    let edge_context = edge_repo
        .load_provider_proxy_resolution_context("codex")
        .await
        .expect("load edge proxy context");
    let fixed_proxy = super::resolve_provider_proxy_config_from_context(
        "fixed",
        Some("proxy-slot-1"),
        &edge_context,
    )
    .expect("resolve fixed edge proxy")
    .expect("fixed proxy present");
    assert_eq!(fixed_proxy.proxy_url, "http://edge.proxy:2222");
    assert_eq!(edge_context.binding.effective_proxy_url.as_deref(), Some("http://edge.proxy:2222"));

    let reset = edge_repo
        .reset_admin_proxy_config_override("proxy-slot-1")
        .await
        .expect("reset edge proxy override")
        .expect("edge proxy slot exists after reset");
    assert_eq!(reset.proxy_url, "http://core.proxy:1111");
    assert_eq!(reset.effective_source, "core");
    assert!(!reset.has_node_override);
}

#[tokio::test]
async fn postgres_repository_records_proxy_endpoint_checks_per_node_scope() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    let core_repo = super::PostgresControlRepository::connect_with_proxy_scope(
        &database_url,
        None,
        super::ProxyConfigScope::core(),
    )
    .await
    .expect("connect core postgres repository");
    let edge_repo = super::PostgresControlRepository::connect_with_proxy_scope(
        &database_url,
        None,
        super::ProxyConfigScope::node("edge-a"),
    )
    .await
    .expect("connect edge postgres repository");

    core_repo
        .create_admin_proxy_config(NewAdminProxyConfig {
            id: "proxy-slot-1".to_string(),
            name: "slot 1".to_string(),
            proxy_url: "http://core.proxy:1111".to_string(),
            proxy_username: None,
            proxy_password: None,
            created_at_ms: 100,
        })
        .await
        .expect("create core proxy slot");

    core_repo
        .record_admin_proxy_endpoint_check(llm_access_core::store::AdminProxyEndpointCheckUpdate {
            proxy_config_id: "proxy-slot-1".to_string(),
            provider_type: "codex".to_string(),
            target_url: "https://chatgpt.com/backend-api/codex/models".to_string(),
            reachable: true,
            status_code: Some(401),
            latency_ms: 1234,
            error_message: Some("unauthorized".to_string()),
            checked_at_ms: 200,
        })
        .await
        .expect("record core codex check");
    edge_repo
        .record_admin_proxy_endpoint_check(llm_access_core::store::AdminProxyEndpointCheckUpdate {
            proxy_config_id: "proxy-slot-1".to_string(),
            provider_type: "codex".to_string(),
            target_url: "https://chatgpt.com/backend-api/codex/models".to_string(),
            reachable: true,
            status_code: Some(200),
            latency_ms: 321,
            error_message: None,
            checked_at_ms: 250,
        })
        .await
        .expect("record edge codex check");

    let core_checked = core_repo
        .get_admin_proxy_config("proxy-slot-1")
        .await
        .expect("load core checked proxy")
        .expect("core proxy exists");
    assert_eq!(
        core_checked
            .latest_codex_check
            .as_ref()
            .map(|check| check.latency_ms),
        Some(1234)
    );

    let edge_checked = edge_repo
        .get_admin_proxy_config("proxy-slot-1")
        .await
        .expect("load edge checked proxy")
        .expect("edge proxy exists");
    assert_eq!(
        edge_checked
            .latest_codex_check
            .as_ref()
            .map(|check| check.latency_ms),
        Some(321)
    );
    assert_eq!(edge_checked.effective_source, "core");
    assert!(!edge_checked.has_node_override);
}

#[tokio::test]
async fn postgres_repository_updates_key_usage_rollups() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    seed_test_key_bundle(&database_url)
        .await
        .expect("seed postgres test key bundle");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    let event = llm_access_core::usage::UsageEvent {
        event_id: "evt-1".to_string(),
        created_at_ms: 1_700_000_000_001,
        provider_type: ProviderType::Codex,
        protocol_family: ProtocolFamily::OpenAi,
        key_id: "key-1".to_string(),
        key_name: "external".to_string(),
        account_name: Some("acct-1".to_string()),
        account_group_id_at_event: None,
        route_strategy_at_event: Some(RouteStrategy::Auto),
        request_method: "POST".to_string(),
        request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        model: Some("gpt-4.1".to_string()),
        mapped_model: Some("gpt-4.1".to_string()),
        status_code: 200,
        request_body_bytes: Some(256),
        quota_failover_count: 0,
        routing_diagnostics_json: None,
        input_uncached_tokens: 10,
        input_cached_tokens: 2,
        output_tokens: 5,
        billable_tokens: 15,
        credit_usage: Some("1.25".to_string()),
        usage_missing: false,
        credit_usage_missing: false,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: None,
        client_request_body_json: None,
        upstream_request_body_json: None,
        full_request_json: None,
        error_message: None,
        error_body: None,
        timing: llm_access_core::usage::UsageTiming {
            latency_ms: Some(120),
            ..Default::default()
        },
        stream: llm_access_core::usage::UsageStreamDetails::default(),
    };
    repo.apply_usage_rollup(&event)
        .await
        .expect("apply usage rollup");

    let key = repo
        .get_public_usage_key_by_secret("secret")
        .await
        .expect("load usage lookup key")
        .expect("public usage lookup row");
    assert_eq!(key.usage_billable_tokens, 15);
    assert_eq!(key.usage_credit_total, 1.25);
    assert_eq!(key.usage_credit_missing_events, 0);
    assert_eq!(key.last_used_at_ms, Some(1_700_000_000_001));
}

#[tokio::test]
async fn postgres_repository_creates_account_contribution_request() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    repo.create_public_account_contribution_request(NewPublicAccountContributionRequest {
        request_id: "req-1".to_string(),
        account_name: "acct-1".to_string(),
        account_id: Some("acct-id-1".to_string()),
        id_token: "id-token".to_string(),
        access_token: "access-token".to_string(),
        refresh_token: "refresh-token".to_string(),
        requester_email: "user@example.com".to_string(),
        contributor_message: "hello".to_string(),
        github_id: None,
        frontend_page_url: None,
        show_on_public_wall: true,
        fingerprint: "fp".to_string(),
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        created_at_ms: 1_700_000_000_100,
    })
    .await
    .expect("create account contribution request");

    let created = repo
        .get_admin_account_contribution_request("req-1")
        .await
        .expect("load request")
        .expect("request row");
    assert_eq!(created.status, "pending");
    assert_eq!(created.account_name, "acct-1");
    assert_eq!(created.account_id.as_deref(), Some("acct-id-1"));
}

#[test]
fn aggregate_usage_rollup_deltas_merges_events() {
    let events = vec![
        llm_access_core::usage::UsageEvent {
            event_id: "evt-1".to_string(),
            created_at_ms: 10,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: "key-1".to_string(),
            key_name: "external".to_string(),
            account_name: None,
            account_group_id_at_event: None,
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: None,
            mapped_model: None,
            status_code: 200,
            request_body_bytes: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 10,
            input_cached_tokens: 1,
            output_tokens: 5,
            billable_tokens: 15,
            credit_usage: Some("1.25".to_string()),
            usage_missing: false,
            credit_usage_missing: false,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            timing: llm_access_core::usage::UsageTiming::default(),
            stream: llm_access_core::usage::UsageStreamDetails::default(),
        },
        llm_access_core::usage::UsageEvent {
            event_id: "evt-2".to_string(),
            created_at_ms: 25,
            provider_type: ProviderType::Codex,
            protocol_family: ProtocolFamily::OpenAi,
            key_id: "key-1".to_string(),
            key_name: "external".to_string(),
            account_name: None,
            account_group_id_at_event: None,
            route_strategy_at_event: None,
            request_method: "POST".to_string(),
            request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
            endpoint: "/v1/chat/completions".to_string(),
            model: None,
            mapped_model: None,
            status_code: 200,
            request_body_bytes: None,
            quota_failover_count: 0,
            routing_diagnostics_json: None,
            input_uncached_tokens: 4,
            input_cached_tokens: 0,
            output_tokens: 1,
            billable_tokens: 5,
            credit_usage: Some("0.5".to_string()),
            usage_missing: false,
            credit_usage_missing: true,
            client_ip: "127.0.0.1".to_string(),
            ip_region: "local".to_string(),
            request_headers_json: "{}".to_string(),
            last_message_content: None,
            client_request_body_json: None,
            upstream_request_body_json: None,
            full_request_json: None,
            error_message: None,
            error_body: None,
            timing: llm_access_core::usage::UsageTiming::default(),
            stream: llm_access_core::usage::UsageStreamDetails::default(),
        },
    ];

    let deltas = super::aggregate_usage_rollup_deltas(&events).expect("aggregate usage deltas");
    assert_eq!(deltas.len(), 1);
    let (key_id, delta) = deltas[0];
    assert_eq!(key_id, "key-1");
    assert_eq!(delta.input_uncached_tokens, 14);
    assert_eq!(delta.input_cached_tokens, 1);
    assert_eq!(delta.output_tokens, 6);
    assert_eq!(delta.billable_tokens, 20);
    assert_eq!(delta.credit_total, 1.75);
    assert_eq!(delta.credit_missing_events, 1);
    assert_eq!(delta.last_used_at_ms, 25);
}

#[tokio::test]
async fn postgres_repository_batches_key_usage_rollups() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    seed_test_key_bundle(&database_url)
        .await
        .expect("seed postgres test key bundle");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    let first = llm_access_core::usage::UsageEvent {
        event_id: "evt-1".to_string(),
        created_at_ms: 1_700_000_000_001,
        provider_type: ProviderType::Codex,
        protocol_family: ProtocolFamily::OpenAi,
        key_id: "key-1".to_string(),
        key_name: "external".to_string(),
        account_name: Some("acct-1".to_string()),
        account_group_id_at_event: None,
        route_strategy_at_event: Some(RouteStrategy::Auto),
        request_method: "POST".to_string(),
        request_url: "https://ackingliu.top/v1/chat/completions".to_string(),
        endpoint: "/v1/chat/completions".to_string(),
        model: Some("gpt-4.1".to_string()),
        mapped_model: Some("gpt-4.1".to_string()),
        status_code: 200,
        request_body_bytes: Some(256),
        quota_failover_count: 0,
        routing_diagnostics_json: None,
        input_uncached_tokens: 10,
        input_cached_tokens: 2,
        output_tokens: 5,
        billable_tokens: 15,
        credit_usage: Some("1.25".to_string()),
        usage_missing: false,
        credit_usage_missing: false,
        client_ip: "127.0.0.1".to_string(),
        ip_region: "local".to_string(),
        request_headers_json: "{}".to_string(),
        last_message_content: None,
        client_request_body_json: None,
        upstream_request_body_json: None,
        full_request_json: None,
        error_message: None,
        error_body: None,
        timing: llm_access_core::usage::UsageTiming {
            latency_ms: Some(120),
            ..Default::default()
        },
        stream: llm_access_core::usage::UsageStreamDetails::default(),
    };
    let second = llm_access_core::usage::UsageEvent {
        event_id: "evt-2".to_string(),
        created_at_ms: 1_700_000_000_101,
        input_uncached_tokens: 4,
        input_cached_tokens: 2,
        output_tokens: 1,
        billable_tokens: 5,
        credit_usage: Some("0.50".to_string()),
        ..first.clone()
    };

    repo.append_usage_events(&[first, second])
        .await
        .expect("append usage events");

    let key = repo
        .get_public_usage_key_by_secret("secret")
        .await
        .expect("load usage lookup key")
        .expect("public usage lookup row");
    assert_eq!(key.usage_input_uncached_tokens, 14);
    assert_eq!(key.usage_input_cached_tokens, 4);
    assert_eq!(key.usage_output_tokens, 6);
    assert_eq!(key.usage_billable_tokens, 20);
    assert_eq!(key.usage_credit_total, 1.75);
    assert_eq!(key.usage_credit_missing_events, 0);
    assert_eq!(key.last_used_at_ms, Some(1_700_000_000_101));
}

#[tokio::test]
async fn postgres_repository_lists_kiro_key_pages_with_candidate_credit_summaries() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    seed_test_kiro_key_page_fixture(&database_url)
        .await
        .expect("seed postgres kiro key page fixture");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    let first_page = repo
        .list_admin_keys_page(Some("kiro"), llm_access_core::store::AdminPageRequest {
            limit: 2,
            offset: 0,
        })
        .await
        .expect("list first kiro key page");
    assert_eq!(first_page.total, 3);
    assert!(first_page.has_more);
    assert_eq!(
        first_page
            .keys
            .iter()
            .map(|key| key.id.as_str())
            .collect::<Vec<_>>(),
        ["kiro-key-new", "kiro-key-mid"]
    );
    let newest_summary = first_page.keys[0]
        .kiro_candidate_credit_summary
        .expect("newest key candidate summary");
    assert_eq!(newest_summary.candidate_count, 4);
    assert_eq!(newest_summary.loaded_balance_count, 3);
    assert_eq!(newest_summary.missing_balance_count, 1);
    assert_eq!(newest_summary.total_limit, 600.0);
    assert_eq!(newest_summary.total_remaining, 280.0);
    let middle_summary = first_page.keys[1]
        .kiro_candidate_credit_summary
        .expect("middle key candidate summary");
    assert_eq!(middle_summary.candidate_count, 2);
    assert_eq!(middle_summary.loaded_balance_count, 1);
    assert_eq!(middle_summary.missing_balance_count, 1);
    assert_eq!(middle_summary.total_limit, 200.0);
    assert_eq!(middle_summary.total_remaining, 150.0);

    let second_page = repo
        .list_admin_keys_page(Some("kiro"), llm_access_core::store::AdminPageRequest {
            limit: 2,
            offset: 2,
        })
        .await
        .expect("list second kiro key page");
    assert_eq!(second_page.total, 3);
    assert!(!second_page.has_more);
    assert_eq!(second_page.keys.len(), 1);
    assert_eq!(second_page.keys[0].id, "kiro-key-old");
    let oldest_summary = second_page.keys[0]
        .kiro_candidate_credit_summary
        .expect("oldest key candidate summary");
    assert_eq!(oldest_summary.candidate_count, 2);
    assert_eq!(oldest_summary.loaded_balance_count, 2);
    assert_eq!(oldest_summary.missing_balance_count, 0);
    assert_eq!(oldest_summary.total_limit, 400.0);
    assert_eq!(oldest_summary.total_remaining, 130.0);
}

#[tokio::test]
async fn postgres_repository_lists_filtered_codex_account_pages() {
    let Ok(database_url) = std::env::var("TEST_POSTGRES_URL") else {
        eprintln!("skipping postgres integration test: TEST_POSTGRES_URL is not set");
        return;
    };
    let _guard = test_db_guard().await;
    reset_test_db(&database_url)
        .await
        .expect("reset postgres test database");
    seed_test_codex_account_page_fixture(&database_url)
        .await
        .expect("seed postgres codex account page fixture");
    let repo = super::PostgresControlRepository::connect(&database_url, None)
        .await
        .expect("connect postgres repository");

    let primary_sorted = repo
        .list_admin_codex_accounts_filtered_page(
            &AdminCodexAccountPageQuery {
                sort: AdminCodexAccountSortMode::PrimaryAsc,
                ..AdminCodexAccountPageQuery::default()
            },
            llm_access_core::store::AdminPageRequest {
                limit: 2,
                offset: 0,
            },
        )
        .await
        .expect("list codex accounts sorted by primary remaining");
    assert_eq!(primary_sorted.total, 3);
    assert!(primary_sorted.has_more);
    assert_eq!(
        primary_sorted
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>(),
        ["codex-old", "codex-new"]
    );
    assert_eq!(primary_sorted.accounts[0].plan_type.as_deref(), Some("Plus"));
    assert_eq!(primary_sorted.accounts[0].primary_remaining_percent, Some(20.0));

    let unhealthy_only = repo
        .list_admin_codex_accounts_filtered_page(
            &AdminCodexAccountPageQuery {
                unhealthy_only: true,
                ..AdminCodexAccountPageQuery::default()
            },
            llm_access_core::store::AdminPageRequest {
                limit: 8,
                offset: 0,
            },
        )
        .await
        .expect("list unhealthy codex accounts");
    assert_eq!(unhealthy_only.total, 2);
    assert_eq!(
        unhealthy_only
            .accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>(),
        ["codex-mid", "codex-old"]
    );

    let searched = repo
        .list_admin_codex_accounts_filtered_page(
            &AdminCodexAccountPageQuery {
                search: Some("plus".to_string()),
                ..AdminCodexAccountPageQuery::default()
            },
            llm_access_core::store::AdminPageRequest {
                limit: 8,
                offset: 0,
            },
        )
        .await
        .expect("search codex accounts by plan type");
    assert_eq!(searched.total, 1);
    assert_eq!(searched.accounts[0].name, "codex-old");
}
