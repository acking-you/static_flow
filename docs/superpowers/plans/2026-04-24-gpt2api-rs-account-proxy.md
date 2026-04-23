# gpt2api-rs Account Proxy Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add self-contained reusable upstream proxy configs plus per-account proxy selection to `gpt2api-rs`, then expose and edit them through StaticFlow's stateless `/admin/gpt2api-rs` integration.

**Architecture:** Keep `gpt2api-rs` as the only source of truth. Extend its SQLite control plane with `proxy_configs` plus account-level `proxy_mode` / `proxy_config_id`, resolve the effective proxy on the real upstream client build path, and expose CRUD/check/admin account views directly from `gpt2api-rs`. StaticFlow only forwards those endpoints and renders them in the existing admin page.

**Tech Stack:** Rust, Axum, rusqlite, primp, reqwest, Yew, gloo-net, wiremock, tempfile

---

## File Structure Map

**gpt2api-rs domain and storage**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/models.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/migrations.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/control.rs`
- Create: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/proxy_storage.rs`

**gpt2api-rs runtime resolution**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/upstream/chatgpt.rs`

**gpt2api-rs admin HTTP and CLI client**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/http/admin_api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/app.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/admin_client.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_client.rs`

**StaticFlow stateless forwarding**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/gpt2api_rs.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`

**StaticFlow frontend API and page**
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_gpt2api_rs.rs`

---

### Task 1: Add Proxy Domain Models And SQLite Persistence In gpt2api-rs

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/models.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/migrations.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/control.rs`
- Test: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/proxy_storage.rs`

- [ ] **Step 1: Write the failing storage round-trip test**

Create `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/proxy_storage.rs`:

```rust
use gpt2api_rs::{
    config::ResolvedPaths,
    models::{AccountProxyMode, AccountRecord, ProxyConfigRecord},
    storage::Storage,
};

#[tokio::test]
async fn account_and_proxy_config_round_trip_through_control_db() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage opens");

    let proxy = ProxyConfigRecord {
        id: "proxy-1".to_string(),
        name: "proxy-one".to_string(),
        proxy_url: "http://127.0.0.1:11111".to_string(),
        proxy_username: Some("alice".to_string()),
        proxy_password: Some("secret".to_string()),
        status: "active".to_string(),
        created_at: 100,
        updated_at: 100,
    };
    storage.control.upsert_proxy_config(&proxy).await.expect("proxy saved");

    let mut account = AccountRecord::minimal("acct-1", "token-1");
    account.proxy_mode = AccountProxyMode::Fixed;
    account.proxy_config_id = Some("proxy-1".to_string());
    storage.control.upsert_account(&account).await.expect("account saved");

    let saved_proxy = storage
        .control
        .get_proxy_config("proxy-1")
        .await
        .expect("proxy lookup")
        .expect("proxy row");
    let saved_account = storage
        .control
        .get_account("acct-1")
        .await
        .expect("account lookup")
        .expect("account row");

    assert_eq!(saved_proxy.proxy_username.as_deref(), Some("alice"));
    assert_eq!(saved_account.proxy_mode, AccountProxyMode::Fixed);
    assert_eq!(saved_account.proxy_config_id.as_deref(), Some("proxy-1"));
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test proxy_storage
```

Expected:
- compile fails because `AccountProxyMode`, `ProxyConfigRecord`, and proxy-config storage methods do not exist yet

- [ ] **Step 3: Add the minimal model and storage implementation**

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/models.rs`, add the account proxy mode and reusable proxy record:

```rust
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountProxyMode {
    Inherit,
    Direct,
    Fixed,
}

