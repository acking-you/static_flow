//! Postgres row -> record/admin-view decoders. Pure functions over `PgRow`
//! and the parent module's row structs.

use anyhow::Context;
use llm_access_core::store::{
    self as core_store, AdminAccountContributionRequest, AdminAccountGroup,
    AdminCodexImportJobItem, AdminCodexImportJobSummary, AdminKey,
    AdminKiroKeyCandidateCreditSummary, AdminProxyConfig, AdminProxyEndpointCheck,
    AdminSponsorRequest, AdminTokenRequest, PublicUsageLookupKey,
};

use super::{
    json::{decode_optional_json, non_negative_i64_to_u64},
    CodexAccountSettings, CodexAdminAccountListRow, KiroAdminAccountListRow, PgRow,
    ProxyEndpointCheckRow,
};
use crate::records::{
    CodexAccountRecord, KeyBundle, KeyRecord, KeyRouteConfig, KeyUsageRollup, KiroAccountRecord,
    RuntimeConfigRecord,
};

pub fn decode_runtime_config_row(row: PgRow) -> anyhow::Result<RuntimeConfigRecord> {
    Ok(RuntimeConfigRecord {
        id: row.get(0),
        auth_cache_ttl_seconds: row.get(1),
        max_request_body_bytes: row.get(2),
        account_failure_retry_limit: row.get(3),
        codex_client_version: row.get(4),
        kiro_channel_max_concurrency: row.get(5),
        kiro_channel_min_start_interval_ms: row.get(6),
        codex_status_refresh_min_interval_seconds: row.get(7),
        codex_status_refresh_max_interval_seconds: row.get(8),
        codex_status_account_jitter_max_seconds: row.get(9),
        codex_weight_free: row.get(10),
        codex_weight_plus: row.get(11),
        codex_weight_pro5x: row.get(12),
        codex_weight_pro20x: row.get(13),
        kiro_status_refresh_min_interval_seconds: row.get(14),
        kiro_status_refresh_max_interval_seconds: row.get(15),
        kiro_status_account_jitter_max_seconds: row.get(16),
        usage_event_flush_batch_size: row.get(17),
        usage_event_flush_interval_seconds: row.get(18),
        usage_event_flush_max_buffer_bytes: row.get(19),
        duckdb_usage_memory_limit_mib: row.get(20),
        duckdb_usage_checkpoint_threshold_mib: row.get(21),
        usage_analytics_retention_days: row.get(22),
        usage_journal_enabled: row.get(23),
        usage_journal_max_file_bytes: row.get(24),
        usage_journal_max_file_age_ms: row.get(25),
        usage_journal_max_files: row.get(26),
        usage_journal_block_target_uncompressed_bytes: row.get(27),
        usage_journal_block_max_events: row.get(28),
        usage_journal_fsync_interval_ms: row.get(29),
        usage_journal_zstd_level: row.get(30),
        usage_journal_consumer_lease_ms: row.get(31),
        usage_journal_delete_bad_files: row.get::<_, i64>(32) != 0,
        usage_query_bind_addr: row.get(33),
        usage_query_base_url: row.get(34),
        usage_event_maintenance_enabled: row.get(35),
        usage_event_maintenance_interval_seconds: row.get(36),
        usage_event_detail_retention_days: row.get(37),
        kiro_cache_kmodels_json: row.get(38),
        kiro_billable_model_multipliers_json: row.get(39),
        kiro_cache_policy_json: row.get(40),
        kiro_context_usage_min_request_tokens: row.get(41),
        kiro_compact_trigger_tokens: row.get(42),
        kiro_prefix_cache_mode: row.get(43),
        kiro_prefix_cache_max_tokens: row.get(44),
        kiro_prefix_cache_entry_ttl_seconds: row.get(45),
        kiro_conversation_anchor_max_entries: row.get(46),
        kiro_conversation_anchor_ttl_seconds: row.get(47),
        kiro_cctest_proxy_base_url: row.get(48),
        kiro_cctest_proxy_api_key: row.get(49),
        codex_session_affinity_enabled: row.get(50),
        codex_session_affinity_max_entries: row.get(51),
        codex_session_affinity_ttl_seconds: row.get(52),
        codex_fallback_affinity_enabled: row.get(53),
        codex_fallback_affinity_ttl_seconds: row.get(54),
        codex_fallback_affinity_prefix_bytes: row.get(55),
        codex_fallback_affinity_min_body_bytes: row.get(56),
        updated_at_ms: row.get(57),
        kiro_cache_snapshot_enabled: row.get(58),
        kiro_cache_snapshot_interval_seconds: row.get(59),
        kiro_cache_snapshot_ttl_seconds: row.get(60),
        kiro_cache_snapshot_max_tokens: row.get(61),
        kiro_cache_snapshot_max_anchor_entries: row.get(62),
    })
}

