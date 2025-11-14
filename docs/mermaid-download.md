# Mermaid 图表下载功能

## 功能更新

由于浏览器的跨域安全限制（Tainted Canvas），已将 Mermaid 图表的**复制功能**改为**下载功能**。

## 使用方法

### 1. 下载图表

1. 鼠标悬停在 Mermaid 图表上方
2. 点击右上角的**下载图标** <i class="fas fa-download"></i>
3. 图表自动下载为 PNG 图片（文件名：`mermaid-diagram-时间戳.png`）
4. 图标变为 ✓ 确认下载成功

### 2. 降级策略

如果 PNG 转换失败（Canvas 污染），会自动降级为下载 SVG 文件：
- 文件名：`mermaid-diagram-时间戳.svg`
- SVG 是矢量格式，可以用浏览器打开或导入设计软件

## 实现原理

### 下载流程

```
1. 用户点击下载按钮
   ↓
2. 获取 Mermaid 渲染的 SVG 元素
   ↓
3. 克隆 SVG 并序列化为字符串
   ↓
4. 创建 Canvas 并设置尺寸
   ↓
5. 在 Canvas 上绘制白色背景
   ↓
6. 将 SVG 转为图片并绘制到 Canvas
   ↓
7. 将 Canvas 转为 PNG Blob
   ↓
8. 创建临时 <a> 标签触发下载
   ↓
9. 清理临时资源（URL.revokeObjectURL）
```

### 降级处理

```
PNG 转换 → 尝试
  ↓ 失败（Canvas tainted）
SVG 直接下载 → 降级
  ↓ 成功
显示成功提示 ✓
```

## 代码关键部分

**下载 PNG**：
```javascript
canvas.toBlob((blob) => {
  const downloadUrl = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = downloadUrl;
  a.download = `mermaid-diagram-${Date.now()}.png`;
  a.click();
  URL.revokeObjectURL(downloadUrl);
}, 'image/png');
```

**降级到 SVG**：
```javascript
function downloadSvgDirectly(svgData) {
  const blob = new Blob([svgData], { type: 'image/svg+xml' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `mermaid-diagram-${Date.now()}.svg`;
  a.click();
  URL.revokeObjectURL(url);
}
```

## 优势对比

| 功能 | 复制到剪贴板 | 下载文件 |
|------|-------------|---------|
| **实现难度** | 高 | 低 |
| **跨域限制** | ❌ 有限制 | ✅ 无限制 |
| **浏览器支持** | 需要 HTTPS | ✅ 全部支持 |
| **Canvas 污染** | ❌ 会报错 | ✅ 可降级 |
| **用户体验** | 需粘贴 | 直接保存 |
| **文件管理** | 无 | ✅ 可归档 |

## 测试方法

1. 启动开发服务器：
   ```bash
   cd frontend
   trunk serve
   ```

2. 访问包含 Mermaid 图表的文章（第 2 篇）

3. 悬停图表，点击下载按钮

4. 检查浏览器下载目录，确认文件已保存

5. 打开下载的图片，验证内容完整

## 调试工具

打开浏览器控制台，运行：
```javascript
window.debugMermaidCopy();
```

输出信息：
```
=== Mermaid Download Debug ===
Mermaid elements found: 1
Mermaid wrappers found: 1
Download buttons found: 1
First mermaid has SVG: true
SVG dimensions: 800 x 600
Has wrapper: true
============================
```

## 常见问题

### Q1: 下载的 PNG 图片是空白的？

**可能原因**：Canvas 污染或 SVG 尺寸获取失败

**解决方案**：
- 检查浏览器控制台错误
- 系统会自动降级到 SVG 下载
- SVG 文件可以用浏览器打开或转换

### Q2: 点击按钮没有反应？

**检查清单**：
- [ ] 图表是否已渲染完成
- [ ] 浏览器控制台是否有错误
- [ ] 运行 `window.debugMermaidCopy()` 检查状态

### Q3: 想要复制而不是下载怎么办？

PNG 复制受浏览器安全限制，但可以：
1. 下载 PNG 文件后，手动复制
2. 或修改代码，直接下载后自动打开图片

---

## 代码块复制功能保持不变

代码块的复制功能没有改动，仍然支持一键复制代码到剪贴板。

**使用方法**：
1. 悬停代码块
2. 点击右上角**复制图标** <i class="far fa-copy"></i>
3. 代码已复制到剪贴板

---

**更新时间**: 2025-11-15
**原因**: 解决 Canvas 跨域污染问题
**影响**: Mermaid 图表从复制改为下载
