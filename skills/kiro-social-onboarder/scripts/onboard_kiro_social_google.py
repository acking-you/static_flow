#!/usr/bin/env python3
"""One-shot Kiro social Google login and llm-access import.

The script intentionally avoids `kiro-cli logout`. It edits only known Kiro
auth metadata keys, writes social auth after device approval, and never prints
raw token values.
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
NODE_DRIVER = Path(__file__).with_name("drive_kiro_social_google.mjs")
SOCIAL_TOKEN_KEY = "kirocli:social:token"
PROFILE_STATE_KEY = "api.codewhisperer.profile"
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


def check_proxy(proxy: str) -> None:
    req = urllib.request.Request("https://www.google.com/generate_204", method="GET")
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({"http": proxy, "https": proxy})
    )
    with opener.open(req, timeout=20) as resp:
        if resp.status not in (200, 204):
            raise RuntimeError(f"proxy probe returned HTTP {resp.status}")


def backup_and_clean_sqlite(path: Path) -> Path:
    if not path.is_file():
        raise FileNotFoundError(f"Kiro SQLite not found: {path}")
    backup_dir = path.parent / "backups"
    backup_dir.mkdir(parents=True, exist_ok=True)
    stamp = dt.datetime.now().strftime("%Y%m%d-%H%M%S")
    backup = backup_dir / f"{path.name}.before-social-google-onboard-{stamp}"
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
        "provider": "google",
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
        "provider": "google",
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


def drive_device_flow_with_node(args: argparse.Namespace, port: int, password: str) -> None:
    if not NODE_DRIVER.is_file():
        raise FileNotFoundError(f"Node DevTools driver not found: {NODE_DRIVER}")
    if not shutil.which("node"):
        raise RuntimeError("node is required for browser automation")
    env = os.environ.copy()
    env.update(
        {
            "KIRO_DEVTOOLS_PORT": str(port),
            "KIRO_GOOGLE_EMAIL": args.email,
            "KIRO_GOOGLE_PASSWORD": password,
            "KIRO_MANUAL_TIMEOUT_SECONDS": str(args.manual_timeout_seconds),
        }
    )
    result = subprocess.run(
        ["node", str(NODE_DRIVER)],
        env=env,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"browser automation failed with code {result.returncode}")


def start_device_authorization(proxy: str, client_id: str) -> dict[str, Any]:
    return open_json(
        "POST",
        f"{AUTH_BASE}/oauth/device/authorization",
        {"clientId": client_id, "loginProvider": "Google"},
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
    profile_dir = args.chrome_profile or tempfile.mkdtemp(prefix="kiro-social-google-")
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


def importer_path() -> Path:
    return (
        Path(__file__).resolve().parents[2]
        / "kiro-local-account-importer/scripts/import_kiro_accounts.py"
    )


def run_importer(args: argparse.Namespace, *, apply: bool) -> dict[str, Any]:
    cmd = [
        sys.executable,
        str(importer_path()),
        "--admin-base-url",
        args.admin_base_url,
        "--sqlite-file",
        str(args.sqlite_file),
        "--account-name",
        args.account_name,
        "--seed",
        str(args.seed),
    ]
    if args.admin_token:
        cmd.extend(["--admin-token", args.admin_token])
    if apply:
        cmd.append("--apply")
    result = subprocess.run(cmd, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if result.returncode != 0:
        raise RuntimeError(f"importer failed with code {result.returncode}:\n{result.stdout}")
    return json.loads(result.stdout)


def delete_account(base_url: str, name: str, token: str | None) -> None:
    try:
        admin_json("DELETE", base_url, f"/admin/kiro-gateway/accounts/{quote_name(name)}", token=token)
        log(f"Deleted llm-access Kiro account: {name}")
    except Exception as exc:
        log(f"Delete skipped/failed for {name}: {exc}")


def refresh_balance(args: argparse.Namespace) -> dict[str, Any]:
    return admin_json(
        "POST",
        args.admin_base_url,
        f"/admin/kiro-gateway/accounts/{quote_name(args.account_name)}/balance",
        token=args.admin_token,
    )


def validate_balance(args: argparse.Namespace, balance: dict[str, Any]) -> None:
    title = str(balance.get("subscription_title") or "")
    limit = float(balance.get("usage_limit") or 0)
    if args.expect_student and "STUDENT" not in title.upper():
        raise RuntimeError(f"expected KIRO STUDENT, got {title!r}")
    if limit < args.expect_usage_limit:
        raise RuntimeError(f"expected usage_limit >= {args.expect_usage_limit}, got {limit}")


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--email", required=True)
    parser.add_argument("--account-name", required=True)
    parser.add_argument("--proxy", default=DEFAULT_PROXY)
    parser.add_argument("--admin-base-url", default=DEFAULT_ADMIN_BASE)
    parser.add_argument("--admin-token")
    parser.add_argument("--sqlite-file", type=Path, default=DEFAULT_SQLITE)
    parser.add_argument("--kiro-cli", default="kiro-cli")
    parser.add_argument("--client-id", default="kiro-cli")
    parser.add_argument("--password-env", default="KIRO_GOOGLE_PASSWORD")
    parser.add_argument("--replace-account", action="store_true")
    parser.add_argument("--delete-account-name", action="append", default=[])
    parser.add_argument("--manual-timeout-seconds", type=int, default=300)
    parser.add_argument("--token-poll-timeout-seconds", type=int, default=300)
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
    password = os.environ.get(args.password_env)
    if not password:
        password = getpass.getpass(f"Google password for {args.email}: ")
    if not password:
        raise SystemExit("empty Google password")

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
    log(f"Started Kiro social Google device flow. User code: {auth.get('userCode')}")

    proc: subprocess.Popen[Any] | None = None
    profile_dir: str | None = None
    try:
        proc, port, profile_dir = launch_chrome(args, verify_url)
        wait_for_page_target(port)
        drive_device_flow_with_node(args, port, password)
        token_payload = poll_device_token(
            args.proxy, args.client_id, device_code, args.token_poll_timeout_seconds
        )
        written = write_social_token(args.sqlite_file.expanduser(), token_payload)
        log(
            "Wrote local social token: "
            + compact_json({key: written[key] for key in written if not key.endswith("_len")})
        )
    finally:
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

    dry_run = run_importer(args, apply=False)
    log("Importer dry-run:")
    log(json.dumps(dry_run, ensure_ascii=False, indent=2))

    applied = run_importer(args, apply=True)
    log("Importer apply:")
    log(json.dumps(applied, ensure_ascii=False, indent=2))

    balance = refresh_balance(args)
    validate_balance(args, balance)
    summary = {
        "account_name": args.account_name,
        "subscription_title": balance.get("subscription_title"),
        "usage_limit": balance.get("usage_limit"),
        "remaining": balance.get("remaining"),
        "current_usage": balance.get("current_usage"),
        "user_id": balance.get("user_id"),
    }
    log("Final balance:")
    log(json.dumps(summary, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
