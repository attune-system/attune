# Graph admission contention implementation plan

Status: implementation and review fixes on `fix/graph-admission-contention` in a
dedicated worktree. Final-state validation is underway; earlier focused passes do
not certify the revised state. No live deployment or live database changes authorized.
Updated: 2026-10-31.

The minimal patch covers focused changes from PRs 1 through 4. PR5 is gated
by the [object-log seal claim review](object-log-seal-claims.md), which found that
existing upload grants and reader leases cannot safely replace a dedicated seal
claim and durable upload-settlement ownership. PR6 remains measurement-dependent.
No per-pack locking, pool-size increase, or readiness-timeout increase is planned.

## Recommendation

Keep the schema-wide shared/exclusive graph protocol. First remove unnecessary
transaction lifetime, then replace broad reclamation with bounded, independently
committed work. Ship maintenance budgets together with progress tracking and a
starvation policy. Move log verification outside transactions only after its
reservation and recovery protocol is specified and tested.

Do not start with per-pack locks, larger pools, or longer log-readiness timers.
Those changes either leave the serialization problem intact or require a much
larger correctness redesign.

## Evidence and scope

Input: `advisory-lock-contention-design.md`, dated 2026-10-30, supplied from
`/home/david/Downloads/`. Its observed timings are development-instance evidence,
not measurements reproduced by this planning pass.

Source reviewed at local commit `1949b92a635055070b2adc0d60ed10be6ab25be5`.
The proposal references `0eda2d0cc7a092a0467e79d1ad913d446f0dd261`.
A local Git comparison shows that the reviewed database constructor, graph-lock
helpers, pack collector, notifier authority checks, log-sealing route, and
supervisor scheduling files are unchanged between those commits. The notifier
listener implementation does differ. Reconcile that implementation before making
listener changes; do not overwrite its newer lifecycle work.

| Finding from source | Implementation consequence |
| --- | --- |
| `PackRetentionRepository::collect` takes exclusive admission before receipts, accepted inputs, snapshots, cache epochs, admissions, policies, releases, and an exact release count | Split by reviewed correctness units, not by arbitrary line ranges. Optimize zero-row scans as well as deletions. |
| The pack collector commits before calling `cleanup_unreferenced` | Do not attribute remote storage latency to this graph transaction. |
| `cleanup_unreferenced` separately holds a `pack_release` table lock while removing local release trees | Track this as a distinct filesystem/table-lock problem. Do not remove the lock without a replacement publication/deletion fence. |
| Notifier `source_fence` and `notification_fence` rely on transaction drop on success and early returns | Add awaited completion while preserving fail-closed behavior and the existing delivery checks. |
| `Database::new` sets `application_name` to `attune:{schema}`; notifier constructs its working pool separately | Name both common and separate pools, plus dedicated listeners and other production constructors. Preserve connection settings and `search_path`. |
| `seal_log_stream` reads and hashes object segments or shared files inside its transaction | Requires a durable seal claim and immutable snapshot, not simply an earlier commit. |
| Durable scheduling and owned reclamation cancellation already exist | Reuse their contracts where applicable rather than building parallel infrastructure. |
| Supervisor logs object-retention errors without returning them into `retention_success` | Give graph cleanup explicit phase outcomes. A successful outer retention job must not conceal failed or indefinitely deferred graph cleanup. |

All implementation belongs in the upstream `attune/` application repository.
Deployment repository work follows only after a reviewed application release.

## Safety contract

Every change must preserve these properties:

- Admission precedes cache, pack, sponsor, policy, source, and target locks.
  Never upgrade shared admission after taking dependent locks.
- Capture and publication cannot race reclamation of their transitive evidence.
  Shared admission can protect writes, not just reads.
- Activation preflight and publication remain atomic against new admissions.
- Revocation, expiry, sponsor state, target permissions, and database-clock checks
  remain fresh. Retained evidence does not grant authority.
- Physical cleanup obligations survive crashes, late PUT completion, and unknown
  provider outcomes. Pins outlive those obligations.
