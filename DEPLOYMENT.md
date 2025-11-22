# StaticFlow 生产环境部署指南

本文档提供 StaticFlow 项目从零到生产环境完整部署流程，适用于前后端分离架构 + 内网穿透场景。

## 📐 架构概览

```
┌─────────────────────┐
│  GitHub Pages       │  前端 WASM (HTTPS)
│  acking-you.github.io│
└──────────┬──────────┘
           │ HTTPS 跨域请求
           ↓
┌─────────────────────┐
│  服务器 (Ubuntu)     │
│  ┌────────────────┐ │
│  │ Nginx (443)    │ │  SSL 终止 + 反向代理
│  │ Let's Encrypt  │ │
│  └────────┬───────┘ │
│           │ HTTP     │
│  ┌────────▼───────┐ │
│  │ rathole 映射端口│ │  例如 8888
│  └────────┬───────┘ │
└───────────┼─────────┘
            │ TCP 隧道
            ↓
┌─────────────────────┐
│  本地开发机器        │
│  ┌────────────────┐ │
│  │ rathole client │ │
│  └────────┬───────┘ │
│           │          │
│  ┌────────▼───────┐ │
│  │ Axum Backend   │ │  监听 127.0.0.1:9999
│  │ + LanceDB      │ │
│  └────────────────┘ │
└─────────────────────┘
```

### 关键设计

- ✅ **前端静态托管**：GitHub Pages 自动 HTTPS，无需维护服务器
- ✅ **后端本地运行**：开发机器运行，通过 rathole 内网穿透到服务器
- ✅ **Nginx SSL 终止**：统一处理 HTTPS，后端保持 HTTP
- ✅ **CORS 安全**：明确限制跨域来源，防止 CSRF 攻击

---

## 🔍 前置准备检查清单

开始部署前，请确认：

- [ ] **域名**：已购买域名（例如 `yourdomain.com`），可配置 DNS
- [ ] **服务器**：Ubuntu 20.04/22.04 服务器，可 SSH 访问，有 sudo 权限
- [ ] **rathole 配置**：已配置并测试连接（本地 → 服务器端口映射）
- [ ] **GitHub PAT**：已在 `static_flow` 仓库配置 `PERSONAL_ACCESS_TOKEN`
- [ ] **后端可运行**：本地 `cargo run` 可正常启动后端服务

### rathole 配置示例（参考）

**服务器端 (`/etc/rathole/config.toml`)**：
```toml
[server]
bind_addr = "0.0.0.0:2333"  # rathole 服务端口
default_token = "your_secret_token"

[server.services.staticflow_api]
bind_addr = "127.0.0.1:8888"  # Nginx 将转发到这个端口
```

**本地客户端 (`~/.config/rathole/config.toml`)**：
```toml
[client]
remote_addr = "your-server-ip:2333"
default_token = "your_secret_token"

[client.services.staticflow_api]
local_addr = "127.0.0.1:9999"  # 本地后端监听端口
```

启动 rathole 后，访问 `http://服务器IP:8888` 应该能访问本地后端。

---

## 🔧 第一步：后端代码准备

### 1.1 环境变量配置

后端现在通过环境变量区分开发和生产环境，无需修改代码。

创建或编辑 `backend/.env`：

**本地开发配置：**
```env
# 后端配置
PORT=3000
RUST_LOG=info

# 数据路径（根据实际情况调整）
CONTENT_DIR=../content
IMAGES_DIR=./content/images

# 开发环境：允许所有跨域请求
# RUST_ENV 不设置或设置为非 production 值
# BIND_ADDR 不设置，默认 0.0.0.0
```

**生产环境配置（rathole + Nginx）：**
```env
# 后端配置
PORT=9999
RUST_LOG=info
RUST_ENV=production

# 安全配置
BIND_ADDR=127.0.0.1  # 仅本地访问，通过 rathole 转发

# 数据路径
CONTENT_DIR=../content
IMAGES_DIR=./content/images
```

### 1.2 CORS 行为说明

后端已配置自动环境检测（`backend/src/routes.rs`）：

- **开发环境**（默认）：允许所有 origin、所有方法
  ```rust
  // RUST_ENV 未设置或非 "production"
  .allow_origin(Any)
  .allow_methods(Any)
  .allow_headers(Any)
  ```

- **生产环境**（`RUST_ENV=production`）：仅允许 GitHub Pages
  ```rust
  .allow_origin("https://acking-you.github.io")
  .allow_methods([GET, POST, OPTIONS])
  ```

### 1.3 本地测试

