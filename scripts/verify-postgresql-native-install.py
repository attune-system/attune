#!/usr/bin/env python3
"""Verify a fresh native schema with real SQLx and Docker histories on PG16/18.

Examples:
  python3 scripts/verify-postgresql-native-install.py --dry-run --output /tmp/opencode/native-install-plan
  python3 scripts/verify-postgresql-native-install.py --versions 16 18 --output /tmp/opencode/native-install
  python3 scripts/verify-postgresql-native-install.py --versions 18 --logical-restore --output /tmp/opencode/native-install-restore

Build the driver after freezing the fresh migration sources:
  SQLX_OFFLINE=true cargo build -p attune-common --example verify_native_migrations

Requires Python 3.10+, Docker and cached postgres:16-alpine/18-alpine images.
Output must be a new directory. No existing database is accepted. There is no
historical baseline, heap conversion, volume profile, or performance benchmark.
"""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
PARENTS = {"event": ("20250101000004_trigger_sensor_event_rule.sql", "created"),
           "execution_history": ("20250101000009_timescaledb_history.sql", "time"),
           "audit_event": ("20250101000013_audit_log.sql", "created")}
PARENT_INDEXES = {
    "event": {"event_pkey", "idx_event_trigger", "idx_event_trigger_ref", "idx_event_source", "idx_event_created",
              "idx_event_trigger_created", "idx_event_trigger_ref_created", "idx_event_source_created", "idx_event_payload_gin", "idx_event_trace_tag"},
    "execution_history": {"idx_execution_history_time", "idx_execution_history_entity", "idx_execution_history_entity_ref",
                          "idx_execution_history_status_changes", "idx_execution_history_changed_fields"},
    "audit_event": {"audit_event_pkey", "idx_audit_event_created", "idx_audit_event_actor", "idx_audit_event_category",
                    "idx_audit_event_event_type", "idx_audit_event_outcome", "idx_audit_event_resource",
                    "idx_audit_event_resource_ref", "idx_audit_event_request", "idx_audit_event_details"},
}
PROBE_MIGRATION = "99991231235959_owned_transaction_probe.sql"
CACHE_INDEXES = {"cache_entry_pkey", "cache_entry_generation_external_id_bytewise_unique"}
CACHE_FUNCTIONS = {"cache_generation_partition_name", "validate_cache_generation_partition",
                   "create_cache_generation_partition", "drop_cleanup_cache_generation",
                   "request_cache_entry_statistics", "bound_cache_iteration_metadata",
                   "preserve_cache_iteration_generation", "release_cache_iteration_metadata"}
INDEPENDENT_REFS = {"execution": ("parent", "enforcement"),
                    "workflow_execution": ("execution",),
                    "inquiry": ("created_by_execution",),
                    "cache_generation": ("created_by_execution",)}
ORACLES = {
    "execution_status_hourly": "SELECT date_trunc('hour',time,'UTC') bucket,entity_ref action_ref,new_values->>'status' new_status,count(*) transition_count FROM execution_history WHERE 'status'=ANY(changed_fields) GROUP BY 1,2,3",
    "execution_throughput_hourly": "SELECT date_trunc('hour',time,'UTC') bucket,entity_ref action_ref,count(*) execution_count FROM execution_history WHERE operation='INSERT' GROUP BY 1,2",
    "event_volume_hourly": "SELECT date_trunc('hour',created,'UTC') bucket,trigger_ref,count(*) event_count FROM event GROUP BY 1,2",
    "worker_status_hourly": "SELECT date_trunc('hour',time,'UTC') bucket,entity_ref worker_name,new_values->>'status' new_status,count(*) transition_count FROM worker_history WHERE 'status'=ANY(changed_fields) GROUP BY 1,2,3",
    "enforcement_volume_hourly": "SELECT date_trunc('hour',created,'UTC') bucket,rule_ref,count(*) enforcement_count FROM enforcement GROUP BY 1,2",
    "execution_volume_hourly": "SELECT date_trunc('hour',created,'UTC') bucket,action_ref,status initial_status,count(*) execution_count FROM execution GROUP BY 1,2,3",
}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def snapshot_inputs(root=ROOT):
    files = {p.name: p.read_bytes() for p in sorted((root / "migrations").glob("*.sql"))}
    require(files and PROBE_MIGRATION not in files, "missing migrations or probe filename collision")
    require(all(re.fullmatch(r"[0-9]{14}_[a-z0-9_]+[.]sql", name) for name in files), "unexpected migration filename")
    declarations = {}
    for table, (filename, key) in PARENTS.items():
        text = files.get(filename, b"").decode()
        # The canonical declarations have line comments containing semicolons.
        # This is a readiness hint, not a SQL parser or the runtime contract check.
        text = re.sub(r"--[^\n]*", "", text)
        declaration = re.search(rf"CREATE TABLE {table}\s*\([\s\S]*?;", text, re.IGNORECASE)
        declarations[table] = bool(declaration and re.search(rf"PARTITION BY RANGE\s*\(\s*\"?{key}\"?\s*\)", declaration[0], re.IGNORECASE))
    support_paths = (
        "docker/run-migrations.sh", "scripts/probe-postgresql-native-maintenance.py",
        "scripts/verify-postgresql-native-install.py", "crates/common/examples/verify_native_migrations.rs",
        "crates/common/src/repositories/cache.rs", "crates/common/src/config.rs", "crates/common/src/models.rs")
    support_paths += tuple(str(p.relative_to(root)) for p in sorted((root / "crates/common/src/repositories/cache").glob("*.rs")))
    support = {name: (root / name).read_bytes() for name in support_paths}
    checksums = {name: hashlib.sha384(data).hexdigest() for name, data in files.items()}
    return files, support, {"migration_sha384": checksums, "schema_source_sha256": digest(canonical(checksums)),
                            "support_sha256": {name: digest(data) for name, data in support.items()},
                            "fresh_declarations": declarations}


def parser():
    cli = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    cli.add_argument("--versions", nargs="+", choices=("16", "18"), default=["16", "18"])
    cli.add_argument("--runners", nargs="+", choices=("sqlx", "docker"), default=["sqlx", "docker"], help="select runners; default is the full four-case matrix")
    cli.add_argument("--preliminary", action="store_true", help="label private snapshot evidence as preliminary, not final source-freeze acceptance")
    cli.add_argument("--output", type=Path, required=True, help="new private evidence directory, never overwritten")
    cli.add_argument("--sqlx-driver", type=Path, default=ROOT / "target/debug/examples/verify_native_migrations")
    cli.add_argument("--dry-run", action="store_true", help="print input hashes/readiness without Docker, writes or compilation")
    cli.add_argument("--logical-restore", action="store_true", help="optional small fresh-schema dump/restore and origin/xmin probe")
    return cli


