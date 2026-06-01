ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_remote_media_resolution_enabled BOOLEAN NOT NULL DEFAULT FALSE;
