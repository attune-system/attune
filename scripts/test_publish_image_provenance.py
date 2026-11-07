"""Check publisher build arguments without building or publishing any image."""

import os
import re
import subprocess
import tempfile
import unittest
from pathlib import Path

import yaml


ROOT = Path(__file__).resolve().parent.parent
JOBS = ("publish-rust-images", "publish-web-images")
RUST_IMAGES = (
    "api", "executor", "notifier", "supervisor", "agent", "migrations",
    "init-user", "init-packs",
)
LABELS = {
    "org.opencontainers.image.source": "https://github.com/${{ github.repository }}",
    "org.opencontainers.image.revision": "${{ github.sha }}",
    "org.opencontainers.image.version": "${{ needs.metadata.outputs.image_tag }}",
}


def push_script(workflow, job):
    steps = workflow["jobs"][job]["steps"]
    return next(step["run"] for step in steps if step["name"] == "Push architecture image")


def build_arguments(script, image, arch):
    """Replay the actual build invocation with a shell function recording argv.

    Stop before post-publication checks. No Docker executable, registry, image,
    binary fixture or build result is involved in this command-construction test.
    """
    invocation = 'run_with_retries 3 5 "${build_cmd[@]}"'
    prefix, separator, _ = script.partition(invocation)
    if not separator:
        raise AssertionError("Publisher build invocation is unavailable")
    values = {
        "github.repository": "example-owner/example-repository",
        "github.sha": "source-revision-fixture",
        "needs.metadata.outputs.image_tag": "1.2.3-rc.4",
        "needs.metadata.outputs.registry": "registry.example.invalid",
        "needs.metadata.outputs.namespace": "namespace-fixture",
        "matrix.platform": "linux/" + arch,
        "matrix.arch": arch,
        "matrix.image.repository": "attune/" + image,
        "matrix.image.dockerfile": "docker/Dockerfile." + image + "-fixture",
        "matrix.image.name": image,
    }
    shell = re.sub(
        r"\$\{\{\s*([^{}]+?)\s*\}\}",
        lambda match: values[match.group(1)],
        prefix + separator,
    )
    with tempfile.TemporaryDirectory(prefix="attune-publish-arguments-") as directory:
        capture = Path(directory) / "arguments"
        recorder = 'docker() { printf "%s\\0" "$@" >> "$ARGUMENT_CAPTURE"; }\n'
        subprocess.run(
            ["bash", "-euo", "pipefail"],
            input=recorder + shell,
            text=True,
            capture_output=True,
            check=True,
            timeout=10,
            env={"PATH": os.defpath, "ARGUMENT_CAPTURE": str(capture)},
        )
        return capture.read_bytes().rstrip(b"\0").decode().split("\0")


class PublishImageProvenanceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.workflow = yaml.safe_load((ROOT / ".github/workflows/publish.yml").read_text())

    def assert_build_labels(self, arguments):
        self.assertEqual(arguments[:2], ["buildx", "build"])
        actual = [arguments[index + 1] for index, value in enumerate(arguments) if value == "--label"]
        self.assertCountEqual(actual, [
            "org.opencontainers.image.source=https://github.com/example-owner/example-repository",
            "org.opencontainers.image.revision=source-revision-fixture",
            "org.opencontainers.image.version=1.2.3-rc.4",
        ])
        self.assertIn("--provenance=false", arguments)
        self.assertIn("--sbom=false", arguments)
        self.assertEqual(arguments.count("--push"), 1)

    def test_every_architecture_packaging_command_has_build_time_source_labels(self):
        repositories = self.workflow["jobs"]["publish-rust-manifests"]["strategy"]["matrix"]["repository"]
        self.assertCountEqual(repositories, ["attune/" + image for image in RUST_IMAGES])
        for job in JOBS:
            script = push_script(self.workflow, job)
            for label, expression in LABELS.items():
                self.assertIn(f'--label "{label}={expression}"', script)
            for image in RUST_IMAGES if job == "publish-rust-images" else ("web",):
                for arch in ("amd64", "arm64"):
                    with self.subTest(job=job, image=image, arch=arch):
                        arguments = build_arguments(script, image, arch)
                        self.assert_build_labels(arguments)
                        self.assertEqual(arguments[arguments.index("--platform") + 1], "linux/" + arch)
                        self.assertEqual(arguments[arguments.index("--tag") + 1],
                            f"registry.example.invalid/namespace-fixture/attune/{image}:1.2.3-rc.4-{arch}")

    def test_removing_any_identity_label_fails_the_argument_check(self):
        for job in JOBS:
            script = push_script(self.workflow, job)
            for label, expression in LABELS.items():
                with self.subTest(job=job, label=label):
                    without_label = script.replace(f'--label "{label}={expression}"', "")
                    with self.assertRaises(AssertionError):
                        self.assert_build_labels(build_arguments(without_label, "api" if job == JOBS[0] else "web", "amd64"))

    def test_new_architecture_publishers_require_review_and_ci_runs_this_test(self):
        publishers = {
            job for job, definition in self.workflow["jobs"].items()
            if any("docker buildx build" in step.get("run", "") for step in definition.get("steps", []))
        }
        self.assertEqual(publishers, set(JOBS))
        ci = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text())
        validation = next(step["run"] for step in ci["jobs"]["release-checks"]["steps"] if step.get("name") == "Validate release scripts")
        self.assertIn("python3 scripts/test_publish_image_provenance.py", validation)


if __name__ == "__main__":
    unittest.main()
