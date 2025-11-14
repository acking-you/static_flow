# StaticFlow 前端技术栈实现原理

> 本文档详细解释 StaticFlow 前端的技术架构、编译流程、以及常见问题。

## 目录

- [技术栈概览](#技术栈概览)
- [Trunk 构建工具详解](#trunk-构建工具详解)
- [Rust → WebAssembly 编译流程](#rust--webassembly-编译流程)
- [静态资源管理](#静态资源管理)
- [开发工作流](#开发工作流)
- [常见问题](#常见问题)

---

## 技术栈概览

### 核心技术

| 技术 | 版本 | 作用 |
|------|------|------|
| **Rust** | nightly-2025-10-28 | 编程语言 |
| **Yew** | 0.21 | 前端框架（类似 React） |
| **WebAssembly** | - | 编译目标格式 |
| **Trunk** | 0.20.3 | 构建工具和开发服务器 |
| **wasm-bindgen** | 0.2 | JS ↔ WASM 桥接 |
| **web-sys** | 0.3 | 浏览器 API 绑定 |
| **yew-router** | 0.18 | 客户端路由 |
| **pulldown-cmark** | 0.9 | Markdown 解析器 |

### 架构对比

```
传统 JavaScript 前端栈         StaticFlow Rust 前端栈
─────────────────────         ────────────────────
React/Vue                  →  Yew
webpack/vite               →  Trunk
JavaScript                 →  Rust
Node.js runtime            →  WebAssembly VM
npm/yarn                   →  Cargo
```

---

## Trunk 构建工具详解

### 什么是 Trunk？

**Trunk** 是专为 Rust WebAssembly 项目设计的构建工具和开发服务器，相当于 JavaScript 生态中的 **webpack** 或 **vite**。

### 核心职责

#### 1. 编译 Rust → WASM
```bash
# Trunk 内部调用
rustc --target wasm32-unknown-unknown
```
将 Rust 代码编译为 `.wasm` 二进制文件（浏览器可执行的格式）。

#### 2. 生成 JavaScript 胶水代码
通过 `wasm-bindgen` 自动生成 JS 和 WASM 之间的桥接代码：
```javascript
// 自动生成的 JS 代码示例
import * as wasm from './static_flow_frontend_bg.wasm';

export function __wbg_init() {
    return wasm.__wbg_init();
}
```

#### 3. 处理 HTML 资源声明
解析 `index.html` 中的 `data-trunk` 属性：
```html
<!-- 告诉 Trunk 编译 Rust 代码并注入 -->
<link data-trunk rel="rust" data-wasm-opt="z" />

<!-- 告诉 Trunk 复制 static/ 目录到输出 -->
<link data-trunk rel="copy-dir" href="static" />
```

Trunk 会自动：
- 注入编译后的 `.wasm` 文件加载脚本
- 复制静态资源到 `dist/`
- 处理 CSS/图片等资源引用

#### 4. 资源管理
- 复制静态文件（CSS、图片、字体）到输出目录
- 处理资源路径重写
- 生成 Hash 文件名（用于缓存控制）

#### 5. 开发服务器
```bash
trunk serve --port 8080 --open
```
- 启动本地开发服务器（默认 `http://127.0.0.1:8080`）
- 监听文件变化，自动重新编译
- 通过 WebSocket 推送热重载信号
- 支持代理后端 API（通过 `proxy_backend` 配置）

#### 6. 优化
生产构建时使用 `wasm-opt` 压缩 WASM 体积：
```bash
trunk build --release
# 内部调用 wasm-opt -Oz output.wasm
```

### Trunk 配置文件

`frontend/Trunk.toml`：
```toml
[build]
target = "index.html"       # 入口 HTML
dist = "dist"               # 输出目录
public_url = "/"            # 资源路径前缀
filehash = true             # 启用文件 Hash 命名
release = true              # 默认 release 模式

[serve]
port = 8080                 # 开发服务器端口
open = false                # 是否自动打开浏览器
# proxy_backend = "http://localhost:3000"  # 代理后端（Week 2 启用）
# proxy_ws = true
```

---

## Rust → WebAssembly 编译流程

### 完整流程图

```
┌─────────────────────────────────────────────────────────────┐
│                      开发阶段                                 │
└─────────────────────────────────────────────────────────────┘

1. 编辑源代码
   frontend/src/*.rs
         │
         ↓
2. trunk serve 监听文件变化
         │
         ↓
3. 调用 Cargo 编译
   $ cargo build --target wasm32-unknown-unknown
         │
         ↓
4. rustc 编译生成 WASM 二进制
   target/wasm32-unknown-unknown/debug/*.wasm
         │
         ↓
5. wasm-bindgen 生成 JS 绑定代码
   - static_flow_frontend_bg.wasm (WASM 模块)
   - static_flow_frontend.js (JS 胶水代码)
         │
         ↓
6. Trunk 复制静态资源
   frontend/static/ → frontend/dist/static/
         │
         ↓
7. Trunk 将所有产物打包到 dist/
   ├── index.html (注入了 WASM 加载代码)
   ├── static_flow_frontend-[hash].wasm
   ├── static_flow_frontend-[hash].js
   └── static/
       ├── styles.css
       ├── avatar.jpg
       └── ...
         │
         ↓
8. Trunk 开发服务器提供 dist/ 内容
   http://127.0.0.1:8080
         │
         ↓
9. 浏览器通过 WebSocket 接收热重载信号

┌─────────────────────────────────────────────────────────────┐
│                     浏览器运行阶段                            │
└─────────────────────────────────────────────────────────────┘

1. 浏览器加载 index.html
   http://127.0.0.1:8080/index.html
         │
         ↓
2. HTML 中的 <script> 标签加载 JS 胶水代码
   <script src="/static_flow_frontend-abc123.js"></script>
         │
         ↓
3. JS 代码异步获取 WASM 文件
   fetch('/static_flow_frontend-abc123.wasm')
         │
         ↓
4. WebAssembly.instantiate() 初始化 WASM 模块
   将字节码加载到 WebAssembly VM
         │
         ↓
5. wasm-bindgen 建立 JS ↔ WASM 双向调用桥梁
   - Rust 可以调用浏览器 API (document.querySelector)
   - JS 可以调用 Rust 函数
         │
         ↓
6. Yew 框架启动
   yew::Renderer::new().render()
         │
         ↓
7. 挂载根组件到 <body>
   虚拟 DOM 开始工作
         │
         ↓
8. 用户交互 → 事件处理器在 WASM 中执行
   onclick={move |_| { /* Rust 代码 */ }}
         │
         ↓
9. 状态更新 → 虚拟 DOM diff → 更新真实 DOM
   通过 web-sys 调用浏览器 DOM API
```

### 关键概念解释

#### WASM 是什么？
**WebAssembly** 是一种**二进制指令格式**，可以在浏览器中以**接近原生速度**运行。类比：
- `.wasm` 文件 ≈ 编译后的 `.exe` 可执行文件
- WebAssembly VM ≈ 操作系统的进程执行环境

#### wasm-bindgen 的作用
解决 **Rust 和 JavaScript 类型不兼容**的问题：

```rust
// Rust 代码
#[wasm_bindgen]
pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}
```

自动生成的 JS 代码：
```javascript
// 自动生成的绑定
export function greet(name) {
    const ptr0 = passStringToWasm0(name);
    const ret = wasm.greet(ptr0, len0);
    return takeStringFromWasm0(ret);
}
```

#### web-sys 的作用
提供 **浏览器 API 的 Rust 绑定**：

```rust
use web_sys::window;

// Rust 代码调用浏览器 API
let document = window()
    .unwrap()
    .document()
    .unwrap();

let element = document
    .query_selector(".my-class")
    .unwrap();
```

等价于 JavaScript：
```javascript
const element = document.querySelector('.my-class');
```

---

## 静态资源管理

### 文件夹职责

| 路径 | 作用 | 谁使用 | 是否提交 Git |
|------|------|--------|-------------|
| `frontend/static/` | **源文件目录**（开发者编辑） | Trunk 读取 | ✅ 是 |
| `frontend/dist/static/` | **构建产物**（自动生成） | 浏览器加载 | ❌ 否（被 .gitignore） |

### 依赖关系图

```
开发者编辑
    ↓
frontend/static/styles.css
    │
    │ (Trunk 每次构建时复制)
    ↓
frontend/dist/static/styles.css
    │
    │ (浏览器 HTTP 请求)
    ↓
http://127.0.0.1:8080/static/styles.css
```

### 浏览器实际加载路径

当你在 HTML/CSS/Rust 中引用静态资源时：
```html
<!-- index.html -->
<img src="/static/avatar.jpg" />
```

```css
/* styles.css */
background-image: url('/static/hero.jpg');
```

```rust
// Rust 代码
html! { <img src="/static/avatar.jpg" /> }
```

实际加载流程：
1. 浏览器请求 `http://127.0.0.1:8080/static/avatar.jpg`
2. Trunk 开发服务器映射到 `frontend/dist/static/avatar.jpg`
3. 返回文件内容

### 必要文件清单

**frontend/static/ 中必须保留：**

| 文件 | 作用 | 可删除 |
|------|------|--------|
| `styles.css` | 核心样式表 | ❌ |
| `avatar.jpg` | 首页头像 | ❌ |
| `favicon.ico` | 浏览器标签图标 | ❌ |
| `favicon-16x16.png` | 多尺寸图标 | ❌ |
| `favicon-32x32.png` | 多尺寸图标 | ❌ |
| `apple-touch-icon.png` | iOS 主屏幕图标 | ❌ |
| `site.webmanifest` | PWA 配置 | ❌ |
| `svg/loading.min.svg` | 加载动画 | ❌ |

**可以安全删除：**
- `frontend/dist/` **整个目录** - 每次构建自动重新生成
- `.gitkeep` 文件 - 仅用于 Git 追踪空目录，现在有内容了

### 验证方法

```bash
# 1. 删除 dist 目录测试
cd frontend
rm -rf dist

# 2. 重新构建
trunk serve

# 3. 检查 dist/static/ 是否正确生成
ls -la dist/static/

# 4. 浏览器访问检查样式是否正常
# http://127.0.0.1:8080
```

---

## 开发工作流

### 日常开发

```bash
# 1. 启动开发服务器（自动监听文件变化）
cd frontend
trunk serve

# 2. 浏览器访问
# http://127.0.0.1:8080

# 3. 编辑代码
# - 修改 src/*.rs 文件
# - 修改 static/styles.css
# - 保存后 Trunk 自动重新编译
# - 浏览器自动刷新（热重载）

# 4. 查看编译错误
# 终端会实时显示 Rust 编译错误
```

### 生产构建

```bash
# 构建优化后的生产版本
cd frontend
trunk build --release

# 产物位于 frontend/dist/
# - WASM 文件已压缩（wasm-opt -Oz）
# - 文件名包含 Hash（缓存优化）
# - 可直接部署到静态服务器
```

### 调试技巧

#### 1. 查看 Rust Panic 信息
在浏览器控制台查看详细的 panic 堆栈：
```rust
// main.rs 中添加
use console_error_panic_hook;
panic::set_hook(Box::new(console_error_panic_hook::hook));
```

#### 2. 启用日志
```rust
use web_sys::console;

// 在 Rust 中打印日志到浏览器控制台
console::log_1(&"Debug message".into());
```

#### 3. 查看 WASM 加载失败
打开浏览器 DevTools → Network 标签页 → 筛选 `.wasm` 文件：
- 检查 HTTP 状态码（应为 200）
- 检查 MIME 类型（应为 `application/wasm`）
- 检查文件大小（是否成功下载）

#### 4. 性能分析
```bash
# 查看 WASM 文件大小
ls -lh dist/*.wasm

# 分析 WASM 模块内容
wasm-objdump -x dist/*.wasm | less
```

---

## 常见问题

### Q1: 为什么有两个 Cargo.lock？

```
/home/ts_user/web/static_flow/Cargo.lock        # Workspace 级别
/home/ts_user/web/static_flow/frontend/Cargo.lock  # 子项目级别
```

**原因**：
- Workspace 的 `Cargo.lock` 锁定所有成员项目的依赖版本
- `frontend/Cargo.lock` 可能是之前独立项目遗留的

**解决方案**：
```bash
# 删除子项目的 Cargo.lock，只保留根目录的
rm frontend/Cargo.lock
```

### Q2: 修改 CSS 后样式不生效？

**可能原因**：
1. 浏览器缓存 - 按 `Ctrl + F5` 强制刷新
2. 编辑了错误的文件 - 确保编辑 `frontend/static/styles.css`（不是 `dist/`）
3. Trunk 未检测到变化 - 重启 `trunk serve`

### Q3: trunk serve 报错 "address already in use"

**原因**：端口 8080 被占用

**解决方案**：
```bash
# 方法 1：使用其他端口
trunk serve --port 8888

# 方法 2：杀死占用进程
lsof -ti:8080 | xargs kill -9
```

### Q4: Rust 编译很慢怎么办？

**优化方法**：
```toml
# Cargo.toml - 开发时依赖用 release 优化
[profile.dev.package."*"]
opt-level = 2
```

```bash
# 使用 sccache 缓存编译结果
cargo install sccache
export RUSTC_WRAPPER=sccache
```

### Q5: WASM 文件太大怎么办？

**当前配置**（Cargo.toml）：
```toml
[profile.release]
opt-level = "z"      # 最小体积优化
lto = true           # 链接时优化
codegen-units = 1    # 单一代码生成单元
strip = true         # 移除调试符号
```

**进一步优化**：
```bash
# 手动运行 wasm-opt（需安装 binaryen）
wasm-opt -Oz -o optimized.wasm dist/*.wasm
```

### Q6: 切换工具链后报错 "mismatched ABI"

**错误示例**：
```
proc macro server error: mismatched ABI
expected: `rustc 1.89.0`, got `rustc 1.93.0-nightly`
```

**原因**：旧版本编译的缓存与新版本不兼容

**解决方案**：
```bash
# 清理所有编译缓存
cargo clean
cd frontend && cargo clean

# 重新编译
cargo check --workspace
```

### Q7: 为什么有些依赖用 `workspace = true`？

**Workspace 依赖管理**（根目录 Cargo.toml）：
```toml
[workspace.dependencies]
yew = { version = "0.21", features = ["csr"] }
```

**子项目引用**（frontend/Cargo.toml）：
```toml
[dependencies]
yew = { workspace = true }
```

**优点**：
- 统一管理版本，避免不同子项目版本冲突
- 升级依赖时只需修改一处
- 减少 `Cargo.lock` 冲突

---

## 技术选型理由

### 为什么选择 Rust + WASM？

| 对比项 | JavaScript | Rust + WASM |
|--------|-----------|-------------|
| **性能** | 解释执行，JIT 优化 | 编译为机器码，接近原生速度 |
| **类型安全** | 动态类型，运行时错误 | 强类型，编译时检查 |
| **内存管理** | GC 自动管理，可能卡顿 | 编译时所有权检查，零 GC 开销 |
| **包体积** | 较小（几百 KB） | 较大（几百 KB ~ 几 MB） |
| **生态成熟度** | 非常成熟 | 快速发展中 |
| **学习曲线** | 平缓 | 陡峭 |

**StaticFlow 选择 Rust 的原因**：
1. **学习目的** - 探索 Rust 前端开发范式
2. **类型安全** - 利用 Rust 的强类型系统减少 bug
3. **代码复用** - 前后端共享 Rust 代码（`shared/` crate）
4. **长期维护** - Rust 的编译时保证降低重构成本

### 为什么选择 Yew 而非其他框架？

| 框架 | 特点 |
|------|------|
| **Yew** | 最接近 React 的 API，社区活跃 |
| **Leptos** | 性能更高，但生态较新 |
| **Dioxus** | 支持跨平台（Web/Desktop/Mobile），API 类似 React |
| **Sycamore** | 基于细粒度响应式，无虚拟 DOM |

**选择 Yew 的理由**：
- API 设计与 React 最接近，降低学习成本
- 文档完善，社区支持好
- 稳定性高，已被多个生产项目验证

---

## 参考资源

### 官方文档
- **Yew 官方文档**: https://yew.rs/docs/
- **Trunk 文档**: https://trunkrs.dev/
- **wasm-bindgen 书**: https://rustwasm.github.io/wasm-bindgen/
- **Rust WASM 书**: https://rustwasm.github.io/book/

### 社区资源
- **Yew GitHub**: https://github.com/yewstack/yew
- **Awesome Yew**: https://github.com/jetli/awesome-yew
- **Rust WASM 示例**: https://github.com/rustwasm/wasm-bindgen/tree/main/examples

### 工具安装
```bash
# 安装 Rust 工具链
rustup install nightly-2025-10-28
rustup target add wasm32-unknown-unknown

# 安装 Trunk
cargo install trunk

# 安装 wasm-opt（可选，用于进一步优化）
cargo install wasm-opt
```

---

**更新日期**: 2025-11-15
**维护者**: Claude Code
