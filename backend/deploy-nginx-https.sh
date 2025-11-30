#!/bin/bash

# Nginx + HTTPS 一键部署脚本
# 使用方法: sudo bash deploy-nginx-https.sh

set -e

# ========== 配置区域（修改这里）==========
DOMAIN="${DOMAIN:-api.example.com}"      # 你的域名
BACKEND_PORT="${BACKEND_PORT:-9999}"     # 后端端口
EMAIL="${EMAIL:-admin@example.com}"      # 证书邮箱
SITE_NAME="${SITE_NAME:-your-site}"      # 站点配置文件名
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