def load_protocol():
    spec = importlib.util.spec_from_file_location("install_protocol", ROOT / "scripts/probe-postgresql-native-maintenance.py")
    protocol = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(protocol)
    original = protocol.command
    servers = {}
    def bounded(*args, **kwargs):
        if args[:2] == ("docker", "run"):
            args = (*args[:2], "--memory=1g", "--cpus=2", *args[2:])
        if args[:2] == ("docker", "rm"):
            info = json.loads(original("docker", "inspect", args[-1]).stdout)[0]
            run_id = info["Config"]["Labels"].get(protocol.LABEL)
            server = servers.get(run_id)
            require(server is not None and info["Id"] == (server.cid or info["Id"]), "refusing changed container identity")
            args = tuple(a for a in args if a not in ("-v", "--volumes"))
            args = (*args[:2], "--volumes", *args[2:])
        return original(*args, **kwargs)
    protocol.command = bounded
    base = protocol.Server
    class InstallServer(base):
        def __init__(self, image, run_id, report):
            super().__init__(image, run_id, report)
            servers[run_id] = self

        def __enter__(self):
            try:
                super().__enter__()
                info = json.loads(protocol.command("docker", "inspect", self.name).stdout)[0]
                image = json.loads(protocol.command("docker", "image", "inspect", self.image).stdout)[0]
                expected = "/var/lib/postgresql/data" if ":16" in self.image else "/var/lib/postgresql"
                self.report["image_declared_volumes"] = sorted(image["Config"].get("Volumes") or {})
                self.report["mounts_at_start"] = info["Mounts"]
                require(expected in self.report["image_declared_volumes"], "unexpected PostgreSQL image VOLUME root")
                require(any(m["Type"] == "volume" and m["Name"] == self.volume and m["Destination"] == expected
                            for m in info["Mounts"]), "named PostgreSQL volume does not match the image VOLUME root")
                return self
            except BaseException:
                self.cleanup()
                raise

        def cleanup(self):
            mounts = []
            if self.container_claimed:
                inspected = protocol.command("docker", "inspect", self.name, check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    if info["Config"]["Labels"].get(protocol.LABEL) == self.run_id and (not self.cid or info["Id"] == self.cid):
                        mounts = info.get("Mounts", [])
                        self.report["mounts_before_container_removal"] = mounts
                        self.report["removed_container_id"] = info["Id"]
            try:
                super().cleanup()
            finally:
                names = sorted({m["Name"] for m in mounts if m["Type"] == "volume"})
                if names:
                    remaining = [name for name in names if protocol.command("docker", "volume", "inspect", name, check=False).returncode == 0]
                    self.report["volume_mount_names_checked"] = names
                    self.report["remaining_captured_volume_mounts"] = remaining
                    if remaining:
                        error = "captured owned-container volume mounts remain: " + ", ".join(remaining)
                        self.report.setdefault("cleanup_errors", []).append(error)
                        raise RuntimeError(error)
    protocol.Server = InstallServer
    return protocol


def remaining_resources(protocol, run_id):
    label = f"label={protocol.LABEL}={run_id}"
    return {
        "containers": protocol.command("docker", "ps", "-a", "--filter", label, "--format", "{{.ID}} {{.Names}}").stdout.splitlines(),
        "volumes": protocol.command("docker", "volume", "ls", "--filter", label, "--format", "{{.Name}}").stdout.splitlines(),
    }


class Database:
    def __init__(self, server, name, protocol, role="native_owner"):
        self.server, self.name, self.protocol, self.role = server, name, protocol, role
        self.sessions = server.sessions

    def psql(self, name="probe"):
        args = self.server.psql(name)
        args[args.index("-d") + 1] = self.name
        args[args.index("-U") + 1] = self.role
        return args

    def sql(self, text):
        return self.protocol.command(*self.psql(), input="SET search_path TO attune,public; SET TIME ZONE 'UTC';\n" + text, timeout=600).stdout.strip()

    def json(self, text):
        return json.loads(self.sql(text))

    def reject(self, text, state):
        result = self.protocol.command(*self.psql(), input="SET search_path TO attune,public;\n" + text, check=False)
        require(result.returncode and state in result.stderr, f"expected SQLSTATE {state}: {result.stderr}")
        return {"sqlstate": state, "stderr": result.stderr}


def canonical_constraint(definition):
    # pg_restore can fold an array-wide constant varchar->text cast into casts
    # of each literal. Preserve types, literals and predicates; leave NULL and
    # dynamic expressions unchanged.
    literal = r"'(?:[^']|'')*'::character varying"
    def fold(match):
        if not re.fullmatch(literal + "(?:, " + literal + ")*", match[1]):
            return match[0]
        return "ARRAY[" + ", ".join(f"({value})::text" for value in re.findall(literal, match[1])) + "]"
    return re.sub(r"\(ARRAY\[([^\]]*)\]\)::text\[\]", fold, definition)


def schema(db):
    value = db.json("""SELECT jsonb_build_object(
      'schema',(SELECT jsonb_build_object('owner',pg_get_userbyid(n.nspowner),
        'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN x.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(x.grantee) END,
          'grantor',pg_get_userbyid(x.grantor),'privilege',x.privilege_type,'grantable',x.is_grantable) ORDER BY x.grantee,x.privilege_type)
          FROM aclexplode(coalesce(n.nspacl,acldefault('n'::"char",n.nspowner))) x))
        FROM pg_namespace n WHERE n.oid='attune'::regnamespace),
      'relations',(SELECT jsonb_agg(jsonb_build_object('name',c.relname,'kind',c.relkind,
        'owner',pg_get_userbyid(c.relowner),'comment',obj_description(c.oid,'pg_class'),
        'partition_key',CASE WHEN c.relkind='p' THEN pg_get_partkeydef(c.oid) END,
        'partition_bound',pg_get_expr(c.relpartbound,c.oid),
        'partition_parent',(SELECT p.relname FROM pg_inherits h JOIN pg_class p ON p.oid=h.inhparent WHERE h.inhrelid=c.oid),
        'acl',(SELECT jsonb_agg(jsonb_build_object('grantee',CASE WHEN x.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(x.grantee) END,
          'grantor',pg_get_userbyid(x.grantor),'privilege',x.privilege_type,'grantable',x.is_grantable) ORDER BY x.grantee,x.privilege_type)
          FROM aclexplode(coalesce(c.relacl,acldefault(CASE WHEN c.relkind='S' THEN 's'::"char" ELSE 'r'::"char" END,c.relowner))) x),
        'sequence_owner',(SELECT p.relname||'.'||a.attname FROM pg_depend dep JOIN pg_class p ON p.oid=dep.refobjid
          JOIN pg_attribute a ON a.attrelid=p.oid AND a.attnum=dep.refobjsubid WHERE dep.classid='pg_class'::regclass
          AND dep.objid=c.oid AND dep.refclassid='pg_class'::regclass AND dep.deptype='a'),
        'columns',(SELECT jsonb_agg(jsonb_build_object('name',a.attname,'type',format_type(a.atttypid,a.atttypmod),
          'notnull',a.attnotnull,'default',pg_get_expr(d.adbin,d.adrelid),'acl',a.attacl::text,
          'comment',col_description(c.oid,a.attnum)) ORDER BY a.attnum) FROM pg_attribute a LEFT JOIN pg_attrdef d
          ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped),
        'view',CASE WHEN c.relkind='v' THEN pg_get_viewdef(c.oid) END) ORDER BY c.relname)
        FROM pg_class c WHERE c.relnamespace='attune'::regnamespace AND c.relkind IN ('r','p','v','S')),
      'constraints',(SELECT jsonb_agg(jsonb_build_object('table',c.relname,'name',k.conname,'type',k.contype,
        'definition',pg_get_constraintdef(k.oid),'comment',obj_description(k.oid,'pg_constraint')) ORDER BY c.relname,k.conname)
        FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid WHERE c.relnamespace='attune'::regnamespace),
      'indexes',(SELECT jsonb_agg(jsonb_build_object('table',c.relname,'name',i.relname,'definition',pg_get_indexdef(i.oid),
        'valid',x.indisvalid,'unique',x.indisunique,'comment',obj_description(i.oid,'pg_class'),
        'attached_to',(SELECT p.relname FROM pg_inherits h JOIN pg_class p ON p.oid=h.inhparent WHERE h.inhrelid=i.oid)) ORDER BY c.relname,i.relname)
        FROM pg_index x JOIN pg_class i ON i.oid=x.indexrelid JOIN pg_class c ON c.oid=x.indrelid WHERE c.relnamespace='attune'::regnamespace),
      'triggers',(SELECT jsonb_agg(jsonb_build_object('table',c.relname,'name',t.tgname,'definition',pg_get_triggerdef(t.oid),
        'enabled',t.tgenabled,'comment',obj_description(t.oid,'pg_trigger')) ORDER BY c.relname,t.tgname)
        FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid WHERE c.relnamespace='attune'::regnamespace AND NOT t.tgisinternal),
      'functions',(SELECT jsonb_agg(jsonb_build_object('name',p.proname,'arguments',pg_get_function_identity_arguments(p.oid),
        'definition',pg_get_functiondef(p.oid),'owner',pg_get_userbyid(p.proowner),'acl',p.proacl::text) ORDER BY p.proname,pg_get_function_identity_arguments(p.oid))
        FROM pg_proc p WHERE p.pronamespace='attune'::regnamespace));""")
    renderings = {c["table"] + "." + c["name"]: c["definition"] for c in value["constraints"]}
    for constraint in value["constraints"]:
        constraint["definition"] = canonical_constraint(constraint["definition"])
    return value, renderings


def fingerprint(db, table):
    require(re.fullmatch(r"[a-z_][a-z0-9_]*", table), "unsafe fixture identifier")
    return db.json(f"""SELECT jsonb_build_object('rows',count(*),'sha256',encode(sha256(convert_to(
      coalesce(string_agg(h,'' ORDER BY h),''),'UTF8')),'hex')) FROM
      (SELECT encode(sha256(convert_to(to_jsonb(r)::text,'UTF8')),'hex') h FROM {table} r) hashes;""")


def state(db):
    catalog, renderings = schema(db)
    tables = db.json("SELECT jsonb_agg(relname ORDER BY relname) FROM pg_class WHERE relnamespace='attune'::regnamespace AND relkind IN ('r','p') AND NOT relispartition;")
    if db.sql("SELECT to_regclass('_attune_migration_runner') IS NOT NULL;") == "t":
        tables.append("_attune_migration_runner")
    seq = {r["name"]: db.json(f"SELECT jsonb_build_object('last_value',last_value,'is_called',is_called) FROM {r['name']};")
           for r in catalog["relations"] if r["kind"] == "S"}
    return {"schema": catalog, "schema_sha256": digest(canonical(catalog)), "constraint_renderings": renderings,
            "raw": {t: fingerprint(db, t) for t in tables}, "sequences": seq,
            "identity": db.json("SELECT jsonb_object_agg(relname,oid::bigint) FROM pg_class WHERE relnamespace='attune'::regnamespace AND relkind IN ('r','p','v','S');")}


def fresh_contract(db):
    require(db.sql("SELECT EXISTS(SELECT 1 FROM pg_extension WHERE extname='timescaledb');") == "f", "unexpected TimescaleDB extension")
    catalog, _ = schema(db)
    relations = {r["name"]: r for r in catalog["relations"]}
    for table, (_, key) in PARENTS.items():
        require(relations[table]["kind"] == "p" and relations[table]["owner"] == "native_owner", f"not a native owner-managed parent: {table}")
        rendered_key = '"time"' if key == "time" else key
        require(db.sql(f"SELECT pg_get_partkeydef('{table}'::regclass);") == f"RANGE ({rendered_key})", f"wrong partition key: {table}")
        require(db.sql(f"SELECT count(*) FROM {table};") == "0", f"fresh managed source unexpectedly populated: {table}")
        require(db.sql(f"SELECT pg_get_expr(relpartbound,oid) FROM pg_class WHERE oid='{table}_default'::regclass;") == "DEFAULT", f"missing DEFAULT: {table}")
    for table in ("execution", "enforcement", "worker_history", "sensor_process_history"):
        require(relations[table]["kind"] == "r", f"ordinary table changed: {table}")
    for table in ("event", "audit_event"):
        primary = [c for c in catalog["constraints"] if c["table"] == table and c["type"] == "p"]
        require(len(primary) == 1 and primary[0]["definition"] == "PRIMARY KEY (id, created)", f"wrong composite PK: {table}")
        require(db.sql(f"SELECT pg_get_serial_sequence('{table}','id');").endswith(f"{table}_id_seq"), f"unowned sequence: {table}")
    require(not any(c["table"] in ("execution_history", "worker_history", "sensor_process_history") and c["type"] == "p" for c in catalog["constraints"]), "history acquired a public ID key")
    require(all(i["valid"] for i in catalog["indexes"]), "invalid installed index")
    for table, expected in PARENT_INDEXES.items():
        actual = {i["name"] for i in catalog["indexes"] if i["table"] == table}
        require(expected <= actual, f"missing canonical parent indexes: {table}: {sorted(expected - actual)}")
    for view in ORACLES:
        require(relations[view]["kind"] == "v", f"not an ordinary view: {view}")
    require(db.sql("SELECT count(*) FROM pg_constraint WHERE contype='f' AND confrelid IN ('event'::regclass,'audit_event'::regclass,'execution_history'::regclass);") == "0", "managed rows became FK targets")
    independent_refs = independent_reference_contract(db)
    # Use the install's anchor, not the verifier's current date across midnight.
    require(db.sql("SELECT min(lower_bound) BETWEEN date_trunc('day',now(),'UTC')-interval '1 day' AND date_trunc('day',now(),'UTC') FROM native_partition_registry;") == "t", "partition horizon is not current")
    horizon = db.json("""SELECT jsonb_object_agg(parent,days) FROM (SELECT parent,array_agg(
      (extract(epoch FROM lower_bound-(SELECT min(lower_bound) FROM native_partition_registry))/86400)::int ORDER BY lower_bound) days
      FROM native_partition_registry GROUP BY parent) h;""")
    require(horizon == {t: list(range(8)) for t in PARENTS}, "expected UTC day offsets 0..7 for each parent")
    db.sql("SELECT native_partition_check(parent,partition_name,lower_bound,upper_bound) FROM native_partition_registry;")
    db.sql("SELECT native_partition_check(p,p::text||'_default',NULL,NULL,true) FROM unnest(enum_range(NULL::native_partition_parent)) p;")
    require(db.sql("SELECT count(*) FROM native_summary_invalidation;") == "0", "installation emitted dirty records")
    cache = fresh_cache_contract(db, catalog)
    return {"utc_day_offsets": horizon, "managed_source_rows": {t: 0 for t in PARENTS},
            "cache": cache, "independent_refs": independent_refs,
            "schema_sha256": digest(canonical(catalog)), "view_columns": {v: relations[v]["columns"] for v in ORACLES}}


def independent_reference_contract(db):
    checked = []
    for table, fields in INDEPENDENT_REFS.items():
        for field in fields:
            require(db.sql(f"""SELECT a.atttypid='int8'::regtype AND NOT EXISTS (
              SELECT 1 FROM pg_constraint k WHERE k.conrelid=a.attrelid
                AND k.contype='f' AND a.attnum=ANY(k.conkey))
              FROM pg_attribute a WHERE a.attrelid='{table}'::regclass
                AND a.attname='{field}' AND NOT a.attisdropped;""") == "t",
                    f"independent-retention ref is missing, not BIGINT, or has an FK: {table}.{field}")
            checked.append(f"{table}.{field}")
    return {"bigint_without_fk": checked}


def fresh_cache_contract(db, catalog):
    parent = next(r for r in catalog["relations"] if r["name"] == "cache_entry")
    require(parent["kind"] == "p" and parent["owner"] == "native_owner", "cache_entry is not owner-managed LIST storage")
    require(db.sql("SELECT pg_get_partkeydef('cache_entry'::regclass);") == "LIST (generation)", "wrong cache LIST key")
    require(db.sql("SELECT count(*) FROM pg_inherits WHERE inhparent='cache_entry'::regclass;") == "0", "fresh cache has a DEFAULT or generation leaf")
    primary = [c["definition"] for c in catalog["constraints"] if c["table"] == "cache_entry" and c["type"] == "p"]
    require(primary == ["PRIMARY KEY (generation, id)"], "cache key lost generation identity")
    require(CACHE_INDEXES <= {i["name"] for i in catalog["indexes"] if i["table"] == "cache_entry"}, "cache parent indexes missing")
    require(db.sql("SELECT pg_get_serial_sequence('cache_entry','id');").endswith("cache_entry_id_seq"), "cache ID sequence is not owned")
    for table in ("cache_namespace", "cache_generation", "cache_entry", "cache_ingest_chunk",
                  "cache_generation_entry_usage", "cache_owner_physical_byte_usage"):
        require(db.sql(f"SELECT count(*) FROM {table};") == "0", f"fresh cache unexpectedly populated: {table}")
    require(db.sql("SELECT count(*) FROM cache_deployment_physical_byte_usage WHERE id=1 AND physical_bytes=0;") == "1", "fresh deployment usage is not zero")
    require(db.sql("SELECT count(*) FROM cache_entry_statistics_state WHERE id AND requested_revision=0 AND completed_revision=0 AND partitions_created=0 AND partitions_dropped=0 AND last_analyzed_at IS NULL;") == "1", "fresh cache statistics state is not zero")
    names = ",".join(f"'{name}'" for name in sorted(CACHE_FUNCTIONS))
    functions = db.json(f"SELECT jsonb_object_agg(proname,prosecdef) FROM pg_proc WHERE pronamespace='attune'::regnamespace AND proname IN ({names});")
    require(set(functions) == CACHE_FUNCTIONS and not any(functions.values()), "cache lifecycle functions missing or SECURITY DEFINER")
    return {"partition_key": "LIST (generation)", "initial_partitions": 0, "deployment_bytes": 0,
            "security_invoker_functions": sorted(functions)}


def expected_history(manifest, runner):
    return {n.split("_", 1)[0] if runner == "sqlx" else n: h for n, h in manifest["migration_sha384"].items()}


def history(db, runner):
    query = "SELECT jsonb_object_agg(version::text,encode(checksum,'hex')) FROM _sqlx_migrations WHERE success;" if runner == "sqlx" else "SELECT jsonb_object_agg(filename,checksum_sha384) FROM _migrations;"
    return db.json(query)


def migrate(db, runner, directory, args, output, phase, allow_failure=False):
    if runner == "sqlx":
        env = os.environ.copy()
        port = db.server.report["port"].rsplit(":", 1)[1]
        env["ATTUNE_VERIFY_DATABASE_URL"] = f"postgresql://native_owner@127.0.0.1:{port}/{db.name}"
        command = [str(args.sqlx_driver.resolve()), "--source", str(directory)]
    else:
        env = None
        command = ["docker", "exec", "-e", "DB_HOST=127.0.0.1", "-e", "DB_USER=native_owner", "-e", "DB_PASSWORD=",
                   "-e", f"DB_NAME={db.name}", "-e", f"MIGRATIONS_DIR=/tmp/{db.server.run_id}/{directory.name}",
                   "-e", "STANDARD_INDEX_SEEDER=/bin/true", db.server.name, "/bin/sh", f"/tmp/{db.server.run_id}/run-migrations.sh"]
    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=1800)
    for stream in ("stdout", "stderr"):
        with (output / f"{db.name}.{phase}.{stream}.log").open("x") as log:
            log.write(getattr(result, stream))
    if not allow_failure:
        require(result.returncode == 0, f"{runner} {phase} failed; see {db.name}.{phase} logs")
    return result


