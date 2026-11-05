# Repository wide database activity reduction plan

Status: initial slice implemented and measured across 7,200 executions; default shared-file capture, production attribution, and later phases remain outstanding.
Date: 2026-11-04.

## Objective and scope

Reduce the database work required to run Attune, especially for trivial actions,
idle services, log capture, and workflow views. Prioritize removing repeated work
over making each query faster. Preserve durable execution, authorization,
retention, audit, and notification contracts.

This follows the [worker execution lifecycle plan](worker-execution-lifecycle-performance.md).
Begin implementation after reviewing that work's final code and validation;
passing its tests does not establish the savings proposed here. This document
authorizes no implementation, migration, reset, deployment, or publication.

Coordinate shared instrumentation, transaction lifetime, and maintenance changes
with the [graph admission contention plan](graph-admission-contention.md).
Reduce query volume and lock occupancy together; fewer queries alone do not
prove lower contention.

## Evidence and measurement limits

Worker benchmark reports counted database-wide `pg_stat_statements` deltas with
`track=all`, divided by measured executions. That includes nested trigger and
function SQL, foreign-key checks, transaction commands, and background activity.
It is a measure of aggregate statement executions per job, not worker network
round trips or exact execution attribution.

The treatment's serial shared-file silent-success profile recorded 9,902
statement executions across 20 jobs. Individual query families included 780
policy-revision reads, 440 pack reads, 400 runtime-identity binding reads, and 420
identity reads. These are profile-wide counts normalized in earlier discussion;
they identify repeated validation as an investigation priority, not proof that
every read is redundant. Reconcile these reports with the final worker revision
before using them as a new baseline.

Initial source inspection also identified empty workflow-log claims on a 100 ms
retry, polling queue and inquiry monitors, per-item workflow-wait lookups, and
recursive workflow requests in the web client. Their production cost and safe
replacement remain to be measured.

## Preliminary hotspot findings

Read-only source review used `880b1b2a`, with relevant changes compared against
the worker lifecycle worktree at `170b778b`. These findings describe query
patterns; their savings and production ranking require the attribution work
below. Recheck the final worker implementation before making changes.

### Repeated policy and sponsor validation

Source: [runtime authority repository](../../crates/common/src/repositories/runtime_authority.rs),
especially `lock_policy_one`, `lock_sponsor`, and `lock_policy_with_deadline`.

One policy load performs an initial policy read, three sponsor-related reads,
a locked policy reread, and a dependency query during traversal. Shared
dependencies can be loaded and validated again before the traversal's visited
check suppresses further processing. Separate validation entry points also
start their own traversals.

This is the strongest candidate for reducing ordinary execution overhead.
Investigate deduplicating dependency loads and reusing validated records within
the owning transaction. The initial unlocked read and later locked read serve
lock ordering and freshness purposes; do not remove either merely because the
SQL looks repetitive. Track retained locks, local mutations, and deadlines when
deciding whether reuse is safe.

### Workflow log enqueue and delivery overhead

Sources: [workflow logger and dispatcher](../../crates/executor/src/workflow/log.rs),
`WorkflowLogger::log`, `log_with_conn`, `ensure_log_artifact`, and the
[workflow log outbox repository](../../crates/common/src/repositories/workflow_log_outbox.rs).

A standalone log call opens a transaction and queries the effective segment
limit. Each enqueued chunk then locks the workflow, checks for a seal, reads the
next sequence, and inserts an outbox record. Delivery calls
`ensure_log_artifact` for each claimed record, including existing streams, before
assigning a delivery sequence and performing publication/reconciliation work.

Investigate bounded enqueue/delivery batches, stream metadata reuse within a
protected batch, and consolidated repository operations. Preserve durable
outbox ordering, idempotent delivery, and recovery after unknown outcomes.

### Empty workflow log claims

Source: `WorkflowLogDispatcher::start` and `dispatch_once` in the workflow logger.

When there is no work, each dispatcher waits 100 ms and issues another database
claim. This creates recurring idle traffic per executor instance.

Use notification-driven wakeups with adaptive backoff and periodic durable-state
reconciliation. Measure idle single-instance and multiple-instance behavior,
including reconnect and missed-notification recovery.

