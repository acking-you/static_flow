#!/usr/bin/env python3
"""One-shot Kiro social GitHub login and llm-access import.

The script intentionally avoids `kiro-cli logout`. It edits only known Kiro
auth metadata keys, writes social auth after device approval, and never prints
raw token values. GitHub login can be prefilled from environment-provided
credentials; 2FA and unusual verification stay manual in the launched browser.
"""

from __future__ import annotations

import argparse
import datetime as dt
import getpass
import json
import os
import shutil
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any


AUTH_BASE = "https://prod.us-east-1.auth.desktop.kiro.dev"
DEFAULT_PROXY = "http://127.0.0.1:11111"
DEFAULT_ADMIN_BASE = "http://127.0.0.1:19182"
DEFAULT_SQLITE = Path.home() / ".local/share/kiro-cli/data.sqlite3"
NODE_DRIVER = Path(__file__).with_name("drive_kiro_social_github.mjs")
SOCIAL_TOKEN_KEY = "kirocli:social:token"
PROFILE_STATE_KEY = "api.codewhisperer.profile"
DEFAULT_REGION = "us-east-1"
LOCAL_AUTH_KEYS = (
    "kirocli:odic:token",
    "kirocli:oidc:token",
    "kirocli:odic:device-registration",
    "kirocli:oidc:device-registration",
    SOCIAL_TOKEN_KEY,
)
LOCAL_STATE_KEYS = (
    PROFILE_STATE_KEY,
    "telemetry-cognito-credentials",
)


def log(message: str) -> None:
    print(message, flush=True)