- Candidate discovery is advisory. Destructive eligibility is rechecked after
  exclusive admission, using a fresh snapshot.

Repository SQL and trigger entry paths must obey the same protocol. Do not rename
or change the advisory key while implementing the first phases.

## Delivery sequence

### PR 1: establish attribution and a repeatable baseline

Primary files:

- `crates/common/src/db.rs`
- `crates/common/src/repositories/schema_type.rs`
- `crates/common/src/observability.rs`
- `crates/notifier/src/service.rs` and `postgres_listener.rs`
- Graph transaction owners in API, worker, notifier, and supervisor code

Work:

1. Pass explicit service/instance/purpose identity into connection construction.
   Keep names within PostgreSQL's 63-byte limit with a stable truncation scheme.
   Cover reconnects, listeners, audit pools, and notifier's four-connection pool.
2. Record admission request/acquisition and transaction completion separately.
   Measure wait time, hold time, transaction duration, and gaps between commands.
   Reentrant acquisitions must not reset the original hold start or count as new
   holders. Lock-helper spans alone cannot measure transaction-scoped release.
3. Record confirmed commit, confirmed rollback, and unknown outcome distinctly.
   Dropping a Rust guard is not proof that PostgreSQL released the lock. Correlate
   cancelled operations with connection disposal and backend observations.
4. Add per-phase cleanup results: candidate probes, considered rows, changed rows,
   deferrals, elapsed time, cursor progress, and bounded backlog/age estimates.
   Do not replace the expensive exact count with another exact graph-wide count.
5. Add a reproducible mixed workload with large retained graphs, pinned prefixes,
   zero-row sweeps, concurrent admissions, and log sealing. Capture identical
   workload/configuration and cold/warm runs for baseline and treatment.

Use fixed operation categories for metrics. Keep backend IDs and correlation IDs
in restricted traces, not metric labels. Exclude credentials, SQL bind values,
policy bodies, and file contents. Use the existing tracing path first; do not
make a new telemetry platform a prerequisite for the fixes.

Exit: identify the owner and stage of slow shared transactions and separate
exclusive queue wait from exclusive hold. Record instrumentation overhead.

### PR 2: close shared transactions promptly

Primary files:

- `crates/notifier/src/recorded_authority.rs`
- `crates/notifier/src/websocket_server.rs`
- `crates/common/src/repositories/runtime_authority.rs`
- Nested authority/configuration/schema callers identified by the audit

Work:

1. Structure notifier validation so one outer owner awaits transaction completion
   for success, denial, handled errors, and empty results. For validation-only
   scopes, explicitly roll back rather than silently committing incidental writes.
   If an audited path intentionally publishes bindings, preserve that contract
   with an explicit commit. Completion failure denies delivery.
2. Audit helpers called with admission held. Reuse `&mut PgConnection` or the
   caller's transaction instead of acquiring another pool connection. Preserve
   authorization and lock order; do not replace protected reads with pool reads.
3. Instrument immediately after the sponsor pack lookup, then dependency
   validation, approvals, target resolution, final clock checks, and pool return.
   Use evidence to distinguish pool waits, executor scheduling, CPU work, and
   unfinished rollback. Drop rollback alone does not explain the observed pause
   before the final clock query.
4. Retain cancellation as a recovery case, with owned shutdown and awaited
   cleanup. Review the existing `reclamation_cancellation` module before extending
   cancellation machinery; its current ownership rules are specialized.
5. Keep socket readiness before final authorization. Preserve the final fresh
   database-clock check and no-await handoff to `start_send`. Do not remove the
   later clock-check acquisition merely because it is a second acquisition; first
   distinguish sequential acquisition from nested acquisition with admission held.

Exit: deterministic queued-writer/nested-read tests complete without a second
admitted connection. Success, denial, stage errors, cancellation, and shutdown
leave no stranded shared holder. Existing delivery/revocation tests still pass.

### PR 3: make the demonstrated slow eligibility checks cheap

