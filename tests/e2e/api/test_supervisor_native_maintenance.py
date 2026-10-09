"""Real supervisor processes on the runner's exclusively owned PostgreSQL stack.

Stop the stack supervisor before selecting this module or the retention module.
These tests change global runtime settings and must not use pytest-xdist.
"""

from __future__ import annotations

import subprocess
import time
from datetime import datetime, timedelta, timezone

import pytest
import requests
import yaml
from e2e.api.test_supervisor_retention import (
    _capture_json,
    _cleanup_marker,
    _configure_runtime_retention,
    _connect,
    _count,
    _restore_runtime_retention_config,
    _seed_foundation,
    _snapshot_runtime_retention_config,
    _start_supervisor,
    _stop_supervisor,
    _uid,
    _wait_for_log,
    _wait_for_supervisor,
    _write_supervisor_config,
)
from psycopg import sql
from psycopg.types.json import Jsonb

NATIVE_CONFIG = {
    "enabled": True,
    "partition_interval_seconds": 1,
    "summary_interval_seconds": 2,
    "partition_lookahead_days": 9,
    "max_partition_operations_per_cycle": 8,
    "default_repair_row_limit": 10,
    "lock_timeout_milliseconds": 250,
    "operation_timeout_milliseconds": 1000,
    "max_partition_cycle_milliseconds": 5000,
    "max_summary_buckets_per_cycle": 16,
    "max_summary_invalidations_per_bucket": 100,
    "summary_bootstrap_hours": 1,
    "max_summary_cycle_milliseconds": 5000,
}


