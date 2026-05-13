#!/usr/bin/env python3
import argparse
import base64
import json
import random
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path


VALIDATED_STATUS = "validated"
CODEX_CLIENT_ID = "app_EMoamEEZ73f0CkXaXp7hrann"
CODEX_CLIENT_VERSION = "0.124.0"
CODEX_WIRE_ORIGINATOR = "codex_cli_rs"
CODEX_MODELS_URL = (
    "https://chatgpt.com/backend-api/codex/models"
    f"?client_version={CODEX_CLIENT_VERSION}"
)
CODEX_REFRESH_URL = "https://auth.openai.com/oauth/token"
DEFAULT_CODEX_PROXY_URL = "http://127.0.0.1:11116"
DEFAULT_CODEX_PROBE_TIMEOUT_SECONDS = 30.0


def api_request(base_url: str, method: str, path: str, payload=None):
    url = f"{base_url.rstrip('/')}{path}"
    data = None
    headers = {}
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        headers["Content-Type"] = "application/json"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(request, timeout=120) as response:
            raw = response.read()
            if not raw:
                return None
            return json.loads(raw)
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {path} failed: HTTP {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"{method} {path} failed: {exc}") from exc


def http_json_request(
    url: str,
    method: str,
    payload=None,
    headers=None,
    timeout: float = 120.0,
    proxy_url: str | None = None,
):
    data = None
    request_headers = dict(headers or {})
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
        request_headers.setdefault("Content-Type", "application/json")
    request = urllib.request.Request(
        url, data=data, headers=request_headers, method=method
    )
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler(
            {"http": proxy_url, "https": proxy_url} if proxy_url else {}
        )
    )
    try:
        with opener.open(request, timeout=timeout) as response:
            raw = response.read()
            if not raw:
                return None
            return json.loads(raw)
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise RuntimeError(f"{method} {url} failed: HTTP {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise RuntimeError(f"{method} {url} failed: {exc}") from exc


def optional_string(value):
    if value is None:
        return None
    trimmed = str(value).strip()
    return trimmed or None


def request_auth_from_row(row):
    return {
        "account_id": optional_string(row.get("account_id")),
        "id_token": optional_string(row.get("id_token")),
        "access_token": optional_string(row.get("access_token")),
        "refresh_token": optional_string(row.get("refresh_token")),
    }


def decode_jwt_payload(token: str):
    parts = token.split(".")
    if len(parts) < 2:
        return None
    payload = parts[1]
    payload += "=" * (-len(payload) % 4)
    try:
        decoded = base64.urlsafe_b64decode(payload.encode("ascii"))
        return json.loads(decoded.decode("utf-8"))
    except Exception:  # noqa: BLE001
        return None


def id_token_is_fedramp_account(id_token: str | None) -> bool:
    if not id_token:
        return False
    payload = decode_jwt_payload(id_token)
    if not isinstance(payload, dict):
        return False
    for key in ("https://api.openai.com/auth", "https://chatgpt.com"):
        section = payload.get(key)
        if isinstance(section, dict):
            value = section.get("chatgpt_account_is_fedramp")
            if isinstance(value, bool):
                return value
    return False


def refresh_codex_auth(request_auth, proxy_url: str, timeout_seconds: float):
    refresh_token = optional_string(request_auth.get("refresh_token"))
    if not refresh_token:
        raise RuntimeError("missing refresh_token")
    response = http_json_request(
        CODEX_REFRESH_URL,
        "POST",
        payload={
            "client_id": CODEX_CLIENT_ID,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        },
        headers={"Accept": "application/json"},
        timeout=timeout_seconds,
        proxy_url=proxy_url,
    )
    access_token = optional_string((response or {}).get("access_token"))
    if not access_token:
        raise RuntimeError("refresh response missing access_token")
    return {
        "account_id": optional_string(request_auth.get("account_id")),
        "id_token": optional_string((response or {}).get("id_token"))
        or optional_string(request_auth.get("id_token")),
        "access_token": access_token,
        "refresh_token": optional_string((response or {}).get("refresh_token"))
        or refresh_token,
    }


def probe_codex_models(request_auth, proxy_url: str, timeout_seconds: float):
    access_token = optional_string(request_auth.get("access_token"))
    if not access_token:
        raise RuntimeError("missing access_token")
    headers = {
        "Accept": "application/json",
        "Authorization": f"Bearer {access_token}",
        "User-Agent": f"{CODEX_WIRE_ORIGINATOR}/{CODEX_CLIENT_VERSION}",
        "originator": CODEX_WIRE_ORIGINATOR,
    }
    account_id = optional_string(request_auth.get("account_id"))
    if account_id:
        headers["chatgpt-account-id"] = account_id
    if id_token_is_fedramp_account(optional_string(request_auth.get("id_token"))):
        headers["x-openai-fedramp"] = "true"
    payload = http_json_request(
        CODEX_MODELS_URL,
        "GET",
        headers=headers,
        timeout=timeout_seconds,
        proxy_url=proxy_url,
    )
    models = (payload or {}).get("models")
    if not isinstance(models, list) or not models:
        raise RuntimeError("Codex models response missing non-empty models array")
    return payload


def preflight_account_auth(request_auth, proxy_url: str, timeout_seconds: float):
    auth = request_auth_from_row(request_auth)
    refresh_ok = False
    refresh_error = None
    refreshed_auth = None
    if auth.get("refresh_token"):
        try:
            refreshed_auth = refresh_codex_auth(auth, proxy_url, timeout_seconds)
            refresh_ok = True
        except Exception as exc:  # noqa: BLE001
            refresh_error = str(exc)
    else:
        refresh_error = "missing refresh_token"

    model_errors = []
    model_candidates = []
    if auth.get("access_token"):
        model_candidates.append(("original", auth))
    if refreshed_auth and refreshed_auth.get("access_token"):
        original_access = auth.get("access_token")
        if not original_access or refreshed_auth.get("access_token") != original_access:
            model_candidates.append(("refreshed", refreshed_auth))

    for source, candidate in model_candidates:
        try:
            probe_codex_models(candidate, proxy_url, timeout_seconds)
            return {
                "model_ok": True,
                "model_source": source,
                "refresh_ok": refresh_ok,
                "refresh_error": refresh_error,
            }
        except Exception as exc:  # noqa: BLE001
            model_errors.append(f"{source}: {exc}")

    if not model_errors:
        model_errors.append("missing access_token")
    return {
        "model_ok": False,
        "model_source": None,
        "model_error": "; ".join(model_errors),
        "refresh_ok": refresh_ok,
        "refresh_error": refresh_error,
    }


def choose_proxy(proxy_rows, proxy_counts):
    ordered = sorted(
        proxy_rows,
        key=lambda row: (proxy_counts.get(row["id"], 0), row["name"]),
    )
    return ordered[0]


def log(message: str):
    print(message, flush=True)


def refresh_account_usage(
    api_request_fn,
    admin_base_url: str,
    imported_name: str,
    max_attempts: int = 6,
    retry_delay_seconds: float = 2.0,
):
    path = (
        f"/admin/llm-gateway/accounts/"
        f"{urllib.parse.quote(imported_name, safe='')}/refresh-usage"
    )
    last_error = None
    for attempt in range(1, max_attempts + 1):
        try:
            response = api_request_fn(admin_base_url, "POST", path)
            if response is None:
                raise RuntimeError(f"usage refresh returned empty payload for `{imported_name}`")
            response["usage_refresh_attempts"] = attempt
            return {
                "ok": True,
                "attempts": attempt,
                "response": response,
                "error": None,
            }
        except Exception as exc:  # noqa: BLE001
            last_error = str(exc)
            if attempt < max_attempts:
                if retry_delay_seconds > 0:
                    time.sleep(retry_delay_seconds)
                continue
            break
    return {
        "ok": False,
        "attempts": max_attempts,
        "response": None,
        "error": last_error
        or f"usage refresh for `{imported_name}` failed without an error message",
    }


def build_usage_refresh_result_fields(usage_refresh):
    response = usage_refresh.get("response") or {}
    return {
        "usage_refresh_ok": bool(usage_refresh.get("ok")),
        "usage_refresh_attempts": usage_refresh.get("attempts"),
        "usage_refresh_error": usage_refresh.get("error"),
        "usage_refresh_status": response.get("status"),
        "usage_refresh_last_refresh": response.get("last_refresh"),
        "usage_refresh_last_usage_checked_at": response.get("last_usage_checked_at"),
        "usage_refresh_last_usage_success_at": response.get("last_usage_success_at"),
        "usage_refresh_usage_error_message": response.get("usage_error_message"),
        # Keep legacy field names for existing result consumers.
        "refresh_status": response.get("status"),
        "refresh_attempts": usage_refresh.get("attempts"),
        "refresh_last_refresh": response.get("last_refresh"),
        "refresh_last_usage_checked_at": response.get("last_usage_checked_at"),
        "refresh_last_usage_success_at": response.get("last_usage_success_at"),
        "refresh_usage_error_message": response.get("usage_error_message")
        if response
        else usage_refresh.get("error"),
    }


def build_plan(pending_rows, proxy_rows, proxy_counts, rng, args):
    plan = []
    local_counts = dict(proxy_counts)
    for row in pending_rows:
        proxy = choose_proxy(proxy_rows, local_counts)
        interval_ms = rng.randint(args.interval_min, args.interval_max)
        plan.append(
            {
                "request_id": row["request_id"],
                "account_name": row["account_name"],
                "selected_proxy_id": proxy["id"],
                "selected_proxy_name": proxy["name"],
                "request_max_concurrency": args.request_max_concurrency,
                "request_min_start_interval_ms": interval_ms,
            }
        )
        local_counts[proxy["id"]] = local_counts.get(proxy["id"], 0) + 1
    return plan


def process_plan_item(index, total, item, args, request_auths_by_id):
    result = dict(item)
    progress = f"[{index}/{total}] {item['account_name']}"
    request_auth = request_auths_by_id.get(item["request_id"])
    if request_auth is None:
        raise RuntimeError(f"missing request auth for `{item['request_id']}`")
    preflight = preflight_account_auth(
        request_auth,
        proxy_url=args.codex_proxy_url,
        timeout_seconds=args.codex_probe_timeout_seconds,
    )
    result["preflight_model_ok"] = preflight.get("model_ok")
    result["preflight_model_source"] = preflight.get("model_source")
    result["preflight_model_error"] = preflight.get("model_error")
    result["preflight_refresh_ok"] = preflight.get("refresh_ok")
    result["preflight_refresh_error"] = preflight.get("refresh_error")
    auto_refresh_enabled = bool(preflight.get("refresh_ok"))
    result["selected_auto_refresh_enabled"] = auto_refresh_enabled
    log(
        f"{progress} preflight model_ok={preflight.get('model_ok')} "
        f"model_source={preflight.get('model_source')} "
        f"refresh_ok={preflight.get('refresh_ok')} "
        f"auto_refresh_enabled={auto_refresh_enabled}"
    )
    if not preflight.get("model_ok"):
        failure_reason = preflight.get("model_error") or "Codex models preflight failed"
        result.update(
            {
                "preflight_failed": True,
                "preflight_failure_reason": failure_reason,
                "validated_status": "skipped",
                "validation_failed": True,
                "validation_failure_reason": failure_reason,
            }
        )
        log(f"{progress} preflight failed reason={failure_reason}")
        return result

    result["preflight_failed"] = False
    log(f"{progress} validate request={item['request_id']}")
    validated = api_request(
        args.admin_base_url,
        "POST",
        f"/admin/llm-gateway/account-contribution-requests/"
        f"{urllib.parse.quote(item['request_id'], safe='')}/validate",
        {"admin_note": args.admin_note},
    )
    result["validated_status"] = validated.get("status")
    if validated.get("status") != VALIDATED_STATUS:
        result.update(
            {
                "validation_failed": True,
                "validation_failure_reason": validated.get("failure_reason"),
            }
        )
        log(
            f"{progress} validation status={validated.get('status')} "
            f"reason={validated.get('failure_reason')}"
        )
        return result

    log(f"{progress} approve-and-issue")
    issued = api_request(
        args.admin_base_url,
        "POST",
        f"/admin/llm-gateway/account-contribution-requests/"
        f"{urllib.parse.quote(item['request_id'], safe='')}/approve-and-issue",
        {"admin_note": args.admin_note},
    )
    imported_name = issued.get("imported_account_name") or item["account_name"]
    log(
        f"{progress} patch account={imported_name} proxy={item['selected_proxy_name']} "
        f"concurrency={item['request_max_concurrency']} "
        f"interval_ms={item['request_min_start_interval_ms']}"
    )
    patched = api_request(
        args.admin_base_url,
        "PATCH",
        f"/admin/llm-gateway/accounts/{urllib.parse.quote(imported_name, safe='')}",
        {
            "auto_refresh_enabled": auto_refresh_enabled,
            "proxy_mode": "fixed",
            "proxy_config_id": item["selected_proxy_id"],
            "request_max_concurrency": item["request_max_concurrency"],
            "request_min_start_interval_ms": item["request_min_start_interval_ms"],
        },
    )
    log(
        f"{progress} refresh usage account={imported_name} "
        f"attempts<={args.refresh_max_attempts}"
    )
    usage_refresh = refresh_account_usage(
        api_request_fn=api_request,
        admin_base_url=args.admin_base_url,
        imported_name=imported_name,
        max_attempts=args.refresh_max_attempts,
        retry_delay_seconds=args.refresh_retry_delay_seconds,
    )
    result.update(
        {
            "validation_failed": False,
            "issued_status": issued.get("status"),
            "imported_account_name": imported_name,
            "issued_key_id": issued.get("issued_key_id"),
            "issued_key_name": issued.get("issued_key_name"),
            "patched_proxy_mode": patched.get("proxy_mode"),
            "patched_proxy_config_id": patched.get("proxy_config_id"),
            "patched_request_max_concurrency": patched.get("request_max_concurrency"),
            "patched_request_min_start_interval_ms": patched.get(
                "request_min_start_interval_ms"
            ),
            "patched_auto_refresh_enabled": patched.get("auto_refresh_enabled"),
        }
    )
    result.update(build_usage_refresh_result_fields(usage_refresh))
    log(
        f"{progress} done imported={imported_name} "
        f"usage_refresh_ok={usage_refresh.get('ok')} "
        f"last_usage_success_at={result.get('usage_refresh_last_usage_success_at')}"
    )
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--admin-base-url", required=True)
    parser.add_argument("--account-prefix", required=True)
    parser.add_argument("--expected-count", type=int, required=True)
    parser.add_argument("--request-status", choices=["pending", "failed"], default="pending")
    parser.add_argument("--admin-note", default="batch validate and issue")
    parser.add_argument("--request-max-concurrency", type=int, default=3)
    parser.add_argument("--interval-min", type=int, default=100)
    parser.add_argument("--interval-max", type=int, default=1000)
    parser.add_argument("--refresh-max-attempts", type=int, default=6)
    parser.add_argument("--refresh-retry-delay-seconds", type=float, default=2.0)
    parser.add_argument("--codex-proxy-url", default=DEFAULT_CODEX_PROXY_URL)
    parser.add_argument(
        "--codex-probe-timeout-seconds",
        type=float,
        default=DEFAULT_CODEX_PROBE_TIMEOUT_SECONDS,
    )
    parser.add_argument("--seed", type=int)
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()

    if args.expected_count <= 0:
        raise SystemExit("expected-count must be positive")
    if args.interval_min < 0 or args.interval_max < args.interval_min:
        raise SystemExit("invalid interval range")
    if args.refresh_max_attempts <= 0:
        raise SystemExit("refresh-max-attempts must be positive")
    if args.refresh_retry_delay_seconds < 0:
        raise SystemExit("refresh-retry-delay-seconds must be >= 0")
    if args.codex_probe_timeout_seconds <= 0:
        raise SystemExit("codex-probe-timeout-seconds must be positive")

    rng = random.Random(args.seed)
    requests_payload = api_request(
        args.admin_base_url,
        "GET",
        "/admin/llm-gateway/account-contribution-requests?"
        f"status={urllib.parse.quote(args.request_status, safe='')}&limit=500",
    )
    pending = [
        row
        for row in requests_payload["requests"]
        if row["status"] == args.request_status
        and row["account_name"].startswith(args.account_prefix)
    ]
    pending.sort(key=lambda row: row["account_name"])

    if len(pending) != args.expected_count:
        raise SystemExit(
            f"expected {args.expected_count} {args.request_status} requests "
            f"for prefix {args.account_prefix}, "
            f"found {len(pending)}"
        )
    request_auths_by_id = {
        row["request_id"]: request_auth_from_row(row)
        for row in requests_payload["requests"]
        if row["request_id"] in {item["request_id"] for item in pending}
    }

    accounts_payload = api_request(
        args.admin_base_url,
        "GET",
        "/admin/llm-gateway/accounts?provider_type=codex&limit=500",
    )
    proxy_payload = api_request(args.admin_base_url, "GET", "/admin/llm-gateway/proxy-configs")
    proxy_rows = [row for row in proxy_payload["proxy_configs"] if row["status"] == "active"]
    if not proxy_rows:
        raise SystemExit("no active proxy configs found")

    proxy_counts = {}
    for account in accounts_payload["accounts"]:
        if account.get("status") != "active":
            continue
        proxy_id = account.get("proxy_config_id")
        if not proxy_id:
            continue
        proxy_counts[proxy_id] = proxy_counts.get(proxy_id, 0) + 1

    timestamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime())
    result_path = Path(f"/tmp/llm-gateway-account-batch-approve-{timestamp}.json")
    plan = build_plan(pending, proxy_rows, proxy_counts, rng, args)

    if not args.apply:
        result_path.write_text(
            json.dumps(
                {
                    "mode": "dry-run",
                    "admin_base_url": args.admin_base_url,
                    "account_prefix": args.account_prefix,
                    "request_status": args.request_status,
                    "expected_count": args.expected_count,
                    "refresh_max_attempts": args.refresh_max_attempts,
                    "refresh_retry_delay_seconds": args.refresh_retry_delay_seconds,
                    "codex_proxy_url": args.codex_proxy_url,
                    "codex_probe_timeout_seconds": args.codex_probe_timeout_seconds,
                    "plan": plan,
                },
                ensure_ascii=False,
                indent=2,
            )
        )
        log(f"plan_file: {result_path}")
        log(
            json.dumps(
                {
                    "mode": "dry-run",
                    "planned": len(plan),
                    "refresh_max_attempts": args.refresh_max_attempts,
                    "codex_probe_timeout_seconds": args.codex_probe_timeout_seconds,
                },
                ensure_ascii=False,
            )
        )
        return

    results = []
    failures = []
    for index, item in enumerate(plan, start=1):
        try:
            results.append(
                process_plan_item(index, len(plan), item, args, request_auths_by_id)
            )
        except Exception as exc:  # noqa: BLE001
            result = dict(item)
            result["error"] = str(exc)
            failures.append(result)
            results.append(result)
            log(f"[{index}/{len(plan)}] {item['account_name']} failed error={exc}")

    preflight_failed_count = sum(1 for result in results if result.get("preflight_failed"))
    validation_failed_count = sum(1 for result in results if result.get("validation_failed"))
    issued_count = sum(1 for result in results if result.get("issued_status") == "issued")
    usage_refresh_failed_count = sum(
        1
        for result in results
        if result.get("issued_status") == "issued" and not result.get("usage_refresh_ok", True)
    )
    result_path.write_text(
        json.dumps(
            {
                "mode": "apply",
                "admin_base_url": args.admin_base_url,
                "account_prefix": args.account_prefix,
                "request_status": args.request_status,
                "expected_count": args.expected_count,
                "refresh_max_attempts": args.refresh_max_attempts,
                "refresh_retry_delay_seconds": args.refresh_retry_delay_seconds,
                "codex_proxy_url": args.codex_proxy_url,
                "codex_probe_timeout_seconds": args.codex_probe_timeout_seconds,
                "results": results,
                "failure_count": len(failures),
                "preflight_failed_count": preflight_failed_count,
                "validation_failed_count": validation_failed_count,
                "issued_count": issued_count,
                "usage_refresh_failed_count": usage_refresh_failed_count,
            },
            ensure_ascii=False,
            indent=2,
        )
    )
    log(f"result_file: {result_path}")
    log(
        json.dumps(
            {
                "mode": "apply",
                "processed": len(results),
                "failures": len(failures),
                "preflight_failed": preflight_failed_count,
                "validation_failed": validation_failed_count,
                "issued": issued_count,
                "usage_refresh_failed": usage_refresh_failed_count,
            },
            ensure_ascii=False,
        )
    )
    if failures:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
