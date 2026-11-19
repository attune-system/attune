#!/usr/bin/env python3
"""Prepare recipe fixtures exclusively for a fresh, runner-owned E2E stack."""

import argparse
import json
from pathlib import Path
import shutil



def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    fixture = repo / "tests/fixtures/runtime-services"
    root = args.directory.resolve()
    root.mkdir()
    core = root / "packs/core"
    shutil.copytree(repo / "packs/core", core)
    digests = []
    for name in ["fixture", "slow_fixture"]:
        definition = json.loads((fixture / f"{name}.json").read_text())
        digest = (fixture / f"{name}.sha256").read_text().strip()
        assert len(digest) == 64 and all(value in "0123456789abcdef" for value in digest)
        digests.append(digest)
        (core / "runtimes" / f"lifecycle_{name}.yaml").write_text(json.dumps(definition, indent=2))
    for section in ["actions", "sensors", "triggers"]:
        for source in (fixture / section).iterdir():
            shutil.copy2(source, core / section / source.name)
    override = {"services": {
        "init-packs": {"volumes": [f"{root}/packs:/source/packs:ro"]},
        "worker-python": {"environment": {
            "ATTUNE_RUNTIME_RECIPE_DIGESTS": ",".join(digests),
            "ATTUNE_WORKER_RUNTIMES": "python,shell,lifecyclefixture,lifecycleslowfixture",
        }},
        "sensor": {"environment": {
            "ATTUNE_RUNTIME_RECIPE_DIGESTS": digests[0],
            "ATTUNE_SENSOR_RUNTIMES": "native,python,shell,lifecyclefixture",
        }},
        "e2e-tests": {"environment": {"ATTUNE_RUNTIME_SERVICE_SMOKE": "1"}},
    }}
    (root / "compose.yaml").write_text(json.dumps(override, indent=2))


if __name__ == "__main__":
    main()
