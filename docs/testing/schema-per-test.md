# Template-cloned test databases

**Status:** Implemented with owned, bounded teardown

**Updated:** 2026-10-06

## Contract

Database-backed Rust tests use `attune_common::test_database::TestDatabase`. The fixture preserves a production-faithful `attune` schema but no longer replays every migration for every test:

1. immutable migration text is loaded and hashed;
2. one run-owned template database is created for that migration hash;
3. canonical migrations run once in the template;
4. the template rejects connections;
5. every test receives a unique physical clone through `CREATE DATABASE ... TEMPLATE ...`;
6. the clone gets its own SQLx pool and is force-dropped by its owner.

With `ATTUNE_TEST_RUN_ID=local1`, resources are named:

```text
attune_tpl_local1_<migration-hash>
attune_db_local1_<uuid>
```

The run ID must match `^[a-z0-9][a-z0-9-]{0,19}$`. Hyphens are encoded as underscores only after underscores have been excluded from valid input, so accepted IDs cannot normalize to the same ownership prefix. Local tests without a run ID use the `local` token; CI and Docker runners always set an explicit ID.

Database cloning requires the configured PostgreSQL role to have `CREATEDB` (the disposable Docker/CI role is the database owner/superuser). Tests must target an explicitly disposable PostgreSQL cluster.

Stock PostgreSQL 16 or newer is required, with PostgreSQL 18 as the Compose and
CI default. No TimescaleDB extension or background-worker management is required.

## Why database clones

The former schema fixture replayed 54 migrations for each of 937 ignored tests. A simple repository assertion spent about 6.7 seconds in setup, and later tests degraded toward 17 seconds each. On the 4-vCPU Rancher validation host:

- building a fully migrated template took 10.26 seconds once;
- five physical clones took 172–261 ms each (178 ms median);
- two warm lifecycle tests completed in 0.72 seconds versus 16.12 seconds with migration-per-test;
- 626 common-crate tests completed in 341 seconds serially, including 45 seconds of explicit migration-fidelity tests.

These timing samples predate TimescaleDB removal. They are historical baselines,
not measurements of the PostgreSQL-only schema. Current fixtures do not start
or stop TimescaleDB background workers.

A database clone is a stronger isolation boundary than a shared database with
separate schemas. It preserves tables, views, indexes, triggers, functions,
constraints, and committed transaction behavior without truncation or rollback
approximations.

## Ownership

Keep the `TestDatabase` owner for the fixture lifetime. Do not clone a `PgPool` and discard the owner.

```rust
let database = TestDatabase::create(&config.database)
    .await?
    .with_cleanup_on_drop();
let pool = database.pool().clone();

// ... test ...

database.cleanup().await?;
```

`database.schema()` remains `attune`; `database.database_name()` and `database.database_url()` identify the owned clone when lifecycle tests need to observe it.

API `TestContext` retains the owner and stops router/audit tasks before database teardown. Bare fixtures that cannot expose explicit teardown use `with_cleanup_on_drop()`. Drop is recovery, not proof that a successful path awaited cleanup.

## Teardown behavior

`TestDatabase::cleanup()` is bounded and ordered:

1. close the clone pool with a 5-second bound;
2. connect to the cluster administrator database with a 10-second bound;
3. force-drop the exact generated clone with a 120-second bound, including any checkpoint PostgreSQL requires;
4. aggregate and return cleanup errors.

`DROP DATABASE ... WITH (FORCE)` terminates sessions only in the exact owned clone; neighboring databases are untouched. The drop fallback performs the same exact cleanup on a joined helper thread so panic/partial-construction recovery does not detach a writer.

Physical clone creation and drop can force cluster-wide checkpoints. On container storage, a four-thread PostgreSQL 18 run spent 37 seconds syncing a required checkpoint while drops waited on `CheckpointStart` and `CheckpointDone`. The DDL deadline allows that work to finish without changing test assertions, adding retries, or suppressing cleanup failures.

`crates/common/tests/test_database_lifecycle_tests.rs` covers explicit cleanup, panic/drop recovery, and a held lock/checkout. `crates/api/tests/authz_cache_isolation_tests.rs` covers partial API construction and equal-primary-ID cache isolation.

## Template lifecycle and migration fidelity

The migration hash is part of the template name, so changed migration content cannot reuse stale state. Template creation is protected by a bounded advisory lock. An incomplete template is rejected and rebuilt; a completed template has `ALLOW_CONNECTIONS false` and `IS_TEMPLATE true`.

Templates are run-level build artifacts rather than per-test leaks. Docker-owned runs remove them with the project volume. CI's owner-scoped finalizer removes the exact run template after checking that no per-test clones leaked.

Migration behavior is still tested separately. `migration_tests` creates fresh databases for six migration-history and upgrade cases. Five DDL, committed-state, and cross-connection cases retain physical clones. Nineteen read-only checks and eleven rollback-safe cases share one runner-owned migrated database through runtime-local pools. Direct Cargo runs fall back to physical per-test clones. Do not move a fresh or physical case into the shared fixture for a benchmark gain.

## Concurrency

Template creation is serialized once; clone use is independent. The common-crate lane has passed its four-thread timing, session, and leak gates. Managed Make, Docker, benchmark, and CI entry points enforce at least four libtest threads; direct Cargo callers must set that concurrency themselves. Executables still run sequentially because overlapping migration DDL with repository tests caused a large regression.

Use unique run IDs for overlapping invocations. Database names, Compose projects, RabbitMQ vhosts, filesystem roots, and external service resources must remain disjoint.

## Repository rules

- Repository SQL remains unqualified and relies on `search_path`.
- Never hardcode schema prefixes in repository SQL.
- Never use `SELECT *` for evolving SQLx `FromRow` models.
- Schema changes still require `cargo sqlx prepare`.
- Runtime, history, and audit tables are ordinary PostgreSQL tables. Preserve intentionally dangling IDs so related records can have independent retention periods.

## Owner-scoped janitor

Normal test cleanup must leave zero `attune_db_<run>_...` clones before janitor recovery. For an interrupted run:

```bash
ATTUNE_TEST_RUN_ID=local1 \
DATABASE_URL=postgresql://attune:attune@localhost:5432/attune_test \
  bash scripts/cleanup-test-schemas.sh --force
```

Despite its historical filename, the utility now removes exact run-owned clones, the run template, and legacy schema fixtures. It refuses broad prefixes, invalid IDs, PostgreSQL errors, and no-progress cleanup.

CI records baseline counts for clones, templates, legacy schemas, and sessions.
Per-test database or schema leftovers fail the gate even when janitor recovery
succeeds. A single run template is expected and removed after clone leak detection.

See [Running tests](running-tests.md) for runner commands and validated timings.
