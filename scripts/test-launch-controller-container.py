#!/usr/bin/env python3
"""Run actual backend test code in a colocated, ordinary owned manager container.

The caller builds a static musl test executable from the candidate source. This
driver uses the explicit colocated Unix socket, never a developer Docker
context, and owns its test images/containers/cache volumes; it never publishes an image
or modifies daemon policy. The Engine socket belongs only to the trusted test
manager. Workload containers are launched by the production fixed builder.
"""
import argparse
import hashlib
import json
import os
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
        ["docker", "--host", "unix:///var/run/docker.sock", *args], check=check, text=True,
        stdout=subprocess.PIPE if capture else None,
        stderr=subprocess.PIPE if capture else None,
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--test-binary", required=True, type=Path)
    parser.add_argument("--runner-binary", required=True, type=Path)
    parser.add_argument("--native-http-binary", required=True, type=Path)
    parser.add_argument("--native-http-tests", required=True, type=Path)
    parser.add_argument("--provider-tests", required=True, type=Path)
    parser.add_argument("--native-echo-binary", required=True, type=Path)
    args = parser.parse_args()
    if os.environ.get("TEST_ARCH") not in ("amd64", "arm64"):
        raise SystemExit("owned Docker test requires TEST_ARCH=amd64 or arm64")
    binary = args.test_binary.resolve(strict=True)
    helper = args.runner_binary.resolve(strict=True)
    provider_tests = args.provider_tests.resolve(strict=True)
    echo = args.native_echo_binary.resolve(strict=True)
    native_http = args.native_http_binary.resolve(strict=True)
    native_tests = args.native_http_tests.resolve(strict=True)
    native_script = Path(__file__).resolve().with_name("test_native_http_action.py").resolve(strict=True)
    for executable in (binary, helper, native_http, native_tests, provider_tests, echo):
        dynamic = subprocess.run(["readelf", "-d", str(executable)], check=True, capture_output=True, text=True)
        if "(NEEDED)" in dynamic.stdout:
            raise SystemExit("all candidate controller/helper/native executables must be statically linked (musl)")
    owner = uuid.uuid4().hex
    volume = f"attune-controller-cache-test-{owner}"
    container = f"attune-controller-test-{owner}"
    image = f"attune-controller-boundary-test:{owner}"
    label = f"io.attune.test.owner={owner}"
    payload_image = f"attune-native-docker-payload-test:{owner}"
    wrong_volume = f"attune-native-docker-wrong-test-{owner}"
    network = f"attune-native-docker-test-{owner}"
    payload_created = wrong_created = network_created = False
    socket_gid = os.stat("/var/run/docker.sock").st_gid
    dummy = None
    tests_succeeded = False
    volume_created = False
    image_created = False
    print(json.dumps({"owner": owner,
                      "source": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
                      "provider_tests_sha256": hashlib.sha256(provider_tests.read_bytes()).hexdigest(),
                      "native_echo_sha256": hashlib.sha256(echo.read_bytes()).hexdigest(),
                      "test_binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                      "runner_binary_sha256": hashlib.sha256(helper.read_bytes()).hexdigest(),
                      "native_http_sha256": hashlib.sha256(native_http.read_bytes()).hexdigest(),
                      "native_http_tests_sha256": hashlib.sha256(native_tests.read_bytes()).hexdigest(),
                      "native_http_script_sha256": hashlib.sha256(native_script.read_bytes()).hexdigest(),
                      "owned_volume": volume, "owned_container": container, "owned_image": image}), flush=True)
    try:
        dummy = subprocess.Popen(["/bin/sleep", "3600"], env={"OWNED_DUMMY_PROCESS_TOKEN": uuid.uuid4().hex},
                                 stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        docker("pull", PYTHON)
        docker("pull", NODE)
        with tempfile.TemporaryDirectory(prefix="attune-controller-boundary-") as directory:
            context = Path(directory)
            payload_context = context / "payload"
            payload_context.mkdir()
            shutil.copyfile(helper, payload_context / "attune-workload-runner")
            shutil.copyfile(echo, payload_context / "echo")
            payload_dockerfile = Path(__file__).resolve().parents[1] / "docker/Dockerfile.workload-provider-test"
            docker("build", "--label", label, "-f", str(payload_dockerfile), "-t", payload_image, str(payload_context))
            payload_created = True
            payload_id = json.loads(docker("image", "inspect", payload_image, capture=True).stdout)[0]["Id"]
            assert payload_id.startswith("sha256:") and len(payload_id) == 71, "missing immutable owned candidate config ID"
            print(json.dumps({"owner": owner, "payload_image_id": payload_id,
                              "platform": "linux/" + os.environ["TEST_ARCH"]}), flush=True)
            shutil.copyfile(provider_tests, context / "native-provider-tests")
            shutil.copyfile(binary, context / "boundary-tests")
            shutil.copyfile(helper, context / "attune-workload-runner")
            shutil.copyfile(native_http, context / "native-http")
            shutil.copyfile(native_tests, context / "native-http-tests")
            shutil.copyfile(native_script, context / "http-semantics.py")
            (context / "native-http").chmod(0o755)
            (context / "native-http-tests").chmod(0o755)
            (context / "boundary-tests").chmod(0o755)
            (context / "native-provider-tests").chmod(0o755)
            (context / "attune-workload-runner").chmod(0o755)
            (context / "Dockerfile").write_text(
                f"FROM {PYTHON} AS public-roots\nFROM {BASE}\n"
                "COPY --from=public-roots /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt\n"
                "COPY boundary-tests native-provider-tests /usr/local/bin/\n"
                "COPY attune-workload-runner /usr/local/bin/attune-workload-runner\n"
                "COPY native-http native-http-tests /usr/local/bin/\n"
                "COPY http-semantics.py /usr/local/lib/http-semantics.py\n"
                'ENTRYPOINT ["/usr/local/bin/boundary-tests"]\n'
            )
            docker("build", "--label", label, "-t", image, str(context))
            image_created = True
        docker("volume", "create", "--label", label, volume)
        observed = json.loads(docker("volume", "inspect", volume, capture=True).stdout)[0]
        if observed["Labels"].get("io.attune.test.owner") != owner:
            raise RuntimeError("cache ownership mismatch; refusing initialization")
        volume_created = True
        docker("volume", "create", "--label", label, wrong_volume)
        observed = json.loads(docker("volume", "inspect", wrong_volume, capture=True).stdout)[0]
        if observed["Labels"].get("io.attune.test.owner") != owner:
            raise RuntimeError("wrong-volume ownership mismatch; refusing test")
        wrong_created = True
        docker("network", "create", "--label", label, network)
        network_created = True
        # Trusted bootstrap owns only this new volume. The tested manager then
        # runs as the packaged UID1000 with no capabilities; guests get neither
        # the socket nor cache parent nor bootstrap capabilities.
        docker("run", "--rm", "--name", container + "-init", "--label", label, "--user", "0:0", "--network", "none",
               "--read-only", "--cap-drop=ALL", "--cap-add=CHOWN", "--cap-add=FOWNER",
               "--security-opt=no-new-privileges:true", "--entrypoint", "/bin/sh",
               "--mount", f"type=volume,source={volume},target=/var/lib/attune/launch", image,
               "-c", "chown 1000:1000 /var/lib/attune/launch && chmod 0700 /var/lib/attune/launch")
        base = [
            "run", "--rm", "--name", container, "--label", label,
            "--user", "1000:1000", "--group-add", str(socket_gid),
            "--network", "none", "--read-only", "--cap-drop=ALL",
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
            "-e", "ATTUNE_ISOLATION_TEST_DOCKER_SOCKET=/var/run/docker.sock",
            "-e", "ATTUNE_ISOLATION_TEST_CACHE_ROOT=/var/lib/attune/launch",
            "-e", f"ATTUNE_ISOLATION_TEST_CACHE_VOLUME={volume}",
            "-e", f"ATTUNE_ISOLATION_TEST_WRONG_VOLUME={wrong_volume}",
            "-e", f"ATTUNE_ISOLATION_TEST_NETWORK={network}",
            "-e", f"ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE={payload_id}",
            "-e", f"ATTUNE_ISOLATION_TEST_PLATFORM=linux/{os.environ['TEST_ARCH']}",
            "-e", "ATTUNE_ISOLATION_TEST_RUNNER_PATH=/usr/local/bin/attune-workload-runner",
            "-e", f"ATTUNE_ISOLATION_TEST_DUMMY_PID={dummy.pid}",
        ]
        docker(*base, image, "engine::tests::real_engine_cache_binding_and_python_publication",
               "--exact", "--ignored", "--nocapture", "--test-threads=4")
        native_base = base.copy()
        native_base[native_base.index("--name") + 1] = container + "-native"
        docker(*native_base, "--entrypoint", "/usr/local/bin/native-provider-tests", image,
               "--include-ignored", "--nocapture", "--test-threads=4")
        if dummy.poll() is not None:
            raise RuntimeError("workload signaled the owned dummy credential process")
        tests_succeeded = True
    finally:
        if dummy is not None:
            if dummy.poll() is None:
                dummy.terminate()
            dummy.wait(timeout=10)
        for owned_container in (container, container + "-native", container + "-init"):
            found = docker("inspect", owned_container, capture=True, check=False)
            if found.returncode == 0:
                observed = json.loads(found.stdout)[0]
                if observed["Config"]["Labels"].get("io.attune.test.owner") != owner:
                    raise RuntimeError("test container ownership changed; refusing cleanup")
                docker("rm", "-f", owned_container)
        if volume_created:
            observed = json.loads(docker("volume", "inspect", volume, capture=True).stdout)[0]
            if observed["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("test volume ownership changed; refusing cleanup")
            docker("volume", "rm", volume)
        if wrong_created:
            observed = json.loads(docker("volume", "inspect", wrong_volume, capture=True).stdout)[0]
            if observed["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("wrong-volume ownership changed; refusing cleanup")
            docker("volume", "rm", wrong_volume)
        if network_created:
            observed = json.loads(docker("network", "inspect", network, capture=True).stdout)[0]
            if observed["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("network ownership changed; refusing cleanup")
            docker("network", "rm", network)
        if payload_created:
            observed = json.loads(docker("image", "inspect", payload_image, capture=True).stdout)[0]
            if observed["Config"]["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("payload ownership changed; refusing cleanup")
            docker("image", "rm", payload_image)
        if image_created:
            observed = json.loads(docker("image", "inspect", image, capture=True).stdout)[0]
            if observed["Config"]["Labels"].get("io.attune.test.owner") != owner:
                raise RuntimeError("test image ownership changed; refusing cleanup")
            docker("image", "rm", image)
        print(json.dumps({"owner": owner, "cleanup_ack": tests_succeeded}), flush=True)


if __name__ == "__main__":
    main()
