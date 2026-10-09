# Cache entry partitioning by generation

Status: schema and repository implementation in progress, 2026-10-07. The fresh
baseline now defines LIST entry partitions and guarded atomic reclamation.
Full workflow, API, supervisor, and workload acceptance remain pending. This
document does not claim operational readiness.

This worktree incorporates the proposal from the main checkout. The original
proposal remains untouched. The implementation follows this branch's
[fresh-install migration policy](../deployment/postgresql-only.md), rather than
adding populated-cache conversion to the delivery scope.

## Scope

Store each generation's entries in one native PostgreSQL LIST partition. Reclaim
an eligible generation by dropping its partition in the same transaction as
accounting and metadata cleanup. Partition only `cache_entry`. Namespace,
generation, ingest-chunk, and workflow-iteration metadata remain ordinary tables.
No DEFAULT partition, background partition creator, dual reader, or v2 format.

Keep ownership, authorization, pagination, quotas, workflow pins, refresh
idempotency, and generation IDs unchanged. Preserve the new `reuse`, `conflict`,
and `parallel` policies and trusted `created_by_execution` attribution. Matching
refresh retries and reuse validate existing storage without creating another
partition or consuming capacity. Reuse never changes the original producer or
upload contract.

Expected deployment workload is few namespaces with a few retained generations,
refreshing every 15 minutes or hourly. Source fetching may take minutes, but must
start only after partition creation commits. Retained capacity includes staging,
ready, active, retired, failed, and pinned generations, including tombstoned owners.

Reporting-performance work remains deferred. This proposal's cleanup and
read/ingest disruption measurements are separate acceptance gates, not evidence
that deferred analytics optimization has passed.

## Schema and accounting

- Define `cache_entry PARTITION BY LIST (generation)` directly in the canonical
  pre-production migration. Use `PRIMARY KEY (generation, id)` and the shared
  BIGINT sequence. Do not rely on global uniqueness of `id` alone.
- Keep unique bytewise `(generation, external_id)`, value/text limits, the
  generation FK, staging-only INSERT, and entry immutability. The primary key
  replaces the redundant `(generation, id)` index.
- Reject entry UPDATE and DELETE. Remove routine entry batch cleanup and its
  DELETE accounting trigger only when guarded partition reclamation is ready.
- Add `cache_generation_entry_usage`, keyed by generation with a restrictive FK,
  exact nonnegative `record_count` and `physical_bytes`. These count admitted
  entries using existing `size_bytes`, not relation/index/TOAST disk size.
- Create the zero usage row transactionally with the generation. Extend the
  parent INSERT transition-table trigger to maintain usage alongside deployment
  and canonical-owner counters. Keep sealed snapshot metadata semantics intact.
- DROP does not execute DELETE triggers. Subtract the usage row explicitly in
  the drop transaction, with checked arithmetic and affected-row assertions.
  No per-entry DELETE, COUNT, or SUM during normal reclamation.

Fresh installations are the target. No automatic reset, checksum rewriting of
recorded migrations, populated heap conversion, or unapproved cache discard.
Starting with 1.0.0, preserve released migration bytes and use forward,
data-preserving migrations. A populated conversion would be separately scoped.

## Repository interface

Keep relation naming, catalog validation, accounting, and DDL private to the cache
repository module. Normal entry reads and writes use `cache_entry`.

Begin refresh acquires admission and relation protection, then namespace metadata.
After idempotency and refresh-policy handling, enforce the global partition cap,
insert generation and usage rows, create an empty standalone child with copied
defaults/checks and a validated `CHECK (generation = <positive-i64>)`, and ATTACH
it. Attachment supplies indexes, FKs, and row triggers. Verify the resulting
catalog shape before commit. Any error rolls back all three resources.

