#!/usr/bin/env python3
"""Owned repository cache benchmark. Never connects to an installed database.

Prepare after the source freeze, then run only in the granted exclusive lane.
The heap comparator retains current admission/lifecycle code and restores the
historical entry table, accounting triggers and bounded row-delete repositories.
Every generated difference and both source fingerprints remain in the evidence.
"""

import argparse
from contextlib import contextmanager
import difflib
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import shutil
import subprocess
import sys
import time
import unittest
import uuid

ROOT = Path(__file__).resolve().parents[1]
DISK_EVIDENCE_ROOT = Path("/home/david/.cache/attune-release-evidence")
REPO = Path("crates/common/src/repositories/cache.rs")
STORAGE = Path("crates/common/src/repositories/cache/storage.rs")
CACHE = "20250101000021_cache.sql"
ACCOUNTING = "20250101000024_cache_physical_byte_accounting.sql"
PARTITIONS = "20261007000001_cache_generation_partitions.sql"
EXAMPLE = "measure_cache_generation_partitions"
DRIVER = Path("scripts/measure-cache-generation-partitions.py")
PROTOCOL = Path("scripts/probe-postgresql-native-maintenance.py")
SOURCE_ITEMS = ("Cargo.toml", "Cargo.lock", "config.test.yaml", "crates", "migrations", ".sqlx",
                str(DRIVER), str(PROTOCOL))
SOURCE_IGNORES = {"target", "node_modules", "__pycache__"}
STAGE_TIMEOUT_SECONDS = 43200
TARGETS = {"small_read_p95_ms": 250, "ingest_p95_ms": 1000,
           "creation_p95_ms": 1000, "cleanup_ratio": .30, "wal_ratio": .10}
PG_MEMORY_BYTES = 1024**3
PG_NANO_CPUS = 2_000_000_000
PG_SHM_BYTES = 256 * 1024**2


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def percentile(values, percent):
    return sorted(values)[math.ceil(len(values) * percent / 100) - 1] if values else None


def filesystem_type(path, mountinfo):
    """Resolve the longest matching Linux mount, including escaped mount names."""
    resolved = path.expanduser().resolve()
    candidates = []
    for line in mountinfo.splitlines():
        left, right = line.split(" - ", 1)
        mount = left.split()[4]
        for escaped, character in [(r"\040", " "), (r"\011", "\t"),
                                   (r"\012", "\n"), (r"\134", "\\")]:
            mount = mount.replace(escaped, character)
        mount = Path(mount)
        if resolved.is_relative_to(mount):
            candidates.append((len(mount.parts), right.split()[0], str(mount)))
    require(candidates, f"cannot determine filesystem backing {resolved}")
    _, kind, mount = max(candidates)
    return kind, mount


def disk_storage(path, purpose, minimum_free=0):
    resolved = path.expanduser().resolve()
    existing = resolved
    while not existing.exists():
        existing = existing.parent
    kind, mount = filesystem_type(resolved, Path("/proc/self/mountinfo").read_text())
    require(kind not in {"tmpfs", "ramfs", "devtmpfs", "hugetlbfs"},
            f"{purpose} is RAM-backed ({kind}): {resolved}; use {DISK_EVIDENCE_ROOT}/<unique> or a /mnt/wdc path")
    free = shutil.disk_usage(existing).free
    require(free >= minimum_free, f"{purpose} needs {minimum_free} free bytes; {resolved} has {free}")
    return {"path":str(resolved),"filesystem_type":kind,"mount":mount,"free_bytes":free}


def effective_resource_caps(inspected):
    host = inspected["HostConfig"]
    effective = {"memory_bytes":host["Memory"],"memory_swap_bytes":host["MemorySwap"],
                  "nano_cpus":host["NanoCpus"],"cpu_set":host.get("CpusetCpus", ""),
                  "shm_size_bytes":host["ShmSize"]}
    require(effective["memory_bytes"] == PG_MEMORY_BYTES
            and effective["memory_swap_bytes"] == PG_MEMORY_BYTES
            and effective["nano_cpus"] == PG_NANO_CPUS
            and effective["shm_size_bytes"] == PG_SHM_BYTES,
            f"Docker did not provision declared 1 GiB/no-swap/2 CPU/256 MiB shm limits: {effective}")
    return effective


def small_summary(path, data):
    if path is None:
        return
    text = json.dumps(data, indent=2) + "\n"
    require(len(text.encode()) <= 4096, "summary exceeds 4 KiB; keep detailed evidence on disk")
    with path.open("x", encoding="utf-8") as stream:
        stream.write(text)


def fingerprint(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob("*")) if p.is_file()}