fn decode_key_bundle(row: &PgRow) -> anyhow::Result<KeyBundle> {
    let key_id: String = row.get(0);
    let credit_total_raw: String = row.get(30);
    let credit_total = credit_total_raw
        .parse::<f64>()
        .with_context(|| format!("parse key rollup credit_total `{credit_total_raw}`"))?;
    Ok(KeyBundle {
        key: KeyRecord {
            key_id: key_id.clone(),
            name: row.get(1),
            secret: row.get(2),
            key_hash: row.get(3),
            status: row.get(4),
            provider_type: row.get(5),
            protocol_family: row.get(6),
            public_visible: row.get(7),
            quota_billable_limit: row.get(8),
            created_at_ms: row.get(9),
            updated_at_ms: row.get(10),
        },
        route: KeyRouteConfig {
            key_id: key_id.clone(),
            route_strategy: row.get(11),
            fixed_account_name: row.get(12),
            auto_account_names_json: row.get(13),
            account_group_id: row.get(14),
            model_name_map_json: row.get(15),
            request_max_concurrency: row.get(16),
            request_min_start_interval_ms: row.get(17),
            codex_fast_enabled: row.get::<_, Option<bool>>(18).unwrap_or(true),
            kiro_request_validation_enabled: row.get::<_, Option<bool>>(19).unwrap_or(false),
            kiro_cache_estimation_enabled: row.get::<_, Option<bool>>(20).unwrap_or(false),
            kiro_zero_cache_debug_enabled: row.get::<_, Option<bool>>(21).unwrap_or(false),
            kiro_full_request_logging_enabled: row.get::<_, Option<bool>>(22).unwrap_or(false),
            kiro_remote_media_resolution_enabled: row.get::<_, Option<bool>>(23).unwrap_or(false),
            kiro_latency_routing_enabled: row
                .get_optional_bool("kiro_latency_routing_enabled")
                .unwrap_or(true),
            kiro_protected_content_validation_enabled: row
                .get_optional_bool("kiro_protected_content_validation_enabled")
                .unwrap_or(false),
            kiro_cctest_text_handling_enabled: row
                .get_optional_bool("kiro_cctest_text_handling_enabled")
                .unwrap_or(false),
            kiro_cache_policy_override_json: row.get(24),
            kiro_billable_model_multipliers_override_json: row.get(25),
        },
        rollup: KeyUsageRollup {
            key_id,
            input_uncached_tokens: row.get(26),
            input_cached_tokens: row.get(27),
            output_tokens: row.get(28),
            billable_tokens: row.get(29),
            credit_total,
            credit_missing_events: row.get(31),
            last_used_at_ms: row.get(32),
            updated_at_ms: row.get(33),
        },
    })
}

pub fn decode_key_bundle_row(row: PgRow) -> anyhow::Result<KeyBundle> {
    decode_key_bundle(&row)
}