Primary files: `crates/common/src/repositories/pack_retention.rs`, relevant SQL
functions and indexes in `migrations/`, and collector performance fixtures.

Work:

1. Inspect non-executing plans for accepted-input and admission cleanup first.
   Include `file_evidence_pinned`, dependency functions, triggers, and cascades,
   not just the outer DELETE. Run executing plans only on owned disposable data.
2. Compare plans against representative table sizes and pin distributions. Add
   indexes only where those plans justify them. Consider separating disjunctive
   reference probes when equivalence and measured plans support it.
3. Introduce bounded raw candidate probes. `LIMIT` after all eligibility predicates
   limits successful candidates, not the amount of work needed to find them.
4. Move release reporting counts outside exclusive admission. Document their
   observation time and whether they are exact or bounded estimates.

Exit: repeated zero-row sweeps have measured bounded cost on the benchmark.
Eligibility, dangling-ID retention semantics, and all existing pin tests remain
unchanged. This PR should not yet change the global locking protocol.

### PR 4: incremental reclamation with bounded, fair scheduling

Primary files:

- `crates/common/src/repositories/pack_retention.rs`
- `crates/common/src/repositories/file_evidence_retention.rs`
- `crates/common/src/repositories/native_maintenance/schedule.rs`
- `crates/supervisor/src/object_retention.rs` and `main.rs`
- Maintenance configuration and migrations if durable progress requires them

Review and record the correctness units before coding. Starting candidates are
receipt leaves, accepted inputs, native snapshots, historical cache epochs,
admission leaves, dormant policy revisions, and pack releases. Validate trigger
cascades and closed-set requirements for each. A dependency cycle or whole-owner
cleanup set stays atomic; an oversized set is deferred with diagnostics rather
than partially deleted.

For each unit:

1. Discover a small raw page outside exclusive admission, using stable ordering.
2. Start a fresh READ COMMITTED transaction and attempt exclusive admission before
   any dependent lock. Use a nonblocking attempt for ordinary maintenance.
3. In subsequent statements, recheck exact identity, current references, pins,
   active state, retention cutoff, and newest-inactive protection for that page.
   Do not carry a pre-admission REPEATABLE READ snapshot into deletion.
4. Delete or record a durable claim, then commit. Persist safe cursor progress
   atomically with the result. Release admission and yield before another page.
5. Return a structured outcome that distinguishes progress, no work, lock
   deferral, deadline deferral, failure, and unknown commit outcome. Preserve
   already committed progress if a later unit fails.

Use a wrapping cursor per phase and rotate phases so an ineligible prefix cannot
hide later work. Revisit skipped/previously pinned rows and newly eligible older
IDs. Persist next-attempt times and deferral age across leader restart. Keep
progress tracking behind the collector's interface, not spread across callers.

Bound acquisition, each protected statement, the page's wall-clock work, and the
cycle separately. Apply transaction-local timeouts. Recompute remaining time
before statements; a per-statement timeout alone does not bound an entire page.
After timeout, await rollback or dispose the owned connection before retrying in
a fresh transaction. Unknown commit outcomes require idempotent reconciliation,
not reporting a rollback or repeating physical cleanup blindly.

Fairness ships in the same change:

- Keep one graph-reclamation attempt active under the existing supervisor leader.
- Defer with jitter/backoff, not an immediate retry loop.
- Track last actual progress and oldest deferral separately from last successful
  outer supervisor cycle. Propagate cleanup failures and deferrals into status.
- After an agreed maximum deferral age, allow a scheduled, short bounded queued
  admission attempt. Its queueing cost must fit the foreground latency budget.
  Pure try-lock polling cannot guarantee maintenance progress under steady load.
- If controlled opportunities still cannot make progress, alert and use an
  approved operator maintenance window. Do not silently widen wait budgets.
- Preserve independent retention/native cadences. Calculate retry deadlines from
  completion/current time so long or failed cycles cannot leave deadlines overdue
  and cause continuous immediate attempts.

