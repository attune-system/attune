#!/usr/bin/env python3
"""Replay a diagnostic fixture on owned Timescale and stock PostgreSQL containers.

No Python packages, host psql, Cargo build, services, or existing database needed.
See docs/research/timescaledb-removal-workload.md for the measurement contract.
"""

import argparse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
ANCHOR = datetime(2026, 10, 6, 12, tzinfo=timezone.utc)
AGGREGATES = {
    "execution_status_hourly": ("action_ref", "new_status", "transition_count"),
    "execution_throughput_hourly": ("action_ref", None, "execution_count"),
    "event_volume_hourly": ("trigger_ref", None, "event_count"),
    "worker_status_hourly": ("worker_name", "new_status", "transition_count"),
    "enforcement_volume_hourly": ("rule_ref", None, "enforcement_count"),
    "execution_volume_hourly": ("action_ref", "initial_status", "execution_count"),
}
TARGETS = {
    "execution_history": "time", "worker_history": "time",
    "sensor_process_history": "time", "event": "created", "audit_event": "created",
}


def command(args, data=None, timeout=600, check=True):
    result = subprocess.run(args, input=data, text=True, capture_output=True, timeout=timeout)
    if check and result.returncode:
        raise RuntimeError(f"Command failed: {args[0:3]}\n{result.stdout}\n{result.stderr}")
    return result


def save(path, value):
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n")


def stamp(value):
    return value.isoformat()


def bucket(value):
    return value.replace(minute=0, second=0, microsecond=0).isoformat()


def snapshot(repo, ref, destination):
    """Freeze source bytes before any neighboring schema edits or measurements."""
    destination.mkdir()
    migrations = destination / "migrations"
    migrations.mkdir()
    if ref:
        names = command(["git", "-C", str(repo), "ls-tree", "-r", "--name-only", ref,
                         "migrations"]).stdout.splitlines()
        files = {name: command(["git", "-C", str(repo), "show", f"{ref}:{name}"]).stdout
                 for name in names if name.endswith(".sql")}
        runner = command(["git", "-C", str(repo), "show", f"{ref}:docker/run-migrations.sh"]).stdout
    else:
        files = {f"migrations/{path.name}": path.read_text()
                 for path in sorted((repo / "migrations").glob("*.sql"))}
        runner = (repo / "docker/run-migrations.sh").read_text()
    for name, content in files.items():
        (migrations / Path(name).name).write_text(content)
    (destination / "run-migrations.sh").write_text(runner)
    return {name: hashlib.sha384(content.encode()).hexdigest() for name, content in files.items()}


