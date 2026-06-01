ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_compact_trigger_tokens BIGINT NOT NULL DEFAULT 780000
        CHECK (kiro_compact_trigger_tokens >= 0);
