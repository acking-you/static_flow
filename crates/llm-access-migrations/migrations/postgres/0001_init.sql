CREATE TABLE IF NOT EXISTS llm_keys (
    key_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret TEXT NOT NULL,
    key_hash TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    protocol_family TEXT NOT NULL CHECK (protocol_family IN ('openai', 'anthropic')),
    public_visible BOOLEAN NOT NULL,
    quota_billable_limit BIGINT NOT NULL CHECK (quota_billable_limit >= 0),
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE UNIQUE INDEX IF NOT EXISTS uq_llm_keys_key_hash ON llm_keys (key_hash);
CREATE INDEX IF NOT EXISTS idx_llm_keys_provider_status ON llm_keys (provider_type, status);
CREATE INDEX IF NOT EXISTS idx_llm_keys_public_visible ON llm_keys (public_visible, status);

CREATE TABLE IF NOT EXISTS llm_key_route_config (
    key_id TEXT PRIMARY KEY REFERENCES llm_keys(key_id) ON DELETE CASCADE,
    route_strategy TEXT,
    fixed_account_name TEXT,
    auto_account_names_json JSONB,
    account_group_id TEXT,
    model_name_map_json JSONB,
    request_max_concurrency BIGINT,
    request_min_start_interval_ms BIGINT,
    codex_fast_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    kiro_request_validation_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_cache_estimation_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_zero_cache_debug_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_cache_policy_override_json JSONB,
    kiro_billable_model_multipliers_override_json JSONB,
    kiro_full_request_logging_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_remote_media_resolution_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_latency_routing_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    kiro_protected_content_validation_enabled BOOLEAN NOT NULL DEFAULT FALSE,
    kiro_cctest_text_handling_enabled BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE INDEX IF NOT EXISTS idx_llm_key_route_config_group
    ON llm_key_route_config(account_group_id);

CREATE TABLE IF NOT EXISTS llm_key_usage_rollups (
    key_id TEXT PRIMARY KEY REFERENCES llm_keys(key_id) ON DELETE CASCADE,
    input_uncached_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_uncached_tokens >= 0),
    input_cached_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_cached_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    billable_tokens BIGINT NOT NULL DEFAULT 0 CHECK (billable_tokens >= 0),
    credit_total TEXT NOT NULL DEFAULT '0',
    credit_missing_events BIGINT NOT NULL DEFAULT 0 CHECK (credit_missing_events >= 0),
    last_used_at_ms BIGINT,
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE TABLE IF NOT EXISTS llm_key_usage_rollup_applied_batches (
    batch_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL,
    source_node_id TEXT,
    source_event_count BIGINT NOT NULL CHECK (source_event_count >= 0),
    delta_count BIGINT NOT NULL CHECK (delta_count >= 0),
    applied_at_ms BIGINT NOT NULL CHECK (applied_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_key_usage_rollup_applied_batches_applied_at
    ON llm_key_usage_rollup_applied_batches(applied_at_ms);

CREATE TABLE IF NOT EXISTS llm_runtime_config (
    id TEXT PRIMARY KEY CHECK (id = 'default'),
    auth_cache_ttl_seconds BIGINT NOT NULL CHECK (auth_cache_ttl_seconds >= 0),
    max_request_body_bytes BIGINT NOT NULL CHECK (max_request_body_bytes >= 0),
    account_failure_retry_limit BIGINT NOT NULL CHECK (account_failure_retry_limit >= 0),
    codex_client_version TEXT NOT NULL,
    kiro_channel_max_concurrency BIGINT NOT NULL CHECK (kiro_channel_max_concurrency >= 0),
    kiro_channel_min_start_interval_ms BIGINT NOT NULL CHECK (
        kiro_channel_min_start_interval_ms >= 0
    ),
    codex_status_refresh_min_interval_seconds BIGINT NOT NULL CHECK (
        codex_status_refresh_min_interval_seconds >= 0
    ),
    codex_status_refresh_max_interval_seconds BIGINT NOT NULL CHECK (
        codex_status_refresh_max_interval_seconds >= 0
    ),
    codex_status_account_jitter_max_seconds BIGINT NOT NULL CHECK (
        codex_status_account_jitter_max_seconds >= 0
    ),
    kiro_status_refresh_min_interval_seconds BIGINT NOT NULL CHECK (
        kiro_status_refresh_min_interval_seconds >= 0
    ),
    kiro_status_refresh_max_interval_seconds BIGINT NOT NULL CHECK (
        kiro_status_refresh_max_interval_seconds >= 0
    ),
    kiro_status_account_jitter_max_seconds BIGINT NOT NULL CHECK (
        kiro_status_account_jitter_max_seconds >= 0
    ),
    usage_event_flush_batch_size BIGINT NOT NULL CHECK (usage_event_flush_batch_size >= 1),
    usage_event_flush_interval_seconds BIGINT NOT NULL CHECK (
        usage_event_flush_interval_seconds >= 1
    ),
    usage_event_flush_max_buffer_bytes BIGINT NOT NULL CHECK (
        usage_event_flush_max_buffer_bytes >= 1
    ),
    usage_event_maintenance_enabled BOOLEAN NOT NULL,
    usage_event_maintenance_interval_seconds BIGINT NOT NULL CHECK (
        usage_event_maintenance_interval_seconds >= 0
    ),
    usage_event_detail_retention_days BIGINT NOT NULL,
    kiro_cache_kmodels_json JSONB NOT NULL,
    kiro_billable_model_multipliers_json JSONB NOT NULL,
    kiro_cache_policy_json JSONB NOT NULL,
    kiro_context_usage_min_request_tokens BIGINT NOT NULL DEFAULT 15000 CHECK (
        kiro_context_usage_min_request_tokens >= 1
    ),
    kiro_compact_trigger_tokens BIGINT NOT NULL DEFAULT 780000 CHECK (
        kiro_compact_trigger_tokens >= 0
    ),
    kiro_prefix_cache_mode TEXT NOT NULL CHECK (
        kiro_prefix_cache_mode IN ('formula', 'prefix_tree')
    ),
    kiro_prefix_cache_max_tokens BIGINT NOT NULL CHECK (kiro_prefix_cache_max_tokens >= 0),
    kiro_prefix_cache_entry_ttl_seconds BIGINT NOT NULL CHECK (
        kiro_prefix_cache_entry_ttl_seconds >= 0
    ),
    kiro_conversation_anchor_max_entries BIGINT NOT NULL CHECK (
        kiro_conversation_anchor_max_entries >= 0
    ),
    kiro_conversation_anchor_ttl_seconds BIGINT NOT NULL CHECK (
        kiro_conversation_anchor_ttl_seconds >= 0
    ),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    duckdb_usage_memory_limit_mib BIGINT NOT NULL DEFAULT 1024,
    duckdb_usage_checkpoint_threshold_mib BIGINT NOT NULL DEFAULT 16,
    usage_journal_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    usage_journal_max_file_bytes BIGINT NOT NULL DEFAULT 67108864,
    usage_journal_max_file_age_ms BIGINT NOT NULL DEFAULT 300000,
    usage_journal_max_files BIGINT NOT NULL DEFAULT 128,
    usage_journal_block_target_uncompressed_bytes BIGINT NOT NULL DEFAULT 1048576,
    usage_journal_block_max_events BIGINT NOT NULL DEFAULT 1024,
    usage_journal_fsync_interval_ms BIGINT NOT NULL DEFAULT 250,
    usage_journal_zstd_level BIGINT NOT NULL DEFAULT 3,
    usage_journal_consumer_lease_ms BIGINT NOT NULL DEFAULT 300000,
    usage_journal_delete_bad_files BIGINT NOT NULL DEFAULT 0,
    usage_query_bind_addr TEXT NOT NULL DEFAULT '127.0.0.1:19081',
    usage_query_base_url TEXT NOT NULL DEFAULT 'http://127.0.0.1:19081',
    codex_weight_free BIGINT NOT NULL DEFAULT 1,
    codex_weight_plus BIGINT NOT NULL DEFAULT 10,
    codex_weight_pro5x BIGINT NOT NULL DEFAULT 50,
    codex_weight_pro20x BIGINT NOT NULL DEFAULT 200,
    codex_session_affinity_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    codex_session_affinity_max_entries BIGINT NOT NULL DEFAULT 20000 CHECK (
        codex_session_affinity_max_entries >= 0
    ),
    codex_session_affinity_ttl_seconds BIGINT NOT NULL DEFAULT 21600 CHECK (
        codex_session_affinity_ttl_seconds >= 0
    ),
    codex_fallback_affinity_enabled BOOLEAN NOT NULL DEFAULT TRUE,
    codex_fallback_affinity_ttl_seconds BIGINT NOT NULL DEFAULT 1800 CHECK (
        codex_fallback_affinity_ttl_seconds >= 0
    ),
    codex_fallback_affinity_prefix_bytes BIGINT NOT NULL DEFAULT 4096 CHECK (
        codex_fallback_affinity_prefix_bytes >= 0
    ),
    codex_fallback_affinity_min_body_bytes BIGINT NOT NULL DEFAULT 128 CHECK (
        codex_fallback_affinity_min_body_bytes >= 0
    ),
    usage_analytics_retention_days BIGINT NOT NULL DEFAULT 7,
    kiro_cctest_proxy_base_url TEXT,
    kiro_cctest_proxy_api_key TEXT
);

CREATE TABLE IF NOT EXISTS llm_account_groups (
    group_id TEXT PRIMARY KEY,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    name TEXT NOT NULL,
    account_names_json JSONB NOT NULL,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_account_groups_provider
    ON llm_account_groups(provider_type);

CREATE TABLE IF NOT EXISTS llm_proxy_configs (
    proxy_config_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    proxy_url TEXT NOT NULL,
    proxy_username TEXT,
    proxy_password TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_proxy_configs_status
    ON llm_proxy_configs(status);

CREATE TABLE IF NOT EXISTS llm_proxy_bindings (
    provider_type TEXT PRIMARY KEY CHECK (provider_type IN ('codex', 'kiro')),
    proxy_config_id TEXT NOT NULL REFERENCES llm_proxy_configs(proxy_config_id),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE TABLE IF NOT EXISTS llm_codex_accounts (
    account_name TEXT PRIMARY KEY,
    account_id TEXT,
    email TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json JSONB NOT NULL,
    settings_json JSONB NOT NULL,
    last_refresh_at_ms BIGINT,
    last_error TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_codex_accounts_status
    ON llm_codex_accounts(status);

CREATE TABLE IF NOT EXISTS llm_kiro_accounts (
    account_name TEXT PRIMARY KEY,
    auth_method TEXT NOT NULL,
    account_id TEXT,
    profile_arn TEXT,
    user_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json JSONB NOT NULL,
    max_concurrency BIGINT,
    min_start_interval_ms BIGINT,
    proxy_config_id TEXT,
    last_refresh_at_ms BIGINT,
    last_error TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_status
    ON llm_kiro_accounts(status);

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_user_id
    ON llm_kiro_accounts(user_id);

CREATE TABLE IF NOT EXISTS llm_kiro_status_cache (
    account_name TEXT PRIMARY KEY REFERENCES llm_kiro_accounts(account_name) ON DELETE CASCADE,
    status TEXT NOT NULL,
    balance_json JSONB NOT NULL,
    cache_json JSONB NOT NULL,
    refreshed_at_ms BIGINT NOT NULL,
    expires_at_ms BIGINT NOT NULL,
    last_error TEXT
);

CREATE INDEX IF NOT EXISTS idx_llm_kiro_status_cache_expires
    ON llm_kiro_status_cache(expires_at_ms);

CREATE TABLE IF NOT EXISTS llm_token_requests (
    request_id TEXT PRIMARY KEY,
    requester_email TEXT NOT NULL,
    requested_quota_billable_limit BIGINT NOT NULL CHECK (requested_quota_billable_limit >= 0),
    request_reason TEXT NOT NULL,
    frontend_page_url TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'issued', 'rejected', 'failed')),
    fingerprint TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    ip_region TEXT NOT NULL,
    admin_note TEXT,
    failure_reason TEXT,
    issued_key_id TEXT,
    issued_key_name TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms BIGINT
);

CREATE INDEX IF NOT EXISTS idx_llm_token_requests_status_created
    ON llm_token_requests(status, created_at_ms);

CREATE TABLE IF NOT EXISTS llm_account_contribution_requests (
    request_id TEXT PRIMARY KEY,
    account_name TEXT NOT NULL,
    account_id TEXT,
    id_token TEXT NOT NULL,
    access_token TEXT NOT NULL,
    refresh_token TEXT NOT NULL,
    requester_email TEXT NOT NULL,
    contributor_message TEXT NOT NULL,
    github_id TEXT,
    frontend_page_url TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'validated', 'issued', 'rejected', 'failed')),
    fingerprint TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    ip_region TEXT NOT NULL,
    admin_note TEXT,
    failure_reason TEXT,
    imported_account_name TEXT,
    issued_key_id TEXT,
    issued_key_name TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms BIGINT,
    show_on_public_wall BOOLEAN NOT NULL DEFAULT TRUE
);

CREATE INDEX IF NOT EXISTS idx_llm_account_contribution_requests_status_created
    ON llm_account_contribution_requests(status, created_at_ms);

CREATE TABLE IF NOT EXISTS gpt2api_account_contribution_requests (
    request_id TEXT PRIMARY KEY,
    account_name TEXT NOT NULL,
    access_token TEXT,
    session_json JSONB,
    requester_email TEXT NOT NULL,
    contributor_message TEXT NOT NULL,
    github_id TEXT,
    frontend_page_url TEXT,
    status TEXT NOT NULL CHECK (status IN ('pending', 'issued', 'rejected', 'failed')),
    fingerprint TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    ip_region TEXT NOT NULL,
    admin_note TEXT,
    failure_reason TEXT,
    imported_account_name TEXT,
    issued_key_id TEXT,
    issued_key_name TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms BIGINT
);

CREATE INDEX IF NOT EXISTS idx_gpt2api_account_contribution_requests_status_created
    ON gpt2api_account_contribution_requests(status, created_at_ms);

CREATE TABLE IF NOT EXISTS llm_sponsor_requests (
    request_id TEXT PRIMARY KEY,
    requester_email TEXT NOT NULL,
    sponsor_message TEXT NOT NULL,
    display_name TEXT,
    github_id TEXT,
    frontend_page_url TEXT,
    status TEXT NOT NULL CHECK (status IN ('submitted', 'payment_email_sent', 'approved')),
    fingerprint TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    ip_region TEXT NOT NULL,
    admin_note TEXT,
    failure_reason TEXT,
    payment_email_sent_at_ms BIGINT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms BIGINT
);

CREATE INDEX IF NOT EXISTS idx_llm_sponsor_requests_status_created
    ON llm_sponsor_requests(status, created_at_ms);

CREATE TABLE IF NOT EXISTS llm_codex_status_cache (
    id TEXT PRIMARY KEY CHECK (id = 'default'),
    snapshot_json JSONB NOT NULL,
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE TABLE IF NOT EXISTS llm_account_import_jobs (
    job_id TEXT PRIMARY KEY,
    provider_type TEXT NOT NULL,
    source_type TEXT NOT NULL,
    validate_before_import BOOLEAN NOT NULL,
    status TEXT NOT NULL,
    total_count BIGINT NOT NULL,
    completed_count BIGINT NOT NULL,
    succeeded_count BIGINT NOT NULL,
    skipped_count BIGINT NOT NULL,
    failed_count BIGINT NOT NULL,
    batch_error_message TEXT,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    finished_at_ms BIGINT
);

CREATE INDEX IF NOT EXISTS idx_llm_account_import_jobs_created
    ON llm_account_import_jobs(created_at_ms);

CREATE TABLE IF NOT EXISTS llm_account_import_job_items (
    job_id TEXT NOT NULL REFERENCES llm_account_import_jobs(job_id) ON DELETE CASCADE,
    item_index BIGINT NOT NULL,
    requested_name TEXT NOT NULL,
    requested_account_id TEXT,
    raw_auth_json JSONB,
    status TEXT NOT NULL,
    error_message TEXT,
    imported_account_name TEXT,
    final_account_id TEXT,
    validated_at_ms BIGINT,
    imported_at_ms BIGINT,
    created_at_ms BIGINT NOT NULL,
    updated_at_ms BIGINT NOT NULL,
    PRIMARY KEY (job_id, item_index)
);

CREATE INDEX IF NOT EXISTS idx_llm_account_import_job_items_job_status
    ON llm_account_import_job_items(job_id, status, item_index);
