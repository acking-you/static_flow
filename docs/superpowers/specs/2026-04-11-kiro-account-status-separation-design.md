# Kiro Account Status Separation Design

## Goal

把当前公开页面可见的 Kiro 账号状态信息完全收回到 admin 边界内，同时修正 admin 内部“账号维护表单”和“大量状态卡片浏览”混在同一个页面里的坏设计。

本次设计的目标是：

- 公开页面继续提供 Kiro 接入说明，但不再暴露任何 Kiro 账号状态
- Kiro 账号状态卡片迁移到独立的 admin 页面中
- 现有账号状态卡片的视觉样式和信息结构保持不变
- 新 admin 状态页支持分页和按账号名前缀快速检索
- 旧的 `Admin Kiro Gateway -> Accounts` tab 收敛为账号维护入口，不再承担状态监控职责
- 不破坏现有公开接口的基本 JSON 结构兼容性

## Scope

### In Scope

- 收口公开 `Kiro Access` 数据暴露边界
- 移除公开页面中的 Kiro 账号状态展示
- 新增独立 admin Kiro 账号状态页面和路由
- 新增 admin Kiro 账号状态查询接口
- 为 admin 状态查询增加前缀搜索和分页
- 从现有 admin 页面复用 `KiroAccountCard` 组件
- 调整 `Admin Kiro Gateway` 的 `Accounts` tab 职责
- 增加后端和前端测试覆盖

### Out of Scope

- 修改 `KiroAccountCard` 的视觉设计或字段内容
- 重构账号导入、手动创建、编辑的业务逻辑
- 引入模糊搜索、多字段搜索或排序器
- 重构 Kiro 余额采集、缓存刷新或状态计算逻辑
- 将 admin 账号维护页整体改成新的表格管理系统

## Confirmed Findings

### 1. 当前公开接口直接暴露所有 Kiro 账号状态

当前公开入口 `/api/kiro-gateway/access` 会直接返回 `accounts` 列表。该列表来自所有已注册 Kiro 账号的状态拼装结果，而不是页面上的演示数据。

这意味着问题不只是前端渲染层，而是公开 API 本身已经越过了 admin 边界。

### 2. 当前公开页面有两处消费这份公开账号状态

Kiro 状态当前会在两个公开页面展示：

- `/kiro-access`
- `/llm-access`

因此如果只改其中一个页面，另一个页面仍会继续泄露相同的信息。

### 3. 当前 admin 页面把维护和浏览混在了一起

`Admin Kiro Gateway -> Accounts` tab 里同时承载了：

- 从本地导入账号
- 手动创建账号
- 账号编辑
- 大量账号状态卡片浏览

随着账号数量增加，这种设计会同时恶化两个场景：

- 维护操作被长页面淹没
- 状态浏览缺少分页和快速定位

### 4. 现有状态卡片本身不需要重做

现有 admin 页面已经有可用的 `KiroAccountCard` 组件。用户希望“保持现在的卡片不变”，因此正确方向是把卡片放到更合适的页面承载，而不是重画一套 UI。

## Options Considered

### Option A: 只改公开前端，不改后端公开接口

做法：

- 公开页不再渲染 Kiro 状态
- 公开 API 仍继续返回 `accounts`
- admin 页面结构基本不变，只在前端做本地分页

优点：

- 改动最小

缺点：

- 账号状态仍然能被公开接口拿到，边界不干净
- admin 坏设计没有真正拆开
- 分页仍然建立在前端全量拉取之上

### Option B: 公开接口收口 + 新 admin 状态页 + 后端分页前缀搜索

做法：

- 公开接口保留 `accounts` 字段，但固定返回空数组
- 公开页不再展示 Kiro 账号状态
- 新增独立 admin 状态页，复用现有卡片
- 旧 `Accounts` tab 只保留维护入口和跳转入口
- 状态页使用 admin 查询接口做分页和前缀搜索

优点：

- 公开和 admin 边界清晰
- 不破坏公开 JSON 结构兼容性
- 维护页和状态页职责分离
- 分页和搜索可以随着账号增长稳定工作
- 复用现有卡片，改动集中且可控

缺点：

- 需要新增 admin 路由、接口和页面

### Option C: 完整重做 admin 账号模块

