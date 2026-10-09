"""
API Tests: Supervisor Retention

Validates that attune-supervisor applies short runtime retention windows and
keeps protected in-flight rows.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import threading
import time
import uuid
from collections import deque
from pathlib import Path

import psycopg
import pytest
import yaml
from psycopg import sql
from psycopg.conninfo import conninfo_to_dict
from psycopg.types.json import Jsonb

PROJECT_ROOT = Path(__file__).resolve().parents[3]
ALL_RETENTION_TARGETS = [
    "events",
    "enforcements",
    "executions",
    "execution_history",
    "worker_history",
    "sensor_process_history",
    "audit_events",
    "notifications",
    "webhook_event_logs",
    "inquiries",
    "work_queue_items",
    "work_queue_dispatches",
    "pack_test_executions",
    "execution_admission",
    "workers",
    "sensor_processes",
]


def _uid() -> str:
    return uuid.uuid4().hex[:8]


def _db_url() -> str:
    return os.environ.get(
        "DATABASE_URL", "postgresql://attune:attune@localhost:5432/attune"
    )


def _connect():
    settings = conninfo_to_dict(_db_url())
    assert os.environ.get("ATTUNE_E2E_RUN_ID") and settings.get("host") == "postgres" \
        and settings.get("dbname") == "attune", (
            "Supervisor E2E requires the runner-owned Docker database, never a developer database"
        )
    try:
        conn = psycopg.connect(_db_url())
    except psycopg.OperationalError as exc:
        pytest.skip(f"Supervisor retention E2E requires a reachable PostgreSQL database: {exc}")
    schema = _detect_schema(conn)
    with conn.cursor() as cur:
        cur.execute(
            sql.SQL("SET search_path TO {}, public").format(sql.Identifier(schema))
        )
    return conn, schema


def _detect_schema(conn) -> str:
    configured = (
        os.environ.get("ATTUNE__DATABASE__SCHEMA")
        or os.environ.get("ATTUNE_DB_SCHEMA")
        or os.environ.get("DATABASE_SCHEMA")
    )
    candidates = [configured] if configured else []
    candidates.extend(["attune", "public"])

    with conn.cursor() as cur:
        for schema in [candidate for candidate in candidates if candidate]:
            cur.execute("SELECT to_regclass(%s)", (f"{schema}.execution",))
            if cur.fetchone()[0] is not None:
                return schema

    pytest.skip("No migrated Attune schema found")


def _supervisor_command() -> list[str]:
    explicit = os.environ.get("ATTUNE_SUPERVISOR_COMMAND")
    if explicit:
        return shlex.split(explicit)

    explicit_bin = os.environ.get("ATTUNE_SUPERVISOR_BIN")
    if explicit_bin:
        return [explicit_bin]

    path_bin = shutil.which("attune-supervisor")
    if path_bin:
        return [path_bin]

    debug_bin = PROJECT_ROOT / "target" / "debug" / "attune-supervisor"
    if debug_bin.exists():
        return [str(debug_bin)]

    if shutil.which("cargo"):
        return ["cargo", "run", "--quiet", "--bin", "attune-supervisor", "--"]

    pytest.skip(
        "attune-supervisor binary not found; set ATTUNE_SUPERVISOR_BIN or "
        "ATTUNE_SUPERVISOR_COMMAND"
    )


def _write_supervisor_config(
    tmp_path: Path,
    *,
    schema: str,
    enabled_targets: set[str],
    max_age_seconds: int = 5,
    dry_run: bool = False,
    artifacts_dir: Path | None = None,
    maintenance: dict[str, object] | None = None,
) -> Path:
    targets = {
        target: {
            "max_age_seconds": max_age_seconds if target in enabled_targets else None,
        }
        for target in ALL_RETENTION_TARGETS
    }
    config = {
        "service_name": "attune-supervisor-e2e",
        "environment": "test",
        "database": {
            "url": _db_url(),
            "schema": schema,
            "max_connections": 5,
            "min_connections": 1,
        },
        "security": {
            "enable_auth": False,
            "jwt_secret": "e2e-supervisor-retention-jwt-secret-32chars",
            "encryption_key": "e2e-supervisor-retention-encryption-key-32chars",
        },
        "retention": {
            "enabled": True,
            "check_interval_seconds": 1,
            "batch_size": 500,
            "max_batches_per_target": 100,
            "dry_run": dry_run,
            "advisory_lock_key": 7_900_000 + int(uuid.uuid4().hex[:5], 16),
            "targets": targets,
        },
    }
    if artifacts_dir is not None:
        config["artifacts_dir"] = str(artifacts_dir)
    if maintenance is not None:
        config["maintenance"] = maintenance

    path = tmp_path / "supervisor-retention.yaml"
    path.write_text(yaml.safe_dump(config), encoding="utf-8")
    return path


def _snapshot_runtime_retention_config(cur) -> dict[str, object]:
    _assert_owned_supervisor_stack(cur)
    tables = {
        "runtime_retention_config": (
            "id", "enabled", "check_interval_seconds", "batch_size",
            "max_batches_per_target", "dry_run", "advisory_lock_key", "created",
            "updated", "cache_retention", "native_maintenance",
        ),
        "runtime_retention_target_config": ("target", "max_age_seconds", "created", "updated"),
        "native_maintenance_schedule": ("job", "next_due", "last_success"),
        "native_partition_reconcile_state": ("id", "next_parent"),
        "native_partition_reconcile_cursor": ("parent", "last_day"),
    }
    snapshot = {}
    for table, columns in tables.items():
        cur.execute(
            """SELECT attname FROM pg_attribute
               WHERE attrelid = to_regclass(%s) AND attnum > 0 AND NOT attisdropped""",
            (table,),
        )
        actual = {row[0] for row in cur.fetchall()}
        assert actual == set(columns), f"Refusing incomplete restoration of {table}: {actual}"
        cur.execute(sql.SQL("SELECT {} FROM {}").format(
            sql.SQL(", ").join(map(sql.Identifier, columns)), sql.Identifier(table)
        ))
        snapshot[table] = (columns, cur.fetchall())
    return snapshot


def _assert_owned_supervisor_stack(cur) -> None:
    run_id = os.environ.get("ATTUNE_E2E_RUN_ID", "")
    assert re.fullmatch(r"[a-z0-9][a-z0-9_-]{0,47}", run_id), (
        "Global supervisor tests require scripts/run-integration-tests.sh's disposable project"
    )
    settings = conninfo_to_dict(_db_url())
    assert settings.get("host") == "postgres" and settings.get("dbname") == "attune", (
        "Refusing global retention changes outside the Docker-owned E2E database"
    )
    assert not os.environ.get("PYTEST_XDIST_WORKER"), "Supervisor scenarios must run serially"
    cur.execute(
        """SELECT service_name FROM supervisor_run
           WHERE clean_shutdown = FALSE AND stopped_at IS NULL"""
    )
    assert not cur.fetchall(), "Stop the runner project's stack supervisor before these tests"


def _restore_runtime_retention_config(snapshot: dict[str, object] | None):
    if snapshot is None:
        return
    conn, _ = _connect()
    try:
        with conn.cursor() as cur:
            for table in reversed(snapshot):
                cur.execute(sql.SQL("DELETE FROM {}").format(sql.Identifier(table)))
            for table, (columns, rows) in snapshot.items():
                if rows:
                    cur.executemany(sql.SQL("INSERT INTO {} ({}) VALUES ({})").format(
                        sql.Identifier(table), sql.SQL(", ").join(map(sql.Identifier, columns)),
                        sql.SQL(", ").join(sql.Placeholder() for _ in columns),
                    ), [tuple(Jsonb(value) if column in {"cache_retention", "native_maintenance"}
                              else value for column, value in zip(columns, row)) for row in rows])
        conn.commit()
        restored = {}
        with conn.cursor() as cur:
            for table, (columns, rows) in snapshot.items():
                cur.execute(sql.SQL("SELECT {} FROM {} ORDER BY {}").format(
                    sql.SQL(", ").join(map(sql.Identifier, columns)), sql.Identifier(table),
                    sql.Identifier(columns[0]),
                ))
                restored[table] = (columns, cur.fetchall())
                # PostgreSQL enums sort by declaration order, not Python's text
                # order. Settings restoration compares rows, not their order.
                assert sorted(restored[table][1], key=lambda row: row[0]) == sorted(rows, key=lambda row: row[0]), (
                    f"Runtime restoration differs for {table}"
                )
        _capture_json("runtime-restoration", {"verified": True, "snapshot": snapshot, "restored": restored})
    finally:
        conn.close()


def _configure_runtime_retention(
    cur,
    *,
    enabled_targets: set[str],
    max_age_seconds: int = 5,
    dry_run: bool = False,
    enabled: bool = True,
    batch_size: int = 500,
    max_batches_per_target: int = 100,
    check_interval_seconds: int = 1,
) -> None:
    advisory_lock_key = 7_900_000 + int(uuid.uuid4().hex[:5], 16)
    cur.execute(
        """
        INSERT INTO runtime_retention_config (
            id, enabled, check_interval_seconds, batch_size, max_batches_per_target,
            dry_run, advisory_lock_key
        )
        VALUES (TRUE, %s, %s, %s, %s, %s, %s)
        ON CONFLICT (id) DO UPDATE SET
            enabled = EXCLUDED.enabled,
            check_interval_seconds = EXCLUDED.check_interval_seconds,
            batch_size = EXCLUDED.batch_size,
            max_batches_per_target = EXCLUDED.max_batches_per_target,
            dry_run = EXCLUDED.dry_run,
            advisory_lock_key = EXCLUDED.advisory_lock_key
        """,
        (enabled, check_interval_seconds, batch_size, max_batches_per_target, dry_run, advisory_lock_key),
    )
    # Old retention scenarios intentionally test row cleanup, not native jobs.
    cur.execute("UPDATE runtime_retention_config SET native_maintenance = '{\"enabled\":false}'::jsonb")
    cur.execute("UPDATE native_maintenance_schedule SET next_due = clock_timestamp(), last_success = NULL")
    for target in ALL_RETENTION_TARGETS:
        cur.execute(
            """
            INSERT INTO runtime_retention_target_config (target, max_age_seconds)
            VALUES (%s, %s)
            ON CONFLICT (target) DO UPDATE SET
                max_age_seconds = EXCLUDED.max_age_seconds
            """,
            (
                target,
                max_age_seconds if target in enabled_targets else None,
            ),
        )


class _SupervisorProcess(subprocess.Popen):
    def __init__(self, *args, **kwargs):
        kwargs["start_new_session"] = True
        super().__init__(*args, **kwargs)
        self.output = deque(maxlen=20_000)
        self.output_changed = threading.Condition()
        self.capture_path = None
        self.reader = threading.Thread(target=self._drain, name=f"supervisor-log-{self.pid}")
        self.reader.start()

    def _drain(self):
        assert self.stdout is not None
        for line in self.stdout:
            with self.output_changed:
                self.output.append(line)
                self.output_changed.notify_all()

    def logs(self) -> str:
        with self.output_changed:
            return "".join(self.output)


def _start_supervisor(config_path: Path) -> _SupervisorProcess:
    command = [*_supervisor_command(), "--config", str(config_path), "--log-level", "info"]
    env = {**os.environ, "RUST_LOG": "info", "ATTUNE_CONFIG": str(config_path)}
    return _SupervisorProcess(
        command,
        cwd=PROJECT_ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def _stop_supervisor(process: subprocess.Popen) -> str:
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)

    process.reader.join(timeout=5)
    if process.reader.is_alive():
        # Explicit commands may wrap the binary. Stop leftover children too.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.reader.join(timeout=5)
    assert not process.reader.is_alive(), "Supervisor log reader did not stop"
    if process.stdout is not None:
        process.stdout.close()
    output = process.logs()
    capture_dir = os.environ.get("ATTUNE_E2E_CAPTURE_DIR")
    if capture_dir and process.capture_path is None:
        process.capture_path = Path(capture_dir) / f"supervisor-{process.pid}-{_uid()}.log"
        process.capture_path.write_text(output, encoding="utf-8")
    return output


def _capture_json(name: str, data) -> None:
    capture_dir = os.environ.get("ATTUNE_E2E_CAPTURE_DIR")
    if capture_dir:
        path = Path(capture_dir) / f"{name}-{_uid()}.json"
        path.write_text(json.dumps(data, default=str, indent=2), encoding="utf-8")


def _wait_for_supervisor(process: subprocess.Popen, predicate, *, timeout: int = 60):
    deadline = time.monotonic() + timeout
    last_error: Exception | None = None

    while time.monotonic() < deadline:
        try:
            if predicate():
                return
        except AssertionError as exc:
            last_error = exc

        if process.poll() is not None:
            output = _stop_supervisor(process)
            if last_error is not None:
                raise AssertionError(f"{last_error}\nattune-supervisor exited early:\n{output}")
            raise AssertionError(f"attune-supervisor exited early:\n{output}")

        time.sleep(0.5)

    output = _stop_supervisor(process)
    if last_error is not None:
        raise AssertionError(f"{last_error}\nSupervisor output:\n{output}")
    raise TimeoutError(f"Retention condition was not met.\nSupervisor output:\n{output}")


def _wait_for_log(process: subprocess.Popen, needle: str, *, timeout: int = 60) -> str:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        output = process.logs()
        if needle in output:
            return output
        if process.poll() is not None:
            output = _stop_supervisor(process)
            if needle in output:
                return output
            raise AssertionError(
                f"attune-supervisor exited before logging {needle!r}:\n{output}"
            )
        with process.output_changed:
            process.output_changed.wait(timeout=min(0.2, max(0, deadline - time.monotonic())))
    raise TimeoutError(f"Timed out waiting for {needle!r}.\nSupervisor output:\n{_stop_supervisor(process)}")


def _count(cur, table: str, predicate: str, params: tuple = ()) -> int:
    cur.execute(
        sql.SQL("SELECT COUNT(*) FROM {} WHERE " + predicate).format(
            sql.Identifier(table)
        ),
        params,
    )
    return cur.fetchone()[0]


def _retention_audit_count(cur, target: str, *, dry_run: bool | None = None) -> int:
    predicate = """
        event_type = 'maintenance.retention.target_completed'
        AND actor_login = 'attune-supervisor'
        AND resource_type = 'runtime_retention'
        AND resource_ref = %s
        AND details->>'service_name' = 'attune-supervisor-e2e'
    """
    params: list[object] = [target]
    if dry_run is not None:
        predicate += " AND details->>'dry_run' = %s"
        params.append("true" if dry_run else "false")
    return _count(cur, "audit_event", predicate, tuple(params))


def _alert_count(cur, correlation_id: str) -> int:
    return _count(
        cur,
        "event",
        "trigger_ref = 'core.alert' AND payload->>'correlation_id' = %s",
        (correlation_id,),
    )


def _seed_foundation(cur, marker: str) -> dict[str, int | str]:
    pack_ref = f"e2eret{_uid()}"
    runtime_ref = f"{pack_ref}.native"
    action_ref = f"{pack_ref}.action"
    trigger_ref = f"{pack_ref}.trigger"
    rule_ref = f"{pack_ref}.rule"
    queue_ref = f"{pack_ref}.queue"
    sensor_ref = f"{pack_ref}.sensor"

    cur.execute(
        """
        INSERT INTO pack (ref, label, version, conf_schema, config, meta, tags)
        VALUES (%s, %s, '0.1.0', '{}'::jsonb, '{}'::jsonb, %s::jsonb, ARRAY[]::text[])
        RETURNING id
        """,
        (pack_ref, f"E2E Retention {marker}", json.dumps({"marker": marker})),
    )
    pack_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO runtime (
            ref, pack, pack_ref, name, aliases, distributions, execution_config
        )
        VALUES (%s, %s, %s, 'native', ARRAY[]::text[], '{}'::jsonb, '{}'::jsonb)
        RETURNING id
        """,
        (runtime_ref, pack_id, pack_ref),
    )
    runtime_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO action (
            ref, pack, pack_ref, label, entrypoint, runtime, param_schema, out_schema
        )
        VALUES (%s, %s, %s, 'Retention Action', 'noop.sh', %s, '{}'::jsonb, '{}'::jsonb)
        RETURNING id
        """,
        (action_ref, pack_id, pack_ref, runtime_id),
    )
    action_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO trigger (ref, pack, pack_ref, label, param_schema, out_schema)
        VALUES (%s, %s, %s, 'Retention Trigger', '{}'::jsonb, '{}'::jsonb)
        RETURNING id
        """,
        (trigger_ref, pack_id, pack_ref),
    )
    trigger_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO rule (
            ref, pack, pack_ref, label, action, action_ref, trigger, trigger_ref,
            conditions, action_params, trigger_params, enabled
        )
        VALUES (
            %s, %s, %s, 'Retention Rule', %s, %s, %s, %s,
            '[]'::jsonb, '{}'::jsonb, '{}'::jsonb, true
        )
        RETURNING id
        """,
        (rule_ref, pack_id, pack_ref, action_id, action_ref, trigger_id, trigger_ref),
    )
    rule_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO sensor (
            ref, pack, pack_ref, label, entrypoint, runtime, runtime_ref, enabled,
            param_schema, config
        )
        VALUES (
            %s, %s, %s, 'Retention Sensor', 'sensor.sh', %s, %s, true,
            '{}'::jsonb, '{}'::jsonb
        )
        RETURNING id
        """,
        (sensor_ref, pack_id, pack_ref, runtime_id, runtime_ref),
    )
    sensor_id = cur.fetchone()[0]

    cur.execute(
        """
        INSERT INTO work_queue (
            ref, pack, pack_ref, is_adhoc, label, enabled, accepting_new_items,
            dispatch_action, dispatch_action_ref, item_schema, action_params, config
        )
        VALUES (
            %s, %s, %s, true, 'Retention Queue', false, true,
            %s, %s, '{}'::jsonb, '{}'::jsonb, %s::jsonb
        )
        RETURNING id
        """,
        (queue_ref, pack_id, pack_ref, action_id, action_ref, json.dumps({"marker": marker})),
    )
    queue_id = cur.fetchone()[0]

    return {
        "pack_ref": pack_ref,
        "pack_id": pack_id,
        "runtime_id": runtime_id,
        "action_ref": action_ref,
        "action_id": action_id,
        "trigger_ref": trigger_ref,
        "trigger_id": trigger_id,
        "rule_ref": rule_ref,
        "rule_id": rule_id,
        "queue_ref": queue_ref,
        "queue_id": queue_id,
        "sensor_ref": sensor_ref,
        "sensor_id": sensor_id,
    }


