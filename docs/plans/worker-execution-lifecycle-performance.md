# Worker execution lifecycle performance

Status: work in progress in `perf/worker-execution-lifecycle`.

## Objective and baseline

Reduce worker receipt-to-terminal-publication latency for warm executions,
especially millisecond actions such as `core.echo`, by removing redundant
database work and unnecessary log allocation. Cover normal success and ordinary
nonzero-exit failure, while preserving cancellation, timeout, authorization,
secret protection, storage settlement, and queue/workflow completion contracts.

Initial inspection used commit `880b1b2a138a335eb549c08452f0d3440de68d41`.
The inspected worker lifecycle paths match tag `v0.10.2`;
matched end-to-end observations are recorded below.

## Invariants

- Persisted execution state determines terminal notifications.
- No new artifact, artifact-version, log-stream, or stream delivery-permit rows
  are created for stdout/stderr streams that receive zero bytes. Existing shared
  artifact rows from earlier executions need not be deleted.
- Stdout and stderr allocate independently. Whitespace is output; empty writes
  are not. Allocation occurs before encrypting/persisting the first bytes.
- An allocation or write failure is distinguishable from intentional silence.
- Preserve worker assignment, attempt fencing, release identity, owner binding,
  revocation, dependency validation, approval expiry, and byte/sequence bounds.
- Reuse immutable metadata across an execution, but reacquire mutable authority
  fences per transaction. Preserve final freshness checks after lock/file waits.
- Preserve canonical lock order and repository-owned database access.
- Required log settlement precedes terminal completion. Background settlement
  requires a separate durability design and is outside this plan.

## Implementation sequence

### 1. Baseline and lifecycle observability

- [x] Provision an owned, isolated v0.10.2 instance using repository lifecycle
  tooling; leave existing development resources alone.
- [x] Measure receipt, preparation, process start/exit, log finalization,
  terminal persistence, and completion publication separately.
- [ ] Capture statement counts, SQL time, pool acquisition, lock waits, and
  internal HTTP calls, correlated by execution without logging secret data.
- [x] Exercise warm and cold release states, serial and concurrent executions,
  shared-file and segmented storage. Keep workloads identical for reruns.
- [x] Include echo, silent success/failure, stderr-only failure, and both streams.
  Record sample sizes and latency distributions; process duration currently
  includes log finalization and excludes preparation/terminal reporting.

Instrumentation and benchmarks can proceed alongside the first small correctness
change; capture an unchanged baseline before claiming performance improvements.

### 2. Persisted state and notification reuse

- [x] Return the execution from the terminal `UPDATE ... RETURNING` through the
  execution outcome instead of discarding it.
- [x] Construct status-change and completion messages from that persisted row,
  removing publication-time execution rereads and raw-result cloning.
- [x] Correct ordinary failure, timeout, and cancellation reporting, including
  accurate previous-state information and early preparation errors.
- [x] Review startup reads: use a guarded Running transition with returned data
  where safe, preserving cancellation, assignment, and MQ release validation.
- [x] Keep public executor callers and notification consumers coherent. Cover
  existing failure paths where persistence itself fails; never invent a durable
  successful terminal transition.

### 3. Execution-scoped immutable preparation context

- [x] Load and validate the immutable schema binding once for input validation,
  log sensitivity, and output protection.
- [x] Carry pinned executable metadata, log settings, and authorized parameter
  secret paths through preparation and completion.
- [x] Remove redundant secret-owner/execution reads only where the existing
  authorization contract can be preserved.
- [x] Continue closing runtime reads and collecting mutable secret origins at
  completion. Do not bypass redaction based only on an empty schema or absent
  API permission sets.

### 4. Existing-admission validation and freshness

- [x] Separate creation/idempotent admission recording from validation of an
  existing retained admission. `ExecutionAdmissionRepository::require` currently
  replays `record_admission`, including executable resolution and conflict-ignored
  admission/attempt/fence inserts.
