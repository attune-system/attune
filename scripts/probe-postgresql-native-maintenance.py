#!/usr/bin/env python3
"""Run isolated PostgreSQL 16/18 maintenance protocol experiments.

Requires Python 3.10+ and Docker, with the requested PostgreSQL images cached.
No application tables, migrations, credentials, or existing containers are used.
JSON includes rejected protocols and failures; --output preserves failed runs too.
"""

import argparse
import concurrent.futures
import json
from pathlib import Path
import queue
import re
import signal
import subprocess
import threading
import time
import uuid


LABEL = "local.attune.native-maintenance-probe"


def command(*args, input=None, check=True, timeout=30):
    result = subprocess.run(args, input=input, text=True, capture_output=True,
                            timeout=timeout, check=False)
    if check and result.returncode:
        raise RuntimeError(f"{args!r}: {result.stdout}{result.stderr}")
    return result


def require(condition, message):
    if not condition:
        raise AssertionError(message)


class Session:
    """A persistent psql connection; command completion uses explicit tokens."""

    def __init__(self, server, name):
        self.server = server
        self.name = name
        self.lines = queue.Queue()
        self.process = subprocess.Popen(
            server.psql(name), stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.reader = threading.Thread(target=self.read_output, daemon=True)
        self.reader.start()
        server.sessions.append(self)
        self.token = None

    def read_output(self):
        for line in self.process.stdout:
            self.lines.put(line.rstrip("\n"))
        self.lines.put(None)

    def send(self, sql):
        require(self.token is None, f"unfinished command in {self.name}")
        self.token = "done_" + uuid.uuid4().hex
        self.process.stdin.write(sql + "\n\\echo " + self.token + "\n")
        self.process.stdin.flush()

    def finish(self, timeout=10):
        deadline = time.monotonic() + timeout
        output = []
        while True:
            line = self.lines.get(timeout=max(0.001, deadline - time.monotonic()))
            if line == self.token:
                self.token = None
                return "\n".join(output)
            if line is None:
                self.token = None
                raise RuntimeError(f"session {self.name} failed: {' '.join(output)}")
            output.append(line)

    def sql(self, sql):
        self.send(sql)
        return self.finish()

    def close(self):
        if self.process.poll() is None:
            try:
                # EOF rolls back an unfinished transaction and releases its locks.
                self.process.stdin.close()
                self.process.wait(timeout=3)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                self.process.kill()
                self.process.wait(timeout=3)
        self.reader.join(timeout=3)
        self.process.stdout.close()


class Server:
    def __init__(self, image, run_id, report):
        self.image = image
        self.run_id = run_id
        self.name = f"attune-native-{run_id}-{image.split(':')[1]}"
        self.volume = self.name + "-data"
        self.cid = None
        self.container_claimed = False
        self.volume_owned = False
        self.sessions = []
        self.report = report

    def psql(self, name="probe"):
        return ["docker", "exec", "-i", "-e", f"PGAPPNAME={name}", self.name,
                "psql", "-X", "-qAt", "-h", "127.0.0.1", "-U", "postgres", "-d", "postgres",
                "-v", "ON_ERROR_STOP=1", "-v", "VERBOSITY=verbose"]

    def sql(self, sql):
        return command(*self.psql(), input=sql).stdout.strip()

    def reject(self, name, sql, state):
        start = time.monotonic()
        result = command(*self.psql(name), input=sql, check=False)
        require(result.returncode != 0 and state in result.stderr,
                f"expected SQLSTATE {state}: {result.stdout}{result.stderr}")
        sample = {"sqlstate": state, "wall_ms": round((time.monotonic()-start)*1000, 3),
                  "stdout": result.stdout.strip(), "stderr": result.stderr.strip()}
        measured = re.search(r"wait_ms=([0-9.]+)", result.stderr)
        if measured:
            sample["server_lock_wait_ms"] = float(measured.group(1))
        self.report["rejections"][name] = sample
        return sample

    def wait(self, sql, description, timeout=10):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.sql(sql) == "t":
                return
            # Poll interval only; progress depends on a catalog predicate.
            threading.Event().wait(0.01)
        raise TimeoutError(f"not ready: {description}; " + self.sql(
            "SELECT application_name||':'||state||':'||coalesce(wait_event,'') "
            "FROM pg_stat_activity WHERE application_name <> 'probe';"))

    def blocked(self, name, blocker):
        self.wait(
            "SELECT EXISTS(SELECT 1 FROM pg_stat_activity a "
            "JOIN pg_stat_activity b ON b.pid=ANY(pg_blocking_pids(a.pid)) "
            f"WHERE a.application_name='{name}' AND b.application_name='{blocker}' "
            "AND a.wait_event_type='Lock');", f"{name} blocked by {blocker}")

    def locks(self, name):
        return self.sql(
            "SELECT c.relname||':'||l.mode||':'||l.granted "
            "FROM pg_locks l JOIN pg_stat_activity a USING(pid) "
            "JOIN pg_class c ON c.oid=l.relation "
            f"WHERE a.application_name='{name}' AND c.relnamespace="
            "'public'::regnamespace ORDER BY c.relname,l.mode;").splitlines()

    def __enter__(self):
        try:
            image = json.loads(command("docker", "image", "inspect", self.image).stdout)[0]
            self.report["image_id"] = image["Id"]
            self.report["image_digests"] = image.get("RepoDigests", [])
            require(command("docker", "inspect", self.name, check=False).returncode != 0,
                    "refusing existing container")
            require(command("docker", "volume", "inspect", self.volume, check=False).returncode != 0,
                    "refusing existing volume")
            self.volume_owned = True
            command("docker", "volume", "create", "--label", f"{LABEL}={self.run_id}",
                    self.volume)
            mount = "/var/lib/postgresql/data" if ":16" in self.image else "/var/lib/postgresql"
            self.container_claimed = True
            self.cid = command(
                "docker", "run", "-d", "--pull=never", "--name", self.name,
                "--label", f"{LABEL}={self.run_id}", "--publish", "127.0.0.1::5432",
                "--mount", f"type=volume,src={self.volume},dst={mount}",
                "-e", "POSTGRES_HOST_AUTH_METHOD=trust", self.image).stdout.strip()
            deadline = time.monotonic() + 30
            while True:
                ready = command(*self.psql(), input="SELECT true;", check=False)
                if ready.returncode == 0 and ready.stdout.strip() == "t":
                    break
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"database startup failed: {ready.stderr}")
                threading.Event().wait(0.05)
            self.report["server_version"] = self.sql("SELECT version();")
            self.report["port"] = command("docker", "port", self.name, "5432").stdout.strip()
            self.report["settings"] = self.sql(
                "SELECT name||'='||setting FROM pg_settings WHERE name IN "
                "('fsync','synchronous_commit','shared_buffers','max_connections',"
                "'default_transaction_isolation','TimeZone') ORDER BY name;").splitlines()
            return self
        except BaseException:
            self.cleanup()
            raise

    def cleanup(self):
        errors = []
        for session in reversed(self.sessions):
            try:
                session.close()
            except Exception as error:
                errors.append(str(error))
        if self.cid and self.report.get("passed"):
            try:
                self.wait("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity "
                          "WHERE backend_type='client backend' AND application_name <> 'probe');",
                          "all protocol sessions closed")
                self.report["sessions_before_container_removal"] = 0
            except Exception as error:
                errors.append(str(error))
        if self.container_claimed:
            try:
                inspected = command("docker", "inspect", self.name, check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    require(info["Config"]["Labels"].get(LABEL) == self.run_id,
                            "container ownership changed")
                    command("docker", "rm", "-f", "-v", info["Id"])
            except Exception as error:
                errors.append(str(error))
        if self.volume_owned:
            try:
                inspected = command("docker", "volume", "inspect", self.volume, check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    require(info["Labels"].get(LABEL) == self.run_id, "volume ownership changed")
                    command("docker", "volume", "rm", self.volume)
            except Exception as error:
                errors.append(str(error))
        self.report["cleanup_errors"] = errors
        if errors:
            raise RuntimeError("; ".join(errors))

    def __exit__(self, *args):
        self.cleanup()


def setup(s):
    s.sql("""
    CREATE TABLE raw(id bigint NOT NULL, created timestamptz NOT NULL,
                     ref text, PRIMARY KEY(id,created)) PARTITION BY RANGE(created);
    CREATE INDEX raw_created ON raw(created);
    CREATE TABLE raw_today PARTITION OF raw FOR VALUES FROM ('2026-01-01') TO ('2026-01-02');
    CREATE TABLE raw_default PARTITION OF raw DEFAULT;
    INSERT INTO raw SELECT n, '2026-01-02'::timestamptz, NULL FROM generate_series(1,3) n;
    INSERT INTO raw VALUES(4,'2026-01-04','other'),(5,'2026-01-04','');
    CREATE TABLE repair_stage(LIKE raw INCLUDING ALL);
    ALTER TABLE repair_stage ADD CHECK(created >= '2026-01-02' AND created < '2026-01-03');
    CREATE TABLE repair_dirty(bucket timestamptz PRIMARY KEY, revision bigint NOT NULL DEFAULT 1);
    CREATE TABLE repair_totals(bucket timestamptz PRIMARY KEY,n bigint NOT NULL);
    INSERT INTO repair_totals VALUES('2026-01-02',3);
    CREATE FUNCTION mark_repair_dirty() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO repair_dirty(bucket) VALUES(date_trunc('hour',NEW.created,'UTC'))
      ON CONFLICT(bucket) DO UPDATE SET revision=repair_dirty.revision+1;
      RETURN NEW; END $$;
    CREATE TRIGGER raw_repair_dirty AFTER INSERT ON raw FOR EACH ROW EXECUTE FUNCTION mark_repair_dirty();
    CREATE TABLE dirty(kind text NOT NULL, bucket timestamptz NOT NULL,
                       revision bigint NOT NULL DEFAULT 1, PRIMARY KEY(kind,bucket));
    CREATE TABLE totals(bucket timestamptz NOT NULL, ref text, status text,
                        n bigint NOT NULL, UNIQUE NULLS NOT DISTINCT(bucket,ref,status));
    CREATE TABLE coverage(bucket timestamptz PRIMARY KEY);
    CREATE TABLE source(id bigserial PRIMARY KEY, created timestamptz NOT NULL, ref text);
    CREATE FUNCTION mark_dirty() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket) VALUES('event',date_trunc('hour',NEW.created,'UTC'))
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
      RETURN NEW;
    END $$;
    CREATE TRIGGER source_dirty AFTER INSERT ON source FOR EACH ROW EXECUTE FUNCTION mark_dirty();
    INSERT INTO source(created,ref) VALUES('2026-01-02',NULL),('2026-01-02',NULL),('2026-01-02','');
    """)


def default_repair(s):
    r = s.report["observations"]
    s.reject("attach_with_default_rows", "ALTER TABLE raw ATTACH PARTITION repair_stage "
             "FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');", "23514")
    s.sql("WITH moved AS (DELETE FROM raw_default WHERE created='2026-01-02' "
          "RETURNING id,created,ref) INSERT INTO repair_stage SELECT id,created,ref FROM moved;")
    r["rejected_committed_stage_gap"] = s.sql(
        "SELECT (SELECT count(*) FROM raw)||'|'||(SELECT count(*) FROM repair_stage);")
    require(r["rejected_committed_stage_gap"] == "2|3", "gap was not reproduced")
    s.sql("WITH moved AS (DELETE FROM repair_stage RETURNING id,created,ref) "
          "INSERT INTO raw SELECT id,created,ref FROM moved; DELETE FROM repair_dirty;")
    # A failed transaction must leave both parent routing and raw counts intact.
    s.reject("repair_rollback", "BEGIN; LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE; "
             "WITH moved AS (DELETE FROM raw_default WHERE created='2026-01-02' RETURNING id,created,ref) "
             "INSERT INTO repair_stage SELECT id,created,ref FROM moved; "
             "INSERT INTO repair_dirty(bucket) VALUES('2026-01-02') "
             "ON CONFLICT(bucket) DO UPDATE SET revision=repair_dirty.revision+1; "
             "DO $$ BEGIN RAISE EXCEPTION 'injected after move'; END $$; COMMIT;", "P0001")
    require(s.sql("SELECT count(*) FROM raw;") == "5", "repair rollback lost rows")
    require(s.sql("SELECT count(*) FROM repair_dirty;") == "0", "repair rollback leaked invalidation")
    repair = Session(s, "repair")
    reader = Session(s, "repair_reader")
    writer = Session(s, "repair_writer")
    started = repair.sql("BEGIN; SET LOCAL lock_timeout='250ms'; "
                         "SET LOCAL statement_timeout='1s'; "
                         "LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE; "
                         "SELECT extract(epoch FROM clock_timestamp());")
    # Simulated held-lock target includes readiness coordination overhead.
    reader.send("SELECT count(*) FROM raw;")
    writer.send("INSERT INTO raw VALUES(6,'2026-01-02','late');")
    s.blocked("repair_reader", "repair")
    s.blocked("repair_writer", "repair")
    result = repair.sql("""
    LOCK TABLE raw_default IN ACCESS EXCLUSIVE MODE;
    LOCK TABLE repair_stage IN ACCESS EXCLUSIVE MODE;
    DO $$ DECLARE size integer; BEGIN
      IF EXISTS(SELECT 1 FROM repair_stage) THEN RAISE EXCEPTION 'destination must be empty'; END IF;
      SELECT count(*) INTO size FROM
        (SELECT id FROM raw_default WHERE created >= '2026-01-02' AND created < '2026-01-03' LIMIT 4) q;
      IF size > 3 THEN RAISE EXCEPTION 'over row budget'; END IF;
    END $$;
    WITH moved AS (DELETE FROM raw_default WHERE created >= '2026-01-02' AND created < '2026-01-03'
      RETURNING id,created,ref) INSERT INTO repair_stage SELECT id,created,ref FROM moved;
    INSERT INTO repair_dirty(bucket) SELECT date_trunc('hour',created,'UTC') FROM repair_stage
      GROUP BY date_trunc('hour',created,'UTC') ORDER BY date_trunc('hour',created,'UTC')
      ON CONFLICT(bucket) DO UPDATE SET revision=repair_dirty.revision+1;
    ALTER TABLE raw_default ADD CONSTRAINT default_excludes_repair
      CHECK(created < '2026-01-02' OR created >= '2026-01-03') NOT VALID;
    ALTER TABLE raw_default VALIDATE CONSTRAINT default_excludes_repair;
    SET LOCAL client_min_messages=debug1;
    ALTER TABLE raw ATTACH PARTITION repair_stage FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');
    SET LOCAL client_min_messages=notice;
    ALTER TABLE raw_default DROP CONSTRAINT default_excludes_repair;
    SELECT extract(epoch FROM clock_timestamp());
    """)
    r["attach_constraint_evidence"] = result.splitlines()[:-1]
    r["repair_locks"] = s.locks("repair")
    ended = repair.sql("COMMIT; SELECT extract(epoch FROM clock_timestamp());")
    visible = reader.finish()
    writer.finish()
    require(visible in {"5", "6"}, f"reader saw gap: {visible}")
    require(s.sql("SELECT count(*) FROM raw;") == "6", "repair count mismatch")
    require(s.sql("SELECT tableoid::regclass FROM raw WHERE id=6;") == "repair_stage",
            "concurrent writer did not reroute")
    r["repair_summary_invalidation"] = s.sql(
        "SELECT (SELECT n FROM repair_totals WHERE bucket='2026-01-02')||'|'||"
        "(SELECT count(*) FROM raw WHERE created='2026-01-02')||'|'||"
        "(SELECT count(*) FROM repair_dirty WHERE bucket='2026-01-02');")
    require(r["repair_summary_invalidation"] == "3|4|1", "physical move lost count/invalidation semantics")
    r["repair"] = {"row_budget": 3, "moved_rows": 3, "reader_count": int(visible),
                   "post_writer_count": 6,
                   "before_commit_ms": round((float(result.splitlines()[-1])-float(started))*1000, 3),
                   "coordinated_parent_hold_upper_ms": round((float(ended)-float(started))*1000, 3)}
    require(r["repair"]["coordinated_parent_hold_upper_ms"] < 1000,
            "three-row coordinated repair exceeded the one-second hold target")
    s.sql("INSERT INTO raw SELECT n,'2026-01-05',NULL FROM generate_series(10,14) n;")
    r["oversized_day"] = s.sql("""
    BEGIN; SET LOCAL lock_timeout='250ms'; SET LOCAL statement_timeout='1s';
    LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE;
    LOCK TABLE raw_default IN ACCESS EXCLUSIVE MODE;
    SELECT count(*) > 3 FROM (SELECT id FROM raw_default
      WHERE created >= '2026-01-05' AND created < '2026-01-06' LIMIT 4) q;
    COMMIT;
    SELECT count(*) FROM raw WHERE created='2026-01-05';
    """)
    require(r["oversized_day"] == "t\n5", "oversized day must remain queryable")
    s.reject("repair_statement_timeout_rollback", """
    BEGIN; SET LOCAL lock_timeout='250ms'; SET LOCAL statement_timeout='100ms';
    LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE;
    CREATE TABLE timeout_stage(LIKE raw INCLUDING ALL);
    WITH moved AS (DELETE FROM raw_default WHERE created='2026-01-05' RETURNING id,created,ref)
      INSERT INTO timeout_stage SELECT id,created,ref FROM moved;
    DO $$ BEGIN LOOP PERFORM 1; END LOOP; END $$;
    COMMIT;
    """, "57014")
    require(s.sql("SELECT count(*) FROM raw WHERE created='2026-01-05';") == "5",
            "timeout rollback lost DEFAULT rows")
    require(s.sql("SELECT to_regclass('timeout_stage') IS NULL;") == "t", "timeout leaked staging table")
    s.reject("concurrent_detach_with_default", "ALTER TABLE raw DETACH PARTITION "
             "repair_stage CONCURRENTLY;", "55000")


def lock_order(s):
    r = s.report["observations"]
    reader = Session(s, "long_read")
    reader.sql("BEGIN; SELECT count(*) FROM raw;")
    timed_lock = """
    BEGIN; SET LOCAL lock_timeout='250ms';
    DO $$ DECLARE started timestamptz := clock_timestamp(); BEGIN
      LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE;
    EXCEPTION WHEN lock_not_available THEN
      RAISE NOTICE 'wait_ms=%',extract(epoch FROM clock_timestamp()-started)*1000;
      RAISE;
    END $$;
    """
    s.reject("parent_lock_busy", timed_lock, "55P03")
    reader.sql("ROLLBACK;")
    writer = Session(s, "long_writer")
    writer.sql("BEGIN; INSERT INTO raw VALUES(20,'2026-01-04','held');")
    s.reject("parent_lock_writer_busy", timed_lock, "55P03")
    writer.sql("ROLLBACK;")
    bad = Session(s, "child_first")
    bad.sql("BEGIN; LOCK TABLE raw_default IN ACCESS EXCLUSIVE MODE;")
    writer.send("INSERT INTO raw VALUES(21,'2026-01-04','inversion');")
    s.blocked("long_writer", "child_first")
    r["child_first_writer_locks"] = s.locks("long_writer")
    bad.send("SET LOCAL lock_timeout='250ms'; LOCK TABLE ONLY raw IN ACCESS EXCLUSIVE MODE;")
    try:
        bad.finish()
        raise AssertionError("child-first inversion unexpectedly succeeded")
    except RuntimeError as error:
        require("55P03" in str(error), str(error))
        s.report["rejections"]["child_first_inversion"] = {"stderr": str(error), "sqlstate": "55P03"}
    writer.finish()
    leader = Session(s, "leader_one")
    require(leader.sql("SELECT pg_try_advisory_lock(760106);") == "t", "leader lock failed")
    r["second_leader_acquired"] = s.sql("SELECT pg_try_advisory_lock(760106);")
    require(r["second_leader_acquired"] == "f", "two leaders acquired same lock")
    leader.sql("SELECT pg_advisory_unlock(760106);")


def refresh_sql():
    return """
    BEGIN;
    INSERT INTO dirty(kind,bucket) VALUES('event','2026-01-02')
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
    SELECT revision FROM dirty WHERE kind='event' AND bucket='2026-01-02' FOR UPDATE;
    DELETE FROM totals WHERE bucket='2026-01-02';
    INSERT INTO totals(bucket,ref,status,n)
      SELECT '2026-01-02',ref,NULL,count(*) FROM source
      WHERE created >= '2026-01-02' AND created < '2026-01-02 01:00:00+00' GROUP BY ref;
    INSERT INTO coverage VALUES('2026-01-02') ON CONFLICT DO NOTHING;
    DELETE FROM dirty WHERE kind='event' AND bucket='2026-01-02';
    COMMIT;
    """


def dirty_races(s):
    r = s.report["observations"]
    refresh = Session(s, "refresh")
    writer = Session(s, "backdated_writer")
    refresh.sql("BEGIN; SELECT revision FROM dirty WHERE kind='event' "
                "AND bucket='2026-01-02' FOR UPDATE; "
                "CREATE TEMP TABLE frozen AS SELECT ref,count(*)::bigint n FROM source "
                "WHERE created >= '2026-01-02' AND created < '2026-01-02 01:00:00+00' GROUP BY ref;")
    writer.send("INSERT INTO source(created,ref) VALUES('2026-01-02',NULL);")
    s.blocked("backdated_writer", "refresh")
    r["writer_blocked_after_source_snapshot"] = True
    refresh.sql("DELETE FROM totals WHERE bucket='2026-01-02'; "
                "INSERT INTO totals SELECT '2026-01-02',ref,NULL,n FROM frozen; "
                "INSERT INTO coverage VALUES('2026-01-02'); "
                "DELETE FROM dirty WHERE kind='event' AND bucket='2026-01-02'; COMMIT;")
    writer.finish()
    r["upsert_race"] = s.sql("SELECT (SELECT sum(n) FROM totals)||'|'||"
                            "(SELECT count(*) FROM source)||'|'||(SELECT count(*) FROM dirty);")
    require(r["upsert_race"] == "3|4|1", "writer invalidation lost")
    s.sql(refresh_sql())
    require(s.sql("SELECT sum(n) FROM totals;") == "4", "refresh not caught up")
    # Negative control deliberately swaps the trigger protocol.
    s.sql("""
    CREATE OR REPLACE FUNCTION mark_dirty() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket) VALUES('event',date_trunc('hour',NEW.created,'UTC'))
      ON CONFLICT DO NOTHING; RETURN NEW;
    END $$;
    INSERT INTO dirty VALUES('event','2026-01-02',1);
    """)
    refresh.sql("BEGIN; SELECT revision FROM dirty WHERE kind='event' AND bucket='2026-01-02' FOR UPDATE; "
                "DROP TABLE frozen; CREATE TEMP TABLE frozen AS SELECT ref,count(*)::bigint n FROM source GROUP BY ref;")
    writer.sql("INSERT INTO source(created,ref) VALUES('2026-01-02',NULL);")
    refresh.sql("DELETE FROM totals; INSERT INTO totals SELECT '2026-01-02',ref,NULL,n FROM frozen; "
                "DELETE FROM dirty; COMMIT;")
    r["rejected_do_nothing_race"] = s.sql("SELECT (SELECT sum(n) FROM totals)||'|'||"
                                        "(SELECT count(*) FROM source)||'|'||(SELECT count(*) FROM dirty);")
    require(r["rejected_do_nothing_race"] == "4|5|0", "negative control failed")
    s.sql("""
    CREATE OR REPLACE FUNCTION mark_dirty() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket) VALUES('event',date_trunc('hour',NEW.created,'UTC'))
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
      RETURN NEW; END $$;
    INSERT INTO dirty VALUES('event','2026-01-02',1);
    """)
    refresh.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) FROM source;")
    writer.sql("INSERT INTO source(created,ref) VALUES('2026-01-02','after_snapshot');")
    refresh.send("SELECT revision FROM dirty WHERE kind='event' AND bucket='2026-01-02' FOR UPDATE;")
    try:
        refresh.finish()
        raise AssertionError("stale repeatable-read marker lock unexpectedly succeeded")
    except RuntimeError as error:
        require("40001" in str(error), str(error))
        s.report["rejections"]["stale_refresh_snapshot"] = {"stderr": str(error), "sqlstate": "40001"}
    s.sql(refresh_sql())
    r["caught_up"] = s.sql("SELECT (SELECT sum(n) FROM totals)||'|'||"
                           "(SELECT count(*) FROM source)||'|'||(SELECT count(*) FROM dirty);")
    require(r["caught_up"] == "6|6|0", "refresh retry failed")
    s.sql("INSERT INTO totals VALUES('2026-01-03',NULL,NULL,1),('2026-01-03','',NULL,2); "
          "INSERT INTO totals VALUES('2026-01-03',NULL,NULL,7) "
          "ON CONFLICT(bucket,ref,status) DO UPDATE SET n=excluded.n;")
    r["nullable_dimensions"] = s.sql("SELECT coalesce(ref,'<NULL>')||'|'||n FROM totals "
                                     "WHERE bucket='2026-01-03' ORDER BY ref NULLS FIRST;")
    require(r["nullable_dimensions"] == "<NULL>|7\n|2", "null and empty string collapsed")
    s.sql("BEGIN; DELETE FROM source; INSERT INTO dirty VALUES('event','2026-01-02',1) "
          "ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1; COMMIT;")
    s.sql(refresh_sql())
    r["empty_hour"] = s.sql("SELECT (SELECT count(*) FROM totals WHERE bucket='2026-01-02')||'|'||"
                           "(SELECT count(*) FROM coverage WHERE bucket='2026-01-02');")
    require(r["empty_hour"] == "0|1", "empty hour lost coverage")
    # Bootstrap with an absent marker must block writers on its uncommitted insert.
    bootstrap = Session(s, "bootstrap")
    bootstrap.sql("BEGIN; INSERT INTO dirty VALUES('event','2026-01-02',1) "
                  "ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1; "
                  "CREATE TEMP TABLE initial_frozen AS SELECT ref,count(*)::bigint n FROM source GROUP BY ref;")
    writer.send("INSERT INTO source(created,ref) VALUES('2026-01-02','bootstrap_race');")
    s.blocked("backdated_writer", "bootstrap")
    bootstrap.sql("DELETE FROM totals WHERE bucket='2026-01-02'; "
                  "INSERT INTO totals SELECT '2026-01-02',ref,NULL,n FROM initial_frozen; "
                  "DELETE FROM dirty WHERE kind='event' AND bucket='2026-01-02'; COMMIT;")
    writer.finish()
    r["bootstrap_race"] = s.sql("SELECT (SELECT count(*) FROM totals WHERE bucket='2026-01-02')||'|'||"
                               "(SELECT count(*) FROM source)||'|'||(SELECT count(*) FROM dirty);")
    require(r["bootstrap_race"] == "0|1|1", "bootstrap invalidation lost")
    s.sql(refresh_sql())
    s.sql("INSERT INTO dirty VALUES('event','2026-01-02',1);")
    s.reject("refresh_rollback", "BEGIN; SELECT revision FROM dirty WHERE kind='event' "
             "AND bucket='2026-01-02' FOR UPDATE; DELETE FROM totals WHERE bucket='2026-01-02'; "
             "DO $$ BEGIN RAISE EXCEPTION 'injected refresh failure'; END $$; COMMIT;", "P0001")
    require(s.sql("SELECT sum(n) FROM totals WHERE bucket='2026-01-02';") == "1",
            "failed refresh changed committed materialization")
    require(s.sql("SELECT count(*) FROM dirty;") == "1", "failed refresh acknowledged marker")


def row_locations_and_horizon(s):
    r = s.report["observations"]
    s.sql("""
    CREATE TABLE locations(created timestamptz NOT NULL) PARTITION BY RANGE(created);
    CREATE TABLE locations_a PARTITION OF locations FOR VALUES FROM ('2026-01-01') TO ('2026-01-02');
    CREATE TABLE locations_b PARTITION OF locations FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');
    INSERT INTO locations VALUES('2026-01-01'),('2026-01-02');
    """)
    r["rejected_ctid_only"] = s.sql("BEGIN; WITH picked AS (SELECT ctid FROM locations LIMIT 1 FOR UPDATE), "
                                   "deleted AS (DELETE FROM locations WHERE ctid IN (SELECT ctid FROM picked) RETURNING created) "
                                   "SELECT count(*) FROM deleted; ROLLBACK;")
    require(r["rejected_ctid_only"] == "2", "ctid collision control failed")
    r["tableoid_ctid_delete"] = s.sql("WITH picked AS (SELECT tableoid,ctid FROM locations LIMIT 1 FOR UPDATE), "
                                    "deleted AS (DELETE FROM locations t USING picked p "
                                    "WHERE t.tableoid=p.tableoid AND t.ctid=p.ctid RETURNING created) "
                                    "SELECT count(*) FROM deleted;")
    require(r["tableoid_ctid_delete"] == "1", "bounded delete crossed leaf boundary")
    s.sql("""
    CREATE TABLE horizon(created timestamptz NOT NULL) PARTITION BY RANGE(created);
    DO $$ DECLARE day date; BEGIN
      FOR day IN SELECT '2026-01-01'::date + n FROM generate_series(0,7) n LOOP
        EXECUTE format('CREATE TABLE %I PARTITION OF horizon FOR VALUES FROM (%L) TO (%L)',
          'horizon_'||to_char(day,'YYYYMMDD'),day::text||' 00:00:00+00',(day+1)::text||' 00:00:00+00');
      END LOOP;
    END $$;
    CREATE TABLE horizon_default PARTITION OF horizon DEFAULT;
    INSERT INTO horizon VALUES('2026-01-01'),('2026-01-08'),('2026-01-09'),('2025-12-31');
    """)
    r["horizon"] = s.sql("SELECT count(*) FROM pg_inherits WHERE inhparent='horizon'::regclass; "
                         "SELECT tableoid::regclass||'|'||count(*) FROM horizon GROUP BY tableoid ORDER BY tableoid::regclass::text;")
    require(r["horizon"] == "9\nhorizon_20260101|1\nhorizon_20260108|1\nhorizon_default|2",
            "today/seven-ahead/DEFAULT routing mismatch")
    r["utc_buckets_in_dst_zone"] = s.sql("SET TimeZone='America/New_York'; "
        "SELECT to_char(date_trunc('hour','2026-11-01 01:30:00-04'::timestamptz,'UTC') AT TIME ZONE 'UTC',"
        "'YYYY-MM-DD HH24:MI')||'|'||to_char(date_trunc('hour','2026-11-01 01:30:00-05'::timestamptz,'UTC') "
        "AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI');")
    require(r["utc_buckets_in_dst_zone"] == "2026-11-01 05:00|2026-11-01 06:00", "DST buckets collapsed")


def statement_markers(s):
    r = s.report["observations"]
    s.sql("""
    CREATE FUNCTION mark_dirty_batch() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket)
        SELECT 'event',date_trunc('hour',created,'UTC') FROM new_rows
        GROUP BY date_trunc('hour',created,'UTC') ORDER BY date_trunc('hour',created,'UTC')
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
      RETURN NULL;
    END $$;
    CREATE TABLE batch_source(id bigint,created timestamptz NOT NULL,ref text) PARTITION BY RANGE(created);
    CREATE TABLE batch_day PARTITION OF batch_source FOR VALUES FROM ('2026-01-06') TO ('2026-01-07');
    CREATE TABLE batch_default PARTITION OF batch_source DEFAULT;
    CREATE TRIGGER batch_dirty AFTER INSERT ON batch_source REFERENCING NEW TABLE AS new_rows
      FOR EACH STATEMENT EXECUTE FUNCTION mark_dirty_batch();
    DELETE FROM dirty;
    INSERT INTO batch_source VALUES(1,'2026-01-06',NULL),(2,'2026-01-06',''),(3,'2026-01-07',NULL);
    """)
    r["statement_marker_initial"] = s.sql("SELECT count(*)||'|'||sum(revision) FROM dirty;")
    require(r["statement_marker_initial"] == "2|2", "statement trigger did not deduplicate hours")
    refresher = Session(s, "batch_refresh")
    writer = Session(s, "batch_writer")
    frozen = refresher.sql("BEGIN; SELECT revision FROM dirty WHERE kind='event' AND bucket='2026-01-06' FOR UPDATE; "
                           "SELECT count(*) FROM batch_source WHERE created='2026-01-06';")
    require(frozen == "1\n2", "batch refresh snapshot incorrect")
    writer.send("INSERT INTO batch_source VALUES(4,'2026-01-06','late'),(5,'2026-01-06','late');")
    s.blocked("batch_writer", "batch_refresh")
    refresher.sql("DELETE FROM dirty WHERE kind='event' AND bucket='2026-01-06'; COMMIT;")
    writer.finish()
    r["statement_marker_race"] = s.sql("SELECT (SELECT count(*) FROM batch_source WHERE created='2026-01-06')||'|'||"
        "(SELECT revision FROM dirty WHERE kind='event' AND bucket='2026-01-06');")
    require(r["statement_marker_race"] == "4|1", "statement invalidation lost")
    # Parent statement triggers do not run for maintenance SQL aimed at a leaf.
    s.sql("DELETE FROM dirty; INSERT INTO batch_default VALUES(6,'2026-01-07','direct_leaf');")
    r["direct_leaf_statement_marker_count"] = s.sql("SELECT count(*) FROM dirty;")
    require(r["direct_leaf_statement_marker_count"] == "0", "direct-leaf control changed")
    s.sql("""
    CREATE FUNCTION mark_dirty_changed() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket)
        SELECT 'event',bucket FROM (
          SELECT date_trunc('hour',created,'UTC') bucket FROM old_rows
          UNION SELECT date_trunc('hour',created,'UTC') bucket FROM new_rows
        ) affected ORDER BY bucket
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
      RETURN NULL;
    END $$;
    CREATE FUNCTION mark_dirty_deleted() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO dirty(kind,bucket)
        SELECT 'event',date_trunc('hour',created,'UTC') FROM old_rows
        GROUP BY date_trunc('hour',created,'UTC') ORDER BY date_trunc('hour',created,'UTC')
      ON CONFLICT(kind,bucket) DO UPDATE SET revision=dirty.revision+1;
      RETURN NULL;
    END $$;
    CREATE TRIGGER batch_changed AFTER UPDATE ON batch_source
      REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
      FOR EACH STATEMENT EXECUTE FUNCTION mark_dirty_changed();
    CREATE TRIGGER batch_deleted AFTER DELETE ON batch_source REFERENCING OLD TABLE AS old_rows
      FOR EACH STATEMENT EXECUTE FUNCTION mark_dirty_deleted();
    UPDATE batch_source SET created='2026-01-07',ref='changed' WHERE id=1;
    """)
    r["correction_old_new_markers"] = s.sql("SELECT count(*) FROM dirty;")
    require(r["correction_old_new_markers"] == "2", "correction lost old or new hour")
    s.sql("DELETE FROM dirty; UPDATE batch_source SET ref='renamed' WHERE id=2;")
    require(s.sql("SELECT count(*) FROM dirty WHERE bucket='2026-01-06';") == "1",
            "group-only correction not invalidated")
    s.sql("DELETE FROM dirty; BEGIN; DELETE FROM batch_source WHERE id=2; ROLLBACK;")
    require(s.sql("SELECT count(*) FROM dirty;") == "0", "rolled-back deletion leaked dirty marker")
    require(s.sql("SELECT count(*) FROM batch_source WHERE id=2;") == "1", "rolled-back deletion lost row")
    s.sql("DELETE FROM batch_source WHERE id=2;")
    r["parent_delete_marker"] = s.sql("SELECT count(*) FROM dirty WHERE bucket='2026-01-06';")
    require(r["parent_delete_marker"] == "1", "parent deletion missed dirty marker")


def expiry_snapshot(s):
    r = s.report["observations"]
    s.sql("""
    CREATE TABLE expiry(id bigint, created timestamptz NOT NULL) PARTITION BY RANGE(created);
    CREATE TABLE expiry_day PARTITION OF expiry FOR VALUES FROM ('2026-01-01') TO ('2026-01-02');
    CREATE TABLE expiry_default PARTITION OF expiry DEFAULT;
    INSERT INTO expiry SELECT n,'2026-01-01' FROM generate_series(1,3) n;
    CREATE TABLE expiry_totals(n bigint);
    INSERT INTO expiry_totals VALUES(3);
    CREATE TABLE expiry_coverage(bucket timestamptz PRIMARY KEY);
    INSERT INTO expiry_coverage VALUES('2026-01-01');
    """)
    drop_sql = ("BEGIN; SET LOCAL lock_timeout='250ms'; SET LOCAL statement_timeout='1s'; "
                "LOCK TABLE ONLY expiry IN ACCESS EXCLUSIVE MODE; "
                "DELETE FROM expiry_totals; DELETE FROM expiry_coverage; "
                "DROP TABLE expiry_day; COMMIT;")
    # Negative control: snapshot alone does not pin the parent's partition topology.
    unsafe = Session(s, "snapshot_without_parent")
    unsafe.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT n FROM expiry_totals;")
    s.sql(drop_sql)
    r["rejected_snapshot_without_parent_lock"] = unsafe.sql(
        "SELECT (SELECT count(*) FROM expiry)||'|'||(SELECT sum(n) FROM expiry_totals);")
    require(r["rejected_snapshot_without_parent_lock"] == "0|3", "DDL snapshot gap not reproduced")
    unsafe.sql("ROLLBACK;")
    s.sql("CREATE TABLE expiry_day PARTITION OF expiry FOR VALUES FROM ('2026-01-01') TO ('2026-01-02'); "
          "INSERT INTO expiry SELECT n,'2026-01-01' FROM generate_series(1,3) n; "
          "INSERT INTO expiry_totals VALUES(3); INSERT INTO expiry_coverage VALUES('2026-01-01');")
    safe = Session(s, "snapshot_with_parent")
    safe.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY expiry IN ACCESS SHARE MODE; "
             "SELECT n FROM expiry_totals;")
    s.reject("expiry_reader_busy", drop_sql, "55P03")
    r["pinned_read"] = safe.sql("SELECT (SELECT count(*) FROM expiry)||'|'||(SELECT sum(n) FROM expiry_totals);")
    require(r["pinned_read"] == "3|3", "pinned snapshot mismatch")
    safe.sql("COMMIT;")
    s.sql(drop_sql)
    r["post_expiry"] = s.sql("SELECT (SELECT count(*) FROM expiry)||'|'||"
                             "(SELECT count(*) FROM expiry_totals)||'|'||(SELECT count(*) FROM expiry_coverage);")
    require(r["post_expiry"] == "0|0|0", "expiry not atomic")


