#!/usr/bin/env python3
"""Prove cache list-partition DDL and lock protocols on owned PostgreSQL.

This is an isolated SQL model, not a migration or production repository test.
It never connects to an existing database. Requires cached Docker images.
"""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import time
import uuid


ROOT = Path(__file__).resolve().parents[1]
PROTOCOL_PATH = ROOT / "scripts/probe-postgresql-native-maintenance.py"
spec = importlib.util.spec_from_file_location("owned_postgresql_protocol", PROTOCOL_PATH)
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)
require = protocol.require

CONTEXT = "SET ROLE cache_owner; SET search_path TO cache_probe, public; "


def identifier(value):
    return '"' + value.replace('"', '""') + '"'


def child_name(generation):
    require(type(generation) is int and 0 < generation <= 2**63 - 1,
            "generation must be a positive i64")
    return "cache_entry_g_" + str(generation)


def migration_function(filename, name):
    source = (ROOT / "migrations" / filename).read_text()
    match = re.search(
        r"CREATE OR REPLACE FUNCTION " + re.escape(name)
        + r"\(\).*?\$\$ LANGUAGE plpgsql;", source, re.S)
    require(match is not None, f"missing migration function {name}")
    return match.group(0)


def create_sql(generation):
    child_name(generation)
    return f"""
        INSERT INTO cache_generation(id, namespace) VALUES ({generation}, 1);
        SELECT create_cache_generation_partition({generation});
    """


def setup(server):
    server.sql("""
        CREATE ROLE cache_owner NOSUPERUSER NOCREATEDB NOCREATEROLE;
        CREATE ROLE cache_client NOSUPERUSER NOCREATEDB NOCREATEROLE;
        CREATE SCHEMA cache_probe AUTHORIZATION cache_owner;
    """)
    server.sql(CONTEXT + """
        CREATE TYPE owner_type_enum AS ENUM ('system');
        CREATE TYPE cache_generation_state_enum AS ENUM ('staging', 'active', 'retired', 'failed');
        CREATE TABLE cache_namespace (
            id bigint PRIMARY KEY, owner_type owner_type_enum NOT NULL,
            owner text NOT NULL, active_generation bigint, tombstoned_at timestamptz
        );
        INSERT INTO cache_namespace VALUES (1, 'system', 'system', NULL, NULL);
        CREATE TABLE cache_generation (
            id bigint PRIMARY KEY, namespace bigint NOT NULL REFERENCES cache_namespace,
            state cache_generation_state_enum NOT NULL DEFAULT 'staging',
            readable_until timestamptz, retired timestamptz
        );
        CREATE TABLE cache_generation_entry_usage (
            generation bigint PRIMARY KEY REFERENCES cache_generation ON DELETE RESTRICT,
            record_count bigint NOT NULL DEFAULT 0 CHECK (record_count >= 0),
            physical_bytes bigint NOT NULL DEFAULT 0 CHECK (physical_bytes >= 0)
        );
        CREATE TABLE cache_entry (
            id bigserial, generation bigint NOT NULL REFERENCES cache_generation ON DELETE RESTRICT,
            external_id text COLLATE "C" NOT NULL, value jsonb NOT NULL,
            source_updated_at timestamptz, source_checksum text,
            size_bytes bigint NOT NULL CHECK (size_bytes >= 0),
            created timestamptz NOT NULL DEFAULT NOW(), PRIMARY KEY (generation, id),
            UNIQUE (generation, external_id)
        ) PARTITION BY LIST (generation);
        CREATE TABLE cache_deployment_physical_byte_usage (
            id smallint PRIMARY KEY CHECK (id = 1), physical_bytes bigint NOT NULL CHECK (physical_bytes >= 0)
        );
        INSERT INTO cache_deployment_physical_byte_usage VALUES (1, 0);
        CREATE TABLE cache_owner_physical_byte_usage (
            owner_type owner_type_enum, owner text, physical_bytes bigint NOT NULL CHECK (physical_bytes >= 0),
            PRIMARY KEY (owner_type, owner)
        );
        CREATE TABLE workflow_execution (id bigint PRIMARY KEY, status text NOT NULL);
        INSERT INTO workflow_execution VALUES (1, 'running');
        CREATE TABLE workflow_cache_iteration (
            id bigint PRIMARY KEY, workflow_execution bigint REFERENCES workflow_execution ON DELETE CASCADE,
            generation bigint REFERENCES cache_generation ON DELETE CASCADE,
            state text NOT NULL DEFAULT 'scanning'
        );
        CREATE TABLE cache_ingest_chunk (
            generation bigint REFERENCES cache_generation ON DELETE RESTRICT
        );
        GRANT USAGE ON SCHEMA cache_probe TO cache_client;
        GRANT SELECT ON ALL TABLES IN SCHEMA cache_probe TO cache_client;
    """)
    server.sql(CONTEXT + migration_function("20250101000021_cache.sql", "account_cache_entry_size"))
    server.sql(CONTEXT + migration_function("20250101000021_cache.sql", "cache_entry_staging_only"))
    accounting = migration_function("20250101000024_cache_physical_byte_accounting.sql",
                                    "account_inserted_cache_entry_physical_bytes")
    server.sql(CONTEXT + accounting + """
        CREATE TRIGGER account_cache_entry_size_trigger BEFORE INSERT ON cache_entry
            FOR EACH ROW EXECUTE FUNCTION account_cache_entry_size();
        CREATE TRIGGER cache_entry_staging_only_trigger BEFORE INSERT OR UPDATE OR DELETE ON cache_entry
            FOR EACH ROW EXECUTE FUNCTION cache_entry_staging_only();
        CREATE TRIGGER account_inserted_cache_entry_physical_bytes_trigger AFTER INSERT ON cache_entry
            REFERENCING NEW TABLE AS inserted_cache_entries
            FOR EACH STATEMENT EXECUTE FUNCTION account_inserted_cache_entry_physical_bytes();
    """)
    lifecycle = (ROOT / "migrations/20261007000001_cache_generation_partitions.sql").read_text()
    lifecycle = "CREATE FUNCTION" + lifecycle.split("CREATE FUNCTION", 1)[1]
    server.sql(CONTEXT + lifecycle)


