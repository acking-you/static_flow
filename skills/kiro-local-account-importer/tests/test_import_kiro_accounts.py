import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "import_kiro_accounts.py"
SPEC = importlib.util.spec_from_file_location("import_kiro_accounts", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


class ImportKiroAccountsTest(unittest.TestCase):
    def test_us_proxy_filter_keeps_only_us_nodes(self):
        proxies = [
            {"id": "sg", "name": "aws_sg1"},
            {"id": "aws-us", "name": "aws_us3_east1"},
            {"id": "do-us", "name": "do-us-1"},
            {"id": "home-us", "name": "us-home1"},
            {"id": "homeus", "name": "my-homeus2"},
            {"id": "dmit", "name": "dmit-us"},
            {"id": "ae", "name": "azure-ae"},
            {"id": "sg-proxy", "name": "proxy_to_sg"},
        ]

        filtered = module.filter_required_region_proxies(proxies, "us")

        self.assertEqual(
            [proxy["name"] for proxy in filtered],
            ["aws_us3_east1", "do-us-1", "us-home1", "my-homeus2", "dmit-us"],
        )

    def test_default_minimum_remaining_credits_is_ten(self):
        args = module.parse_args([])

        self.assertEqual(args.minimum_remaining_credits, 10.0)

    def test_balance_must_refresh_with_minimum_remaining(self):
        self.assertTrue(
            module.balance_has_minimum_remaining({"remaining": 10.0}, minimum_remaining=10.0)
        )
        self.assertFalse(
            module.balance_has_minimum_remaining({"remaining": 9.99}, minimum_remaining=10.0)
        )
        self.assertFalse(module.balance_has_minimum_remaining({}, minimum_remaining=10.0))

    def test_failed_validated_import_is_deleted(self):
        calls = []

        def fake_request_json(method, base_url, path, token, body=None, timeout=30.0):
            calls.append((method, path, body))
            if method == "POST" and path == "/admin/kiro-gateway/accounts":
                return {"name": "kiro-bad"}
            if method == "PATCH":
                return {"name": "kiro-bad"}
            if method == "POST" and path == "/admin/kiro-gateway/accounts/kiro-bad/balance":
                return {"remaining": 5.0, "usage_limit": 100.0, "current_usage": 95.0}
            if method == "DELETE":
                return {"status": "ok"}
            raise AssertionError((method, path, body))

        original = module.request_json
        module.request_json = fake_request_json
        try:
            args = module.parse_args(["--apply"])
            auth = module.ImportedAuth(
                name="kiro-bad",
                sqlite_path=Path("/tmp/kiro.sqlite3"),
                body={"name": "kiro-bad", "minimum_remaining_credits_before_block": 10.0},
            )
            result = module.import_account(
                auth,
                args,
                proxies=[{"id": "us-proxy-1", "name": "do-us-1"}],
                min_interval_ms=337,
            )
        finally:
            module.request_json = original

        self.assertFalse(result["validated"])
        self.assertTrue(result["deleted"])
        self.assertIn(("DELETE", "/admin/kiro-gateway/accounts/kiro-bad", None), calls)

    def test_duplicate_user_id_import_is_deleted(self):
        calls = []

        def fake_request_json(method, base_url, path, token, body=None, timeout=30.0):
            calls.append((method, path, body))
            if method == "POST" and path == "/admin/kiro-gateway/accounts":
                return {"name": "kiro-duplicate"}
            if method == "PATCH":
                return {"name": "kiro-duplicate"}
            if method == "POST" and path == "/admin/kiro-gateway/accounts/kiro-duplicate/balance":
                return {
                    "remaining": 1000.0,
                    "usage_limit": 1000.0,
                    "current_usage": 0.0,
                    "user_id": "upstream-1",
                }
            if method == "DELETE":
                return {"status": "ok"}
            raise AssertionError((method, path, body))

        original = module.request_json
        module.request_json = fake_request_json
        try:
            args = module.parse_args(["--apply"])
            auth = module.ImportedAuth(
                name="kiro-duplicate",
                sqlite_path=Path("/tmp/kiro.sqlite3"),
                body={"name": "kiro-duplicate", "minimum_remaining_credits_before_block": 10.0},
            )
            result = module.import_account(
                auth,
                args,
                proxies=[{"id": "us-proxy-1", "name": "do-us-1"}],
                min_interval_ms=337,
                existing_user_ids={"upstream-1": "kiro-existing"},
            )
        finally:
            module.request_json = original

        self.assertFalse(result["validated"])
        self.assertTrue(result["deleted"])
        self.assertEqual(result["duplicate_of"], "kiro-existing")
        self.assertEqual(result["duplicate_user_id"], "upstream-1")
        self.assertIn(("DELETE", "/admin/kiro-gateway/accounts/kiro-duplicate", None), calls)


if __name__ == "__main__":
    unittest.main()
