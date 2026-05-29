use super::*;

fn sample_account_contribution_request(
    requester_email: &str,
) -> core_store::AdminAccountContributionRequest {
    core_store::AdminAccountContributionRequest {
        request_id: "req-1".to_string(),
        account_name: "codex-alpha".to_string(),
        account_id: Some("acct-alpha".to_string()),
        id_token: "id-token".to_string(),
        access_token: "access-token".to_string(),
        refresh_token: "refresh-token".to_string(),
        requester_email: requester_email.to_string(),
        contributor_message: "thanks".to_string(),
        github_id: Some("octocat".to_string()),
        frontend_page_url: Some("https://example.com/llm-access".to_string()),
        status: core_store::PUBLIC_ACCOUNT_CONTRIBUTION_STATUS_VALIDATED.to_string(),
        client_ip: "127.0.0.1".to_string(),
        ip_region: "Local".to_string(),
        admin_note: None,
        failure_reason: None,
        imported_account_name: Some("codex-alpha".to_string()),
        issued_key_id: Some("llm-key-1".to_string()),
        issued_key_name: Some("contrib-req-1".to_string()),
        created_at: 10,
        updated_at: 10,
        processed_at: Some(10),
    }
}

fn empty_key_patch_request() -> PatchLlmGatewayKeyRequest {
    PatchLlmGatewayKeyRequest {
        name: None,
        status: None,
        public_visible: None,
        quota_billable_limit: None,
        route_strategy: None,
        account_group_id: None,
        fixed_account_name: None,
        auto_account_names: None,
        model_name_map: None,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        request_max_concurrency_unlimited: false,
        request_min_start_interval_ms_unlimited: false,
        codex_fast_enabled: None,
        kiro_request_validation_enabled: None,
        kiro_cache_estimation_enabled: None,
        kiro_zero_cache_debug_enabled: None,
        kiro_full_request_logging_enabled: None,
        kiro_remote_media_resolution_enabled: None,
        kiro_latency_routing_enabled: None,
        kiro_cache_policy_override_json: None,
        kiro_billable_model_multipliers_override_json: None,
    }
}

fn sample_kiro_key(policy_override_json: Option<String>) -> core_store::AdminKey {
    core_store::AdminKey {
        id: "kiro-key-test".to_string(),
        name: "Kiro test".to_string(),
        secret: "sk-test".to_string(),
        key_hash: "hash-test".to_string(),
        status: KEY_STATUS_ACTIVE.to_string(),
        provider_type: PROVIDER_KIRO.to_string(),
        public_visible: true,
        quota_billable_limit: 1_000_000,
        usage_input_uncached_tokens: 0,
        usage_input_cached_tokens: 0,
        usage_output_tokens: 0,
        usage_credit_total: 0.0,
        usage_credit_missing_events: 0,
        remaining_billable: 1_000_000,
        last_used_at: None,
        created_at: 10,
        updated_at: 10,
        route_strategy: Some("auto".to_string()),
        account_group_id: None,
        fixed_account_name: None,
        auto_account_names: None,
        model_name_map: None,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        codex_fast_enabled: true,
        kiro_request_validation_enabled: true,
        kiro_cache_estimation_enabled: true,
        kiro_zero_cache_debug_enabled: false,
        kiro_full_request_logging_enabled: false,
        kiro_remote_media_resolution_enabled: false,
        kiro_latency_routing_enabled: true,
        kiro_cache_policy_override_json: policy_override_json,
        kiro_billable_model_multipliers_override_json: None,
        effective_kiro_cache_policy_json: "{}".to_string(),
        uses_global_kiro_cache_policy: true,
        effective_kiro_billable_model_multipliers_json:
            core_store::default_kiro_billable_model_multipliers_json(),
        uses_global_kiro_billable_model_multipliers: true,
        kiro_candidate_credit_summary: None,
    }
}

