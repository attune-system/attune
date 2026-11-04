# Worker execution lifecycle performance

Status: work in progress in `perf/worker-execution-lifecycle`.

## Objective and baseline

Reduce worker receipt-to-terminal-publication latency for warm executions,
especially millisecond actions such as `core.echo`, by removing redundant
database work and unnecessary log allocation. Cover normal success and ordinary
nonzero-exit failure, while preserving cancellation, timeout, authorization,
secret protection, storage settlement, and queue/workflow completion contracts.

Initial inspection used commit `880b1b2a138a335eb549c08452f0d3440de68d41`.
The inspected worker lifecycle paths match tag `v0.10.2`. Findings are static;
no end-to-end timings or speedup claims have yet been established.

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

- [ ] Provision an owned, isolated v0.10.2 instance using repository lifecycle
  tooling; leave existing development resources alone.
- [ ] Measure receipt, preparation, process start/exit, log finalization,
  terminal persistence, and completion publication separately.
- [ ] Capture statement counts, SQL time, pool acquisition, lock waits, and
  internal HTTP calls, correlated by execution without logging secret data.
- [ ] Exercise warm and cold release states, serial and concurrent executions,
  shared-file and segmented storage. Keep workloads identical for reruns.
- [ ] Include echo, silent success/failure, stderr-only failure, and both streams.
  Record sample sizes and latency distributions; process duration currently
  includes log finalization and excludes preparation/terminal reporting.

Instrumentation and benchmarks can proceed alongside the first small correctness
change; capture an unchanged baseline before claiming performance improvements.

### 2. Persisted state and notification reuse

- [x] Return the execution from the terminal `UPDATE ... RETURNING` through the
  execution outcome instead of discarding it.
- [x] Construct status-change and completion messages from that persisted row,
  removing publication-time execution rereads and raw-result cloning.
- [ ] Correct ordinary failure, timeout, and cancellation reporting, including
  accurate previous-state information and early preparation errors.
- [ ] Review startup reads: use a guarded Running transition with returned data
  where safe, preserving cancellation, assignment, and MQ release validation.
- [ ] Keep public executor callers and notification consumers coherent. Cover
  existing failure paths where persistence itself fails; never invent a durable
  successful terminal transition.

### 3. Execution-scoped immutable preparation context

- [ ] Load and validate the immutable schema binding once for input validation,
  log sensitivity, and output protection.
- [ ] Carry pinned executable metadata, log settings, and authorized parameter
  secret paths through preparation and completion.
- [ ] Remove redundant secret-owner/execution reads only where the existing
  authorization contract can be preserved.
- [ ] Continue closing runtime reads and collecting mutable secret origins at
  completion. Do not bypass redaction based only on an empty schema or absent
  API permission sets.

### 4. Existing-admission validation and freshness

- [ ] Separate creation/idempotent admission recording from validation of an
  existing retained admission. `ExecutionAdmissionRepository::require` currently
  replays `record_admission`, including executable resolution and conflict-ignored
  admission/attempt/fence inserts.
- [ ] Pass validated source/context/permit records between helpers within a
  transaction rather than rediscovering them.
- [ ] Consolidate each delivery freshness check to one PostgreSQL wall-clock
  sample checking both approval and delivery bounds.
- [ ] Retain source and dependency locks and final checks after target/file
  waits; do not substitute process-clock or cross-transaction cached authority.

### 5. Independent lazy stream allocation

- [ ] Replace eager stdout/stderr allocation with uninitialized writer state
  holding execution-level context and stream-specific settings.
- [ ] On the first nonempty write, allocate the artifact/version/stream/permit
  transactionally and return identifiers, path, and permit directly to the writer.
- [ ] Initialize encryption after the version ID exists and before encoding
  the first frame. Preserve bounded buffers, truncation notices, and manifests.
- [ ] Reuse initialized writers for subsequent writes. Empty writes and sealing
  an unused writer cause no stream database allocation, file initialization,
  segment task, or seal request.
- [ ] Retain execution-level delivery registration independently of log output.
- [ ] Define cancellation during first allocation, failed allocation retries,
  and partial initialization behavior. Latch failures; prevent duplicate active
  writers or orphan publication from being reported as intentional silence.

### 6. Log settlement, cleanup, and reader semantics

