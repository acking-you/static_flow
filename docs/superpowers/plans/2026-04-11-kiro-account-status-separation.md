# Kiro Account Status Separation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove all public Kiro account status exposure, move the existing Kiro account cards into a dedicated admin-only status page with backend pagination and prefix search, and shrink the old `Admin Kiro Gateway -> Accounts` tab back to account-maintenance entry points.

**Architecture:** Keep the public `/api/kiro-gateway/access` contract shape stable by returning an empty `accounts` array, add one admin-only paginated status endpoint for Kiro accounts, extract the existing `KiroAccountCard` into a reusable frontend component, and introduce a dedicated admin route/page that owns status browsing state (`prefix`, `page`, `page_size`). The old admin gateway page will keep account import/create flows but stop rendering the full card wall, linking to the new status page instead.

**Tech Stack:** Rust (Axum backend, Serde, Yew/WASM frontend), existing `Pagination` component, targeted `cargo test`, `cargo clippy`, and per-file `rustfmt`.

---

## File Structure

- Modify: `backend/src/kiro_gateway/types.rs`
  - Add admin account-status query/response DTOs while keeping public `KiroAccessResponse.accounts`
- Modify: `backend/src/kiro_gateway/mod.rs`
  - Return empty public `accounts`, add filtered/paginated admin status handler, and add pure helper tests
- Modify: `backend/src/routes.rs`
  - Register `GET /admin/kiro-gateway/accounts/statuses`
- Modify: `frontend/src/api.rs`
  - Add paginated admin account-status query/response DTOs and fetch helper
- Create: `frontend/src/components/admin_kiro_account_card.rs`
  - Extract the current `KiroAccountCard` as a reusable component without changing its visible behavior
- Modify: `frontend/src/components/mod.rs`
  - Re-export the new account-card component
- Create: `frontend/src/pages/admin_kiro_account_status.rs`
  - Dedicated admin status page with prefix search, page-size selection, refresh, and pagination
- Modify: `frontend/src/pages/mod.rs`
  - Register the new admin status page module
- Modify: `frontend/src/router.rs`
  - Add the new admin status route and switch arm
- Modify: `frontend/src/seo.rs`
  - Register the new admin route path and exclude it from public SEO surfaces
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
  - Use the extracted account-card component where still needed, remove the full card wall from `Accounts`, and add a status-page entry link
- Modify: `frontend/src/pages/kiro_access.rs`
  - Remove public quota/status rendering while keeping access instructions
- Modify: `frontend/src/pages/llm_access.rs`
  - Remove the Kiro public-status fetch/state/render path entirely

---

### Task 1: Add backend account-status query types and lock down public/admin behavior with tests

**Files:**
- Modify: `backend/src/kiro_gateway/types.rs`
- Modify: `backend/src/kiro_gateway/mod.rs`
- Test: `backend/src/kiro_gateway/types.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write failing backend tests for the new query contract and pure filtering helpers**

Add tests to `backend/src/kiro_gateway/types.rs`:

```rust
#[test]
fn admin_kiro_account_status_query_defaults_to_none() {
    let query: AdminKiroAccountStatusesQuery =
        serde_urlencoded::from_str("").expect("parse empty query");

    assert_eq!(query.prefix, None);
    assert_eq!(query.limit, None);
    assert_eq!(query.offset, None);
}
```

Add tests to `backend/src/kiro_gateway/mod.rs`:

```rust
#[test]
fn filter_kiro_account_views_by_prefix_trims_and_matches_case_insensitively() {
    let accounts = vec![
        test_account_view("Alpha"),
        test_account_view("alpha-two"),
        test_account_view("beta"),
    ];

    let filtered = filter_kiro_account_views_by_prefix(&accounts, Some("  ALpHa "));

    assert_eq!(
        filtered.iter().map(|item| item.name.as_str()).collect::<Vec<_>>(),
        vec!["Alpha", "alpha-two"]
    );
}

