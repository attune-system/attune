"""
API Tests: Supervisor Retention

Validates that attune-supervisor applies short runtime retention windows and
keeps protected in-flight rows.
"""

from __future__ import annotations

import json
import os
import select
import shlex
import shutil
import subprocess
import time
import uuid
from pathlib import Path

import psycopg
import pytest
import yaml
from psycopg import sql


PROJECT_ROOT = Path(__file__).resolve().parents[3]
ALL_RETENTION_TARGETS = [
    "events",
    "enforcements",
    "executions",
    "execution_history",
    "worker_history",
    "sensor_process_history",
    "audit_events",
    "continuous_aggregates",
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
    cur.execute(
        """
        SELECT enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
        FROM runtime_retention_config
        WHERE id = TRUE
        """
    )
    config = cur.fetchone()
    cur.execute(
        """
        SELECT target, max_age_seconds
        FROM runtime_retention_target_config
        ORDER BY target ASC
        """
    )
    targets = cur.fetchall()
    return {"config": config, "targets": targets}


def _restore_runtime_retention_config(snapshot: dict[str, object] | None):
    if snapshot is None:
        return
    conn, _ = _connect()
    try:
        with conn.cursor() as cur:
            cur.execute("DELETE FROM runtime_retention_target_config")
            cur.execute("DELETE FROM runtime_retention_config")
            config = snapshot["config"]
            if config is not None:
                cur.execute(
                    """
                    INSERT INTO runtime_retention_config (
                        id, enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
                    )
                    VALUES (TRUE, %s, %s, %s, %s, %s)
                    """,
                    config,
                )
            cur.executemany(
                """
                INSERT INTO runtime_retention_target_config (target, max_age_seconds)
                VALUES (%s, %s)
                """,
                snapshot["targets"],
            )
        conn.commit()
    finally:
        conn.close()


def _configure_runtime_retention(
    cur,
    *,
    enabled_targets: set[str],
    max_age_seconds: int = 5,
    dry_run: bool = False,
    enabled: bool = True,
) -> None:
    advisory_lock_key = 7_900_000 + int(uuid.uuid4().hex[:5], 16)
    cur.execute(
        """
        INSERT INTO runtime_retention_config (
            id, enabled, check_interval_seconds, batch_size, dry_run, advisory_lock_key
        )
        VALUES (TRUE, %s, 1, 500, %s, %s)
        ON CONFLICT (id) DO UPDATE SET
            enabled = EXCLUDED.enabled,
            check_interval_seconds = EXCLUDED.check_interval_seconds,
            batch_size = EXCLUDED.batch_size,
            dry_run = EXCLUDED.dry_run,
            advisory_lock_key = EXCLUDED.advisory_lock_key
        """,
        (enabled, dry_run, advisory_lock_key),
    )
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


def _start_supervisor(config_path: Path) -> subprocess.Popen:
    command = [*_supervisor_command(), "--config", str(config_path), "--log-level", "info"]
    env = {**os.environ, "RUST_LOG": "info", "ATTUNE_CONFIG": str(config_path)}
    return subprocess.Popen(
        command,
        cwd=PROJECT_ROOT,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )


def _stop_supervisor(process: subprocess.Popen) -> str:
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)

    output = ""
    if process.stdout is not None:
        output = process.stdout.read()
    return output


