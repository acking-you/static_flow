//! One-shot seeding from the current StaticFlow state.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use llm_access_kiro::auth_file::{load_auth_records, KiroAuthRecord};
use rusqlite::{params, Connection, Transaction};
use serde::Serialize;
use serde_json::Value;
use static_flow_shared::llm_gateway_store::{
    Gpt2ApiAccountContributionRequestRecord, LlmGatewayAccountContributionRequestRecord,
    LlmGatewayAccountGroupRecord, LlmGatewayKeyRecord, LlmGatewayProxyBindingRecord,
    LlmGatewayProxyConfigRecord, LlmGatewayRuntimeConfigRecord, LlmGatewaySponsorRequestRecord,
    LlmGatewayStore, LlmGatewayTokenRequestRecord,
};

use crate::config::SeedStaticFlowConfig;

const PAGE_SIZE: usize = 1_000;

#[derive(Debug, Serialize)]
struct SeedStats {
    source_lancedb: String,
    target_sqlite: String,
    target_duckdb: String,
    keys: usize,
    runtime_config: usize,
    account_groups: usize,
    proxy_configs: usize,
    proxy_bindings: usize,
    codex_accounts: usize,
    kiro_accounts: usize,
    token_requests: usize,
    account_contribution_requests: usize,
    gpt2api_account_contribution_requests: usize,
    sponsor_requests: usize,
}

/// Seed standalone llm-access control tables from the current StaticFlow state.
pub(crate) async fn seed_staticflow(config: SeedStaticFlowConfig) -> Result<()> {
    let source = LlmGatewayStore::connect(&config.source_lancedb.display().to_string())
        .await
        .with_context(|| {
            format!("connect source StaticFlow LanceDB `{}`", config.source_lancedb.display())
        })?;

    let keys = source.list_keys().await.context("load source keys")?;
    let runtime_config = source
        .get_runtime_config_or_default()
        .await
        .context("load source runtime config")?;
    let account_groups = source
        .list_account_groups()
        .await
        .context("load source account groups")?;
    let proxy_configs = source
        .list_proxy_configs()
        .await
        .context("load source proxy configs")?;
    let proxy_bindings = source
        .list_proxy_bindings()
        .await
        .context("load source proxy bindings")?;
    let token_requests = collect_token_requests(&source).await?;
    let account_contribution_requests = collect_account_contribution_requests(&source).await?;
    let gpt2api_account_contribution_requests =
        collect_gpt2api_account_contribution_requests(&source).await?;
    let sponsor_requests = collect_sponsor_requests(&source).await?;
    let codex_accounts = load_codex_accounts(&config.auths_dir)?;
    let kiro_accounts = load_auth_records(&config.auths_dir.join("kiro"))
        .await
        .context("load source Kiro auth files")?;

    let conn = Connection::open(&config.storage.sqlite_control).with_context(|| {
        format!("open target SQLite `{}`", config.storage.sqlite_control.display())
    })?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .context("enable SQLite foreign keys")?;
    let tx = conn
        .unchecked_transaction()
        .context("begin seed transaction")?;
    clear_seed_tables(&tx)?;
    upsert_runtime_config(&tx, &runtime_config)?;
    for group in &account_groups {
        upsert_account_group(&tx, group)?;
    }
    for proxy in &proxy_configs {
        upsert_proxy_config(&tx, proxy)?;
    }
    for binding in &proxy_bindings {
        upsert_proxy_binding(&tx, binding)?;
    }
    for key in &keys {
        upsert_key(&tx, key)?;
    }
    for account in &codex_accounts {
        upsert_codex_account(&tx, account)?;
    }
    for account in &kiro_accounts {
        upsert_kiro_account(&tx, account)?;
    }
    for request in &token_requests {
        upsert_token_request(&tx, request)?;
    }
    for request in &account_contribution_requests {
        upsert_account_contribution_request(&tx, request)?;
    }
    for request in &gpt2api_account_contribution_requests {
        upsert_gpt2api_account_contribution_request(&tx, request)?;
    }
    for request in &sponsor_requests {
        upsert_sponsor_request(&tx, request)?;
    }
    tx.commit().context("commit seed transaction")?;

    let stats = SeedStats {
        source_lancedb: config.source_lancedb.display().to_string(),
        target_sqlite: config.storage.sqlite_control.display().to_string(),
        target_duckdb: config.storage.duckdb.display().to_string(),
        keys: keys.len(),
        runtime_config: 1,
        account_groups: account_groups.len(),
        proxy_configs: proxy_configs.len(),
        proxy_bindings: proxy_bindings.len(),
        codex_accounts: codex_accounts.len(),
        kiro_accounts: kiro_accounts.len(),
        token_requests: token_requests.len(),
        account_contribution_requests: account_contribution_requests.len(),
        gpt2api_account_contribution_requests: gpt2api_account_contribution_requests.len(),
        sponsor_requests: sponsor_requests.len(),
    };
    println!("{}", serde_json::to_string_pretty(&stats)?);
    Ok(())
}

