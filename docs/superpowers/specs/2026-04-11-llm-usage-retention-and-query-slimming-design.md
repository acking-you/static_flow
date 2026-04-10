# LLM Usage Retention And Query Slimming Design

## Goal

Fix the current admin-side LLM/Kiro usage-memory blow-up without breaking
historical quota accounting.

The design must satisfy four constraints:

1. Keep `llm_gateway_usage_events` as the source of truth for startup rollup
   rebuilds.
2. Stop list views from loading large request-body fields on the hot path.
3. Add periodic maintenance that trims old diagnostic detail while preserving
   summary history.
4. Ensure `usage_events` scalar indices are periodically optimized instead of
   relying on compaction alone.

## Current Problem

`llm_gateway_usage_events` stores both summary billing fields and heavy request
diagnostics in the same row. The current list APIs query the full row shape,
including:

- `request_headers_json`
- `last_message_content`
- `client_request_body_json`
- `upstream_request_body_json`
- `full_request_json`

The shared store query path always selects `usage_event_columns()` for list
queries, which means admin paging and key-filtered usage searches materialize
large JSON fields even when the UI only needs summary fields. Kiro usage is
especially expensive because new Kiro rows persist `full_request_json`
unconditionally.

At the same time, backend startup still rebuilds in-memory per-key usage
rollups and event counters from the full `llm_gateway_usage_events` table, so
blind row deletion would destroy historical usage totals.

## Non-Goals

- Do not split usage data into a new summary/detail table pair in this change.
- Do not change historical usage/quota semantics.
- Do not add a Kiro-only maintenance path.
- Do not keep the current list APIs returning full request bodies “for
  compatibility”; that would preserve the hot-path bug.

## Design

### 1. Keep One Source Table, But Separate Summary And Detail At The API Layer

`llm_gateway_usage_events` remains the single persisted table.

The system will instead define two projections:

- `usage summary`
- `usage detail`

#### Usage summary fields

- `id`
- `key_id`
- `key_name`
- `provider_type`
- `account_name`
- `request_method`
- `request_url`
- `latency_ms`
- `endpoint`
- `model`
- `status_code`
- `input_uncached_tokens`
- `input_cached_tokens`
- `output_tokens`
- `billable_tokens`
- `usage_missing`
- `credit_usage`
- `credit_usage_missing`
- `client_ip`
- `ip_region`
- `created_at`

#### Usage detail fields

- `request_headers_json`
- `last_message_content`
- `client_request_body_json`
- `upstream_request_body_json`
- `full_request_json`

The list endpoints will only return summary fields. The detail endpoints will
return the full summary row plus the five detail fields for a single `event_id`.

This keeps the data model simple while removing large JSON from the paging
path.

### 2. Add Usage Maintenance Runtime Settings Under LLM Gateway Runtime Config

The new settings belong in `LlmGatewayRuntimeConfig`, not in global compaction
config, because they are specific to LLM gateway usage semantics.

Add these persisted runtime-config fields:

- `usage_event_maintenance_enabled: bool`
- `usage_event_maintenance_interval_seconds: u64`
- `usage_event_detail_retention_days: i64`

Defaults:

- `usage_event_maintenance_enabled = true`
- `usage_event_maintenance_interval_seconds = 3600`
- `usage_event_detail_retention_days = -1`

Semantics:

- `enabled = false`: disable the dedicated usage maintenance loop entirely.
- `interval_seconds`: how often the usage maintenance loop runs.
- `detail_retention_days = -1`: keep usage detail indefinitely.
- `detail_retention_days = 1..3650`: preserve only the most recent N days of
  detail payloads while keeping summary fields and rows.

### 3. Add A Dedicated Usage Maintenance Loop

Create a dedicated maintenance loop in `LlmGatewayRuntimeState`, separate from
the generic table compactor.

Each maintenance tick performs:

1. Detail trimming
2. Usage-index optimization

#### 3.1 Detail trimming

If `usage_event_detail_retention_days > 0`, compute a cutoff timestamp and run
an in-place filtered update on `llm_gateway_usage_events` that sets the detail
columns to `NULL` for rows older than the cutoff.

Columns to clear:

- `request_headers_json`
- `last_message_content`
- `client_request_body_json`
- `upstream_request_body_json`
- `full_request_json`