Exit: bounded hold/queue tails and sustained cleanup progress under foreground
load. Restart, phase failure, cursor wrap, pinned prefixes, empty passes, and
oversized atomic sets do not lose work or conceal backlog.

### PR 5: move object-log verification outside graph transactions

Primary files: `crates/api/src/routes/internal_files.rs`, log-stream and
artifact repositories, file-evidence retention, and claim migrations.

Start with object-segment sealing. Specify a durable state machine before
implementation:

- Prepare under admission: authorize, create a fenced seal claim, pin exact
  evidence and provider versions, and freeze a segment manifest or stream
  generation. Resolve active uploads and prevent concurrent append/publication
  from invalidating the selected bytes.
- Verify outside admission: stream/hash/verify exactly that immutable manifest.
- Finalize under fresh admission: revalidate authority, claim token/generation,
  stream state, byte identity, and upload settlement. Atomically publish readiness
  and completion evidence. Never reuse the earlier positive authorization.
- Recover: retries are idempotent; stale claim owners cannot finalize. Expired
  claims do not release pins while uploads or physical obligations remain.

Test crashes at every transition, concurrent append/seal, revoked or expired
authority, late PUTs, provider-version mismatch, slow storage, and lost responses.
Update readiness/recovery queries to understand claims before enabling producers.

Treat shared-file sealing as a separate follow-up. It needs an immutable snapshot
or compatible append/flock fence; recording a path or digest is not sufficient.
Likewise, local pack-tree deletion needs its own publication/deletion fence before
removing the current table lock.

Exit: storage delay no longer extends the redesigned object-seal graph transaction,
and crash/revocation tests prove that evidence and readiness remain correct.

### PR 6: optional immutable-definition preparation

Only proceed if traces still show material parsing, verification, or dependency
traversal under admission. Cache immutable parsed definitions by revision plus
authenticated content digest, with bounded memory. Revalidate authoritative
references under admission. Never cache live sponsor, revocation, approval,
target, or expiry decisions. Benchmark cold and warm paths separately.

## Local query-plan evidence

On 2026-10-30, both demonstrated zero-row query shapes were compared with the
baseline SQL at `0eda2d0c` on owned PostgreSQL 18. The fixture had 10,000 accepted
inputs, 10,000 admissions, and file-evidence pins retaining every row. Both
variants used the same schema and indexes and a delete limit of 100. The new
queries used a raw candidate page of 256. Executing plans ran only on this
disposable fixture, inside rolled-back transactions holding exclusive admission.

| Query | Baseline eligibility probes | New eligibility probes | Baseline warm execution | New warm execution |
| --- | ---: | ---: | ---: | ---: |
| Accepted inputs | 10,000 | 256 | 508 ms | 31 ms |
| Admissions | 10,000 | 256 | 627 ms | 20 ms |

The plans show primary-key index probes for the bounded candidates. No new index
was needed for this fixture. These are individual synthetic warm observations,
not p99 measurements or a production latency claim. They do not model large
transitive evidence closures, concurrent storage work, or client scheduling
pauses. Full local plans are in
`/tmp/opencode/graph-admission-query-plans.json`, outside the source tree.

## Implementation validation record

### Review fixes in progress

The independent intent and consistency reviews identified unbounded graph-entry
paths around pack collection, insufficient pinned-prefix scan capacity, unowned
notifier cancellation, and missing shared-holder lifetime attribution. The revised
implementation:

- Bounds evidence audit, recovery, and deletion metadata transactions rather than
  allowing an empty follow-up sweep to queue an unrestricted exclusive waiter.
- Runs graph pages round-robin within the existing cycle budget and persists
  one-second continuations separately from one-hour clean rescans.
- Gives graph/object maintenance its own `object_retention` schedule row, failure
  age, and success record. Pending graph work does not rerun hourly row retention.
- Disposes cancelled notifier sockets under explicit ownership and awaits normal
  rollback and pool return, with blocked-holder and shutdown regressions.
