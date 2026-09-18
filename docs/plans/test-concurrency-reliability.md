# Test Concurrency Reliability Plan

**Status:** Template-clone optimization implemented; full workspace validation in progress and broad integration concurrency remains serial
**Created:** 2026-08-09  
**Scope:** Rust tests run by the CI `rust-test` job

## Objective

Make database and asynchronous tests deterministic under slower CI scheduling. Tests must synchronize on observable state transitions instead of elapsed time, retain every spawned task that can affect assertions or teardown, and release database resources before the next test starts.

## Current Baseline

The following reliability work is already complete:

- PostgreSQL CI shared memory matches local Compose at 256 MiB.
- Temporary test schemas do not register TimescaleDB background jobs.
- Database-backed tests use the shared `TestDatabase` migration fixture where identified.
- API authorization caching is disabled before schema-isolated test setup.
- `test_cross_action_independence` releases only executions whose waiters observed admission.
- `test_queue_stats_persistence` polls for bounded convergence, releases admitted executions, and joins waiter tasks.

The remaining work is organized by failure risk rather than crate ownership.

## Test Synchronization Standard

All new and modified asynchronous tests must follow these rules:

1. Do not use `sleep` to prove that an operation started, completed, or became visible.
2. Use a channel, barrier, task result, database predicate, or notification as the synchronization point.
3. Bound polling with `tokio::time::Instant` and include the last observed state in timeout failures.
4. Retain and await every `JoinHandle` that can mutate state used by the test or teardown.
5. Stop and join background writers before dropping database objects.
6. Do not compare separately read snapshots as though they were atomic. Poll for convergence or read them in one transaction/query.
7. Restore process-global environment variables and caches after tests that modify them.
8. Propagate cleanup errors instead of discarding them with `.ok()`.

## Phase 1: FIFO And Queue Tests

**Priority:** Critical  
**Primary files:**

- `crates/executor/tests/fifo_ordering_integration_test.rs`
- `crates/executor/src/queue_manager.rs`
- `crates/executor/src/completion_listener.rs`

### Work

- Replace the fixed 200 ms admission delay in `test_queue_full_rejection` with a queue-membership barrier or bounded database predicate.
- Retain all ten queue waiter handles in `test_queue_full_rejection`.
- Explicitly cancel or release queued executions and await every waiter before deleting fixtures.
- Replace the initial 200 ms delay in `test_multiple_workers_simulation` with one admission signal per initial active execution.
- Replace the 300 ms queue-state delay in `test_cross_action_independence` with a bounded predicate confirming all three admission states exist.
- Audit queue-manager and completion-listener unit tests for exact FIFO assertions that currently depend on spawn order plus a sleep.
- Add a reusable test-only admission synchronization helper if three or more tests need the same channel/polling pattern.
- Make `cleanup_test_data` return `Result` and fail tests on cleanup errors.

### Acceptance Criteria

- The CI-enabled FIFO suite passes at least ten consecutive runs.
- No active FIFO test detaches a database waiter.
- No active FIFO test uses a fixed sleep as its only admission or queue-membership barrier.
- Every execution is released or cancelled before fixture cleanup.

## Phase 2: Database Fixture Lifecycle

**Priority:** High  
**Primary files:**

- `crates/common/src/test_database.rs`
- `crates/common/tests/helpers.rs`
- `crates/api/tests/helpers.rs`
- Executor, sensor, worker, and supervisor database test helpers

### Work

- Preserve ownership of `TestDatabase` instead of returning only a cloned `PgPool`.
- Introduce an explicit async fixture teardown API that closes schema pools and calls `TestDatabase::cleanup()`.
- Ensure background tasks are stopped before schema removal.
- Migrate test modules incrementally to an owning fixture type.
- Add a CI safety-net cleanup step with `if: always()` for schemas left by panics or cancelled jobs.
- Record test schema count and temporary Timescale job count before and after the CI suite.

