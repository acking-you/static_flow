# 文章目录（TOC）和回到顶部功能

## 功能概述

为文章详情页添加了两个重要的导航辅助功能：

1. **右侧目录（Table of Contents）**：自动提取文章标题，生成可跳转的层级目录
2. **回到顶部按钮**：快速滚动回页面顶部

---

## 功能 1：文章目录（TOC）

### 使用方法

1. **自动生成**：打开任何包含标题的文章，目录自动在右侧显示
2. **点击跳转**：点击目录中的任一标题，页面平滑滚动到对应位置
3. **当前位置高亮**：滚动页面时，目录自动高亮当前阅读的章节
4. **层级缩进**：根据标题级别（h1-h6）自动缩进

### 特性说明

#### 1. 自动提取标题

- 扫描 `.article-content` 中的所有 `h1-h6` 标签
- 自动为没有 ID 的标题添加唯一 ID（`heading-0`, `heading-1`, ...）
- 如果文章没有标题，目录不显示

#### 2. 层级结构

目录根据标题级别显示缩进：

```
📄 目录
  H1 标题（不缩进）
    H2 标题（0.75rem 缩进）
      H3 标题（1.5rem 缩进）
        H4 标题（2.25rem 缩进）
          H5 标题（3rem 缩进）
            H6 标题（3.75rem 缩进）
```

#### 3. 当前位置高亮

- **实时跟踪**：滚动页面时，目录自动高亮当前阅读的章节
- **性能优化**：使用 `requestAnimationFrame` 优化滚动性能
- **高亮样式**：
  - 激活项：蓝色文字 + 左侧蓝色边框 + 浅蓝背景
  - 悬停效果：浅灰背景 + 向右移动 2px

#### 4. 平滑滚动

点击目录项时：
```javascript
target.scrollIntoView({ behavior: 'smooth', block: 'start' });
```

- `behavior: 'smooth'` - 平滑滚动
- `block: 'start'` - 目标元素对齐到视口顶部

### 视觉设计

**位置**：
- 右侧固定：`position: fixed`
- 距顶部：`header 高度 + 2rem`
- 距右侧：`2rem`
- 宽度：`260px`

**样式**：
- 白色背景卡片
- 圆角边框 + 阴影
- 最大高度：`视口高度 - header - 4rem`
- 超出部分滚动

**响应式**：
- `> 1280px`：显示目录
- `≤ 1280px`：隐藏目录（屏幕太窄）

### 代码位置

#### JavaScript（crates/frontend/index.html:414-513）

**生成目录**：
```javascript
function generateTOC() {
  const articleContent = document.querySelector('.article-content');
  if (!articleContent) return;

  const headings = articleContent.querySelectorAll('h1, h2, h3, h4, h5, h6');
  if (headings.length === 0) return;

  // 为标题添加 ID
  headings.forEach((heading, index) => {
    if (!heading.id) {
      heading.id = `heading-${index}`;
    }
  });

  // 创建目录 DOM
  const tocContainer = document.createElement('aside');
  tocContainer.className = 'article-toc';
  // ...
}
```

**更新高亮**：
```javascript
function updateActiveTOC(headings) {
  const scrollY = window.scrollY + 100; // 偏移量

  let currentHeading = null;
  headings.forEach((heading) => {
    if (scrollY >= heading.offsetTop) {
      currentHeading = heading;
    }
  });

  // 移除所有激活状态
  document.querySelectorAll('.toc-link').forEach(link =>
    link.classList.remove('active')
  );

  // 高亮当前标题
  if (currentHeading) {
    const activeLink = document.querySelector(
      `.toc-link[href="#${currentHeading.id}"]`
    );
    if (activeLink) activeLink.classList.add('active');
  }
}
```

#### CSS（crates/frontend/static/styles.css:2080-2165）

**目录容器**：
```css
.article-toc {
  position: fixed;
  top: calc(var(--header-height-desktop) + 2rem);
  right: 2rem;
  width: 260px;
  max-height: calc(100vh - var(--header-height-desktop) - 4rem);
  overflow-y: auto;
  background: var(--surface);
  border-radius: var(--radius);
  box-shadow: var(--shadow);
}
```