fn sample_kiro_account(name: &str, remaining: f64, limit: f64) -> core_store::AdminKiroAccount {
    core_store::AdminKiroAccount {
        name: name.to_string(),
        auth_method: "oauth".to_string(),
        provider: Some("aws".to_string()),
        upstream_user_id: Some(format!("user-{name}")),
        email: None,
        expires_at: None,
        profile_arn: None,
        has_refresh_token: true,
        disabled: false,
        disabled_reason: None,
        source: None,
        source_db_path: None,
        last_imported_at: None,
        subscription_title: Some("Pro".to_string()),
        region: Some("us-east-1".to_string()),
        auth_region: Some("us-east-1".to_string()),
        api_region: Some("us-east-1".to_string()),
        machine_id: None,
        kiro_channel_max_concurrency: 1,
        kiro_channel_min_start_interval_ms: 0,
        minimum_remaining_credits_before_block: 0.0,
        proxy_mode: "inherit".to_string(),
        proxy_config_id: None,
        effective_proxy_source: "inherit".to_string(),
        effective_proxy_url: None,
        effective_proxy_config_name: None,
        proxy_url: None,
        balance: Some(core_store::AdminKiroBalanceView {
            current_usage: (limit - remaining).max(0.0),
            usage_limit: limit,
            remaining,
            next_reset_at: None,
            subscription_title: Some("Pro".to_string()),
            user_id: Some(format!("user-{name}")),
        }),
        cache: core_store::AdminKiroCacheView::default(),
    }
}

fn sample_kiro_group(id: &str, account_names: &[&str]) -> core_store::AdminAccountGroup {
    core_store::AdminAccountGroup {
        id: id.to_string(),
        provider_type: PROVIDER_KIRO.to_string(),
        name: id.to_string(),
        account_names: account_names
            .iter()
            .map(|name| (*name).to_string())
            .collect(),
        created_at: 1,
        updated_at: 1,
    }
}

fn sample_provider_proxy(url: &str) -> core_store::ProviderProxyConfig {
    core_store::ProviderProxyConfig {
        proxy_url: url.to_string(),
        proxy_username: None,
        proxy_password: None,
    }
}

#[test]
fn normalize_probe_kiro_account_model_request_accepts_inline_proxy() {
    let request = ProbeKiroAccountModelRequest {
        model: " claude-opus-4-8 ".to_string(),
        proxy_config_id: Some("  proxy-1 ".to_string()),
        proxy_url: Some(" http://127.0.0.1:7890 ".to_string()),
        proxy_username: Some(" user ".to_string()),
        proxy_password: Some(" pass ".to_string()),
    };

    let normalized =
        normalize_probe_kiro_account_model_request(request).expect("request should normalize");

    assert_eq!(normalized.model, "claude-opus-4-8");
    assert_eq!(normalized.proxy_config_id.as_deref(), Some("proxy-1"));
    assert_eq!(
        normalized.inline_proxy,
        Some(core_store::ProviderProxyConfig {
            proxy_url: "http://127.0.0.1:7890".to_string(),
            proxy_username: Some("user".to_string()),
            proxy_password: Some("pass".to_string()),
        })
    );
}

#[test]
fn normalize_probe_kiro_account_model_request_rejects_proxy_auth_without_url() {
    let err = normalize_probe_kiro_account_model_request(ProbeKiroAccountModelRequest {
        model: "claude-opus-4-8".to_string(),
        proxy_config_id: None,
        proxy_url: None,
        proxy_username: Some("user".to_string()),
        proxy_password: None,
    })
    .expect_err("inline proxy credentials without url must fail");

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.message.contains("proxy_url"));
}

#[test]
fn build_direct_kiro_model_probe_request_preserves_requested_model_id() {
    let request = build_direct_kiro_model_probe_request(
        "claude-opus-4-8",
        Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/test".to_string()),
    );

    assert_eq!(
        request
            .conversation_state
            .current_message
            .user_input_message
            .model_id,
        "claude-opus-4-8"
    );
    assert_eq!(
        request
            .conversation_state
            .current_message
            .user_input_message
            .content,
        ADMIN_KIRO_MODEL_PROBE_PROMPT
    );
    assert_eq!(request.conversation_state.chat_trigger_type.as_deref(), Some("MANUAL"));
    assert!(request.conversation_state.history.is_empty());
    assert_eq!(
        request.profile_arn.as_deref(),
        Some("arn:aws:codewhisperer:us-east-1:123456789012:profile/test")
    );
}

