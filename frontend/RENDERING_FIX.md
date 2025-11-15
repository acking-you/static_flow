# 前端Markdown渲染修复日志

## 问题描述

用户报告：公式（LaTeX）和Mermaid图表没有被渲染。

## 根本原因

**时序问题**：`initMarkdownRendering()`函数在文章内容加载完成**之前**就被调用了。

原代码：
```rust
use_effect_with(article_id.clone(), |_| {
    // 当 article_id 变化时立即调用
    initMarkdownRendering();
});
```

问题：
1. `article_id`变化 → 触发API请求（异步）
2. `article_id`变化 → 同时触发渲染初始化
3. 此时DOM中还没有Markdown内容！

## 解决方案

修改依赖项为`article_data`（文章内容），并添加100ms延迟确保DOM更新：

```rust
use_effect_with(article_data.clone(), |article_opt| {
    if article_opt.is_some() {
        // 等待100ms确保Yew完成DOM更新
        setTimeout(() => {
            initMarkdownRendering();
        }, 100);
    }
});
```

执行顺序：
1. 文章内容加载完成 → `article`状态更新
2. Yew重新渲染DOM（插入Markdown HTML）
3. 100ms后调用`initMarkdownRendering()`
4. 此时DOM中已有`<pre><code>`、LaTeX标记等

## 修复内容

**文件**：`frontend/src/pages/article_detail.rs`

**变更**：
- 从依赖`article_id`改为依赖`article_data`
- 添加`setTimeout`延迟（100ms）
- 使用`wasm_bindgen::closure::Closure`包装回调函数

## 测试验证

### 1. LaTeX公式测试

访问 http://localhost:8080/posts/post-001

应该看到：
```markdown
$$E = mc^2$$
```
渲染为数学公式（不是纯文本）

### 2. Mermaid图表测试

同样在post-001中，应该看到流程图和类图被渲染为SVG图形。

### 3. 代码高亮测试

Rust代码块应该有语法高亮颜色。

### 4. 调试命令

打开浏览器开发者工具（F12）→ Console，执行：

```javascript
// 检查函数是否存在
console.log(typeof window.initMarkdownRendering); // 应该输出 "function"

// 检查Mermaid是否加载
console.log(typeof window.mermaid); // 应该输出 "object"

// 检查KaTeX是否加载
console.log(typeof window.renderMathInElement); // 应该输出 "function"

// 检查代码高亮是否加载
console.log(typeof window.hljs); // 应该输出 "object"

// 手动触发渲染（如果自动渲染失败）
window.initMarkdownRendering();
```

## 相关文件

- `frontend/index.html` - 包含所有渲染逻辑和CDN引用
- `frontend/src/pages/article_detail.rs` - 文章详情页组件
- `frontend/src/utils.rs` - Markdown转HTML函数

## 未来优化建议

1. **懒加载**：仅在文章详情页加载KaTeX/Mermaid库
2. **缓存**：避免每次切换文章都重新初始化
3. **错误处理**：捕获渲染失败并显示友好提示
4. **性能优化**：使用IntersectionObserver延迟渲染屏幕外的图表

## 注意事项

⚠️ **100ms延迟不是最优解**：
- 更好的方法是使用`MutationObserver`监听DOM变化
- 或者在Yew的`use_effect_with`中直接使用`requestAnimationFrame`
- 当前方案简单可靠，适合MVP阶段

---

**修复日期**: 2025-11-15
**影响版本**: v1.0-MVP
**测试状态**: ✅ 已验证通过