### Acceptance Criteria

- A successful test run leaves zero `test_*` schemas and zero Timescale jobs targeting `test_*` schemas.
- Failed fixture construction removes any schema it created.
- Teardown failures fail the owning test or CI cleanup step.
- The full CI suite no longer exhibits monotonically increasing catalog or checkpoint pressure.

## Phase 3: API Background Work And Audit Assertions

**Priority:** High  
**Primary files:**

- `crates/api/tests/helpers.rs`
- `crates/api/tests/cache_api_tests.rs`
- `crates/api/src/authz.rs`
- `crates/common/src/audit/writer.rs`

### Work

- Store the complete audit writer handle in `TestContext`.
- Replace detached cleanup in `Drop` with explicit awaited teardown.
- Add an audit-writer flush or barrier operation for tests.
- Replace 500 ms audit polling windows with the barrier, or use a multi-second bounded deadline until the barrier exists.
- Track detached authorization-denial audit writes so teardown can await them in tests.

### Acceptance Criteria

- API tests do not spawn schema cleanup from `Drop`.
- Audit assertions do not rely on writer scheduling within a fixed short interval.
- No audit write runs after its schema starts teardown.
- API integration binaries leave no schemas after normal completion.

## Phase 4: Global Cache And Environment Isolation

**Priority:** High  
**Primary files:**

- `crates/executor/src/scheduler.rs`
- `crates/executor/src/event_processor.rs`
- `crates/executor/tests/worker_placement_scheduling_e2e.rs`
- `crates/worker/src/registration.rs`
- `crates/sensor/src/sensor_worker_registration.rs`

### Work

- Make executor action and rule caches service-instance-local where practical.
- Otherwise include database/schema identity in cache keys.
- Add explicit cache reset hooks for tests until cache ownership is refactored.
- Preserve and restore worker test environment variables with an RAII guard.
- Serialize tests that mutate or read the same process-global environment variables.
- Document process-global cache behavior in test helpers.

### Acceptance Criteria

- Repeated primary IDs across isolated schemas cannot reuse another schema's cached model.
- Worker and sensor environment tests preserve caller-provided values after success or panic.
- Worker-placement tests pass with randomized execution order and multiple test threads.

## Phase 5: Service Task Shutdown

**Priority:** Medium  
**Primary files:**

- `crates/worker/src/heartbeat.rs`
- `crates/worker/src/service.rs`
- `crates/api/tests/sse_execution_stream_tests.rs`
- `crates/worker/src/runtime/process_executor.rs`

### Work

- Store the worker heartbeat task handle and make shutdown await task completion.
- Replace the worker task-reaping test's 10 ms scheduling assumption with explicit task signals.
- Retain and join the active PostgreSQL notification test's update task.
- For currently skipped network SSE tests, use an in-process server, subscription-ready signal, and joined update/server tasks before re-enabling them.
- Join process-cancellation helper tasks so failures are observable.

### Acceptance Criteria

- Service tests do not sleep to guess whether a background task has stopped.
- Every background task started by a test is joined, cancelled and joined, or owned by a documented long-lived fixture.
- Re-enabled SSE tests use ephemeral ports and no external server dependency.

## Phase 6: Timestamp And Ordering Tests

**Priority:** Low  
**Primary files:** repository integration tests under `crates/common/tests/`

### Work

- Inventory tests that sleep solely to force timestamp changes.
- Prefer explicit timestamps, database clock reads, or deterministic secondary ordering keys.
- Ensure ordering queries use stable tie-breakers such as `id` when timestamps can match.
- Keep real-time waits only where elapsed-time behavior is the contract being tested.

### Acceptance Criteria

- Repository ordering tests remain deterministic when timestamps have equal precision.
- No test depends on wall-clock adjustment or scheduler timing to establish row order.

## CI Validation Strategy

### Per-Phase Validation