def _cleanup_marker(marker: str):
    conn, _ = _connect()
    try:
        with conn.cursor() as cur:
            cur.execute(
                """
                DELETE FROM work_queue_dispatch d
                USING work_queue q
                WHERE d.queue = q.id AND q.config->>'marker' = %s
                """,
                (marker,),
            )
            cur.execute(
                "DELETE FROM work_queue_item WHERE payload->>'marker' = %s",
                (marker,),
            )
            cur.execute("DELETE FROM work_queue WHERE config->>'marker' = %s", (marker,))
            cur.execute("DELETE FROM inquiry WHERE prompt LIKE %s", (f"%{marker}%",))
            cur.execute(
                """
                DELETE FROM execution_admission_entry e
                USING execution_admission_state s
                WHERE e.state_id = s.id AND s.group_key LIKE %s
                """,
                (f"%{marker}%",),
            )
            cur.execute(
                "DELETE FROM execution_admission_state WHERE group_key LIKE %s",
                (f"%{marker}%",),
            )
            cur.execute("DELETE FROM execution WHERE config->>'marker' = %s", (marker,))
            cur.execute(
                "DELETE FROM enforcement WHERE config->>'marker' = %s", (marker,)
            )
            cur.execute(
                "DELETE FROM webhook_event_log WHERE headers->>'marker' = %s",
                (marker,),
            )
            cur.execute("DELETE FROM event WHERE payload->>'marker' = %s", (marker,))
            cur.execute(
                """
                DELETE FROM event
                WHERE trigger_ref = 'core.alert'
                  AND (
                    payload->'details'->>'marker' = %s
                    OR payload->'details'->>'service_name' = 'attune-supervisor-e2e'
                  )
                """,
                (marker,),
            )
            cur.execute("DELETE FROM notification WHERE content->>'marker' = %s", (marker,))
            cur.execute("DELETE FROM artifact WHERE ref LIKE %s", (f"%{marker}%",))
            cur.execute(
                """
                DELETE FROM pack_test_execution pte
                USING pack p
                WHERE pte.pack_id = p.id AND p.meta->>'marker' = %s
                """,
                (marker,),
            )
            cur.execute("DELETE FROM sensor_process WHERE meta->>'marker' = %s", (marker,))
            cur.execute("DELETE FROM worker WHERE meta->>'marker' = %s", (marker,))
            cur.execute(
                "DELETE FROM supervisor_run WHERE id LIKE %s OR meta->>'marker' = %s",
                (f"%{marker}%", marker),
            )
            cur.execute("DELETE FROM execution_history WHERE entity_ref LIKE %s", (f"%{marker}%",))
            cur.execute("DELETE FROM worker_history WHERE entity_ref LIKE %s", (f"%{marker}%",))
            cur.execute("DELETE FROM sensor_process_history WHERE entity_ref LIKE %s", (f"%{marker}%",))
            cur.execute(
                """
                DELETE FROM audit_event
                WHERE event_type LIKE 'maintenance.%'
                  AND details->>'service_name' = 'attune-supervisor-e2e'
                """
            )
            cur.execute("DELETE FROM audit_event WHERE details->>'marker' = %s", (marker,))
            cur.execute("DELETE FROM pack WHERE meta->>'marker' = %s", (marker,))
            cur.execute("""DELETE FROM supervisor_run WHERE service_name = 'attune-supervisor-e2e'
                           AND clean_shutdown = TRUE AND stopped_at IS NOT NULL""")
            remaining = {}
            for table, predicate in {
                "pack": "meta->>'marker' = %s",
                "event": "payload->>'marker' = %s",
                "enforcement": "config->>'marker' = %s",
                "execution": "config->>'marker' = %s",
                "notification": "content->>'marker' = %s",
                "webhook_event_log": "headers->>'marker' = %s",
                "worker": "meta->>'marker' = %s",
                "sensor_process": "meta->>'marker' = %s",
                "work_queue": "config->>'marker' = %s",
                "work_queue_item": "payload->>'marker' = %s",
                "audit_event": "details->>'marker' = %s",
            }.items():
                remaining[table] = _count(cur, table, predicate, (marker,))
            assert not any(remaining.values()), f"Owned seed rows remain: {remaining}"
            _capture_json("owned-seed-cleanup", {"marker": marker, "remaining_rows": remaining})
        conn.commit()
    finally:
        conn.close()