#[test]
fn select_admin_kiro_probe_proxy_prefers_inline_then_proxy_config_then_resolved() {
    let inline_proxy = sample_provider_proxy("http://inline:7890");
    let proxy_config_proxy = sample_provider_proxy("http://config:7890");
    let resolved_proxy = sample_provider_proxy("http://resolved:7890");

    let (selected, source) = select_admin_kiro_probe_proxy(
        Some(inline_proxy.clone()),
        Some(proxy_config_proxy.clone()),
        Some(resolved_proxy.clone()),
    );
    assert_eq!(selected, Some(inline_proxy));
    assert_eq!(source, AdminKiroProbeProxySource::Inline);

    let (selected, source) = select_admin_kiro_probe_proxy(
        None,
        Some(proxy_config_proxy.clone()),
        Some(resolved_proxy.clone()),
    );
    assert_eq!(selected, Some(proxy_config_proxy));
    assert_eq!(source, AdminKiroProbeProxySource::ProxyConfig);

    let (selected, source) =
        select_admin_kiro_probe_proxy(None, None, Some(resolved_proxy.clone()));
    assert_eq!(selected, Some(resolved_proxy));
    assert_eq!(source, AdminKiroProbeProxySource::Resolved);

    let (selected, source) = select_admin_kiro_probe_proxy(None, None, None);
    assert_eq!(selected, None);
    assert_eq!(source, AdminKiroProbeProxySource::None);
}

#[test]
fn kiro_probe_eventstream_error_message_prefers_stream_errors() {
    let message = kiro_probe_eventstream_error_message(&[
        llm_access_kiro::wire::Event::Unknown {},
        llm_access_kiro::wire::Event::Error {
            error_code: "InvalidModel".to_string(),
            error_message: r#"{"message":"model is not supported"}"#.to_string(),
        },
    ])
    .expect("stream error should be surfaced");

    assert!(message.contains("InvalidModel"));
    assert!(message.contains("model is not supported"));
}

#[test]
fn normalize_key_patch_accepts_partial_kiro_cache_policy_override() {
    let mut request = empty_key_patch_request();
    request.kiro_cache_policy_override_json = Some(Some(
        r#"{"small_input_high_credit_boost":{"target_input_tokens":50000}}"#.to_string(),
    ));

    let patch = normalize_key_patch(request).expect("partial override should be accepted");

    assert!(patch
        .kiro_cache_policy_override_json
        .as_ref()
        .and_then(|value| value.as_ref())
        .is_some_and(|json| json.contains("target_input_tokens")));
}

#[test]
fn normalize_key_patch_accepts_kiro_full_request_logging_toggle() {
    let mut request = empty_key_patch_request();
    request.kiro_full_request_logging_enabled = Some(true);

    let patch = normalize_key_patch(request).expect("full request logging toggle");

    assert_eq!(patch.kiro_full_request_logging_enabled, Some(true));
}

#[test]
fn normalize_key_patch_accepts_kiro_remote_media_resolution_toggle() {
    let mut request = empty_key_patch_request();
    request.kiro_remote_media_resolution_enabled = Some(true);

    let patch = normalize_key_patch(request).expect("remote media resolution toggle");

    assert_eq!(patch.kiro_remote_media_resolution_enabled, Some(true));
}

#[test]
fn normalize_account_patch_accepts_auto_refresh_toggle() {
    let patch = normalize_account_patch(PatchLlmGatewayAccountRequest {
        status: None,
        route_weight_tier: None,
        proxy_mode: None,
        proxy_config_id: None,
        map_gpt53_codex_to_spark: None,
        auto_refresh_enabled: Some(false),
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        request_max_concurrency_unlimited: false,
        request_min_start_interval_ms_unlimited: false,
    })
    .expect("auto refresh toggle should be accepted");

    assert_eq!(patch.auto_refresh_enabled, Some(false));
}

