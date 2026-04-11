#!/usr/bin/env python3
"""Report one China-time day's Kiro credit usage from llm_gateway_usage_events."""

from __future__ import annotations

import argparse
import json
import math
from collections import defaultdict
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from pathlib import Path
from typing import Iterable
from zoneinfo import ZoneInfo

import lancedb


DEFAULT_DB_PATH = Path("/mnt/wsl/data4tb/static-flow-data/lancedb")
DEFAULT_TABLE = "llm_gateway_usage_events"
DEFAULT_PROVIDER = "kiro"
CHINA_TZ = ZoneInfo("Asia/Shanghai")


@dataclass
class AccountStats:
    credit_total: float = 0.0
    row_count: int = 0
    missing_credit_rows: int = 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Report one Asia/Shanghai calendar day's Kiro credit usage from "
            "llm_gateway_usage_events."
        )
    )
    parser.add_argument("--date", required=True, help="China-time day in YYYY-MM-DD format.")
    parser.add_argument(
        "--db-path",
        default=str(DEFAULT_DB_PATH),
        help=f"LanceDB content DB path. Default: {DEFAULT_DB_PATH}",
    )
    parser.add_argument(
        "--table",
        default=DEFAULT_TABLE,
        help=f"Usage events table name. Default: {DEFAULT_TABLE}",
    )
    parser.add_argument(
        "--provider",
        default=DEFAULT_PROVIDER,
        help=f"Provider filter. Default: {DEFAULT_PROVIDER}",
    )
    parser.add_argument(
        "--contains",
        action="append",
        default=[],
        help="Case-insensitive account-name substring filter. Repeatable.",
    )
    parser.add_argument(
        "--account",
        action="append",
        default=[],
        help="Exact account name to include even if it does not match --contains. Repeatable.",
    )
    parser.add_argument(
        "--format",
        choices=("text", "json"),
        default="text",
        help="Output format. Default: text.",
    )
    return parser.parse_args()


def parse_china_day(value: str) -> date:
    return date.fromisoformat(value)


def china_window(day: date) -> tuple[datetime, datetime]:
    start = datetime.combine(day, time.min, tzinfo=CHINA_TZ)
    end = start + timedelta(days=1)
    return start, end