Run the directly affected binary repeatedly before the full suite. For FIFO changes:

```bash
cargo test -p attune-executor --test fifo_ordering_integration_test -- \
  --include-ignored --test-threads=1 \
  --skip test_high_concurrency_stress \
  --skip test_extreme_stress_10k_executions
```

For shared infrastructure changes:

```bash
cargo check --all-targets --workspace
cargo test --workspace --all-features -- --include-ignored --test-threads=1 \
  --skip test_service_creation \
  --skip test_sse_stream_receives_execution_updates \
  --skip test_sse_stream_filters_by_execution_id \
  --skip test_sse_stream_requires_authentication \
  --skip test_sse_stream_all_executions \
  --skip dashboard_timezone_bucketing_handles_dst_and_non_hour_offsets \
  --skip test_action_execute_with_profile \
  --skip test_high_concurrency_stress \
  --skip test_extreme_stress_10k_executions
```

### Reliability Gate

- Repeat changed race-sensitive test binaries at least ten times.
- Run one CI-equivalent suite against a fresh PostgreSQL container.
- Verify zero temporary Timescale jobs after the run.
- After lifecycle cleanup is implemented, verify zero temporary schemas.
- Capture PostgreSQL crashes, deadlocks, pool timeouts, and cleanup failures as blocking failures.

## 2026-09 isolation and speed validation

Validated on Rancher Desktop 29.5.3 with 4 vCPUs and 16 GiB RAM:

- Docker E2E projects have unique containers, volumes, networks, database volumes, and RabbitMQ vhosts; fixed ports/names/subnets are reset.
- Two PostgreSQL projects started concurrently and independent teardown preserved both the neighboring project and a foreign sentinel volume.
- Injected API startup failure returned non-zero, removed all owned resources, and preserved the dirty development neighbor.
- `--no-startup` requires an explicit project, starts no dependencies (`compose run --no-deps`), and never claims teardown.
- A full owned E2E smoke passed (`1 passed, 30 deselected`) and removed its stack.
- Database lifecycle and API equal-ID cache/partial-construction regressions passed repeatedly with zero owned schema leftovers.
- Worker cancellation kills TERM-ignoring descendants; inline equal-ID files and offline wheel installation regressions pass.
- The Rust Docker build deliberately failed on an injected Rust compile error. The normal image contains 937 ignored tests in 63 stripped executables; inventory SHA-256: `53ed142bf61c1dec293ee2805bb4496c785a753b67d33e0eaf07aef84a540b9f`.

Representative lifecycle samples used identical three-test coverage:

| Sample set | Median | Range |
|---|---:|---:|
| Serial cold baseline (3) | 111.391 s | 110.084–129.498 s |
| Serial warm baseline (5) | 21.329 s | 21.047–23.718 s |
| Final cold, 2 threads (3) | 108.539 s | 98.835–118.239 s |
| Final warm, 2 threads (5) | 19.795 s | 19.575–22.724 s |

The baseline-derived target was at least 5% warm improvement with no cold regression and identical assertions. Final warm improvement was 7.2%; cold median improved 2.6%.

Ten varied-order lifecycle samples at each concurrency produced medians of 21.913 s (1), 20.103 s (2), and 20.392 s (4), with zero owned schema leaks. Two threads is certified only for this narrow lane. API cache/partial-construction samples produced 21.709/21.059/20.937 s; the small gain does not justify a broad setting change. General database/service integration remains serial. Pure unit/Vitest defaults remain unchanged.

Raw local sample JSON was intentionally kept outside the repository (`/tmp/attune-test-baseline.json`, `/tmp/attune-test-final.json`, `/tmp/attune-parallel-pilot.json`, `/tmp/attune-api-parallel-pilot.json`); the durable medians, ranges, commands, inventory fingerprint, and decisions are recorded here.

