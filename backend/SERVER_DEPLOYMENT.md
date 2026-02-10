# Backend Server Deployment Notes

本文件用于“本地运行 backend，云端仅映射入口”的部署模式。

## 部署原则

1. backend 与 LanceDB 在本地机器运行。
2. 本地 Nginx 先把本地 backend HTTP 封装成 HTTPS。
3. pb-mapper 把本地 Nginx HTTPS 映射到云端 endpoint。
4. 前端直接请求云端 pb-mapper endpoint。
5. 云端 Nginx 仅可选（用于 443 统一域名）。

## 主链路（前端请求视角）

```text
Frontend -> https://<cloud-host>:8888/api
         -> pb-mapper tunnel
         -> Local Nginx https://127.0.0.1:3443
         -> Local backend http://127.0.0.1:3000
```

## 本地 backend 建议参数

```env
RUST_ENV=production
PORT=3000
BIND_ADDR=127.0.0.1
LANCEDB_URI=/path/to/lancedb
ALLOWED_ORIGINS=https://acking-you.github.io,https://your-frontend-domain.com
RUST_LOG=info
```

## systemd（本地机器托管 backend）

`backend/staticflow-backend.service` 使用 `EnvironmentFile`：

```ini
EnvironmentFile=-/opt/staticflow/.env
ExecStart=/opt/staticflow/static-flow-backend
```

你只需要维护 `/opt/staticflow/.env` 即可。

## 本地 Nginx

使用：`deployment-examples/nginx-staticflow-api.conf`

核心：

```nginx
location /api/ {
    proxy_pass http://127.0.0.1:3000/api/;
}
```

并由本地 Nginx 监听 HTTPS（如 `3443`）。

## pb-mapper

示例：

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

## 可选云端 Nginx

使用：`deployment-examples/nginx-staticflow-cloud-proxy.conf`

仅在需要标准 `443` 域名入口时使用，核心：

```nginx
location /api/ {
    proxy_pass https://127.0.0.1:8888/api/;
}
```

## 验证顺序

1. 本地 backend：`curl http://127.0.0.1:3000/api/articles`
2. 本地 Nginx HTTPS：`curl -k https://127.0.0.1:3443/api/articles`
3. 云端映射口：`curl -k https://127.0.0.1:8888/api/articles`
4. 外部访问（直连模式）：`curl -k https://<cloud-host>:8888/api/articles`
5. 若启用云端 Nginx：`curl https://api.yourdomain.com/api/articles`