def conversion(s):
    r = s.report["observations"]
    s.sql("""
    CREATE ROLE maint; CREATE ROLE observer;
    GRANT USAGE,CREATE ON SCHEMA public TO maint;
    GRANT USAGE ON SCHEMA public TO observer;
    SET ROLE maint;
    CREATE TABLE refs(ref text PRIMARY KEY); INSERT INTO refs VALUES('r');
    CREATE TABLE conv_event(id bigserial PRIMARY KEY, created timestamptz NOT NULL,
                            ref text REFERENCES refs(ref));
    CREATE TABLE captured(id bigint);
    CREATE FUNCTION capture_insert() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO captured VALUES(NEW.id); RETURN NEW; END $$;
    CREATE TRIGGER capture AFTER INSERT ON conv_event FOR EACH ROW EXECUTE FUNCTION capture_insert();
    INSERT INTO conv_event(created,ref) VALUES('2026-01-01','r'),('2026-01-02','r');
    CREATE VIEW conv_view AS SELECT count(*) n FROM conv_event;
    CREATE FUNCTION conv_count() RETURNS bigint LANGUAGE SQL
      BEGIN ATOMIC SELECT count(*) FROM conv_event; END;
    GRANT SELECT ON conv_event,conv_view TO observer;
    RESET ROLE;
    """)
    old_oid = s.sql("SELECT 'conv_event'::regclass::oid;")
    s.reject("conversion_rollback", "SET ROLE maint; BEGIN; ALTER TABLE conv_event RENAME TO conv_old; "
             "CREATE TABLE conv_event(id bigint,created timestamptz) PARTITION BY RANGE(created); "
             "DO $$ BEGIN RAISE EXCEPTION 'injected replacement failure'; END $$; COMMIT;", "P0001")
    require(s.sql("SELECT 'conv_event'::regclass::oid;") == old_oid, "conversion rollback changed parent")
    connection = Session(s, "existing_plan")
    connection.sql("PREPARE count_before AS SELECT count(*) FROM conv_event; EXECUTE count_before;")
    s.sql("""
    SET ROLE maint;
    BEGIN; SET LOCAL lock_timeout='250ms';
    LOCK TABLE conv_event IN ACCESS EXCLUSIVE MODE;
    ALTER TABLE conv_event RENAME TO conv_old;
    CREATE TABLE conv_event(id bigint NOT NULL DEFAULT nextval('conv_event_id_seq'),
      created timestamptz NOT NULL,ref text REFERENCES refs(ref),PRIMARY KEY(id,created))
      PARTITION BY RANGE(created);
    CREATE TABLE conv_d1 PARTITION OF conv_event FOR VALUES FROM ('2026-01-01') TO ('2026-01-02');
    CREATE TABLE conv_d2 PARTITION OF conv_event FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');
    CREATE TABLE conv_default PARTITION OF conv_event DEFAULT;
    INSERT INTO conv_event(id,created,ref) SELECT id,created,ref FROM conv_old;
    ALTER SEQUENCE conv_event_id_seq OWNED BY conv_event.id;
    CREATE OR REPLACE VIEW conv_view AS SELECT count(*) n FROM conv_event;
    CREATE OR REPLACE FUNCTION conv_count() RETURNS bigint LANGUAGE SQL
      BEGIN ATOMIC SELECT count(*) FROM conv_event; END;
    CREATE TRIGGER capture AFTER INSERT ON conv_event FOR EACH ROW EXECUTE FUNCTION capture_insert();
    GRANT SELECT ON conv_event TO observer;
    DROP TABLE conv_old;
    COMMIT;
    RESET ROLE;
    """)
    require(connection.sql("EXECUTE count_before;") == "2", "cached plan did not rebind")
    require(s.sql("SELECT count(*) FROM captured;") == "2", "copy fired business trigger")
    r["conversion_sequence_owner"] = s.sql(
        "SELECT c.relname||'.'||a.attname FROM pg_depend d JOIN pg_class c ON c.oid=d.refobjid "
        "JOIN pg_attribute a ON a.attrelid=c.oid AND a.attnum=d.refobjsubid "
        "WHERE d.objid='conv_event_id_seq'::regclass AND d.deptype='a';")
    require(r["conversion_sequence_owner"] == "conv_event.id", "sequence ownership lost")
    r["conversion_next_id"] = s.sql("SET ROLE maint; INSERT INTO conv_event(created,ref) "
                                   "VALUES('2026-01-02','r') RETURNING id;")
    require(r["conversion_next_id"] == "3", "sequence position lost")
    require(s.sql("SELECT count(*) FROM captured;") == "3", "new trigger did not fire")
    require(connection.sql("EXECUTE count_before;") == "3", "existing connection read stale heap")
    require(s.sql("SELECT conv_count();") == "3", "SQL function dependency did not rebind")
    require(s.sql("SET ROLE observer; SELECT n FROM conv_view; SELECT count(*) FROM conv_event;") == "3\n3",
            "grants/view not preserved")
    s.reject("conversion_foreign_key", "SET ROLE maint; INSERT INTO conv_event(created,ref) "
             "VALUES('2026-01-02','missing');", "23503")
    s.reject("startup_without_ddl_privileges", "SET ROLE observer; CREATE TABLE denied(id bigint);", "42501")
    s.reject("startup_without_parent_ownership", "SET ROLE observer; "
             "ALTER TABLE conv_event ADD COLUMN denied bigint;", "42501")
    require(s.sql("SET ROLE observer; SELECT count(*) FROM conv_event;") == "3", "DDL failure broke raw reads")
    r["conversion"] = {"copied_rows": 2, "copy_trigger_fires": 0,
                       "prepared_plan_count": 2, "post_insert_count": 3,
                       "existing_connection_after_insert": 3, "atomic_sql_function_count": 3,
                       "owner_superuser": s.sql("SELECT rolsuper FROM pg_roles WHERE rolname='maint';")}
    s.reject("partition_id_only_upsert", "SET ROLE maint; INSERT INTO conv_event(id,created,ref) "
             "VALUES(1,'2026-01-02','r') ON CONFLICT(id) DO NOTHING;", "42P10")
    r["composite_key_not_global_id_uniqueness"] = s.sql("BEGIN; SET LOCAL ROLE maint; "
        "INSERT INTO conv_event(id,created,ref) VALUES(1,'2026-01-02','r'); "
        "SELECT count(*) FROM conv_event WHERE id=1; ROLLBACK;")
    require(r["composite_key_not_global_id_uniqueness"] == "2", "composite key control failed")
    # View dependency is object-based: renaming the heap alone is insufficient.
    s.sql("CREATE TABLE dependency_heap(id bigint); INSERT INTO dependency_heap VALUES(1); "
          "CREATE VIEW dependency_view AS SELECT count(*) n FROM dependency_heap; "
          "ALTER TABLE dependency_heap RENAME TO dependency_old; CREATE TABLE dependency_heap(id bigint);")
    r["rejected_rename_only_view"] = s.sql("SELECT (SELECT n FROM dependency_view)||'|'||"
                                        "(SELECT count(*) FROM dependency_heap);")
    require(r["rejected_rename_only_view"] == "1|0", "view dependency control failed")
    s.sql("CREATE TABLE sequence_heap(id bigserial PRIMARY KEY); "
          "CREATE TABLE sequence_replacement(id bigint DEFAULT nextval('sequence_heap_id_seq'));")
    s.reject("drop_old_sequence_owner", "DROP TABLE sequence_heap;", "2BP01")
    r["rejected_sequence_owner_cascade"] = s.sql("BEGIN; DROP TABLE sequence_heap CASCADE; "
        "SELECT count(*) FROM pg_attrdef WHERE adrelid='sequence_replacement'::regclass; ROLLBACK;")
    require(r["rejected_sequence_owner_cascade"] == "0", "sequence cascade control failed")