Prefer standalone CREATE plus ATTACH over `CREATE TABLE ... PARTITION OF` to
avoid an ACCESS EXCLUSIVE parent lock during creation. Resolve the parent schema
from search_path/catalogs. Quote identifiers centrally and derive child names only
from validated positive i64 IDs. Reject unexpected names or bounds. Never accept
an API-provided relation name or hide inconsistency with `IF NOT EXISTS`.

Replace `delete_cleanup_batch` and `delete_if_empty` with one
`CacheGenerationRepository::drop_if_cleanup_eligible` operation. Typed outcomes
distinguish committed reclamation with records/bytes, ineligible/already absent,
and contention deferral. Permission and integrity failures remain errors.

One transaction per generation must:

1. Apply bounded local timeouts and acquire locks in canonical order.
2. Lock namespace and generation, then recheck failed/expired-retired state,
   database-time expiry, minimum traversal window, no active pointer, and no live
   scanning workflow pin. Candidate selection is only a hint.
3. Verify the exact attached child, schema, single generation bound, and usage row.
4. Lock counters in the same order as ingestion, then the generation usage row.
5. DROP the validated child without CASCADE, subtract exact bytes, remove usage,
   chunks, and generation metadata, and handle terminal iteration cascades.
6. Commit before reporting totals. Failure rolls back DDL and counters together.

Keep quota charged until physical reclamation commits. An already absent
generation cannot decrement counters again. Missing storage for an extant
generation is an integrity failure, not an empty snapshot.

Chunk and terminal-iteration deletion still costs row work. Measure large chunk
counts separately and resolve metadata admission bounds if cleanup cannot fit its
statement budget. Do not describe the entire transaction as constant-time.

## Locking gate

Transactional DROP takes ACCESS EXCLUSIVE on the entry parent and briefly blocks
other cache traffic. This design chooses atomic cleanup and simple recovery, not
zero blocking. Use `LOCK TABLE ONLY` to avoid recursive leaf locks.

Canonical order is admission advisory lock for cache mutations and pin cascades, parent
relation, namespace, generation, deployment counter, bytewise ordered owner
counters, and generation usage. Parent modes are ACCESS SHARE for reads/seal/pins,
ROW EXCLUSIVE for upload, SHARE UPDATE EXCLUSIVE for attach, and ACCESS EXCLUSIVE
for drop. Readers do not acquire admission.

Cleanup queues for admission under the actual remaining cleanup-cycle budget,
not the short DDL lock timeout. The supervisor passes that remaining budget to
the repository for each candidate. Pool acquisition, transaction setup, admission,
DDL, rollback and commit acknowledgement all share its monotonic deadline.
Admission has server-local statement and lock timeouts bounded by the remaining
cycle; it takes no parent relation lock. After admission, cleanup applies the
unchanged DDL limits, clamped again to the time actually left. The SQL function's
advisory call is reentrant in the same transaction. Creation, ordinary reads,
pin mutations and their parent modes are unchanged.

An admission timeout is `DeferredDeadline`, including `55P03` when the cycle-bound
lock timer wins the race with `57014`. In the DDL phase, `55P03` is `DeferredBusy`
when the short lock limit wins, or `DeferredDeadline` when that limit was clamped
to the cycle deadline; `57014` remains `DeferredDeadline`. Structured cleanup logs
record the phase and admission duration. A transport deadline or task cancellation
discards the connection rather than returning pending SQL/rollback to the pool;
dropping a Rust future alone does not cancel a PostgreSQL statement. Server timers
still bound pending SQL. An unacknowledged commit is an error with an uncertain
outcome, never a claimed drop or deferral. Only acknowledged committed reclamation
increments supervisor drop and usage counters. This ordering does not establish
benchmark performance acceptance.

Acquire relation protection before any workflow or iteration row that a cleanup
cascade can touch. A late lock inside the entry scan helper is insufficient.
Audit transaction entry in API authorization, owner deletion, seal/promotion,
tombstone, executor initialization/refill/completion, and pin repositories.
Keep pin protection until its durable pin commits; cleanup rechecks pins after
exclusive generation protection.

