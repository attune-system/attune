#!/usr/bin/env python3
"""Run actual controller test code in a colocated, ordinary owned container.

The caller builds a static musl test executable from the candidate source. This
driver owns one test image/container/cache volume; it never publishes an image
or modifies daemon policy. The Engine socket belongs only to the trusted test
controller. Workload containers are launched by the production fixed builder.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile
import uuid

BASE = "debian@sha256:7c7b2c966bc9ee8cedfeef67e0e279108992c77681fa595db4a9d65c06ccc587"
PYTHON = "python@sha256:935c3c5b12357a7e10caa9c1e6ca5692344d97cf229832ef584be2cec9fc03df"


def docker(*args, capture=False, check=True):
    return subprocess.run(
        ["docker", *args], check=check, text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", required=True, type=Path)
    args = parser.parse_args()
    binary = args.test_binary.resolve(strict=True)
    dynamic = subprocess.run(["readelf", "-d", str(binary)], check=True, capture_output=True, text=True)
    if "(NEEDED)" in dynamic.stdout:
        raise SystemExit("test executable must be statically linked (build the musl test target)")
    owner = uuid.uuid4().hex
    volume = f"attune-controller-cache-test-{owner}"
    container = f"attune-controller-test-{owner}"
    image = f"attune-controller-boundary-test:{owner}"
    label = f"io.attune.test.owner={owner}"
    volume_created = False
    image_created = False
    print(json.dumps({"owner": owner, "test_binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                      "owned_volume": volume, "owned_container": container, "owned_image": image}), flush=True)
    try:
        docker("pull", PYTHON)
        with tempfile.TemporaryDirectory(prefix="attune-controller-boundary-") as directory:
            context = Path(directory)
            shutil.copyfile(binary, context / "boundary-tests")
            (context / "boundary-tests").chmod(0o755)
            (context / "Dockerfile").write_text(
                f"FROM {PYTHON} AS public-roots\nFROM {BASE}\n"
                "COPY --from=public-roots /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt\n"
                "COPY boundary-tests /usr/local/bin/boundary-tests\n"
                'ENTRYPOINT ["/usr/local/bin/boundary-tests"]\n'
            )
            docker("build", "--label", label, "-t", image, str(context))
            image_created = True
        docker("volume", "create", "--label", label, volume)
        volume_created = True
        docker(
            "run", "--rm", "--name", container, "--label", label,
            "--network", "none", "--read-only", "--cap-drop=ALL",
            "--cap-add=CHOWN", "--cap-add=DAC_OVERRIDE", "--cap-add=FOWNER",
            "--security-opt=no-new-privileges:true",
            "--tmpfs", "/tmp:rw,nosuid,nodev,size=268435456",
            "--mount", f"type=volume,source={volume},target=/var/lib/attune/launch",
            "--mount", "type=bind,source=/var/run/docker.sock,target=/var/run/docker.sock",
            "-e", "ATTUNE_TEST_DOCKER_SOCKET=/var/run/docker.sock",
            "-e", "ATTUNE_TEST_CACHE_ROOT=/var/lib/attune/launch",
            "-e", f"ATTUNE_TEST_CACHE_VOLUME={volume}",
            "-e", f"ATTUNE_TEST_PYTHON_IMAGE={PYTHON}",
            image, "engine::tests::real_engine_cache_binding_and_python_publication",
            "--exact", "--ignored", "--nocapture", "--test-threads=4",
        )
    finally:
        found = docker("inspect", container, capture=True, check=False)
        if found.returncode == 0:
            observed = json.loads(found.stdout)[0]
            if observed["Config"]["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("test container ownership changed; refusing cleanup")
            docker("rm", "-f", container)
        if volume_created:
            observed = json.loads(docker("volume", "inspect", volume, capture=True).stdout)[0]
            if observed["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("test volume ownership changed; refusing cleanup")
            docker("volume", "rm", volume)
        if image_created:
            observed = json.loads(docker("image", "inspect", image, capture=True).stdout)[0]
            if observed["Config"]["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("test image ownership changed; refusing cleanup")
            docker("image", "rm", image)


if __name__ == "__main__":
    main()
