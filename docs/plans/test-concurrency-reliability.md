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

### Docker Desktop follow-up

Validation resumed on Docker Desktop 4.91.0 with 8 CPUs and 16 GiB allocated. The Rust image contains 63 executables and 938 ignored tests; its inventory SHA-256 is `07ab142e3881ee4a6019207ab517d85c89c1ccded07e95e82f297916191e9d99`.

- The common lane passed in 685 seconds serially and 424 seconds at four threads with identical selection, a 38% reduction.
- The included migration-fidelity binary took 168 seconds serially and 102 seconds at four threads.
- Ten warm lifecycle repetitions passed all 30 executions in 1.79–2.39 seconds per three-test run, with zero clone databases and zero owned sessions before janitor recovery.
- A cold Docker Desktop checkpoint exceeded the former 10-second database DDL bound. The bound is now 30 seconds, and fixture cleanup first stops the clone's TimescaleDB workers. That reduced a warm two-test sample from 14.43 seconds to 1.54 seconds.
- The multi-stage Rust test image is 2.44 GB instead of 5.07 GB. Test artifacts are 1.6 GB instead of 2.1 GB, and a harness-only rebuild takes 26 seconds without recompiling Rust.
- An injected Rust compile error failed the image build. A nonexistent test filter also fails instead of returning a zero-test success.
- A neighboring Compose project and foreign sentinel volume survived owned teardown. A fresh runner-owned project left no labelled containers, volumes, or networks after its exit trap.

The four-thread common result is a successful pilot, not yet the global default. Three cold and five warm full-run samples, the remaining prerequisite tickets, and a whole-workspace gate are still required.

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
