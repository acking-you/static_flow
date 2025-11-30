# StaticFlow 后端服务器直接部署指南

本指南适用于将 StaticFlow 后端**直接部署到服务器**（非 rathole 内网穿透方案）。

## 📐 架构

```
外网 HTTPS 请求 (443)
    ↓
Nginx (反向代理 + SSL 终止)
    ↓
后端进程 (127.0.0.1:9999)
    ↓
文件系统 (/opt/staticflow/content)
```

## ✅ 前置条件

- **服务器**: Ubuntu 20.04/22.04 或 Debian 11+
- **SSH 访问**: 可以 SSH 登录并有 sudo 权限
- **域名**: 已购买域名并配置 DNS A 记录指向服务器 IP
- **本地环境**: 已安装 Rust 和 cargo

## 🚀 快速部署（推荐）

### 1. 本地编译和打包

```bash
# 在项目根目录
cd /path/to/static_flow

# 使用自动化部署脚本
REMOTE_HOST=your-server.com REMOTE_USER=ubuntu ./backend/deploy.sh
```

脚本会自动完成：
- ✅ 编译 release 版本
- ✅ 打包二进制 + 配置文件 + content 目录
- ✅ 上传到服务器
- ✅ 安装 systemd 服务
- ✅ 启动服务并验证

### 2. 配置 Nginx

SSH 登录服务器：

```bash
ssh ubuntu@your-server.com
```

安装 Nginx 和 Certbot：

```bash
sudo apt update
sudo apt install -y nginx certbot python3-certbot-nginx
```

创建 Nginx 配置（**替换域名**）：

```bash
sudo nano /etc/nginx/sites-available/staticflow-api
```

粘贴以下内容（修改 `api.yourdomain.com`）：

```nginx
# HTTP (redirect to HTTPS)
server {
    listen 80;
    listen [::]:80;
    server_name api.yourdomain.com;

    location /.well-known/acme-challenge/ {
        root /var/www/html;
    }

    location / {
        return 301 https://$server_name$request_uri;
    }
}

# HTTPS
server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name api.yourdomain.com;

    # SSL certificates (certbot will add these)
    # ssl_certificate /etc/letsencrypt/live/api.yourdomain.com/fullchain.pem;
    # ssl_certificate_key /etc/letsencrypt/live/api.yourdomain.com/privkey.pem;

    # Security headers
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;
    add_header X-XSS-Protection "1; mode=block";
    add_header Strict-Transport-Security "max-age=31536000" always;

    # API reverse proxy
    location /api/ {
        proxy_pass http://127.0.0.1:9999/api/;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
        proxy_connect_timeout 60s;
        proxy_read_timeout 60s;
    }

    # Health check
    location /health {
        access_log off;
        return 200 "OK\n";
    }

    access_log /var/log/nginx/staticflow-access.log;
    error_log /var/log/nginx/staticflow-error.log;
}
```

启用配置：

```bash
sudo ln -s /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

### 3. 配置 SSL 证书

```bash
sudo certbot --nginx -d api.yourdomain.com
```

按提示输入邮箱并同意条款，Certbot 会自动配置 HTTPS。

### 4. 验证部署

测试后端 API：

```bash
# 本地端口测试
curl http://127.0.0.1:9999/api/articles

# HTTPS 测试
curl https://api.yourdomain.com/api/articles
```

检查服务状态：

```bash
sudo systemctl status staticflow-backend
sudo journalctl -u staticflow-backend -n 50
```

### 5. 配置前端

在 GitHub 仓库设置中添加变量：

1. 访问 `https://github.com/your-username/static_flow/settings/variables/actions`
2. 添加变量：
   - Name: `STATICFLOW_API_BASE`
   - Value: `https://api.yourdomain.com/api`

推送代码触发重新部署：

```bash
git commit --allow-empty -m "Update API endpoint"
git push origin master
```

等待 GitHub Actions 完成后，访问 `https://your-username.github.io` 验证前端。

## 🔧 手动部署（可选）

如果不使用自动化脚本，可以手动操作：

### 1. 编译

```bash
cargo build --release -p static-flow-backend
```

### 2. 上传文件

```bash
# 创建部署包
tar -czf staticflow.tar.gz \
    target/release/static-flow-backend \
    backend/.env.production \
    content/

# 上传
scp staticflow.tar.gz ubuntu@your-server.com:/tmp/
```

### 3. 服务器安装

SSH 登录后：

```bash
cd /tmp
tar -xzf staticflow.tar.gz

# 创建目录
sudo mkdir -p /opt/staticflow/{logs,content/images}

# 复制文件
sudo cp target/release/static-flow-backend /opt/staticflow/
sudo cp backend/.env.production /opt/staticflow/.env
sudo cp -r content/* /opt/staticflow/content/

# 设置权限
sudo chown -R www-data:www-data /opt/staticflow
sudo chmod +x /opt/staticflow/static-flow-backend
```

