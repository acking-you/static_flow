# Tailwind Styling Guide

## 1. 项目 CSS 架构概述
- **Tailwind 版本**：项目固定在 Tailwind CSS v4.1.17，由根目录的 `tailwindcss/tailwindcss` CLI 二进制执行。
- **混合使用策略**：我们保留 `static/styles.css` 中已经存在的组件/排版规则（兼容旧布局），同时对新页面、新交互优先使用 Tailwind utility classes 来提升开发效率。必要时再写自定义组件样式或主题变量。
- **构建流程**：所有源样式都在 `frontend/input.css`。Tailwind CLI 以 `input.css` 为入口，产出 `static/styles.css`，该文件在构建过程中被 Trunk 注入到最终的 WASM 页面中，不应手动编辑。

```
input.css  --(Tailwind CLI v4.1.17)-->  static/styles.css  --(Trunk serve/build)-->  dist/
```

## 2. 三种添加样式的方式

### 方式一：使用 Tailwind Utility Classes（推荐用于新功能）
- **适用场景**：绝大多数布局（flex/grid）、间距、颜色、排版、阴影、边框状态等。
- **Yew 中的写法**：通过 `classes!` 宏组合多个字符串，每个 class 必须是独立的参数，便于条件拼接。
- **CSS 变量**：Tailwind v4 支持 `bg-[var(--bg)]`、`text-[var(--primary)]` 等写法，可直接引用 `@theme` 中的设计令牌。

```rust
use yew::prelude::*;

#[function_component(SaveButton)]
pub fn save_button() -> Html {
    html! {
        <button
            class={classes!(
                "inline-flex",
                "items-center",
                "gap-2",
                "rounded-full",
                "bg-[var(--primary)]",
                "px-4",
                "py-2",
                "text-sm",
                "font-medium",
                "text-white",
                "shadow-lg",
                "transition",
                "hover:bg-[color-mix(in srgb,var(--primary),#ffffff_12%)]"
            )}
        >
            <span class="i-ph-check-bold" />
            {"保存"}
        </button>
    }
}
```

```rust
// 🚫 错误示例：Rust 编译器会报 “string literals must not contain more than one class”
html! {
    <button class="px-4 py-2 text-white bg-[var(--primary)]">
        {"保存"}
    </button>
}
```

### 方式二：在 @layer components 中添加组件样式
- **适用场景**：需要复杂交互动效、同一个组件被复用多次、或 utility classes 难以表达的长样式块。
- **操作路径**：编辑 `frontend/input.css`，在现有的 `@layer components { ... }` 块中追加规则。

```css
@layer components {
  .cta-button {
    @apply inline-flex items-center gap-2 rounded-full px-5 py-3 font-semibold text-white transition;
    background: color-mix(in srgb, var(--primary), #ffffff 5%);
    box-shadow: 0 12px 24px rgba(29, 158, 216, 0.25);
  }

  .cta-button:hover {
    transform: translateY(-1px);
    box-shadow: 0 16px 32px rgba(29, 158, 216, 0.3);
  }

  .article-card {
    @apply grid gap-4 rounded-2xl border border-[var(--border)] bg-[var(--surface)] p-6 transition;
    box-shadow: var(--shadow);
  }
}
```

> 提示： `.article-card` 继续在 Rust 组件中以 `classes!("article-card", "md:grid-cols-12")` 的方式和 Tailwind utility 混用。

### 方式三：扩展设计令牌（CSS 变量）
- **适用场景**：需要新增全局颜色、间距、阴影、断点等变量，并在多个组件中复用。
- **操作路径**：在 `frontend/input.css` 的 `@theme { ... }` 中定义变量，Tailwind 会自动把它们暴露为 `var(--token-name)` 并允许在 utility classes 中引用。

```css
@theme {
  --brand-accent: #7c3aed;
  --card-shadow-strong: 0 35px 80px rgba(15, 23, 42, 0.25);
  --breakpoint-xl: 1344px;
}
```