def smoke_costs(s):
    """Equal tiny cohorts, deliberately not a production acceptance fixture."""
    s.sql("""
    CREATE TABLE delete_cohort(id bigint,created timestamptz);
    INSERT INTO delete_cohort SELECT n,'2026-01-01' FROM generate_series(1,10000) n;
    CREATE TABLE drop_cohort(id bigint,created timestamptz) PARTITION BY RANGE(created);
    CREATE TABLE drop_day PARTITION OF drop_cohort FOR VALUES FROM ('2026-01-01') TO ('2026-01-02');
    INSERT INTO drop_cohort SELECT id,created FROM delete_cohort;
    CREATE TABLE clock_sample(started timestamptz);
    """)
    deletion = s.sql("BEGIN; INSERT INTO clock_sample VALUES(clock_timestamp()); DELETE FROM delete_cohort; "
                     "SELECT extract(epoch FROM clock_timestamp()-started)*1000 FROM clock_sample; COMMIT;")
    drop = s.sql("TRUNCATE clock_sample; BEGIN; LOCK TABLE ONLY drop_cohort IN ACCESS EXCLUSIVE MODE; "
                "INSERT INTO clock_sample VALUES(clock_timestamp()); DROP TABLE drop_day; "
                "SELECT extract(epoch FROM clock_timestamp()-started)*1000 FROM clock_sample; COMMIT;")
    s.report["observations"]["toy_equal_expiry_cost"] = {
        "rows_each": 10000, "delete_server_ms": float(deletion), "drop_server_ms": float(drop),
        "includes_commit": False, "includes_vacuum": False, "samples": 1}
    s.sql("CREATE TABLE write_fixture(id bigserial PRIMARY KEY,created timestamptz NOT NULL,ref text);")
    writers = [Session(s, f"cost_writer_{i}") for i in range(4)]
    for writer in writers:
        writer.sql("SET statement_timeout='10s';")
    samples = []
    for sample in range(3):
        # Alternate order, retaining every sample. Baseline and treatment have identical rows.
        variants = ["none", "row", "statement"] if sample % 2 == 0 else ["statement", "row", "none"]
        for variant in variants:
            s.sql("DROP TRIGGER IF EXISTS fixture_dirty ON write_fixture; TRUNCATE write_fixture; "
                  "DELETE FROM dirty;")
            if variant == "row":
                s.sql("CREATE TRIGGER fixture_dirty AFTER INSERT ON write_fixture "
                      "FOR EACH ROW EXECUTE FUNCTION mark_dirty();")
            elif variant == "statement":
                s.sql("CREATE TRIGGER fixture_dirty AFTER INSERT ON write_fixture "
                      "REFERENCING NEW TABLE AS new_rows FOR EACH STATEMENT EXECUTE FUNCTION mark_dirty_batch();")
            barrier = threading.Barrier(5)
            def write_batches(writer):
                latencies = []
                barrier.wait(timeout=10)
                for _ in range(20):
                    output = writer.sql("BEGIN; SELECT extract(epoch FROM clock_timestamp()); "
                        "INSERT INTO write_fixture(created,ref) SELECT '2026-01-10', "
                        "CASE WHEN n % 2=0 THEN NULL ELSE '' END FROM generate_series(1,100) n; "
                        "COMMIT; SELECT extract(epoch FROM clock_timestamp());")
                    start, end = map(float, output.splitlines())
                    latencies.append((end-start)*1000)
                return latencies
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                futures = [pool.submit(write_batches, writer) for writer in writers]
                start = time.monotonic()
                barrier.wait(timeout=10)
                latencies = [latency for future in futures for latency in future.result()]
                elapsed = time.monotonic() - start
            require(s.sql("SELECT count(*) FROM write_fixture;") == "8000", "write fixture mismatch")
            samples.append({"sample": sample, "dirty_tracking": variant, "rows": 8000,
                            "writers": 4, "transactions_each": 20, "rows_per_transaction": 100,
                            "wall_ms": round(elapsed*1000, 3), "rows_per_second": round(8000/elapsed, 3),
                            "transaction_server_ms": [round(v, 3) for v in latencies],
                            "transaction_server_p95_ms": round(sorted(latencies)[75], 3)})
    s.report["observations"]["toy_four_writer_cost"] = samples
    s.sql("CREATE TABLE toy_totals AS SELECT ref,count(*)::bigint n FROM write_fixture GROUP BY ref; "
          "ANALYZE write_fixture; ANALYZE toy_totals;")
    raw = "SELECT ref,count(*)::bigint n FROM write_fixture WHERE created >= '2026-01-10' AND created < '2026-01-10 01:00:00+00' GROUP BY ref"
    materialized = "SELECT ref,n FROM toy_totals"
    oracle = s.sql(f"SELECT jsonb_agg(to_jsonb(q) ORDER BY ref NULLS FIRST) FROM ({raw}) q;")
    require(s.sql(f"SELECT jsonb_agg(to_jsonb(q) ORDER BY ref NULLS FIRST) FROM ({materialized}) q;") == oracle,
            "toy summary differs from raw oracle")
    timings = {"rows": 8000, "groups": 2, "concurrency": 1, "warm_samples": 5,
               "raw_sql": raw, "summary_sql": materialized}
    for name, query in (("raw", raw), ("summary", materialized)):
        timings[name + "_server_ms"] = [json.loads(s.sql(
            "EXPLAIN (ANALYZE,BUFFERS,FORMAT JSON) " + query))[0]["Execution Time"] for _ in range(5)]
    s.report["observations"]["toy_query_cost"] = timings


