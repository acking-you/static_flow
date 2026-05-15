# Cloud Ingress、LLM Path Split 与 pb-mapper Runbook

这份 runbook 记录当前 `ackingliu.top` 的现网入口形态。旧架构里云机只做
Caddy TLS 和 pb-mapper 中继，所有后端流量都回到本地机器。当前架构已经
改变：GCP 云机同时承载 `llm-access`，LLM 路径在云端直接完成，非 LLM
StaticFlow 路径才继续通过 pb-mapper 回本地。

## 1. 当前链路

```text
public client
  -> https://ackingliu.top / https://www.ackingliu.top
  -> GCP Caddy :443
     ├── LLM paths
     │   -> cloud llm-access 127.0.0.1:19080
     │      -> Neon control plane via /mnt/llm-access/config/neon.env
     │      -> usage journal on /var/lib/staticflow/llm-access/usage-journal
     │      -> active DuckDB on /var/lib/staticflow/llm-access/analytics-active
     │      -> archived DuckDB segments/catalog on /mnt/llm-access-usage
     │      -> heavy usage details as compressed packs on /mnt/llm-access-usage/details
     └── non-LLM paths
         -> cloud pb-mapper-client-cli 127.0.0.1:39080
         -> configured cloud pb-mapper relay
         -> local pb-mapper-server-cli key=sf-backend
         -> local Pingora gateway 127.0.0.1:39180
         -> active StaticFlow backend slot
```

关键端口语义：

| 地址 | 作用 | 是否公网暴露 |
| --- | --- | --- |
| GCP `:443` | Caddy TLS 入口 | 是 |
| GCP `:80` | Caddy HTTP-01/重定向 | 是 |
| GCP configured relay port | pb-mapper server，供本地注册服务 | 是，需受 key 保护 |
| GCP `127.0.0.1:19080` | cloud `llm-access` | 否 |
| GCP `127.0.0.1:39080` | cloud pb-mapper client 暴露的 non-LLM StaticFlow 本地入口 | 否 |
| local `127.0.0.1:39180` | 本地 Pingora 稳定入口 | 否 |
| local `127.0.0.1:19182` | 本地订阅 cloud `llm-access` 的 back-link | 否 |

## 2. GCP 主机预检

GCP 公网地址、SSH 用户、SSH key 路径不写入 tracked 文档。它们统一放在
本机 ignored 配置 `.local/llm-access-cloud-release.env`，新 checkout 用
`conf/llm-access-cloud-release.env.example` 复制后填写。

```bash
set -a
source .local/llm-access-cloud-release.env
set +a
ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST"
```

不要假设默认 cloud 用户可用；以 `GCP_DEST` 为准。

常用只读检查：

```bash
set -a
source .local/llm-access-cloud-release.env
set +a
ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST" \
  'hostname; date -u +%FT%TZ; sudo ss -lntup'

ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST" \
  'systemctl is-active caddy pb-mapper-server.service pb-mapper-client-cli@sf-backend.service llm-access.service juicefs-llm-access.service pb-mapper-server-cli@llm-access.service'
```

## 3. Caddy Path Split

`/etc/caddy/Caddyfile` 必须先匹配 LLM 路径，再进入默认 non-LLM 反代。
不要用 `handle_path`，它会剥离前缀并破坏 `/v1/*`、`/api/llm-gateway/*`
这类 provider 路由。

```caddyfile
{
    email admin@ackingliu.top
    servers {
        protocols h1 h2
    }
}

ackingliu.top, www.ackingliu.top {
    @health path /_caddy_health
    handle @health {
        respond "ok" 200
    }

    @admin path /admin*
    handle @admin {
        respond "forbidden" 403
    }

    @llm_access path /v1/* /cc/v1/* /api/llm-gateway/* /api/kiro-gateway/* /api/codex-gateway/* /api/llm-access/*
    handle @llm_access {
        reverse_proxy 127.0.0.1:19080 {
            header_up X-Real-IP {remote_host}
            header_up X-Forwarded-For {remote_host}
            header_up X-Forwarded-Proto {scheme}
            header_up X-Forwarded-Host {host}
        }
    }

    handle {
        reverse_proxy 127.0.0.1:39080 {
            header_up X-Real-IP {remote_host}
            header_up X-Forwarded-For {remote_host}
            header_up X-Forwarded-Proto {scheme}
            header_up X-Forwarded-Host {host}
        }
    }
}
```