pub fn admin_key_from_bundle(bundle: &KeyBundle) -> AdminKey {
    let quota = bundle.key.quota_billable_limit.max(0) as u64;
    let billable = bundle.rollup.billable_tokens.max(0) as u64;
    AdminKey {
        id: bundle.key.key_id.clone(),
        name: bundle.key.name.clone(),
        secret: bundle.key.secret.clone(),
        key_hash: bundle.key.key_hash.clone(),
        status: bundle.key.status.clone(),
        provider_type: bundle.key.provider_type.clone(),
        public_visible: bundle.key.public_visible,
        quota_billable_limit: quota,
        usage_input_uncached_tokens: bundle.rollup.input_uncached_tokens.max(0) as u64,
        usage_input_cached_tokens: bundle.rollup.input_cached_tokens.max(0) as u64,
        usage_output_tokens: bundle.rollup.output_tokens.max(0) as u64,
        usage_credit_total: bundle.rollup.credit_total,
        usage_credit_missing_events: bundle.rollup.credit_missing_events.max(0) as u64,
        remaining_billable: (quota as i64).saturating_sub(billable as i64),
        last_used_at: bundle.rollup.last_used_at_ms,
        created_at: bundle.key.created_at_ms,
        updated_at: bundle.key.updated_at_ms,
        route_strategy: bundle.route.route_strategy.clone(),
        account_group_id: bundle.route.account_group_id.clone(),
        fixed_account_name: bundle.route.fixed_account_name.clone(),
        auto_account_names: decode_optional_json(bundle.route.auto_account_names_json.as_deref()),
        model_name_map: decode_optional_json(bundle.route.model_name_map_json.as_deref()),
        request_max_concurrency: bundle
            .route
            .request_max_concurrency
            .and_then(non_negative_i64_to_u64),
        request_min_start_interval_ms: bundle
            .route
            .request_min_start_interval_ms
            .and_then(non_negative_i64_to_u64),
        codex_fast_enabled: bundle.route.codex_fast_enabled,
        kiro_request_validation_enabled: bundle.route.kiro_request_validation_enabled,
        kiro_cache_estimation_enabled: bundle.route.kiro_cache_estimation_enabled,
        kiro_zero_cache_debug_enabled: bundle.route.kiro_zero_cache_debug_enabled,
        kiro_full_request_logging_enabled: bundle.route.kiro_full_request_logging_enabled,
        kiro_remote_media_resolution_enabled: bundle.route.kiro_remote_media_resolution_enabled,
        kiro_latency_routing_enabled: bundle.route.kiro_latency_routing_enabled,
        kiro_protected_content_validation_enabled: bundle
            .route
            .kiro_protected_content_validation_enabled,
        kiro_cctest_text_handling_enabled: bundle.route.kiro_cctest_text_handling_enabled,
        kiro_cache_policy_override_json: bundle.route.kiro_cache_policy_override_json.clone(),
        kiro_billable_model_multipliers_override_json: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .clone(),
        effective_kiro_cache_policy_json: bundle
            .route
            .kiro_cache_policy_override_json
            .clone()
            .unwrap_or_else(core_store::default_kiro_cache_policy_json),
        uses_global_kiro_cache_policy: bundle.route.kiro_cache_policy_override_json.is_none(),
        effective_kiro_billable_model_multipliers_json: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .clone()
            .unwrap_or_else(core_store::default_kiro_billable_model_multipliers_json),
        uses_global_kiro_billable_model_multipliers: bundle
            .route
            .kiro_billable_model_multipliers_override_json
            .is_none(),
        kiro_candidate_credit_summary: None,
    }
}

fn decode_kiro_candidate_credit_summary_row(
    row: &PgRow,
    offset: usize,
) -> AdminKiroKeyCandidateCreditSummary {
    AdminKiroKeyCandidateCreditSummary {
        candidate_count: row.get::<_, i64>(offset).max(0) as usize,
        loaded_balance_count: row.get::<_, i64>(offset + 1).max(0) as usize,
        missing_balance_count: row.get::<_, i64>(offset + 2).max(0) as usize,
        total_limit: row.get(offset + 3),
        total_remaining: row.get(offset + 4),
    }
}

pub fn decode_kiro_admin_key_row(row: PgRow) -> anyhow::Result<AdminKey> {
    let bundle = decode_key_bundle(&row)?;
    let mut key = admin_key_from_bundle(&bundle);
    key.kiro_candidate_credit_summary = Some(decode_kiro_candidate_credit_summary_row(&row, 34));
    Ok(key)
}

pub fn decode_admin_account_group_row(row: PgRow) -> anyhow::Result<AdminAccountGroup> {
    let account_names_json: String = row.get(3);
    let account_names = serde_json::from_str::<Vec<String>>(&account_names_json)
        .with_context(|| format!("decode account_names_json `{account_names_json}`"))?;
    Ok(AdminAccountGroup {
        id: row.get(0),
        provider_type: row.get(1),
        name: row.get(2),
        account_names,
        created_at: row.get(4),
        updated_at: row.get(5),
    })
}

pub fn decode_admin_proxy_config_row(row: PgRow) -> AdminProxyConfig {
    AdminProxyConfig {
        id: row.get(0),
        name: row.get(1),
        proxy_url: row.get(2),
        proxy_username: row.get(3),
        proxy_password: row.get(4),
        status: row.get(5),
        created_at: row.get(6),
        updated_at: row.get(7),
        scope_node_id: None,
        effective_source: "core".to_string(),
        has_node_override: false,
        can_edit_slot_metadata: true,
        latest_codex_check: None,
        latest_kiro_check: None,
    }
}

