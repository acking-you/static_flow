ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS codex_image_generation_enabled BOOLEAN NOT NULL DEFAULT FALSE;
