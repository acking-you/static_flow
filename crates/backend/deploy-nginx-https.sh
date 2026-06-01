#!/bin/bash

# Optional cloud Nginx + HTTPS deployment script (for pb-mapper exposure)
# Usage: sudo DOMAIN=api.yourdomain.com PBMAPPER_PORT=8888 EMAIL=admin@yourdomain.com bash deploy-nginx-https.sh

set -e

DOMAIN="${DOMAIN:-api.example.com}"
PBMAPPER_PORT="${PBMAPPER_PORT:-8888}"
EMAIL="${EMAIL:-admin@example.com}"
SITE_NAME="${SITE_NAME:-staticflow-api}"

echo "🚀 Deploying Nginx + HTTPS for ${DOMAIN} (pb-mapper:${PBMAPPER_PORT})"
echo ""

if [ "$EUID" -ne 0 ]; then
  echo "❌ Please run with sudo"
  exit 1
fi

echo "📦 Installing Nginx and Certbot..."
apt update
apt install -y nginx certbot python3-certbot-nginx

echo "🔥 Configuring firewall..."
ufw allow 80/tcp
ufw allow 443/tcp

echo "📝 Writing Nginx config..."
cat > /etc/nginx/sites-available/${SITE_NAME} << EOF
server {
    listen 80;
    listen [::]:80;
    server_name ${DOMAIN};

    location /.well-known/acme-challenge/ {
        root /var/www/html;
    }

    location / {
        return 301 https://\$server_name\$request_uri;
    }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name ${DOMAIN};

    ssl_certificate /etc/letsencrypt/live/${DOMAIN}/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/${DOMAIN}/privkey.pem;
    include /etc/letsencrypt/options-ssl-nginx.conf;
    ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

    add_header Strict-Transport-Security "max-age=31536000; includeSubDomains" always;
    add_header X-Content-Type-Options nosniff;
    add_header X-Frame-Options DENY;

    location /api/ {
        proxy_pass https://127.0.0.1:${PBMAPPER_PORT}/api/;

        # common when upstream is local self-signed TLS
        proxy_ssl_verify off;
        proxy_ssl_server_name on;

        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;

        proxy_connect_timeout 60s;
        proxy_read_timeout 60s;
        proxy_send_timeout 60s;
    }

    location /health {
        access_log off;
        return 200 "OK\\n";
        add_header Content-Type text/plain;
    }

    location / {
        return 404 '{"error":"Not Found"}';
        add_header Content-Type application/json;
    }
}
EOF

ln -sf /etc/nginx/sites-available/${SITE_NAME} /etc/nginx/sites-enabled/${SITE_NAME}
nginx -t
systemctl reload nginx

echo "🧪 Checking pb-mapper local port..."
if curl -skf "https://127.0.0.1:${PBMAPPER_PORT}/api/articles" > /dev/null; then
    echo "✅ pb-mapper local port is reachable"
else
    echo "⚠️ pb-mapper local port is not reachable: 127.0.0.1:${PBMAPPER_PORT}"
    echo "   Continue anyway; HTTPS setup may still complete."
fi

echo "🔐 Requesting certificate..."
certbot --nginx -d ${DOMAIN} --email ${EMAIL} --agree-tos --redirect --non-interactive

echo "🧪 Verifying HTTPS..."
curl -sf "https://${DOMAIN}/api/articles" > /dev/null && echo "✅ HTTPS API OK"

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "✅ Done"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "API: https://${DOMAIN}/api"
echo ""