def setup_append(s):
    s.sql("""
    CREATE TABLE append_notice(id bigserial PRIMARY KEY,kind text NOT NULL,bucket timestamptz NOT NULL);
    CREATE INDEX append_notice_bucket ON append_notice(kind,bucket,id);
    CREATE TABLE append_state(kind text PRIMARY KEY,revision bigint NOT NULL DEFAULT 0);
    INSERT INTO append_state(kind) VALUES('event'),('execution_status'),('execution_creation'),('worker_status');
    CREATE TABLE append_source(id bigserial,created timestamptz NOT NULL,ref text,
      PRIMARY KEY(id,created)) PARTITION BY RANGE(created);
    CREATE TABLE append_day PARTITION OF append_source FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');
    CREATE TABLE append_default PARTITION OF append_source DEFAULT;
    CREATE INDEX append_source_time ON append_source(created);
    CREATE TABLE append_totals(kind text NOT NULL,bucket timestamptz NOT NULL,ref text,status text,n bigint NOT NULL,
      UNIQUE NULLS NOT DISTINCT(kind,bucket,ref,status));
    CREATE TABLE append_coverage(kind text NOT NULL,bucket timestamptz NOT NULL,refreshed timestamptz NOT NULL,
      PRIMARY KEY(kind,bucket));
    CREATE FUNCTION append_insert_notice() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO append_notice(kind,bucket)
        SELECT 'event',date_trunc('hour',created,'UTC') FROM new_rows
        GROUP BY date_trunc('hour',created,'UTC') ORDER BY date_trunc('hour',created,'UTC');
      RETURN NULL;
    END $$;
    CREATE TRIGGER append_source_insert AFTER INSERT ON append_source REFERENCING NEW TABLE AS new_rows
      FOR EACH STATEMENT EXECUTE FUNCTION append_insert_notice();
    CREATE FUNCTION append_changed_notice() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO append_notice(kind,bucket) SELECT 'event',bucket FROM (
        SELECT date_trunc('hour',created,'UTC') bucket FROM old_rows
        UNION SELECT date_trunc('hour',created,'UTC') FROM new_rows
      ) hours ORDER BY bucket; RETURN NULL; END $$;
    CREATE FUNCTION append_deleted_notice() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO append_notice(kind,bucket) SELECT 'event',date_trunc('hour',created,'UTC') FROM old_rows
        GROUP BY date_trunc('hour',created,'UTC') ORDER BY date_trunc('hour',created,'UTC');
      RETURN NULL; END $$;
    CREATE TRIGGER append_source_update AFTER UPDATE ON append_source
      REFERENCING OLD TABLE AS old_rows NEW TABLE AS new_rows
      FOR EACH STATEMENT EXECUTE FUNCTION append_changed_notice();
    CREATE TRIGGER append_source_delete AFTER DELETE ON append_source REFERENCING OLD TABLE AS old_rows
      FOR EACH STATEMENT EXECUTE FUNCTION append_deleted_notice();
    """)