### Work queue polling and unnecessary search results

Sources: [queue dispatcher](../../crates/executor/src/queue_dispatcher.rs),
`poll_once`, `reconcile_leased_dispatches`, and `dispatch_ready_batches_for_queue`;
[work queue repository](../../crates/common/src/repositories/work_queue.rs).

Each cycle searches enabled queues and reconciles leased dispatches, fetching
referenced executions individually. Per-queue context resolution happens before
establishing whether there is dispatchable work. Capacity checks call a search
method that executes both an exact count and a row fetch even though that caller
only consumes the count.

Start with count-only or purpose-specific repository methods and bounded bulk
reconciliation reads. Then investigate selecting ready queues before resolving
their full context. Preserve current configuration evaluation, capacity
admission, lease recovery, and dangling-execution handling.

### Workflow wait lookups and recursive view refresh

Sources: `list_workflow_task_waits` in the
[execution routes](../../crates/api/src/routes/executions.rs) and
`useChildExecutions` in the [execution hooks](../../web/src/hooks/useExecutions.ts).

Wait handling fetches target executions or queue items individually. Execution
target authorization constructs a fresh visibility cache for each target;
queue-item targets also load their queue. Descendant expansion requests children
for each discovered execution, including leaves, and asks for totals on every
page. The expansion repeats while executions remain active.

Add bounded bulk target reads and reuse one request-scoped visibility cache.
Measure bounded descendant retrieval and avoid unnecessary repeated totals.
Preserve per-target visibility, pagination, missing references, and depth/size
bounds. This is a contained first implementation candidate with directly
testable query growth.

### Duplicate inquiry expiration scans

Sources: [API inquiry timeout monitor](../../crates/api/src/inquiry_timeout.rs),
[executor inquiry handler](../../crates/executor/src/inquiry_handler.rs), and the
executor service's `InquiryHandler::timeout_check_loop` startup.

Both services invoke the same inquiry expiration operation every second;
additional replicas multiply the scans. The executor loop also reconciles
workflow waits, which is a separate responsibility.

Establish explicit expiration ownership and recovery before removing a loop.
Preserve bounded timeout detection, response/timeout race behavior, and durable
workflow wakeups. Do not remove wait reconciliation with the duplicate scan.

### Policy enforcement aggregate queries

Source: [policy enforcer](../../crates/executor/src/policy_enforcer.rs),
`check_policies_except_concurrency`, `count_executions_since`, `count_executions`,
and `count_running_executions`.

Configured rate limits and quotas issue separate aggregate queries across action,
pack, and global scopes, including all-time execution counts. Cost depends on
enabled policies, retained rows, and query plans; this does not establish a
material overhead for executions without those policies.

Consolidate compatible aggregates before considering durable counters. Any
counter design must specify concurrency, retention, retry, and reconciliation
semantics; fewer reads must not weaken enforcement.

### Existing optimizations and starting order

Notifier dispatch already memoizes authorization decisions per fingerprint for
each notification. Final socket delivery deliberately checks current authority.
Do not propose broad notifier authorization caching as though neither mechanism
exists. Profile target metadata reuse and validation composition while
preserving final delivery fences.

After baseline attribution, prioritize bulk workflow-wait reads and specialized
queue queries as contained changes. In parallel with that investigation, trace
the authorization transaction graph for the larger optimization. Workflow-log
idle backoff is another contained candidate. Reorder based on measured cost,
service replica counts, and workload frequency; this order does not replace the
safety and acceptance requirements in the delivery sequence.

## Nonnegotiable contracts

- Keep database access in repositories and preserve canonical lock ordering.
- Preserve active owner binding, sponsor suspension, revocation, attempt fencing,
  approval expiry, secret origins, and publication permissions. Retained evidence
  or cached metadata never grants current authority.
- Reacquire mutable authorization fences in each transaction. Preserve freshness
  checks after lock, storage, and decoder waits. Transaction-scoped reuse must
  account for transaction-local mutations and expiring authority.
- Preserve required history, audit, log ordering, capture completeness, evidence
  pins, independent retention, and durable completion before notification.
