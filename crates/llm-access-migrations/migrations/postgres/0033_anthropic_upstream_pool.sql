ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_anthropic_upstream_pool_mode TEXT NOT NULL DEFAULT 'disabled';

UPDATE llm_key_route_config
SET kiro_anthropic_upstream_pool_mode = 'disabled'
WHERE kiro_anthropic_upstream_pool_mode IS NULL
   OR kiro_anthropic_upstream_pool_mode NOT IN (
        'disabled',
        'preferred_before_kiro',
        'only'
   );

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'ck_llm_key_route_config_anthropic_pool_mode'
    ) THEN
        ALTER TABLE llm_key_route_config
            ADD CONSTRAINT ck_llm_key_route_config_anthropic_pool_mode
            CHECK (
                kiro_anthropic_upstream_pool_mode IN (
                    'disabled',
                    'preferred_before_kiro',
                    'only'
                )
            );
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS llm_anthropic_upstream_channels (
    channel_name TEXT PRIMARY KEY,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled')),
    base_url TEXT NOT NULL,
    auth_json JSONB NOT NULL,
    weight BIGINT NOT NULL DEFAULT 100 CHECK (weight >= 0),
    max_concurrency BIGINT NOT NULL DEFAULT 3 CHECK (max_concurrency >= 1),
    min_start_interval_ms BIGINT NOT NULL DEFAULT 0 CHECK (min_start_interval_ms >= 0),
    proxy_mode TEXT NOT NULL DEFAULT 'inherit' CHECK (proxy_mode IN ('inherit', 'direct', 'fixed')),
    proxy_config_id TEXT REFERENCES llm_proxy_configs(proxy_config_id) ON DELETE SET NULL,
    last_error TEXT,
    created_at_ms BIGINT NOT NULL CHECK (created_at_ms >= 0),
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);

CREATE INDEX IF NOT EXISTS idx_llm_anthropic_upstream_channels_status_weight
    ON llm_anthropic_upstream_channels(status, weight DESC, channel_name);

CREATE TABLE IF NOT EXISTS llm_anthropic_upstream_channel_usage_rollups (
    channel_name TEXT PRIMARY KEY REFERENCES llm_anthropic_upstream_channels(channel_name)
        ON DELETE CASCADE,
    input_uncached_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_uncached_tokens >= 0),
    input_cached_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_cached_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    billable_tokens BIGINT NOT NULL DEFAULT 0 CHECK (billable_tokens >= 0),
    usage_missing_events BIGINT NOT NULL DEFAULT 0 CHECK (usage_missing_events >= 0),
    last_used_at_ms BIGINT,
    updated_at_ms BIGINT NOT NULL CHECK (updated_at_ms >= 0)
);