def append_reset(s):
    s.sql("TRUNCATE append_source,append_notice,append_totals,append_coverage RESTART IDENTITY; "
          "UPDATE append_state SET revision=0;")


def append_start(builder, cap=100):
    # LOCK is a utility statement: no data snapshot is taken before parent pinning.
    return builder.sql(f"""
    BEGIN ISOLATION LEVEL REPEATABLE READ;
    SET LOCAL lock_timeout='2s'; SET LOCAL statement_timeout='5s';
    LOCK TABLE ONLY append_source IN ACCESS SHARE MODE;
    SELECT revision FROM append_state WHERE kind='event' FOR UPDATE;
    UPDATE append_state SET revision=revision+1 WHERE kind='event';
    CREATE TEMP TABLE captured_ids ON COMMIT DROP AS
      SELECT id FROM append_notice WHERE kind='event' AND bucket='2026-01-02' ORDER BY id LIMIT {cap};
    CREATE TEMP TABLE captured_groups ON COMMIT DROP AS
      SELECT ref,count(*)::bigint n FROM append_source
      WHERE created >= '2026-01-02' AND created < '2026-01-02 01:00:00+00' GROUP BY ref;
    SELECT coalesce(json_agg(id ORDER BY id),'[]') FROM captured_ids;
    SELECT coalesce(sum(n),0) FROM captured_groups;
    """)


