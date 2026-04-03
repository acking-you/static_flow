# LanceDB Memory And Polling Outage Runbook

## Scope

This runbook is for the planned maintenance window that:

- deploys the new backend/frontend/runtime-config behavior
- rebuilds the two hot event tables into stable-row-id layouts
- verifies storage shrinkage and post-maintenance health

Canonical data root:

```bash
/mnt/wsl/data4tb/static-flow-data
```

Content DB:

```bash
/mnt/wsl/data4tb/static-flow-data/lancedb
```

## Preconditions

1. Stop the backend so no writers are appending to the event tables.
2. Confirm the new binaries are built from this branch:

```bash
cargo build -p sf-cli -p static-flow-backend
```

3. Use the freshly built CLI from this checkout:

```bash
target/debug/sf-cli
```

## Optional Backup

If you want a fast rollback point, copy the two hot tables before rebuild:

```bash
cp -a /mnt/wsl/data4tb/static-flow-data/lancedb/llm_gateway_usage_events.lance \
  /mnt/wsl/data4tb/static-flow-data/lancedb/llm_gateway_usage_events.lance.bak.$(date +%Y%m%d-%H%M%S)
cp -a /mnt/wsl/data4tb/static-flow-data/lancedb/api_behavior_events.lance \
  /mnt/wsl/data4tb/static-flow-data/lancedb/api_behavior_events.lance.bak.$(date +%Y%m%d-%H%M%S)
```

## Pre-Rebuild Audit

Record the current storage state before touching the tables:

```bash
target/debug/sf-cli db audit-storage \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table llm_gateway_usage_events

target/debug/sf-cli db audit-storage \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table api_behavior_events
```

## Table Rebuild

Rebuild both hot append-only tables with stable row IDs:

```bash
target/debug/sf-cli db rebuild-table-stable \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table llm_gateway_usage_events \
  --force \
  --batch-size 256

target/debug/sf-cli db rebuild-table-stable \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table api_behavior_events \
  --force \
  --batch-size 256
```

## Post-Rebuild Audit

Verify that both tables now report the expected healthier layout:

```bash
target/debug/sf-cli db audit-storage \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table llm_gateway_usage_events

target/debug/sf-cli db audit-storage \
  --db-path /mnt/wsl/data4tb/static-flow-data/lancedb \
  --table api_behavior_events
```

Expected signals:

- `stable_row_ids=true`
- `_versions/` size is materially lower than before rebuild
- fragment counts are reset to a reasonable baseline

## Restart And Verify

1. Start the upgraded backend binary.
2. Verify the admin runtime config page shows the new controls:
   - Codex refresh min/max
   - Codex account jitter max
   - Kiro refresh min/max
   - Kiro account jitter max
   - usage flush batch / interval / max buffer bytes
3. Verify the default values are:
   - polling window: `240-300` seconds
   - per-account jitter max: `10` seconds
   - usage flush: `256` rows / `15` seconds / `8388608` bytes
4. Verify compaction defaults after restart:
   - scan interval: `900` seconds
   - fragment threshold: `128`
   - prune older than: `1` hour

## Post-Maintenance Watch Items

Watch the first 30 to 60 minutes for:

- RSS no longer climbing with the previous slope
- `llm_gateway_usage_events` and `api_behavior_events` version growth slowing materially
- usage/admin pages no longer triggering heavy `count_rows` scans
- Codex and Kiro polling spreading over time instead of synchronizing on fixed one-minute bursts
