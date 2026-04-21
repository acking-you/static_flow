---
name: gpt2api-rs-admin
description: Use when operating the local gpt2api-rs service in deps/gpt2api_rs for status checks or list-style admin tasks through its REST admin surface and bundled CLI.
---

# gpt2api-rs Admin

Use this skill when the user wants to inspect or operate the standalone
`gpt2api-rs` service in `deps/gpt2api_rs`.

## Requirements

- Running service process
- Admin token
- Base URL, for example `http://127.0.0.1:8787`

## Common commands

Start the service:

```bash
cd /Users/boliu/rust_pro/static_flow/deps/gpt2api_rs
cargo run -- serve --listen 127.0.0.1:8787 --storage-dir /tmp/gpt2api --admin-token "$ADMIN_TOKEN"
```

List accounts:

```bash
cd /Users/boliu/rust_pro/static_flow/deps/gpt2api_rs
cargo run -- admin --base-url "$BASE_URL" --admin-token "$ADMIN_TOKEN" accounts list --json
```

List keys:

```bash
cd /Users/boliu/rust_pro/static_flow/deps/gpt2api_rs
cargo run -- admin --base-url "$BASE_URL" --admin-token "$ADMIN_TOKEN" keys list --json
```

List recent usage:

```bash
cd /Users/boliu/rust_pro/static_flow/deps/gpt2api_rs
cargo run -- admin --base-url "$BASE_URL" --admin-token "$ADMIN_TOKEN" usage list --limit 50 --json
```

Read the admin status snapshot:

```bash
curl -H "Authorization: Bearer $ADMIN_TOKEN" "$BASE_URL/admin/status"
```
