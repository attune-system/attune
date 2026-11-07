# Database transaction reuse remediation

Date: 2026-11-06; updated 2026-11-07
Status: main integrated; affected validation complete; required final-head CI/review pending

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

## Main integration and validation (2026-11-07)

Status: work in progress until required CI/review on the final main-integrated head pass.

The PR targets **main**. A normal merge integrated main
`d9da94122ff6fab725c212ac81a6a965a3bd2dbd` into the existing branch at
`59912e63c17229fcc2f582ecdb1315d7f948f78e`, preserving main's version **0.10.3**.
The four MCP conflicts were resolved by retaining main's complete latest files;
their contents match main exactly. This preserves the tool-list policy refactor.
No force push, additional PR, merge into main, deployment or live mutation was used.

Semantic review retained main's execution worker-transition guards, ad hoc/queue
admission, retry recovery, supervisor cancellation-safe leader session, log
wakeups/worker lifecycle changes and runner retention profiles. The remediation
still uses tracked execution transactions, leases spanning scheduler lock
acknowledgement through post-commit publication/unlock, and independent vendored
driver clean-session checks. It retains owning-frame prospective CONTROL
witnesses, bounded/deduplicated audit retries and failure-aware shutdown/flush,
schema-admitted optional alerts and bounded persistent body discovery.

All substantive source edits, compilation, tests and owned Docker services ran over SSH on
192.168.1.38 in `/home/david/attune-transaction-remote-5VcMrDI8/source`.
The coordinator stayed local. Local actions were Git integration/publication,
source synchronization and evidence reading/recording in
`/home/david/.t3/worktrees/attune/t3-cd751dc2`, branch `t3/cd751dc2`.
The remote source is a plain snapshot, not a manufactured Git checkout.

The validated source is merge 59912e63 plus the separately tested fixture-only
correction (local commit 8e64442b); per-command hashes identify exact contents.
Later documentation edits do not invalidate those code results.

The former head's 14 green checks used the old base and do not certify this
merge. The results below are fresh merged-source runs, with unique identities,
source SHA256 manifests and unchanged binary hashes. Test executables and
compilation ran sequentially with modest Cargo jobs and four DB-test threads.
Owned native database networks avoided published-port latency without changing
receipt consent windows or watchdogs.

`npm run generate:api` and `npm run check:api` passed remotely. Exported
OpenAPI and generated client versions match workspace 0.10.3; no artificial
version bump was made. Seven actual MCP policy smoke tests passed.
The explicitly enabled HTTP broker suite exposed a fixture fault: a closed
connection is recoverable, so its old fault injection could succeed. The test
now uses a separate owned empty vhost with no declared execution exchange and still requires an HTTP server
error and zero durable reschedule marks for both caller types. The failed diagnostics ne480e1db3c2f4d5 (recoverable closure) and
n73ca1f2966d0492 (configuration exchange ignored by envelope routing) are
retained as failures, never counted as passes.

### Fresh integrated results

