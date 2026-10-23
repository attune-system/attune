"""Exercise the policy script with owned source fixtures and no ripgrep."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import unittest


class DatabaseTestIgnorePolicyTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        scripts = self.root / "scripts"
        scripts.mkdir()
        self.script = scripts / "database-test-ignore-policy.sh"
        shutil.copyfile(Path(__file__).with_name(self.script.name), self.script)
        self.source = self.root / "crates/api/tests/fixture.rs"
        self.source.parent.mkdir(parents=True)
        self.source.write_text("#[test]\nfn database_test() {}\n")
        binaries = self.root / "bin"
        binaries.mkdir()
        for name in ["python3", "dirname"]:
            (binaries / name).symlink_to(shutil.which(name))
        self.environment = dict(os.environ, PATH=str(binaries))
        self.bash = shutil.which("bash")

    def run_policy(self, mode="--check"):
        return subprocess.run(
            [self.bash, str(self.script), mode], env=self.environment,
            text=True, capture_output=True, timeout=10,
        )

    def test_clean_scan_without_ripgrep(self):
        result = self.run_policy()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_rejects_database_only_ignores(self):
        for annotation in [
            '#[ignore = "integration test fixture requires database"]',
            '#[ignore = "integration test - requires PostgreSQL"]',
            '#[ignore = "requires disposable PostgreSQL"]',
            '#[ignore = "e2e test requires PostgreSQL"]',
            '#[ignore] // Requires database',
        ]:
            with self.subTest(annotation=annotation):
                self.source.write_text(f"#[test]\n  {annotation}\nfn database_test() {{}}\n")
                result = self.run_policy()
                self.assertEqual(result.returncode, 1, result.stderr)
                self.assertIn(f"{self.source}:2:", result.stdout)

    def test_fix_preserves_external_service_and_deferred_ignores(self):
        self.source.write_text(
            '#[ignore = "requires disposable PostgreSQL"]\n'
            '#[ignore = "requires RustFS"]\n'
            '#[ignore = "deferred deployment scenario"]\n'
        )
        webhook = self.source.with_name("webhook_security_tests.rs")
        webhook.write_text("#[test]\n#[ignore]\nfn webhook() {}\n")
        excluded = self.root / "crates/common/src/repositories/pack_test.rs"
        excluded.parent.mkdir(parents=True)
        excluded.write_text('#[ignore = "requires disposable PostgreSQL"]\n')
        result = self.run_policy("--fix")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.source.read_text(),
                         '#[ignore = "requires RustFS"]\n'
                         '#[ignore = "deferred deployment scenario"]\n')
        self.assertEqual(webhook.read_text(), "#[test]\nfn webhook() {}\n")
        self.assertIn("#[ignore", excluded.read_text())
        self.assertEqual(self.run_policy().returncode, 0)

    def test_source_read_error_fails_closed(self):
        self.source.write_bytes(b"\xff")
        result = self.run_policy()
        self.assertEqual(result.returncode, 2)
        self.assertIn("cannot scan", result.stderr)

    def test_missing_source_directory_fails_closed(self):
        self.source.unlink()
        self.source.parent.rmdir()
        self.source.parent.parent.rmdir()
        self.source.parent.parent.parent.rmdir()
        result = self.run_policy()
        self.assertEqual(result.returncode, 2)
        self.assertIn("source directory is missing", result.stderr)

    def test_invalid_mode(self):
        self.assertEqual(self.run_policy("--unknown").returncode, 2)


if __name__ == "__main__":
    unittest.main()
