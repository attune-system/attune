#!/usr/bin/env python3
"""Bounded diagnostics for exactly one runner-owned Compose project.

Never relay arbitrary log text, inspect Config/Env, or emit State.Error. Logs
may contain selected context; report only fixed diagnostic categories/counts.
"""
import argparse
import json
import re
import subprocess


SERVICES = {
    "worker-shell", "worker-python", "worker-node", "worker-full", "sensor",
    "worker-standalone", "sensor-standalone",
}
SIGNATURES = {
    "sandbox_prerequisite_failed": "workload isolation prerequisite check failed",
    "sandbox_requires_api_artifacts": "bubblewrap workloads require Linux and API artifact transport",
    "sandbox_launcher_unavailable": "isolated workloads require usable /usr/bin/bwrap",
    "sandbox_uid_map_denied": "setting up uid map: Permission denied",
    "sandbox_proc_mount_denied": "mount /proc: Operation not permitted",
    "sandbox_proc_mount_failed": "Can't mount proc on",
    "operation_not_permitted": "Operation not permitted",
    "namespace_creation_denied": "Creating new namespace failed: Operation not permitted",
    "invalid_sandbox_mode": "invalid ATTUNE_WORKLOAD_SANDBOX mode",
    "database_connect_failed": "error communicating with database",
    "connection_refused": "Connection refused",
    "tls_roots_missing": "No CA certificates were loaded from the system",
    "registration_failed": "Failed to register",
    "runtime_detection_failed": "Failed to detect runtimes",
}


def classify(logs):
    # Values in the result are constants, never substrings taken from logs.
    return {key: logs.count(value) for key, value in SIGNATURES.items() if value in logs}


def command(args, ceiling=262144):
    # Bound memory independently of Docker's --tail (one log line can be huge).
    with subprocess.Popen(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as child:
        try:
            # Stream only a bounded prefix; Docker calls also have an
            # independent wall-clock timeout, including blocked reads.
            import threading
            timer = threading.Timer(10, child.kill)
            timer.start()
            try:
                output = child.stdout.read(ceiling + 1)
                if len(output) > ceiling:
                    child.kill()
                result = child.wait(timeout=2)
            finally:
                timer.cancel()
            return output[:ceiling].decode("utf-8", errors="replace"), result, len(output) > ceiling
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def collect(project):
    raw, code, truncated = command([
        "docker", "ps", "-aq", "--filter", f"label=com.docker.compose.project={project}",
    ], 16384)
    if code or truncated:
        return [{"diagnostic": "owned_container_listing_failed"}]
    reports = []
    for container in raw.splitlines()[:32]:
        if not re.fullmatch(r"[0-9a-f]{12,64}", container):
            continue
        # Formatting selects only nonsecret state and ownership labels, never
        # an entire inspect document containing credential-bearing config.
        data, code, truncated = command([
            "docker", "inspect", "--format",
            '{"project":{{json (index .Config.Labels "com.docker.compose.project")}},'
            '"service":{{json (index .Config.Labels "com.docker.compose.service")}},'
            '"state":{{json .State.Status}},"exit":{{.State.ExitCode}},'
            '"oom":{{.State.OOMKilled}},"restarts":{{.RestartCount}}}', container,
        ], 4096)
        try:
            info = json.loads(data) if not code and not truncated else {}
        except ValueError:
            info = {}
        if info.get("project") != project or info.get("service") not in SERVICES:
            continue
        state = info.get("state")
        state = state if state in {"created", "running", "paused", "restarting", "removing", "exited", "dead"} else "unknown"
        report = {"service": info["service"], "state": state}
        for key in ("exit", "restarts"):
            value = info.get(key)
            if type(value) is int and 0 <= value <= 2147483647:
                report[key] = value
        report["oom"] = info.get("oom") is True
        # Docker multiplexes container stdout/stderr into stdout for this CLI
        # request. Neither stream is forwarded verbatim.
        logs, code, truncated = command([
            "docker", "logs", "--tail", "80", container,
        ])
        report["log_categories"] = classify(logs)
        report["log_bytes_examined"] = len(logs.encode("utf-8"))
        report["log_collection_failed"] = code != 0
        report["log_truncated"] = truncated
        reports.append(report)
    return reports or [{"diagnostic": "no_owned_worker_or_sensor_container"}]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--project", required=True)
    args = parser.parse_args()
    if not re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,47}", args.project):
        parser.error("invalid owned Compose project identity")
    for report in collect(args.project):
        print(json.dumps(report, sort_keys=True))


if __name__ == "__main__":
    main()
