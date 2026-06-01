#!/bin/bash

# StaticFlow Nginx + HTTPS 一键配置脚本
# 场景（可选层）：云端 Nginx -> pb-mapper server local -> 本地 Nginx(HTTPS) -> 本地 backend

set -e

DOMAIN="${DOMAIN:-api.acking-you.top}"
EMAIL="${EMAIL:-admin@acking-you.top}"          # 可通过环境变量覆盖
PBMAPPER_PORT="${PBMAPPER_PORT:-8888}"          # 云端 pb-mapper server local 端口

echo "🚀 开始配置 Nginx + HTTPS for ${DOMAIN} (pb-mapper:${PBMAPPER_PORT})"
echo ""

# 1. 安装依赖
echo "📦 安装 Nginx 和 Certbot..."
sudo apt update
sudo apt install -y nginx certbot python3-certbot-nginx

# 2. 配置防火墙
echo "🔥 配置防火墙规则..."
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
echo "✅ 防火墙已开放 80/443 端口"

# 3. 创建 Nginx 配置
echo "📝 创建 Nginx 配置..."
sudo tee /etc/nginx/sites-available/staticflow-api > /dev/null << 'EOF'
# HTTP Server (重定向到 HTTPS)
server {
    listen 80;
    listen [::]:80;
    server_name __DOMAIN__;

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
    server_name __DOMAIN__;

    # SSL 证书路径（certbot 会自动配置）
    ssl_certificate /etc/letsencrypt/live/__DOMAIN__/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/__DOMAIN__/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

    # 安全头
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;
    add_header X-XSS-Protection "1; mode=block";
    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;

    # API 反向代理
    location /api/ {
        # 代理到 pb-mapper server local 端口
        proxy_pass https://127.0.0.1:__PBMAPPER_PORT__/api/;

        # 当上游是本地自签证书时
        proxy_ssl_verify off;
        proxy_ssl_server_name on;

        # 请求头
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;

        # 超时配置
        proxy_connect_timeout 60s;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;

        # WebSocket 支持
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }

    # 健康检查
    location /health {
        access_log off;
        return 200 "OK\n";
        add_header Content-Type text/plain;
    }

    # 根路径
    location = / {
        return 200 '{"service":"StaticFlow API","status":"running","version":"1.0.0"}';
        add_header Content-Type application/json;
    }

    # 日志
    access_log /var/log/nginx/staticflow-access.log;
    error_log /var/log/nginx/staticflow-error.log;
}
EOF

sudo sed -i "s/__DOMAIN__/${DOMAIN}/g" /etc/nginx/sites-available/staticflow-api
sudo sed -i "s/__PBMAPPER_PORT__/${PBMAPPER_PORT}/g" /etc/nginx/sites-available/staticflow-api

echo "✅ Nginx 配置已创建"

# 4. 启用站点
echo "🔗 启用站点配置..."
sudo ln -sf /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/

# 5. 测试配置
echo "🧪 测试 Nginx 配置..."
if sudo nginx -t; then
    echo "✅ Nginx 配置语法正确"
else
    echo "❌ Nginx 配置语法错误，请检查"
    exit 1
fi

# 6. 重载 Nginx
echo "🔄 重载 Nginx..."
sudo systemctl reload nginx

# 7. 申请 SSL 证书
echo ""
echo "🔐 申请 SSL 证书..."
echo "域名: ${DOMAIN}"
echo "邮箱: ${EMAIL}"
echo ""

# 注意：首次运行需要 DNS 已生效
if sudo certbot --nginx -d ${DOMAIN} --email ${EMAIL} --agree-tos --non-interactive --redirect; then
    echo "✅ SSL 证书申请成功"
else
    echo "⚠️  SSL 证书申请失败，可能原因："
    echo "  1. DNS 记录未生效（检查: dig ${DOMAIN}）"
    echo "  2. 防火墙未开放 80 端口"
    echo "  3. Nginx 配置错误"
    echo ""
    echo "手动运行: sudo certbot --nginx -d ${DOMAIN}"
    exit 1
fi

# 8. 验证部署
echo ""
echo "🧪 验证部署..."

echo "1. 测试 pb-mapper 映射端口..."
if curl -skf https://127.0.0.1:${PBMAPPER_PORT}/api/articles > /dev/null; then
    echo "   ✅ pb-mapper 映射端口正常"
else
    echo "   ❌ pb-mapper 映射端口无响应"
fi

echo "2. 测试 HTTPS API..."
sleep 2
if curl -sf https://${DOMAIN}/api/articles > /dev/null; then
    echo "   ✅ HTTPS API 正常"
    echo ""
    echo "   响应示例："
    curl -s https://${DOMAIN}/api/articles | head -c 200
    echo "..."
else
    echo "   ❌ HTTPS API 无响应"
fi

# 9. 显示结果
echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ 部署完成！"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo "🌐 API 地址: https://${DOMAIN}/api"
echo ""
echo "📡 测试端点:"
echo "  curl https://${DOMAIN}/api/articles"
echo "  curl https://${DOMAIN}/api/tags"
echo "  curl https://${DOMAIN}/api/categories"
echo ""
echo "📋 下一步:"
echo "  1. 更新 GitHub Actions 变量:"
echo "     STATICFLOW_API_BASE=https://${DOMAIN}/api"
echo ""
echo "  2. 推送代码触发前端重新部署"
echo ""
echo "  3. 访问前端验证:"
echo "     https://acking-you.github.io"
echo ""
echo "🛠️  常用命令:"
echo "  查看日志: sudo journalctl -u staticflow-backend -f"
echo "  重启后端: sudo systemctl restart staticflow-backend"
echo "  查看 Nginx 日志: sudo tail -f /var/log/nginx/staticflow-error.log"
echo "  续期证书: sudo certbot renew --dry-run"
echo ""