- [ ] Thread validated records through seal authorization, manifest verification,
  bounds checks, and publication. Keep locked rereads that detect real races.
- [ ] Carry allocated log metadata to result construction instead of rediscovering
  stderr paths. Preserve ordinary stderr whitespace/display behavior.
- [ ] Exclude log streams in the ordinary file-finalization repository query,
  removing per-version exclusion queries. Batch compatible metadata size updates
  where useful and preserve zero-byte non-log file artifacts.
- [ ] Treat terminal absence of a lazily allocated stream as successful empty
  output unless capture metadata indicates failure. Define behavior for crashes
  and supervisor-settled executions where capture completeness is unknown.
- [ ] Update protected and ordinary readers, UI behavior, subscription discovery,
  reconnect/offset handling, and completion wakeups. An absent active stream
  remains discoverable after its first write; do not wait a terminal grace period
  for a positively known empty stream.
- [ ] Evaluate combined sealing only if it reduces work without violating lock
  order, independent manifests, failure isolation, or storage settlement.

### 7. Warm release verification

- [ ] Remove synchronous full-archive read/hash from every shared-volume receipt.
- [ ] Verify during materialization and reuse verification only under an explicit
  immutable-release integrity contract. Define invalidation for replacement,
  eviction, and shared-volume mutation; a marker alone is not proof of integrity.
- [ ] Move remaining blocking archive work off Tokio executor threads and
  coordinate concurrent materialization. Verify new/unverified content by digest.

### 8. Validation and measured acceptance

- [ ] Add focused cases for silent success/failure, stdout only, stderr only,
  both streams, whitespace, empty writes, truncation, first-write allocation
  failure, cancellation during allocation, and sealing failure.
- [ ] Exercise encrypted shared-file and segmented storage, subscription before
  first output, terminal empty output, reconnects, and protected readers.
- [ ] Cover expired approvals, revoked/dependent admissions, stale attempts,
  mismatched owners, and concurrent mutations. Keep security failure cases.
- [ ] Verify persisted states agree with messages and queue/workflow completion
  remains correct, including retries and early preparation failures.
- [ ] Repeat identical baseline workloads; report latency and statement counts
  by phase. Do not promise a latency target before measuring the baseline.
- [ ] Run formatting, affected crate checks/tests, relevant database and storage
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

## Progress

- Separate worktree: `/mnt/wdc/attune-worktrees/worker-execution-lifecycle`,
  branch `perf/worker-execution-lifecycle`, based on `880b1b2a`.
- Terminal handlers now return the persisted row to the worker service. Both
  terminal messages use that row, removing their two normal-path execution
  reads. Existing public executor methods retain process-result/error behavior.
- Nonzero exits, timeouts, and cancellations report the persisted status.
  Preparation/catastrophic errors that successfully persist Failed carry that
  row too. Other errors perform one recovery read and cannot announce completion
  for a nonterminal row. Pre-start Canceling no longer fabricates Cancelled
  notifications without a durable transition.
- Added pure service tests for terminal status/message agreement, protected
  persisted results, nonterminal rejection, and missing action metadata.
- Accurate previous-status reporting remains unresolved: the existing status
  event convention uses the loaded row's status. A returned terminal row cannot
  reveal its locked predecessor. Startup transition/cancellation races also
  remain to be addressed with repository-owned transition support.
- Lazy stream allocation, admission/sealing changes, immutable context reuse,
  release verification, full-stack validation, and baseline measurements remain
  unimplemented. No measured performance claim is made.

### Initial slice validation

Tested base `880b1b2a` plus the uncommitted worker executor/service changes in
this worktree. These checks cover worker compilation and message construction;
they do not establish database/MQ lifecycle or performance acceptance.

- `cargo fmt --all -- --check`: passed.
- `git diff --check`: passed.
- `SQLX_OFFLINE=true cargo check -p attune-worker --all-targets`: passed without warnings.
- `ATTUNE_TEST_RUN_ID=worker-lifecycle1 SQLX_OFFLINE=true cargo test -p attune-worker --lib service::tests:: -- --test-threads=4`:
  12 passed, 178 filtered out; no database or broker required by this selection.
- `SQLX_OFFLINE=true cargo clippy -p attune-worker --all-targets --no-deps -- -D warnings`: passed.
- Database/MQ lifecycle tests, workspace validation, and required CI remain unrun.

