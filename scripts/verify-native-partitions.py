#!/usr/bin/env python3
"""Verify fresh-schema native maintenance protocols on owned stock PG16/18 servers.

Uses the protocol runner's labeled-resource/session implementation. Each server
has a 1 GiB / 2 CPU limit and an ephemeral loopback port. No developer database is used.
Run: python3 scripts/verify-native-partitions.py --versions 16 18
"""

import argparse
import hashlib
import importlib.util
import json
import os
import re
import subprocess
import time
import uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("protocol", ROOT / "scripts/probe-postgresql-native-maintenance.py")
protocol = importlib.util.module_from_spec(spec)
spec.loader.exec_module(protocol)
original_command = protocol.command


def memory_bounded_command(*args, **kwargs):
    if args[:2] == ("docker", "run"):
        args = (*args[:2], "--memory=1g", "--cpus=2", *args[2:])
    return original_command(*args, **kwargs)


protocol.command = memory_bounded_command
require = protocol.require


def native_sql(name):
    return (ROOT / "migrations" / name).read_text()


PARTITIONS = "20261006000001_native_partitions.sql"
SUMMARIES = "20261006000002_hourly_summaries.sql"
CURSORS = "20261006000003_native_reconciliation_cursor.sql"
NATIVE_MIGRATIONS = (PARTITIONS, SUMMARIES, CURSORS)


def metadata_paths():
    return [path for path in sorted((ROOT / "migrations").glob("*.sql"))
            if path.name not in NATIVE_MIGRATIONS]


def metadata_schema(server):
    # The runner-claim migration requires a recognizable bootstrap marker.
    # This empty marker is only for the direct-DDL protocol fixture, not proof
    # of either migration runner's history/checksum behavior. The install
    # verifier owns those contracts.
    server.sql("CREATE TABLE public._migrations(filename TEXT PRIMARY KEY);")
    for path in metadata_paths():
        server.sql("BEGIN; SET search_path TO attune,public;\n" + path.read_text() + "\nCOMMIT;")
    require(server.sql("""SET search_path TO attune,public;
      SELECT string_agg(relname||':'||relkind::text,',' ORDER BY relname)
      FROM pg_class WHERE oid IN ('event'::regclass,'execution_history'::regclass,'audit_event'::regclass);
    """) == "audit_event:p,event:p,execution_history:p", "canonical metadata creates partitioned parents directly")
    require(server.sql("""SET search_path TO attune,public;
      SELECT count(*) FROM pg_inherits i JOIN pg_class c ON c.oid=i.inhrelid
      WHERE i.inhparent IN ('event'::regclass,'execution_history'::regclass,'audit_event'::regclass)
        AND pg_get_expr(c.relpartbound,c.oid)='DEFAULT';
    """) == "3", "canonical metadata creates all three DEFAULT children")


def apply_native_migrations(server, *, owner=True):
    for name in NATIVE_MIGRATIONS:
        try:
            # Installation DDL is setup, not a timed maintenance operation.
            # Match the clone fixture's bounded 120s DDL budget and leave room
            # for Docker transport. Lifecycle lock/deadline probes stay unchanged.
            role = "SET ROLE native_owner; " if owner else ""
            protocol.command(*server.psql(), input=role + "SET search_path TO attune,public; "
                             "SET statement_timeout='120s'; BEGIN;\n" + native_sql(name) + "\nCOMMIT;",
                             timeout=140)
        except Exception as error:
            diagnostics = {"migration": name, "error": str(error)}
            try:
                diagnostics["server_activity"] = json.loads(server.sql("""
                  SELECT coalesce(json_agg(json_build_object(
                    'application', application_name, 'state', state,
                    'wait_type', wait_event_type, 'wait_event', wait_event,
                    'blockers', pg_blocking_pids(pid), 'query', left(query, 3000))), '[]'::json)
                  FROM pg_stat_activity WHERE backend_type='client backend'
                    AND pid <> pg_backend_pid();"""))
            except Exception as diagnostic_error:
                diagnostics["diagnostic_error"] = str(diagnostic_error)
            server.report["native_migration_failure"] = diagnostics
            raise RuntimeError(f"native migration {name}: {error}") from error


def fresh_native_state(server, *, owner=True):
    execute = sql if owner else lambda connection, text: connection.sql("SET search_path TO attune,public; " + text)
    require(execute(server, "SELECT count(*) FROM native_partition_registry;") == "24", "fresh daily horizon")
    require(execute(server, "SELECT count(*) FROM native_maintenance_schedule;") == "3", "three durable job schedules")
    require(execute(server, "SELECT next_parent FROM native_partition_reconcile_state WHERE id=TRUE;") == "0", "fresh parent rotation cursor")
    require(execute(server, "SELECT count(*) FROM native_partition_reconcile_cursor WHERE last_day IS NULL;") == "3", "fresh per-parent repair cursors")
    require(execute(server, "SELECT count(*) FROM native_summary_state;") == "4", "four builder states")
    require(execute(server, "SELECT to_regprocedure('native_partition_next_day(native_partition_parent,timestamptz,timestamptz,timestamptz)') IS NOT NULL;") == "t", "cursor discovery function installed")


def sql(server, text):
    return server.sql("SET ROLE native_owner; SET search_path TO attune, public; " + text)


def database_ddl(server, statement):
    # Physical database clone/drop can wait for a shared checkpoint. Match the
    # fixture's existing 120s server DDL budget, with time for transport/teardown.
    # This does not change native partition lock/statement timeouts or retry DDL.
    return protocol.command(*server.psql(), input="SET statement_timeout='120s';\n" + statement,
                            timeout=140).stdout.strip()