- Notifications are wakeup hints. Durable state and reconciliation must recover
  missed notifications, reconnects, crashes, and duplicate deliveries.
- Keep initial contracts at v1. Any schema work follows the repository migration
  policy; never reset databases or rewrite recorded checksums automatically.
- Never record raw secrets, tokens, artifact bodies, or SQL bind values in traces.

## Delivery sequence

### 1. Establish attribution and budgets

Separate three measurements: idle work per service instance per minute,
top-level database calls per logical operation, and nested database work per
mutation. Attribute connections to service and instance, and traces to operation
and phase without introducing unbounded metric labels.

Record top-level and nested statement counts separately before aggregating.
Group by query fingerprint, service, and purpose; distinguish benchmark observer
queries and background maintenance from execution work. Include round trips,
database execution time, pool waits, lock waits and hold time, transaction
duration, rows examined/changed, WAL bytes, and operation latency. Establish
whether the collector counts statements, protocol exchanges, or logical calls.
Do not add parent and nested execution times as independent wall-clock costs.

Use owned disposable stacks and identical workloads: idle single/multiple
replicas, silent and output-producing actions, event-to-execution flow, workflow
fanout, live readers, and populated list/detail views. Include concurrent
revocation, maintenance, and reconnect cases where relevant. Record warm/cold
state, retention volume, samples, configuration, and source revisions.

Deliver a ranked inventory by total database cost and frequency, with explicit
budgets for the next changes. Set numeric targets from this baseline rather
than promising an arbitrary universal queries-per-execution threshold.

### 2. Consolidate authorization validation

Start in `crates/common/src/repositories/runtime_authority.rs` and its API,
executor, worker, sensor, and notifier callers. Draw the validation call graph
for one silent execution and one log publication. Identify repeated sponsor,
policy, admission, dependency, release, and executable checks within each
transaction and across necessary authorization boundaries.

Batch related repository reads and introduce transaction-owned validation state
where a prior lock and validation remain sufficient. Deduplicate dependency
traversal within the protected operation. Preserve each lock's mode, acquisition
order, and lifetime; a join is not automatically equivalent to several locking
queries. Carry the earliest applicable expiry and recheck freshness at the
effect boundary. Invalidate reused results after relevant local mutations.

Cache immutable content separately from current eligibility. Do not replace
revocation checks with a process-wide TTL cache.

Acceptance: the traced operation eliminates identified duplicate checks while
revocation, expiry-after-wait, sponsor suspension, reassignment, retry, parent
lineage, and wrong-owner rejection tests retain their semantics.

### 3. Amortize log persistence

Trace worker and sensor capture, API upload/seal paths, and
`crates/executor/src/workflow/log.rs` with the log-stream and workflow-log-outbox
repositories. Measure database work per stream, segment, append, and seal.

Look for metadata reloaded during each append, duplicate authorization within
one protected transaction, one transaction per small log record, and separate
updates that can commit together. Batch compatible appends within existing
latency and memory bounds. Retain stream sequence, byte limits, idempotency,
attempt ownership, evidence retention, and recovery behavior.

Coordinate moving storage verification outside transactions with the durable
claim/recovery design in the graph admission plan. Do not acknowledge durable
capture early or defer required sealing past terminal completion.

Acceptance: reduce database work per captured byte/record without sacrificing
first-byte latency, completion budgets, bounded buffering, ordering, or crash
recovery. Test both shared-file and object-segment backends.

### 4. Reduce idle polling and redundant refresh

Review workflow-log dispatch, work-queue dispatch, inquiry timeout monitors,
worker/sensor heartbeats, supervisor jobs, and API stream reconciliation.
Measure empty queries and replica multiplication before changing intervals.

Use durable-state notifications to wake dispatchers where practical, drain work
in bounded batches, and apply adaptive idle backoff with jitter. Establish
listeners before catch-up reads and reconcile on reconnect. Keep a bounded
fallback poll and preserve lease renewal, deadlines, and dispatch latency.
Do not replace essential liveness heartbeats with optional UI-style refreshes.
Retain the notifier's single `listen_all()` registration contract.

