"""Opt-in, matched lifecycle workloads on the runner-owned disposable stack.

Run with -m performance -s and the worker-lifecycle benchmark Compose override.
The report records database-terminal latency and polled MQ completion observation.
MQ observation includes a documented 10ms management-poll interval.
"""
import hashlib
import json
from pathlib import Path
import math
import os
import time
import threading
import uuid
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import quote

import requests
from collections import Counter

import psycopg
import pytest
import yaml

pytestmark = [pytest.mark.api, pytest.mark.performance, pytest.mark.timeout(3600)]
TERMINAL = {"completed", "failed", "cancelled", "timeout", "abandoned"}


def _percentiles(values):
    ordered = sorted(values)
    return {f"p{p}_ms": ordered[min(len(ordered) - 1, math.ceil(len(ordered) * p / 100) - 1)]
            for p in (50, 95, 99)}


def _statements(connection):
    return {query: (calls, elapsed) for query, calls, elapsed in connection.execute(
        "SELECT query, sum(calls)::bigint, sum(total_exec_time) FROM pg_stat_statements "
        "WHERE dbid=(SELECT oid FROM pg_database WHERE datname=current_database()) GROUP BY query"
    )}


class CompletionObserver:
    """Own one broker queue and continuously timestamp both completion signals."""
    def __init__(self, listener):
        self.listener = listener
        self.condition = threading.Condition()
        self.stop = threading.Event()
        self.database = {}
        self.broker = {}
        self.errors = []
        self.session = requests.Session()
        self.session.auth = ("attune", "attune")  # runner-owned test broker
        vhost = quote("attune_" + os.environ["ATTUNE_E2E_RUN_ID"], safe="")
        self.queue = "worker-lifecycle-" + uuid.uuid4().hex
        self.queue_url = f"http://rabbitmq:15672/api/queues/{vhost}/{self.queue}"
        self.bind_url = f"http://rabbitmq:15672/api/bindings/{vhost}/e/attune.executions/q/{self.queue}"
        self.threads = []

    def __enter__(self):
        self.session.put(self.queue_url, json={"durable":False,"auto_delete":False,
            "arguments":{"x-expires":600000}}, timeout=10).raise_for_status()
        try:
            self.session.post(self.bind_url, json={"routing_key":"execution.completed","arguments":{}}, timeout=10).raise_for_status()
            for target in (self.database_loop, self.broker_loop):
                thread = threading.Thread(target=self.run, args=(target,))
                thread.start()
                self.threads.append(thread)
        except BaseException:
            self.__exit__(None, None, None)
            raise
        return self

    def run(self, target):
        try:
            target()
        except BaseException as error:
            with self.condition:
                self.errors.append(str(error))
                self.condition.notify_all()

    def database_loop(self):
        while not self.stop.is_set():
            for notification in self.listener.notifies(timeout=1, stop_after=1):
                value = json.loads(notification.payload)
                if value.get("status") in TERMINAL:
                    with self.condition:
                        self.database[value["id"]] = (time.monotonic(), value["status"])
                        self.condition.notify_all()

    def broker_loop(self):
        while not self.stop.is_set():
            response = self.session.post(self.queue_url + "/get", json={"count":100,
                "ackmode":"ack_requeue_false","encoding":"auto","truncate":100000}, timeout=10)
            response.raise_for_status()
            for message in response.json():
                payload = json.loads(message["payload"])["payload"]
                with self.condition:
                    self.broker[payload["execution_id"]] = (time.monotonic(), payload["status"].lower())
                    self.condition.notify_all()
            self.stop.wait(0.01)

    def wait(self, started, expected_status, require_correct_messages):
        with self.condition:
            complete = self.condition.wait_for(lambda: self.errors or all(
                execution in self.database and execution in self.broker for execution in started), timeout=120)
            assert complete and not self.errors, f"completion observer failed: {self.errors}; pending: {set(started)-self.broker.keys()}"
            records = {}
            for execution, begin in started.items():
                terminal_time, terminal_status = self.database[execution]
                publication_time, publication_status = self.broker[execution]
                assert terminal_status == expected_status, (execution, terminal_status)
                if require_correct_messages:
                    assert publication_status == terminal_status, (execution, publication_status, terminal_status)
                records[execution] = {"database_terminal_ms": (terminal_time-begin)*1000,
                                      "observed_completion_ms": (publication_time-begin)*1000,
                                      "publication_status":publication_status}
            return records

    def __exit__(self, *_):
        self.stop.set()
        for thread in self.threads:
            thread.join(timeout=12)
        alive = [thread.name for thread in self.threads if thread.is_alive()]
        try:
            self.session.delete(self.queue_url, timeout=10).raise_for_status()
        finally:
            self.session.close()
        assert not alive, f"completion observers did not stop: {alive}"