def prepared_statement_lifecycle(server, report):
    # Prepared readers and cached trigger plans must remain coherent while a
    # fresh schema attaches and expires normal maintenance leaves.
    original_oid = sql(server, "SELECT 'event'::regclass::oid;")
    prepared = protocol.Session(server, "native_prepared")
    try:
        prepared.sql("SET ROLE native_owner; SET search_path TO attune,public;")
        require(prepared.sql("PREPARE saved_count AS SELECT count(*) FROM event WHERE trigger_ref='fresh.prepared'; EXECUTE saved_count;") == "0", "empty prepared reader")
        first_id = prepared.sql("INSERT INTO event(created,trigger_ref) VALUES('2021-05-01','fresh.prepared') RETURNING id;")
        require(first_id.isdigit(), "owned event ID")
        require(prepared.sql("EXECUTE saved_count;") == "1", "prepared reader sees DEFAULT insert")
        require(sql(server, "SELECT outcome FROM native_partition_ensure_day('event','2021-05-01',3);") == "applied", "prepared reader day attached")
        require(sql(server, "SELECT 'event'::regclass::oid;") == original_oid, "maintenance preserves parent identity")
        require(prepared.sql("EXECUTE saved_count;") == "1", "prepared reader sees repaired partition")
        second_id = prepared.sql("INSERT INTO event(trigger_ref) VALUES('fresh.prepared') RETURNING id;")
        require(int(second_id) > int(first_id), "partition routing uses the parent's sequence")
        require(sql(server, "SELECT pg_get_serial_sequence('event','id');").endswith("event_id_seq"), "event sequence remains parent-owned")
        execution_id = prepared.sql("INSERT INTO execution(action_ref) VALUES('fresh.cached') RETURNING id;")
        require(execution_id.isdigit(), "owned execution ID")
        prepared.sql(f"UPDATE execution SET status='running' WHERE id={execution_id};")
        prepared.sql(f"UPDATE execution SET status='completed',result='{{\"payload\":\"cached\"}}' WHERE id={execution_id};")
        require(sql(server, f"SELECT count(*) FROM execution_history WHERE entity_id={execution_id}; SELECT count(*) FROM audit_event WHERE resource_type='execution' AND resource_id={execution_id};") == "3\n3", "cached history/audit plans write exactly once per execution change")
        require(sql(server, f"SELECT new_values->'result'->>'type' FROM execution_history WHERE entity_id={execution_id} AND new_values->>'status'='completed';") == "object", "cached history plan retains the digest contract")
        require(server.sql("SET ROLE native_observer; SET search_path TO attune,public; SELECT count(*) FROM event WHERE trigger_ref='fresh.prepared'; SELECT sum(event_count) FROM event_volume_hourly WHERE trigger_ref='fresh.prepared';") == "2\n2", "ordinary table/view reader grants")
        sql(server, "SELECT setval('audit_event_id_seq',20000,true); SELECT outcome FROM native_partition_ensure_day('audit_event','2021-05-01',3);")
        require(sql(server, "INSERT INTO audit_event(created,category,event_type,outcome) VALUES('2021-05-01','auth','fresh.sequence','success') RETURNING id;") == "20001", "partition creation does not rewind reserved sequence positions")
        server.reject("outbound_fk", "SET ROLE native_owner; SET search_path TO attune,public; INSERT INTO event(trigger,trigger_ref) VALUES(-123456,'bad');", "23503")
        sql(server, "INSERT INTO execution_history(time,operation,entity_id,entity_ref,changed_fields,new_values) VALUES('2021-05-01','UPDATE',9000001,NULL,ARRAY['status'],'{}');")
        require(sql(server, "SELECT count(*) FROM execution_status_hourly WHERE action_ref IS NULL AND new_status IS NULL AND bucket='2021-05-01';") == "1", "nullable raw-view dimensions")
        require(sql(server, "SELECT native_partition_expire('event',id,'2021-05-02') FROM native_partition_registry WHERE parent='event' AND lower_bound='2021-05-01';") == "t", "prepared reader day expired")
        require(prepared.sql("EXECUTE saved_count;") == "1", "prepared reader rebinds after leaf expiry")
    finally:
        prepared.close()
    report["prepared_statement_lifecycle"] = "passed"