def create_database(server, protocol, name, owned):
    require(re.fullmatch(r"install_[a-z0-9_]+", name), "unsafe owned database name")
    protocol.command(*server.psql(), input=f"SET statement_timeout='120s'; CREATE DATABASE {name} OWNER native_owner;", timeout=140)
    owned.append(name)  # Claim before schema bootstrap so partial failures clean up.
    db = Database(server, name, protocol)
    db.sql("CREATE SCHEMA attune AUTHORIZATION native_owner;")
    return db


def provision_writers(db):
    # These are later-provisioned roles, not default grants on every new table.
    db.sql("""GRANT USAGE ON SCHEMA attune TO native_writer,native_observer;
      GRANT SELECT,INSERT,UPDATE ON execution TO native_writer;
      GRANT INSERT ON event,audit_event,execution_history,worker_history TO native_writer;
      GRANT SELECT(id) ON event,audit_event TO native_writer;
      GRANT USAGE ON SEQUENCE event_id_seq,audit_event_id_seq,execution_id_seq TO native_writer;
      GRANT INSERT(created,trigger_ref,payload),SELECT(id) ON event TO native_observer;
      GRANT USAGE ON SEQUENCE event_id_seq TO native_observer;
      GRANT SELECT ON event_volume_hourly TO native_observer;
      GRANT INSERT ON native_summary_invalidation TO native_writer,native_observer;
      GRANT SELECT(kind,bucket,transaction_origin,xmin) ON native_summary_invalidation TO native_writer,native_observer;
      GRANT USAGE ON SEQUENCE native_summary_invalidation_id_seq TO native_writer,native_observer;
      SELECT setval('event_id_seq',4096,true); SELECT setval('audit_event_id_seq',8192,true);""")


