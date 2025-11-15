# Tailwind CSS v4 样式使用指南

## 项目 CSS 架构概述
- **Tailwind 版本**：项目固定在 Tailwind CSS v4.1.17，使用随仓库提交的 `frontend/tailwindcss/tailwindcss` CLI，可离线复现构建。
- **源码文件**：所有样式均从 `frontend/input.css` 入口开始，文件中包含 `@import "tailwindcss"`、`@theme` 设计令牌以及 `@layer components` 自定义组件类。
- **构建流程**：Trunk 的 `pre_build` hook 会调用 `./tailwindcss -i input.css -o static/styles.css --minify`，输出的 `frontend/static/styles.css` 会被 `index.html` 通过 `<link data-trunk rel="css">` 注入，Trunk 再把它打包进 `dist/`。
- **混合策略**：新的组件与页面优先使用 Tailwind utility classes；遗留的 `.footer`、`.article-card` 等语义类保留在 `@layer components` 内，逐步迁移时也可以与 utility 混用。

```text
frontend/input.css
    └─ tailwindcss v4.1.17 (CLI, Trunk pre_build hook)
        ├─ 解析 @theme 令牌
        ├─ 合并 @layer components
        └─ tree-shake 工具类
    ↓
frontend/static/styles.css
    └─ trunk serve/build 引入并写入 dist/
```

## 添加新样式的三种方式

### 1. 使用 Tailwind Utility Classes（推荐）
- **适用场景**：页面布局、交互动效、状态切换都可以直接拼出 utility 类。Tailwind v4 提供完整的原子化语法（`grid`, `gap-*`, `bg-*`, `text-*`, `dark:*`, `transition-*` 等），只要类名能表达需求就无需写 CSS。
- **在 Yew 中的用法**：`classes!` 宏接受 `&str`、`String`、`Option<&str>`、`Option<String>` 以及 `Classes`。可以把静态类与条件类混在一起，宏会自动跳过 `None`。
- **`classes!` 宏语法**：

```rust
use yew::{classes, function_component, html, Callback, Html};

#[function_component(PrimaryButton)]
pub fn primary_button() -> Html {
    let highlighted = true;
    let onclick = Callback::from(|_| web_sys::console::log_1(&"clicked".into()));

    html! {
        <button
            {onclick}
            class={classes!(
                "inline-flex",
                "items-center",
                "justify-center",
                "rounded-full",
                "px-5",
                "py-2.5",
                "font-medium",
                "text-[var(--text)]",
                "bg-[var(--surface)]",
                highlighted.then_some("ring-2 ring-primary/50"),
                "transition-all",
                "duration-200",
            )}
        >{ "保存" }</button>
    }
}
```

- **CSS 变量引用**：Tailwind v4 支持任何 `var(--token)` 写法，例如 `bg-[var(--surface)]`、`text-[var(--primary)]`、`border-border`（`border-border` 会自动映射到 `var(--border)`）。将颜色、间距等变量放进 `@theme` 后就可以在 utility 中调用。

### 2. 添加到 @layer components
- **适用场景**：当组件包含复杂的层级（如 `.article-card`、`.header`），或者需要复用遗留的 BEM/语义类时，把样式集中在 `@layer components` 更易维护，也方便 SSR/非 Yew 入口重用。
- **编辑位置**：`frontend/input.css` 的 `@layer components { ... }` 块已经包含 `body`, `.header`, `.article-card` 等样式；将新增类放在相同块内，Tailwind 会保持组件层级优先级。
- **示例**：

```css
/* frontend/input.css */
@layer components {
  .cta-banner {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(260px, 1fr));
    gap: var(--space-lg);
    padding: var(--space-lg);
    border-radius: var(--radius);
    background: linear-gradient(135deg, var(--primary), var(--surface-alt));
    color: var(--surface);
  }

  .cta-banner__cta {
    font-size: 1.1rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.08em;
  }
}
```

在组件中继续通过 `classes!("cta-banner", "md:items-center")` 或 `classes!("cta-banner__cta", "text-shadow")` 与 Tailwind 工具类混合。

### 3. 扩展设计令牌（@theme）
- **适用场景**：需要新增颜色、间距、阴影或断点时，通过 `@theme` 统一管理，便于主题切换和多组件共享。
- **添加变量**：在 `@theme { ... }` 内直接添加自定义变量，Tailwind 会把 `--token-name` 暴露成 `var(--token-name)`，并允许结合 `bg-[var(--token-name)]`。
- **示例**：

```css
/* frontend/input.css */
@theme {
  --brand-surface: #11133c;
  --brand-surface-dark: #050713;
  --space-3xl: 6rem;
  --shadow-pop: 0 18px 45px rgba(17, 23, 45, 0.25);
}
```