def lifecycle(server, report):
    require(sql(server, "SELECT count(*) FROM native_partition_registry WHERE lower_bound >= date_trunc('day',now(),'UTC') AND lower_bound <= date_trunc('day',now(),'UTC')+interval '7 days';") == "24", "today plus seven UTC days for all parents")
    sql(server, """
      INSERT INTO event(created,trigger_ref) SELECT '2021-06-01 04:00+00','repair' FROM generate_series(1,3);
      INSERT INTO event(created,trigger_ref) SELECT '2021-06-02 04:00+00','oversize' FROM generate_series(1,4);
    """)
    require(sql(server, "SELECT outcome||':'||rows_moved FROM native_partition_ensure_day('event','2021-06-02',3);") == "deferred_over_budget:4", "bounded oversized day")
    require(sql(server, "SELECT count(*) FROM ONLY event_default WHERE created='2021-06-02 04:00+00';") == "4", "oversized data stays visible")
    server.reject("repair_rollback", "SET ROLE native_owner; SET search_path TO attune,public; BEGIN; SELECT * FROM native_partition_ensure_day('event','2021-06-01',3); DO $$ BEGIN RAISE EXCEPTION 'cancel'; END $$; COMMIT;", "P0001")
    require(sql(server, "SELECT count(*) FROM ONLY event_default WHERE created='2021-06-01 04:00+00';") == "3", "repair rollback restores DEFAULT")
    require(sql(server, "SELECT to_regclass('event_p20210601') IS NULL;") == "t", "rollback removes staging")
    # A lost client after successful in-transaction repair must also roll back.
    crashed = protocol.Session(server, "native_crash_before_commit")
    require(crashed.sql("SET ROLE native_owner; SET search_path TO attune,public; BEGIN; SELECT outcome FROM native_partition_ensure_day('event','2021-06-01',3);") == "applied", "repair before lost client")
    crashed.close()
    require(sql(server, "SELECT count(*) FROM ONLY event_default WHERE created='2021-06-01 04:00+00';") == "3", "lost client restores DEFAULT")
    require(sql(server, "SELECT to_regclass('event_p20210601') IS NULL;") == "t", "lost client removes staging")
    sql(server, "CREATE FUNCTION fixture_slow_repair() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_sleep(1); RETURN OLD; END $$; CREATE TRIGGER fixture_slow_repair BEFORE DELETE ON event_default FOR EACH ROW EXECUTE FUNCTION fixture_slow_repair();")
    server.reject("repair_deadline", "SET ROLE native_owner; SET search_path TO attune,public; BEGIN; SET LOCAL statement_timeout='150ms'; SELECT * FROM native_partition_ensure_day('event','2021-06-01',3); COMMIT;", "57014")
    sql(server, "DROP TRIGGER fixture_slow_repair ON event_default; DROP FUNCTION fixture_slow_repair();")
    require(sql(server, "SELECT count(*) FROM ONLY event_default WHERE created='2021-06-01 04:00+00';") == "3", "deadline retains source visibility")
    require(sql(server, "SELECT to_regclass('event_p20210601') IS NULL;") == "t", "deadline removes destination")
    require(sql(server, "SELECT outcome||':'||rows_moved FROM native_partition_ensure_day('event','2021-06-01',3);") == "applied:3", "whole-day repair")
    require(sql(server, "SELECT outcome FROM native_partition_ensure_day('event','2021-06-01',3);") == "already_present", "idempotent day creation")
    require(sql(server, "SELECT count(*) FROM event WHERE created='2021-06-01 04:00+00';") == "3", "repair parent visibility")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2021-06-01 04:00+00';") == "2", "statement and physical-move notices dedup")
    reader = protocol.Session(server, "native_lock_reader")
    reader.sql("SET ROLE native_owner; SET search_path TO attune,public; BEGIN; LOCK TABLE ONLY event IN ACCESS SHARE MODE;")
    server.reject("bounded_reader_lock", "SET ROLE native_owner; SET search_path TO attune,public; BEGIN; SET LOCAL lock_timeout='40ms'; SET LOCAL statement_timeout='250ms'; SELECT * FROM native_partition_ensure_day('event','2021-06-03',3); COMMIT;", "55P03")
    reader.sql("COMMIT;")
    reader.close()
    sql(server, """
      INSERT INTO event_volume_hourly_summary VALUES('2021-06-01 04:00+00','repair',3);
      INSERT INTO native_summary_hour VALUES('event_volume','2021-06-01 04:00+00',now());
    """)
    expire = "SELECT native_partition_expire('event',id,'2021-06-02') FROM native_partition_registry WHERE parent='event' AND lower_bound='2021-06-01';"
    server.reject("expiry_rollback", "SET ROLE native_owner; SET search_path TO attune,public; BEGIN; " + expire + " DO $$ BEGIN RAISE EXCEPTION 'crash'; END $$; COMMIT;", "P0001")
    require(sql(server, "SELECT count(*) FROM event_volume_hourly_summary;") == "1", "failed drop restores summary")
    require(sql(server, expire) == "t", "whole partition expired")
    require(sql(server, "SELECT count(*) FROM event_volume_hourly_summary; SELECT count(*) FROM native_summary_hour; SELECT count(*) FROM native_summary_invalidation WHERE bucket='2021-06-01 04:00+00';") == "0\n0\n0", "drop purges summaries, coverage and notifications atomically")
    sql(server, "INSERT INTO event(created,trigger_ref) VALUES('2021-06-01 04:00+00','late');")
    require(sql(server, "SELECT count(*) FROM ONLY event_default WHERE created='2021-06-01 04:00+00';") == "1", "late writer routes through DEFAULT after expiry")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation WHERE bucket='2021-06-01 04:00+00';") == "1", "late write makes new notification")
    sql(server, "CREATE TABLE event_p20210603(LIKE event INCLUDING ALL);")
    server.reject("unregistered_relation", "SET ROLE native_owner; SET search_path TO attune,public; SELECT * FROM native_partition_ensure_day('event','2021-06-03',3);", "55000")
    sql(server, "DROP TABLE event_p20210603;")
    server.reject("non_owner_ddl", "SET ROLE native_observer; SET search_path TO attune,public; SELECT * FROM native_partition_ensure_day('event','2021-06-03',3);", "42501")
    # INSERT/UPDATE/DELETE use statement transition tables, old+new predicates,
    # and one record per distinct kind/hour rather than one per row.
    sql(server, "DELETE FROM native_summary_invalidation; INSERT INTO execution_history(time,operation,entity_id,changed_fields,new_values) SELECT '2021-06-04 04:30+00','INSERT',42,ARRAY['status'],'{}'::jsonb FROM generate_series(1,3);")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "2", "history INSERT kind/hour dedup")
    sql(server, "UPDATE execution_history SET time='2021-06-04 05:30+00',changed_fields='{}',operation='UPDATE' WHERE entity_id=42;")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "4", "removed history predicates invalidate old buckets")
    sql(server, "UPDATE execution_history SET changed_fields=ARRAY['status'] WHERE entity_id=42; DELETE FROM execution_history WHERE entity_id=42;")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "6", "history UPDATE/DELETE invalidation")
    require(sql(server, "SELECT count(*) FROM pg_constraint WHERE conrelid='native_summary_invalidation'::regclass AND contype='f';") == "0", "producers have no state FK")
    require(sql(server, "SELECT native_partition_backlog_day('event') IS NOT NULL;") == "t", "catalog reconciliation and bounded backlog lookup")
    require(sql(server, "SELECT default_rows_at_least||':'||default_count_exact FROM native_partition_status('event',date_trunc('day',now(),'UTC'),7,3);") == "4:false", "bounded DEFAULT status lower bound")
    # Exercise the repository's parent-directed TID join on overlapping ctid
    # values. Explicit day ranges make full old-leaf pruning possible.
    sql(server, """
      SELECT * FROM native_partition_ensure_day('execution_history','2021-07-01',3);
      SELECT * FROM native_partition_ensure_day('execution_history','2021-07-02',3);
      INSERT INTO execution_history(time,operation,entity_id,changed_fields) VALUES
        ('2021-07-01 04:00+00','UPDATE',54321,ARRAY['status']),
        ('2021-07-02 04:00+00','UPDATE',54321,ARRAY['status']);
    """)
    require(sql(server, "SELECT count(DISTINCT ctid) FROM execution_history WHERE entity_id=54321;") == "1", "fixture has colliding leaf TIDs")
    require(sql(server, """
      WITH doomed AS MATERIALIZED (
        SELECT tableoid,ctid FROM execution_history WHERE entity_id=54321 ORDER BY time,tableoid,ctid LIMIT 1 FOR UPDATE SKIP LOCKED
      ), deleted AS (
        DELETE FROM execution_history source USING doomed
        WHERE source.tableoid=doomed.tableoid AND source.ctid=doomed.ctid RETURNING 1
      ) SELECT count(*) FROM deleted;
    """) == "1", "matched tableoid/TID deletes one source row")
    require(sql(server, "SELECT count(*) FROM execution_history WHERE entity_id=54321;") == "1", "retained overlapping TID survives")
    # Execute the repository's SQL string, not a separately maintained oracle.
    retention_source = (ROOT / "crates/common/src/repositories/retention.rs").read_text()
    function = retention_source.split("fn delete_native_boundary_sql", 1)[1].split("fn delete_executions_sql", 1)[0]
    match = re.search(r'format!\(\s*"([^"]+)"', function)
    require(match is not None, "production boundary SQL extraction")
    boundary_sql = match.group(1).format(table="event", fallback="event_default", col="created")
    sql(server, """
      DELETE FROM event;
      SELECT * FROM native_partition_ensure_day('event','2021-08-01',3);
      SELECT * FROM native_partition_ensure_day('event','2021-08-03',3);
      INSERT INTO event(created,trigger_ref) SELECT '2021-08-01','leftover_leaf' FROM generate_series(1,100);
      INSERT INTO event(created,trigger_ref) VALUES
        ('2021-07-30','old_default'),('2021-08-03 11:59:59.999999+00','old_boundary'),
        ('2021-08-03 12:00+00','exact_cutoff'),('2021-08-03 12:00:00.000001+00','retained');
      DELETE FROM native_summary_invalidation;
    """)
    require(sql(server, "PREPARE boundary_expiry(timestamptz,bigint) AS " + boundary_sql + "; EXECUTE boundary_expiry('2021-08-03 12:00+00',2);") == "2", "actual repository boundary/default SQL deletes bounded cohort")
    require(sql(server, "SELECT count(*) FROM event WHERE trigger_ref='leftover_leaf'; SELECT count(*) FROM event WHERE trigger_ref IN ('exact_cutoff','retained'); SELECT count(*) FROM native_summary_invalidation;") == "100\n2\n2", "unselected full leaf/exact cutoff survive and changed hours invalidate once")
    report["production_boundary_sql"] = "passed"
    report["lifecycle"] = "passed"


