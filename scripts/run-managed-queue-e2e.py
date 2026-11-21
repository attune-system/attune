#!/usr/bin/env python3
"""Exercise managed and ad hoc queues on owned PostgreSQL and RabbitMQ."""

import argparse
import hashlib
import json
import os
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid
from pathlib import Path
from urllib.parse import parse_qsl, unquote, urlencode, urlsplit, urlunsplit

import psycopg
from argon2 import PasswordHasher
from psycopg import sql
from psycopg.types.json import Jsonb

def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binaries", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    return parser.parse_args()


args = arguments()
os.umask(0o077)
repo = Path(__file__).resolve().parent.parent
run = uuid.uuid4().hex
database = "queue_e2e_" + run
try:
    admin_dsn = os.environ["TEST_DB_ADMIN_URL"]
    broker = os.environ["ATTUNE_TEST_RABBITMQ_URL"]
    parts = urlsplit(admin_dsn)
    broker_parts = urlsplit(broker)
    if (parts.scheme not in {"postgresql", "postgres"}
            or broker_parts.scheme not in {"amqp", "amqps"}
            or not parts.hostname or not broker_parts.hostname):
        raise ValueError
except (KeyError, ValueError):
    raise SystemExit("Set TEST_DB_ADMIN_URL and ATTUNE_TEST_RABBITMQ_URL for owned test services")

binary_root = args.binaries.resolve(strict=True)
service_roles = {"api": 15, "notifier": 16, "executor": 65, "worker": 40}
for service in service_roles:
    if not os.access(binary_root / f"attune-{service}", os.X_OK):
        raise SystemExit(f"Missing executable: attune-{service}")
    if not (binary_root / f"attune-{service}").is_file():
        raise SystemExit(f"Not a regular binary: attune-{service}")

report = Path(tempfile.mkdtemp(prefix="managed-queue-", dir=args.evidence_dir.resolve(strict=True)))
print("Owned evidence:", report, flush=True)
credentials = report / "private"
credentials.mkdir(mode=0o700)
roles, processes = [], {}
secrets_in_test = {admin_dsn, broker}
for password in (parts.password, broker_parts.password):
    if password:
        secrets_in_test.update((password, unquote(password)))
created = False


def local_url(db, user=None, password=None):
    if user is None:
        netloc = parts.netloc
    else:
        netloc = f"{user}:{password}@{parts.netloc.rsplit('@', 1)[-1]}"
    settings = {key: value for key, value in parse_qsl(parts.query)
                if key in {"sslmode", "sslrootcert"}}
    settings["options"] = "-csearch_path=attune,public"
    return urlunsplit(parts._replace(netloc=netloc, path=f"/{db}", query=urlencode(settings)))


def connect(db, user=None, password=None):
    return psycopg.connect(local_url(db, user, password), autocommit=True, connect_timeout=5, client_encoding="UTF8")

def new_login(cluster, role, password=None):
    assert len(role) <= 63 and re.fullmatch(r"[a-z0-9_]+", role)
    assert not cluster.execute("SELECT 1 FROM pg_roles WHERE rolname=%s", (role,)).fetchone()
    command = sql.SQL("CREATE ROLE {} NOLOGIN")
    args = [sql.Identifier(role)]
    if password is not None:
        command = sql.SQL("CREATE ROLE {} LOGIN PASSWORD {}")
        args.append(sql.Literal(password))
        secrets_in_test.add(password)
    cluster.execute(command.format(*args))
    roles.append(role)
    (report / "owned-roles.json").write_text(json.dumps(roles))

def private_json(path, value):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600), "w") as fd:
        json.dump(value, fd)

def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]

def bounded_wait(predicate, description, seconds):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        for service, process in processes.items():
            if process.poll() is not None:
                raise RuntimeError(f"Owned {service} stopped before {description}")
        if predicate():
            return
        time.sleep(.15)
    raise RuntimeError(f"Timeout waiting for {description}")

