#!/usr/bin/env python3
"""Crash/restart the actual summary repository on exclusively owned PG16/18 servers.

Builds only native_summary_repository_tests. Runs its explicit crash prerequisite
helper at four test threads. For bootstrap, covered replacement and empty-hour
replacement it observes both discarded cache commits and cache WAL made durable
by the real synchronous schedule repository. Source commits, fsync and the
production one-second operation deadline stay unchanged.

Only the owned proof server disables autovacuum, extends background intervals,
and pauses idle WAL/background/checkpoint writers to make the unflushed branch
observable. No production/acceptance settings are changed. Every case and failure
is recorded; no proof case retries its materialization.

Usage: python3 scripts/prove-native-summary-crash.py --output /tmp/opencode/summary-crash-proof
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import socket
import subprocess
import threading
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
LABEL = "local.attune.summary-crash-proof"


def command(args, *, check=True, timeout=120, **kwargs):
    result = subprocess.run(args, text=True, capture_output=True, timeout=timeout, **kwargs)
    if check and result.returncode:
        raise RuntimeError(f"{args[:3]}: {result.stderr}")
    return result


def save(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def require_owner(name, owner, volume=False):
    args = ["docker", "volume", "inspect", name] if volume else ["docker", "inspect", name]
    info = json.loads(command(args).stdout)[0]
    labels = info["Labels"] if volume else info["Config"]["Labels"]
    if labels.get(LABEL) != owner:
        raise RuntimeError(f"refusing changed/foreign owner: {name}")
    return info


def sql(name, query):
    return command(["docker", "exec", name, "psql", "-X", "-v", "ON_ERROR_STOP=1",
                    "-U", "postgres", "-d", "postgres", "-Atc", query]).stdout.strip()


def ready(name):
    deadline = time.monotonic() + 60
    while command(["docker", "exec", name, "pg_isready", "-h", "127.0.0.1",
                   "-U", "postgres", "-d", "postgres"], check=False).returncode:
        if time.monotonic() >= deadline:
            raise RuntimeError("server readiness deadline")
        time.sleep(0.05)


def pause_idle_flushers(name, owner):
    deadline = time.monotonic() + 30
    expected = {"walwriter", "background writer", "checkpointer"}
    idle = {"WalWriterMain", "BgWriterMain", "BgWriterHibernate", "BgwriterMain", "BgwriterHibernate", "CheckpointerMain"}
    while True:
        rows = json.loads(sql(name, "SELECT json_agg(json_build_object('pid',pid,'type',backend_type,'wait',wait_event)) "
                             "FROM pg_stat_activity WHERE backend_type IN ('walwriter','background writer','checkpointer')"))
        if {r["type"] for r in rows} == expected and all(r["wait"] in idle for r in rows):
            require_owner(name, owner)
            pids = [str(int(r["pid"])) for r in rows]
            command(["docker", "exec", "--user", "postgres", name, "kill", "-STOP", *pids])
            return rows
        if time.monotonic() >= deadline:
            raise RuntimeError(f"flushers did not reach idle readiness: {rows}")
        time.sleep(0.02)


class Probe:
    def __init__(self, binary, env, output):
        self.messages = queue.Queue()
        self.stdout = (output / "helper.stdout").open("w")
        self.stderr = (output / "helper.stderr").open("w")
        self.process = subprocess.Popen([str(binary), "--ignored", "--exact", "async_materialization_crash_probe",
                                         "--test-threads=4", "--nocapture"], env=env, text=True,
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.stderr)
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        for line in self.process.stdout:
            self.stdout.write(line)
            self.stdout.flush()
            start = line.find("{")
            if start >= 0:
                try:
                    self.messages.put(json.loads(line[start:]))
                except json.JSONDecodeError:
                    pass
        self.messages.put(None)

    def wait(self, kind, timeout=300):
        deadline = time.monotonic() + timeout
        while True:
            item = self.messages.get(timeout=max(0.001, deadline - time.monotonic()))
            if item is None:
                raise RuntimeError(f"helper exited before {kind}; inspect helper.stderr")
            if item.get("type") == kind:
                return item

    def send(self, text):
        self.process.stdin.write(text + "\n")
        self.process.stdin.flush()

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=10)
        self.thread.join(timeout=10)
        if self.thread.is_alive():
            raise RuntimeError("helper stdout thread failed to join")
        self.process.stdout.close()
        self.process.stdin.close()
        self.stdout.close()
        self.stderr.close()


def server(version, owner, binary, output):
    name = f"attune-summary-crash-{owner}-pg{version}"
    volume = name + "-data"
    created_container = created_volume = False
    probe = None
    metadata = {"version": version, "owner": owner, "cases": []}
    try:
        if command(["docker", "inspect", name], check=False).returncode == 0:
            raise RuntimeError("refusing existing container")
        if command(["docker", "volume", "inspect", volume], check=False).returncode == 0:
            raise RuntimeError("refusing existing volume")
        command(["docker", "volume", "create", "--label", f"{LABEL}={owner}", volume])
        created_volume = True
        mount = "/var/lib/postgresql/data" if version == 16 else "/var/lib/postgresql"
        # Ephemeral port chosen by the OS, explicitly bound so the same container
        # keeps that port after SIGKILL/start. An allocation race fails setup.
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            host_port = reservation.getsockname()[1]
        # Claim partial setup before run: Docker may create the container and then
        # fail startup. Teardown still verifies ownership before removing anything.
        created_container = True
        command(["docker", "run", "--pull=never", "--detach", "--name", name,
                 "--label", f"{LABEL}={owner}", "--memory=1g", "--cpus=2", "--shm-size=128m",
                 "-e", "POSTGRES_HOST_AUTH_METHOD=trust", "-p", f"127.0.0.1:{host_port}:5432", "-v", f"{volume}:{mount}",
                 f"postgres:{version}-alpine", "-c", "autovacuum=off", "-c", "checkpoint_timeout=1h",
                 "-c", "wal_writer_delay=10s", "-c", "bgwriter_delay=10s", "-c", "wal_buffers=16MB"])
        ready(name)
        port = command(["docker", "port", name, "5432"]).stdout.strip().rsplit(":", 1)[1]
        metadata["settings"] = json.loads(sql(name, "SELECT json_build_object('fsync',current_setting('fsync'),"
            "'synchronous_commit',current_setting('synchronous_commit'),'wal_writer_delay',current_setting('wal_writer_delay'))"))
        if metadata["settings"]["fsync"] != "on" or metadata["settings"]["synchronous_commit"] != "on":
            raise RuntimeError("proof requires default synchronous source commits and fsync")
        for index, (scenario, outcome) in enumerate((s, o) for s in ["bootstrap", "covered", "empty"] for o in ["lost", "retained"]):
            directory = output / f"{index}-{scenario}-{outcome}"
            directory.mkdir()
            record = {"case": scenario, "expected": outcome}
            metadata["cases"].append(record)
            env = {**os.environ, "ATTUNE__DATABASE__URL": f"postgresql://postgres@127.0.0.1:{port}/postgres",
                   "ATTUNE_TEST_RUN_ID": f"sc-{owner}-{index}", "ATTUNE_SUMMARY_CRASH_CASE": scenario,
                   "ATTUNE_SUMMARY_CRASH_OUTCOME": outcome}
            probe = Probe(binary, env, directory)
            record["ready"] = probe.wait("crash_ready")
            record["paused_flushers"] = pause_idle_flushers(name, owner)
            probe.send("refresh")
            record["acknowledged"] = probe.wait("cache_acknowledged")
            if outcome == "retained":
                probe.send("flush_schedule")
                record["schedule"] = probe.wait("schedule_acknowledged")
            # SIGKILL, not graceful shutdown or docker stop. Restart the same container/volume.
            require_owner(name, owner)
            record["kill"] = command(["docker", "kill", "--signal", "KILL", name]).stdout.strip()
            command(["docker", "start", name])
            ready(name)
            if command(["docker", "port", name, "5432"]).stdout.strip().rsplit(":", 1)[1] != port:
                raise RuntimeError("restart changed the owned host port")
            probe.send("recovered")
            record["verified"] = probe.wait("crash_verified")
            record["exit_code"] = probe.process.wait(timeout=120)
            if record["exit_code"]:
                raise RuntimeError("crash helper failed after verification")
            probe.close()
            probe = None
            save(directory / "proof.json", record)
        metadata["inventory"] = json.loads(sql(name, "SELECT json_build_object('clones',(SELECT json_agg(datname) "
            "FROM pg_database WHERE datname LIKE 'attune_db_sc-%'),'sessions',(SELECT count(*) FROM pg_stat_activity "
            "WHERE datname LIKE 'attune_db_sc-%'))"))
        if metadata["inventory"]["clones"] or metadata["inventory"]["sessions"]:
            raise RuntimeError("owned clone/session leak before teardown")
    except BaseException as error:
        metadata["error"] = f"{type(error).__name__}: {error}"
    finally:
        errors = []
        if created_container:
            try:
                if command(["docker", "inspect", name], check=False).returncode == 0:
                    require_owner(name, owner)
                    log = command(["docker", "logs", name], check=False)
                    (output / "postgres.log").write_text(log.stdout + log.stderr)
                    # A failed proof may leave paused flushers. Kill only this owned server.
                    command(["docker", "rm", "--force", "--volumes", name])
            except BaseException as error:
                errors.append(str(error))
        if probe:
            try:
                probe.close()
            except BaseException as error:
                errors.append(str(error))
        if created_volume:
            try:
                require_owner(volume, owner, volume=True)
                command(["docker", "volume", "rm", volume])
            except BaseException as error:
                errors.append(str(error))
        metadata["cleanup_errors"] = errors
        metadata["remaining_containers"] = command(["docker", "ps", "-aq", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines()
        metadata["remaining_volumes"] = command(["docker", "volume", "ls", "-q", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines()
        save(output / "server.json", metadata)
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--versions", nargs="+", type=int, choices=[16, 18], default=[16, 18])
    args = parser.parse_args()
    args.output.parent.resolve(strict=True)
    args.output.mkdir()
    owner = uuid.uuid4().hex[:12]
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    paths = [*sorted((ROOT / "migrations").glob("*.sql")), ROOT / "crates/common/src/repositories/native_maintenance/summaries.rs",
             ROOT / "crates/common/tests/native_summary_repository_tests.rs", Path(__file__)]
    hashes = lambda: {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}
    run = {"owner": owner, "source_before": hashes(), "results": {}}
    for path in paths:
        dest = args.output / "source" / path.relative_to(ROOT)
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, dest)
    build = command(["cargo", "test", "-p", "attune-common", "--test", "native_summary_repository_tests",
                     "--no-run", "--message-format=json"], cwd=ROOT, timeout=2400,
                    env={**os.environ, "ATTUNE_TEST_RUN_ID": "sc-" + owner})
    (args.output / "build.log").write_text(build.stdout + build.stderr)
    executables = [json.loads(line)["executable"] for line in build.stdout.splitlines()
                   if line.startswith("{") and json.loads(line).get("executable")]
    if len(executables) != 1:
        raise RuntimeError(f"unexpected helper executables: {executables}")
    binary = args.output / "native-summary-crash-helper"
    shutil.copy2(executables[0], binary)
    run["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    save(args.output / "run.json", run)
    for version in args.versions:
        directory = args.output / f"pg{version}"
        directory.mkdir()
        result = server(version, owner, binary, directory)
        run["results"][str(version)] = result
        save(args.output / "run.json", run)
        print(json.dumps({"version": version, "error": result.get("error"),
                          "cases": [{"case": r["case"], "expected": r["expected"], "verified": "verified" in r} for r in result["cases"]],
                          "cleanup_errors": result["cleanup_errors"]}), flush=True)
        if result.get("error"):
            break
    run["source_after"] = hashes()
    run["passed"] = run["source_before"] == run["source_after"] and len(run["results"]) == len(args.versions) and all(
        not r.get("error") and not r["cleanup_errors"] and not r["remaining_containers"] and not r["remaining_volumes"]
        and len(r["cases"]) == 6 for r in run["results"].values())
    save(args.output / "run.json", run)
    return int(not run["passed"])


if __name__ == "__main__":
    raise SystemExit(main())