def business_probe(db):
    writer = Database(db.server, db.name, db.protocol, "native_writer")
    observer = Database(db.server, db.name, db.protocol, "native_observer")
    before = {t: int(db.sql(f"SELECT count(*) FROM {t};")) for t in PARENTS}
    event = int(writer.sql("INSERT INTO event(created,trigger_ref,payload) VALUES('2021-10-02','install.writer','{\"record\":1}') RETURNING id;"))
    require(event > 4096, "writer rewound event reservation")
    execution = int(writer.sql("INSERT INTO execution(action_ref,config) VALUES('install.writer','{\"record\":1}') RETURNING id;"))
    writer.sql(f"UPDATE execution SET status='running' WHERE id={execution};")
    writer.sql(f"UPDATE execution SET status='completed',result='{{\"payload\":\"installed\"}}' WHERE id={execution};")
    audit = int(writer.sql("INSERT INTO audit_event(category,event_type,outcome) VALUES('auth','install.writer','success') RETURNING id;"))
    require(audit > 8192, "writer rewound audit reservation")
    observed_event = int(observer.sql("INSERT INTO event(created,trigger_ref,payload) VALUES('2021-10-02','install.observer','{\"record\":2}') RETURNING id;"))
    writer.sql("INSERT INTO worker_history(time,operation,entity_id,entity_ref,changed_fields,new_values) VALUES('2021-10-02','UPDATE',-77,NULL,ARRAY['status'],'{\"status\":\"offline\"}');")
    db.sql("INSERT INTO enforcement(rule_ref,trigger_ref,event,payload) VALUES('install.missing','install.writer',-77,'{}');")
    deltas = {t: int(db.sql(f"SELECT count(*) FROM {t};")) - before[t] for t in PARENTS}
    require(deltas == {"event": 2, "execution_history": 3, "audit_event": 4}, "business trigger counts changed")
    require(db.sql(f"SELECT new_values->'result'->>'type' FROM execution_history WHERE entity_id={execution} AND new_values->>'status'='completed';") == "object", "history digest contract")
    require(db.sql(f"SELECT count(*) FROM audit_event WHERE resource_type='execution' AND resource_id={execution};") == "3", "lifecycle audit contract")
    require(db.sql("SELECT count(*) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2021-10-02';") == "2", "writer/column-writer invalidations")
    for view, oracle in ORACLES.items():
        require(db.sql(f"SELECT count(*) FROM ((SELECT * FROM {view} EXCEPT ALL {oracle}) UNION ALL ({oracle} EXCEPT ALL SELECT * FROM {view})) delta;") == "0", f"raw view oracle failed: {view}")
    fk = writer.reject("BEGIN; INSERT INTO event(trigger,trigger_ref) VALUES(-999999,'install.bad-fk'); ROLLBACK;", "23503")
    metadata = writer.json("SELECT jsonb_build_object('visible',count(*),'has_origin',bool_and(transaction_origin IS NOT NULL AND xmin IS NOT NULL)) FROM native_summary_invalidation;")
    require(metadata["visible"] > 0 and metadata["has_origin"], "writer could not read actual ownership metadata")
    observer_metadata = observer.json("SELECT jsonb_build_object('visible',count(*),'has_origin',bool_and(transaction_origin IS NOT NULL AND xmin IS NOT NULL)) FROM native_summary_invalidation;")
    require(observer_metadata == metadata, "column-only producer could not read ownership metadata")
    # A producer's repeated statements should reuse only its own marker. The
    # rolled-back source rows leave no extra fixture data or dirty records.
    origin = writer.json("""BEGIN;
      INSERT INTO event(created,trigger_ref) VALUES('2002-01-01','install.origin');
      INSERT INTO event(created,trigger_ref) VALUES('2002-01-01','install.origin');
      SELECT jsonb_build_object('origin',pg_current_xact_id()::text,'xmin',pg_current_xact_id()::xid::text,'own_markers',count(*))
        FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2002-01-01'
        AND transaction_origin=pg_current_xact_id() AND xmin=pg_current_xact_id()::xid;
      ROLLBACK;""")
    require(origin["own_markers"] == 1, "same-transaction origin/xmin marker reuse failed")
    # Composite IDs are not globally unique, and rollback also removes notices.
    db.sql("BEGIN; INSERT INTO event(id,created,trigger_ref) VALUES(777,'2001-01-01','install.same-id'),(777,'2001-01-02','install.same-id'); ROLLBACK;")
    require(db.sql("SELECT count(*) FROM event WHERE id=777;") == "0", "rolled-back independent-ID probe leaked")
    return {"event_id": event, "observer_event_id": observed_event, "execution_id": execution, "audit_id": audit,
            "raw_deltas": deltas, "outbound_fk": fk, "producer_metadata": metadata, "transaction_origin": origin,
            "six_raw_view_oracles": "passed"}


