-- Standalone-gateway image generation becomes default-on. The backfill flips
-- existing FALSE rows to TRUE so the rollout enables image generation for all
-- keys at once. This is safe here because codex_image_generation_enabled was
-- introduced in the same unreleased change set (migration 0030), so no
-- operator could yet have intentionally opted a key out. NOTE: if a future
-- release lands 0030 before 0032, narrow this backfill so it does not override
-- deliberate opt-outs.
ALTER TABLE IF EXISTS llm_key_route_config
    ALTER COLUMN codex_image_generation_enabled SET DEFAULT TRUE;

UPDATE llm_key_route_config
SET codex_image_generation_enabled = TRUE
WHERE codex_image_generation_enabled = FALSE;

-- The integrated Codex-API ("direct") path stays opt-in: default FALSE, gated
-- independently of the standalone switch above.
ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS codex_image_direct_generation_enabled BOOLEAN NOT NULL DEFAULT FALSE;
