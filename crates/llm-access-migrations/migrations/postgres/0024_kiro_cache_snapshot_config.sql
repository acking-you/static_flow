ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cache_snapshot_enabled BOOLEAN NOT NULL DEFAULT FALSE;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cache_snapshot_interval_seconds BIGINT NOT NULL DEFAULT 300
        CHECK (kiro_cache_snapshot_interval_seconds >= 1);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cache_snapshot_ttl_seconds BIGINT NOT NULL DEFAULT 86400
        CHECK (kiro_cache_snapshot_ttl_seconds >= 1);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cache_snapshot_max_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (kiro_cache_snapshot_max_tokens >= 0);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_cache_snapshot_max_anchor_entries BIGINT NOT NULL DEFAULT 0
        CHECK (kiro_cache_snapshot_max_anchor_entries >= 0);
