CREATE TABLE IF NOT EXISTS llm_codex_status_cache (
    id TEXT PRIMARY KEY CHECK (id = 'default'),
    snapshot_json TEXT NOT NULL CHECK (json_valid(snapshot_json)),
    updated_at_ms INTEGER NOT NULL CHECK (updated_at_ms >= 0)
) STRICT, WITHOUT ROWID;
