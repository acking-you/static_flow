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