@pytest.mark.api
@pytest.mark.integration
@pytest.mark.supervisor
class TestSupervisorRetention:
    def test_supervisor_purges_regular_runtime_rows_with_short_retention(self, tmp_path):
        marker = f"retention-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                ids = _seed_foundation(cur, marker)
                old = "NOW() - INTERVAL '10 seconds'"
                recent = "NOW() + INTERVAL '1 hour'"

                cur.execute(
                    f"""
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    VALUES
                        (%s, %s, 'completed', %s::jsonb, {old}, {old}),
                        (%s, %s, 'running', %s::jsonb, {old}, {old}),
                        (%s, %s, 'completed', %s::jsonb, {recent}, {recent})
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"old-terminal"}}',
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"old-running"}}',
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"recent-terminal"}}',
                    ),
                )
                old_execution_id, running_execution_id, recent_execution_id = [
                    row[0] for row in cur.fetchall()
                ]
                cur.execute(
                    f"""
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    VALUES
                        (%s, %s, 'completed', %s::jsonb, {old}, {old}),
                        (%s, %s, 'running', %s::jsonb, {old}, {old})
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"old-responded-inquiry"}}',
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"old-pending-inquiry"}}',
                    ),
                )
                responded_inquiry_execution_id, pending_inquiry_execution_id = [
                    row[0] for row in cur.fetchall()
                ]

                cur.execute(
                    f"""
                    INSERT INTO enforcement (
                        rule, rule_ref, trigger_ref, config, event, status, payload,
                        condition, conditions, created, resolved_at
                    )
                    VALUES
                        (%s, %s, %s, %s::jsonb, NULL, 'processed', %s::jsonb, 'all', '[]'::jsonb, {old}, {old}),
                        (%s, %s, %s, %s::jsonb, NULL, 'created', %s::jsonb, 'all', '[]'::jsonb, {old}, NULL)
                    """,
                    (
                        ids["rule_id"],
                        ids["rule_ref"],
                        ids["trigger_ref"],
                        f'{{"marker":"{marker}","kind":"old-processed"}}',
                        f'{{"marker":"{marker}"}}',
                        ids["rule_id"],
                        ids["rule_ref"],
                        ids["trigger_ref"],
                        f'{{"marker":"{marker}","kind":"old-created"}}',
                        f'{{"marker":"{marker}"}}',
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO notification (channel, entity_type, entity, activity, content, created, updated)
                    VALUES
                        ('e2e', 'execution', %s, 'completed', %s::jsonb, {old}, {old}),
                        ('e2e', 'execution', %s, 'completed', %s::jsonb, {recent}, {recent})
                    """,
                    (
                        str(old_execution_id),
                        f'{{"marker":"{marker}","kind":"old"}}',
                        str(recent_execution_id),
                        f'{{"marker":"{marker}","kind":"recent"}}',
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO webhook_event_log (
                        trigger_id, trigger_ref, webhook_key, status_code, headers, created
                    )
                    VALUES
                        (%s, %s, %s, 200, %s::jsonb, {old}),
                        (%s, %s, %s, 200, %s::jsonb, {recent})
                    """,
                    (
                        ids["trigger_id"],
                        ids["trigger_ref"],
                        f"wh_{marker}_old",
                        f'{{"marker":"{marker}","kind":"old"}}',
                        ids["trigger_id"],
                        ids["trigger_ref"],
                        f"wh_{marker}_recent",
                        f'{{"marker":"{marker}","kind":"recent"}}',
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO inquiry (
                        created_by_execution, prompt, response_options, status, response, created, updated
                    )
                    VALUES
                        (%s, %s, %s::jsonb, 'responded', %s::jsonb, {old}, {old}),
                        (%s, %s, %s::jsonb, 'pending', NULL, {old}, {old})
                    """,
                    (
                        responded_inquiry_execution_id,
                        f"{marker} old responded",
                        '[{"ref":"continue","label":"Continue","style":"default","response":{}}]',
                        '{"ok": true}',
                        pending_inquiry_execution_id,
                        f"{marker} old pending",
                        '[{"ref":"continue","label":"Continue","style":"default","response":{}}]',
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO work_queue_item (
                        queue, queue_ref, item_key, status, payload, metadata,
                        enqueue_source, created, updated
                    )
                    VALUES
                        (%s, %s, %s, 'completed', %s::jsonb, %s::jsonb, 'e2e', {old}, {old}),
                        (%s, %s, %s, 'queued', %s::jsonb, %s::jsonb, 'e2e', {old}, {old})
                    """,
                    (
                        ids["queue_id"],
                        ids["queue_ref"],
                        f"{marker}-old-item",
                        f'{{"marker":"{marker}"}}',
                        "{}",
                        ids["queue_id"],
                        ids["queue_ref"],
                        f"{marker}-queued-item",
                        f'{{"marker":"{marker}"}}',
                        "{}",
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO work_queue_dispatch (
                        queue, queue_ref, execution, status, leased_item_count, created, updated
                    )
                    VALUES
                        (%s, %s, %s, 'completed', 1, {old}, {old}),
                        (%s, %s, %s, 'dispatched', 1, {old}, {old})
                    """,
                    (
                        ids["queue_id"],
                        ids["queue_ref"],
                        old_execution_id,
                        ids["queue_id"],
                        ids["queue_ref"],
                        recent_execution_id,
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO pack_test_execution (
                        pack_id, pack_version, execution_time, trigger_reason, total_tests,
                        passed, failed, skipped, pass_rate, duration_ms, result, created
                    )
                    VALUES
                        (%s, '0.1.0', {old}, 'manual', 1, 1, 0, 0, 1.0, 1, %s::jsonb, {old}),
                        (%s, '0.1.0', {recent}, 'manual', 1, 1, 0, 0, 1.0, 1, %s::jsonb, {recent})
                    """,
                    (
                        ids["pack_id"],
                        f'{{"marker":"{marker}","kind":"old"}}',
                        ids["pack_id"],
                        f'{{"marker":"{marker}","kind":"recent"}}',
                    ),
                )

                cur.execute(
                    f"""
                    INSERT INTO execution_admission_state (
                        action_id, group_key, max_concurrent, created, updated
                    )
                    VALUES
                        (%s, %s, 1, {old}, {old}),
                        (%s, %s, 1, {old}, {old})
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        f"{marker}-orphan",
                        ids["action_id"],
                        f"{marker}-active",
                    ),
                )
                orphan_state_id, active_state_id = [row[0] for row in cur.fetchall()]
                cur.execute(
                    f"""
                    INSERT INTO execution_admission_entry (
                        state_id, execution_id, status, queue_order, enqueued_at, created, updated
                    )
                    VALUES (%s, %s, 'active', 1, {old}, {old}, {old})
                    """,
                    (active_state_id, running_execution_id),
                )

                cur.execute(
                    f"""
                    INSERT INTO worker (
                        name, worker_type, worker_role, status, capabilities, meta,
                        last_heartbeat, cordoned, created, updated
                    )
                    VALUES
                        (%s, 'local', 'action', 'inactive', '{{}}'::jsonb, %s::jsonb, {old}, false, {old}, {old}),
                        (%s, 'local', 'action', 'inactive', '{{}}'::jsonb, %s::jsonb, {old}, true, {old}, {old}),
                        (%s, 'local', 'action', 'active', '{{}}'::jsonb, %s::jsonb, {old}, false, {old}, {old})
                    RETURNING id, name
                    """,
                    (
                        f"{marker}-stale-worker",
                        f'{{"marker":"{marker}","kind":"stale"}}',
                        f"{marker}-cordoned-worker",
                        f'{{"marker":"{marker}","kind":"cordoned"}}',
                        f"{marker}-active-worker",
                        f'{{"marker":"{marker}","kind":"active"}}',
                    ),
                )
                worker_rows = cur.fetchall()

                for status, active_rules, suffix in [
                    ("stopped", 0, "old-stopped"),
                    ("running", 0, "old-running"),
                    ("stopped", 1, "old-active-rule"),
                ]:
                    cur.execute(
                        f"""
                        INSERT INTO worker (
                            name, worker_type, worker_role, status, capabilities, meta,
                            last_heartbeat, created, updated
                        )
                        VALUES (%s, 'local', 'sensor', 'active', '{{}}'::jsonb, %s::jsonb, {old}, {old}, {old})
                        RETURNING id, name
                        """,
                        (
                            f"{marker}-{suffix}-sensor-worker",
                            f'{{"marker":"{marker}","kind":"{suffix}"}}',
                        ),
                    )
                    worker_id, worker_name = cur.fetchone()
                    cur.execute(
                        f"""
                        INSERT INTO sensor_process (
                            sensor, sensor_ref, worker, worker_name, status, active_rule_count,
                            meta, created, updated
                        )
                        VALUES (%s, %s, %s, %s, %s, %s, %s::jsonb, {old}, {old})
                        """,
                        (
                            ids["sensor_id"],
                            ids["sensor_ref"],
                            worker_id,
                            worker_name,
                            status,
                            active_rules,
                            f'{{"marker":"{marker}","kind":"{suffix}"}}',
                        ),
                    )

                _configure_runtime_retention(
                    cur,
                    enabled_targets={
                        "enforcements",
                        "executions",
                        "notifications",
                        "webhook_event_logs",
                        "inquiries",
                        "work_queue_items",
                        "work_queue_dispatches",
                        "pack_test_executions",
                        "execution_admission",
                        "workers",
                        "sensor_processes",
                    },
                )
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                maintenance={
                    "corrective_actions_enabled": False,
                },
            )
            process = _start_supervisor(config_path)

            def retained_state_is_correct() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        assert _count(cur, "execution", "id = %s", (old_execution_id,)) == 0
                        assert _count(cur, "execution", "id = %s", (running_execution_id,)) == 1
                        assert _count(cur, "execution", "id = %s", (recent_execution_id,)) == 1
                        assert (
                            _count(
                                cur,
                                "enforcement",
                                "config->>'marker' = %s AND status = 'processed'",
                                (marker,),
                            )
                            == 0
                        )
                        assert (
                            _count(
                                cur,
                                "enforcement",
                                "config->>'marker' = %s AND status = 'created'",
                                (marker,),
                            )
                            == 1
                        )
                        assert (
                            _count(cur, "notification", "content->>'marker' = %s", (marker,))
                            == 1
                        )
                        assert (
                            _count(cur, "webhook_event_log", "headers->>'marker' = %s", (marker,))
                            == 1
                        )
                        assert (
                            _count(cur, "inquiry", "prompt = %s", (f"{marker} old responded",))
                            == 0
                        )
                        assert (
                            _count(cur, "inquiry", "prompt = %s", (f"{marker} old pending",))
                            == 1
                        )
                        assert (
                            _count(cur, "work_queue_item", "payload->>'marker' = %s", (marker,))
                            == 1
                        )
                        assert (
                            _count(cur, "work_queue_dispatch", "queue_ref = %s", (ids["queue_ref"],))
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "pack_test_execution",
                                "result->>'marker' = %s",
                                (marker,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "execution_admission_state",
                                "id = %s",
                                (orphan_state_id,),
                            )
                            == 0
                        )
                        assert (
                            _count(
                                cur,
                                "execution_admission_state",
                                "id = %s",
                                (active_state_id,),
                            )
                            == 1
                        )
                        assert (
                            _count(cur, "worker", "meta->>'marker' = %s", (marker,))
                            == len(worker_rows) - 1 + 3
                        )
                        assert (
                            _count(cur, "sensor_process", "meta->>'marker' = %s", (marker,))
                            == 2
                        )
                        assert _retention_audit_count(cur, "executions", dry_run=False) >= 1
                return True

            _wait_for_supervisor(process, retained_state_is_correct)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_cleans_expired_artifacts_and_emits_stuck_alerts(self, tmp_path):
        marker = f"maintenance-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None
        seed_committed = False
        artifacts_dir = tmp_path / "artifacts"
        artifacts_dir.mkdir()

        execution_correlation = "supervisor:stuck-runtime:execution:canceling"
        item_correlation = "supervisor:stuck-runtime:work_queue_item:leased"
        dispatch_correlation = "supervisor:stuck-runtime:work_queue_dispatch:leased"

        def capture_phase(phase: str) -> None:
            with _connect()[0] as check_conn, check_conn.cursor() as cur:
                rows = {
                    "supervisor_pid": process.pid if process is not None else None,
                    "supervisor_stopped": process.poll() is not None if process is not None else True,
                    "reader_joined": not process.reader.is_alive() if process is not None else True,
                }
                for name, query, params in [
                    ("clock", "SELECT clock_timestamp()", ()),
                    ("executions", "SELECT id, status, created, updated FROM execution WHERE config->>'marker' = %s ORDER BY id", (marker,)),
                    ("dispatches", "SELECT id, execution, status, created, updated FROM work_queue_dispatch WHERE queue = %s ORDER BY id", (ids["queue_id"],)),
                    ("items", "SELECT id, status, leased_execution, lease_expires_at, created, updated FROM work_queue_item WHERE payload->>'marker' = %s ORDER BY id", (marker,)),
                    ("admission_state", "SELECT max_concurrent, next_queue_order, total_enqueued, total_completed, created, updated FROM execution_admission_state WHERE id = %s", (admission_state_id,)),
                    ("admission_entries", "SELECT execution_id, status, queue_order, enqueued_at, activated_at, created, updated FROM execution_admission_entry WHERE state_id = %s ORDER BY queue_order", (admission_state_id,)),
                    ("alerts", "SELECT id, created, payload FROM event WHERE trigger_ref = 'core.alert' AND created >= %s ORDER BY created", (scenario_started,)),
                    ("audits", "SELECT id, created, event_type, details FROM audit_event WHERE actor_login = 'attune-supervisor' AND created >= %s ORDER BY created", (scenario_started,)),
                ]:
                    cur.execute(query, params)
                    rows[name] = cur.fetchall()
                _capture_json(f"maintenance-{phase}", rows)

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                cur.execute("SELECT clock_timestamp()")
                scenario_started = cur.fetchone()[0]
                ids = _seed_foundation(cur, marker)
                cur.execute(
                    """
                    INSERT INTO trigger (ref, pack, pack_ref, label, param_schema, out_schema)
                    SELECT 'core.alert', %s, %s, 'Core Alert', '{}'::jsonb, '{}'::jsonb
                    WHERE NOT EXISTS (SELECT 1 FROM trigger WHERE ref = 'core.alert')
                    """,
                    (ids["pack_id"], ids["pack_ref"]),
                )

                cur.execute(
                    """
                    INSERT INTO policy (
                        ref, pack, pack_ref, action, action_ref, name,
                        method, threshold, parameters
                    )
                    VALUES (%s, %s, %s, %s, %s, 'Remediation Admission',
                            'enqueue', 1, ARRAY['marker']::text[])
                    """,
                    (
                        f"{ids['pack_ref']}.admission",
                        ids["pack_id"],
                        ids["pack_ref"],
                        ids["action_id"],
                        ids["action_ref"],
                    ),
                )

                artifact_file = artifacts_dir / f"{marker}-v1.txt"
                artifact_file.write_text("expired artifact content", encoding="utf-8")
                cur.execute(
                    """
                    INSERT INTO artifact (
                        ref, scope, owner, type, visibility, retention_policy, retention_limit,
                        name, content_type
                    )
                    VALUES (%s, 'pack', %s, 'file_text', 'private', 'minutes', 1, %s, 'text/plain')
                    RETURNING id
                    """,
                    (f"{ids['pack_ref']}.{marker}.artifact", ids["pack_ref"], marker),
                )
                artifact_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO artifact_version (
                        artifact, version, content_type, size_bytes, file_path, created_by, created
                    )
                    VALUES (
                        %s, 1, 'text/plain', 24, %s, 'e2e',
                        NOW() - INTERVAL '2 minutes'
                    )
                    """,
                    (artifact_id, artifact_file.name),
                )

                cur.execute(
                    """
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    VALUES (
                        %s, %s, 'canceling', %s::jsonb,
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '10 seconds'
                    )
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"stuck-canceling"}}',
                    ),
                )
                stuck_execution_id = cur.fetchone()[0]

                cur.execute(
                    """
                    INSERT INTO execution_admission_state (
                        action_id, group_key, max_concurrent, next_queue_order,
                        total_enqueued, created, updated
                    )
                    VALUES (
                        %s, %s, 1, 2, 1,
                        NOW() - INTERVAL '20 seconds',
                        clock_timestamp()
                    )
                    RETURNING id
                    """,
                    (ids["action_id"], json.dumps({"marker": marker}, separators=(",", ":"))),
                )
                admission_state_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO execution_admission_entry (
                        state_id, execution_id, status, queue_order, enqueued_at, activated_at,
                        created, updated
                    )
                    VALUES
                        (
                            %s, %s, 'active', 1,
                            NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '20 seconds',
                            NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '20 seconds'
                        )
                    """,
                    (
                        admission_state_id,
                        stuck_execution_id,
                    ),
                )

                cur.execute(
                    """
                    INSERT INTO work_queue_item (
                        queue, queue_ref, item_key, status, payload, metadata,
                        enqueue_source, leased_execution, lease_expires_at, created, updated
                    )
                    VALUES (
                        %s, %s, %s, 'leased', %s::jsonb, '{}'::jsonb,
                        'e2e', %s, NOW() - INTERVAL '10 seconds',
                        NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '20 seconds'
                    )
                    """,
                    (
                        ids["queue_id"],
                        ids["queue_ref"],
                        f"{marker}-leased",
                        f'{{"marker":"{marker}"}}',
                        stuck_execution_id,
                    ),
                )

                cur.execute(
                    """
                    INSERT INTO work_queue_dispatch (
                        queue, queue_ref, execution, status, leased_item_count, created, updated
                    )
                    VALUES (
                        %s, %s, %s, 'leased', 1,
                        NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '20 seconds'
                    )
                    """,
                    (ids["queue_id"], ids["queue_ref"], stuck_execution_id),
                )

                cur.execute(
                    """
                    DELETE FROM event
                    WHERE trigger_ref = 'core.alert'
                      AND payload->>'correlation_id' = ANY(%s)
                    """,
                    ([execution_correlation, item_correlation, dispatch_correlation],),
                )
                before_execution_alerts = _alert_count(cur, execution_correlation)
                before_item_alerts = _alert_count(cur, item_correlation)
                before_dispatch_alerts = _alert_count(cur, dispatch_correlation)

                _configure_runtime_retention(cur, enabled_targets=set(), enabled=False)
                conn.commit()
                seed_committed = True

            maintenance_config = {
                "enabled": True,
                "artifact_cleanup_enabled": True,
                "artifact_cleanup_batch_size": 10,
                "monitoring_enabled": True,
                "corrective_actions_enabled": True,
                "stuck_execution_seconds": 5,
                "execution_remediation_seconds": 5,
                "execution_reschedule_grace_seconds": 1,
                "stuck_queue_seconds": 5,
                "queue_remediation_seconds": 5,
                "admission_remediation_seconds": 5,
                "retention_lag_alert_seconds": 5,
                "alert_limit_per_cycle": 10,
                "alert_cooldown_seconds": 1,
            }
            deadline = time.monotonic() + 60
            capture_phase("before-monitoring")
            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                artifacts_dir=artifacts_dir,
                maintenance={
                    **maintenance_config,
                    "artifact_cleanup_enabled": False,
                    "corrective_actions_enabled": False,
                },
            )
            process = _start_supervisor(config_path)

            def monitoring_state_is_correct() -> bool:
                with _connect()[0] as check_conn, check_conn.cursor() as cur:
                    assert _alert_count(cur, execution_correlation) > before_execution_alerts
                    assert _alert_count(cur, item_correlation) > before_item_alerts
                    assert _alert_count(cur, dispatch_correlation) > before_dispatch_alerts
                    assert _count(cur, "execution", "id = %s AND status = 'canceling'", (stuck_execution_id,)) == 1
                    assert _count(cur, "work_queue_dispatch", "execution = %s AND status = 'leased'", (stuck_execution_id,)) == 1
                    assert _count(cur, "work_queue_item", "payload->>'marker' = %s AND status = 'leased'", (marker,)) == 1
                return True

            # Monitoring must observe the leased rows before correction releases
            # them. Both real supervisor phases share one observation deadline.
            _wait_for_supervisor(
                process, monitoring_state_is_correct,
                timeout=max(0, deadline - time.monotonic()),
            )
            _stop_supervisor(process)
            capture_phase("alerts-observed")

            with conn.cursor() as cur:
                cur.execute(
                    """
                    INSERT INTO workflow_definition (
                        ref, pack, pack_ref, label, version, definition
                    )
                    VALUES (%s, %s, %s, %s, '1.0.0', %s::jsonb)
                    RETURNING id
                    """,
                    (
                        f"{ids['pack_ref']}.{marker}.workflow",
                        ids["pack_id"],
                        ids["pack_ref"],
                        f"{marker} Workflow",
                        '{"tasks": {}}',
                    ),
                )
                workflow_def_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO execution (
                        action, action_ref, status, config, workflow_def, created, updated
                    )
                    VALUES (
                        %s, %s, 'completed', %s::jsonb, %s,
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"workflow-terminal-parent"}}',
                        workflow_def_id,
                    ),
                )
                terminal_parent_execution_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO workflow_execution (
                        execution, workflow_def, task_graph, status, created, updated
                    )
                    VALUES (
                        %s, %s, %s::jsonb, 'running',
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    """,
                    (terminal_parent_execution_id, workflow_def_id, '{"tasks": {}}'),
                )
                cur.execute(
                    """
                    INSERT INTO execution (
                        action, action_ref, status, config, workflow_def, created, updated
                    )
                    VALUES (
                        %s, %s, 'running', %s::jsonb, %s,
                        NOW() - INTERVAL '20 seconds',
                        clock_timestamp()
                    )
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"workflow-stale-parent"}}',
                        workflow_def_id,
                    ),
                )
                stale_parent_execution_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO workflow_execution (
                        execution, workflow_def, task_graph, status, created, updated
                    )
                    VALUES (
                        %s, %s, %s::jsonb, 'running',
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    """,
                    (stale_parent_execution_id, workflow_def_id, '{"tasks": {}}'),
                )
                cur.execute(
                    """
                    INSERT INTO execution (
                        action, action_ref, parent, status, config, workflow_task, created, updated
                    )
                    VALUES (
                        %s, %s, %s, 'failed', %s::jsonb, %s::jsonb,
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        stale_parent_execution_id,
                        f'{{"marker":"{marker}","kind":"workflow-failed-child"}}',
                        '{"task_name": "child"}',
                    ),
                )
                # Seed the fresh successor last. NOW() would reuse the timestamp
                # from before workflow setup in this transaction.
                cur.execute(
                    """
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    VALUES (%s, %s, 'requested', %s::jsonb,
                            clock_timestamp(), clock_timestamp())
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"admission-queued"}}',
                    ),
                )
                queued_execution_id = cur.fetchone()[0]
                cur.execute(
                    """
                    INSERT INTO execution_admission_entry (
                        state_id, execution_id, status, queue_order, enqueued_at,
                        created, updated
                    )
                    VALUES (%s, %s, 'queued', 2, clock_timestamp(),
                            clock_timestamp(), clock_timestamp())
                    """,
                    (admission_state_id, queued_execution_id),
                )
                cur.execute(
                    """UPDATE execution_admission_state
                       SET next_queue_order = 3, total_enqueued = 2 WHERE id = %s""",
                    (admission_state_id,),
                )
                conn.commit()

            capture_phase("before-correction")
            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                artifacts_dir=artifacts_dir,
                maintenance=maintenance_config,
            )
            process = _start_supervisor(config_path)

            def maintenance_state_is_correct() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        cur.execute(
                            """SELECT id, status, created, updated FROM execution
                               WHERE id = ANY(%s) ORDER BY id""",
                            ([stuck_execution_id, queued_execution_id,
                              terminal_parent_execution_id, stale_parent_execution_id],),
                        )
                        executions = cur.fetchall()
                        cur.execute(
                            """SELECT action_id, group_key, max_concurrent, next_queue_order,
                                      total_enqueued, total_completed, created, updated
                               FROM execution_admission_state WHERE id = %s""",
                            (admission_state_id,),
                        )
                        admission_state = cur.fetchall()
                        cur.execute(
                            """SELECT execution_id, status, queue_order, enqueued_at,
                                      activated_at, created, updated
                               FROM execution_admission_entry WHERE state_id = %s
                               ORDER BY queue_order""",
                            (admission_state_id,),
                        )
                        _capture_json("admission-remediation-cycle", {
                            "executions": executions,
                            "state": admission_state,
                            "entries": cur.fetchall(),
                            "supervisor_stopped": process.poll() is not None,
                        })
                        assert _count(cur, "artifact_version", "artifact = %s", (artifact_id,)) == 0
                        assert _count(cur, "artifact", "id = %s", (artifact_id,)) == 0
                        assert not artifact_file.exists()
                        assert (
                            _count(
                                cur,
                                "execution",
                                "id = %s AND status = 'cancelled'",
                                (stuck_execution_id,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "work_queue_dispatch",
                                "execution = %s AND status = 'cancelled'",
                                (stuck_execution_id,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "work_queue_item",
                                "leased_execution IS NULL AND payload->>'marker' = %s AND status = 'failed'",
                                (marker,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "execution_admission_entry",
                                "execution_id = %s",
                                (stuck_execution_id,),
                            )
                            == 0
                        )
                        assert (
                            _count(
                                cur,
                                "execution_admission_entry",
                                "execution_id = %s AND status = 'active'",
                                (queued_execution_id,),
                            )
                            == 1
                        )
                        assert _alert_count(cur, execution_correlation) > before_execution_alerts
                        assert _alert_count(cur, item_correlation) > before_item_alerts
                        assert _alert_count(cur, dispatch_correlation) > before_dispatch_alerts
                        assert _alert_count(cur, f"supervisor:corrective:execution:{stuck_execution_id}") >= 1
                        assert (
                            _count(
                                cur,
                                "workflow_execution",
                                "execution = %s AND status = 'completed'",
                                (terminal_parent_execution_id,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "execution",
                                "id = %s AND status = 'failed'",
                                (stale_parent_execution_id,),
                            )
                            == 1
                        )
                        assert (
                            _count(
                                cur,
                                "workflow_execution",
                                "execution = %s AND status = 'failed'",
                                (stale_parent_execution_id,),
                            )
                            == 1
                        )
                        assert (
                            _alert_count(
                                cur,
                                "supervisor:corrective:workflow_execution:stale_state",
                            )
                            >= 1
                        )
                        assert (
                            _count(
                                cur,
                                "audit_event",
                                """
                                event_type = 'maintenance.artifact.cleanup_completed'
                                AND actor_login = 'attune-supervisor'
                                AND details->>'service_name' = 'attune-supervisor-e2e'
                                """,
                            )
                            >= 1
                        )
                        assert (
                            _count(
                                cur,
                                "audit_event",
                                """
                                event_type = 'maintenance.corrective_action.applied'
                                AND actor_login = 'attune-supervisor'
                                AND details->>'service_name' = 'attune-supervisor-e2e'
                                """,
                            )
                            >= 1
                        )
                return True

            # The alerts are already recorded. Stop after the corrective cycle
            # before a later cycle abandons the undispatched requested successor.
            _wait_for_log(
                process, "Supervisor maintenance cycle finished",
                timeout=max(0, deadline - time.monotonic()),
            )
            _stop_supervisor(process)
            capture_phase("after-correction")
            assert maintenance_state_is_correct()
        finally:
            if process is not None:
                _stop_supervisor(process)
            conn.rollback()
            if seed_committed:
                capture_phase("before-cleanup")
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_runs_later_targets_and_lag_monitoring_after_batch_budget(self, tmp_path):
        marker = f"retention-lag-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None
        correlation_id = "supervisor:retention-lag:executions"

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                ids = _seed_foundation(cur, marker)
                cur.execute(
                    """
                    INSERT INTO trigger (ref, pack, pack_ref, label, param_schema, out_schema)
                    SELECT 'core.alert', %s, %s, 'Core Alert', '{}'::jsonb, '{}'::jsonb
                    WHERE NOT EXISTS (SELECT 1 FROM trigger WHERE ref = 'core.alert')
                    """,
                    (ids["pack_id"], ids["pack_ref"]),
                )
                cur.execute(
                    """
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    SELECT %s, %s, 'completed', %s::jsonb,
                           NOW() - INTERVAL '2 days', NOW() - INTERVAL '2 days'
                    FROM generate_series(1, 5)
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"retention-lag-candidate"}}',
                    ),
                )
                execution_ids = [row[0] for row in cur.fetchall()]
                cur.execute(
                    """
                    INSERT INTO notification (channel, entity_type, entity, activity, content, created)
                    VALUES ('e2e', 'execution', %s, 'completed', %s::jsonb,
                            NOW() - INTERVAL '2 days') RETURNING id
                    """,
                    (str(execution_ids[0]), json.dumps({"marker": marker})),
                )
                notification_id = cur.fetchone()[0]
                before_alerts = _alert_count(cur, correlation_id)
                _configure_runtime_retention(
                    cur,
                    enabled_targets={"executions", "notifications"},
                    max_age_seconds=86400,
                    batch_size=1,
                    max_batches_per_target=2,
                    check_interval_seconds=3600,
                )
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                maintenance={
                    "enabled": True,
                    "artifact_cleanup_enabled": False,
                    "monitoring_enabled": True,
                    "corrective_actions_enabled": False,
                    "retention_lag_alert_seconds": 1,
                    "alert_limit_per_cycle": 10,
                    "alert_cooldown_seconds": 1,
                },
            )
            process = _start_supervisor(config_path)

            def lag_alert_is_written() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        assert _count(cur, "execution", "id = ANY(%s)", (execution_ids,)) == 3
                        assert _count(cur, "notification", "id = %s", (notification_id,)) == 0
                        assert _alert_count(cur, correlation_id) > before_alerts
                        assert _retention_audit_count(cur, "executions", dry_run=False) >= 1
                return True

            _wait_for_supervisor(process, lag_alert_is_written)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_detects_dirty_shutdown_on_boot(self, tmp_path):
        marker = f"dirty-shutdown-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                cur.execute(
                    """
                    INSERT INTO supervisor_run (
                        id, service_name, instance_id, started_at, heartbeat_at,
                        clean_shutdown, meta
                    )
                    VALUES (
                        %s, 'attune-supervisor-e2e', %s,
                        NOW() - INTERVAL '1 hour', NOW() - INTERVAL '1 hour',
                        FALSE, %s::jsonb
                    )
                    """,
                    (
                        f"{marker}-previous",
                        f"{marker}-previous-instance",
                        f'{{"marker":"{marker}"}}',
                    ),
                )
                _configure_runtime_retention(cur, enabled_targets=set(), enabled=False)
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                maintenance={
                    "enabled": True,
                    "artifact_cleanup_enabled": False,
                    "monitoring_enabled": False,
                    "corrective_actions_enabled": False,
                },
            )
            process = _start_supervisor(config_path)
            output = _wait_for_log(process, "Dirty supervisor shutdown detected")
            assert "startup recovery checks" in output

            def supervisor_run_is_recorded() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        assert (
                            _count(
                                cur,
                                "supervisor_run",
                                """
                                service_name = 'attune-supervisor-e2e'
                                AND id <> %s
                                AND clean_shutdown = FALSE
                                """,
                                (f"{marker}-previous",),
                            )
                            >= 1
                        )
                return True

            _wait_for_supervisor(process, supervisor_run_is_recorded)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_marks_run_clean_on_graceful_shutdown(self, tmp_path):
        marker = f"clean-shutdown-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None
        run_id: str | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                _configure_runtime_retention(cur, enabled_targets=set(), enabled=False)
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                maintenance={
                    "enabled": True,
                    "artifact_cleanup_enabled": False,
                    "monitoring_enabled": False,
                    "corrective_actions_enabled": False,
                },
            )
            process = _start_supervisor(config_path)

            def run_row_exists() -> bool:
                nonlocal run_id
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        cur.execute(
                            """
                            SELECT id
                            FROM supervisor_run
                            WHERE service_name = 'attune-supervisor-e2e'
                              AND clean_shutdown = FALSE
                              AND stopped_at IS NULL
                            ORDER BY started_at DESC
                            LIMIT 1
                            """
                        )
                        row = cur.fetchone()
                        assert row is not None
                        run_id = row[0]
                return True

            _wait_for_supervisor(process, run_row_exists)
            _stop_supervisor(process)
            process = None
            assert run_id is not None

            with _connect()[0] as check_conn:
                with check_conn.cursor() as cur:
                    assert (
                        _count(
                            cur,
                            "supervisor_run",
                            """
                            id = %s
                            AND clean_shutdown = TRUE
                            AND stopped_at IS NOT NULL
                            AND stop_reason = 'graceful_shutdown'
                            """,
                            (run_id,),
                        )
                        == 1
                    )
        finally:
            if process is not None:
                _stop_supervisor(process)
            if run_id is not None:
                with _connect()[0] as cleanup_conn:
                    with cleanup_conn.cursor() as cur:
                        cur.execute("DELETE FROM supervisor_run WHERE id = %s", (run_id,))
                    cleanup_conn.commit()
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_dry_run_leaves_candidates_untouched(self, tmp_path):
        marker = f"retention-dry-run-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                ids = _seed_foundation(cur, marker)
                cur.execute(
                    """
                    INSERT INTO execution (
                        action, action_ref, status, config, created, updated
                    )
                    VALUES (
                        %s, %s, 'completed', %s::jsonb,
                        NOW() - INTERVAL '10 seconds',
                        NOW() - INTERVAL '10 seconds'
                    )
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"dry-run"}}',
                    ),
                )
                execution_id = cur.fetchone()[0]
                _configure_runtime_retention(
                    cur,
                    enabled_targets={"executions"},
                    dry_run=True,
                )
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
            )
            process = _start_supervisor(config_path)

            def dry_run_audit_is_written() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        assert _count(cur, "execution", "id = %s", (execution_id,)) == 1
                        assert _retention_audit_count(cur, "executions", dry_run=True) >= 1
                return True

            _wait_for_supervisor(process, dry_run_audit_is_written)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_deletes_expired_event_history_and_audit_rows(self, tmp_path):
        marker = f"retention-rows-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                ids = _seed_foundation(cur, marker)
                old = "NOW() - INTERVAL '2 days'"

                cur.execute(
                    f"""
                    INSERT INTO event (
                        trigger, trigger_ref, config, payload, created, rule, rule_ref
                    )
                    SELECT %s, %s, %s::jsonb, %s::jsonb, {old}, %s, %s
                    FROM generate_series(1, 5)
                    """,
                    (
                        ids["trigger_id"],
                        ids["trigger_ref"],
                        f'{{"marker":"{marker}"}}',
                        f'{{"marker":"{marker}"}}',
                        ids["rule_id"],
                        ids["rule_ref"],
                    ),
                )
                cur.execute(
                    f"""
                    INSERT INTO execution_history (
                        time, operation, entity_id, entity_ref, changed_fields, new_values
                    )
                    SELECT {old}, 'INSERT', 9000001, %s, ARRAY['status'], %s::jsonb
                    FROM generate_series(1, 5)
                    """,
                    (f"{marker}-execution-history", f'{{"marker":"{marker}"}}'),
                )
                cur.execute(
                    f"""
                    INSERT INTO worker_history (
                        time, operation, entity_id, entity_ref, changed_fields, new_values
                    )
                    SELECT {old}, 'INSERT', 9000002, %s, ARRAY['status'], %s::jsonb
                    FROM generate_series(1, 5)
                    """,
                    (f"{marker}-worker-history", f'{{"marker":"{marker}"}}'),
                )
                cur.execute(
                    f"""
                    INSERT INTO sensor_process_history (
                        time, operation, entity_id, entity_ref, worker_name,
                        changed_fields, new_values
                    )
                    SELECT {old}, 'INSERT', 9000003, %s, %s, ARRAY['status'], %s::jsonb
                    FROM generate_series(1, 5)
                    """,
                    (
                        f"{marker}-sensor-process-history",
                        f"{marker}-worker",
                        f'{{"marker":"{marker}"}}',
                    ),
                )
                cur.execute(
                    f"""
                    INSERT INTO audit_event (
                        created, category, event_type, outcome, details
                    )
                    SELECT {old}, 'api', 'e2e.retention', 'success', %s::jsonb
                    FROM generate_series(1, 5)
                    """,
                    (f'{{"marker":"{marker}"}}',),
                )
                _configure_runtime_retention(
                    cur,
                    enabled_targets={
                        "events",
                        "execution_history",
                        "worker_history",
                        "sensor_process_history",
                        "audit_events",
                    },
                    batch_size=2,
                    max_batches_per_target=3,
                    check_interval_seconds=3600,
                    max_age_seconds=86400,
                )
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
            )
            process = _start_supervisor(config_path)

            def expired_rows_are_gone() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
                        assert _count(cur, "event", "payload->>'marker' = %s", (marker,)) == 0
                        assert (
                            _count(
                                cur,
                                "execution_history",
                                "entity_ref = %s",
                                (f"{marker}-execution-history",),
                            )
                            == 0
                        )
                        assert (
                            _count(
                                cur,
                                "worker_history",
                                "entity_ref = %s",
                                (f"{marker}-worker-history",),
                            )
                            == 0
                        )
                        assert (
                            _count(
                                cur,
                                "sensor_process_history",
                                "entity_ref = %s",
                                (f"{marker}-sensor-process-history",),
                            )
                            == 0
                        )
                        assert (
                            _count(cur, "audit_event", "details->>'marker' = %s", (marker,))
                            == 0
                        )
                        assert _retention_audit_count(cur, "events", dry_run=False) >= 1
                        for target in [
                            "events", "execution_history", "worker_history",
                            "sensor_process_history", "audit_events",
                        ]:
                            cur.execute(
                                """
                                SELECT details->>'deleted'
                                FROM audit_event
                                WHERE event_type = 'maintenance.retention.target_completed'
                                  AND details->>'service_name' = 'attune-supervisor-e2e'
                                  AND resource_ref = %s
                                ORDER BY created DESC LIMIT 1
                                """,
                                (target,),
                            )
                            row = cur.fetchone()
                            assert row is not None and int(row[0]) == 5
                return True

            _wait_for_supervisor(process, expired_rows_are_gone)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)