async fn collect_token_requests(
    source: &LlmGatewayStore,
) -> Result<Vec<LlmGatewayTokenRequestRecord>> {
    let total = source.count_token_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    for offset in (0..total).step_by(PAGE_SIZE) {
        rows.extend(source.query_token_requests(None, PAGE_SIZE, offset).await?);
    }
    Ok(rows)
}

async fn collect_account_contribution_requests(
    source: &LlmGatewayStore,
) -> Result<Vec<LlmGatewayAccountContributionRequestRecord>> {
    let total = source.count_account_contribution_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    for offset in (0..total).step_by(PAGE_SIZE) {
        rows.extend(
            source
                .query_account_contribution_requests(None, PAGE_SIZE, offset)
                .await?,
        );
    }
    Ok(rows)
}

async fn collect_gpt2api_account_contribution_requests(
    source: &LlmGatewayStore,
) -> Result<Vec<Gpt2ApiAccountContributionRequestRecord>> {
    let total = source
        .count_gpt2api_account_contribution_requests(None)
        .await?;
    let mut rows = Vec::with_capacity(total);
    for offset in (0..total).step_by(PAGE_SIZE) {
        rows.extend(
            source
                .query_gpt2api_account_contribution_requests(None, PAGE_SIZE, offset)
                .await?,
        );
    }
    Ok(rows)
}

async fn collect_sponsor_requests(
    source: &LlmGatewayStore,
) -> Result<Vec<LlmGatewaySponsorRequestRecord>> {
    let total = source.count_sponsor_requests(None).await?;
    let mut rows = Vec::with_capacity(total);
    for offset in (0..total).step_by(PAGE_SIZE) {
        rows.extend(
            source
                .query_sponsor_requests(None, PAGE_SIZE, offset)
                .await?,
        );
    }
    Ok(rows)
}

#[derive(Debug)]
struct CodexSeedAccount {
    name: String,
    account_id: Option<String>,
    auth_json: String,
    settings_json: String,
    last_refresh_at_ms: Option<i64>,
    created_at_ms: i64,
}

fn load_codex_accounts(auths_dir: &Path) -> Result<Vec<CodexSeedAccount>> {
    if !auths_dir.exists() {
        return Ok(Vec::new());
    }
    let mut rows = Vec::new();
    for entry in fs::read_dir(auths_dir)
        .with_context(|| format!("read auths dir `{}`", auths_dir.display()))?
    {
        let entry = entry.with_context(|| format!("read entry in `{}`", auths_dir.display()))?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        let auth_json =
            fs::read_to_string(&path).with_context(|| format!("read `{}`", path.display()))?;
        let auth_value: Value = serde_json::from_str(&auth_json)
            .with_context(|| format!("parse `{}`", path.display()))?;
        let settings_json = load_codex_settings(&path)?;
        let created_at_ms = file_modified_at_ms(&path).unwrap_or_else(now_ms);
        rows.push(CodexSeedAccount {
            name: name.to_string(),
            account_id: optional_json_string_nested(&auth_value, &[
                &["account_id", "accountId"],
                &["tokens", "account_id"],
                &["tokens", "accountId"],
            ]),
            auth_json,
            settings_json,
            last_refresh_at_ms: parse_codex_last_refresh_ms(&auth_value).or(Some(created_at_ms)),
            created_at_ms,
        });
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(rows)
}

fn load_codex_settings(auth_path: &Path) -> Result<String> {
    let meta_path = auth_path.with_extension("meta");
    if meta_path.exists() {
        let raw = fs::read_to_string(&meta_path)
            .with_context(|| format!("read `{}`", meta_path.display()))?;
        serde_json::from_str::<Value>(&raw)
            .with_context(|| format!("parse `{}`", meta_path.display()))?;
        return Ok(raw);
    }
    Ok(serde_json::json!({
        "map_gpt53_codex_to_spark": false,
        "proxy_mode": "inherit",
        "proxy_config_id": null,
        "request_max_concurrency": null,
        "request_min_start_interval_ms": null
    })
    .to_string())
}

fn clear_seed_tables(tx: &Transaction<'_>) -> Result<()> {
    for table in [
        "llm_kiro_status_cache",
        "llm_codex_status_cache",
        "llm_key_usage_rollups",
        "llm_key_route_config",
        "llm_keys",
        "llm_runtime_config",
        "llm_proxy_bindings",
        "llm_proxy_configs",
        "llm_account_groups",
        "llm_codex_accounts",
        "llm_kiro_accounts",
        "llm_token_requests",
        "llm_account_contribution_requests",
        "gpt2api_account_contribution_requests",
        "llm_sponsor_requests",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])
            .with_context(|| format!("clear target table `{table}`"))?;
    }
    Ok(())
}

