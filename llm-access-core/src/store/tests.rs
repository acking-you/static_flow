use std::collections::BTreeMap;

fn sample_admin_codex_account(
    name: &str,
    status: &str,
    plan_type: Option<&str>,
    primary_remaining_percent: Option<f64>,
    auth_refresh_error_message: Option<&str>,
) -> super::AdminCodexAccount {
    super::AdminCodexAccount {
        name: name.to_string(),
        status: status.to_string(),
        account_id: Some(format!("acct-{name}")),
        plan_type: plan_type.map(str::to_string),
        route_weight_tier: "auto".to_string(),
        primary_remaining_percent,
        secondary_remaining_percent: None,
        map_gpt53_codex_to_spark: false,
        auto_refresh_enabled: true,
        request_max_concurrency: None,
        request_min_start_interval_ms: None,
        proxy_mode: "inherit".to_string(),
        proxy_config_id: None,
        effective_proxy_source: "binding".to_string(),
        effective_proxy_url: None,
        effective_proxy_config_name: None,
        last_refresh: None,
        access_token_expires_at: None,
        auth_refresh_error_message: auth_refresh_error_message.map(str::to_string),
        last_usage_checked_at: None,
        last_usage_success_at: None,
        usage_error_message: None,
    }
}

#[test]
fn compute_kiro_billable_tokens_applies_family_multiplier() {
    let multipliers = BTreeMap::from([
        ("opus".to_string(), 2.0),
        ("sonnet".to_string(), 1.0),
        ("haiku".to_string(), 1.0),
    ]);

    let base = super::compute_billable_tokens(100, 20, 5);
    let adjusted =
        super::compute_kiro_billable_tokens(Some("claude-opus-4-6"), 100, 20, 5, &multipliers);

    assert_eq!(adjusted, base * 2);
}

#[test]
fn compute_kiro_billable_tokens_defaults_for_unknown_models() {
    let multipliers = BTreeMap::from([
        ("opus".to_string(), 2.0),
        ("sonnet".to_string(), 3.0),
        ("haiku".to_string(), 0.5),
    ]);

    let base = super::compute_billable_tokens(80, 10, 4);
    let adjusted =
        super::compute_kiro_billable_tokens(Some("claude-unknown-1"), 80, 10, 4, &multipliers);

    assert_eq!(adjusted, base);
}

#[test]
fn admin_runtime_config_uses_tightened_kiro_cache_defaults() {
    let config = super::AdminRuntimeConfig::default();

    assert_eq!(config.kiro_prefix_cache_max_tokens, 1_000_000);
    assert_eq!(config.kiro_prefix_cache_entry_ttl_seconds, 2 * 60 * 60);
    assert_eq!(config.kiro_conversation_anchor_max_entries, 4_096);
    assert_eq!(config.kiro_conversation_anchor_ttl_seconds, 6 * 60 * 60);
    assert_eq!(config.kiro_context_usage_min_request_tokens, 15_000);
}

#[test]
fn admin_codex_account_query_supports_search_sort_and_unhealthy_filters() {
    let mut accounts = vec![
        sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
        sample_admin_codex_account("codex-mid", "disabled", None, None, None),
        sample_admin_codex_account(
            "codex-old",
            "active",
            Some("Plus"),
            Some(20.0),
            Some("refresh failed"),
        ),
    ];

    super::apply_admin_codex_account_query(&mut accounts, &super::AdminCodexAccountPageQuery {
        sort: super::AdminCodexAccountSortMode::PrimaryAsc,
        ..super::AdminCodexAccountPageQuery::default()
    });
    assert_eq!(
        accounts
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>(),
        ["codex-old", "codex-new", "codex-mid"]
    );

    let mut searched = vec![
        sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
        sample_admin_codex_account(
            "codex-old",
            "active",
            Some("Plus"),
            Some(20.0),
            Some("refresh failed"),
        ),
    ];
    super::apply_admin_codex_account_query(&mut searched, &super::AdminCodexAccountPageQuery {
        search: Some("plus".to_string()),
        ..super::AdminCodexAccountPageQuery::default()
    });
    assert_eq!(searched.len(), 1);
    assert_eq!(searched[0].name, "codex-old");

    let mut unhealthy = vec![
        sample_admin_codex_account("codex-new", "active", Some("Pro"), Some(70.0), None),
        sample_admin_codex_account("codex-mid", "disabled", None, None, None),
        sample_admin_codex_account(
            "codex-old",
            "active",
            Some("Plus"),
            Some(20.0),
            Some("refresh failed"),
        ),
    ];
    super::apply_admin_codex_account_query(&mut unhealthy, &super::AdminCodexAccountPageQuery {
        unhealthy_only: true,
        ..super::AdminCodexAccountPageQuery::default()
    });
    assert_eq!(
        unhealthy
            .iter()
            .map(|account| account.name.as_str())
            .collect::<Vec<_>>(),
        ["codex-mid", "codex-old"]
    );
}