In the web client, coordinate notification invalidation with polling in
`web/src/hooks/useExecutions.ts`, sensor logs, workflow waits, and artifact
refresh. Deduplicate refreshes and stop unnecessary work for terminal entities
or inactive views. Keep recovery behavior when subscriptions are unavailable.

Acceptance: idle query rates fall with no missed work or unbounded detection
delay. Exercise lost notifications, disconnected subscribers, multiple replicas,
lease expiry, and startup/reconnect catch-up.

### 5. Bound queries for lists and workflow views

Audit repository calls inside loops in API list/detail handlers, visibility
checks, secret redaction, workflow waits, and notifier fanout. Start with
per-item execution/queue-item lookups in `crates/api/src/routes/executions.rs`
and recursive descendant retrieval in the execution web hooks.

Add bounded bulk repository methods for related IDs and reuse request-scoped
results when their authorization context is identical. Consider a bounded
descendant endpoint if traces justify it; preserve visibility per entity,
pagination, cycle handling, and depth/size limits. Avoid loading a whole graph
to remove a few queries. Request totals only when consumers need them.

Acceptance: query growth follows bounded batches rather than one lookup per
returned item. Verify mixed visibility, secret protection, missing/dangling
references, deep workflows, and pagination. API contract changes require
generated-client validation.

### 6. Reduce write amplification and share immutable metadata

Trace execution, worker, sensor, and artifact mutations through history, audit,
notifications, summary invalidation, and foreign-key checks. Remove redundant
same-value updates and combine fields belonging to one semantic transition.
Assess trigger lookups and existing invalidation deduplication before adding
another mechanism. Preserve meaningful intermediate states and required audit
events; do not disable triggers to improve a benchmark.

Review caching of release manifests, executable snapshots, and compiled schema
contracts across services. Key immutable content by stable ID/digest, bound
cache memory, and coalesce simultaneous misses. Inspect existing caches before
adding new ones. Keep mutable permissions, activation state, and retention
eligibility outside immutable-content caches.

Acceptance: fewer writes, nested statements, and WAL bytes for equivalent
operations; unchanged history/audit meaning, notifications, analytics freshness,
and retention behavior. Warm-cache savings must not hide cold-start regressions
or stale authorization.

## Validation and handoff

Implement small, independently reviewable changes. Follow
[Running Tests](../testing/running-tests.md) for infrastructure ownership, test
selection, result reuse, required CI, and readiness reporting. Use real
repository and cross-service tests for authorization and durability boundaries;
unit tests alone do not establish those contracts.

For each change, report the exact revision and configuration, preserved workload
coverage, before/after query attribution, database time, lock occupancy, WAL,
latency, correctness results, and remaining gaps. Run schema preparation and
generated-client checks when their contracts change. Keep new warnings at zero.

The first implementation deliverable is a silent-execution trace and a list of
duplicate authorization checks with their owning transactions, locks, deadlines,
and safe reuse rules. Use that evidence to select the first optimization;
reorder later phases when measured cost warrants it.


## Initial implementation checkpoint — 2026-11-04

Work starts from `6dc75148f8d3e163f3f7f6b72f82d69dfd312fa1`, the
worker lifecycle PR #25 head, on a separate local branch
`perf/database-activity-reduction`. This branch depends on that PR; it does not
change the published PR. The source audit below reconciles the preliminary
findings with that final worker implementation.

### Silent execution: source and archived trace

The retained shared-file benchmark at
`ca202d5f721051377543e1618bd623d169f228fb` includes serial silent-success
execution 79. Its worker `execute_started_outcome` span contains 80 SQLx query
events, eight pool acquisitions, and three pairs of unlocked/locked policy
reads. Two policy validations precede the same input-restoration COMMIT; the
third belongs to the subsequent delivery registration transaction. Empty
`db.statement` fields must use the SQLx `summary` field: some are database-clock
queries, not transaction commands. These are archived client query events,
not a fresh end-to-end baseline, nested statement counts, or protocol exchange
counts. The benchmark used the documented 10,000 ms finalization budget.

