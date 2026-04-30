CREATE TABLE IF NOT EXISTS cdc_outbox (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE,
    source_instance TEXT NOT NULL,
    entity TEXT NOT NULL CHECK (entity IN (
        'key',
        'runtime_config',
        'account_group',
        'proxy_config',
        'proxy_binding',
        'token_request',
        'account_contribution_request',
        'gpt2api_account_contribution_request',
        'sponsor_request',
        'usage_event'
    )),
    op TEXT NOT NULL CHECK (op IN ('append', 'upsert', 'delete')),
    primary_key TEXT NOT NULL,
    schema_version INTEGER NOT NULL CHECK (schema_version >= 1),
    payload_json TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at_ms INTEGER NOT NULL CHECK (created_at_ms >= 0),
    committed_at_ms INTEGER NOT NULL CHECK (committed_at_ms >= 0)
) STRICT;

CREATE INDEX IF NOT EXISTS idx_cdc_outbox_entity_seq
    ON cdc_outbox(entity, seq);