Scheduler activation, wait resolution, cache dispatch, and completion transaction
entry use `PinMutation`: mutation admission, parent ACCESS SHARE, then workflow,
iteration, and generation rows. Admission uses PostgreSQL's two-integer key
space, separate from single-BIGINT workflow advisory IDs. Ordinary `Read`
does not acquire mutation admission. API
transactions acquire admission and the appropriate parent mode before sensor
fences or owner checks. Repository read and lifecycle helpers also protect the
parent before generation or namespace locks. Full production interleaving
acceptance must still cover these paths, not only the SQL model.

The rejected parent-only protocol reproduced SQLSTATE `40P01` on PostgreSQL 16
and 18 when two workflow roots pinned two generations in opposite order. Sorted
per-statement counter locks do not protect this multi-statement case. Green
production workflow/pin/cascade acceptance and mutation-admission latency remain
pending; serializing mutations is a correctness choice, not a speedup claim.

`scripts/probe-cache-generation-partitions.py` exercises an isolated SQL model
on owned PostgreSQL 16/18. It tests non-superuser table-owner ATTACH/DROP,
transition accounting, inherited row triggers, rollback, expected row-first
deadlock, parent-first reader/pin ordering, and bounded lock deferral. It is not
proof of the production repository, executor, deployment roles, or all cascades.

Do not ship until deterministic repository/executor interleaving tests cover
both arrival orders, upload/seal/promote/tombstone, workflow completion/cascades,
cancellation, and held transactions. Synchronize on observed locks or explicit
signals, not fixed sleeps. Roll back on lock timeout and continue later candidates.

Concurrent DETACH is outside this scope. If measured parent blocking fails the
gate, stop rollout and design durable pending-detach recovery separately.

## Configuration and operations

Add `cache_admission.max_entry_partitions`. Count every attached generation state
until reclamation; enforce under admission locking. Allow matching retries at the
cap and return a typed admission failure for new refreshes. Benchmark expected
inventory and staging/failure/pin headroom before fixing the default cap.

Replace cache-only `batch_size` and `max_batches_per_generation` with a total
cleanup time budget and DDL lock/statement timeouts. Retain per-cycle generation
and namespace limits. Check the remaining budget before each attempt and do not
cancel after a commit without recording progress. Rotate attempted candidates so
a blocked oldest generation cannot starve later candidates across cycles.

Update persisted retention JSON, validation/OpenAPI, generated web/Python clients,
config examples, and tests together. Unrelated runtime-retention batch settings
remain unchanged. Dry-run performs no DDL or counter changes.

Report created/dropped partitions, reclaimed records/bytes, lock deferrals,
backlog age, partition count, and DDL duration. Preserve committed progress when
a later operation fails. Use low-cardinality labels; never log cache values or
external IDs. Verify schema CREATE and effective ownership using deployed roles.
Do not grant superuser or arbitrary drop privileges to solve DDL access.

Refresh statistics after loading/sealing and define bounded parent ANALYZE
maintenance. Leaf autovacuum does not analyze the partitioned parent. Keep ANALYZE
outside the short drop transaction and measure its cost.

## Acceptance

Use [owned fixtures and runners](../testing/running-tests.md). Run PostgreSQL 16
and 18 and preserve existing common cache, API, executor iteration, supervisor,
migration, retention-config, and E2E semantic coverage.

Cover creation/reuse/retry atomicity, injected DDL failure, exact-cap/retry-at-cap,
missing/wrong-bound partitions, routing/pruning, duplicate numeric IDs across
generations, byte-bounded pagination, SQLx generic/prepared plans after repeated
DDL, zero-entry drops, ingest/drop rollback, chunk replay, detached owners,
active/unexpired/pinned protection, traversal windows, fair deferral, and dry-run.

