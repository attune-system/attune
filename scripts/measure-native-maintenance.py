#!/usr/bin/env python3
"""Own PG16/18 servers and collect actual Rust repository workload evidence.

No third-party Python dependencies. No prune, fixed ports, or shared databases.
The JSON summary is derived from preserved JSONL and PostgreSQL statement logs.
"""
import argparse
from collections import defaultdict
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
LABEL = "local.attune.native-workload"


def command(args, *, check=True, **kwargs):
    p = subprocess.run(args, text=True, capture_output=True, timeout=7200, **kwargs)
    if check and p.returncode:
        raise RuntimeError(f"{args[:3]} failed: {p.stdout}\n{p.stderr}")
    return p


def save(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def recover_partial_setup(output, versions):
    """Recover exact stopped/never-started resources from this evidence owner."""
    owner = json.loads((output / "run.json").read_text())["owner"]
    if not re.fullmatch(r"[0-9a-f]{12}", owner):
        raise RuntimeError("invalid evidence owner")
    recovered = []
    for version in versions:
        name = f"attune-native-workload-{owner}-pg{version}"
        volume = name + "-data"
        inspected = command(["docker", "inspect", name], check=False)
        if inspected.returncode == 0:
            info = json.loads(inspected.stdout)[0]
            if info["Config"]["Labels"].get(LABEL) != owner or info["State"]["Running"]:
                raise RuntimeError("refusing foreign or running recovery container")
            recovered.append({"name": name, "state_before": info["State"], "mounts": info["Mounts"]})
            command(["docker", "rm", "-v", name])
        inspected = command(["docker", "volume", "inspect", volume], check=False)
        if inspected.returncode == 0:
            info = json.loads(inspected.stdout)[0]
            if info["Labels"].get(LABEL) != owner:
                raise RuntimeError("refusing foreign recovery volume")
            command(["docker", "volume", "rm", volume])
    result = {"owner": owner, "recovered": recovered,
              "remaining_containers": command(["docker", "ps", "-aq", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines(),
              "remaining_volumes": command(["docker", "volume", "ls", "-q", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines()}
    path = output / f"partial-setup-recovery-{uuid.uuid4().hex[:8]}.json"
    save(path, result)
    print(json.dumps({"evidence": str(path), **result}, indent=2))
    return int(bool(result["remaining_containers"] or result["remaining_volumes"]))


def p95(values):
    if not values:
        return None
    return sorted(values)[math.ceil(len(values) * .95) - 1]


def source_paths():
    return sorted(set([ROOT / "scripts/measure-native-maintenance.py", ROOT / "scripts/measure-timescaledb-removal.py",
                       ROOT / "crates/common/examples/measure_native_workload.rs", ROOT / "Cargo.toml", ROOT / "Cargo.lock",
                       ROOT / "crates/common/Cargo.toml", ROOT / "config.test.yaml",
                       *sorted((ROOT / "migrations").glob("*.sql")), *sorted((ROOT / "crates/common/src").rglob("*.rs"))]))


def source_hashes():
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in source_paths()}


def link_expiry(path, versions):
    original = json.loads((path / "run.json").read_text())
    migration = "migrations/20261006000002_hourly_summaries.sql"
    old = (path / "source" / migration).read_text()
    current = (ROOT / migration).read_text()
    functions = {}
    for name in ["native_partition_check", "native_partition_expire"]:
        pattern = rf"CREATE FUNCTION {name}\(.*?END \$\$;"
        a, b = (re.search(pattern, text, re.S).group(0) for text in [old, current])
        if a != b:
            raise RuntimeError(f"linked expiry protocol changed: {name}")
        functions[name] = hashlib.sha256(a.encode()).hexdigest()
    results = {}
    for version in versions:
        record_path = path / f"pg{version}" / "repository.jsonl"
        records = [json.loads(s) for s in record_path.read_text().splitlines()]
        samples = [r for r in records if r["type"] == "expiry"]
        valid = len(samples) == 4 and {(r["rows"], r["partition"]) for r in samples} == {(n, drop) for n in [10000,1000000] for drop in [False,True]}
        valid = valid and all(r["actual_rows_before"] == r["rows"]+1 and r["actual_rows_after"] == 1 and r["actual_rows_removed"] == r["rows"] and
                              (r["parent_lock_hold_upper_bound_ms"] < 1000 if r["partition"] else True) and
                              sum(c["partitions_dropped"] for c in r["cycles"]) == (1 if r["partition"] else 0) and
                              sum(c["rows_deleted"] for c in r["cycles"]) == (0 if r["partition"] else r["rows"]) for r in samples)
        if not valid:
            raise RuntimeError(f"invalid linked PG{version} expiry cohorts")
        results[str(version)] = {"accepted": True, "repository_jsonl_sha256": hashlib.sha256(record_path.read_bytes()).hexdigest(), "samples": samples}
    return {"path": str(path), "original_owner": original["owner"], "original_source_sha256": original["source_sha256"],
            "unchanged_drop_functions_sha256": functions, "results": results}


def diagnostic_plans(log):
    lines = log.splitlines()
    results = []
    for index, line in enumerate(lines):
        match = re.search(r"\] (nd_\S+) LOG:  duration: ([\d.]+) ms\s+plan:", line)
        if not match:
            continue
        body = []
        for later in lines[index+1:]:
            if re.match(r"^\d{4}-\d\d-\d\d ", later):
                break
            body.append(later)
        try:
            plan, _ = json.JSONDecoder().raw_decode("\n".join(body).lstrip())
        except json.JSONDecodeError:
            continue
        query = plan.get("Query Text", "")
        if not query.startswith("INSERT INTO native_summary_invalidation"):
            continue
        scans = []
        def walk(node):
            if node.get("Relation Name") == "native_summary_invalidation":
                scans.append({k: node[k] for k in ["Node Type", "Index Name", "Index Cond", "Filter", "Actual Rows", "Actual Loops", "Rows Removed by Filter", "Shared Hit Blocks", "Shared Read Blocks"] if k in node})
            for child in node.get("Plans", []):
                walk(child)
        walk(plan["Plan"])
        results.append({"tag": match[1], "instrumented_server_ms": float(match[2]), "query": query, "scans": scans, "acceptance_sample": False})
    return results


def summarize(output):
    contract = json.loads((output.parent / "run.json").read_text())
    profiles = [f"d{d}" for d in contract.get("profiles", [7, 30])]
    records = [json.loads(s) for s in (output / "repository.jsonl").read_text().splitlines()]
    statements = defaultdict(list)
    postgres_log = (output / "postgres.log").read_text()
    for line in postgres_log.splitlines():
        m = re.search(r"\[(\d+)\] (nw_\S+) LOG:  duration: ([\d.]+) ms\s+(.*)", line)
        if m and "set_config('application_name'" not in m[4] and "SET application_name" not in m[4]:
            statements[m[2]].append({"ms": float(m[3]), "statement": m[4]})
    reads = defaultdict(list)
    for r in records:
        if r["type"] == "read":
            r["server_statements"] = statements[r["tag"]]
            r["server_ms"] = sum(s["ms"] for s in r["server_statements"])
            r["coverage_metadata_server_ms"] = sum(s["ms"] for s in r["server_statements"] if "native_summary_hour" in s["statement"])
            r["read_protocol_server_ms"] = sum(s["ms"] for s in r["server_statements"] if any(text in s["statement"] for text in ["BEGIN", "COMMIT", "SET TRANSACTION", "LOCK TABLE", "CURRENT_TIMESTAMP", "SAVEPOINT"]))
            reads[(r["profile"], r["phase"], r["kind"])].append(r)
    results = []
    for (profile, phase, kind), samples in reads.items():
        warm = [r for r in samples if r["sample"] != "first"]
        repository = p95([r["repository_ms"] for r in warm])
        server = p95([r["server_ms"] for r in warm])
        limit = 10 if phase == "summary" else 500
        correct = all(r.get("correct") and r.get("mode_correct") for r in samples)
        results.append({"profile": profile, "phase": phase, "kind": kind,
                        "server_p95_ms": server, "repository_p95_ms": repository,
                        "limit_ms": limit, "sample_count": len(warm), "correct": correct,
                        "coverage_metadata_server_p95_ms": p95([r["coverage_metadata_server_ms"] for r in warm]),
                        "read_protocol_server_p95_ms": p95([r["read_protocol_server_ms"] for r in warm]),
                        "server_gate": len(warm) == 20 and correct and server > 0 and (server < limit if phase == "summary" else server <= limit),
                        "repository_gate": len(warm) == 20 and correct and (repository < limit if phase == "summary" else repository <= limit)})
    pairs = defaultdict(dict)
    for r in records:
        if r["type"] == "ingest":
            pairs[(r["source"], r["batch_rows"], r["pair"])][r["tracking"]] = r
    ingestion = []
    for (source, batch, pair), sides in pairs.items():
        if set(sides) != {False, True}:
            ingestion.append({"source": source, "batch_rows": batch, "pair": pair, "passed": False, "missing_side": True})
            continue
        base, added = sides[False], sides[True]
        wall = added["wall_ms"] / base["wall_ms"]
        latency = added["transaction_p95_ms"] / base["transaction_p95_ms"] if added["transaction_p95_ms"] and base["transaction_p95_ms"] else None
        ingestion.append({"source": source, "batch_rows": batch, "pair": pair,
                          "elapsed_ratio": wall, "p95_ratio": latency,
                          "baseline_rows_per_second": base["rows_per_second"],
                          "tracking_rows_per_second": added["rows_per_second"],
                           "passed": wall <= 1.10 and latency is not None and latency <= 1.10 and not base["errors"] and not added["errors"] and base.get("marker_checks", True) and added.get("marker_checks", True) and len(base["transaction_samples_ms"]) == 400 and len(added["transaction_samples_ms"]) == 400})
    expiry = [{k: v for k, v in r.items() if k not in {"config", "cycles"}} for r in records if r["type"] in {"expiry", "expiry_error", "expiry_lock_wait"}]
    expiry_samples = [r for r in expiry if r["type"] == "expiry"]
    expiry_gate = len(expiry_samples) == 4 and {(r["rows"], r["partition"]) for r in expiry_samples} == {(n, drop) for n in [10000, 1000000] for drop in [False, True]} and all(
        r["actual_rows_before"] == r["rows"] + 1 and r["actual_rows_after"] == 1 and r["actual_rows_removed"] == r["rows"] and (not r["partition"] or r["parent_lock_hold_upper_bound_ms"] < 1000)
        for r in expiry_samples)
    failures = [r for r in records if r["type"] in {"failure", "refresh_error", "expiry_error", "partition_prepare_error"}]
    expected_reads = {(p, phase, k) for p in profiles for phase in ["raw", "summary", "mixed"] for k in ["event_volume", "execution_status"]}
    fingerprints = {}
    fixture_match = True
    relation_names = {"execution_status": "execution_status_hourly", "execution_creation": "execution_throughput_hourly", "event_volume": "event_volume_hourly", "worker_status": "worker_status_hourly"}
    for r in records:
        if r["type"] != "full_oracle":
            continue
        days = r["profile"][1:]
        expected = json.loads((output.parent / f"expected-{days}.json").read_text())[relation_names[r["kind"]]]
        actual = []
        for row in r["actual"]:
            item = [row["bucket"].replace("Z", "+00:00"), row["reference"]]
            if r["kind"] in {"execution_status", "worker_status"}:
                item.append(row["status"])
            item.append(row["count"])
            actual.append(item)
        canonical = lambda rows: json.dumps(sorted(rows, key=lambda row: json.dumps(row)), separators=(",", ":"))
        matched = canonical(actual) == canonical(expected) and r["raw"] == r["actual"]
        fingerprints[f"{r['profile']}.{r['kind']}"] = {"sha256": hashlib.sha256(canonical(actual).encode()).hexdigest(), "independent_fixture_oracle_match": matched}
        fixture_match = fixture_match and matched
    fixture_counts = []
    for r in records:
        if r["type"] == "fixture":
            days = int(r["profile"][1:])
            expected_counts = {"event": days * 40000 + 3, "execution_history": days * 64000,
                               "execution": days * 20000, "enforcement": days * 20000,
                               "audit_event": (90 if days == 30 else days) * 20000,
                               "worker_history": days * 400, "default_event": 0, "default_history": 0, "default_audit": 0}
            fixture_counts.append({"profile": r["profile"], "actual": r["counts"], "expected": expected_counts, "matched": r["counts"] == expected_counts})
    fixture_match = fixture_match and len(fingerprints) == 4 * len(profiles) and len(fixture_counts) == len(profiles) and all(r["matched"] for r in fixture_counts)
    summary = {"reads": results, "ingestion_pairs": ingestion, "expiry": expiry,
               "read_server_gate": len(results) == len(expected_reads) and all(r["server_gate"] for r in results),
               "read_repository_gate": len(results) == len(expected_reads) and all(r["repository_gate"] for r in results),
               "strict_ingestion_gate": len(ingestion) == 24 and all(r["passed"] for r in ingestion),
               "records": len(records), "expiry_gate": expiry_gate, "failures": failures, "missing_read_groups": sorted(expected_reads - set(reads)), "count_fingerprints": fingerprints, "fixture_oracle_gate": fixture_match,
               "production_protocol": [r for r in records if r["type"] == "production_protocol"], "fixture_counts": fixture_counts,
               "materialization": [r for r in records if r["type"] in {"materialization", "materialization_incomplete"}],
               "materialization_first_pass": [r for r in records if r["type"] == "materialization_first_pass"],
               "materialization_cycles": [r for r in records if r["type"] == "materialization_cycle"],
               "setup_attempts": [r for r in records if r["type"] in {"partition_prepare", "partition_prepare_error", "partition_prepare_exhausted"}],
               "fixture_guard_proofs": [r for r in records if r["type"] == "fixture_guard_proof"]}
    summary["ingest_diagnostics"] = [r for r in records if r["type"] == "ingest_diagnostic"]
    summary["all_first_materialization_attempts_on_budget"] = len(summary["materialization"]) == len(profiles) and all(r.get("all_first_attempts_on_budget", False) for r in summary["materialization"])
    summary["all_catchup_attempts_on_budget"] = len(summary["materialization"]) == len(profiles) and all(r.get("all_cycle_attempts_on_budget", False) for r in summary["materialization"])
    plans = diagnostic_plans(postgres_log)
    if plans:
        save(output / "diagnostic-plans.json", plans)
        summary["diagnostic_plan_count"] = len(plans)
    save(output / "read-samples-with-server-statements.json", [r for r in records if r["type"] == "read"])
    summary["production_protocol_gate"] = len(summary["production_protocol"]) == 1 and all(
        r["invalidation_foreign_keys"] == 0 and len(r["trigger_catalog"]) == 9 and all(t["statement_level"] and t["enabled"] == "O" for t in r["trigger_catalog"])
        for r in summary["production_protocol"])
    save(output / "summary.json", summary)
    return summary


def fixtures(output):
    spec = importlib.util.spec_from_file_location("phase1", ROOT / "scripts/measure-timescaledb-removal.py")
    baseline = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(baseline)
    result = {}
    for days in [7, 30]:
        sql = baseline.fixture(argparse.Namespace(days=days, executions_per_day=20000))
        if days == 30:
            # Keep the event/history rate, extend only audit to its default horizon.
            begin = sql.index("INSERT INTO audit_event(")
            end = sql.index("INSERT INTO worker_history(", begin)
            sql = sql[:begin] + """INSERT INTO audit_event(category,event_type,outcome,resource_ref,created,details)
 SELECT 'execution','execution.completed','success','evidence.action_' || g%8,
 '2026-07-08T12:00:00Z'::timestamptz + ((g::bigint-1)*7776000/1800000)*interval '1 second',
 jsonb_build_object('execution_id',g,'attempt',g%3,'duration_ms',g%1000)
 FROM generate_series(1,1800000) g;
""" + sql[end:]
        (output / f"fixture-{days}.sql").write_text(sql)
        result[str(days)] = hashlib.sha256(sql.encode()).hexdigest()
        save(output / f"expected-{days}.json", baseline.expected(argparse.Namespace(days=days, executions_per_day=20000)))
    return result


def run_server(version, owner, output, binary, scope="full", profiles=(7, 30)):
    name = f"attune-native-workload-{owner}-pg{version}"
    volume = name + "-data"
    image = f"postgres:{version}-alpine"
    password = uuid.uuid4().hex
    created_volume = created_container = False
    metadata = {"name": name, "volume": volume, "owner": owner, "image": image}
    try:
        image_info = json.loads(command(["docker", "image", "inspect", image]).stdout)[0]
        metadata["image_id"] = image_info["Id"]
        metadata["image_digests"] = image_info["RepoDigests"]
        if command(["docker", "inspect", name], check=False).returncode == 0 or command(["docker", "volume", "inspect", volume], check=False).returncode == 0:
            raise RuntimeError("refusing existing resource names")
        created_volume = True
        command(["docker", "volume", "create", "--label", f"{LABEL}={owner}", volume])
        mount = "/var/lib/postgresql" if version == 18 else "/var/lib/postgresql/data"
        created_container = True  # An interrupted docker-run can leave a created container.
        command(["docker", "run", "-d", "--pull=never", "--name", name,
                 "--label", f"{LABEL}={owner}", "--cpus", "4", "--memory", "4g", "--shm-size", "256m",
                 "-p", "127.0.0.1::5432", "-v", f"{volume}:{mount}",
                 "-e", f"POSTGRES_PASSWORD={password}", "-e", "POSTGRES_DB=evidence",
                 image, "postgres", "-c", "shared_buffers=256MB", "-c", "work_mem=16MB", "-c", "jit=off",
                 "-c", "max_connections=40", "-c", "log_line_prefix=%m [%p] %a ", "-c", "log_parameter_max_length=0"])
        info = json.loads(command(["docker", "inspect", name]).stdout)[0]
        metadata["limits"] = {k: info["HostConfig"][k] for k in ["NanoCpus", "Memory"]}
        metadata["mounts"] = info["Mounts"]
        port = info["NetworkSettings"]["Ports"]["5432/tcp"][0]["HostPort"]
        metadata["port"] = port
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            ready = command(["docker", "exec", name, "psql", "-h", "127.0.0.1", "-U", "postgres", "-d", "evidence", "-Atc", "SELECT 1"], check=False)
            if ready.returncode == 0:
                break
            time.sleep(.2)
        else:
            raise RuntimeError("TCP readiness timed out")
        metadata["server_settings"] = json.loads(command(["docker", "exec", name, "psql", "-U", "postgres", "-d", "evidence", "-Atc", "SELECT jsonb_build_object('version',version(),'settings',(SELECT jsonb_object_agg(name,setting) FROM pg_settings WHERE name IN ('TimeZone','fsync','synchronous_commit','shared_buffers','work_mem','jit','max_connections','max_parallel_workers_per_gather')))"]).stdout)
        url = f"postgresql://postgres:{password}@127.0.0.1:{port}/evidence"
        env = {**os.environ, "ATTUNE__DATABASE__URL": url, "ATTUNE_TEST_RUN_ID": f"nw{owner[:10]}p{version}"}
        save(output / "server.json", metadata)
        print(f"PG{version}: actual repository workloads started", flush=True)
        with (output / "runner.stdout").open("w") as stdout, (output / "runner.stderr").open("w") as stderr:
            process = subprocess.Popen([str(binary), str(output), scope, ",".join(str(d) for d in profiles)], env=env, cwd=ROOT, stdout=stdout, stderr=stderr)
            try:
                metadata["exit_code"] = process.wait(timeout=7200)
            finally:
                if process.poll() is None:
                    process.terminate()
                    try:
                        process.wait(timeout=30)
                    except subprocess.TimeoutExpired:
                        process.kill()
                        process.wait()
        # Rust awaited clone cleanup. Record all surviving templates/databases/sessions.
        inventory = command(["docker", "exec", name, "psql", "-U", "postgres", "-d", "evidence", "-Atc",
                             "SELECT jsonb_build_object('databases',(SELECT jsonb_agg(datname) FROM pg_database WHERE datname LIKE 'attune_%'),'sessions',(SELECT count(*) FROM pg_stat_activity WHERE datname LIKE 'attune_db_%'))"]).stdout.strip()
        metadata["post_runner_inventory"] = json.loads(inventory)
        clones = [n for n in metadata["post_runner_inventory"].get("databases", []) or [] if n.startswith("attune_db_")]
        if clones or metadata["post_runner_inventory"]["sessions"]:
            raise RuntimeError(f"Rust clone/session leak before teardown: {metadata['post_runner_inventory']}")
    except BaseException as error:
        metadata["error"] = f"{type(error).__name__}: {error}"
    finally:
        cleanup_errors = []
        if created_container:
            try:
                inspected = command(["docker", "inspect", name], check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    if info["Config"]["Labels"].get(LABEL) != owner:
                        raise RuntimeError("container ownership changed")
                    logs = command(["docker", "logs", name])
                    (output / "postgres.log").write_text(logs.stdout + logs.stderr)
                    command(["docker", "stop", "-t", "30", name])
                    command(["docker", "rm", "-v", name])
            except BaseException as error:
                cleanup_errors.append(str(error))
        if created_volume:
            try:
                inspected = command(["docker", "volume", "inspect", volume], check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    if info["Labels"].get(LABEL) != owner:
                        raise RuntimeError("volume ownership changed")
                    command(["docker", "volume", "rm", volume])
            except BaseException as error:
                cleanup_errors.append(str(error))
        metadata["cleanup_errors"] = cleanup_errors
        metadata["remaining_containers"] = command(["docker", "ps", "-aq", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines()
        metadata["remaining_volumes"] = command(["docker", "volume", "ls", "-q", "--filter", f"label={LABEL}={owner}"]).stdout.splitlines()
        save(output / "server.json", metadata)
    if (output / "repository.jsonl").exists() and (output / "postgres.log").exists():
        metadata["summary"] = summarize(output)
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--versions", nargs="+", type=int, choices=[16, 18], default=[16, 18])
    parser.add_argument("--summarize", action="store_true", help="Recompute summaries from an existing evidence directory")
    parser.add_argument("--background", action="store_true", help="Launch an owned run and print its PID and log path")
    parser.add_argument("--compare", type=Path, help="Compare fixture and count fingerprints with another preserved run")
    parser.add_argument("--protocol-only", action="store_true", help="Check and log migrated append triggers without performance samples")
    parser.add_argument("--reads-only", action="store_true", help="Collect both retained read profiles without repeating ingestion or expiry")
    parser.add_argument("--profiles", nargs="+", type=int, choices=[7, 30], default=[7, 30])
    parser.add_argument("--recover", action="store_true", help="Remove only exact stopped partial-setup resources for an existing evidence owner")
    parser.add_argument("--report", action="store_true", help="Print compact clocks, expiry results, and every paired gate from preserved artifacts")
    parser.add_argument("--scope", choices=["full", "acceptance", "reads-only", "ingest-only", "ingest-diagnostic", "protocol-only"])
    parser.add_argument("--expiry-evidence", type=Path, help="Accepted equal-cohort evidence to link for acceptance scope instead of repeating expiry")
    args = parser.parse_args()
    args.output = args.output.resolve()
    if sum([bool(args.scope), args.protocol_only, args.reads_only]) > 1:
        parser.error("choose at most one scope flag")
    scope = args.scope or ("protocol-only" if args.protocol_only else "reads-only" if args.reads_only else "full")
    if scope == "acceptance" and not args.expiry_evidence:
        parser.error("acceptance scope requires --expiry-evidence")
    if len(args.profiles) != len(set(args.profiles)) or len(args.versions) != len(set(args.versions)):
        parser.error("profiles and versions must be distinct")
    if args.recover:
        return recover_partial_setup(args.output, args.versions)
    if args.background:
        args.output.parent.resolve(strict=True)
        log = args.output.with_suffix(".launcher.log")
        with log.open("x") as stream:
            argv = [sys.executable, str(Path(__file__).resolve()), *[a for a in sys.argv[1:] if a != "--background"]]
            process = subprocess.Popen(argv, stdout=stream, stderr=stream, start_new_session=True, cwd=ROOT)
        print(json.dumps({"pid": process.pid, "log": str(log), "output": str(args.output)}))
        return 0
    if args.report:
        for version in args.versions:
            summary = summarize(args.output / f"pg{version}")
            print(json.dumps({"version": version, **{k:summary[k] for k in ["reads", "expiry", "ingestion_pairs", "failures", "missing_read_groups", "fixture_oracle_gate", "production_protocol_gate", "ingest_diagnostics"]}}, indent=2))
        return 0
    if args.summarize:
        for version in args.versions:
            print(json.dumps(summarize(args.output / f"pg{version}"), indent=2))
        return 0
    if args.compare:
        left, right = (json.loads((p / "run.json").read_text()) for p in [args.output, args.compare])
        compared = {"fixture_bytes_identical": left["fixture_fingerprints"] == right["fixture_fingerprints"], "counts": {}}
        for version in args.versions:
            a = summarize(args.output / f"pg{version}")
            b = summarize(args.compare / f"pg{version}")
            compared["counts"][str(version)] = {"available_fingerprints_identical": a["count_fingerprints"] == b["count_fingerprints"], "complete_oracle_coverage": a["fixture_oracle_gate"] and b["fixture_oracle_gate"]}
        print(json.dumps(compared, indent=2))
        return int(not compared["fixture_bytes_identical"] or not all(r["available_fingerprints_identical"] and r["complete_oracle_coverage"] for r in compared["counts"].values()))
    args.output.parent.resolve(strict=True)
    args.output.mkdir()
    owner = uuid.uuid4().hex[:12]
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    signal.signal(signal.SIGHUP, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    contract = {"owner": owner, "started_utc": datetime.now(timezone.utc).isoformat(), "scope": scope, "profiles": args.profiles,
                "versions": args.versions, "ingestion_pairs": 6, "transactions_per_writer": 100,
                "writers": 4, "batch_rows": [1, 25], "samples": 20, "query_concurrency": 4,
                 "aggregation": "nearest-rank p95; every pair must pass both ratios <=1.10; no performance sample exclusions/retries",
                 "setup_reconciliation": {"max_attempts_per_day": 3, "overall_profile_setup_seconds": 600, "only_retry_outcomes": ["DeferredBusy", "DeferredDeadline"], "record_every_attempt": True},
                 "materialization_reconciliation": {"first_pass_attempts_per_kind_hour": 1, "max_catchup_cycles": 96, "overall_seconds": 600, "max_cycle_milliseconds": 5000, "operation_milliseconds": 1000, "fresh_pool_per_cycle": True, "extra_source_warmup": False},
                 "fixture_fingerprints": fixtures(args.output), "source_sha256": {}}
    if args.expiry_evidence:
        contract["linked_expiry"] = link_expiry(args.expiry_evidence.resolve(strict=True), args.versions)
        save(args.output / "linked-expiry.json", contract["linked_expiry"])
    for path in source_paths():
        relative = path.relative_to(ROOT)
        contract["source_sha256"][str(relative)] = hashlib.sha256(path.read_bytes()).hexdigest()
        destination = args.output / "source" / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(path.read_bytes())
    save(args.output / "run.json", contract)
    build = command(["cargo", "build", "-p", "attune-common", "--example", "measure_native_workload"], cwd=ROOT, env={**os.environ,"SQLX_OFFLINE":"true"})
    (args.output / "build.log").write_text(build.stdout + build.stderr)
    binary = ROOT / "target/debug/examples/measure_native_workload"
    contract["binary_sha256"] = hashlib.sha256(binary.read_bytes()).hexdigest()
    frozen_binary = args.output / "measure-native-workload"
    frozen_binary.write_bytes(binary.read_bytes())
    frozen_binary.chmod(0o700)
    binary = frozen_binary
    contract["results"] = {}
    contract["source_after_build_sha256"] = source_hashes()
    if contract["source_after_build_sha256"] != contract["source_sha256"]:
        save(args.output / "run.json", contract)
        raise RuntimeError("source changed during compilation; measurement refused")
    save(args.output / "run.json", contract)
    for version in args.versions:
        output = args.output / f"pg{version}"
        output.mkdir()
        for days in [7, 30]:
            (output / f"fixture-{days}.sql").write_bytes((args.output / f"fixture-{days}.sql").read_bytes())
        before = source_hashes()
        if before != contract["source_sha256"]:
            raise RuntimeError("source changed before server start; measurement refused")
        result = run_server(version, owner, output, binary, scope, args.profiles)
        result["source_before_sha256"] = before
        result["source_after_sha256"] = source_hashes()
        result["source_stable"] = result["source_after_sha256"] == before
        contract["results"][str(version)] = result
        save(args.output / "run.json", contract)
        summary = result.get('summary', {})
        print(f"PG{version}: {json.dumps({'error': result.get('error'), 'reads': summary.get('reads'), 'ingestion_failed_pairs': [r for r in summary.get('ingestion_pairs', []) if not r['passed']], 'fixture_oracle_gate': summary.get('fixture_oracle_gate')})}", flush=True)
        if result.get("error", "").startswith("KeyboardInterrupt"):
            return 1
    summaries = [r.get("summary", {}) for r in contract["results"].values()]
    contract["cross_version_count_match"] = len(summaries) == 2 and all(s.get("fixture_oracle_gate") for s in summaries) and summaries[0]["count_fingerprints"] == summaries[1]["count_fingerprints"]
    save(args.output / "run.json", contract)
    contract["source_after_run_sha256"] = source_hashes()
    contract["source_stable"] = contract["source_after_run_sha256"] == contract["source_sha256"] and all(r["source_stable"] for r in contract["results"].values())
    failed = not contract["source_stable"] or any(r.get("error") or r.get("exit_code") != 0 or r["cleanup_errors"] or r["remaining_containers"] or r["remaining_volumes"] for r in contract["results"].values())
    if scope == "protocol-only":
        failed = failed or not all(s.get("production_protocol_gate") for s in summaries)
    elif scope == "reads-only":
        failed = failed or not all(s.get("read_server_gate") and s.get("read_repository_gate") and s.get("fixture_oracle_gate") and s.get("production_protocol_gate") for s in summaries)
    elif scope == "ingest-only":
        failed = failed or not all(s.get("strict_ingestion_gate") and s.get("production_protocol_gate") for s in summaries)
    elif scope == "ingest-diagnostic":
        failed = failed or not all(s.get("production_protocol_gate") and len(s.get("ingest_diagnostics", [])) == 4 and all(r["correct"] for r in s["ingest_diagnostics"]) for s in summaries)
    elif scope == "acceptance":
        failed = failed or not all(s.get("strict_ingestion_gate") and s.get("read_server_gate") and s.get("read_repository_gate") and s.get("fixture_oracle_gate") and s.get("production_protocol_gate") for s in summaries) or not all(r["accepted"] for r in contract["linked_expiry"]["results"].values())
    else:
        failed = failed or not all(s.get("strict_ingestion_gate") and s.get("read_server_gate") and s.get("fixture_oracle_gate") and s.get("expiry_gate") and s.get("production_protocol_gate") for s in summaries)
    contract["scope_passed"] = not failed
    save(args.output / "run.json", contract)
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
