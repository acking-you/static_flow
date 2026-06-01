# GitHub Pages SPA 路由问题与解决方案

## 📋 问题描述

在将 Yew WASM 单页应用（SPA）部署到 GitHub Pages 后，遇到经典的路由刷新 404 问题：

### 复现步骤

1. 访问主页：`https://acking-you.github.io` ✅ 正常
2. 通过导航进入其他页面：`https://acking-you.github.io/latest` ✅ 正常
3. **在 `/latest` 页面刷新浏览器** ❌ **404 错误**
4. 直接访问或分享 `/latest` 链接 ❌ **404 错误**

### 问题表现

```
GET https://acking-you.github.io/latest
→ GitHub Pages: 404 Not Found
```

## 🔍 根本原因

### 客户端路由 vs 服务器端路由的冲突

```
┌─────────────────────────────────────────────────────────────┐
│  单页应用（SPA）工作原理                                        │
├─────────────────────────────────────────────────────────────┤
│  1. 所有路由由前端 JavaScript（Yew Router）处理                │
│  2. 实际上只有一个 HTML 文件（index.html）                      │
│  3. /latest、/posts、/article/123 都是"虚拟路由"               │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│  GitHub Pages 服务器行为                                       │
├─────────────────────────────────────────────────────────────┤
│  用户请求：/latest                                             │
│  服务器查找：                                                  │
│    - /latest 目录？ ❌ 不存在                                  │
│    - /latest.html 文件？ ❌ 不存在                             │
│    - /latest/index.html？ ❌ 不存在                           │
│  结果：返回 404 Not Found                                      │
│                                                               │
│  问题：前端 JavaScript 还没有机会运行就被 404 阻止了！           │
└─────────────────────────────────────────────────────────────┘
```

### 为什么从主页导航正常？

```javascript
// 从主页点击链接到 /latest
// 1. index.html 已加载，Yew 应用已运行
// 2. Yew Router 拦截点击事件
// 3. 通过 history.pushState() 改变 URL（不向服务器发请求）
// 4. 渲染对应组件
// ✅ 全程不经过服务器，纯客户端操作

// 直接访问或刷新 /latest
// 1. 浏览器向服务器请求 /latest
// 2. 服务器找不到文件，返回 404
// ❌ JavaScript 无法执行
```

## ✅ 解决方案

### 核心思路：404.html 重定向 Hack

利用 GitHub Pages 的特性：**当文件不存在时，会自动返回根目录的 `404.html`，但保持原始 URL 不变**。

### 实现步骤

#### 1. 创建 404.html 捕获并保存原始 URL

**文件位置：** `crates/frontend/404.html`（与 `index.html` 同级）

```html
<!DOCTYPE html>
<html lang="zh-CN">
<head>
    <meta charset="UTF-8">
    <title>Redirecting...</title>
    <script>
        // GitHub Pages SPA Router Hack
        // 捕获当前路径并重定向到 index.html

        // 获取完整路径（路径 + 查询参数 + hash）
        var path = window.location.pathname;
        var search = window.location.search;
        var hash = window.location.hash;

        // 保存到 sessionStorage（只在当前标签页有效）
        sessionStorage.setItem('redirectPath', path + search + hash);

        // 重定向到根路径
        window.location.replace('/');
    </script>
</head>
<body>
    <p>Redirecting to app...</p>
</body>
</html>
```

**关键设计：**
- ✅ 使用 `sessionStorage`：只在当前标签页有效，关闭后自动清除
- ✅ 使用 `window.location.replace()`：不留历史记录，避免无限循环
- ❌ 不使用 `localStorage`：会污染其他标签页
- ❌ 不使用 Query String：会暴露在 URL，影响分享链接

#### 2. 修改 index.html 恢复原始 URL

**文件位置：** `crates/frontend/index.html`（在 `<head>` 最前面添加）

```html
<head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>StaticFlow</title>

    <!-- GitHub Pages SPA Router: 恢复原始 URL -->
    <script>
      (function() {
        var redirectPath = sessionStorage.getItem('redirectPath');
        if (redirectPath) {
          sessionStorage.removeItem('redirectPath');  // 清理
          history.replaceState(null, '', redirectPath);  // 恢复 URL
        }
      })();
    </script>

    <!-- 其他内容... -->
</head>
```

**关键 API：**
```javascript
history.replaceState(null, '', '/latest');
// 1. 修改浏览器地址栏和历史记录
// 2. 不触发页面重新加载（不向服务器发请求）
// 3. Yew Router 会读取新 URL 并渲染对应页面
```

#### 3. 配置 Trunk 和 GitHub Actions

**问题：** Trunk 会将 `static/` 目录下的文件复制到 `dist/static/`，而 GitHub Pages 只识别根目录的 `404.html`。

