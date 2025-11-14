# Markdown 渲染功能说明

## 概述

StaticFlow 前端现已支持完整的 Markdown 渲染功能，包括：
1. ✅ **代码语法高亮** - Highlight.js
2. ✅ **数学公式渲染** - KaTeX
3. ✅ **Mermaid 图表** - Mermaid.js

---

## 功能特性

### 1. 代码语法高亮

**引擎**: [Highlight.js 11.9.0](https://highlightjs.org/)

**支持的语言**:
- Rust, JavaScript, TypeScript
- Python, Bash, SQL
- JSON, YAML, TOML, Markdown

**主题**:
- 深色模式: Atom One Dark
- 浅色模式: Atom One Light
- 自动跟随系统主题切换

**使用方法**:
```markdown
​```rust
fn main() {
    println!("Hello, world!");
}
​```
```

### 2. 数学公式渲染

**引擎**: [KaTeX 0.16.9](https://katex.org/)

**支持的语法**:
- 行内公式: `$E = mc^2$` 或 `\(E = mc^2\)`
- 块级公式: `$$...$$` 或 `\[...\]`

**示例**:
```markdown
行内：质能方程 $E = mc^2$ 描述了能量和质量的关系。

块级：
$$
\int_0^\infty e^{-x^2} dx = \frac{\sqrt{\pi}}{2}
$$
```

**特性**:
- 支持完整的 LaTeX 数学符号
- 支持希腊字母、矩阵、积分、求和等
- 自动识别多种定界符

### 3. Mermaid 图表

**引擎**: [Mermaid 10.x](https://mermaid.js.org/)

**支持的图表类型**:
1. **Flowchart** - 流程图
2. **Sequence Diagram** - 时序图
3. **Class Diagram** - 类图
4. **State Diagram** - 状态图
5. **Gantt Chart** - 甘特图
6. **Pie Chart** - 饼图
7. **ER Diagram** - 实体关系图
8. **Git Graph** - Git 分支图
9. **Mindmap** - 思维导图
10. **User Journey** - 用户旅程图

**使用方法**:
```markdown
​```mermaid
graph TD
    A[开始] --> B{条件判断}
    B -->|是| C[执行操作]
    B -->|否| D[结束]
​```
```

**主题**:
- 深色模式: 自动使用 Mermaid dark 主题
- 浅色模式: 使用 Mermaid default 主题
- 与页面主题自动同步

**横向滚动支持**:
- 横向超长的图表（如复杂流程图）会保持原始尺寸
- 容器内可以横向滚动查看完整内容
- 避免图表被强制缩小导致文字难以阅读
- 较小的图表仍然自动居中显示
- 最大高度限制为 80vh，超出部分可垂直滚动

---

## 实现原理

### 渲染流程

```
1. 用户编写 Markdown
   ↓
2. Rust (pulldown-cmark) 解析 → HTML
   - 代码块: <pre><code class="language-rust">
   - 公式: 保留原始 $ 定界符
   - Mermaid: <pre><code class="language-mermaid">
   ↓
3. Yew 组件渲染到 DOM
   ↓
4. use_effect_with 触发（组件挂载后）
   ↓
5. window.initMarkdownRendering() 执行
   ├─ 查找 <code class="language-mermaid">
   │  └─ 转换为 <div class="mermaid">
   ├─ hljs.highlightElement() → 语法高亮
   ├─ mermaid.run() → 渲染图表
   └─ renderMathInElement() → 渲染公式
```

### 关键代码

**index.html** - 引入外部库
```html
<!-- KaTeX -->
<link rel="stylesheet" href="...katex.min.css" />
<script defer src="...katex.min.js"></script>

<!-- Highlight.js -->
<link rel="stylesheet" href="...atom-one-dark.min.css" />
<script src="...highlight.min.js"></script>

<!-- Mermaid -->
<script type="module">
  import mermaid from '...mermaid.esm.min.mjs';
  window.mermaid = mermaid;
  mermaid.initialize({ startOnLoad: false });
</script>
```

**article_detail.rs** - 触发渲染
```rust
use_effect_with(article_id.clone(), |_| {
    if let Some(win) = window() {
        if let Ok(init_fn) = js_sys::Reflect::get(&win, &JsValue::from_str("initMarkdownRendering")) {
            if let Ok(func) = init_fn.dyn_into::<js_sys::Function>() {
                let _ = func.call0(&win);
            }
        }
    }
    || ()
});
```

---

## 测试方法

### 方法 1: 查看 Mock 数据

1. 启动开发服务器:
   ```bash
   cd frontend
   trunk serve
   ```

2. 访问 http://127.0.0.1:8080

3. 点击文章:
   - **第 1 篇**: 包含 Rust 代码和数学公式 ($E=mc^2$)
   - **第 2 篇**: 包含 Mermaid 流程图和表格

### 方法 2: 使用完整测试文档

测试文档位置: `docs/markdown-rendering-test.md`

包含内容:
- 6 种语言的代码高亮示例
- 行内/块级公式示例
- 10 种 Mermaid 图表类型
- 混合内容测试
- 完整的验证清单

### 方法 3: 浏览器调试

打开浏览器 DevTools Console，检查:
```javascript
// 确认库已加载
window.hljs         // Highlight.js
window.katex        // KaTeX
window.mermaid      // Mermaid
window.initMarkdownRendering  // 初始化函数

// 手动触发渲染
window.initMarkdownRendering();
```

---

## 配置选项

### 添加更多语言支持

编辑 `frontend/index.html`，添加语言包:
```html
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/go.min.js"></script>
<script src="https://cdnjs.cloudflare.com/ajax/libs/highlight.js/11.9.0/languages/cpp.min.js"></script>
```

### 更改代码高亮主题

替换 CSS 链接:
```html
<!-- 可选主题 -->
<link rel="stylesheet" href=".../styles/github-dark.min.css" />
<link rel="stylesheet" href=".../styles/monokai.min.css" />
<link rel="stylesheet" href=".../styles/dracula.min.css" />
```

查看所有主题: https://highlightjs.org/examples

### 自定义 Mermaid 主题

编辑 `index.html` 中的 Mermaid 初始化:
```javascript
mermaid.initialize({
  startOnLoad: false,
  theme: 'dark',  // 'default', 'dark', 'forest', 'neutral'
  themeVariables: {
    primaryColor: '#ff6b6b',
    primaryTextColor: '#fff',
    lineColor: '#F8B229'
  }
});
```

### 修改 KaTeX 定界符

编辑 `index.html` 中的 `renderMathInElement` 配置:
```javascript
renderMathInElement(document.body, {
  delimiters: [
    {left: '$$', right: '$$', display: true},
    {left: '$', right: '$', display: false},
    // 添加自定义定界符
    {left: '\\begin{equation}', right: '\\end{equation}', display: true}
  ]
});
```

---

## 常见问题

### Q1: Mermaid 图表不显示？

**检查清单**:
- [ ] 浏览器控制台是否有错误
- [ ] `window.mermaid` 是否已定义
- [ ] 代码块语言标识是否为 `mermaid`
- [ ] 图表语法是否正确（参考 [Mermaid 文档](https://mermaid.js.org/)）

**常见错误**:
```markdown
# ❌ 错误 - 语言标识拼写错误
​```mermid
graph TD
​```

# ✅ 正确
​```mermaid
graph TD
​```
```

### Q2: 公式显示为原始文本？

**可能原因**:
1. KaTeX 库未加载完成 - 刷新页面
2. 定界符不匹配 - 确保使用 `$...$` 或 `$$...$$`
3. LaTeX 语法错误 - 检查公式语法

**调试方法**:
```javascript
// 浏览器控制台
window.katex  // 应返回对象
window.renderMathInElement  // 应返回函数
```

### Q3: 代码高亮不生效？

**检查**:
- 代码块语言标识是否正确
- Highlight.js 是否支持该语言
- 是否有 CSS 样式冲突

**手动测试**:
```javascript
// 浏览器控制台
document.querySelectorAll('pre code').forEach(hljs.highlightElement);
```

### Q4: 主题切换后图表颜色不变？

**原因**: Mermaid 主题在初始化时确定

**解决方案**: 需要重新初始化 Mermaid（未来可改进）
```javascript
// 临时方案：切换主题后刷新页面
window.location.reload();
```

---

## 性能优化

### CDN 加载优化

当前使用的 CDN:
- KaTeX: cdn.jsdelivr.net
- Highlight.js: cdnjs.cloudflare.com
- Mermaid: cdn.jsdelivr.net (ESM)

**优化建议**:
1. 生产环境考虑自托管（避免 CDN 不可用）
2. 使用 `defer` 或 `async` 异步加载
3. 按需加载语言包（减少初始加载大小）

### 按需渲染

当前实现在每次文章加载后重新渲染所有元素。

**优化方向**:
- 仅渲染新增的代码块/公式/图表
- 使用 IntersectionObserver 懒加载屏幕外的图表

---

## 技术栈

| 库 | 版本 | 用途 | License |
|---|------|------|---------|
| KaTeX | 0.16.9 | 数学公式渲染 | MIT |
| Highlight.js | 11.9.0 | 代码语法高亮 | BSD-3 |
| Mermaid | 10.x | 图表绘制 | MIT |

---

## 参考资源

- **KaTeX**: https://katex.org/docs/supported.html (支持的符号)
- **Highlight.js**: https://highlightjs.org/examples (主题预览)
- **Mermaid**: https://mermaid.js.org/intro/ (官方文档)
- **pulldown-cmark**: https://github.com/raphlinus/pulldown-cmark (Markdown 解析器)

---

**最后更新**: 2025-11-15
**维护者**: Claude Code