def utc_iso(ts: datetime) -> str:
    return ts.astimezone(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def sql_quote(value: str) -> str:
    return "'" + value.replace("'", "''") + "'"


def missing_credit(credit_usage: float | None, credit_usage_missing: bool) -> bool:
    return credit_usage_missing or credit_usage is None or not math.isfinite(credit_usage)


def normalize_items(values: Iterable[str]) -> list[str]:
    seen: set[str] = set()
    normalized: list[str] = []
    for raw in values:
        value = raw.strip()
        if not value:
            continue
        lowered = value.lower()
        if lowered in seen:
            continue
        seen.add(lowered)
        normalized.append(value)
    return normalized


def account_matches(account_name: str, contains_terms: list[str], exact_accounts: set[str]) -> bool:
    lowered = account_name.lower()
    if lowered in exact_accounts:
        return True
    return any(term in lowered for term in contains_terms)


def build_result(
    requested_day: str,
    provider: str,
    start_cn: datetime,
    end_cn: datetime,
    contains_filters: list[str],
    exact_accounts_input: list[str],
    account_rows: list[str | None],
    credit_rows: list[float | None],
    missing_rows: list[bool],
) -> dict:
    provider_credit_total = 0.0
    provider_row_count = len(account_rows)
    provider_missing_credit_rows = 0

    contains_terms = [value.lower() for value in contains_filters]
    exact_accounts_lookup = {value.lower() for value in exact_accounts_input}
    selected_account_names: set[str] = set()
    exact_accounts_found: set[str] = set()
    per_account: dict[str, AccountStats] = defaultdict(AccountStats)
    selected_credit_total = 0.0
    selected_row_count = 0
    selected_missing_credit_rows = 0

    for account_name, credit_usage, credit_usage_missing in zip(
        account_rows, credit_rows, missing_rows, strict=True
    ):
        row_is_missing = missing_credit(credit_usage, credit_usage_missing)
        if row_is_missing:
            provider_missing_credit_rows += 1
        else:
            provider_credit_total += float(credit_usage)

        if account_name is None:
            continue

        if not contains_terms and not exact_accounts_lookup:
            continue

        if not account_matches(account_name, contains_terms, exact_accounts_lookup):
            continue

        selected_account_names.add(account_name)
        if account_name.lower() in exact_accounts_lookup:
            exact_accounts_found.add(account_name.lower())

        stats = per_account[account_name]
        stats.row_count += 1
        selected_row_count += 1
        if row_is_missing:
            stats.missing_credit_rows += 1
            selected_missing_credit_rows += 1
        else:
            credit_value = float(credit_usage)
            stats.credit_total += credit_value
            selected_credit_total += credit_value

    zero_usage_exact_accounts = [
        name for name in exact_accounts_input if name.lower() not in exact_accounts_found
    ]

    selected_accounts = []
    for account_name in sorted(selected_account_names):
        stats = per_account[account_name]
        selected_accounts.append(
            {
                "account_name": account_name,
                "credit_total": stats.credit_total,
                "row_count": stats.row_count,
                "missing_credit_rows": stats.missing_credit_rows,
            }
        )

    result = {
        "date": requested_day,
        "timezone": "Asia/Shanghai",
        "window": {
            "china_start": start_cn.isoformat(),
            "china_end": end_cn.isoformat(),
            "utc_start": utc_iso(start_cn),
            "utc_end": utc_iso(end_cn),
        },
        "provider": provider,
        "provider_day": {
            "credit_total": provider_credit_total,
            "row_count": provider_row_count,
            "missing_credit_rows": provider_missing_credit_rows,
            "is_lower_bound": provider_missing_credit_rows > 0,
        },
    }

    if contains_terms or exact_accounts_input:
        result["selection"] = {
            "contains_filters": contains_filters,
            "exact_accounts": exact_accounts_input,
            "matched_account_count": len(selected_accounts),
            "credit_total": selected_credit_total,
            "row_count": selected_row_count,
            "missing_credit_rows": selected_missing_credit_rows,
            "is_lower_bound": selected_missing_credit_rows > 0,
            "accounts": selected_accounts,
            "zero_usage_exact_accounts": zero_usage_exact_accounts,
        }

    return result


def format_credit(total: float, is_lower_bound: bool) -> str:
    prefix = ">= " if is_lower_bound else ""
    return f"{prefix}{total:.6f}"


def print_text(result: dict) -> None:
    window = result["window"]
    provider_day = result["provider_day"]
    print(f"Date: {result['date']}")
    print(f"Timezone: {result['timezone']}")
    print(
        "Window: "
        f"{window['china_start']} to {window['china_end']} "
        f"(UTC {window['utc_start']} to {window['utc_end']})"
    )
    print(f"Provider: {result['provider']}")
    print(f"Provider credit total: {format_credit(provider_day['credit_total'], provider_day['is_lower_bound'])}")
    print(f"Provider row count: {provider_day['row_count']}")
    print(f"Provider missing credit rows: {provider_day['missing_credit_rows']}")

    selection = result.get("selection")
    if selection is None:
        return

    print("")
    print("Selection:")
    if selection["contains_filters"]:
        print(f"  Contains filters: {', '.join(selection['contains_filters'])}")
    if selection["exact_accounts"]:
        print(f"  Exact accounts: {', '.join(selection['exact_accounts'])}")
    print(
        "  Selected credit total: "
        f"{format_credit(selection['credit_total'], selection['is_lower_bound'])}"
    )
    print(f"  Selected row count: {selection['row_count']}")
    print(f"  Selected missing credit rows: {selection['missing_credit_rows']}")
    print(f"  Matched account count: {selection['matched_account_count']}")

    if selection["accounts"]:
        print("  Accounts:")
        for item in selection["accounts"]:
            print(
                "    "
                f"{item['account_name']} | credit_total={item['credit_total']:.6f} "
                f"| rows={item['row_count']} "
                f"| missing_credit_rows={item['missing_credit_rows']}"
            )

    if selection["zero_usage_exact_accounts"]:
        print("  Exact accounts with zero usage:")
        for name in selection["zero_usage_exact_accounts"]:
            print(f"    {name}")


def main() -> int:
    args = parse_args()
    requested_day = parse_china_day(args.date)
    start_cn, end_cn = china_window(requested_day)
    filter_expr = (
        f"provider_type = {sql_quote(args.provider)} "
        f"AND created_at >= timestamp {sql_quote(utc_iso(start_cn))} "
        f"AND created_at < timestamp {sql_quote(utc_iso(end_cn))}"
    )

    table = lancedb.connect(args.db_path).open_table(args.table)
    arrow_table = (
        table.search(None)
        .where(filter_expr)
        .select(["account_name", "credit_usage", "credit_usage_missing"])
        .to_arrow()
    )

    result = build_result(
        requested_day=args.date,
        provider=args.provider,
        start_cn=start_cn,
        end_cn=end_cn,
        contains_filters=normalize_items(args.contains),
        exact_accounts_input=normalize_items(args.account),
        account_rows=arrow_table.column("account_name").to_pylist(),
        credit_rows=arrow_table.column("credit_usage").to_pylist(),
        missing_rows=arrow_table.column("credit_usage_missing").to_pylist(),
    )

    if args.format == "json":
        print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
    else:
        print_text(result)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