fn upsert_key(tx: &Transaction<'_>, key: &LlmGatewayKeyRecord) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_keys (
            key_id, name, secret, key_hash, status, provider_type, protocol_family,
            public_visible, quota_billable_limit, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            &key.id,
            &key.name,
            &key.secret,
            &key.key_hash,
            &key.status,
            &key.provider_type,
            &key.protocol_family,
            key.public_visible as i64,
            key.quota_billable_limit as i64,
            key.created_at,
            key.updated_at,
        ],
    )?;
    tx.execute(
        "INSERT INTO llm_key_route_config (
            key_id, route_strategy, fixed_account_name, auto_account_names_json,
            account_group_id, model_name_map_json, request_max_concurrency,
            request_min_start_interval_ms, kiro_request_validation_enabled,
            kiro_cache_estimation_enabled, kiro_zero_cache_debug_enabled,
            kiro_cache_policy_override_json,
            kiro_billable_model_multipliers_override_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            &key.id,
            &key.route_strategy,
            &key.fixed_account_name,
            optional_json(&key.auto_account_names)?,
            &key.account_group_id,
            optional_json(&key.model_name_map)?,
            key.request_max_concurrency.map(|value| value as i64),
            key.request_min_start_interval_ms.map(|value| value as i64),
            key.kiro_request_validation_enabled as i64,
            key.kiro_cache_estimation_enabled as i64,
            key.kiro_zero_cache_debug_enabled as i64,
            &key.kiro_cache_policy_override_json,
            &key.kiro_billable_model_multipliers_override_json,
        ],
    )?;
    tx.execute(
        "INSERT INTO llm_key_usage_rollups (
            key_id, input_uncached_tokens, input_cached_tokens, output_tokens,
            billable_tokens, credit_total, credit_missing_events, last_used_at_ms,
            updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            &key.id,
            key.usage_input_uncached_tokens as i64,
            key.usage_input_cached_tokens as i64,
            key.usage_output_tokens as i64,
            key.usage_billable_tokens as i64,
            key.usage_credit_total.to_string(),
            key.usage_credit_missing_events as i64,
            key.last_used_at,
            key.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_runtime_config(
    tx: &Transaction<'_>,
    record: &LlmGatewayRuntimeConfigRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_runtime_config (
            id, auth_cache_ttl_seconds, max_request_body_bytes,
            account_failure_retry_limit, codex_client_version,
            kiro_channel_max_concurrency, kiro_channel_min_start_interval_ms,
            codex_status_refresh_min_interval_seconds,
            codex_status_refresh_max_interval_seconds,
            codex_status_account_jitter_max_seconds,
            kiro_status_refresh_min_interval_seconds,
            kiro_status_refresh_max_interval_seconds,
            kiro_status_account_jitter_max_seconds,
            usage_event_flush_batch_size, usage_event_flush_interval_seconds,
            usage_event_flush_max_buffer_bytes, usage_event_maintenance_enabled,
            usage_event_maintenance_interval_seconds, usage_event_detail_retention_days,
            kiro_cache_kmodels_json, kiro_billable_model_multipliers_json,
            kiro_cache_policy_json, kiro_prefix_cache_mode, kiro_prefix_cache_max_tokens,
            kiro_prefix_cache_entry_ttl_seconds, kiro_conversation_anchor_max_entries,
            kiro_conversation_anchor_ttl_seconds, updated_at_ms
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26,
            ?27, ?28
        )",
        params![
            &record.id,
            record.auth_cache_ttl_seconds as i64,
            record.max_request_body_bytes as i64,
            record.account_failure_retry_limit as i64,
            &record.codex_client_version,
            record.kiro_channel_max_concurrency as i64,
            record.kiro_channel_min_start_interval_ms as i64,
            record.codex_status_refresh_min_interval_seconds as i64,
            record.codex_status_refresh_max_interval_seconds as i64,
            record.codex_status_account_jitter_max_seconds as i64,
            record.kiro_status_refresh_min_interval_seconds as i64,
            record.kiro_status_refresh_max_interval_seconds as i64,
            record.kiro_status_account_jitter_max_seconds as i64,
            record.usage_event_flush_batch_size as i64,
            record.usage_event_flush_interval_seconds as i64,
            record.usage_event_flush_max_buffer_bytes as i64,
            record.usage_event_maintenance_enabled as i64,
            record.usage_event_maintenance_interval_seconds as i64,
            record.usage_event_detail_retention_days,
            &record.kiro_cache_kmodels_json,
            &record.kiro_billable_model_multipliers_json,
            &record.kiro_cache_policy_json,
            &record.kiro_prefix_cache_mode,
            record.kiro_prefix_cache_max_tokens as i64,
            record.kiro_prefix_cache_entry_ttl_seconds as i64,
            record.kiro_conversation_anchor_max_entries as i64,
            record.kiro_conversation_anchor_ttl_seconds as i64,
            record.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_account_group(tx: &Transaction<'_>, group: &LlmGatewayAccountGroupRecord) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_account_groups (
            group_id, provider_type, name, account_names_json, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            &group.id,
            &group.provider_type,
            &group.name,
            serde_json::to_string(&group.account_names)?,
            group.created_at,
            group.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_proxy_config(tx: &Transaction<'_>, proxy: &LlmGatewayProxyConfigRecord) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_proxy_configs (
            proxy_config_id, name, proxy_url, proxy_username, proxy_password, status,
            created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            &proxy.id,
            &proxy.name,
            &proxy.proxy_url,
            &proxy.proxy_username,
            &proxy.proxy_password,
            &proxy.status,
            proxy.created_at,
            proxy.updated_at,
        ],
    )?;
    Ok(())
}

fn upsert_proxy_binding(
    tx: &Transaction<'_>,
    binding: &LlmGatewayProxyBindingRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_proxy_bindings (provider_type, proxy_config_id, updated_at_ms)
         VALUES (?1, ?2, ?3)",
        params![&binding.provider_type, &binding.proxy_config_id, binding.updated_at],
    )?;
    Ok(())
}

fn upsert_codex_account(tx: &Transaction<'_>, account: &CodexSeedAccount) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_codex_accounts (
            account_name, account_id, email, status, auth_json, settings_json,
            last_refresh_at_ms, last_error, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, NULL, 'active', ?3, ?4, ?5, NULL, ?6, ?6)",
        params![
            &account.name,
            &account.account_id,
            &account.auth_json,
            &account.settings_json,
            account.last_refresh_at_ms,
            account.created_at_ms,
        ],
    )?;
    Ok(())
}