def append_finish(builder):
    return builder.sql("""
    DELETE FROM append_totals WHERE kind='event' AND bucket='2026-01-02';
    INSERT INTO append_totals SELECT 'event','2026-01-02',ref,NULL,n FROM captured_groups;
    INSERT INTO append_coverage VALUES('event','2026-01-02',clock_timestamp())
      ON CONFLICT(kind,bucket) DO UPDATE SET refreshed=excluded.refreshed;
    DELETE FROM append_notice n USING captured_ids c WHERE n.id=c.id;
    COMMIT;
    """)


def append_counts(s):
    return s.sql("SELECT (SELECT coalesce(sum(n),0) FROM append_totals)||'|'||"
                 "(SELECT count(*) FROM append_source)||'|'||(SELECT count(*) FROM append_notice);")


def append_protocols(s):
    r = s.report["observations"]
    lower = Session(s, "append_lower_writer")
    builder = Session(s, "append_builder")
    # A lower sequence value is already inserted but uncommitted when the upper ID commits.
    append_reset(s)
    lower.sql("BEGIN; INSERT INTO append_source(created,ref) VALUES('2026-01-02',NULL); "
              "SELECT id FROM append_notice;")
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','');")
    snapshot = append_start(builder)
    require(snapshot == "0\n[2]\n1", f"out-of-order snapshot incorrect: {snapshot}")
    lower.sql("COMMIT;")
    append_finish(builder)
    ids = s.sql("SELECT json_agg(id ORDER BY id) FROM append_notice;")
    require(ids == "[1]", f"lower committed ID was acknowledged: {ids}")
    r["append_out_of_order_commit"] = {"snapshot_ids": [2], "pending_ids": [1],
                                      "summary_raw_pending": append_counts(s)}
    require(append_counts(s) == "1|2|1", "out-of-order write lost invalidation")
    append_start(builder)
    append_finish(builder)
    require(append_counts(s) == "2|2|0", "out-of-order catch-up failed")

    # Reserve the lower ID without even inserting its notification before the snapshot.
    append_reset(s)
    require(lower.sql("SELECT nextval('append_notice_id_seq');") == "1", "reservation wrong")
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','upper');")
    require(append_start(builder) == "0\n[2]\n1", "preassigned snapshot wrong")
    # Maintenance-directed leaf writes supply their own transactional notification.
    lower.sql("BEGIN; LOCK TABLE ONLY append_source IN ACCESS SHARE MODE; "
              "INSERT INTO append_day(created,ref) VALUES('2026-01-02','lower'); "
              "INSERT INTO append_notice(id,kind,bucket) VALUES(1,'event','2026-01-02'); COMMIT;")
    append_finish(builder)
    r["append_preassigned_lower_id"] = {"snapshot_ids": [2], "pending_ids": json.loads(s.sql(
        "SELECT json_agg(id ORDER BY id) FROM append_notice;")), "summary_raw_pending": append_counts(s)}
    require(r["append_preassigned_lower_id"]["pending_ids"] == [1], "preassigned lower ID lost")
    require(append_counts(s) == "1|2|1", "preassigned lower write lost")
    append_start(builder)
    append_finish(builder)
    require(append_counts(s) == "2|2|0", "preassigned catch-up failed")

    # Negative control freezes a source SELECT in RC, then range-acknowledges after the late commit.
    append_reset(s)
    lower.sql("BEGIN; INSERT INTO append_source(created,ref) VALUES('2026-01-02','lower');")
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','upper');")
    builder.sql("BEGIN; LOCK TABLE ONLY append_source IN ACCESS SHARE MODE; "
                "SELECT revision FROM append_state WHERE kind='event' FOR UPDATE; "
                "CREATE TEMP TABLE bad_groups ON COMMIT DROP AS SELECT ref,count(*)::bigint n FROM append_source GROUP BY ref; "
                "CREATE TEMP TABLE bad_max ON COMMIT DROP AS SELECT max(id) id FROM append_notice;")
    lower.sql("COMMIT;")
    builder.sql("INSERT INTO append_totals SELECT 'event','2026-01-02',ref,NULL,n FROM bad_groups; "
                "DELETE FROM append_notice WHERE id <= (SELECT id FROM bad_max); COMMIT;")
    r["rejected_append_max_id_ack"] = append_counts(s)
    require(r["rejected_append_max_id_ack"] == "1|2|0", "max-ID regression control failed")

    # Bootstrap does not need a flag; writers after the snapshot must create independent records.
    append_reset(s)
    s.sql("INSERT INTO append_day(created,ref) VALUES('2026-01-02',NULL);")
    require(append_start(builder) == "0\n[]\n1", "bootstrap snapshot wrong")
    lower.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','late_bootstrap');")
    require(s.sql("SELECT cardinality(pg_blocking_pids(pid))=0 FROM pg_stat_activity "
                  "WHERE application_name='append_lower_writer';") == "t", "builder blocked writer")
    append_finish(builder)
    r["append_bootstrap"] = append_counts(s)
    require(r["append_bootstrap"] == "1|2|1", "flag-free bootstrap lost writer")
    append_start(builder)
    append_finish(builder)
    require(append_counts(s) == "2|2|0", "bootstrap catch-up failed")

    append_reset(s)
    for _ in range(3):
        s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02',NULL);")
    require(append_start(builder, cap=1) == "0\n[1]\n3", "bounded capture did not read whole hour")
    append_finish(builder)
    r["append_bounded_capture"] = append_counts(s)
    require(r["append_bounded_capture"] == "3|3|2", "bounded capture over-acknowledged")
    append_start(builder, cap=2)
    append_finish(builder)
    require(append_counts(s) == "3|3|0", "bounded catch-up double counted")
    append_reset(s)
    require(append_start(builder) == "0\n[]\n0", "empty bootstrap wrong")
    append_finish(builder)
    r["append_empty_coverage"] = s.sql("SELECT count(*) FROM append_coverage;")
    require(r["append_empty_coverage"] == "1", "empty hour lacks coverage")
    append_reset(s)
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02',NULL),('2026-01-02','gone');")
    append_start(builder)
    append_finish(builder)
    s.sql("UPDATE append_source SET created='2026-01-03',ref='moved' WHERE id=2; "
          "DELETE FROM append_source WHERE id=1;")
    snapshot = append_start(builder)
    require(snapshot == "1\n[2, 4]\n0", f"correction/deletion capture wrong: {snapshot}")
    append_finish(builder)
    r["append_correction_delete"] = {
        "cleared_hour_summary_groups": int(s.sql("SELECT count(*) FROM append_totals;")),
        "other_hour_pending_ids": json.loads(s.sql("SELECT json_agg(id ORDER BY id) FROM append_notice;"))}
    require(r["append_correction_delete"] == {"cleared_hour_summary_groups": 0,"other_hour_pending_ids": [3]},
            "correction or group removal over-acknowledged another hour")
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','rollback');")
    append_start(builder)
    builder.send("DELETE FROM append_totals; DELETE FROM append_notice n USING captured_ids c WHERE n.id=c.id; "
                 "DO $$ BEGIN RAISE EXCEPTION 'injected append refresh failure'; END $$;")
    try:
        builder.finish()
        raise AssertionError("injected append refresh failure succeeded")
    except RuntimeError as error:
        require("P0001" in str(error), str(error))
        s.report["rejections"]["append_refresh_rollback"] = {"sqlstate": "P0001", "stderr": str(error)}
    r["append_refresh_rollback_pending_ids"] = json.loads(s.sql("SELECT json_agg(id ORDER BY id) FROM append_notice;"))
    require(r["append_refresh_rollback_pending_ids"] == [3,5], "failed refresh acknowledged notification IDs")