- [x] Pass validated source/context/permit records between helpers within a
  transaction rather than rediscovering them.
- [x] Consolidate each delivery freshness check to one PostgreSQL wall-clock
  sample checking both approval and delivery bounds.
- [x] Retain source and dependency locks and final checks after target/file
  waits; do not substitute process-clock or cross-transaction cached authority.

### 5. Independent lazy stream allocation

- [x] Replace eager stdout/stderr allocation with uninitialized writer state
  holding execution-level context and stream-specific settings.
- [x] On the first nonempty write, allocate the artifact/version/stream/permit
  transactionally and return identifiers, path, and permit directly to the writer.
- [x] Initialize encryption after the version ID exists and before encoding
  the first frame. Preserve bounded buffers, truncation notices, and manifests.
- [x] Reuse initialized writers for subsequent writes. Empty writes and sealing
  an unused writer cause no stream database allocation, file initialization,
  segment task, or seal request.
- [x] Retain execution-level delivery registration independently of log output.
- [x] Define cancellation during first allocation, failed allocation retries,
  and partial initialization behavior. Latch failures; prevent duplicate active
  writers or orphan publication from being reported as intentional silence.

### 6. Log settlement, cleanup, and reader semantics

- [x] Thread validated records through seal authorization, manifest verification,
  bounds checks, and publication. Keep locked rereads that detect real races.
- [x] Carry allocated log metadata to result construction instead of rediscovering
  stderr paths. Preserve ordinary stderr whitespace/display behavior.
- [x] Exclude log streams in the ordinary file-finalization repository query,
  removing per-version exclusion queries. Batch compatible metadata size updates
  where useful and preserve zero-byte non-log file artifacts.
- [x] Treat terminal absence of a lazily allocated stream as successful empty
  output unless capture metadata indicates failure. Define behavior for crashes
  and supervisor-settled executions where capture completeness is unknown.
- [x] Update protected and ordinary readers, UI behavior, subscription discovery,
  reconnect/offset handling, and completion wakeups. An absent active stream
  remains discoverable after its first write; do not wait a terminal grace period
  for a positively known empty stream.
- [x] Evaluate combined sealing only if it reduces work without violating lock
  order, independent manifests, failure isolation, or storage settlement.

### 7. Warm release verification

- [x] Remove synchronous full-archive read/hash from every shared-volume receipt.
- [x] Verify during materialization and reuse verification only under an explicit
  immutable-release integrity contract. Define invalidation for replacement,
  eviction, and shared-volume mutation; a marker alone is not proof of integrity.
- [x] Move remaining blocking archive work off Tokio executor threads and
  coordinate concurrent materialization. Verify new/unverified content by digest.

### 8. Validation and measured acceptance

- [x] Add focused cases for silent success/failure, stdout only, stderr only,
  both streams, whitespace, empty writes, truncation, first-write allocation
  failure, cancellation during allocation, and sealing failure.
- [ ] Exercise encrypted shared-file and segmented storage, subscription before
  first output, terminal empty output, reconnects, and protected readers.
- [x] Cover expired approvals, revoked/dependent admissions, stale attempts,
  mismatched owners, and concurrent mutations. Keep security failure cases.
- [ ] Verify persisted states agree with messages and queue/workflow completion
  remains correct, including retries and early preparation failures.
- [x] Repeat identical baseline workloads; report latency and statement counts
  by phase. Do not promise a latency target before measuring the baseline.
- [x] Run formatting, affected crate checks/tests, relevant database and storage
  integration suites, Clippy as applicable, and workspace checks. Follow
  `docs/testing/running-tests.md`; disclose unrun coverage and pending CI.

## Reviewable implementation slices

1. Baseline instrumentation and terminal reporting/returned-row reuse.
2. Immutable preparation data reuse and file-finalization query filtering.
3. Existing-admission validation and clock-query consolidation with security tests.
4. Lazy writers plus both reader paths and empty-output semantics.
5. Seal-path query reduction and release verification optimization.
6. Combined validation and before/after measurements.

