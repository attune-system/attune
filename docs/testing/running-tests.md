# Running Tests

Attune has pure unit tests, template-cloned PostgreSQL tests, broker/service tests, and full Docker E2E tests. Do not infer infrastructure independence from a Rust target's location or from `--lib`.

## Fast local checks

```bash
cargo fmt --all -- --check
cargo check --all-targets --workspace
(cd web && npm test)
python3 -m unittest packs.core.tests.test_actions.TestHttpRequestAction
bash scripts/test-test-isolation-guards.sh
```

Run crate tests with Cargo or the existing Make targets:

```bash
make test-common
make test-api
make test-executor
make test-worker
make test-sensor
make test-cli
```

Database-backed Rust tests run normally and require PostgreSQL/TimescaleDB:

```bash
ATTUNE_TEST_RUN_ID=local1 \
  cargo test -p attune-common --test test_database_lifecycle_tests -- \
  --test-threads=4
```

`ATTUNE_TEST_RUN_ID` must be 1–20 lowercase ASCII letters/digits, optionally with non-leading `-`. It makes database clones and the migration template attributable to one invocation, for example `attune_db_local1_<uuid>` and `attune_tpl_local1_<migration-hash>`.

## Docker-owned integration runs

Use the runner scripts rather than raw Compose. They generate or require explicit run identities, refuse existing project resources before startup/build, remove only stacks they started, and do not publish fixed host ports.

```bash
# Rust integration tests against an owned PostgreSQL project
bash scripts/run-rust-integration-tests.sh --crate common
bash scripts/run-rust-integration-tests.sh --crate api --filter test_name
bash scripts/run-rust-integration-tests.sh --test action_repository_tests

# Full service stack plus Python E2E
bash scripts/run-integration-tests.sh --tier 1
bash scripts/run-integration-tests.sh --tier 3 -k websocket
```

Useful controls:

```bash
# Keep an owned stack for inspection
ATTUNE_E2E_RUN_ID=debug1 ATTUNE_E2E_PROJECT_NAME=attune-debug1 \
  bash scripts/run-integration-tests.sh --no-teardown --tier 1

# Attach without claiming teardown; an explicit disposable project is mandatory
ATTUNE_E2E_RUN_ID=debug-attach ATTUNE_E2E_PROJECT_NAME=attune-debug1 \
  bash scripts/run-integration-tests.sh --no-startup --no-build --tier 1

# Database-backed tests require at least four libtest threads
ATTUNE_RUST_TEST_THREADS=4 \
  bash scripts/run-rust-integration-tests.sh --crate common

# Target one executable at a larger candidate budget
ATTUNE_RUST_TEST_THREADS=8 \
  bash scripts/run-rust-integration-tests.sh --test action_repository_tests

# Collect the standard three-cold/five-warm common-crate baseline
bash scripts/benchmark-rust-integration-tests.sh \
  /tmp/attune-rust-integration-benchmark.tsv
```

The Rust image uses a small runtime stage and stores stripped test executables, not the Rust toolchain or Cargo's incremental directory. Its `inventory.tsv` is the exact runnable-test fingerprint. A normal runtime invocation executes those binaries directly and does not compile. Changes limited to the entrypoint or other runtime Docker fixtures do not invalidate the workspace compile layer.