做法：

- 维护页、状态页、编辑页全部拆成独立模块
- 重新定义一套列表和编辑交互

优点：

- 结构最彻底

缺点：

- 远超当前需求
- 会引入不必要的 UI 和状态迁移风险

## Chosen Design

采用 Option B。

核心原则：

- 公开边界只保留接入信息，不保留账号状态信息
- admin 状态浏览和 admin 维护操作必须拆开
- 状态卡片原样复用，不重做视觉
- 搜索和分页放到后端查询层做，避免把问题继续留在前端

## Detailed Design

### 1. Public Boundary

#### 1.1 公开响应结构

`KiroAccessResponse` 继续保留：

- `base_url`
- `gateway_path`
- `auth_cache_ttl_seconds`
- `accounts`
- `generated_at`

其中 `accounts` 字段继续存在，但公开接口固定返回空数组 `[]`。

这样可以满足两个约束：

- 新的公开页面拿不到任何账号状态
- 旧的外部调用方如果仍然依赖这个字段名，不会因为字段消失而直接反序列化失败

#### 1.2 公开页面行为

以下公开页面继续存在，但不再显示 Kiro 账号状态区块：

- `/kiro-access`
- `/llm-access`

它们仍然保留：

- base URL 展示
- copy 按钮
- 接入说明和示例命令

它们不再保留：

- quota snapshot
- Kiro account usage bars
- Kiro disabled / subscription / reset 状态摘要

### 2. Admin Status Surface

#### 2.1 新页面职责

新增独立 admin 页面：

- route: `/admin/kiro-gateway/accounts`

该页面只负责：

- 展示账号状态卡片
- 按账号名前缀搜索
- 分页浏览
- 刷新当前查询结果

该页面不承载：

- 本地导入账号
- 手动创建账号
- 复杂编辑表单

#### 2.2 卡片复用

页面直接复用现有 `KiroAccountCard` 组件。

不修改卡片内部视觉结构，不改变卡片展示字段，不重新设计卡片布局。

唯一变化是卡片所在的页面容器增加：

- 搜索工具条
- 分页控件
- 列表摘要

#### 2.3 页面顶部工具条

新状态页顶部提供以下最小控件：

- 前缀搜索输入框
- `Search`
- `Clear`
- `Refresh`
- 每页数量选择器，例如 `12 / 24 / 48`

搜索语义固定为“账号名前缀匹配”，不做模糊搜索，不做跨字段搜索。

#### 2.4 列表和分页

状态卡片继续使用现有双列网格布局，移动端自动退化为单列。

分页控件使用最小集合：

- `Prev`
- 当前页 / 总页数
- `Next`
- 总结果数

不引入复杂页码跳转器或无限滚动。

### 3. Admin Maintenance Surface

#### 3.1 旧 Accounts Tab 的新职责

`Admin Kiro Gateway -> Accounts` tab 调整为“账号维护入口页”。

它保留：

- Import Local Kiro CLI Auth
- Create Manual Kiro Account
- 与账号维护直接相关的入口

它移除：

- 整页的大规模 `KiroAccountCard` 浏览区

它新增：

- 一个明显的“打开账号状态页”入口

#### 3.2 页面分工

调整后分工如下：

- `Kiro Access`: 公开接入说明
- `Admin Kiro Gateway`: 配置、导入、创建、key、group、usage
- `Admin Kiro Account Status`: 账号状态浏览、搜索、分页

这样维护操作和状态监控不再混在同一个长页面里。

### 4. Admin API

#### 4.1 新查询接口

新增 admin-only Kiro 账号状态查询接口：

- `GET /admin/kiro-gateway/accounts/statuses`

查询参数：

- `prefix`: 可选，账号名前缀
- `limit`: 可选，每页数量
- `offset`: 可选，偏移量

返回结构：

```json
{
  "accounts": [...],
  "total": 123,
  "limit": 24,
  "offset": 48,
  "generated_at": 1712812345
}
```

其中 `accounts` 的元素继续使用现有 `KiroAccountView`。

#### 4.2 搜索语义

搜索规则明确如下：

- 对 `prefix` 做首尾空格裁剪
- 使用账号名匹配
- 仅支持前缀匹配
- 建议按不区分大小写处理

