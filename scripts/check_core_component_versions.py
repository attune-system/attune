#!/usr/bin/env python3
"""Require core component versions to change exactly when their inputs change."""

import argparse
import hashlib
import re
import subprocess


def git(*args):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def file_at(ref, path):
    return subprocess.run(
        ["git", "show", f"{ref}:{path}"], check=True, capture_output=True
    ).stdout


def version_at(ref, path, pattern):
    content = file_at(ref, path).decode()
    match = re.search(pattern, content, re.MULTILINE)
    if not match:
        raise RuntimeError(f"version not found in {path} at {ref}")
    return match.group(1)


def normalized_file(ref, path):
    content = file_at(ref, path)
    if path == "crates/core-timer-sensor/Cargo.toml":
        content = re.sub(
            rb'(?m)^(version\s*=\s*")[^"]+("\s*)$', rb"\g<1>VERSION\2", content, count=1
        )
    elif path == "crates/core-timer-sensor/Cargo.lock":
        content = re.sub(
            rb'(?ms)(name = "core-timer-sensor"\nversion = ")[^"]+',
            rb"\g<1>VERSION",
            content,
            count=1,
        )
    elif path == "packs/core/pack.yaml":
        content = re.sub(
            rb'(?m)^(version:\s*)[^\n]+$', rb'\g<1>"VERSION"', content, count=1
        )
    elif path == "packs/core/workflows/install_packs.yaml":
        content = re.sub(
            rb'(?m)^(version:\s*)[^\n]+$', rb'\g<1>"VERSION"', content, count=1
        )
    return content


def tree_digest(ref, path):
    digest = hashlib.sha256()
    files = git("ls-tree", "-r", "--name-only", ref, "--", path).splitlines()
    for file_path in files:
        digest.update(file_path.encode())
        digest.update(b"\0")
        digest.update(normalized_file(ref, file_path))
        digest.update(b"\0")
    return digest.hexdigest()


def file_digest(ref, path):
    return hashlib.sha256(normalized_file(ref, path)).hexdigest()


def timer_toolchain(ref):
    workflow = file_at(ref, ".github/workflows/publish.yml").decode()
    values = []
    for name in ("RUST_TOOLCHAIN", "ZIG_VERSION", "CARGO_ZIGBUILD_VERSION"):
        match = re.search(rf'^  {name}: "([^"]+)"$', workflow, re.MULTILINE)
        if not match:
            return None
        values.append(f"{name}={match.group(1)}")
    return "\n".join(values)


def validate_component(name, old_version, old_digest, new_version, new_digest):
    content_changed = old_digest != new_digest
    version_changed = old_version != new_version
    if content_changed and not version_changed:
        raise RuntimeError(f"{name} inputs changed without a version bump")
    if version_changed and not content_changed:
        raise RuntimeError(f"{name} version changed but its inputs did not")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("base_ref")
    parser.add_argument("--head-ref", default="HEAD")
    args = parser.parse_args()

    timer_manifest = "crates/core-timer-sensor/Cargo.toml"
    timer_pattern = r'^version\s*=\s*"([^"]+)"'
    pack_manifest = "packs/core/pack.yaml"
    pack_pattern = r'^version:\s*"?([^"\s]+)"?'

    old_timer_version = version_at(args.base_ref, timer_manifest, timer_pattern)
    new_timer_version = version_at(args.head_ref, timer_manifest, timer_pattern)
    old_timer_digest = tree_digest(args.base_ref, "crates/core-timer-sensor")
    new_timer_digest = tree_digest(args.head_ref, "crates/core-timer-sensor")
    old_toolchain = timer_toolchain(args.base_ref)
    new_toolchain = timer_toolchain(args.head_ref)
    if old_toolchain is not None and new_toolchain is not None:
        old_timer_digest = hashlib.sha256(
            f"{old_timer_digest}:{old_toolchain}".encode()
        ).hexdigest()
        new_timer_digest = hashlib.sha256(
            f"{new_timer_digest}:{new_toolchain}".encode()
        ).hexdigest()
    validate_component(
        "core timer sensor",
        old_timer_version,
        old_timer_digest,
        new_timer_version,
        new_timer_digest,
    )

    workflow_path = "packs/core/workflows/install_packs.yaml"
    workflow_pattern = r'^version:\s*"?([^"\s]+)"?'
    validate_component(
        "core install_packs workflow",
        version_at(args.base_ref, workflow_path, workflow_pattern),
        file_digest(args.base_ref, workflow_path),
        version_at(args.head_ref, workflow_path, workflow_pattern),
        file_digest(args.head_ref, workflow_path),
    )

    old_pack_version = version_at(args.base_ref, pack_manifest, pack_pattern)
    new_pack_version = version_at(args.head_ref, pack_manifest, pack_pattern)
    old_pack_digest = tree_digest(args.base_ref, "packs/core")
    new_pack_digest = tree_digest(args.head_ref, "packs/core")
    old_pack_digest = hashlib.sha256(
        f"{old_pack_digest}:{old_timer_version}".encode()
    ).hexdigest()
    new_pack_digest = hashlib.sha256(
        f"{new_pack_digest}:{new_timer_version}".encode()
    ).hexdigest()
    validate_component(
        "core pack",
        old_pack_version,
        old_pack_digest,
        new_pack_version,
        new_pack_digest,
    )

    print(
        f"core component versions match their changes since {args.base_ref}: "
        f"timer={new_timer_version}, pack={new_pack_version}"
    )


if __name__ == "__main__":
    main()