def usage(server):
    return server.sql(CONTEXT + """
        SELECT json_build_object(
            'deployment', (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id = 1),
            'owner', (SELECT physical_bytes FROM cache_owner_physical_byte_usage
                      WHERE owner_type = 'system' AND owner = 'system'),
            'generations', (SELECT json_agg(u ORDER BY generation) FROM cache_generation_entry_usage u)
        );
    """)


def drop_sql(generation):
    child_name(generation)
    return f"SELECT outcome, records_reclaimed, bytes_reclaimed FROM drop_cleanup_cache_generation({generation}, 0);"


def ddl_and_accounting(server, report):
    server.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE;"
               + create_sql(1) + create_sql(2) + "COMMIT;")
    server.sql(CONTEXT + "BEGIN;" + create_sql(3) + "ROLLBACK;")
    require(server.sql(CONTEXT + "SELECT to_regclass('cache_entry_g_3') IS NULL AND "
                       "NOT EXISTS(SELECT 1 FROM cache_generation WHERE id=3) AND "
                       "NOT EXISTS(SELECT 1 FROM cache_generation_entry_usage WHERE generation=3);") == "t",
            "creation rollback leaked storage or metadata")
    shape = server.sql(CONTEXT + """
        SELECT c.relname || ':' || pg_get_expr(c.relpartbound, c.oid)
        FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
        WHERE i.inhparent = 'cache_entry'::regclass ORDER BY c.relname;
    """).splitlines()
    require(shape == ["cache_entry_g_1:FOR VALUES IN ('1')", "cache_entry_g_2:FOR VALUES IN ('2')"],
            f"unexpected attached partitions: {shape}")
    report["observations"]["catalog_bounds"] = shape
    require(server.sql(CONTEXT + """
        SELECT COUNT(*) = 4 FROM pg_index WHERE indrelid IN
            ('cache_entry_g_1'::regclass, 'cache_entry_g_2'::regclass) AND indisunique;
    """) == "t", "attachment did not create composite and external-ID indexes")
    server.sql(CONTEXT + """
        BEGIN;
        LOCK TABLE ONLY cache_entry IN ROW EXCLUSIVE MODE;
        INSERT INTO cache_entry(id,generation,external_id,value,size_bytes)
        VALUES (10,1,'a','{"ok":true}',0), (11,1,'b','{"ok":true}',0), (10,2,'other','{"ok":false}',0);
        COMMIT;
    """)
    exact = server.sql(CONTEXT + """
        SELECT (SELECT SUM(physical_bytes) FROM cache_generation_entry_usage) = SUM(size_bytes)
            AND (SELECT SUM(record_count) FROM cache_generation_entry_usage) = COUNT(*)
            AND (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1) = SUM(size_bytes)
            AND (SELECT physical_bytes FROM cache_owner_physical_byte_usage
                 WHERE owner_type='system' AND owner='system') = SUM(size_bytes) FROM cache_entry;
    """)
    require(exact == "t", "transition trigger totals differ from admitted entries")
    original = usage(server)
    server.sql(CONTEXT + "BEGIN; INSERT INTO cache_entry(generation,external_id,value,size_bytes) "
               "VALUES (1,'rollback','{}',0); ROLLBACK;")
    require(usage(server) == original, "insert rollback changed usage")
    server.reject("sealed_insert", CONTEXT + "BEGIN; UPDATE cache_generation SET state='active' WHERE id=1; "
                  "INSERT INTO cache_entry(generation,external_id,value,size_bytes) VALUES(1,'no','{}',0);", "P0001")
    server.reject("immutable_update", CONTEXT + "UPDATE cache_entry SET value='{}' WHERE generation=1;", "P0001")
    server.reject("missing_partition", CONTEXT + "INSERT INTO cache_generation(id,namespace) VALUES (4,1); "
                  "INSERT INTO cache_entry(generation,external_id,value,size_bytes) VALUES(4,'no','{}',0);", "23514")
    server.reject("nonowner_drop", "SET ROLE cache_client; SET search_path TO cache_probe,public; "
                  "DROP TABLE cache_entry_g_1;", "42501")
    server.sql(CONTEXT + "UPDATE cache_generation SET state='retired', readable_until=NOW()-INTERVAL '1s', "
               "retired=NOW()-INTERVAL '1s' WHERE id IN (1,2);")
    server.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;"
               + drop_sql(1) + "ROLLBACK;")
    require(usage(server) == original, "drop rollback changed usage")
    require(server.sql(CONTEXT + "SELECT COUNT(*) FROM cache_entry WHERE generation=1;") == "2",
            "rollback failed to restore dropped entries")
    report["observations"]["owner_role"] = json.loads(server.sql(CONTEXT + """
        SELECT json_build_object('current_user',current_user,'superuser',rolsuper,
                                 'create_schema',has_schema_privilege(current_user,'cache_probe','CREATE'))
        FROM pg_roles WHERE rolname=current_user;
    """))
    report["observations"]["accounting_after_ingest"] = json.loads(original)
    report["observations"]["creation_insert_drop_rollback"] = "passed"