The final worker source retains this input call graph. Its later retry/queue
correctness fixes do not make the archived measurement a new-head benchmark.
The earlier 9,902 statement profile remains aggregate evidence; do not divide
these 80 span events into that profile or treat their difference as background
traffic. Span scope omits API admission, scheduling, portions of startup and
MQ publication; tracing may also omit events outside instrumented futures.

| Transaction / owner | Validation and work | Retained locks / deadlines / completion |
| --- | --- | --- |
| Worker parameter restoration, `SecretManager::restore_execution_parameters_with_paths` | Shared admission; stored-secret and execution reads; `authorize_execution_parameter_key_origins_with_conn` → `input_authorities` → recorded admission validation; queue-contributor lookup; ordinary recorded/rule input authority | Admission → pack → sponsor binding/identity → policy → execution/executable → admission/attempt → approvals/dependencies. Keep locks through secret-origin validation/decryption and await COMMIT. Carry approval expiry through any later input-row wait. |
| Worker optional execution credential issuance | Only for nonempty `permission_set_refs`; its own current admission, owner, attempt, and approval fences | Separate transaction and current authority boundary. Empty selections never gain an API credential. This is absent from the selected credential-free execution. |
| Worker delivery registration, `completion::register` → `WorkerDeliveryRepository::register` | Validate recorded invocation, running worker assignment, retained attempt; create/read bounded delivery context | A new transaction must reacquire mutable authority. Check approval and delivery deadlines after waits and before COMMIT. Preserve this validation even when parameter restoration already validated the same policy. |
| First nonempty log write, `allocate_log_prepared` | Fresh launch-context validation, structural action target, artifact/version/stream/permit allocation | Source fences precede artifact/version locks. All metadata commits atomically; silent executions skip this transaction. |
| Shared log append, `CompletionTransport::append_shared` | Fresh delivery validation, owner binding, artifact/version/stream locks, file lock, append | Hold source/target locks through append; check the database clock after blocking-task/file waits and before COMMIT. Filesystem verification cannot move outside this transaction without a replacement durable protocol. |
| API-backed append/seal, internal file routes | Fresh mutation authorization; reuse its validated delivery fence and locked stream within that effect transaction | Keep exact permit, attempt, ownership, byte/sequence and manifest checks. The worker PR already removed some repeated delivery validation here; do not count it as a new opportunity. |
| Result redaction and terminal persistence | Close runtime-read origins in an awaited transaction; redact against retained contracts; guarded persisted terminal transition; publish persisted outcome | Preserve origin pins, history/audit/notification meaning and terminal persistence before MQ publication. Nested trigger SQL is measured separately from client calls. |

The current base already provides service/instance/purpose attribution through
`db::ConnectionIdentity` and acknowledged owner timing through
`observability::SharedGraphTransactionTrace`. Reuse these for the next workload
capture. Only actual transaction owners may acknowledge commit/rollback; span
destruction still does not prove lock release. Extend missing owner coverage
rather than introducing a second tracing or authorization-cache framework.

### Duplicate-check inventory and first budgets

| Candidate | Owning scope and reuse rule | Decision |
| --- | --- | --- |
| `input_authorities` falls through to `input_authority` after finding no queue contributors | Both perform `find` → `require` → input-liveness checks on the same connection, within the caller's transaction. Reuse only inside this one call. Keep the initially acquired sponsor/policy/owner/approval locks and a final DB-clock fence after input metadata waits. No process cache or state survives a public call. | First authorization change: resolve the recorded ordinary input authority through a private helper after one validation. Each separate public call must still see transaction-local revocation, suspension and terminal transitions. Budget: one admission traversal for the recorded no-queue collection path. |
| Repeated foreign workflow policy edges | `lock_policy_with_deadline` loads and validates a dependency before traversal's visited check. A shared diamond or multiple pins may repeat sponsor/policy/approval reads. Any future deduplication must still verify every pin and retain the earliest expiry, graph bound and lock order. | Investigate next; not implemented by the initial checkpoint. |
| Sponsor reads shared by policies | Initial policy discovery precedes sponsor locks and a locked policy reread. Joining or caching can change row-lock ordering and freshness. | No removal without an owned transaction design, mutation invalidation rules, and concurrent revocation/expiry evidence. |
| Same policy in input restoration and delivery registration | Locks end at the first COMMIT. Later registration grants a distinct effect. | Required new validation; do not share a positive authorization across transactions. |
| Queue capacity search discards fetched rows | `dispatch_ready_batches_for_queue` uses only `.total`; its existing search runs COUNT plus an ordered row query. Neither read supplies authorization. | Replace with `count_active_for_queue`; budget one application SELECT per capacity observation, preserving queue/status predicates and dispatch behavior. |

