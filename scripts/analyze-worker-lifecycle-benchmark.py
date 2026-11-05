#!/usr/bin/env python3
"""Export compact lifecycle benchmark evidence from completed, owned run folders.

Inputs are directories containing report.json and manifest.json emitted by the
matched measurement driver. SQLx service logs are optional; missing logs remain
explicit coverage gaps. This reads files only and never contacts a live service.
"""
import argparse
import collections
import csv
import datetime
import hashlib
import json
import math
from pathlib import Path

SCENARIOS = ("echo", "silent_success", "silent_failure", "stderr_failure", "both", "json_stdout")
SERVICES = ("api", "executor", "executor-2", "notifier", "supervisor",
            "worker-shell", "worker-python", "worker-node", "worker-full")


def percentile(values, percent):
    ordered = sorted(values)
    if not ordered:
        return None
    return ordered[math.ceil(len(ordered) * percent / 100) - 1]


def query_summary(query, values):
    return {"query_sha256": hashlib.sha256(query.encode()).hexdigest(),
            "sql_excerpt": " ".join(query.split())[:220], **values}


def write_csv(path, rows):
    if not rows:
        return
    with path.open("w", newline="") as stream:
        writer = csv.DictWriter(stream, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)


def logs(folder, report):
    """Only execution-scoped worker events are attributed to individual jobs."""
    by_execution = collections.defaultdict(lambda: {"client_query_events": 0, "phases": {}})
    services = {}
    graph = collections.defaultdict(lambda: {"hold_ms": [], "admission_wait_ms": [], "unknown": 0})
    windows = [(p["start_wall_time"], p["end_wall_time"]) for p in report["profiles"]]
    for service in SERVICES:
        path = folder / (service + ".log")
        if not path.exists():
            services[service] = {"available": False}
            continue
        first = last = None
        idle_families = collections.Counter()
        for line in path.open(errors="replace"):
            try:
                event = json.loads(line[line.index("{"):])
                stamp = datetime.datetime.fromisoformat(event["timestamp"].replace("Z", "+00:00")).timestamp()
            except (ValueError, KeyError):
                continue
            first = stamp if first is None else min(first, stamp)
            last = stamp if last is None else max(last, stamp)
            fields = event.get("fields", {})
            if fields.get("message") == "Shared graph transaction" and any(a <= stamp <= b for a, b in windows):
                entry = graph[(service, fields["operation"])]
                if fields.get("event") == "owner_dropped":
                    entry["unknown"] += 1
                elif fields.get("event") == "transaction_completed" and fields.get("hold_us") is not None:
                    entry["hold_ms"].append(fields["hold_us"] / 1000)
                    wait = fields.get("admission_wait_us", "None")
                    if isinstance(wait, str) and wait.startswith("Some("):
                        wait = float(wait[5:-1])
                    if isinstance(wait, (int, float)):
                        entry["admission_wait_ms"].append(wait / 1000)
            is_query = "db.statement" in fields
            if is_query and report["idle"]["start_wall_time"] <= stamp <= report["idle"]["end_wall_time"]:
                sql = fields.get("db.statement") or fields.get("summary", "")
                idle_families[" ".join(sql.split())] += 1
            if not service.startswith("worker-"):
                continue
            identity = {}
            for span in event.get("spans", []):
                identity.update(span)
            identity.update(event.get("span", {}))
            identity.update(fields)
            execution = identity.get("execution_id")
            if execution is None:
                continue
            record = by_execution[str(execution)]
            if is_query:
                record["client_query_events"] += 1
            if fields.get("message") == "Worker lifecycle phase":
                phase = fields["phase"]
                record["phases"].setdefault(phase, []).append(fields["elapsed_us"] / 1000)
        services[service] = {
            "available": True, "first_log_time": first, "last_log_time": last,
            # A quiet service can legitimately have no event near the window end.
            # The driver collects logs after completion, before owned teardown.
            "starts_before_idle": first is not None and first <= report["idle"]["start_wall_time"],
            "idle_client_query_events": sum(idle_families.values()),
            "idle_client_query_events_per_minute": sum(idle_families.values()) * 60 / report["idle"]["seconds"],
            "top_idle_query_families": [query_summary(q, {"calls": count})
                                        for q, count in idle_families.most_common(10)],
        }
    return by_execution, services, graph