def fresh_install_and_clone_scope(server, report):
    # A second fresh database exercises database-local lock observations. This
    # does not simulate an upgrade or either real migration-runner history.
    database_ddl(server, "CREATE DATABASE native_fresh;")
    original_psql = server.psql
    def fresh_psql(name="probe"):
        args = original_psql(name)
        args[args.index("-d") + 1] = "native_fresh"
        return args
    server.psql = fresh_psql
    try:
        metadata_schema(server)
        apply_native_migrations(server, owner=False)
        fresh_native_state(server, owner=False)
        report["fresh_migrations"] = "passed"
        server.psql = original_psql
        database_ddl(server, "CREATE DATABASE native_lock_neighbor TEMPLATE native_fresh;")
        neighbor = None
        observer = None
        try:
            def neighbor_psql(name="probe"):
                args = original_psql(name)
                args[args.index("-d") + 1] = "native_lock_neighbor"
                return args
            server.psql = neighbor_psql
            neighbor = protocol.Session(server, "native_clone_lock_neighbor")
            neighbor.sql("SET search_path TO attune,public; BEGIN; LOCK TABLE ONLY event IN ACCESS EXCLUSIVE MODE;")
            server.psql = fresh_psql
            observer = protocol.Session(server, "native_clone_lock_observer")
            observer.sql("SET search_path TO attune,public;")
            unscoped = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='event'::regclass AND mode='AccessExclusiveLock' AND granted AND pid <> pg_backend_pid());"
            scoped = "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE relation='event'::regclass AND database=(SELECT oid FROM pg_database WHERE datname=current_database()) AND mode='AccessExclusiveLock' AND granted AND pid <> pg_backend_pid());"
            require(observer.sql(unscoped) == "t", "unscoped readiness falsely sees neighboring clone lock")
            require(observer.sql(scoped) == "f", "database-scoped readiness excludes neighboring clone lock")
            report["clone_lock_scope_control"] = {"unscoped_false_positive": True, "scoped_false_positive": False}
        finally:
            if observer:
                observer.close()
            if neighbor:
                neighbor.close()
            server.psql = original_psql
            database_ddl(server, "DROP DATABASE native_lock_neighbor;")
    finally:
        server.psql = original_psql
        database_ddl(server, "DROP DATABASE native_fresh;")