The direct single-authority and queue-contributor paths gain a final post-read
database-clock check. The query reduction is specific to the ordinary recorded
collection path; it is not a claim that every input-read path issues fewer SQL
statements. No validation result is cached across public calls or transactions.

The initial scope deliberately does not alter graph admission, storage claims,
maintenance scheduling, notifier delivery fences, heartbeat cadence, or API
contracts. Broader idle/per-service attribution, nested SQL, WAL and lock-occupancy
budgets remain required before claiming repository-wide savings. Query-count
reductions here are not an end-to-end throughput or latency claim.

### Validation record

The first functional query probe compares baseline `3147d31589c3f9b8fa91cbe5d4bafedd82cb6cc0`
plus the added regression fixture against implementation
`94aa4b6d` using the same positive operation: a running recorded execution with
external approval evidence, a minimal accepted human input snapshot, frozen
original human, and no queue contributors.
Fixture setup and subsequent rejection assertions are outside the observed
`input_collection_probe` span. The baseline fixture overlay and both executable
hashes are retained in `.scratch/database-activity/`.

| Events inside one operation | Baseline | Treatment |
| --- | ---: | ---: |
| SQLx client query events | 46 | 27 |
| Driver PostgreSQL catalog queries | 4 | 4 |
| Application query events | 42 | 23 |
| Unlocked policy reads | 2 | 1 |
| Locked policy reads | 2 | 1 |

This is a single functional probe, not a latency benchmark. It does not measure
secret decryption, queue contributors, or an end-to-end execution. Both runs
occurred with build activity; no wall time, server-time, WAL, lock-occupancy, cold/warm or
end-to-end performance conclusion is drawn from them. SQLx query events are not
protocol exchange counts or nested server SQL. Catalog discovery can nest client
timing events, so their durations must not be summed as independent database
execution time. The separate count-only queue tests also recorded their SELECT
fingerprints in owned PostgreSQL 18 `pg_stat_statements` with `track=all`;
those read statements reported no WAL records. This is not a workload WAL result.

To repeat the functional probe on an owned test database, run the existing
`runtime_authority_repository_tests` target filtered to
`input_authority_validation::recorded_input_reads_revalidate_local_mutations_and_keep_accepted_human_authority`
with `RUST_LOG=sqlx::query=debug,runtime_authority_repository_tests=debug`, a unique
`ATTUNE_TEST_RUN_ID`, `--nocapture`, and `--test-threads=4`. Count only SQLx query
events in `input_collection_probe`, using `summary` when `db.statement` is empty,
and separate `pg_catalog` discovery from application statements. Do not count
fixture setup or logging events as queries.

Static checks passed on code checkpoint
`94aa4b6d2e683fa1dc579c561a4f6421358d5392` (only this plan was untracked):

- `cargo check --workspace --all-targets`.
- `cargo clippy -p attune-common -p attune-executor -p attune-worker --all-targets --no-deps -- -D warnings`.
- `cargo fmt --all -- --check`, explicit formatting of the new included test module, and `git diff --check`.

The owned PostgreSQL 18 instance was freshly migrated through `make db-test-setup`
before this code checkpoint. Migration bytes are unchanged through the checkpoint;
the same owned instance was reused and the suites created their migration-hashed
physical clones normally.

Both new regression cases passed independently and within the full authority
suite: local source mutations/historical human authority, and consent expiry
during an observed metadata wait for both public read paths.

The following suites passed on that same code checkpoint, with four test threads
and a unique run identity per invocation:

