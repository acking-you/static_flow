# gpt2api admin 管理面优化 + 公开聊天/图片页面

## Context

上一轮 admin 优化告一段落（Tier 1 / Tier 2 已完成）。用户两个新需求：

1. 把 `/admin/gpt2api-rs` 管理界面做得更好用（目前只是 1126 行的单页实现，form 堆在一起、账号列表缺排序/分页、错误信息埋得很深）。
2. 新增一个**公开页面**（非 admin），用户用 admin 发的 key 登录，可以做**临时对话 + 临时图片生成**（历史仅在前端存，后端不留）。UI 参考 `~/llm/chatgpt2api/web`。

已澄清的关键决策：
- **Key 生成/鉴权**：确认「后端补一个完整的公开 key 管理 + 转发栈」。当前代码状态：后端 `list_admin_keys` 只返回 `secret_hash` / 无明文；**没有 create_key 路由**；没有「基于 key 鉴权的公开转发路由」。这些要一并补齐。
- **配额语义**：采用「总调用次数统一配额」。chat 和 image 调用都从一个 `quota_total_calls / quota_used_calls` 里扣。**废弃** `quota_total_images / quota_used_images` 的「只按图片张数」语义。
- **前端路由前缀**：`/gpt2api/*`（用户选择）。后端公开转发路径用 `/api/gpt2api/*`（和 staticflow 已有 `/api/` 约定一致，不新开 `/public/`）。
- **历史存储**：IndexedDB（跨会话保留）。
- **Chat 流式**：SSE（上游 `/v1/chat/completions` 支持 `stream: true`）。

## 目标与范围

1. **后端（backend/src/gpt2api_rs.rs + routes.rs）**：新增 key 生成/重置 API、key 鉴权中间件、公开转发路由、用 1 个"总调用次数"字段替换「图片张数字段」，usage 记录扩展到 chat/responses。
2. **前端 admin `/admin/gpt2api-rs`**：UI 重构（form 友好化、列表分页/排序/状态标签、Accounts 错误诊断提升、Keys tab 加「新建 key」弹窗，明文只显示一次）。
3. **前端公开 `/gpt2api/*`**：Login、Image（文生/编辑 + 粘贴 + Lightbox + 左侧历史）、Chat（单轮 SSE completion + 历史）三页，参考 chatgpt2api WebUI 外观，但用 Yew+Tailwind，不引入 React。

## 实施分期

由于整个工作量很大，拆成 5 期（每期独立通过 `cargo check --target wasm32-unknown-unknown` + `cargo clippy -- -D warnings` + 手动 smoke test 再进下期）。**时间充裕**，不赶；任何一期发现设计偏差可以及时折返。

---

### Phase A — 后端：key 生成 + 公开转发（骨架）

**关键文件**：
- `backend/src/gpt2api_rs.rs` — 添加 `create_key` / `rotate_key` / `delete_key` / `update_key`；新增「公开 handler 组」：`public_chat_completions` / `public_image_generation` / `public_image_edit` / `public_responses`；公共 key 鉴权中间件 `require_public_api_key`。
- `backend/src/routes.rs` — 注册新路由。
- `backend/src/schema.sql`（或相关 migration） — `gpt2api_rs_keys` 表字段：
  - 新增 `quota_total_calls`、`quota_used_calls`（保留老字段，但停止读写；或迁移后 drop，看你的 db 风格）
  - 现有 `secret_hash` 字段继续存 sha256；在创建/重置时返回**一次明文**给调用方

**新路由（全部 JSON、除标注外都要求 key 鉴权）**：
- `POST /admin/gpt2api-rs/keys`（admin 创建 key；**返回明文一次**）
- `POST /admin/gpt2api-rs/keys/:id/rotate`（admin 重置 key；**返回新明文一次**）
- `DELETE /admin/gpt2api-rs/keys/:id`
- `PATCH /admin/gpt2api-rs/keys/:id`（改 name / quota_total_calls / status / route_strategy 等）
- `POST /api/gpt2api/auth/verify`（**用户** Bearer 校验，返回 `{ ok, key: { name, quota_total_calls, quota_used_calls, status } }`；无副作用）
- `POST /api/gpt2api/chat/completions`（用户 Bearer；扣 1 次；stream=true 时走 SSE 透传）
- `POST /api/gpt2api/images/generations`（用户 Bearer；按 `n` 扣多次，即 1 次调用扣 `n` 次；如果你更偏向"固定扣 1 次"请在实施前再确认，默认按 `n` 更公平）
- `POST /api/gpt2api/images/edits`（用户 Bearer；同上）
- `POST /api/gpt2api/responses`（可选，视 upstream /v1/responses 是否常用；同上）

