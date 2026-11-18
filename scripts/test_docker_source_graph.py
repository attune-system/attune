#!/usr/bin/env python3
"""Selective release builders must replace stubs throughout their local graph."""
from copy import deepcopy
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parent.parent


def manifests():
    return {path.parent.name: tomllib.loads(path.read_text())
            for path in (ROOT / "crates").glob("*/Cargo.toml")}


def missing_sources(dockerfile, packages):
    # Exact full-directory copies count. A Cargo.toml-only fetch scaffold does
    # not provide a lib.rs, include file, module or build script implementation.
    copied = set(re.findall(r"^COPY crates/([a-z0-9-]+)/ ./crates/\1/\s*$", dockerfile, re.M))
    binaries = {binary["name"]: name for name, package in packages.items()
                for binary in package.get("bin", [])}
    selected = re.findall(r"--bin\s+([a-z0-9-]+)", dockerfile)
    if not selected:
        raise ValueError("release builder has no explicit selected binaries")
    pending = [binaries[binary] for binary in selected]
    required = set()
    while pending:
        name = pending.pop()
        if name in required:
            continue
        required.add(name)
        package = packages[name]
        tables = [package, *package.get("target", {}).values()]
        for table in tables:
            # Cargo does not compile dev-dependencies for release binaries.
            for group in ["dependencies", "build-dependencies"]:
                for dependency in table.get(group, {}).values():
                    if isinstance(dependency, dict) and "path" in dependency:
                        target = (ROOT / "crates" / name / dependency["path"]).resolve().name
                        if target in packages:
                            pending.append(target)
    return required - copied


def missing_script_exemptions(dockerfile, rules):
    # This repository excludes scripts/ from its build context and permits
    # selected files explicitly. Check that policy, not a general ignore parser.
    excluded_at = max(i for i, rule in enumerate(rules) if rule == 'scripts/')
    included = set(rules[excluded_at + 1:])
    scripts = set(re.findall(r'^COPY (scripts/[^\s]+) ', dockerfile, re.M))
    return {source for source in scripts if '!' + source not in included}


class DockerSourceGraphTests(unittest.TestCase):
    def test_source_builder_scripts_are_explicitly_included_in_the_build_context(self):
        rules = [line.strip() for line in (ROOT / '.dockerignore').read_text().splitlines()]
        self.assertIn('scripts/', rules)  # Keep the broad exclusion intact.
        source = (ROOT / 'docker/Dockerfile.agent').read_text()
        self.assertEqual(missing_script_exemptions(source, rules), set())
        self.assertEqual(
            missing_script_exemptions(source, [rule for rule in rules if rule != '!scripts/package-cli-archive.py']),
            {'scripts/package-cli-archive.py'},
        )

    def test_selective_release_builders_copy_normal_local_dependency_closure(self):
        packages = manifests()
        for name in ["optimized", "agent", "mcp"]:
            with self.subTest(builder=name):
                source = (ROOT / "docker" / ("Dockerfile." + name)).read_text()
                self.assertEqual(missing_sources(source, packages), set())

    def test_api_worker_preparer_dependency_cannot_be_satisfied_by_fetch_stubs(self):
        # Model the dependent credential workstream's normal API dependency,
        # including on this branch before its whole foundation is integrated.
        packages = deepcopy(manifests())
        packages["api"]["dependencies"]["attune-worker"] = {"path": "../worker"}
        source = (ROOT / "docker/Dockerfile.optimized").read_text()
        self.assertEqual(missing_sources(source, packages), set())
        without_worker = source.replace("COPY crates/worker/ ./crates/worker/", "")
        self.assertEqual(missing_sources(without_worker, packages), {"worker"})


if __name__ == "__main__":
    unittest.main()
