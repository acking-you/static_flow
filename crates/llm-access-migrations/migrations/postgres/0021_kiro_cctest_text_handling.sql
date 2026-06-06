ALTER TABLE llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_cctest_text_handling_enabled BOOLEAN NOT NULL DEFAULT FALSE;