- Attributes the first shared acquisition through confirmed transaction end on
  the two suspect API owners, preserving reentry timestamps and marking unknown
  outcomes rather than treating guard destruction as database completion.

Current main added an artifact-upload-grant index with migration version
`20261030000001`. This branch fast-forwarded that existing commit and uses
`20261030000002_graph_reclamation_progress.sql`; no applied checksums were edited.

New regressions cover whole object-cycle deferral while the shared blocker stays
open, production-sized pinned prefixes with independent supervisor continuation,
lost COMMIT acknowledgement, release cutoff/tie ordering, and cancellation before
the identity blocker is released. Final runs are required before publication.

A second review found a duplicate unbounded abandoned-upload loop in hourly
housekeeping, a deadline spent during provider I/O, and a retry identity lost
between version deletion and artifact refresh. The duplicate loop is removed;
metadata acknowledgements get fresh bounded windows; and version deletion plus
artifact reconciliation now share one protected transaction. A real-time
11-second successful-provider regression failed before the deadline fix and
passes afterward, including later deletion progress. The rebuilt notifier and
supervisor suites pass (87 and 47 tests respectively; the existing notifier load
profile remains explicitly ignored). Final complete validation is still required.

The production audit caller also reserves bounded rollback/progress-bookkeeping
time after a whole-page deadline. Its regression failed before that reservation
was restored, then passed with the slow owner intact and its cheap follower
visited. This reserve does not extend owner-mutation work past its page deadline.

### Initial focused evidence

The implementation adds service/purpose/instance connection names, admission and
transaction traces, awaited notifier rollback, same-connection ad-hoc delegation,
bounded graph collection with durable retry/progress, and supervisor failure and
deferral reporting. Queue-acquisition timeouts do not leak into dependent work.
Deadline cleanup drains pending PostgreSQL cancellation responses before awaiting
ROLLBACK. No pool capacities, authorization ceilings, or global lock keys changed.

Passing focused runs recorded during this implementation:

| Coverage | Passed |
| --- | ---: |
| Common connection names, schedule, and bounded collector | 30 |
| Notifier library and binary | 83 |
| Supervisor | 45 |
| Execution-token permissions and delegation | 6 |
| Database connection identity replacement | 1 |
| Asynchronous release pins | 14 |
| File-evidence retention | 9 |
| Artifact retention graph order | 3 |
| Platform catalog boundaries | 3 |
| Four existing authority-retention cases invoking the collector | 4 |

The existing notifier authorization-load profile remains explicitly ignored. It
is not passing performance evidence. Final formatting, workspace all-target
checking, all-target/all-feature Clippy with warnings denied, fresh owned-database
migrations, and SQLx preparation passed. SQLx preparation reported no static
queries to cache. Logs remain under `/tmp/opencode/graph-admission-*`.

### Wider validation history

A broader 269-test runtime-authority executable did not complete within the
40-minute run budget and showed native-cache timing failures. A bounded rerun of
six checkpoint/completion cases passed three and failed three at
`runtime_authority_repository_tests/native_cache_checkpoint.rs:312` with
`source preparation exceeded unchanged five-second bound`. An earlier isolated
dynamic-selector rerun passed. These results do not establish whether the timing
problem predates this patch.

The failing path is cache completion preparation and native closure capture, not
the changed collector or maintenance scheduler. Diagnostics observed an idle
transaction waiting on `ClientRead`, without blocking PIDs. That does not identify
the client-side cause. Profile this path before claiming that all long shared
holders are fixed or implementing the optional immutable-data cache. The local
repro log is `/tmp/opencode/graph-native-check.log`.

On 2026-10-31, the unchanged executable passed the exact six cases twice on a new
owned PostgreSQL instance, then passed the 14-case checkpoint and 24-case native
cache groups. These runs did not justify a source fix or timeout increase. They
also do not validate the revised source: the final rebuilt suite must pass. Logs:
`/tmp/opencode/native-cache-{red1,red2,wide1,wide2}.log`.