```bash
cd backend

# 编译并运行
cargo run

# 另一个终端测试
curl http://127.0.0.1:9999/api/articles

# 应该返回 JSON 数据（当前是 mock 数据）
```

---

## 🌐 第二步：服务器 Nginx 配置

### 2.1 安装 Nginx 和 Certbot

SSH 登录服务器后执行：

```bash
# 更新包索引
sudo apt update

# 安装 Nginx 和 Let's Encrypt 工具
sudo apt install -y nginx certbot python3-certbot-nginx

# 验证安装
nginx -v
certbot --version
```

### 2.2 配置域名 DNS

登录你的域名服务商（阿里云/Cloudflare/Namesilo 等），添加 A 记录：

```
类型:      A
主机记录:  api                    # 或其他子域名
记录值:    你的服务器公网 IP
TTL:       600（默认）
```

**验证 DNS 生效**（可能需要 1-10 分钟）：
```bash
# 应该返回你的服务器 IP
dig api.yourdomain.com +short

# 或使用 nslookup
nslookup api.yourdomain.com
```

### 2.3 创建 Nginx 配置

创建站点配置文件：

```bash
sudo nano /etc/nginx/sites-available/staticflow-api
```

粘贴以下配置（**替换 `api.yourdomain.com` 和端口号**）：

```nginx
# HTTP 服务器（用于 Let's Encrypt 验证和强制跳转）
server {
    listen 80;
    listen [::]:80;
    server_name api.yourdomain.com;  # 改成你的实际域名

    # Let's Encrypt ACME 验证路径
    location /.well-known/acme-challenge/ {
        root /var/www/html;
    }

    # 其他请求跳转 HTTPS
    location / {
        return 301 https://$server_name$request_uri;
    }
}

# HTTPS 服务器（稍后由 Certbot 自动配置）
# 此时先留空，Certbot 会自动添加 SSL 配置
```

启用配置并测试：

```bash
# 创建软链接启用站点
sudo ln -s /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/

# 测试配置语法
sudo nginx -t

# 重载 Nginx
sudo systemctl reload nginx

# 检查服务状态
sudo systemctl status nginx
```

### 2.4 申请 SSL 证书（Let's Encrypt）

运行 Certbot 自动配置 HTTPS：

```bash
sudo certbot --nginx -d api.yourdomain.com
```

**交互式提示回答：**
1. **输入邮箱**：用于证书过期提醒（虽然会自动续期）
   ```
   Enter email address: your-email@example.com
   ```

2. **同意服务条款**：输入 `A`
   ```
   (A)gree/(C)ancel: A
   ```

3. **是否接收 EFF 新闻**：输入 `N`（可选）
   ```
   (Y)es/(N)o: N
   ```

4. **Certbot 会自动修改 Nginx 配置**，完成后显示：
   ```
   Successfully deployed certificate for api.yourdomain.com
   Congratulations! You have successfully enabled HTTPS
   ```

### 2.5 手动添加反向代理配置

Certbot 已添加 SSL 配置，现在需要手动添加反向代理规则：

```bash
sudo nano /etc/nginx/sites-available/staticflow-api
```

找到 `server { listen 443 ssl; ... }` 块，在 `location` 部分添加：

```nginx
server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name api.yourdomain.com;

    # Certbot 自动添加的 SSL 配置
    ssl_certificate /etc/letsencrypt/live/api.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.yourdomain.com/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

    # 反向代理到 rathole 映射的端口
    location /api/ {
        proxy_pass http://127.0.0.1:8888/api/;  # ← 改成 rathole 映射的端口

        # 传递真实客户端信息
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;

        # 超时配置
        proxy_connect_timeout 60s;
        proxy_send_timeout 60s;
        proxy_read_timeout 60s;

        # WebSocket 支持（如果需要）
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }

    # 健康检查端点（可选）
    location /health {
        access_log off;
        return 200 "OK\n";
        add_header Content-Type text/plain;
    }

    # 安全头
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;
    add_header X-XSS-Protection "1; mode=block";
}
```

**重点配置说明**：
- `proxy_pass http://127.0.0.1:8888/api/;` → 改成你的 rathole 服务端映射端口
- 末尾的 `/api/` 确保路径正确转发

测试并重载：

```bash
# 测试语法
sudo nginx -t

# 重载配置
sudo systemctl reload nginx
```

### 2.6 验证部署