Compare identical baseline and treatment cycles with 200k and at least 1m records
per large generation plus small caches, expected retained inventory, and scheduled
refresh/cleanup bursts. Record cleanup duration, WAL, dead tuples/vacuum work,
relation/index/TOAST sizes, backend/planner memory, create latency, and read/ingest
p50/p95/p99 during cleanup. Include cold/warm runs and long-lived connections.

Declare inventory, partition headroom, numerical latency SLOs, and comparison
thresholds before benchmarking. Acceptance requires measured cleanup/WAL gains,
single-generation pruning, no deadlocks/leaks/accounting drift, and bounded
read/ingest/refresh disruption. Performance acceptance is pending until those
deployment inputs and measurements are recorded.

### Provisional benchmark targets

The user approved provisional targets on 2026-10-07. Use 8 namespaces with 4
retained generations each, plus staging/failure/pin headroom to the provisional
128-partition cap. Compare 200k and 1m-record large generations plus small caches,
with synchronized eight-namespace refresh/reclamation bursts and four concurrent
read/ingest clients. Use identical record payloads and full lifecycle coverage.

For the first native PostgreSQL release, the user subsequently accepted the
observed slower page and creation timings. Numerical latency targets and the
70-percent wall-time / 90-percent WAL reduction targets below are report-only,
not release gates. Record all timings, failed calls and deferrals. Do not rewrite
sealed failed runs or treat lifecycle, accounting, rollback or deadline failures
as acceptable latency. The 128-generation cap remains provisional; report its
measured suitability for the tested workload, not a universal capacity guarantee.

Correctness and bounded progress remain release requirements: preserve published,
readable and pinned data, exact storage/quota accounting, atomic reclamation,
transactional rollback and replay, and eventual cleanup under the normal burst.
The existing whole-cycle bound and server-side DDL limits still apply. Accepting
slower end-to-end work does not permit unlimited admission waits, larger
production timeouts, disabled autovacuum or smaller metadata populations. A new
external correctness report may assess completed native evidence separately from
the old performance comparator. It must retain coverage gaps and the original
comparator verdict rather than relabel that verdict as passing.

During cleanup, target p95 below 250 ms for small point/page reads, below 1 second
for 1,000-record chunk ingest, and below 1 second for refresh creation. Target at
least 70 percent less full-generation reclamation wall time and 90 percent less
cleanup WAL than bounded row deletion. Record cold/warm p50/p95/p99 and deferred
operations, not only successful calls. These original comparison thresholds are
now report-only, not measured guarantees. DDL defaults are a 250 ms lock wait, a 1-second cleanup
statement deadline, a separate 5-second creation statement deadline, and a
30-second cleanup-cycle budget. None is performance-validated yet.

The user approved a separate creation budget after two unchanged-source
four-thread PostgreSQL 16 runs hit the 1-second creation deadline, including
empty-partition ATTACH. The creation budget is
`cache_retention.ddl_creation_statement_timeout_milliseconds`, provisionally
`5000`. Cleanup continues to use `ddl_statement_timeout_milliseconds: 1000`.
Admission coordination precedes DDL timeout application so concurrent refresh
callers can inspect the winner. The below-1-second creation p95 is retained for
reporting; native maintenance-status deadlines remain unchanged. Failed results are retained at
`/tmp/opencode/cache-68eb9df2e292/evidence.json`,
`/tmp/opencode/cache-db759eacdd30/evidence.json`, and
`/tmp/opencode/cache-d1a2680b05fe/evidence.json`; none counts as acceptance.

Finally run `cargo sqlx prepare`, targeted tests,
`cargo fmt --all -- --check`, `cargo check --all-targets --workspace`, and generated
client/type checks. Update `docs/KEY_CACHE.md`, `docs/deployment/supervisor.md`,
deployment docs, and the doc-site cache pages when partition storage is enabled.