The Docker lane intentionally uses libtest rather than cargo-nextest. An earlier 0.9.145 prototype matched the then-ignored database-test identities but was 7.2% slower by median test time across three equal 591-test common-crate samples. The stock nextest archive was also larger than the complete runtime image. See [Test concurrency reliability](../plans/test-concurrency-reliability.md#cargo-nextest-scheduler-investigation) for the coverage map and measurements.

The database/broker lane excludes tests that declare additional API, S3, installed-CLI, or high-load prerequisites. Those identities remain in the image and in their owning CI/E2E lanes; they are not silently discovered or conditionally skipped. To run them after provisioning every prerequisite:

```bash
ATTUNE_RUST_INCLUDE_EXTERNAL=1 \
  bash scripts/run-rust-integration-tests.sh --no-build
```

External mode includes SSE tests requiring a live API, runtime-log/S3 tests requiring the owned RustFS harness and `ATTUNE_TEST_S3_*` credentials, the installed-CLI profile test, and explicit high-concurrency stress tests.

For the versioned S3 correctness suite, including artifact preview across API replicas, follow [Runtime log verification](../deployment/runtime-log-verification.md#run-the-correctness-suite). Host database setup uses `make db-test-setup` and requires `psql` and `sqlx`. Set both `TEST_DB_ADMIN_URL` and `TEST_DB_URL` to your disposable PostgreSQL cluster. Direct Cargo runs use `ATTUNE__DATABASE__URL` for the test database connection.

Docker Desktop must share the checkout path with its VM. If a checkout is on an unshared external mount, bind-mounted migration/pack files may appear empty; add that path to Docker Desktop file sharing before running E2E tests.

## Concurrency policy

- Default database/service integration concurrency is **4**. The Make, Docker, benchmark, and CI entry points reject lower values.
- The full template-cloned common lane is certified at **4** threads. After the migration fixture optimization, three warm samples had a 335.865-second median and passed the predeclared 393-second adoption target. Representative action, execution, and cache repository binaries have passed at **8** threads.
- `action_repository_tests` is the first explicit hybrid rollback-isolated binary. The Docker runner gives it one migrated database per executable, 18 tests create runtime-local pools and transactions, and the two timestamp-update tests retain physical clones. Its 20 tests passed in 4.24 seconds serially, 3.25 seconds at four threads, and 2.68 seconds at eight threads while preserving individual test identities.
- `migration_tests` uses one runner-owned migrated database for 19 read-only tests and 11 rollback-isolated tests. Five DDL, committed-state, or cross-connection tests retain physical clones, and six migration-history tests retain fresh databases. Its 41 selected identities passed with medians of 24.714 seconds at one thread and 27.325 seconds at four threads, compared with the previous 104.09-second four-thread median.
- Pure Rust and Vitest suites may use their native defaults.
- Test executables still run sequentially. Notification, retention, service-restart, broker topology, and load tests keep exclusive external resources while libtest runs eligible tests with at least four threads. The migration executable uses four libtest threads but remains exclusive from other executables because concurrent database DDL caused a measured regression.
- Use unique run IDs for overlapping invocations. Never share a mutable Compose project, schema prefix, RabbitMQ vhost, filesystem root, or host port.

The migration-per-schema runner was retired after the full serial suite exceeded 3.5 hours. On the measured 4-vCPU/16-GiB Rancher Desktop host, a fully migrated template took 10.26 seconds once and physical clones took 172–261 ms (178 ms median). Two warm lifecycle tests fell from 16.12 seconds to 0.72 seconds. The common crate's 626 selected tests took 341 seconds serially and 208 seconds at four threads, with zero clone leaks; both runs exposed the same independently owned S3 prerequisite failure rather than hiding it.

On Docker Desktop with 8 CPUs and 16 GiB allocated, the same common-crate lane passed in 685 seconds serially and 424 seconds at four threads, a 38% reduction. A later repeated four-thread baseline selected 625 tests from the 938-test inventory in every run. Three fresh-stack samples had a 487.427-second total median and 434.644–524.603-second range. Five retained-stack samples had a 436.721-second total median and 431.328–440.155-second range. All samples passed and left zero run-owned clones, sessions, and schemas before teardown. Peak run-owned PostgreSQL sessions ranged from 11 to 15. The fresh-migration binary remained included and took 168 seconds serially and 102 seconds at four threads. Ten warm three-test lifecycle runs took 1.79–2.39 seconds inside the test binary and left zero clones and sessions before janitor recovery.

The initial common-lane pilot used selected-test fingerprint `fb6cb3ce8b72430eb2fcddd0c67aef7a76a8a7e747a705a44cc371b10b1b223f`. One thread had a 599.152-second warm median. Two threads had a 531.906-second warm median. Four threads had a 422.405-second warm median, which missed the 393-second target.

After the migration fixture optimization, three four-thread warm samples retained the same 625-test fingerprint and completed in 332.557, 354.038, and 335.865 seconds. The 335.865-second median passes the target. One fresh-stack sample completed in 358.932 seconds, below the earlier 446.831-second cold median. Peak sessions were 11–12, and every sample reported zero run-owned clone, migration-database, session, and schema leaks before teardown.

A later 1+3 executable-overlap prototype made the common lane slower at 456.911 seconds because `migration_tests` grew to 272.83 seconds under concurrent database DDL. The runner does not overlap executables. Fresh migration databases now include the run token in their names, and benchmark resource checks count their databases and sessions.

Stopping each clone's TimescaleDB background workers before teardown reduced a two-test lifecycle sample from 14.43 seconds to 1.54 seconds on Docker Desktop. The multi-stage image and stronger executable stripping reduced image size from 5.07 GB to 2.44 GB; ignored-test artifacts fell from 2.1 GB to 1.6 GB. A runtime-harness-only rebuild now takes about 26 seconds without recompiling Rust.

## Cleanup and leak checks

Normal success must clean its own resources before any janitor runs. `TestDatabase::cleanup()` is the explicit async path; cleanup-on-drop is panic/partial-construction recovery.

Manual database/template cleanup is deliberately owner-scoped:

```bash
ATTUNE_TEST_RUN_ID=local1 \
DATABASE_URL=postgresql://attune:attune@localhost:5432/attune_test \
  bash scripts/cleanup-test-schemas.sh --force
```

Despite its historical filename, the utility removes exact run-owned database clones, the run template, and legacy schemas. It refuses broad prefixes, invalid/non-canonical run IDs, PostgreSQL errors, and no-progress batches. CI uses `scripts/ci-test-db-safety.sh` with the same ownership contract and fails if per-test clones existed before janitor recovery.

For Compose runs, inspect only resources with the invocation's `com.docker.compose.project` label. Never remove a neighboring developer or CI project's containers, volumes, networks, queues, rows, or files.

## Validation expectations

Before completing changes to test infrastructure:

1. Run focused tests that can fail for the changed behavior.
2. Repeat race-sensitive tests in varied order.
3. Check the required four-thread baseline and any higher candidate setting before adopting it.
4. Exercise normal teardown, partial setup failure, panic, cancellation, and held-resource paths.
5. Run two overlapping unique projects and preserve a dirty-neighbor sentinel.
6. Compare identical test inventories; do not credit skipped tests, hidden retries, or omitted setup.
7. Run `cargo check --all-targets --workspace` with zero warnings.

See [Template-cloned test databases](schema-per-test.md) and [Test concurrency reliability](../plans/test-concurrency-reliability.md).
