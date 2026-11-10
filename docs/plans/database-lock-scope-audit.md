# Database lock scope and latency audit

Status: source audit and proposed work; application behavior unchanged.
Date: 2026-11-09 UTC.

## Recommendation

Reduce lock occupancy and the number of unrelated operations sharing a gate.
Prioritize storage I/O inside transactions, schema-wide graph admission,
sensor admission, partition DDL, and global cache admission. Preserve short
atomic claims and transitions until an equivalent concurrency guarantee replaces
them. Removing every explicit lock would leave implicit database locks and
introduce races in publication, authorization, quotas, and cleanup.

The most concrete opportunities are:

1. Move log verification and filesystem work outside database transactions,
   using a durable, narrowly scoped lifecycle fence where necessary.
2. Avoid strong maintenance locks for work that can be rejected or skipped before
   admission. Bound both acquisition and the complete transaction, with backoff.
3. Replace broad coordination with per-resource admission where dependencies
   permit it, starting with sensor/rule admission and independent log versions.
4. Remove repeated validation and redundant locking within one transaction;
   replace suitable read/lock/write sequences with conditional writes.

These are implementation candidates, not measured speedup promises. Historical
latency evidence shows simultaneous delays across services, but does not identify
a particular lock as the cause of the November 9 timer gaps. Capture blockers and
wait events before attributing those stalls to locks.

## Scope and evidence

The primary checkout is 880b1b2a138a335eb549c08452f0d3440de68d41, including
existing local changes. The review searched Rust sources, migrations, and other
SQL-bearing scripts, then followed acquisition helpers, callers, transaction
boundaries, and relevant trigger overrides. Tests and diagnostic probes are not
treated as production lock sites.

The previously deployed 0.10.3 source archive was also checked. Graph admission,
sensor admission, cache admission, pack filesystem cleanup, and the partition SQL
definitions cited below are byte-identical in that archive. Internal file routes,
runtime authority, and the workflow scheduler differ; their relevant locking
patterns were compared separately. In particular, 0.10.3 already has SessionLease
disposal guards for workflow session ownership. Older manual transaction handling
in this checkout is not evidence of a deployed session-leak bug.

Local investigation evidence is in the workspace's
.scratch/platform-db-analysis/lock-audit/lock-site-candidates.json. It includes
source locations, inline-test annotations, and selected release comparisons.
It is a text-search inventory, not an AST proof or runtime trace; historical
migration definitions may be superseded. This document records the evaluated
families independently of that scratch artifact.

Coordinate this work with the existing plans:

- [Graph admission contention](graph-admission-contention.md).
- [Bounded retention query optimization](bounded-retention-query-optimization.md).
- [Single-writer log streaming](single-writer-log-streaming-simplification.md)
  and the [object-log seal claims review](object-log-seal-claims.md).
- [Database activity reduction](database-activity-reduction.md) and
  [transaction reuse remediation](database-transaction-reuse-remediation.md).
- [Code-level implementation map](database-lock-reduction-implementation.md) and
  [test assumption audit](database-lock-test-assumptions.md) translate these
  findings into change boundaries and conditional test updates.

No application edits, live lock experiments, database mutations, migration,
release, or deployment were performed for this audit. Existing local work is
preserved.

## What the different locks mean

Attune explicitly requests advisory, row, and table locks in Rust and its own
SQL functions/triggers. PostgreSQL supplies the lock primitives; Attune chooses
these application protocols. Ordinary writes, unique constraints, foreign keys,
and DDL also acquire locks without an explicit lock call in application code.

Transaction-scoped locks generally last until transaction completion, not the
return of the helper that acquired them. Releasing a savepoint normally retains
its locks in the outer transaction; rolling back to it releases later locks.
Session advisory locks can survive commit and require confirmed unlock or
session disposal. An async task awaiting either kind can retain a connection and
other locks throughout the wait.