def append_builders_and_expiry(s):
    r = s.report["observations"]
    append_reset(s)
    # A seemingly useful FK to the builder state would reintroduce writer blocking.
    s.sql("CREATE TABLE append_fk_notice(id bigserial PRIMARY KEY,kind text REFERENCES append_state(kind));")
    fk_builder = Session(s, "append_fk_builder")
    fk_writer = Session(s, "append_fk_writer")
    fk_builder.sql("BEGIN; LOCK TABLE ONLY append_source IN ACCESS SHARE MODE; "
                   "SELECT revision FROM append_state WHERE kind='event' FOR UPDATE;")
    fk_writer.send("INSERT INTO append_fk_notice(kind) VALUES('event');")
    s.blocked("append_fk_writer", "append_fk_builder")
    r["rejected_append_notice_fk_to_state_blocks_writer"] = True
    fk_builder.sql("ROLLBACK;")
    fk_writer.finish()
    s.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','initial');")
    one = Session(s, "append_builder_one")
    two = Session(s, "append_builder_two")
    writer = Session(s, "append_free_writer")
    append_start(one)
    two.send("BEGIN ISOLATION LEVEL REPEATABLE READ; SET LOCAL lock_timeout='2s'; "
             "LOCK TABLE ONLY append_source IN ACCESS SHARE MODE; "
             "SELECT revision FROM append_state WHERE kind='event' FOR UPDATE;")
    s.blocked("append_builder_two", "append_builder_one")
    writer.sql("SET statement_timeout='1s'; INSERT INTO append_source(created,ref) VALUES('2026-01-02','while_builders_wait');")
    r["append_writer_committed_while_builders_serialized"] = True
    append_finish(one)
    try:
        two.finish()
        raise AssertionError("stale waiting builder should require a fresh transaction")
    except RuntimeError as error:
        require("40001" in str(error), str(error))
        s.report["rejections"]["append_waiting_builder_snapshot"] = {"sqlstate": "40001", "stderr": str(error)}
    r["append_serialized_first_result"] = append_counts(s)
    require(r["append_serialized_first_result"] == "1|2|1", "first builder lost concurrent notification")
    retry = Session(s, "append_builder_two_retry")
    append_start(retry)
    append_finish(retry)
    r["append_serialized_retry_result"] = append_counts(s)
    require(r["append_serialized_retry_result"] == "2|2|0", "builder retry failed")

    append_start(one)
    expiry_sql = """
    BEGIN; SET LOCAL lock_timeout='250ms'; SET LOCAL statement_timeout='1s';
    LOCK TABLE ONLY append_source IN ACCESS EXCLUSIVE MODE;
    SELECT revision FROM append_state WHERE kind='event' FOR UPDATE;
    UPDATE append_state SET revision=revision+1 WHERE kind='event';
    DELETE FROM append_totals WHERE kind='event' AND bucket >= '2026-01-02' AND bucket < '2026-01-03';
    DELETE FROM append_coverage WHERE kind='event' AND bucket >= '2026-01-02' AND bucket < '2026-01-03';
    DELETE FROM append_notice WHERE kind='event' AND bucket >= '2026-01-02' AND bucket < '2026-01-03';
    DROP TABLE append_day;
    COMMIT;
    """
    s.reject("append_expiry_refresh_busy", expiry_sql, "55P03")
    append_finish(one)
    s.sql(expiry_sql)
    r["append_expiry_atomic"] = append_counts(s) + "|" + s.sql("SELECT count(*) FROM append_coverage;")
    require(r["append_expiry_atomic"] == "0|0|0|0", "append expiry not atomic")
    # A request acquiring its parent lock behind expiry takes its snapshot afterward.
    s.sql("CREATE TABLE append_day PARTITION OF append_source FOR VALUES FROM ('2026-01-02') TO ('2026-01-03'); "
          "INSERT INTO append_source(created,ref) VALUES('2026-01-02','again');")
    append_start(one)
    append_finish(one)
    expiry = Session(s, "append_expiry")
    reader = Session(s, "append_read_after_expiry")
    expiry.sql("BEGIN; LOCK TABLE ONLY append_source IN ACCESS EXCLUSIVE MODE;")
    reader.send("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY append_source IN ACCESS SHARE MODE; "
                "SELECT count(*) FROM append_source; SELECT count(*) FROM append_totals; "
                "SELECT count(*) FROM append_coverage; COMMIT;")
    s.blocked("append_read_after_expiry", "append_expiry")
    expiry.sql("SELECT revision FROM append_state WHERE kind='event' FOR UPDATE; "
               "UPDATE append_state SET revision=revision+1 WHERE kind='event'; "
               "DELETE FROM append_totals; DELETE FROM append_coverage; DELETE FROM append_notice; "
               "DROP TABLE append_day; COMMIT;")
    r["append_read_waiting_for_expiry"] = reader.finish()
    require(r["append_read_waiting_for_expiry"] == "0\n0\n0", "reader pinned a pre-lock snapshot")
    writer.sql("INSERT INTO append_source(created,ref) VALUES('2026-01-02','after_expiry');")
    require(s.sql("SELECT tableoid::regclass FROM append_source;") == "append_default", "late row did not route DEFAULT")
    r["append_after_expiry_writer"] = append_counts(s)
    require(r["append_after_expiry_writer"] == "0|1|1", "post-expiry notification lost")
    append_reset(s)
    s.sql("CREATE TABLE append_day PARTITION OF append_source FOR VALUES FROM ('2026-01-02') TO ('2026-01-03');")