**鉴权中间件 `require_public_api_key`**：
- 从 `Authorization: Bearer <secret>` 读取；`secret` 以 sha256 查 `gpt2api_rs_keys.secret_hash`；status=active、未超配额才放行。
- 不放行时 401，响应体 `{ "error": "invalid_key" | "quota_exhausted" | "disabled" }` 以便前端给出确切提示。
- 扣费方式：中间件无法知道 `n`。改成由 handler 在实际转发前扣（select + update），失败回滚。写一个 `consume_key_quota(key_id, cost)` 异步函数。

**数据库字段迁移策略**：
- 若已有部署：加 `ALTER TABLE gpt2api_rs_keys ADD COLUMN quota_total_calls BIGINT NOT NULL DEFAULT 0;` + `quota_used_calls`；读时用老字段初始化（`quota_total_calls = quota_total_images` 迁移一次）；写入用新字段。
- 前端 `AdminGpt2ApiRsKeyView` 也要加 `quota_total_calls` / `quota_used_calls`（先保留老字段兼容，再逐步弃用）。

**SSE 透传**：Axum 0.8 + `reqwest` 目前没有现成 helper；实现思路：
- 把 upstream response stream bytes 用 `axum::body::Body::from_stream`（或 `Sse::new(event_stream)`）转回。
- 错误事件按 SSE 协议 `event: error\ndata: {...}` 写。
- 客户端关闭：`axum::extract::ws::CloseFrame` 不适用；用 `hyper::Body` 的 drop 被 upstream 观察到即可。

**Phase A 验收**：`curl` 手测：
```bash
# 1. admin 创建 key（返回 plaintext）
curl -X POST http://127.0.0.1:39080/admin/gpt2api-rs/keys -d '{"name":"demo","quota_total_calls":100}'
# → {"id":"...","secret_plaintext":"sk-xxx","quota_total_calls":100,...}

# 2. 用该 secret 跑公开 chat
curl -N http://127.0.0.1:39080/api/gpt2api/chat/completions \
  -H "Authorization: Bearer sk-xxx" \
  -d '{"model":"gpt-5","messages":[{"role":"user","content":"hi"}],"stream":true}'
# → SSE 流

# 3. 无 bearer 应 401
curl -i http://127.0.0.1:39080/api/gpt2api/chat/completions -d '{...}'
# → 401 invalid_key
```

---

### Phase B — 前端 admin 重构（低风险 UI 改进）

**目标**：不改 page 大结构（Phase A 完成的 TabBar 已到位），在现有 Config / Accounts / Keys / Image Gen / Playground tab 内做精细化：

**B-1 Config tab 表单化**
- 当前 `config` 用多条 textarea。改成结构化字段：`base_url`（http url 正则校验）、`admin_token`（MaskedSecretCode 展示当前值）、`api_key`（同）、`timeout_seconds`（number, min=1）。
- 每个字段 inline 校验；保存成功弹 toast `sonner` 风格（Yew 没有 sonner，用已有 `admin.rs` 的 notice 横幅即可，或者下面 B-7 做一个 Toast 组件）。

**B-2 Accounts tab**
- 目前 search 已做（Tier 1）。追加：
  - **按列排序**：`status` / `quota_remaining` / `last_used_at` / `last_refresh_at`。表头点击切换 asc/desc。
  - **状态标签**：用共享 `status_badge_class`（pattern 同 admin.rs）：active / restricted / disabled / unknown 各有色。
  - **相对时间**：`last_refresh_at` / `last_used_at` 显示「3 分钟前」（用 `format_relative_time(ts_ms)`，新增在 `llm_access_shared.rs`）。
  - **Error 诊断**：`last_error` 现在在 `<div class="mt-2 text-xs text-red-600">` 里，加一个 error icon + hover tooltip + 「复制错误」按钮；提供「临时禁用」 + 「立即重试刷新」两个快捷键。
- 分页：若 accounts > 20，前端简单分页（`PAGE_SIZE=20`）。