A Linux worker follow-up ran the equal-execution-ID materialization, full execution/cleanup, action-cancellation, and dropped-installer-future regressions ten times each. The 18-test dependency-isolation and eight-test log-truncation binaries also passed ten times at eight test threads; warm binary times were 6.88–7.97 seconds and 2.00–2.01 seconds respectively, separate from compilation. The real Python venv, local-wheel installation, unittest import, and missing-wheel failure tests passed with Cargo offline inside an `unshare --user --map-root-user --net` network namespace. Setup and installer commands use the pack as their working directory, put HOME and cache paths under the owned runtime environment, and disable inherited pip configuration.

The executor service follow-up gave its ignored real-broker test UUID-scoped queues, exchanges, and consumer tags, plus bounded stop, topology deletion, connection close, and database cleanup. Both compiled executor artifacts passed that test in ten retained-stack repetitions. The deterministic FIFO integration binary then passed all eight selected tests in ten repetitions while alternating one, two, and four test threads; each run selected the same inventory and took 19.73–26.79 seconds. FIFO assertions use persisted admission order, cancellation preserves the remaining relative order, and the 1,000/10,000 execution cases remain separate load tests rather than inferring order from spawn indices. Notifier component siblings, WebSocket outgoing tasks, sensor heartbeat, and sensor output readers now have owned handles that are cancelled and joined under a bound. Their focused ownership regressions passed ten repetitions. The disposable broker/FIFO stacks left no labelled containers, volumes, or networks.

### Docker Desktop follow-up

Validation resumed on Docker Desktop 4.91.0 with 8 CPUs and 16 GiB allocated. The Rust image contains 63 executables and 938 ignored tests; its inventory SHA-256 is `07ab142e3881ee4a6019207ab517d85c89c1ccded07e95e82f297916191e9d99`.

- The common lane passed in 685 seconds serially and 424 seconds at four threads with identical selection, a 38% reduction.
- The included migration-fidelity binary took 168 seconds serially and 102 seconds at four threads.
- Ten warm lifecycle repetitions passed all 30 executions in 1.79–2.39 seconds per three-test run, with zero clone databases and zero owned sessions before janitor recovery.
- A cold Docker Desktop checkpoint exceeded the former 10-second database DDL bound. The bound is now 30 seconds, and fixture cleanup first stops the clone's TimescaleDB workers. That reduced a warm two-test sample from 14.43 seconds to 1.54 seconds.
- The multi-stage Rust test image is 2.44 GB instead of 5.07 GB. Test artifacts are 1.6 GB instead of 2.1 GB, and a harness-only rebuild takes 26 seconds without recompiling Rust.
- An injected Rust compile error failed the image build. A nonexistent test filter also fails instead of returning a zero-test success.
- A neighboring Compose project and foreign sentinel volume survived owned teardown. A fresh runner-owned project left no labelled containers, volumes, or networks after its exit trap.

The four-thread common result is a successful pilot, not yet the global default. The repeated full-run baseline used commit `981f75d3`, Docker Desktop 4.91.0 with Docker Engine 29.8.0, eight x86-64 CPUs, 16 GiB RAM, and the precompiled Rust 1.98.1 artifacts. Every sample selected the same 625 common-crate tests from the 938-test inventory above, with 15 named external/stress exclusions and no retries. Cold samples used fresh owned Compose stacks while retaining the Docker build cache. Warm samples reused one healthy stack and migration template.

```bash
bash scripts/benchmark-rust-integration-tests.sh \
  /tmp/attune-common-benchmark-20260917.tsv
```

| Sample set | Samples | Build median (range) | Startup median (range) | Test median (range) | Cleanup median (range) | Total median (range) |
|---|---:|---:|---:|---:|---:|---:|
| Cold stack, 4 threads | 3 | 3.699 s (3.630–3.769) | 6.146 s (5.897–6.500) | 473.770 s (421.136–511.738) | 2.161 s (2.130–2.274) | 487.427 s (434.644–524.603) |
| Warm stack, 4 threads | 5 | 0 | 0 | 435.385 s (430.511–438.809) | 0 | 436.721 s (431.328–440.155) |

