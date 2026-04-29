# Cloud Ingress Relay and pb-mapper Runbook

这份 runbook 记录 2026-04-30 迁移 `ackingliu.top` 云入口时的实际教训。目标场景是：公网云机只负责 Caddy TLS 入口和 pb-mapper 中继，本地机器继续承载真正的 StaticFlow backend、Pingora gateway、LanceDB 数据。

## 1. 当前链路

```text
public client
  -> https://ackingliu.top
  -> cloud Caddy :443
  -> cloud pb-mapper-client-cli 127.0.0.1:39080
  -> cloud pb-mapper-server :7666
  -> local pb-mapper-server-cli
  -> local Pingora gateway 127.0.0.1:39180
  -> active StaticFlow backend slot
```

本地稳定入口是 `127.0.0.1:39180`。云端 `127.0.0.1:39080` 只是 pb-mapper client 在云机上暴露出来的本地端口，供 Caddy reverse proxy 使用。

## 2. 云机预检

先确认架构、sudo、端口和安全组。Azure 免费/学生机常见是 ARM64，不要直接复制旧 x86_64 二进制。

```bash
ssh azureuser@4.193.216.253 'uname -m; dpkg --print-architecture; sudo -n true && echo sudo_ok'
ssh azureuser@4.193.216.253 'sudo ss -lntup | grep -E ":(80|443|39080|7666)\b" || true'
```

云安全组必须放行：

- `80/tcp`：Caddy HTTP-01 / HTTP redirect
- `443/tcp`：HTTPS
- `7666/tcp`：local side 的 `pb-mapper-server-cli` 注册到云端

`39080` 只绑定 `127.0.0.1`，不要公网暴露。

## 3. Caddy 部署

安装官方 Caddy 包即可。Caddy 自带 Automatic HTTPS，不需要额外 certbot timer。

```bash
sudo apt-get update -y
sudo apt-get install -y debian-keyring debian-archive-keyring apt-transport-https curl ca-certificates gnupg
curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/gpg.key |
  sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt |
  sudo tee /etc/apt/sources.list.d/caddy-stable.list >/dev/null
sudo apt-get update -y
sudo apt-get install -y caddy
```

`/etc/caddy/Caddyfile` 的关键形状：

```caddyfile
{
    email admin@ackingliu.top
    servers {
        protocols h1 h2
    }
}

ackingliu.top, www.ackingliu.top {
    @health path /_caddy_health
    respond @health "ok" 200

    @admin path /admin*
    respond @admin "forbidden" 403

    reverse_proxy 127.0.0.1:39080 {
        header_up X-Real-IP {remote_host}
        header_up X-Forwarded-For {remote_host}
        header_up X-Forwarded-Proto {scheme}
        header_up X-Forwarded-Host {host}
    }
}
```

验证 Caddy 是否启用自动证书管理：

```bash
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl restart caddy
sudo journalctl -u caddy -n 120 --no-pager -l |
  grep -Ei 'certificate|automatic TLS|maintenance|storage'
```

正常日志会出现：

- `started background certificate maintenance`
- `enabling automatic TLS certificate management`

如果域名还没切到新云机，可以先迁移旧机 `/var/lib/caddy/.local/share/caddy` 里的证书 storage。正式续期仍要求 `ackingliu.top` 和 `www.ackingliu.top` 最终解析到这台云机，且公网 `80/443` 可达。

## 4. pb-mapper 二进制部署

优先确认 release 里有没有目标架构产物。如果 ARM64 Linux 没有发布包，就在目标云机原生编译最新源码或指定 tag。

```bash
sudo apt-get install -y build-essential pkg-config git curl ca-certificates
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
  sh -s -- -y --profile minimal

rm -rf /tmp/pb-mapper-src
git clone --depth 1 https://github.com/acking-you/pb-mapper.git /tmp/pb-mapper-src
cd /tmp/pb-mapper-src
. "$HOME/.cargo/env"
cargo build --release \
  --bin pb-mapper-server \
  --bin pb-mapper-client-cli \
  --bin pb-mapper-server-cli \
  --jobs 1
```

安装到和现有运维脚本一致的路径：

```bash
sudo install -d /opt/pb-mapper-server /opt/pb-mapper-client-cli/current
sudo install -m 0755 /tmp/pb-mapper-src/target/release/pb-mapper-server \
  /opt/pb-mapper-server/pb-mapper-server
sudo install -m 0755 /tmp/pb-mapper-src/target/release/pb-mapper-server-cli \
  /opt/pb-mapper-server/pb-mapper-server-cli
sudo install -m 0755 /tmp/pb-mapper-src/target/release/pb-mapper-client-cli \
  /opt/pb-mapper-client-cli/current/pb-mapper-client-cli
sudo install -m 0755 /tmp/pb-mapper-src/target/release/pb-mapper-server-cli \
  /opt/pb-mapper-client-cli/current/pb-mapper-server-cli

file /opt/pb-mapper-server/pb-mapper-server
/opt/pb-mapper-server/pb-mapper-server --version
/opt/pb-mapper-client-cli/current/pb-mapper-client-cli --version
```