Row locks block conflicting operations on the same rows. ACCESS SHARE permits
normal DML; SHARE UPDATE EXCLUSIVE also permits normal DML but conflicts with
other maintenance/schema operations. SHARE blocks normal table writes. ACCESS
EXCLUSIVE blocks all table access. The latter modes have much greater direct
reach than a lock on one execution. These distinctions follow PostgreSQL's
[lock compatibility rules](https://www.postgresql.org/docs/18/explicit-locking.html).

There is no single explicit lock here on the entire PostgreSQL server. Broad
application gates, table locks on busy parents, and chains of blocked transactions
can nevertheless delay many services and exhaust connection availability.

## Lock family evaluation

### 1. Shared graph admission and exclusive reclamation — high priority

Sources: [schema type repository](../../crates/common/src/repositories/schema_type.rs),
[maintenance admission](../../crates/common/src/repositories/maintenance_admission.rs),
[bounded release collector](../../crates/common/src/repositories/pack_retention/bounded.rs),
and [file evidence migration](../../migrations/20261015006005_file_evidence_retention.sql).

The schema-wide named-type-graph advisory key has shared holders for selection,
publication, and evidence capture, and exclusive holders for mutation/reclamation.
File-evidence DELETE entry triggers request exclusive admission even when a DELETE
matches no rows. The upload-ledger entry trigger requests shared admission. These
trigger paths are easy to miss when searching only the calling repository.

The gate prevents deletion while dependencies or evidence are being attached.
Independent shared holders coexist, but an exclusive operation couples otherwise
unrelated resources. Slow work inside either mode increases interference.

Decision: retain the ordering protocol initially. Discover bounded candidates
outside the exclusive transaction; acquire admission only for a bounded page,
recheck eligibility, mutate, and commit. Optimize query planning as well as query
execution so an empty sweep does not occupy a gate or connection unnecessarily.
Use deferral/backoff instead of repeated unsuccessful acquisition loops.

Removal or sharding requires a complete replacement for dependency capture versus
reclamation, including cycles, negative lookups, and implicit trigger paths. A
per-pack gate alone is insufficient for cross-pack and cross-resource evidence.
Never change only the Rust helper while leaving SQL trigger participants on a
different protocol. Preserve each entry point's proven lock order and the
existing rejection of shared-to-exclusive upgrades behind source locks.
Reclamation must not retain graph admission while blocking on busy cache
admission: workflow entry can already hold cache admission before requesting
shared graph admission. Preserve the cache preflight/release/recheck protocol
that prevents that cycle; there is no universal graph-before-cache order.

### 2. Log append, seal, and artifact-version coordination — high priority

Sources: [internal file routes](../../crates/api/src/routes/internal_files.rs),
[artifact repository](../../crates/common/src/repositories/artifact.rs),
[log stream repository](../../crates/common/src/repositories/log_stream.rs), and
[storage maintenance](../../crates/common/src/repositories/storage_maintenance.rs).

Mutation paths coordinate the parent artifact, version row, per-version stream
advisory key, stream row, and current authorization/evidence. In seal_log_stream,
the transaction remains open while object segments are read and hashed. The
shared-file path also inspects/hashes the file under its file-lock protocol.
Shared append has filesystem work within its protected transaction as well.

Decision: implement the existing single-writer simplification plan. Aim for a
short prepare/close-frontier transaction, storage verification without a database
transaction, and a short fresh final publication. Preserve authenticated final
frame/manifest checks, accepted sequence/identity, exact provider identity,
ownership, and execution-completion ordering.

One logical worker writer does not exclude retries, late uploads, cleanup,
cancellation, or overlapping process attempts. Neither a writer credential nor
upload-grant expiry proves that an in-flight PUT stopped. Moving hashing alone
without a cleanup pin and immutable input frontier is unsafe. File chmod alone
does not fence an existing writable descriptor.

The per-stream advisory lock plus stream FOR UPDATE is a candidate for removal
of one layer once every participant uses the same existing row and missing-row
creation is handled atomically. Parent artifact coordination is broader: separate
executions may write different versions of one artifact. Narrowing that protocol
requires auditing version allocation, parent projection, and deletion together.

### 3. Pack release filesystem cleanup — high priority when exercised

Source: [pack retention repository](../../crates/common/src/repositories/pack_retention.rs),
cleanup_unreferenced.

This starts a new transaction after release metadata deletion, takes LOCK TABLE
pack_release IN SHARE MODE, checks remaining digest/path references, and calls
remove_release_tree inside the transaction. The table lock blocks writes to all
release rows while the filesystem work runs. It is separate from the collector's
earlier graph transaction; do not attribute this I/O to that earlier transaction.

Decision: replace the table-wide fence with a durable per-content cleanup claim
honored by publishers/reusers, or an equivalent storage ownership protocol.
Commit the claim, delete outside the transaction, then finalize briefly. A simple
reference check followed by an unlocked unlink is unsafe: a publisher could reuse
the same digest/path in between. A per-content advisory lock held across the same
I/O narrows contention but still occupies a database session; treat it as an
intermediate option, not the final design.

A related, broader physical-I/O fence exists in PhysicalCleanup in
[maintenance admission](../../crates/common/src/repositories/maintenance_admission.rs).
[Expired-artifact cleanup](../../crates/supervisor/src/artifact_cleanup.rs) and
[legacy snapshot cleanup](../../crates/supervisor/src/main.rs) retain exclusive
graph admission while awaiting deletion. Their metadata deadlines do not bound
that physical wait. They need a durable per-resource deletion obligation before
the transaction can safely end; adding a client timeout alone would release a
fence while filesystem work might still run. Include these paths in the storage
transaction work, independently of pack-release cleanup.

### 4. Sensor/rule admission — high priority coupling to event ingestion

Sources: [sensor admission](../../crates/common/src/repositories/sensor_admission.rs),
[events](../../crates/api/src/routes/events.rs),
[pack routes](../../crates/api/src/routes/packs.rs), and
[sensor workload admission](../../crates/common/src/repositories/sensor_workload_admission.rs).

The sensor_rule_admission advisory key is global within a database rather than
per sensor. Rule, trigger, runtime, and pack mutation paths take it exclusively.
Sensor-fenced event creation, workload checks, and several runtime request paths
take it shared. An event handler reacquiring shared admission later in the same
transaction does not release or shorten its earlier acquisition.

Decision: shorten mutation transactions first. Preparse and validate immutable
candidate files before admission; retain current authority and membership checks
inside. Pack activation already uploads its archive and stages the active
projection before the main transaction; do not claim those steps still need to
be moved. Component loading and other filesystem reads inside activation deserve
separate measurement.

Then evaluate per-sensor admission or checked configuration/assignment revisions.
Cross-pack rules and runtime-wide changes must discover and lock the full affected
sensor set in a stable order. Preserve stale-fence rejection and placement
compatibility. Removing shared admission from events without an equivalent
current-membership fence would allow stale assignments to emit accepted events.

### 5. Runtime partition maintenance — high priority potential reach

Sources: [partition repository](../../crates/common/src/repositories/native_maintenance/partitions.rs),
[partition/summary functions](../../migrations/20261006000002_hourly_summaries.sql),
and the native_partition_expire override in the
[file evidence migration](../../migrations/20261015006005_file_evidence_retention.sql).

native_partition_ensure_day requests ACCESS EXCLUSIVE on the parent and default
partition before checking whether the requested day is already present. Repair
then moves bounded rows and validates/attaches the partition in that transaction.
Expiry acquires graph admission and ACCESS EXCLUSIVE on the parent before registry
and evidence checks. The event, execution-history, and audit parents are busy
platform-wide ingestion dependencies.

Decision: preserve actual DDL's required locks, but avoid entering that phase for
known absent/ineligible/no-op work where a safe preflight can establish it.
For already-present days, evaluate validation under weaker protection with a
locked authoritative fallback for repair. Preserve physical-layout and registry
validation; an unchecked registry row is not proof of a valid partition. The
reconciler already discovers a candidate day before calling ensure_day; this
audit does not establish that every cycle unnecessarily takes these strong locks.

Keep partitions ahead of ingestion, separate expensive default repair from routine
creation, and cap work per transaction. Bound strong-lock acquisition especially
tightly: an incompatible queued DDL request can create additional waiting. Do not
assume dropping the explicit LOCK removes the locks taken by ALTER/DROP itself.

### 6. Cache admission, DDL, and accounting — high priority for cache workloads

Sources: [cache repository](../../crates/common/src/repositories/cache.rs),
[statistics storage](../../crates/common/src/repositories/cache/storage.rs),
[generation partitioning](../../migrations/20261007000001_cache_generation_partitions.sql),
[live mode](../../migrations/20261011000002_cache_live_mode.sql), and the final
[physical-parent cleanup override](../../migrations/20261015006013_file_native_physical_parents.sql).

Every protect_transaction mode except Read takes the exclusive global cache
admission key, including Write and PinMutation. Namespace/generation locks and
global/owner/generation usage rows provide additional serialization. Global byte
accounting UPDATEs also lock the singleton usage row implicitly. Removing the
advisory gate alone would not remove that accounting contention.

Normal parent protection uses ACCESS SHARE or ROW EXCLUSIVE, both compatible
with normal DML. Attach and statistics refresh use SHARE UPDATE EXCLUSIVE. Actual
generation cleanup uses ACCESS EXCLUSIVE on cache_entry, so one drop can delay
unrelated generation reads/writes as well as contend on the graph gate.

Decision: separate quota reservation, namespace mutation, and DDL coordination.
Evaluate short atomic global reservations followed by per-namespace work, with
exact release/recovery and bounded outstanding reservations. A per-namespace
mutex alone cannot enforce deployment-wide caps. Batch accounting deltas within
the existing atomic unit where safe; do not make strict quotas eventually
consistent by accident. Keep parent lifetime protection and pin ordering until
DDL has an equivalent protocol.

Statistics refresh already has lock/statement budgets. Consider a cheap due-work
preflight before its parent lock; retain an authoritative due/revision recheck.
Its lock does not block ordinary DML and is a lower priority than generation DROP.

### 7. Workflow scheduling and publication — medium priority

Sources: [scheduler](../../crates/executor/src/scheduler.rs),
[native iteration](../../crates/executor/src/scheduler/native_iteration.rs),
[workflow repository](../../crates/common/src/repositories/workflow.rs), and
[runtime control](../../crates/common/src/repositories/execution_runtime_control.rs).

Per-workflow advisory locks serialize scheduling, retry, cancellation, and native
iteration. Some session locks span commit and awaited message publication or
checkpoint work. Workflow/execution rows and dispatch reservations also take row
locks. A slow broker can therefore delay the same workflow even after commit;
this does not directly lock every workflow.

Decision: use conditional transitions and unique dispatch reservations where
they can replace an entire read/lock/write critical section. To release session
ownership before broker I/O, first provide durable publication ownership, fencing,
ordering, and idempotent recovery, potentially using an outbox. An outbox still
needs an atomic claim; its benefit is bounding the database critical section.
Keep session-disposal safeguards on cancellation or unknown cleanup outcomes.
Do not replace cross-executor coordination with a process-local Tokio mutex.

### 8. Authorization, secret provenance, and immutable bindings — reduce work

Sources: [runtime authority](../../crates/common/src/repositories/runtime_authority.rs),
[runtime targets](../../crates/common/src/repositories/runtime_target.rs),
[identity](../../crates/common/src/repositories/identity.rs),
[worker delivery](../../crates/common/src/repositories/worker_delivery.rs), and
[IAM ownership](../../crates/common/src/repositories/iam_ownership.rs).

These use pack-owner advisory locks and many FOR SHARE/UPDATE fences on policies,
identities, approvals, execution owners, and origin/binding rows. Shared readers
coexist, but retaining those locks delays revocation/configuration writers.
They guarantee current permission and source state while publishing effects or
disclosing protected bytes.

Decision: deduplicate dependency traversal and repeated validation within the
owning transaction, tracking local mutations and proof deadlines. Perform pure
computation before acquiring fences where possible. Separate immutable payload
reads from mutable authority checks, and revalidate the latter at publication.
Do not cache authorization across requests or weaken revocation semantics for
performance. FOR KEY SHARE is suitable only when preserving identity/existence
is sufficient; active_release, frozen identity, policy, and approval changes
usually require stronger protection. The managed-release SQL trigger explicitly
uses FOR SHARE because KEY SHARE would allow a conflicting active-release update.

### 9. Queue, outbox, object cleanup, and execution claims — generally retain

Sources: [work queues](../../crates/common/src/repositories/work_queue.rs),
[workflow log outbox](../../crates/common/src/repositories/workflow_log_outbox.rs),
[object maintenance](../../crates/common/src/repositories/object_maintenance.rs),
[event emitter](../../crates/common/src/repositories/execution_event_emitter.rs),
and [execution dispatch](../../crates/common/src/repositories/execution.rs).

FOR UPDATE SKIP LOCKED selects independent available work without waiting on a
competing row claimant. Other row locks protect dispatch reservations and exact
execution transitions. Some claims are already conditional UPDATEs.

Decision: retain short claims, commit ownership before external work, and bound
batch sizes. Audit callers rather than replacing every FOR UPDATE. Prefer a
single guarded UPDATE RETURNING when one row and expected state fully express
the transition. It still takes an implicit row lock. Preserve recovery for expired
claims, duplicate delivery, and unknown commit outcomes. SKIP LOCKED cannot be
used indiscriminately for required foreground updates without a retry contract.

### 10. Remaining narrow and administrative lock families — lower priority

| Family and source | Scope and recommendation |
| --- | --- |
| [Event-origin capture](../../crates/common/src/repositories/event_origin.rs) | Per-event advisory coordination plus graph admission. Retain until exact event identity, legacy ID projection, and retry uniqueness are handled by an equivalent atomic path. Avoid repeated capture work. |
| [Native file setup](../../crates/common/src/repositories/file_native_setup.rs) | Per-generation advisory lock around original-binding creation. Unique insert/conflict handling may replace it if losing attempts cannot publish side effects. |
| [Log stream leases](../../crates/common/src/repositories/execution_log_stream_lease.rs) | Global admission lock for bounded live-stream counts, not held for the entire streaming connection. Narrow/replace count-and-insert only with strict global and per-identity reservation accounting. This lease is not a seal/cleanup pin. |
| [Pack mutation](../../crates/common/src/repositories/pack.rs) | Per-canonical-pack advisory lock, including filesystem/metadata coordination. Prestage immutable content and shorten final publication; preserve cross-process ordering. |
| [Pack registry indexes](../../crates/api/src/routes/packs.rs) | Global administrative mutation key. Low steady-state priority; per-index keys require preserving cross-index checks, including the guard against deleting the last managed index and reactivating static indexes. |
| [Dashboard default-home trigger](../../migrations/20250101000018_dashboards.sql) | Advisory lock per scope plus updates and a partial unique index. Retain unless conflict/retry behavior intentionally replaces automatic displacement of the previous default. Low event-path relevance. |
| [Platform catalog reconcile](../../crates/common/src/repositories/platform_catalog.rs) | Singleton row, then SHARE ROW EXCLUSIVE on five catalog tables only when a revision update is needed. Startup/upgrade work; a matching revision exits before the table locks. Conditional upserts/uniqueness could narrow future reconcile work, with ownership checks preserved. |
| [Supervisor leadership](../../crates/common/src/repositories/retention.rs) | Long-lived session advisory lock elects one maintainer. It does not directly block unrelated application SQL. Keep dedicated ownership and reliable disposal; changing it offers little foreground benefit. |
| [Summary refresh](../../crates/common/src/repositories/native_maintenance/summaries.rs) | Per-kind builder state writes and ACCESS SHARE parent protection. Source invalidations do not FK-lock builder state. Retain atomic refresh/acknowledgment and bounded transactions; do not replace append invalidations with one producer-contended counter without evidence. |
| Migration runner and offline encryption conversion | Migration claim and conversion table locks are intentional exclusive maintenance. Keep outside normal traffic; do not count one-time migration LOCK statements as steady-state acquisitions. |

## Hidden serialization after explicit locks are removed

The final [artifact retention trigger](../../migrations/20261015006010_defer_artifact_version_retention.sql)
projects ready-version metadata onto its parent artifact using UPDATE. It no
longer performs inline version reclamation. Parent projections, global cache
accounting, histories, notification triggers, and summary invalidations all add
work and may acquire implicit locks. Independent log versions can still meet on
their common parent even if their explicit advisory lock is removed.

Evaluate eliminating unchanged metadata writes with precise IS DISTINCT FROM
guards, coalescing projections whose freshness contract permits it, and separating
per-version progress from parent metadata. Preserve public notifications,
history/revision meaning, and terminal state boundaries. Do not remove real
integrity constraints merely to eliminate their implicit lock requests.

## Implementation sequence and acceptance evidence

1. **Attribute waits and bound transactions.** Extend existing graph/transaction
   tracing with fixed operation names, acquisition wait, actual hold duration,
   backend correlation, pool wait, storage/broker wait, and confirmed transaction
   outcome. Sample pg_stat_activity, pg_locks, pg_blocking_pids, transaction age,
   and CPU/WAL/storage latency during a spike. Acquisition timing alone is not
   lock hold timing. Include built-in relation/transaction-ID waits, not just
   advisory locks. Use scoped server deadlines and observe cancellation cleanup;
   client timeout is not proof of database rollback.
2. **Fix bounded retention queries and retry behavior.** Follow the existing query
   plan. Keep expensive discovery/planning outside exclusive admission where
   safe, then revalidate under it. Track oldest eligible backlog and durable scan
   progress so contention avoidance does not silently disable retention.
3. **Remove storage I/O from log and release transactions.** Implement the narrow
   lifecycle/cleanup protocols described above. Inject delayed object reads,
   uploads, shared-file writes, and filesystem deletion; demonstrate unrelated
   event requests proceed without inheriting those storage delays through locks.
4. **Narrow sensor and cache coordination.** Measure occupancy first; split global
   gates only with complete affected-resource discovery, strict quota accounting,
   stable lock order, and atomic activation semantics. Test independent sensors
   and namespaces concurrently, including cross-resource mutations.
5. **Simplify workflow and remaining row transitions.** Use conditional writes,
   constrained inserts, and durable publication ownership only where they replace
   a complete protocol. Deduplicate validation inside one transaction. Measure
   the surviving implicit parent/accounting row contention after each change.

Use the same mixed workload and database size for baseline and treatment. Record
event-ingestion p50/p95/p99/max, tick scheduled-to-persisted delay, transaction and
lock hold tails, pool occupancy, throughput, and maintenance progress. Set numeric
latency budgets from the baseline and product requirement before implementation;
this source audit does not invent a credible millisecond target.

Concurrency validation must include duplicate/reordered delivery, two claimants,
stale worker attempts, cancel/retry, revocation during publication, pin creation
versus GC, eligible and ineligible partition drops, quota races, lost connections,
unknown commits, and recovery after process termination. Use owned disposable
PostgreSQL and the repository's runtime-log storage harness where needed. Verify
correct outcomes as well as latency: no lost bytes, unauthorized disclosures,
double dispatch, early deletion, quota overshoot, or indefinitely deferred cleanup.

Ship small independently measurable changes. Keep fsync and durable execution
semantics intact. Do not add unbounded event concurrency or simply increase pool
sizes to hide blocked transactions; either can increase pressure on a stalled
database. An asynchronous application benefits from short, bounded database
critical sections, even when some locking remains necessary.

## Validation of this audit

Documentation-only validation: source anchors and related-plan links checked,
selected deployed-source comparisons recorded, and whitespace checked. No
application tests were needed because executable behavior was not changed.
Runtime contention rankings, replacement protocols, and performance improvements
remain to be validated by the implementation work above.
