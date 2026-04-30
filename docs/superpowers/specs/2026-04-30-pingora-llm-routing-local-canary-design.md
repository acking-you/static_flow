# Pingora LLM Routing Local Canary Design

## Background

The first `llm-access` migration target should be local validation, not cloud
storage. JuiceFS can wait. The immediate goal is to prove that LLM traffic can
be split away from the normal StaticFlow backend through the existing
Pingora-based gateway, while the rest of StaticFlow keeps using the current
blue/green backend path.

The current local production path is:

```text
cloud Caddy/pb-mapper -> local Pingora gateway 127.0.0.1:39180
  -> active StaticFlow backend slot 127.0.0.1:39080 or 127.0.0.1:39081
```

This gateway already supports in-process config reload for `active_upstream`.
The new routing capability should reuse that control plane instead of adding a
separate proxy layer.

## Goals

- Run `llm-access` locally on `127.0.0.1:19080`.
- Store local `llm-access` state on the large `/mnt` data disk, not repo `tmp`
  or the system disk.
- Let Pingora split only selected LLM requests to `llm-access`.
- Keep default behavior unchanged when LLM routing is disabled.
- Support quick rollback by config reload.
- Avoid exposing plaintext API keys in gateway config.

## Non-Goals

- Do not move traffic to GCP in this step.
- Do not introduce JuiceFS in this step.
- Do not switch all production LLM traffic before local canary passes.
- Do not run two writers against the same SQLite/DuckDB target.
- Do not run a second StaticFlow backend against the same LanceDB production
  root unless it is explicitly read-only or otherwise proven safe.

## Local `llm-access` State

Use this local state root:

```text
/mnt/wsl/data4tb/static-flow-data/llm-access-local
  /control/llm-access.sqlite3
  /analytics/usage.duckdb
  /auths/kiro
  /auths/codex
  /cdc
  /logs
```

Example startup:

```bash
llm-access serve \
  --bind 127.0.0.1:19080 \
  --state-root /mnt/wsl/data4tb/static-flow-data/llm-access-local \
  --sqlite-control /mnt/wsl/data4tb/static-flow-data/llm-access-local/control/llm-access.sqlite3 \
  --duckdb /mnt/wsl/data4tb/static-flow-data/llm-access-local/analytics/usage.duckdb
```

The service should fail if required state paths are outside `--state-root`.
This keeps later GCP migration simple: the state root can be copied or mounted
as a unit.

## Gateway Config Shape

Extend `staticflow` config with an optional LLM routing section:

```yaml
staticflow:
  upstreams:
    blue: 127.0.0.1:39080
    green: 127.0.0.1:39081
    llm_access_local: 127.0.0.1:19080
  active_upstream: green

  llm_routing:
    enabled: false
    upstream: llm_access_local
    path_prefixes:
      - /v1/
      - /cc/v1/
      - /api/llm-gateway/
      - /api/kiro-gateway/
      - /api/codex-gateway/
      - /api/llm-access/
    bearer_token_sha256_allowlist: []
```

When `llm_routing` is absent or `enabled: false`, the gateway behavior remains
exactly the current blue/green behavior.

When enabled, a request is routed to `llm_access_local` only if:

1. Its path starts with one of `path_prefixes`.
2. Its bearer token SHA-256 hex digest is present in
   `bearer_token_sha256_allowlist`.

If either condition fails, it falls back to the active StaticFlow backend.

## Routing Algorithm

For each request:

1. Take a fresh config snapshot in `request_filter`, matching the current
   reload behavior.
2. Capture method, path, request id, trace id, and remote address as today.
3. Evaluate LLM routing:
   - If disabled, choose `active_upstream`.
   - If path does not match, choose `active_upstream`.
   - If token is absent, malformed, or not allowlisted, choose
     `active_upstream`.
   - If path and token match, choose `llm_routing.upstream`.
4. Store selected upstream name, address, and reason in request context.
5. `upstream_peer` connects to the selected upstream.

This keeps routing decisions per-request and reloadable without stopping the
gateway.

## Token Matching

The gateway should never store plaintext API keys in YAML.

Supported input:

```http
Authorization: Bearer <secret>
```

The gateway computes:

```text
sha256(<secret>) as lowercase hex
```

Then it compares the digest against `bearer_token_sha256_allowlist`.

This is enough for local canary and can later be extended with header matching,
percentage rollout, or per-path upstream selection. Those are intentionally out
of scope for the first implementation.

## Access Logging

Add fields to gateway access logs:

```text
selected_upstream=<blue|green|llm_access_local>
selected_upstream_addr=<host:port>
route_target=<staticflow|llm_access>
route_reason=<disabled|path_miss|missing_bearer|token_miss|token_match>
```

The existing `active_upstream` field can remain for compatibility, but logs
must make the actual selected target visible. This matters during canary
because `active_upstream=green` may be true while the selected upstream is
`llm_access_local`.

## Local Test Flow

1. Start `llm-access` locally on `127.0.0.1:19080` using the `/mnt` state root.
2. Add `llm_access_local` to gateway config with `llm_routing.enabled: false`.
3. Reload gateway and confirm all requests still go to the active backend.
4. Add one test key hash to `bearer_token_sha256_allowlist` and enable routing.
5. Reload gateway.
6. Verify:
   - Test-key LLM requests route to `llm_access_local`.
   - Non-test-key LLM requests still route to active StaticFlow backend.
   - Non-LLM paths such as `/api/articles` still route to active StaticFlow
     backend.
7. Roll back by setting `llm_routing.enabled: false` and reloading.

## Temporary Separate `sf-backend` Option

A second backend can be useful as a temporary target before the real provider
runtime is fully moved into `llm-access`, but it must not write to the same
production LanceDB root as the live backend.

Allowed safe uses:

- Use an independent `DB_ROOT`.
- Use test-only keys and test-only state.
- Use it as a route-shape compatibility target, not as the final architecture.

The long-term split still moves LLM access into `llm-access`, not another full
StaticFlow backend.

## Tests

Gateway config tests should cover:

- Existing config without `llm_routing` still parses and behaves as before.
- `llm_routing.enabled: false` never selects the LLM upstream.
- Enabled routing requires the configured upstream to exist.
- Matching path + matching token selects `llm_access_local`.
- Matching path + missing or non-matching token falls back to active backend.
- Non-LLM path always falls back to active backend.

Proxy tests should cover request-context selected upstream metadata so access
logs and `upstream_peer` use the same decision.

## Rollback

Rollback is config-only:

```yaml
staticflow:
  llm_routing:
    enabled: false
```

After reload, all traffic returns to the current active StaticFlow backend. No
process restart and no Caddy change should be required for local rollback.

## Implementation Boundary

The next implementation should only add the routing control plane and local
canary mechanics. It should not yet move the full Kiro/Codex runtime into
`llm-access`. Provider runtime migration is the next step after gateway routing
is proven stable.
