# Nginx HTTPS Guide (for pb-mapper API Exposure)

本文件聚焦：
1. 本地 Nginx 如何把 backend 封装成 HTTPS
2. 前端如何通过云端 pb-mapper endpoint 直接访问本地 backend
3. 云端 Nginx 作为可选层（443/证书/域名）如何接入

> 更新时间：2026-02-10

## 1) 主目标链路

```text
Frontend(fetch/XHR) -> https://<cloud-host>:8888/api
                    -> pb-mapper tunnel
                    -> Local Nginx:3443 (HTTPS)
                    -> Local backend:3000 (HTTP)
```

> 云端 Nginx 不在主链路里；仅在需要标准 443 域名入口时启用。

## 2) 配置文件

- 本地 Nginx HTTPS：`deployment-examples/nginx-staticflow-api.conf`
- 云端 Nginx HTTPS 反代（可选）：`deployment-examples/nginx-staticflow-cloud-proxy.conf`

本地使用（把 backend 前置为 HTTPS）：

```bash
sudo cp deployment-examples/nginx-staticflow-api.conf /etc/nginx/conf.d/staticflow-local.conf
sudo nginx -t
sudo systemctl reload nginx
curl -k https://127.0.0.1:3443/api/articles
```

## 3) pb-mapper 映射与验证

参考命令：

```bash
# 本地
pb-mapper-server-cli tcp-server \
  --key staticflow-api-https \
  --addr 127.0.0.1:3443 \
  --pb-mapper-server <cloud-ip>:7666

# 云端
pb-mapper-client-cli tcp-server \
  --key staticflow-api-https \
  --addr 0.0.0.0:8888 \
  --pb-mapper-server 127.0.0.1:7666
```

验证：

```bash
# 云端本机
curl -k https://127.0.0.1:8888/api/articles

# 外部客户端
curl -k https://<cloud-host>:8888/api/articles
```

## 4) 可选：云端 Nginx 443

如果你希望前端使用 `https://api.yourdomain.com/api`（不带端口），启用云端 Nginx：

```bash
sudo cp deployment-examples/nginx-staticflow-cloud-proxy.conf /etc/nginx/sites-available/staticflow-api
sudo ln -sf /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/staticflow-api
sudo nginx -t
sudo systemctl reload nginx
sudo certbot --nginx -d api.yourdomain.com
```

关键项：
- `proxy_pass https://127.0.0.1:8888/api/`（pb-mapper 映射口）

## 5) 常见错误

### 502 Bad Gateway

通常是 `proxy_pass` 目标不可达。检查顺序：
1. `pb-mapper-client-cli` 是否在线并监听端口
2. 本地 `pb-mapper-server-cli` 是否在线
3. 本地 Nginx HTTPS (`127.0.0.1:3443`) 是否健康
4. 本地 backend 是否健康

### HTTPS 证书错误（直连 pb-mapper 模式）

浏览器会校验证书是否匹配 `https://<cloud-host>:8888`。

处理方式：
1. 本地 Nginx 使用匹配 host 且受信任证书
2. 或启用云端 Nginx 443 做 TLS 终止

### CORS 错误

不是 Nginx 问题，需检查 backend：
- `RUST_ENV=production`
- `ALLOWED_ORIGINS` 包含前端来源

### 图片接口异常

当前图片来自 LanceDB，不再走静态目录。

验证：

```bash
curl -k https://<cloud-host>:8888/api/images
curl -k https://<cloud-host>:8888/api/images/<image_id>
```