#[test]
fn paginate_kiro_account_views_returns_total_and_slice() {
    let accounts = vec![
        test_account_view("alpha"),
        test_account_view("beta"),
        test_account_view("gamma"),
    ];

    let page = paginate_kiro_account_views(accounts, 1, 1);

    assert_eq!(page.total, 3);
    assert_eq!(page.offset, 1);
    assert_eq!(page.limit, 1);
    assert_eq!(page.accounts.len(), 1);
    assert_eq!(page.accounts[0].name, "beta");
}

#[test]
fn public_kiro_access_accounts_are_always_empty() {
    let response = KiroAccessResponse {
        base_url: "https://example.com/api/kiro-gateway".to_string(),
        gateway_path: "/api/kiro-gateway".to_string(),
        auth_cache_ttl_seconds: 60,
        accounts: public_kiro_access_accounts(),
        generated_at: 0,
    };

    assert!(response.accounts.is_empty());
}
```

- [ ] **Step 2: Run the focused backend tests and verify they fail before implementation**

Run:

```bash
cargo test -p static-flow-backend admin_kiro_account_status_query_defaults_to_none -- --nocapture
cargo test -p static-flow-backend filter_kiro_account_views_by_prefix_trims_and_matches_case_insensitively -- --nocapture
cargo test -p static-flow-backend paginate_kiro_account_views_returns_total_and_slice -- --nocapture
cargo test -p static-flow-backend public_kiro_access_accounts_are_always_empty -- --nocapture
```

Expected:

- The tests fail to compile because `AdminKiroAccountStatusesQuery`, `filter_kiro_account_views_by_prefix`, `paginate_kiro_account_views`, and `public_kiro_access_accounts` do not exist yet

- [ ] **Step 3: Implement the backend DTOs and pure helpers**

Add the new query/response types to `backend/src/kiro_gateway/types.rs`:

```rust
#[derive(Debug, Default, Deserialize)]
pub struct AdminKiroAccountStatusesQuery {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct AdminKiroAccountStatusesResponse {
    pub accounts: Vec<KiroAccountView>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub generated_at: i64,
}
```

Add pure helpers near `build_account_views` in `backend/src/kiro_gateway/mod.rs`:

```rust
const DEFAULT_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT: usize = 24;
const MAX_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT: usize = 96;

fn public_kiro_access_accounts() -> Vec<KiroPublicStatusView> {
    Vec::new()
}

fn normalize_admin_kiro_account_status_limit(raw: Option<usize>) -> usize {
    raw.unwrap_or(DEFAULT_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT)
        .clamp(1, MAX_ADMIN_KIRO_ACCOUNT_STATUS_LIMIT)
}

fn normalized_kiro_account_status_prefix(raw: Option<&str>) -> Option<String> {
    let trimmed = raw.unwrap_or_default().trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_ascii_lowercase())
    }
}

fn filter_kiro_account_views_by_prefix(
    accounts: &[KiroAccountView],
    prefix: Option<&str>,
) -> Vec<KiroAccountView> {
    let Some(prefix) = normalized_kiro_account_status_prefix(prefix) else {
        return accounts.to_vec();
    };
    accounts
        .iter()
        .filter(|item| item.name.to_ascii_lowercase().starts_with(&prefix))
        .cloned()
        .collect()
}

struct PaginatedKiroAccountViews {
    accounts: Vec<KiroAccountView>,
    total: usize,
    limit: usize,
    offset: usize,
}