def repository_scan_queries():
    source = (ROOT / "crates/common/src/repositories/cache.rs").read_text()
    models = (ROOT / "crates/common/src/models.rs").read_text()
    columns = re.search(r'pub const CACHE_ENTRY_SELECT_COLUMNS: &str = "(.*?)";', models, re.S)
    require(columns is not None, "missing entry column constant")
    columns = columns.group(1).replace("\\\n", "")
    selected = ", ".join("e." + column.strip() for column in columns.split(","))
    queries = re.findall(
        r'"(WITH candidates AS MATERIALIZED .*?)",\s*qualified_columns\("e", CACHE_ENTRY_SELECT_COLUMNS\)',
        source, re.S)
    require(len(queries) == 2, "expected both repository bounded-scan queries")
    return [query.replace("\\\n", "").replace('\\"', '"').replace("{}", selected)
            for query in queries]


def repository_scans(server, report):
    observations = []
    for index, query in enumerate(repository_scan_queries()):
        session = protocol.Session(server, f"repository_scan_{index}")
        session.sql(CONTEXT + "SET plan_cache_mode=force_generic_plan; "
                    "PREPARE bounded_scan(bigint,text,bigint,bigint) AS " + query + ";")
        first = session.sql("EXECUTE bounded_scan(1,NULL,10,1);").splitlines()
        require(len(first) == 1 and first[0].split("|")[:3] == ["10", "1", "a"],
                f"bounded scan joined a different generation's ID: {first}")
        second = session.sql("EXECUTE bounded_scan(1,'a',10,1);").splitlines()
        require(len(second) == 1 and second[0].split("|")[:3] == ["11", "1", "b"],
                f"bounded scan continuation failed: {second}")
        plan = json.loads(session.sql("EXPLAIN (ANALYZE, FORMAT JSON) EXECUTE bounded_scan(1,NULL,10,1);"))
        visited = []

        def visit(node):
            if isinstance(node, dict):
                relation = node.get("Relation Name", "")
                if relation.startswith("cache_entry_g_") and node.get("Actual Loops", 0) > 0:
                    visited.append(relation)
                for value in node.values():
                    visit(value)
            elif isinstance(node, list):
                for value in node:
                    visit(value)

        visit(plan)
        require(visited and set(visited) == {"cache_entry_g_1"}, f"scan failed generation pruning: {visited}")
        observations.append({"query_sha256": hashlib.sha256(query.encode()).hexdigest(),
                             "visited_relations": visited, "plan": plan})
    report["observations"]["repository_bounded_scans"] = observations


