ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_session_affinity_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_session_affinity_max_entries BIGINT NOT NULL DEFAULT 20000
        CHECK (codex_session_affinity_max_entries >= 0);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_session_affinity_ttl_seconds BIGINT NOT NULL DEFAULT 21600
        CHECK (codex_session_affinity_ttl_seconds >= 0);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_fallback_affinity_enabled BOOLEAN NOT NULL DEFAULT TRUE;

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_fallback_affinity_ttl_seconds BIGINT NOT NULL DEFAULT 1800
        CHECK (codex_fallback_affinity_ttl_seconds >= 0);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_fallback_affinity_prefix_bytes BIGINT NOT NULL DEFAULT 4096
        CHECK (codex_fallback_affinity_prefix_bytes >= 0);

ALTER TABLE IF EXISTS llm_runtime_config
    ADD COLUMN IF NOT EXISTS codex_fallback_affinity_min_body_bytes BIGINT NOT NULL DEFAULT 128
        CHECK (codex_fallback_affinity_min_body_bytes >= 0);