fn paginate_kiro_account_views(
    accounts: Vec<KiroAccountView>,
    offset: usize,
    limit: usize,
) -> PaginatedKiroAccountViews {
    let total = accounts.len();
    let page_accounts = accounts.into_iter().skip(offset).take(limit).collect();
    PaginatedKiroAccountViews {
        accounts: page_accounts,
        total,
        limit,
        offset,
    }
}
```

Also change public access response assembly:

```rust
Ok(Json(KiroAccessResponse {
    base_url,
    gateway_path,
    auth_cache_ttl_seconds,
    accounts: public_kiro_access_accounts(),
    generated_at: now_ms(),
}))
```

- [ ] **Step 4: Run the focused backend tests and verify they pass**

Run:

```bash
cargo test -p static-flow-backend admin_kiro_account_status_query_defaults_to_none -- --nocapture
cargo test -p static-flow-backend filter_kiro_account_views_by_prefix_trims_and_matches_case_insensitively -- --nocapture
cargo test -p static-flow-backend paginate_kiro_account_views_returns_total_and_slice -- --nocapture
cargo test -p static-flow-backend public_kiro_access_accounts_are_always_empty -- --nocapture
```

Expected:

- All four tests pass

- [ ] **Step 5: Commit the backend contract groundwork**

Run:

```bash
git add backend/src/kiro_gateway/types.rs backend/src/kiro_gateway/mod.rs
git commit -m "feat: add kiro account status query contract"
```

Expected:

- A commit is created with the new DTOs, pure helpers, and passing tests

---

### Task 2: Add the admin-only paginated status handler and route

**Files:**
- Modify: `backend/src/kiro_gateway/mod.rs`
- Modify: `backend/src/routes.rs`
- Test: `backend/src/kiro_gateway/mod.rs`

- [ ] **Step 1: Write a failing backend test for the paginated admin response shape**

Add this test to `backend/src/kiro_gateway/mod.rs`:

```rust
#[test]
fn paginated_kiro_account_views_preserve_requested_window_metadata() {
    let accounts = vec![
        test_account_view("alpha"),
        test_account_view("beta"),
        test_account_view("gamma"),
        test_account_view("delta"),
    ];

    let filtered = filter_kiro_account_views_by_prefix(&accounts, Some("g"));
    let page = paginate_kiro_account_views(filtered, 0, 24);

    let response = AdminKiroAccountStatusesResponse {
        accounts: page.accounts,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        generated_at: 0,
    };

    assert_eq!(response.total, 1);
    assert_eq!(response.limit, 24);
    assert_eq!(response.offset, 0);
    assert_eq!(response.accounts[0].name, "gamma");
}
```

- [ ] **Step 2: Run the targeted test and verify it fails if the handler path is still missing**

Run:

```bash
cargo test -p static-flow-backend paginated_kiro_account_views_preserve_requested_window_metadata -- --nocapture
```

Expected:

- The test initially fails to compile or fails until the handler code is added

- [ ] **Step 3: Implement the handler and route registration**

Add the new handler in `backend/src/kiro_gateway/mod.rs`:

```rust
pub async fn list_admin_account_statuses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<AdminKiroAccountStatusesQuery>,
) -> Result<Json<AdminKiroAccountStatusesResponse>, (StatusCode, Json<ErrorResponse>)> {
    ensure_admin_access(&state, &headers)?;
    let limit = normalize_admin_kiro_account_status_limit(query.limit);
    let offset = query.offset.unwrap_or(0);
    let accounts = build_account_views(&state).await;
    let filtered = filter_kiro_account_views_by_prefix(&accounts, query.prefix.as_deref());
    let page = paginate_kiro_account_views(filtered, offset, limit);

    Ok(Json(AdminKiroAccountStatusesResponse {
        accounts: page.accounts,
        total: page.total,
        limit: page.limit,
        offset: page.offset,
        generated_at: now_ms(),
    }))
}
```

Register the route in `backend/src/routes.rs` immediately before the existing `/admin/kiro-gateway/accounts` route:

```rust
.route(
    "/admin/kiro-gateway/accounts/statuses",
    get(kiro_gateway::list_admin_account_statuses),
)
.route(
    "/admin/kiro-gateway/accounts",
    get(kiro_gateway::list_admin_accounts).post(kiro_gateway::create_manual_account),
)
```

- [ ] **Step 4: Run the focused backend tests again**

Run:

```bash
cargo test -p static-flow-backend paginated_kiro_account_views_preserve_requested_window_metadata -- --nocapture
cargo test -p static-flow-backend filter_kiro_account_views_by_prefix_trims_and_matches_case_insensitively -- --nocapture
```

Expected:

- The response-shape test and prefix-filter test pass

- [ ] **Step 5: Commit the admin status endpoint**

Run:

```bash
git add backend/src/kiro_gateway/mod.rs backend/src/routes.rs
git commit -m "feat: add admin kiro account status endpoint"
```

Expected:

- A commit is created with the new admin-only endpoint and route

---

### Task 3: Add frontend transport, route, and SEO support for the new admin status page

**Files:**
- Modify: `frontend/src/api.rs`
- Modify: `frontend/src/router.rs`
- Modify: `frontend/src/pages/mod.rs`
- Modify: `frontend/src/seo.rs`
- Test: `frontend/src/api.rs`

- [ ] **Step 1: Write failing frontend tests for the new query-string builder and response defaults**

Add tests to `frontend/src/api.rs`:

```rust
#[test]
fn admin_kiro_account_statuses_response_defaults_are_empty() {
    let response: AdminKiroAccountStatusesResponse =
        serde_json::from_str("{}").expect("response should parse");

    assert!(response.accounts.is_empty());
    assert_eq!(response.total, 0);
    assert_eq!(response.limit, 0);
    assert_eq!(response.offset, 0);
}

