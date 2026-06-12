#!/usr/bin/env python3
"""Import local Kiro CLI auth SQLite records through llm-access admin APIs."""

from __future__ import annotations

import argparse
import json
import random
import re
import sqlite3
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SOCIAL_TOKEN_KEY = "kirocli:social:token"
IDC_TOKEN_KEYS = ("kirocli:odic:token", "kirocli:oidc:token")
IDC_DEVICE_KEYS = (
    "kirocli:odic:device-registration",
    "kirocli:oidc:device-registration",
)
PROFILE_STATE_KEY = "api.codewhisperer.profile"
DEFAULT_SQLITE = Path.home() / ".local/share/kiro-cli/data.sqlite3"
DEFAULT_MINIMUM_REMAINING_CREDITS = 10.0
DEFAULT_KIRO_REGION = "us-east-1"
REQUIRED_PROXY_REGION = "us"
US_PROXY_NAME_RE = re.compile(r"(^|[-_])us([_-]|\d|$)|homeus|aws_us|dmit-us|do-us", re.I)


@dataclass
class ImportedAuth:
    name: str
    sqlite_path: Path
    body: dict[str, Any]


def stable_account_name(raw: str, fallback: str) -> str:
    value = re.sub(r"[^A-Za-z0-9_-]+", "-", raw.strip())
    value = value.strip("-_") or fallback
    return value[:64]


def load_json(raw: str, context: str) -> dict[str, Any]:
    value = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError(f"{context} must be a JSON object")
    return value


def field(data: dict[str, Any], *names: str) -> Any:
    for name in names:
        value = data.get(name)
        if isinstance(value, str):
            value = value.strip()
        if value not in (None, ""):
            return value
    return None


def resolved_region(*sources: dict[str, Any]) -> str:
    for source in sources:
        if not source:
            continue
        value = field(source, "region")
        if value is not None:
            text = str(value).strip()
            if text:
                return text
    return DEFAULT_KIRO_REGION


def query_auth_kv(conn: sqlite3.Connection, keys: tuple[str, ...]) -> str | None:
    for key in keys:
        row = conn.execute("SELECT value FROM auth_kv WHERE key = ? LIMIT 1", (key,)).fetchone()
        if row:
            return str(row[0])
    return None


def load_profile_arn(conn: sqlite3.Connection) -> str | None:
    try:
        row = conn.execute(
            "SELECT value FROM state WHERE key = ? LIMIT 1", (PROFILE_STATE_KEY,)
        ).fetchone()
    except sqlite3.Error:
        return None
    if not row:
        return None
    try:
        state = load_json(str(row[0]), PROFILE_STATE_KEY)
    except (TypeError, ValueError, json.JSONDecodeError):
        return None
    value = field(state, "profileArn", "profile_arn")
    return str(value) if value is not None else None