def maintenance_probe(db):
    maintainer = Database(db.server, db.name, db.protocol, "native_maintainer")
    writer = Database(db.server, db.name, db.protocol, "native_writer")
    denial = writer.reject("SELECT * FROM native_partition_ensure_day('event','2035-01-01',10);", "42501")
    result = maintainer.sql("BEGIN; SELECT outcome FROM native_partition_ensure_day('event','2035-01-01',10); ROLLBACK;")
    require(result == "applied", "non-superuser maintainer could not create a native leaf")
    require(db.sql("SELECT to_regclass('event_p20350101') IS NULL;") == "t", "maintenance rollback leaked a leaf")
    privileges = db.json("""SELECT jsonb_object_agg(rolname,jsonb_build_object('superuser',rolsuper,'createdb',rolcreatedb,'createrole',rolcreaterole,'bypassrls',rolbypassrls))
      FROM pg_roles WHERE rolname IN ('native_owner','native_writer','native_observer','native_maintainer','native_api','native_cache_owner','native_cache_supervisor','native_cache_dml');""")
    require(all(not any(p.values()) for p in privileges.values()), "unexpected privileged role")
    require(db.sql("SELECT has_column_privilege('native_writer','native_summary_invalidation','xmin','SELECT') AND NOT has_table_privilege('native_writer','native_summary_state','SELECT');") == "t", "producer privilege separation")
    return {"roles": privileges, "writer_ddl_denial": denial, "non_superuser_ddl_and_rollback": "passed"}


def cache_driver(db, args, output, phase, *command):
    env = os.environ.copy()
    port = db.server.report["port"].rsplit(":", 1)[1]
    env["ATTUNE_VERIFY_DATABASE_URL"] = f"postgresql://{db.role}@127.0.0.1:{port}/{db.name}"
    result = subprocess.run([str(args.sqlx_driver.resolve()), *command], env=env,
                            capture_output=True, text=True, timeout=300)
    for stream in ("stdout", "stderr"):
        (output / f"{db.name}.{phase}.{stream}.log").write_text(getattr(result, stream))
    require(result.returncode == 0, f"cache repository {phase} failed; see {db.name}.{phase} logs")
    return json.loads(result.stdout)


def cache_usage(db):
    return db.json("""SELECT jsonb_build_object(
      'deployment',(SELECT physical_bytes FROM cache_deployment_physical_byte_usage WHERE id=1),
      'owner',(SELECT physical_bytes FROM cache_owner_physical_byte_usage WHERE owner_type='system' AND owner='system'),
      'entries',(SELECT coalesce(sum(size_bytes),0) FROM cache_entry),
      'records',(SELECT count(*) FROM cache_entry),
      'generations',(SELECT jsonb_agg(jsonb_build_object('id',generation,'records',record_count,'bytes',physical_bytes,'retained_iterations',retained_iterations) ORDER BY generation) FROM cache_generation_entry_usage));""")