Changes remain work in progress until applicable checks pass. The user authorized
checkpoint commits on this branch. Pushes, PR creation, merging, releasing, and
deployment to an existing environment remain outside the current authorization.

## Entry points

- `crates/worker/src/service.rs`: receipt and lifecycle notifications.
- `crates/worker/src/executor.rs`: preparation and terminal persistence.
- `crates/worker/src/completion.rs`: delivery and log allocation/transport.
- `crates/worker/src/runtime/log_writer.rs`: bounded/encrypted writer state.
- `crates/worker/src/runtime/process_executor.rs`: capture and finalization.
- `crates/common/src/repositories/execution_runtime_admission.rs` and
  `runtime_authority.rs`: admission validation.
- `crates/common/src/repositories/worker_delivery.rs`: permits and freshness.
- `crates/common/src/repositories/artifact.rs`: file finalization discovery.
- `crates/api/src/routes/internal_files.rs`: sealing and publication.
- `crates/api/src/routes/executions.rs` and protected stream helpers: log readers.
- `crates/common/src/pack_transport/volume.rs`: receipt-time archive verification.

## Implementation checkpoints

The functional worker/common/API performance changes were frozen at
`ca202d5f721051377543e1618bd623d169f228fb` for measurement. The test fixtures were
subsequently committed at `231c6ec0` and `618ddc09`. Terminal state reuse, guarded
starts, immutable preparation, existing-admission validation, lazy independent
capture, positive empty-output evidence, seal reuse, and release verification
are implemented. Admission creation still belongs to the admission owner;
validation does not silently repair missing records.

Combined stdout/stderr sealing was evaluated and left independent. Lazy capture
removes unused seals without coupling failures or adding multi-stream lock order.
The ordinary artifact query excludes log versions; zero-byte ordinary files retain
their existing settlement. A first allocation after a recorded execution enters
Canceling still fails its existing launch fence and records incomplete capture.

The bounded shared-volume cache trusts coherent Unix file identity, size, mtime
and ctime plus immutable extracted releases. It rehashes after mutation,
replacement, removal or eviction, and on platforms without reusable identity.
A marker alone is not proof. Hashing runs outside Tokio; concurrent receipts
share one verification. The distribution documentation records this trust model.

## Matched measurements (2026-11-04)

Baseline: `v0.10.2`, `6906b96846eb83299cfb9b8a740ccf8854b25e8e`.
Only four CLI MCP policy/docs files differ from the task's `880b1b2a` starting
commit; worker, common, API, executor, migrations and workspace dependencies match.
Treatment: `ca202d5f721051377543e1618bd623d169f228fb`.

Both revisions used the same published workload, observer, harness SHA-256
`b2000877ad666efb083e123d7d233ef11630d92b3f7fee92fcd2aec7d63ff102`
and Compose override SHA-256
`aca96f6ffbd4b7b012a48930923cd9f8eb752e3a55d5561227242b7cba74150b`.
Each owned fresh stack ran six workloads at concurrency 1 and 4, 20 measured
executions per profile: 240 per stack, 960 across all four stacks. First calls
and 16 warmups per workload were recorded separately. Measured worker IDs had
to belong to the warmed set. First release receipt does not establish a cold OS
page cache, and one first call is not a latency distribution.

Images were built before measurement, using release musl agents and repository
Compose services. Stacks ran sequentially without concurrent compilation.
Docker supplied eight CPUs and 16,765,710,336 bytes of memory. PostgreSQL 18,
RabbitMQ 4.3.6 and verbose SQL instrumentation were shared experimental inputs.
API-backed segments used the filesystem object backend; S3 latency is not measured.
Each profile required complete capture and sealed, ready versions on both revisions.