def create_admin_fixture(db, login, password):
    # Only initial authentication and interpreter prerequisites are test seeds.
    # Releases, queue items, dispatch, outputs, and role observations are all product paths.
    user = db.execute("INSERT INTO identity (login,display_name,password_hash) VALUES (%s,'Queue E2E Operator',%s) RETURNING id",
                      (login, PasswordHasher().hash(password))).fetchone()[0]
    grants = [
        {"resource": resource, "actions": actions.split()}
        for resource, actions in {
            "packs": "read create install configure delete decrypt",
            "actions": "read create execute update delete",
            "permissions": "read manage",
            "keys": "read create update decrypt delete",
            "queues": "read create update delete decrypt",
            "queue_items": "read create update delete decrypt",
            "executions": "read update cancel decrypt",
            "runtimes": "read create update delete",
            "artifacts": "read create update delete decrypt",
            "events": "read create decrypt",
        }.items()
    ]
    permission = db.execute("INSERT INTO permission_set(ref,grants) VALUES('e2e.operator',%s) RETURNING id",
                            (Jsonb(grants),)).fetchone()[0]
    db.execute("INSERT INTO permission_assignment(identity,permset) VALUES(%s,%s)", (user, permission))


def install_owned_profile(cluster, dba, release):
    owner = "queue_e2e_owner_" + run
    operator = "queue_e2e_op_" + run
    operator_password = secrets.token_hex(24)
    role_name = dba.execute("SELECT recorded_rollout_operator_role()::text").fetchone()[0]
    if isinstance(role_name, bytes):
        role_name = role_name.decode("ascii")
    new_login(cluster, role_name)
    new_login(cluster, operator, operator_password)
    dba.execute(sql.SQL("GRANT {} TO {} WITH INHERIT FALSE, SET FALSE").format(sql.Identifier(role_name), sql.Identifier(operator)))
    dba.execute(sql.SQL("GRANT USAGE ON SCHEMA attune TO {}").format(sql.Identifier(operator)))
    dba.execute(sql.SQL("GRANT EXECUTE ON FUNCTION issue_recorded_rollout_receipt(JSONB), consume_recorded_rollout_receipt(JSONB) TO {}").format(sql.Identifier(operator)))
    with dba.transaction():
        dba.execute(sql.SQL("SET LOCAL ROLE {}").format(sql.Identifier(owner)))
        dba.execute((repo / "deployment/rollout-admin/consumer-observations.candidate.sql").read_text())
    epoch, revision = dba.execute("SELECT compatibility_epoch,revision FROM platform_catalog_state WHERE singleton").fetchone()
    rollout = str(uuid.uuid4())
    snapshot = {"contract_version": 1, "catalog_epoch": epoch, "catalog_revision": revision,
                "rollout_generation": rollout, "release_digest": release}
    members = [{"instance": str(uuid.uuid4()), "boot": str(uuid.uuid4()), "role": role, "required_mask": mask}
               for role, mask in service_roles.items()]
    with connect(database, operator, operator_password) as actor:
        receipt = actor.execute("SELECT issue_recorded_rollout_receipt(%s)", (Jsonb({
            "snapshot": snapshot, "members": members,
        }),)).fetchone()[0]
    assert dba.execute("SELECT generation,revision FROM recorded_rollout_active WHERE singleton").fetchone() == (None, 0)
    generation_ddl = {"recorded_consumer_binding", "recorded_consumer_observation"}
    # Keep all rollout state and authority controls outside the runtime DML grants.
    runtime_tables = [
        row[0]
        for row in dba.execute("SELECT tablename FROM pg_tables WHERE schemaname='attune' ORDER BY tablename")
        if not row[0].startswith("recorded_rollout_") and row[0] not in generation_ddl
        and not row[0].startswith("_sqlx") and row[0] != "platform_catalog_state"
    ]
    customer_roles = {}
    for member in members:
        name = member["role"]
        login, password = f"queue_e2e_{name}_{run}", secrets.token_hex(24)
        new_login(cluster, login, password)
        ident = sql.Identifier(login)
        dba.execute(sql.SQL("GRANT USAGE ON SCHEMA attune TO {}").format(ident))
        dba.execute(sql.SQL("GRANT SELECT ON platform_catalog_state TO {}").format(ident))
        dba.execute(sql.SQL("GRANT SELECT,INSERT,UPDATE,DELETE ON {} TO {}").format(
            sql.SQL(",").join(sql.Identifier(table) for table in runtime_tables), ident))
        dba.execute(sql.SQL("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA attune TO {}").format(ident))
        dba.execute(sql.SQL("GRANT EXECUTE ON FUNCTION report_recorded_consumer(JSONB,UUID,UUID,UUID,INTEGER,BIGINT,BOOLEAN) TO {}").format(ident))
        if name in ("api", "executor"):
            dba.execute(sql.SQL("GRANT EXECUTE ON FUNCTION current_recorded_consumer_evidence(UUID) TO {}").format(ident))
        dba.execute("SELECT bind_recorded_consumer(%s,%s,%s,%s::NAME)",
                    (rollout, member["instance"], member["boot"], login))
        with connect(database, login, password) as runtime:
            expect = runtime.execute("SELECT session_user::TEXT,current_user::TEXT,has_schema_privilege('attune','CREATE'),has_table_privilege('recorded_rollout_active','UPDATE'),has_function_privilege('issue_recorded_rollout_receipt(jsonb)','EXECUTE')").fetchone()
            assert expect == (login, login, False, False, False)
        customer_roles[name] = (login, password, member)
    with connect(database, operator, operator_password) as actor:
        actor.execute("SELECT consume_recorded_rollout_receipt(%s)", (Jsonb(receipt),))
    active = dba.execute("SELECT generation,revision FROM recorded_rollout_active WHERE singleton").fetchone()
    assert active == (receipt["target"]["row_id"], 1)
    assert dba.execute("SELECT count(*) FROM recorded_consumer_observation").fetchone() == (0,)
    login, password, _ = customer_roles["api"]
    with connect(database, login, password) as runtime:
        unobserved = runtime.execute("SELECT current_recorded_consumer_evidence(NULL)").fetchone()[0]
        assert unobserved["expected"]["snapshot"] == snapshot and not unobserved["observations"]
    return snapshot, customer_roles