### Lifecycle and lazy-capture checkpoint (work in progress)

- Added repository-owned atomic worker transitions returning the locked previous
  status and updated row in one statement. Startup returns its row directly to
  preparation and publishes Running only after persistence. Terminal state and
  assignment guards reject stale updates; missing-action failures persist Failed.
- Shared immutable schema bindings and authorized input-secret paths across
  preparation, log policy selection, and result protection. Completion still
  closes runtime reads and queries recorded runtime origins.
- Ordinary file settlement excludes log streams in its repository query.
- Delivery freshness checks share one PostgreSQL clock sample. Existing-admission
  validation now avoids admission/attempt inserts while retaining executable,
  owner, policy, approval, dependency and attempt fences. Delivery helpers reuse
  its fence within the transaction.
- Each stream allocates on its first nonempty write. Empty writes and unused
  sealing allocate nothing. Failed/cancelled initialization is latched. Prepared
  allocation returns its permit directly to transport construction. Allocated
  paths are retained for terminal result construction.
- Worker completion records per-stream capture state. Both SSE reader paths
  recognize positively known empty streams, reject nonzero cursors on them, and
  keep missing/failed/crashed capture distinct from silence.
- Added tracing phase timings for receipt/materialization, preparation, process
  wait, capture settlement, ordinary artifacts, persistence and publication.
- Validation so far: two repository transition tests passed twice (including the
  concurrent terminal race); both delivery-fence unit tests passed; 12 worker
  service tests and all 15 log-writer tests passed, including four new lazy-state
  regressions. These scoped results do not certify untested admission/reader
  integration behavior. The API all-targets check and ordinary-file test are
  ongoing. Full lifecycle/storage/security tests and measurements remain pending.
- Warm release verification and seal-path query reuse remain unimplemented.

### Warm release and seal checkpoint (work in progress)

- Warm Unix release verification reuses a bounded, per-transport file-identity
  proof; mutations and replacements force rehashing. Hashing streams in bounded
  buffers off Tokio threads. Non-Unix consumers retain full verification. The
  storage trust/invalidation contract is documented in
  `docs/deployment/pack-candidate-distribution.md`.
- Sealing reuses the already-authorized worker context and permit for encrypted
  proof, shared-file limits and operation bounds. Version/stream rows remain
  locked continuously across storage verification, so the duplicate locked reads
  and second active-upload lookup were removed. Final deadline checks remain.
- Added route-level tests for normal/protected empty streams, failed/unknown
  capture, terminal statuses and offsets. Added a statement-trigger regression
  that rejects any admission/attempt INSERT during existing-binding validation.
- All five volume-transport tests and all five manifest/capture-marker tests
  passed. The five existing recorded-worker tests passed (including stale owner,
  terminal delivery and admission revocation). The new trigger regression and
  API reader/seal tests are pending. The ordinary zero-byte file regression passed.
- API/worker all-targets validation passed for the prior lifecycle checkpoint.
  Subsequent seal changes still need the queued compiler/Clippy/integration runs.
- The existing frontend artifact test suite failed with Node 26's global
  `localStorage` unavailable. Its source/dependencies are unchanged by this task;
  baseline reproduction and an invocation using `--no-experimental-webstorage`
  are in progress. No frontend behavior change is being made for that issue.

### Startup hardening and validation follow-up

- Assigned executions now start only from Scheduled. In-flight duplicate MQ
  deliveries retain the original cancellation token instead of overwriting it.
  Added cancellation-token and repository duplicate-start regressions.
- Missing action/schema-binding failures now persist a fixed, safe diagnostic
  before parameter restoration or code launch. Added repository-backed coverage
  proving Failed status and no allocated logs for those prelaunch errors.
- All eight stream-admission API tests passed, including the new empty/incomplete
  cases on both reader paths. All 41 existing frontend artifact tests passed with
  `NODE_OPTIONS=--no-experimental-webstorage` and `--pool=threads --maxWorkers=1`.
  The default Node 26 invocation failed, and intermediate fork-worker attempts
  timed out; these are retained as environment/validation limitations rather than
  hidden retries. No frontend source changes were required.
- Scoped common/worker/API all-targets Clippy passed with warnings denied before
  the latest startup additions. A new unused test import was removed; the final
  startup and new trigger regressions still need their focused validation.