| Target / filter | Passed | Failed / ignored | Run identity |
| --- | ---: | --- | --- |
| common `runtime_authority_repository_tests` | 271 | 0 / 0 | `dt-b76aec2add` |
| common `execution_admission_tests` | 24 | 0 / 0 | `dt-37c6ef589d` |
| worker `recorded_input_restoration_tests` | 7 | 0 / 0 | `dt-f4fc3fae6f` |
| common `work_queue_repository_tests` | 10 | 0 / 0 | `dt-8588049962` |
| executor library `queue_dispatcher::tests::` | 18 | 0 / 0; 299 intentionally outside this filter | `dt-2c1d501f0e` |

The authority target completed in 1,188.45 seconds; this validation duration is
not a performance comparison. These are 330 passing cases across the selected
suite runs, with the earlier focused probes reported separately. No assertion,
readiness gate, approval lifetime, or authorization predicate was relaxed to
obtain passing results. Worker coverage includes independent contributor ceilings,
rejection after IAM widening, terminal/revoked admission, and rejection of any
attempt to replay admission/attempt creation.

Binaries were copied into the run-owned evidence directory and verified by
SHA-256 before execution. Full commands, logs, executable hashes, query event
summaries, and test run identities are retained under
`.scratch/database-activity/`. Subsequent edits are confined to this plan and a
clarification of the count method's documentation comment; they cannot affect
these runtime results, so the recorded checks are reused.
Schema and public API contracts are unchanged; SQLx metadata and generated-client
updates are not applicable to this slice.

Status: **work in progress** for the broader plan and merge readiness. Required CI
has not run for this local branch. At this code checkpoint, full unfiltered
Rust/E2E suites and broader performance measurements remained outstanding. The
campaign below supplies full-stack execution/idle counters, server time, WAL,
and observed API holder timings; it does not supply full lock occupancy or
production ranking. The
single-operation query trace supports only this contained reduction. Workflow-wait
batching, policy-graph traversal reuse, idle workflow-log dispatch, inquiry ownership,
log batching, and the remaining write/metadata opportunities are not implemented
by this checkpoint.

Owned PostgreSQL teardown completed with zero client sessions before removing
the container and its volume; absence of both resources was independently
verified. Ownership and cleanup results are retained in
`.scratch/database-activity/db-cleanup.json`. No deployment or database reset is
part of this work.

## Broader measurement campaign — 2026-11-05

The fresh full-stack campaign is complete; see the
[measurement report and compact dataset](../testing/benchmarks/database-activity-2026-11-05/README.md). Runtime checkpoints are the original
pre-worker-optimization `6906b968`, the validated worker treatment `ece3c859`
(runtime source/build inputs identical to worker PR head `6dc75148`), and the
database-reduction checkpoint `747832c9`. Validated baseline image identities are
retained; the database treatment service/agent images are rebuilt. Python test
instrumentation and observer images are identical across phases and recorded
separately from runtime source revisions.

Primary comparisons use six action workloads, serial and concurrency-four
profiles, shared-file and API/object-segment transports, and 100 measured samples
per profile. Each action's first invocation and 16 warmup executions remain
separate. All primary runs use the same 10,000 ms capture-finalization budget;
current-treatment 1,000 ms default-budget probes are separate 20-sample runs.
The accepted primary dataset contains **7,200 measured executions** across the
three checkpoints and two transports, with 72 complete profiles. All configured
primary correctness checks and 12 post-matrix reader probes pass; provenance,
counter epoch and teardown checks pass. The initial pilot rejected by an
incorrect original-build Git-metadata assumption is preserved and excluded.
The report records the corrected image-identity validation and fresh rerun.

Each disposable stack includes eight enabled empty queues, a 60-second settling
interval, and a 60-second idle measurement. This exercises capacity observations
rather than measuring a stack with no enabled queues. Counter snapshots separate
top-level/nested SQL and observer statements, collect server execution time, WAL
and block counters, and reject resets/evictions. Nested time and WAL can overlap
parent accounting; top-level totals are reported separately. The statement table
is sized to 50,000 entries on every stack to retain startup and workload entries.

Execution clocks separately record HTTP-request-to-observed durable terminal
notification and HTTP-request-to-observed MQ completion. The latter includes
management polling and is not exact publication time. Original-baseline incorrect
failure message statuses remain recorded defects, not silently accepted as
correct behavior. Whole-stack SQL/profile totals include background work and
must not be described as attributed worker round trips.

