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

Database-backed Rust tests run normally and require stock PostgreSQL 16 or newer.
Compose and CI default to PostgreSQL 18. No TimescaleDB extension or preload is
required. Use a disposable cluster and follow
[PostgreSQL-only deployment](../deployment/postgresql-only.md) for fresh-install and reset decisions.

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

Docker Desktop must share the checkout path with its VM. Unshared external mounts
can produce denied or empty migration/pack bind mounts. Either explicitly share
the path, or run from an owned source snapshot in an already shared directory.
Verify snapshot hashes against the worktree, retain run evidence, and remove only
that snapshot and its owned test resources. Do not change Docker preferences
automatically. A startup mount failure is not a test failure or a passing run.

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

The timing samples above predate TimescaleDB removal and are historical
baselines, not PostgreSQL-only performance measurements. Current fixtures do
not start or stop TimescaleDB background workers. The historical multi-stage
image and executable-stripping measurements reduced image size from 5.07 GB
to 2.44 GB; ignored-test artifacts fell from 2.1 GB to 1.6 GB.

## Native partition and summary validation

Use stock PostgreSQL 16 and 18 for native-maintenance correctness checks. The
schema has daily RANGE parents for `event`, `execution_history`, and `audit_event`,
plus DEFAULT partitions. `worker_history` and `sensor_process_history` remain
ordinary tables. Test roles that exercise DDL need schema CREATE and effective
table-owner privileges. Production native maintenance does not require superuser
or database creation. The test fixture needs permission to create owned databases.

### Cache generation partition protocol gate

Cache generation partition storage is proposed separately from the daily RANGE
parents. Before enabling it, run the isolated SQL lock/DDL model:

```bash
python3 scripts/probe-cache-generation-partitions.py --versions 16 18 \
  --output /tmp/opencode/cache-partition-protocols.json
```

The probe uses cached images, unique owned containers/volumes, and non-superuser
table-owner SQL. It deliberately reproduces rejected row-before-parent lock
orders and expects SQLSTATE `40P01` for those cases. Parent-first protocols must
complete without deadlock; lock contention must roll back without releasing byte
usage. JSON records all failures and cleanup, and an existing output file is never
overwritten. It tests a SQL model, not the production executor or deployment-role
configuration. See [the implementation gates](../plans/cache-generation-partitioning.md).

The cache repository suite also contains a transaction-local entry-table shadow
that permits duplicate numeric IDs across generations, proving generation-aware
byte-bounded joins without changing the installed schema:

```bash
bash scripts/run-rust-integration-tests.sh --test cache_repository_tests
```

### Runtime partition and summary suites

Run focused database suites through the existing owned runner:

```bash
bash scripts/run-rust-integration-tests.sh --test native_partition_repository_tests
bash scripts/run-rust-integration-tests.sh --test native_summary_repository_tests
bash scripts/run-rust-integration-tests.sh --test native_producer_repository_tests
bash scripts/run-rust-integration-tests.sh --test analytics_repository_tests
bash scripts/run-rust-integration-tests.sh --test native_retention_api_tests
bash scripts/run-rust-integration-tests.sh --test dashboard_acceptance_tests
```

Direct Cargo invocations need the disposable database setup described above and
a different `ATTUNE_TEST_RUN_ID` for each invocation. Keep at least four threads.
For supervisor cadence, leader replacement, failure accounting, and stale-cache
metadata checks:

```bash
ATTUNE_TEST_RUN_ID=native-supervisor \
  cargo test -p attune-supervisor -- --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=native-stale-cache \
  cargo test -p attune-api stale_cache_does_not_claim_current_database_summary_coverage \
  -- --nocapture --test-threads=4
```

The SQL verifier provisions exclusively owned Docker servers from cached
`postgres:16-alpine` and `postgres:18-alpine` images. It applies the current fresh
migration baseline and checks DEFAULT repair, rollback, expiry, producer
invalidations, and the production boundary-delete SQL. Populated phase-1 conversion
and old migration-byte preservation are not pre-production delivery requirements:

```bash
python3 scripts/verify-native-partitions.py --versions 16 18 --brief \
  --output /tmp/opencode/native-partitions-check.json
python3 scripts/probe-postgresql-native-maintenance.py --protocols-only \
  --repeat 2 --summary --output /tmp/opencode/native-protocol-check.json
```

