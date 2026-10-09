"""Pure runner tests. No Docker daemon, Rust compilation or database required."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SCRIPT = Path(__file__).with_name("verify-postgresql-native-install.py")
spec = importlib.util.spec_from_file_location("native_install", SCRIPT)
install = importlib.util.module_from_spec(spec)
spec.loader.exec_module(install)


class ContractDatabase:
    def __init__(self):
        self.responses = {}
        self.statements = []

    def sql(self, text):
        self.statements.append(text)
        for fragment, response in self.responses.items():
            if fragment in text:
                return response
        if "pg_extension" in text:
            return "f"
        if "pg_get_partkeydef" in text:
            if "cache_entry" in text:
                return "LIST (generation)"
            return 'RANGE ("time")' if "execution_history" in text else "RANGE (created)"
        if "pg_get_expr(relpartbound" in text:
            return "DEFAULT"
        if "pg_get_serial_sequence" in text:
            table = "event" if "'event'" in text else "cache_entry" if "'cache_entry'" in text else "audit_event"
            return f"attune.{table}_id_seq"
        if "min(lower_bound) BETWEEN" in text:
            return "t"
        if "a.atttypid='int8'" in text:
            return "t"
        if "count(*) FROM cache_deployment_physical_byte_usage" in text:
            return "1"
        if "count(*) FROM cache_entry_statistics_state" in text:
            return "1"
        if "count(*)" in text:
            return "0"
        return ""

    def json(self, text):
        self.statements.append(text)
        if "prosecdef" in text:
            return {name: False for name in install.CACHE_FUNCTIONS}
        return {t: list(range(8)) for t in install.PARENTS}


def contract_catalog():
    relations = [{"name": t, "kind": "p", "owner": "native_owner", "columns": []} for t in install.PARENTS]
    relations += [{"name": t, "kind": "r"} for t in ("execution", "enforcement", "worker_history", "sensor_process_history")]
    relations += [{"name": t, "kind": "v", "columns": []} for t in install.ORACLES]
    relations += [{"name": "cache_entry", "kind": "p", "owner": "native_owner"}]
    return {"relations": relations,
            "constraints": [{"table": t, "type": "p", "definition": "PRIMARY KEY (id, created)"} for t in ("event", "audit_event")]
                           + [{"table": "cache_entry", "type": "p", "definition": "PRIMARY KEY (generation, id)"}],
            "indexes": [{"table": t, "name": name, "valid": True} for t, names in (install.PARENT_INDEXES | {"cache_entry": install.CACHE_INDEXES}).items() for name in names]}


class RunnerTests(unittest.TestCase):
    def test_distributed_config_is_in_the_rust_test_compile_context(self):
        recipe = (SCRIPT.parent.parent / "docker/Dockerfile.rust-tests").read_text()
        compile_inputs = recipe.split("cargo test --workspace", 1)[0]
        self.assertIn(
            "COPY docker/distributable/config.docker.yaml ./docker/distributable/config.docker.yaml",
            compile_inputs,
        )

    def test_distributed_services_use_the_migrated_schema(self):
        compose = (SCRIPT.parent.parent / "docker/distributable/docker-compose.yaml").read_text()
        services = dict(re.findall(r"^  ([\w-]+):\n(.*?)(?=^  [\w-]+:|\Z)", compose, re.MULTILINE | re.DOTALL))
        configured = {
            name: re.findall(r"^      ATTUNE__DATABASE__SCHEMA: (\S+)$", body, re.MULTILINE)
            for name, body in services.items()
            if "ATTUNE__DATABASE__URL:" in body
        }
        self.assertIn("supervisor", configured)
        for name, schemas in configured.items():
            with self.subTest(service=name):
                self.assertEqual(schemas, ["attune"])

    def test_current_inputs_not_a_committed_baseline(self):
        files, support, manifest = install.snapshot_inputs()
        self.assertGreater(len(files), 3)
        self.assertEqual(set(files), set(manifest["migration_sha384"]))
        self.assertEqual(set(support), set(manifest["support_sha256"]))
        self.assertNotIn("baseline_commit", manifest)
        self.assertNotIn("baseline_sha384", manifest)

    def test_input_hashes_detect_content_edits_and_additions(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "migrations").mkdir()
            for table, (filename, key) in install.PARENTS.items():
                (root / "migrations" / filename).write_text(f"CREATE TABLE {table}(id BIGINT, -- plain ID; comment is not a statement boundary\n{key} TIMESTAMPTZ) PARTITION BY RANGE (\"{key}\");")
            _, support, _ = install.snapshot_inputs()
            for name in support:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("owned fixture")
            _, _, before = install.snapshot_inputs(root)
            self.assertTrue(all(before["fresh_declarations"].values()))
            path = root / "migrations" / next(iter(install.PARENTS.values()))[0]
            path.write_bytes(path.read_bytes() + b"\n-- changed\n")
            _, _, edited = install.snapshot_inputs(root)
            self.assertNotEqual(before["schema_source_sha256"], edited["schema_source_sha256"])
            (root / "migrations/20261007000001_extra.sql").write_text("SELECT 1;")
            _, _, added = install.snapshot_inputs(root)
            self.assertNotEqual(edited["migration_sha384"], added["migration_sha384"])
            path.write_text("CREATE TABLE event(id BIGINT);")
            _, _, heap = install.snapshot_inputs(root)
            self.assertFalse(heap["fresh_declarations"]["event"])

    def test_history_keys_use_the_actual_runner_contract(self):
        manifest = {"migration_sha384": {"20250101000004_example.sql": "abc"}}
        self.assertEqual(install.expected_history(manifest, "sqlx"), {"20250101000004": "abc"})
        self.assertEqual(install.expected_history(manifest, "docker"), {"20250101000004_example.sql": "abc"})

    def test_selected_lineage_refs_are_bigint_without_any_outbound_fk(self):
        db = ContractDatabase()
        result = install.independent_reference_contract(db)
        self.assertEqual(set(result["bigint_without_fk"]), {"execution.parent", "execution.enforcement",
            "workflow_execution.execution", "inquiry.created_by_execution", "cache_generation.created_by_execution"})
        for text in db.statements:
            self.assertIn("a.attnum=ANY(k.conkey)", text)
            self.assertIn("a.atttypid='int8'::regtype", text)
        for response in ("f", ""):
            db.responses = {"a.attname='execution'": response}
            with self.assertRaisesRegex(RuntimeError, "workflow_execution.execution"):
                install.independent_reference_contract(db)

    def test_help_has_noninteractive_examples(self):
        result = subprocess.run(["python3", str(SCRIPT), "--help"], capture_output=True, text=True, check=True)
        for text in ("--dry-run", "--logical-restore", "--versions 16 18", "--output"):
            self.assertIn(text, result.stdout)
        self.assertNotIn("--rows", result.stdout)
        self.assertNotIn("--operator-volume", result.stdout)

    def test_dry_run_neither_writes_nor_loads_docker(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "new"
            with patch.object(install, "load_protocol", side_effect=AssertionError("Docker loaded")), contextlib.redirect_stdout(io.StringIO()) as stdout:
                self.assertEqual(install.main(["--dry-run", "--output", str(output)]), 0)
            self.assertFalse(output.exists())
            evidence = json.loads(stdout.getvalue())
            self.assertTrue(evidence["dry_run"])
            self.assertEqual(evidence["versions_requested"], ["16", "18"])
            self.assertEqual(evidence["performance"], "REPORTING_PERFORMANCE_DEFERRED")
            self.assertFalse(evidence["production_certification"])

    def test_existing_outputs_and_broken_symlinks_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            for name, symlink in (("dir", False), ("broken", True)):
                output = Path(directory) / name
                output.symlink_to(Path(directory) / "missing") if symlink else output.mkdir()
                with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit) as error:
                    install.main(["--dry-run", "--output", str(output)])
                self.assertEqual(error.exception.code, 2)

    def test_duplicate_versions_rejected(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            install.main(["--dry-run", "--versions", "16", "16", "--output", "/tmp/opencode/unused-test-output"])

    def test_partial_matrix_requires_explicit_runner_selection_and_keeps_preliminary_label(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stdout(io.StringIO()) as stdout:
            install.main(["--dry-run", "--preliminary", "--runners", "sqlx", "--output", str(Path(directory)/"new")])
        evidence = json.loads(stdout.getvalue())
        self.assertTrue(evidence["preliminary"])
        self.assertEqual(evidence["versions_requested"], ["16", "18"])
        self.assertEqual(evidence["runners_requested"], ["sqlx"])
        self.assertEqual(install.parser().parse_args(["--output", "/tmp/opencode/unused"]).runners, ["sqlx", "docker"])
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            install.main(["--dry-run", "--runners", "sqlx", "sqlx", "--output", "/tmp/opencode/unused"])

    def test_missing_output_parent_rejected(self):
        with tempfile.TemporaryDirectory() as directory, contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            install.main(["--dry-run", "--output", str(Path(directory) / "missing/new")])

    def test_unready_fresh_declarations_fail_before_resource_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "new"
            manifest = {"fresh_declarations": {"event": False}}
            with patch.object(install, "snapshot_inputs", return_value=({}, {}, manifest)), patch.object(install, "load_protocol", side_effect=AssertionError("Docker loaded")):
                with self.assertRaisesRegex(RuntimeError, "fresh declarations are not ready"):
                    install.main(["--output", str(output)])
            self.assertFalse(output.exists())

    def test_database_adapter_replaces_only_database_and_role(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:16-alpine", "unit-test", {})
        db = install.Database(server, "install_unit", protocol, "native_writer")
        original = server.psql("unit")
        adapted = db.psql("unit")
        self.assertEqual(adapted[adapted.index("-d") + 1], "install_unit")
        self.assertEqual(adapted[adapted.index("-U") + 1], "native_writer")
        self.assertEqual(original[original.index("-d") + 1], "postgres")
        self.assertEqual(db.sessions, server.sessions)
        self.assertIn("ON_ERROR_STOP=1", adapted)

    def test_protocol_docker_run_has_bounded_resources(self):
        with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "", "")) as run:
            protocol = install.load_protocol()
            protocol.command("docker", "run", "-d", "postgres:16-alpine")
        command = run.call_args.args[0]
        self.assertIn("--memory=1g", command)
        self.assertIn("--cpus=2", command)

    def test_cleanup_checks_every_captured_mount_and_removes_anonymous_volumes_with_container(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        server.cid = "owned-id"
        server.container_claimed = server.volume_owned = True
        anonymous = "a" * 64
        removed = set()
        calls = []
        def run(args, **kwargs):
            calls.append(args)
            info = {"Id": server.cid, "Config": {"Labels": {protocol.LABEL: server.run_id}},
                    "Mounts": [{"Type": "volume", "Name": server.volume, "Destination": "/var/lib/postgresql"},
                               {"Type": "volume", "Name": anonymous, "Destination": "/unexpected"}]}
            if args[:2] == ("docker", "rm"):
                self.assertIn("--volumes", args)
                removed.add(anonymous)
            if args[:3] == ("docker", "volume", "rm"):
                removed.add(server.volume)
            if args[:3] == ("docker", "volume", "inspect"):
                if args[-1] in removed:
                    return subprocess.CompletedProcess(args, 1, "", "not found")
                info = {"Labels": {protocol.LABEL: server.run_id}}
            return subprocess.CompletedProcess(args, 0, json.dumps([info]), "")
        with patch.object(subprocess, "run", side_effect=run):
            server.cleanup()
        self.assertEqual(set(server.report["volume_mount_names_checked"]), {server.volume, anonymous})
        self.assertEqual(server.report["remaining_captured_volume_mounts"], [])
        self.assertEqual(server.report["cleanup_errors"], [])
        self.assertFalse(any(args[:3] == ("docker", "volume", "rm") and args[-1] == anonymous for args in calls))

    def test_schema_bootstrap_failure_is_already_owned(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:16-alpine", "unit-test", {})
        owned = []
        with patch.object(protocol, "command"), patch.object(install.Database, "sql", side_effect=RuntimeError("bootstrap")):
            with self.assertRaisesRegex(RuntimeError, "bootstrap"):
                install.create_database(server, protocol, "install_unit", owned)
        self.assertEqual(owned, ["install_unit"])

    def test_unowned_database_name_rejected_before_create(self):
        with self.assertRaisesRegex(RuntimeError, "unsafe owned database"):
            install.create_database(None, None, "users_database", [])

    def test_fingerprint_keeps_duplicate_content_not_physical_identity(self):
        class Capture:
            def json(self, text):
                self.statement = text
                return {}
        db = Capture()
        install.fingerprint(db, "event")
        self.assertIn("ORDER BY h", db.statement)
        self.assertIn("to_jsonb(r)", db.statement)
        for excluded in ("DISTINCT", "ctid", "xmin", "tableoid"):
            self.assertNotIn(excluded, db.statement)
        with self.assertRaisesRegex(RuntimeError, "unsafe fixture identifier"):
            install.fingerprint(db, "event; DROP TABLE event")

    def test_restore_normalizes_only_lossless_constant_cast(self):
        original = "CHECK (x = ANY ((ARRAY['install'::character varying, 'manual'::character varying])::text[]))"
        restored = "CHECK (x = ANY (ARRAY[('install'::character varying)::text, ('manual'::character varying)::text]))"
        self.assertEqual(install.canonical_constraint(original), install.canonical_constraint(restored))
        self.assertNotEqual(install.canonical_constraint(original), install.canonical_constraint(restored.replace("'manual'", "'other'")))
        unsupported = "CHECK (x = ANY ((ARRAY[dynamic_function(),NULL])::text[]))"
        self.assertEqual(install.canonical_constraint(unsupported), unsupported)

    def test_restore_list_omits_only_owned_schema_creation(self):
        toc = "; archive\n6; 2615 2071 SCHEMA - attune native_owner\n42; 1259 2072 TABLE attune cache_entry native_cache_owner\n43; 0 0 ACL - SCHEMA attune native_owner\n"
        filtered = install.restore_archive_list(toc)
        self.assertIn("; 6; 2615 2071 SCHEMA - attune native_owner", filtered)
        self.assertIn("42; 1259 2072 TABLE attune cache_entry native_cache_owner", filtered)
        self.assertIn("43; 0 0 ACL - SCHEMA attune native_owner", filtered)
        for invalid in (toc.replace("SCHEMA - attune", "SCHEMA - unrelated"), toc + "7; 2615 2073 SCHEMA - attune native_owner\n"):
            with self.assertRaisesRegex(RuntimeError, "exactly the expected"):
                install.restore_archive_list(invalid)

    def test_fresh_contract_checks_leaf_indexes_and_default(self):
        db = ContractDatabase()
        with patch.object(install, "schema", return_value=(contract_catalog(), {})):
            result = install.fresh_contract(db)
        self.assertEqual(result["managed_source_rows"], {t: 0 for t in install.PARENTS})
        self.assertTrue(any("native_partition_check(parent" in text for text in db.statements))
        self.assertTrue(any("'_default',NULL,NULL,true" in text for text in db.statements))
        self.assertTrue(any("confrelid IN" in text for text in db.statements))

    def test_contract_rejects_heap_bad_primary_key_invalid_or_missing_indexes(self):
        mutations = (
            lambda catalog: catalog["relations"][0].update(kind="r"),
            lambda catalog: catalog["constraints"][0].update(definition="PRIMARY KEY (id)"),
            lambda catalog: catalog["indexes"][0].update(valid=False),
            lambda catalog: catalog["indexes"].clear(),
            lambda catalog: next(r for r in catalog["relations"] if r["name"] in install.ORACLES).update(kind="m"),
        )
        for mutate in mutations:
            catalog = contract_catalog()
            mutate(catalog)
            with self.subTest(mutate=mutate), patch.object(install, "schema", return_value=(catalog, {})), self.assertRaises(RuntimeError):
                install.fresh_contract(ContractDatabase())

    def test_contract_rejects_extension_rows_default_unowned_sequence_and_fk(self):
        responses = (
            {"pg_extension": "t"}, {"count(*) FROM event;": "1"},
            {"pg_get_expr(relpartbound": "FOR VALUES FROM ('x') TO ('y')"},
            {"pg_get_serial_sequence": ""}, {"confrelid IN": "1"},
        )
        for response in responses:
            db = ContractDatabase()
            db.responses = response
            with self.subTest(response=response), patch.object(install, "schema", return_value=(contract_catalog(), {})), self.assertRaises(RuntimeError):
                install.fresh_contract(db)

    def test_wrong_horizon_rejected(self):
        db = ContractDatabase()
        db.json = lambda _: {t: [0, 1] for t in install.PARENTS}
        with patch.object(install, "schema", return_value=(contract_catalog(), {})), self.assertRaisesRegex(RuntimeError, "offsets 0..7"):
            install.fresh_contract(db)

    def test_grants_keep_event_immutable_and_producer_state_private(self):
        class Capture:
            def sql(self, text):
                self.statement = text
        db = Capture()
        install.provision_writers(db)
        self.assertIn("INSERT(created,trigger_ref,payload),SELECT(id) ON event", db.statement)
        self.assertIn("SELECT(kind,bucket,transaction_origin,xmin)", db.statement)
        self.assertNotIn("UPDATE(payload)", db.statement)
        self.assertNotIn("GRANT SELECT ON native_summary_state", db.statement)

    def test_sqlx_driver_uses_real_runtime_migrator(self):
        driver = (install.ROOT / "crates/common/examples/verify_native_migrations.rs").read_text()
        self.assertIn("Migrator::new", driver)
        self.assertIn("migrator.run", driver)
        self.assertIn("connection.close().await", driver)
        self.assertIn("ATTUNE_VERIFY_DATABASE_URL", driver)
        self.assertNotIn("INSERT INTO _sqlx_migrations", driver)

    def test_cache_contract_rejects_wrong_storage_usage_and_definer(self):
        responses = ({"pg_get_partkeydef('cache_entry'": "RANGE (generation)"},
                     {"inhparent='cache_entry'": "1"},
                     {"count(*) FROM cache_generation;": "1"},
                     {"count(*) FROM cache_deployment_physical_byte_usage": "0"},
                     {"count(*) FROM cache_entry_statistics_state": "0"})
        for response in responses:
            db = ContractDatabase()
            db.responses = response
            with self.subTest(response=response), self.assertRaises(RuntimeError):
                install.fresh_cache_contract(db, contract_catalog())
        db = ContractDatabase()
        db.json = lambda _: {name: True for name in install.CACHE_FUNCTIONS}
        with self.assertRaisesRegex(RuntimeError, "SECURITY DEFINER"):
            install.fresh_cache_contract(db, contract_catalog())

    def test_cache_contract_rejects_bad_key_and_missing_parent_index(self):
        for change in ("key", "index", "owner"):
            catalog = contract_catalog()
            if change == "key":
                catalog["constraints"][-1]["definition"] = "PRIMARY KEY (id)"
            elif change == "owner":
                catalog["relations"][-1]["owner"] = "native_api"
            else:
                catalog["indexes"] = [i for i in catalog["indexes"] if i["table"] != "cache_entry"]
            with self.subTest(change=change), self.assertRaises(RuntimeError):
                install.fresh_cache_contract(ContractDatabase(), catalog)

    def test_cache_driver_uses_service_login_and_keeps_failure_logs(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {"port": "127.0.0.1:12345"})
        db = install.Database(server, "install_unit", protocol, "native_api")
        args = install.parser().parse_args(["--output", "/tmp/opencode/unused"])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 1, "", "role denied")) as run:
                with self.assertRaisesRegex(RuntimeError, "cache repository cache_stage failed"):
                    install.cache_driver(db, args, output, "cache_stage", "--cache-stage")
            self.assertEqual(run.call_args.kwargs["env"]["ATTUNE_VERIFY_DATABASE_URL"], "postgresql://native_api@127.0.0.1:12345/install_unit")
            self.assertEqual((output / "install_unit.cache_stage.stderr.log").read_text(), "role denied")

    def test_cache_driver_exercises_repositories_and_closes_pool(self):
        driver = (install.ROOT / "crates/common/examples/verify_native_migrations.rs").read_text()
        for text in ("CacheNamespaceRepository::create_api", "CacheGenerationRepository::create_or_get",
                     "CacheIngestRepository::insert_chunk", "CacheGenerationRepository::fail",
                     "CacheStorageRepository::observe", "CacheStorageRepository::refresh_statistics",
                     "CacheGenerationRepository::drop_if_cleanup_eligible", "pg_has_role(current_user,c.relowner,'USAGE')", "pool.close().await"):
            self.assertIn(text, driver)
        self.assertNotIn('sqlx::query("SET ROLE', driver)

    def test_cache_roles_limit_owner_membership_to_entry_storage(self):
        class Capture:
            def sql(self, text):
                self.statement = text
        db = Capture()
        install.provision_cache_roles(db)
        self.assertIn("ALTER TABLE cache_entry OWNER TO native_cache_owner", db.statement)
        self.assertIn("GRANT USAGE,CREATE ON SCHEMA attune TO native_cache_owner", db.statement)
        self.assertIn("cache_entry_statistics_state TO native_api", db.statement)
        self.assertIn("CREATE TABLE cache_role_sentinel", db.statement)
        self.assertNotIn("ALTER SCHEMA", db.statement)
        self.assertNotIn("ALL TABLES", db.statement)
        self.assertNotIn("GRANT native_owner", db.statement)

    def test_cache_creation_rollback_uses_two_integer_admission_namespace(self):
        source = SCRIPT.read_text()
        self.assertIn("pg_advisory_xact_lock(7821101, 0)", source)
        self.assertNotIn("pg_advisory_xact_lock(7821101)", source)

    def test_reclaim_rejects_nonzero_accounting_or_remaining_leaf(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        db = install.Database(server, "install_unit", protocol)
        before = {"entries": 23}
        zero = {"deployment": 0, "owner": 0, "entries": 0, "records": 0, "generations": None}
        result = {"reclaimed": {"outcome": "dropped", "records": 2, "bytes": 23}}
        for usage, rows, message in ((zero | {"deployment": 1}, "0", "accounting"), (zero, "1", "partition")):
            with patch.object(install, "cache_usage", side_effect=[before, usage]), patch.object(install, "cache_driver", return_value=result), patch.object(db, "sql", return_value=rows):
                with self.assertRaisesRegex(RuntimeError, message):
                    install.reclaim_cache(db, None, None, 1, "unit")

    def test_actual_docker_runner_and_logs_not_mock_history(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        db = install.Database(server, "install_unit", protocol)
        args = install.parser().parse_args(["--output", "/tmp/opencode/unused"])
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            with patch.object(subprocess, "run", return_value=subprocess.CompletedProcess([], 0, "runner output", "")) as run:
                install.migrate(db, "docker", output / "migrations", args, output, "install")
            command = run.call_args.args[0]
            self.assertIn("STANDARD_INDEX_SEEDER=/bin/true", command)
            self.assertTrue(command[-1].endswith("/run-migrations.sh"))
            self.assertEqual((output / "install_unit.install.stdout.log").read_text(), "runner output")
            self.assertTrue((output / "install_unit.install.stderr.log").exists())

    def test_case_failure_closes_sessions_and_drops_only_claimed_database(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        class Session:
            closed = False
            def close(self):
                self.closed = True
        session = Session()
        server.sessions.append(session)
        report = {}
        args = install.parser().parse_args(["--output", "/tmp/opencode/unused"])
        def create(server, protocol, name, owned):
            owned.append(name)
            return install.Database(server, name, protocol)
        with patch.object(install, "create_database", side_effect=create), patch.object(install, "migrate", side_effect=RuntimeError("install failure")), patch.object(protocol, "command") as command:
            with self.assertRaisesRegex(RuntimeError, "install failure"):
                install.verify_case(server, protocol, "docker", args, Path("/tmp/opencode/unused"), {}, report)
        self.assertTrue(session.closed)
        self.assertEqual(report["database_cleanup_errors"], [])
        self.assertEqual(len(report["databases"]), 1)
        self.assertEqual(command.call_count, 1)
        self.assertIn("DROP DATABASE " + report["databases"][0], command.call_args.kwargs["input"])

    def test_server_cleanup_never_removes_changed_ownership(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        server.container_claimed = True
        server.volume_owned = True
        calls = []
        def command(*args, **kwargs):
            calls.append(args)
            info = {"Id": "unowned", "Config": {"Labels": {protocol.LABEL: "someone-else"}}, "Labels": {protocol.LABEL: "someone-else"}}
            return subprocess.CompletedProcess(args, 0, json.dumps([info]), "")
        with patch.object(protocol, "command", side_effect=command), self.assertRaisesRegex(RuntimeError, "ownership changed"):
            server.cleanup()
        self.assertFalse(any("rm" in call for call in calls))
        self.assertEqual(len(server.report["cleanup_errors"]), 2)

    def test_resource_proof_filters_only_the_current_run_label(self):
        protocol = install.load_protocol()
        with patch.object(protocol, "command", return_value=subprocess.CompletedProcess([], 0, "", "")) as command:
            self.assertEqual(install.remaining_resources(protocol, "install-unit"), {"containers": [], "volumes": []})
        self.assertEqual(command.call_count, 2)
        for call in command.call_args_list:
            self.assertIn("label=" + protocol.LABEL + "=install-unit", call.args)
            self.assertNotIn("rm", call.args)

    def test_cleanup_failures_remain_visible_and_other_cleanup_still_runs(self):
        protocol = install.load_protocol()
        server = protocol.Server("postgres:18-alpine", "unit-test", {})
        class Session:
            def close(self):
                raise RuntimeError("session close failed")
        server.sessions.append(Session())
        args = install.parser().parse_args(["--output", "/tmp/opencode/unused"])
        report = {}
        def create(server, protocol, name, owned):
            owned.append(name)
            return install.Database(server, name, protocol)
        with patch.object(install, "create_database", side_effect=create), patch.object(install, "migrate", side_effect=RuntimeError("install failed")), patch.object(protocol, "command", side_effect=RuntimeError("drop failed")) as command:
            with self.assertRaisesRegex(RuntimeError, "owned database cleanup failed"):
                install.verify_case(server, protocol, "docker", args, Path("/tmp/opencode/unused"), {}, report)
        self.assertEqual(command.call_count, 1)
        self.assertEqual(report["database_cleanup_errors"], ["session close failed", "drop failed"])

    def test_sqlx_state_does_not_assume_the_docker_bootstrap_marker(self):
        catalog = {"relations": []}
        class Capture:
            def sql(self, text):
                return "f"
            def json(self, text):
                return ["event"] if "jsonb_agg(relname" in text else {}
        with patch.object(install, "schema", return_value=(catalog, {})), patch.object(install, "fingerprint", return_value={"rows": 0}) as fingerprint:
            result = install.state(Capture())
        self.assertEqual(set(result["raw"]), {"event"})
        self.assertEqual(fingerprint.call_count, 1)


if __name__ == "__main__":
    unittest.main()