class Database:
    def __init__(self, owner, variant, image, source, output, args):
        self.owner, self.variant, self.image = owner, variant, image
        self.name = f"attune-evidence-{owner}-{variant}"
        self.source, self.output, self.args = source, output, args
        self.claimed = False
        self.password = uuid.uuid4().hex
        self.volumes = []
        self.metadata = {}

    def start(self):
        command(["docker", "image", "inspect", self.image])  # Never pull a moving image silently.
        if command(["docker", "inspect", self.name], check=False).returncode == 0:
            raise RuntimeError(f"Refusing existing container {self.name}")
        self.claimed = True  # Includes partial docker-run failure recovery, after label verification.
        storage = ["--tmpfs", "/evidence-data:rw,size=6g", "-e", "PGDATA=/evidence-data"] if self.args.tmpfs else []
        command(["docker", "run", "-d", "--pull=never", "--name", self.name,
                 "--label", f"attune.evidence.owner={self.owner}", "--cpus", str(self.args.cpus),
                 "--memory", self.args.memory, "--shm-size=256m", "-p", "127.0.0.1::5432",
                 "-e", f"POSTGRES_PASSWORD={self.password}", "-e", "POSTGRES_DB=evidence", *storage,
                 self.image, "postgres", "-c", "max_connections=40", "-c", "shared_buffers=256MB",
                 "-c", "work_mem=16MB", "-c", "jit=off"])
        info = json.loads(command(["docker", "inspect", self.name]).stdout)[0]
        self.volumes = [m["Name"] for m in info["Mounts"] if m["Type"] == "volume"]
        self.metadata = {"name": self.name, "id": info["Id"], "image_id": info["Image"],
                         "ports": info["NetworkSettings"]["Ports"], "volumes": self.volumes}
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            # TCP avoids the temporary init server's Unix socket readiness false positive.
            ready = command(["docker", "exec", "-e", f"PGPASSWORD={self.password}", self.name,
                             "psql", "-h", "127.0.0.1",
                             "-U", "postgres", "-d", "evidence", "-c", "SELECT 1"], check=False)
            if ready.returncode == 0:
                break
            state = json.loads(command(["docker", "inspect", self.name]).stdout)[0]["State"]
            if state["Status"] == "exited":
                raise RuntimeError(f"Database exited before readiness: {self.name}. See postgres.log.")
            time.sleep(0.2)
        else:
            raise RuntimeError(f"Database readiness deadline exceeded: {self.name}")
        command(["docker", "cp", str(self.source), f"{self.name}:/evidence-source"])
        started = time.monotonic()
        migrated = command(["docker", "exec", "-e", "DB_HOST=127.0.0.1", "-e", "DB_USER=postgres",
                            "-e", f"DB_PASSWORD={self.password}",
                            "-e", "DB_NAME=evidence", "-e", "MIGRATIONS_DIR=/evidence-source/migrations",
                            "-e", "STANDARD_INDEX_SEEDER=/bin/true", self.name,
                            "bash", "/evidence-source/run-migrations.sh"])
        (self.output / "migrations.log").write_text(migrated.stdout + migrated.stderr)
        self.metadata["migration_seconds"] = time.monotonic() - started
        self.metadata["server"] = self.sql("SELECT version();").strip()
        self.metadata["extensions"] = self.json("SELECT jsonb_object_agg(extname,extversion) FROM pg_extension;")
        if self.variant == "timescale":
            self.sql("SELECT alter_job(job_id, scheduled => false) FROM timescaledb_information.jobs;")
        self.metadata["settings"] = self.sql("SELECT name, setting, unit FROM pg_settings WHERE name IN "
                                             "('shared_buffers','work_mem','max_connections','jit',"
                                             "'max_parallel_workers_per_gather');")
        save(self.output / "database.json", self.metadata)

    def sql(self, sql, zone="UTC", timeout=600):
        return command(["docker", "exec", "-i", "-e",
                        f"PGOPTIONS=-c search_path=attune,public -c timezone={zone} -c statement_timeout=120000",
                        self.name, "psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1",
                        "-U", "postgres", "-d", "evidence"], data=sql, timeout=timeout).stdout

    def json(self, sql, zone="UTC"):
        return json.loads(self.sql(sql, zone))

    def explain(self, name, sql):
        plan = self.json("EXPLAIN (ANALYZE, BUFFERS, WAL, FORMAT JSON) " + sql)
        save(self.output / f"plan-{name}.json", plan)
        return plan[0]["Execution Time"]

    def cleanup(self):
        if not self.claimed:
            return {"owned": False}
        inspected = command(["docker", "inspect", self.name], check=False)
        if inspected.returncode == 0:
            info = json.loads(inspected.stdout)[0]
            if info["Config"]["Labels"].get("attune.evidence.owner") != self.owner:
                raise RuntimeError(f"Refusing to delete a container without this run's label: {self.name}")
            self.volumes = [m["Name"] for m in info["Mounts"] if m["Type"] == "volume"]
            logs = command(["docker", "logs", self.name], check=False)
            (self.output / "postgres.log").write_text(logs.stdout + logs.stderr)
            command(["docker", "rm", "-f", "-v", self.name])
        leaked = [v for v in self.volumes
                  if command(["docker", "volume", "inspect", v], check=False).returncode == 0]
        container_exists = command(["docker", "inspect", self.name], check=False).returncode == 0
        result = {"container_removed": not container_exists, "remaining_volumes": leaked}
        save(self.output / "cleanup.json", result)
        if leaked or container_exists:
            raise RuntimeError(f"Owned resources leaked: {result}")
        return result