采用前缀匹配的原因：

- 账号名就是主索引
- 实现简单、性能稳定
- 避免把状态页演变成复杂检索台

#### 4.3 分页语义

- `limit` 为空时使用默认值 `24`
- `limit` 必须受到最大值约束，防止一次拉取过大
- `offset` 小于 `0` 的情况不接受
- 返回中的 `total` 代表当前筛选条件下的总条数

前端规则：

- 搜索词变化时，页码重置到第一页
- 每页条数变化时，页码重置到第一页
- 点击刷新时保留当前筛选条件和当前页

### 5. Data Flow

#### 5.1 公开页

1. 前端请求 `/api/kiro-gateway/access`
2. 后端返回接入信息和空的 `accounts`
3. 公开页只渲染接入信息

#### 5.2 admin 状态页

1. 页面加载时请求 admin 状态接口
2. 后端根据 `prefix/limit/offset` 查询并分页
3. 前端用返回的 `accounts` 渲染现有卡片
4. 用户修改搜索词、每页数量或页码时重新请求

#### 5.3 admin 维护页

1. 维护页继续使用现有 admin 账号加载逻辑
2. 页面只承载导入/创建/维护入口
3. 浏览状态时跳转到独立状态页

## Compatibility

### Backward Compatibility

- 公开 `KiroAccessResponse` 不删除 `accounts` 字段
- 公开 `accounts` 固定为空数组，而不是移除字段
- 现有 `KiroAccountCard` 组件保持原样复用
- 现有账号导入、创建、编辑逻辑保持不变

### Intentional Behavior Changes

- 公开页面不再展示任何 Kiro 账号状态
- `LLM Access` 页面不再夹带 Kiro 账号额度卡片
- admin 账号状态浏览迁移到独立页面

## Testing

### Backend

- public access 响应仍包含 `accounts` 字段，但内容为空
- 非 admin 调用新状态接口会被拒绝
- admin 状态接口的 `prefix/limit/offset` 工作正确
- 空结果、首页、最后一页、超大 limit 被裁剪等边界正常

### Frontend

- `/kiro-access` 不再渲染 Kiro quota/status 区块
- `/llm-access` 不再渲染 Kiro 状态卡片
- 新 admin 状态页可正常渲染卡片、搜索、翻页、刷新
- `Clear` 能恢复默认查询
- 切换每页条数后回到第一页
- 旧 `Accounts` tab 保留维护入口且能跳转到状态页

### Manual Verification

- admin 登录后可打开新的状态页
- 搜索给定前缀时结果正确收窄
- 大量账号下切页流畅，没有把全部卡片一次性铺满
- 公开页面和公开接口都不再泄露账号状态数据

## Risks

### 1. 维护页和状态页可能再次耦合

如果新状态页继续复用维护页的大状态管理，而不是独立请求自己的分页接口，旧问题会重新出现。

规避方式：

- 状态页使用独立 admin 查询接口
- 维护页只保留维护职责

### 2. 前端只隐藏 UI 但后端仍泄露数据

如果只移除页面渲染，不收口公开接口，任何调用方仍可直接拿到账号状态。

规避方式：

- 公开接口固定返回空 `accounts`

### 3. 搜索和分页仍在前端做

如果新状态页依然先全量取回账号再做前端切片，账号数量继续增长时性能和可用性都会退化。

规避方式：

- 搜索和分页都在后端查询层做

## Implementation Targets

预计会涉及这些位置：

- `backend/src/kiro_gateway/mod.rs`
- `backend/src/kiro_gateway/types.rs`
- `frontend/src/api.rs`
- `frontend/src/router.rs`
- `frontend/src/pages/kiro_access.rs`
- `frontend/src/pages/llm_access.rs`
- `frontend/src/pages/admin_kiro_gateway.rs`
- 新增独立 admin Kiro 账号状态页面文件

## Success Criteria

- 公开页面无法看到任何 Kiro 账号状态
- 公开响应保持结构兼容，不因删除 `accounts` 字段破坏现有调用方
- admin 内存在独立的账号状态页
- 新状态页可以按账号名前缀搜索并分页浏览
- 现有卡片样式和信息保持不变
- 旧 `Accounts` tab 不再承担状态监控职责