Both revisions used a **10,000 ms benchmark-only capture-finalization budget**.
Earlier diagnostic runs at the product's 1,000 ms default are excluded: baseline
capture was incomplete for 30/240 measured executions, and treatment stopped at
its first measured incomplete capture. The default remains unchanged. These
results do not establish capacity within that default window. Action timeouts,
approval bounds, attempt fences and grant expiry were not extended.

Latency below is request to observed MQ completion, including 10 ms polling
and management overhead. SQL calls are database-wide `pg_stat_statements`
deltas divided by 20; `track=all` includes nested SQL, orchestration and background
work. Summed SQL execution time is not wall-clock time. Values are rounded.

| Storage | Workload | Concurrency | p50 ms, before → after | p95 ms, before → after | SQL calls/execution, before → after |
| --- | --- | ---: | ---: | ---: | ---: |
| Shared file | echo | 1 | 689 → 507 | 811 → 720 | 1291 → 731 |
| Shared file | echo | 4 | 1972 → 1246 | 2351 → 1482 | 1276 → 712 |
| Shared file | silent_success | 1 | 614 → 200 | 671 → 337 | 1241 → 495 |
| Shared file | silent_success | 4 | 1604 → 448 | 1962 → 545 | 1217 → 474 |
| Shared file | silent_failure | 1 | 685 → 206 | 762 → 291 | 1299 → 490 |
| Shared file | silent_failure | 4 | 2122 → 430 | 2504 → 504 | 1247 → 475 |
| Shared file | stderr_failure | 1 | 780 → 466 | 844 → 529 | 1414 → 737 |
| Shared file | stderr_failure | 4 | 2306 → 1334 | 2668 → 1535 | 1351 → 707 |
| Shared file | both | 1 | 874 → 627 | 1100 → 733 | 1541 → 962 |
| Shared file | both | 4 | 2496 → 2089 | 2903 → 2427 | 1425 → 961 |
| Shared file | json_stdout | 1 | 827 → 514 | 932 → 646 | 1488 → 779 |
| Shared file | json_stdout | 4 | 2327 → 1453 | 2971 → 1763 | 1438 → 742 |
| API segments | echo | 1 | 520 → 392 | 580 → 455 | 1366 → 824 |
| API segments | echo | 4 | 1036 → 817 | 2249 → 1028 | 1323 → 799 |
| API segments | silent_success | 1 | 406 → 217 | 507 → 250 | 1127 → 508 |
| API segments | silent_success | 4 | 783 → 480 | 867 → 543 | 1074 → 482 |
| API segments | silent_failure | 1 | 399 → 221 | 458 → 350 | 1140 → 494 |
| API segments | silent_failure | 4 | 723 → 529 | 827 → 627 | 1106 → 488 |
| API segments | stderr_failure | 1 | 476 → 350 | 630 → 406 | 1434 → 811 |
| API segments | stderr_failure | 4 | 919 → 782 | 1002 → 948 | 1359 → 794 |
| API segments | both | 1 | 498 → 373 | 644 → 521 | 1658 → 1140 |
| API segments | both | 4 | 1054 → 852 | 1235 → 990 | 1634 → 1088 |
| API segments | json_stdout | 1 | 526 → 422 | 703 → 655 | 1479 → 854 |
| API segments | json_stdout | 4 | 1275 → 877 | 1722 → 1062 | 1381 → 816 |

All 24 p50 and p95 observations decreased. This is a bounded local comparison,
not a statistical confidence claim or production capacity result. At n=20 the
reported nearest-rank p99 is the maximum. One maximum worsened: serial API echo
was 1,685 ms on treatment versus 589 ms on baseline, despite lower p50/p95.
The raw samples and interactive comparison retain that outlier.

Silent success/failure allocated zero log versions/streams on treatment, versus
two per execution on baseline. Stdout-only and stderr-only allocated one each;
both-output allocated two. Parsed JSON stdout retained capture evidence after
its convenience stdout string was cleared. All measured treatment failure
notifications agreed with persisted Failed; baseline published Completed for
these ordinary failures.

