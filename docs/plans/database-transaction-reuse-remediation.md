# Database transaction reuse remediation

Date: 2026-11-06
Status: work in progress; implementation preserved and extended, remote validation underway

Attune must prevent cancelled transaction startup from leaking transaction
ownership and independently reject dirty or uncertain sessions at pool reuse.
These are correctness requirements: an inherited transaction can spread errors
across requests or acknowledge a write without committing it.

Implement driver prevention and pool containment first, then application
transaction and session-lock ownership, audit resilience, alert payloads, and
bounded maintenance discovery. Keep these changes separately reviewable.

## Evidence and assessment

The source review used revision
`880b1b2a138a335eb549c08452f0d3440de68d41` and the incident report at
`.scratch/database-transaction-reuse-analysis-2026-11-06.md`. The report is local
investigation material, not an enduring documentation dependency.

| Finding | Assessment and consequence |
| --- | --- |
| Cancellation before BEGIN acknowledgement | Confirmed source-level gap in vendored SQLx: its rollback guard depends on transaction depth, incremented only after acknowledgement. |
| Dirty session reuse | Confirmed: Sync-based ping does not require the server transaction status to be idle. |
| Analytics isolation error | Consistent with inherited transaction state; preserve REPEATABLE READ and source-lock ordering. |
| Manual transaction wrappers | Confirmed in execution repository methods and three additional production scheduler paths. |
| Audit loss on insert failure | Confirmed: the writer drains its batch before insertion succeeds; flush acknowledgement also succeeds after failed insertion. |
| Alert optional fields | Confirmed serialization/schema mismatch: absent fields become null, while the flat schema declares optional typed values. |
| Retention scans and lock timeouts | Separate maintenance findings; current evidence does not establish a particular index fix or connect retention causally to the leak. |

The original leaking request and any live data loss remain unproven. The
companion reproducer archive was absent at the report's referenced location
during review; its reported characterization results are not independently
verified product-fix evidence. Recheck current source before implementation.

## Required invariants

- Once transaction startup is queued or sent, cancellation/error cannot permit
  reuse without confirmed cleanup or disposal. Cover ordinary and custom BEGIN.
- After draining pending protocol responses, pool reuse requires PostgreSQL
  wire-level Idle, no outstanding tracked transaction, no unresolved ownership,
  and no pending protocol work. Client depth and successful ping alone do not
  establish this invariant.
- A nested startup failure must not roll back an unrelated caller-owned
  transaction. Handle SAVEPOINT startup and cleanup separately from top-level
  transaction ownership.
- Successful later writes commit at their intended boundary and cannot inherit
  or accidentally commit earlier borrowers' work.
- Session advisory locks need their own ownership and cleanup guarantees. Idle
  transaction status does not prove that session locks were released.
- Preserve authorization, graph/cache admission and lock ordering, analytics
  snapshot semantics, flat schemas, independent retention, search_path, TLS,
  connection attribution, and existing cancellation capabilities.
- Do not substitute larger pools, longer global timeouts, blanket retries,
  SELECT 1 checks, or broad RESET ALL for ownership correctness. Never blindly
  retry non-idempotent writes or ambiguous commits.

## Phase 1 Deterministic regression coverage

Recover the portable harness or recreate its loopback PostgreSQL protocol relay
using the repository's owned test infrastructure. Hold BEGIN's ReadyForQuery
after server acceptance, cancel the future, then release the response. Use
readiness signals and diagnostic deadlines, not timing sleeps.

Cover borrowed connections and pool-owned transactions, ordinary/custom BEGIN,
failed startup, nested SAVEPOINT creation/failure, cancellation after BEGIN,
during statements, during commit/rollback, and during pool cleanup. Add controls
for acknowledged rollback and normal clean reuse.

Convert characterization tests that assert the defect into clean-reuse or
quarantine requirements. Check actual server state independently of SQLx's
transaction-depth counter and verify committed visibility through an observer.
Use one-connection pools where needed to prove the reuse boundary.

Exit: deterministic baseline failures demonstrate the target defect; tests
identify whether subsequent use is on the cleaned backend or a replacement.

## Phase 2 Driver prevention and pool containment

Primary files:

- `vendor/sqlx-postgres-0.8.6/src/transaction.rs`
- `vendor/sqlx-postgres-0.8.6/src/connection/mod.rs`
- `vendor/sqlx-core-0.8.6/src/connection.rs`
- `vendor/sqlx-core-0.8.6/src/pool/connection.rs`
- `vendor/sqlx-core-0.8.6/src/pool/inner.rs`
- `crates/common/src/db.rs`

Track startup ownership and uncertainty across suspension points. Review queue
failure, partial writes, failed BEGIN, nested startup, drop ordering, and
interrupted cleanup; moving the depth increment alone is insufficient.

Prefer a narrow driver capability for pool reuse safety, enforced by the shared
SQLx pool lifecycle. PostgreSQL must use actual ReadyForQuery status and protocol
state. Review release, acquisition, fresh connection initialization, and hooks.
The existing after_release hook runs before SQLx's final ping drains responses:
checking cached status there alone is unsafe. Hooks themselves can issue SQL,
so the final check must account for their effects.

Keep ordinary ping valid inside legitimate transactions. Avoid duplicate healthy
round trips where existing protocol responses establish readiness. Bound cleanup;
discard uncertain sessions rather than silently sanitizing an unknown boundary.
Verify raw/manual healthy-looking open transactions and aborted transactions are
contained independently of the startup fix.

Inventory all pool constructors, including common service pools, notifier
working/listener pools, audit pools, and utilities. Ensure default or separately
constructed pools cannot bypass containment. Dedicated connection owners still
need explicit lifecycle guarantees. Preserve other drivers' supported behavior
if changing a shared Connection capability.

Update both vendored ATTUNE-PATCH.md provenance notes as applicable, documenting
the local correctness changes without implying upstream acceptance.

Exit: both prevention and containment pass independently; healthy reuse remains
possible, and later writes are committed independently of prior borrowers.

## Phase 3 Application transaction and session-lock ownership

Replace manual BEGIN/COMMIT/ROLLBACK in
`ExecutionRepository::create_workflow_task_if_absent` and
`create_workflow_task_if_absent_pinned` with tracked transaction ownership and
awaited normal completion.

Audit the production scheduler methods `checkpoint_initial_native_wait`,
`checkpoint_native_cache`, and `checkpoint_native_wait` together with their
session advisory locks. Their trailing cleanup can be bypassed by cancellation.
Install safe lease ownership before acquisition, including cancellation after
server lock acquisition but before acknowledgement. Return a session only after
confirmed transaction completion and unlock; otherwise dispose of it.

Preserve session-lock protection across post-commit publication. Replacing a
session lock with a transaction-scoped lock would shorten that protection and
requires a separate correctness design. A tracked transaction alone does not
solve the post-commit session-lock hazard. Keep raw DB access within owning
repositories; put reusable lifecycle support at the appropriate shared boundary.

Inventory remaining manual transaction SQL and timeout, task-abort, shutdown,
and request-cancellation surfaces. Preserve caller-owned connections and nested
transaction behavior in the analytics reader. Reuse existing uncertain-session
disposal patterns where appropriate, after checking their actual current code.

Exit: independent observation confirms no leaked transactions or workflow locks
after cancellation, including during lock acquisition and post-commit publication.
Preserve existing analytics nested-caller, lock-failure, and cancellation tests.

## Phase 4 Audit durability and diagnostics

In `crates/common/src/audit/writer.rs` and its emitter/lifecycle owners:

1. Retain batches until confirmed persistence or an explicit bounded loss policy.
2. Bound queueing and retries; define overload behavior without accidentally
   turning non-blocking audit emission into unbounded request stalls or memory.
3. Make flush acknowledgement distinguish persistence from failure/loss.
4. Use stable identity and deduplication before retrying ambiguous outcomes.
   Account for audit_event's partition key when designing uniqueness enforcement.
5. Retain and join the writer during orderly shutdown; bound draining and report
   residual loss rather than assuming a detached task has completed.
6. Evaluate a separately attributed, bounded audit pool within the total
   connection budget. It reduces shared-pool interference but is not independent
   storage or a substitute for session correctness.

Add sanitized lifecycle diagnostics and counters for uncertain startup/cleanup,
dirty-session disposal, SQLSTATE category, retries, and audit loss. Restrict
request/backend identifiers to traces rather than unbounded metric labels.
Never log bind values, payloads, credentials, cancellation keys, or secret data.