pub fn decode_proxy_endpoint_check_row(row: PgRow) -> ProxyEndpointCheckRow {
    let status_code = row
        .get::<_, Option<i32>>(4)
        .and_then(|value| u16::try_from(value).ok());
    ProxyEndpointCheckRow {
        proxy_config_id: row.get(0),
        provider_type: row.get(1),
        check: AdminProxyEndpointCheck {
            target_url: row.get(2),
            reachable: row.get(3),
            status_code,
            latency_ms: row.get(5),
            error_message: row.get(6),
            checked_at: row.get(7),
        },
    }
}

pub fn decode_codex_account_row(row: PgRow) -> CodexAccountRecord {
    CodexAccountRecord {
        account_name: row.get(0),
        account_id: row.get(1),
        email: row.get(2),
        status: row.get(3),
        auth_json: row.get(4),
        settings_json: row.get(5),
        last_refresh_at_ms: row.get(6),
        last_error: row.get(7),
        created_at_ms: row.get(8),
        updated_at_ms: row.get(9),
    }
}

pub fn decode_kiro_account_row(row: PgRow) -> KiroAccountRecord {
    KiroAccountRecord {
        account_name: row.get(0),
        auth_method: row.get(1),
        account_id: row.get(2),
        profile_arn: row.get(3),
        user_id: row.get(4),
        status: row.get(5),
        auth_json: row.get(6),
        max_concurrency: row.get(7),
        min_start_interval_ms: row.get(8),
        proxy_config_id: row.get(9),
        last_refresh_at_ms: row.get(10),
        last_error: row.get(11),
        created_at_ms: row.get(12),
        updated_at_ms: row.get(13),
    }
}

pub fn decode_codex_admin_account_list_row(row: PgRow) -> CodexAdminAccountListRow {
    CodexAdminAccountListRow {
        account_name: row.get(0),
        account_id: row.get(1),
        status: row.get(2),
        map_gpt53_codex_to_spark: row.get(3),
        auth_refresh_enabled: row.get(4),
        route_weight_tier: row.get(5),
        proxy_mode: row.get(6),
        proxy_config_id: row.get(7),
        request_max_concurrency: row.get(8),
        request_min_start_interval_ms: row.get(9),
        last_refresh_at_ms: row.get(10),
        last_error: row.get(11),
        access_token: row.get(12),
        plan_type: row.get(13),
        primary_remaining_percent: row.get(14),
        secondary_remaining_percent: row.get(15),
        last_usage_checked_at_ms: row.get(16),
        last_usage_success_at_ms: row.get(17),
        usage_error_message: row.get(18),
    }
}

pub fn decode_kiro_admin_account_list_row(row: PgRow) -> KiroAdminAccountListRow {
    KiroAdminAccountListRow {
        account_name: row.get(0),
        auth_method: row.get(1),
        profile_arn: row.get(2),
        user_id: row.get(3),
        status: row.get(4),
        provider: row.get(5),
        email: row.get(6),
        expires_at: row.get(7),
        auth_profile_arn: row.get(8),
        has_refresh_token: row.get(9),
        disabled_json: row.get(10),
        disabled_reason: row.get(11),
        source: row.get(12),
        source_db_path: row.get(13),
        last_imported_at: row.get(14),
        subscription_title: row.get(15),
        region: row.get(16),
        auth_region: row.get(17),
        api_region: row.get(18),
        machine_id: row.get(19),
        max_concurrency: row.get(20),
        auth_max_concurrency: row.get(21),
        min_start_interval_ms: row.get(22),
        auth_min_start_interval_ms: row.get(23),
        minimum_remaining_credits_before_block: row.get(24),
        proxy_mode: row.get(25),
        proxy_config_id: row.get(26),
        auth_proxy_config_id: row.get(27),
        proxy_url: row.get(28),
        last_error: row.get(29),
    }
}