impl AccountProxyMode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Inherit => "inherit",
            Self::Direct => "direct",
            Self::Fixed => "fixed",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "inherit" => Some(Self::Inherit),
            "direct" => Some(Self::Direct),
            "fixed" => Some(Self::Fixed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProxyConfigRecord {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}
```

Extend `AccountRecord` with:

```rust
pub proxy_mode: AccountProxyMode,
pub proxy_config_id: Option<String>,
```

and update `AccountRecord::minimal(...)` to default to:

```rust
proxy_mode: AccountProxyMode::Inherit,
proxy_config_id: None,
```

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/migrations.rs`, extend the schema:

```sql
CREATE TABLE IF NOT EXISTS proxy_configs (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    proxy_url TEXT NOT NULL,
    proxy_username TEXT,
    proxy_password TEXT,
    status TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
```

and add account migration guards:

```rust
ensure_account_column(
    conn,
    "proxy_mode",
    "ALTER TABLE accounts ADD COLUMN proxy_mode TEXT NOT NULL DEFAULT 'inherit'",
)?;
ensure_account_column(
    conn,
    "proxy_config_id",
    "ALTER TABLE accounts ADD COLUMN proxy_config_id TEXT",
)?;
```

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/storage/control.rs`, update account row mapping and inserts:

```rust
proxy_mode: crate::models::AccountProxyMode::parse(row.get::<_, String>(18)?.as_str())
    .ok_or_else(|| rusqlite::Error::InvalidColumnType(18, "proxy_mode".to_string(), rusqlite::types::Type::Text))?,
proxy_config_id: row.get(19)?,
browser_profile_json: row.get(20)?,
```

and add proxy-config CRUD:

```rust
pub async fn get_proxy_config(&self, proxy_id: &str) -> Result<Option<ProxyConfigRecord>> {
    // SELECT id, name, proxy_url, proxy_username, proxy_password, status,
    //        created_at, updated_at
    // FROM proxy_configs WHERE id = ?1 LIMIT 1
}

pub async fn list_proxy_configs(&self) -> Result<Vec<ProxyConfigRecord>> {
    // SELECT id, name, proxy_url, proxy_username, proxy_password, status,
    //        created_at, updated_at
    // FROM proxy_configs ORDER BY name ASC
}

pub async fn upsert_proxy_config(&self, proxy: &ProxyConfigRecord) -> Result<()> {
    // INSERT INTO proxy_configs (
    //   id, name, proxy_url, proxy_username, proxy_password, status, created_at, updated_at
    // ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
    // ON CONFLICT(id) DO UPDATE SET
    //   name = excluded.name,
    //   proxy_url = excluded.proxy_url,
    //   proxy_username = excluded.proxy_username,
    //   proxy_password = excluded.proxy_password,
    //   status = excluded.status,
    //   created_at = excluded.created_at,
    //   updated_at = excluded.updated_at
}

pub async fn delete_proxy_config(&self, proxy_id: &str) -> Result<bool> {
    // DELETE FROM proxy_configs WHERE id = ?1
}

pub async fn count_accounts_bound_to_proxy_config(&self, proxy_id: &str) -> Result<u64> {
    // SELECT COUNT(*) FROM accounts WHERE proxy_config_id = ?1
}
```

- [ ] **Step 4: Re-run the targeted storage test**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test proxy_storage
```

Expected:
- test passes

- [ ] **Step 5: Commit**

```bash
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs add \
  src/models.rs \
  src/storage/migrations.rs \
  src/storage/control.rs \
  tests/proxy_storage.rs
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs commit -m "feat: persist gpt2api-rs account proxy config state"
```

### Task 2: Resolve Effective Proxies On The Real Upstream Client Path

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/upstream/chatgpt.rs`
- Test: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`

- [ ] **Step 1: Write the failing runtime resolution tests**

Add to the existing test module at the bottom of `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`:

```rust
#[tokio::test]
async fn resolve_account_proxy_prefers_fixed_direct_and_inherit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = ResolvedPaths::new(temp.path().to_path_buf());
    let storage = Storage::open(&paths).await.expect("storage");
    let service = AppService::new(
        storage.clone(),
        "admin".to_string(),
        ChatgptUpstreamClient::new("http://127.0.0.1:9", Some("http://global:11111".to_string())),
    )
    .await
    .expect("service");

    storage
        .control
        .upsert_proxy_config(&crate::models::ProxyConfigRecord {
            id: "proxy-a".to_string(),
            name: "proxy-a".to_string(),
            proxy_url: "http://fixed:22222".to_string(),
            proxy_username: Some("bob".to_string()),
            proxy_password: Some("pw".to_string()),
            status: "active".to_string(),
            created_at: 1,
            updated_at: 1,
        })
        .await
        .expect("seed proxy");

    let inherit = AccountRecord::minimal("inherit", "tok-inherit");

    let mut direct = AccountRecord::minimal("direct", "tok-direct");
    direct.proxy_mode = crate::models::AccountProxyMode::Direct;

    let mut fixed = AccountRecord::minimal("fixed", "tok-fixed");
    fixed.proxy_mode = crate::models::AccountProxyMode::Fixed;
    fixed.proxy_config_id = Some("proxy-a".to_string());

    let inherit_resolved = service.resolve_account_proxy(&inherit).await.expect("inherit");
    let direct_resolved = service.resolve_account_proxy(&direct).await.expect("direct");
    let fixed_resolved = service.resolve_account_proxy(&fixed).await.expect("fixed");

    assert_eq!(inherit_resolved.proxy_url.as_deref(), Some("http://global:11111"));
    assert_eq!(direct_resolved.proxy_url, None);
    assert_eq!(fixed_resolved.proxy_url.as_deref(), Some("http://fixed:22222"));
    assert_eq!(fixed_resolved.proxy_config_name.as_deref(), Some("proxy-a"));
}
```

- [ ] **Step 2: Run the targeted test and confirm the missing helper failure**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml resolve_account_proxy_prefers_fixed_direct_and_inherit -- --nocapture
```

Expected:
- compile fails because `resolve_account_proxy` and resolved proxy types do not exist yet

- [ ] **Step 3: Implement effective proxy resolution and client construction**

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`, add a resolved proxy view:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedAccountProxy {
    pub source: &'static str,
    pub proxy_url: Option<String>,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub proxy_config_id: Option<String>,
    pub proxy_config_name: Option<String>,
}
```

Add the helper on `AppService`:

```rust
async fn resolve_account_proxy(&self, account: &AccountRecord) -> Result<ResolvedAccountProxy> {
    match account.proxy_mode {
        crate::models::AccountProxyMode::Direct => Ok(ResolvedAccountProxy {
            source: "direct",
            proxy_url: None,
            proxy_username: None,
            proxy_password: None,
            proxy_config_id: None,
            proxy_config_name: None,
        }),
        crate::models::AccountProxyMode::Inherit => Ok(ResolvedAccountProxy {
            source: "inherit",
            proxy_url: self.upstream.default_proxy_url(),
            proxy_username: None,
            proxy_password: None,
            proxy_config_id: None,
            proxy_config_name: None,
        }),
        crate::models::AccountProxyMode::Fixed => {
            let proxy_id = account
                .proxy_config_id
                .as_deref()
                .ok_or_else(|| anyhow!("proxy_config_id is required when proxy_mode=`fixed`"))?;
            let proxy = self
                .storage
                .control
                .get_proxy_config(proxy_id)
                .await?
                .ok_or_else(|| anyhow!("proxy config `{proxy_id}` not found"))?;
            if proxy.status != "active" {
                bail!("proxy config `{proxy_id}` is not active");
            }
            Ok(ResolvedAccountProxy {
                source: "fixed",
                proxy_url: Some(proxy.proxy_url.clone()),
                proxy_username: proxy.proxy_username.clone(),
                proxy_password: proxy.proxy_password.clone(),
                proxy_config_id: Some(proxy.id.clone()),
                proxy_config_name: Some(proxy.name.clone()),
            })
        },
    }
}
```

Thread this resolved proxy into every upstream call path in `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`:

```rust
let resolved_proxy = self.resolve_account_proxy(&account).await?;
let result = self
    .upstream
    .complete_text(&account, prompt, requested_model, &resolved_proxy)
    .await?;
```

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/upstream/chatgpt.rs`, keep the global proxy as the inherit default but add a request-level proxy override:

```rust
pub fn default_proxy_url(&self) -> Option<String> {
    self.proxy_url.clone()
}

fn build_client(
    &self,
    profile: &BrowserProfile,
    resolved_proxy: &crate::service::ResolvedAccountProxy,
) -> Result<Client> {
    let mut builder = Client::builder()
        .impersonate(resolve_impersonate(profile))
        .impersonate_os(ImpersonateOS::Windows)
        .cookie_store(true)
        .redirect(primp::redirect::Policy::none())
        .timeout(Duration::from_secs(180))
        .connect_timeout(Duration::from_secs(30))
        .user_agent(profile.user_agent.as_deref().unwrap_or(DEFAULT_USER_AGENT));

    if let Some(proxy_url) = render_proxy_url(
        resolved_proxy.proxy_url.as_deref(),
        resolved_proxy.proxy_username.as_deref(),
        resolved_proxy.proxy_password.as_deref(),
    )? {
        builder = builder.proxy(Proxy::all(proxy_url).context("invalid upstream proxy URL")?);
    }

    builder.build().context("build upstream client failed")
}
```

Add a helper that injects username/password into the URL before building the proxy:

```rust
fn render_proxy_url(
    proxy_url: Option<&str>,
    proxy_username: Option<&str>,
    proxy_password: Option<&str>,
) -> Result<Option<String>> {
    let Some(proxy_url) = proxy_url.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let mut url = reqwest::Url::parse(proxy_url).context("invalid upstream proxy URL")?;
    if let Some(username) = proxy_username.filter(|value| !value.trim().is_empty()) {
        url.set_username(username)
            .map_err(|_| anyhow::anyhow!("invalid proxy username"))?;
    }
    if let Some(password) = proxy_password.filter(|value| !value.trim().is_empty()) {
        url.set_password(Some(password))
            .map_err(|_| anyhow::anyhow!("invalid proxy password"))?;
    }
    Ok(Some(url.to_string()))
}
```

- [ ] **Step 4: Re-run the resolution test**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml resolve_account_proxy_prefers_fixed_direct_and_inherit -- --nocapture
```

Expected:
- test passes

- [ ] **Step 5: Commit**

```bash
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs add \
  src/service.rs \
  src/upstream/chatgpt.rs
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs commit -m "feat: resolve gpt2api-rs account proxies at runtime"
```

### Task 3: Expose Proxy Config CRUD, Checks, And Account Fields From gpt2api-rs

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/http/admin_api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/app.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/admin_client.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_api.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_client.rs`

- [ ] **Step 1: Extend admin API tests with failing proxy-config coverage**

Append to `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_api.rs`:

```rust
#[tokio::test]
async fn admin_proxy_config_lifecycle_supports_create_patch_check_and_delete() {
    let (_temp, app) = build_test_app("secret").await;

    let create = send_json(
        app.clone(),
        Method::POST,
        "/admin/proxy-configs",
        "secret",
        json!({
            "name": "proxy-one",
            "proxy_url": "http://127.0.0.1:11111",
            "proxy_username": "alice",
            "proxy_password": "pw",
            "status": "active"
        }),
    )
    .await;
    assert_eq!(create.status(), StatusCode::OK);
}
```

Append to `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/tests/admin_client.rs`:

```rust
#[tokio::test]
async fn list_proxy_configs_calls_admin_rest() {
    let server = MockServer::start().await;
    let payload = vec![gpt2api_rs::models::ProxyConfigRecord {
        id: "proxy-1".to_string(),
        name: "proxy-1".to_string(),
        proxy_url: "http://127.0.0.1:11111".to_string(),
        proxy_username: None,
        proxy_password: None,
        status: "active".to_string(),
        created_at: 1,
        updated_at: 1,
    }];

    Mock::given(method("GET"))
        .and(path("/admin/proxy-configs"))
        .and(header("authorization", "Bearer secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&payload))
        .mount(&server)
        .await;

    let items = gpt2api_rs::admin_client::list_proxy_configs(&server.uri(), "secret")
        .await
        .expect("proxy configs response");

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, "proxy-1");
}
```

- [ ] **Step 2: Run the failing admin tests**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test admin_api admin_proxy_config_lifecycle_supports_create_patch_check_and_delete -- --nocapture
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test admin_client list_proxy_configs_calls_admin_rest -- --nocapture
```

Expected:
- tests fail because routes, handlers, and admin-client methods do not exist yet

- [ ] **Step 3: Implement service methods, routes, and HTTP handlers**

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/service.rs`, add proxy-config service methods:

```rust
pub async fn list_proxy_configs(&self) -> Result<Vec<ProxyConfigRecord>> {
    self.storage.control.list_proxy_configs().await
}

pub async fn create_proxy_config(&self, input: &ProxyConfigCreate) -> Result<ProxyConfigRecord> {
    let now = unix_timestamp_secs();
    let record = ProxyConfigRecord {
        id: format!("proxy_{}", Uuid::new_v4().simple()),
        name: normalize_required_string(&input.name, "name")?,
        proxy_url: normalize_required_string(&input.proxy_url, "proxy_url")?,
        proxy_username: normalize_optional_string(input.proxy_username.as_deref()),
        proxy_password: normalize_optional_string(input.proxy_password.as_deref()),
        status: normalize_proxy_status(input.status.as_deref().unwrap_or("active"))?,
        created_at: now,
        updated_at: now,
    };
    self.storage.control.upsert_proxy_config(&record).await?;
    Ok(record)
}
```

Also add:

```rust
pub async fn update_proxy_config(
    &self,
    proxy_id: &str,
    update: &ProxyConfigUpdate,
) -> Result<Option<ProxyConfigRecord>> {
    let Some(mut record) = self.storage.control.get_proxy_config(proxy_id).await? else {
        return Ok(None);
    };
    if let Some(name) = update.name.as_deref() {
        record.name = normalize_required_string(name, "name")?;
    }
    if let Some(proxy_url) = update.proxy_url.as_deref() {
        record.proxy_url = normalize_required_string(proxy_url, "proxy_url")?;
    }
    if update.proxy_username.is_some() {
        record.proxy_username = normalize_optional_string(update.proxy_username.as_deref());
    }
    if update.proxy_password.is_some() {
        record.proxy_password = normalize_optional_string(update.proxy_password.as_deref());
    }
    if let Some(status) = update.status.as_deref() {
        record.status = normalize_proxy_status(status)?;
    }
    record.updated_at = unix_timestamp_secs();
    self.storage.control.upsert_proxy_config(&record).await?;
    Ok(Some(record))
}

pub async fn delete_proxy_config(&self, proxy_id: &str) -> Result<bool> {
    if self.storage.control.count_accounts_bound_to_proxy_config(proxy_id).await? > 0 {
        bail!("proxy config `{proxy_id}` is still bound to one or more accounts");
    }
    self.storage.control.delete_proxy_config(proxy_id).await
}

pub async fn check_proxy_config(&self, proxy_id: &str) -> Result<ProxyConfigCheckResult> {
    let proxy = self
        .storage
        .control
        .get_proxy_config(proxy_id)
        .await?
        .ok_or_else(|| anyhow!("proxy config `{proxy_id}` not found"))?;
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy.proxy_url)?)
        .timeout(Duration::from_secs(15))
        .build()?;
    let response = client.get(self.upstream.base_url()).send().await?;
    Ok(ProxyConfigCheckResult {
        ok: response.status().is_success(),
        status_code: Some(response.status().as_u16()),
        message: format!("HTTP {}", response.status()),
    })
}
```

Extend `AccountUpdate` with:

```rust
pub proxy_mode: Option<String>,
pub proxy_config_id: Option<Option<String>>,
```

and validate it in `update_account(...)`:

```rust
if update.proxy_mode.is_some() || update.proxy_config_id.is_some() {
    let proxy_mode = update
        .proxy_mode
        .as_deref()
        .and_then(crate::models::AccountProxyMode::parse)
        .ok_or_else(|| anyhow!("unsupported proxy_mode"))?;
    let proxy_config_id = update
        .proxy_config_id
        .clone()
        .flatten()
        .and_then(|value| normalize_optional_string(Some(value.as_str())));
    if proxy_mode == crate::models::AccountProxyMode::Fixed {
        let proxy_id = proxy_config_id
            .as_deref()
            .ok_or_else(|| anyhow!("proxy_config_id is required when proxy_mode=`fixed`"))?;
        let proxy = self
            .storage
            .control
            .get_proxy_config(proxy_id)
            .await?
            .ok_or_else(|| anyhow!("proxy config `{proxy_id}` not found"))?;
        if proxy.status != "active" {
            bail!("proxy config `{proxy_id}` is not active");
        }
    }
    account.proxy_mode = proxy_mode;
    account.proxy_config_id = if proxy_mode == crate::models::AccountProxyMode::Fixed {
        proxy_config_id
    } else {
        None
    };
}
```

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/http/admin_api.rs`, add request/response shapes:

```rust
#[derive(Debug, Deserialize)]
pub struct CreateProxyConfigRequest {
    name: String,
    proxy_url: String,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpdateProxyConfigRequest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    proxy_username: Option<String>,
    #[serde(default)]
    proxy_password: Option<String>,
    #[serde(default)]
    status: Option<String>,
}
```

and handlers:

```rust
pub async fn list_proxy_configs(
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
) -> Result<Json<Vec<ProxyConfigRecord>>, AppError> {
    require_admin(&headers, &service)?;
    Ok(Json(service.list_proxy_configs().await.map_err(AppError::internal)?))
}

pub async fn create_proxy_config(
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
    Json(body): Json<CreateProxyConfigRequest>,
) -> Result<Json<ProxyConfigRecord>, AppError> {
    require_admin(&headers, &service)?;
    let record = service
        .create_proxy_config(&ProxyConfigCreate {
            name: body.name,
            proxy_url: body.proxy_url,
            proxy_username: body.proxy_username,
            proxy_password: body.proxy_password,
            status: body.status,
        })
        .await
        .map_err(AppError::internal)?;
    Ok(Json(record))
}

pub async fn update_proxy_config(
    Path(proxy_id): Path<String>,
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
    Json(body): Json<UpdateProxyConfigRequest>,
) -> Result<Json<ProxyConfigRecord>, AppError> {
    require_admin(&headers, &service)?;
    let Some(record) = service
        .update_proxy_config(
            &proxy_id,
            &ProxyConfigUpdate {
                name: body.name,
                proxy_url: body.proxy_url,
                proxy_username: body.proxy_username,
                proxy_password: body.proxy_password,
                status: body.status,
            },
        )
        .await
        .map_err(AppError::internal)?
    else {
        return Err(AppError::not_found("proxy config not found"));
    };
    Ok(Json(record))
}

pub async fn delete_proxy_config(
    Path(proxy_id): Path<String>,
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    require_admin(&headers, &service)?;
    let deleted = service
        .delete_proxy_config(&proxy_id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(json!({ "deleted": deleted, "id": proxy_id })))
}

pub async fn check_proxy_config(
    Path(proxy_id): Path<String>,
    State(service): State<Arc<AppService>>,
    headers: HeaderMap,
) -> Result<Json<Value>, AppError> {
    require_admin(&headers, &service)?;
    let result = service
        .check_proxy_config(&proxy_id)
        .await
        .map_err(AppError::internal)?;
    Ok(Json(json!({
        "ok": result.ok,
        "message": result.message,
        "status_code": result.status_code,
    })))
}
```

Add routes in `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/app.rs`:

```rust
.route("/admin/proxy-configs", get(admin_api::list_proxy_configs).post(admin_api::create_proxy_config))
.route(
    "/admin/proxy-configs/:proxy_id",
    patch(admin_api::update_proxy_config).delete(admin_api::delete_proxy_config),
)
.route("/admin/proxy-configs/:proxy_id/check", post(admin_api::check_proxy_config))
```

In `/home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/src/admin_client.rs`, add:

```rust
pub async fn list_proxy_configs(base_url: &str, admin_token: &str) -> Result<Vec<ProxyConfigRecord>> {
    get_json(base_url, admin_token, "/admin/proxy-configs").await
}
```

- [ ] **Step 4: Re-run the admin API and admin-client tests**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test admin_api admin_proxy_config_lifecycle_supports_create_patch_check_and_delete -- --nocapture
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --test admin_client list_proxy_configs_calls_admin_rest -- --nocapture
```

Expected:
- both tests pass

- [ ] **Step 5: Commit**

```bash
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs add \
  src/http/admin_api.rs \
  src/app.rs \
  src/service.rs \
  src/admin_client.rs \
  tests/admin_api.rs \
  tests/admin_client.rs
git -C /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs commit -m "feat: expose gpt2api-rs proxy config admin APIs"
```

### Task 4: Wire The New Admin Surface Through StaticFlow Backend And Frontend API

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/gpt2api_rs.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`

- [ ] **Step 1: Add the failing type and forwarding references**

In `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`, define the new frontend view types and request structs:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsProxyConfigView {
    pub id: String,
    pub name: String,
    pub proxy_url: String,
    pub proxy_username: Option<String>,
    pub proxy_password: Option<String>,
    pub status: String,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminGpt2ApiRsProxyCheckResponse {
    pub ok: bool,
    pub message: String,
    pub status_code: Option<u16>,
}
```

Extend `AdminGpt2ApiRsAccountView` with:

```rust
pub proxy_mode: String,
pub proxy_config_id: Option<String>,
pub effective_proxy_source: String,
pub effective_proxy_url: Option<String>,
pub effective_proxy_config_name: Option<String>,
```

Extend `AdminGpt2ApiRsUpdateAccountRequest` with:

```rust
#[serde(default)]
pub proxy_mode: Option<String>,
#[serde(default)]
pub proxy_config_id: Option<String>,
```

- [ ] **Step 2: Run compile checks to verify the missing API helpers**

Run:

```bash
cargo check -p static-flow-backend
cargo check -p static-flow-frontend
```

Expected:
- frontend still compiles before calling the new helpers
- once page code in Task 5 starts using proxy-config APIs, compile will fail until those helpers and backend routes exist

- [ ] **Step 3: Add thin backend forwarding and frontend API helpers**

In `/home/ts_user/rust_pro/static_flow/backend/src/gpt2api_rs.rs`, add handlers mirroring the existing key/account forwarding style:

```rust
pub async fn list_admin_proxy_configs(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::GET,
        "/admin/proxy-configs",
        None,
        None,
    )
    .await
}
```

Also add:

```rust
pub async fn create_admin_proxy_config(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        "/admin/proxy-configs",
        None,
        Some(request),
    )
    .await
}

pub async fn update_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::PATCH,
        &format!("/admin/proxy-configs/{proxy_id}"),
        None,
        Some(request),
    )
    .await
}

pub async fn delete_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::DELETE,
        &format!("/admin/proxy-configs/{proxy_id}"),
        None,
        None,
    )
    .await
}

pub async fn check_admin_proxy_config(
    AxumPath(proxy_id): AxumPath<String>,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> HandlerResult<Json<Value>> {
    ensure_admin_access(&state, &headers)?;
    proxy_json_request(
        state.gpt2api_rs.as_ref(),
        TokenScope::Admin,
        Method::POST,
        &format!("/admin/proxy-configs/{proxy_id}/check"),
        None,
        Some(serde_json::json!({})),
    )
    .await
}
```

Register them in `/home/ts_user/rust_pro/static_flow/backend/src/routes.rs`:

```rust
.route(
    "/admin/gpt2api-rs/proxy-configs",
    get(gpt2api_rs::list_admin_proxy_configs).post(gpt2api_rs::create_admin_proxy_config),
)
.route(
    "/admin/gpt2api-rs/proxy-configs/:proxy_id",
    patch(gpt2api_rs::update_admin_proxy_config).delete(gpt2api_rs::delete_admin_proxy_config),
)
.route(
    "/admin/gpt2api-rs/proxy-configs/:proxy_id/check",
    post(gpt2api_rs::check_admin_proxy_config),
)
```

In `/home/ts_user/rust_pro/static_flow/frontend/src/api.rs`, add helpers:

```rust
pub async fn fetch_admin_gpt2api_rs_proxy_configs() -> Result<Vec<AdminGpt2ApiRsProxyConfigView>, String> {
    #[cfg(feature = "mock")]
    {
        Ok(Vec::new())
    }

    #[cfg(not(feature = "mock"))]
    {
        get_admin_gpt2api_rs("/proxy-configs").await
    }
}
```

and:

```rust
pub async fn create_admin_gpt2api_rs_proxy_config(
    request: &AdminGpt2ApiRsCreateProxyConfigRequest,
) -> Result<AdminGpt2ApiRsProxyConfigView, String> {
    post_admin_gpt2api_rs("/proxy-configs", request).await
}

pub async fn patch_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
    request: &AdminGpt2ApiRsPatchProxyConfigRequest,
) -> Result<AdminGpt2ApiRsProxyConfigView, String> {
    patch_admin_gpt2api_rs(&format!("/proxy-configs/{proxy_id}"), request).await
}

pub async fn delete_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
) -> Result<serde_json::Value, String> {
    delete_admin_gpt2api_rs_empty(&format!("/proxy-configs/{proxy_id}")).await
}

pub async fn check_admin_gpt2api_rs_proxy_config(
    proxy_id: &str,
) -> Result<AdminGpt2ApiRsProxyCheckResponse, String> {
    post_admin_gpt2api_rs(&format!("/proxy-configs/{proxy_id}/check"), &serde_json::json!({})).await
}
```

- [ ] **Step 4: Re-run backend and frontend compile checks**

Run:

```bash
cargo check -p static-flow-backend
cargo check -p static-flow-frontend
```

Expected:
- both packages compile with the new forwarding and API helper surface

- [ ] **Step 5: Commit**

```bash
git -C /home/ts_user/rust_pro/static_flow add \
  /home/ts_user/rust_pro/static_flow/backend/src/gpt2api_rs.rs \
  /home/ts_user/rust_pro/static_flow/backend/src/routes.rs \
  /home/ts_user/rust_pro/static_flow/frontend/src/api.rs
git -C /home/ts_user/rust_pro/static_flow commit -m "feat: forward gpt2api-rs proxy admin APIs"
```

### Task 5: Add Proxy Config Management And Account Proxy Selection To The Admin Page

**Files:**
- Modify: `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_gpt2api_rs.rs`

- [ ] **Step 1: Add the failing page state and callback references**

In `/home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_gpt2api_rs.rs`, update the imports:

```rust
use crate::api::{
    check_admin_gpt2api_rs_proxy_config, create_admin_gpt2api_rs_proxy_config,
    delete_admin_gpt2api_rs_proxy_config, fetch_admin_gpt2api_rs_proxy_configs,
    patch_admin_gpt2api_rs_proxy_config, AdminGpt2ApiRsProxyConfigView,
};
```

Add local helpers near the top of the file:

```rust
fn gpt2api_account_proxy_select_value(account: &AdminGpt2ApiRsAccountView) -> String {
    match account.proxy_mode.as_str() {
        "direct" => "direct".to_string(),
        "fixed" => account
            .proxy_config_id
            .as_ref()
            .map(|id| format!("fixed:{id}"))
            .unwrap_or_else(|| "inherit".to_string()),
        _ => "inherit".to_string(),
    }
}
```

Add page state:

```rust
let proxy_configs = use_state(Vec::<AdminGpt2ApiRsProxyConfigView>::new);
let account_proxy_inputs = use_state(std::collections::BTreeMap::<String, String>::new);
let creating_proxy = use_state(|| false);
let proxy_form_name = use_state(String::new);
let proxy_form_url = use_state(String::new);
let proxy_form_username = use_state(String::new);
let proxy_form_password = use_state(String::new);
```

- [ ] **Step 2: Run the frontend compile check to verify missing render/callback code**

Run:

```bash
cargo check -p static-flow-frontend
```

Expected:
- compile fails because the new state is not yet loaded or rendered completely

- [ ] **Step 3: Load proxy configs, save account proxy settings, and render the UI**

Update `reload_all` so it also fetches proxy configs and seeds account selector state:

```rust
match fetch_admin_gpt2api_rs_proxy_configs().await {
    Ok(value) => {
        let mut next_proxy_inputs = std::collections::BTreeMap::new();
        for account in (*accounts).iter() {
            next_proxy_inputs.insert(account.name.clone(), gpt2api_account_proxy_select_value(account));
        }
        proxy_configs.set(value);
        account_proxy_inputs.set(next_proxy_inputs);
    }
    Err(err) => load_error.set(Some(err)),
}
```

When building `AdminGpt2ApiRsUpdateAccountRequest`, include proxy settings:

```rust
let selection = (*account_proxy_inputs)
    .get(&account.name)
    .cloned()
    .unwrap_or_else(|| "inherit".to_string());
let (proxy_mode, proxy_config_id) = if selection == "direct" {
    (Some("direct".to_string()), None)
} else if let Some(proxy_id) = selection.strip_prefix("fixed:") {
    (Some("fixed".to_string()), Some(proxy_id.to_string()))
} else {
    (Some("inherit".to_string()), None)
};
```

and send them:

```rust
let request = AdminGpt2ApiRsUpdateAccountRequest {
    access_token,
    plan_type: None,
    status: None,
    quota_remaining: None,
    restore_at: None,
    session_token: None,
    user_agent: None,
    impersonate_browser: None,
    request_max_concurrency: Some(request_max_concurrency),
    request_min_start_interval_ms: Some(request_min_start_interval_ms),
    proxy_mode,
    proxy_config_id,
};
```

Render the proxy selector beside the existing scheduler controls:

```rust
<select
    class={classes!("rounded-lg", "border", "border-[var(--border)]", "bg-[var(--surface)]", "px-3", "py-2", "text-xs")}
    value={selected_proxy_value.clone()}
    onchange={{
        let account_proxy_inputs = account_proxy_inputs.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<web_sys::HtmlSelectElement>() {
                let mut next = (*account_proxy_inputs).clone();
                next.insert(account_name.clone(), target.value());
                account_proxy_inputs.set(next);
            }
        })
    }}
>
    <option value="inherit">{ "继承全局代理" }</option>
    <option value="direct">{ "Direct / 不走代理" }</option>
    { for proxy_configs.iter().map(|proxy_config| {
        let option_value = format!("fixed:{}", proxy_config.id);
        html! {
            <option value={option_value.clone()}>
                { format!("固定到 {} · {}", proxy_config.name, proxy_config.proxy_url) }
            </option>
        }
    }) }
</select>
```

Render a simple proxy config management card above the accounts list:

```rust
<section class={classes!("rounded-xl", "border", "border-[var(--border)]", "bg-[var(--surface)]", "p-4")}>
    <h3 class={classes!("m-0", "text-base", "font-semibold")}>{ "Proxy Configs" }</h3>
    <div class={classes!("mt-4", "grid", "gap-3", "md:grid-cols-2")}>
        <input value={(*proxy_form_name).clone()} placeholder="proxy name" />
        <input value={(*proxy_form_url).clone()} placeholder="http://127.0.0.1:11111" />
        <input value={(*proxy_form_username).clone()} placeholder="username" />
        <input value={(*proxy_form_password).clone()} placeholder="password" />
    </div>
    <button class={classes!("btn-terminal", "btn-terminal-primary")} onclick={on_create_proxy_config}>
        { if *creating_proxy { "创建中..." } else { "新增代理配置" } }
    </button>
    { for proxy_configs.iter().map(|proxy_config| html! {
        <article key={proxy_config.id.clone()} class={classes!("mt-3", "rounded-lg", "border", "border-[var(--border)]", "p-3")}>
            <div class={classes!("font-mono", "text-xs")}>{ format!("{} · {}", proxy_config.name, proxy_config.proxy_url) }</div>
            <div class={classes!("mt-2", "flex", "gap-2")}>
                <button class={classes!("btn-terminal")} onclick={Callback::from(move |_| on_check_proxy.emit(proxy_config.id.clone()))}>{ "检查" }</button>
                <button class={classes!("btn-terminal")} onclick={Callback::from(move |_| on_delete_proxy.emit(proxy_config.id.clone()))}>{ "删除" }</button>
            </div>
        </article>
    }) }
</section>
```

- [ ] **Step 4: Re-run the frontend compile check**

Run:

```bash
cargo check -p static-flow-frontend
```

Expected:
- the page compiles with proxy config management and account proxy selection wired up

- [ ] **Step 5: Commit**

```bash
git -C /home/ts_user/rust_pro/static_flow add \
  /home/ts_user/rust_pro/static_flow/frontend/src/pages/admin_gpt2api_rs.rs
git -C /home/ts_user/rust_pro/static_flow commit -m "feat: add gpt2api-rs proxy admin UI"
```

### Task 6: Full Verification, Formatting, And Final Smoke Pass

**Files:**
- Modify only as needed based on verification failures

- [ ] **Step 1: Format only the touched Rust files**

Run:

```bash
cd /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs && \
rustfmt src/models.rs src/storage/migrations.rs src/storage/control.rs src/service.rs src/upstream/chatgpt.rs src/http/admin_api.rs src/app.rs src/admin_client.rs tests/proxy_storage.rs tests/admin_api.rs tests/admin_client.rs

cd /home/ts_user/rust_pro/static_flow && \
rustfmt backend/src/gpt2api_rs.rs backend/src/routes.rs frontend/src/api.rs frontend/src/pages/admin_gpt2api_rs.rs
```

Expected:
- formatting completes without touching unrelated workspace files

- [ ] **Step 2: Run the full gpt2api-rs test suite**

Run:

```bash
cargo test --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml
```

Expected:
- all `gpt2api-rs` tests pass, including the new proxy-storage and admin coverage

- [ ] **Step 3: Run required clippy for the affected crates**

Run:

```bash
cargo clippy --manifest-path /home/ts_user/rust_pro/static_flow/deps/gpt2api_rs/Cargo.toml --all-targets --all-features
cargo clippy -p static-flow-backend -p static-flow-frontend --all-targets
```

Expected:
- zero warnings and zero errors

- [ ] **Step 4: Run final compile checks for the StaticFlow integration path**

Run:

```bash
cargo check -p static-flow-backend
cargo check -p static-flow-frontend
```

Expected:
- both packages compile after the new gpt2api-rs proxy surface is wired through

- [ ] **Step 5: Manual smoke test through the admin path**

Run against a local `gpt2api-rs` instance after starting both services:

```bash
curl -sS -H "Authorization: Bearer $ADMIN_TOKEN" http://127.0.0.1:8787/admin/proxy-configs

curl -sS -X POST \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:8787/admin/proxy-configs \
  -d '{"name":"proxy-one","proxy_url":"http://127.0.0.1:11111","status":"active"}'

curl -sS -X POST \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  http://127.0.0.1:8787/admin/accounts/update \
  -d '{"access_token":"<token>","proxy_mode":"fixed","proxy_config_id":"<proxy-id>"}'
```

Expected:
- proxy configs can be listed and created
- account update accepts the fixed binding
- account list shows `effective_proxy_source`, `effective_proxy_url`, and `effective_proxy_config_name`

---

## Self-Review

**Spec coverage**
- self-contained `gpt2api-rs` proxy config storage: Task 1
- account-level `inherit` / `direct` / `fixed`: Tasks 1 and 2
- effective runtime resolution on the real client path: Task 2
- proxy-config CRUD and check endpoints: Task 3
- StaticFlow stateless forwarding only: Task 4
- admin page proxy config UI plus account selector: Task 5
- formatting, tests, clippy, smoke verification: Task 6

**Placeholder scan**
- no red-flag placeholders remain in task steps or code snippets
- every task has concrete files, commands, and commit messages

**Type consistency**
- plan uses one consistent naming set:
  - `AccountProxyMode`
  - `ProxyConfigRecord`
  - `ResolvedAccountProxy`
  - `AdminGpt2ApiRsProxyConfigView`
  - `proxy_mode`
  - `proxy_config_id`
  - `effective_proxy_source`
  - `effective_proxy_url`
  - `effective_proxy_config_name`