定义完成后即可在 Rust 组件中写 `class={classes!("bg-[var(--brand-accent)]", "xl:max-w-[var(--breakpoint-xl)]")}`。新增变量后别忘了在暗色主题块中提供相应的值。

## 3. 在 Rust/Yew 组件中使用样式
- **classes! 宏**：始终把每个类名作为单独参数，避免一个字符串里包含多个类。宏会去重并合并。
- **动态类**：可以内联 `if/else` 表达式或 `Option`，只要最终返回 `&str`/`String`。
- **组合多个来源**：可以把 `Classes` 再次传入 `classes!`，或用 `classes!(base_classes, conditional_classes)`，以便重用。

```rust
let is_active = use_state(|| false);
let highlight_size = 2;

let button_classes = classes!(
    "group",
    "flex",
    "items-center",
    "justify-between",
    "rounded-xl",
    "px-4",
    "py-3",
    format!("gap-{}", highlight_size),
    if *is_active { "text-[var(--primary)]" } else { "text-[var(--muted)]" },
    if *is_active { "bg-[color-mix(in srgb,var(--primary),transparent_80%)]" } else { "bg-transparent" }
);

html! {
    <button class={button_classes.clone()} onclick={{
        let is_active = is_active.clone();
        Callback::from(move |_| is_active.set(!*is_active))
    }}>
        <span>{"主题切换"}</span>
        <span class={classes!("i-ph-sun-bold", "text-lg")} />
    </button>
}
```

## 4. 示范组件参考
- `src/components/theme_toggle.rs`：全部使用 Tailwind utility classes，展示条件类名、CSS 变量引用、状态动画的最佳实践。
- `src/components/footer.rs`：同时包含 `.footer` 之类的组件类和 Tailwind utility，用于展示“保留旧样式 + 按需添加 utility” 的混合策略。
- `src/components/article_card.rs`：保留复杂的 `.article-card`、`.meta` 等样式，但在内部文字、标签、按钮上仍然搭配 `flex`, `gap`, `text-sm` 等 utility classes。

## 5. 常见问题和注意事项
- **编译错误**：`string literals must not contain more than one class` 提示说明你把多个 class 写在同一个字符串里。拆成 `classes!("px-4", "py-2", ...)` 即可。
- **主题切换**：切换按钮会把 `data-theme` 设置为 `dark` 或 `light`。任何新样式如果依赖颜色，应使用 `var(--token)` 或在 `[data-theme=dark]` 块内覆盖，避免硬编码。
- **响应式设计**：Tailwind 提供 `sm:`, `md:`, `lg:`, `xl:` 前缀。若定义了自定义断点变量，可在 `@theme` 中配置 `--breakpoint-*` 并在 utility 中使用 `@media`。
- **性能优化**：避免在组件中生成大量字符串拼接；尽量复用 `Classes` 对象。CSS 层面保持 utility+组件类混合可减少最终 CSS 体积，Tailwind 会移除未使用的样式。

## 6. 开发工作流
- **修改样式后的构建命令**：`./tailwindcss/tailwindcss -i ./input.css -o ./static/styles.css`；在 CI 或单次构建中加入 `--minify`。
- **热重载开发**：同时运行 `trunk serve` 和 `./tailwindcss/tailwindcss -i ./input.css -o ./static/styles.css --watch`。Trunk 会重新加载编译后的 CSS 与 WASM。
- **手动编译 Tailwind**：若只想验证 CSS，可执行 `TAILWIND_MODE=watch ./tailwindcss/tailwindcss -i ./input.css -o ./static/styles.css --watch`，或在 VS Code 任务中加入该命令。
- **生产构建**：先运行 `./tailwindcss/tailwindcss -i ./input.css -o ./static/styles.css --minify`，随后 `trunk build --release` 生成 `dist/`。确保在提交前包含更新后的 `static/styles.css`。