#[test]
fn build_admin_kiro_account_statuses_url_encodes_prefix_and_window() {
    let url = build_admin_kiro_account_statuses_url(&AdminKiroAccountStatusesQuery {
        prefix: Some("alpha team".to_string()),
        limit: Some(24),
        offset: Some(48),
    });

    assert!(url.contains("/admin/kiro-gateway/accounts/statuses"));
    assert!(url.contains("prefix=alpha%20team"));
    assert!(url.contains("limit=24"));
    assert!(url.contains("offset=48"));
}
```

- [ ] **Step 2: Run the focused frontend tests and verify they fail before implementation**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_account_statuses_response_defaults_are_empty -- --nocapture
cargo test -p static-flow-frontend build_admin_kiro_account_statuses_url_encodes_prefix_and_window -- --nocapture
```

Expected:

- The tests fail because the new query/response helpers do not exist yet

- [ ] **Step 3: Implement API transport, route enum, and SEO wiring**

Add DTOs and fetch helper to `frontend/src/api.rs`:

```rust
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AdminKiroAccountStatusesResponse {
    pub accounts: Vec<KiroAccountView>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    pub generated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AdminKiroAccountStatusesQuery {
    pub prefix: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

fn build_admin_kiro_account_statuses_url(query: &AdminKiroAccountStatusesQuery) -> String {
    let mut params = vec![];
    if let Some(prefix) = query.prefix.as_deref() {
        params.push(format!("prefix={}", urlencoding::encode(prefix)));
    }
    if let Some(limit) = query.limit {
        params.push(format!("limit={limit}"));
    }
    if let Some(offset) = query.offset {
        params.push(format!("offset={offset}"));
    }
    let suffix = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    format!("{}/admin/kiro-gateway/accounts/statuses{}", admin_base(), suffix)
}

pub async fn fetch_admin_kiro_account_statuses(
    query: &AdminKiroAccountStatusesQuery,
) -> Result<AdminKiroAccountStatusesResponse, String> {
    let url = build_admin_kiro_account_statuses_url(query);
    let response = api_get(&url)
        .send()
        .await
        .map_err(|e| format!("Network error: {:?}", e))?;
    if !response.ok() {
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Failed: {text}"));
    }
    response
        .json()
        .await
        .map_err(|e| format!("Parse error: {:?}", e))
}
```

Add a new route variant in `frontend/src/router.rs`:

```rust
#[at("/admin/kiro-gateway/accounts")]
AdminKiroAccountStatus,
```

and the switch arm:

```rust
Route::AdminKiroAccountStatus => {
    html! { <pages::admin_kiro_account_status::AdminKiroAccountStatusPage /> }
},
```

Register the page module in `frontend/src/pages/mod.rs`:

```rust
pub mod admin_kiro_account_status;
```

Update `frontend/src/seo.rs`:

```rust
Route::AdminKiroAccountStatus => config::route_path("/admin/kiro-gateway/accounts"),
```

and add the new route to the non-indexed admin route match arm:

```rust
| Route::AdminKiroAccountStatus
```

