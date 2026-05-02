ALTER TABLE llm_key_route_config
ADD COLUMN kiro_full_request_logging_enabled INTEGER NOT NULL DEFAULT 0 CHECK (
    kiro_full_request_logging_enabled IN (0, 1)
);
