#!/usr/bin/env python3
"""Plan or centrally build native-inclusive acceptance OCI artifacts. Never deploys."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import signal
import subprocess
import sys
import tarfile

BASES = ("toolchain", "node", "service", "python", "busybox", "postgres", "nginx")
IMAGES = ("api", "executor", "notifier", "supervisor", "agent", "init-packs", "migrations", "web")
DOCKERFILE = "docker/Dockerfile.acceptance-artifacts"
REQUIRED = (DOCKERFILE, DOCKERFILE + ".dockerignore", "Cargo.lock", "crates/core-timer-sensor/Cargo.lock", "crates/common/examples/storage_probe_inspector.rs", "crates/common/examples/bootstrap_acceptance_archive.rs", "crates/common/examples/interoperability_acceptance_archive.rs", "web/package-lock.json", "scripts/bootstrap_core_pack.py", "docker/acceptance/init-packs.sh")
NATIVE_MODULES = ("main.py", "controller.py", "journal.py", "native.py", "protocol.py", "http_input.py")
REQUIRED += ("crates/common/examples/interoperability_inspector.rs", "crates/common/src/repositories/interoperability_inspection.rs")
REQUIRED += ("crates/common/examples/legacy_native_writer.rs", "docker/acceptance/attune-native-legacy@.service", *["scripts/legacy_storage_controller/" + name for name in NATIVE_MODULES])
REQUIRED += ("scripts/acceptance/native-controller-source-pins.json", "crates/common/examples/native_settlement_adapter.rs", "crates/common/examples/native_producer_acceptance_archive.rs", "crates/common/src/repositories/file_native_setup.rs", "crates/common/src/repositories/native_generation_origin.rs", "migrations/20261016006015_controlled_native_file_operations.sql", "scripts/legacy_storage_controller/producer-context.example.json", "scripts/legacy_storage_controller/config.example.json", *["scripts/legacy_storage_controller/producer_pack/" + name for name in ("pack.yaml", "actions/producer.yaml", "actions/producer.py")])
REQUIRED += ("scripts/acceptance/native_controller_lifecycle.py", "scripts/acceptance/native_producer_fixture.py")


class InputError(RuntimeError):
    pass


def require(value, message):
    if not value:
        raise InputError(message)


def sha(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def save(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, "w") as target:
        json.dump(value, target, indent=2)


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, epilog="Examples:\n  %(prog)s --manifest scripts/acceptance/artifact-build.example.json\n  %(prog)s --manifest /owned/build.json --work-root /owned/new-build --execute --permit-build", formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).resolve().parent / "acceptance/artifact-build.example.json")
    parser.add_argument("--work-root", type=Path, default=Path("/tmp/opencode/acceptance-artifacts-new"), help="Must not exist. Evidence and OCI archives are retained, never recursively removed")
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--permit-build", action="store_true", help="Explicit central scheduler authorization; mandatory with --execute")
    parser.add_argument("--timeout-seconds", type=int, default=14400, help="Per foreground build deadline, 60..14400 seconds")
    result = parser.parse_args(argv)
    result.work_root = result.work_root.resolve()
    require(not result.execute or result.permit_build, "--execute requires explicit central --permit-build; no agent build authorization is inferred")
    require(60 <= result.timeout_seconds <= 14400, "Build deadline must be 60..14400 seconds")
    return result


def commands(args, manifest):
    root = args.work_root
    docker = ["docker", "--context", manifest.get("docker_context", "<local-context>"), "buildx", "build", "--builder", manifest.get("buildx_builder", "<central-owned-builder>"), "--platform", manifest.get("platform", "linux/amd64"), "--provenance=false", "--sbom=false", "--file", str(root / "source" / DOCKERFILE)]
    for name in BASES:
        argument = {"toolchain": "TOOLCHAIN_IMAGE", "node": "NODE_IMAGE", "service": "SERVICE_BASE", "python": "PYTHON_BASE", "busybox": "BUSYBOX_IMAGE", "postgres": "POSTGRES_IMAGE", "nginx": "NGINX_IMAGE"}[name]
        docker += ["--build-arg", argument + "=" + manifest.get("base_images", {}).get(name, "<required-immutable-base>")]
    docker += ["--build-arg", "ATTUNE_BUILD_GIT_SHA=" + manifest.get("source_revision", "<source-revision>"), "--build-arg", "SOURCE_DATE_EPOCH=" + str(manifest.get("source_date_epoch", "<fixed-source-time>"))]
    output = []
    for name, target in (("native", "native-export"), ("frontend", "frontend-export"), *[(image, "service" if image in IMAGES[:4] else image) for image in IMAGES]):
        command = [*docker, "--target", target, "--metadata-file", str(root / (name + ".build-metadata.json"))]
        if name != "native":
            command += ["--build-context", "compiled=" + str(root / "native")]
        if name in IMAGES:
            command += ["--label", "org.opencontainers.image.revision=" + manifest.get("source_revision", "<source-revision>"), "--label", "io.attune.acceptance.source-sha256=" + manifest.get("source_sha256", "<source-archive-sha256>"), "--build-context", "frontend=" + str(root / "frontend"), "--output", "type=oci,dest=" + str(root / (name + ".oci.tar")) + ",rewrite-timestamp=true"]
            if target == "service":
                command += ["--build-arg", "SERVICE=" + name]
        else:
            command += ["--output", "type=local,dest=" + str(root / name)]
        command += [str(root / "source")]
        output.append({"artifact": name, "command": command})
    return output


def plan(args, manifest):
    return {"mode": "EXECUTE" if args.execute else "DRY_RUN", "work_root": str(args.work_root), "build_jobs": 2, "build_invocations_concurrent": 1, "commands": commands(args, manifest), "creates": ["new private filesystem work root", "native binaries and measured canonical core archive", "eight measured OCI archives and build receipts"], "does_not_create": ["running application/database/provider containers", "Docker networks or volumes", "registry/release publication", "Kubernetes cluster/namespace", "admission gate changes"], "builder": "Explicit pre-existing central-owned BuildKit slot; never create/remove/prune it. Scheduler must bound its parallelism and drain cancelled builds.", "readiness": "Unchanged compiled gates. Artifact production is not admission, startup or lifecycle acceptance.", "next": "Import actual OCI bytes into an explicitly authorized owned registry, preserving or remeasuring manifest digests; use real repo@sha256 references in Compose/Helm acceptance. No image reference or core digest is invented by a plan."}


def validate(args, manifest):
    require(not args.work_root.exists(), "Work root exists; choose a new path. Never adopt/reset an earlier build")
    require(manifest.get("schema_version") == 1, "artifact manifest schema_version must be 1")
    require(re.fullmatch(r"[0-9a-f]{40}", manifest.get("source_revision", "")), "Record the real integrated source revision; its archive hash separately binds any reviewed staged snapshot")
    require(type(manifest.get("source_date_epoch")) is int and manifest["source_date_epoch"] > 0, "Supply a fixed source timestamp from the source receipt")
    require(isinstance(manifest.get("docker_context"), str) and manifest["docker_context"], "Explicit local Docker context required")
    require(bool(re.fullmatch(r"[a-zA-Z0-9][a-zA-Z0-9_.-]*", manifest.get("buildx_builder", ""))), "Explicit existing central permissioned builder name required")
    native = {"x86_64": "linux/amd64", "aarch64": "linux/arm64"}.get(platform.machine())
    require(sys.platform == "linux" and manifest.get("platform") == native, "Run on the selected native Linux architecture; no implicit QEMU or host helper substitution")
    require(set(manifest.get("base_images", {})) == set(BASES), "Supply all seven actual immutable build/runtime bases")
    for name, image in manifest["base_images"].items():
        require(bool(re.fullmatch(r"[^\s]+@sha256:[0-9a-f]{64}", image)), "Missing immutable base digest for " + name)
    archive = Path(manifest.get("source_archive", ""))
    require(archive.is_file() and not archive.is_symlink() and sha(archive) == manifest.get("source_sha256"), "Missing/mismatched actual source archive; never build a moving primary checkout")


def unpack_source(archive, destination):
    destination.mkdir(mode=0o700)
    names = set()
    total = 0
    with tarfile.open(archive, "r:*") as source:
        for member in source:
            path = PurePosixPath(member.name)
            if str(path) == "." and member.isdir():
                continue
            require(not path.is_absolute() and ".." not in path.parts and str(path) not in names, "Unsafe or duplicate source archive member")
            require(not any(part in (".git", "target", "node_modules") or (part.startswith(".env") and part not in (".env.example", ".env.template")) or ".private" in part for part in path.parts), "Operational secrets/caches are not acceptance build inputs")
            require(member.isdir() or member.isfile(), "Source archive must contain ordinary files/directories, not links/devices")
            total += member.size
            require(total <= 4 * 1024**3 and len(names) < 100000, "Source archive exceeds the explicit 4 GiB/100000 member bound")
            names.add(str(path))
            target = destination.joinpath(*path.parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with source.extractfile(member) as incoming, target.open("xb") as outgoing:
                    while block := incoming.read(1024 * 1024):
                        outgoing.write(block)
                target.chmod(member.mode & 0o755)
    require(all((destination / relative).is_file() for relative in REQUIRED), "Source snapshot lacks current acceptance recipes, native locks or integrated F inspector source")
    pins = json.loads((destination / "scripts/acceptance/native-controller-source-pins.json").read_text())
    require(set(pins["modules"]) == set(NATIVE_MODULES), "Frozen native source pins must include all six service modules")
    for name, expected in pins["modules"].items():
        require(sha(destination / "scripts/legacy_storage_controller" / name) == expected, "Frozen native controller source differs from reviewed F contract")
    require(sha(destination / pins["migration"]["path"]) == pins["migration"]["sha256"], "Frozen forward 06015 migration differs from reviewed F contract")


def oci_receipt(path):
    """Hash actual exported blobs and root descriptor, not an assumed tag digest."""
    with tarfile.open(path, "r:*") as archive:
        members = {}
        for member in archive:
            name = str(PurePosixPath(member.name))
            require(name not in members and (member.isdir() or member.isfile()), "Invalid/duplicate OCI archive member")
            members[name] = member
        def data(name):
            require(name in members and members[name].isfile() and members[name].size <= 4 * 1024 * 1024, "Missing/oversized OCI metadata")
            with archive.extractfile(members[name]) as source:
                return source.read()
        require(json.loads(data("oci-layout"))["imageLayoutVersion"] == "1.0.0", "Unsupported OCI layout")
        index = json.loads(data("index.json"))
        require(len(index["manifests"]) == 1, "Expected exactly one native image descriptor without attestations")
        for name, member in members.items():
            if name.startswith("blobs/sha256/") and member.isfile():
                with archive.extractfile(member) as source:
                    require(hashlib.file_digest(source, "sha256").hexdigest() == name.rsplit("/", 1)[1], "OCI exported blob digest mismatch")
        descriptor = index["manifests"][0]
        require(re.fullmatch(r"sha256:[0-9a-f]{64}", descriptor["digest"]), "Unsupported OCI descriptor digest")
        manifest_name = "blobs/sha256/" + descriptor["digest"].split(":")[1]
        require(descriptor["size"] == members[manifest_name].size, "OCI descriptor size mismatch")
        image = json.loads(data(manifest_name))
        require(image.get("schemaVersion") == 2 and "config" in image and "layers" in image, "Expected an OCI image manifest, not an unresolved index")
        for item in [image["config"], *image["layers"]]:
            require(re.fullmatch(r"sha256:[0-9a-f]{64}", item["digest"]), "Unsupported OCI child digest")
            member = members.get("blobs/sha256/" + item["digest"].split(":")[1])
            require(member is not None and member.isfile() and member.size == item["size"], "OCI manifest references missing/incomplete content")
        return {"oci_archive": str(path), "archive_sha256": sha(path), "oci_manifest_digest": descriptor["digest"]}


def native_receipt(root):
    result = json.loads((root / "core-release.json").read_text())
    require(re.fullmatch(r"[0-9a-f]{64}", result["digest"]), "Canonical helper did not report a measured SHA256")
    archive = root / "canonical-store/.releases/sha256" / result["digest"] / "pack.tar.gz"
    require(archive.is_file() and sha(archive) == result["digest"], "Actual canonical core archive does not match the helper's digest")
    for arch, machine in (("amd64", 62), ("arm64", 183)):
        binary = root / "core/sensors/bin" / ("linux-" + arch) / "attune-core-timer-sensor"
        with binary.open("rb") as source:
            header = source.read(20)
        require(len(header) == 20 and header[:6] == b"\x7fELF\x02\x01" and int.from_bytes(header[18:20], "little") == machine and binary.stat().st_mode & 0o111, "Missing actual architecture-specific executable timer")
    machine = {"x86_64": 62, "aarch64": 183}[platform.machine()]
    binaries = {}
    for family, names in (("services", ["attune-" + name for name in IMAGES[:4]]), ("agents", ["attune", "attune-mcp", "attune-agent", "attune-sensor-agent"]), ("tools", ["bootstrap_acceptance_archive", "storage_probe_inspector", "interoperability_acceptance_archive", "interoperability_inspector", "legacy_native_writer", "native_settlement_adapter", "native_producer_acceptance_archive"])):
        for name in names:
            path = root / family / name
            require(path.is_file() and not path.is_symlink() and path.stat().st_mode & 0o111, "Missing compiled executable: " + family + "/" + name)
            with path.open("rb") as source:
                header = source.read(20)
            require(len(header) == 20 and header[:6] == b"\x7fELF\x02\x01" and int.from_bytes(header[18:20], "little") == machine, "Wrong native ELF architecture for " + name)
            binaries[name] = {"path": str(path), "sha256": sha(path), "size": path.stat().st_size}
    modules = {}
    pins = json.loads((root / "native-controller/native-controller-source-pins.json").read_text())
    for name in (*NATIVE_MODULES, "attune-native-legacy@.service", "native-controller-source-pins.json", "producer-context.example.json", "config.example.json"):
        path = root / "native-controller" / name
        require(path.is_file() and not path.is_symlink(), "Missing real native controller packaging input")
        if name in NATIVE_MODULES:
            require(sha(path) == pins["modules"][name], "Exported native module differs from frozen source")
        modules[name] = {"path": str(path), "sha256": sha(path)}
    producer = json.loads((root / "native-producer-release.json").read_text())
    require(producer["ref"] == "f_native_settlement" and re.fullmatch(r"[0-9a-f]{64}", producer["digest"]), "Missing actual canonical native producer release")
    producer_archive = root / "native-producer-store/.releases/sha256" / producer["digest"] / "pack.tar.gz"
    require(producer_archive.is_file() and sha(producer_archive) == producer["digest"], "Native producer archive must match the actual canonical publisher measurement")
    producer["archive_path"] = str(producer_archive)
    producer["source_files"] = {}
    for name in ("pack.yaml", "actions/producer.yaml", "actions/producer.py"):
        path = root / "native-producer-pack" / name
        require(path.is_file() and not path.is_symlink(), "Missing exported actual managed producer source")
        producer["source_files"][name] = {"path": str(path), "sha256": sha(path)}
    return {"core": {"version": result["version"], "digest": result["digest"], "archive_path": str(archive), "archive_size": archive.stat().st_size}, "binaries": binaries, "native_controller_files": modules, "native_producer_release": producer}


def require_local_builder(info, manifest, endpoint):
    require(info.get("Name") == manifest["buildx_builder"] and info.get("Driver") in ("docker", "docker-container"), "Use an explicitly selected existing local Docker builder, not a remote/Kubernetes builder")
    nodes = info.get("Nodes", [])
    require(len(nodes) == 1 and nodes[0].get("Endpoint") in (manifest["docker_context"], endpoint), "Builder node is not bound to the validated local Docker context")
    require(str(nodes[0].get("Status", "")).lower() == "running", "Central builder must already be running; this script never bootstraps/owns a BuildKit daemon")


def execute(args, manifest):
    validate(args, manifest)
    spec = importlib.util.spec_from_file_location("owned_lifecycle_commands", Path(__file__).with_name("run-bootstrap-lifecycle-acceptance.py"))
    driver = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(driver)
    docker = ["docker", "--context", manifest["docker_context"]]
    inspected = json.loads(driver.command([*docker, "context", "inspect", manifest["docker_context"]]))[0]
    require(inspected["Endpoints"]["docker"]["Host"].startswith("unix://"), "Only an explicit local Docker socket is supported")
    builder = json.loads(driver.command([*docker, "buildx", "inspect", manifest["buildx_builder"], "--format", "{{json .}}"]))
    require_local_builder(builder, manifest, inspected["Endpoints"]["docker"]["Host"])
    args.work_root.mkdir(mode=0o700)
    receipt = {"status": "INCOMPLETE", "source_revision": manifest["source_revision"], "source_sha256": manifest["source_sha256"], "build_jobs": 2, "completed": [], "images": {}, "acceptance": "NOT_RUN", "admission_gates": "UNCHANGED"}
    try:
        unpack_source(manifest["source_archive"], args.work_root / "source")
        for entry in commands(args, manifest):
            driver.command(entry["command"], timeout=args.timeout_seconds)
            name = entry["artifact"]
            if name == "native":
                receipt.update(native_receipt(args.work_root / "native"))
            elif name in IMAGES:
                receipt["images"][name] = oci_receipt(args.work_root / (name + ".oci.tar"))
            receipt["completed"].append(name)
        require(sha(manifest["source_archive"]) == manifest["source_sha256"], "Source archive changed during build; reject these artifacts")
        receipt["status"] = "BUILT_NOT_DEPLOYED"
    finally:
        require(not driver.ACTIVE_GROUPS, "Build client has not joined. Central scheduler must drain its builder; retained output is not complete")
        save(args.work_root / "artifact-receipt.json", receipt)


def main(argv=None):
    try:
        args = arguments(argv)
        manifest = json.loads(args.manifest.read_text())
        print(json.dumps(plan(args, manifest), indent=2))
        if args.execute:
            def interrupted(_signal, _frame):
                raise KeyboardInterrupt
            signal.signal(signal.SIGTERM, interrupted)
            execute(args, manifest)
        return 0
    except InputError as error:
        print(str(error), file=sys.stderr)
        return 1
    except (OSError, ValueError, KeyError, RuntimeError, tarfile.TarError, subprocess.SubprocessError):
        print("Artifact preparation failed; retained receipt is incomplete. Inspect source/toolchain prerequisites and the central builder. No publication or deployment occurred.", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