def producer_coalescing(server, report):
    # Real execution DML invokes record_execution_history once for each row.
    # Coalescing must affect notifications, never raw history/audit records.
    sql(server, "DELETE FROM native_summary_invalidation;")
    writer = protocol.Session(server, "native_coalesced_execution_writer")
    writer.sql("SET ROLE native_owner; SET search_path TO attune,public; BEGIN;")
    before = int(writer.sql("SELECT last_value FROM native_summary_invalidation_id_seq;"))
    writer.sql("INSERT INTO execution(action_ref) SELECT 'coalesce.execution' FROM generate_series(1,32);")
    writer.sql("UPDATE execution SET status='running' WHERE action_ref='coalesce.execution';")
    writer.sql("UPDATE execution SET status='completed' WHERE action_ref='coalesce.execution';")
    require(writer.sql("SELECT count(*) FROM execution_history WHERE entity_ref='coalesce.execution';") == "96", "execution history row records preserved")
    require(writer.sql("SELECT kind::text||':'||count(*) FROM native_summary_invalidation GROUP BY kind ORDER BY native_summary_invalidation.kind;") == "execution_status:1\nexecution_creation:1", "row history INSERT statements coalesce by kind/hour/transaction")
    require(writer.sql("SELECT bool_and(transaction_origin=pg_current_xact_id()) FROM native_summary_invalidation;") == "t", "top-level xid8 origin")
    sequence_delta = int(writer.sql("SELECT last_value FROM native_summary_invalidation_id_seq;")) - before
    require(sequence_delta == 2, "own duplicates are filtered before sequence defaults")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "0", "entire producer is invisible until commit")
    writer.sql("COMMIT;")
    writer.close()
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "2", "transaction publishes exactly two execution-kind records")
    report["producer_coalescing"] = {"execution_rows": 32, "history_rows": 96, "notifications": 2,
                                       "sequence_values_consumed": sequence_delta}

    sql(server, "DELETE FROM native_summary_invalidation;")
    h = "2021-10-02 00:00+00"
    mixed = protocol.Session(server, "native_coalesced_corrections")
    mixed.sql("SET ROLE native_owner; SET search_path TO attune,public; BEGIN;")
    mixed.sql(f"INSERT INTO execution_history(time,operation,entity_id,changed_fields,new_values) VALUES('{h}','INSERT',98765,ARRAY['status'],'{{}}');")
    mixed.sql("UPDATE execution_history SET time='2021-10-02 01:00+00',entity_ref='corrected' WHERE entity_id=98765;")
    mixed.sql("UPDATE execution_history SET entity_ref=NULL WHERE entity_id=98765; DELETE FROM execution_history WHERE entity_id=98765;")
    mixed.sql(f"INSERT INTO worker_history(time,operation,entity_id,changed_fields,new_values) VALUES('{h}','UPDATE',98765,ARRAY['status'],'{{}}');")
    mixed.sql("UPDATE worker_history SET time='2021-10-02 01:00+00' WHERE entity_id=98765; DELETE FROM worker_history WHERE entity_id=98765;")
    require(mixed.sql("SELECT kind::text||':'||count(*) FROM native_summary_invalidation GROUP BY kind ORDER BY native_summary_invalidation.kind;") == "execution_status:2\nexecution_creation:2\nworker_status:2", "corrections include old/new hours and each relevant kind")
    mixed.sql("ROLLBACK;")
    mixed.close()
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "0", "producer rollback removes all coalesced markers")

    # Reserve a lower notification ID in an uncommitted producer. A second
    # origin commits first while the first remains active. The builder then
    # holds state while the late producer changes its source again and commits.
    late = protocol.Session(server, "native_coalesced_late")
    early = protocol.Session(server, "native_coalesced_early")
    builder = protocol.Session(server, "native_coalesced_builder")
    for session in [late, early, builder]:
        session.sql("SET ROLE native_owner; SET search_path TO attune,public;")
    late.sql("BEGIN; SET LOCAL statement_timeout='1s';")
    for _ in range(2):
        late.sql(f"INSERT INTO event(created,trigger_ref) VALUES('{h}','coalesce.late');")
    low = int(late.sql(f"SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}';"))
    origin = late.sql("SELECT pg_current_xact_id()::text;")
    early.sql("BEGIN; SET LOCAL statement_timeout='1s';")
    for _ in range(2):
        early.sql(f"INSERT INTO event(created,trigger_ref) VALUES('{h}','coalesce.early');")
    high = int(early.sql(f"SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}' AND transaction_origin=pg_current_xact_id();"))
    require(early.sql("SELECT pg_current_xact_id()::text;") != origin, "concurrent producers use different origins")
    require(high > low, "sequence order differs from commit order")
    early.sql("COMMIT;")
    builder.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY event IN ACCESS SHARE MODE; SELECT kind FROM native_summary_state WHERE kind='event_volume' FOR UPDATE; UPDATE native_summary_state SET updated=clock_timestamp() WHERE kind='event_volume';")
    require(builder.sql(f"CREATE TEMP TABLE captured_ids ON COMMIT DROP AS SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}' ORDER BY id LIMIT 100; SELECT id FROM captured_ids;") == str(high), "capture exact visible ID only")
    builder.sql(f"DELETE FROM event_volume_hourly_summary WHERE bucket='{h}'; INSERT INTO event_volume_hourly_summary SELECT '{h}'::timestamptz,trigger_ref,count(*) FROM event WHERE created>='{h}' AND created<'2021-10-02 01:00+00' GROUP BY trigger_ref; INSERT INTO native_summary_hour VALUES('event_volume','{h}',clock_timestamp());")
    late.sql(f"INSERT INTO event(created,trigger_ref) VALUES('{h}','coalesce.late'); COMMIT;")
    require(builder.sql("DELETE FROM native_summary_invalidation n USING captured_ids c WHERE n.id=c.id RETURNING n.id;") == str(high), "ack only captured IDs despite late commit")
    builder.sql("COMMIT;")
    require(sql(server, f"SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}';") == str(low), "lower late ID survives with the entire late transaction")
    require(sql(server, f"SELECT sum(event_count) FROM event_volume_hourly_summary WHERE bucket='{h}'; SELECT count(*) FROM event WHERE created='{h}';") == "2\n5", "dirty marker protects stale pre-commit summary")
    builder.sql("BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY event IN ACCESS SHARE MODE; SELECT kind FROM native_summary_state WHERE kind='event_volume' FOR UPDATE; UPDATE native_summary_state SET updated=clock_timestamp() WHERE kind='event_volume';")
    builder.sql(f"CREATE TEMP TABLE captured_ids ON COMMIT DROP AS SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}' ORDER BY id LIMIT 100; DELETE FROM event_volume_hourly_summary WHERE bucket='{h}'; INSERT INTO event_volume_hourly_summary SELECT '{h}'::timestamptz,trigger_ref,count(*) FROM event WHERE created>='{h}' AND created<'2021-10-02 01:00+00' GROUP BY trigger_ref; DELETE FROM native_summary_invalidation n USING captured_ids c WHERE n.id=c.id; COMMIT;")
    require(sql(server, f"SELECT sum(event_count) FROM event_volume_hourly_summary WHERE bucket='{h}'; SELECT count(*) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='{h}';") == "5\n0", "subsequent rebuild catches up without lost invalidation")
    for session in [builder, early, late]:
        session.close()
    report["producer_coalescing"].update({"lower_id": low, "higher_id": high, "builder_state_did_not_block_producer": True,
                                         "initial_summary": 2, "retained_raw": 5, "final_summary": 5,
                                         "final_pending": 0, "correction_and_rollback": "passed"})