fn upsert_kiro_account(tx: &Transaction<'_>, account: &KiroAuthRecord) -> Result<()> {
    let auth_json = serde_json::to_string(account).context("serialize kiro auth")?;
    let created_at_ms = account.last_imported_at.unwrap_or_else(now_ms);
    tx.execute(
        "INSERT INTO llm_kiro_accounts (
            account_name, auth_method, account_id, profile_arn, user_id, status, auth_json,
            max_concurrency, min_start_interval_ms, proxy_config_id, last_refresh_at_ms,
            last_error, created_at_ms, updated_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?12)",
        params![
            &account.name,
            account.auth_method(),
            optional_json_string(&serde_json::from_str::<Value>(&auth_json)?, &[
                "accountId",
                "account_id"
            ]),
            &account.profile_arn,
            optional_json_string(&serde_json::from_str::<Value>(&auth_json)?, &[
                "userId", "user_id"
            ]),
            if account.disabled { "disabled" } else { "active" },
            auth_json,
            account
                .kiro_channel_max_concurrency
                .map(|value| value as i64),
            account
                .kiro_channel_min_start_interval_ms
                .map(|value| value as i64),
            &account.proxy_config_id,
            Some(created_at_ms),
            created_at_ms,
        ],
    )?;
    Ok(())
}

