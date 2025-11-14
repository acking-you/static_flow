# 头像旋转动画效果

## 功能说明

为首页头像添加了鼠标悬停旋转动画效果。

## 效果展示

- **默认状态**：头像静止
- **鼠标悬停**：头像持续旋转（360度）
- **动画速度**：3秒完成一圈
- **动画类型**：线性匀速旋转（linear）
- **重复次数**：无限循环（infinite）

## 实现原理

### CSS 动画定义

```css
@keyframes avatar-spin {
  from {
    transform: rotate(0deg);
  }
  to {
    transform: rotate(360deg);
  }
}
```

### 触发条件

```css
.home-avatar-link:hover img {
  animation: avatar-spin 3s linear infinite;
}
```

**解释**：
- 当鼠标悬停在头像链接上时（`.home-avatar-link:hover`）
- 对 `img` 元素应用旋转动画
- 动画持续 `3s`（3秒）
- 使用 `linear` 时间函数（匀速）
- `infinite` 表示无限循环

### 保持其他效果

原有的悬停效果保持不变：
```css
.home-avatar-link:hover {
  transform: translateY(-6px);  /* 向上浮动 */
  box-shadow: 0 20px 45px rgba(0, 0, 0, 0.2);  /* 阴影加深 */
}
```

**设计思路**：
- 外层容器（`.home-avatar-link`）负责位移和阴影
- 内部图片（`img`）负责旋转
- 两个效果互不干扰

## 技术细节

### 为什么用 `transform: rotate()` 而不是其他方式？

- ✅ **性能优化**：`transform` 不会触发重排（reflow），只触发重绘（repaint）
- ✅ **GPU 加速**：浏览器会使用硬件加速渲染旋转
- ✅ **流畅度高**：60fps 流畅动画

### 为什么对 `img` 而不是 `.home-avatar-link` 旋转？

因为外层容器已经有 `translateY(-6px)` 变换：
```css
/* ❌ 错误示例 - transform 会互相覆盖 */
.home-avatar-link:hover {
  transform: translateY(-6px) rotate(360deg); /* 只有最后一个生效 */
}

/* ✅ 正确方案 - 分层变换 */
.home-avatar-link:hover {
  transform: translateY(-6px); /* 外层负责位移 */
}
.home-avatar-link:hover img {
  animation: avatar-spin 3s linear infinite; /* 内层负责旋转 */
}
```

### 动画参数说明

| 参数 | 值 | 作用 |
|------|---|------|
| `animation-name` | `avatar-spin` | 使用的动画名称 |
| `animation-duration` | `3s` | 旋转一圈需要 3 秒 |
| `animation-timing-function` | `linear` | 匀速旋转（不加速减速） |
| `animation-iteration-count` | `infinite` | 无限循环 |

**完整写法**：
```css
animation: avatar-spin 3s linear infinite;

/* 等价于： */
animation-name: avatar-spin;
animation-duration: 3s;
animation-timing-function: linear;
animation-iteration-count: infinite;
```

## 自定义调整

### 1. 调整旋转速度

**更快**（2秒一圈）：
```css
.home-avatar-link:hover img {
  animation: avatar-spin 2s linear infinite;
}
```

**更慢**（5秒一圈）：
```css
.home-avatar-link:hover img {
  animation: avatar-spin 5s linear infinite;
}
```

### 2. 改为反向旋转

```css
.home-avatar-link:hover img {
  animation: avatar-spin 3s linear infinite reverse;
}
```

### 3. 使用缓动函数（先快后慢）

```css
.home-avatar-link:hover img {
  animation: avatar-spin 3s ease-in-out infinite;
}
```

常用时间函数：
- `linear` - 匀速
- `ease` - 慢-快-慢
- `ease-in` - 慢-快
- `ease-out` - 快-慢
- `ease-in-out` - 慢-快-慢（更平滑）

### 4. 旋转固定角度后停止

如果只想旋转一圈后停止：
```css
.home-avatar-link:hover img {
  animation: avatar-spin 3s linear 1; /* 最后的 1 表示只执行一次 */
  animation-fill-mode: forwards; /* 保持最终状态 */
}
```

### 5. 添加延迟启动

悬停后 0.5 秒再开始旋转：
```css
.home-avatar-link:hover img {
  animation: avatar-spin 3s linear infinite;
  animation-delay: 0.5s;
}
```

## 浏览器兼容性

| 浏览器 | 支持版本 |
|--------|---------|
| Chrome | 43+ ✅ |
| Firefox | 16+ ✅ |
| Safari | 9+ ✅ |
| Edge | 12+ ✅ |

**注意**：现代浏览器全部支持，无需前缀（`-webkit-`、`-moz-`）。

## 性能影响

- **CPU 占用**：极低（GPU 硬件加速）
- **内存占用**：无额外开销
- **影响范围**：仅悬停时生效，不影响其他元素

**性能测试**：
- 使用 `transform` 的动画通常保持在 60fps
- 不会造成页面卡顿或性能问题

## 测试方法

1. 启动开发服务器：
   ```bash
   cd frontend
   trunk serve
   ```

2. 访问首页：`http://127.0.0.1:8080`

3. 鼠标悬停在头像上

4. 观察效果：
   - ✅ 头像开始旋转
   - ✅ 旋转持续进行
   - ✅ 同时头像向上浮动
   - ✅ 阴影加深

5. 移开鼠标：
   - ✅ 旋转立即停止
   - ✅ 头像恢复原位

## 调试工具

打开浏览器 DevTools：

**查看动画**（Chrome）：
1. 打开 DevTools → 更多工具 → Animations
2. 悬停头像触发动画
3. 可以看到动画时间线

**检查 CSS**：
1. 右键头像 → 检查元素
2. 查看 Computed 标签页
3. 搜索 `animation` 查看应用的动画

**性能分析**：
1. DevTools → Performance 标签
2. 点击录制
3. 悬停头像
4. 停止录制
5. 查看 FPS 和渲染性能

## 代码位置

**CSS 文件**：`frontend/static/styles.css`（末尾）

```css
/* 头像旋转动画 */
@keyframes avatar-spin {
  from { transform: rotate(0deg); }
  to { transform: rotate(360deg); }
}

.home-avatar-link:hover img {
  animation: avatar-spin 3s linear infinite;
}
```

**HTML 结构**：`frontend/src/pages/home.rs`

```rust
<div class="home-avatar">
    <Link<Route> to={Route::Posts} classes={classes!("home-avatar-link")}>
        <img src="/static/avatar.jpg" alt="作者头像" />
    </Link<Route>>
</div>
```

---

**实现时间**: 2025-11-15
**效果**: 头像悬停持续旋转
**性能**: 优秀（GPU 加速）
