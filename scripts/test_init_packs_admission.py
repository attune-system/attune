"""Exercise both shell packaging modes without touching a deployment."""
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class InitPacksAdmissionTests(unittest.TestCase):
    def run_mode(self, mode):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "source" / "core"
            source.mkdir(parents=True)
            (source / "sensors").mkdir()
            (source / "pack.yaml").write_text("ref: core\nversion: 2.0.0\n")
            target = root / "target" / "core"
            (target / "sensors").mkdir(parents=True)
            (target / "pack.yaml").write_text("ref: core\nversion: 1.0.0\n")
            (target / "sensors" / "attune-core-timer-sensor").write_text("owned-binary")
            binaries = root / "bin"
            binaries.mkdir()
            for name in ("pip", "chown"):
                script = binaries / name
                script.write_text("#!/bin/sh\nexit 0\n")
                script.chmod(0o755)
            python = binaries / "python3"
            python.write_text(
                '#!/bin/sh\nset -eu\n'
                'test "$1" = /scripts/bootstrap_core_pack.py\n'
                'test "$2" = publish\n'
                'cat "$SOURCE_PACKS_DIR/core/pack.yaml" > "$EVIDENCE"\n'
                'if [ -f "$SOURCE_PACKS_DIR/core/sensors/attune-core-timer-sensor" ]; then\n'
                '  cat "$SOURCE_PACKS_DIR/core/sensors/attune-core-timer-sensor" >> "$EVIDENCE"\nfi\n'
                'printf "%s" "$SOURCE_PACKS_DIR" > "$CANDIDATE_PATH"\n'
            )
            python.chmod(0o755)
            environment = dict(os.environ, PATH=f"{binaries}:/usr/bin:/bin",
                               ATTUNE_BOOTSTRAP_MODE=mode,
                               SOURCE_PACKS_DIR=str(source.parent),
                               TARGET_PACKS_DIR=str(target.parent),
                               RUNTIME_ENVS_DIR=str(root / "runtime"),
                               ARTIFACTS_DIR=str(root / "artifacts"),
                               EVIDENCE=str(root / "evidence"),
                               CANDIDATE_PATH=str(root / "candidate-path"))
            result = subprocess.run(["/bin/sh", "docker/init-packs.sh"], env=environment,
                                    capture_output=True, text=True, timeout=10,
                                    cwd=Path(__file__).parent.parent)
            self.assertEqual(result.returncode, 0, result.stderr)
            evidence = (root / "evidence").read_text()
            self.assertIn("version: 2.0.0", evidence)
            self.assertEqual("owned-binary" in evidence, mode == "volume")
            self.assertIn("version: 1.0.0", (target / "pack.yaml").read_text())
            self.assertFalse(Path((root / "candidate-path").read_text()).exists())

    def test_volume_mode_stages_private_candidate_and_keeps_active_projection(self):
        self.run_mode("volume")

    def test_api_mode_uses_same_publisher_without_volume_overlay(self):
        self.run_mode("api")


if __name__ == "__main__":
    unittest.main()
