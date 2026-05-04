# StaticFlow Self-Hosted systemd Templates

For the end-to-end setup guide, start here:

- [../../docs/self-hosted-systemd-quick-start.zh.md](../../docs/self-hosted-systemd-quick-start.zh.md)

This directory is the template/reference layer. These files keep the gateway
and backend slots under `systemd` while still using the repository scripts as
the single operational entrypoints.

Files:

- `staticflow-gateway.service.template`
- `staticflow-backend-slot@.service.template`
- `staticflow-common.env.example`
- `staticflow-gateway.env.example`
- `staticflow-backend-slot.env.example`
- `llm-access.service.template`
- `llm-access-juicefs.mount.template`
- `juicefs-llm-access.resource-guard.conf`
- `staticflow-wait-llm-access-state`

Suggested workflow:

1. Prepare a release bundle directory:
   `./scripts/prepare_selfhosted_systemd_bundle.sh --output-dir /opt/staticflow/releases/current`
2. Copy the example env files to your target host paths and edit them.
3. Render units:
   `./scripts/render_selfhosted_systemd_units.sh --unit-dir /etc/systemd/system --workdir /opt/staticflow/current --common-env /etc/staticflow/selfhosted/common.env --gateway-env /etc/staticflow/selfhosted/gateway.env --backend-env-pattern /etc/staticflow/selfhosted/backend-slot-%i.env`
4. Reload and start services:
   `sudo systemctl daemon-reload`
   `sudo systemctl enable --now staticflow-backend-slot@blue.service staticflow-backend-slot@green.service staticflow-gateway.service`

Runtime operations stay script-driven:

- System-scope summary and health:
  `SYSTEMD_SCOPE=system CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml ./scripts/pingora_gateway.sh status`
  `SYSTEMD_SCOPE=system CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml ./scripts/pingora_gateway.sh health`
- Gateway lifecycle and cutover:
  `SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh start`
  `SYSTEMD_SCOPE=system CONF_FILE=/etc/staticflow/selfhosted/pingora-gateway.yaml ./scripts/pingora_gateway.sh switch green`
- Backend slot lifecycle:
  `SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh start-backend blue`
  `SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh restart-backend green`
- Journal logs:
  `SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh logs gateway --lines 200`
  `SYSTEMD_SCOPE=system ./scripts/pingora_gateway.sh logs green --follow`

Validation:

- `./scripts/test_selfhosted_systemd_stack.sh`

## Cloud llm-access Templates

Current production routes LLM paths on the GCP edge directly to standalone
`llm-access` on `127.0.0.1:19080`; non-LLM StaticFlow paths still use
pb-mapper back to the local Pingora gateway. The `llm-access*` templates in
this directory describe that cloud-side service shape.

Storage model:

- state root: `/mnt/llm-access`
- SQLite control DB: `/mnt/llm-access/control/llm-access.sqlite3`
- active mutable DuckDB dir: `/var/lib/staticflow/llm-access/analytics-active`
- GeoIP MMDB cache: `/var/lib/staticflow/llm-access/geoip/GeoLite2-City.mmdb`
- archived DuckDB segments: `/mnt/llm-access/analytics/segments`
- DuckDB segment catalog: `/mnt/llm-access/analytics/catalog`
- JuiceFS cache dir: `/var/cache/juicefs/llm-access`

The production JuiceFS volume is backed by Cloudflare R2 object storage and
external Valkey metadata. Credentials belong in ignored private env files, not
in these templates. `llm-access.service` must be a single writer for the
SQLite/DuckDB/auth tree.
The GeoIP MMDB is a rebuildable local cache and should stay on the VM block
disk, not under `/mnt/llm-access`.

Bundle rendering and template validation:

- `./scripts/render_llm_access_cloud_bundle.sh /tmp/llm-access-cloud-bundle`
- `./scripts/test_llm_access_cloud_bundle.sh`
