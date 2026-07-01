import importlib.util
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path


SCRIPT = (
    Path(__file__).resolve().parents[1]
    / "scripts"
    / "onboard_kiro_social_github.py"
)
SPEC = importlib.util.spec_from_file_location("onboard_kiro_social_github", SCRIPT)
module = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
sys.modules[SPEC.name] = module
SPEC.loader.exec_module(module)


class KiroSocialGithubOnboarderTest(unittest.TestCase):
    def test_device_authorization_requests_github_provider(self):
        calls = []

        def fake_open_json(method, url, body=None, **kwargs):
            calls.append((method, url, body, kwargs))
            return {
                "deviceCode": "device-code",
                "verificationUriComplete": "https://kiro.example/verify",
            }

        original = module.open_json
        module.open_json = fake_open_json
        try:
            result = module.start_device_authorization("http://127.0.0.1:11111", "kiro-cli")
        finally:
            module.open_json = original

        self.assertEqual(result["deviceCode"], "device-code")
        self.assertEqual(len(calls), 1)
        method, url, body, kwargs = calls[0]
        self.assertEqual(method, "POST")
        self.assertTrue(url.endswith("/oauth/device/authorization"))
        self.assertEqual(body, {"clientId": "kiro-cli", "loginProvider": "Github"})
        self.assertEqual(kwargs["proxy"], "http://127.0.0.1:11111")

    def test_write_social_token_marks_provider_github_without_returning_raw_tokens(self):
        with tempfile.TemporaryDirectory() as tmp:
            db_path = Path(tmp) / "data.sqlite3"
            conn = sqlite3.connect(db_path)
            conn.execute("CREATE TABLE auth_kv (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            conn.execute("CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
            conn.commit()
            conn.close()

            written = module.write_social_token(
                db_path,
                {
                    "accessToken": "access-token-value",
                    "refreshToken": "refresh-token-value",
                    "profileArn": "arn:aws:codewhisperer:us-east-1:123:profile/test",
                },
            )

            self.assertEqual(written["provider"], "github")
            self.assertEqual(written["profile_arn"], "arn:aws:codewhisperer:us-east-1:123:profile/test")
            self.assertNotIn("access-token-value", json.dumps(written))
            self.assertNotIn("refresh-token-value", json.dumps(written))

            conn = sqlite3.connect(db_path)
            try:
                row = conn.execute(
                    "SELECT value FROM auth_kv WHERE key = ?",
                    (module.SOCIAL_TOKEN_KEY,),
                ).fetchone()
            finally:
                conn.close()
            self.assertIsNotNone(row)
            token = json.loads(row[0])
            self.assertEqual(token["provider"], "github")
            self.assertEqual(token["access_token"], "access-token-value")
            self.assertEqual(token["refresh_token"], "refresh-token-value")

    def test_parse_args_accepts_optional_github_login_without_password_argument(self):
        args = module.parse_args(["--account-name", "kiro-gh", "--github-login", "gfryuuu"])

        self.assertEqual(args.account_name, "kiro-gh")
        self.assertEqual(args.github_login, "gfryuuu")
        self.assertEqual(args.password_env, "KIRO_GITHUB_PASSWORD")
        self.assertEqual(args.manual_timeout_seconds, 600)

    def test_parse_args_allows_omitted_account_name_for_auto_match(self):
        args = module.parse_args([])

        self.assertIsNone(args.account_name)
        self.assertFalse(args.replace_account)

    def test_browser_helper_starts_without_provider_credentials_by_default(self):
        calls = []

        class FakeProcess:
            pass

        def fake_which(name):
            return "/usr/bin/node" if name == "node" else None

        def fake_popen(cmd, env, **kwargs):
            calls.append((cmd, env, kwargs))
            return FakeProcess()

        args = module.parse_args(["--account-name", "kiro-gh"])
        original_which = module.shutil.which
        original_popen = module.subprocess.Popen
        module.shutil.which = fake_which
        module.subprocess.Popen = fake_popen
        try:
            process = module.start_device_flow_helper(args, 9222)
        finally:
            module.shutil.which = original_which
            module.subprocess.Popen = original_popen

        self.assertIsInstance(process, FakeProcess)
        self.assertEqual(len(calls), 1)
        cmd, env, _kwargs = calls[0]
        self.assertEqual(cmd, ["node", str(module.NODE_DRIVER)])
        self.assertEqual(env["KIRO_DEVTOOLS_PORT"], "9222")
        self.assertEqual(env["KIRO_MANUAL_TIMEOUT_SECONDS"], "600")
        self.assertNotIn("KIRO_GOOGLE_PASSWORD", env)
        self.assertNotIn("KIRO_GITHUB_PASSWORD", env)
        self.assertNotIn("KIRO_GITHUB_LOGIN", env)

    def test_browser_helper_passes_github_login_and_password_via_environment_only(self):
        calls = []

        class FakeProcess:
            pass

        def fake_which(name):
            return "/usr/bin/node" if name == "node" else None

        def fake_popen(cmd, env, **kwargs):
            calls.append((cmd, env, kwargs))
            return FakeProcess()

        args = module.parse_args(
            [
                "--account-name",
                "kiro-gh",
                "--github-login",
                "gfryuuu",
                "--password-env",
                "KIRO_TEST_GITHUB_PASSWORD",
            ]
        )
        original_which = module.shutil.which
        original_popen = module.subprocess.Popen
        old_password = module.os.environ.get("KIRO_TEST_GITHUB_PASSWORD")
        module.shutil.which = fake_which
        module.subprocess.Popen = fake_popen
        module.os.environ["KIRO_TEST_GITHUB_PASSWORD"] = "secret-password"
        try:
            password = module.resolve_github_password(args, "gfryuuu")
            process = module.start_device_flow_helper(args, 9222, password)
        finally:
            module.shutil.which = original_which
            module.subprocess.Popen = original_popen
            if old_password is None:
                module.os.environ.pop("KIRO_TEST_GITHUB_PASSWORD", None)
            else:
                module.os.environ["KIRO_TEST_GITHUB_PASSWORD"] = old_password

        self.assertIsInstance(process, FakeProcess)
        self.assertEqual(len(calls), 1)
        cmd, env, _kwargs = calls[0]
        self.assertEqual(cmd, ["node", str(module.NODE_DRIVER)])
        self.assertNotIn("secret-password", cmd)
        self.assertEqual(env["KIRO_GITHUB_LOGIN"], "gfryuuu")
        self.assertEqual(env["KIRO_GITHUB_PASSWORD"], "secret-password")

    def test_fetch_kiro_accounts_paginates_past_server_limit(self):
        calls = []

        def fake_admin_json(method, base_url, path, body=None, token=None):
            calls.append((method, base_url, path, body, token))
            if "offset=0" in path:
                return {
                    "accounts": [{"name": "first"}],
                    "limit": 1,
                    "offset": 0,
                    "has_more": True,
                }
            if "offset=1" in path:
                return {
                    "accounts": [{"name": "gfryuuu"}],
                    "limit": 1,
                    "offset": 1,
                    "has_more": False,
                }
            self.fail(f"unexpected path: {path}")

        original = module.admin_json
        module.admin_json = fake_admin_json
        try:
            accounts = module.fetch_kiro_accounts("http://admin", "token")
        finally:
            module.admin_json = original

        self.assertEqual([account["name"] for account in accounts], ["first", "gfryuuu"])
        self.assertEqual(len(calls), 2)

    def test_select_existing_account_prefers_importer_duplicate(self):
        probe = {
            "results": [
                {
                    "name": "kiro-refresh-probe-1",
                    "duplicate_user_id": "user-1",
                    "duplicate_of": "existing-kiro",
                }
            ]
        }

        matched = module.select_existing_account_name_from_probe(probe, [])

        self.assertEqual(matched, "existing-kiro")

    def test_select_existing_account_matches_balance_user_id(self):
        probe = {
            "results": [
                {
                    "name": "kiro-refresh-probe-1",
                    "validated": True,
                    "balance": {"user_id": "user-2"},
                }
            ]
        }
        accounts = [
            {"name": "other", "balance": {"user_id": "user-1"}},
            {"name": "target", "upstream_user_id": "user-2"},
        ]

        matched = module.select_existing_account_name_from_probe(probe, accounts)

        self.assertEqual(matched, "target")

    def test_select_existing_account_ignores_probe_account(self):
        probe = {
            "results": [
                {
                    "name": "kiro-refresh-probe-1",
                    "validated": True,
                    "balance": {"user_id": "user-2"},
                }
            ]
        }
        accounts = [
            {"name": "kiro-refresh-probe-1", "upstream_user_id": "user-2"},
        ]

        matched = module.select_existing_account_name_from_probe(
            probe,
            accounts,
            exclude_names={"kiro-refresh-probe-1"},
        )

        self.assertIsNone(matched)

    def test_select_existing_account_ignores_probe_duplicate(self):
        probe = {
            "results": [
                {
                    "name": "kiro-refresh-probe-1",
                    "duplicate_of": "kiro-refresh-probe-1",
                }
            ]
        }

        matched = module.select_existing_account_name_from_probe(
            probe,
            [],
            exclude_names={"kiro-refresh-probe-1"},
        )

        self.assertIsNone(matched)

    def test_parse_whoami_email_extracts_email(self):
        output = """Logged in with GitHub
Email: unsmore@utexas.edu
User ID: d-123
"""

        self.assertEqual(module.parse_whoami_email(output), "unsmore@utexas.edu")

    def test_parse_whoami_email_returns_none_when_absent(self):
        output = """Logged in with GitHub
User ID: d-123
"""

        self.assertIsNone(module.parse_whoami_email(output))

    def test_build_existing_account_import_body_preserves_settings(self):
        current = {
            "name": "existing-kiro",
            "auth_method": "social",
            "provider": "github",
            "profile_arn": "old-profile",
            "region": "us-east-1",
            "auth_region": "us-east-1",
            "api_region": "us-east-1",
            "kiro_channel_max_concurrency": 4,
            "kiro_channel_min_start_interval_ms": 250,
            "minimum_remaining_credits_before_block": 10.0,
            "manual_usage_limit": 900.0,
            "pool_strategy": "balanced",
            "disabled": False,
            "subscription_title": "KIRO STUDENT",
            "source_db_path": "/old.sqlite3",
        }
        token_payload = {
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "profileArn": "new-profile",
        }

        body = module.build_existing_account_import_body(
            current,
            token_payload,
            expires_at="2030-01-01T00:00:00Z",
            source_db_path=Path("/tmp/data.sqlite3"),
        )

        self.assertEqual(body["name"], "existing-kiro")
        self.assertEqual(body["access_token"], "new-access")
        self.assertEqual(body["refresh_token"], "new-refresh")
        self.assertEqual(body["profile_arn"], "new-profile")
        self.assertEqual(body["provider"], "github")
        self.assertEqual(body["kiro_channel_max_concurrency"], 4)
        self.assertEqual(body["kiro_channel_min_start_interval_ms"], 250)
        self.assertEqual(body["minimum_remaining_credits_before_block"], 10.0)
        self.assertEqual(body["manual_usage_limit"], 900.0)
        self.assertEqual(body["pool_strategy"], "balanced")
        self.assertEqual(body["source_db_path"], "/tmp/data.sqlite3")
        self.assertFalse(body["disabled"])

    def test_build_existing_account_import_body_records_whoami_email(self):
        current = {"name": "existing-kiro"}
        token_payload = {
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "profileArn": "new-profile",
        }

        body = module.build_existing_account_import_body(
            current,
            token_payload,
            expires_at="2030-01-01T00:00:00Z",
            source_db_path=Path("/tmp/data.sqlite3"),
            email="unsmore@utexas.edu",
        )

        self.assertEqual(body["email"], "unsmore@utexas.edu")

    def test_build_existing_account_import_body_preserves_existing_email_without_override(self):
        current = {"name": "existing-kiro", "email": "existing@example.edu"}
        token_payload = {
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "profileArn": "new-profile",
        }

        body = module.build_existing_account_import_body(
            current,
            token_payload,
            expires_at="2030-01-01T00:00:00Z",
            source_db_path=Path("/tmp/data.sqlite3"),
        )

        self.assertEqual(body["email"], "existing@example.edu")

    def test_build_existing_account_import_body_reactivates_refresh_token_failures(self):
        current = {
            "name": "existing-kiro",
            "disabled": True,
            "disabled_reason": "invalid_refresh_token",
        }
        token_payload = {
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "profileArn": "new-profile",
        }

        body = module.build_existing_account_import_body(
            current,
            token_payload,
            expires_at="2030-01-01T00:00:00Z",
            source_db_path=Path("/tmp/data.sqlite3"),
        )

        self.assertFalse(body["disabled"])

    def test_build_existing_account_import_body_preserves_manual_disabled(self):
        current = {
            "name": "existing-kiro",
            "disabled": True,
            "disabled_reason": "manually disabled for maintenance",
        }
        token_payload = {
            "accessToken": "new-access",
            "refreshToken": "new-refresh",
            "profileArn": "new-profile",
        }

        body = module.build_existing_account_import_body(
            current,
            token_payload,
            expires_at="2030-01-01T00:00:00Z",
            source_db_path=Path("/tmp/data.sqlite3"),
        )

        self.assertTrue(body["disabled"])


if __name__ == "__main__":
    unittest.main()