def _wait_for_supervisor(process: subprocess.Popen, predicate, *, timeout: int = 60):
    deadline = time.time() + timeout
    last_error: Exception | None = None

    while time.time() < deadline:
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
    assert process.stdout is not None
    deadline = time.time() + timeout
    output: list[str] = []

    while time.time() < deadline:
        if process.poll() is not None:
            output.append(process.stdout.read())
            raise AssertionError(
                f"attune-supervisor exited before logging {needle!r}:\n{''.join(output)}"
            )

        ready, _, _ = select.select([process.stdout], [], [], 0.5)
        if not ready:
            continue

        line = process.stdout.readline()
        output.append(line)
        if needle in line:
            return "".join(output)

    output.append(_stop_supervisor(process))
    raise TimeoutError(f"Timed out waiting for {needle!r}.\nSupervisor output:\n{''.join(output)}")


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
                    INSERT INTO inquiry (execution, prompt, status, response, created, updated)
                    VALUES
                        (%s, %s, 'responded', %s::jsonb, {old}, {old}),
                        (%s, %s, 'pending', NULL, {old}, {old})
                    """,
                    (
                        responded_inquiry_execution_id,
                        f"{marker} old responded",
                        '{"ok": true}',
                        pending_inquiry_execution_id,
                        f"{marker} old pending",
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
        artifacts_dir = tmp_path / "artifacts"
        artifacts_dir.mkdir()

        execution_correlation = "supervisor:stuck-runtime:execution:canceling"
        item_correlation = "supervisor:stuck-runtime:work_queue_item:leased"
        dispatch_correlation = "supervisor:stuck-runtime:work_queue_dispatch:leased"

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
                        NOW() - INTERVAL '10 seconds',
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
                    INSERT INTO execution (action, action_ref, status, config, created, updated)
                    VALUES (
                        %s, %s, 'requested', %s::jsonb,
                        NOW() - INTERVAL '1 second',
                        NOW() - INTERVAL '1 second'
                    )
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
                    INSERT INTO execution_admission_state (
                        action_id, group_key, max_concurrent, created, updated
                    )
                    VALUES (
                        %s, %s, 1,
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    RETURNING id
                    """,
                    (ids["action_id"], f"{marker}-remediation"),
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
                        ),
                        (
                            %s, %s, 'queued', 2,
                            NOW() - INTERVAL '20 seconds', NULL,
                            NOW() - INTERVAL '20 seconds', NOW() - INTERVAL '20 seconds'
                        )
                    """,
                    (
                        admission_state_id,
                        stuck_execution_id,
                        admission_state_id,
                        queued_execution_id,
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
                        NOW()
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

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
                artifacts_dir=artifacts_dir,
                maintenance={
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
                },
            )
            process = _start_supervisor(config_path)

            def maintenance_state_is_correct() -> bool:
                with _connect()[0] as check_conn:
                    with check_conn.cursor() as cur:
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

            _wait_for_supervisor(process, maintenance_state_is_correct)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)

    def test_supervisor_emits_retention_lag_alerts(self, tmp_path):
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
                    VALUES (
                        %s, %s, 'completed', %s::jsonb,
                        NOW() - INTERVAL '20 seconds',
                        NOW() - INTERVAL '20 seconds'
                    )
                    RETURNING id
                    """,
                    (
                        ids["action_id"],
                        ids["action_ref"],
                        f'{{"marker":"{marker}","kind":"retention-lag-candidate"}}',
                    ),
                )
                execution_id = cur.fetchone()[0]
                before_alerts = _alert_count(cur, correlation_id)
                _configure_runtime_retention(
                    cur,
                    enabled_targets={"executions"},
                    max_age_seconds=5,
                    dry_run=True,
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
                        assert _count(cur, "execution", "id = %s", (execution_id,)) == 1
                        assert _alert_count(cur, correlation_id) > before_alerts
                        assert _retention_audit_count(cur, "executions", dry_run=True) >= 1
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

    def test_supervisor_drops_hypertable_chunks_with_short_retention(self, tmp_path):
        marker = f"retention-hypertable-{_uid()}"
        conn, schema = _connect()
        process: subprocess.Popen | None = None
        retention_snapshot: dict[str, object] | None = None

        try:
            with conn.cursor() as cur:
                retention_snapshot = _snapshot_runtime_retention_config(cur)
                ids = _seed_foundation(cur, marker)
                old_chunk = "NOW() - INTERVAL '40 days'"

                cur.execute(
                    f"""
                    INSERT INTO event (
                        trigger, trigger_ref, config, payload, created, rule, rule_ref
                    )
                    VALUES (%s, %s, %s::jsonb, %s::jsonb, {old_chunk}, %s, %s)
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
                    VALUES ({old_chunk}, 'INSERT', 9000001, %s, ARRAY['status'], %s::jsonb)
                    """,
                    (f"{marker}-execution-history", f'{{"marker":"{marker}"}}'),
                )
                cur.execute(
                    f"""
                    INSERT INTO worker_history (
                        time, operation, entity_id, entity_ref, changed_fields, new_values
                    )
                    VALUES ({old_chunk}, 'INSERT', 9000002, %s, ARRAY['status'], %s::jsonb)
                    """,
                    (f"{marker}-worker-history", f'{{"marker":"{marker}"}}'),
                )
                cur.execute(
                    f"""
                    INSERT INTO sensor_process_history (
                        time, operation, entity_id, entity_ref, worker_name,
                        changed_fields, new_values
                    )
                    VALUES ({old_chunk}, 'INSERT', 9000003, %s, %s, ARRAY['status'], %s::jsonb)
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
                    VALUES ({old_chunk}, 'api', 'e2e.retention', 'success', %s::jsonb)
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
                )
                conn.commit()

            config_path = _write_supervisor_config(
                tmp_path,
                schema=schema,
                enabled_targets=set(),
            )
            process = _start_supervisor(config_path)

            def hypertable_rows_are_gone() -> bool:
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
                return True

            _wait_for_supervisor(process, hypertable_rows_are_gone)
        finally:
            if process is not None:
                _stop_supervisor(process)
            _restore_runtime_retention_config(retention_snapshot)
            conn.close()
            _cleanup_marker(marker)