def compact_json(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def open_json(
    method: str,
    url: str,
    body: dict[str, Any] | None = None,
    *,
    proxy: str | None = None,
    headers: dict[str, str] | None = None,
    timeout: float = 30.0,
) -> Any:
    data = None if body is None else compact_json(body).encode("utf-8")
    merged = {"Accept": "application/json", "User-Agent": "kiro-cli"}
    if body is not None:
        merged["Content-Type"] = "application/json"
    if headers:
        merged.update(headers)
    req = urllib.request.Request(url, data=data, headers=merged, method=method)
    proxy_handler = urllib.request.ProxyHandler(
        {"http": proxy, "https": proxy} if proxy else {}
    )
    opener = urllib.request.build_opener(proxy_handler)
    try:
        with opener.open(req, timeout=timeout) as resp:
            raw = resp.read()
            return json.loads(raw.decode("utf-8")) if raw else None
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {url} failed: HTTP {exc.code}: {text[:500]}") from exc


def admin_json(
    method: str,
    base_url: str,
    path: str,
    body: dict[str, Any] | None = None,
    token: str | None = None,
) -> Any:
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
    headers = {"x-admin-token": token} if token else None
    return open_json(method, url, body, headers=headers)


def quote_name(name: str) -> str:
    return urllib.parse.quote(name, safe="")


def field(data: dict[str, Any], *names: str) -> Any:
    for name in names:
        value = data.get(name)
        if isinstance(value, str):
            value = value.strip()
        if value not in (None, ""):
            return value
    return None


def account_name(account: dict[str, Any]) -> str | None:
    value = field(account, "name", "account_name", "id")
    return str(value) if value is not None else None


def account_user_id(account: dict[str, Any]) -> str | None:
    value = field(account, "upstream_user_id")
    if value is not None:
        return str(value)
    balance = account.get("balance")
    if isinstance(balance, dict):
        value = field(balance, "user_id")
        if value is not None:
            return str(value)
    return None


def existing_user_id_map(accounts: list[dict[str, Any]]) -> dict[str, str]:
    result: dict[str, str] = {}
    for account in accounts:
        name = account_name(account)
        user_id = account_user_id(account)
        if name and user_id:
            result.setdefault(user_id, name)
    return result


def select_existing_account_name_from_probe(
    probe_result: dict[str, Any],
    accounts: list[dict[str, Any]],
    *,
    exclude_names: set[str] | None = None,
) -> str | None:
    exclude_names = exclude_names or set()
    results = probe_result.get("results")
    if not isinstance(results, list):
        return None
    for result in results:
        if not isinstance(result, dict):
            continue
        duplicate_of = field(result, "duplicate_of")
        if duplicate_of is not None:
            duplicate_name = str(duplicate_of)
            if duplicate_name not in exclude_names:
                return duplicate_name
    user_ids = existing_user_id_map(
        [account for account in accounts if account_name(account) not in exclude_names]
    )
    for result in results:
        if not isinstance(result, dict):
            continue
        balance = result.get("balance")
        if not isinstance(balance, dict):
            continue
        user_id = field(balance, "user_id")
        if user_id is not None and str(user_id) in user_ids:
            return user_ids[str(user_id)]
    return None


def find_account(accounts: list[dict[str, Any]], name: str) -> dict[str, Any] | None:
    return next((account for account in accounts if account_name(account) == name), None)


def account_has_refreshable_auth_issue(account: dict[str, Any]) -> bool:
    issue_kind = field(account, "issue_kind")
    if str(issue_kind).lower() == "auth_401":
        return True

    messages: list[str] = []
    for key in ("disabled_reason", "issue_summary", "last_error"):
        value = field(account, key)
        if value is not None:
            messages.append(str(value))
    cache = account.get("cache")
    if isinstance(cache, dict):
        value = field(cache, "error_message", "last_error")
        if value is not None:
            messages.append(str(value))

    refreshable_markers = (
        "401",
        "unauthorized",
        "invalid_refresh_token",
        "invalid_grant",
    )
    return any(
        marker in message.lower()
        for message in messages
        for marker in refreshable_markers
    )


def should_preserve_disabled_state(account: dict[str, Any]) -> bool:
    return bool(account.get("disabled", False)) and not account_has_refreshable_auth_issue(account)


def build_existing_account_import_body(
    current: dict[str, Any],
    token_payload: dict[str, Any],
    *,
    expires_at: str,
    source_db_path: Path,
    email: str | None = None,
) -> dict[str, Any]:
    name = account_name(current)
    if not name:
        raise RuntimeError("matched Kiro account is missing a name")
    access_token = token_payload.get("accessToken")
    refresh_token = token_payload.get("refreshToken")
    profile_arn = token_payload.get("profileArn") or current.get("profile_arn")
    if not access_token or not refresh_token or not profile_arn:
        raise RuntimeError("social token response missing required fields")

    effective_email = email.strip() if isinstance(email, str) else None
    if not effective_email:
        existing_email = field(current, "email")
        effective_email = str(existing_email) if existing_email is not None else None

    body: dict[str, Any] = {
        "name": name,
        "access_token": access_token,
        "refresh_token": refresh_token,
        "profile_arn": profile_arn,
        "expires_at": expires_at,
        "auth_method": "social",
        "provider": "github",
        "region": current.get("region") or DEFAULT_REGION,
        "auth_region": current.get("auth_region") or current.get("region") or DEFAULT_REGION,
        "api_region": current.get("api_region") or current.get("region") or DEFAULT_REGION,
        "source_db_path": str(source_db_path),
        "last_imported_at": int(time.time() * 1000),
        "disabled": should_preserve_disabled_state(current),
    }
    if effective_email:
        body["email"] = effective_email
    for key in (
        "subscription_title",
        "machine_id",
        "kiro_channel_max_concurrency",
        "kiro_channel_min_start_interval_ms",
        "minimum_remaining_credits_before_block",
        "manual_usage_limit",
        "pool_strategy",
    ):
        if current.get(key) is not None:
            body[key] = current[key]
    return body


def check_proxy(proxy: str) -> None:
    req = urllib.request.Request("https://github.com/login", method="GET")
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({"http": proxy, "https": proxy})
    )
    with opener.open(req, timeout=20) as resp:
        if resp.status not in (200, 302):
            raise RuntimeError(f"proxy probe returned HTTP {resp.status}")


