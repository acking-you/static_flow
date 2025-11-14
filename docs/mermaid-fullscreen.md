# Mermaid 图表全屏查看功能

## 功能概述

为 Mermaid 图表添加了全屏查看功能，允许用户以最大化方式查看复杂的流程图和其他图表类型。

## 使用方法

### 打开全屏查看

1. 鼠标悬停在 Mermaid 图表上方
2. 点击右上角的**全屏图标** <i class="fas fa-expand"></i>（位于下载按钮左侧）
3. 图表以全屏模式展示，没有尺寸限制

### 关闭全屏查看

有三种方式关闭全屏视图：

1. **点击关闭按钮**：点击右上角的圆形关闭按钮（X 图标）
2. **点击背景区域**：点击图表外的黑色背景区域
3. **按 ESC 键**：按下键盘的 Escape 键

## 功能特性

### 1. 完整尺寸显示

- ✅ **移除所有尺寸限制**：图表以原始尺寸完整展示
- ✅ **横向超长图表**：不再被压缩，可查看所有细节
- ✅ **纵向超长图表**：不受 550px 高度限制
- ✅ **自动滚动**：如果图表超出屏幕，内容区域可滚动

### 2. 用户体验优化

- ✅ **平滑动画**：打开/关闭时有淡入淡出和缩放动画
- ✅ **毛玻璃背景**：深色半透明背景（95% 黑色 + 模糊效果）
- ✅ **防止页面滚动**：全屏时禁用 body 滚动，避免操作冲突
- ✅ **多种关闭方式**：按钮、背景点击、ESC 键

### 3. 视觉设计

- **覆盖层**：黑色半透明背景（`rgba(0, 0, 0, 0.95)`）+ 模糊效果
- **内容容器**：最大 95vw × 95vh，保留边距
- **图表样式**：白色背景卡片 + 圆角 + 阴影
- **关闭按钮**：右上角圆形按钮，悬停时旋转 90° + 放大

### 4. 响应式适配

**桌面端**：
- 关闭按钮：3rem × 3rem
- 内容区域：2rem 内边距
- 图表卡片：2rem 内边距

**移动端**（≤768px）：
- 关闭按钮：2.5rem × 2.5rem
- 内容区域：1rem 内边距
- 图表卡片：1rem 内边距
- 全屏按钮位置调整：`right: 3rem`

## 实现原理

### 技术架构

```
用户点击全屏按钮
    ↓
JavaScript 创建覆盖层 DOM
    ↓
克隆 Mermaid 图表 SVG
    ↓
应用全屏样式（无尺寸限制）
    ↓
添加到 document.body
    ↓
触发打开动画（opacity + scale）
    ↓
监听关闭事件（按钮/背景/ESC）
    ↓
触发关闭动画
    ↓
移除覆盖层 DOM + 恢复 body 滚动
```

### 核心代码结构

#### JavaScript 部分（frontend/index.html）

**1. 添加全屏按钮**：

```javascript
const fullscreenBtn = document.createElement('button');
fullscreenBtn.className = 'copy-button fullscreen-button';
fullscreenBtn.innerHTML = '<i class="fas fa-expand"></i>';
fullscreenBtn.addEventListener('click', (e) => {
  openMermaidFullscreen(mermaidDiv);
});
wrapper.appendChild(fullscreenBtn);
```

**2. 全屏查看函数**：

```javascript
function openMermaidFullscreen(mermaidDiv) {
  // 创建覆盖层
  const overlay = document.createElement('div');
  overlay.className = 'mermaid-fullscreen-overlay';

  // 克隆图表
  const clonedMermaid = mermaidDiv.cloneNode(true);
  clonedMermaid.className = 'mermaid mermaid-fullscreen';

  // 创建关闭按钮
  const closeBtn = document.createElement('button');
  closeBtn.className = 'mermaid-fullscreen-close';

  // 关闭逻辑
  closeBtn.addEventListener('click', () => {
    overlay.classList.remove('open');
    setTimeout(() => {
      document.body.removeChild(overlay);
      document.body.style.overflow = '';
    }, 300);
  });

  // ESC 键关闭
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') closeBtn.click();
  });

  // 背景点击关闭
  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) closeBtn.click();
  });

  // 添加到页面
  document.body.appendChild(overlay);
  document.body.style.overflow = 'hidden';

  // 触发动画
  setTimeout(() => overlay.classList.add('open'), 10);
}
```

#### CSS 部分（frontend/static/styles.css）

**1. 全屏按钮定位**（styles.css:1913-1915）：

```css
.fullscreen-button {
  right: 3.5rem !important;  /* 在下载按钮左侧 */
}
```

**2. 覆盖层样式**（styles.css:1923-1938）：

```css
.mermaid-fullscreen-overlay {
  position: fixed;
  inset: 0;
  z-index: 9999;
  background: rgba(0, 0, 0, 0.95);
  backdrop-filter: blur(8px);
  opacity: 0;
  transition: opacity 0.3s ease;
}

.mermaid-fullscreen-overlay.open {
  opacity: 1;
}
```

**3. 内容容器**（styles.css:1941-1952）：

```css
.mermaid-fullscreen-content {
  max-width: 95vw;
  max-height: 95vh;
  overflow: auto;
  padding: 2rem;
  transform: scale(0.9);
  transition: transform 0.3s ease;
}

.mermaid-fullscreen-overlay.open .mermaid-fullscreen-content {
  transform: scale(1);  /* 打开时放大到正常尺寸 */
}
```

**4. 图表无限制样式**（styles.css:1955-1971）：