Reference: [PostgreSQL 16 partitioning documentation](https://www.postgresql.org/docs/16/ddl-partitioning.html).

## Current validation evidence

The preparatory repository changes constrain both byte-bounded entry joins and
the existing batch-delete join by generation as well as numeric entry ID. The
new repository regression uses duplicate IDs in isolated, transaction-local
storage. Its pre-fix PostgreSQL 16 run failed with two returned rows where one
was expected; the other 42 repository tests passed. Failed evidence is retained
at `/tmp/opencode/cache-2bfdb730de66/evidence.json`.

The SQL-model gate passed on PostgreSQL 16.15 and 18.6 at
`/tmp/opencode/cache-partition-protocols-composite.json`. Both actual repository
bounded-scan SQL strings returned only the selected generation with duplicate
numeric IDs, including byte-limited continuation. Forced generic plans accessed
only that generation's leaf. Non-superuser owner ATTACH/DROP, transition-table
accounting, creation/ingest/drop rollback, live-pin visibility, terminal-iteration
cascades, and bounded lock deferral passed in the model. Both generation-first
and iteration-first protocols deliberately produced `40P01`; parent-first
interleavings completed. All owned sessions, containers, and volumes were removed.

An earlier probe startup/teardown failure is retained at
`/tmp/opencode/cache-partition-protocols-final.json`. Its remaining owned volume
was explicitly removed after checking its ownership label; recovery evidence is
`/tmp/opencode/cache-partition-protocols-cleanup-recovery.json`. Do not count that
failed run as a passing gate.

The first native-storage repository run passed all 43 tests on PostgreSQL 16 and
18 with unchanged inputs and zero owned resource leaks, at
`/tmp/opencode/cache-e2878049a699/evidence.json`. Subsequent cap, storage integrity,
drop rollback, traversal-window, and reverse reader-arrival regressions extend
that source and require a new run. The actual migration lifecycle functions also
passed the non-superuser SQL-model gate on both versions at
`/tmp/opencode/cache-partition-production-functions.json`.

Partition creation, guarded DROP, transaction-entry protection, provisional
capacity defaults, and retention contract replacement are implemented. Full
workflow/API/supervisor correctness acceptance remains pending; numerical
performance comparisons are report-only for the first native release. No installed
database was reset and no recorded migration checksum was rewritten automatically.

The subsequent operational changes still need acceptance. Parent/leaf statistics
have independent persisted defaults of 300 seconds and a 5,000-millisecond
statement deadline. Lifecycle changes request a revision; analysis acknowledges
only its earlier snapshot. Dry-run and deferrals preserve pending work. Sampling
covers lookup/scan columns rather than JSON values. Observations report partition
count, cumulative creation/drop counts, eligible backlog and age, reclamation
duration, and statistics progress/failures. Partial maintenance failures preserve
already committed cleanup counts for auditing.

Generation metadata is bounded at 10,000 ingest chunks and 10,000 retained workflow
iterations, including terminal iterations. Database constraints and atomic
iteration counters enforce admission; conflict replay does not consume another
slot. Iteration generation/namespace cannot change in place. New leaves receive
the catalog parent owner so distinct API and supervisor logins sharing that role
can create and reclaim the same storage. Cross-service role acceptance and
high-metadata cleanup measurements remain pending.

The separate creation-budget source passed all 49 cache repository tests on
PostgreSQL 16 and 18 at four threads, without skips or input changes, at
`/tmp/opencode/cache-e22db85ed1c1/evidence.json`. The new regression observed a
persisted 7-second creation deadline and the unchanged 250 ms lock-wait limit;
an owned deletion trigger verified cleanup still ran under its own 1-second
statement deadline. Owned databases, containers, and volumes were removed.
Configuration/API validation tests, generated-client parity and contract tests,
web type checking and retention-page tests, and workspace compilation passed.
This validates deadline separation, not the below-1-second creation p95 target
or the remaining full partition-acceptance gates.