Treatment phase spans were present for all 480 measured executions, including
materialization, preparation, process wait, capture settlement, ordinary artifact
settlement, terminal persistence, terminal publication and receipt-to-publication.
The process-wait span includes spawn/stdin delivery/wait; public duration_ms still
includes capture and excludes preparation. Baseline lacks the new phase labels,
so no before/after phase decomposition is claimed.

Serial treatment phase p50 values (ms):

| Storage | Workload | Preparation | Process wait | Capture settlement | Receipt → publication |
| --- | --- | ---: | ---: | ---: | ---: |
| Shared file | echo | 43.36 | 3.02 | 275.82 | 357.72 |
| Shared file | silent_success | 32.14 | 3.68 | 0.17 | 68.49 |
| Shared file | both | 32.30 | 3.81 | 419.25 | 491.05 |
| API segments | echo | 32.36 | 2.96 | 141.61 | 212.88 |
| API segments | silent_success | 32.23 | 3.98 | 0.11 | 68.06 |
| API segments | both | 32.20 | 3.99 | 141.77 | 213.36 |

Capture settlement remains the dominant worker cost for output-producing actions.
Correlated worker pool acquisitions had medians 0.134 ms (shared file) and
0.123 ms (API); those traces exclude pre-start SQL outside the execution span.
PostgreSQL logs retain waits exceeding 50 ms. API route counters are whole-run
observations, including warmups and ancillary tests: baseline API recorded 768
seal requests and treatment 333; segment direct-upload/PUT counts were 385/385
and 386/386. Sanitized route templates and log rotation prevent exact per-execution
HTTP attribution. These counters are not normalized measured-execution savings.

Raw reports, per-profile statement deltas, worker/API/PostgreSQL traces, source,
agent, harness and image hashes, dependency versions and awaited cleanup logs:

- `.scratch/worker-lifecycle-reports/worker-base-volume-993f085b46-volume/`
- `.scratch/worker-lifecycle-reports/worker-treat-volume-fc085a3e01-volume/`
- `.scratch/worker-lifecycle-reports/worker-base-api-a5a994adba-api/`
- `.scratch/worker-lifecycle-reports/worker-treat-api-a00c4e797d-api/`
- `.scratch/worker-lifecycle-reports/comparison.json` and `comparison.html`

Docker Desktop cannot mount the external worktree. Hash-verified source snapshots
under the original repository's Docker-shared `.scratch` supplied the owned runs;
Docker sharing preferences and existing development resources were not changed.
RabbitMQ's default policy rejected the first transient observer queue. The
accepted fixture uses an expiring durable classic queue with explicit deletion.
An initial fixture check also used a DTO field that is not exported; the corrected
fixture checks the stored JSON contract through PostgreSQL. Setup failures and
the excluded one-second diagnostic runs remain in the raw evidence.

## Validation handoff

Status: work in progress; local streaming follow-up and required CI pending.

Validation applies to `ca202d5f` functional source plus the stated benchmark
fixture commits; the pending reader fix needs its own validation. Subsequent
prose/comment-only changes preserve the corresponding code-test evidence.