def attach_with_reader(server, report):
    reader = protocol.Session(server, "attach_reader")
    reader.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE;")
    attacher = protocol.Session(server, "attacher")
    output = attacher.sql(CONTEXT + "BEGIN; SET LOCAL lock_timeout='500ms'; "
                          "LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE;" + create_sql(5))
    require("ERROR" not in output, f"attachment blocked an ordinary parent reader: {output}")
    locks = server.sql("""
        SELECT c.relname||':'||l.mode||':'||l.granted
        FROM pg_locks l JOIN pg_stat_activity a USING(pid) JOIN pg_class c ON c.oid=l.relation
        WHERE a.application_name='attacher' AND c.relname='cache_entry' ORDER BY l.mode;
    """).splitlines()
    require("cache_entry:ShareUpdateExclusiveLock:true" in locks,
            f"unexpected attach lock: {locks}")
    require("cache_entry:AccessExclusiveLock:true" not in locks, "attachment took exclusive parent lock")
    report["observations"]["attachment_parent_locks"] = locks
    attacher.sql("COMMIT;")
    reader.sql("COMMIT;")


def rejected_row_first_order(server, report):
    reader = protocol.Session(server, "row_first_reader")
    cleanup = protocol.Session(server, "row_first_cleanup")
    reader.sql(CONTEXT + "BEGIN; "
               "SELECT id FROM cache_generation WHERE id=1 FOR SHARE;")
    cleanup.send(CONTEXT + "BEGIN; "
                 "LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE; "
                 "SELECT id FROM cache_generation WHERE id=1 FOR UPDATE;")
    server.blocked("row_first_cleanup", "row_first_reader")
    reader.send("LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE;")
    outputs = []
    for session in (reader, cleanup):
        try:
            outputs.append(session.finish())
            session.sql("ROLLBACK;")
        except RuntimeError as error:
            outputs.append(str(error))
    require(any("40P01" in output for output in outputs), f"expected deadlock: {outputs}")
    report["rejections"]["generation_before_parent"] = {"sqlstate": "40P01", "outputs": outputs}