def fresh_producer_schema(server, report):
    # Transfer every parent and child explicitly. ALTER TABLE transfers its
    # owned serial sequences; the second loop handles any standalone sequences.
    server.sql("""
      CREATE ROLE native_owner; CREATE ROLE native_observer;
      ALTER SCHEMA attune OWNER TO native_owner;
      GRANT USAGE ON SCHEMA attune TO native_observer;
      SET search_path TO attune,public;
      DO $$ DECLARE r RECORD; BEGIN
        FOR r IN SELECT relname,relkind FROM pg_class
          WHERE relnamespace='attune'::regnamespace AND relkind IN ('p','r','v')
          ORDER BY CASE relkind WHEN 'p' THEN 0 WHEN 'r' THEN 1 ELSE 2 END,relname LOOP
          EXECUTE format('ALTER %s %I OWNER TO native_owner',CASE r.relkind WHEN 'v' THEN 'VIEW' ELSE 'TABLE' END,r.relname);
        END LOOP;
        FOR r IN SELECT relname FROM pg_class
          WHERE relnamespace='attune'::regnamespace AND relkind='S'
            AND relowner <> 'native_owner'::regrole ORDER BY relname LOOP
          EXECUTE format('ALTER SEQUENCE %I OWNER TO native_owner',r.relname);
        END LOOP;
      END $$;
      SET ROLE native_owner;
      GRANT SELECT ON event, execution_history, audit_event, event_volume_hourly TO native_observer;
      GRANT UPDATE(payload) ON event TO native_observer;
    """)
    apply_native_migrations(server)
    fresh_native_state(server)
    require(server.sql("SELECT NOT rolsuper FROM pg_roles WHERE rolname='native_owner';") == "t", "maintenance owner is not superuser")
    require(sql(server, """SELECT count(*) FROM pg_class
      WHERE relnamespace=current_schema()::regnamespace AND relkind IN ('p','r','v','S')
        AND relowner <> 'native_owner'::regrole;""") == "0", "parent, child, view and sequence ownership")
    require(sql(server, """SELECT count(*) FROM pg_proc
      WHERE pronamespace=current_schema()::regnamespace AND proname LIKE 'native_%'
        AND (prosecdef OR proowner <> 'native_owner'::regrole);""") == "0", "native functions are owner-installed invokers")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation;") == "0", "fresh install has no producer notifications")
    sql(server,"INSERT INTO event(trigger_ref) VALUES('producer.permissions');")
    server.sql("SET ROLE native_observer; SET search_path TO attune,public; UPDATE event SET payload='{}'::jsonb WHERE trigger_ref='producer.permissions';")
    require(sql(server, "SELECT has_column_privilege('native_observer','event','payload','UPDATE');") == "t", "column-only writer grant")
    require(sql(server, "SELECT count(*) FROM native_summary_invalidation WHERE kind='event_volume';") == "2", "column-only writer appends its invalidation")
    require(sql(server, "SELECT has_table_privilege('native_observer','native_summary_state','SELECT');") == "f", "producer has no builder-state privilege")
    report["fresh_producer_schema"] = "passed"