```bash
# 1. 确保 rathole 正在运行
# （在服务器和本地分别检查）

# 2. 本地后端运行
cd /path/to/static_flow/backend
cargo run

# 3. 测试本地端口（本地机器执行）
curl http://127.0.0.1:9999/api/articles

# 4. 测试 rathole 映射（服务器执行）
curl http://127.0.0.1:8888/api/articles  # 改成你的端口

# 5. 测试 HTTPS 反向代理（任意机器执行）
curl https://api.yourdomain.com/api/articles

# 6. 验证 SSL 证书
curl -I https://api.yourdomain.com
# 应该看到：HTTP/2 200
```

---

## 🚀 第三步：GitHub Actions 配置

### 3.1 配置 Repository Variables

1. 访问 https://github.com/acking-you/static_flow/settings/secrets/actions
2. 切换到 **Variables** 标签
3. 点击 **New repository variable**
4. 添加以下变量：

```
Name:  STATICFLOW_API_BASE
Value: https://api.yourdomain.com/api
```

⚠️ **注意**：
- 必须是 `https://`（不是 `http://`）
- 必须包含 `/api` 路径
- 不要末尾斜杠

### 3.2 验证 workflow 配置

检查 `.github/workflows/deploy.yml` 是否包含以下配置：

```yaml
name: Deploy StaticFlow Frontend (Production)

on:
  push:
    branches:
      - master
  workflow_dispatch:  # 支持手动触发

# ...

- name: Build frontend (production)
  working-directory: frontend
  run: trunk build --release
  env:
    STATICFLOW_API_BASE: ${{ vars.STATICFLOW_API_BASE }}  # ← 关键
    TRUNK_SKIP_VERSION_CHECK: "true"

- name: Deploy to User Pages (acking-you.github.io)
  uses: peaceiris/actions-gh-pages@v3
  with:
    personal_token: ${{ secrets.PERSONAL_ACCESS_TOKEN }}  # ← 确认已配置
    external_repository: acking-you/acking-you.github.io
    publish_dir: frontend/dist
    publish_branch: master
    force_orphan: true
```

### 3.3 触发部署

**方法 1：推送代码（自动触发）**
```bash
git add .
git commit -m "Configure production deployment"
git push origin master
```

**方法 2：手动触发**
1. 访问 https://github.com/acking-you/static_flow/actions
2. 点击左侧 **Deploy StaticFlow Frontend (Production)**
3. 点击右侧 **Run workflow** → 选择 `master` → 点击 **Run workflow**

**监控部署进度**：
- Actions 页面查看实时日志
- 预计耗时 3-5 分钟
- 成功后访问 https://acking-you.github.io

---

## ✅ 第四步：验证完整链路

### 4.1 后端验证

```bash
# 1. 检查本地后端运行状态
ps aux | grep backend
# 应该看到进程正在运行

# 2. 测试本地端口
curl http://127.0.0.1:9999/api/articles

# 3. 检查 rathole 连接（服务器）
sudo lsof -i :8888  # 改成你的 rathole 端口
# 应该看到 rathole 进程

# 4. 测试 Nginx 反向代理
curl https://api.yourdomain.com/api/articles
# 应该返回 JSON 数据
```

### 4.2 前端验证

1. 访问 https://acking-you.github.io
2. 打开浏览器 DevTools（F12）
3. 切换到 **Network** 标签
4. 刷新页面

**检查项**：
- ✅ WASM 文件加载成功（`static-flow-frontend-*.wasm`）
- ✅ API 请求发往 `https://api.yourdomain.com/api/articles`
- ✅ 响应状态 `200 OK`
- ✅ 无 CORS 错误（Console 标签无红色错误）

### 4.3 端到端功能测试

- [ ] 首页文章列表加载
- [ ] 点击文章查看详情
- [ ] 搜索功能正常
- [ ] 标签筛选正常
- [ ] 分类筛选正常
- [ ] 深色/浅色主题切换

---

## 🔧 常见问题排查

### 问题 1：Mixed Content 错误

**现象**：
```
Mixed Content: The page at 'https://acking-you.github.io/' was loaded over HTTPS,
but requested an insecure resource 'http://...'
```

**解决方案**：
1. 检查 `STATICFLOW_API_BASE` 是否为 `https://`
2. 清空浏览器缓存（Ctrl+Shift+Delete）
3. 重新构建前端（方法：推送空 commit）
   ```bash
   git commit --allow-empty -m "Rebuild with correct API URL"
   git push origin master
   ```

### 问题 2：CORS 错误

**现象**：
```
Access to fetch at 'https://api.yourdomain.com/api/articles' from origin
'https://acking-you.github.io' has been blocked by CORS policy
```