**尝试过的方案（均失败）：**
- ❌ `[[copy]]` 指令：Trunk 不支持
- ❌ `[[assets]]` 指令：只对 index.html 引用的资源有效
- ❌ `post_build` hook：会被 "applying new distribution" 步骤覆盖

**最终方案：在 GitHub Actions 中手动复制**

**Trunk.toml：**
```toml
# 不需要额外配置，404.html 放在 frontend/ 根目录即可
```

**GitHub Actions workflow（`.github/workflows/deploy.yml`）：**
```yaml
- name: Build frontend (production)
  working-directory: frontend
  run: trunk build --release

- name: Copy 404.html to dist root (GitHub Pages SPA routing)
  working-directory: frontend
  run: cp 404.html dist/404.html  # 手动复制到 dist 根目录

- name: Deploy to User Pages (acking-you.github.io)
  uses: peaceiris/actions-gh-pages@v3
  with:
    personal_token: ${{ secrets.PERSONAL_ACCESS_TOKEN }}
    external_repository: acking-you/acking-you.github.io
    publish_dir: crates/frontend/dist
```

## 📊 完整流程图

```
用户刷新 /latest 页面
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 1: 浏览器请求                                            │
│ GET https://acking-you.github.io/latest                      │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 2: GitHub Pages 查找文件                                 │
│ - 查找 /latest → 不存在                                       │
│ - 触发 404 处理逻辑                                           │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 3: GitHub Pages 特殊行为                                 │
│ 返回根目录的 404.html                                         │
│ HTTP 200 OK（不是 404 状态码）                                 │
│ URL 栏仍然显示：/latest                                       │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 4: 404.html 执行                                        │
│ var path = window.location.pathname; // "/latest"           │
│ sessionStorage.setItem('redirectPath', '/latest');          │
│ window.location.replace('/');                               │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 5: 重定向到根路径                                        │
│ GET https://acking-you.github.io/                           │
│ → GitHub Pages 返回 index.html                              │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 6: index.html 恢复 URL                                  │
│ var redirectPath = sessionStorage.getItem('redirectPath');  │
│ history.replaceState(null, '', '/latest');                  │
│ → URL 栏变回：/latest                                        │
└─────────────────────────────────────────────────────────────┘
         ↓
┌─────────────────────────────────────────────────────────────┐
│ Step 7: Yew 应用启动                                          │
│ - Yew Router 读取 URL：/latest                               │
│ - 匹配路由规则                                                │
│ - 渲染 LatestArticlesPage 组件                               │
│ ✅ 用户看到正确的页面                                         │
└─────────────────────────────────────────────────────────────┘
```

## 🧪 验证方法

### 本地测试

1. 构建并启动本地服务器
```bash
cd frontend
trunk build
python3 -m http.server 8000 --directory dist
```

2. 访问 `http://localhost:8000/latest`（直接访问，不从主页导航）
   - ❌ 期望：显示 404.html 的 "Redirecting to app..." 然后跳转
   - ✅ 修复后：直接显示正确页面

### 浏览器控制台验证

```javascript
// 在 /latest 页面打开控制台

// 检查当前路径
console.log(window.location.pathname);  // "/latest"

// 检查 sessionStorage（正常情况应该为空）
console.log(sessionStorage.getItem('redirectPath'));  // null

// 手动触发 404 重定向流程（模拟刷新）
sessionStorage.setItem('redirectPath', '/latest');
window.location.replace('/');
// → 页面会重新加载并恢复到 /latest
```

### 生产环境验证

1. 推送代码到 master 分支
2. 等待 GitHub Actions 部署完成
3. 测试场景：
   - ✅ 直接访问 `https://acking-you.github.io/latest`
   - ✅ 在 `/latest` 页面刷新浏览器（Ctrl+R / Cmd+R）
   - ✅ 在 `/latest` 页面强制刷新（Ctrl+Shift+R）
   - ✅ 分享 `/latest` 链接给他人

## 📂 最终目录结构

**源码：**
```
frontend/
├── index.html          ← Trunk 入口（包含 URL 恢复脚本）
├── 404.html           ← GitHub Pages SPA 路由处理
├── Trunk.toml
├── src/
└── static/
    ├── avatar.jpg
    ├── styles.css
    └── ...
```

**构建产物（部署到 GitHub Pages）：**
```
dist/
├── index.html                    ← 主应用
├── 404.html                     ← ✅ 在根目录！
├── static-flow-*.js              ← WASM glue code
├── static-flow-*_bg.wasm         ← WASM 二进制
└── static/
    ├── avatar.jpg
    ├── styles.css
    └── ...
```

## ⚠️ 注意事项

### 1. 不要禁用 JavaScript

这个方案依赖 JavaScript：
- 404.html 需要执行 JS 保存路径
- index.html 需要执行 JS 恢复 URL

如果用户禁用 JavaScript，会卡在 404.html 页面。