The focused successes do not replace full CI. Production-sized mixed-load
objectives, a dev canary, and deployment acceptance remain separate gates. No
commit, push, release, or deployment has been performed at this stage.

## Acceptance and validation

Agree numeric objectives after PR 1 and before enabling behavior changes. Suggested
initial laboratory targets, not deployment defaults or established SLOs:

| Measure | Proposed starting target |
| --- | --- |
| Protected GC page hold | p99 below 250 ms; investigate any page exceeding 1 s |
| Foreground admission wait with GC active | p99 below 100 ms and no regression against the agreed foreground baseline |
| Maintenance starvation | bounded progress within an agreed deferral interval; alert before the retention/backlog limit is breached |
| Reclamation capacity | drain a fixed eligible backlog and outpace eligible arrivals under the agreed sustained workload |
| Cancellation cleanup | confirmed transaction end or backend disposal within an agreed diagnostic deadline |
| Safety | zero missing pins/evidence, unauthorized publications, or permanently skipped candidates |

If realistic graph sizes miss these targets, optimize or revise the design before
rollout. Do not satisfy a latency goal by suppressing cleanup or reducing coverage.

Extend existing owning tests rather than creating a parallel test framework:

- `file_evidence_retention_tests`: capture/GC in both orderings, no late upgrade,
  stale discovery snapshots, physical obligations, and atomic closed sets.
- `runtime_authority_repository_tests`: bounded proof collection, oldest pinned
  rows, new pins between discovery and deletion, receipt/admission dependencies,
  policy activation, and fresh authorization.
- `async_release_pin_tests` and `artifact_retention_graph_order_tests`: release
  publication/capture races, dependent lock order, and eventual release cleanup.
- Notifier authority/delivery tests: queued exclusive waiter, all validation exits,
  pool saturation/return, expiry after readiness, cancellation, and shutdown.
- Supervisor/schedule tests: completion-time deadlines, per-phase outcomes,
  persisted cursor wrap, leader restart, fairness, and no-progress backoff.
- Existing runtime-log correctness suite: slow/versioned object storage, seal
  claims, crash recovery, and revoked-authority completion.

Order races with barriers or observed PostgreSQL waiters, not fixed sleeps. Use
owned disposable PostgreSQL and storage with awaited teardown. Follow
`docs/testing/running-tests.md`, including four or more database test threads and
unique run IDs. Each code PR needs targeted tests, workspace formatting/checks,
and all applicable `.github/workflows/` validations on its exact final state.
Run `cargo sqlx prepare` after schema changes.

## Rollout and stop conditions

1. Land observability first, then collect a baseline in an approved dev canary.
2. Roll out one behavior change at a time through the existing reviewed deployment
   process. Observe representative traffic, at least several cleanup cycles, and
   restart/recovery, not just one fast request.
3. Retain per-phase wait/hold histograms, phase outcomes, backlog/age, log readiness,
   notifier delivery, and cleanup-capacity evidence for each treatment.
4. Stop on safety failures, unknown transaction outcomes that cannot reconcile,
   worse foreground tails, stranded transactions, or rising cleanup age without
   a recovery path. A fast foreground path with starved GC is not acceptance.
5. Define upgrade/rollback separately for indexes, durable progress, and seal
   claims. Pre-1.0 migration policy permits an explicitly approved development
   reset, not an automatic reset. Do not assume an image-only rollback understands
   new durable claim states or pins.

Only consider replacing the global gate if PRs 1 through 5 still miss agreed
objectives. That requires a separate dependency-domain design covering transitive
cross-pack references, trigger callers, global mutations, recovery, and a safe
transition for all callers. It is not part of this implementation sequence.

## Immediate next work

Profile the reproducible native-cache preparation timing failures and resolve the
wider validation blocker. Then run full required CI and agree numeric latency and
cleanup-capacity objectives before an approved dev canary. Storage claim work and
immutable-definition caching remain separate, evidence-gated follow-ups. Do not
replace the global gate as part of this patch.