- [ ] **Step 4: Run the focused frontend tests and verify they pass**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_account_statuses_response_defaults_are_empty -- --nocapture
cargo test -p static-flow-frontend build_admin_kiro_account_statuses_url_encodes_prefix_and_window -- --nocapture
```

Expected:

- Both tests pass

- [ ] **Step 5: Commit the frontend transport and route plumbing**

Run:

```bash
git add frontend/src/api.rs frontend/src/router.rs frontend/src/pages/mod.rs frontend/src/seo.rs
git commit -m "feat: add admin kiro account status route"
```

Expected:

- A commit is created with the new route and transport plumbing

---

### Task 4: Extract `KiroAccountCard` into a reusable component and build the new admin status page

**Files:**
- Create: `frontend/src/components/admin_kiro_account_card.rs`
- Modify: `frontend/src/components/mod.rs`
- Create: `frontend/src/pages/admin_kiro_account_status.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_account_status.rs`

- [ ] **Step 1: Write failing page-helper tests for prefix normalization and total-page math**

Add tests to `frontend/src/pages/admin_kiro_account_status.rs`:

```rust
#[test]
fn normalized_admin_kiro_status_prefix_trims_empty_input() {
    assert_eq!(normalized_admin_kiro_status_prefix("   "), None);
    assert_eq!(
        normalized_admin_kiro_status_prefix("  alpha "),
        Some("alpha".to_string())
    );
}

#[test]
fn admin_kiro_status_total_pages_never_drops_below_one() {
    assert_eq!(admin_kiro_status_total_pages(0, 24), 1);
    assert_eq!(admin_kiro_status_total_pages(25, 24), 2);
}
```

- [ ] **Step 2: Run the focused frontend tests and verify they fail before the new page exists**

Run:

```bash
cargo test -p static-flow-frontend normalized_admin_kiro_status_prefix_trims_empty_input -- --nocapture
cargo test -p static-flow-frontend admin_kiro_status_total_pages_never_drops_below_one -- --nocapture
```

Expected:

- The tests fail because `admin_kiro_account_status.rs` and the helper functions do not exist yet

- [ ] **Step 3: Extract the account card and implement the new admin page**

Create `frontend/src/components/admin_kiro_account_card.rs` by moving the existing `KiroAccountCardProps` + `KiroAccountCard` implementation out of `frontend/src/pages/admin_kiro_gateway.rs`, keeping the visual structure and actions unchanged:

```rust
#[derive(Properties, PartialEq)]
pub struct AdminKiroAccountCardProps {
    pub account: KiroAccountView,
    pub proxy_configs: Vec<AdminUpstreamProxyConfigView>,
    pub on_reload: Callback<()>,
    pub flash: UseStateHandle<Option<String>>,
    pub notify: Callback<(String, bool)>,
    pub error: UseStateHandle<Option<String>>,
}

#[function_component(AdminKiroAccountCard)]
pub fn admin_kiro_account_card(props: &AdminKiroAccountCardProps) -> Html {
    let expanded = use_state(|| false);
    let scheduler_max = use_state(|| props.account.kiro_channel_max_concurrency.to_string());
    let scheduler_min = use_state(|| props.account.kiro_channel_min_start_interval_ms.to_string());
    let minimum_remaining_credits_before_block =
        use_state(|| format_float4(props.account.minimum_remaining_credits_before_block));
    let selected_proxy = use_state(|| kiro_account_proxy_select_value(&props.account));
    let feedback = use_state(|| None::<String>);
    let busy = use_state(|| false);

    html! {
        <article class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
            <div class={classes!("flex", "items-start", "justify-between", "gap-3", "flex-wrap")}>
                <div>
                    <span class={kiro_badge()}>{ "Kiro" }</span>
                    <h3 class={classes!("m-0", "text-lg", "font-semibold")}>{ props.account.name.clone() }</h3>
                </div>
            </div>
        </article>
    }
}
```

After creating the file, paste the current `KiroAccountCard` state/effect/callback/html body into this component verbatim, then fix only import paths, the props type name, and the component function name.

Export it from `frontend/src/components/mod.rs`:

```rust
pub mod admin_kiro_account_card;
```

Create `frontend/src/pages/admin_kiro_account_status.rs`:

```rust
use yew::prelude::*;