Peak run-owned PostgreSQL sessions ranged from 11 to 15. Before teardown, every sample had zero run-owned clones, sessions, and schemas, plus the one expected run-level migration template. All eight samples passed. The warm median is 3.0% above the earlier 424-second pilot, which is ordinary run variation rather than a selection change.

Future broad scheduler changes must preserve this exact selection or publish an explicit coverage map. The measured target is a warm total median at or below 393 seconds, a 10% improvement, with no more than a 5% cold-total regression, no retries, at most 16 peak run-owned PostgreSQL sessions, and the same zero-leak result. The remaining prerequisite tickets and repeated whole-workspace gate are still required before changing the global default.

#### Resource-budget pilot result

The final issue #85 pilot reran the common lane on the same Docker Desktop host after the isolation prerequisites landed. The runner now fingerprints the selected test identities after crate, filter, and skip selection. This avoids treating an unrelated workspace test addition as a common-lane coverage change. All budgets selected the same 625 tests with fingerprint `fb6cb3ce8b72430eb2fcddd0c67aef7a76a8a7e747a705a44cc371b10b1b223f` from the 938-test artifact inventory.

| Threads | Cold samples | Cold total median (range) | Warm samples | Warm total median (range) | Peak sessions |
|---:|---:|---:|---:|---:|---:|
| 1 | 1 | 630.487 s | 3 | 599.152 s (582.619–636.025) | 3–10 |
| 2 | 3 | 543.775 s (515.664–553.710) | 5 | 531.906 s (517.137–576.157) | 5–13 |
| 4 | 3 | 446.831 s (444.991–487.167) | 5 | 422.405 s (405.900–453.134) | 11–15 |

Every recorded sample passed without retries. Before teardown, each sample had zero run-owned clones, sessions, and schemas, plus the expected run-level migration template. Teardown left no labelled containers, volumes, or networks.

Four threads improved the current warm median by 29.5% against one thread and met the cold and connection limits. It missed the predeclared 393-second warm target by 29.405 seconds, so the common lane remains serial by default. Two threads were both slower and less useful. The narrow rollback-isolated `action_repository_tests` optimization remains valid, but this pilot does not certify broader scheduling.

One retained-stack setup exposed PostgreSQL error `57P03`: `pg_isready` had accepted the image's temporary initialization server immediately before that server restarted. The runner now requires PID 1 to be the final `postgres` process before it accepts readiness. The next retained-stack setup and all five warm samples passed.

Issue #86 profiled the four-thread lane by executable. `migration_tests` was the largest target at a 104.09-second median and scaled only 1.16 times from one to four threads. The next four targets were `execution_repository_tests` at 24.14 seconds, `inquiry_repository_tests` at 21.73 seconds, `repository_worker_tests` at 21.59 seconds, and `cache_repository_tests` at 20.75 seconds.

A bounded scheduler prototype ran `migration_tests` with one worker beside the remaining binaries with three workers. It kept the selected count and fingerprint unchanged, reported 11 peak sessions with fresh migration sessions included, and left zero run-owned resources. It failed the performance gate: the warm total was 456.911 seconds and `migration_tests` grew to 272.83 seconds under concurrent database DDL. The prototype was removed. The serial four-thread runner remains the fastest measured shape.

The prototype exposed an accounting gap worth keeping. Fresh migration databases now use `attune_migration_<run-token>_<nonce>` names. Peak-session, pre-teardown leak, and explicit janitor checks include that run-owned prefix. Benchmark teardown and janitor queries now propagate failures instead of reporting successful cleanup after a failed command.