This preserves:

- usage rows
- startup rollup rebuild input
- historical token/credit totals
- per-key event counts

It only removes old diagnostic detail.

#### 3.2 Index optimization

Every maintenance tick will also run `OptimizeAction::Index` for
`llm_gateway_usage_events`.

This is required because the existing background compactor only performs
`Compact + Prune`. It does not optimize indices for append-heavy usage data.

The new maintenance loop will therefore own periodic index optimization for
usage events.

### 4. Replace Full-Row List Queries With Summary Queries

Add store-layer summary query helpers that select only the summary projection.

Do not reuse `usage_event_columns()` for list endpoints.

Required store operations:

- count usage events
- query usage summary rows with optional `key_id`
- query usage summary rows with optional `provider_type`
- fetch one usage-detail row by `event_id`

The existing aggregation code for startup rollups and event counts remains
unchanged because it reads directly from the dataset with SQL aggregation.

### 5. Admin API Changes

#### LLM gateway

Replace the current admin usage list payload with summary rows only.

Add a new admin detail endpoint by `event_id`.

List path:

- `GET /api/admin/llm-gateway/usage`

Detail path:

- `GET /api/admin/llm-gateway/usage/:event_id`

#### Kiro gateway

Apply the same summary/detail split to the Kiro admin usage path.

List path:

- `GET /api/admin/kiro-gateway/usage`

Detail path:

- `GET /api/admin/kiro-gateway/usage/:event_id`

Kiro usage list remains filtered to provider `kiro`, but it must also return
summary only.

### 6. Frontend Behavior Changes

#### `/admin/llm-gateway`

- Keep the usage list UI, paging, and key filter.
- Change the list fetch to consume summary rows.
- Change “view details / full request” to lazy-load detail by `event_id`.

#### `/admin/kiro-gateway`

- Remove the current page-load usage prefetch.
- Only fetch Kiro usage summary when the user actually opens the usage section.
- Detail modal uses the new single-event detail endpoint.

This removes the current behavior where entering `/admin/kiro-gateway` causes a
heavy usage query before the user explicitly asks for it.

## Validation Rules

Runtime-config validation:

- `usage_event_maintenance_interval_seconds` must be within a bounded positive
  range.
- `usage_event_detail_retention_days` must be `-1` or within `1..3650`.

Store-side maintenance logic:

- detail trimming must only target rows older than the cutoff
- already-null detail fields remain null
- summary columns must never be modified by the trimming path

## Compatibility

Preserved:

- historical usage rows
- startup rebuild of usage rollups
- startup rebuild of usage event counts
- existing usage totals shown on keys

Changed intentionally:

- list endpoints no longer include heavy request-body fields
- detail must now be fetched on demand by `event_id`
- old rows outside the retention window will show empty detail payloads

## Risks

### Runtime-config drift

The new settings touch backend runtime state, persisted runtime-config rows, and
frontend admin forms. All three representations must stay aligned.

### Over-aggressive maintenance interval

If index optimization is run too frequently, it can create unnecessary write
churn. The default should be conservative, and the admin setting should remain
explicitly configurable.

### False fix from retention alone

Retention helps long-term table growth, but it does not fix hot-path memory on
its own. The summary/detail query split is the primary runtime fix and must be
implemented in the same change.

## Testing

### Backend store tests

- summary query only returns summary fields
- detail lookup returns heavy fields for one event
- trimming old detail sets only the five detail columns to null
- trimming old detail preserves token/credit/count fields

### Backend runtime/config tests

- runtime-config parsing/defaults include the new maintenance fields
- invalid maintenance interval is rejected
- invalid retention days are rejected

### Backend handler tests

- admin list endpoints return summary rows only
- admin detail endpoints return one event detail row
- Kiro usage list endpoint does not expose heavy fields

### Frontend tests

- `/admin/llm-gateway` usage modal lazy-loads detail
- `/admin/kiro-gateway` does not prefetch usage on initial page load
- retention settings round-trip in runtime-config forms

## Implementation Notes

The change should be executed in this order:

1. store projections and detail lookup
2. admin handler split for summary/detail
3. frontend lazy detail loading and Kiro prefetch removal
4. runtime-config persistence and admin form fields
5. dedicated usage maintenance loop
6. tests and verification