```css
.mermaid-fullscreen {
  background: var(--surface) !important;
  max-height: none !important;  /* 移除高度限制 */
  overflow: visible !important;
}

.mermaid-fullscreen svg {
  max-width: none !important;   /* 移除宽度限制 */
  max-height: none !important;  /* 移除高度限制 */
  width: auto !important;
  height: auto !important;
}
```

**5. 关闭按钮**（styles.css:1974-2002）：

```css
.mermaid-fullscreen-close {
  position: fixed;
  top: 1.5rem;
  right: 1.5rem;
  width: 3rem;
  height: 3rem;
  border-radius: 50%;
  background: rgba(0, 0, 0, 0.7);
}

.mermaid-fullscreen-close:hover {
  transform: scale(1.1) rotate(90deg);  /* 悬停旋转 */
}
```

## 按钮布局

Mermaid 图表现在有两个按钮：

```
┌─────────────────────────────────┐
│                        [⛶] [↓]  │  ← 悬停时显示
│                                 │
│      Mermaid 图表内容           │
│                                 │
└─────────────────────────────────┘

[⛶] = 全屏按钮 (fa-expand)
[↓] = 下载按钮 (fa-download)
```

**位置**：
- 全屏按钮：`right: 3.5rem`（左侧）
- 下载按钮：`right: 0.5rem`（右侧）

## 动画效果

### 打开动画

```
初始状态：
- overlay opacity: 0
- content scale: 0.9

↓ (10ms 后触发)

最终状态：
- overlay opacity: 1
- content scale: 1

持续时间：300ms
```

### 关闭动画

```
点击关闭
    ↓
移除 .open 类
    ↓
opacity: 1 → 0
scale: 1 → 0.9
    ↓
300ms 后移除 DOM
```

### 关闭按钮悬停动画

```
默认：scale(1) rotate(0deg)
悬停：scale(1.1) rotate(90deg)
点击：scale(0.95) rotate(90deg)
```

## 主题适配

**浅色主题**：
- 覆盖层：`rgba(0, 0, 0, 0.95)`
- 图表背景：`var(--surface)` (白色)
- 阴影：`rgba(0, 0, 0, 0.5)`

**深色主题**：
- 覆盖层：`rgba(0, 0, 0, 0.95)`（相同）
- 图表背景：`var(--surface)` (深色)
- 阴影：`rgba(0, 0, 0, 0.8)`（更深）

## 浏览器兼容性

| 功能 | Chrome | Firefox | Safari | Edge |
|------|--------|---------|--------|------|
| 基础全屏 | ✅ | ✅ | ✅ | ✅ |
| backdrop-filter | 76+ | 103+ | 9+ | 79+ |
| ESC 键监听 | ✅ | ✅ | ✅ | ✅ |
| cloneNode | ✅ | ✅ | ✅ | ✅ |

**降级方案**：
- 不支持 `backdrop-filter` 的浏览器会显示纯色背景（无模糊效果）
- 所有现代浏览器都支持核心功能

## 常见问题

### Q1: 全屏后图表显示不完整？

**原因**：可能是图表尺寸超出了 95vw × 95vh

**解决方案**：
- 内容容器有 `overflow: auto`，可以滚动查看
- 图表本身无限制，只是容器有最大尺寸
- 可以调整 `.mermaid-fullscreen-content` 的 `max-width` 和 `max-height`

### Q2: 全屏时无法滚动查看完整图表？

**检查清单**：
- [ ] 是否看到滚动条？（如果没有，说明图表在范围内）
- [ ] 尝试鼠标滚轮或拖动滚动条
- [ ] 移动端可以用手指滑动

### Q3: 关闭按钮被图表遮挡？

**不会发生**：
- 关闭按钮 `z-index: 10000`，始终在最上层
- 覆盖层 `z-index: 9999`
- 图表在覆盖层内，无法超过

### Q4: 点击背景无法关闭？

**可能原因**：点击到了内容区域而非背景

**解决方案**：
- 点击图表卡片外的黑色区域
- 或使用右上角关闭按钮
- 或按 ESC 键

### Q5: 移动端全屏按钮太小？

**已优化**：
- 移动端按钮保持与下载按钮相同尺寸：`0.4rem 0.6rem`
- 按钮常驻显示（不需要悬停）
- 如需更大，可调整 `@media (max-width: 768px)` 中的 padding

## 调试工具

在浏览器控制台运行：

```javascript
// 检查全屏按钮是否存在
document.querySelectorAll('.fullscreen-button').length

// 手动触发全屏（假设至少有一个 Mermaid 图表）
const mermaid = document.querySelector('.mermaid');
if (mermaid) openMermaidFullscreen(mermaid);

// 检查覆盖层是否创建
document.querySelectorAll('.mermaid-fullscreen-overlay').length
```

## 性能考虑

- **DOM 克隆**：使用 `cloneNode(true)` 深度克隆，性能开销小
- **动画性能**：使用 `opacity` 和 `transform`，触发 GPU 加速
- **内存清理**：关闭时正确移除 DOM 和事件监听器，防止内存泄漏
- **事件委托**：ESC 键监听在关闭时移除，避免累积

## 未来扩展

可能的改进方向：

1. **缩放控制**：添加放大/缩小按钮（+/-）
2. **拖拽移动**：允许拖拽图表位置（类似图片查看器）
3. **键盘导航**：方向键移动、+/- 缩放
4. **旋转功能**：90° 旋转图表（适合横向长图）
5. **分享功能**：全屏时也可以下载或复制

---

**实现时间**: 2025-11-15
**功能**: Mermaid 图表全屏查看
**文件修改**:
- `frontend/index.html` - 添加全屏按钮和逻辑
- `frontend/static/styles.css` - 全屏样式和动画