**B-3 Keys tab**
- 现状：只读列表（secret 只有 hash）。
- 改为：
  - 顶部「新建 Key」按钮 → 弹 `KeyCreateDialog`：字段 name / quota_total_calls / route_strategy / request_max_concurrency / request_min_start_interval_ms。
  - 提交后拿到 plaintext，**一次性显示在 Dialog 中**（用 `MaskedSecretCode`），用户必须点「已复制」才关闭。关闭后前端丢失明文。
  - 行内「Rotate」按钮 → 二次确认 → 再显示一次新明文。
  - 行内「Delete」按钮 → 确认（复用 `confirm_destructive`）。
  - 行内「Edit」按钮 → 弹 Dialog 修改 name/quota/status。
  - 新增列：`quota_used_calls / quota_total_calls`（带进度条，用共享 `usage_ratio` / `format_percent`）。

**B-4 Usage tab**
- 分页：复用 `components::pagination::Pagination`。
- 过滤：`key` / `account_name` / `endpoint` dropdown；按时间倒序默认。
- 每行展开查看 error_message（error_code 非空时默认展开）。

**B-5 Image Gen / Playground tab**
- 继续保留给管理员手测用，不做大改；只把 request/response JSON 的 pre 改成带行号的可折叠组件（可选）。

**B-6 新共享 util**
- `frontend/src/pages/llm_access_shared.rs`：新增
  - `pub fn format_relative_time(now_ms: f64, ts_ms: i64) -> String`
  - 如果 B-7 要做 Toast 组件，放 `frontend/src/components/toast.rs`。

**B-7（可选）Toast 组件**
- 用 `gloo_timers::Timeout` 做 3s 自动消失，`use_context` 或全局 `UseStateHandle<VecDeque<Toast>>`。不是必须，现有 notice 横幅也够用；**默认不做，除非时间充裕**。

---

### Phase C — 前端公开页面骨架

**新增前端路由**（`frontend/src/router.rs`）：
- `Route::Gpt2apiLogin` → `/gpt2api/login`
- `Route::Gpt2apiImage` → `/gpt2api/image`
- `Route::Gpt2apiChat` → `/gpt2api/chat`

**新增页面**（`frontend/src/pages/`）：
- `gpt2api_public_shared.rs`：key 存储（IndexedDB wrapper）+ `use_require_key` hook + API client（fetch `/api/gpt2api/*`）。
- `gpt2api_login.rs`：单输入框「输入 API Key」→ 调 `/api/gpt2api/auth/verify` → 存 IndexedDB → 跳 `/gpt2api/chat`。
- `gpt2api_layout.rs`：顶栏（右上角 Logout 清除 key + 跳登录页；中间 tab：Chat / Image；左上 brand）。所有公开页共享。
- `gpt2api_image.rs` + `gpt2api_chat.rs`：见下两期。

**IndexedDB 层**（`components/idb_store.rs` 或 `pages/gpt2api_public_shared.rs`）：
- 用 `web_sys::IdbDatabase` 手写最小 wrapper：
  - `open("staticflow_gpt2api", 1)` 建 object store `conversations`（keyPath=id）、`images`（keyPath=id）、`auth`（keyPath=id）。
  - async `put<T: Serialize>(store, item)` / `get<T: DeserializeOwned>(store, key)` / `delete(store, key)` / `all<T>(store) -> Vec<T>`。
- 参考：`gloo-storage` 不直接做 IndexedDB（只做 localStorage），需要自己写。或者用 **`idb-rs`** crate 来省事，但那又引入新依赖 — 我倾向自己写 100 行 wrapper。

**路由守卫**：`gpt2api_image.rs` / `gpt2api_chat.rs` 在 mount 时调用 `use_require_key`，没有则 `navigator.push(Route::Gpt2apiLogin)`。

**Phase C 验收**：打开 `/gpt2api/image` 未登录 → 跳 `/gpt2api/login`；登录后回 `/gpt2api/image`，显示空状态（still no 生成逻辑）。

---

### Phase D — 公开图片页

**文件**：`frontend/src/pages/gpt2api_image.rs`（+ 拆分 `gpt2api_image/sidebar.rs / composer.rs / results.rs` 子模块）。

