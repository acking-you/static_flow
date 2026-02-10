# StaticFlow Deployment Guide (Local Nginx HTTPS + pb-mapper)

本指南已按你的最新要求对齐：

- backend 在本地机器运行（LanceDB 同机存储）
- 本地 Nginx 提供 HTTPS（前置 backend）
- pb-mapper 把本地 HTTPS 服务映射到云端端点
- 前端页面加载后，直接请求云端 pb-mapper 暴露端点
- 云端 Nginx 仅作为可选层（域名/443/证书管理）

> 更新时间：2026-02-10

---

## 1. 架构说明

### 1.1 主链路（按前端请求视角）

```text
Frontend(fetch/XHR, 已加载页面)
        |
        v
https://<cloud-host>:8888/api   (pb-mapper 在云端暴露的 endpoint)
        |
        v
pb-mapper tunnel
        |
        v
Local Nginx :3443 (HTTPS)
        |
        v
Local backend :3000 (HTTP)
        |
        v
Local LanceDB (/path/to/lancedb)
```

### 1.2 可选链路（需要标准 443 + 公网证书时）

```text
Frontend(fetch/XHR)
        |
        v
Cloud Nginx :443
        |
        v
https://127.0.0.1:8888/api (pb-mapper local endpoint)
        |
        v
pb-mapper tunnel -> Local Nginx -> Local backend
```

> 重点：你的主诉求是“前端直接请求云端 pb-mapper endpoint”。
> 云端 Nginx 是可选增强，不是必选前置。

---

## 2. 本地准备

### 2.1 初始化 LanceDB

```bash
make bin-all
./target/release/sf-cli init --db-path ./data/lancedb
```

### 2.2 导入笔记与图片（推荐）

```bash
./target/release/sf-cli sync-notes \
  --db-path ./data/lancedb \
  --dir ./content \
  --recursive \
  --generate-thumbnail

# 批量导入后可手动补建索引
./target/release/sf-cli ensure-indexes --db-path ./data/lancedb
```

行为说明：
- 扫描 markdown
- 解析本地图片引用并写入 `images` 表（二进制 + 向量 + 可选缩略图）
- 自动把 markdown 图片链接改写为 `images/<sha256_id>`
- upsert 文章到 `articles` 表

### 2.3 启动本地 backend

```bash
LANCEDB_URI=../data/lancedb \
PORT=3000 \
BIND_ADDR=127.0.0.1 \
RUST_ENV=production \
ALLOWED_ORIGINS=https://acking-you.github.io \
./target/release/static-flow-backend
```

本地验证：

```bash
curl http://127.0.0.1:3000/api/articles
curl http://127.0.0.1:3000/api/images
```

### 2.4 启动本地 Nginx HTTPS

复制配置：

```bash
sudo cp deployment-examples/nginx-staticflow-api.conf /etc/nginx/conf.d/staticflow-local.conf
```

按本机环境调整：
- backend 端口（默认 `127.0.0.1:3000`）
- 证书路径（`ssl_certificate` / `ssl_certificate_key`）

验证：

```bash
sudo nginx -t
sudo systemctl reload nginx
curl -k https://127.0.0.1:3443/api/articles
```

---

## 3. pb-mapper 配置

仓库：`https://github.com/acking-you/pb-mapper/`

已按源码确认当前 CLI 语法（`tcp-server --key --addr --pb-mapper-server`）。

### 3.1 示例命令

```bash
# 本地（服务提供侧）
pb-mapper-server-cli tcp-server \
  --key staticflow-api-https \
  --addr 127.0.0.1:3443 \
  --pb-mapper-server <cloud-ip>:7666

# 云端（映射消费侧）
# 若要让前端可直接访问，建议绑定 0.0.0.0:8888
pb-mapper-client-cli tcp-server \
  --key staticflow-api-https \
  --addr 0.0.0.0:8888 \
  --pb-mapper-server 127.0.0.1:7666
```

### 3.2 验证

在云端：

```bash
curl -k https://127.0.0.1:8888/api/articles
```

在可访问云端网络的客户端：

```bash
curl -k https://<cloud-host>:8888/api/articles
```

> 若浏览器要无警告访问 `https://<cloud-host>:8888`，需要本地 Nginx 提供的证书对该 host 可验证。
> 如果证书不便管理，建议启用第 4 节（可选云端 Nginx 443 终止）。

---

## 4. 可选：云端 Nginx HTTPS（443 域名统一入口）

如果你不想让前端直接访问 `:8888`，可用云端 Nginx 暴露标准 `443`。

### 4.1 安装

```bash
sudo apt update
sudo apt install -y nginx certbot python3-certbot-nginx
```

### 4.2 使用示例配置

```bash
sudo cp deployment-examples/nginx-staticflow-cloud-proxy.conf /etc/nginx/sites-available/staticflow-api
sudo nano /etc/nginx/sites-available/staticflow-api
```

必须修改：
- `server_name`（你的真实域名）
- `proxy_pass https://127.0.0.1:8888/api/`（你的 pb-mapper 映射端口）

### 4.3 启用并申请证书

```bash
sudo ln -sf /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/staticflow-api
sudo nginx -t
sudo systemctl reload nginx

sudo certbot --nginx -d api.yourdomain.com
```

验证：

```bash
curl https://api.yourdomain.com/api/articles
```

---

## 5. 前端接入

### 5.1 主模式（直连 pb-mapper endpoint）

```text
STATICFLOW_API_BASE=https://<cloud-host>:8888/api
```

### 5.2 可选模式（经云端 Nginx 443）

```text
STATICFLOW_API_BASE=https://api.yourdomain.com/api
```

---

## 6. 生产环境变量建议

`backend/.env.production` 示例：

```env
RUST_ENV=production
PORT=3000
BIND_ADDR=127.0.0.1
LANCEDB_URI=/opt/staticflow/data/lancedb
ALLOWED_ORIGINS=https://acking-you.github.io,https://your-frontend-domain.com
RUST_LOG=info
```

---

## 7. 运维检查清单

### 7.1 本地

```bash
# backend
curl http://127.0.0.1:3000/api/articles

# 本地 Nginx HTTPS
curl -k https://127.0.0.1:3443/api/articles

# LanceDB 可读（示例）
./target/release/sf-cli query --db-path ./data/lancedb --table articles --limit 3
```

### 7.2 云端

```bash
# pb-mapper 映射是否可达
curl -k https://127.0.0.1:8888/api/articles

# 若启用云端 Nginx，验证域名
curl https://api.yourdomain.com/api/articles

# Nginx 配置与日志（可选云端 Nginx）
sudo nginx -t
sudo tail -f /var/log/nginx/staticflow_error.log
```

---

## 8. 常见问题

### Q1: 图片返回 404

检查：
1. markdown 中图片是否为 `images/<sha256_id>`
2. `images` 表里是否存在该 id
3. 前端 `STATICFLOW_API_BASE` 是否配置正确

### Q2: CORS 报错

1. 确认 `RUST_ENV=production`
2. 正确设置 `ALLOWED_ORIGINS`
3. 前端页面来源与白名单一致

### Q3: pb-mapper 通了，但浏览器 HTTPS 报证书错误

原因通常是证书与访问 host 不匹配。

可选处理：
1. 在本地 Nginx 使用可被浏览器信任且匹配 host 的证书
2. 或启用云端 Nginx 443 做 TLS 终止