def backup_and_clean_sqlite(path: Path) -> Path:
    if not path.is_file():
        raise FileNotFoundError(f"Kiro SQLite not found: {path}")
    backup_dir = path.parent / "backups"
    backup_dir.mkdir(parents=True, exist_ok=True)
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = backup_dir / f"{path.name}.before-social-github-onboard-{stamp}"
    shutil.copy2(path, backup)

    conn = sqlite3.connect(path)
    try:
        conn.executemany("DELETE FROM auth_kv WHERE key = ?", [(key,) for key in LOCAL_AUTH_KEYS])
        conn.executemany("DELETE FROM state WHERE key = ?", [(key,) for key in LOCAL_STATE_KEYS])
        conn.commit()
    finally:
        conn.close()
    return backup


def write_social_token(path: Path, payload: dict[str, Any]) -> dict[str, Any]:
    access_token = payload.get("accessToken")
    refresh_token = payload.get("refreshToken")
    profile_arn = payload.get("profileArn")
    if not access_token or not refresh_token or not profile_arn:
        raise RuntimeError("social token response missing required fields")
    expires_at = (
        dt.datetime.now(dt.timezone.utc) + dt.timedelta(hours=1)
    ).isoformat().replace("+00:00", "Z")
    token = {
        "access_token": access_token,
        "refresh_token": refresh_token,
        "expires_at": expires_at,
        "provider": "github",
        "profile_arn": profile_arn,
    }
    profile = {"arn": profile_arn, "profile_name": "Social_Default_Profile"}
    conn = sqlite3.connect(path)
    try:
        conn.execute(
            "INSERT OR REPLACE INTO auth_kv(key, value) VALUES (?, ?)",
            (SOCIAL_TOKEN_KEY, compact_json(token)),
        )
        conn.execute(
            "INSERT OR REPLACE INTO state(key, value) VALUES (?, ?)",
            (PROFILE_STATE_KEY, compact_json(profile)),
        )
        conn.commit()
    finally:
        conn.close()
    return {
        "provider": "github",
        "profile_arn": profile_arn,
        "access_token_len": len(access_token),
        "refresh_token_len": len(refresh_token),
        "expires_at": expires_at,
    }


def find_free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def chrome_binary(explicit: str | None) -> str:
    if explicit:
        return explicit
    for name in ("google-chrome", "chromium", "chromium-browser"):
        found = shutil.which(name)
        if found:
            return found
    raise RuntimeError("Chrome/Chromium binary not found")


def wait_http_json(url: str, timeout: float = 20.0) -> Any:
    deadline = time.monotonic() + timeout
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    while time.monotonic() < deadline:
        try:
            with opener.open(url, timeout=2) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except Exception:
            time.sleep(0.25)
    raise RuntimeError(f"timed out waiting for {url}")


def start_device_flow_helper(
    args: argparse.Namespace,
    port: int,
    github_password: str | None = None,
) -> subprocess.Popen[Any]:
    if not NODE_DRIVER.is_file():
        raise FileNotFoundError(f"Node DevTools driver not found: {NODE_DRIVER}")
    if not shutil.which("node"):
        raise RuntimeError("node is required for browser automation")
    env = os.environ.copy()
    env.update(
        {
            "KIRO_DEVTOOLS_PORT": str(port),
            "KIRO_MANUAL_TIMEOUT_SECONDS": str(args.manual_timeout_seconds),
        }
    )
    github_login = resolve_github_login(args)
    if github_login and github_password:
        env["KIRO_GITHUB_LOGIN"] = github_login
        env["KIRO_GITHUB_PASSWORD"] = github_password
    return subprocess.Popen(
        ["node", str(NODE_DRIVER)],
        env=env,
        stdout=None,
        stderr=None,
    )


