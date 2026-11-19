#!/usr/bin/env python3
"""Offline command-level checks of bundled recipes; requires Docker and PyYAML.

This tests actual tool flags and outputs. Rust/service tests own Attune admission,
authority, process supervision, and generation cleanup guarantees.
"""

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import uuid

import yaml


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("fixture")
    parser.add_argument("--image", required=True, help="Provisioned image tag or digest")
    parser.add_argument("--memory", default="2g")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    fixtures = repo / "tests/fixtures/runtime-toolchains"
    fixture = yaml.safe_load((fixtures / "fixtures.yaml").read_text())[args.fixture]
    recipe = yaml.safe_load((repo / fixture["recipe"]).read_text())["execution_config"]["lifecycle"]
    scratch = repo / ".scratch"
    scratch.mkdir(exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="toolchain-smoke-", dir=scratch))
    name = "attune-toolchain-smoke-" + uuid.uuid4().hex

    def docker(*command, **kwargs):
        return subprocess.run(["docker", *command], text=True, capture_output=True, check=True, **kwargs)

    try:
        pack = root / "pack"
        shutil.copytree(fixtures / fixture.get("inputs", args.fixture), pack)
        generation = root / "generation"
        for directory in [generation, generation / "home", generation / "output", generation / "scratch"]:
            directory.mkdir()
        docker("run", "--detach", "--name", name, "--network", "none", "--read-only",
               "--user", f"{os.getuid()}:{os.getgid()}", "--memory", args.memory,
               "--mount", f"type=bind,src={root},dst=/work", "--tmpfs", "/tmp:rw,mode=1777,size=256m",
               "--entrypoint", "/bin/sh", args.image, "-c", "exec sleep 3600", timeout=300)
        print("image", docker("inspect", "--format", "{{.Image}}", name).stdout.strip(), flush=True)
        image_env = json.loads(docker("inspect", "--format", "{{json .Config.Env}}", name).stdout)
        path = next(value.partition("=")[2] for value in image_env if value.startswith("PATH="))
        env = {"PATH": path, "HOME": "/work/generation/home", "LANG": "C.UTF-8",
               "XDG_CONFIG_HOME": "/work/generation/home/config", "XDG_CACHE_HOME": "/work/generation/home/cache",
               "TMPDIR": "/work/generation/scratch"}

        def run(command, cwd="/work/pack", stdin=None, timeout=300):
            result = docker("exec", "--interactive", "--workdir", cwd, name,
                            "/usr/bin/env", "-i", *[f"{key}={value}" for key, value in env.items()],
                            *command, input=stdin, timeout=timeout)
            return result.stdout

        variables = {"pack_dir": "/work/pack", "workspace_dir": "/work/generation/workspace",
                     "env_dir": "/work/generation", "output_dir": "/work/generation/output",
                     "scratch_dir": "/work/generation/scratch"}

        def expand(value):
            return re.sub(r"(?<!\$)\{([a-z_]+)\}", lambda match: variables[match[1]], value)

        for tool in recipe["tools"]:
            binary = None
            for candidate in tool["binaries"]:
                try:
                    binary = run(["/bin/sh", "-c", 'command -v "$1"', "probe", candidate]).strip()
                    break
                except subprocess.CalledProcessError:
                    continue
            if not binary:
                raise RuntimeError(f"Required tool {tool['name']} is absent")
            # Java writes its version to stderr; combine streams explicitly.
            output = run(["/bin/sh", "-c", '"$@" 2>&1', "probe", binary, *tool.get("args", [])])
            version = re.search(tool["version_pattern"].replace("(?<version>", "(?P<version>"), output)
            if not version:
                raise RuntimeError(f"Version probe did not match for {tool['name']}: {output}")
            print(args.fixture, tool["name"], version["version"], flush=True)
            variables["tool_" + tool["name"]] = binary

        for command in fixture.get("setup", []):
            run([expand(value) for value in command])
        for relative in fixture.get("discard", []):
            shutil.rmtree(pack / relative)

        def snapshot():
            return {str(file.relative_to(pack)): file.read_bytes() for file in pack.rglob("*") if file.is_file()}

        original = snapshot()
        for file in [*pack.rglob("*"), pack]:
            file.chmod(0o555 if file.is_dir() else 0o444)
        workspace = generation / "workspace"
        if recipe.get("workspace"):
            shutil.copytree(pack, workspace)
            for file in [workspace, *workspace.rglob("*")]:
                file.chmod(0o700 if file.is_dir() else file.stat().st_mode | 0o200)
        else:
            workspace.mkdir()
        source_root = variables["workspace_dir"] if recipe.get("workspace") else variables["pack_dir"]
        variables["source_file"] = source_root + "/" + fixture["source"]
        env.update({key: expand(value) for key, value in recipe.get("env", {}).items()})
        for step in recipe.get("prepare", []):
            if step.get("when_file_exists") and not (pack / step["when_file_exists"]).is_file():
                continue
            run([expand(value) for value in step["command"]],
                expand(step.get("cwd", "{workspace_dir}")), timeout=step.get("timeout_seconds", 300))
        launch = recipe["launch"]
        output = run([expand(value) for value in launch["command"]],
                     expand(launch.get("cwd", "{pack_dir}")), stdin=json.dumps({"message": "stdin-json-ok"}) + "\n")
        assert json.loads(output) == {"message": "stdin-json-ok"}, output
        for check in fixture.get("checks", []):
            output = run([expand(value) for value in check["command"]], variables["workspace_dir"])
            assert output == check["stdout"], output
        assert snapshot() == original, "Recipe modified immutable inputs"
        print(args.fixture, "offline prepare, launch, imports/resources, stdin JSON, immutable inputs: PASS", flush=True)
    except subprocess.CalledProcessError as error:
        print(error.stdout, error.stderr, flush=True)
        raise
    finally:
        # Remove private files only after the container's writers are stopped.
        result = subprocess.run(["docker", "rm", "--force", "--volumes", name], capture_output=True, text=True)
        if result.returncode != 0:
            print(f"Cleanup unconfirmed; retained {root}: {result.stderr}", flush=True)
            raise RuntimeError("Could not confirm smoke-container teardown")
        for directory in [root, *root.rglob("*")]:
            if not directory.is_symlink() and directory.is_dir():
                directory.chmod(0o700)
        shutil.rmtree(root)


if __name__ == "__main__":
    main()