def test_worker_lifecycle_profile(client, tmp_path):
    samples = int(os.getenv("ATTUNE_WORKER_BENCHMARK_SAMPLES", "20"))
    assert samples >= 20, "latency profiles require at least 20 complete samples"
    expect_lazy = os.getenv("ATTUNE_WORKER_BENCHMARK_EXPECT_LAZY") == "1"
    db_url = os.environ["DATABASE_URL"]
    scenarios = [("echo", "core.echo", {"message": "worker-lifecycle-benchmark"}, "completed", 1, 0)]
    pack_ref = "worker_lifecycle_benchmark"
    pack = tmp_path / "pack"
    actions = pack / "actions"
    actions.mkdir(parents=True)
    (pack / "pack.yaml").write_text(yaml.safe_dump({
        "ref": pack_ref, "name": "Worker lifecycle benchmark", "version": "1.0.0",
        "author": "Attune", "description": "Owned benchmark workloads",
    }))
    for name, script, status, stdout, stderr in [
        ("silent_success", "cat >/dev/null; exit 0", "completed", 0, 0),
        ("silent_failure", "cat >/dev/null; exit 7", "failed", 0, 0),
        ("stderr_failure", "cat >/dev/null; printf 'failure\\n' >&2; exit 7", "failed", 0, 1),
        ("both", "cat >/dev/null; printf 'output\\n'; printf 'diagnostic\\n' >&2", "completed", 1, 1),
        ("json_stdout", "cat >/dev/null; printf '{\"answer\":42}\\n'", "completed", 1, 0),
    ]:
        output_format = "json" if name == "json_stdout" else "text"
        (actions / f"{name}.sh").write_text("#!/bin/sh\n" + script + "\n")
        (actions / f"{name}.yaml").write_text(yaml.safe_dump({
            "ref": f"{pack_ref}.{name}", "label": name, "runner_type": "shell",
            "entry_point": f"{name}.sh", "parameters": {}, "output_format": output_format,
        }))
        scenarios.append((name, f"{pack_ref}.{name}", {}, status, stdout, stderr))
    client.upload_pack(str(pack))
    assert client.get_action_by_ref(f"{pack_ref}.json_stdout")["output_format"] == "json"

    report = {"source_revision": os.environ["ATTUNE_WORKER_BENCHMARK_REVISION"],
              "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "samples_per_profile": samples, "clock": "monotonic request to observed PostgreSQL/MQ completion; MQ observation polls management at 10ms",
              "scope": "owned full-stack; SQL totals include orchestration and background activity; phase timings are in worker logs",
              "backend": os.getenv("ATTUNE_WORKER_BENCHMARK_TRANSPORT", "volume"), "profiles": [], "first_invocations": {}}
    # This extension and its counters belong exclusively to this disposable stack.
    with psycopg.connect(db_url, autocommit=True) as observer, psycopg.connect(db_url, autocommit=True) as listener:
        observer.execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
        listener.execute("LISTEN execution_status_changed")

        def cohort(action_ref, parameters, expected_status, concurrency, completion):
            def submit():
                with requests.Session() as session:
                    session.headers.update(client.session.headers)
                    begin = time.monotonic()
                    response = session.post(f"{client.base_url}/api/v1/executions/execute",
                        json={"action_ref":action_ref,"parameters":parameters}, timeout=30)
                    response.raise_for_status()
                    return response.json()["data"]["id"], begin
            with ThreadPoolExecutor(max_workers=concurrency) as pool:
                started = dict(pool.map(lambda _: submit(), range(concurrency)))
            return completion.wait(started, expected_status, expect_lazy)

        with CompletionObserver(listener) as completion:
            for name, reference, parameters, status, stdout, stderr in scenarios:
                # Warm preparation/release state is explicit and excluded from samples.
                report["first_invocations"][name] = cohort(reference, parameters, status, 1, completion)
                warmed = {}
                for _ in range(4):
                    warmed.update(cohort(reference, parameters, status, 4, completion))
                warm_workers = {worker for (worker,) in observer.execute("SELECT DISTINCT worker FROM execution WHERE id=ANY(%s)", (list(warmed),))}
                for concurrency in (1, 4):
                    before = _statements(observer)
                    values = {}
                    while len(values) < samples:
                        values.update(cohort(reference, parameters, status, min(concurrency, samples-len(values)), completion))
                    after = _statements(observer)
                    ids = list(values)
                    streams = observer.execute(
                        "SELECT av.execution, av.meta->>'stream', s.backend::text, count(*) FROM artifact_version av "
                        "JOIN log_stream s ON s.artifact_version=av.id WHERE av.execution=ANY(%s) "
                        "GROUP BY av.execution,av.meta->>'stream',s.backend", (ids,)).fetchall()
                    counts = Counter({(execution, stream): count for execution, stream, backend, count in streams})
                    expected_backend = "shared_file" if report["backend"] == "volume" else "object_segments"
                    assert all(backend == expected_backend for _, _, backend, _ in streams), "storage backend differs from the requested profile"
                    if expect_lazy:
                        for execution in ids:
                            assert counts[execution, "stdout"] == stdout
                            assert counts[execution, "stderr"] == stderr
                    measured_workers = {worker for (worker,) in observer.execute("SELECT DISTINCT worker FROM execution WHERE id=ANY(%s)", (ids,))}
                    assert measured_workers <= warm_workers, "a previously unseen worker entered a warm sample"
                    versions = observer.execute("SELECT id,execution FROM artifact_version WHERE execution=ANY(%s)", (ids,)).fetchall()
                    if expect_lazy:
                        version_counts = Counter(execution for _, execution in versions)
                        for execution in ids:
                            assert version_counts.get(execution, 0) == stdout+stderr
                        if stdout + stderr == 0:
                            assert observer.execute("SELECT count(*) FROM artifact WHERE owner=%s", (reference,)).fetchone()[0] == 0
                        for execution, result in observer.execute("SELECT id,result FROM execution WHERE id=ANY(%s)", (ids,)):
                            capture = result["runtime_log_capture"]
                            assert capture == {"stdout":"captured" if stdout else "empty", "stderr":"captured" if stderr else "empty"}, (execution, capture)
                            if name == "json_stdout":
                                assert result["data"] == {"answer": 42} and not result.get("stdout")
                    deltas = {query: {"calls": calls-before.get(query, (0, 0))[0],
                                      "sql_exec_ms": elapsed-before.get(query, (0, 0))[1]}
                              for query, (calls, elapsed) in after.items()
                              if calls-before.get(query, (0, 0))[0] > 0}
                    report["profiles"].append({"scenario": name, "concurrency": concurrency,
                        "samples": values, "execution_ids": ids, "artifact_versions": dict(versions),
                        **_percentiles([value["observed_completion_ms"] for value in values.values()]), "statement_calls": sum(value["calls"] for value in deltas.values()),
                        "sql_exec_ms": sum(value["sql_exec_ms"] for value in deltas.values()),
                        "statement_deltas": deltas, "allocated_stream_rows": sum(counts.values())})
    print("WORKER_LIFECYCLE_REPORT=" + json.dumps(report, sort_keys=True))