def provision_cache_roles(db):
    # Cache DDL authority is not application-schema ownership. Only the cache
    # entry parent/sequence and its future children use this dedicated owner.
    db.sql("""GRANT USAGE,CREATE ON SCHEMA attune TO native_cache_owner;
      ALTER TABLE cache_entry OWNER TO native_cache_owner;
      GRANT SELECT,INSERT,UPDATE ON cache_namespace,cache_generation,cache_ingest_chunk,
        cache_generation_entry_usage,cache_owner_physical_byte_usage TO native_api;
      GRANT SELECT,UPDATE ON cache_deployment_physical_byte_usage,cache_entry_statistics_state TO native_api;
      GRANT USAGE ON SEQUENCE cache_namespace_id_seq,cache_generation_id_seq,cache_ingest_chunk_id_seq TO native_api;
      GRANT SELECT,INSERT,UPDATE,DELETE ON cache_namespace,cache_generation,cache_ingest_chunk,
        cache_generation_entry_usage,cache_owner_physical_byte_usage TO native_cache_supervisor;
      GRANT SELECT,UPDATE ON cache_deployment_physical_byte_usage,cache_entry_statistics_state TO native_cache_supervisor;
      GRANT SELECT ON workflow_cache_iteration,workflow_execution,runtime_retention_config TO native_api,native_cache_supervisor;
      GRANT USAGE ON SCHEMA attune TO native_cache_dml;
      GRANT SELECT,UPDATE ON cache_entry,cache_entry_statistics_state TO native_cache_dml;
      CREATE TABLE cache_role_sentinel(id BIGINT PRIMARY KEY);
      INSERT INTO cache_role_sentinel VALUES(17);""")


def cache_probe(db, args, output):
    api = Database(db.server, db.name, db.protocol, "native_api")
    supervisor = Database(db.server, db.name, db.protocol, "native_cache_supervisor")
    writer = Database(db.server, db.name, db.protocol, "native_writer")
    staged = cache_driver(api, args, output, "cache_stage", "--cache-stage")
    require(staged["roles"]["login"] == staged["roles"]["effective"] == "native_api", "API role was switched instead of using inherited ownership")
    populated, empty = (g["id"] for g in staged["generations"])
    usage = cache_usage(db)
    require(usage["deployment"] == usage["owner"] == usage["entries"] > 0 and usage["records"] == 2, "cache admitted usage is not exact")
    require(usage["generations"] == [{"id": populated, "records": 2, "bytes": usage["entries"], "retained_iterations": 0},
                                      {"id": empty, "records": 0, "bytes": 0, "retained_iterations": 0}], "cache per-generation usage mismatch")
    # A distinct supervisor must DROP an API-created leaf through inherited
    # ownership, not SET ROLE or membership in the API login. This is red before
    # the helper transfers child ownership to the catalog parent owner.
    before_drop = state(db)
    cross_service = supervisor.json(f"BEGIN; SELECT row_to_json(r) FROM drop_cleanup_cache_generation({empty},0) r; ROLLBACK;")
    require(cross_service == {"outcome": "dropped", "records_reclaimed": 0, "bytes_reclaimed": 0}
            and state(db) == before_drop, "cross-service inherited-owner DROP/rollback failed")
    for generation in (populated, empty):
        leaf = f"cache_entry_g_{generation}"
        db.sql(f"SELECT validate_cache_generation_partition({generation});")
        require(db.sql(f"SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='{leaf}'::regclass;") == "native_cache_owner", "API-created leaf has a different owner from supervisor")
        require(db.sql(f"SELECT count(*) FROM pg_index x JOIN pg_inherits h ON h.inhrelid=x.indexrelid JOIN pg_index p ON p.indexrelid=h.inhparent WHERE x.indrelid='{leaf}'::regclass AND p.indrelid='cache_entry'::regclass AND x.indisvalid;") == str(len(CACHE_INDEXES)), "cache leaf indexes are not attached and valid")
    denied = {}
    for label, statement in {
        "parent_insert": f"INSERT INTO cache_entry(generation,external_id,value,size_bytes) VALUES({populated},'denied','{{}}',0);",
        "leaf_insert": f"INSERT INTO cache_entry_g_{populated}(generation,external_id,value,size_bytes) VALUES({populated},'denied','{{}}',0);",
        "usage_update": "UPDATE cache_deployment_physical_byte_usage SET physical_bytes=0;",
        "create_attach": f"SELECT create_cache_generation_partition({populated});",
        "drop": f"SELECT * FROM drop_cleanup_cache_generation({populated},0);",
    }.items():
        denied[label] = writer.reject(statement, "42501")
    require(db.sql(f"SELECT NOT has_table_privilege('native_writer','cache_entry','INSERT') AND NOT has_table_privilege('native_writer','cache_entry_g_{populated}','INSERT') AND NOT has_schema_privilege('native_writer','attune','CREATE');") == "t", "direct cache writer permissions are not restrictive")
    unrelated = {role: Database(db.server, db.name, db.protocol, role).reject("BEGIN; DROP TABLE cache_role_sentinel;", "42501")
                 for role in ("native_api", "native_cache_supervisor")}
    require(db.sql("SELECT NOT pg_has_role('native_api','native_owner','USAGE') AND NOT pg_has_role('native_cache_supervisor','native_owner','USAGE') AND (SELECT id=17 FROM cache_role_sentinel);") == "t", "cache services gained unrelated application ownership")
    before = state(db)
    rolled = supervisor.json(f"BEGIN; SELECT row_to_json(r) FROM drop_cleanup_cache_generation({populated},0) r; ROLLBACK;")
    require(rolled == {"outcome": "dropped", "records_reclaimed": 2, "bytes_reclaimed": usage["entries"]} and state(db) == before, "cache drop rollback changed storage or usage")
    failed = supervisor.reject(f"BEGIN; UPDATE cache_deployment_physical_byte_usage SET physical_bytes=0; SELECT * FROM drop_cleanup_cache_generation({populated},0);", "P0001")
    require("cache deployment usage underflow" in failed["stderr"] and state(db) == before, "failed cache DROP did not roll back storage, metadata and accounting")
    # Exercise the migration helper's transactional DDL rollback separately.
    # The Rust stage command verifies committed repository creation and retry.
    api.sql(f"""BEGIN; SELECT pg_advisory_xact_lock(7821101, 0);
      LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE;
      DO $$ DECLARE g BIGINT; BEGIN
        INSERT INTO cache_generation(namespace,client_refresh_id,expected_chunk_count)
          VALUES({staged['namespace']},'rolled-back-create',0) RETURNING id INTO g;
        PERFORM create_cache_generation_partition(g);
      END $$; ROLLBACK;""")
    after = state(db)
    require(all(after[k] == before[k] for k in ("schema_sha256", "raw", "identity")), "cache create rollback leaked storage, metadata or usage")
    reclaimed = cache_driver(supervisor, args, output, "cache_empty_reclaim", "--cache-reclaim", str(empty))
    require(reclaimed["roles"]["login"] == reclaimed["roles"]["effective"] == "native_cache_supervisor" and reclaimed["reclaimed"] == {"outcome": "dropped", "records": 0, "bytes": 0}, "supervisor could not reclaim zero-byte generation")
    require(db.sql(f"SELECT to_regclass('cache_entry_g_{empty}') IS NULL AND NOT EXISTS(SELECT 1 FROM cache_generation WHERE id={empty}) AND NOT EXISTS(SELECT 1 FROM cache_generation_entry_usage WHERE generation={empty});") == "t", "empty cache reclamation left storage or metadata")
    require(cache_usage(db)["deployment"] == usage["deployment"], "empty reclamation released populated bytes")
    before_statistics = state(db)
    statistics_denial = cache_driver(Database(db.server, db.name, db.protocol, "native_cache_dml"),
                                    args, output, "cache_statistics_denied", "--cache-statistics-denied")
    require(statistics_denial["outcome"] == "rejected" and not statistics_denial["roles"]["inherits_parent_owner"]
            and state(db) == before_statistics, "DML-only statistics role bypassed ownership or changed pending state")
    statistics = cache_driver(supervisor, args, output, "cache_statistics", "--cache-statistics")
    require(statistics["before_pending"] and not statistics["after_pending"] and statistics["last_analyzed_at"]
            and statistics["registered_partitions"] == statistics["cleanup_backlog"] == 1
            and statistics["partitions_created"] == 2 and statistics["partitions_dropped"] == 1,
            "cache statistics refresh or metadata-only counters failed")
    return {"staged": staged, "admitted_usage": usage, "direct_writer_denials": denied,
            "cross_service_inherited_owner_drop_rollback": cross_service,
            "unrelated_table_drop_denials": unrelated,
            "drop_failure": failed, "create_and_drop_rollback": "passed", "zero_usage_reclaim": reclaimed,
            "statistics": statistics,
            "statistics_dml_only_denial": statistics_denial,
            "retained_generation": populated}


