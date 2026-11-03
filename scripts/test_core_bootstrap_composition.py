#!/usr/bin/env python3
"""Offline inventory of the actual shipped core archive, not executable stand-ins."""
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest

import yaml

ROOT = Path(__file__).resolve().parent.parent
CORE = ROOT / "packs/core"
SPEC = importlib.util.spec_from_file_location("core_publisher", ROOT / "scripts/bootstrap_core_pack.py")
publisher = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publisher)


class CoreCompositionTests(unittest.TestCase):
    def test_owned_archive_preserves_every_actual_file_and_metadata_body(self):
        archive = publisher.deterministic_archive(CORE)
        self.assertEqual(archive, publisher.deterministic_archive(CORE))
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="core-composition-") as directory:
            # Locally generated archive, with Python's traversal/link-safe data filter.
            # nosemgrep: trailofbits.python.tarfile-extractall-traversal.tarfile-extractall-traversal
            with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as source:
                source.extractall(directory, filter="data")
            owned = Path(directory) / "core"
            expected = {str(path.relative_to(CORE)): path.read_bytes() for path in CORE.rglob("*") if path.is_file()}
            actual = {str(path.relative_to(owned)): path.read_bytes() for path in owned.rglob("*") if path.is_file()}
            self.assertEqual(actual, expected)
            for relative in expected:
                self.assertEqual((CORE / relative).stat().st_mode & 0o111, (owned / relative).stat().st_mode & 0o111, relative)
            for folder, count in (("actions", 11), ("triggers", 7), ("sensors", 1), ("runtimes", 9), ("permission_sets", 5)):
                self.assertEqual(len(list((owned / folder).glob("*.yaml"))), count, folder)

    def test_reserved_platform_policy_definitions_match_catalog_without_local_rescoping(self):
        catalog = ROOT / "crates/common/src/platform_catalog/permission_sets"
        self.assertEqual({path.stem for path in catalog.glob("*.yaml")}, {"admin", "editor", "executor", "viewer"})
        for source in catalog.glob("*.yaml"):
            actual = yaml.safe_load((CORE / "permission_sets" / source.name).read_text())
            canonical = yaml.safe_load(source.read_text())
            self.assertEqual(actual, canonical, source.name)
            self.assertNotIn("scope", actual)
            self.assertTrue(all("scope" not in grant and "managing_pack_id" not in grant.get("constraints", {}) for grant in actual["grants"]))
        # key_creator is an actual pack-owned runtime policy, NOT a reserved
        # platform role. Its local envelope must survive independently.
        self.assertFalse((catalog / "key_creator.yaml").exists())
        self.assertEqual(yaml.safe_load((CORE / "permission_sets/key_creator.yaml").read_text())["ref"], "core.key_creator")

    def test_shipped_refs_and_canonical_installer_are_explicit(self):
        self.assertEqual(publisher.bundled_pack_version(CORE), "1.0.5")
        actions = {yaml.safe_load(path.read_text())["ref"] for path in (CORE / "actions").glob("*.yaml")}
        self.assertEqual(actions, {"core.echo", "core.noop", "core.sleep", "core.http_request", "core.generate_ssh_key_pair", "core.run_agent_command", "core.download_packs", "core.register_packs", "core.get_pack_dependencies", "core.build_pack_envs", "core.install_packs"})
        sensor = yaml.safe_load((CORE / "sensors/timer_sensor.yaml").read_text())
        triggers = {yaml.safe_load(path.read_text())["ref"] for path in (CORE / "triggers").glob("*.yaml")}
        self.assertTrue(set(sensor["trigger_types"]).issubset(triggers))
        self.assertTrue((CORE / "sensors" / sensor["entry_point"]).is_file())
        self.assertFalse(list((CORE / "workflows").glob("*.yaml")))
        installer = yaml.safe_load((CORE / "actions/install_packs.yaml").read_text())
        self.assertEqual(installer["workflow_file"], "workflows/install_packs.workflow.yaml")
        self.assertTrue((CORE / "actions" / installer["workflow_file"]).is_file())
        self.assertFalse(list((CORE / "queues").glob("*.yaml")))

    def test_real_action_entrypoints_and_runtime_grant_refs_resolve_in_shipped_tree(self):
        policies = {yaml.safe_load(path.read_text())["ref"] for path in (CORE / "permission_sets").glob("*.yaml")}
        for path in (CORE / "actions").glob("*.yaml"):
            action = yaml.safe_load(path.read_text())
            with self.subTest(action=action["ref"]):
                self.assertTrue((CORE / "actions" / action.get("entry_point", action.get("workflow_file"))).is_file())
                self.assertTrue(set(action.get("default_execution_permission_set_refs", [])).issubset(policies))
        creator = yaml.safe_load((CORE / "actions/generate_ssh_key_pair.yaml").read_text())
        self.assertEqual(creator["default_execution_permission_set_refs"], ["core.key_creator"])
        self.assertTrue(creator["parameters"]["local_ref"]["required"])

    def test_distributable_selects_actual_core_version_and_existing_api_credential(self):
        version = publisher.bundled_pack_version(CORE)
        manifest = ROOT / "docker/distributable/docker-compose.yaml"
        services = yaml.safe_load(manifest.read_text())["services"]
        bootstrap = services["init-packs"]
        self.assertTrue(bootstrap["image"].endswith("/attune/init-packs:${ATTUNE_INIT_PACKS_IMAGE_TAG:-core-" + version + "}"))
        self.assertNotIn("ATTUNE_IMAGE_TAG", bootstrap["image"])
        environment = bootstrap["environment"]
        self.assertEqual(environment["ATTUNE_CORE_PACK_VERSION"], "${ATTUNE_CORE_PACK_VERSION:-" + version + "}")
        self.assertEqual(environment["ATTUNE_CORE_PACK_DIGEST"], "${ATTUNE_CORE_PACK_DIGEST:-}")
        self.assertEqual(environment["ATTUNE_BOOTSTRAP_TOKEN"], "${ATTUNE_BOOTSTRAP_TOKEN:-}")
        self.assertEqual(environment["ATTUNE_BOOTSTRAP_MODE"], "api")
        self.assertFalse(any(key.startswith("DB_") for key in environment))
        self.assertFalse(bootstrap.get("volumes"))
        self.assertNotIn("core-1.0.3", manifest.read_text())

    def test_build_metadata_derives_version_and_includes_real_native_binary_inputs(self):
        workflow = (ROOT / ".github/workflows/publish.yml").read_text()
        self.assertIn('core_pack_version=$(awk -F', workflow)
        self.assertIn('packs/core/pack.yaml)', workflow)
        self.assertIn('tags+=("core-${{ needs.metadata.outputs.core_pack_version }}")', workflow)
        dockerfile = (ROOT / "docker/Dockerfile.init-packs-package").read_text()
        self.assertIn("COPY packs/ /source/packs/", dockerfile)
        self.assertIn("COPY dist/core-timer-sensor/ /source/packs/core/sensors/bin/", dockerfile)
        for architecture in ("linux-amd64", "linux-arm64"):
            self.assertIn("/source/packs/core/sensors/bin/" + architecture + "/attune-core-timer-sensor", dockerfile)
        for text in (workflow, dockerfile, (ROOT / "scripts/bootstrap_core_pack.py").read_text()):
            self.assertNotIn("core-1.0.3", text)
        # The publisher derives its version from actual content, never a stale
        # release constant. A release digest still requires the packaged archive.
        self.assertIn('bundled_pack_version(pack_dir)', (ROOT / "scripts/bootstrap_core_pack.py").read_text())


if __name__ == "__main__":
    unittest.main()