| Suite / invocation | Result | Run ID |
| --- | --- | --- |
| integrated-cli-policy | 7 passed; 0 failed; 0 ignored | z6e365d41c9f14d8 |
| integrated-driver | 19 passed; 0 failed; 0 ignored | za8d3a86f2d224b4 |
| integrated-pg16-analytics_repository_tests | 12 passed; 0 failed; 0 ignored | nf9d406282a4b48f |
| integrated-pg16-audit_writer_resilience_tests | 5 passed; 0 failed; 0 ignored | n6562e47a4dca451 |
| integrated-pg16-broker_cache_ | 6 passed; 0 failed; 0 ignored | ncd1f7428730c42d |
| integrated-pg16-cache_generation_partition_tests | 14 passed; 0 failed; 0 ignored | nd4b57f05246f469 |
| integrated-pg16-checkpoint_cancellation | 3 passed; 0 failed; 0 ignored | n10171d28a3514ab |
| integrated-pg16-dashboard_acceptance_tests | 19 passed; 0 failed; 1 ignored | na1ffdf2b359347f |
| integrated-pg16-event_schema_binding_tests | 4 passed; 0 failed; 0 ignored | n4f96c2e0ce504db |
| integrated-pg16-execution_repository_tests | 27 passed; 0 failed; 0 ignored | n8bac32216a24426 |
| integrated-pg16-executor-default | 313 passed; 0 failed; 16 ignored | n7771d5c791ee418 |
| integrated-pg16-runtime_authority_repository_tests | 271 passed; 0 failed; 0 ignored | n6833d431811442f |
| integrated-pg16-scheduler | 6 passed; 0 failed; 0 ignored | na57739e0de054c3 |
| integrated-pg16-service | 1 passed; 0 failed; 0 ignored | n6246939b6be642b |
| integrated-pg16-session_lease_tests | 3 passed; 0 failed; 0 ignored | n781171eac0ab4cf |
| integrated-pg16-storage_maintenance_repository_tests | 30 passed; 0 failed; 0 ignored | n5c53ecbc3cfb4a1 |
| integrated-pg16-supervisor | 58 passed; 0 failed; 0 ignored | nb1d4bf9bff8a464 |
| integrated-pg16-work_queue_repository_tests | 11 passed; 0 failed; 0 ignored | ncf591decb6fd42b |
| integrated-pg16-worker_execution_transition_tests | 2 passed; 0 failed; 0 ignored | n57a3d00b30d1442 |
| integrated-pg18-analytics_repository_tests | 12 passed; 0 failed; 0 ignored | n5a08b28d15c74c4 |
| integrated-pg18-audit_writer_resilience_tests | 5 passed; 0 failed; 0 ignored | nc48183351803495 |
| integrated-pg18-broker_cache_ | 6 passed; 0 failed; 0 ignored | nf605d29d9ecc466 |
| integrated-pg18-cache_generation_partition_tests | 14 passed; 0 failed; 0 ignored | nb4049d3b383147d |
| integrated-pg18-checkpoint_cancellation | 3 passed; 0 failed; 0 ignored | n6d28703a6d75424 |
| integrated-pg18-dashboard_acceptance_tests | 19 passed; 0 failed; 1 ignored | nabce644ca816466 |
| integrated-pg18-event_schema_binding_tests | 4 passed; 0 failed; 0 ignored | n9f09dec480ea406 |
| integrated-pg18-execution_log_stream_admission_api_tests0 | 8 passed; 0 failed; 0 ignored | nbf63cf16dd004c3 |
| integrated-pg18-execution_repository_tests | 27 passed; 0 failed; 0 ignored | n751bb8b73d2e4f5 |
| integrated-pg18-executor-default | 313 passed; 0 failed; 16 ignored | n0029527b92fb4b3 |
| integrated-pg18-recorded-consumers-final | 46 passed; 0 failed; 4 ignored | n7b6b9a782c84416 |
| integrated-pg18-recorded_execution_events_tests0 | 46 passed; 0 failed; 4 ignored | n37bd97ce12c94ed |
| integrated-pg18-recorded_execution_events_tests1 | 12 passed; 0 failed; 0 ignored | n8d647dd7328c43f |
| integrated-pg18-requested-publication-broker | 4 passed; 0 failed; 0 ignored | n9b5e596cdc8d48c |
| integrated-pg18-runtime_authority_repository_tests | 271 passed; 0 failed; 0 ignored | n87cf0b6f542a463 |
| integrated-pg18-scheduler | 6 passed; 0 failed; 0 ignored | nebfee97e3e924b0 |
| integrated-pg18-service | 1 passed; 0 failed; 0 ignored | nb4f66fddcdca4d4 |
| integrated-pg18-session_lease_tests | 3 passed; 0 failed; 0 ignored | naf2128db3054400 |
| integrated-pg18-storage_maintenance_repository_tests | 30 passed; 0 failed; 0 ignored | ne71ed35d4d7f484 |
| integrated-pg18-supervisor | 58 passed; 0 failed; 0 ignored | n0e72eb52791148d |
| integrated-pg18-work_queue_api_tests0 | 10 passed; 0 failed; 0 ignored | n25557b907d9940c |
| integrated-pg18-work_queue_repository_tests | 11 passed; 0 failed; 0 ignored | nc6553d3349f741a |
| integrated-pg18-worker-restoration | 7 passed; 0 failed; 0 ignored | ndc5ce0648aab497 |
| integrated-pg18-worker-streams-final | 12 passed; 0 failed; 0 ignored | n641ced1741ff456 |
| integrated-pg18-worker_execution_transition_tests | 2 passed; 0 failed; 0 ignored | n6afa9e576a844f5 |

Full-stack tier 1, SSE, worker native-completion and log-stream-discovery selection: 42 passed; exact pytest summary and cleanup are retained with z1fe8a92a9d81422.

Workspace check, workspace Clippy with `-D warnings`, workspace/standalone/vendor
formatting and owned PostgreSQL-backed SQLx preparation are recorded in the
command manifest. Fresh migrations passed on PostgreSQL 16.14 and 18.6.
No new Rust warnings remain. API generation/check/version validation completed.

Original driver defect/healthy-control baselines and dense-pin query plans are
historical evidence, reused only for unchanged driver/discovery source. They are
not substitutes for these merged-source consumer runs. Discovery's bounded
primary-key windows retain final eligibility/pin/identity checks and eventual
progress; no speculative index claim is made.

Gaps: required CI and review on the pushed main-integrated head are the parent
coordinator's gate. The complete workspace test suite was not run locally.
The existing dashboard timezone setup-only placeholder and six documented
policy-deferred deployment cases are excluded from passing coverage. No known
task regression remains in the completed selected checks. Owned fixtures and
Compose project resources were removed and absence verified; warm caches remain.
The original leaking request and live data loss remain unproven.
