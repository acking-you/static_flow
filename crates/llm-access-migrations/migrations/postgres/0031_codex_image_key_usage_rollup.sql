ALTER TABLE IF EXISTS llm_key_usage_rollups
    ADD COLUMN IF NOT EXISTS codex_image_usage_tokens BIGINT NOT NULL DEFAULT 0
        CHECK (codex_image_usage_tokens >= 0);

ALTER TABLE IF EXISTS llm_key_usage_rollups
    ADD COLUMN IF NOT EXISTS codex_image_usage_missing_events BIGINT NOT NULL DEFAULT 0
        CHECK (codex_image_usage_missing_events >= 0);

ALTER TABLE IF EXISTS llm_key_usage_rollups
    ADD COLUMN IF NOT EXISTS codex_image_last_used_at_ms BIGINT;