**对照 chatgpt2api WebUI**：
- 左侧 `ImageSidebar`：历史会话列表（IndexedDB `conversations` store），点击切换，「删除」「清空」。
- 主区域 `ImageResults`：当前会话的 prompt + reference images + generated images 网格；点击图片打开 Lightbox。
- 底部 `ImageComposer`：textarea（回车发送，Shift+Enter 换行）、模式切换（文生/编辑）、引用图上传（粘贴 + 多图 + 缩略图删除按钮）、模型下拉、张数 number、发送按钮。

**Lightbox**：新增 `components/image_lightbox.rs`；props：images `Vec<String (data:url)>` + current_index + onClose。键盘 `ArrowLeft/Right` 切换、`Esc` 关闭。用 `portal` 绑 body 渲染（Yew 里可以直接把 `<dialog>` 渲到 main 区域，不必 portal，tailwind `fixed inset-0 z-50` 即可）。

**图片存储**：生成后 upstream 返回 `b64_json`，IndexedDB 只存 `{ id, conversationId, b64_json }`，列表视图和 Lightbox 用 `data:image/png;base64,...` 直接渲染。**后端不留**（用户要求；Phase A 的 `/api/gpt2api/images/*` 路由在后端就不持久化，只转发）。

**API 调用**：走 `/api/gpt2api/images/generations` 和 `/edits`（后端透传 upstream）；成功响应 `{ data: [{ b64_json }] }`；扣 `n` 次配额。

---

### Phase E — 公开聊天页（SSE）

**文件**：`frontend/src/pages/gpt2api_chat.rs`。

**设计**：
- 单轮 completion。左侧历史（IndexedDB `conversations` 里的 chat 类型）；主区 messages 列表；底部 textarea + 发送按钮。
- **不做多轮上下文**（按 chatgpt2api WebUI 示例也是每次只发最新 prompt）。若要支持多轮，下期再加。
- **SSE 实现**：
  - `web_sys::EventSource` 不支持 POST body，不能用。
  - 改用 `fetch` + `ReadableStream`，`gloo_net::http::Request::post(...).send().await?.body()` 再 `ReadableStream::getReader()`。Yew/wasm 没现成的 SSE for POST helper，需要写一个解析器：按 `data: ...\n\n` 分包。或者更稳：直接在响应 body 里 poll 字节、用 `\n\n` 切分。
  - 上游 SSE 每行 `data: {"choices":[{"delta":{"content":"..."}}]}`，空行表示结束。我在 parser 里 accumulate `delta.content` 拼到当前 assistant message。
  - 已经有 SSE 批处理先例：`frontend/src/components/stream_chunk_batcher.rs`（但它是 listen 模式，基于 `EventSource`）。**chat SSE 用 POST-stream**，不能直接用；需要新增 `components/post_sse_stream.rs`（reusable wrapper）。

**UI 参考 chatgpt2api**：
- 消息气泡：用户右、assistant 左；assistant 渲染 markdown（用现有 `components::raw_html` 吗？那是给管理员文章用的；聊天 markdown 建议引入一个轻量 md parser 或直接用 `pulldown_cmark` — frontend/Cargo.toml 已经有 `pulldown-cmark`，复用它）。
- 滚动：自动滚到底部；流式过程中也保持底部。
- 停止生成按钮：点击后 abort `fetch`（`AbortController` via `wasm-bindgen` or gloo_net 自带）。

**API 调用**：`POST /api/gpt2api/chat/completions` body: `{ model, messages, stream: true }`；扣 1 次配额；Bearer 为 localforage 里的 key。

---

### Phase F（可选）— 细节打磨

- Dark mode 适配（chatgpt2api 是 light only，我们继承 staticflow 主题变量 `var(--surface)` 等即可免费拿到 dark）。
- 移动端响应式（sidebar 折叠成抽屉）。
- Toast（如果 B-7 没做的话补齐）。
- 若用户反馈，加多轮聊天上下文。

## 复用已有能力

- 共享组件：`SearchBox`、`render_tab_bar`、`MaskedSecretCode`、`ChunkBatcher`（chat 流里可参考它的批渲染思路，但要独立写一个 POST SSE 版）。
- Util：`format_ms` / `format_ms_iso` / `confirm_destructive` / `usage_ratio` / `format_percent`。
- `components::pagination::Pagination`。
- `pulldown-cmark`（已引入，直接渲染 assistant markdown）。

## 关键文件清单

后端（Phase A）：
- `backend/src/gpt2api_rs.rs`
- `backend/src/routes.rs`
- `backend/src/schema.sql`（若有 migration 机制）
- 相关 state 结构里的 key repo 模块

