# Test Concurrency Reliability Plan

**Status:** Implemented; local validation complete, fresh-runner CI pending  
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