| Check | Result / scope |
| --- | --- |
| `cargo fmt --all -- --check`, `git diff --check` | Passed; final rerun pending reader fix. |
| `SQLX_OFFLINE=true cargo check --all-targets --workspace` | Passed without warnings. |
| Scoped common/worker/API `cargo clippy --all-targets --no-deps -- -D warnings` | Passed. |
| Worker `cargo test --lib` | 196 passed. |
| `recorded_input_restoration_tests` | 7 passed, including rejected admission INSERT replay and safe early failure. |
| `worker_execution_transition_tests` | 2 passed, including guarded starts and terminal fencing. |
| `runtime_authority_repository_tests` | All 269 passed on an idle host in 1191.91 s; includes native retry materialization and revocation/expiry after waits. |
| `recorded_execution_events_tests recorded_file_completion_cases::` | 14 focused seal cases passed on the current executable. |
| `recorded_execution_events_tests worker_live_streams::` | Initial 11 passed; new selection-race regression pending. |
| `execution_log_stream_admission_api_tests` | 8 passed across ordinary/protected empty and unknown readers. |
| Worker integration executables | 8 truncation, 2 runtime-access and 7 secret-handling cases passed. |
| Shared release cache / completion marker / clock fences | Focused cases passed; concurrency, mutation, replacement, eviction and conservative unknown capture are covered. |
| Versioned-storage correctness harness | 1 direct-storage authority plus 9 cross-replica correctness cases passed; four load/prerequisite cases excluded. |
| `npm run generate:api`, `npm run check:api` | Passed, no generated changes; workspace/spec/client remain 0.10.2. No subsequent exported DTO changes. |
| Artifact-page UI selection | 41 passed with `NODE_OPTIONS=--no-experimental-webstorage`, thread pool, one worker. |
| Matched E2E performance selection | Four runs passed all 12 profiles; 960 measured complete-output executions. |
| Subscription-first E2E selection | Both baseline modes and treatment shared-file passed both cases; treatment API had one pass and one failure; follow-up pending. |
| Treatment E2E regression selection | 23 passed / 4 failed on both storage modes; baseline reproduction below. |

All database Cargo tests used owned disposable PostgreSQL, unique run identities
and four test threads. A first authority run during image builds had 266 passes
and three failures: retained-batch reclamation was deferred, empty-wait cleanup
exceeded its consent budget, and a ten-second approval expired before iteration
preparation. The retained-batch case passed alone, then the complete identical
269-case suite passed with no source change on an idle host. No failing cases
were dropped. The initial UI invocation hit this host's Node 26 localStorage
behavior; an intermediate fork-pool attempt timed out before the passing run.
Both musl builds emit the same pre-existing deprecated linker optimization `1`
warning; host checking and warnings-denied Clippy are clean.

Versioned-storage evidence is reused because its passing binary contains the
new seal authorization helper, and its API/common storage paths were unchanged
by the subsequent tracing and unrecorded-worker allocation edits. It exercises
real versioned storage, not the filesystem performance backend. Exported API
contract evidence remains applicable to later private helpers and worker changes.

### Remaining concerns

- The subscription-first API-backed treatment run exposed a human live-log
  selection race: sealing can publish the provider descriptor between pending
  discovery and the retained version check. A lock-controlled regression reproduced HTTP 403 before the fix. Human live
  selection now repeats full authorization once for the same version after a
  Pending → Ready segment-descriptor transition, with immutable identity/path
  checks. The latest workspace check passes; runtime follow-up is pending.
  Completion and retained bytes were correct; streaming acceptance is not yet
  complete.
- Three native workflow retry cases and queue acknowledgement fail because
  recorded invocation authority is missing. The unchanged baseline reproduced
  the same four failures, with the same executor diagnostics, under matching API
  storage/configuration. Baseline also failed parallel workflow failure isolation
  by reporting Completed; treatment passed that case. Existing admission and
  coordinated credential readiness gates were not weakened to make E2E pass.
- Exact per-execution HTTP attribution and baseline phase decomposition are not
  established by these traces. First-call observations do not prove a cold OS
  cache, and the ten-second fixture window is not default-window capacity proof.
- Required CI, full unfiltered Rust/E2E suites, and the separate storage load lane
  remain unrun. Six deferred deployment cases are not passing release evidence.
  No schema migrations changed, so there is no schema/offline metadata update.

Owned stack/volume/network teardown completed for all four measured runs. The
owned PostgreSQL and RustFS correctness harnesses were also removed; the new
reader regression currently uses a separately owned disposable PostgreSQL.