#[test]
fn effective_kiro_policy_merges_partial_override_with_global_policy() {
    let config = AdminRuntimeConfig::default();
    let keys = vec![sample_kiro_key(Some(
        r#"{"small_input_high_credit_boost":{"target_input_tokens":50000}}"#.to_string(),
    ))];

    let keys = apply_effective_kiro_cache_policies(keys, &config).expect("effective policy merge");
    let policy: serde_json::Value = serde_json::from_str(&keys[0].effective_kiro_cache_policy_json)
        .expect("effective policy json");

    assert_eq!(policy["small_input_high_credit_boost"]["target_input_tokens"], 50_000);
    assert_eq!(policy["small_input_high_credit_boost"]["credit_start"], 1.0);
    assert!(!keys[0].uses_global_kiro_cache_policy);
}

#[test]
fn apply_kiro_candidate_credit_summaries_uses_all_accounts_for_auto_pool() {
    let keys = vec![sample_kiro_key(None)];
    let accounts = vec![
        sample_kiro_account("kiro-a", 800.0, 1_000.0),
        sample_kiro_account("kiro-b", 650.0, 1_000.0),
        sample_kiro_account("kiro-c", 900.0, 1_000.0),
    ];

    let keys = apply_kiro_candidate_credit_summaries(keys, &accounts, &[]);
    let summary = keys[0]
        .kiro_candidate_credit_summary
        .expect("summary should be attached");

    assert_eq!(summary.candidate_count, 3);
    assert_eq!(summary.loaded_balance_count, 3);
    assert_eq!(summary.missing_balance_count, 0);
    assert_eq!(summary.total_limit, 3_000.0);
    assert_eq!(summary.total_remaining, 2_350.0);
}

#[test]
fn apply_kiro_candidate_credit_summaries_respects_account_group_scope() {
    let mut key = sample_kiro_key(None);
    key.account_group_id = Some("group-beta".to_string());
    let accounts = vec![
        sample_kiro_account("kiro-a", 800.0, 1_000.0),
        sample_kiro_account("kiro-b", 650.0, 1_000.0),
        sample_kiro_account("kiro-c", 900.0, 1_000.0),
    ];
    let groups = vec![sample_kiro_group("group-beta", &["kiro-b", "kiro-c"])];

    let keys = apply_kiro_candidate_credit_summaries(vec![key], &accounts, &groups);
    let summary = keys[0]
        .kiro_candidate_credit_summary
        .expect("summary should be attached");

    assert_eq!(summary.candidate_count, 2);
    assert_eq!(summary.loaded_balance_count, 2);
    assert_eq!(summary.total_limit, 2_000.0);
    assert_eq!(summary.total_remaining, 1_550.0);
}

#[test]
fn runtime_config_update_accepts_duckdb_usage_runtime_settings() {
    let updated =
        apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
            duckdb_usage_memory_limit_mib: Some(1024),
            duckdb_usage_checkpoint_threshold_mib: Some(32),
            usage_journal_enabled: Some(false),
            usage_journal_max_file_bytes: Some(128 * 1024 * 1024),
            usage_journal_max_file_age_ms: Some(600_000),
            usage_journal_max_files: Some(64),
            usage_journal_block_target_uncompressed_bytes: Some(2 * 1024 * 1024),
            usage_journal_block_max_events: Some(2048),
            usage_journal_fsync_interval_ms: Some(500),
            usage_journal_zstd_level: Some(5),
            usage_journal_consumer_lease_ms: Some(600_000),
            usage_journal_delete_bad_files: Some(true),
            usage_analytics_retention_days: Some(14),
            usage_query_bind_addr: Some("127.0.0.1:19091".to_string()),
            usage_query_base_url: Some("http://127.0.0.1:19091/".to_string()),
            ..UpdateAdminRuntimeConfig::default()
        })
        .expect("duckdb runtime settings should be valid");

    assert_eq!(updated.duckdb_usage_memory_limit_mib, 1024);
    assert_eq!(updated.duckdb_usage_checkpoint_threshold_mib, 32);
    assert!(!updated.usage_journal_enabled);
    assert_eq!(updated.usage_journal_max_file_bytes, 128 * 1024 * 1024);
    assert_eq!(updated.usage_journal_max_file_age_ms, 600_000);
    assert_eq!(updated.usage_journal_max_files, 64);
    assert_eq!(updated.usage_journal_block_target_uncompressed_bytes, 2 * 1024 * 1024);
    assert_eq!(updated.usage_journal_block_max_events, 2048);
    assert_eq!(updated.usage_journal_fsync_interval_ms, 500);
    assert_eq!(updated.usage_journal_zstd_level, 5);
    assert_eq!(updated.usage_journal_consumer_lease_ms, 600_000);
    assert!(updated.usage_journal_delete_bad_files);
    assert_eq!(updated.usage_analytics_retention_days, 14);
    assert_eq!(updated.usage_query_bind_addr, "127.0.0.1:19091");
    assert_eq!(updated.usage_query_base_url, "http://127.0.0.1:19091");
}