def parse_sqlite(path: Path, name: str) -> ImportedAuth:
    if not path.is_file():
        raise FileNotFoundError(f"Kiro SQLite file not found: {path}")

    conn = sqlite3.connect(str(path))
    try:
        profile_arn = load_profile_arn(conn)
        social_raw = query_auth_kv(conn, (SOCIAL_TOKEN_KEY,))
        if social_raw:
            token = load_json(social_raw, SOCIAL_TOKEN_KEY)
            refresh_token = field(token, "refresh_token", "refreshToken")
            if not refresh_token:
                raise ValueError(f"{path}: social token is missing refresh_token")
            region = resolved_region(token)
            body = {
                "name": name,
                "access_token": field(token, "access_token", "accessToken"),
                "refresh_token": refresh_token,
                "profile_arn": field(token, "profile_arn", "profileArn") or profile_arn,
                "expires_at": field(token, "expires_at", "expiresAt"),
                "auth_method": "social",
                "provider": field(token, "provider"),
                "region": region,
                "auth_region": region,
                "api_region": region,
                "minimum_remaining_credits_before_block": DEFAULT_MINIMUM_REMAINING_CREDITS,
                "disabled": False,
            }
            return ImportedAuth(name=name, sqlite_path=path, body=strip_none(body))

        idc_raw = query_auth_kv(conn, IDC_TOKEN_KEYS)
        device_raw = query_auth_kv(conn, IDC_DEVICE_KEYS)
        if not idc_raw:
            raise ValueError(f"{path}: no supported Kiro token found in auth_kv")
        if not device_raw:
            raise ValueError(f"{path}: missing Kiro IDC device registration in auth_kv")
        token = load_json(idc_raw, "idc token")
        device = load_json(device_raw, "idc device registration")
        refresh_token = field(token, "refresh_token", "refreshToken")
        client_id = field(device, "client_id", "clientId")
        client_secret = field(device, "client_secret", "clientSecret")
        missing = [
            key
            for key, value in (
                ("refresh_token", refresh_token),
                ("client_id", client_id),
                ("client_secret", client_secret),
            )
            if not value
        ]
        if missing:
            raise ValueError(f"{path}: IDC auth missing {', '.join(missing)}")
        region = resolved_region(token, device)
        body = {
            "name": name,
            "access_token": field(token, "access_token", "accessToken"),
            "refresh_token": refresh_token,
            "profile_arn": field(token, "profile_arn", "profileArn") or profile_arn,
            "expires_at": field(token, "expires_at", "expiresAt"),
            "auth_method": "idc",
            "client_id": client_id,
            "client_secret": client_secret,
            "provider": field(token, "provider") or "aws",
            "region": region,
            "auth_region": region,
            "api_region": region,
            "minimum_remaining_credits_before_block": DEFAULT_MINIMUM_REMAINING_CREDITS,
            "disabled": False,
        }
        return ImportedAuth(name=name, sqlite_path=path, body=strip_none(body))
    finally:
        conn.close()


def strip_none(data: dict[str, Any]) -> dict[str, Any]:
    return {key: value for key, value in data.items() if value is not None}


def discover_sqlite_files(files: list[str], dirs: list[str]) -> list[Path]:
    discovered: list[Path] = []
    for item in files:
        discovered.append(Path(item).expanduser())
    for item in dirs:
        root = Path(item).expanduser()
        for path in root.rglob("*"):
            if path.is_file() and path.suffix.lower() in {".sqlite", ".sqlite3", ".db"}:
                discovered.append(path)
    if not discovered and DEFAULT_SQLITE.is_file():
        discovered.append(DEFAULT_SQLITE)

    result: list[Path] = []
    seen: set[Path] = set()
    for path in discovered:
        resolved = path.resolve()
        if resolved not in seen:
            seen.add(resolved)
            result.append(resolved)
    return result


def request_json(
    method: str,
    base_url: str,
    path: str,
    token: str | None,
    body: dict[str, Any] | None = None,
    timeout: float = 30.0,
) -> Any:
    url = urllib.parse.urljoin(base_url.rstrip("/") + "/", path.lstrip("/"))
    data = None
    headers = {"Accept": "application/json"}
    if token:
        headers["x-admin-token"] = token
    if body is not None:
        data = json.dumps(body, separators=(",", ":")).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            raw = resp.read()
            if not raw:
                return None
            return json.loads(raw.decode("utf-8"))
    except urllib.error.HTTPError as exc:
        text = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {url} failed: HTTP {exc.code}: {text[:500]}") from exc


def fetch_active_proxies(base_url: str, token: str | None) -> list[dict[str, Any]]:
    payload = request_json("GET", base_url, "/admin/llm-gateway/proxy-configs", token)
    proxies = payload.get("proxy_configs", []) if isinstance(payload, dict) else []
    return [
        proxy
        for proxy in proxies
        if proxy.get("status") == "active" and proxy.get("id") and proxy.get("name")
    ]


def is_us_proxy(proxy: dict[str, Any]) -> bool:
    return bool(US_PROXY_NAME_RE.search(str(proxy.get("name") or "")))


def filter_required_region_proxies(
    proxies: list[dict[str, Any]], required_region: str
) -> list[dict[str, Any]]:
    if required_region.lower() != REQUIRED_PROXY_REGION:
        raise ValueError(f"unsupported required proxy region: {required_region}")
    return [proxy for proxy in proxies if is_us_proxy(proxy)]