**解决方案**：
1. 检查后端 `routes.rs` 的 `allow_origin` 配置
2. 确认 origin 为 `https://acking-you.github.io`（不要多余的斜杠）
3. 重启后端服务
4. 测试 OPTIONS 请求：
   ```bash
   curl -X OPTIONS -H "Origin: https://acking-you.github.io" \
        -H "Access-Control-Request-Method: GET" \
        -I https://api.yourdomain.com/api/articles

   # 应该看到：Access-Control-Allow-Origin: https://acking-you.github.io
   ```

### 问题 3：502 Bad Gateway

**现象**：Nginx 返回 502，无法访问 API

**排查步骤**：
```bash
# 1. 检查后端是否运行
ps aux | grep backend

# 2. 检查 rathole 客户端连接（本地）
sudo lsof -i :9999

# 3. 检查 rathole 服务端监听（服务器）
sudo lsof -i :8888

# 4. 测试本地后端直连（本地）
curl http://127.0.0.1:9999/api/articles

# 5. 测试 rathole 映射（服务器）
curl http://127.0.0.1:8888/api/articles

# 6. 查看 Nginx 错误日志
sudo tail -f /var/log/nginx/error.log
```

**常见原因**：
- 后端未启动或崩溃
- rathole 隧道断开
- Nginx `proxy_pass` 端口配置错误

### 问题 4：504 Gateway Timeout

**现象**：请求超时

**解决方案**：
1. 增加 Nginx 超时配置（`/etc/nginx/sites-available/staticflow-api`）：
   ```nginx
   location /api/ {
       proxy_read_timeout 120s;
       proxy_connect_timeout 120s;
       # ...
   }
   ```

2. 检查后端性能（数据库查询慢、计算量大）

3. 检查 rathole 网络延迟

### 问题 5：GitHub Actions 部署失败

**现象**：workflow 报错红色标记

**排查步骤**：
1. 点击失败的 run 查看详细日志
2. 常见错误：
   - **PAT 权限不足**：检查 `PERSONAL_ACCESS_TOKEN` 是否有 `repo` 权限
   - **变量未配置**：检查 `STATICFLOW_API_BASE` 是否正确配置
   - **编译错误**：检查 Rust 代码语法错误
   - **Trunk 版本**：确认 Trunk 0.21.14 可用

3. 本地测试构建：
   ```bash
   cd frontend
   export STATICFLOW_API_BASE="https://api.yourdomain.com/api"
   trunk build --release
   ```

### 问题 6：WASM 加载失败

**现象**：浏览器 Console 显示 WASM 加载错误

**解决方案**：
1. 检查 GitHub Pages 是否已启用
2. 确认 `acking-you.github.io` 仓库的 `master` 分支有内容
3. 检查浏览器是否支持 WASM（现代浏览器都支持）
4. 清空缓存并硬刷新（Ctrl+Shift+R）

### 问题 7：证书过期

**现象**：浏览器显示证书无效

**解决方案**：
```bash
# 查看证书状态
sudo certbot certificates

# 手动续期
sudo certbot renew

# 测试自动续期
sudo certbot renew --dry-run

# 检查续期定时任务
sudo systemctl status certbot.timer
```

Let's Encrypt 证书 90 天过期，但 Certbot 会在到期前 30 天自动续期。

---

## 🔐 安全加固建议

### 1. 限流保护（推荐）

编辑 `/etc/nginx/sites-available/staticflow-api`：

```nginx
# 在 server 块外添加
limit_req_zone $binary_remote_addr zone=api_limit:10m rate=10r/s;

# 在 location /api/ 块内添加
location /api/ {
    limit_req zone=api_limit burst=20 nodelay;
    # ... 其他配置
}
```

### 2. 防火墙配置

```bash
# 安装 UFW
sudo apt install ufw

# 允许 SSH（避免锁定自己）
sudo ufw allow 22/tcp

# 允许 HTTP/HTTPS
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# 允许 rathole 端口（如果需要外网访问）
sudo ufw allow 2333/tcp

# 启用防火墙
sudo ufw enable

# 检查状态
sudo ufw status
```

### 3. 后端日志管理

```bash
# 将后端日志重定向到文件
cd backend
cargo run 2>&1 | tee -a logs/backend.log

# 或使用 systemd 管理（如果配置了服务）
sudo journalctl -u staticflow-backend -f
```

---

## 📝 维护指南

### 日常维护清单

| 任务 | 频率 | 命令 |
|------|------|------|
| 检查后端运行状态 | 每天 | `ps aux \| grep backend` |
| 查看 Nginx 访问日志 | 按需 | `sudo tail -f /var/log/nginx/access.log` |
| 查看 Nginx 错误日志 | 按需 | `sudo tail -f /var/log/nginx/error.log` |
| 检查 SSL 证书有效期 | 每月 | `sudo certbot certificates` |
| 检查 rathole 连接 | 每天 | `sudo lsof -i :8888` |

