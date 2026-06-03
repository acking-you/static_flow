ALTER TABLE llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_protected_content_validation_enabled BOOLEAN NOT NULL DEFAULT FALSE;