def proxy_region_for_kiro_region(region: str) -> str | None:
    value = region.strip().lower()
    if value.startswith("us-"):
        return "us"
    return None


def required_proxy_region_for_auth(auth: ImportedAuth) -> str:
    region = str(field(auth.body, "api_region", "region", "auth_region") or "").strip()
    if not region:
        region = DEFAULT_KIRO_REGION
    proxy_region = proxy_region_for_kiro_region(region)
    if proxy_region is None:
        raise ValueError(
            f"{auth.name}: Kiro region `{region}` has no supported proxy-region mapping; "
            "the standard importer currently supports only US proxy configs"
        )
    return proxy_region


def fetch_kiro_accounts(base_url: str, token: str | None) -> list[dict[str, Any]]:
    payload = request_json(
        "GET", base_url, "/admin/kiro-gateway/accounts?limit=10000&offset=0", token
    )
    return payload.get("accounts", []) if isinstance(payload, dict) else []


def fetch_latency_snapshot(base_url: str, token: str | None) -> dict[str, Any] | None:
    try:
        payload = request_json(
            "GET",
            base_url,
            "/internal/kiro-gateway/latency-ranking?source=hot&window=1h",
            token,
            timeout=10.0,
        )
    except Exception:
        return None
    return payload if isinstance(payload, dict) else None


def proxy_latency_by_id(snapshot: dict[str, Any] | None) -> dict[str, float]:
    if not snapshot:
        return {}
    result: dict[str, float] = {}
    for row in snapshot.get("proxies", []):
        proxy_id = row.get("proxy_config_id")
        latency = row.get("avg_first_token_ms")
        samples = row.get("first_token_samples") or 0
        if proxy_id and isinstance(latency, (int, float)) and samples > 0:
            result[str(proxy_id)] = float(latency)
    return result


def choose_proxy(
    proxies: list[dict[str, Any]],
    account_counts: dict[str, int],
    latencies: dict[str, float],
    balance_penalty_ms: float,
) -> dict[str, Any] | None:
    ranked = rank_proxies(proxies, account_counts, latencies, balance_penalty_ms)
    if not ranked:
        return None
    selected = ranked[0]
    account_counts[str(selected["id"])] = account_counts.get(str(selected["id"]), 0) + 1
    return selected


def rank_proxies(
    proxies: list[dict[str, Any]],
    account_counts: dict[str, int],
    latencies: dict[str, float],
    balance_penalty_ms: float,
) -> list[dict[str, Any]]:
    if not proxies:
        return []

    def score(proxy: dict[str, Any]) -> tuple[float, int, str]:
        proxy_id = str(proxy["id"])
        count = account_counts.get(proxy_id, 0)
        latency = latencies.get(proxy_id)
        if latency is None:
            latency = 1_000_000.0
        return (latency + count * balance_penalty_ms, count, str(proxy.get("name") or proxy_id))

    return sorted(proxies, key=score)


def build_account_names(
    paths: list[Path], account_name: str | None, name_prefix: str | None
) -> list[str]:
    if account_name:
        if len(paths) != 1:
            raise ValueError("--account-name can only be used with exactly one SQLite file")
        return [stable_account_name(account_name, "kiro")]

    names: list[str] = []
    used: set[str] = set()
    for index, path in enumerate(paths, start=1):
        if name_prefix:
            base = f"{name_prefix}{index}"
        elif path == DEFAULT_SQLITE.resolve():
            base = "default"
        else:
            base = path.parent.name or path.stem or f"kiro-{index}"
        name = stable_account_name(base, f"kiro-{index}")
        original = name
        suffix = 2
        while name in used:
            tail = f"-{suffix}"
            name = f"{original[:64 - len(tail)]}{tail}"
            suffix += 1
        used.add(name)
        names.append(name)
    return names