class NativeScenario:
    def __init__(self, tmp_path):
        self.marker = f"native-e2e-{_uid()}"
        self.conn, self.schema = _connect()
        self.processes = []
        self.owned_days = set()
        self.extra_leaves = []
        self.snapshot = None
        self.ids = None
        self.started = None
        self.config_path = _write_supervisor_config(
            tmp_path, schema=self.schema, enabled_targets=set(),
            maintenance={"enabled": False, "monitoring_enabled": False,
                         "artifact_cleanup_enabled": False, "corrective_actions_enabled": False},
        )
        # Give every process run an explicit owning service name.
        config = yaml.safe_load(self.config_path.read_text())
        config["service_name"] = self.marker
        self.config_path.write_text(yaml.safe_dump(config))

    def setup(self):
        with self.conn.cursor() as cur:
            self.snapshot = _snapshot_runtime_retention_config(cur)
            self.ids = _seed_foundation(cur, self.marker)
            cur.execute("SELECT clock_timestamp(), date_trunc('hour', clock_timestamp(), 'UTC') - interval '3 hours'")
            self.started, self.hour = cur.fetchone()
            _configure_runtime_retention(cur, enabled_targets=set(), enabled=False)
            self.configure(cur)
        self.conn.commit()

    def configure(self, cur, **overrides):
        config = {**NATIVE_CONFIG, **overrides}
        cur.execute("UPDATE runtime_retention_config SET native_maintenance = %s", (Jsonb(config),))
        return config

    def start(self):
        process = _start_supervisor(self.config_path)
        self.processes.append(process)
        return process

    def stop(self, process):
        output = _stop_supervisor(process)
        self.processes.remove(process)
        return output

    def check(self, predicate):
        with _connect()[0] as conn, conn.cursor() as cur:
            return predicate(cur)

    def wait(self, process, predicate, timeout=90):
        _wait_for_supervisor(process, lambda: self.check(predicate), timeout=timeout)

    def fresh_day(self, cur, offset=0):
        # Historical days cannot contain the stack's current runtime activity.
        day = datetime(2001, 1, 1, tzinfo=timezone.utc) + timedelta(days=offset)
        for parent, column in (("event", "created"), ("execution_history", "time"), ("audit_event", "created")):
            assert _count(cur, parent, f"{column} >= %s AND {column} < %s", (day, day + timedelta(days=1))) == 0
            assert _count(cur, "native_partition_registry", "parent = %s AND lower_bound = %s", (parent, day)) == 0
        for table in ("native_summary_hour", "native_summary_invalidation",
                      "event_volume_hourly_summary", "execution_status_hourly_summary",
                      "execution_creation_hourly_summary", "worker_status_hourly_summary"):
            assert _count(cur, table, "bucket >= %s AND bucket < %s", (day, day + timedelta(days=1))) == 0
        self.owned_days.add(day)
        return day

    def event(self, cur, created, count=1):
        cur.execute(
            """INSERT INTO event (trigger, trigger_ref, payload, created)
               SELECT %s, %s, %s, %s FROM generate_series(1, %s) RETURNING id""",
            (self.ids["trigger_id"], self.ids["trigger_ref"], Jsonb({"marker": self.marker}), created, count),
        )
        return [row[0] for row in cur.fetchall()]

    def summary(self, cur, count):
        assert _count(cur, "native_summary_hour", "kind = 'event_volume' AND bucket = %s", (self.hour,)) == 1
        assert _count(cur, "native_summary_invalidation", "kind = 'event_volume' AND bucket = %s", (self.hour,)) == 0
        cur.execute("SELECT event_count FROM event_volume_hourly_summary WHERE bucket = %s AND trigger_ref = %s",
                    (self.hour, self.ids["trigger_ref"]))
        assert cur.fetchone() == (count,)
        return True

    def analytics(self, client, mode):
        response = client._request(
            "GET", "/api/v1/analytics/events/volume",
            params={"since": self.hour.isoformat(), "until": self.hour.isoformat()},
        )
        assert response.status_code == 200, response.text
        data = response.json()["data"]
        meta = data["read_coverage"]
        assert meta["mode"] == mode
        used = meta["summary_ranges"] if mode == "summary_only" else meta["raw_ranges"]
        unused = meta["raw_ranges"] if mode == "summary_only" else meta["summary_ranges"]
        assert not unused
        assert len(used) == 1
        assert datetime.fromisoformat(used[0]["start"]) == self.hour
        assert datetime.fromisoformat(used[0]["end"]) == self.hour + timedelta(hours=1)
        assert (meta["oldest_refresh"] is not None) is (mode == "summary_only")
        expected = self.check(lambda cur: _count(
            cur, "event", "created >= %s AND created < %s",
            (self.hour, self.hour + timedelta(hours=1)),
        ))
        assert sum(point["value"] for point in data["data"]) == expected

    def schedule(self, cur):
        cur.execute("SELECT job, next_due, last_success FROM native_maintenance_schedule ORDER BY job")
        return {job: (due, success) for job, due, success in cur.fetchall()}

    def native_status(self, client, timeout=30):
        deadline = time.monotonic() + timeout
        attempts = []
        try:
            while True:
                remaining = deadline - time.monotonic()
                assert remaining > 0, f"Native status remained unavailable: {attempts}"
                response = client._request(
                    "GET", "/api/v1/retention-config/native-status", timeout=min(10, remaining),
                )
                attempts.append({"status": response.status_code})
                if response.status_code != 503:
                    assert response.status_code == 200, response.text
                    return response
                assert response.json() == {
                    "error": "Database operation temporarily unavailable; please retry",
                    "code": "RETRYABLE_DATABASE_ERROR",
                }, response.text
                # Only the documented transient response is polled. A 500,
                # auth error or malformed 503 fails immediately, never retried.
                time.sleep(min(0.2, max(0, deadline - time.monotonic())))
        finally:
            _capture_json("native-status-availability", {"attempts": attempts, "timeout_seconds": timeout})

    def audits(self, cur, job):
        # Native audits currently omit service_name. Scope through this exclusive
        # service run and its lifetime rather than matching another run's counters.
        cur.execute(
            """SELECT a.outcome::text, a.details FROM audit_event a
               WHERE a.event_type = 'maintenance.native.job_completed'
                 AND a.actor_login = 'attune-supervisor' AND a.resource_type = 'native_maintenance'
                 AND a.resource_ref = %s
                 AND a.created >= %s
                 AND EXISTS (SELECT 1 FROM supervisor_run r WHERE r.service_name = %s
                     AND a.created >= r.started_at AND (r.stopped_at IS NULL OR a.created <= r.stopped_at))
               ORDER BY a.created, a.id""",
            (job, self.started, self.marker),
        )
        return cur.fetchall()

    def cleanup(self):
        stop_errors = []
        for process in list(self.processes):
            try:
                self.stop(process)
            except (AssertionError, OSError, subprocess.TimeoutExpired) as exc:
                stop_errors.append(exc)
        if stop_errors:
            self.conn.close()
            raise RuntimeError(
                f"Failed to stop owned supervisor processes before restoration: {stop_errors}"
            ) from stop_errors[0]
        self.conn.rollback()
        try:
            if self.snapshot is not None and self.started is not None:
                with self.conn.cursor() as cur:
                    cur.execute(
                        """DELETE FROM audit_event a WHERE a.event_type = 'maintenance.native.job_completed'
                           AND a.created >= %s AND EXISTS (SELECT 1 FROM supervisor_run r
                               WHERE r.service_name = %s AND a.created >= r.started_at
                                 AND a.created <= r.stopped_at)""", (self.started, self.marker),
                    )
                    cur.execute("DELETE FROM audit_event WHERE details->>'service_name' = %s", (self.marker,))
                self.conn.commit()
            try:
                if self.snapshot is not None and self.ids is not None:
                    # Seeds are removed only after every writer and log reader joins.
                    _cleanup_marker(self.marker)
                    with self.conn.cursor() as cur:
                        for leaf in self.extra_leaves:
                            cur.execute("SELECT to_regclass(%s)", (leaf,))
                            if cur.fetchone()[0] is None:
                                continue
                            assert _count(cur, leaf, "TRUE") == 0
                            cur.execute(sql.SQL("DROP TABLE {}").format(sql.Identifier(leaf)))
                            cur.execute("DELETE FROM native_partition_registry WHERE partition_name = %s", (leaf,))
                        for day in self.owned_days:
                            cur.execute("SELECT partition_name FROM native_partition_registry WHERE lower_bound = %s", (day,))
                            for (leaf,) in cur.fetchall():
                                assert _count(cur, leaf, "TRUE") == 0, f"Unowned rows appeared in {leaf}"
                                cur.execute(sql.SQL("DROP TABLE {}").format(sql.Identifier(leaf)))
                                cur.execute("DELETE FROM native_partition_registry WHERE partition_name = %s", (leaf,))
                            for table in ("native_summary_hour", "native_summary_invalidation",
                                          "event_volume_hourly_summary", "execution_status_hourly_summary",
                                          "execution_creation_hourly_summary", "worker_status_hourly_summary"):
                                cur.execute(sql.SQL("DELETE FROM {} WHERE bucket >= %s AND bucket < %s").format(sql.Identifier(table)),
                                            (day, day + timedelta(days=1)))
                        cur.execute("DELETE FROM event_volume_hourly_summary WHERE trigger_ref = %s", (self.ids["trigger_ref"],))
                        cur.execute("DELETE FROM supervisor_run WHERE service_name = %s", (self.marker,))
                    self.conn.commit()
            finally:
                self.conn.rollback()
        finally:
            try:
                _restore_runtime_retention_config(self.snapshot)
            finally:
                self.conn.close()