def fixture(args):
    start = ANCHOR - timedelta(days=args.days)
    n = args.executions_per_day * args.days
    # Explicit backdated diagnostic import. Live trigger behavior is checked separately.
    return f"""
CREATE FUNCTION evidence_payload(i bigint, blocks integer) RETURNS jsonb LANGUAGE SQL IMMUTABLE AS $$
 SELECT jsonb_build_object('request_id', i, 'host', 'node-' || i % 100,
   'records', (SELECT string_agg(md5(i::text || ':' || g::text), '') FROM generate_series(1, blocks) g),
   'labels', jsonb_build_object('region','test-region','attempt',i % 3));
$$;
CREATE TEMP TABLE fixture_execution AS
 SELECT g::bigint AS id, 'evidence.action_' || g % 8 AS ref,
   '{stamp(start)}'::timestamptz + ((g::bigint-1)*{args.days * 86400} / {n}) * interval '1 second' AS t,
   CASE WHEN g % 20 = 0 THEN 'timeout' ELSE 'completed' END AS terminal
 FROM generate_series(1,{n}) g;
ALTER TABLE execution DISABLE TRIGGER USER;
INSERT INTO execution(id,action_ref,config,result,status,created,updated,retry_count)
 SELECT id,ref,jsonb_build_object('host','node-' || id % 100,'limit',100,'tags',jsonb_build_array('synthetic','test')),
   evidence_payload(id,64),terminal::execution_status_enum,t,t + interval '4 seconds',
   CASE WHEN id % 10 = 0 THEN 1 ELSE 0 END FROM fixture_execution;
ALTER TABLE execution ENABLE TRIGGER USER;
INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,old_values,new_values)
 SELECT t,'INSERT',id,ref,'{{}}',NULL,jsonb_build_object('status','requested','action_ref',ref)
 FROM fixture_execution;
INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,old_values,new_values)
 SELECT t + s.seconds * interval '1 second','UPDATE',id,ref,ARRAY['status'],
   jsonb_build_object('status',s.old_status),jsonb_build_object('status',s.new_status)
 FROM fixture_execution CROSS JOIN LATERAL (
   SELECT 1 AS seconds,'requested' AS old_status,'running' AS new_status
   UNION ALL SELECT 2,'running','failed' WHERE id % 10 = 0
   UNION ALL SELECT 3,'failed','running' WHERE id % 10 = 0
   UNION ALL SELECT CASE WHEN id % 10 = 0 THEN 4 ELSE 2 END,'running',terminal
 ) s;
INSERT INTO event(id,trigger_ref,created,payload)
 SELECT g,'evidence.trigger_' || g % 8,
   '{stamp(start)}'::timestamptz + ((g::bigint-1)*{args.days * 86400} / {2*n}) * interval '1 second',
   evidence_payload(g,32) FROM generate_series(1,{2*n}) g;
INSERT INTO event(id,trigger_ref,created,payload) VALUES
 (-1,'evidence.boundary','2026-10-06T11:59:59Z','{{"boundary":true}}'),
 (-2,'evidence.boundary','2026-10-06T12:00:00Z','{{"boundary":true}}'),
 (-3,'evidence.boundary','2026-10-06T12:15:00Z','{{"boundary":true}}');
INSERT INTO enforcement(rule_ref,trigger_ref,event,payload,status,created)
 SELECT 'evidence.rule_' || id % 8,'evidence.trigger_' || id % 8,id,
   jsonb_build_object('execution',id),'processed',t FROM fixture_execution;
INSERT INTO audit_event(category,event_type,outcome,resource_ref,created,details)
 SELECT 'execution','execution.completed','success',ref,t,
   jsonb_build_object('execution_id',id,'attempt',id % 3,'duration_ms',id % 1000)
 FROM fixture_execution;
INSERT INTO worker_history(time,operation,entity_id,entity_ref,changed_fields,new_values)
 SELECT '{stamp(start)}'::timestamptz + (g-1)*interval '6 hours', 'UPDATE',w,
   'evidence.worker_' || w,ARRAY['status'],
   jsonb_build_object('status',CASE WHEN g % 2 = 0 THEN 'inactive' ELSE 'active' END)
 FROM generate_series(1,{args.days*4}) g CROSS JOIN generate_series(1,100) w;
INSERT INTO sensor_process_history(time,operation,entity_id,entity_ref,changed_fields,new_values)
 SELECT time,operation,entity_id,'evidence.sensor_' || entity_id,changed_fields,new_values
 FROM worker_history;
"""


