ALTER TABLE IF EXISTS llm_key_route_config
    ALTER COLUMN codex_image_generation_enabled SET DEFAULT TRUE;

UPDATE llm_key_route_config
SET codex_image_generation_enabled = TRUE
WHERE codex_image_generation_enabled = FALSE;

ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS codex_image_direct_generation_enabled BOOLEAN NOT NULL DEFAULT FALSE;