前端 admin（Phase B）：
- `frontend/src/pages/admin_gpt2api_rs.rs`（继续重构）
- `frontend/src/pages/llm_access_shared.rs`（加 `format_relative_time`）
- 可能新增 `frontend/src/components/toast.rs`（B-7 可选）

前端公开（Phase C-E）：
- `frontend/src/router.rs`（新增 3 条 Route）
- `frontend/src/pages/mod.rs`
- `frontend/src/pages/gpt2api_public_shared.rs`（新）
- `frontend/src/pages/gpt2api_login.rs`（新）
- `frontend/src/pages/gpt2api_layout.rs`（新，公共顶栏）
- `frontend/src/pages/gpt2api_image.rs`（新，含 composer/results/sidebar 子模块）
- `frontend/src/pages/gpt2api_chat.rs`（新）
- `frontend/src/components/image_lightbox.rs`（新）
- `frontend/src/components/post_sse_stream.rs`（新；支持 fetch POST + 读 SSE）
- `frontend/src/components/idb_store.rs` 或放 `gpt2api_public_shared.rs`（IndexedDB wrapper）

## 验证流程

每期按下面表手动验证；每项前先过 `cargo check --target wasm32-unknown-unknown` + `cargo clippy -- -D warnings`。

| 期  | 验证路径                         | 验证点                                                                                                                                |
| --- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- |
| A   | `curl` 命令见上方                | create/rotate 返回明文一次；公开路由用错 key 返回 401；用对 key 能转发；配额耗尽后 403                                                |
| B   | `/admin/gpt2api-rs` → Config tab | 字段独立输入、保存弹 toast/notice；Accounts 列可点排序；相对时间正确；Keys 「新建 Key」弹窗能拿到明文；Rotate 生效；Usage 分页 + 过滤 |
| C   | `/gpt2api/image`                 | 没 key → 跳 login；登录后回到原页；Logout 清 key；IndexedDB 里能看到 auth 记录（devtools）                                            |
| D   | `/gpt2api/image`                 | 文生图：输入 prompt → 生成；编辑图：粘贴图片也能进 reference；Lightbox Esc 关闭；删除 / 清空历史；刷新页面历史还在（IndexedDB）       |
| E   | `/gpt2api/chat`                  | 输入问题 → SSE 逐 token 显示；能 Abort；markdown 代码块/链接能渲染；历史保留；配额耗尽 toast                                          |

## 风险与假设

- **不破坏现有 admin**：B 期间 admin_gpt2api_rs.rs 继续在同一个 page 下演进；任何中途状态都要保证 cargo check / clippy 通过。
- **`quota_total_images` 字段迁移**：新字段上线后保留老字段读兼容，但写只写新字段。**假设**你可以接受这个迁移；若不接受，改成"保留 images 字段 + 新加 calls 字段 + Chat 扣 calls，Image 扣 images"也行（更复杂，不推荐）。
- **SSE POST stream**：`gloo_net` / `web_sys` 对 fetch ReadableStream 不算一等支持；可能需要用 `web_sys::Response::body().get_reader()` 或用 `wasm-streams` crate（~~已有~~ 没有；加入要审视 wasm bundle 大小）。Phase E 启动前先写个最小 POC 验证可行性（~50 行），不行再降级到非流式。
- **Lightbox 键盘事件**：和 Yew `use_effect_with` 的 document listener 交互要小心 leak；参考现有 `components/image_with_loading.rs` 有无类似。
- **跨会话保持密码**：IndexedDB 明文存 key；若 XSS 风险敏感，改成 `sessionStorage`（关 tab 丢失）。本轮按 IndexedDB 实现，留一个 `clear_all_data` 按钮在 Logout 旁。
- **Phase A 最大**：要改数据库模型 + 新增 4-5 条路由 + SSE 透传。估计工作量是 Phase B+C+D+E 之和的一半。提交前请单独在 backend 里手测。

## 推进节奏

按 A → B → C → D → E 顺序推进。每期独立 checkpoint：
- Phase A 完成后先 `cargo run -p static-flow-backend` 起后端，用 curl 手测新路由全部 OK，**再**进 B。
- Phase C 是前端骨架，没有生成/发送功能，页面打开能跑通即可。
- D / E 各自独立，谁先谁后不重要；建议先 D（参考代码更直接），E 的 SSE POC 会在 D 完成后有更多信心。