```rust
html! {
    <section class={classes!(
        "rounded-[var(--radius)]",
        "p-[var(--space-3xl)]",
        "bg-[var(--brand-surface)]",
        "shadow-[var(--shadow-pop)]",
        "dark:bg-[var(--brand-surface-dark)]",
    )}>
        { "借助 @theme token 的 Hero 区块" }
    </section>
}
```

## Rust/Yew 组件中使用样式
- `classes!` 返回 `Classes`，可在多个元素之间克隆。对重复类先用 `let base = classes!(...)`，再在子元素中 `.clone()`，减少分配。
- 条件渲染借助 `if`/`match` 和 `Option`。`bool::then_some` 是最直观的写法。
- 当类名依赖数据（如 `format!("col-span-{}", span)`）时，`classes!` 会接收 `String`。避免在 `view` 中频繁 `format!`：可提前缓存为 `use_memo` 或 `let responsive_gap = ...;`，再传入。

```rust
use yew::prelude::*;

#[derive(Properties, PartialEq)]
pub struct CardProps {
    pub featured: bool,
    pub compact: bool,
}

#[function_component(InfoCard)]
pub fn info_card(props: &CardProps) -> Html {
    let gap_token = if props.compact { "sm" } else { "lg" };
    let container = classes!(
        "flex",
        "w-full",
        "rounded-2xl",
        "border",
        "border-border",
        "bg-surface",
        format!("gap-[var(--space-{})]", gap_token),
        props.featured.then_some("shadow-xl ring-1 ring-primary/30"),
        "transition",
        "duration-200",
    );

    html! {
        <article class={container}>
            <header class={classes!("flex-1", "text-[var(--text)]", "font-semibold")}>{"Title"}</header>
            <p class={classes!("text-muted", props.compact.then_some("text-sm"))}>{"Body"}</p>
        </article>
    }
}
```

## 示范组件
- **`frontend/src/components/theme_toggle.rs`**：100% utility-first，展示 `classes!` + `group` + `dark:` + CSS 变量的组合；按钮状态和 `aria` 文案通过 Hooks + JS 交互保持同步。
- **`frontend/src/components/footer.rs`**：`<footer class="footer">` 仍引用 `@layer components` 中的布局规则，其余子节点用 Tailwind utility（`flex`, `gap`, `dark:hover:*`）快速搭建；适合逐渐迁移大型旧组件。
- **`frontend/src/components/article_card.rs`**：外层使用 `.article-card` 语义类（定义在 `input.css`），内部 `Link<Route>` 与标签列表使用 `classes!` 组合 `border-border`, `hover:text-primary` 等 utility，演示“组件类 + utility”混合策略。

## 常见问题
- **编译错误**：若看到 `unknown at-rule @theme` 或 `Cannot find module tailwindcss`，确认使用的是仓库自带 CLI (`./tailwindcss`)；断开 `trunk serve` 后手动运行 `./tailwindcss -i input.css -o static/styles.css --watch` 以获取更详细日志。
- **主题切换**：`ThemeToggle` 依赖 `window.__toggleTheme` 设置 `<html data-theme>`。若按钮无效，确认全局 JS 挂钩存在，且 `@theme` 中 dark 模式变量 (`[data-theme=dark]`) 覆盖了需要的 token。
- **响应式效果缺失**：确保使用 `sm:`/`md:` 等断点前缀。若需要自定义断点，可在 `@theme` 里添加 `--breakpoint-xxl: 1440px;` 并在 `@custom-variant`（Tailwind v4 语法）里引用。
- **性能/体积**：Tailwind CLI 会按 Rust/Yew 模板提取实际类。避免在循环中动态拼写大量唯一类（例如把数据 ID 注入类名），否则会导致 CSS 无法 tree-shake；可改用 `data-*` 属性或内联样式。

## 开发工作流
- **本地调试**：在 `frontend/` 下运行 `trunk serve --open`。Trunk 会在每次构建前执行 Tailwind hook，监听 Rust/Yew 与 CSS 改动。
- **增量查看 CSS**：若仅调试样式，可单独运行：

```bash
cd frontend
TAILWIND_MODE=watch ./tailwindcss -i input.css -o static/styles.css --watch
```

  然后在另一个终端运行 `trunk serve --no-default-features` 或保持浏览器自动刷新。
- **生产构建**：CI 或本地执行 `trunk build --release`（或仓库根目录 `trunk build -d frontend/dist`）；Tailwind 输出在 hook 中自动加上 `--minify`。
- **仅产出 CSS**：需要快速验证 CSS 产物时执行 `./tailwindcss -i input.css -o static/styles.css --minify`，再把 `static/styles.css` 提供给设计/测试同学。