use crate::{
    api::{
        fetch_admin_kiro_account_statuses, fetch_admin_llm_gateway_proxy_configs,
        AdminKiroAccountStatusesQuery, AdminKiroAccountStatusesResponse,
        AdminUpstreamProxyConfigView,
    },
    components::{admin_kiro_account_card::AdminKiroAccountCard, pagination::Pagination},
    router::Route,
};

const DEFAULT_STATUS_PAGE_SIZE: usize = 24;

fn normalized_admin_kiro_status_prefix(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn admin_kiro_status_total_pages(total: usize, page_size: usize) -> usize {
    total.max(1).div_ceil(page_size.max(1))
}

#[function_component(AdminKiroAccountStatusPage)]
pub fn admin_kiro_account_status_page() -> Html {
    let search_input = use_state(String::new);
    let active_prefix = use_state(|| None::<String>);
    let current_page = use_state(|| 1usize);
    let page_size = use_state(|| DEFAULT_STATUS_PAGE_SIZE);
    let response = use_state(|| None::<AdminKiroAccountStatusesResponse>);
    let proxy_configs = use_state(Vec::<AdminUpstreamProxyConfigView>::new);
    let loading = use_state(|| true);
    let error = use_state(|| None::<String>);
    let flash = use_state(|| None::<String>);
    let notify = Callback::from(|_: (String, bool)| ());

    let reload = {
        let active_prefix = active_prefix.clone();
        let current_page = current_page.clone();
        let page_size = page_size.clone();
        let response = response.clone();
        let loading = loading.clone();
        let error = error.clone();
        Callback::from(move |_| {
            let active_prefix_value = (*active_prefix).clone();
            let current_page_value = *current_page;
            let page_size_value = *page_size;
            let response = response.clone();
            let loading = loading.clone();
            let error = error.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let query = AdminKiroAccountStatusesQuery {
                    prefix: active_prefix_value,
                    limit: Some(page_size_value),
                    offset: Some((current_page_value.saturating_sub(1)) * page_size_value),
                };
                match fetch_admin_kiro_account_statuses(&query).await {
                    Ok(data) => response.set(Some(data)),
                    Err(err) => error.set(Some(err)),
                }
                loading.set(false);
            });
        })
    };

    html! {
        <main class={classes!("container", "py-8", "space-y-5")}>
            <section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
                <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
                    <input value={(*search_input).clone()} />
                    <button type="button" class={classes!("btn-terminal")} onclick={reload.clone()}>{ "Refresh" }</button>
                    <Link<Route> to={Route::AdminKiroGateway} classes={classes!("btn-terminal")}>{ "Back" }</Link<Route>>
                </div>
                <div class={classes!("mt-3", "font-mono", "text-xs", "text-[var(--muted)]")}>
                    {
                        response.as_ref()
                            .as_ref()
                            .map(|value| format!("total {} · page {}", value.total, *current_page))
                            .unwrap_or_else(|| "loading".to_string())
                    }
                </div>
            </section>
            <section class={classes!("grid", "gap-4", "xl:grid-cols-2")}>
                {
                    response.as_ref()
                        .as_ref()
                        .map(|value| html! {
                            for value.accounts.iter().map(|account| html! {
                                <AdminKiroAccountCard
                                    key={account.name.clone()}
                                    account={account.clone()}
                                    proxy_configs={(*proxy_configs).clone()}
                                    on_reload={reload.clone()}
                                    flash={flash.clone()}
                                    notify={notify.clone()}
                                    error={error.clone()}
                                />
                            })
                        })
                        .unwrap_or_default()
                }
            </section>
            <Pagination
                current_page={*current_page}
                total_pages={
                    response.as_ref()
                        .as_ref()
                        .map(|value| admin_kiro_status_total_pages(value.total, value.limit.max(1)))
                        .unwrap_or(1)
                }
                on_page_change={Callback::from(move |_| ())}
            />
        </main>
    }
}
```

Use the extracted component from `frontend/src/pages/admin_kiro_gateway.rs`:

```rust
use crate::components::admin_kiro_account_card::AdminKiroAccountCard;
```

and replace `<KiroAccountCard ... />` with:

```rust
<AdminKiroAccountCard
    key={account.name.clone()}
    account={account.clone()}
    proxy_configs={(*proxy_configs).clone()}
    on_reload={on_reload.clone()}
    flash={flash.clone()}
    notify={notify.clone()}
    error={error.clone()}