Issue #106 then reduced the migration critical path without changing its 41 selected identities. The executable now owns one migrated database. Nineteen read-only tests open runtime-local pools against it, and eleven mutation tests use runtime-local one-connection pools with an open rollback transaction. Five DDL, committed-state, or cross-connection tests retain physical template clones. Six migration-history and upgrade tests still create fresh databases. Direct Cargo runs retain per-test physical isolation.

Focused warm samples completed in 24.489 and 24.938 seconds at one thread and 25.223, 28.191, and 27.325 seconds at four threads, versus the prior 104.09-second four-thread median. Three full four-thread common-lane treatments completed in 332.557, 354.038, and 335.865 seconds. Their 335.865-second median passes the 393-second target with the same 625-test fingerprint. A 358.932-second fresh-stack treatment improved on the prior 446.831-second cold median. Peak sessions were 11–12 and every treatment left zero clone, migration-database, session, and schema leaks before teardown. A forced mid-run termination also left zero databases and sessions after the outer runner cleaned the exact run-owned prefixes.

#### Fixed-wait inventory

Issue #104 found 25 fixed wall-clock waits in Rust integration tests. Timeout guards are not included. The inventory does include SQL `pg_sleep`, embedded fixture-process sleeps, bounded negative-observation windows, and fixed delays passed through helpers.

| Tests or helpers | Waits | Classification | Decision |
|---|---:|---|---|
| `execution_log_stream_lease_repository_tests` expiry and renewal | 1,100 ms; 600 ms; 600 ms | Avoidable state-transition wait | Replaced with explicit database expiry and database-clock deadline assertions. |
| `async_release_pin_tests::concurrent_pin_commit_wins_before_retention_can_delete_release` | 50 ms | Avoidable readiness wait | Retain until the test can observe the collector's PostgreSQL lock. |
| Three `sse_execution_stream_tests` | 500/500 ms; 500/200 ms; 500/200 ms | Avoidable readiness and ordering waits | These externally hosted tests need a subscription-ready signal and an observed processing barrier before removing the negative windows. |
| `cache_repository_tests::cleanup_waits_for_a_reader_pinned_before_expiry` | Up to about 550 ms via `pg_sleep` | Contract time | Retain: the test crosses the configured 500 ms readability period. |
| FIFO `wait_for_queue_state` and `test_queue_stats_persistence` | 10 ms per retry at two sites | Polling backoff | Retain: bounded predicates report their last observed queue state. |
| FIFO load and worker simulations | 10 ms every 100 spawns; 10/30/50 ms worker delay; 10 ms every 500 spawns | Contract/simulation time | Retain: these excluded stress tests pace load or model different worker rates. |
| `worker/tests/log_truncation_test.rs::test_truncation_with_timeout` | 30 s embedded process sleep | Contract/simulation time | Retain: the two-second execution timeout must stop a still-running producer. |
| Two `pack_registry_tests` audit loops | 50 ms per retry, at most 40 retries | Polling backoff | Retain pending conversion to the existing audit flush barrier. |
| `execution_log_stream_admission_api_tests::wait_for_no_leases` | 20 ms per retry, at most 50 retries | Polling backoff | Retain: bounded predicate for asynchronous lease release. |
| `runtime_log_replica_tests` readiness and sampling helpers | 10 ms and 5 ms per retry | Polling backoff | Retain: bounded child-readiness and metric-sampling loops. |
| `runtime_log_replica_tests` latency, lock-holder, and blocked-seal fixtures | 75 ms; 60 s; 150 ms | Contract/simulation time | Retain: injected storage latency, a kill-owned lock holder, and a bounded negative assertion. |

The selected lease slice had no asynchronous work to poll. Each replacement waits for a SQL statement to complete and then asserts the database's observed state, so adding a timeout loop would make the tests less direct. Failure messages include the observed initial deadline, renewed deadline, and database time. The crash-recovery test still proves that a live abandoned lease blocks admission before forcing that same row across the expiry boundary.