def redact_body(body: dict[str, Any]) -> dict[str, Any]:
    redacted = dict(body)
    for key in ("access_token", "refresh_token", "client_secret"):
        if redacted.get(key):
            redacted[key] = "<redacted>"
    return redacted


def account_path(name: str) -> str:
    return f"/admin/kiro-gateway/accounts/{urllib.parse.quote(name, safe='')}"


def balance_path(name: str) -> str:
    return f"{account_path(name)}/balance"


def balance_has_minimum_remaining(
    balance: dict[str, Any], minimum_remaining: float
) -> bool:
    remaining = balance.get("remaining")
    return isinstance(remaining, (int, float)) and float(remaining) >= minimum_remaining


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


def delete_account(args: argparse.Namespace, name: str) -> bool:
    try:
        request_json("DELETE", args.admin_base_url, account_path(name), args.admin_token)
        return True
    except Exception:
        return False


def import_account(
    auth: ImportedAuth,
    args: argparse.Namespace,
    proxies: list[dict[str, Any]],
    min_interval_ms: int,
    existing_user_ids: dict[str, str] | None = None,
) -> dict[str, Any]:
    existing_user_ids = existing_user_ids or {}
    body = dict(auth.body)
    body["kiro_channel_max_concurrency"] = args.max_concurrency
    body["kiro_channel_min_start_interval_ms"] = min_interval_ms
    body["minimum_remaining_credits_before_block"] = args.minimum_remaining_credits
    body["source_db_path"] = str(auth.sqlite_path)
    body["last_imported_at"] = int(time.time() * 1000)
    first_proxy = proxies[0] if proxies else None

    result = {
        "name": auth.name,
        "sqlite_path": str(auth.sqlite_path),
        "auth_method": body.get("auth_method"),
        "min_start_interval_ms": min_interval_ms,
        "proxy_config_id": first_proxy.get("id") if first_proxy else None,
        "proxy_config_name": first_proxy.get("name") if first_proxy else None,
        "applied": bool(args.apply),
    }
    if not args.apply:
        result["request"] = redact_body(body)
        return result

    created = request_json(
        "POST",
        args.admin_base_url,
        "/admin/kiro-gateway/accounts/import-auth",
        args.admin_token,
        body,
    )
    result["created_name"] = created.get("name") if isinstance(created, dict) else auth.name
    result["validated"] = False
    result["deleted"] = False
    result["validation_attempts"] = []

    for proxy in proxies:
        attempt = {
            "proxy_config_id": proxy["id"],
            "proxy_config_name": proxy.get("name"),
        }
        result["validation_attempts"].append(attempt)
        try:
            patch = {"proxy_mode": "fixed", "proxy_config_id": proxy["id"]}
            request_json("PATCH", args.admin_base_url, account_path(auth.name), args.admin_token, patch)
            balance = request_json(
                "POST", args.admin_base_url, balance_path(auth.name), args.admin_token
            )
            attempt["balance"] = balance
            if not isinstance(balance, dict) or not balance_has_minimum_remaining(
                balance, args.minimum_remaining_credits
            ):
                attempt["error"] = "remaining credits below minimum"
                break
            user_id = field(balance, "user_id")
            if user_id is not None and str(user_id) in existing_user_ids:
                duplicate_of = existing_user_ids[str(user_id)]
                attempt["error"] = f"duplicate upstream user_id already imported as {duplicate_of}"
                result["duplicate_user_id"] = str(user_id)
                result["duplicate_of"] = duplicate_of
                result["balance"] = balance
                result["proxy_config_id"] = proxy["id"]
                result["proxy_config_name"] = proxy.get("name")
                result["deleted"] = delete_account(args, auth.name)
                return result
            result["validated"] = True
            result["balance"] = balance
            result["proxy_config_id"] = proxy["id"]
            result["proxy_config_name"] = proxy.get("name")
            return result
        except Exception as exc:
            attempt["error"] = str(exc)

    result["deleted"] = delete_account(args, auth.name)
    return result


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--admin-base-url", default="http://127.0.0.1:19182")
    parser.add_argument("--admin-token", default=None)
    parser.add_argument("--sqlite-file", action="append", default=[])
    parser.add_argument("--search-dir", action="append", default=[])
    parser.add_argument("--account-name")
    parser.add_argument("--name-prefix")
    parser.add_argument("--max-concurrency", type=int, default=3)
    parser.add_argument(
        "--minimum-remaining-credits",
        type=float,
        default=DEFAULT_MINIMUM_REMAINING_CREDITS,
    )
    parser.add_argument("--min-interval-min-ms", type=int, default=200)
    parser.add_argument("--min-interval-max-ms", type=int, default=1000)
    parser.add_argument("--balance-penalty-ms", type=float, default=250.0)
    parser.add_argument("--no-proxy", action="store_true")
    parser.add_argument("--apply", action="store_true")
    parser.add_argument("--seed", type=int)
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    if args.max_concurrency != 3:
        raise SystemExit("--max-concurrency must remain 3 for the standard Kiro import policy")
    if args.no_proxy:
        raise SystemExit("Kiro imports must use an active US proxy; --no-proxy is not allowed")
    if args.minimum_remaining_credits < DEFAULT_MINIMUM_REMAINING_CREDITS:
        raise SystemExit("--minimum-remaining-credits must be at least 10")
    if args.min_interval_min_ms < 0 or args.min_interval_max_ms < args.min_interval_min_ms:
        raise SystemExit("invalid min interval range")
    rng = random.Random(args.seed)

    paths = discover_sqlite_files(args.sqlite_file, args.search_dir)
    if not paths:
        raise SystemExit(f"no SQLite files found; default checked: {DEFAULT_SQLITE}")
    names = build_account_names(paths, args.account_name, args.name_prefix)
    imports = [parse_sqlite(path, name) for path, name in zip(paths, names)]

    proxies: list[dict[str, Any]] = []
    counts: dict[str, int] = {}
    latencies: dict[str, float] = {}
    existing_user_ids: dict[str, str] = {}
    if not args.no_proxy:
        try:
            required_proxy_regions = {
                required_proxy_region_for_auth(auth) for auth in imports
            }
        except ValueError as exc:
            raise SystemExit(str(exc)) from exc
        if len(required_proxy_regions) != 1:
            raise SystemExit(
                "all Kiro imports in one batch must require the same proxy region"
            )
        required_proxy_region = next(iter(required_proxy_regions))
        proxies = filter_required_region_proxies(
            fetch_active_proxies(args.admin_base_url, args.admin_token), required_proxy_region
        )
        if not proxies:
            raise SystemExit(f"no active {required_proxy_region.upper()} proxy configs found")
        accounts = fetch_kiro_accounts(args.admin_base_url, args.admin_token)
        existing_user_ids = existing_user_id_map(accounts)
        for account in accounts:
            if account.get("disabled"):
                continue
            proxy_id = account.get("proxy_config_id")
            if proxy_id:
                counts[str(proxy_id)] = counts.get(str(proxy_id), 0) + 1
        latencies = proxy_latency_by_id(
            fetch_latency_snapshot(args.admin_base_url, args.admin_token)
        )

    results = []
    for auth in imports:
        candidate_proxies = rank_proxies(proxies, counts, latencies, args.balance_penalty_ms)
        min_interval_ms = rng.randint(args.min_interval_min_ms, args.min_interval_max_ms)
        result = import_account(
            auth, args, candidate_proxies, min_interval_ms, existing_user_ids
        )
        selected_proxy_id = result.get("proxy_config_id")
        if selected_proxy_id and (not args.apply or result.get("validated")):
            counts[str(selected_proxy_id)] = counts.get(str(selected_proxy_id), 0) + 1
        balance = result.get("balance")
        user_id = field(balance, "user_id") if isinstance(balance, dict) else None
        if result.get("validated") and user_id is not None:
            existing_user_ids.setdefault(str(user_id), name)
        results.append(result)

    print(
        json.dumps({"count": len(results), "results": results}, ensure_ascii=False, indent=2)
    )
    if args.apply and any(not result.get("validated") for result in results):
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
