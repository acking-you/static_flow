CREATE TABLE IF NOT EXISTS llm_keys (
    key_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    secret TEXT NOT NULL,
    key_hash TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    protocol_family TEXT NOT NULL CHECK (protocol_family IN ('openai', 'anthropic')),
    public_visible INTEGER NOT NULL CHECK (public_visible IN (0, 1)),
    quota_billable_limit INTEGER NOT NULL CHECK (quota_billable_limit >= 0),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_keys_provider_status
    ON llm_keys(provider_type, status);

CREATE INDEX IF NOT EXISTS idx_llm_keys_public_visible
    ON llm_keys(public_visible, status);

CREATE TABLE IF NOT EXISTS llm_key_route_config (
    key_id TEXT PRIMARY KEY REFERENCES llm_keys(key_id) ON DELETE CASCADE,
    route_strategy TEXT CHECK (
        route_strategy IS NULL OR route_strategy IN ('auto', 'fixed')
    ),
    fixed_account_name TEXT,
    auto_account_names_json TEXT CHECK (
        auto_account_names_json IS NULL OR json_valid(auto_account_names_json)
    ),
    account_group_id TEXT,
    model_name_map_json TEXT CHECK (
        model_name_map_json IS NULL OR json_valid(model_name_map_json)
    ),
    request_max_concurrency INTEGER CHECK (
        request_max_concurrency IS NULL OR request_max_concurrency >= 0
    ),
    request_min_start_interval_ms INTEGER CHECK (
        request_min_start_interval_ms IS NULL OR request_min_start_interval_ms >= 0
    ),
    kiro_request_validation_enabled INTEGER NOT NULL DEFAULT 0 CHECK (
        kiro_request_validation_enabled IN (0, 1)
    ),
    kiro_cache_estimation_enabled INTEGER NOT NULL DEFAULT 0 CHECK (
        kiro_cache_estimation_enabled IN (0, 1)
    ),
    kiro_zero_cache_debug_enabled INTEGER NOT NULL DEFAULT 0 CHECK (
        kiro_zero_cache_debug_enabled IN (0, 1)
    ),
    kiro_cache_policy_override_json TEXT CHECK (
        kiro_cache_policy_override_json IS NULL OR json_valid(kiro_cache_policy_override_json)
    ),
    kiro_billable_model_multipliers_override_json TEXT CHECK (
        kiro_billable_model_multipliers_override_json IS NULL
        OR json_valid(kiro_billable_model_multipliers_override_json)
    )
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_key_route_config_group
    ON llm_key_route_config(account_group_id);

CREATE TABLE IF NOT EXISTS llm_key_usage_rollups (
    key_id TEXT PRIMARY KEY REFERENCES llm_keys(key_id) ON DELETE CASCADE,
    input_uncached_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_uncached_tokens >= 0),
    input_cached_tokens INTEGER NOT NULL DEFAULT 0 CHECK (input_cached_tokens >= 0),
    output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    billable_tokens INTEGER NOT NULL DEFAULT 0 CHECK (billable_tokens >= 0),
    credit_total TEXT NOT NULL DEFAULT '0',
    credit_missing_events INTEGER NOT NULL DEFAULT 0 CHECK (credit_missing_events >= 0),
    last_used_at_ms INTEGER CHECK (last_used_at_ms IS NULL OR last_used_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS llm_runtime_config (
    id TEXT PRIMARY KEY CHECK (id = 'default'),
    auth_cache_ttl_seconds INTEGER NOT NULL CHECK (auth_cache_ttl_seconds >= 0),
    max_request_body_bytes INTEGER NOT NULL CHECK (max_request_body_bytes >= 0),
    account_failure_retry_limit INTEGER NOT NULL CHECK (account_failure_retry_limit >= 0),
    codex_client_version TEXT NOT NULL,
    kiro_channel_max_concurrency INTEGER NOT NULL CHECK (kiro_channel_max_concurrency >= 0),
    kiro_channel_min_start_interval_ms INTEGER NOT NULL CHECK (
        kiro_channel_min_start_interval_ms >= 0
    ),
    codex_status_refresh_min_interval_seconds INTEGER NOT NULL CHECK (
        codex_status_refresh_min_interval_seconds >= 0
    ),
    codex_status_refresh_max_interval_seconds INTEGER NOT NULL CHECK (
        codex_status_refresh_max_interval_seconds >= 0
    ),
    codex_status_account_jitter_max_seconds INTEGER NOT NULL CHECK (
        codex_status_account_jitter_max_seconds >= 0
    ),
    kiro_status_refresh_min_interval_seconds INTEGER NOT NULL CHECK (
        kiro_status_refresh_min_interval_seconds >= 0
    ),
    kiro_status_refresh_max_interval_seconds INTEGER NOT NULL CHECK (
        kiro_status_refresh_max_interval_seconds >= 0
    ),
    kiro_status_account_jitter_max_seconds INTEGER NOT NULL CHECK (
        kiro_status_account_jitter_max_seconds >= 0
    ),
    usage_event_flush_batch_size INTEGER NOT NULL CHECK (usage_event_flush_batch_size >= 1),
    usage_event_flush_interval_seconds INTEGER NOT NULL CHECK (
        usage_event_flush_interval_seconds >= 1
    ),
    usage_event_flush_max_buffer_bytes INTEGER NOT NULL CHECK (
        usage_event_flush_max_buffer_bytes >= 1
    ),
    usage_event_maintenance_enabled INTEGER NOT NULL CHECK (
        usage_event_maintenance_enabled IN (0, 1)
    ),
    usage_event_maintenance_interval_seconds INTEGER NOT NULL CHECK (
        usage_event_maintenance_interval_seconds >= 0
    ),
    usage_event_detail_retention_days INTEGER NOT NULL,
    kiro_cache_kmodels_json TEXT NOT NULL CHECK (json_valid(kiro_cache_kmodels_json)),
    kiro_billable_model_multipliers_json TEXT NOT NULL CHECK (
        json_valid(kiro_billable_model_multipliers_json)
    ),
    kiro_cache_policy_json TEXT NOT NULL CHECK (json_valid(kiro_cache_policy_json)),
    kiro_prefix_cache_mode TEXT NOT NULL CHECK (
        kiro_prefix_cache_mode IN ('formula', 'prefix_tree')
    ),
    kiro_prefix_cache_max_tokens INTEGER NOT NULL CHECK (kiro_prefix_cache_max_tokens >= 0),
    kiro_prefix_cache_entry_ttl_seconds INTEGER NOT NULL CHECK (
        kiro_prefix_cache_entry_ttl_seconds >= 0
    ),
    kiro_conversation_anchor_max_entries INTEGER NOT NULL CHECK (
        kiro_conversation_anchor_max_entries >= 0
    ),
    kiro_conversation_anchor_ttl_seconds INTEGER NOT NULL CHECK (
        kiro_conversation_anchor_ttl_seconds >= 0
    ),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS llm_account_groups (
    group_id TEXT PRIMARY KEY,
    provider_type TEXT NOT NULL CHECK (provider_type IN ('codex', 'kiro')),
    name TEXT NOT NULL,
    account_names_json TEXT NOT NULL CHECK (json_valid(account_names_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_account_groups_provider
    ON llm_account_groups(provider_type);

CREATE TABLE IF NOT EXISTS llm_proxy_configs (
    proxy_config_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    proxy_url TEXT NOT NULL,
    proxy_username TEXT,
    proxy_password TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_proxy_configs_status
    ON llm_proxy_configs(status);

CREATE TABLE IF NOT EXISTS llm_proxy_bindings (
    provider_type TEXT PRIMARY KEY CHECK (provider_type IN ('codex', 'kiro')),
    proxy_config_id TEXT NOT NULL REFERENCES llm_proxy_configs(proxy_config_id),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS llm_codex_accounts (
    account_name TEXT PRIMARY KEY,
    account_id TEXT,
    email TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json TEXT NOT NULL CHECK (json_valid(auth_json)),
    settings_json TEXT NOT NULL CHECK (json_valid(settings_json)),
    last_refresh_at_ms INTEGER CHECK (last_refresh_at_ms IS NULL OR last_refresh_at_ms >= 0),
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_codex_accounts_status
    ON llm_codex_accounts(status);

CREATE TABLE IF NOT EXISTS llm_kiro_accounts (
    account_name TEXT PRIMARY KEY,
    auth_method TEXT NOT NULL,
    account_id TEXT,
    profile_arn TEXT,
    user_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('active', 'disabled', 'unavailable')),
    auth_json TEXT NOT NULL CHECK (json_valid(auth_json)),
    max_concurrency INTEGER CHECK (max_concurrency IS NULL OR max_concurrency >= 0),
    min_start_interval_ms INTEGER CHECK (
        min_start_interval_ms IS NULL OR min_start_interval_ms >= 0
    ),
    proxy_config_id TEXT,
    last_refresh_at_ms INTEGER CHECK (last_refresh_at_ms IS NULL OR last_refresh_at_ms >= 0),
    last_error TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_status
    ON llm_kiro_accounts(status);

CREATE INDEX IF NOT EXISTS idx_llm_kiro_accounts_user_id
    ON llm_kiro_accounts(user_id);

CREATE TABLE IF NOT EXISTS llm_kiro_status_cache (
    account_name TEXT PRIMARY KEY REFERENCES llm_kiro_accounts(account_name) ON DELETE CASCADE,
    status TEXT NOT NULL,
    balance_json TEXT NOT NULL CHECK (json_valid(balance_json)),
    cache_json TEXT NOT NULL CHECK (json_valid(cache_json)),
    refreshed_at_ms INTEGER NOT NULL CHECK (refreshed_at_ms >= 0),
    expires_at_ms INTEGER NOT NULL CHECK (expires_at_ms >= 0),
    last_error TEXT
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_kiro_status_cache_expires
    ON llm_kiro_status_cache(expires_at_ms);

CREATE TABLE IF NOT EXISTS llm_token_requests (
    request_id TEXT PRIMARY KEY,
    requester_email TEXT NOT NULL,
    requested_quota_billable_limit INTEGER NOT NULL CHECK (requested_quota_billable_limit >= 0),
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
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms INTEGER CHECK (processed_at_ms IS NULL OR processed_at_ms >= 0)
) STRICT, WITHOUT ROWID;

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
    status TEXT NOT NULL CHECK (status IN ('pending', 'issued', 'rejected', 'failed')),
    fingerprint TEXT NOT NULL,
    client_ip TEXT NOT NULL,
    ip_region TEXT NOT NULL,
    admin_note TEXT,
    failure_reason TEXT,
    imported_account_name TEXT,
    issued_key_id TEXT,
    issued_key_name TEXT,
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms INTEGER CHECK (processed_at_ms IS NULL OR processed_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_account_contribution_requests_status_created
    ON llm_account_contribution_requests(status, created_at_ms);

CREATE TABLE IF NOT EXISTS gpt2api_account_contribution_requests (
    request_id TEXT PRIMARY KEY,
    account_name TEXT NOT NULL,
    access_token TEXT,
    session_json TEXT CHECK (session_json IS NULL OR json_valid(session_json)),
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
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms INTEGER CHECK (processed_at_ms IS NULL OR processed_at_ms >= 0)
) STRICT, WITHOUT ROWID;

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
    payment_email_sent_at_ms INTEGER CHECK (
        payment_email_sent_at_ms IS NULL OR payment_email_sent_at_ms >= 0
    ),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0),
    processed_at_ms INTEGER CHECK (processed_at_ms IS NULL OR processed_at_ms >= 0)
) STRICT, WITHOUT ROWID;

CREATE INDEX IF NOT EXISTS idx_llm_sponsor_requests_status_created
    ON llm_sponsor_requests(status, created_at_ms);