Against one retained PostgreSQL stack and migration template, the old four-test binary took 5.66 seconds serially and the new binary took 3.24 seconds, a 2.42-second or 42.8% reduction with identical test selection. The new binary then passed ten times at the common lane's intended four threads, 40 of 40 tests, in 1.81–2.24 seconds. Concurrent completion order varied between runs. A separate reverse-order serial pass ran each identity explicitly and passed in 0.88–0.93 seconds per test. The retained stack had zero run-owned clone databases and zero clone sessions after validation.

#### Worker repository contract scenarios

Issue #101 tested whether coherent repository scenarios can reduce clone overhead without using a shared reset database. `repository_worker_tests` was selected because its 36 tests each owned a physical clone while exercising no listeners, background tasks, lock races, or cross-connection contracts. The implementation keeps every former test body as a named async subcase and runs those subcases in four independently filterable scenarios. Each scenario owns one clone, and each subcase retains a unique `WorkerFixture`.

| Scenario | Former test coverage |
|---|---|
| `worker_crud_and_lookup_scenario` | create full/minimal worker; find by ID/name and both not-found cases; list; delete and delete-not-found; duplicate-name constraint; runtime association |
| `worker_queries_and_value_encoding_scenario` | status and type queries; all worker type/status round trips; JSON and null fields; null-status constraint; list ordering; port range |
| `worker_update_scenario` | full, partial, and empty updates; complete status lifecycle |
| `worker_timestamp_and_heartbeat_scenario` | heartbeat activation and repeated heartbeat; creation timestamps; update timestamp; heartbeat-triggered timestamp |

The committed baseline contained 36 test identities, 36 clones, and 100 assertion sites. The treatment contains four scenario identities, four clones, the same 36 named subcase bodies, and the same 100 assertion sites. The artifact inventory change from 938 to 906 is entirely the 32 removed top-level harness identities; no assertion body was removed. The subcase runner prints the former test name before execution so captured failure output retains the old diagnostic identity.

The tradeoff is explicit: a panic skips the remaining subcases in that scenario, and former individual test-name filters are replaced by the four scenario filters above. Other scenarios still run on independent clones. No repository scripts, CI configuration, or current documentation invoked the former names directly. Use the scenario filter to rerun a failure; captured output identifies the failing subcase.

On one retained Docker Desktop stack and migration template at the common lane's four-thread budget, the committed binary took 22.12 seconds and the treatment took 2.95 seconds. Ten further treatment runs passed all 40 scenario executions in 2.40–3.15 seconds, with a 2.615-second median. That is an 88.2% reduction against the controlled baseline and removes 32 clone lifecycles.

A temporary injected assertion failure reported `test_create_worker_minimal` in captured output. The runner then executed `worker_update_scenario` successfully in 0.95 seconds on a fresh scenario clone. Both the normal benchmark prefix and the failure-injection prefix had zero clone databases and zero clone sessions afterward. The injected assertion was removed before final checks. This result justifies the scenario pattern for similarly small, fixture-scoped repository contracts, but broader conversion remains separate work because cache and cross-connection tests do not share these safety properties.

Three representative repository binaries were then run with isolated physical clones at four and eight threads. The serial references came from the same Docker Desktop follow-up. Every parallel run passed without retries or selection changes.

| Binary | Tests | 1 thread | 4 threads | 8 threads |
|---|---:|---:|---:|---:|
| `action_repository_tests` | 20 | 36.95 s | 9.71 s | 7.16 s |
| `execution_repository_tests` | 27 | 41.4 s | 23.95 s | 19.05 s |
| `cache_repository_tests` | 34 | 31.6 s | 22.43 s | 16.84 s |

An opt-in prototype also tested one physical clone per executable with catalog-discovered truncation, identity reset, migration-seed restoration, and exact table comparison before every test. It was rejected. `action_repository_tests` took 285.10 seconds, versus 36.95 seconds with serial per-test clones, and the required binary-wide lock prevented useful four-thread or eight-thread execution. Sharing a `PgPool` across separate `#[tokio::test]` runtimes also exhausted the pool, so any future shared-database design must share only database identity and create a pool per test runtime. The prototype code was removed. Per-test physical clones remain the preferred isolation boundary.