def source_identity(root):
    """Hash the sorted selected path-to-SHA256 map, not mtimes or a live Git ref."""
    selected = {}
    for name in SOURCE_ITEMS:
        source = root / name
        # Workspace preparation can leave an empty root cache. The tracked
        # per-crate caches are already included under `crates`.
        if name == ".sqlx" and not source.exists():
            continue
        require(source.exists(), f"assigned source is missing {name}")
        if source.is_dir():
            files = [p for p in source.rglob("*") if p.is_file()
                     and not SOURCE_IGNORES.intersection(p.relative_to(source).parts)]
        else:
            files = [source]
        for path in files:
            selected[str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    selected = dict(sorted(selected.items()))
    digest = hashlib.sha256(json.dumps(selected, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    return {"source_sha256":digest,"files":selected}


def function(source, name):
    """Extract Rust method using its next equally indented declaration boundary."""
    prefix = "    pub async fn " if "    pub async fn " + name + "(" in source else "    async fn "
    start = source.index(prefix + name + "(")
    end = source.index("\n    }", start) + len("\n    }")
    return source[start:end]


def admission_protocol(source):
    start = source.index("\nasync fn lock_cache_admission(") + 1
    end = source.index("\n}", start) + len("\n}")
    constants = [next(line for line in source.splitlines() if line.startswith("const " + name + ":"))
                 for name in ["CACHE_ADMISSION_ADVISORY_LOCK_CLASS", "CACHE_ADMISSION_ADVISORY_LOCK_KEY"]]
    return "\n".join(constants) + "\n" + source[start:end]


def replace_once(source, before, after):
    require(source.count(before) == 1, f"source freeze changed: {before[:100]!r}")
    return source.replace(before, after, 1)


def heap_creation_protection(native):
    # Heap generation creation inserts metadata/usage rows, not partitions.
    # Keep admission, deadlines and every other mode byte-identical, but do not
    # import ATTACH's vacuum-conflicting parent DDL lock into a plain heap.
    return replace_once(native,
        '"LOCK TABLE ONLY cache_entry IN SHARE UPDATE EXCLUSIVE MODE"',
        '"LOCK TABLE ONLY cache_entry IN ROW EXCLUSIVE MODE"')


def heap_cleanup_shell(native, step):
    """Normalize only operation/signature differences for shared-budget proof."""
    start = "            let result = async {"
    end = "            let (outcome, records, bytes) = match result"
    step = replace_once(step, step[step.index(start):step.index(end)],
        native[native.index("            // The SQL function's admission call"):native.index(end)])
    step = replace_once(step, "async fn cleanup_heap_step(", "pub async fn drop_if_cleanup_eligible(")
    step = replace_once(step, "config: &CacheRetentionConfig, finalizing: bool,", "config: &CacheRetentionConfig,")
    return replace_once(step,
        "let code = match &error { Error::Database(e) => e.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned()), _ => None };",
        "let code = error.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned());")


def heap_baseline(snapshot, historical):
    """Only mutate the owned baseline copy, never the checkout or a database."""
    old_repo = (historical / REPO).read_text()
    current = (snapshot / REPO).read_text()
    native_protection = function(current, "protect_transaction")
    current = replace_once(current, native_protection, heap_creation_protection(native_protection))
    current = replace_once(current,
        "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::regclass",
        "SELECT COUNT(*) FROM cache_generation")
    old_batch = function(old_repo, "delete_cleanup_batch")
    old_finalize = function(old_repo, "delete_if_empty")
    # Execute historical heap operations inside the native repository's bounded
    # admission/transaction shell. Each batch uses the same absolute remaining
    # cycle budget; neither arm gets an unlimited advisory or parent wait.
    old_batch = replace_once(old_batch,
        "pub async fn delete_cleanup_batch(pool: &PgPool, generation_id: Id, limit: i64)",
        "async fn delete_cleanup_batch(connection: &mut sqlx::PgConnection, generation_id: Id, limit: i64)")
    old_batch = replace_once(old_batch, "let mut tx = pool.begin().await?;", "")
    old_batch = old_batch.replace("&mut *tx", "&mut *connection")
    old_batch = replace_once(old_batch, "tx.commit().await?;", "")
    old_batch = replace_once(old_batch, "tx.rollback().await?;", "")
    old_batch = replace_once(old_batch, "WHERE e.id = c.id", "WHERE e.generation = $1 AND e.id = c.id")
    old_finalize = replace_once(old_finalize,
        "pub async fn delete_if_empty(pool: &PgPool, generation_id: Id)",
        "async fn delete_if_empty(connection: &mut sqlx::PgConnection, generation_id: Id)")
    old_finalize = replace_once(old_finalize, "let mut tx = pool.begin().await?;", "")
    old_finalize = old_finalize.replace("&mut *tx", "&mut *connection")
    old_finalize = replace_once(old_finalize, "tx.commit().await?;", "")
    old_finalize = replace_once(old_finalize, "tx.rollback().await?;", "")
    old_finalize = replace_once(old_finalize,
        'let result = sqlx::query("DELETE FROM cache_generation WHERE id = $1")',
        '''sqlx::query("DELETE FROM cache_generation_entry_usage WHERE generation = $1")
            .bind(generation_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE cache_entry_statistics_state SET partitions_dropped=partitions_dropped+1 WHERE id=TRUE")
            .execute(&mut *tx).await?;
        let result = sqlx::query("DELETE FROM cache_generation WHERE id = $1")''')
    old_finalize = old_finalize.replace("&mut *tx", "&mut *connection")
    bounded_step = function(current, "drop_if_cleanup_eligible")
    bounded_step = replace_once(bounded_step, "pub async fn drop_if_cleanup_eligible(",
                               "async fn cleanup_heap_step(")
    bounded_step = replace_once(bounded_step, "config: &CacheRetentionConfig,",
                               "config: &CacheRetentionConfig, finalizing: bool,")
    query_start = bounded_step.index('            // The SQL function\'s admission call')
    query_end = bounded_step.index('            let (outcome, records, bytes) = match result',query_start)
    bounded_step = replace_once(bounded_step,bounded_step[query_start:query_end],'''            let result = async {
                if traversal_seconds != 0 {
                    return Err(Error::validation("heap comparator requires declared zero traversal window"));
                }
                sqlx::query("LOCK TABLE ONLY cache_entry IN ROW EXCLUSIVE MODE")
                    .execute(&mut *tx).await?;
                if finalizing {
                    let deleted = Self::delete_if_empty(&mut tx, generation_id).await?;
                    Ok::<_, Error>((if deleted { "dropped" } else { "ineligible" }.to_string(), 0i64, 0i64))
                } else {
                    let deleted = CacheEntryRepository::delete_cleanup_batch(&mut tx, generation_id, 1000).await?;
                    Ok(("dropped".to_string(), deleted as i64, 0i64))
                }
            }.await;
''')
    bounded_step = replace_once(bounded_step,
        "let code = error.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned());",
        "let code = match &error { Error::Database(e) => e.as_database_error().and_then(|e| e.code()).map(|c| c.into_owned()), _ => None };")
    wrapper = '''    pub async fn drop_if_cleanup_eligible(
        pool: &PgPool, generation_id: Id, config: &CacheRetentionConfig,
    ) -> Result<CacheGenerationCleanupOutcome> {
        if config.min_traversal_window_seconds != 0 {
            return Err(Error::validation("heap comparator requires declared zero traversal window"));
        }
        config.validate_storage_maintenance().map_err(Error::validation)?;
        let start = std::time::Instant::now();
        let generation = match tokio::time::timeout(std::time::Duration::from_millis(config.max_cleanup_cycle_milliseconds), Self::find_by_id(pool, generation_id)).await {
            Ok(result) => result?,
            Err(_) => return Ok(CacheGenerationCleanupOutcome::DeferredDeadline),
        };
        let Some(generation) = generation else {
            return Ok(CacheGenerationCleanupOutcome::Absent);
        };
        let mut finalizing = false;
        loop {
            let remaining = config.max_cleanup_cycle_milliseconds.saturating_sub(
                u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX));
            if remaining == 0 {
                return Ok(CacheGenerationCleanupOutcome::DeferredDeadline);
            }
            let mut bounded = config.clone();
            bounded.max_cleanup_cycle_milliseconds = remaining;
            match Self::cleanup_heap_step(pool, generation_id, &bounded, finalizing).await? {
                CacheGenerationCleanupOutcome::Dropped { .. } if finalizing => {
                    return Ok(CacheGenerationCleanupOutcome::Dropped {
                        records: generation.record_count as u64, bytes: generation.size_bytes as u64,
                    });
                },
                CacheGenerationCleanupOutcome::Dropped { records: 0, .. } => finalizing = true,
                CacheGenerationCleanupOutcome::Dropped { .. } => {},
                outcome => return Ok(outcome),
            }
        }
    }'''
    current = replace_once(current, function(current, "drop_if_cleanup_eligible"),
                           wrapper + "\n\n" + bounded_step + "\n\n" + old_finalize)
    marker = "impl CacheEntryRepository {"
    current = replace_once(current, marker, marker + "\n" + old_batch + "\n")
    (snapshot / REPO).write_text(current)
    migration = snapshot / "migrations" / CACHE
    source = migration.read_text()
    old = (historical / "migrations" / CACHE).read_text()
    for start, end in [("CREATE TABLE cache_entry (", "CREATE OR REPLACE FUNCTION account_cache_entry_size()"),
                       ("CREATE OR REPLACE FUNCTION cache_entry_staging_only()", "CREATE TABLE cache_ingest_chunk (")]:
        source = replace_once(source, source[source.index(start):source.index(end)],
                               old[old.index(start):old.index(end)])
    migration.write_text(source)
    # Keep the current generation-usage and metadata accounting on both arms.
    # Restore historical DELETE transition accounting, then decrement that same
    # per-generation usage under the same deployment/owner/generation lock order.
    old_accounting = (historical / "migrations" / ACCOUNTING).read_text()
    delete_function = old_accounting[old_accounting.index(
        "CREATE OR REPLACE FUNCTION account_deleted_cache_entry_physical_bytes()"):
        old_accounting.index("CREATE TRIGGER account_inserted_cache_entry_physical_bytes_trigger")]
    delete_function = replace_once(delete_function, "owner_delta RECORD;",
                                    "owner_delta RECORD; generation_delta RECORD;")
    delete_function = replace_once(delete_function, "    RETURN NULL;", '''    FOR generation_delta IN
        SELECT generation,COUNT(*)::BIGINT AS records,SUM(size_bytes)::BIGINT AS bytes
        FROM deleted_cache_entries GROUP BY generation ORDER BY generation
    LOOP
        UPDATE cache_generation_entry_usage
           SET record_count=record_count-generation_delta.records,
               physical_bytes=physical_bytes-generation_delta.bytes
         WHERE generation=generation_delta.generation;
        IF NOT FOUND THEN RAISE EXCEPTION 'cache generation entry usage is missing'; END IF;
    END LOOP;
    RETURN NULL;''')
    accounting = snapshot / "migrations" / ACCOUNTING
    accounting.write_text(accounting.read_text() + "\n" + delete_function + '''
CREATE TRIGGER account_deleted_cache_entry_physical_bytes_trigger
    AFTER DELETE ON cache_entry REFERENCING OLD TABLE AS deleted_cache_entries
    FOR EACH STATEMENT EXECUTE FUNCTION account_deleted_cache_entry_physical_bytes();
''')
    lifecycle = snapshot / "migrations" / PARTITIONS
    source = lifecycle.read_text()
    # Preserve statistics state and all current chunk/iteration metadata bounds.
    prefix = source[:source.index("CREATE FUNCTION cache_generation_partition_name(")]
    lifecycle.write_text(prefix + '''
-- Owned heap comparator lifecycle. Relation DDL is replaced by usage creation.
CREATE FUNCTION validate_cache_generation_partition(generation_id BIGINT)
RETURNS TEXT LANGUAGE plpgsql AS $$
BEGIN
    IF generation_id IS NULL OR generation_id <= 0 OR NOT EXISTS (
        SELECT 1 FROM cache_generation_entry_usage WHERE generation=generation_id
    ) THEN RAISE EXCEPTION 'cache generation heap or usage invariant is invalid'; END IF;
    RETURN 'cache_entry';
END; $$;
CREATE FUNCTION create_cache_generation_partition(generation_id BIGINT)
RETURNS VOID LANGUAGE plpgsql AS $$
BEGIN
    IF generation_id IS NULL OR generation_id <= 0 THEN
        RAISE EXCEPTION 'cache generation ID must be positive';
    END IF;
    INSERT INTO cache_owner_physical_byte_usage(owner_type,owner,physical_bytes)
        SELECT n.owner_type,n.owner,0 FROM cache_generation g
        JOIN cache_namespace n ON n.id=g.namespace WHERE g.id=generation_id
        ON CONFLICT (owner_type,owner) DO NOTHING;
    INSERT INTO cache_generation_entry_usage(generation) VALUES(generation_id);
    UPDATE cache_entry_statistics_state SET partitions_created=partitions_created+1 WHERE id=TRUE;
    PERFORM validate_cache_generation_partition(generation_id);
END; $$;
''')
    storage = snapshot / STORAGE
    source = storage.read_text()
    source = replace_once(source,
        "SELECT COUNT(*) FROM pg_inherits WHERE inhparent = 'cache_entry'::REGCLASS",
        "SELECT COUNT(*) FROM cache_generation")
    source = replace_once(source, "AND relkind='p'", "AND relkind IN ('p','r')")
    storage.write_text(source)


def snapshot(destination, source_root):
    destination.mkdir()
    for name in SOURCE_ITEMS:
        source = source_root / name
        if name == ".sqlx" and not source.exists():
            continue
        if source.is_dir():
            shutil.copytree(source, destination / name,
                            ignore=shutil.ignore_patterns("target", "node_modules", "__pycache__"))
        else:
            (destination / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, destination / name)


def prepare(output, historical, source_root, expected_hash):
    require(not output.exists(), "refusing existing evidence directory")
    require((historical / REPO).is_file(), "historical baseline source missing")
    output.parent.resolve(strict=True)
    storage = disk_storage(output, "benchmark output", minimum_free=2 * 1024**3)
    source_storage = disk_storage(source_root, "assigned source snapshot")
    identity = source_identity(source_root)
    require(identity["source_sha256"] == expected_hash, "assigned source SHA256 mismatch")
    require(identity["files"][str(DRIVER)] == hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "use the benchmark driver included in the assigned final source snapshot")
    output.mkdir()
    evidence = {"run_id": "cachemeasure-" + uuid.uuid4().hex[:12], "targets": TARGETS,
                "historical_source": str(historical.resolve()), "status": "preparing",
                "output_storage":storage,"source_storage":source_storage,
                "assigned_source":str(source_root),"source_sha256":expected_hash}
    try:
        snapshot(output / "treatment", source_root)
        require(fingerprint(output / "treatment") == identity["files"], "copied source differs from assigned snapshot")
        require(source_identity(source_root) == identity, "assigned snapshot changed during preparation")
        shutil.copytree(output / "treatment", output / "baseline")
        evidence["historical_fingerprint"] = {
            str(p): hashlib.sha256((historical / p).read_bytes()).hexdigest()
            for p in [REPO, Path("migrations") / CACHE, Path("migrations") / ACCOUNTING]}
        heap_baseline(output / "baseline", historical)
        before = fingerprint(output / "treatment")
        after = fingerprint(output / "baseline")
        evidence["treatment_fingerprint"] = before
        evidence["driver_sha256"] = before[str(DRIVER)]
        evidence["baseline_fingerprint"] = after
        # This entire current prefix includes chunk bounds, insertion/replay-safe
        # iteration admission, grouped DELETE accounting and sorted usage locks.
        treatment_lifecycle = (output / "treatment/migrations" / PARTITIONS).read_text()
        prefix = treatment_lifecycle[:treatment_lifecycle.index("CREATE FUNCTION cache_generation_partition_name(")]
        require((output / "baseline/migrations" / PARTITIONS).read_text().startswith(prefix),
                "baseline changed shared metadata or statistics contract")
        evidence["shared_metadata_contract_sha256"] = hashlib.sha256(prefix.encode()).hexdigest()
        treatment_protocol = function((output / "treatment" / REPO).read_text(), "protect_transaction")
        baseline_protocol = function((output / "baseline" / REPO).read_text(), "protect_transaction")
        require(baseline_protocol == heap_creation_protection(treatment_protocol),
                "baseline changed coordination beyond its non-DDL creation lock")
        evidence["transaction_protocol"] = {
            "treatment_sha256":hashlib.sha256(treatment_protocol.encode()).hexdigest(),
            "baseline_sha256":hashlib.sha256(baseline_protocol.encode()).hexdigest(),
            "only_difference":"Attach parent lock: native SHARE UPDATE EXCLUSIVE, heap ROW EXCLUSIVE",
            "admission_deadlines_and_other_modes_unchanged":True}
        admission = admission_protocol((output / "treatment" / REPO).read_text())
        require(admission_protocol((output / "baseline" / REPO).read_text()) == admission,
                "baseline changed the two-integer admission advisory lock")
        evidence["shared_admission_protocol_sha256"] = hashlib.sha256(admission.encode()).hexdigest()
        native_cleanup = function((output / "treatment" / REPO).read_text(), "drop_if_cleanup_eligible")
        heap_cleanup = function((output / "baseline" / REPO).read_text(), "cleanup_heap_step")
        require(heap_cleanup_shell(native_cleanup, heap_cleanup) == native_cleanup,
                "baseline changed bounded cleanup coordination or transport semantics")
        evidence["cleanup_budget_protocol"] = {
            "native_sha256": hashlib.sha256(native_cleanup.encode()).hexdigest(),
            "heap_step_sha256": hashlib.sha256(heap_cleanup.encode()).hexdigest(),
            "normalized_shell_byte_identical": True,
            "heap_batches_share_one_remaining_cycle_budget": True,
            "ddl_lock_timeout_ms": 250, "ddl_statement_timeout_ms": 1000,
            "max_cleanup_cycle_ms": 30000}
        changed = [p for p in before if before[p] != after[p]]
        require(set(changed) == {str(REPO), str(STORAGE), "migrations/" + CACHE, "migrations/" + ACCOUNTING,
                                 "migrations/" + PARTITIONS}, "unexpected baseline changes")
        diff = []
        for name in changed:
            diff.extend(difflib.unified_diff((output / "treatment" / name).read_text().splitlines(True),
                        (output / "baseline" / name).read_text().splitlines(True),
                        fromfile="treatment/" + name, tofile="baseline/" + name))
        (output / "baseline.diff").write_text("".join(diff))
        evidence["status"] = "prepared"
    except BaseException as error:
        evidence["error"] = repr(error)
        raise
    finally:
        (output / "manifest.json").write_text(json.dumps(evidence, indent=2) + "\n")


def compile_examples(output, cargo_target):
    require(not any(cargo_target.resolve().is_relative_to(output / arm) for arm in ["baseline","treatment"]),
            "Cargo target must be outside frozen source trees; pass --cargo-target with the shared disk-backed target")
    disk_storage(output, "benchmark output", minimum_free=2 * 1024**3)
    disk_storage(cargo_target, "Cargo target", minimum_free=8 * 1024**3)
    compiled = {}
    for arm in ["treatment", "baseline"]:
        # Cargo's incremental freshness checks use mtimes. Independently copied
        # same-version workspace trees can reuse the other arm's newer library.
        # Keep frozen inputs untouched; force a source-fresh common build in an
        # owned byte-identical copy, while retaining the shared dependency cache.
        build_source = output / f"{arm}.build-source"
        require(not build_source.exists(), "refusing reused arm build source")
        shutil.copytree(output / arm, build_source)
        expected = fingerprint(output / arm)
        require(fingerprint(build_source) == expected, "arm build copy differs from frozen source")
        for path in build_source.rglob("*"):
            if path.is_file():
                os.utime(path, None)
        env = {**os.environ, "SQLX_OFFLINE": "true", "CARGO_BUILD_JOBS": "2",
                "CARGO_TARGET_DIR": str(cargo_target.resolve())}
        result = subprocess.run(["cargo", "build", "--locked", "-p", "attune-common",
                                  "--example", EXAMPLE], cwd=build_source, env=env,
                                 capture_output=True, text=True, timeout=7200)
        (output / f"{arm}.build.log").write_text(result.stdout + result.stderr)
        require(result.returncode == 0, f"{arm} compilation failed")
        require("Compiling attune-common " in result.stderr,
                f"{arm} did not compile its own source-fresh common library")
        require(fingerprint(build_source) == expected and fingerprint(output / arm) == expected,
                "compilation changed arm source bytes")
        shutil.copy2(cargo_target / "debug/examples" / EXAMPLE, output / f"{arm}.bin")
        compiled[arm] = {"source_fresh_common_build":True,"build_source":str(build_source),
                         "binary_sha256":hashlib.sha256((output / f"{arm}.bin").read_bytes()).hexdigest()}
    (output / "compiled-examples.json").write_text(json.dumps(compiled, indent=2) + "\n")
    return compiled


def load_protocol(path):
    spec = importlib.util.spec_from_file_location("owned_cache_protocol",
                path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def anchored_command(args, server_name, anchor_name):
    """Publish on a persistent network namespace, not the restarting database."""
    if args[:2] == ("docker", "run") and "--name" in args and args[args.index("--name") + 1] == server_name:
        publish = args.index("--publish")
        require(args[publish + 1] == "127.0.0.1::5432", "unexpected benchmark port publication")
        args = args[:publish] + ("--network", "container:" + anchor_name) + args[publish + 2:]
    elif args[:3] == ("docker", "port", server_name):
        args = args[:2] + (anchor_name,) + args[3:]
    return args


@contextmanager
def stable_server(protocol, image, run_id, report):
    """Keep Docker's allocated host port alive while PostgreSQL really restarts."""
    server = protocol.Server(image, run_id, report)
    anchor = server.name + "-network"
    original = protocol.command
    claimed = False
    mounts = []
    report["owner"] = run_id
    report["container_name"] = server.name
    report["network_anchor"] = anchor
    try:
        require(original("docker", "inspect", anchor, check=False).returncode != 0,
                "refusing existing network anchor")
        claimed = True
        original("docker", "run", "-d", "--pull=never", "--name", anchor,
                 "--label", f"{protocol.LABEL}={run_id}", "--publish", "127.0.0.1::5432",
                 "--entrypoint", "/bin/sh", image, "-c", "exec sleep infinity")
        info = json.loads(original("docker", "inspect", anchor).stdout)[0]
        require(info["Config"]["Labels"].get(protocol.LABEL) == run_id, "anchor ownership changed")
        mounts = info["Mounts"]
        report["anchor_mounts"] = mounts
        def command(*args, **kwargs):
            return original(*anchored_command(args, server.name, anchor), **kwargs)
        protocol.command = command
        with server:
            report["database_mounts"] = json.loads(original("docker", "inspect", server.name).stdout)[0]["Mounts"]
            yield server
    finally:
        protocol.command = original
        errors = []
        if claimed:
            try:
                inspected = original("docker", "inspect", anchor, check=False)
                if inspected.returncode == 0:
                    info = json.loads(inspected.stdout)[0]
                    require(info["Config"]["Labels"].get(protocol.LABEL) == run_id, "anchor ownership changed")
                    original("docker", "rm", "-f", "-v", info["Id"])
            except Exception as error:
                errors.append(str(error))
        try:
            containers = original("docker", "ps", "-aq", "--filter", f"label={protocol.LABEL}={run_id}").stdout.splitlines()
            volumes = original("docker", "volume", "ls", "-q", "--filter", f"label={protocol.LABEL}={run_id}").stdout.splitlines()
            mounted = [m["Name"] for m in mounts + report.get("database_mounts", []) if m["Type"] == "volume"]
            remaining_mounts = [name for name in mounted
                                if original("docker", "volume", "inspect", name, check=False).returncode == 0]
            report["resources_after_teardown"] = {"containers": containers, "labeled_volumes": volumes,
                                                   "mounted_volumes": remaining_mounts}
            require(not containers and not volumes and not remaining_mounts, "owned benchmark resources remain after teardown")
        except Exception as error:
            errors.append(str(error))
        report.setdefault("cleanup_errors", []).extend(errors)
        require(not errors, "; ".join(errors))


def run(output, versions, sizes, modes, exclusive, smoke=False, cargo_target=None, expected_hash=None):
    require(exclusive, "--exclusive-lane-granted is required; ask main before running")
    cargo_target = cargo_target or ROOT / "target"
    output_storage = disk_storage(output, "benchmark output", minimum_free=2 * 1024**3)
    cargo_storage = disk_storage(cargo_target, "Cargo target", minimum_free=8 * 1024**3)
    manifest = json.loads((output / "manifest.json").read_text())
    require(manifest["status"] == "prepared", "expected prepared source freeze")
    require(manifest.get("source_sha256") == expected_hash and expected_hash is not None,
            "run must use the coordinator-assigned source SHA256")
    require(hashlib.sha256(Path(__file__).read_bytes()).hexdigest() == manifest.get("driver_sha256"),
            "benchmark runner changed after preparation; prepare a new source freeze")
    require(not (output / "run.json").exists(), "refusing repeated run or discarded samples")
    for arm in ["treatment", "baseline"]:
        require(fingerprint(output / arm) == manifest[arm + "_fingerprint"], "source freeze changed")
    evidence = {"manifest": "manifest.json", "source_sha256":expected_hash,"samples": [], "targets": TARGETS,
                "full_declared_admitted_records":201_920_000,
                "smoke_only":smoke,
                "stage_timeout_seconds":STAGE_TIMEOUT_SECONDS,
                "resources": {"postgres_cpus": 2,"postgres_memory_bytes":PG_MEMORY_BYTES,
                              "postgres_memory_swap_bytes":PG_MEMORY_BYTES,
                              "postgres_shm_size_bytes":PG_SHM_BYTES},
                "output_storage":output_storage,"cargo_storage":cargo_storage,
                "performance_scope":"1 GiB / 2 CPU owned fixture, not a production hardware claim",
                "source_transport":"host Cargo and SQLx TCP; no source bind mount into PostgreSQL"}
    protocol = load_protocol(output / "treatment" / PROTOCOL)
    # The shared fixture is reused for ownership-checked teardown. Insert explicit
    # resource limits into its Docker invocation before startup, not afterward.
    original_command = protocol.command
    def limited_command(*args, **kwargs):
        if args[:2] == ("docker", "run"):
            # /dev/shm remains charged to the 1 GiB cgroup. Docker's default
            # 64 MiB cannot fit PostgreSQL's default parallel-vacuum segment.
            args = args[:2] + ("--memory=1g", "--memory-swap=1g", "--cpus=2", "--shm-size=256m") + args[2:]
        return original_command(*args, **kwargs)
    protocol.command = limited_command
    try:
        evidence["compilation"] = compile_examples(output, cargo_target)
        for version in versions:
            for records in sizes:
                for mode_index, mode in enumerate(modes):
                    order = ["baseline", "treatment"] if mode_index % 2 == 0 else ["treatment", "baseline"]
                    for arm in order:
                        name = f"pg{version}-{records}-{mode}-{arm}"
                        sample = {"name": name, "version": version, "records": records,
                                  "mode": mode, "arm": arm, "rejections": {}, "observations": {}}
                        evidence["samples"].append(sample)
                        with stable_server(protocol, f"postgres:{version}-alpine", manifest["run_id"] + "-" + name, sample) as server:
                            inspected = json.loads(protocol.command("docker", "inspect", server.name).stdout)[0]
                            sample["effective_resources"] = effective_resource_caps(inspected)
                            port = sample["port"].rsplit(":", 1)[1]
                            server.sql("CREATE DATABASE cache_measure;")
                            env = {**os.environ, "ATTUNE__DATABASE__URL":
                                   f"postgresql://postgres@127.0.0.1:{port}/cache_measure",
                                   "ATTUNE_TEST_RUN_ID": "cm" + uuid.uuid4().hex[:12],
                                   "SQLX_OFFLINE": "true"}
                            command = [str((output / f"{arm}.bin").resolve()), str(records), mode,
                                       arm, str((output / f"{name}.json").resolve())]
                            env["CACHE_MEASURE_CONTROLLED"] = "1"
                            env["CACHE_MEASURE_SMOKE"] = "1" if smoke else "0"
                            start = time.monotonic()
                            with (output / f"{name}.stdout.log").open("w") as stdout, (output / f"{name}.stderr.log").open("w") as stderr:
                                process = subprocess.Popen(command, cwd=output / arm, env=env,
                                                           stdout=stdout, stderr=stderr)
                                try:
                                    seeded = output / f"{name}.seeded"
                                    while not seeded.exists():
                                        if process.poll() is not None:
                                            break
                                        require(time.monotonic() - start < STAGE_TIMEOUT_SECONDS, "seed deadline exceeded")
                                        time.sleep(.05)  # Readiness predicate, not a workload timing delay.
                                    if seeded.exists() and mode == "cold":
                                        sample["postmaster_before_restart"] = server.sql("SELECT pg_postmaster_start_time();")
                                        protocol.command("docker", "restart", server.name, timeout=120)
                                        ready_deadline = time.monotonic() + 120
                                        while True:
                                            ready = protocol.command(*server.psql(), input="SELECT true;", check=False)
                                            if ready.returncode == 0 and ready.stdout.strip() == "t":
                                                break
                                            require(time.monotonic() < ready_deadline, "owned PostgreSQL restart failed")
                                            time.sleep(.05)
                                        sample["port_after_restart"] = protocol.command("docker", "port", server.name, "5432").stdout.strip()
                                        require(sample["port_after_restart"] == sample["port"],
                                                "cold restart changed the running fixture endpoint")
                                        sample["postmaster_after_restart"] = server.sql("SELECT pg_postmaster_start_time();")
                                        require(sample["postmaster_after_restart"] != sample["postmaster_before_restart"],
                                                "cold sample did not restart PostgreSQL")
                                        sample["shared_buffers_restarted"] = True
                                    else:
                                        sample["shared_buffers_restarted"] = False
                                    if seeded.exists():
                                        (output / f"{name}.continue").write_text("owned parent ready\n")
                                    code = process.wait(timeout=STAGE_TIMEOUT_SECONDS)
                                finally:
                                    if process.poll() is None:
                                        process.terminate()
                                        try:
                                            process.wait(timeout=30)
                                        except subprocess.TimeoutExpired:
                                            process.kill()
                                            process.wait(timeout=30)
                            sample.update({"exit_code": code, "wall_seconds": time.monotonic() - start})
                            logs = protocol.command("docker", "logs", server.name, check=False)
                            (output / f"{name}.postgres.log").write_text(logs.stdout + logs.stderr)
                            sample["passed"] = code == 0
                            sample["clones_before_teardown"] = int(server.sql("SELECT count(*) FROM pg_database WHERE datname LIKE 'attune_db_%';"))
                            require(sample["clones_before_teardown"] == 0,
                                    f"benchmark exit {code}; run-owned clone leak before teardown; see {name}.stderr.log")
                            require(code == 0, f"benchmark exit {code}; see {name}.stderr.log")
        evidence["score"] = score(output, evidence["samples"])
    except BaseException as error:
        evidence["error"] = repr(error)
        raise
    finally:
        (output / "run.json").write_text(json.dumps(evidence, indent=2) + "\n")


def score(output, samples):
    pairs = {}
    for sample in samples:
        key = (sample["version"], sample["records"], sample["mode"])
        pairs.setdefault(key, {})[sample["arm"]] = json.loads((output / (sample["name"] + ".json")).read_text())
    rows = []
    for key, pair in pairs.items():
        baseline, treatment = pair["baseline"], pair["treatment"]
        checks = {}
        for operation, target in [("small_point", 250), ("small_page", 250), ("ingest", 1000), ("create", 1000)]:
            calls = [s["ms"] for s in treatment.get("calls", [])
                     if s["operation"] == operation and s["phase"] == "burst"]
            checks[operation] = {"samples": len(calls), "p50_ms": percentile(calls, 50),
                                "p95_ms": percentile(calls, 95), "p99_ms": percentile(calls, 99),
                                "passed": bool(calls) and percentile(calls, 95) < target}
            overlapping = [s["ms"] for s in treatment.get("calls", []) if s["operation"] == operation
                           and s["phase"] == "burst" and s["during_cleanup"]]
            checks[operation]["during_cleanup"] = {"samples": len(overlapping),
                "p50_ms":percentile(overlapping,50),"p95_ms":percentile(overlapping,95),
                "p99_ms":percentile(overlapping,99)}
            if operation != "create":
                checks[operation]["passed"] &= bool(overlapping) and percentile(overlapping,95) < target
        for label, field, threshold in [("cleanup", "ms", .30), ("wal", "wal_bytes", .10)]:
            numerator = treatment.get("isolated_cleanup", {}).get(field)
            denominator = baseline.get("isolated_cleanup", {}).get(field)
            ratio = numerator / denominator if numerator is not None and denominator else None
            checks[label] = {"ratio": ratio, "passed": ratio is not None and ratio <= threshold}
        roots = {}
        for root in [0,1]:
            calls = [s for s in treatment.get("calls",[]) if s["phase"] == "burst"
                     and s["operation"] == f"pin_pair_mutation_root_{root}"]
            values = [s["ms"] for s in calls]
            roots[root] = {"samples":len(calls),"p50_ms":percentile(values,50),
                "p95_ms":percentile(values,95),"p99_ms":percentile(values,99),
                "passed":len(calls) == 200 and all(s["outcome"] == "ok" for s in calls)}
        checks["two_root_pin_burst"] = {"roots":roots,"passed":all(r["passed"] for r in roots.values()),
                                       "latency_target":"reported only; read/ingest/creation SLOs remain unchanged"}
        correct = all(data.get("correct") is True and data.get("cleanup_complete") is True
                      for data in [baseline, treatment])
        gaps = treatment.get("method_gaps", []) + baseline.get("method_gaps", [])
        latencies = {}
        for arm, data in pair.items():
            groups = {}
            for call in data.get("calls", []):
                groups.setdefault(call["phase"] + "/" + call["operation"], []).append(call)
            latencies[arm] = {key: {"samples":len(calls),"p50_ms":percentile([c["ms"] for c in calls],50),
                "p95_ms":percentile([c["ms"] for c in calls],95),"p99_ms":percentile([c["ms"] for c in calls],99),
                "outcomes":{outcome:sum(c["outcome"] == outcome for c in calls)
                            for outcome in sorted({c["outcome"] for c in calls})}}
                for key,calls in groups.items()}
        rows.append({"profile": key, "checks": checks, "latencies":latencies,"correct": correct, "method_gaps": gaps,
                     "passed": correct and not gaps and all(c["passed"] for c in checks.values())})
    expected = {(version,records,mode) for version in ["16","18"]
                for records in [200000,1000000] for mode in ["cold","warm"]}
    complete = set(pairs) == expected and all(set(pair) == {"baseline","treatment"} for pair in pairs.values())
    return {"profiles": rows,"complete_release_coverage":complete,
            "passed": complete and all(r["passed"] for r in rows)}


class PureTests(unittest.TestCase):
    def test_source_digest_is_stable_and_snapshot_excludes_build_outputs(self):
        import tempfile
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-source-test-") as temporary:
            source = Path(temporary) / "assigned"
            source.mkdir()
            for name in SOURCE_ITEMS:
                path = source / name
                if name in {"crates","migrations",".sqlx"}:
                    path.mkdir()
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    path.write_text("owned source fixture\n")
            before = source_identity(source)
            (source / ".sqlx").rmdir()
            self.assertEqual(source_identity(source), before)
            without_root_cache = Path(temporary) / "without-root-cache"
            snapshot(without_root_cache, source)
            self.assertEqual(fingerprint(without_root_cache), before["files"])
            root_cache = source / ".sqlx"
            root_cache.mkdir()
            query = root_cache / "query-fixture.json"
            query.write_text('{"query":"SELECT 1"}\n')
            with_root_cache = source_identity(source)
            self.assertNotEqual(with_root_cache["source_sha256"], before["source_sha256"])
            self.assertIn(".sqlx/query-fixture.json", with_root_cache["files"])
            query.unlink()
            self.assertEqual(source_identity(source), before)
            target = source / "crates/target"
            target.mkdir()
            (target / "ignored-build").write_text("ignored\n")
            self.assertEqual(source_identity(source),before)
            copied = Path(temporary) / "copied"
            snapshot(copied,source)
            self.assertEqual(fingerprint(copied),before["files"])
            (source / "Cargo.toml").write_text("changed\n")
            self.assertNotEqual(source_identity(source)["source_sha256"],before["source_sha256"])

    def test_summary_is_small_and_cannot_overwrite_evidence(self):
        import tempfile
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-summary-test-") as temporary:
            path = Path(temporary) / "summary.json"
            small_summary(path, {"status":"prepared"})
            with self.assertRaises(FileExistsError):
                small_summary(path, {"status":"replacement"})
            with self.assertRaisesRegex(RuntimeError,"exceeds 4 KiB"):
                small_summary(Path(temporary) / "large.json", {"error":"x" * 5000})

    def test_ram_backed_output_is_rejected(self):
        from unittest.mock import patch
        with patch(__name__ + ".filesystem_type", return_value=("tmpfs","/tmp")):
            with self.assertRaisesRegex(RuntimeError,"RAM-backed"):
                disk_storage(Path("/tmp/opencode/future-output"),"benchmark output")

    def test_effective_caps_are_verified_not_just_reported(self):
        inspected = {"HostConfig":{"Memory":PG_MEMORY_BYTES,"MemorySwap":PG_MEMORY_BYTES,
                                   "NanoCpus":PG_NANO_CPUS,"ShmSize":PG_SHM_BYTES}}
        self.assertEqual(effective_resource_caps(inspected)["memory_bytes"],PG_MEMORY_BYTES)
        inspected["HostConfig"]["Memory"] *= 2
        with self.assertRaisesRegex(RuntimeError,"did not provision"):
            effective_resource_caps(inspected)

    def test_nested_ram_mount_and_escaped_mount_names(self):
        mounts = ("1 0 8:1 / / rw - ext4 /dev/sda rw\n"
                  "2 1 0:1 / /tmp rw - tmpfs tmpfs rw\n"
                  "3 1 8:2 / /home rw - ext4 /dev/sdb rw\n"
                  r"4 1 8:3 / /mnt/disk\040name rw - xfs /dev/sdc rw" + "\n")
        self.assertEqual(filesystem_type(Path("/tmp/future-output"),mounts)[0],"tmpfs")
        self.assertEqual(filesystem_type(Path("/home/future-output"),mounts)[0],"ext4")
        self.assertEqual(filesystem_type(Path("/mnt/disk name/output"),mounts)[0],"xfs")

    def test_percentiles_keep_tail(self):
        self.assertEqual(percentile(list(range(1, 101)), 99), 99)
        self.assertEqual(percentile([10000], 95), 10000)
        self.assertIsNone(percentile([], 95))

    def test_replacement_rejects_drift(self):
        with self.assertRaises(RuntimeError):
            replace_once("x x", "x", "y")

    def test_method_extraction(self):
        text = "impl R {\n    pub async fn a() {\n        if true { }\n    }\n}"
        self.assertTrue(function(text, "a").endswith("\n    }"))

    def test_empty_or_partial_coverage_cannot_pass(self):
        self.assertFalse(score(Path("/tmp/opencode"), [])["passed"])
        import tempfile
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-score-test-") as temporary:
            output = Path(temporary)
            data = {"correct":True,"cleanup_complete":True,"method_gaps":[],
                "calls":[{"phase":"burst","operation":op,"ms":1,"during_cleanup":True,"outcome":"ok"}
                         for op in ["small_point","small_page","ingest","create"]],
                "isolated_cleanup":{"ms":100,"wal_bytes":1000}}
            data["calls"].extend({"phase":"burst","operation":f"pin_pair_mutation_root_{root}",
                                  "ms":1,"during_cleanup":True,"outcome":"ok"}
                                 for root in [0,1] for _ in range(200))
            (output / "baseline.json").write_text(json.dumps(data))
            data["isolated_cleanup"] = {"ms":1,"wal_bytes":1}
            (output / "treatment.json").write_text(json.dumps(data))
            samples = [{"version":"16","records":200000,"mode":"warm","arm":arm,"name":arm}
                       for arm in ["baseline","treatment"]]
            result = score(output, samples)
            self.assertTrue(result["profiles"][0]["passed"])
            self.assertFalse(result["complete_release_coverage"])
            self.assertFalse(result["passed"])

    def test_historical_baseline_adapter(self):
        # This verifies source transformation without Docker or Cargo work.
        import tempfile
        historical = Path("/tmp/opencode/native-workload-current-final/source")
        with tempfile.TemporaryDirectory(dir="/tmp/opencode", prefix="cache-adapter-test-") as temporary:
            target = Path(temporary)
            (target / REPO).parent.mkdir(parents=True)
            (target / REPO).write_bytes((ROOT / REPO).read_bytes())
            (target / STORAGE).parent.mkdir(parents=True)
            (target / STORAGE).write_bytes((ROOT / STORAGE).read_bytes())
            (target / "migrations").mkdir()
            for name in [CACHE, ACCOUNTING, PARTITIONS]:
                (target / "migrations" / name).write_bytes((ROOT / "migrations" / name).read_bytes())
            heap_baseline(target, historical)
            self.assertNotIn("PARTITION BY LIST", (target / "migrations" / CACHE).read_text())
            source = (target / REPO).read_text()
            self.assertIn("e.generation = $1 AND e.id = c.id", source)
            self.assertIn("refresh_concurrency", source)
            self.assertIn("delete_cleanup_batch(&mut tx, generation_id, 1000)", source)
            self.assertIn("bounded.max_cleanup_cycle_milliseconds = remaining;", source)
            self.assertIn("async fn cleanup_heap_step(", source)
            self.assertIn("retained_iterations BETWEEN 0 AND 10000",
                          (target / "migrations" / PARTITIONS).read_text())
            current = (ROOT / "migrations" / PARTITIONS).read_text()
            prefix = current[:current.index("CREATE FUNCTION cache_generation_partition_name(")]
            self.assertTrue((target / "migrations" / PARTITIONS).read_text().startswith(prefix))
            self.assertIn("REFERENCING OLD TABLE AS removed_cache_iterations", prefix)
            self.assertIn("ORDER BY u.generation FOR UPDATE OF u", prefix)
            self.assertIn("FOR EACH STATEMENT EXECUTE FUNCTION release_cache_iteration_metadata()", prefix)
            protocol = function((ROOT / REPO).read_text(), "protect_transaction")
            self.assertEqual(function(source,"protect_transaction"), heap_creation_protection(protocol))
            self.assertIn("CacheTransactionMode::PinMutation", protocol)
            self.assertIn("!matches!(mode, CacheTransactionMode::Read)", protocol)
            admission = admission_protocol((ROOT / REPO).read_text())
            self.assertEqual(admission_protocol(source),admission)
            self.assertIn('pg_advisory_xact_lock($1, $2)',admission)
            self.assertIn('CACHE_ADMISSION_ADVISORY_LOCK_CLASS: i32 = 7_821_101;',admission)
            self.assertIn('CACHE_ADMISSION_ADVISORY_LOCK_KEY: i32 = 0;',admission)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""Examples:
  python3 scripts/measure-cache-generation-partitions.py --self-test
  python3 /assigned/source/scripts/measure-cache-generation-partitions.py --source-digest --source-root /assigned/source
  python3 /assigned/source/scripts/measure-cache-generation-partitions.py --prepare --source-root /assigned/source --source-sha256 ASSIGNED_HASH --output /home/david/.cache/attune-release-evidence/cache-measure-unique
  python3 /assigned/source/scripts/measure-cache-generation-partitions.py --run --source-sha256 ASSIGNED_HASH --exclusive-lane-granted --cargo-target /mnt/wdc/attune-worktrees/remove-timescaledb/target --output /home/david/.cache/attune-release-evidence/cache-measure-unique --summary-output /tmp/opencode/cache-measure-unique-summary.json
  python3 /assigned/source/scripts/measure-cache-generation-partitions.py --run --smoke --versions 16 --modes cold --source-sha256 ASSIGNED_HASH --exclusive-lane-granted --cargo-target /mnt/wdc/attune-worktrees/remove-timescaledb/target --output /home/david/.cache/attune-release-evidence/cache-measure-smoke-unique

Preparation copies source and generates a diff; it starts no database.
Preparation and execution require the coordinator-assigned source digest.
Run requires the coordinator's exclusive lane, uses only uniquely labeled
containers/volumes, and refuses any reused run or evidence path.
Output and Cargo targets must be disk-backed, not /tmp tmpfs. PostgreSQL runs
without source bind mounts; any separate Docker-built E2E lane must verify
Docker Desktop shares its snapshot path without changing preferences here.
Exit 0 means the runner completed; read run.json score.passed for acceptance.
""")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--source-root", type=Path, help="assigned immutable final source snapshot; required for preparation")
    parser.add_argument("--source-sha256", help="coordinator-assigned selected-source digest; required for prepare/run")
    parser.add_argument("--source-digest", action="store_true", help="read-only digest of the assigned --source-root")
    parser.add_argument("--summary-output", type=Path, help="optional new summary file, at most 4 KiB; RAM-backed storage allowed")
    parser.add_argument("--cargo-target", type=Path, default=ROOT / "target",
                        help="disk-backed Cargo target, defaults to the shared worktree target")
    parser.add_argument("--historical-source", type=Path,
                        default=Path("/tmp/opencode/native-workload-current-final/source"))
    parser.add_argument("--prepare", action="store_true")
    parser.add_argument("--run", action="store_true")
    parser.add_argument("--exclusive-lane-granted", action="store_true")
    parser.add_argument("--versions", nargs="+", choices=["16", "18"], default=["16", "18"])
    parser.add_argument("--sizes", nargs="+", type=int, default=[200000, 1000000])
    parser.add_argument("--modes", nargs="+", choices=["cold", "warm"], default=["cold", "warm"])
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--smoke", action="store_true", help="1000-record fixture preflight, never release acceptance")
    args = parser.parse_args()
    if args.self_test:
        suite = unittest.defaultTestLoader.loadTestsFromTestCase(PureTests)
        raise SystemExit(not unittest.TextTestRunner(verbosity=2).run(suite).wasSuccessful())
    if args.source_root is not None:
        args.source_root = args.source_root.expanduser().resolve()
    if args.source_digest:
        require(args.source_root is not None and not args.prepare and not args.run,
                "--source-digest requires --source-root and no prepare/run")
        storage = disk_storage(args.source_root,"assigned source snapshot")
        identity = source_identity(args.source_root)
        print(json.dumps({"source":str(args.source_root),"source_sha256":identity["source_sha256"],
                          "selected_files":len(identity["files"]),"storage":storage}))
        return
    require(args.output is not None,
            "--output required; example: --prepare --output /home/david/.cache/attune-release-evidence/cache-measure-unique")
    args.output = args.output.expanduser().resolve()
    args.historical_source = args.historical_source.expanduser().resolve()
    args.cargo_target = args.cargo_target.expanduser().resolve()
    if args.summary_output is not None:
        args.summary_output = args.summary_output.expanduser().resolve()
        args.summary_output.parent.resolve(strict=True)
        require(not args.summary_output.exists(), "refusing existing summary output")
    require(args.prepare != args.run, "choose exactly one of --prepare or --run")
    require(args.source_sha256 is not None and len(args.source_sha256) == 64
            and all(c in "0123456789abcdef" for c in args.source_sha256),
            "--source-sha256 must be the coordinator-assigned 64-character lowercase SHA256")
    require(not args.prepare or args.source_root is not None, "--prepare requires assigned --source-root")
    require(args.sizes == [200000, 1000000], "release comparison requires both declared sizes")
    require(len(set(args.versions)) == len(args.versions) and len(set(args.modes)) == len(args.modes),
            "duplicate versions or modes would reuse evidence paths")
    try:
        if args.prepare:
            prepare(args.output, args.historical_source, args.source_root, args.source_sha256)
            summary = {"status":"prepared","manifest":str(args.output / "manifest.json")}
        else:
            run(args.output, args.versions, [1000] if args.smoke else args.sizes, args.modes,
                args.exclusive_lane_granted, args.smoke, args.cargo_target, args.source_sha256)
            evidence = json.loads((args.output / "run.json").read_text())
            summary = {"status":"completed","output":str(args.output),
                       "acceptance_passed":evidence["score"]["passed"],
                       "performance_scope":evidence["performance_scope"]}
    except BaseException as error:
        small_summary(args.summary_output, {"status":"failed","output":str(args.output),
                                           "error":str(error)[:1000]})
        raise
    small_summary(args.summary_output, summary)
    print(json.dumps(summary))


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(f"Error: {error}", file=sys.stderr)
        raise SystemExit(2)