def stop_processes():
    for process in processes.values():
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
    forced = []
    unexpected = []
    for name, process in processes.items():
        try:
            process.wait(timeout=20)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            forced.append(name)
        if process.returncode != 0:
            unexpected.append(name)
    processes.clear()
    if forced or unexpected:
        raise RuntimeError(f"Owned service shutdown failed: forced={forced}, unexpected={unexpected}")

def clean_logs():
    for path in report.glob("*.log"):
        text = path.read_text(errors="replace")
        for secret in sorted(secrets_in_test, key=len, reverse=True):
            text = text.replace(secret, "[fixture secret]")
        path.write_text(text)


def run_http_tests(env, filename, *tests):
    command = [sys.executable, "-m", "pytest", "-c", "tests/pytest.ini", *tests,
               "--basetemp", str(credentials / "pytest"),
               "-o", f"cache_dir={credentials / 'pytest-cache'}", "-n", "0", "-q"]
    with (report / filename).open("w") as out:
        result = subprocess.run(command, cwd=repo, env=env, stdout=out,
                                stderr=subprocess.STDOUT, timeout=240, check=False)
    if result.returncode:
        raise RuntimeError(f"Owned HTTP tests failed. See {filename} in the evidence directory")


def interrupted(_signal, _frame):
    raise KeyboardInterrupt

signal.signal(signal.SIGTERM, interrupted)