def append_write_costs(s):
    """Paired ingestion measurements; server loops avoid per-query psql pacing."""
    s.sql("""
    CREATE FUNCTION append_payload(i bigint) RETURNS jsonb LANGUAGE SQL IMMUTABLE AS $$
      SELECT jsonb_build_object('request_id',i,'host','node-'||i%100,
        'records',(SELECT string_agg(md5(i::text||':'||g::text),'') FROM generate_series(1,32) g),
        'labels',jsonb_build_object('region','test-region','attempt',i%3));
    $$;
    CREATE TABLE cost_toy(id bigserial PRIMARY KEY,created timestamptz NOT NULL,ref text);
    CREATE TABLE cost_event(id bigserial PRIMARY KEY,trigger bigint,trigger_ref text NOT NULL,
      config jsonb,payload jsonb,source bigint,source_ref text,created timestamptz NOT NULL DEFAULT now(),
      rule bigint,rule_ref text,trace_tag text);
    CREATE INDEX cost_event_trigger ON cost_event(trigger);
    CREATE INDEX cost_event_ref ON cost_event(trigger_ref);
    CREATE INDEX cost_event_source ON cost_event(source);
    CREATE INDEX cost_event_time ON cost_event(created DESC);
    CREATE INDEX cost_event_trigger_time ON cost_event(trigger,created DESC);
    CREATE INDEX cost_event_ref_time ON cost_event(trigger_ref,created DESC);
    CREATE INDEX cost_event_source_time ON cost_event(source,created DESC);
    CREATE INDEX cost_event_payload ON cost_event USING gin(payload);
    CREATE INDEX cost_event_trace ON cost_event(trace_tag) WHERE trace_tag IS NOT NULL;
    CREATE TABLE cost_history(time timestamptz NOT NULL DEFAULT now(),operation text NOT NULL,
      entity_id bigint NOT NULL,entity_ref text,changed_fields text[] NOT NULL DEFAULT '{}',
      old_values jsonb,new_values jsonb);
    CREATE INDEX cost_history_time ON cost_history(time DESC);
    CREATE INDEX cost_history_entity ON cost_history(entity_id,time DESC);
    CREATE INDEX cost_history_ref ON cost_history(entity_ref,time DESC);
    CREATE INDEX cost_history_status ON cost_history(time DESC) WHERE 'status'=ANY(changed_fields);
    CREATE INDEX cost_history_fields ON cost_history USING gin(changed_fields);
    CREATE FUNCTION append_history_notice() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
      INSERT INTO append_notice(kind,bucket)
        SELECT kind,bucket FROM (
          SELECT 'execution_status'::text kind,date_trunc('hour',time,'UTC') bucket FROM new_rows
            WHERE 'status'=ANY(changed_fields)
          UNION
          SELECT 'execution_creation',date_trunc('hour',time,'UTC') FROM new_rows WHERE operation='INSERT'
        ) hours ORDER BY kind,bucket;
      RETURN NULL; END $$;
    """)
    profiles = [
        ("toy-batch", "cost_toy", 100, "created,ref", "append_insert_notice",
         "SELECT '2026-01-10'::timestamptz created,CASE WHEN n%2=0 THEN NULL ELSE '' END ref FROM generate_series(1,100) n"),
        ("event-batch", "cost_event", 100, "trigger_ref,created,payload", "append_insert_notice",
         "SELECT 'evidence.trigger_'||n%8 trigger_ref,'2026-01-10'::timestamptz created,append_payload(n) payload FROM generate_series(1,100) n"),
        ("history-batch", "cost_history", 100, "time,operation,entity_id,entity_ref,changed_fields,old_values,new_values", "append_history_notice",
         "SELECT '2026-01-10'::timestamptz time,CASE WHEN n%4=0 THEN 'INSERT' ELSE 'UPDATE' END operation,"
         "n::bigint entity_id,'evidence.action_'||n%8 entity_ref,CASE WHEN n%4=0 THEN '{}'::text[] ELSE ARRAY['status'] END changed_fields,"
         "CASE WHEN n%4=0 THEN NULL ELSE jsonb_build_object('status','running') END old_values,"
         "CASE WHEN n%4=0 THEN jsonb_build_object('status','requested','action_ref','evidence.action_'||n%8) "
         "ELSE jsonb_build_object('status',CASE WHEN n%20=0 THEN 'timeout' ELSE 'completed' END) END new_values FROM generate_series(1,100) n"),
    ]
    # Single-row event/status statements exercise the same indexes and JSONB shapes.
    for name, table, _, columns, trigger, seed in profiles[1:]:
        profiles.append((name.replace("batch", "single"), table, 1, columns, trigger, seed + " LIMIT 1"))
    writers = [Session(s, f"append_cost_writer_{i}") for i in range(4)]
    samples = []
    pairs = []
    catalogs = {}
    for profile, table, rows_per_tx, columns, trigger, seed in profiles:
        for writer in writers:
            writer.sql("SET statement_timeout='120s'; DROP TABLE IF EXISTS bench_input,bench_result; "
                       f"CREATE TEMP TABLE bench_input AS {seed}; "
                       "CREATE TEMP TABLE bench_result(times double precision[]);")
        catalogs[profile] = json.loads(s.sql(
            "SELECT jsonb_build_object('columns',(SELECT jsonb_agg(jsonb_build_array(attname,format_type(atttypid,atttypmod)) "
            f"ORDER BY attnum) FROM pg_attribute WHERE attrelid='{table}'::regclass AND attnum>0 AND NOT attisdropped),"
            f"'indexes',(SELECT jsonb_agg(indexdef ORDER BY indexname) FROM pg_indexes WHERE tablename='{table}'));"))
        profile_samples = []
        for sample in range(3):
            for tracked in ([False, True] if sample % 2 == 0 else [True, False]):
                s.sql(f"DROP TRIGGER IF EXISTS cost_tracking ON {table}; "
                      f"TRUNCATE {table},append_notice RESTART IDENTITY;")
                if tracked:
                    s.sql(f"CREATE TRIGGER cost_tracking AFTER INSERT ON {table} REFERENCING NEW TABLE AS new_rows "
                          f"FOR EACH STATEMENT EXECUTE FUNCTION {trigger}();")
                for writer in writers:
                    writer.sql("TRUNCATE bench_result;")
                barrier = threading.Barrier(5)
                def run_writer(writer):
                    barrier.wait(timeout=10)
                    writer.send(f"""
                    DO $$ DECLARE started timestamptz; durations double precision[] := '{{}}'; i integer;
                    BEGIN FOR i IN 1..100 LOOP
                      started := clock_timestamp();
                      INSERT INTO {table}({columns}) SELECT {columns} FROM bench_input;
                      COMMIT;
                      durations := array_append(durations,extract(epoch FROM clock_timestamp()-started)*1000);
                    END LOOP;
                    INSERT INTO bench_result VALUES(durations);
                    END $$;
                    SELECT array_to_json(times) FROM bench_result;
                    """)
                    return json.loads(writer.finish(timeout=120))
                with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                    futures = [pool.submit(run_writer, writer) for writer in writers]
                    started = time.monotonic()
                    barrier.wait(timeout=10)
                    times = [value for future in futures for value in future.result()]
                    elapsed = time.monotonic()-started
                rows = 400 * rows_per_tx
                require(s.sql(f"SELECT count(*) FROM {table};") == str(rows), "append benchmark cohort mismatch")
                notices = int(s.sql("SELECT count(*) FROM append_notice;"))
                expected = 800 if profile == "history-batch" else 400
                require(notices == (expected if tracked else 0), "statement dedup notification count wrong")
                data = {"profile": profile, "sample": sample, "tracking": "append" if tracked else "none",
                        "writers": 4, "transactions_each": 100, "rows_per_transaction": rows_per_tx,
                        "rows": rows, "notifications": notices, "wall_ms": elapsed*1000,
                        "rows_per_second": round(rows/elapsed, 3),
                        "transaction_server_p95_ms": sorted(times)[379],
                        "transaction_server_ms": times}
                samples.append(data)
                profile_samples.append(data)
            base = next(item for item in profile_samples if item["sample"] == sample and item["tracking"] == "none")
            treatment = next(item for item in profile_samples if item["sample"] == sample and item["tracking"] == "append")
            wall = (treatment["wall_ms"]/base["wall_ms"]-1)*100
            p95 = (treatment["transaction_server_p95_ms"]/base["transaction_server_p95_ms"]-1)*100
            pairs.append({"profile": profile, "sample": sample, "wall_overhead_pct": round(wall, 3),
                          "p95_overhead_pct": round(p95, 3), "strict_10pct_pass": wall <= 10 and p95 <= 10})
        if table == "cost_event":
            catalogs[profile]["payload_size"] = s.sql("SELECT round(avg(pg_column_size(payload)),1)||'|'||"
                                                      "round(avg(octet_length(payload::text)),1) FROM cost_event;")
    s.report["observations"]["append_four_writer_cost"] = samples
    s.report["observations"]["append_four_writer_pairs"] = pairs
    s.report["observations"]["append_strict_10pct_pass_all_pairs"] = all(pair["strict_10pct_pass"] for pair in pairs)
    s.report["observations"]["append_cost_source_catalogs"] = catalogs


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--versions", nargs="+", choices=["16", "18"], default=["16", "18"])
    parser.add_argument("--repeat", type=int, default=1, help="repeat full race probes with new owned servers")
    parser.add_argument("--output", type=Path, help="write complete JSON evidence, including failures")
    parser.add_argument("--brief", action="store_true", help="omit per-transaction arrays from stdout only")
    parser.add_argument("--fail-after-ready", action="store_true",
                        help="inject a setup failure to verify owned-resource cleanup; exits 1")
    parser.add_argument("--protocols-only", action="store_true", help="skip all cost samples")
    parser.add_argument("--append-only", action="store_true", help="run the append-log alternative without earlier UPSERT probes")
    parser.add_argument("--summary", action="store_true", help="print compact protocol and gate results; output file remains complete")
    args = parser.parse_args()
    require(args.repeat > 0, "--repeat must be positive")
    if args.output:
        require(args.output.parent.is_dir(), "output parent must already exist")
        require(not args.output.exists(), "refusing to overwrite existing evidence")
    run_id = uuid.uuid4().hex[:12]
    report = {"run_id": run_id, "utc_started": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
              "fixture": "isolated SQL protocol probes, not repository acceptance benchmarks",
              "runs": []}
    error = None
    def interrupted(signum, frame):
        raise KeyboardInterrupt(f"signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    try:
        for repetition in range(args.repeat):
            for version in args.versions:
                result = {"version": version, "repetition": repetition,
                          "rejections": {}, "observations": {}}
                report["runs"].append(result)
                with Server(f"postgres:{version}-alpine", f"{run_id}-{repetition}", result) as server:
                    if args.fail_after_ready:
                        raise RuntimeError("injected failure after server readiness")
                    # Retry only startup connectivity, never failed protocol assertions.
                    probes = []
                    if not args.append_only:
                        setup(server)
                        probes = [default_repair, lock_order, dirty_races, expiry_snapshot,
                                  conversion, row_locations_and_horizon, statement_markers]
                    if repetition % 2 and not args.append_only:
                        probes = [dirty_races, expiry_snapshot, conversion,
                                  row_locations_and_horizon, default_repair, lock_order, statement_markers]
                    if not args.protocols_only and not args.append_only:
                        probes.append(smoke_costs)
                    setup_append(server)
                    probes += ([append_builders_and_expiry, append_protocols] if repetition % 2
                               else [append_protocols, append_builders_and_expiry])
                    if not args.protocols_only:
                        probes.append(append_write_costs)
                    result["probe_order"] = [probe.__name__ for probe in probes]
                    for probe in probes:
                        probe(server)
                    result["passed"] = True
    except BaseException as failure:
        error = failure
        report["failure"] = f"{type(failure).__name__}: {failure}"
    finally:
        # Explicit cleanup precedes this observation; no janitor is involved.
        report["owned_container_leaks"] = command(
            "docker", "ps", "-a", "--filter", f"label={LABEL}", "--format", "{{.Names}}").stdout.splitlines()
        report["owned_volume_leaks"] = command(
            "docker", "volume", "ls", "--filter", f"label={LABEL}", "--format", "{{.Name}}").stdout.splitlines()
        # Only this invocation's token is a leak. Overlapping probes are allowed.
        for key in ("owned_container_leaks", "owned_volume_leaks"):
            report[key] = [name for name in report[key] if run_id in name]
        rendered = json.dumps(report, indent=2) + "\n"
        if args.output:
            with args.output.open("x") as output:
                output.write(rendered)
        if args.brief:
            brief = json.loads(rendered)
            for run in brief["runs"]:
                for sample in run["observations"].get("toy_four_writer_cost", []):
                    del sample["transaction_server_ms"]
                for sample in run["observations"].get("append_four_writer_cost", []):
                    del sample["transaction_server_ms"]
            rendered = json.dumps(brief, indent=2) + "\n"
        if args.summary:
            summary = {key: value for key, value in report.items() if key != "runs"}
            summary["runs"] = []
            for run in report["runs"]:
                compact = {key: run[key] for key in ("version", "repetition", "server_version", "passed",
                           "sessions_before_container_removal", "cleanup_errors") if key in run}
                compact["rejections"] = {key: value["sqlstate"] for key, value in run["rejections"].items()}
                compact["append_observations"] = {key: value for key, value in run["observations"].items()
                    if (key.startswith("append_") or key.startswith("rejected_append_"))
                    and key not in ("append_four_writer_cost", "append_cost_source_catalogs")}
                summary["runs"].append(compact)
            rendered = json.dumps(summary, indent=2) + "\n"
        print(rendered, end="")
    return 1 if error or report["owned_container_leaks"] or report["owned_volume_leaks"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