@pytest.fixture
def native_scenario(tmp_path):
    scenario = NativeScenario(tmp_path)
    try:
        scenario.setup()
        yield scenario
    finally:
        scenario.cleanup()


@pytest.mark.api
@pytest.mark.integration
@pytest.mark.supervisor
class TestSupervisorNativeMaintenance:
    def test_native_cadences_run_with_retention_disabled_and_authenticated_status(self, native_scenario, client, api_base_url):
        s = native_scenario
        with s.conn.cursor() as cur:
            _configure_runtime_retention(cur, enabled_targets={"notifications"}, enabled=False,
                                        max_age_seconds=5, check_interval_seconds=3600)
            s.configure(cur)
            day = s.fresh_day(cur)
            s.event(cur, day + timedelta(hours=1))
            s.event(cur, s.hour)
            cur.execute("INSERT INTO notification (channel, entity_type, entity, activity, content, created) VALUES (%s, 'event', '1', 'created', %s, %s)",
                        (s.marker, Jsonb({"marker": s.marker}), day))
        s.conn.commit()
        s.analytics(client, "raw_only")
        process = s.start()
        retention_completed = []

        def ready(cur):
            s.summary(cur, 1)
            assert _count(cur, "native_partition_registry", "parent = 'event' AND lower_bound = %s", (day,)) == 1
            assert _count(cur, "notification", "channel = %s", (s.marker,)) == 1
            assert _count(cur, "event", "payload->>'marker' = %s", (s.marker,)) == 2
            schedule = s.schedule(cur)
            assert schedule["retention"][1] is not None
            if not retention_completed:
                retention_completed.append(schedule["retention"])
            assert schedule["retention"] == retention_completed[0]
            for job, seconds in (("partition", 1), ("summary", 2), ("retention", 3600)):
                due, success = schedule[job]
                assert success is not None
                assert due - success == timedelta(seconds=seconds)
            assert len(s.audits(cur, "partition")) >= 3
            assert len(s.audits(cur, "summary")) >= 2
            assert s.schedule(cur)["partition"][1] > s.started
            assert _count(cur, "supervisor_run", "service_name = %s AND clean_shutdown = FALSE", (s.marker,)) == 1
            return True

        s.wait(process, ready)
        s.stop(process)
        response = requests.get(f"{api_base_url}/api/v1/retention-config/native-status", timeout=10)
        assert response.status_code == 401, response.text
        response = s.native_status(client)
        assert response.status_code == 200, response.text
        data = response.json()["data"]
        assert data["enabled"] is True
        assert {row["job"] for row in data["schedule"]} == {"partition", "summary", "retention"}
        assert {row["parent"] for row in data["partitions"]} == {"event", "execution_history", "audit_event"}
        assert {row["kind"] for row in data["summaries"]} == {"event_volume", "execution_status", "execution_creation", "worker_status"}
        s.analytics(client, "summary_only")
        with s.conn.cursor() as cur:
            s.event(cur, s.hour)
        s.conn.commit()
        s.analytics(client, "raw_only")

    def test_replacement_leader_resumes_durable_coverage_and_pending_invalidations(self, native_scenario):
        s = native_scenario
        with s.conn.cursor() as cur:
            s.event(cur, s.hour)
        s.conn.commit()
        first = s.start()
        s.wait(first, lambda cur: s.summary(cur, 1))
        lock_conn, _ = _connect()
        try:
            with lock_conn.cursor() as cur:
                cur.execute("SELECT advisory_lock_key FROM runtime_retention_config WHERE id = TRUE")
                key = cur.fetchone()[0]
                cur.execute("SET statement_timeout = '10s'")
                cur.execute("SELECT pg_advisory_lock(%s)", (key,))
            replacement = s.start()
            _wait_for_log(replacement, "Another supervisor owns the retention lock")
            s.stop(first)
            with s.conn.cursor() as cur:
                before = s.schedule(cur)
                cur.execute("UPDATE native_maintenance_schedule SET next_due = clock_timestamp() + interval '1 hour' WHERE job IN ('summary', 'retention')")
                due_before = s.schedule(cur)
                s.event(cur, s.hour)
                cur.execute("SELECT id FROM native_summary_invalidation WHERE kind = 'event_volume' AND bucket = %s", (s.hour,))
                pending_ids = [row[0] for row in cur.fetchall()]
                assert pending_ids
                assert _count(cur, "native_summary_hour", "kind = 'event_volume' AND bucket = %s", (s.hour,)) == 1
            s.conn.commit()
            with lock_conn.cursor() as cur:
                cur.execute("SELECT pg_advisory_unlock(%s)", (key,))
                assert cur.fetchone() == (True,)
            _wait_for_log(replacement, "Supervisor maintenance cycle finished")
            with s.conn.cursor() as cur:
                assert s.schedule(cur)["summary"] == due_before["summary"]
                assert s.schedule(cur)["retention"] == due_before["retention"]
                assert _count(cur, "native_summary_invalidation", "id = ANY(%s)", (pending_ids,)) == len(pending_ids)
                cur.execute("UPDATE native_maintenance_schedule SET next_due = clock_timestamp() WHERE job = 'summary'")
            s.conn.commit()

            def resumed(cur):
                s.summary(cur, 2)
                assert s.schedule(cur)["summary"][1] > before["summary"][1]
                assert s.schedule(cur)["retention"] == due_before["retention"]
                assert _count(cur, "supervisor_run", "service_name = %s AND clean_shutdown = TRUE", (s.marker,)) == 1
                assert _count(cur, "supervisor_run", "service_name = %s AND clean_shutdown = FALSE", (s.marker,)) == 1
                return True

            s.wait(replacement, resumed)
        finally:
            lock_conn.close()

    @pytest.mark.parametrize("native_enabled", [True, False])
    def test_expiry_removes_owned_leaf_boundary_default_and_worker_summary_metadata(self, native_scenario, native_enabled):
        s = native_scenario
        with s.conn.cursor() as cur:
            leaf_day = s.fresh_day(cur)
            default_day = s.fresh_day(cur, 2)
            s.event(cur, leaf_day + timedelta(hours=1), count=3)
            cur.execute("SELECT outcome, rows_moved FROM native_partition_ensure_day('event', %s, 10)", (leaf_day,))
            assert cur.fetchone() == ("applied", 3)
            cur.execute("SELECT partition_name FROM native_partition_registry WHERE parent = 'event' AND lower_bound = %s", (leaf_day,))
            expired_leaf = cur.fetchone()[0]
            s.event(cur, default_day + timedelta(hours=1), count=2)
            # Keep the retention cutoff well inside yesterday, away from midnight.
            cur.execute("SELECT date_trunc('day', clock_timestamp(), 'UTC') - interval '1 day'")
            boundary_day = cur.fetchone()[0]
            s.event(cur, boundary_day + timedelta(hours=1))
            cur.execute("SELECT outcome FROM native_partition_ensure_day('event', %s, 10)", (boundary_day,))
            boundary_outcome = cur.fetchone()[0]
            assert boundary_outcome in {"applied", "already_present"}
            if boundary_outcome == "applied":
                cur.execute("SELECT partition_name FROM native_partition_registry WHERE parent = 'event' AND lower_bound = %s", (boundary_day,))
                s.extra_leaves.append(cur.fetchone()[0])
            kept_ids = s.event(cur, boundary_day + timedelta(hours=23))
            # Fixed 12:00 yesterday cutoff, represented through the runtime age.
            cutoff = boundary_day + timedelta(hours=12)
            cur.execute("SELECT ceil(extract(epoch FROM clock_timestamp() - %s))::bigint", (cutoff,))
            age = cur.fetchone()[0]
            # Refuse unrelated historical cohorts before enabling global cleanup.
            assert _count(cur, "event", "created < %s AND payload->>'marker' IS DISTINCT FROM %s", (cutoff, s.marker)) == 0
            assert _count(cur, "worker_history", "time < %s", (cutoff,)) == 0
            cur.execute("INSERT INTO worker_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES (%s, 'UPDATE', 9000002, %s, ARRAY['status'], %s)",
                        (default_day + timedelta(hours=1), s.marker, Jsonb({"status": "inactive", "marker": s.marker})))
            worker_bucket = default_day + timedelta(hours=1)
            cur.execute("INSERT INTO worker_status_hourly_summary (bucket, worker_name, new_status, transition_count) VALUES (%s, %s, 'inactive', 1)", (worker_bucket, s.marker))
            cur.execute("INSERT INTO native_summary_hour (kind, bucket, refreshed_at) VALUES ('worker_status', %s, clock_timestamp())", (worker_bucket,))
            for bucket, count in ((leaf_day + timedelta(hours=1), 3), (default_day + timedelta(hours=1), 2)):
                cur.execute("INSERT INTO event_volume_hourly_summary (bucket, trigger_ref, event_count) VALUES (%s, %s, %s)", (bucket, s.ids["trigger_ref"], count))
                cur.execute("INSERT INTO native_summary_hour (kind, bucket, refreshed_at) VALUES ('event_volume', %s, clock_timestamp())", (bucket,))
            _configure_runtime_retention(cur, enabled_targets={"events", "worker_history"}, max_age_seconds=age, check_interval_seconds=1)
            # No partition/summary job is due. Native expiry remains independent.
            s.configure(cur, enabled=native_enabled, partition_interval_seconds=3600,
                        summary_interval_seconds=3600, default_repair_row_limit=1)
            cur.execute("UPDATE native_maintenance_schedule SET next_due = clock_timestamp() + interval '1 hour' WHERE job IN ('partition', 'summary')")
        s.conn.commit()
        process = s.start()

        def expired(cur):
            assert _count(cur, "event", "payload->>'marker' = %s", (s.marker,)) == 1
            assert _count(cur, "event", "id = ANY(%s)", (kept_ids,)) == 1
            assert _count(cur, "worker_history", "entity_ref = %s", (s.marker,)) == 0
            for kind in ("event_volume", "worker_status"):
                for table in ("native_summary_hour", "native_summary_invalidation"):
                    assert _count(cur, table, "kind = %s AND bucket >= %s AND bucket < %s", (kind, leaf_day, default_day + timedelta(days=1))) == 0
            assert _count(cur, "event_volume_hourly_summary", "trigger_ref = %s AND bucket < %s", (s.ids["trigger_ref"], default_day + timedelta(days=1))) == 0
            assert _count(cur, "worker_status_hourly_summary", "worker_name = %s", (s.marker,)) == 0
            cur.execute("SELECT to_regclass(%s)", (expired_leaf,))
            assert (cur.fetchone()[0] is None) is native_enabled
            cur.execute("""SELECT details FROM audit_event WHERE event_type = 'maintenance.retention.target_completed'
                           AND details->>'service_name' = %s AND resource_ref = 'events' AND created >= %s ORDER BY created, id""", (s.marker, s.started))
            audits = [row[0] for row in cur.fetchall()]
            assert audits
            assert sum(row["deleted"] for row in audits) == (3 if native_enabled else 6)
            assert sum(row["partitions_dropped"] for row in audits) == (1 if native_enabled else 0)
            assert all(isinstance(row["candidates_exact"], bool) for row in audits)
            return True

        s.wait(process, expired)

    def test_cap_one_repair_rotates_past_oversized_default_to_other_parents_and_future_days(self, native_scenario, client):
        s = native_scenario
        with s.conn.cursor() as cur:
            oversized = s.fresh_day(cur)
            small = s.fresh_day(cur, 1)
            s.event(cur, oversized + timedelta(hours=1), count=3)
            s.event(cur, small + timedelta(hours=1))
            cur.execute("INSERT INTO execution_history (time, operation, entity_id, entity_ref, changed_fields, new_values) VALUES (%s, 'UPDATE', 9000001, %s, ARRAY['status'], %s)",
                        (small + timedelta(hours=1), s.marker, Jsonb({"status": "completed", "marker": s.marker})))
            s.configure(cur, max_partition_operations_per_cycle=1, default_repair_row_limit=1,
                        partition_lookahead_days=11)
            cur.execute("SELECT date_trunc('day', clock_timestamp(), 'UTC') + interval '11 days'")
            future_day = cur.fetchone()[0]
            for parent in ("event", "execution_history", "audit_event"):
                assert _count(cur, "native_partition_registry", "parent = %s AND lower_bound = %s", (parent, future_day)) == 0
            cur.execute("UPDATE native_partition_reconcile_state SET next_parent = 0 WHERE id = TRUE")
            cur.execute("UPDATE native_partition_reconcile_cursor SET last_day = NULL")
        s.conn.commit()
        process = s.start()

        def fair(cur):
            assert _count(cur, "event_default", "payload->>'marker' = %s AND created >= %s AND created < %s", (s.marker, oversized, small)) == 3
            for parent in ("event", "execution_history"):
                assert _count(cur, "native_partition_registry", "parent = %s AND lower_bound = %s", (parent, small)) == 1
            for parent in ("event", "execution_history", "audit_event"):
                assert _count(cur, "native_partition_registry", "parent = %s AND lower_bound = %s", (parent, future_day)) == 1
            audits = s.audits(cur, "partition")
            assert any(row["deferred_over_budget"] == 1 for _, row in audits)
            assert all(row["attempted"] <= 1 for _, row in audits)
            return True

        s.wait(process, fair, timeout=120)
        s.stop(process)
        response = s.native_status(client)
        assert response.status_code == 200, response.text
        events = next(row for row in response.json()["data"]["partitions"] if row["parent"] == "event")
        assert events["default_rows_at_least"] == 2
        assert events["default_count_exact"] is False
        assert datetime.fromisoformat(events["oldest_default_day"]) == oversized
        assert events["missing_future_partitions"] == 0

    def test_reconciliation_failure_audits_earlier_committed_repair(self, native_scenario):
        s = native_scenario
        with s.conn.cursor() as cur:
            repaired_day = s.fresh_day(cur)
            bad_day = s.fresh_day(cur, 2)
            s.event(cur, repaired_day + timedelta(hours=1))
            # An unregistered, empty, test-owned child fails catalog validation
            # on a later parent without replacing production functions or leaves.
            leaf = f"native_e2e_bad_{_uid()}"
            cur.execute(sql.SQL("CREATE TABLE {} PARTITION OF audit_event FOR VALUES FROM ({}) TO ({})").format(
                sql.Identifier(leaf), sql.Literal(bad_day), sql.Literal(bad_day + timedelta(days=1)),
            ))
            s.extra_leaves.append(leaf)
            s.configure(cur, partition_interval_seconds=3600, max_partition_operations_per_cycle=8)
            cur.execute("UPDATE native_partition_reconcile_state SET next_parent = 0 WHERE id = TRUE")
            cur.execute("UPDATE native_partition_reconcile_cursor SET last_day = NULL")
        s.conn.commit()
        process = s.start()

        def partial_progress(cur):
            assert _count(cur, "native_partition_registry", "parent = 'event' AND lower_bound = %s", (repaired_day,)) == 1
            assert _count(cur, "event", "payload->>'marker' = %s", (s.marker,)) == 1
            assert _count(cur, "event_default", "payload->>'marker' = %s", (s.marker,)) == 0
            failed = [row for outcome, row in s.audits(cur, "partition") if outcome == "failure"]
            assert failed
            assert failed[0]["partial_counters"]["created"] >= 1
            assert failed[0]["partial_counters"]["rows_moved"] == 1
            assert failed[0]["failed"] is True
            assert s.schedule(cur)["partition"][1] is None
            return True

        s.wait(process, partial_progress)
