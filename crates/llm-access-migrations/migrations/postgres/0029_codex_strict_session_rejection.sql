ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS codex_strict_session_rejection_enabled BOOLEAN NOT NULL DEFAULT FALSE;