pub fn decode_public_usage_lookup_row(row: PgRow) -> anyhow::Result<PublicUsageLookupKey> {
    let credit_total_raw: String = row.get(10);
    let usage_credit_total = credit_total_raw
        .parse::<f64>()
        .with_context(|| format!("parse usage credit_total `{credit_total_raw}`"))?;
    Ok(PublicUsageLookupKey {
        key_id: row.get(0),
        key_name: row.get(1),
        provider_type: row.get(2),
        status: row.get(3),
        public_visible: row.get(4),
        quota_billable_limit: row.get::<_, i64>(5).max(0) as u64,
        usage_input_uncached_tokens: row.get::<_, i64>(6).max(0) as u64,
        usage_input_cached_tokens: row.get::<_, i64>(7).max(0) as u64,
        usage_output_tokens: row.get::<_, i64>(8).max(0) as u64,
        usage_billable_tokens: row.get::<_, i64>(9).max(0) as u64,
        usage_credit_total,
        usage_credit_missing_events: row.get::<_, i64>(11).max(0) as u64,
        last_used_at_ms: row.get(12),
    })
}

pub fn decode_admin_token_request_row(row: PgRow) -> AdminTokenRequest {
    AdminTokenRequest {
        request_id: row.get(0),
        requester_email: row.get(1),
        requested_quota_billable_limit: row.get::<_, i64>(2).max(0) as u64,
        request_reason: row.get(3),
        frontend_page_url: row.get(4),
        status: row.get(5),
        client_ip: row.get(6),
        ip_region: row.get(7),
        admin_note: row.get(8),
        failure_reason: row.get(9),
        issued_key_id: row.get(10),
        issued_key_name: row.get(11),
        created_at: row.get(12),
        updated_at: row.get(13),
        processed_at: row.get(14),
    }
}

pub fn decode_admin_account_contribution_request_row(
    row: PgRow,
) -> AdminAccountContributionRequest {
    AdminAccountContributionRequest {
        request_id: row.get(0),
        account_name: row.get(1),
        account_id: row.get(2),
        id_token: row.get(3),
        access_token: row.get(4),
        refresh_token: row.get(5),
        requester_email: row.get(6),
        contributor_message: row.get(7),
        github_id: row.get(8),
        frontend_page_url: row.get(9),
        status: row.get(10),
        client_ip: row.get(11),
        ip_region: row.get(12),
        admin_note: row.get(13),
        failure_reason: row.get(14),
        imported_account_name: row.get(15),
        issued_key_id: row.get(16),
        issued_key_name: row.get(17),
        created_at: row.get(18),
        updated_at: row.get(19),
        processed_at: row.get(20),
    }
}

pub fn decode_admin_sponsor_request_row(row: PgRow) -> AdminSponsorRequest {
    AdminSponsorRequest {
        request_id: row.get(0),
        requester_email: row.get(1),
        sponsor_message: row.get(2),
        display_name: row.get(3),
        github_id: row.get(4),
        frontend_page_url: row.get(5),
        status: row.get(6),
        client_ip: row.get(7),
        ip_region: row.get(8),
        admin_note: row.get(9),
        failure_reason: row.get(10),
        payment_email_sent_at: row.get(11),
        created_at: row.get(12),
        updated_at: row.get(13),
        processed_at: row.get(14),
    }
}

pub fn decode_codex_import_job_summary_row(row: PgRow) -> AdminCodexImportJobSummary {
    AdminCodexImportJobSummary {
        job_id: row.get(0),
        provider_type: row.get(1),
        source_type: row.get(2),
        validate_before_import: row.get(3),
        status: row.get(4),
        total_count: row.get::<_, i64>(5).max(0) as usize,
        completed_count: row.get::<_, i64>(6).max(0) as usize,
        succeeded_count: row.get::<_, i64>(7).max(0) as usize,
        skipped_count: row.get::<_, i64>(8).max(0) as usize,
        failed_count: row.get::<_, i64>(9).max(0) as usize,
        batch_error_message: row.get(10),
        created_at_ms: row.get(11),
        updated_at_ms: row.get(12),
        finished_at_ms: row.get(13),
    }
}

pub fn decode_codex_import_job_item_row(row: PgRow) -> AdminCodexImportJobItem {
    AdminCodexImportJobItem {
        item_index: row.get::<_, i64>(0).max(0) as usize,
        requested_name: row.get(1),
        requested_account_id: row.get(2),
        status: row.get(3),
        error_message: row.get(4),
        imported_account_name: row.get(5),
        final_account_id: row.get(6),
        validated_at_ms: row.get(7),
        imported_at_ms: row.get(8),
    }
}

pub fn decode_codex_account_settings(value: &str) -> anyhow::Result<CodexAccountSettings> {
    serde_json::from_str(value).context("decode codex account settings")
}