### 2. 首次加载会有短暂闪烁

```
用户视角：
/latest → "Redirecting to app..." (闪现) → 正确页面

实际流程：
/latest → 404.html → / → index.html → 恢复到 /latest
```

闪烁时间通常 < 100ms，用户可能感觉不到。

### 3. SEO 影响

- **不影响**：Google Bot 会执行 JavaScript
- **不影响**：最终 URL 是正确的（/latest）
- **不影响**：HTTP 状态码是 200（不是 404）

### 4. sessionStorage 的作用域

- ✅ 同一标签页有效
- ❌ 不跨标签页
- ❌ 关闭标签页后失效

这正是我们想要的行为：避免污染其他标签页。

## 🆚 其他方案对比

| 方案 | 优点 | 缺点 | 适用场景 |
|------|------|------|----------|
| **404.html hack** | 简单，无需服务器配置 | 首次加载闪烁，需要 JS | GitHub Pages（推荐） |
| **HashRouter (#/latest)** | 完全客户端，无服务器请求 | URL 带 `#` 不美观，SEO 不友好 | 快速原型 |
| **服务器配置** | 完美体验，无闪烁 | GitHub Pages 不支持 | 自托管服务器 |
| **Netlify _redirects** | 原生支持，零配置 | 不适用于 GitHub Pages | Netlify 部署 |
| **Vercel 配置** | 原生支持，零配置 | 不适用于 GitHub Pages | Vercel 部署 |

### GitHub Pages 不支持的方案

**为什么不能用服务器配置？**

理想的服务器配置（Nginx/Apache）：
```nginx
# Nginx
location / {
    try_files $uri $uri/ /index.html;
}

# Apache (.htaccess)
<IfModule mod_rewrite.c>
  RewriteEngine On
  RewriteBase /
  RewriteRule ^index\.html$ - [L]
  RewriteCond %{REQUEST_FILENAME} !-f
  RewriteCond %{REQUEST_FILENAME} !-d
  RewriteRule . /index.html [L]
</IfModule>
```

**但是：**
- ❌ GitHub Pages 不允许自定义服务器配置
- ❌ 无法上传 `.htaccess` 或 `nginx.conf`
- ❌ 无法修改服务器行为

## 🔧 故障排查

### 问题 1：404.html 不生效

**检查清单：**
```bash
# 1. 确认 404.html 在 dist 根目录
ls -la crates/frontend/dist/ | grep 404
# 应该看到：-rw-r--r-- ... 404.html

# 2. 确认 GitHub Actions 复制步骤存在
cat .github/workflows/deploy.yml | grep "404"
# 应该看到：cp 404.html dist/404.html

# 3. 检查部署后的文件
# 访问 https://acking-you.github.io/404.html
# 应该看到 "Redirecting to app..."
```

### 问题 2：无限重定向循环

**原因：** 404.html 或 index.html 的脚本有问题

**检查：**
```javascript
// 404.html 应该有：
sessionStorage.setItem('redirectPath', path + search + hash);
window.location.replace('/');  // ← 确保重定向到 /

// index.html 应该有：
if (redirectPath) {
  sessionStorage.removeItem('redirectPath');  // ← 确保清理
  history.replaceState(null, '', redirectPath);
}
```

### 问题 3：刷新后显示主页而不是目标页面

**原因：** Yew Router 配置问题或 URL 恢复脚本位置错误

**检查：**
1. URL 恢复脚本必须在 `<head>` 最前面（在所有其他 script 之前）
2. Yew Router 配置正确：
```rust
#[derive(Debug, Clone, Copy, PartialEq, Routable)]
pub enum Route {
    #[at("/")]
    Home,
    #[at("/latest")]
    LatestArticles,
    // ...
}
```

## 📚 参考资料

- [GitHub Pages 官方文档](https://docs.github.com/en/pages)
- [Single Page Apps for GitHub Pages](https://github.com/rafgraph/spa-github-pages)
- [Yew Router 文档](https://yew.rs/docs/concepts/router)
- [MDN: History API](https://developer.mozilla.org/en-US/docs/Web/API/History_API)
- [MDN: Window.sessionStorage](https://developer.mozilla.org/en-US/docs/Web/API/Window/sessionStorage)

## 🎯 总结

通过 **404.html 重定向 hack**，我们成功解决了 GitHub Pages 上的 SPA 路由刷新问题：

1. ✅ 用户可以直接访问任意路由
2. ✅ 刷新页面不会 404
3. ✅ 分享链接正常工作
4. ✅ 浏览器历史记录正常
5. ✅ SEO 友好（URL 正确，状态码 200）
6. ✅ 简单实现（2 个脚本 + 1 行 GitHub Actions）

虽然有短暂闪烁，但对于免费的 GitHub Pages 托管来说，这是最佳解决方案！