ARM64 云机上 `file` 必须显示 `ARM aarch64`。不要把 x86_64 旧机的 `/opt/pb-mapper-*` 二进制直接复制到 ARM64 机器。

## 5. MSG_HEADER_KEY 的坑

不要在迁移到新云机时继续使用 `--use-machine-msg-header-key` 并指望它和旧机器一致。这个参数会按当前机器 hostname/MAC 派生 key，并写入 `/var/lib/pb-mapper-server/msg_header_key`。换机器后派生出来的 key 必然不同，client/server 会报：

```text
datalen not valid
read checksum from network error
Connection reset by peer
```

如果要让新云机和旧 `ackingliu.top` 使用同一个 pb-mapper key，做法是把旧机 key 写成 root-only env，并用 systemd drop-in 覆盖掉 `--use-machine-msg-header-key`。

```bash
sudo install -d -m 700 /etc/pb-mapper /var/lib/pb-mapper-server
sudo install -m 600 /tmp/msg_header_key /var/lib/pb-mapper-server/msg_header_key
sudo sh -c 'printf "MSG_HEADER_KEY=%s\n" "$(tr -d "\r\n" </tmp/msg_header_key)" > /etc/pb-mapper/server.env'
sudo chmod 600 /etc/pb-mapper/server.env

sudo install -d /etc/systemd/system/pb-mapper-server.service.d
sudo tee /etc/systemd/system/pb-mapper-server.service.d/30-msg-header-key.conf >/dev/null <<'EOF'
[Service]
EnvironmentFile=/etc/pb-mapper/server.env
ExecStart=
ExecStart=/opt/pb-mapper-server/pb-mapper-server --pb-mapper-port 7666
EOF
```

client env 也必须使用同一个 key：

```bash
sudo install -d -m 700 /etc/pb-mapper/client-cli
sudo tee /etc/pb-mapper/client-cli/sf-backend.env >/dev/null <<'EOF'
PB_SERVER=127.0.0.1:7666
SERVICE_KEY=sf-backend
LOCAL_ADDR=127.0.0.1:39080
RUST_LOG=info
PB_MAPPER_KEEP_ALIVE=ON
MSG_HEADER_KEY=<same-32-byte-key>
PB_MAPPER_LOG_FORMAT=json
EOF
sudo chmod 600 /etc/pb-mapper/client-cli/sf-backend.env
```

校验时只比较 hash，不打印明文：

```bash
sudo sh -c 'tr -d "\r\n" </var/lib/pb-mapper-server/msg_header_key | sha256sum'
sudo sh -c 'sed -n "s/^MSG_HEADER_KEY=//p" /etc/pb-mapper/server.env | tr -d "\r\n" | sha256sum'
sudo sh -c 'sed -n "s/^MSG_HEADER_KEY=//p" /etc/pb-mapper/client-cli/sf-backend.env | tr -d "\r\n" | sha256sum'
```

三者必须一致。

## 6. systemd units

`/etc/systemd/system/pb-mapper-server.service`：

```ini
[Unit]
Description=pb-mapper server
After=network.target

[Service]
Type=simple
WorkingDirectory=/opt/pb-mapper-server
ExecStart=/opt/pb-mapper-server/pb-mapper-server --pb-mapper-port 7666
Environment=RUST_LOG=info
Restart=on-failure
RestartSec=3
LimitNOFILE=65535

[Install]
WantedBy=multi-user.target
```

推荐 drop-ins：

```ini
# /etc/systemd/system/pb-mapper-server.service.d/10-log-format.conf
[Service]
Environment=PB_MAPPER_LOG_FORMAT=json

# /etc/systemd/system/pb-mapper-server.service.d/20-stream-timeouts.conf
[Service]
Environment=PB_MAPPER_STREAM_ACK_TIMEOUT=3s
Environment=PB_MAPPER_STREAM_READY_TIMEOUT=5s

# /etc/systemd/system/pb-mapper-server.service.d/30-msg-header-key.conf
[Service]
EnvironmentFile=/etc/pb-mapper/server.env
ExecStart=
ExecStart=/opt/pb-mapper-server/pb-mapper-server --pb-mapper-port 7666
```

`/etc/systemd/system/pb-mapper-client-cli@.service`：

```ini
[Unit]
Description=pb-mapper client tunnel (%i)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
EnvironmentFile=/etc/pb-mapper/client-cli/%i.env
ExecStart=/opt/pb-mapper-client-cli/current/pb-mapper-client-cli \
  --pb-mapper-server ${PB_SERVER} \
  tcp-server \
  --key ${SERVICE_KEY} \
  --addr ${LOCAL_ADDR}
Restart=always
RestartSec=2
LimitNOFILE=524288

[Install]
WantedBy=multi-user.target
```