fn upsert_token_request(
    tx: &Transaction<'_>,
    request: &LlmGatewayTokenRequestRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_token_requests (
            request_id, requester_email, requested_quota_billable_limit, request_reason,
            frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
            failure_reason, issued_key_id, issued_key_name, created_at_ms, updated_at_ms,
            processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            &request.request_id,
            &request.requester_email,
            request.requested_quota_billable_limit as i64,
            &request.request_reason,
            &request.frontend_page_url,
            &request.status,
            &request.fingerprint,
            &request.client_ip,
            &request.ip_region,
            &request.admin_note,
            &request.failure_reason,
            &request.issued_key_id,
            &request.issued_key_name,
            request.created_at,
            request.updated_at,
            request.processed_at,
        ],
    )?;
    Ok(())
}

fn upsert_account_contribution_request(
    tx: &Transaction<'_>,
    request: &LlmGatewayAccountContributionRequestRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_account_contribution_requests (
            request_id, account_name, account_id, id_token, access_token, refresh_token,
            requester_email, contributor_message, github_id, frontend_page_url, status,
            fingerprint, client_ip, ip_region, admin_note, failure_reason,
            imported_account_name, issued_key_id, issued_key_name, created_at_ms,
            updated_at_ms, processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20, ?21, ?22)",
        params![
            &request.request_id,
            &request.account_name,
            &request.account_id,
            &request.id_token,
            &request.access_token,
            &request.refresh_token,
            &request.requester_email,
            &request.contributor_message,
            &request.github_id,
            &request.frontend_page_url,
            &request.status,
            &request.fingerprint,
            &request.client_ip,
            &request.ip_region,
            &request.admin_note,
            &request.failure_reason,
            &request.imported_account_name,
            &request.issued_key_id,
            &request.issued_key_name,
            request.created_at,
            request.updated_at,
            request.processed_at,
        ],
    )?;
    Ok(())
}

fn upsert_gpt2api_account_contribution_request(
    tx: &Transaction<'_>,
    request: &Gpt2ApiAccountContributionRequestRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO gpt2api_account_contribution_requests (
            request_id, account_name, access_token, session_json, requester_email,
            contributor_message, github_id, frontend_page_url, status, fingerprint, client_ip,
            ip_region, admin_note, failure_reason, imported_account_name, issued_key_id,
            issued_key_name, created_at_ms, updated_at_ms, processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, \
         ?19, ?20)",
        params![
            &request.request_id,
            &request.account_name,
            &request.access_token,
            &request.session_json,
            &request.requester_email,
            &request.contributor_message,
            &request.github_id,
            &request.frontend_page_url,
            &request.status,
            &request.fingerprint,
            &request.client_ip,
            &request.ip_region,
            &request.admin_note,
            &request.failure_reason,
            &request.imported_account_name,
            &request.issued_key_id,
            &request.issued_key_name,
            request.created_at,
            request.updated_at,
            request.processed_at,
        ],
    )?;
    Ok(())
}

fn upsert_sponsor_request(
    tx: &Transaction<'_>,
    request: &LlmGatewaySponsorRequestRecord,
) -> Result<()> {
    tx.execute(
        "INSERT INTO llm_sponsor_requests (
            request_id, requester_email, sponsor_message, display_name, github_id,
            frontend_page_url, status, fingerprint, client_ip, ip_region, admin_note,
            failure_reason, payment_email_sent_at_ms, created_at_ms, updated_at_ms,
            processed_at_ms
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        params![
            &request.request_id,
            &request.requester_email,
            &request.sponsor_message,
            &request.display_name,
            &request.github_id,
            &request.frontend_page_url,
            &request.status,
            &request.fingerprint,
            &request.client_ip,
            &request.ip_region,
            &request.admin_note,
            &request.failure_reason,
            request.payment_email_sent_at,
            request.created_at,
            request.updated_at,
            request.processed_at,
        ],
    )?;
    Ok(())
}

fn optional_json<T: Serialize>(value: &Option<T>) -> Result<Option<String>> {
    value
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .context("serialize optional json")
}

fn parse_codex_last_refresh_ms(value: &Value) -> Option<i64> {
    let raw = value.get("last_refresh")?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|time| time.with_timezone(&Utc).timestamp_millis())
}

fn optional_json_string(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    })
}

fn optional_json_string_nested(value: &Value, paths: &[&[&str]]) -> Option<String> {
    paths.iter().find_map(|path| {
        let mut current = value;
        for key in *path {
            current = current.get(*key)?;
        }
        current.as_str().map(ToOwned::to_owned)
    })
}

fn file_modified_at_ms(path: &Path) -> Option<i64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}