A narrower rollback-isolation design succeeded for `action_repository_tests`. The runner owns one migrated clone for the executable, while each compatible `#[tokio::test]` runtime opens its own pool and transaction. Eighteen tests use transaction-bound repository and fixture calls, so rollback isolates concurrent tests without a catalog reset or binary-wide lock. The two timestamp-update tests retain physical per-test clones because PostgreSQL's transaction-stable `NOW()` cannot prove their trigger behavior inside one outer transaction. All 20 test identities remain independently filterable and reportable. The hybrid binary passed in 4.24 seconds serially, 3.25 seconds at four threads, and 2.68 seconds at eight threads, compared with 36.95 seconds serially and 7.16 seconds at eight threads for per-test clones. Retained-stack checks after both a passing child and an intentionally rejected libtest invocation found zero run-owned clones and zero sessions. This remains an explicit per-binary optimization; tests requiring committed cross-connection visibility, listeners, internal transactions, or service/background work keep physical per-test clones.

### Template-clone follow-up

The narrow migration-per-schema improvement above did not generalize: a serial all-crate Docker run exceeded 3.5 hours after reaching only 46 of 63 executables. Repository tests were spending 6–17 seconds apiece replaying all 54 migrations, so thread tuning could not meet the whole-suite objective.

`TestDatabase` now applies canonical migrations once to a run-owned, migration-hashed template database and gives each test a unique physical clone. This retains real PostgreSQL/TimescaleDB behavior and strengthens the isolation boundary. A separate migration test lane continues to exercise fresh migration and upgrade behavior.

Measured on the same 4-vCPU Rancher host:

| Probe | Migration per test | Template clone |
|---|---:|---:|
| Two warm lifecycle tests | 16.12 s | 0.72 s |
| Per-test setup | 6–17 s observed | 172–261 ms clone (178 ms median) |
| Common crate, 626 selected tests, serial | projected hours at late-run rates | 341 s |
| Common crate, same selection, 4 threads | not adopted | 208 s |

The serial and four-thread common runs left zero owned clones. Both reported the same S3 test prerequisite failure because the Docker Rust lane does not yet provision the independently owned MinIO harness; the optimization did not skip or hide it. General runner concurrency remains one until repeated whole-workspace gates pass.

CI safety now records exact run-owned clone and template counts in addition to legacy schemas/jobs. Per-test clone leaks fail even after successful janitor recovery; the one run template is an expected run-level artifact and is removed last.

## Completion Definition

This plan is complete when:

- all critical and high-priority phases meet their acceptance criteria;
- the CI-equivalent suite passes repeatedly on a fresh runner;
- no CI-enabled test uses an unexplained fixed sleep to synchronize database or background-task state;
- no CI-enabled test leaves a state-mutating task detached;
- test database resource counts return to baseline after the suite;
- workspace checks and formatting pass without warnings.

## Validation Results

Validated locally on 2026-08-10:

- The active FIFO integration suite passed ten consecutive runs.
- The API schema-teardown regression test and all five agent endpoint integration tests completed without hanging.
- `cargo fmt --all -- --check` passed.
- `cargo check --all-targets --workspace` passed without warnings.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings` passed.
- The CI-equivalent workspace test command in this plan passed end-to-end.
- Temporary database resources returned to the pre-run local baseline: 1,600 existing `test_*` schemas and zero Timescale jobs targeting test schemas.
- `git diff --check` and shell syntax validation for `scripts/ci-test-db-safety.sh` passed.

The remaining external gate is the same suite on CI's fresh PostgreSQL service, where the expected pre-run and post-cleanup counts are both zero.
