# LLM Access Usage Analysis Query Design

## Goal

Upgrade the existing usage-event admin query path from a lightweight online page
viewer into a real analysis query interface that supports:

- full offset pagination over the analyzable usage window;
- rich filtering by key, time range, source, model, account, endpoint, and
  status code;
- full-result token sums that are computed over the entire filtered result set,
  not just the current page.

This design keeps existing route paths stable and does not introduce ad-hoc SQL
execution or free-form analytical expressions.

## Non-Goals

The first version does not add:

- arbitrary SQL;
- server-side group-by pivots;
- CSV export;
- saved filter presets;
- multi-node distributed analytics.

The first version also does not change the event schema itself.

## Problem Statement

The current usage query path is still shaped like an online diagnostics view:

- the API layer clamps pagination aggressively;
- the DuckDB analytics store also clamps online pages;
- the frontend mirrors those same limits;
- the response only returns page rows and does not return full filtered sums.

That makes the usage page unsuitable for analyzing historical windows, model
mix, or token composition for one key or one time range.

## Stable API Contract

Keep the existing routes:

- `GET /admin/llm-gateway/usage`
- `GET /admin/kiro-gateway/usage`
- `GET /admin/llm-gateway/usage/:event_id`
- `GET /admin/kiro-gateway/usage/:event_id`

Extend the list query contract with optional filters:

- `key_id`
- `start_ms`
- `end_ms`
- `source`
- `model`
- `account_name`
- `endpoint`
- `status_code`
- `limit`
- `offset`

Extend the list response with a new `totals` object:

- `event_count`
- `input_uncached_tokens`
- `input_cached_tokens`
- `output_tokens`
- `billable_tokens`

`total` remains the total matching row count for pagination. `totals.event_count`
must match `total`.

## Query Semantics

### Pagination

- There is no global offset cap.
- `limit` remains bounded to a reasonable page size to protect the online path.
- The server computes `count(*)` for the full filtered result, then returns
  `offset + limit` rows in newest-first order.

### Filtering

All filters are conjunctive:

- `key_id` exact match
- `provider_type` exact match from route semantics
- `start_ms` inclusive
- `end_ms` exclusive
- `source` in `hot | archive | all`
- `model` exact match
- `account_name` exact match
- `endpoint` exact match
- `status_code` exact match

The first version should keep exact-match filters for structured fields. It
should not add fuzzy or substring search at the storage layer.

### Totals

The backend returns a totals block computed from the full filtered result set,
not from the current page:

- `sum(input_uncached_tokens)`
- `sum(input_cached_tokens)`
- `sum(output_tokens)`
- `sum(billable_tokens)`
- `count(*)`

This is required so the UI can answer "how much token usage happened in this
window" without downloading all rows.

## Storage Strategy

### Core Principle

The online query path must still prune aggressively before opening archived
segments.

### Catalog-first pruning

The worker already maintains:

- archived segment catalog rows;
- per-segment key rollups.

The new analysis query path should continue to use the catalog first:

1. apply time-range pruning against segment start/end;
2. apply key/provider pruning against segment rollups where available;
3. open only active DuckDB and the archived segments that can still match;
4. run row-level filtering only inside the reduced candidate set.

This keeps the query scalable without attaching and scanning every archive file.

### Row-level filters

The segment catalog can only prune on fields it already knows. The new filters
`model`, `account_name`, `endpoint`, and `status_code` are row-level filters in
the first version.

That means:

- catalog pruning still runs first on time/key/provider;
- row-level filters are applied when scanning candidate active/archive files;
- correctness is preserved even when catalog cannot fully answer the filter.

### Why not attach every segment?

DuckDB can attach multiple databases, but the current production design avoids
using one giant attached query for online admin traffic because:

- attach-and-scan-all-segments grows planning and I/O cost with archive count;
- catalog pruning is already available and cheaper;
- the usage page is still an online admin path, not a free-form warehouse.

The analysis query should therefore stay on top of catalog-pruned candidate
segments rather than become a global attached scan.

## Backend Shape

### Core store types

Extend `UsageEventQuery` with:

- `model: Option<String>`
- `account_name: Option<String>`
- `endpoint: Option<String>`
- `status_code: Option<i32>`

Add a new aggregate struct:

- `UsageEventTotals`

Extend `UsageEventPage` with:

- `totals: UsageEventTotals`

### Store behavior

The DuckDB analytics store should expose one logical query that returns both:

- the paginated rows;
- the full filtered totals.

Internally it may issue separate count/page/sum work, but callers should receive
one coherent `UsageEventPage`.

### Worker/API behavior

The worker remains the query truth source. The API service continues to proxy
the same routes through `usage_query_base_url`.

No new public write behavior is introduced.

## Frontend Behavior

### Filter panel

The admin usage page should gain a dedicated filter panel with at least:

- key selector
- time range selector
- source selector
- model text/select filter
- account text/select filter
- endpoint select filter
- status code select/input

The design target is a dense operations filter bar, not a decorative card UI.

### Summary strip

Above the table, show:

- total matching events
- total uncached input tokens
- total cached input tokens
- total output tokens
- total billable tokens

These numbers always reflect the full filtered result set, not the current page.

### Pagination

- changing any filter resets to page 1;
- total pages are derived from `total / limit`;
- no artificial 200-offset or 11-page cap remains in admin or public usage
  query pages.

## Performance Expectations

The design intentionally accepts that:

- deep offset pagination is still offset pagination;
- very large offsets will eventually cost more than cursor/seek pagination.

That is acceptable for this first version. The immediate correctness target is:

- complete filtered totals;
- complete pagination across the analyzable window;
- catalog-pruned archive access.

If deep-page cost later becomes a real operational problem, the next step is a
cursor/seek contract, not another artificial clamp.

## Testing Requirements

The implementation must add coverage for:

1. query normalization for new filters and unbounded offset behavior;
2. DuckDB list queries with offsets beyond 200;
3. totals correctness for filtered results across active and archived segments;
4. catalog-pruned archive discovery with key/provider filtering intact;
5. frontend response parsing for the new `totals` payload;
6. frontend pagination behavior without hard-coded page caps.

## Risks

### Catalog mismatch risk

If a future filter is not representable in catalog metadata, pruning must remain
conservative. It is acceptable to scan more candidate segments; it is not
acceptable to prune a segment that still contains matching rows.

### Online query cost risk

Rich filters and totals introduce more work than the old sample page. This is
why page size stays bounded and catalog pruning remains mandatory.

### Compatibility risk

Routes stay stable, but response JSON changes by adding `totals`. Existing
frontend code must be updated in lockstep. Additive response fields preserve
backward compatibility for other consumers.
