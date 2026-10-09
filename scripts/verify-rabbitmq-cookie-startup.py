#!/usr/bin/env python3
"""Exercise Compose's real health command before/after fresh broker cookie creation.

Uses one cached broker at a time, no published ports, and explicitly owned volumes.
Evidence is retained even when an assertion fails; existing output is never replaced.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import time
import uuid


LABEL = "attune.rabbit-cookie.owner"
COOKIE = "/var/lib/rabbitmq/.erlang.cookie"
ROOT = Path(__file__).resolve().parent.parent

# The real image entrypoint performs its ownership pass and drops privileges before
# reaching this PATH shim. Only server launch is gated, not entrypoint behavior.
GATED_START = r"""
set -eu
mkdir /tmp/cookie-gate /tmp/cookie-bin
chmod 777 /tmp/cookie-gate
cat > /tmp/cookie-bin/rabbitmq-server <<'SH'
#!/bin/sh
set -eu
id -u > /tmp/cookie-gate/uid
touch /tmp/cookie-gate/arrived
while [ ! -f /tmp/cookie-gate/release ]; do sleep 0.05; done
exec /opt/rabbitmq/sbin/rabbitmq-server
SH
chmod 755 /tmp/cookie-bin/rabbitmq-server
export PATH="/tmp/cookie-bin:$PATH"
exec docker-entrypoint.sh rabbitmq-server
"""


def run(args, *, check=True, env=None, timeout=60):
    result = subprocess.run(args, text=True, capture_output=True, env=env, timeout=timeout)
    if check and result.returncode:
        # Command arguments and subprocess output can contain credentials.
        raise RuntimeError(f"{args[0]} failed with exit {result.returncode}")
    return result


def wait_for(predicate, description, timeout=90):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}")


def amqp_auth():
    """Complete AMQP 0-9-1 authentication and Connection.Open inside the container."""
    # The broker image includes Erlang. Its loopback socket needs no extra image
    # or published host port.
    # Receive whole frames, asserting expected class/method IDs. Both failed
    # authentication and denied vhost access yield Connection.Close instead.
    # Read credentials only inside the container, never embed them in eval text
    # or process arguments. Match the server's advertised connection limits.
    return r"""
    User=list_to_binary(os:getenv("RABBITMQ_DEFAULT_USER")),
    Password=list_to_binary(os:getenv("RABBITMQ_DEFAULT_PASS")),
    VHost=list_to_binary(os:getenv("RABBITMQ_DEFAULT_VHOST")),
    Plain = <<0,User/binary,0,Password/binary>>,
    StartOK = <<10:16,11:16,0:32,5,"PLAIN",(byte_size(Plain)):32,Plain/binary,5,"en_US">>,
    {ok,S}=gen_tcp:connect("127.0.0.1",5672,[binary,{active,false}],5000),
    Read=fun() -> {ok,<<1,0:16,N:32>>}=gen_tcp:recv(S,7,5000),
                    {ok,<<10:16,M:16,Body:(N-4)/binary,206>>}=gen_tcp:recv(S,N+1,5000), {M,Body} end,
    Send=fun(Body) -> gen_tcp:send(S,<<1,0:16,(byte_size(Body)):32,Body/binary,206>>) end,
    ok=gen_tcp:send(S,<<"AMQP",0,0,9,1>>),{10,_}=Read(),
    ok=Send(StartOK),{30,<<Channels:16,FrameMax:32,_Heartbeat:16>>}=Read(),
    ok=Send(<<10:16,31:16,Channels:16,FrameMax:32,0:16>>),
    ok=Send(<<10:16,40:16,(byte_size(VHost)):8,VHost/binary,0,0>>),{41,_}=Read(),
    ok=gen_tcp:close(S),io:format("AMQP authentication and vhost open passed~n"),halt(0).
    """


class Probe:
    def __init__(self, output, service, health):
        self.output = output
        self.service = service
        self.health = health
        self.owner = "rabbit-cookie-" + uuid.uuid4().hex[:16]
        self.secret = str(service["environment"]["RABBITMQ_DEFAULT_PASS"])
        self.report = {"owner": self.owner, "cases": [], "cleanup": []}
        self.container = None
        self.volume = None

    def save(self):
        (self.output / "report.json").write_text(self.redact(json.dumps(self.report, indent=2)) + "\n")

    def redact(self, text):
        text = text.replace(self.secret, "<REDACTED>")
        return re.sub(r"(?i)(cookie hash: )[^\s\\]+", r"\1<REDACTED>", text)

    def execute(self, *args, check=True):
        return run(["docker", "exec", self.container, *args], check=check)

    def metadata(self):
        result = self.execute("stat", "-c", "%u:%g:%a", COOKIE, check=False)
        return result.stdout.strip() if result.returncode == 0 else None

    def state(self):
        return json.loads(run(["docker", "inspect", self.container]).stdout)[0]["State"]

    def cleanup(self):
        errors = []
        if self.container:
            # Stop and await PID 1 before removing the container or its mounts.
            for args in (["stop", "--time", "10", self.container], ["rm", "-v", self.container]):
                result = run(["docker", *args], check=False)
                if result.returncode:
                    errors.append(self.redact(result.stderr))
            self.container = None
        if self.volume:
            result = run(["docker", "volume", "rm", self.volume], check=False)
            if result.returncode:
                errors.append(self.redact(result.stderr))
            self.volume = None
        remaining = {}
        for kind in ("container", "volume", "network"):
            ids = run(["docker", kind, "ls", "-q", "--filter", f"label={LABEL}={self.owner}"]).stdout.split()
            remaining[kind] = ids
        record = {"remaining": remaining, "errors": errors}
        self.report["cleanup"].append(record)
        self.save()
        if errors or any(remaining.values()):
            raise AssertionError("Owned resource cleanup failed; see report.json")

    def case(self, order, health, expect_failure=False, baseline=False):
        name = ("baseline-" if baseline or expect_failure else "configured-") + order
        evidence = {"name": name, "health_command": health, "order": order}
        self.report["cases"].append(evidence)
        try:
            self.volume = run(["docker", "volume", "create", "--label", f"{LABEL}={self.owner}"]).stdout.strip()
            env = os.environ.copy()
            env.update({k: str(v) for k, v in self.service["environment"].items()})
            args = ["docker", "create", "--pull", "never", "--label", f"{LABEL}={self.owner}",
                    "--memory", "512m", "--cpus", "2", "--restart", "no",
                    "--mount", f"type=volume,source={self.volume},target=/var/lib/rabbitmq",
                    "--health-cmd", shlex.join(health),
                    "--health-interval", self.service["healthcheck"]["interval"],
                    "--health-timeout", self.service["healthcheck"]["timeout"],
                    "--health-retries", str(self.service["healthcheck"]["retries"])]
            for key in self.service["environment"]:
                args += ["-e", key]
            if order != "normal":
                args += ["--entrypoint", "sh"]
            args += [self.service["image"]]
            if order != "normal":
                args += ["-c", GATED_START]
            self.container = run(args, env=env).stdout.strip()
            details = json.loads(run(["docker", "inspect", self.container]).stdout)[0]
            evidence["resources"] = {"container": self.container, "mounts": details["Mounts"],
                                     "memory": details["HostConfig"]["Memory"],
                                     "nano_cpus": details["HostConfig"]["NanoCpus"],
                                     "ports": details["HostConfig"]["PortBindings"]}
            assert len(details["Mounts"]) == 1 and details["Mounts"][0]["Name"] == self.volume
            assert not details["HostConfig"]["PortBindings"]
            run(["docker", "start", self.container])
            uid = self.execute("id", "-u", "rabbitmq").stdout.strip()
            gid = self.execute("id", "-g", "rabbitmq").stdout.strip()
            evidence["service_uid"] = uid
            evidence["service_gid"] = gid
            evidence["default_exec_uid"] = self.execute("id", "-u").stdout.strip()
            evidence["privilege_tool"] = self.execute("sh", "-c", "command -v su-exec").stdout.strip()
            entrypoint = self.execute("cat", "/usr/local/bin/docker-entrypoint.sh").stdout
            (self.output / "image-entrypoint.sh").write_text(entrypoint)
            evidence["entrypoint_sha256"] = hashlib.sha256(entrypoint.encode()).hexdigest()
            if order != "normal":
                wait_for(lambda: self.execute("test", "-f", "/tmp/cookie-gate/arrived", check=False).returncode == 0,
                         "entrypoint ownership pass and service UID gate")
                assert self.execute("cat", "/tmp/cookie-gate/uid").stdout.strip() == uid
                evidence["cookie_at_gate"] = self.metadata()
                assert evidence["cookie_at_gate"] is None
                if order == "health-first":
                    early = self.execute("timeout", "5", *health, check=False)
                    evidence["early_health_exit"] = early.returncode
                    (self.output / f"{name}-early-health.log").write_text(self.redact(early.stdout + early.stderr))
                    evidence["cookie_after_early_health"] = self.metadata()
                    assert early.returncode != 0, "A gated server must not be healthy"
                    if expect_failure:
                        assert evidence["cookie_after_early_health"] == "0:0:400"
                self.execute("touch", "/tmp/cookie-gate/release")
            if expect_failure:
                wait_for(lambda: not self.state()["Running"], "baseline startup failure", timeout=30)
                logged = run(["docker", "logs", self.container])
                logs = logged.stdout + logged.stderr
                assert f"Error when reading {COOKIE}: eacces" in logs
                assert self.state()["ExitCode"] != 0
                evidence["verdict"] = "EXPECTED EACCES"
            else:
                if order == "server-first":
                    wait_for(self.metadata, "server-created cookie")
                    evidence["cookie_before_health"] = self.metadata()
                    early = self.execute("timeout", "5", *health, check=False)
                    evidence["early_health_exit"] = early.returncode
                    (self.output / f"{name}-early-health.log").write_text(self.redact(early.stdout + early.stderr))

                def healthy():
                    state = self.state()
                    if not state["Running"]:
                        logs = run(["docker", "logs", self.container], check=False)
                        symptom = f"Error when reading {COOKIE}: eacces"
                        assert symptom not in logs.stdout + logs.stderr, symptom
                        raise AssertionError("Broker exited before healthy")
                    return state.get("Health", {}).get("Status") == "healthy"

                wait_for(healthy, "Docker healthcheck healthy")
                evidence["cookie_final"] = self.metadata()
                assert evidence["cookie_final"] == f"{uid}:{gid}:400"
                if order == "health-first":
                    assert evidence["cookie_after_early_health"] == f"{uid}:{gid}:400"
                evidence["health_status"] = "healthy"
                for cli_user in ("0", uid):
                    # su-exec requires root to set supplementary groups. For an
                    # exec already using the server UID, invoke the CLI directly.
                    command = health
                    if cli_user == uid and health[:2] == ["su-exec", "rabbitmq"]:
                        command = health[2:]
                    result = run(["docker", "exec", "--user", cli_user, self.container,
                                  "timeout", "5", *command], check=False)
                    evidence[f"health_from_uid_{cli_user}"] = result.returncode
                    (self.output / f"{name}-uid-{cli_user}-health.log").write_text(self.redact(result.stdout + result.stderr))
                    assert result.returncode == 0, f"Health CLI failed from UID {cli_user}"
                params = self.service["environment"]
                expression = amqp_auth()
                auth = self.execute("su-exec", "rabbitmq", "erl", "-noshell", "-eval", expression, check=False)
                (self.output / f"{name}-amqp.log").write_text(self.redact(auth.stdout + auth.stderr))
                evidence["amqp_auth_and_vhost"] = auth.returncode
                assert auth.returncode == 0, "AMQP authentication or vhost open failed; see AMQP log"
                evidence["vhost_sha256"] = hashlib.sha256(str(params["RABBITMQ_DEFAULT_VHOST"]).encode()).hexdigest()
                evidence["verdict"] = "PASS"
            print(f"{name}: {evidence['verdict']}", flush=True)
        except BaseException as error:
            evidence["failure"] = {"type": type(error).__name__, "message": self.redact(str(error))}
            raise
        finally:
            try:
                if self.container:
                    logs = run(["docker", "logs", self.container], check=False)
                    (self.output / f"{name}-broker.log").write_text(self.redact(logs.stdout + logs.stderr))
                    evidence["final_state"] = self.state()
                self.save()
            finally:
                self.cleanup()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--compose-file", type=Path, default=ROOT / "docker/distributable/docker-compose.yaml")
    parser.add_argument("--reference-config", type=Path, help="Use retained resolved broker environment/image")
    parser.add_argument("--baseline", action="store_true", help="Prove old root healthcheck fails, then test configured healthcheck")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    config = json.loads(run(["docker", "compose", "-f", str(args.compose_file), "config", "--format", "json"]).stdout)
    service = config["services"]["rabbitmq"]
    health = service["healthcheck"]["test"]
    assert health[0] == "CMD", "Probe requires an exec-form Compose healthcheck"
    if args.reference_config:
        reference = json.loads(args.reference_config.read_text())["services"]["rabbitmq"]
        assert reference["image"] == service["image"]
        service["environment"] = reference["environment"]
    image = json.loads(run(["docker", "image", "inspect", service["image"]]).stdout)[0]
    probe = Probe(args.output, service, health[1:])
    probe.report.update({"image": service["image"], "image_id": image["Id"],
                         "compose_file": str(args.compose_file),
                         "compose_sha256": hashlib.sha256(args.compose_file.read_bytes()).hexdigest(),
                         "image_user": image["Config"].get("User"),
                         "image_home": [v for v in image["Config"]["Env"] if v.startswith("HOME=")],
                         "healthcheck": service["healthcheck"]})
    if args.reference_config:
        probe.report["reference_config_sha256"] = hashlib.sha256(args.reference_config.read_bytes()).hexdigest()
    try:
        if args.baseline:
            probe.case("health-first", ["rabbitmq-diagnostics", "-q", "ping"], expect_failure=True)
            probe.case("server-first", ["rabbitmq-diagnostics", "-q", "ping"], baseline=True)
        for order in ("health-first", "server-first", "normal"):
            probe.case(order, probe.health)
        probe.report["verdict"] = "PASS"
    except BaseException:
        probe.report["verdict"] = "FAIL"
        raise
    finally:
        probe.save()


if __name__ == "__main__":
    main()