def expected(args):
    counters = {name: Counter() for name in AGGREGATES}
    start = ANCHOR - timedelta(days=args.days)
    n, span = args.executions_per_day * args.days, args.days * 86400
    for i in range(1, n + 1):
        t, ref = start + timedelta(seconds=(i-1)*span//n), f"evidence.action_{i % 8}"
        terminal = "timeout" if i % 20 == 0 else "completed"
        counters["execution_throughput_hourly"][(bucket(t), ref)] += 1
        counters["execution_volume_hourly"][(bucket(t), ref, terminal)] += 1
        counters["enforcement_volume_hourly"][(bucket(t), f"evidence.rule_{i % 8}")] += 1
        transitions = [(1, "running"), (4 if i % 10 == 0 else 2, terminal)]
        if i % 10 == 0:
            transitions += [(2, "failed"), (3, "running")]
        for seconds, status in transitions:
            counters["execution_status_hourly"][(bucket(t + timedelta(seconds=seconds)), ref, status)] += 1
    for i in range(1, 2*n + 1):
        counters["event_volume_hourly"][(bucket(start + timedelta(seconds=(i-1)*span//(2*n))),
                                           f"evidence.trigger_{i % 8}")] += 1
    counters["event_volume_hourly"][(bucket(ANCHOR - timedelta(seconds=1)), "evidence.boundary")] += 1
    counters["event_volume_hourly"][(bucket(ANCHOR), "evidence.boundary")] += 2
    for g in range(1, args.days*4 + 1):
        for w in range(1, 101):
            counters["worker_status_hourly"][(bucket(start + timedelta(hours=(g-1)*6)),
                f"evidence.worker_{w}", "inactive" if g % 2 == 0 else "active")] += 1
    return {name: [list(key) + [value] for key, value in sorted(counter.items())]
            for name, counter in counters.items()}


def view_query(name, where="true"):
    ref, status, count = AGGREGATES[name]
    columns = f"to_char(bucket AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS') || '+00:00',{ref},"
    if status:
        columns += f"{status}::text,"
    columns += count
    return (f"SELECT COALESCE(jsonb_agg(row_value ORDER BY row_value::text),'[]'::jsonb) FROM "
            f"(SELECT jsonb_build_array({columns}) AS row_value FROM {name} WHERE {where}) rows;")


def normalize(rows):
    return sorted(rows, key=lambda row: tuple(row[:-1]))


def refresh(db, until):
    if db.variant == "timescale":
        for name in list(AGGREGATES)[:4]:
            db.sql(f"CALL refresh_continuous_aggregate('{name}',NULL,'{stamp(until)}'::timestamptz);")


def correctness(db, oracle):
    observed = {name: normalize(db.json(view_query(name))) for name in AGGREGATES}
    save(db.output / "hourly-counts.json", observed)
    for name in AGGREGATES:
        if observed[name] != oracle[name]:
            raise AssertionError(f"{db.variant}: hourly fixture oracle mismatch for {name}")
    non_utc = {name: normalize(db.json(view_query(name), "Asia/Kathmandu")) for name in AGGREGATES}
    save(db.output / "hourly-counts-kathmandu.json", non_utc)
    zone_match = {name: non_utc[name] == observed[name] for name in AGGREGATES}
    if db.variant != "timescale" and not all(zone_match.values()):
        raise AssertionError(f"Stock PostgreSQL hourly buckets depend on session timezone: {zone_match}")
    partial = db.json(view_query("event_volume_hourly",
        "bucket >= '2026-10-06T11:30:00Z' AND bucket <= '2026-10-06T12:30:00Z' "
        "AND trigger_ref='evidence.boundary'"))
    assert partial == [[bucket(ANCHOR), "evidence.boundary", 2]], partial
    filters = {}
    for name, (ref, _, _) in AGGREGATES.items():
        wanted = oracle[name][0][1]
        rows = normalize(db.json(view_query(name, f"{ref}='{wanted}'")))
        assert rows == [row for row in oracle[name] if row[1] == wanted], name
        assert db.json(view_query(name, f"{ref}='evidence.missing'")) == [], name
        filters[name] = wanted
    terminal_counts = Counter()
    for row in oracle["execution_status_hourly"]:
        if row[2] in ("completed", "failed", "timeout"):
            terminal_counts[row[2]] += row[3]
    failure = db.json("SELECT COALESCE(jsonb_object_agg(new_status,n),'{}') FROM "
        "(SELECT new_status,SUM(transition_count)::bigint n FROM execution_status_hourly "
        "WHERE new_status IN ('completed','failed','timeout') GROUP BY new_status) terminal;")
    assert failure == dict(terminal_counts), failure
    failure["total_terminal"] = sum(terminal_counts.values())
    failure["failure_rate_pct"] = 100*(terminal_counts["failed"] + terminal_counts["timeout"])/sum(terminal_counts.values())
    columns = db.json("SELECT jsonb_object_agg(name,columns) FROM (" + " UNION ALL ".join(
        f"SELECT '{name}'::text AS name,jsonb_agg(jsonb_build_object('name',attname,"
        f"'type',format_type(atttypid,atttypmod)) ORDER BY attnum) AS columns FROM pg_attribute "
        f"WHERE attrelid='{name}'::regclass AND attnum>0 AND NOT attisdropped"
        for name in AGGREGATES) + ") types;")
    for name, (_, _, count) in AGGREGATES.items():
        assert next(col['type'] for col in columns[name] if col['name'] == count) == 'bigint', name
    save(db.output / "analytics-columns.json", columns)
    # A real trigger probe uses a rollback so clock-generated history cannot pollute the fixture.
    probe = db.sql("""
BEGIN;
INSERT INTO execution(id,action_ref,config,status) VALUES
 (-999,'evidence.trigger_probe','{"host":"probe"}','requested');
UPDATE execution SET status='running',result=evidence_payload(999,64) WHERE id=-999;
UPDATE execution SET status=status,result=result WHERE id=-999;
SELECT jsonb_build_object('history_rows',(SELECT count(*) FROM execution_history WHERE entity_id=-999),
 'digest_matches',(SELECT new_values->'result' = _jsonb_digest_summary(evidence_payload(999,64))
                  FROM execution_history WHERE entity_id=-999 AND operation='UPDATE'),
 'history_result_bytes',(SELECT octet_length((new_values->'result')::text)
                  FROM execution_history WHERE entity_id=-999 AND operation='UPDATE'));
ROLLBACK;
""")
    trigger = json.loads(probe)
    assert trigger["history_rows"] == 2 and trigger["digest_matches"], trigger
    tail = ANCHOR + timedelta(hours=1, minutes=15)
    db.sql(f"INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,new_values) "
           f"VALUES ('{stamp(tail)}','UPDATE',-998,'evidence.tail',ARRAY['status'],'{{\"status\":\"completed\"}}');")
    where = "action_ref='evidence.tail'"
    before = db.json(view_query("execution_status_hourly", where))
    raw = int(db.sql("SELECT count(*) FROM execution_history WHERE entity_ref='evidence.tail' "
                     "AND new_values->>'status'='completed';"))
    if db.variant == "timescale":
        assert before == [], before
    else:
        assert before == [[bucket(tail), "evidence.tail", "completed", 1]], before
    refresh(db, ANCHOR + timedelta(hours=2))
    after = db.json(view_query("execution_status_hourly", where))
    assert raw == 1 and after == [[bucket(tail), "evidence.tail", "completed", 1]], after
    result = {"oracle_match": True, "partial_hour_boundary_count": 2, "timezone_match": zone_match,
              "trigger_probe": trigger, "tail_raw_count": raw, "tail_before_refresh": before,
              "tail_after_refresh": after, "filtered_refs": filters, "failure_rate": failure}
    save(db.output / "correctness.json", result)
    return result


def measurements(db):
    start = stamp(ANCHOR - timedelta(hours=24))
    until = stamp(ANCHOR)
    queries = {
        "status-view-24h": f"SELECT bucket,new_status,SUM(transition_count)::bigint "
            f"FROM execution_status_hourly WHERE bucket >= '{start}' AND bucket <= '{until}' "
            "GROUP BY bucket,new_status ORDER BY bucket,new_status;",
        "event-view-24h": f"SELECT bucket,SUM(event_count)::bigint FROM event_volume_hourly "
            f"WHERE bucket >= '{start}' AND bucket <= '{until}' GROUP BY bucket ORDER BY bucket;",
        "status-raw-bounded-24h": "SELECT date_trunc('hour',time AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' "
            f"AS bucket,new_values->>'status',count(*) FROM execution_history WHERE time >= '{start}' "
            f"AND time < '{stamp(ANCHOR + timedelta(hours=1))}' AND 'status'=ANY(changed_fields) "
            "GROUP BY 1,2 ORDER BY 1,2;",
        "event-raw-bounded-24h": "SELECT date_trunc('hour',created AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' "
            f"AS bucket,count(*) FROM event WHERE created >= '{start}' "
            f"AND created < '{stamp(ANCHOR + timedelta(hours=1))}' GROUP BY 1 ORDER BY 1;",
        "live-completed-24h": f"SELECT count(*) FROM execution WHERE created >= '{start}' "
            f"AND created <= '{until}' AND status='completed';",
        "recent-history": "SELECT time,operation,changed_fields,new_values FROM execution_history "
            "WHERE entity_id=100 ORDER BY time DESC LIMIT 100;",
        "recent-events": "SELECT id,created,payload FROM event WHERE trigger_ref='evidence.trigger_0' "
            "ORDER BY created DESC LIMIT 100;",
    }
    results = {}
    for name, sql in queries.items():
        (db.output / f"query-{name}.sql").write_text(sql + "\n")
        first = db.explain(name + "-first-observed", sql)
        with ThreadPoolExecutor(max_workers=db.args.concurrency) as pool:
            samples = list(pool.map(lambda _: db.json("EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON) " + sql)[0]
                                    ["Execution Time"], range(db.args.concurrency * db.args.samples)))
        ordered = sorted(samples)
        p95 = ordered[max(0, (95*len(ordered) + 99)//100 - 1)]
        limit = db.args.latency_ms
        results[name] = {"first_observed_server_ms": first, "warm_server_ms": samples,
                         "warm_p95_ms": p95, "limit_ms": limit, "within_limit": p95 <= limit}
    save(db.output / "latency.json", results)
    sizes = db.json("SELECT jsonb_object_agg(name,jsonb_build_object('bytes',pg_total_relation_size(name),"
        "'rows',rows)) FROM (" + " UNION ALL ".join(
            f"SELECT '{table}'::text AS name,count(*) AS rows FROM {table}"
            for table in ["execution", "execution_history", "worker_history", "sensor_process_history",
                          "event", "audit_event", "enforcement"]) + ") totals;")
    if db.variant == "timescale":
        for table in TARGETS:
            sizes[table]["bytes"] = int(db.sql(f"SELECT hypertable_size('{table}'::regclass);"))
    payloads = db.json("SELECT jsonb_build_object('result_avg_bytes',(SELECT avg(octet_length(result::text)) "
        "FROM execution),'event_payload_avg_bytes',(SELECT avg(octet_length(payload::text)) FROM event));")
    save(db.output / "sizes.json", {"relations": sizes, "jsonb": payloads,
        "note": "Uncompressed storage, including Timescale chunks; no compression jobs run"})
    return results


def retention(db):
    args = db.args
    cutoff = stamp(ANCHOR - timedelta(days=args.days))
    # All targets have an explicit backlog, a cutoff equality sentinel, and duplicate timestamps.
    seed = []
    for table, column in TARGETS.items():
        t = "'2026-09-01T00:00:00Z'::timestamptz + (g % 86400)*interval '1 second'"
        if column == "time":
            seed.append(f"INSERT INTO {table}(time,operation,entity_id,entity_ref,changed_fields,new_values) "
                f"SELECT {t},'UPDATE',-g,'evidence.backlog',ARRAY['status'],"
                f"jsonb_build_object('status','running','sequence',g) FROM generate_series(1,{args.backlog}) g;")
            seed.append(f"INSERT INTO {table}(time,operation,entity_id,entity_ref) "
                        f"VALUES ('{cutoff}','INSERT',-900000000,'evidence.cutoff');")
        elif table == "event":
            seed.append(f"INSERT INTO event(trigger_ref,created,payload) SELECT 'evidence.backlog',{t},"
                        f"evidence_payload(g,32) FROM generate_series(1,{args.backlog}) g;")
            seed.append(f"INSERT INTO event(trigger_ref,created) VALUES ('evidence.cutoff','{cutoff}');")
        else:
            seed.append(f"INSERT INTO audit_event(category,event_type,outcome,created,details) "
                f"SELECT 'execution','evidence.backlog','success',{t},jsonb_build_object('sequence',g) "
                f"FROM generate_series(1,{args.backlog}) g;")
            seed.append(f"INSERT INTO audit_event(category,event_type,outcome,created) "
                        f"VALUES ('execution','evidence.cutoff','success','{cutoff}');")
    # Bulk explicit positive IDs above must not collide with this subsequent sequence-backed import.
    db.sql("SELECT setval(pg_get_serial_sequence('event','id'),(SELECT max(id) FROM event));")
    sql = "\n".join(seed) + "\nANALYZE;"
    (db.output / "backlog.sql").write_text(sql)
    db.sql(sql)
    results = {}
    for table, column in TARGETS.items():
        candidates = int(db.sql(f"SELECT count(*) FROM {table} WHERE {column} < '{cutoff}';"))
        keep = int(db.sql(f"SELECT count(*) FROM {table} WHERE {column} >= '{cutoff}';"))
        assert candidates == args.backlog, (table, candidates)
        started = time.monotonic()
        if db.variant == "timescale":
            chunks = db.sql(f"SELECT drop_chunks('{table}',older_than => '{cutoff}'::timestamptz);")
            result = {"dropped_chunks": chunks.splitlines(), "method": "drop_chunks"}
        else:
            identity = "id" if column == "created" else "ctid"
            batch = (f"WITH doomed AS MATERIALIZED (SELECT {identity} FROM {table} "
                     f"WHERE {column} < '{cutoff}' ORDER BY {column} ASC,{identity} ASC "
                     f"LIMIT {args.batch_size} FOR UPDATE SKIP LOCKED), "
                     f"deleted AS (DELETE FROM {table} WHERE {identity} IN "
                     f"(SELECT {identity} FROM doomed) RETURNING 1) SELECT count(*)::bigint FROM deleted;")
            (db.output / f"retention-{table}.sql").write_text(batch + "\n")
            # One psql session, autocommit per statement, no DO-block transaction spanning batches.
            script = "\\timing on\n\\set continue true\n"
            for _ in range(args.max_batches):
                script += ("\\if :continue\n" + batch + "\n"
                           f"SELECT EXISTS(SELECT 1 FROM {table} WHERE {column} < '{cutoff}') "
                           "AS continue \\gset\n\\endif\n")
            output = db.sql(script)
            (db.output / f"retention-{table}.log").write_text(output)
            deleted = [int(line) for line in output.splitlines() if re.fullmatch(r"[0-9]+", line)]
            durations = [float(v) for v in re.findall(r"Time: ([0-9.]+) ms", output)]
            assert sum(deleted) == min(candidates, args.batch_size*args.max_batches), (table, deleted)
            assert all(0 < count <= args.batch_size for count in deleted), (table, deleted)
            result = {"method": "bounded row deletes", "batch_rows": deleted,
                      "sql_statement_ms": durations, "server_total_ms": sum(durations)}
        result["wall_seconds"] = time.monotonic() - started
        remaining = int(db.sql(f"SELECT count(*) FROM {table} WHERE {column} < '{cutoff}';"))
        retained = int(db.sql(f"SELECT count(*) FROM {table} WHERE {column} >= '{cutoff}';"))
        assert keep == retained, (table, keep, retained)
        result.update(candidates=candidates, deleted=candidates-remaining, remaining=remaining,
                      retained_before=keep, retained_after=retained,
                      hourly_capacity_rows=args.batch_size*args.max_batches,
                      daily_capacity_rows=args.batch_size*args.max_batches*24)
        if db.variant != "timescale":
            assert result["wall_seconds"] < 3600, result
        results[table] = result
    save(db.output / "retention.json", results)
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True, help="New evidence directory; never overwrite")
    parser.add_argument("--baseline-repo", type=Path, default=ROOT)
    parser.add_argument("--baseline-ref", default="252ce12e")
    parser.add_argument("--candidate-repo", type=Path, default=ROOT)
    parser.add_argument("--variants", nargs="+", choices=["timescale", "pg18", "pg16"],
                        default=["timescale", "pg18", "pg16"])
    parser.add_argument("--days", type=int, default=7)
    parser.add_argument("--executions-per-day", type=int, default=20000)
    parser.add_argument("--backlog", type=int, default=250001)
    parser.add_argument("--batch-size", type=int, default=1000)
    parser.add_argument("--max-batches", type=int, default=100)
    parser.add_argument("--concurrency", type=int, default=4)
    parser.add_argument("--samples", type=int, default=5)
    parser.add_argument("--latency-ms", type=float, default=500)
    parser.add_argument("--cpus", type=int, default=4)
    parser.add_argument("--memory", default="4g")
    parser.add_argument("--tmpfs", action="store_true",
                        help="Explicit in-memory PGDATA, 6 GiB cap; labels results as synthetic tmpfs")
    args = parser.parse_args()
    for key in ["days", "executions_per_day", "backlog", "batch_size", "max_batches",
                "concurrency", "samples", "latency_ms", "cpus"]:
        if getattr(args, key) <= 0:
            parser.error(f"--{key.replace('_','-')} must be positive")
    if args.days > 28:
        parser.error("--days must be at most 28 so the fixed September backlog precedes retention")
    if len(args.variants) != len(set(args.variants)):
        parser.error("--variants must not contain duplicates")
    if args.backlog <= args.batch_size * args.max_batches:
        parser.error("--backlog must exceed one target's batch budget to exercise exhaustion")
    args.output.parent.resolve(strict=True)
    args.output.mkdir()  # Refuse an existing destination before any resource creation.
    source_bytes = Path(__file__).read_bytes()
    (args.output / "measurement-runner.py").write_bytes(source_bytes)
    owner = uuid.uuid4().hex[:16]
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    signal.signal(signal.SIGHUP, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    metadata = {"owner": owner, "anchor_utc": stamp(ANCHOR), "arguments": vars(args).copy(),
                "started_utc": stamp(datetime.now(timezone.utc)), "results": {}}
    metadata["runner_sha256"] = hashlib.sha256(source_bytes).hexdigest()
    metadata["arguments"] = {key: str(value) if isinstance(value, Path) else value
                             for key, value in metadata["arguments"].items()}
    baseline = args.output / "source-timescale"
    candidate = args.output / "source-postgres"
    metadata["baseline_commit"] = command(["git", "-C", str(args.baseline_repo), "rev-parse",
                                           args.baseline_ref]).stdout.strip()
    metadata["baseline_checksums"] = snapshot(args.baseline_repo, args.baseline_ref, baseline)
    metadata["candidate_checksums"] = snapshot(args.candidate_repo, None, candidate)
    metadata["host"] = command(["docker", "info", "--format",
                                '{{json .}}']).stdout
    save(args.output / "run.json", metadata)
    sql, oracle = fixture(args), expected(args)
    metadata["fixture_sha256"] = hashlib.sha256(sql.encode()).hexdigest()
    (args.output / "fixture.sql").write_text(sql)
    save(args.output / "expected-hourly-counts.json", oracle)
    save(args.output / "run.json", metadata)
    failed = False
    images = {"timescale": "timescale/timescaledb:2.30.1-pg18",
              "pg18": "postgres:18-alpine", "pg16": "postgres:16-alpine"}
    try:
        for variant in args.variants:
            output = args.output / variant
            output.mkdir()
            db = Database(owner, variant, images[variant], baseline if variant == "timescale" else candidate,
                          output, args)
            print(f"{variant}: {db.name}: migrate, seed, compare, measure, retain", flush=True)
            try:
                db.start()
                started = time.monotonic()
                db.sql(sql + "\nANALYZE;", timeout=1200)
                refresh(db, ANCHOR + timedelta(hours=1))
                seeded = time.monotonic() - started
                counts = correctness(db, oracle)
                latency = measurements(db)
                cleanup_capacity = retention(db)
                metadata["results"][variant] = {"correctness": counts, "latency": latency,
                    "retention": cleanup_capacity, "seed_and_refresh_seconds": seeded}
                # A performance rejection remains evidence and does not hide later database variants.
                if not all(item["within_limit"] for item in latency.values()):
                    failed = True
                save(args.output / "run.json", metadata)
            finally:
                db.cleanup()
    except BaseException as exc:
        metadata["error"] = str(exc) or type(exc).__name__
        raise
    finally:
        remaining = command(["docker", "ps", "-aq", "--filter",
                             f"label=attune.evidence.owner={owner}"], check=False)
        metadata["remaining_owned_containers"] = remaining.stdout.splitlines()
        metadata["finished_utc"] = stamp(datetime.now(timezone.utc))
        save(args.output / "run.json", metadata)
        if remaining.returncode or remaining.stdout.strip():
            raise RuntimeError("Could not prove owned-container teardown")
    print(f"Evidence: {args.output}. Owned containers remaining: 0.")
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (RuntimeError, AssertionError, subprocess.TimeoutExpired, KeyboardInterrupt, OSError) as error:
        print(f"Evidence run failed: {str(error) or type(error).__name__}", file=sys.stderr)
        sys.exit(1)