/>
```

- [ ] **Step 4: Run the focused frontend tests and a page compile check**

Run:

```bash
cargo test -p static-flow-frontend normalized_admin_kiro_status_prefix_trims_empty_input -- --nocapture
cargo test -p static-flow-frontend admin_kiro_status_total_pages_never_drops_below_one -- --nocapture
cargo test -p static-flow-frontend --lib --no-run
```

Expected:

- The helper tests pass
- The crate compiles with the new page and extracted component

- [ ] **Step 5: Commit the reusable card and admin status page**

Run:

```bash
git add frontend/src/components/admin_kiro_account_card.rs frontend/src/components/mod.rs frontend/src/pages/admin_kiro_account_status.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "feat: add admin kiro account status page"
```

Expected:

- A commit is created with the new page and the extracted reusable card

---

### Task 5: Remove public Kiro account status rendering and shrink the old `Accounts` tab

**Files:**
- Modify: `frontend/src/pages/kiro_access.rs`
- Modify: `frontend/src/pages/llm_access.rs`
- Modify: `frontend/src/pages/admin_kiro_gateway.rs`
- Test: `frontend/src/pages/admin_kiro_gateway.rs`

- [ ] **Step 1: Write failing frontend tests for the new maintenance-page helpers**

Add tests to `frontend/src/pages/admin_kiro_gateway.rs`:

```rust
#[test]
fn admin_kiro_gateway_accounts_tab_shows_status_page_entry_link() {
    assert_eq!(
        kiro_account_status_route(),
        Route::AdminKiroAccountStatus
    );
}

#[test]
fn kiro_account_status_cta_text_is_stable() {
    assert_eq!(kiro_account_status_cta_text(), "Open Account Status Page");
}
```

- [ ] **Step 2: Run the focused frontend tests and verify they fail before the helpers exist**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_gateway_accounts_tab_shows_status_page_entry_link -- --nocapture
cargo test -p static-flow-frontend kiro_account_status_cta_text_is_stable -- --nocapture
```

Expected:

- The tests fail because the route helper and CTA text helper do not exist yet

- [ ] **Step 3: Implement the public-page cleanup and maintenance-page shrink**

In `frontend/src/pages/kiro_access.rs`, remove the quota snapshot section and adjust the refresh button labeling so it no longer says quota:

```rust
title="刷新接入信息"
aria-label="刷新接入信息"
```

Delete the entire block that starts with:

```rust
// ── Quota Cards ──
```

In `frontend/src/pages/llm_access.rs`, remove:

```rust
fetch_kiro_access,
KiroAccessResponse,
let kiro_access = use_state(|| None::<KiroAccessResponse>);
let kiro_loading = use_state(|| true);
let kiro_error = use_state(|| None::<String>);
```

Delete the `use_effect_with` block that calls `fetch_kiro_access()` and remove the Kiro-account render block inside the status section.

In `frontend/src/pages/admin_kiro_gateway.rs`, add tiny helpers near the tab constants:

```rust
fn kiro_account_status_route() -> Route {
    Route::AdminKiroAccountStatus
}

fn kiro_account_status_cta_text() -> &'static str {
    "Open Account Status Page"
}
```

Then replace the old card wall in the `Accounts` tab:

```rust
<section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-5")}>
    <div class={classes!("flex", "items-center", "justify-between", "gap-3", "flex-wrap")}>
        <div>
            <h2 class={classes!("m-0", "font-mono", "text-base", "font-bold", "text-[var(--text)]")}>
                { "Account Status" }
            </h2>
            <p class={classes!("mt-2", "mb-0", "text-sm", "text-[var(--muted)]")}>
                { "状态卡片已迁移到独立 admin 页面，维护入口留在这里。" }
            </p>
        </div>
        <Link<Route> to={kiro_account_status_route()} classes={classes!("btn-terminal", "btn-terminal-primary")}>
            { kiro_account_status_cta_text() }
        </Link<Route>>
    </div>
</section>
```