#[test]
fn runtime_config_update_rejects_zero_usage_analytics_retention_days() {
    let err =
        apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
            usage_analytics_retention_days: Some(0),
            ..UpdateAdminRuntimeConfig::default()
        })
        .expect_err("zero retention days should be rejected");

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.message.contains("usage_analytics_retention_days"));
}

#[test]
fn runtime_config_update_accepts_kiro_context_usage_threshold() {
    let updated =
        apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
            kiro_context_usage_min_request_tokens: Some(12_345),
            ..UpdateAdminRuntimeConfig::default()
        })
        .expect("kiro context usage threshold should be valid");

    assert_eq!(updated.kiro_context_usage_min_request_tokens, 12_345);
}

#[test]
fn runtime_config_update_rejects_zero_kiro_context_usage_threshold() {
    let err =
        apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
            kiro_context_usage_min_request_tokens: Some(0),
            ..UpdateAdminRuntimeConfig::default()
        })
        .expect_err("zero threshold should be rejected");

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err
        .message
        .contains("kiro_context_usage_min_request_tokens"));
}

#[test]
fn usage_journal_file_lists_split_current_and_orphan_files() {
    let journal = JournalStatusSnapshot {
        journal_enabled: true,
        journal_root: "/tmp/journal".to_string(),
        active_file_sequence: Some(9),
        active_file_bytes: 4096,
        ..JournalStatusSnapshot::default()
    };
    let files = JournalFileListsSnapshot {
        active: vec![
            JournalFileSnapshot {
                file_name: "usage-000000000007.open".to_string(),
                path: "/tmp/journal/active/usage-000000000007.open".to_string(),
                sequence: Some(7),
                bytes: 1024,
                age_ms: Some(1000),
            },
            JournalFileSnapshot {
                file_name: "usage-000000000009.open".to_string(),
                path: "/tmp/journal/active/usage-000000000009.open".to_string(),
                sequence: Some(9),
                bytes: 4096,
                age_ms: Some(200),
            },
        ],
        consuming: vec![
            JournalFileSnapshot {
                file_name: "usage-000000000003.journal".to_string(),
                path: "/tmp/journal/consuming/usage-000000000003.journal".to_string(),
                sequence: Some(3),
                bytes: 8192,
                age_ms: Some(3000),
            },
            JournalFileSnapshot {
                file_name: "usage-000000000004.journal".to_string(),
                path: "/tmp/journal/consuming/usage-000000000004.journal".to_string(),
                sequence: Some(4),
                bytes: 16384,
                age_ms: Some(4000),
            },
        ],
        ..JournalFileListsSnapshot::default()
    };
    let worker = AdminUsageWorkerProgressView {
        state: "importing".to_string(),
        current_file_path: Some("/tmp/journal/consuming/usage-000000000004.journal".to_string()),
        current_file_sequence: Some(4),
        total_compressed_bytes: 16384,
        ..AdminUsageWorkerProgressView::default()
    };

    let partitioned = partition_usage_journal_files(&journal, &files, &worker);

    assert_eq!(
        partitioned
            .producer_current_file
            .as_ref()
            .and_then(|file| file.sequence),
        Some(9)
    );
    assert_eq!(partitioned.orphan_active_files.len(), 1);
    assert_eq!(partitioned.orphan_active_files[0].sequence, Some(7));
    assert_eq!(
        partitioned
            .current_consuming_file
            .as_ref()
            .and_then(|file| file.sequence),
        Some(4)
    );
    assert_eq!(partitioned.orphan_consuming_files.len(), 1);
    assert_eq!(partitioned.orphan_consuming_files[0].sequence, Some(3));
}