Use unused output paths. The verifier's optional `--rust-tests` runs its declared
migration, partition, fixture-lifecycle, and retention subset. It does not replace
the summary, analytics, API, or supervisor suites. The protocol runner records
rejected protocols as evidence. Its `passed` field is not a workload-performance
gate. See [SQL protocol evidence](../research/postgresql-native-maintenance-protocols.md).
For both real migration histories and fresh-install checksum, role, and rerun
checks, see [Native-install verification](../deployment/postgresql-native-install-verification.md).

### Counts and races to verify

- Whole-day repair keeps all rows parent-visible before and after commit.
  Oversized days stay in DEFAULT. Lock timeouts, statement deadlines, cancellation,
  and failed transactions leave no committed detached destination or query gap.
- Builders acknowledge only captured snapshot-visible invalidation IDs. Cover
  lower-ID late commits, pre-reserved IDs, capped acknowledgment, empty bootstrap
  hours, removed groups, and a waiting builder's serialization retry. Producers
  must commit while another builder holds its kind-state lock. An FK to that
  state is a rejected protocol.
- Transaction-owned marker reuse must check both full origin and actual tuple
  `xmin`. Test restored origin collisions, repeated writes, and savepoint rollback
  and release. Extra child-created markers are safe; lost invalidations are not.
- Derived refresh commits alone use local asynchronous commit. Kill and restart
  an owned server to verify coverage, summary replacement, and acknowledgments
  survive or disappear together. Raw rows and their notifications remain durable.
  Verify the connection's commit setting after success, error, and cancellation.
- Readers take source-parent locks before their snapshots. Compare all four
  summary kinds with raw oracles for clean coverage, dirty hours, ledger holes,
  current hours, partial ranges, null dimensions, and unavailable maintenance
  relations or privileges. Bad SQL must still fail rather than silently fall back.
- Verify old refs absent from the current catalog, nullable/`unknown` display
  dimensions, explicit ref scopes, pack-only catalog expansion, and unauthorized
  refs. Apply the same predicates in raw and summary modes. Identity-filtered
  small-cohort suppression must stay distinct from operator-global totals.
- Verify terminal retry attempts, creation counts, status transitions, and live
  execution volumes against their own metric definitions. Stale cached results
  must report `cache_rawfallback` with null coverage and watermark.
- Whole-leaf expiry removes summaries, coverage, and pending invalidations
  atomically. Boundary/DEFAULT row deletion invalidates only affected hours.
  Colliding leaf `ctid` values must not delete extra rows. Audit row totals count
  confirmed commits; partition drops use separate counters. Check lower-bound
  flags, exact API metadata counts, and partial progress after failure.
- Independent due times survive restart and leader replacement. A five-minute
  summary tick must not make hourly retention run every five minutes. Disabled
  or invalid native settings must preserve row-cleanup and remediation behavior.

### Workload profiles and pending gates

Keep the seven-day profile for an equal comparison with the phase 1 workload.
Also exercise the default retention horizons: 30 days of events and execution
history, and 90 days of audit records. Include ordinary worker/sensor history,
late writes to DEFAULT, missed partition ticks, raw fallback, dirty historical
hours, scoped reads, and the status/count checks above.

Measure the actual repository reader, builder, and retention paths. Preserve
equal rows, filters, durability settings, and concurrency when comparing results.
Capture raw-versus-summary correctness, warm query p95, raw-tail latency,
four-writer throughput and latency, invalidation overhead, summary catch-up,
parent lock acquisition/hold times, and expiry of equal complete cohorts. Record
storage, WAL, and dead tuples/autovacuum too.

Reporting-performance optimization is deferred. The completed historical phase-2
workload passed count checks but failed some latency and writer-overhead targets.
Those runs predate the current fresh-install baseline. The sub-10-ms summarized-read
target, 500-ms raw-query limit, 250-ms lock-acquisition limit, one-second parent
operation target, and at-most-10-percent writer-overhead gate are acceptance
targets, not passed measurements. Correctness tests and small SQL probes do not
establish those workload results. Existing timing samples earlier in this page
remain historical baselines.

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