- [ ] **Step 4: Run the focused frontend tests and check the pages compile**

Run:

```bash
cargo test -p static-flow-frontend admin_kiro_gateway_accounts_tab_shows_status_page_entry_link -- --nocapture
cargo test -p static-flow-frontend kiro_account_status_cta_text_is_stable -- --nocapture
cargo test -p static-flow-frontend --lib --no-run
```

Expected:

- The helper tests pass
- The frontend crate still compiles after removing the Kiro public status flow

- [ ] **Step 5: Commit the public/admin UI split**

Run:

```bash
git add frontend/src/pages/kiro_access.rs frontend/src/pages/llm_access.rs frontend/src/pages/admin_kiro_gateway.rs
git commit -m "feat: separate public and admin kiro status views"
```

Expected:

- A commit is created with the public cleanup and old-tab shrink

---

### Task 6: Run full verification, format touched files, and close the branch cleanly

**Files:**
- Modify: touched files from Tasks 1-5 only

- [ ] **Step 1: Format only the changed Rust files**

Run:

```bash
rustfmt backend/src/kiro_gateway/types.rs \
  backend/src/kiro_gateway/mod.rs \
  backend/src/routes.rs \
  frontend/src/api.rs \
  frontend/src/router.rs \
  frontend/src/pages/mod.rs \
  frontend/src/seo.rs \
  frontend/src/components/admin_kiro_account_card.rs \
  frontend/src/components/mod.rs \
  frontend/src/pages/admin_kiro_account_status.rs \
  frontend/src/pages/admin_kiro_gateway.rs \
  frontend/src/pages/kiro_access.rs \
  frontend/src/pages/llm_access.rs
```

Expected:

- Formatting completes without touching unrelated files or submodules

- [ ] **Step 2: Run the affected backend tests**

Run:

```bash
cargo test -p static-flow-backend
```

Expected:

- Backend tests pass, including the new Kiro admin/public boundary tests

- [ ] **Step 3: Run the affected frontend tests**

Run:

```bash
cargo test -p static-flow-frontend
```

Expected:

- Frontend tests pass, including the new query/page helper tests

- [ ] **Step 4: Run clippy to zero warnings on the affected crates**

Run:

```bash
cargo clippy -p static-flow-backend --all-targets -- -D warnings
cargo clippy -p static-flow-frontend --all-targets -- -D warnings
```

Expected:

- Both commands finish with zero warnings and zero errors

- [ ] **Step 5: Commit the verified final state**

Run:

```bash
git add backend/src/kiro_gateway/types.rs \
  backend/src/kiro_gateway/mod.rs \
  backend/src/routes.rs \
  frontend/src/api.rs \
  frontend/src/router.rs \
  frontend/src/pages/mod.rs \
  frontend/src/seo.rs \
  frontend/src/components/admin_kiro_account_card.rs \
  frontend/src/components/mod.rs \
  frontend/src/pages/admin_kiro_account_status.rs \
  frontend/src/pages/admin_kiro_gateway.rs \
  frontend/src/pages/kiro_access.rs \
  frontend/src/pages/llm_access.rs
git commit -m "feat: move kiro account status into admin"
```

Expected:

- The final implementation is committed after tests, clippy, and formatting all pass

---

## Self-Review Checklist

- Spec coverage:
  - Public `accounts` field retained but emptied: Tasks 1-2
  - New admin status route/page: Tasks 3-4
  - Backend prefix search + pagination: Tasks 1-2
  - Old `Accounts` tab shrunk to maintenance entry points: Task 5
  - Public `kiro-access` and `llm-access` cleanup: Task 5
- Placeholder scan:
  - No `TODO`, `TBD`, or unnamed helper/function placeholders remain
- Type consistency:
  - Route name is `AdminKiroAccountStatus`
  - API query type is `AdminKiroAccountStatusesQuery`
  - API response type is `AdminKiroAccountStatusesResponse`
  - Extracted card component name is `AdminKiroAccountCard`