启动：

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now pb-mapper-server.service
sudo systemctl enable --now pb-mapper-client-cli@sf-backend.service
sudo systemctl enable --now caddy.service
```

## 7. 本地注册 sf-backend

在本地机器额外开一条隧道到新云机时，不要先杀旧隧道。用独立 tmux session 并行注册：

```bash
tmux new-session -d -s pbmapper-sf-backend-azure \
  "cd /home/ts_user/rust_pro/pb-mapper && \
   export MSG_HEADER_KEY=\"\$(ssh -o BatchMode=yes -o ConnectTimeout=8 azureuser@4.193.216.253 'sudo cat /var/lib/pb-mapper-server/msg_header_key')\" && \
   export PB_MAPPER_KEEP_ALIVE=ON && \
   exec /home/ts_user/.local/pbmapper/current/pb-mapper-server-cli \
     --pb-mapper-server 4.193.216.253:7666 \
     tcp-server \
     --key sf-backend \
     --addr 127.0.0.1:39180"
```

成功信号：

```text
local_server_connected_remote
local_server_registered
client_key_available
```

云机上应出现：

```bash
sudo ss -lntup | grep -E ':(39080|7666|80|443)\b'
```

其中 `127.0.0.1:39080` 由 `pb-mapper-client-cli@sf-backend` 监听，`0.0.0.0:7666` 由 `pb-mapper-server` 监听。

## 8. 验证顺序

先验证本地稳定入口：

```bash
curl -o /dev/null -sS -w 'code=%{http_code} total=%{time_total}\n' \
  http://127.0.0.1:39180/api/healthz
```

再在云机验证 pb-mapper HTTP tunnel：

```bash
ssh azureuser@4.193.216.253 \
  "curl -i -sS -H 'Host: ackingliu.top' http://127.0.0.1:39080/api/healthz | sed -n '1,20p'"
```

最后从本地模拟 DNS 切换后的 HTTPS：

```bash
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl --resolve ackingliu.top:443:4.193.216.253 \
  -o /dev/null -sS \
  -w 'code=%{http_code} verify=%{ssl_verify_result} start=%{time_starttransfer} total=%{time_total}\n' \
  https://ackingliu.top/api/healthz
```

`verify=0` 且 `code=200` 才代表证书、SNI、Caddy、pb-mapper、本地 backend 全链路可用。

裸 IP HTTPS 失败是正常的：

```text
https://4.193.216.253/api/healthz
```

Caddy 的站点和证书是按 `ackingliu.top` / `www.ackingliu.top` 配置的，不是按 IP 配置的。

公网 HTTP 到 IP 会先被 Caddy 308 到 HTTPS：

```bash
curl -i -H 'Host: ackingliu.top' http://4.193.216.253/api/healthz
```

这只能证明 `:80` 到达 Caddy，不能证明后端反代成功。后端 HTTP 直测应该打云机本地 `127.0.0.1:39080`。

## 9. DNS 迁移教训

NameSilo 默认 DNSOwl 的权威同步可能出现短时间新旧 IP 交替。先看权威 DNS，而不是只看公共递归 DNS：

```bash
for ns in ns1.dnsowl.com ns2.dnsowl.com ns3.dnsowl.com; do
  echo "--- $ns"
  dig @$ns +short A ackingliu.top
  dig @$ns +short A www.ackingliu.top
done
```

权威 DNS 全部稳定返回新 IP 之后，才进入递归缓存等待阶段。当前常见 TTL 约 3600 秒，所以公共 DNS 继续返回旧 IP 一小时左右是正常的。

如果以后经常迁移或灰度：

- 普通 A 记录没有可靠权重语义；多个 A 只是多答案，不适合精确灰度。
- 需要权重用 Route53 Weighted Routing 或 Cloudflare Load Balancing。
- 需要更快普通切换，优先把 DNS 托管迁到 Cloudflare，DNS-only 非企业最低 TTL 通常可设到 60 秒，Auto 为 300 秒；proxied 记录固定 Auto 300 秒。

## 10. 快速排障表

| 现象 | 优先判断 |
| --- | --- |
| `datalen not valid` | `MSG_HEADER_KEY` 不一致，尤其是误用了 `--use-machine-msg-header-key` |
| 云机 `39080` 没监听 | 本地 `pb-mapper-server-cli` 没注册，或 key 不一致 |
| `client key sf-backend has no healthy remote server connections` | 云端 client 已启动，但本地服务端还没注册 |
| `client_key_available` | 云端已看到本地 `sf-backend`，`39080` 应该开始监听 |
| HTTPS `verify=0` 但裸 IP HTTPS 失败 | 正常，证书按域名签发 |
| 公网 HTTP 只返回 308 | 正常，Caddy 自动 HTTPS；后端 HTTP 要测 `127.0.0.1:39080` |
| DNS 公共解析仍旧 IP | 先问权威 NS；权威没变就是 DNS 托管没发布，权威变了才是递归缓存 |