验证配置：

```bash
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
sudo journalctl -u caddy -n 120 --no-pager -l
```

`/_caddy_health` 只证明 Caddy 活着，不证明 pb-mapper 或 `llm-access` 可用。

## 4. llm-access 云端状态

`llm-access` 是当前 LLM 接入层的生产 source of truth。它必须保持单写：
不要在本地再运行一个会写同一份 JuiceFS 状态的 `llm-access`。

当前状态布局：

```text
/mnt/llm-access
  /config/neon.env
  /control/llm-access.sqlite3   # rollback snapshot only
  /auths/codex
  /auths/kiro
  /support/llm_access_support

/var/lib/staticflow/llm-access/usage-journal
/var/lib/staticflow/llm-access/analytics-active
/mnt/llm-access-usage
  usage-active-*.duckdb
```

`/mnt/llm-access` 是 JuiceFS FUSE mount。对象存储后端是 Cloudflare R2，
元数据后端是 Valkey。R2/Valkey 密钥只放在被 git 忽略的私有 env 文件里，
不要写入 README、runbook、systemd 模板或 shell history。

当前 usage analytics 已经拆成两层：

- summary fact 继续写入 tiered DuckDB
- 单条事件的重明细 payload 继续保持 pack 形式，但写入独立的 JuiceFS usage
  mount `/mnt/llm-access-usage/details`

因此 worker 现在依赖两个 mount：

- `/mnt/llm-access`
- `/mnt/llm-access-usage`

当前资源保护基线：

- GCP VM 是 2c8g 级别。
- 主机有两个 2GiB swap 文件：`/swapfile` 和 `/swapfile-llm-extra`。
- `vm.swappiness=10`。
- `llm-access.service`：`MemoryHigh=3584M`、`MemoryMax=4096M`、
  `MemorySwapMax=1024M`、`TasksMax=256`、`OOMPolicy=kill`。
- `juicefs-llm-access.service`：`MemoryHigh=1800M`、`MemoryMax=2560M`、
  `MemorySwapMax=0`、`TasksMax=256`、`OOMPolicy=kill`。

只读检查：

```bash
sudo systemctl status llm-access.service --no-pager -l
sudo systemctl status juicefs-llm-access.service --no-pager -l
findmnt -T /mnt/llm-access
free -h
swapon --show
curl -sS -o /dev/null -w 'code=%{http_code} total=%{time_total}\n' \
  http://127.0.0.1:19080/healthz
```

生产 usage 明细排障不要用大分页 admin API 扫全量 DuckDB。那些查询在
`llm-access` 进程内执行，DuckDB scan buffer 会计入服务 RSS。广域诊断应
使用外部只读 DuckDB 连接，或者只打窄时间窗/窄 key 的 API。

## 5. pb-mapper 服务边界

GCP 侧仍然需要 pb-mapper，但它现在只负责两类事情：

1. non-LLM StaticFlow 路径从 GCP Caddy 回到本地 Pingora。
2. 把 cloud `llm-access` 注册成 `llm-access` key，供本地机器订阅到
   `127.0.0.1:19182`。

核心 systemd units：

```text
pb-mapper-server.service
pb-mapper-client-cli@sf-backend.service
pb-mapper-server-cli@llm-access.service
```

pb-mapper message header key 必须在 GCP server、GCP client、local
server/client 之间一致。实际值只放在对应 ignored/private env 文件里。
排障时只比较 hash，不打印明文：

```bash
sudo sh -c 'tr -d "\r\n" </var/lib/pb-mapper-server/msg_header_key | sha256sum'
sudo sh -c '. /etc/pb-mapper/server.env; printf "%s" "$MSG_HEADER_KEY" | tr -d "\r\n" | sha256sum'
sudo sh -c '. /etc/pb-mapper/client-cli/sf-backend.env; printf "%s" "$MSG_HEADER_KEY" | tr -d "\r\n" | sha256sum'
```