#[test]
fn proxied_usage_list_body_preserves_api_process_activity_counters() {
    let body = br#"{
            "total": 0,
            "offset": 0,
            "limit": 20,
            "has_more": false,
            "current_rpm": 0,
            "current_in_flight": 0,
            "events": [],
            "generated_at": 1700000000000
        }"#;
    let activity = crate::activity::RequestActivitySnapshot {
        rpm: 7,
        in_flight: 2,
    };

    let overlaid =
        overlay_usage_activity_response_body(body, activity).expect("usage list overlay");
    let value: serde_json::Value =
        serde_json::from_slice(&overlaid).expect("overlaid response json");

    assert_eq!(value["current_rpm"], 7);
    assert_eq!(value["current_in_flight"], 2);
}

#[test]
fn usage_activity_key_id_comes_from_query_string() {
    let uri: Uri = "/admin/llm-gateway/usage?limit=20&key_id=key-a"
        .parse()
        .expect("uri");

    assert_eq!(usage_activity_key_id_from_uri(&uri).as_deref(), Some("key-a"));
}

#[test]
fn runtime_config_update_rejects_too_small_duckdb_checkpoint_threshold() {
    let err =
        apply_runtime_config_update(AdminRuntimeConfig::default(), UpdateAdminRuntimeConfig {
            duckdb_usage_checkpoint_threshold_mib: Some(8),
            ..UpdateAdminRuntimeConfig::default()
        })
        .expect_err("checkpoint threshold below 16 MiB should be rejected");

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err
        .message
        .contains("duckdb_usage_checkpoint_threshold_mib"));
}

#[test]
fn admin_codex_accounts_include_cached_rate_limits_and_usage_errors() {
    let accounts = vec![core_store::AdminCodexAccount {
        name: "alpha".to_string(),
        status: "active".to_string(),
        account_id: Some("acct-alpha".to_string()),
        plan_type: None,
        route_weight_tier: "auto".to_string(),
        primary_remaining_percent: None,
        secondary_remaining_percent: None,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        request_max_concurrency: Some(3),
        request_min_start_interval_ms: Some(1000),
        proxy_mode: "inherit".to_string(),
        proxy_config_id: None,
        effective_proxy_source: "binding".to_string(),
        effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
        effective_proxy_config_name: Some("us-home1".to_string()),
        last_refresh: Some(900),
        access_token_expires_at: Some(1_800),
        auth_refresh_error_message: None,
        last_usage_checked_at: None,
        last_usage_success_at: None,
        usage_error_message: None,
    }];
    let status = core_store::CodexRateLimitStatus {
        status: "degraded".to_string(),
        refresh_interval_seconds: 300,
        last_checked_at: Some(1200),
        last_success_at: Some(1100),
        source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
        error_message: None,
        accounts: vec![core_store::CodexPublicAccountStatus {
            name: "alpha".to_string(),
            status: "active".to_string(),
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(62.0),
            secondary_remaining_percent: Some(39.0),
            last_usage_checked_at: Some(1200),
            last_usage_success_at: Some(1100),
            usage_error_message: Some("upstream 503".to_string()),
        }],
        buckets: Vec::new(),
    };

    let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

    assert_eq!(accounts[0].plan_type.as_deref(), Some("Pro"));
    assert_eq!(accounts[0].primary_remaining_percent, Some(62.0));
    assert_eq!(accounts[0].secondary_remaining_percent, Some(39.0));
    assert_eq!(accounts[0].last_refresh, Some(900));
    assert_eq!(accounts[0].last_usage_checked_at, Some(1200));
    assert_eq!(accounts[0].last_usage_success_at, Some(1100));
    assert_eq!(accounts[0].usage_error_message.as_deref(), Some("upstream 503"));
}

