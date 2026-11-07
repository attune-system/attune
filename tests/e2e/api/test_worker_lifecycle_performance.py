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
    # Keep nested SQL distinct; its execution time overlaps its parent's time.
    rows = connection.execute(
        "SELECT query, toplevel, sum(calls)::bigint, sum(total_exec_time), "
        "sum(wal_bytes), sum(shared_blks_read), sum(shared_blks_hit), sum(rows) "
        "FROM pg_stat_statements "
        "WHERE dbid=(SELECT oid FROM pg_database WHERE datname=current_database()) "
        "GROUP BY query,toplevel"
    )
    return {query + (" /* profile:top */" if top else " /* profile:nested */"):
            (calls, elapsed, int(wal), int(read), int(hit), int(count))
            for query, top, calls, elapsed, wal, read, hit, count in rows}


def _stats_epoch(connection):
    dealloc, reset = connection.execute(
        "SELECT dealloc,stats_reset FROM pg_stat_statements_info"
    ).fetchone()
    return {"dealloc": dealloc, "stats_reset": reset.isoformat() if reset is not None else None}


def _statement_deltas(before, after):
    return {query: {"calls": values[0]-before.get(query, (0,)*6)[0],
                    "sql_exec_ms": values[1]-before.get(query, (0,)*6)[1],
                    "wal_bytes": values[2]-before.get(query, (0,)*6)[2],
                    "shared_blks_read": values[3]-before.get(query, (0,)*6)[3],
                    "shared_blks_hit": values[4]-before.get(query, (0,)*6)[4],
                    "rows": values[5]-before.get(query, (0,)*6)[5],
                    "toplevel": query.endswith("/* profile:top */"),
                    "observer": "pg_stat_statements" in query}
            for query, values in after.items()
            if values[0]-before.get(query, (0,)*6)[0] > 0}


