#!/usr/bin/env python3
"""Probe actual producer DDL, restore collisions and system xmin on PG16/18."""

import argparse
import importlib.util
import json
from pathlib import Path
import re
import uuid

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("native_verify", ROOT / "scripts/verify-native-partitions.py")
verify = importlib.util.module_from_spec(spec)
spec.loader.exec_module(verify)
protocol = verify.protocol
require = protocol.require


def setup(server):
    migration = verify.native_sql(verify.SUMMARIES)
    log = re.search(r"CREATE TABLE native_summary_invalidation \([\s\S]+?^\);", migration, re.MULTILINE).group()
    trigger = re.search(r"CREATE FUNCTION native_summary_notify\(\)[\s\S]+?^END \$\$;", migration, re.MULTILINE).group()
    indexes = "\n".join(re.findall(r"^CREATE INDEX idx_native_summary_invalidation[^;]+;", migration, re.MULTILINE))
    # Use the actual table/trigger bytes with a small isolated source relation.
    server.sql("""
      CREATE SCHEMA attune; SET search_path TO attune,public;
      CREATE TYPE native_summary_kind AS ENUM ('execution_status','execution_creation','event_volume','worker_status');
      CREATE TABLE event(id BIGSERIAL,created timestamptz NOT NULL,trigger_ref text NOT NULL) PARTITION BY RANGE(created);
      CREATE TABLE event_default PARTITION OF event DEFAULT;
    """ + log + "\n" + indexes + "\n" + trigger + """
      CREATE TRIGGER native_summary_insert AFTER INSERT ON event REFERENCING NEW TABLE AS native_new
        FOR EACH STATEMENT EXECUTE FUNCTION native_summary_notify();
      CREATE TABLE probe_state(kind text PRIMARY KEY); INSERT INTO probe_state VALUES('event_volume');
    """)


def session(server, name):
    result = protocol.Session(server, name)
    result.sql("SET search_path TO attune,public;")
    return result


def origin_collision(server, report):
    h = "2021-10-02 00:00+00"
    producer = session(server, "restore_collision_producer")
    builder = session(server, "restore_collision_builder")
    producer.sql("BEGIN;")
    origin = producer.sql("SELECT pg_current_xact_id()::text;")
    actual = producer.sql("SELECT pg_current_xact_id()::xid::text;")
    imported = server.sql(f"SET search_path TO attune,public; INSERT INTO native_summary_invalidation(kind,bucket,transaction_origin) VALUES('event_volume','{h}','{origin}'::xid8) RETURNING id::text||':'||xmin::text;")
    imported_id, imported_xmin = imported.split(":")
    require(imported_xmin != actual, "import must have another transaction's actual xmin")
    builder.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY event IN ACCESS SHARE MODE; SELECT kind FROM probe_state FOR UPDATE;")
    captured = builder.sql(f"CREATE TEMP TABLE captured ON COMMIT DROP AS SELECT id FROM native_summary_invalidation WHERE bucket='{h}'; SELECT id FROM captured;")
    require(captured == imported_id, "builder captures the imported ID")
    require(builder.sql("SELECT count(*) FROM event;") == "0", "builder source snapshot precedes producer write")
    producer.sql(f"INSERT INTO event(created,trigger_ref) VALUES('{h}','restored-origin');")
    producer_ids = producer.sql(f"SELECT id FROM native_summary_invalidation WHERE bucket='{h}' ORDER BY id;")
    builder.sql("DELETE FROM native_summary_invalidation n USING captured c WHERE n.id=c.id; COMMIT;")
    producer.sql("COMMIT;")
    result = server.sql(f"SET search_path TO attune,public; SELECT count(*) FROM event; SELECT count(*) FROM native_summary_invalidation WHERE bucket='{h}';").splitlines()
    report["restored_origin_collision"] = {"origin": origin, "producer_xmin": actual, "imported_xmin": imported_xmin,
                                            "captured_id": imported_id, "producer_visible_ids": producer_ids.splitlines(),
                                            "raw_after": int(result[0]), "pending_after": int(result[1])}
    for s in [builder, producer]:
        s.close()


def savepoint_xmin(server, report):
    s = session(server, "savepoint_xmin")
    s.sql("BEGIN;")
    top = s.sql("SELECT pg_current_xact_id()::xid::text;")
    s.sql("SAVEPOINT child;")
    row_xmin = s.sql("INSERT INTO native_summary_invalidation(kind,bucket) VALUES('worker_status','2021-10-03') RETURNING xmin::text;")
    active = s.sql("SELECT transactionid::text FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='transactionid' AND mode='ExclusiveLock' ORDER BY transactionid::text;").splitlines()
    s.sql("RELEASE SAVEPOINT child;")
    released = s.sql("SELECT transactionid::text FROM pg_locks WHERE pid=pg_backend_pid() AND locktype='transactionid' AND mode='ExclusiveLock' ORDER BY transactionid::text;").splitlines()
    report["savepoint_xmin"] = {"top_xmin": top, "row_xmin": row_xmin,
                                "active_transaction_locks": active, "after_release_locks": released}
    s.sql("ROLLBACK;")
    s.close()


def xmin_privilege(server, report):
    server.sql("CREATE ROLE probe_xmin_reader; GRANT USAGE ON SCHEMA attune TO probe_xmin_reader;")
    grant = protocol.command(*server.psql(), input="SET search_path TO attune,public; GRANT SELECT(xmin) ON native_summary_invalidation TO probe_xmin_reader;", check=False)
    report["xmin_column_grant"] = {"exit_code":grant.returncode,"diagnostic":grant.stderr.strip()}
    column_read = protocol.command(*server.psql(), input="SET ROLE probe_xmin_reader; SET search_path TO attune,public; SELECT xmin::text FROM native_summary_invalidation;", check=False)
    report["xmin_column_grant"].update({"read_exit_code":column_read.returncode,"read_diagnostic":column_read.stderr.strip()})
    # Whether the server rejects a system-column grant or doesn't make it an
    # effective SELECT privilege, verify the actual metadata read requirement.
    server.sql("SET search_path TO attune,public; GRANT SELECT ON native_summary_invalidation TO probe_xmin_reader;")
    expected=server.sql("SET search_path TO attune,public; SELECT count(*) FROM native_summary_invalidation;")
    require(server.sql("SET ROLE probe_xmin_reader; SET search_path TO attune,public; SELECT count(xmin) FROM native_summary_invalidation;") == expected, "table SELECT permits system xmin inspection")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--versions", nargs="+", choices=["16","18"], default=["16","18"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--expect", choices=["lost","retained"], required=True)
    args = parser.parse_args()
    require(not args.output.exists(), "refusing to overwrite evidence")
    evidence = {"run_id":"origin-"+uuid.uuid4().hex[:12], "versions":[]}
    try:
        for version in args.versions:
            report = {"version":version,"rejections":{},"observations":{}}
            evidence["versions"].append(report)
            with protocol.Server(f"postgres:{version}-alpine", evidence["run_id"], report) as server:
                setup(server)
                origin_collision(server, report)
                savepoint_xmin(server, report)
                xmin_privilege(server, report)
                require(report["restored_origin_collision"]["pending_after"] == (0 if args.expect=="lost" else 1), "unexpected restored-origin protocol result")
                report["passed"] = True
    except BaseException as error:
        evidence["error"] = str(error)
        raise
    finally:
        args.output.write_text(json.dumps(evidence,indent=2)+"\n")
        print(json.dumps(evidence,indent=2))


if __name__ == "__main__":
    main()
