"""Release activation and install-prepared launches on owned Bubblewrap pools."""

import json
import os
from pathlib import Path
import uuid

import pytest
import yaml

from helpers.polling import wait_for_condition, wait_for_execution_status

pytestmark = [
    pytest.mark.api,
    pytest.mark.integration,
    pytest.mark.timeout(240),
    pytest.mark.skipif(
        os.getenv("ATTUNE_E2E_INSTALL_PREPARATION") != "1",
        reason="requires owned Bubblewrap action and sensor pools with the install fixture admitted",
    ),
]

RECIPE = Path(__file__).resolve().parents[2] / "fixtures/runtime-services/install_recipe.json"

BUILD = """
import json
import os
from pathlib import Path
import socket
import sys
import uuid

pack, source, output = map(Path, sys.argv[1:])
assert not any(key.startswith("ATTUNE_") and key != "ATTUNE_ARTIFACTS_DIR"
               for key in os.environ)
assert sys.stdin.read() == ""
assert {name for _, name in socket.if_nameindex()} == {"lo"}
try:
    (pack / "unexpected").write_text("modified")
except OSError:
    pass
else:
    raise RuntimeError("Pack input is writable during preparation")
release = (pack / "release.txt").read_text()
(output / "build.json").write_text(json.dumps({
    "release": release, "generation": uuid.uuid4().hex, "entrypoint": source.name,
}))
if release == "fail":
    raise RuntimeError("Build failed after writing partial output")
"""

INVOCATION = """
import json
import os
from pathlib import Path
import sys

pack, output = map(Path, sys.argv[1:])
build = json.loads((output / "build.json").read_text())
assert build["entrypoint"] == Path(__file__).name
assert build["release"] == (pack / "release.txt").read_text()
for directory in (pack, output):
    try:
        (directory / "unexpected").write_text("modified")
    except OSError:
        pass
    else:
        raise RuntimeError("Published files are writable")
for key in ("HOME", "TMPDIR"):
    marker = Path(os.environ[key]) / "invocation"
    assert not marker.exists()
    marker.write_text("private")
from fixture_dependency import dependency
assert dependency == "retained"
assert any(Path(os.environ["PYTHONPYCACHEPREFIX"]).rglob("fixture_dependency*.pyc"))
"""

ACTION = INVOCATION + """
build["input"] = json.load(sys.stdin)["message"]
print(json.dumps(build))
"""

SENSOR = INVOCATION + """
from threading import Event
from urllib.request import Request, urlopen

for trigger in json.load(sys.stdin)["triggers"]:
    body = {
        "trigger_ref": trigger["trigger_ref"],
        "trigger_instance_id": f"rule_{trigger['id']}",
        "config": trigger["config"],
        "payload": {"message": build["release"]},
    }
    request = Request(
        os.environ["ATTUNE_API_URL"].rstrip("/") + "/api/v1/events",
        data=json.dumps(body).encode(),
        headers={
            "Content-Type": "application/json",
            "Authorization": "Bearer " + os.environ["ATTUNE_API_TOKEN"],
        },
        method="POST",
    )
    with urlopen(request, timeout=10) as response:
        assert response.status in (200, 201)
Event().wait()
"""


def _fixture(root, ref):
    for directory in ("actions", "runtimes", "sensors", "triggers"):
        (root / directory).mkdir(parents=True)

    def metadata(path, **fields):
        (root / path).write_text(yaml.safe_dump(fields))

    metadata("runtimes/prepared.yaml", ref=f"{ref}.prepared", name="InstallPreparedPython",
             aliases=["python"], **json.loads(RECIPE.read_text()))
    metadata("actions/read.yaml", ref=f"{ref}.read", label="Read prepared build",
             runner_type=f"{ref}.prepared", entry_point="read.py",
             parameter_delivery="stdin", parameter_format="json", output_format="json",
             parameters={"message": {"type": "string", "required": True}})
    metadata("sensors/watch.yaml", ref=f"{ref}.watch", label="Report prepared build",
             runner_type=f"{ref}.prepared", entry_point="watch.py",
             enabled=False, trigger_types=[f"{ref}.tick"])
    metadata("triggers/tick.yaml", ref=f"{ref}.tick", label="Prepared sensor event",
             type="prepared_fixture", enabled=True, parameters={},
             output={"message": {"type": "string", "required": True}})
    (root / "build.py").write_text(BUILD)
    (root / "actions/read.py").write_text(ACTION)
    (root / "sensors/watch.py").write_text(SENSOR)
    for directory in ("actions", "sensors"):
        (root / directory / "fixture_dependency.py").write_text("dependency = 'retained'\n")

    def release(version, content):
        metadata("pack.yaml", ref=ref, name=ref, version=version)
        (root / "release.txt").write_text(content)

    return release


def test_install_prepares_actions_and_sensors_before_activation(client, tmp_path):
    ref = "install_prep_" + uuid.uuid4().hex[:10]
    set_release = _fixture(tmp_path, ref)

    def run(message):
        execution = client.create_execution(action_ref=f"{ref}.read", parameters={"message": message})
        result = wait_for_execution_status(client, execution["id"], "completed", timeout=30)
        assert result["result"]["input"] == message
        return result["result"]

    set_release("1.0.0", "original")
    assert client.upload_pack(str(tmp_path), skip_tests=True)["version"] == "1.0.0"
    original = run("first")
    assert original["release"] == "original"
    assert run("second")["generation"] == original["generation"]

    rule = client.create_rule(pack_ref=ref, name="watch_prepared", trigger_ref=f"{ref}.tick",
                              action_ref=f"{ref}.read", action_params={"message": "from-sensor"})
    sensor = f"/api/v1/sensors/{ref}.watch"
    response = client._request("POST", sensor + "/enable")
    assert response.status_code == 200, response.text
    try:
        wait_for_condition(
            lambda: any(event.get("payload", {}).get("message") == "original"
                        for event in client.list_events(trigger_ref=f"{ref}.tick", rule_id=rule["id"])),
            timeout=45,
            error_message="Managed sensor did not start with its prepared entrypoint",
        )
    finally:
        response = client._request("POST", sensor + "/disable")
        assert response.status_code == 200, response.text

    set_release("1.1.0", "fail")
    with pytest.raises(Exception, match="Runtime command failed"):
        client.upload_pack(str(tmp_path), force=True, skip_tests=True)
    assert client.get_pack_by_ref(ref)["version"] == "1.0.0"
    assert run("after failure")["generation"] == original["generation"]

    runtime_path = tmp_path / "runtimes/prepared.yaml"
    admitted = runtime_path.read_text()
    unadmitted = yaml.safe_load(admitted)
    unadmitted["execution_config"]["lifecycle"]["env"]["FIXTURE_REVISION"] = "unadmitted"
    runtime_path.write_text(yaml.safe_dump(unadmitted))
    set_release("1.2.0", "unadmitted")
    with pytest.raises(Exception, match="No live runtime preparation storage scope"):
        client.upload_pack(str(tmp_path), force=True, skip_tests=True)
    assert client.get_pack_by_ref(ref)["version"] == "1.0.0"
    assert run("after rejected scope")["generation"] == original["generation"]

    runtime_path.write_text(admitted)
    set_release("1.3.0", "changed input")
    assert client.upload_pack(str(tmp_path), force=True, skip_tests=True)["version"] == "1.3.0"
    updated = run("after update")
    assert updated["release"] == "changed input"
    assert updated["generation"] != original["generation"]