def rejected_iteration_first_order(server, report):
    server.sql(CONTEXT + "INSERT INTO workflow_cache_iteration(id,workflow_execution,generation,state) "
               "VALUES(9,1,2,'completed');")
    executor = protocol.Session(server, "iteration_first_executor")
    cleanup = protocol.Session(server, "iteration_first_cleanup")
    executor.sql(CONTEXT + "BEGIN; SELECT id FROM workflow_cache_iteration WHERE id=9 FOR UPDATE;")
    cleanup.send(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;"
                 + drop_sql(2))
    server.blocked("iteration_first_cleanup", "iteration_first_executor")
    executor.send("LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE;")
    outputs = []
    for session in (executor, cleanup):
        try:
            outputs.append(session.finish())
            session.sql("ROLLBACK;")
        except RuntimeError as error:
            outputs.append(str(error))
    require(any("40P01" in output for output in outputs), f"expected cascade deadlock: {outputs}")
    require(server.sql(CONTEXT + "SELECT COUNT(*) FROM cache_entry WHERE generation=2;") == "1",
            "rejected cascade protocol failed to restore generation storage")
    report["rejections"]["iteration_before_parent"] = {"sqlstate": "40P01", "outputs": outputs}


def parent_first_interleavings(server, report):
    reader = protocol.Session(server, "parent_first_reader")
    reader.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE; "
               "SELECT id FROM cache_generation WHERE id=1 FOR SHARE; "
               "SELECT id FROM workflow_execution WHERE id=1 FOR UPDATE; "
               "INSERT INTO workflow_cache_iteration(id,workflow_execution,generation) VALUES(1,1,1);")
    cleanup = protocol.Session(server, "parent_first_cleanup")
    cleanup.send(CONTEXT + "BEGIN; SET LOCAL lock_timeout='2s'; "
                 "LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;")
    server.blocked("parent_first_cleanup", "parent_first_reader")
    reader.sql("COMMIT;")
    require("ERROR" not in cleanup.finish(), "parent-first cleanup failed after reader commit")
    pinned = cleanup.sql("SELECT EXISTS(SELECT 1 FROM workflow_cache_iteration i JOIN workflow_execution w "
                         "ON w.id=i.workflow_execution WHERE i.generation=1 AND i.state='scanning' "
                         "AND w.status NOT IN ('completed','failed','cancelled','timeout','abandoned'));")
    require(pinned == "t", f"cleanup failed to see the committed durable pin: {pinned}")
    cleanup.sql("ROLLBACK;")

    # Cleanup first: the reader must wait on the parent before holding workflow,
    # iteration, or generation rows which cleanup's metadata cascades can touch.
    cleanup.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE; "
                "SELECT id FROM cache_generation WHERE id=2 FOR UPDATE;")
    reader.send(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE; "
                "SELECT id FROM workflow_execution WHERE id=1 FOR UPDATE; "
                "SELECT id FROM workflow_cache_iteration WHERE id=9 FOR UPDATE; "
                "SELECT id FROM cache_generation WHERE id=2 FOR SHARE;")
    server.blocked("parent_first_reader", "parent_first_cleanup")
    cleanup.sql(drop_sql(2) + "COMMIT;")
    require(reader.finish() == "1", "reader observed a reclaimed generation or failed to resume")
    reader.sql("COMMIT;")
    require(server.sql(CONTEXT + "SELECT to_regclass('cache_entry_g_2') IS NULL;") == "t",
            "committed drop left child storage")
    require(server.sql(CONTEXT + "SELECT NOT EXISTS(SELECT 1 FROM workflow_cache_iteration WHERE id=9);") == "t",
            "committed drop left terminal iteration metadata")
    require(server.sql(CONTEXT + "SELECT (SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1) "
                       "= SUM(size_bytes) FROM cache_entry;") == "t", "committed drop released the wrong bytes")

    # Bounded lock waits roll back without decrementing counters or dropping data.
    reader.sql(CONTEXT + "BEGIN; LOCK TABLE ONLY cache_entry IN ACCESS SHARE MODE;")
    before = usage(server)
    server.reject("bounded_drop_lock", CONTEXT + "BEGIN; SET LOCAL lock_timeout='100ms'; "
                  "LOCK TABLE ONLY cache_entry IN ACCESS EXCLUSIVE MODE;" + drop_sql(1), "55P03")
    reader.sql("COMMIT;")
    require(usage(server) == before, "lock deferral released usage")
    report["observations"]["reader_and_cleanup_orders"] = "passed"
    report["observations"]["accounting_after_drop"] = json.loads(usage(server))