**链接样式**：
```css
.toc-link {
  display: block;
  padding: 0.5rem 0.75rem;
  color: var(--muted);
  border-left: 2px solid transparent;
  transition: all 0.2s ease;
}

.toc-link:hover {
  color: var(--primary);
  background: rgba(0, 0, 0, 0.03);
  border-left-color: var(--primary);
  transform: translateX(2px);
}

.toc-link.active {
  color: var(--primary);
  font-weight: 600;
  background: rgba(29, 158, 216, 0.08);
  border-left-color: var(--primary);
}
```

---

## 功能 2：回到顶部按钮

### 使用方法

1. **显示条件**：向下滚动超过 300px 后，按钮从右下角淡入
2. **点击回顶**：点击按钮，页面平滑滚动回顶部
3. **自动隐藏**：回到顶部后，按钮自动淡出

### 特性说明

#### 1. 智能显示

```javascript
if (window.scrollY > 300) {
  btn.classList.add('visible');
} else {
  btn.classList.remove('visible');
}
```

- `scrollY > 300px`：显示按钮
- `scrollY ≤ 300px`：隐藏按钮

#### 2. 平滑滚动

```javascript
window.scrollTo({ top: 0, behavior: 'smooth' });
```

点击后平滑滚动到页面顶部（`top: 0`）。

#### 3. 性能优化

使用 `requestAnimationFrame` 节流滚动事件：
```javascript
let ticking = false;
window.addEventListener('scroll', () => {
  if (!ticking) {
    window.requestAnimationFrame(() => {
      // 更新按钮显示状态
      ticking = false;
    });
    ticking = true;
  }
});
```

避免频繁触发，提升性能。

### 视觉设计

**位置**：
- 右下角固定：`position: fixed`
- 距底部：`2rem`（移动端 `1.5rem`）
- 距右侧：`2rem`（移动端 `1.5rem`）

**样式**：
- 圆形按钮：`3rem × 3rem`（移动端 `2.75rem × 2.75rem`）
- 背景色：主题色（`var(--primary)`）
- 图标：向上箭头 `fa-arrow-up`
- 阴影：`0 4px 12px rgba(29, 158, 216, 0.4)`

**动画效果**：

1. **淡入/淡出**：
```css
.back-to-top {
  opacity: 0;
  visibility: hidden;
  transform: translateY(10px);
  transition: all 0.3s ease;
}

.back-to-top.visible {
  opacity: 1;
  visibility: visible;
  transform: translateY(0);
}
```

2. **悬停效果**：
```css
.back-to-top:hover {
  transform: translateY(-4px);
  box-shadow: 0 6px 20px rgba(29, 158, 216, 0.5);
}
```

- 向上浮动 4px
- 阴影增强

3. **点击效果**：
```css
.back-to-top:active {
  transform: translateY(-2px);
}
```

### 代码位置

#### JavaScript（crates/frontend/index.html:515-548）

```javascript
function initBackToTop() {
  // 创建按钮
  const btn = document.createElement('button');
  btn.className = 'back-to-top';
  btn.innerHTML = '<i class="fas fa-arrow-up"></i>';
  btn.title = '回到顶部';

  // 点击回顶
  btn.addEventListener('click', () => {
    window.scrollTo({ top: 0, behavior: 'smooth' });
  });

  // 滚动时显示/隐藏
  window.addEventListener('scroll', () => {
    if (window.scrollY > 300) {
      btn.classList.add('visible');
    } else {
      btn.classList.remove('visible');
    }
  });

  document.body.appendChild(btn);
}
```

#### CSS（crates/frontend/static/styles.css:2169-2228）

```css
.back-to-top {
  position: fixed;
  bottom: 2rem;
  right: 2rem;
  width: 3rem;
  height: 3rem;
  background: var(--primary);
  color: #fff;
  border-radius: 50%;
  box-shadow: 0 4px 12px rgba(29, 158, 216, 0.4);
  z-index: 100;
  opacity: 0;
  visibility: hidden;
  transform: translateY(10px);
  transition: all 0.3s ease;
}

.back-to-top.visible {
  opacity: 1;
  visibility: visible;
  transform: translateY(0);
}
```

---

## 浏览器兼容性

| 功能 | Chrome | Firefox | Safari | Edge |
|------|--------|---------|--------|------|
| position: fixed | ✅ | ✅ | ✅ | ✅ |
| scrollIntoView | 61+ | 36+ | 14+ | 79+ |
| requestAnimationFrame | ✅ | ✅ | ✅ | ✅ |
| scrollTo behavior | 61+ | 36+ | 14+ | 79+ |
| CSS transitions | ✅ | ✅ | ✅ | ✅ |