def _statement_totals(deltas):
    return {"statement_calls": sum(x["calls"] for x in deltas.values()),
            "sql_exec_ms": sum(x["sql_exec_ms"] for x in deltas.values()),
            "top_level_statement_calls": sum(x["calls"] for x in deltas.values() if x["toplevel"]),
            "nested_statement_calls": sum(x["calls"] for x in deltas.values() if not x["toplevel"]),
            "top_level_exec_ms": sum(x["sql_exec_ms"] for x in deltas.values() if x["toplevel"]),
            "nested_exec_ms": sum(x["sql_exec_ms"] for x in deltas.values() if not x["toplevel"]),
            "observer_statement_calls": sum(x["calls"] for x in deltas.values() if x["observer"]),
            "top_level_wal_bytes": sum(x["wal_bytes"] for x in deltas.values() if x["toplevel"])}


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
        try:
            # Supported by the broker's default policy, including RabbitMQ 4.
            self.session.put(self.queue_url, json={"durable":True,"auto_delete":False,
                "arguments":{"x-expires":600000,"x-queue-type":"classic"}}, timeout=10).raise_for_status()
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
            response = self.session.delete(self.queue_url, timeout=10)
            if response.status_code != 404:
                response.raise_for_status()
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

    report = {"source_revision": os.environ["ATTUNE_WORKER_BENCHMARK_REVISION"],
              "harness_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "capture_finalization_budget_ms": int(os.environ["ATTUNE_WORKER_BENCHMARK_LOG_FINALIZATION_MS"]),
              "samples_per_profile": samples, "clock": "monotonic request to observed PostgreSQL/MQ completion; MQ observation polls management at 10ms",
              "scope": "owned full-stack; SQL totals include orchestration and background activity; MQ uses an additional expiring durable classic observer queue; phase timings are in worker logs",
              "backend": os.getenv("ATTUNE_WORKER_BENCHMARK_TRANSPORT", "volume"), "profiles": [], "first_invocations": {}}
    # This extension and its counters belong exclusively to this disposable stack.
    with psycopg.connect(db_url, autocommit=True) as observer, psycopg.connect(db_url, autocommit=True) as listener:
        assert observer.execute("SELECT output_format::text FROM action WHERE ref=%s", (f"{pack_ref}.json_stdout",)).fetchone()[0] == "json"
        expected_workers = {f"worker-{kind}-01" for kind in ("shell", "python", "node", "full")}
        readiness_deadline = time.monotonic() + 180
        while True:
            active_workers = {name for (name,) in observer.execute("SELECT name FROM worker WHERE status IN ('active','busy')")}
            if expected_workers <= active_workers:
                break
            assert time.monotonic() < readiness_deadline, f"benchmark workers not ready: {sorted(expected_workers-active_workers)}"
            time.sleep(0.1)
        observer.execute("CREATE EXTENSION IF NOT EXISTS pg_stat_statements")
        listener.execute("LISTEN execution_status_changed")
        report["postgres_settings"] = dict(observer.execute(
            "SELECT name,setting FROM pg_settings WHERE name IN "
            "('server_version','pg_stat_statements.track','pg_stat_statements.max',"
            "'fsync','synchronous_commit','shared_buffers','max_connections')"))

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

        # Include enabled empty queues so the capacity-query optimization is
        # actually exercised during idle and execution measurements.
        idle_queues = []
        for index in range(8):
            reference = f"adhoc.database_measurement_{index}"
            response = client.session.post(f"{client.base_url}/api/v1/queues", json={
                "ref": reference, "label": "Database measurement idle queue",
                "dispatch_action_ref": "core.echo", "enabled": True,
                "accepting_new_items": True, "batch_mode": "single",
                "item_schema": {}, "config": {"ack_contract": {"version": 2}},
            }, timeout=30)
            assert response.status_code == 201, response.text
            idle_queues.append(response.json()["data"]["id"])
        report["enabled_empty_queue_ids"] = idle_queues
        response = client.session.get(f"{client.base_url}/api/v1/retention-config", timeout=30)
        response.raise_for_status()
        report["runtime_retention_config"] = response.json()["data"]
        report["maintenance_schedule_before_idle"] = [
            {"job": job, "next_due": due.isoformat(), "last_success": success.isoformat() if success else None,
             "deferred_since": deferred.isoformat() if deferred else None}
            for job, due, success, deferred in observer.execute(
                "SELECT job,next_due,last_success,deferred_since FROM native_maintenance_schedule ORDER BY job")]
        settle_seconds = int(os.getenv("ATTUNE_WORKER_BENCHMARK_SETTLE_SECONDS", "60"))
        idle_seconds = int(os.getenv("ATTUNE_WORKER_BENCHMARK_IDLE_SECONDS", "60"))
        assert 0 <= settle_seconds <= 3600 and 1 <= idle_seconds <= 3600
        report["settle_seconds"] = settle_seconds
        report["requested_idle_seconds"] = idle_seconds
        # Fixed-duration observation, not readiness synchronization. Both revisions
        # use the same settling and measurement intervals with identical replicas.
        time.sleep(settle_seconds)
        idle_epoch = _stats_epoch(observer)
        idle_before = _statements(observer)
        idle_start_wall = time.time()
        idle_start = time.monotonic()
        time.sleep(idle_seconds)
        idle_after = _statements(observer)
        assert _stats_epoch(observer) == idle_epoch, "idle counters reset or evicted"
        idle = _statement_deltas(idle_before, idle_after)
        report["idle"] = {"start_wall_time": idle_start_wall, "end_wall_time": time.time(), "seconds": time.monotonic()-idle_start,
                          **_statement_totals(idle), "statement_deltas": idle}
        capacity = sum(value["calls"] for query, value in idle.items()
                       if "SELECT COUNT(*) FROM work_queue_dispatch" in " ".join(query.split()))
        readiness = sum(value["calls"] for query, value in idle.items()
                        if "SELECT candidate.queue FROM unnest" in " ".join(query.split()))
        assert capacity or readiness, "idle queues did not exercise queue observation"
        report["idle"]["queue_capacity_calls"] = capacity
        report["idle"]["queue_readiness_calls"] = readiness
        report["maintenance_schedule_after_idle"] = [
            {"job": job, "next_due": due.isoformat(), "last_success": success.isoformat() if success else None,
             "deferred_since": deferred.isoformat() if deferred else None}
            for job, due, success, deferred in observer.execute(
                "SELECT job,next_due,last_success,deferred_since FROM native_maintenance_schedule ORDER BY job")]
        report["sql_timing_note"] = "Top-level and nested times overlap; summed sql_exec_ms is legacy inclusive work, not independent elapsed time."
        with CompletionObserver(listener) as completion:
            for name, reference, parameters, status, stdout, stderr in scenarios:
                # Warm preparation/release state is explicit and excluded from samples.
                report["first_invocations"][name] = cohort(reference, parameters, status, 1, completion)
                warmed = {}
                for _ in range(4):
                    warmed.update(cohort(reference, parameters, status, 4, completion))
                warm_workers = {worker for (worker,) in observer.execute("SELECT DISTINCT worker FROM execution WHERE id=ANY(%s)", (list(warmed),))}
                for concurrency in (1, 4):
                    epoch = _stats_epoch(observer)
                    profile_start_wall = time.time()
                    before = _statements(observer)
                    values = {}
                    while len(values) < samples:
                        values.update(cohort(reference, parameters, status, min(concurrency, samples-len(values)), completion))
                    after = _statements(observer)
                    assert _stats_epoch(observer) == epoch, "profile counters reset or evicted"
                    ids = list(values)
                    streams = observer.execute(
                        "SELECT av.execution, av.meta->>'stream', s.backend::text, count(*), bool_and(s.sealed AND av.body_state='ready') FROM artifact_version av "
                        "JOIN log_stream s ON s.artifact_version=av.id WHERE av.execution=ANY(%s) "
                        "GROUP BY av.execution,av.meta->>'stream',s.backend", (ids,)).fetchall()
                    counts = Counter({(execution, stream): count for execution, stream, backend, count, _ in streams})
                    expected_backend = "shared_file" if report["backend"] == "volume" else "object_segments"
                    assert all(backend == expected_backend for _, _, backend, _, _ in streams), "storage backend differs from the requested profile"
                    assert all(ready for _, _, _, _, ready in streams), "measured output must be sealed and ready"
                    results = observer.execute("SELECT id,result FROM execution WHERE id=ANY(%s)", (ids,)).fetchall()
                    assert all(not result.get("logs_incomplete", False) for _, result in results), "measured output capture must be complete"
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
                        for execution, result in results:
                            capture = result["runtime_log_capture"]
                            assert capture == {"stdout":"captured" if stdout else "empty", "stderr":"captured" if stderr else "empty"}, (execution, capture)
                            if name == "json_stdout":
                                assert result["data"] == {"answer": 42} and not result.get("stdout")
                    deltas = _statement_deltas(before, after)
                    report["profiles"].append({"scenario": name, "concurrency": concurrency,
                        "start_wall_time": profile_start_wall, "end_wall_time": time.time(),
                        "samples": values, "execution_ids": ids, "artifact_versions": dict(versions),
                        **_percentiles([value["observed_completion_ms"] for value in values.values()]), **_statement_totals(deltas),
                        "statement_deltas": deltas, "allocated_stream_rows": sum(counts.values())})
                    print("WORKER_LIFECYCLE_PARTIAL_REPORT=" + json.dumps(report, sort_keys=True), flush=True)
    print("WORKER_LIFECYCLE_REPORT=" + json.dumps(report, sort_keys=True))