### 更新 API 地址

如果需要更换后端域名或修改配置：

```bash
# 1. 更新 GitHub Variables
# 访问 https://github.com/acking-you/static_flow/settings/variables/actions
# 修改 STATICFLOW_API_BASE 的值

# 2. 触发重新构建
git commit --allow-empty -m "Rebuild with new API endpoint"
git push origin master

# 3. 等待 Actions 完成（约 3-5 分钟）

# 4. 清空浏览器缓存并访问
# Ctrl+Shift+R 强制刷新
```

### 监控与告警（可选）

**简易监控脚本**（`/home/user/monitor.sh`）：

```bash
#!/bin/bash

# 检查后端健康
if ! curl -f http://127.0.0.1:9999/api/articles >/dev/null 2>&1; then
    echo "❌ Backend down at $(date)" | mail -s "StaticFlow Alert" your-email@example.com
fi

# 检查 Nginx
if ! systemctl is-active --quiet nginx; then
    echo "❌ Nginx down at $(date)" | mail -s "StaticFlow Alert" your-email@example.com
fi
```

添加到 cron 定时任务：
```bash
crontab -e

# 每 5 分钟检查一次
*/5 * * * * /home/user/monitor.sh
```

---

## 📚 快速参考

### 配置文件位置

| 描述 | 路径 |
|------|------|
| Nginx 站点配置 | `/etc/nginx/sites-available/staticflow-api` |
| SSL 证书 | `/etc/letsencrypt/live/api.yourdomain.com/` |
| Nginx 访问日志 | `/var/log/nginx/access.log` |
| Nginx 错误日志 | `/var/log/nginx/error.log` |
| 后端配置 | `backend/.env` |
| rathole 服务端配置 | `/etc/rathole/config.toml` |
| rathole 客户端配置 | `~/.config/rathole/config.toml` |

### 端口映射关系

```
外网请求 → 443 (Nginx HTTPS)
          ↓
         127.0.0.1:8888 (rathole 服务端映射)
          ↓ TCP 隧道
         127.0.0.1:9999 (本地 Axum 后端)
```

### 常用命令速查

```bash
# ========== Nginx ==========
sudo nginx -t                     # 测试配置
sudo systemctl reload nginx       # 重载配置
sudo systemctl restart nginx      # 重启服务
sudo systemctl status nginx       # 查看状态

# ========== Certbot ==========
sudo certbot certificates         # 查看证书
sudo certbot renew               # 手动续期
sudo certbot renew --dry-run     # 测试续期

# ========== 后端 ==========
cd backend && cargo run          # 启动后端
curl http://127.0.0.1:9999/api/articles  # 本地测试

# ========== rathole ==========
# （根据实际启动方式调整）
sudo systemctl status rathole    # 如果配置为服务
./rathole /path/to/config.toml   # 手动启动

# ========== GitHub Actions ==========
# 访问 https://github.com/acking-you/static_flow/actions
# 点击 Run workflow 手动触发

# ========== 完整测试链路 ==========
# 1. 本地后端
curl http://127.0.0.1:9999/api/articles

# 2. rathole 映射（服务器）
curl http://127.0.0.1:8888/api/articles

# 3. Nginx HTTPS
curl https://api.yourdomain.com/api/articles

# 4. 前端页面
# 浏览器访问 https://acking-you.github.io
```

---

## 🎯 下一步计划

部署完成后，可以继续开发以下功能（参考 `CLAUDE.md`）：

1. **AI Agent 集成**（Phase 1）
   - 集成 Rig 框架
   - 实现本地 Markdown 文件监听
   - 自动生成文章元数据

2. **LanceDB 集成**（Phase 2）
   - 替换 mock 数据
   - 实现向量搜索
   - 图片相似度搜索

3. **功能增强**（Phase 3+）
   - 语义搜索 UI
   - 图片搜索功能
   - 统计分析

---

## 🆘 获取帮助

- **项目文档**：查看 `CLAUDE.md` 了解架构设计
- **GitHub Issues**：https://github.com/acking-you/static_flow/issues
- **Nginx 文档**：https://nginx.org/en/docs/
- **Let's Encrypt 文档**：https://letsencrypt.org/docs/
- **rathole 文档**：https://github.com/rapiz1/rathole

---

**祝部署顺利！🚀**

如遇到文档未涵盖的问题，欢迎提 Issue 反馈。