def reclaim_cache(db, args, output, generation, phase):
    supervisor = Database(db.server, db.name, db.protocol, "native_cache_supervisor")
    before = cache_usage(db)
    reclaimed = cache_driver(supervisor, args, output, phase, "--cache-reclaim", str(generation))
    require(reclaimed["reclaimed"] == {"outcome": "dropped", "records": 2, "bytes": before["entries"]}, "supervisor reclaimed incorrect cache usage")
    require(cache_usage(db) == {"deployment": 0, "owner": 0, "entries": 0, "records": 0, "generations": None}, "cache reclamation did not return all accounting to zero")
    require(db.sql("SELECT count(*) FROM pg_inherits WHERE inhparent='cache_entry'::regclass;") == "0", "cache reclamation left a partition")
    require(db.sql("SELECT count(*) FROM cache_generation;") == "0" and db.sql("SELECT count(*) FROM cache_ingest_chunk;") == "0", "cache reclamation left generation or ingest metadata")
    return reclaimed


def restore_archive_list(contents):
    # Keep the owned empty schema and its narrow CREATE grant in place before
    # pg_restore reaches table OWNER commands. Schema ACLs occur later in a dump.
    entries = contents.splitlines()
    schema_entries = [i for i, line in enumerate(entries) if re.fullmatch(r"[0-9]+; [0-9]+ [0-9]+ SCHEMA - attune native_owner", line)]
    require(len(schema_entries) == 1, "archive must contain exactly the expected owned schema entry")
    entries[schema_entries[0]] = "; " + entries[schema_entries[0]]
    return "\n".join(entries) + "\n"


def logical_restore(source, target, output):
    producer_db = Database(source.server, target.name, source.protocol, "native_writer")
    producer = source.protocol.Session(producer_db, "install_restored_producer")
    builder = None
    try:
        producer.sql("BEGIN; SET search_path TO attune,public;")
        origin = producer.sql("SELECT pg_current_xact_id()::text;")
        xmin = producer.sql("SELECT pg_current_xact_id()::xid::text;")
        source.sql(f"UPDATE native_summary_invalidation SET transaction_origin='{origin}'::xid8 WHERE kind='event_volume' AND bucket='2021-10-02';")
        before = state(source)
        require(target.sql("SELECT count(*) FROM pg_class WHERE relnamespace='attune'::regnamespace;") == "0", "restore target schema is not empty")
        target.sql("GRANT USAGE,CREATE ON SCHEMA attune TO native_cache_owner;")
        remote = f"/tmp/{source.server.run_id}/{source.name}.dump"
        source.protocol.command("docker", "exec", source.server.name, "pg_dump", "-h", "127.0.0.1", "-U", "native_owner", "-d", source.name, "-Fc", "-f", remote, timeout=600)
        source.protocol.command("docker", "cp", f"{source.server.name}:{remote}", str(output / f"{source.name}.dump"), timeout=600)
        toc = source.protocol.command("docker", "exec", source.server.name, "pg_restore", "--list", remote, timeout=600).stdout
        archive_list = output / f"{source.name}.restore.list"
        archive_list.write_text(restore_archive_list(toc))
        source.protocol.command("docker", "cp", str(archive_list), f"{source.server.name}:{remote}.list", timeout=120)
        source.protocol.command("docker", "exec", source.server.name, "pg_restore", "-h", "127.0.0.1", "-U", "native_owner", "-d", target.name, "--exit-on-error", "--single-transaction", "--use-list", remote + ".list", remote, timeout=600)
        after = state(target)
        for key in ("raw", "schema_sha256", "sequences"):
            require(before[key] == after[key], f"fresh logical restore changed {key}")
        imported = target.json("SELECT jsonb_agg(jsonb_build_object('id',id,'origin',transaction_origin::text,'xmin',xmin::text) ORDER BY id) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2021-10-02';")
        require(imported and all(r["origin"] == origin and r["xmin"] != xmin for r in imported), "real restored-origin collision not established")
        builder = source.protocol.Session(target, "install_restored_builder")
        builder.sql("SET search_path TO attune,public; BEGIN ISOLATION LEVEL REPEATABLE READ; LOCK TABLE ONLY event IN ACCESS SHARE MODE; SELECT kind FROM native_summary_state WHERE kind='event_volume' FOR UPDATE;")
        builder.sql("CREATE TEMP TABLE captured ON COMMIT DROP AS SELECT id FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2021-10-02';")
        count = int(builder.sql("SELECT count(*) FROM event WHERE created='2021-10-02';"))
        producer.sql("INSERT INTO event(created,trigger_ref) VALUES('2021-10-02','install.restored'); INSERT INTO event(created,trigger_ref) VALUES('2021-10-02','install.restored');")
        builder.sql("DELETE FROM native_summary_invalidation n USING captured c WHERE n.id=c.id; COMMIT;")
        producer.sql("COMMIT;")
        require(int(target.sql("SELECT count(*) FROM event WHERE created='2021-10-02';")) == count + 2, "restored source delta")
        require(target.sql(f"SELECT count(*) FROM native_summary_invalidation WHERE kind='event_volume' AND bucket='2021-10-02' AND transaction_origin='{origin}'::xid8 AND xmin='{xmin}'::xid;") == "1", "imported markers consumed a fresh invalidation")
        return {"archive_sha256": digest((output / f"{source.name}.dump").read_bytes()), "restored_state": after,
                "imported_markers": imported, "producer_origin": origin, "producer_xmin": xmin, "raw_before": count, "raw_after": count + 2, "fresh_pending": 1}
    finally:
        if builder:
            builder.close()
        producer.close()