def main():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="Examples:\n  python3 scripts/probe-cache-generation-partitions.py --versions 16 18 "
               "--output /tmp/opencode/cache-partition-protocols.json")
    parser.add_argument("--versions", type=int, nargs="+", choices=(16, 18), default=[16, 18])
    parser.add_argument("--output", type=Path, required=True, help="Write JSON evidence, including failures and cleanup")
    args = parser.parse_args()
    require(len(set(args.versions)) == len(args.versions), "duplicate versions")
    # Reserve evidence before provisioning. Never replace an earlier failed run.
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as output:
        report = {"run_id": "cache-partitions-" + uuid.uuid4().hex[:12], "scope": "isolated SQL model",
                  "source_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), "versions": []}
        inputs = [PROTOCOL_PATH, ROOT / "migrations/20250101000021_cache.sql",
                  ROOT / "migrations/20250101000024_cache_physical_byte_accounting.sql",
                  ROOT / "migrations/20261007000001_cache_generation_partitions.sql",
                  ROOT / "crates/common/src/repositories/cache.rs", ROOT / "crates/common/src/models.rs",
                  Path(__file__).resolve()]
        report["inputs_before"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                   for path in inputs}
        started = time.monotonic()
        try:
            for version in args.versions:
                item = {"version": version, "observations": {}, "rejections": {}}
                report["versions"].append(item)
                with protocol.Server(f"postgres:{version}-alpine", report["run_id"], item) as server:
                    setup(server)
                    ddl_and_accounting(server, item)
                    repository_scans(server, item)
                    attach_with_reader(server, item)
                    rejected_row_first_order(server, item)
                    rejected_iteration_first_order(server, item)
                    parent_first_interleavings(server, item)
                    item["passed"] = True
            report["passed"] = all(item.get("passed") and not item.get("cleanup_errors")
                                   for item in report["versions"])
        except BaseException as error:
            report["passed"] = False
            report["error"] = str(error)
            raise
        finally:
            label = "label=" + protocol.LABEL + "=" + report["run_id"]
            report["remaining_containers"] = protocol.command(
                "docker", "ps", "-a", "--filter", label, "--format", "{{.Names}}").stdout.splitlines()
            report["remaining_volumes"] = protocol.command(
                "docker", "volume", "ls", "--filter", label, "--format", "{{.Name}}").stdout.splitlines()
            report["inputs_after"] = {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest()
                                     for path in inputs}
            report["inputs_unchanged"] = report["inputs_before"] == report["inputs_after"]
            report["passed"] = (report.get("passed", False) and report["inputs_unchanged"]
                                and not report["remaining_containers"] and not report["remaining_volumes"])
            report["wall_seconds"] = time.monotonic() - started
            output.write(json.dumps(report, indent=2) + "\n")
            output.flush()
            print(json.dumps({"output": str(args.output), "passed": report["passed"], "error": report.get("error")}))
    require(report["passed"], "protocol validation or owned cleanup failed")


if __name__ == "__main__":
    main()