def stop_device_flow_helper(proc: subprocess.Popen[Any] | None) -> None:
    if not proc or proc.poll() is not None:
        return
    proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.kill()


def start_device_authorization(proxy: str, client_id: str) -> dict[str, Any]:
    return open_json(
        "POST",
        f"{AUTH_BASE}/oauth/device/authorization",
        {"clientId": client_id, "loginProvider": "Github"},
        proxy=proxy,
    )


def poll_device_token(proxy: str, client_id: str, device_code: str, timeout_seconds: int) -> dict[str, Any]:
    deadline = time.monotonic() + timeout_seconds
    while time.monotonic() < deadline:
        try:
            payload = open_json(
                "POST",
                f"{AUTH_BASE}/oauth/device/poll",
                {"clientId": client_id, "deviceCode": device_code},
                proxy=proxy,
            )
        except RuntimeError as exc:
            if "AuthorizationPending" in str(exc) or "authorization" in str(exc).lower():
                time.sleep(5)
                continue
            raise
        if isinstance(payload, dict) and payload.get("accessToken") and payload.get("refreshToken"):
            return payload
        time.sleep(5)
    raise RuntimeError("timed out polling Kiro social token")


def launch_chrome(args: argparse.Namespace, url: str) -> tuple[subprocess.Popen[Any], int, str]:
    port = args.debug_port or find_free_port()
    profile_dir = args.chrome_profile or tempfile.mkdtemp(prefix="kiro-social-github-")
    cmd = [
        chrome_binary(args.chrome_bin),
        f"--user-data-dir={profile_dir}",
        f"--proxy-server={args.proxy}",
        "--no-first-run",
        "--no-default-browser-check",
        "--disable-background-networking",
        "--disable-gpu",
        "--disable-software-rasterizer",
        "--remote-debugging-address=127.0.0.1",
        f"--remote-debugging-port={port}",
        url,
    ]
    proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    return proc, port, profile_dir


def wait_for_page_target(port: int) -> None:
    pages = wait_http_json(f"http://127.0.0.1:{port}/json/list", timeout=25)
    page = next((item for item in pages if item.get("type") == "page"), None)
    if not page:
        raise RuntimeError("Chrome DevTools page target not found")


