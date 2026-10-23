"""Exercise already-built CLI binaries without credentials or a server."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parent.parent
VERSION = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]


def windows_sized_stack():
    # Linux reproduces Windows' 1 MiB main-thread limit without changing binaries.
    import resource

    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    resource.setrlimit(resource.RLIMIT_STACK, (1024 * 1024, 1024 * 1024))


class CliStartupTests(unittest.TestCase):
    binary_dir: Path
    binaries: list[str]

    def setUp(self):
        self.home = tempfile.TemporaryDirectory(prefix="attune-cli-startup-")
        self.addCleanup(self.home.cleanup)

    def run_cli(self, binary, *arguments):
        suffix = ".exe" if sys.platform == "win32" else ""
        executable = self.binary_dir / f"{binary}{suffix}"
        self.assertTrue(executable.is_file(), f"Build the binary first: {executable}")
        environment = {
            name: os.environ[name]
            for name in ("PATH", "SystemRoot", "WINDIR", "TEMP", "TMP")
            if name in os.environ
        }
        for name in ("HOME", "USERPROFILE", "XDG_CONFIG_HOME", "APPDATA", "LOCALAPPDATA"):
            environment[name] = self.home.name
        result = subprocess.run(
            [str(executable), *arguments],
            cwd=self.home.name,
            env=environment,
            capture_output=True,
            text=True,
            timeout=30,
            preexec_fn=windows_sized_stack if sys.platform == "linux" else None,
        )
        self.assertEqual(result.returncode, 0, f"{binary} {arguments}: {result.stderr}")
        self.assertEqual(list(Path(self.home.name).iterdir()), [], "Offline startup created user configuration")
        return result.stdout

    def test_versions(self):
        for binary in self.binaries:
            with self.subTest(binary=binary):
                self.assertEqual(self.run_cli(binary, "--version").strip(), f"{binary} {VERSION}")

    def test_help(self):
        for binary in self.binaries:
            with self.subTest(binary=binary):
                self.assertIn("Usage:", self.run_cli(binary, "--help"))

    def test_local_info(self):
        for binary in self.binaries:
            with self.subTest(binary=binary):
                arguments = ("--json", "info", "--local") if binary == "attune" else ("--info", "--local")
                info = json.loads(self.run_cli(binary, *arguments))
                self.assertEqual(info["binary"], binary)
                self.assertEqual(info["version"], VERSION)

    def test_nested_commands(self):
        if "attune" not in self.binaries:
            return
        for arguments in (
            ("pack", "--help"),
            ("pack", "test", "--help"),
            ("pack", "checksum", "--help"),
            ("pack", "update", "--help"),
            ("pack", "release", "build", "--help"),
            ("cache", "entry", "scan", "--help"),
            ("queue", "items", "update", "--help"),
            ("action", "execute", "--help"),
            ("action", "search", "--help"),
            ("rule", "create", "--help"),
            ("auth", "token", "delete", "--help"),
            ("artifact", "version", "show", "--help"),
            ("run", "--help"),
        ):
            with self.subTest(arguments=arguments):
                self.assertIn("Usage:", self.run_cli("attune", *arguments))

    def test_completion_scripts(self):
        if "attune" not in self.binaries:
            return
        for shell in ("bash", "fish", "zsh", "powershell"):
            with self.subTest(shell=shell):
                self.assertIn("attune", self.run_cli("attune", "completion", shell))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Example: python scripts/test_cli_startup.py --bin-dir target/debug",
    )
    parser.add_argument("--bin-dir", required=True, type=Path, help="Directory containing the actual built binaries")
    parser.add_argument("--binary", action="append", choices=("attune", "attune-mcp"), help="Limit diagnostics to one binary; default: both")
    arguments = parser.parse_args()
    CliStartupTests.binary_dir = arguments.bin_dir.resolve()
    CliStartupTests.binaries = arguments.binary or ["attune", "attune-mcp"]
    unittest.main(argv=[sys.argv[0]], verbosity=2)