Exit: test definitive failure, ambiguous completion, duplicate prevention, retry
exhaustion, overload, flush semantics, shutdown draining, and exact loss accounting.

## Phase 5 Alert payload and retention follow-ups

Omit absent optional fields in `SystemAlert`; review every optional field against
`packs/core/triggers/alert.yaml` and the platform catalog's canonical schema.
Exercise absent/present values through
`EventSchemaRepository::create_internal`, not serialization-only tests. Keep
the canonical flat schema and validate delivery of maintenance deferral alerts.

For `StorageMaintenanceRepository::body_candidates`, collect representative query
plans and bounded workload measurements on owned infrastructure. LIMIT bounds
returned rows, not eligibility-filter scan work. Evaluate cursor-based candidate
windows, per-cycle discovery budgets, and evidence-supported indexes. Preserve
final eligibility, pin, identity, and transaction revalidation before deletion.
Prove eventual eligible-object discovery without starvation; do not merely move
unbounded work into another query or retry loop.

Coordinate retention/lock-timeout work with
[graph admission contention](graph-admission-contention.md) and
[supervisor maintenance contracts](../deployment/supervisor.md). Keep maintenance
performance work separate from the transaction-correctness acceptance gate.

Exit: actual alert-schema validation succeeds, candidate discovery has measured
bounded work, and cleanup safety and eventual progress remain intact.

## Validation and delivery gates

Follow [Running Tests](../testing/running-tests.md) before choosing runners or
provisioning dependencies. Use owned disposable PostgreSQL 16 and 18, existing
fixtures/runners, unique direct-Cargo run IDs, and at least four DB-test threads.
Join relay/background tasks and verify owned resource cleanup explicitly.

Required coverage includes deterministic driver/pool regressions, existing
analytics regressions, repository/API cross-request cancellation tests,
scheduler session-lock cancellation tests, audit failure/lifecycle tests, actual
alert schema-path tests, and representative maintenance correctness/work budgets.

Run formatting, `cargo check --all-targets --workspace`, relevant Clippy checks,
and affected integration suites, then broaden according to the shared-driver
change and test results. Maintain zero new warnings. Run SQLx preparation when
schema/query metadata changes require it. Generate and check the web client if
API contracts change. Required CI must pass before merge readiness.

Keep implementation changes separately reviewable by phase. Follow current task
authorization for commits, pushes, and PRs. Implementation does not authorize
live mutation, database resets, merging, releasing, or deployment.

An eventual separately authorized rollout must verify clean session reuse under
cancellation, independent write commitment, no cross-request aborted state, no
stranded session locks, and visible audit/maintenance diagnostics.

Handoff using the repository format:

```text
Status: work in progress / ready to merge
Validation: commands/results, tested revision and uncommitted changes, scope
Gaps: remaining implementation, unavailable evidence, unrun/failed checks, CI
```

## Replacement implementation checkpoint (2026-11-06)

The coordinator remains local. All implementation edits, compilation, test
execution, and disposable services run over SSH on `192.168.1.38`, in
`/home/david/attune-transaction-remote-5VcMrDI8/source`. The remote directory is a
source snapshot without Git metadata. Source is synchronized back to the assigned
review worktree, excluding build outputs and dependency/service state.

Preserved implementation includes startup cleanup independent of depth, nested
completion quarantine, final driver pool checks after hooks and draining,
tracked execution repository transactions, and session leases in all three
native scheduler checkpoint paths. Leases dispose on cancellation before lock
acknowledgement and span post-commit publication. Audit batches reserve stable
partitioned identities, retry within bounded deadlines, expose failure-aware
flush/loss counters, and drain/join at API shutdown. Optional alert fields are
omitted and tested through internal event-schema admission. Body discovery
reserves bounded primary-key windows with persistent fixed-high-water sweeps;
existing claim/delete transactions retain final eligibility and identity checks.
No new performance index is justified or claimed.

Additional regressions cover shutdown with live emitter clones, exact queue loss
on abort, and revisiting newly eligible older rows despite continuous inserts.
The standalone driver suite and prior emitter passes were reported by the stopped
implementation agent; their historical logs are unavailable in this snapshot,
so they are not independently verified evidence. New remote run results are
recorded separately. Shared lease tests alone do not certify cancellation through
each real scheduler checkpoint; representative query-plan/workload evidence and
full affected-consumer results remain required until explicitly recorded.
