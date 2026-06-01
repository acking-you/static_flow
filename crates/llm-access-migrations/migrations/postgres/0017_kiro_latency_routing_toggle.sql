ALTER TABLE IF EXISTS llm_key_route_config
    ADD COLUMN IF NOT EXISTS kiro_latency_routing_enabled BOOLEAN NOT NULL DEFAULT TRUE;
