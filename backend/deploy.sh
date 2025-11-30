#!/bin/bash

# StaticFlow Backend Deployment Script
# This script automates the deployment of the backend to a production server

set -e  # Exit on any error

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Configuration
DEPLOY_DIR="/opt/staticflow"
BINARY_NAME="static-flow-backend"
SERVICE_NAME="staticflow-backend"
REMOTE_USER="${REMOTE_USER:-ubuntu}"  # Default to ubuntu, override with env var
REMOTE_HOST="${REMOTE_HOST:-}"        # Must be set

# Functions
log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_prerequisites() {
    log_info "Checking prerequisites..."

    if [ -z "$REMOTE_HOST" ]; then
        log_error "REMOTE_HOST environment variable is not set"
        echo "Usage: REMOTE_HOST=your-server.com REMOTE_USER=ubuntu ./deploy.sh"
        exit 1
    fi

    if ! command -v ssh &> /dev/null; then
        log_error "ssh command not found"
        exit 1
    fi

    if [ ! -f "target/release/${BINARY_NAME}" ]; then
        log_error "Binary not found. Please run 'cargo build --release -p static-flow-backend' first"
        exit 1
    fi

    if [ ! -d "content" ]; then
        log_error "content directory not found"
        exit 1
    fi

    log_info "Prerequisites check passed"
}

build_backend() {
    log_info "Building backend in release mode..."
    cargo build --release -p static-flow-backend
    log_info "Build completed"
}

prepare_deployment_package() {
    log_info "Preparing deployment package..."

    local TEMP_DIR=$(mktemp -d)
    mkdir -p "$TEMP_DIR/staticflow"

    # Copy binary
    cp "target/release/${BINARY_NAME}" "$TEMP_DIR/staticflow/"

    # Copy configuration
    cp "backend/.env.production" "$TEMP_DIR/staticflow/.env"
    cp "backend/staticflow-backend.service" "$TEMP_DIR/staticflow/"

    # Copy content directory (for testing)
    log_info "Copying content directory..."
    cp -r "content" "$TEMP_DIR/staticflow/"

    # Count files
    local ARTICLE_COUNT=$(find "$TEMP_DIR/staticflow/content" -name "*.md" | wc -l)
    local IMAGE_COUNT=$(find "$TEMP_DIR/staticflow/content/images" -type f 2>/dev/null | wc -l || echo 0)
    log_info "Packaged ${ARTICLE_COUNT} articles and ${IMAGE_COUNT} images"

    # Create archive
    tar -czf "staticflow-deploy.tar.gz" -C "$TEMP_DIR" staticflow

    rm -rf "$TEMP_DIR"

    local PACKAGE_SIZE=$(du -h staticflow-deploy.tar.gz | cut -f1)
    log_info "Deployment package created: staticflow-deploy.tar.gz (${PACKAGE_SIZE})"
}

upload_to_server() {
    log_info "Uploading to server ${REMOTE_USER}@${REMOTE_HOST}..."

    scp "staticflow-deploy.tar.gz" "${REMOTE_USER}@${REMOTE_HOST}:/tmp/"

    log_info "Upload completed"
}

install_on_server() {
    log_info "Installing on server..."

    ssh "${REMOTE_USER}@${REMOTE_HOST}" bash << 'EOF'
set -e

# Extract archive
cd /tmp
tar -xzf staticflow-deploy.tar.gz

# Backup existing content if it exists
if [ -d "/opt/staticflow/content" ]; then
    echo "Backing up existing content..."
    sudo mv /opt/staticflow/content "/opt/staticflow/content.backup.$(date +%Y%m%d_%H%M%S)"
fi

# Create deployment directory structure
sudo mkdir -p /opt/staticflow/logs

# Copy files
sudo cp /tmp/staticflow/static-flow-backend /opt/staticflow/
sudo cp /tmp/staticflow/.env /opt/staticflow/
sudo chmod +x /opt/staticflow/static-flow-backend

# Copy content directory
sudo cp -r /tmp/staticflow/content /opt/staticflow/

# Install systemd service
sudo cp /tmp/staticflow/staticflow-backend.service /etc/systemd/system/

# Set permissions
sudo chown -R www-data:www-data /opt/staticflow

# Reload systemd
sudo systemctl daemon-reload

# Clean up
rm -rf /tmp/staticflow /tmp/staticflow-deploy.tar.gz

echo "Installation completed"
EOF

    log_info "Installation completed"
}

start_service() {
    log_info "Starting service..."

    ssh "${REMOTE_USER}@${REMOTE_HOST}" bash << 'EOF'
set -e

# Enable and start service
sudo systemctl enable staticflow-backend
sudo systemctl restart staticflow-backend

# Wait a moment for service to start
sleep 3

# Check status
echo "=== Service Status ==="
sudo systemctl status staticflow-backend --no-pager || true

echo ""
echo "=== Testing API Endpoints ==="

# Test articles list
echo "Testing /api/articles..."
curl -s http://127.0.0.1:9999/api/articles | head -c 200
echo "..."

echo ""
echo "Testing /api/tags..."
curl -s http://127.0.0.1:9999/api/tags | head -c 200
echo "..."

echo ""
echo "Testing /api/categories..."
curl -s http://127.0.0.1:9999/api/categories | head -c 200
echo "..."

echo ""
echo "=== API Tests Completed ==="
EOF

    log_info "Service started and tested"
}

show_next_steps() {
    log_info "Deployment completed successfully!"
    echo ""
    echo "✅ Backend deployed with test content"
    echo ""
    echo "Next steps:"
    echo "1. Configure Nginx (see backend/nginx.conf.example)"
    echo "   sudo nano /etc/nginx/sites-available/staticflow-api"
    echo ""
    echo "2. Enable Nginx site"
    echo "   sudo ln -s /etc/nginx/sites-available/staticflow-api /etc/nginx/sites-enabled/"
    echo "   sudo nginx -t"
    echo "   sudo systemctl reload nginx"
    echo ""
    echo "3. Setup SSL with Let's Encrypt"
    echo "   sudo certbot --nginx -d api.yourdomain.com"
    echo ""
    echo "4. Update GitHub Actions variable"
    echo "   STATICFLOW_API_BASE=https://api.yourdomain.com/api"
    echo ""
    echo "5. Test HTTPS endpoint"
    echo "   curl https://api.yourdomain.com/api/articles"
    echo ""
    echo "Useful commands:"
    echo "  Check logs:   ssh ${REMOTE_USER}@${REMOTE_HOST} 'sudo journalctl -u staticflow-backend -f'"
    echo "  Restart:      ssh ${REMOTE_USER}@${REMOTE_HOST} 'sudo systemctl restart staticflow-backend'"
    echo "  Check status: ssh ${REMOTE_USER}@${REMOTE_HOST} 'sudo systemctl status staticflow-backend'"
    echo "  Test API:     ssh ${REMOTE_USER}@${REMOTE_HOST} 'curl http://127.0.0.1:9999/api/articles'"
}

# Main execution
main() {
    log_info "Starting StaticFlow Backend Deployment"
    echo ""

    check_prerequisites
    build_backend
    prepare_deployment_package
    upload_to_server
    install_on_server
    start_service
    show_next_steps

    # Clean up local files
    rm -f staticflow-deploy.tar.gz
}

# Run main function
main