def validate(report, manifest):
    assert manifest["cleanup_verified"], "Owned teardown not verified"
    assert report["harness_sha256"] == manifest["benchmark_sha256"], "Collector hash mismatch"
    assert report["source_revision"] == manifest["revision"], "Runtime revision mismatch"
    assert report["capture_finalization_budget_ms"] == manifest["capture_budget_ms"]
    assert len(report["enabled_empty_queue_ids"]) == 8
    assert len(set(report["enabled_empty_queue_ids"])) == 8
    expected = {(s, c) for s in SCENARIOS for c in (1, 4)}
    assert {(p["scenario"], p["concurrency"]) for p in report["profiles"]} == expected
    assert len(report["profiles"]) == len(expected)
    seen = set()
    for profile in report["profiles"]:
        ids = {str(i) for i in profile["execution_ids"]}
        assert len(ids) == report["samples_per_profile"]
        assert ids == set(profile["samples"])
        assert not seen.intersection(ids), "Repeated measured execution"
        seen.update(ids)
        assert profile["p50_ms"] == percentile([x["observed_completion_ms"] for x in profile["samples"].values()], 50)
        deltas = profile["statement_deltas"].values()
        assert sum(x["calls"] for x in deltas if x["toplevel"]) == profile["top_level_statement_calls"]
        assert sum(x["calls"] for x in deltas if not x["toplevel"]) == profile["nested_statement_calls"]
        assert all(x["calls"] > 0 and x["sql_exec_ms"] >= 0 for x in deltas)
    assert len(manifest["verified_runtime_images"]) == 7, "Runtime provenance incomplete"
    return len(seen)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("folders", nargs="+", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--skip-logs", action="store_true", help="Counters/latency only; record missing attribution")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    runs, profiles, samples, idle, phase_rows, graph_rows = [], [], [], [], [], []
    collectors = set()
    for folder in args.folders:
        report = json.loads((folder / "report.json").read_text())
        manifest = json.loads((folder / "manifest.json").read_text())
        count = validate(report, manifest)
        collectors.add(report["harness_sha256"])
        assert len(collectors) == 1, "Collectors differ across compared runs"
        key = {"kind": manifest["kind"], "backend": manifest["backend"],
               "budget_ms": manifest["capture_budget_ms"]}
        execution_logs, service_logs, graph = ({}, {}, {}) if args.skip_logs else logs(folder, report)
        for (service, operation), entry in sorted(graph.items()):
            graph_rows.append({**key, "service": service, "operation": operation,
                               "completed_holders": len(entry["hold_ms"]), "unknown_owner_drops": entry["unknown"],
                               "hold_sum_ms": sum(entry["hold_ms"]),
                               "hold_p50_ms": percentile(entry["hold_ms"], 50),
                               "hold_p95_ms": percentile(entry["hold_ms"], 95),
                               "admission_wait_p50_ms": percentile(entry["admission_wait_ms"], 50),
                               "admission_wait_p95_ms": percentile(entry["admission_wait_ms"], 95)})
        run = {**key, "run": manifest["run"], "revision": manifest["revision"],
               "samples": count, "samples_per_profile": report["samples_per_profile"],
               "harness_sha256": report["harness_sha256"], "overlay_sha256": manifest["overlay_sha256"],
               "report_sha256": hashlib.sha256((folder / "report.json").read_bytes()).hexdigest(),
               "started_utc": manifest["started_utc"], "host": manifest["host"], "docker": manifest["docker"],
               "runtime_images": manifest["verified_runtime_images"], "provenance_basis": manifest["provenance_basis"],
               "cleanup_verified": True, "postgres_settings": report["postgres_settings"],
               "reader_exit_code": manifest["stream_discovery_exit_code"],
               "first_invocations": report["first_invocations"], "service_logs": service_logs}
        active_queries = collections.defaultdict(lambda: {"calls": 0, "sql_exec_ms": 0, "wal_bytes": 0})
        for profile in report["profiles"]:
            n = len(profile["execution_ids"])
            case = {**key, "scenario": profile["scenario"], "concurrency": profile["concurrency"]}
            row = {**case, "n": n, "seconds": profile["end_wall_time"] - profile["start_wall_time"],
                   "allocated_stream_rows": profile["allocated_stream_rows"]}
            for metric in ("database_terminal_ms", "observed_completion_ms"):
                values = [s[metric] for s in profile["samples"].values()]
                for p in (50, 95, 99):
                    row[metric.replace("_ms", f"_p{p}_ms")] = percentile(values, p)
            for name in ("top_level_statement_calls", "nested_statement_calls", "observer_statement_calls",
                         "top_level_exec_ms", "nested_exec_ms", "top_level_wal_bytes"):
                row[name + "_per_job"] = profile[name] / n
            for block in ("shared_blks_read", "shared_blks_hit", "rows"):
                row["top_level_" + block + "_per_job"] = sum(
                    x[block] for x in profile["statement_deltas"].values() if x["toplevel"]) / n
            row["correlated_worker_client_events_per_job"] = sum(
                execution_logs.get(str(i), {}).get("client_query_events", 0) for i in profile["execution_ids"]) / n
            phases = collections.defaultdict(list)
            for execution, sample in profile["samples"].items():
                record = execution_logs.get(execution, {})
                samples.append({**case, "execution_id": execution, **sample,
                                "correlated_worker_client_events": record.get("client_query_events", 0)})
                for name, values in record.get("phases", {}).items():
                    phases[name].extend(values)
            for name, values in sorted(phases.items()):
                phase_rows.append({**case, "phase": name, "events": len(values),
                                   "p50_ms": percentile(values, 50), "p95_ms": percentile(values, 95)})
            for query, delta in profile["statement_deltas"].items():
                if not delta["toplevel"] or delta["observer"]:
                    continue
                for metric in ("calls", "sql_exec_ms", "wal_bytes"):
                    active_queries[query][metric] += delta[metric]
            profiles.append(row)
        run["top_active_queries_by_calls"] = [query_summary(q, d) for q, d in sorted(
            active_queries.items(), key=lambda item: item[1]["calls"], reverse=True)[:20]]
        run["top_active_queries_by_time"] = [query_summary(q, d) for q, d in sorted(
            active_queries.items(), key=lambda item: item[1]["sql_exec_ms"], reverse=True)[:20]]
        runs.append(run)
        window = report["idle"]
        idle.append({**key, **{k: v for k, v in window.items() if k != "statement_deltas"},
                     "top_queries_by_calls": [query_summary(q, d) for q, d in sorted(
                         window["statement_deltas"].items(), key=lambda item: item[1]["calls"], reverse=True)[:20]]})
    write_csv(args.output / "profiles.csv", profiles)
    write_csv(args.output / "samples.csv", samples)
    write_csv(args.output / "phases.csv", phase_rows)
    write_csv(args.output / "graph-transactions.csv", graph_rows)
    result = {"format": 1, "runs": runs, "idle": idle,
              "notes": ["Top-level counters include whole-stack background work; they are not attributed worker round trips.",
                        "Nested server time/WAL overlap parent accounting and must not be added as independent elapsed cost.",
                        "Observer calls are included in top-level counts and separately identified.",
                        "Execution-scoped SQLx events and lifecycle phase logs have partial attribution coverage.",
                        "Sensor SQLx DEBUG logging was not enabled; its work remains in whole-database counters.",
                        "Graph hold is owner-observed first admission through acknowledged completion, including SQL, I/O and scheduling; holders overlap."]}
    (args.output / "runs.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Validated {len(runs)} runs, {len(profiles)} profiles, {len(samples)} measured executions")


if __name__ == "__main__":
    main()