**降级方案**：
- 不支持 `behavior: 'smooth'` 的浏览器会瞬间跳转（无平滑效果）
- 核心功能在所有现代浏览器都正常工作

---

## 响应式设计

### 桌面端（> 1280px）

- ✅ 显示右侧目录
- ✅ 显示回到顶部按钮
- 按钮尺寸：`3rem × 3rem`
- 目录宽度：`260px`

### 中等屏幕（768px - 1280px）

- ❌ 隐藏目录（屏幕太窄）
- ✅ 显示回到顶部按钮
- 按钮尺寸：`3rem × 3rem`

### 移动端（< 768px）

- ❌ 隐藏目录
- ✅ 显示回到顶部按钮
- 按钮尺寸：`2.75rem × 2.75rem`
- 按钮位置：`bottom: 1.5rem, right: 1.5rem`

---

## 性能优化

### 1. 滚动节流

使用 `requestAnimationFrame` 限制滚动事件触发频率：
```javascript
let ticking = false;
window.addEventListener('scroll', () => {
  if (!ticking) {
    window.requestAnimationFrame(() => {
      updateActiveTOC(headings);
      ticking = false;
    });
    ticking = true;
  }
});
```

**优势**：
- 保证最多每帧更新一次（60fps = 16.6ms/次）
- 避免频繁 DOM 查询和样式更新
- CPU 占用极低

### 2. CSS 硬件加速

使用 `transform` 而非 `top`/`left` 实现动画：
```css
transform: translateY(-4px);  /* ✅ GPU 加速 */
```

### 3. 延迟初始化

目录和按钮仅在文章详情页初始化，不影响其他页面。

---

## 常见问题

### Q1: 目录不显示？

**可能原因**：
- 不在文章详情页（没有 `.article-content`）
- 文章中没有标题（`h1-h6`）
- 屏幕宽度小于 1280px

**解决方案**：
- 确认在文章详情页
- 确认文章有标题
- 扩大浏览器窗口到 > 1280px

### Q2: 点击目录不跳转？

**检查清单**：
- [ ] 浏览器控制台是否有错误
- [ ] 标题是否有 ID 属性
- [ ] 是否禁用了 JavaScript

### Q3: 回到顶部按钮不显示？

**可能原因**：
- 页面未滚动超过 300px
- 按钮被其他元素遮挡（`z-index` 问题）

**解决方案**：
- 向下滚动页面超过 300px
- 检查是否有其他元素 `z-index > 100`

### Q4: 目录遮挡文章内容？

**不会发生**：
- 目录使用 `position: fixed`，不占用文档流空间
- 文章内容区域有足够边距
- 小屏幕自动隐藏目录

### Q5: 移动端能否显示目录？

**当前设计**：移动端隐藏目录，因为屏幕太窄

**未来扩展**：可以添加：
- 底部抽屉式目录
- 点击按钮弹出目录
- 横向滚动目录

---

## 调试工具

在浏览器控制台运行：

```javascript
// 检查目录是否生成
document.querySelector('.article-toc')

// 检查标题数量
document.querySelectorAll('.article-content h1, .article-content h2, .article-content h3, .article-content h4, .article-content h5, .article-content h6').length

// 检查回到顶部按钮
document.querySelector('.back-to-top')

// 手动触发回到顶部
window.scrollTo({ top: 0, behavior: 'smooth' })

// 检查当前滚动位置
window.scrollY
```

---

## 未来扩展

可能的改进方向：

### 目录功能

1. **折叠/展开**：点击父级标题折叠子级
2. **搜索功能**：在目录中搜索关键词
3. **进度指示**：显示阅读进度百分比
4. **移动端抽屉**：底部滑出式目录

### 回到顶部按钮

1. **显示进度**：圆形进度条显示阅读百分比
2. **双向按钮**：添加"下一章节"按钮
3. **快捷键**：支持键盘快捷键（如 `Home` 键）
4. **记忆位置**：记住上次阅读位置

---

**实现时间**: 2025-11-15
**功能**: 文章目录 + 回到顶部
**文件修改**:
- `crates/frontend/index.html` - JavaScript 逻辑
- `crates/frontend/static/styles.css` - 样式定义