def verify_case(server, protocol, runner, args, output, manifest, report):
    owned = []
    try:
        token = uuid.uuid4().hex[:8]
        db = create_database(server, protocol, f"install_pg{server.image.split(':')[1].split('-')[0]}_{runner}_{token}", owned)
        report["databases"] = owned
        start = time.monotonic()
        migrate(db, runner, output / "migrations", args, output, "install")
        report["install_wall_seconds"] = time.monotonic() - start
        require(history(db, runner) == expected_history(manifest, runner), "recorded migration checksum mismatch")
        report["fresh_contract"] = fresh_contract(db)
        installed = state(db)
        report["installed_state"] = installed
        migrate(db, runner, output / "migrations", args, output, "noop")
        require(state(db) == installed, "migration rerun changed fresh state")
        failed = migrate(db, runner, output / "checksum_failure", args, output, "checksum_failure", allow_failure=True)
        diagnostic = (failed.stdout + failed.stderr).lower()
        require(failed.returncode and any(word in diagnostic for word in ("checksum", "modified", "versionmismatch")), "runner accepted altered applied migration bytes")
        require(state(db) == installed, "checksum rejection changed fresh database/history")
        failed = migrate(db, runner, output / "transaction_failure", args, output, "transaction_failure", allow_failure=True)
        require(failed.returncode and "owned fresh migration rollback probe" in failed.stdout + failed.stderr, "missing normal migrator transaction failure")
        require(db.sql("SELECT to_regclass('fresh_migration_rollback_probe') IS NULL;") == "t" and state(db) == installed, "failed migration did not roll back DDL/history")
        migrate(db, runner, output / "migrations", args, output, "retry")
        require(state(db) == installed, "retry changed canonical installed state")
        report["noop_checksum_rejection_transaction_rollback_retry"] = "passed"
        provision_writers(db)
        report["business"] = business_probe(db)
        report["maintenance"] = maintenance_probe(db)
        provision_cache_roles(db)
        report["cache"] = cache_probe(db, args, output)
        report["final_state"] = state(db)
        if args.logical_restore:
            restored = create_database(server, protocol, db.name + "_restored", owned)
            report["logical_restore"] = logical_restore(db, restored, output)
            require(history(restored, runner) == expected_history(manifest, runner), "restored history checksums changed")
            report["logical_restore"]["cache_reclaim"] = reclaim_cache(restored, args, output, report["cache"]["retained_generation"], "cache_restored_reclaim")
        report["cache"]["populated_reclaim"] = reclaim_cache(db, args, output, report["cache"]["retained_generation"], "cache_populated_reclaim")
        report["reclaimed_state"] = state(db)
        report["passed"] = True
    finally:
        errors = []
        for session in reversed(server.sessions):
            try:
                session.close()
            except Exception as error:
                errors.append(str(error))
        for name in reversed(owned):
            try:
                protocol.command(*server.psql(), input=f"SET statement_timeout='120s'; DROP DATABASE {name};", timeout=140)
            except Exception as error:
                errors.append(str(error))
        report["database_cleanup_errors"] = errors
        require(not errors, "owned database cleanup failed: " + "; ".join(errors))


def main(argv=None):
    cli = parser()
    args = cli.parse_args(argv)
    if len(set(args.versions)) != len(args.versions):
        cli.error("duplicate --versions")
    if len(set(args.runners)) != len(args.runners):
        cli.error("duplicate --runners")
    if args.output.exists() or args.output.is_symlink():
        cli.error("refusing existing output; choose a new --output directory")
    if not args.output.parent.is_dir():
        cli.error("--output parent must exist, for example /tmp/opencode")
    files, support, manifest = snapshot_inputs()
    evidence = {"run_id": "install-" + uuid.uuid4().hex[:12], "inputs_before": manifest, "versions_requested": args.versions,
                "runners_requested": args.runners, "preliminary": args.preliminary,
                "logical_restore_requested": args.logical_restore, "performance": "REPORTING_PERFORMANCE_DEFERRED",
                "production_certification": False, "versions": []}
    if args.dry_run:
        print(json.dumps(evidence | {"dry_run": True}, indent=2))
        return 0
    require(all(manifest["fresh_declarations"].values()), "fresh declarations are not ready; wait for the core fresh-source freeze")
    require(args.sqlx_driver.is_file() and os.access(args.sqlx_driver, os.X_OK), "build verify_native_migrations after the fresh-source freeze")
    evidence["driver_sha256_before"] = digest(args.sqlx_driver.read_bytes())
    args.output.mkdir(mode=0o700)
    output = args.output.resolve()
    tampered = dict(files)
    first = sorted(files)[0]
    tampered[first] += b"\n-- owned checksum rejection probe\n"
    rollback = files | {PROBE_MIGRATION: b"CREATE TABLE fresh_migration_rollback_probe(id BIGINT PRIMARY KEY);\nINSERT INTO fresh_migration_rollback_probe VALUES(1);\nDO $$ BEGIN RAISE EXCEPTION 'owned fresh migration rollback probe'; END $$;\n"}
    for name, contents in (("migrations", files), ("checksum_failure", tampered), ("transaction_failure", rollback)):
        (output / name).mkdir(mode=0o700)
        for filename, data in contents.items():
            (output / name / filename).write_bytes(data)
    (output / "run-migrations.sh").write_bytes(support["docker/run-migrations.sh"])
    protocol = None
    try:
        protocol = load_protocol()
        for version in args.versions:
            report = {"version": version, "rejections": {}, "observations": {}, "runners": []}
            evidence["versions"].append(report)
            with protocol.Server(f"postgres:{version}-alpine", evidence["run_id"], report) as server:
                report["container_name"], report["volume_name"] = server.name, server.volume
                server.sql("CREATE ROLE native_owner LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_writer LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_observer LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_maintainer LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_api LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_cache_owner NOLOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_cache_supervisor LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; CREATE ROLE native_cache_dml LOGIN NOSUPERUSER NOBYPASSRLS NOCREATEDB NOCREATEROLE; GRANT native_owner TO native_maintainer; GRANT native_cache_owner TO native_owner,native_api,native_cache_supervisor;")
                protocol.command("docker", "exec", server.name, "mkdir", f"/tmp/{server.run_id}")
                for name in ("migrations", "checksum_failure", "transaction_failure", "run-migrations.sh"):
                    protocol.command("docker", "cp", str(output / name), f"{server.name}:/tmp/{server.run_id}/{name}", timeout=120)
                for runner in args.runners:
                    case = {"runner": runner}
                    report["runners"].append(case)
                    verify_case(server, protocol, runner, args, output, manifest, case)
                require(server.sql("SELECT count(*) FROM pg_database WHERE datname LIKE 'install_%';") == "0", "owned database leak")
                report["database_leaks"] = 0
                report["passed"] = True
    except BaseException as error:
        evidence["error"] = str(error)
        raise
    finally:
        if protocol is not None:
            try:
                evidence["remaining_resources"] = remaining_resources(protocol, evidence["run_id"])
                require(not any(evidence["remaining_resources"].values()), "owned container/volume leak after cleanup")
            except Exception as error:
                evidence["cleanup_proof_error"] = str(error)
        try:
            _, _, after = snapshot_inputs()
            evidence["inputs_after"] = after
            evidence["driver_sha256_after"] = digest(args.sqlx_driver.read_bytes())
            evidence["inputs_unchanged"] = manifest == after and evidence["driver_sha256_before"] == evidence["driver_sha256_after"]
        except Exception as error:
            evidence["input_recheck_error"] = str(error)
            evidence["inputs_unchanged"] = False
        evidence["passed"] = (not evidence.get("error") and not evidence.get("cleanup_proof_error") and evidence["inputs_unchanged"]
                              and len(evidence["versions"]) == len(args.versions)
                              and all(v.get("passed") and not v.get("cleanup_errors") for v in evidence["versions"]))
        (output / "evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
        print(json.dumps({k: evidence.get(k) for k in ("run_id", "passed", "preliminary", "inputs_unchanged", "performance", "error")} | {"output": str(output)}, indent=2))
    require(evidence["passed"], "fresh acceptance incomplete or inputs changed; see evidence.json")
    return 0


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(143))
    main()