with psycopg.connect(admin_dsn, autocommit=True, connect_timeout=5, client_encoding="UTF8") as cluster:
    try:
        owner = "queue_e2e_owner_" + run
        migrator = "queue_e2e_migrate_" + run
        migrator_password = secrets.token_hex(24)
        new_login(cluster, owner)
        new_login(cluster, migrator, migrator_password)
        cluster.execute(sql.SQL("GRANT {} TO {} WITH INHERIT FALSE, SET TRUE").format(
            sql.Identifier(owner), sql.Identifier(migrator)))
        cluster.execute(sql.SQL("CREATE DATABASE {} TEMPLATE template0 OWNER {}").format(
            sql.Identifier(database), sql.Identifier(owner)))
        created = True
        with connect(database) as dba:
            # Reconcile the fixture's platform catalog using the real
            # one-shot API migrator before staging the exact generation.
            config = credentials / "migrator.json"
            dba.execute(sql.SQL("ALTER ROLE {} IN DATABASE {} SET role TO {}").format(
                sql.Identifier(migrator), sql.Identifier(database), sql.Identifier(owner)))
            migrate_url = local_url(database, migrator, migrator_password)
            with psycopg.connect(migrate_url, client_encoding="UTF8") as actor:
                identity = actor.execute("SELECT session_user::TEXT,current_user::TEXT").fetchone()
                assert identity == (migrator, owner), {"migration_login_matches": identity[0] == migrator,
                                                       "migration_owner_matches": identity[1] == owner}
            private_json(config, {"environment": "owned-queue-test",
                                  "database": {"url": migrate_url, "schema": "attune"}})
            def history():
                name = dba.execute("SELECT to_regclass('_sqlx_migrations')::TEXT").fetchone()[0]
                assert name
                return dba.execute("SELECT version,checksum,success FROM _sqlx_migrations ORDER BY version").fetchall()
            assert dba.execute("SELECT to_regclass('_sqlx_migrations') IS NULL").fetchone() == (True,)
            with (report / "migrator.log").open("w") as out:
                migrator = subprocess.run([str(binary_root / "attune-api"), "--migrate"], cwd=credentials,
                    env={"PATH": "/usr/local/bin:/usr/bin:/bin", "ATTUNE_CONFIG": str(config)},
                    stdout=out, stderr=subprocess.STDOUT, timeout=45, check=False)
            assert migrator.returncode == 0, "Owned catalog bootstrap failed"
            expected = [(int(migration.name.split("_")[0]), hashlib.sha384(migration.read_bytes()).digest(), True)
                        for migration in sorted((repo / "migrations").glob("*.sql"))]
            assert history() == expected, "Fresh SQLx ledger differs from the checked-in migrations"
            hashes = []
            for role in service_roles:
                with (binary_root / f"attune-{role}").open("rb") as executable:
                    hashes.append(hashlib.file_digest(executable, "sha256").digest())
            release = hashlib.sha256(b"".join(hashes)).hexdigest()
            snapshot, profile = install_owned_profile(cluster, dba, release)
            app_login = "e2e-queue-operator@attune.local"
            app_password = "Fixture-" + secrets.token_hex(24) + "!"
            secrets_in_test.add(app_password)
            create_admin_fixture(dba, app_login, app_password)
            print("Owned fixture: exact staged and activated receipt, dedicated DB sessions, no observations", flush=True)
            api_port, notifier_port = free_port(), free_port()
            jwt_key, encryption_key = secrets.token_hex(32), secrets.token_hex(32)
            secrets_in_test.update((jwt_key, encryption_key))
            paths = {key: str(report / key) for key in ("packs", "envs", "artifacts", "blobs", "logs")}
            for path in paths.values():
                Path(path).mkdir(mode=0o700)
            for service, (login, password, member) in profile.items():
                private = credentials / service
                private.mkdir(mode=0o700)
                cfg = {"environment": "owned-queue-test",
                       "database": {"url": local_url(database, login, password), "schema": "attune", "max_connections": 12},
                       "message_queue": {"url": broker, "enable_dlq": False},
                       "server": {"host": "127.0.0.1", "port": api_port, "shutdown_grace_period": 8},
                       "notifier": {"host": "127.0.0.1", "port": notifier_port},
                       "security": {"jwt_secret": jwt_key, "encryption_key": encryption_key,
                                    "enable_auth": True, "allow_self_registration": True},
                       "packs_base_dir": paths["packs"], "runtime_envs_dir": paths["envs"],
                       "artifacts_dir": paths["artifacts"], "storage": {"provider": "filesystem", "root": paths["blobs"]},
                       "packs": {"transport": "volume"}, "artifacts": {"transport": "volume"},
                       "worker": {"name": f"queue-e2e-{run}", "heartbeat_interval": 5, "max_concurrent_tasks": 2},
                       "log": {"level": "info", "console": True}}
                private_json(private / "config.json", cfg)
                private_json(private / "identity.json", {
                    "snapshot": snapshot, "instance": member["instance"], "boot": member["boot"],
                })
                process_env = {"PATH": "/usr/local/bin:/usr/bin:/bin",
                               "ATTUNE_CONFIG": str(private / "config.json"),
                               "ATTUNE_API_URL": f"http://127.0.0.1:{api_port}",
                               "ATTUNE_RECORDED_CONSUMER_IDENTITY_FILE": str(private / "identity.json")}
                with (report / f"{service}.log").open("w") as out:
                    processes[service] = subprocess.Popen([str(binary_root / f"attune-{service}")],
                                                         cwd=private, env=process_env,
                                                         stdout=out, stderr=subprocess.STDOUT)
            def live_roles():
                samples = dba.execute("SELECT count(*) FROM recorded_consumer_observation WHERE observed > clock_timestamp() - INTERVAL '15 seconds'").fetchone()[0]
                return samples == len(service_roles)
            try:
                bounded_wait(live_roles, "four independently published service observations", 60)
            except RuntimeError:
                state = dba.execute("SELECT b.role, o.observed IS NOT NULL FROM recorded_consumer_binding b LEFT JOIN recorded_consumer_observation o ON o.rollout=b.rollout AND o.instance=b.instance ORDER BY b.role").fetchall()
                print("Reported roles:", state, flush=True)
                raise
            print("All four roles published current observations", flush=True)
            test_env = {"PATH": "/usr/local/bin:/usr/bin:/bin",
                        "ATTUNE_API_URL": f"http://127.0.0.1:{api_port}",
                        "ATTUNE_API_BASE_URL": f"http://127.0.0.1:{api_port}",
                        "TEST_USER_LOGIN": app_login, "TEST_USER_PASSWORD": app_password}
            test_env["ATTUNE_TEST_RUN_ID"] = "qe" + run[:18]
            managed = "tests/e2e/api/test_worker_native_completion.py::"
            run_http_tests(test_env, "queue-http-e2e.log",
                           managed + "test_worker_managed_queue_completion",
                           managed + "test_managed_queue_retries_until_retained_limit",
                           managed + "test_cross_pack_queue_batches_enqueue_only_contributors",
                           managed + "test_managed_queue_native_workflow_acknowledges_item",
                           "tests/e2e/api/test_work_queue_api.py::TestWorkQueueApi::test_enabled_queue_dispatches_item_and_applies_ack")
            print("Owned queue HTTP and worker test passed", flush=True)

            outage_test = "tests/e2e/api/queue_consumer_outage.py::test_missing_consumer_rejects_without_accepting"

            def consumer_withdrawn(role):
                return dba.execute("""SELECT NOT EXISTS (
                    SELECT 1 FROM recorded_consumer_binding AS binding
                    JOIN recorded_consumer_observation USING (rollout, instance)
                    WHERE binding.role=%s AND observed > clock_timestamp() - INTERVAL '30 seconds'
                )""", (role,)).fetchone()[0]

            for role, sequence in (
                ("api", "work_queue_item_configuration_input_id_seq"),
                ("executor", "work_queue_dispatch_id_seq"),
            ):
                sequence = sql.Identifier("attune", sequence)
                login = sql.Identifier(profile[role][0])
                dba.execute(sql.SQL("REVOKE USAGE ON SEQUENCE {} FROM {}").format(sequence, login))
                bounded_wait(lambda: consumer_withdrawn(role), f"{role} sequence privilege withdrawal", 15)
                run_http_tests(test_env, f"queue-{role}-sequence-outage-e2e.log", outage_test)
                dba.execute(sql.SQL("GRANT USAGE ON SEQUENCE {} TO {}").format(sequence, login))
                bounded_wait(live_roles, f"{role} sequence privilege recovery", 15)
                print(f"Missing {role} sequence privilege denied admission until restored", flush=True)

            notifier = processes["notifier"]
            notifier.send_signal(signal.SIGINT)
            if notifier.wait(timeout=20) != 0:
                raise RuntimeError("Owned notifier shutdown failed")
            del processes["notifier"]

            bounded_wait(lambda: consumer_withdrawn("notifier"), "notifier readiness withdrawal", 35)
            run_http_tests(test_env, "queue-outage-e2e.log", outage_test)
            print("Missing notifier denied single and bulk enqueue with no accepted work", flush=True)
    finally:
        try:
            stop_processes()
        finally:
            try:
                clean_logs()
            finally:
                if created:
                    bounded_wait(
                        lambda: cluster.execute(
                            "SELECT 1 FROM pg_stat_activity WHERE datname=%s", (database,),
                        ).fetchone() is None,
                        "owned database sessions to close after service shutdown",
                        10,
                    )
                    cluster.execute(sql.SQL("DROP DATABASE {}").format(sql.Identifier(database)))
                for role in reversed(roles):
                    cluster.execute(sql.SQL("DROP ROLE {}").format(sql.Identifier(role)))
                assert not cluster.execute("SELECT 1 FROM pg_roles WHERE rolname=ANY(%s)", (roles,)).fetchone()
                assert not cluster.execute("SELECT 1 FROM pg_database WHERE datname=%s", (database,)).fetchone()
                shutil.rmtree(credentials)
                print("Exact owned roles, clone and credential-file directory absent", flush=True)