Raw snapshots, manifests, build logs, image identities and analysis scripts are
retained under `.scratch/database-measurements/`. Collection runs sequentially
after image compilation to avoid compiler load contaminating runtime latency.
All three snapshots receive the same instrumentation overlays; runtime inputs remain
frozen. The original baselines are reused only with explicit image/source
provenance; they are measured again on fresh owned stacks, not compared solely
against historic timings.


### Measured outcome and next priorities

Combined original → database-checkpoint top-level SQL/job falls **52.9% with
shared files and 48.0% with API segments**. Median durable completion improves in
all 24 matched profiles; p95 improves in 22. Beyond the worker checkpoint, the
first database slice removes **9.1% / 8.7%** of top-level calls/job, with roughly
18 fewer execution-scoped worker SQLx events/job. Original worker client-event
attribution is unavailable because its execution spans are absent. Background
work is included in whole-database totals; no round-trip attribution is claimed.

Incremental completion medians improve in 20/24 profiles, while p95 improves in
11/24. Shared-file PostgreSQL execution time rises 3.7% despite fewer queries;
API execution time falls 9.7%. Incremental WAL does not improve consistently.
These results support the contained read reduction, not a uniform latency,
contention or write-volume improvement.

The 1,000 ms shared-file probe passes serial echo then fails concurrency-four
echo's sealed/ready output assertion. Its failed-profile timings are unavailable
because validation stops before report emission. The separate API default probe
passes all 240 measured executions and its two reader cases. No matched default
worker-baseline run was performed here, so the shared-file failure's introduction
is unresolved. The 10,000 ms primary lane remains distinct; production timeout
configuration and assertions are unchanged.

Idle attribution places the supervisor at approximately 4,860–5,301 SQLx events/min
and the two executors at 2,469 events/min in the database checkpoint. The empty
workflow-log outbox candidate query executes 1,170 times/min. Worker heartbeats
contribute 24 events/min across four workers. Exact per-service/client and server
counts differ in coverage, and sensor SQLx attribution is absent. Idle-rate
improvement is not established by the single one-minute windows.

Next measured priorities are therefore idle outbox wakeups/claims, supervisor
maintenance configuration/cadence work, and shared-file log sealing. Observed API
seal holders remain expensive (database checkpoint p50 174.2 ms, p95 619.3 ms),
while pool-acquisition p95 remains below 0.4 ms for instrumented services with
events. Preserve delivery fences and independent maintenance budgets when
working these opportunities. Workflow/permission-heavy paths, populated queues,
real object storage, independent repeated trials and full holder occupancy still
need measurement.

Status: **work in progress** for the broader reduction plan and merge readiness.
The measurement objective is complete with the default shared-file failure
explicitly retained as a gap. Raw evidence stays in ignored scratch storage;
committed CSV/JSON exports preserve individual primary latencies, profile
counters, worker phases, observed API holder timings and provenance. All nine
campaign stacks have verified owned-resource cleanup. Required CI and unfiltered
workspace/full-stack suites have not run for this local branch.


### Idle attribution correction and implementation analysis — 2026-11-05

A follow-up source/log investigation found that the E2E runner globally sets
retention to **two seconds** during startup. All six primary supervisor logs
confirm that interval. The 4,860–5,301 supervisor events/minute above include
22–24 full maintenance cycles per minute; they are not production-default idle.
Paired execution results still describe identical accelerated maintenance across
phases, but normal-cadence performance is not established. Production ranking
of supervisor work requires a fresh baseline with actual persisted settings
recorded. Avoid extrapolating the earlier idle totals to a normal installation.

The [idle database activity analysis](idle-database-activity-analysis.md) ranks
specific implementation candidates and their safeguards: empty workflow-log
wakeups, unnecessary previous-dispatch reads, batched queue readiness hints,
retention seeding in configuration reads, inquiry ownership/deadlines and safe
connection reuse. The first three are stronger immediate candidates than
changing maintenance cadences. Estimates are explicitly unmeasured; no runtime
implementation is included in this follow-up.
