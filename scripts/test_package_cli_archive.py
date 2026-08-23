import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).with_name("package-cli-archive.py")


class PackageCliArchiveTests(unittest.TestCase):
    def test_archives_are_reproducible_and_flat(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            stage = root / "stage"
            stage.mkdir()
            binary = stage / "attune"
            binary.write_bytes(b"binary")
            binary.chmod(0o755)
            (stage / "LICENSE").write_text("license\n", encoding="ascii")
            nested = stage / "nested"
            nested.mkdir()
            (nested / "config.yaml").write_text("enabled: true\n", encoding="ascii")

            for suffix in ("tar.gz", "zip"):
                first = root / f"first.{suffix}"
                second = root / f"second.{suffix}"
                subprocess.run(
                    [sys.executable, SCRIPT, first, "1787443200", stage],
                    check=True,
                )
                os.utime(binary, (1787529600, 1787529600))
                subprocess.run(
                    [sys.executable, SCRIPT, second, "1787443200", stage],
                    check=True,
                )
                self.assertEqual(first.read_bytes(), second.read_bytes())

            with tarfile.open(root / "first.tar.gz") as archive:
                self.assertEqual(
                    archive.getnames(), ["LICENSE", "attune", "nested/config.yaml"]
                )
            with zipfile.ZipFile(root / "first.zip") as archive:
                self.assertEqual(
                    archive.namelist(), ["LICENSE", "attune", "nested/config.yaml"]
                )


if __name__ == "__main__":
    unittest.main()
