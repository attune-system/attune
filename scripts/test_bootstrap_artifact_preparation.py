"""Offline format/planning boundaries, never builds or executes an image/binary."""
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
SPEC = importlib.util.spec_from_file_location("artifact_preparation", ROOT / "scripts/prepare-bootstrap-artifacts.py")
artifacts = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(artifacts)


def tar(path, entries):
    with tarfile.open(path, "w") as archive:
        for name, payload in entries.items():
            member = tarfile.TarInfo(name)
            member.size = len(payload)
            archive.addfile(member, io.BytesIO(payload))


class Preparation(unittest.TestCase):
    def test_local_context_does_not_authorize_a_remote_or_unstarted_builder(self):
        manifest = {"buildx_builder": "central", "docker_context": "default"}
        for info in ({"Name": "central", "Driver": "remote", "Nodes": []}, {"Name": "central", "Driver": "docker-container", "Nodes": [{"Endpoint": "ssh://foreign", "Status": "running"}]}, {"Name": "central", "Driver": "docker-container", "Nodes": [{"Endpoint": "default", "Status": "inactive"}]}):
            with self.assertRaises(artifacts.InputError):
                artifacts.require_local_builder(info, manifest, "unix:///unit/docker.sock")

    def test_existing_running_single_local_node_is_required_not_created(self):
        manifest = {"buildx_builder": "central", "docker_context": "default"}
        artifacts.require_local_builder({"Name": "central", "Driver": "docker-container", "Nodes": [{"Endpoint": "default", "Status": "running"}]}, manifest, "unix:///unit/docker.sock")

    def test_default_plan_starts_no_process_and_emits_no_digest_or_publication_claim(self):
        output = io.StringIO()
        with mock.patch.object(artifacts.subprocess, "Popen") as process, contextlib.redirect_stdout(output):
            self.assertEqual(artifacts.main([]), 0)
        process.assert_not_called()
        plan = json.loads(output.getvalue())
        self.assertEqual(plan["mode"], "DRY_RUN")
        self.assertEqual(plan["build_jobs"], 2)
        self.assertEqual(plan["build_invocations_concurrent"], 1)
        self.assertEqual([entry["artifact"] for entry in plan["commands"]], ["native", "frontend", *artifacts.IMAGES])
        self.assertFalse(any("--push" in entry["command"] or "--load" in entry["command"] for entry in plan["commands"]))

    def test_execute_requires_separate_central_build_permission(self):
        with self.assertRaisesRegex(artifacts.InputError, "permit-build"):
            artifacts.arguments(["--execute"])

    def test_unresolved_example_fails_before_any_docker_process(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            args = artifacts.arguments(["--execute", "--permit-build", "--work-root", str(Path(directory) / "new")])
            manifest = json.loads(args.manifest.read_text())
            with mock.patch.object(artifacts.subprocess, "Popen") as process:
                with self.assertRaises(artifacts.InputError):
                    artifacts.execute(args, manifest)
            process.assert_not_called()

    def test_source_archive_rejects_traversal_secrets_and_links(self):
        for name in ("../foreign", "/foreign", ".env.local", "credentials.private.json"):
            with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
                root = Path(directory)
                path = root / "source.tar"
                tar(path, {name: b"offline input"})
                with self.assertRaises(artifacts.InputError):
                    artifacts.unpack_source(path, root / "context")
                self.assertFalse((root / "foreign").exists())
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            path = root / "link.tar"
            with tarfile.open(path, "w") as archive:
                link = tarfile.TarInfo("linked")
                link.type = tarfile.SYMTYPE
                link.linkname = "/foreign"
                archive.addfile(link)
            with self.assertRaisesRegex(artifacts.InputError, "not links"):
                artifacts.unpack_source(path, root / "context")

    def test_missing_integrated_native_inputs_is_not_a_skipped_build(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            root = Path(directory)
            path = root / "source.tar"
            tar(path, {"Cargo.lock": b"unit boundary"})
            with self.assertRaisesRegex(artifacts.InputError, "lacks current"):
                artifacts.unpack_source(path, root / "context")

    def oci(self, root, corrupt=False):
        # OCI format fixture only, not a runnable image or fabricated live pin.
        config = b'{"architecture":"amd64","os":"linux"}'
        layer = b"unit-test-layer-format-boundary"
        def descriptor(data, media):
            return {"mediaType": media, "digest": "sha256:" + hashlib.sha256(data).hexdigest(), "size": len(data)}
        manifest = json.dumps({"schemaVersion": 2, "config": descriptor(config, "application/vnd.oci.image.config.v1+json"), "layers": [descriptor(layer, "application/vnd.oci.image.layer.v1.tar")]}).encode()
        root_descriptor = descriptor(manifest, "application/vnd.oci.image.manifest.v1+json")
        entries = {"oci-layout": b'{"imageLayoutVersion":"1.0.0"}', "index.json": json.dumps({"schemaVersion": 2, "manifests": [root_descriptor]}).encode()}
        for value in (config, layer, manifest):
            entries["blobs/sha256/" + hashlib.sha256(value).hexdigest()] = value
        if corrupt:
            entries["blobs/sha256/" + hashlib.sha256(layer).hexdigest()] = b"different-bytes"
        path = root / "unit-oci.tar"
        tar(path, entries)
        return path, root_descriptor["digest"]

    def test_oci_digest_is_measured_from_exported_content_not_a_tag(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            path, expected = self.oci(Path(directory))
            receipt = artifacts.oci_receipt(path)
            self.assertEqual(receipt["oci_manifest_digest"], expected)
            self.assertEqual(receipt["archive_sha256"], artifacts.sha(path))
            self.assertNotIn("repository", receipt)

    def test_corrupt_oci_content_cannot_produce_a_receipt(self):
        with tempfile.TemporaryDirectory(dir="/tmp/opencode") as directory:
            path, _ = self.oci(Path(directory), corrupt=True)
            with self.assertRaisesRegex(artifacts.InputError, "blob digest mismatch"):
                artifacts.oci_receipt(path)

    def test_recipes_use_actual_sources_locked_two_job_builds_and_no_runtime_pip(self):
        recipe = (ROOT / artifacts.DOCKERFILE).read_text()
        self.assertIn("CARGO_BUILD_JOBS=2", recipe)
        self.assertEqual(recipe.count("cargo build --locked --release -j 2"), 2)
        self.assertEqual(recipe.count("cargo zigbuild --locked --release -j 2"), 2)
        self.assertIn("amd64:x86_64-unknown-linux-musl arm64:aarch64-unknown-linux-musl", recipe)
        self.assertIn("bootstrap_acceptance_archive /out/core /out/canonical-store", recipe)
        self.assertNotIn("fn main(){}", recipe)
        self.assertNotIn("pip install", recipe)
        init = (ROOT / "docker/acceptance/init-packs.sh").read_text()
        self.assertIn("exec python3 /scripts/bootstrap_core_pack.py publish", init)
        self.assertNotIn("cp ", init)
        self.assertNotIn("BOOTSTRAP_ADMISSION_ENABLED", recipe)


if __name__ == "__main__":
    unittest.main()