def run_kiro_whoami(args: argparse.Namespace) -> str:
    env = os.environ.copy()
    env.update({"HTTP_PROXY": args.proxy, "HTTPS_PROXY": args.proxy, "ALL_PROXY": args.proxy})
    result = subprocess.run(
        [args.kiro_cli, "whoami"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"kiro-cli whoami failed: {result.stdout.strip()}")
    return result.stdout.strip()


def parse_whoami_email(output: str) -> str | None:
    for line in output.splitlines():
        key, separator, value = line.partition(":")
        if separator and key.strip().lower() == "email":
            email = value.strip()
            return email or None
    return None


def resolve_github_login(args: argparse.Namespace) -> str | None:
    login = field(vars(args), "github_login")
    if login is not None:
        return str(login)
    if args.account_name:
        return str(args.account_name)
    return None


def resolve_github_password(args: argparse.Namespace, github_login: str | None) -> str | None:
    password = os.environ.get(args.password_env)
    if password:
        return password
    if github_login and sys.stdin.isatty():
        password = getpass.getpass(f"GitHub password for {github_login}: ")
        if not password:
            raise SystemExit("empty GitHub password")
        return password
    return None


def importer_path() -> Path:
    return (
        Path(__file__).resolve().parents[2]
        / "kiro-local-account-importer/scripts/import_kiro_accounts.py"
    )


def run_importer(
    args: argparse.Namespace,
    *,
    apply: bool,
    account_name_override: str | None = None,
    allow_failure: bool = False,
) -> dict[str, Any]:
    account_name_value = account_name_override or args.account_name
    if not account_name_value:
        raise RuntimeError("account name is required for importer invocation")
    cmd = [
        sys.executable,
        str(importer_path()),
        "--admin-base-url",
        args.admin_base_url,
        "--sqlite-file",
        str(args.sqlite_file),
        "--account-name",
        account_name_value,
        "--seed",
        str(args.seed),
    ]
    if args.admin_token:
        cmd.extend(["--admin-token", args.admin_token])
    if apply:
        cmd.append("--apply")
    result = subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode != 0:
        if allow_failure:
            try:
                payload = json.loads(result.stdout)
            except json.JSONDecodeError:
                payload = {"raw_output": result.stdout}
            payload["returncode"] = result.returncode
            return payload
        raise RuntimeError(f"importer failed with code {result.returncode}:\n{result.stdout}")
    payload = json.loads(result.stdout)
    payload["returncode"] = result.returncode
    return payload


def fetch_kiro_accounts(base_url: str, token: str | None) -> list[dict[str, Any]]:
    accounts: list[dict[str, Any]] = []
    offset = 0
    limit = 200
    while True:
        payload = admin_json(
            "GET",
            base_url,
            f"/admin/kiro-gateway/accounts?limit={limit}&offset={offset}",
            token=token,
        )
        page_accounts = payload.get("accounts", []) if isinstance(payload, dict) else []
        batch = [account for account in page_accounts if isinstance(account, dict)]
        accounts.extend(batch)
        if not isinstance(payload, dict) or not payload.get("has_more") or not batch:
            return accounts
        offset += len(batch)


def temporary_probe_account_name() -> str:
    return f"kiro-refresh-probe-{int(time.time())}-{os.getpid()}"


def delete_account(base_url: str, name: str, token: str | None) -> None:
    try:
        admin_json("DELETE", base_url, f"/admin/kiro-gateway/accounts/{quote_name(name)}", token=token)
        log(f"Deleted llm-access Kiro account: {name}")
    except Exception as exc:
        log(f"Delete skipped/failed for {name}: {exc}")


def refresh_balance(args: argparse.Namespace) -> dict[str, Any]:
    if not args.account_name:
        raise RuntimeError("account name is required for balance refresh")
    return admin_json(
        "POST",
        args.admin_base_url,
        f"/admin/kiro-gateway/accounts/{quote_name(args.account_name)}/balance",
        token=args.admin_token,
    )


def refresh_balance_for_account(args: argparse.Namespace, account_name_value: str) -> dict[str, Any]:
    return admin_json(
        "POST",
        args.admin_base_url,
        f"/admin/kiro-gateway/accounts/{quote_name(account_name_value)}/balance",
        token=args.admin_token,
    )


def update_existing_account_from_token(
    args: argparse.Namespace,
    target_name: str,
    token_payload: dict[str, Any],
    expires_at: str,
    email: str | None = None,
) -> dict[str, Any]:
    accounts = fetch_kiro_accounts(args.admin_base_url, args.admin_token)
    current = find_account(accounts, target_name)
    if current is None:
        raise RuntimeError(f"matched Kiro account not found: {target_name}")
    body = build_existing_account_import_body(
        current,
        token_payload,
        expires_at=expires_at,
        source_db_path=args.sqlite_file.expanduser(),
        email=email,
    )
    saved = admin_json(
        "POST",
        args.admin_base_url,
        "/admin/kiro-gateway/accounts/import-auth",
        body=body,
        token=args.admin_token,
    )
    proxy_mode = current.get("proxy_mode")
    proxy_config_id = current.get("proxy_config_id")
    patched = None
    if proxy_mode == "fixed" and proxy_config_id:
        patched = admin_json(
            "PATCH",
            args.admin_base_url,
            f"/admin/kiro-gateway/accounts/{quote_name(target_name)}",
            body={"proxy_mode": "fixed", "proxy_config_id": proxy_config_id},
            token=args.admin_token,
        )
    elif proxy_mode in ("inherit", "none"):
        patched = admin_json(
            "PATCH",
            args.admin_base_url,
            f"/admin/kiro-gateway/accounts/{quote_name(target_name)}",
            body={"proxy_mode": proxy_mode},
            token=args.admin_token,
        )
    balance = refresh_balance_for_account(args, target_name)
    return {
        "account_name": target_name,
        "saved_name": saved.get("name") if isinstance(saved, dict) else target_name,
        "patched_proxy": patched is not None,
        "proxy_mode": proxy_mode,
        "proxy_config_id": proxy_config_id,
        "proxy_config_name": current.get("effective_proxy_config_name"),
        "balance": balance,
    }


def auto_refresh_existing_account(
    args: argparse.Namespace,
    token_payload: dict[str, Any],
    expires_at: str,
    email: str | None = None,
) -> dict[str, Any]:
    probe_name = temporary_probe_account_name()
    log(f"Probing refreshed credentials with temporary account: {probe_name}")
    dry_run = run_importer(args, apply=False, account_name_override=probe_name)
    log("Probe importer dry-run:")
    log(json.dumps(dry_run, ensure_ascii=False, indent=2))
    probe = run_importer(
        args,
        apply=True,
        account_name_override=probe_name,
        allow_failure=True,
    )
    log("Probe importer apply:")
    log(json.dumps(probe, ensure_ascii=False, indent=2))
    accounts = fetch_kiro_accounts(args.admin_base_url, args.admin_token)
    target_name = select_existing_account_name_from_probe(
        probe,
        accounts,
        exclude_names={probe_name},
    )
    if target_name is None:
        delete_account(args.admin_base_url, probe_name, args.admin_token)
        raise RuntimeError("refreshed credentials did not match an existing Kiro account")
    if find_account(fetch_kiro_accounts(args.admin_base_url, args.admin_token), probe_name):
        delete_account(args.admin_base_url, probe_name, args.admin_token)
    updated = update_existing_account_from_token(
        args,
        target_name,
        token_payload,
        expires_at,
        email=email,
    )
    updated["probe_name"] = probe_name
    return updated


def validate_balance(args: argparse.Namespace, balance: dict[str, Any]) -> None:
    title = str(balance.get("subscription_title") or "")
    limit = float(balance.get("usage_limit") or 0)
    if args.expect_student and "STUDENT" not in title.upper():
        raise RuntimeError(f"expected KIRO STUDENT, got {title!r}")
    if limit < args.expect_usage_limit:
        raise RuntimeError(f"expected usage_limit >= {args.expect_usage_limit}, got {limit}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--account-name",
        help=(
            "Explicit llm-access account name to import. Omit it to auto-match "
            "the refreshed credentials to an existing account by upstream user_id."
        ),
    )
    parser.add_argument("--proxy", default=DEFAULT_PROXY)
    parser.add_argument("--admin-base-url", default=DEFAULT_ADMIN_BASE)
    parser.add_argument("--admin-token")
    parser.add_argument("--sqlite-file", type=Path, default=DEFAULT_SQLITE)
    parser.add_argument("--kiro-cli", default="kiro-cli")
    parser.add_argument("--client-id", default="kiro-cli")
    parser.add_argument(
        "--github-login",
        help=(
            "GitHub username or email for automatic login-page prefill. "
            "Defaults to --account-name when a password is available."
        ),
    )
    parser.add_argument("--password-env", default="KIRO_GITHUB_PASSWORD")
    parser.add_argument("--replace-account", action="store_true")
    parser.add_argument("--delete-account-name", action="append", default=[])
    parser.add_argument("--manual-timeout-seconds", type=int, default=600)
    parser.add_argument("--token-poll-timeout-seconds", type=int, default=600)
    parser.add_argument("--seed", type=int, default=745)
    parser.add_argument("--expect-usage-limit", type=float, default=1000.0)
    parser.add_argument("--no-expect-student", dest="expect_student", action="store_false")
    parser.set_defaults(expect_student=True)
    parser.add_argument("--chrome-bin")
    parser.add_argument("--chrome-profile")
    parser.add_argument("--debug-port", type=int)
    parser.add_argument("--keep-browser", action="store_true")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.replace_account and not args.account_name:
        raise SystemExit("--replace-account requires --account-name")
    github_login = resolve_github_login(args)
    github_password = resolve_github_password(args, github_login)

    log("Checking proxy...")
    check_proxy(args.proxy)

    for name in args.delete_account_name:
        delete_account(args.admin_base_url, name, args.admin_token)
    if args.replace_account:
        delete_account(args.admin_base_url, args.account_name, args.admin_token)

    backup = backup_and_clean_sqlite(args.sqlite_file.expanduser())
    log(f"Backed up and cleaned local Kiro auth metadata: {backup}")

    auth = start_device_authorization(args.proxy, args.client_id)
    device_code = auth["deviceCode"]
    verify_url = auth["verificationUriComplete"]
    log(f"Started Kiro social GitHub device flow. User code: {auth.get('userCode')}")
    log("Complete GitHub login, 2FA, and consent in the launched browser when prompted.")

    proc: subprocess.Popen[Any] | None = None
    helper: subprocess.Popen[Any] | None = None
    profile_dir: str | None = None
    try:
        proc, port, profile_dir = launch_chrome(args, verify_url)
        wait_for_page_target(port)
        helper = start_device_flow_helper(args, port, github_password)
        token_payload = poll_device_token(
            args.proxy, args.client_id, device_code, args.token_poll_timeout_seconds
        )
        written = write_social_token(args.sqlite_file.expanduser(), token_payload)
        log(
            "Wrote local social token: "
            + compact_json({key: written[key] for key in written if not key.endswith("_len")})
        )
    finally:
        stop_device_flow_helper(helper)
        if proc and not args.keep_browser:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
        if profile_dir and not args.keep_browser and not args.chrome_profile:
            shutil.rmtree(profile_dir, ignore_errors=True)

    whoami = run_kiro_whoami(args)
    log("kiro-cli whoami:")
    log(whoami)
    whoami_email = parse_whoami_email(whoami)
    if whoami_email:
        log(f"Detected Kiro account email: {whoami_email}")

    if args.account_name:
        dry_run = run_importer(args, apply=False)
        log("Importer dry-run:")
        log(json.dumps(dry_run, ensure_ascii=False, indent=2))

        applied = run_importer(args, apply=True)
        log("Importer apply:")
        log(json.dumps(applied, ensure_ascii=False, indent=2))
        target_name = args.account_name
        if whoami_email:
            refreshed = update_existing_account_from_token(
                args,
                target_name,
                token_payload,
                written["expires_at"],
                email=whoami_email,
            )
            log("Updated existing Kiro account email/token:")
            log(json.dumps(refreshed, ensure_ascii=False, indent=2))
            balance = refreshed["balance"]
        else:
            balance = refresh_balance(args)
    else:
        refreshed = auto_refresh_existing_account(
            args,
            token_payload,
            written["expires_at"],
            email=whoami_email,
        )
        target_name = str(refreshed["account_name"])
        balance = refreshed["balance"]
        log("Updated existing Kiro account:")
        log(json.dumps(refreshed, ensure_ascii=False, indent=2))

    validate_balance(args, balance)
    summary = {
        "account_name": target_name,
        "subscription_title": balance.get("subscription_title"),
        "usage_limit": balance.get("usage_limit"),
        "remaining": balance.get("remaining"),
        "current_usage": balance.get("current_usage"),
        "user_id": balance.get("user_id"),
        "email": whoami_email,
    }
    log("Final balance:")
    log(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