def rust_tests(server, report, output_path, selected_suites):
    database_ddl(server, "CREATE DATABASE attune_native_test;")
    port = report["port"].rsplit(":", 1)[1]
    env = os.environ.copy()
    env["ATTUNE__DATABASE__URL"] = f"postgresql://postgres@127.0.0.1:{port}/attune_native_test"
    env["SQLX_OFFLINE"] = "true"
    commands = [
        ["cargo", "test", "-p", "attune-common", "--test", "migration_tests", "--", "--nocapture", "--test-threads=4"],
        ["cargo", "test", "-p", "attune-common", "--test", "native_partition_repository_tests", "--", "--nocapture", "--test-threads=4"],
        ["cargo", "test", "-p", "attune-common", "--test", "native_producer_repository_tests", "--", "--nocapture", "--test-threads=4"],
        ["cargo", "test", "-p", "attune-common", "--test", "test_database_lifecycle_tests", "--", "--nocapture", "--test-threads=4"],
        ["cargo", "test", "-p", "attune-common", "--lib", "repositories::retention::", "--", "--nocapture", "--test-threads=4"],
    ]
    report["rust_tests"] = []
    for command in commands:
        suite = command[command.index("--test") + 1] if "--test" in command else "retention"
        if selected_suites and suite not in selected_suites:
            continue
        env["ATTUNE_TEST_RUN_ID"] = "np" + uuid.uuid4().hex[:10]
        started = time.monotonic()
        result = subprocess.run(command, cwd=ROOT, env=env, capture_output=True, text=True, timeout=2400)
        identities = sorted(re.findall(r"^test (\S+) \.\.\. (ok|FAILED|ignored)$", result.stdout, re.MULTILINE))
        summaries = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;", result.stdout)
        run = {"command": command, "suite": suite, "exit_code": result.returncode,
               "run_id": env["ATTUNE_TEST_RUN_ID"], "seconds": time.monotonic()-started,
               "identities": identities,
               "identity_sha256": hashlib.sha256("\n".join(name for name, _ in identities).encode()).hexdigest(),
               "summaries": summaries, "stdout": result.stdout, "stderr": result.stderr}
        report["rust_tests"].append(run)
        if output_path:
            for stream in ["stdout", "stderr"]:
                path = output_path.with_name(f"{output_path.stem}.pg{report['version']}.{suite}.{stream}.log")
                require(not path.exists(), f"refusing to overwrite {path}")
                path.write_text(run[stream])
                run[f"{stream}_path"] = str(path)
        for line in result.stdout.splitlines():
            if line.startswith("test result:"):
                print(f"PG{report['version']} {env['ATTUNE_TEST_RUN_ID']}: {line}", flush=True)
    require(server.sql("SELECT count(*) FROM pg_database WHERE datname LIKE 'attune_db_%' OR datname LIKE 'attune_migration_%';") == "0", "no test clone leaks")
    report["rust_test_clone_leaks"] = 0
    server.wait("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE backend_type='client backend' AND application_name <> 'probe');", "Rust test client sessions closed")
    report["rust_test_client_session_leaks"] = 0
    for run in report["rust_tests"]:
        if run["exit_code"] == 0:
            require(len(run["summaries"]) == 1, f"missing unique libtest summary: {run['suite']}")
            summary = run["summaries"][0]
            require(len(run["identities"]) == sum(int(value) for value in summary[1:4]), f"incomplete test identity capture: {run['suite']}")
    return all(run["exit_code"] == 0 for run in report["rust_tests"])


def close_protocol_sessions(server, report):
    errors = []
    for session in reversed(server.sessions):
        try:
            session.close()
            require(session.process.poll() is not None, f"client process remains: {session.name}")
            require(not session.reader.is_alive(), f"client log reader remains: {session.name}")
        except Exception as error:
            errors.append(str(error))
    report["session_cleanup_errors"] = errors
    require(not errors, "; ".join(errors))
    server.wait("SELECT NOT EXISTS(SELECT 1 FROM pg_stat_activity WHERE backend_type='client backend' AND application_name <> 'probe');",
                "all verifier client sessions closed")
    report["verifier_client_session_leaks"] = 0


def resource_inventory(run_id):
    commands = {
        "containers": ("docker", "ps", "-aq"),
        "volumes": ("docker", "volume", "ls", "-q"),
    }
    return {kind: protocol.command(*command, "--filter", f"label={protocol.LABEL}={run_id}").stdout.splitlines()
            for kind, command in commands.items()}


def migration_fingerprints():
    paths = sorted([*metadata_paths(), *(ROOT / "migrations" / name for name in NATIVE_MIGRATIONS)])
    return {path.name: hashlib.sha384(path.read_bytes()).hexdigest() for path in paths}


