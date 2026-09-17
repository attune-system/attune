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

Some Rust tests marked `#[ignore]` require PostgreSQL/TimescaleDB or other services:

```bash
ATTUNE_TEST_RUN_ID=local1 \
  cargo test -p attune-common --test test_database_lifecycle_tests -- \
  --ignored --test-threads=1
```

`ATTUNE_TEST_RUN_ID` must be 1–20 lowercase ASCII letters/digits, optionally with non-leading `-`. It makes database clones and the migration template attributable to one invocation, for example `attune_db_local1_<uuid>` and `attune_tpl_local1_<migration-hash>`.

## Docker-owned integration runs

Use the runner scripts rather than raw Compose. They generate or require explicit run identities, refuse existing project resources before startup/build, remove only stacks they started, and do not publish fixed host ports.

```bash
# Rust ignored/integration tests against an owned PostgreSQL project
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

# Optional bounded concurrency; certify the selected class before adopting it
ATTUNE_RUST_TEST_THREADS=2 \
  bash scripts/run-rust-integration-tests.sh --crate common

# Target one executable at a larger candidate budget
ATTUNE_RUST_TEST_THREADS=8 \
  bash scripts/run-rust-integration-tests.sh --test action_repository_tests
```

The Rust image uses a small runtime stage and stores only stripped executables containing ignored tests, not the Rust toolchain or Cargo's incremental directory. Its `inventory.tsv` is the exact artifact fingerprint. A normal runtime invocation executes those binaries directly and does not compile. Changes limited to the entrypoint or other runtime Docker fixtures do not invalidate the workspace compile layer.

The database/broker lane excludes tests that declare additional API, MinIO, installed-CLI, or high-load prerequisites. Those identities remain in the image and in their owning CI/E2E lanes; they are not silently discovered or conditionally skipped. To run them after provisioning every prerequisite:

```bash
ATTUNE_RUST_INCLUDE_EXTERNAL=1 \
  bash scripts/run-rust-integration-tests.sh --no-build
```

External mode includes SSE tests requiring a live API, runtime-log/S3 tests requiring the owned MinIO harness and `ATTUNE_TEST_S3_*` credentials, the installed-CLI profile test, and explicit high-concurrency stress tests.

Docker Desktop must share the checkout path with its VM. If a checkout is on an unshared external mount, bind-mounted migration/pack files may appear empty; add that path to Docker Desktop file sharing before running E2E tests.

## Concurrency policy

- Default database/service integration concurrency is **1**.
- The full template-cloned common lane has passed at **1** and **4** threads. Representative action, execution, and cache repository binaries have also passed at **8** threads. The default remains serial until repeated whole-workspace gates complete.
- `action_repository_tests` is the first explicit hybrid rollback-isolated binary. The Docker runner gives it one migrated database per executable, 18 tests create runtime-local pools and transactions, and the two timestamp-update tests retain physical clones. Its 20 tests passed in 4.24 seconds serially, 3.25 seconds at four threads, and 2.68 seconds at eight threads while preserving individual test identities.
- Pure Rust and Vitest suites may use their native defaults.
- Notification, retention, migration, service-restart, broker topology, and load tests remain serial/exclusive unless their owning fixture documents a stronger gate.
- Use unique run IDs for overlapping invocations. Never share a mutable Compose project, schema prefix, RabbitMQ vhost, filesystem root, or host port.

The migration-per-schema runner was retired after the full serial suite exceeded 3.5 hours. On the measured 4-vCPU/16-GiB Rancher Desktop host, a fully migrated template took 10.26 seconds once and physical clones took 172–261 ms (178 ms median). Two warm lifecycle tests fell from 16.12 seconds to 0.72 seconds. The common crate's 626 selected tests took 341 seconds serially and 208 seconds at four threads, with zero clone leaks; both runs exposed the same independently owned S3 prerequisite failure rather than hiding it.

On Docker Desktop with 8 CPUs and 16 GiB allocated, the same common-crate lane passed in 685 seconds serially and 424 seconds at four threads, a 38% reduction. The fresh-migration binary remained included and took 168 seconds serially and 102 seconds at four threads. Ten warm three-test lifecycle runs took 1.79–2.39 seconds inside the test binary and left zero clones and sessions before janitor recovery.

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
3. Check concurrency 1/2/4 before adopting a parallel setting.
4. Exercise normal teardown, partial setup failure, panic, cancellation, and held-resource paths.
5. Run two overlapping unique projects and preserve a dirty-neighbor sentinel.
6. Compare identical test inventories; do not credit skipped tests, hidden retries, or omitted setup.
7. Run `cargo check --all-targets --workspace` with zero warnings.

See [Template-cloned test databases](schema-per-test.md) and [Test concurrency reliability](../plans/test-concurrency-reliability.md).
