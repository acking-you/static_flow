# Nginx 反向代理 + HTTPS 一键配置指南

本文档提供完整的 Nginx 反向代理和 HTTPS/SSL 证书配置流程，适用于任何需要将本地服务（如后端 API）通过域名对外提供 HTTPS 访问的场景。

## 📋 目录

- [前置条件](#前置条件)
- [第一步：DNS 配置](#第一步dns-配置)
- [第二步：后端服务准备](#第二步后端服务准备)
- [第三步：Nginx 安装](#第三步nginx-安装)
- [第四步：配置 HTTP 反向代理](#第四步配置-http-反向代理)
- [第五步：申请 SSL 证书](#第五步申请-ssl-证书)
- [第六步：验证 HTTPS](#第六步验证-https)
- [证书自动续期](#证书自动续期)
- [完整配置示例](#完整配置示例)
- [常见问题](#常见问题)
- [维护指南](#维护指南)

---

## 前置条件

### 必需条件

- ✅ **Linux 服务器**：Ubuntu 20.04/22.04 或 Debian 11+ 推荐
- ✅ **域名**：已购买域名（例如 `example.com`）
- ✅ **服务器 root 或 sudo 权限**
- ✅ **后端服务已运行**：监听本地端口（例如 `127.0.0.1:9999`）
- ✅ **开放端口**：80 (HTTP) 和 443 (HTTPS)

### 检查清单

```bash
# 1. 检查服务器操作系统
cat /etc/os-release

# 2. 检查后端服务是否运行
curl http://127.0.0.1:9999/api/articles
# 应该返回数据

# 3. 检查端口是否开放
sudo ufw status
# 或
sudo firewall-cmd --list-all
```

---

## 第一步：DNS 配置

### 方案一：使用子域名（推荐）

**适用场景**：后端 API 独立域名，前端使用其他域名

登录域名服务商（阿里云/腾讯云/Cloudflare 等），添加 A 记录：

```
记录类型: A
主机记录: api                  ← 只填 "api"，不是完整域名
记录值:   your-server-ip        ← 服务器公网 IP
TTL:      600 或默认
```

**结果**：可以通过 `api.example.com` 访问服务

---

### 方案二：使用主域名

**适用场景**：主域名直接用于后端 API

添加 A 记录：

```
记录类型: A
主机记录: @                    ← @ 表示主域名
记录值:   your-server-ip
TTL:      600 或默认
```

**结果**：可以通过 `example.com` 访问服务

---

### 验证 DNS 生效

```bash
# 方法一：使用 dig（推荐）
dig api.example.com +short
# 应该返回服务器 IP

# 方法二：使用 nslookup
nslookup api.example.com

# 方法三：使用公共 DNS 验证
dig @8.8.8.8 api.example.com +short
```

⚠️ **注意**：DNS 生效需要 1-10 分钟，必须等待生效后再继续。

---

## 第二步：后端服务准备

### 检查后端服务

确保后端服务已运行并可访问：

```bash
# 测试本地访问
curl http://127.0.0.1:9999/api/articles

# 检查进程状态
ps aux | grep your-backend-service

# 检查监听端口
sudo lsof -i :9999
# 或
sudo netstat -tlnp | grep 9999
```

### 后端配置建议

**推荐配置**：
- **监听地址**：`127.0.0.1`（仅本地访问，通过 Nginx 代理）
- **端口**：自定义端口（例如 9999），避免使用 80/443
- **日志**：启用访问日志和错误日志

**示例配置**（以 Rust Axum 为例）：

```env
# .env 配置
BIND_ADDR=127.0.0.1
PORT=9999
RUST_LOG=info
```

---

## 第三步：Nginx 安装

### Ubuntu/Debian

```bash
# 更新包索引
sudo apt update

# 安装 Nginx 和 Certbot（SSL 证书工具）
sudo apt install -y nginx certbot python3-certbot-nginx

# 验证安装
nginx -v
certbot --version
```

### CentOS/RHEL

```bash
# 安装 EPEL 仓库
sudo yum install -y epel-release

# 安装 Nginx 和 Certbot
sudo yum install -y nginx certbot python3-certbot-nginx

# 启动 Nginx
sudo systemctl start nginx
sudo systemctl enable nginx
```

### 配置防火墙

```bash
# Ubuntu/Debian (UFW)
sudo ufw allow 22/tcp    # SSH（避免锁定自己）
sudo ufw allow 80/tcp    # HTTP
sudo ufw allow 443/tcp   # HTTPS
sudo ufw enable
sudo ufw status

# CentOS/RHEL (firewalld)
sudo firewall-cmd --permanent --add-service=http
sudo firewall-cmd --permanent --add-service=https
sudo firewall-cmd --reload
```

---

## 第四步：配置 HTTP 反向代理

### 步骤 1：删除默认配置

```bash
# 禁用默认站点（避免冲突）
sudo rm -f /etc/nginx/sites-enabled/default
```

### 步骤 2：创建站点配置

**方法一：使用 nano 编辑器**

```bash
# 创建配置文件
sudo nano /etc/nginx/sites-available/your-site
```

粘贴以下内容（**替换域名和端口**）：

```nginx
server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name api.example.com;  # ← 改成你的域名

    location / {
        # 反向代理到后端
        proxy_pass http://127.0.0.1:9999;  # ← 改成你的后端端口

        # 传递客户端真实信息
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # 超时配置（可选）
        proxy_connect_timeout 60s;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;
    }
}
```

保存退出（`Ctrl+X` → `Y` → `Enter`）

---

**方法二：一键创建配置**

```bash
# 设置变量
DOMAIN="api.example.com"        # ← 改成你的域名
BACKEND_PORT="9999"             # ← 改成你的后端端口

# 创建配置
sudo tee /etc/nginx/sites-available/your-site > /dev/null << EOF
server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name ${DOMAIN};

    location / {
        proxy_pass http://127.0.0.1:${BACKEND_PORT};
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
EOF
```

---

### 步骤 3：启用配置

```bash
# 创建软链接启用站点
sudo ln -s /etc/nginx/sites-available/your-site /etc/nginx/sites-enabled/

# 测试配置语法
sudo nginx -t

# 如果测试通过，重载 Nginx
sudo systemctl reload nginx

# 检查状态
sudo systemctl status nginx
```

---

### 步骤 4：测试 HTTP 访问

```bash
# 本地测试
curl http://127.0.0.1/api/articles

# 域名测试（DNS 生效后）
curl http://api.example.com/api/articles

# 查看访问日志
sudo tail -f /var/log/nginx/access.log
```

---

## 第五步：申请 SSL 证书

### 自动配置（推荐）

Certbot 会自动申请证书并修改 Nginx 配置：

```bash
# 一键申请并配置
sudo certbot --nginx -d api.example.com --email admin@example.com --agree-tos --redirect --non-interactive
```

**参数说明**：
- `-d api.example.com` - 指定域名
- `--email admin@example.com` - 证书过期提醒邮箱
- `--agree-tos` - 同意服务条款
- `--redirect` - 自动配置 HTTP 重定向到 HTTPS
- `--non-interactive` - 非交互模式（自动确认）

---

### 交互式配置（可选）

如果你想手动确认每一步：

```bash
sudo certbot --nginx -d api.example.com
```

按提示操作：
1. 输入邮箱：`admin@example.com`
2. 同意条款：`A`（Agree）
3. 是否重定向：`2`（Redirect - Make all requests redirect to HTTPS）

---

### 多域名证书

```bash
# 为主域名和子域名同时申请
sudo certbot --nginx -d example.com -d www.example.com -d api.example.com
```

---

### 成功提示

申请成功后会显示：

```
Successfully deployed certificate for api.example.com
Congratulations! You have successfully enabled HTTPS on https://api.example.com

Certificate is saved at: /etc/letsencrypt/live/api.example.com/fullchain.pem
Key is saved at:         /etc/letsencrypt/live/api.example.com/privkey.pem
This certificate expires on 2026-02-28.
Certbot has set up a scheduled task to automatically renew this certificate.
```

---

## 第六步：验证 HTTPS

### 验证脚本

```bash
#!/bin/bash

DOMAIN="api.example.com"  # ← 改成你的域名

echo "=== 测试 HTTPS API ==="
curl -s https://${DOMAIN}/api/articles | head -c 200
echo "..."

echo ""
echo "=== 测试 HTTP 重定向 ==="
curl -I http://${DOMAIN}/api/articles | grep -E "HTTP|Location"

echo ""
echo "=== 测试 SSL 证书 ==="
curl -I https://${DOMAIN} | grep -E "HTTP|Server"

echo ""
echo "=== 查看证书信息 ==="
sudo certbot certificates

echo ""
echo "=== 查看 Nginx 最终配置 ==="
sudo cat /etc/nginx/sites-available/your-site
```

---

### 浏览器测试

1. 访问：`https://api.example.com/api/articles`
2. 检查地址栏是否显示 🔒 锁图标
3. 点击锁图标 → 查看证书详情
4. 确认证书颁发者：`Let's Encrypt`

---

### 检查安全头（可选）

```bash
curl -I https://api.example.com | grep -E "Strict-Transport-Security|X-Content-Type|X-Frame"
```

---

## 证书自动续期

### 自动续期机制

Let's Encrypt 证书：
- **有效期**：90 天
- **自动续期**：Certbot 会在到期前 30 天自动续期
- **续期频率**：每天检查 2 次

---

### 验证自动续期

```bash
# 1. 查看定时任务状态
sudo systemctl status certbot.timer

# 2. 查看下次运行时间
sudo systemctl list-timers | grep certbot

# 3. 测试续期（不会真的续期）
sudo certbot renew --dry-run

# 成功会显示：
# Congratulations, all simulated renewals succeeded
```

---

### 手动续期（可选）

```bash
# 续期所有证书
sudo certbot renew

# 续期特定证书
sudo certbot renew --cert-name api.example.com

# 续期后重载 Nginx
sudo systemctl reload nginx
```

---

### 查看证书信息

```bash
# 查看所有证书
sudo certbot certificates

# 输出示例：
# Certificate Name: api.example.com
#   Domains: api.example.com
#   Expiry Date: 2026-02-28 12:34:56+00:00 (VALID: 89 days)
#   Certificate Path: /etc/letsencrypt/live/api.example.com/fullchain.pem
#   Private Key Path: /etc/letsencrypt/live/api.example.com/privkey.pem
```

---

## 完整配置示例

### 最终 Nginx 配置

Certbot 会自动修改配置，最终类似：

```nginx
# HTTP Server (重定向到 HTTPS)
server {
    listen 80;
    listen [::]:80;
    server_name api.example.com;

    # Let's Encrypt ACME 验证
    location /.well-known/acme-challenge/ {
        root /var/www/html;
    }

    # 重定向到 HTTPS
    location / {
        return 301 https://$server_name$request_uri;
    }
}

# HTTPS Server
server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name api.example.com;

    # SSL 证书配置（Certbot 自动添加）
    ssl_certificate /etc/letsencrypt/live/api.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/api.example.com/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

    # 安全头
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;
    add_header X-XSS-Protection "1; mode=block";

    # API 反向代理
    location / {
        proxy_pass http://127.0.0.1:9999;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;

        # 超时配置
        proxy_connect_timeout 60s;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;
    }

    # 健康检查端点（可选）
    location /health {
        access_log off;
        return 200 "OK\n";
        add_header Content-Type text/plain;
    }

    # 日志
    access_log /var/log/nginx/api-access.log;
    error_log /var/log/nginx/api-error.log;
}
```

---

### 高级配置选项

#### 1. 限流保护

```nginx
# 在 http 块外添加
limit_req_zone $binary_remote_addr zone=api_limit:10m rate=10r/s;

# 在 location 块内添加
location /api/ {
    limit_req zone=api_limit burst=20 nodelay;
    # ... 其他配置
}
```

#### 2. 压缩响应

```nginx
server {
    # ... 其他配置

    gzip on;
    gzip_vary on;
    gzip_min_length 1024;
    gzip_types text/plain text/css application/json application/javascript text/xml application/xml;
}
```

#### 3. 缓存静态资源

```nginx
location ~* \.(jpg|jpeg|png|gif|ico|css|js)$ {
    expires 7d;
    add_header Cache-Control "public, immutable";
}
```

#### 4. WebSocket 支持

```nginx
location /ws {
    proxy_pass http://127.0.0.1:9999;
    proxy_http_version 1.1;
    proxy_set_header Upgrade $http_upgrade;
    proxy_set_header Connection "upgrade";
}
```

---

## 常见问题

### 问题 1：DNS 解析失败

**错误**：`curl: (6) Could not resolve host: api.example.com`

**解决**：
```bash
# 1. 检查 DNS 配置
dig api.example.com +short

# 2. 如果没返回 IP，检查域名服务商配置
# 3. 等待 DNS 生效（1-10 分钟）

# 4. 临时测试：修改本地 hosts
echo "your-server-ip api.example.com" | sudo tee -a /etc/hosts
```

---

### 问题 2：Nginx 返回 502 Bad Gateway

**原因**：后端服务未运行或端口配置错误

**解决**：
```bash
# 1. 检查后端服务
curl http://127.0.0.1:9999/api/articles

# 2. 检查进程
ps aux | grep backend

# 3. 检查端口
sudo lsof -i :9999

# 4. 查看 Nginx 错误日志
sudo tail -f /var/log/nginx/error.log

# 5. 检查 Nginx 配置中的 proxy_pass 端口
sudo cat /etc/nginx/sites-available/your-site | grep proxy_pass
```

---

### 问题 3：Certbot 申请证书失败

**错误**：`DNS problem: NXDOMAIN looking up A for api.example.com`

**解决**：
```bash
# 1. 验证 DNS 是否生效
dig api.example.com +short

# 2. 如果 DNS 未生效，等待后重试
# 3. 如果 DNS 已生效，检查防火墙
sudo ufw status | grep 80

# 4. 查看详细错误日志
sudo tail -50 /var/log/letsencrypt/letsencrypt.log
```

---

### 问题 4：证书文件不存在

**错误**：`open() "/etc/letsencrypt/options-ssl-nginx.conf" failed`

**解决**：
```bash
# 1. 删除错误配置
sudo rm /etc/nginx/sites-enabled/your-site

# 2. 重新创建只有 HTTP 的配置
# （参考第四步）

# 3. 重新申请证书
sudo certbot --nginx -d api.example.com
```

---

### 问题 5：CORS 错误（跨域问题）

**错误**：浏览器控制台显示 CORS policy 错误

**解决**：在后端代码中配置 CORS，**不推荐在 Nginx 配置**

后端示例（Rust Axum）：
```rust
use tower_http::cors::{CorsLayer, Any};

let cors = CorsLayer::new()
    .allow_origin("https://your-frontend.com".parse::<HeaderValue>().unwrap())
    .allow_methods([Method::GET, Method::POST])
    .allow_headers(Any);

Router::new()
    .route("/api/articles", get(handler))
    .layer(cors);
```

---

### 问题 6：HTTP 没有重定向到 HTTPS

**解决**：
```bash
# 重新运行 certbot 并选择重定向
sudo certbot --nginx -d api.example.com

# 或手动添加重定向（在 HTTP server 块）
location / {
    return 301 https://$server_name$request_uri;
}
```

---

## 维护指南

### 日常检查

```bash
# 1. 检查后端服务状态
sudo systemctl status your-backend-service

# 2. 检查 Nginx 状态
sudo systemctl status nginx

# 3. 查看最近访问日志
sudo tail -50 /var/log/nginx/access.log

# 4. 查看错误日志
sudo tail -50 /var/log/nginx/error.log

# 5. 检查证书有效期
sudo certbot certificates
```

---

### 重启服务

```bash
# 重启后端服务
sudo systemctl restart your-backend-service

# 重载 Nginx 配置（不中断连接）
sudo systemctl reload nginx

# 重启 Nginx（中断连接）
sudo systemctl restart nginx

# 测试配置后重载
sudo nginx -t && sudo systemctl reload nginx
```

---

### 更新配置

```bash
# 1. 编辑配置
sudo nano /etc/nginx/sites-available/your-site

# 2. 测试配置
sudo nginx -t

# 3. 重载 Nginx
sudo systemctl reload nginx

# 4. 验证更改
curl -I https://api.example.com
```

---

### 日志管理

```bash
# 查看实时日志
sudo tail -f /var/log/nginx/access.log

# 统计今天的请求数
sudo cat /var/log/nginx/access.log | grep "$(date +%d/%b/%Y)" | wc -l

# 查看最常访问的 URL
sudo awk '{print $7}' /var/log/nginx/access.log | sort | uniq -c | sort -rn | head -10

# 清理旧日志（可选）
sudo logrotate /etc/logrotate.d/nginx
```

---

### 证书维护

```bash
# 查看证书状态
sudo certbot certificates

# 手动续期
sudo certbot renew

# 测试续期
sudo certbot renew --dry-run

# 删除证书
sudo certbot delete --cert-name api.example.com

# 撤销证书
sudo certbot revoke --cert-path /etc/letsencrypt/live/api.example.com/cert.pem
```

---

### 性能监控

```bash
# 查看 Nginx 连接统计
curl http://127.0.0.1/nginx_status
# 需要在配置中启用 stub_status 模块

# 查看系统资源使用
htop

# 查看网络连接
sudo netstat -tlnp | grep nginx

# 测试响应时间
time curl https://api.example.com/api/articles
```

---

## 一键部署脚本

完整的自动化部署脚本：

```bash
#!/bin/bash

# Nginx + HTTPS 一键部署脚本
# 使用方法: sudo bash deploy-nginx-https.sh

set -e

# ========== 配置区域（修改这里）==========
DOMAIN="api.example.com"           # ← 你的域名
BACKEND_PORT="9999"                # ← 后端端口
EMAIL="admin@example.com"          # ← 证书邮箱
SITE_NAME="your-site"              # ← 站点配置文件名
# ======================================

echo "🚀 开始部署 Nginx + HTTPS for ${DOMAIN}"
echo ""

# 1. 检查权限
if [ "$EUID" -ne 0 ]; then
  echo "❌ 请使用 sudo 运行此脚本"
  exit 1
fi

# 2. 安装依赖
echo "📦 安装 Nginx 和 Certbot..."
apt update
apt install -y nginx certbot python3-certbot-nginx

# 3. 配置防火墙
echo "🔥 配置防火墙..."
ufw allow 80/tcp
ufw allow 443/tcp
echo "✅ 防火墙已开放 80/443 端口"

# 4. 删除默认配置
echo "🗑️  删除默认配置..."
rm -f /etc/nginx/sites-enabled/default

# 5. 创建 Nginx 配置
echo "📝 创建 Nginx 配置..."
cat > /etc/nginx/sites-available/${SITE_NAME} << EOF
server {
    listen 80 default_server;
    listen [::]:80 default_server;
    server_name ${DOMAIN};

    location / {
        proxy_pass http://127.0.0.1:${BACKEND_PORT};
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
    }
}
EOF

# 6. 启用配置
echo "🔗 启用站点配置..."
ln -sf /etc/nginx/sites-available/${SITE_NAME} /etc/nginx/sites-enabled/

# 7. 测试配置
echo "🧪 测试 Nginx 配置..."
if nginx -t; then
    echo "✅ Nginx 配置语法正确"
else
    echo "❌ Nginx 配置语法错误"
    exit 1
fi

# 8. 重载 Nginx
echo "🔄 重载 Nginx..."
systemctl reload nginx

# 9. 测试 HTTP
echo "🧪 测试 HTTP 访问..."
sleep 2
if curl -sf http://127.0.0.1 > /dev/null; then
    echo "✅ HTTP 访问正常"
else
    echo "⚠️  HTTP 访问失败，请检查后端服务"
fi

# 10. 验证 DNS
echo "🌐 验证 DNS 解析..."
if dig ${DOMAIN} +short | grep -q .; then
    echo "✅ DNS 解析成功"
else
    echo "⚠️  DNS 未生效，跳过证书申请"
    echo "请等待 DNS 生效后手动运行："
    echo "sudo certbot --nginx -d ${DOMAIN} --email ${EMAIL} --agree-tos --redirect --non-interactive"
    exit 0
fi

# 11. 申请 SSL 证书
echo "🔐 申请 SSL 证书..."
if certbot --nginx -d ${DOMAIN} --email ${EMAIL} --agree-tos --redirect --non-interactive; then
    echo "✅ SSL 证书申请成功"
else
    echo "❌ SSL 证书申请失败，请检查 DNS 配置和防火墙"
    exit 1
fi

# 12. 验证 HTTPS
echo "🧪 验证 HTTPS..."
sleep 3
if curl -sf https://${DOMAIN} > /dev/null; then
    echo "✅ HTTPS 访问正常"
else
    echo "⚠️  HTTPS 访问失败"
fi

# 13. 显示结果
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ 部署完成！"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "🌐 域名: https://${DOMAIN}"
echo "📡 测试: curl https://${DOMAIN}"
echo ""
echo "🛠️  常用命令:"
echo "  查看日志: sudo tail -f /var/log/nginx/access.log"
echo "  重载配置: sudo systemctl reload nginx"
echo "  查看证书: sudo certbot certificates"
echo ""
