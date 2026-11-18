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
NODE = "node@sha256:e9f565d32859f0cbfb52125eafe8be39390f24dac302dc3cdf566e8ba3ab21ee"
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
    parser.add_argument("--runner-binary", required=True, type=Path)
    parser.add_argument("--native-http-binary", required=True, type=Path)
    parser.add_argument("--native-http-tests", required=True, type=Path)
    args = parser.parse_args()
    binary = args.test_binary.resolve(strict=True)
    helper = args.runner_binary.resolve(strict=True)
    native_http = args.native_http_binary.resolve(strict=True)
    native_tests = args.native_http_tests.resolve(strict=True)
    native_script = Path(__file__).resolve().with_name("test_native_http_action.py").resolve(strict=True)
    for executable in (binary, helper, native_http, native_tests):
        dynamic = subprocess.run(["readelf", "-d", str(executable)], check=True, capture_output=True, text=True)
        if "(NEEDED)" in dynamic.stdout:
            raise SystemExit("all candidate controller/helper/native executables must be statically linked (musl)")
    owner = uuid.uuid4().hex
    volume = f"attune-controller-cache-test-{owner}"
    container = f"attune-controller-test-{owner}"
    image = f"attune-controller-boundary-test:{owner}"
    label = f"io.attune.test.owner={owner}"
    volume_created = False
    image_created = False
    print(json.dumps({"owner": owner, "test_binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                      "runner_binary_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
                      "native_http_sha256": hashlib.sha256(native_http.read_bytes()).hexdigest(),
                      "native_http_tests_sha256": hashlib.sha256(native_tests.read_bytes()).hexdigest(),
                      "native_http_script_sha256": hashlib.sha256(native_script.read_bytes()).hexdigest(),
                      "owned_volume": volume, "owned_container": container, "owned_image": image}), flush=True)
    try:
        docker("pull", PYTHON)
        docker("pull", NODE)
        with tempfile.TemporaryDirectory(prefix="attune-controller-boundary-") as directory:
            context = Path(directory)
            shutil.copyfile(binary, context / "boundary-tests")
            shutil.copyfile(helper, context / "attune-workload-runner")
            shutil.copyfile(native_http, context / "native-http")
            shutil.copyfile(native_tests, context / "native-http-tests")
            shutil.copyfile(native_script, context / "http-semantics.py")
            (context / "native-http").chmod(0o755)
            (context / "native-http-tests").chmod(0o755)
            (context / "boundary-tests").chmod(0o755)
            (context / "attune-workload-runner").chmod(0o755)
            (context / "Dockerfile").write_text(
                f"FROM {PYTHON} AS public-roots\nFROM {BASE}\n"
                "COPY --from=public-roots /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt\n"
                "COPY boundary-tests /usr/local/bin/boundary-tests\n"
                "COPY attune-workload-runner /usr/local/bin/attune-workload-runner\n"
                "COPY native-http native-http-tests /usr/local/bin/\n"
                "COPY http-semantics.py /usr/local/lib/http-semantics.py\n"
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
            "-e", f"ATTUNE_TEST_NODE_IMAGE={NODE}",
            "-e", "ATTUNE_TEST_NATIVE_HTTP_BIN=/usr/local/bin/native-http",
            "-e", "ATTUNE_TEST_NATIVE_HTTP_TESTS=/usr/local/bin/native-http-tests",
            "-e", "ATTUNE_TEST_NATIVE_HTTP_SCRIPT=/usr/local/lib/http-semantics.py",
            "-e", "ATTUNE_TEST_WORKLOAD_RUNNER_BIN=/usr/local/bin/attune-workload-runner",
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