def write_evidence(path, evidence):
    if path:
        with path.open("x", encoding="utf-8") as output:
            output.write(json.dumps(evidence, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""Examples:
  python3 scripts/verify-native-partitions.py --dry-run
  python3 scripts/verify-native-partitions.py --versions 16 18 --brief --output /tmp/opencode/native-protocols.json
  python3 scripts/verify-native-partitions.py --versions 16 --producer-only --brief
  python3 scripts/verify-native-partitions.py --rust-tests --rust-suite native_producer_repository_tests
""")
    parser.add_argument("--versions", nargs="+", choices=["16", "18"], default=["16", "18"])
    parser.add_argument("--output", type=Path)
    parser.add_argument("--rust-tests", action="store_true", help="run the five declared focused Cargo suites on the fresh schema")
    parser.add_argument("--brief", action="store_true", help="print suite counts and identities hashes; evidence keeps full outputs")
    parser.add_argument("--producer-only", action="store_true", help="fresh owner/grant and producer probes; omit prepared-reader, partition lifecycle and clone-lock SQL probes; keep selected Rust suites")
    parser.add_argument("--dry-run", action="store_true", help="print ordered fresh migrations, checksums and selected probes; do not launch Docker or Cargo")
    parser.add_argument("--rust-suite", nargs="+", choices=["migration_tests","native_partition_repository_tests","native_producer_repository_tests","test_database_lifecycle_tests","retention"], help="explicit focused Rust suite selection; default runs all five")
    args = parser.parse_args()
    if args.output and (args.output.exists() or args.output.is_symlink()):
        parser.error("refusing to overwrite evidence")
    if args.output and not args.output.parent.is_dir():
        parser.error("--output parent directory must already exist")
    if len(args.versions) != len(set(args.versions)):
        parser.error("--versions must not repeat a PostgreSQL version")
    if args.rust_suite and not args.rust_tests:
        parser.error("--rust-suite requires --rust-tests; example: --rust-tests --rust-suite native_producer_repository_tests")
    fingerprints = migration_fingerprints()
    script_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    if args.dry_run:
        plan = {"dry_run": True, "schema_policy": "fresh_install", "versions": args.versions,
                "script_sha256": script_sha256, "migrations": fingerprints,
                "metadata_migrations": [path.name for path in metadata_paths()],
                "native_migrations": list(NATIVE_MIGRATIONS),
                "sql_probes": ["fresh_producer_schema", "producer_coalescing"] if args.producer_only else
                    ["fresh_producer_schema", "prepared_statement_lifecycle", "lifecycle", "producer_coalescing", "fresh_install_and_clone_scope"],
                "rust_tests": args.rust_tests, "selected_rust_suites": args.rust_suite}
        write_evidence(args.output, plan)
        if args.brief:
            print(json.dumps({"dry_run": True, "script_sha256": script_sha256,
                              "versions": args.versions, "metadata_migration_count": len(plan["metadata_migrations"]),
                              "native_migrations": plan["native_migrations"], "sql_probes": plan["sql_probes"],
                              "output": str(args.output)}, indent=2))
        else:
            print(json.dumps(plan, indent=2))
        return
    run_id = "schema-" + uuid.uuid4().hex[:12]
    evidence = {"run_id": run_id, "schema_policy": "fresh_install", "script_sha256": script_sha256,
                "producer_only": args.producer_only, "selected_rust_suites": args.rust_suite,
                "migrations": fingerprints, "versions": []}
    failed_versions = []
    try:
        for version in args.versions:
            report = {"version": version, "rejections": {}, "observations": {}}
            evidence["versions"].append(report)
            with protocol.Server(f"postgres:{version}-alpine", run_id, report) as server:
                try:
                    metadata_schema(server)
                    fresh_producer_schema(server, report)
                    if not args.producer_only:
                        prepared_statement_lifecycle(server, report)
                        lifecycle(server, report)
                    producer_coalescing(server, report)
                    if not args.producer_only:
                        fresh_install_and_clone_scope(server, report)
                    if args.rust_tests:
                        report["passed"] = rust_tests(server, report, args.output, args.rust_suite)
                        if not report["passed"]:
                            failed_versions.append(version)
                    else:
                        report["passed"] = True
                finally:
                    try:
                        close_protocol_sessions(server, report)
                    except BaseException:
                        report["passed"] = False
                        raise
        require(not failed_versions, f"focused Rust suites failed on PostgreSQL {', '.join(failed_versions)}; see per-suite stdout/stderr in evidence")
    except BaseException as error:
        evidence["error"] = str(error)
        raise
    finally:
        cleanup_error = None
        try:
            evidence["remaining_resources"] = resource_inventory(run_id)
            require(not any(evidence["remaining_resources"].values()), "owned native verifier resources remain")
        except Exception as error:
            cleanup_error = error
            evidence["resource_cleanup_error"] = str(error)
        rendered = json.dumps(evidence, indent=2)
        write_evidence(args.output, evidence)
        if args.brief:
            print(json.dumps({"run_id": run_id, "producer_only": args.producer_only, "output": str(args.output), "error": evidence.get("error"),
                "remaining_resources": evidence.get("remaining_resources"), "resource_cleanup_error": evidence.get("resource_cleanup_error"), "versions": [
                {"version": r["version"], "passed": r.get("passed"), "clone_lock_scope_control": r.get("clone_lock_scope_control"),
                 "prepared_statement_lifecycle": r.get("prepared_statement_lifecycle"),
                 "producer_coalescing": r.get("producer_coalescing"),
                 "verifier_client_session_leaks": r.get("verifier_client_session_leaks"),
                 "rust_test_clone_leaks": r.get("rust_test_clone_leaks"), "rust_test_client_session_leaks": r.get("rust_test_client_session_leaks"),
                 "cleanup_errors": r.get("cleanup_errors"), "suites": [
                     {k: run[k] for k in ["suite", "run_id", "exit_code", "seconds", "summaries", "identity_sha256"]}
                     for run in r.get("rust_tests", [])]} for r in evidence["versions"]]}, indent=2))
        else:
            print(rendered)
        if cleanup_error:
            raise RuntimeError("owned verifier resource cleanup failed") from cleanup_error


if __name__ == "__main__":
    main()