#[test]
fn disabled_admin_codex_accounts_do_not_keep_cached_rate_limits() {
    let accounts = vec![core_store::AdminCodexAccount {
        name: "alpha".to_string(),
        status: "disabled".to_string(),
        account_id: Some("acct-alpha".to_string()),
        plan_type: None,
        route_weight_tier: "auto".to_string(),
        primary_remaining_percent: None,
        secondary_remaining_percent: None,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        request_max_concurrency: Some(3),
        request_min_start_interval_ms: Some(1000),
        proxy_mode: "inherit".to_string(),
        proxy_config_id: None,
        effective_proxy_source: "binding".to_string(),
        effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
        effective_proxy_config_name: Some("us-home1".to_string()),
        last_refresh: Some(900),
        access_token_expires_at: Some(1_800),
        auth_refresh_error_message: None,
        last_usage_checked_at: None,
        last_usage_success_at: None,
        usage_error_message: None,
    }];
    let status = core_store::CodexRateLimitStatus {
        status: "ready".to_string(),
        refresh_interval_seconds: 300,
        last_checked_at: Some(1200),
        last_success_at: Some(1100),
        source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
        error_message: None,
        accounts: vec![core_store::CodexPublicAccountStatus {
            name: "alpha".to_string(),
            status: "active".to_string(),
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(62.0),
            secondary_remaining_percent: Some(39.0),
            last_usage_checked_at: Some(1200),
            last_usage_success_at: Some(1100),
            usage_error_message: None,
        }],
        buckets: Vec::new(),
    };

    let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

    assert_eq!(accounts[0].status, "disabled");
    assert_eq!(accounts[0].plan_type, None);
    assert_eq!(accounts[0].primary_remaining_percent, None);
    assert_eq!(accounts[0].secondary_remaining_percent, None);
}

#[test]
fn admin_codex_accounts_keep_newer_local_error_until_status_catches_up() {
    let accounts = vec![core_store::AdminCodexAccount {
        name: "alpha".to_string(),
        status: "active".to_string(),
        account_id: Some("acct-alpha".to_string()),
        plan_type: None,
        route_weight_tier: "auto".to_string(),
        primary_remaining_percent: None,
        secondary_remaining_percent: None,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        request_max_concurrency: Some(3),
        request_min_start_interval_ms: Some(1000),
        proxy_mode: "inherit".to_string(),
        proxy_config_id: None,
        effective_proxy_source: "binding".to_string(),
        effective_proxy_url: Some("http://127.0.0.1:11118".to_string()),
        effective_proxy_config_name: Some("us-home1".to_string()),
        last_refresh: Some(1300),
        access_token_expires_at: Some(1_800),
        auth_refresh_error_message: Some(
            "codex refresh token returned 401 Unauthorized: \
             {\"error\":{\"code\":\"refresh_token_reused\"}}"
                .to_string(),
        ),
        last_usage_checked_at: None,
        last_usage_success_at: None,
        usage_error_message: None,
    }];
    let status = core_store::CodexRateLimitStatus {
        status: "ready".to_string(),
        refresh_interval_seconds: 300,
        last_checked_at: Some(1200),
        last_success_at: Some(1200),
        source_url: "https://chatgpt.com/backend-api/wham/usage".to_string(),
        error_message: None,
        accounts: vec![core_store::CodexPublicAccountStatus {
            name: "alpha".to_string(),
            status: "active".to_string(),
            plan_type: Some("Pro".to_string()),
            primary_remaining_percent: Some(62.0),
            secondary_remaining_percent: Some(39.0),
            last_usage_checked_at: Some(1200),
            last_usage_success_at: Some(1200),
            usage_error_message: None,
        }],
        buckets: Vec::new(),
    };

    let accounts = apply_cached_codex_status_to_admin_accounts(accounts, Some(status));

    assert_eq!(accounts[0].plan_type.as_deref(), Some("Pro"));
    assert_eq!(accounts[0].last_refresh, Some(1300));
    assert_eq!(accounts[0].usage_error_message, None);
    assert_eq!(
        accounts[0].auth_refresh_error_message.as_deref(),
        Some(
            "codex refresh token returned 401 Unauthorized: \
             {\"error\":{\"code\":\"refresh_token_reused\"}}"
        )
    );
}