### 4. 配置 systemd

```bash
sudo nano /etc/systemd/system/staticflow-backend.service
```

粘贴：

```ini
[Unit]
Description=StaticFlow Backend API
After=network.target

[Service]
Type=simple
User=www-data
Group=www-data
WorkingDirectory=/opt/staticflow
ExecStart=/opt/staticflow/static-flow-backend
Restart=always
RestartSec=5
Environment="RUST_LOG=info"

[Install]
WantedBy=multi-user.target
```

启动服务：

```bash
sudo systemctl daemon-reload
sudo systemctl enable staticflow-backend
sudo systemctl start staticflow-backend
sudo systemctl status staticflow-backend
```

## 📊 监控和维护

### 查看日志

```bash
# 实时日志
sudo journalctl -u staticflow-backend -f

# 最近 100 条
sudo journalctl -u staticflow-backend -n 100

# 今天的日志
sudo journalctl -u staticflow-backend --since today
```

### 重启服务

```bash
sudo systemctl restart staticflow-backend
```

### 更新内容

```bash
# 上传新文章到服务器
scp your-article.md ubuntu@your-server.com:/tmp/

# SSH 登录后移动文件
sudo mv /tmp/your-article.md /opt/staticflow/content/
sudo chown www-data:www-data /opt/staticflow/content/your-article.md

# 重启服务加载新内容
sudo systemctl restart staticflow-backend
```

### 更新后端代码

```bash
# 本地重新编译
cargo build --release -p static-flow-backend

# 上传新二进制
scp target/release/static-flow-backend ubuntu@your-server.com:/tmp/

# 服务器替换
ssh ubuntu@your-server.com 'sudo systemctl stop staticflow-backend && \
    sudo mv /tmp/static-flow-backend /opt/staticflow/ && \
    sudo chown www-data:www-data /opt/staticflow/static-flow-backend && \
    sudo chmod +x /opt/staticflow/static-flow-backend && \
    sudo systemctl start staticflow-backend'
```

## 🔐 安全加固

### 防火墙配置

```bash
sudo ufw allow 22/tcp   # SSH
sudo ufw allow 80/tcp   # HTTP
sudo ufw allow 443/tcp  # HTTPS
sudo ufw enable
sudo ufw status
```

### Nginx 限流

在 Nginx 配置中添加（已包含在示例中）：

```nginx
limit_req_zone $binary_remote_addr zone=api_limit:10m rate=10r/s;

location /api/ {
    limit_req zone=api_limit burst=20 nodelay;
    # ...
}
```

### 定期更新证书

Certbot 会自动续期，验证自动续期：

```bash
sudo certbot renew --dry-run
sudo systemctl status certbot.timer
```

## 🐛 故障排查

### 502 Bad Gateway

**原因**: 后端未运行

```bash
# 检查服务状态
sudo systemctl status staticflow-backend

# 查看错误日志
sudo journalctl -u staticflow-backend -n 50

# 手动运行测试
cd /opt/staticflow
sudo -u www-data ./static-flow-backend
```

### CORS 错误

**原因**: 生产环境未配置或配置错误

```bash
# 检查环境变量
sudo cat /opt/staticflow/.env | grep RUST_ENV

# 应该包含: RUST_ENV=production
# 检查 routes.rs 中的 CORS 配置 (backend/src/routes.rs:14-33)
```

### 文章列表为空

**原因**: content 目录路径错误

```bash
# 检查文件是否存在
sudo ls -la /opt/staticflow/content/*.md

# 检查权限
sudo ls -ld /opt/staticflow/content
# 应该是 www-data:www-data

# 查看日志中的路径
sudo journalctl -u staticflow-backend | grep "Content directory"
```

### 证书过期

```bash
# 手动续期
sudo certbot renew

# 重启 Nginx
sudo systemctl reload nginx
```

## 📝 文件结构

服务器上的目录结构：

```
/opt/staticflow/
├── static-flow-backend         # 二进制文件
├── .env                         # 环境变量
├── content/                     # 文章目录
│   ├── post-001.md
│   ├── post-002.md
│   └── images/                  # 图片目录
│       └── example.png
└── logs/                        # 日志目录（可选）
```

## 🔗 相关文档

- [DEPLOYMENT.md](../DEPLOYMENT.md) - rathole 内网穿透方案
- [CLAUDE.md](../CLAUDE.md) - 项目架构说明
- [backend/API.md](./API.md) - API 接口文档

## 📞 支持

遇到问题请查看：
- 后端日志: `sudo journalctl -u staticflow-backend -f`
- Nginx 日志: `sudo tail -f /var/log/nginx/staticflow-error.log`
- GitHub Issues: https://github.com/acking-you/static_flow/issues