常见错误：

| 现象 | 优先判断 |
| --- | --- |
| `datalen not valid` | pb-mapper message header key 不一致，尤其是误用了按机器派生 key |
| GCP `127.0.0.1:39080` 没监听 | 本地 `sf-backend` 没注册，或 key 不一致 |
| `client key sf-backend has no healthy remote server connections` | GCP client 已启动，但本地服务端还没注册 |
| `client_key_available` | GCP 已看到本地 `sf-backend`，`39080` 应该开始监听 |

## 6. 验证顺序

先在 GCP 本机区分 LLM 与 non-LLM：

```bash
# LLM service direct
curl -o /dev/null -sS \
  -w 'llm code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n' \
  http://127.0.0.1:19080/healthz

# Non-LLM pb-mapper tunnel direct, must send real Host header
curl -o /dev/null -sS \
  -w 'sf code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n' \
  -H 'Host: ackingliu.top' \
  http://127.0.0.1:39080/api/healthz
```

再从本地做真实公网检查。注意先清掉代理环境变量，避免通过本机代理得到
假阳性：

```bash
env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl -o /dev/null -sS \
  -w 'llm status code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n' \
  https://ackingliu.top/api/llm-gateway/status

env -u https_proxy -u HTTPS_PROXY -u http_proxy -u HTTP_PROXY -u all_proxy -u ALL_PROXY \
  curl -o /dev/null -sS \
  -w 'staticflow code=%{http_code} start=%{time_starttransfer} total=%{time_total}\n' \
  https://ackingliu.top/api/healthz
```

裸 IP HTTPS 失败是正常的；证书按 `ackingliu.top` 和 `www.ackingliu.top`
签发，不按 IP 签发。

## 7. 恢复策略

### Non-LLM public outage

如果文章、图片、普通 API 或首页卡住，但 LLM routes 正常，优先处理
Caddy/pb-mapper/local Pingora 链路。

```bash
set -a
source .local/llm-access-cloud-release.env
set +a
ssh -i "$GCP_SSH_KEY" -o IdentitiesOnly=yes "$GCP_DEST"
sudo systemctl restart caddy
sudo systemctl restart pb-mapper-server.service
sudo systemctl restart pb-mapper-client-cli@sf-backend.service
```

如果日志显示 `sf-backend` key 不存在或没有 healthy remote server，问题很
可能在本地机器没有重新注册 `pbmapper-sf-backend`。这时不要反复重启 GCP
client；先恢复本地注册，再重启一次 GCP
`pb-mapper-client-cli@sf-backend.service` 让它重新订阅。

### LLM outage or memory pressure

如果 Codex/Kiro/NewAPI 流量异常，优先看 cloud `llm-access`，不是本地
Pingora：

```bash
sudo systemctl status llm-access.service --no-pager -l
sudo journalctl -u llm-access.service -n 200 --no-pager -l
tail -n 200 /var/log/staticflow-runtime/llm-access/app/current.*.log
systemctl show llm-access.service -p MemoryCurrent -p MemoryPeak -p MemoryHigh -p MemoryMax -p MemorySwapMax -p NRestarts
free -h
swapon --show
```

如果 usage 页面或诊断查询导致 RSS 快速升高，先停止大分页/宽时间窗查询，
再用窄条件或外部只读 DuckDB 诊断。不要把
`/mnt/llm-access/analytics/usage.duckdb` 当作实时单体 DuckDB 写入目标；
当前设计要求 active mutable segment 在 VM 本地盘，归档 segment 才进入
JuiceFS/R2。

## 8. DNS 迁移检查

域名解析应以权威 DNS 为准，不要只看公共递归缓存：

```bash
for ns in ns1.dnsowl.com ns2.dnsowl.com ns3.dnsowl.com; do
  echo "--- $ns"
  dig @$ns +short A ackingliu.top
  dig @$ns +short A www.ackingliu.top
done
```

当前 GCP IP 以 `.local/llm-access-cloud-release.env` 里的 `GCP_HOST` 或
`GCP_DEST` 为准。如果权威 DNS 全部稳定返回新 IP 后，公共 DNS 仍返回旧
IP，通常只是递归缓存等待。
