ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS kiro_context_usage_min_request_tokens BIGINT NOT NULL DEFAULT 15000
        CHECK (kiro_context_usage_min_request_tokens >= 1);
