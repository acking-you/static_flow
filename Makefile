.PHONY: help install dev dev-backend dev-frontend build clean test check stop kill-backend kill-frontend fmt lint ci \
	bin bin-cli bin-backend bin-all

# Binary output directory
BIN_DIR ?= ./bin
TARGET_DIR ?= ./target/release

# 默认目标：显示帮助信息
help:
	@echo "StaticFlow 开发工具"
	@echo ""
	@echo "使用方法："
	@echo "  make install           - 安装所有依赖"
	@echo "  make dev               - 一键启动前后端（推荐）"
	@echo "  make dev-backend       - 仅启动后端（端口3000）"
	@echo "  make dev-frontend      - 仅启动前端（端口8080）"
	@echo "  make build             - 构建整个项目"
	@echo "  make check             - 检查代码"
	@echo "  make test              - 运行测试"
	@echo "  make clean             - 清理构建产物"
	@echo "  make stop              - 停止所有服务"
	@echo ""
	@echo "二进制构建："
	@echo "  make bin-cli           - 编译 CLI 二进制（sf-cli）"
	@echo "  make bin-backend       - 编译后端二进制（static-flow-backend）"
	@echo "  make bin-all           - 编译全部 Rust 二进制并导出到 ./bin"
	@echo "  make bin BIN=<name>    - 编译指定 package 二进制并导出到 ./bin"
	@echo "                           例如：make bin BIN=sf-cli"
	@echo ""

# 安装依赖
install:
	@echo "🔧 安装依赖..."
	@rustup target add wasm32-unknown-unknown
	@cargo install trunk --locked || true
	@cd frontend && npm install
	@echo "✅ 依赖安装完成"

# 一键启动前后端
dev:
	@echo "🚀 启动开发环境..."
	@trap 'make stop' EXIT; \
	$(MAKE) dev-backend & \
	sleep 3; \
	$(MAKE) dev-frontend & \
	wait

# 启动后端
dev-backend:
	@echo "🔧 启动后端（http://localhost:3000）..."
	@cd backend && [ -f .env ] || cp .env.example .env
	@cd backend && RUST_LOG=info cargo run

# 启动前端
dev-frontend:
	@echo "🎨 启动前端（http://localhost:8080）..."
	@cd frontend && trunk serve --open

# 构建项目
build:
	@echo "📦 构建项目..."
	@cargo build --workspace --release
	@cd frontend && trunk build --release
	@echo "✅ 构建完成"

# 检查代码
check:
	@cargo check --workspace

# 运行测试
test:
	@cargo test --workspace

# 清理
clean:
	@cargo clean
	@rm -rf frontend/dist
	@rm -rf $(BIN_DIR)

# 停止服务
stop:
	@echo "🛑 停止服务..."
	@-pkill -INT -f "cargo run" 2>/dev/null || true
	@-pkill -INT -f "trunk serve" 2>/dev/null || true
	@sleep 1
	@echo "✅ 已停止"

# 强制停止后端
kill-backend:
	@-pkill -9 -f "static-flow-backend" 2>/dev/null || true

# 强制停止前端
kill-frontend:
	@-pkill -9 -f "trunk serve" 2>/dev/null || true

# 格式化代码
fmt:
	@cargo fmt --all

# Lint 检查
lint:
	@cargo clippy --workspace -- -D warnings

# 完整检查
ci: fmt lint test check
	@echo "✅ 所有检查通过"

# 编译指定 package 的 release binary，并导出到 ./bin
# 用法：make bin BIN=sf-cli
bin:
	@if [ -z "$(BIN)" ]; then \
		echo "❌ 缺少 BIN 参数，用法：make bin BIN=<name>"; \
		echo "   示例：make bin BIN=sf-cli"; \
		exit 1; \
	fi
	@echo "📦 编译 $(BIN) ..."
	@cargo build -p $(BIN) --release
	@mkdir -p $(BIN_DIR)
	@cp $(TARGET_DIR)/$(BIN) $(BIN_DIR)/$(BIN)
	@echo "✅ 输出: $(BIN_DIR)/$(BIN)"

# 编译 CLI binary
bin-cli:
	@echo "📦 编译 sf-cli ..."
	@cargo build -p sf-cli --release
	@mkdir -p $(BIN_DIR)
	@cp $(TARGET_DIR)/sf-cli $(BIN_DIR)/sf-cli
	@echo "✅ 输出: $(BIN_DIR)/sf-cli"

# 编译 backend binary
bin-backend:
	@echo "📦 编译 static-flow-backend ..."
	@cargo build -p static-flow-backend --release
	@mkdir -p $(BIN_DIR)
	@cp $(TARGET_DIR)/static-flow-backend $(BIN_DIR)/static-flow-backend
	@echo "✅ 输出: $(BIN_DIR)/static-flow-backend"

# 编译所有 Rust binary
bin-all: bin-cli bin-backend
	@echo "✅ 全部二进制已导出到 $(BIN_DIR)"