#[test]
fn imported_codex_auth_accepts_partial_and_preserves_raw_json() {
    let raw = serde_json::json!({
        "tokens": {
            "refreshToken": " refresh-token ",
            "accountId": "acct-1"
        },
        "device_id": "device-1"
    });

    let auth = normalize_imported_codex_auth(Some(raw), None).expect("normalize auth json");
    let stored: serde_json::Value =
        serde_json::from_str(&auth.auth_json).expect("stored auth json");

    assert_eq!(auth.account_id.as_deref(), Some("acct-1"));
    assert_eq!(auth.refresh_token.as_deref(), Some("refresh-token"));
    assert_eq!(auth.id_token, None);
    assert_eq!(auth.access_token, None);
    assert_eq!(stored["device_id"], "device-1");
    assert_eq!(stored["tokens"]["refreshToken"], " refresh-token ");
}

#[test]
fn codex_import_validation_prefers_present_access_token() {
    let auth = normalize_imported_codex_auth(
        Some(serde_json::json!({
            "access_token": "access-token",
            "refresh_token": "refresh-token",
            "account_id": "acct-1"
        })),
        None,
    )
    .expect("normalize auth json");

    assert!(should_validate_codex_access_token_directly(&auth));
}

#[test]
fn codex_access_token_validation_requires_models_payload() {
    validate_codex_models_probe_payload(&serde_json::json!({
        "models": [{"slug": "gpt-5.5"}]
    }))
    .expect("models payload should validate");

    let err = validate_codex_models_probe_payload(&serde_json::json!({"models": []}))
        .expect_err("empty models should not validate");
    assert!(err.to_string().contains("models array"));
}

#[test]
fn codex_batch_import_request_rejects_empty_items() {
    let err = normalize_codex_batch_import_request(CreateCodexBatchImportJobRequest {
        provider_type: "codex".to_string(),
        source_type: "local_json".to_string(),
        validate_before_import: false,
        items: Vec::new(),
    })
    .expect_err("empty items must fail");

    assert_eq!(err.status, StatusCode::BAD_REQUEST);
    assert!(err.message.contains("items"));
}

#[test]
fn codex_batch_import_item_reuses_auth_json_normalization() {
    let normalized = normalize_codex_batch_import_request(CreateCodexBatchImportJobRequest {
        provider_type: "codex".to_string(),
        source_type: "local_json".to_string(),
        validate_before_import: false,
        items: vec![CreateCodexBatchImportJobItemRequest {
            name: "codex_primary".to_string(),
            tokens: None,
            auth_json: Some(serde_json::json!({
                "tokens": {
                    "refreshToken": " refresh-token ",
                    "accountId": "acct-1"
                },
                "device_id": "device-1"
            })),
        }],
    })
    .expect("normalize batch request");

    assert_eq!(normalized.items.len(), 1);
    assert_eq!(normalized.items[0].requested_name, "codex_primary");
    assert_eq!(normalized.items[0].requested_account_id.as_deref(), Some("acct-1"));
    assert!(normalized.items[0].raw_auth_json.contains("device_id"));
}

#[test]
fn account_contribution_issue_email_policy_skips_blank_recipient() {
    let request = sample_account_contribution_request("   ");

    assert_eq!(
        account_contribution_issue_email_policy(&request, false),
        AccountContributionIssueEmailPolicy::SkipNoRecipient
    );
}

#[test]
fn account_contribution_issue_email_policy_skips_when_notifier_missing() {
    let request = sample_account_contribution_request("user@example.com");

    assert_eq!(
        account_contribution_issue_email_policy(&request, false),
        AccountContributionIssueEmailPolicy::SkipNoNotifier
    );
}

#[test]
fn account_contribution_issue_email_policy_sends_when_recipient_and_notifier_exist() {
    let request = sample_account_contribution_request("user@example.com");

    assert_eq!(
        account_contribution_issue_email_policy(&request, true),
        AccountContributionIssueEmailPolicy::Send
    );
}

#[test]
fn account_contribution_access_artifacts_skip_blank_recipient() {
    let request = sample_account_contribution_request("   ");

    assert!(!should_issue_account_contribution_access_artifacts(&request));
}

#[test]
fn account_contribution_access_artifacts_keep_email_backed_issue_flow() {
    let request = sample_account_contribution_request("user@example.com");

    assert!(should_issue_account_contribution_access_artifacts(&request));
}
