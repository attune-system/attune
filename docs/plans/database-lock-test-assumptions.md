> Current object-close checkpoint: the production object seal route now selects
> durable input, awaits prepare commit, verifies outside database ownership, and
> freshly authorizes atomic publication. Migration 006 provides deferred shape
> constraints; bounded object housekeeping abandons expired matching revisions.
> This is work in progress. Earlier statements below describing publication as
> disabled or the route as unsplit are historical checkpoint evidence, superseded
> by [the current protocol](object-log-seal-claims.md). New acceptance tests and
> source-specific validation must pass before lock-removal/certification claims.
> No full PUT settlement or filesystem fence removal is claimed.

# Database lock reduction: test assumption audit

Status: predictive source audit; no tests changed or executed for this analysis.
Date: 2026-11-09 UTC.

Read with the [implementation map](database-lock-reduction-implementation.md)
and [lock scope audit](database-lock-scope-audit.md). This identifies tests that
could fail, need new synchronization, or silently lose coverage when a particular
slice is implemented. It is not a list of existing failing tests.

The source baseline and PR #31 integration requirements are recorded in the
implementation map. Existing local changes to file_evidence_retention_tests/
bounded_admission.rs belong to other work and were inspected without alteration.
Local search evidence is in the workspace's
.scratch/platform-db-analysis/lock-audit/test-lock-map.json and
test-lock-candidates.txt. Those are text candidates, including test helpers;
nearest-function association is heuristic, not a semantic coverage proof.

## Rules for changing expectations

- **Keep the outcome:** uniqueness, exact retry identity, stale-owner rejection,
  current authority, no early deletion, strict quotas, bounded waiting, no leaked
  backend, and correct unknown-commit recovery remain requirements.
- **Change a mechanism assertion only with its replacement:** a specific advisory
  key, table lock mode, one retained lock set, or one transaction across I/O may
  become obsolete. Identify the replacement fence and assert it actually works
  across independent connections/processes.
- **Move the barrier when the phase moves:** a test waiting for a lock that is
  intentionally removed will hang or time out. Synchronize on the replacement
  claim/frontier or an explicit storage/phase rendezvous, then assert outcomes.
- **Do not fix concurrency regressions by accepting more outcomes:** a timeout,
  403, 409, duplicate dispatch, or missing bytes is not an acceptable substitute
  for success merely because the implementation is more concurrent.
- **A passing old test may be insufficient:** fixture-acquired locks can hide
  that the production path no longer uses the coordination being tested.

## Tests tied directly to the current mechanism

### Artifact advisory identity and wire type

Source: [artifact_lock_wire_types_tests.rs](../../crates/common/tests/artifact_lock_wire_types_tests.rs),
both_log_ready_paths_reuse_decimal_text_lock_and_preserve_cross_connection_exclusion.

It primes the connection through ArtifactRepository::find_by_id_for_update,
seals/publishes, then probes the exact artifact_version advisory key from another
connection. It protects a real SQLx cached-argument-type regression and consistent
lock identity across callers.

If parent coordination changes, this does not automatically become a valid test
of the replacement. The fixture already holds the parent advisory lock before
the Ready callback: the exclusion assertion could keep passing even if that
callback stopped acquiring the intended lock. Split the parameter-type coverage
for remaining advisory participants from publication behavior. Add an independent
connection test proving unrelated versions proceed during delayed verification
and conflicting transitions on one version still resolve correctly. Do not
remove the wire-type regression while generic artifact paths still use the SQL.

### PhysicalCleanup ownership and rollback

Source: [storage maintenance bounded tests](../../crates/common/tests/storage_maintenance_repository_tests/bounded_admission.rs),
physical_cleanup_metadata_deadline_rolls_back_and_releases_admission and
physical_cleanup_queued_admission_restores_the_inherited_lock_timeout.

These call PhysicalCleanup directly and verify acknowledged metadata rollback,
graph release, and restoration of the inherited lock timeout. They will need
API/fixture changes if that owner is removed. Keep them while any caller remains.
The timeout restoration and protocol cleanup requirements also apply to each
replacement transaction.

The new lifecycle needs phase-specific assertions: a failed prepare rolls back
its claim, but a failed finish cannot roll back a previously committed deletion
claim or restore removed bytes. Add restart/retry and unknown-commit reconciliation
tests. During a paused physical delete, prove unrelated graph capture can commit
and the exact target cannot be revived, replaced, or repinned unsafely.

### Cache lock set, preflight, and wire fault injection

Sources: [cache_reclamation_admission_tests.rs](../../crates/common/tests/cache_reclamation_admission_tests.rs),
[direct cleanup ordering](../../crates/common/tests/cache_generation_partition_tests/direct_cleanup_graph_ordering.rs),
and [cleanup acknowledgment](../../crates/common/tests/cache_generation_partition_tests/cleanup_acknowledgement.rs).

require_acquired_and_rollback explicitly requires exclusive graph admission,
advisory key (7821101, 0), and ACCESS SHARE on cache_entry. That exact tuple must
change if a reviewed cache protocol replaces one component. It should remain
unchanged for initial query/statistics/log-lifetime work.

cache_busy_preflight_does_not_queue_graph_or_block_later_capture_and_read,
graph_busy_entry_releases_preflight_and_retries_when_cache_changes_owner, and
direct_cleanup_cache_wait_allows_shared_graph_admission protect deadlock
avoidance, not just an incidental lock count. Keep the no-retained-graph-while-
waiting behavior even when keys or quota coordination change.

The acknowledgment harness matches an exact setter SQL string and BEGIN/COMMIT
wire messages. If shared budget SQL or transaction count changes, update the
interception point deliberately; otherwise a test may miss its fault or stall.
After splitting an operation into transactions, arm faults for the intended
prepare/finalize boundary. Continue proving that an unknown commit is not
reported as a rollback and does not cause a duplicate destructive operation.

### Executor cache/workflow ordering

Source: [cache_partition_ordering/mod.rs](../../crates/executor/tests/cache_partition_ordering/mod.rs).
Examples include cleanup_first_blocks_dispatch_before_workflow_and_iteration_row_locks,
workflow_first_commits_scanning_pin_before_cleanup_rechecks_eligibility,
broker_cache_cleanup_first_blocks_real_transaction_entry_before_rows, and
broker_cache_workflow_first_protects_parent_before_blocked_workflow_row.

Helpers wait for particular table/advisory locks and observed blockers. If cache
admission is narrowed, those harnesses and exact lock-set expectations may need
replacement. The outcomes remain mandatory: no deletion beneath a scanning pin,
no target locks held while waiting on earlier admission, no lock-order cycle,
and recovery through the real broker path. Do not rewrite these as serial tests
or replace them only with repository mocks.

### Graph transaction tracing

Source: [graph transaction trace tests](../../crates/common/src/observability/graph_transaction/tests.rs),
including reentry_preserves_first_request_and_acquisition_and_one_holder.

The unit test correctly describes one transaction and should remain valid.
After seal splitting, a route owns two short transactions rather than one owner
spanning verification. Add route-level assertions for separate transaction IDs,
acknowledged ends, and no hold attributed to the storage phase. Do not make one
trace pretend its first commit also ended a later transaction, or relax unknown
outcome reporting on drop/cancellation.

## Behavioral regressions that must remain failures

| Test family and examples | Required treatment |
| --- | --- |
| [Artifact graph ordering](../../crates/common/tests/artifact_retention_graph_order_tests.rs): create_and_both_ready_callbacks_defer_reclamation_without_upgrading_source_admission; queued_gc_writer_does_not_make_publication_a_lock_upgrade_and_real_delete_still_denies | Keep publication under shared capture free of inline reclamation/upgrades. Keep real deletion admission and current-pin rejection. Update fixtures only if the publication API changes. |
| [Bounded release collector](../../crates/common/src/repositories/pack_retention/bounded/tests.rs): raw_pages_cross_pinned_prefix_and_wrap_even_with_new_arrivals; admission_first_rechecks_new_pin_after_queued_gate_with_fresh_snapshot; replacement_identity_after_discovery_is_skipped_until_the_next_pass | Preserve exact raw-page progress, fresh revalidation, and stale-identity skipping while rewriting predicates. They should not require weaker assertions for a faster query. |
| Same collector: empty_sweeps_need_no_exclusive_admission_and_complete_their_transactions; discovery_timeout_does_not_shrink_or_advance_visit_progress; release_retention_preserves_strict_cutoff_and_newest_inactive_id_ties | Keep no-op admission avoidance, correct cursor behavior, strict time cutoff, and newest-release tie rules. Add positive eligible-release cases with nested JSON pins so empty production data cannot mask a missing predicate. |
| Same collector: release_commit_ack_loss_withholds_cleanup_payload_but_keeps_confirmed_pages | Preserve the rule that an uncertain response cannot authorize physical deletion. If a durable cleanup obligation is committed atomically, recovery must first read its confirmed identity/ownership. Do not turn an unknown result into a fabricated confirmed payload. |
| [File-evidence bounded admission](../../crates/common/tests/file_evidence_retention_tests/bounded_admission.rs): empty_metadata_passes_never_queue_behind_capture; deletion_claim_rechecks_identity_after_waiting_for_admission; deletion_unknown_commit_never_records_a_failed_visit_or_returns_claims | Keep current graph/evidence protocol for initial slices. Do not edit these tests just because log verification stops retaining graph admission. A separate durable close protection must cover logs without fabricating file-origin authority. |
| [Storage maintenance](../../crates/common/tests/storage_maintenance_repository_tests.rs): claimed_deletion_cannot_be_revived_by_upload_calls; stale_upload_cleanup_does_not_delete_a_refreshed_reservation; shared_log_seal_and_cleanup_claim_have_exactly_one_winner | Preserve exact identity and publication-versus-cleanup exclusion. Shared-file behavior stays unchanged initially. Add an object-close-versus-cleanup race through the production lifecycle; an older hand-built one-transaction fixture cannot prove the split path. |
| [Runtime authority](../../crates/common/tests/runtime_authority_repository_tests.rs): graph_writer_waits_before_pack_lock_so_recorded_creation_can_finish; approval_effect_after_target_wait and its callers | Keep fresh authority after waits and canonical admission before conflicting owner writes. Traversal deduplication must still verify each incoming pin and reject expiry/revocation before publication. |
| [Notifier transaction ownership](../../crates/notifier/src/recorded_authority_transaction_tests.rs): queued_exclusive_waiter_does_not_require_a_second_admitted_connection; cancellation_recovers_the_owned_pool_and_releases_shared_admission | Keep bounded pool use and confirmed cleanup. A second short transaction may reuse the pool sequentially; it must not require simultaneous ownership of two connections. |
| [Reclamation cancellation](../../crates/common/src/repositories/reclamation_cancellation/tests.rs): dropped_blocked_query_with_main_one_uses_backend_key_and_disposes_exact_owner; unknown_commit_never_retries_or_inserts_a_fresh_row | Preserve backend ownership and cancellation guarantees. More phase boundaries require more failure cases, not removal of disposal/quarantine coverage. |
| [Cache quota tests](../../crates/common/tests/cache_repository_tests.rs): aggregate_admission_limits_are_atomic_across_racing_writers; managed_namespace_admission_is_atomic_and_updates_remain_idempotent | Keep strict caps across independent writers. Add outstanding reservations to coverage only if that design is implemented; quota overshoot is not an acceptable new concurrent outcome. |
| [Pack release tests](../../crates/common/tests/pack_release_repository_tests.rs): dev_version_republish_serializes_concurrent_identical_publications; pack_republish_off_serializes_conflicting_first_publications | Preserve publication identity, version policy, and idempotency. Add publisher-versus-cleanup and same-content reuse races before removing the table fence. |

## No-op and scheduling expectations

[Native partition tests](../../crates/common/tests/native_partition_repository_tests.rs)
include busy_default_repair_returns_typed_outcome_then_retries_after_reader_unlocks
and concurrent_repairers_serialize_on_parent_and_restart_converges. Actual default
repair still requires its strong DDL locks, so those busy/atomicity tests should
remain. Do not change them to expect two concurrent repairs to mutate the same
default data independently.

Add an already-present-day test while a compatible reader is active: the validated
fast path should finish without exclusive graph or ACCESS EXCLUSIVE acquisition.
Also test drift/corrupt registry/owner/bounds, and a repair/drop race. The initial
preflight cannot turn incompatible_catalog_or_owner_never_gets_adopted_or_dropped
into success. Existing restart/cursor/UTC tests remain relevant.

[Cache statistics tests](../../crates/common/tests/cache_repository_tests.rs)
include cache_statistics_lock_deferral_preserves_pending_revision and
statistics_acknowledgement_does_not_consume_a_concurrent_lifecycle_request.
Their fixtures create due work, so due-work deferral and revision assertions stay.
Add a separate not-due case under a conflicting maintenance lock: it should return
NotDue through the preflight instead of waiting merely to discover no work.
Dry-run, ownership validation, cancellation rollback, and pending-revision
preservation must continue to hold.

[Sensor admission tests](../../crates/common/tests/sensor_admission_repository_tests.rs)
cover eligibility/placement but do not establish the proposed per-sensor gate
protocol. Broad mutation fixtures such as
[managed_readmission_fixture](../../crates/api/tests/managed_readmission_fixture/mod.rs)
can continue using global admission for bulk deployment. For scoped operations,
add real-router races: mutate A while B emits; mutate A while A emits; retarget
a rule from A to B; runtime-wide mutation versus both sensors; stale workload
generation and cross-pack dependency changes. Preserve current admission and
rollback decisions. Any helper now requiring a sensor set will need explicit
fixture inputs rather than an arbitrary default sensor ID.

## Logging coverage and PR #31

Preserve the PR #31 tests in common/log_stream.rs, worker/runtime/process_executor.rs,
and artifact delivery routes/tests. Their longer bounded budget, independent pipe
draining, process-success separation, accurate per-stream state, exact retries,
and verified-EOF authorization/audit remain requirements. Lock reduction must not
replace them with a longer unbounded wait or reintroduce generic 403 responses.

In [internal_files.rs](../../crates/api/src/routes/internal_files.rs),
concurrent_log_commit_and_seal_work_with_one_database_connection should continue
to succeed. Expand it with delayed storage verification and an unrelated query
through a size-one pool: a paused verifier must not retain that connection.
Keep identical segment/seal retries successful and changed inputs conflicting.

[runtime_log_replica_tests.rs](../../crates/api/tests/runtime_log_replica_tests.rs)
already has delayed/failing storage controls and real multi-replica requests.
Extend object_s3_duplicate_ambiguous_and_finalize_orderings and
object_s3_upload_reconnect_and_pinned_reads for delayed PUTs crossing close,
expired grants with unresolved physical work, exact provider pinning, and lost
final-publication acknowledgments. Exercise direct and proxy uploads separately.
Counts of accepted segments alone cannot prove physical settlement.

shared_volume_cross_process_locking_writer_loss_and_retention explicitly checks
that sealing cannot bypass a child process's flock. Keep that test and expectation
while the shared-file backend remains unchanged. A future immutable-snapshot
implementation may let the database transaction end during that wait, but still
must not claim complete bytes before the writer/snapshot is safely fenced.

## New tests needed for the changed guarantees

1. **Frozen close input:** pause verification after prepare; reject fresh append,
   altered retry, segment replacement, body replacement, and parent/version/stream
   cleanup. Exact accepted retries succeed without a new PUT or byte charge.
2. **Authority after the gap:** revoke policy/approval, freeze identity, replace
   worker attempt, or expire the credential during verification. Fresh finalize
   denies publication and retains safe recovery/cleanup state.
3. **No retained connection or graph lock:** pause provider GET/hash/delete with
   an explicit channel/barrier. Complete unrelated event publication and a
   size-one-pool query before releasing that barrier. Test the same-target conflict
   separately; unrelated progress must not imply an unsafe same-target success.
4. **Per-phase crash/unknown commit:** kill or disconnect before and after prepare
   commit, during I/O, and before/after finish commit. Reconcile durable state
   without double append, double dispatch, false completion, or unsafe deletion.
   Execution/admission retention must not erase an unresolved close or physical
   obligation; preserve independent lifetimes without adding execution FKs.
5. **Cleanup recovery:** hold a delayed filesystem operation across cancellation
   and attempted path reuse. Its eventual completion cannot remove replacement
   content. Failed finalization leaves a recoverable obligation and bounded backlog.
6. **Scoped admission and strict reservations:** independent sensors/namespaces
   progress concurrently; shared dependencies, quotas, and global mutations remain
   serialized or fail with their existing safe semantics.
7. **Query equivalence:** eligible and pinned candidates, strict cutoff, version
   gaps, nonpositive limits, ready/pending bodies, nested release bindings, changed
   retention during discovery, and wrapping past pinned prefixes. Assert selected
   identities and committed effects, not an exact EXPLAIN node tree.
   Check that transaction-local JIT policy is restored after commit, rollback,
   timeout, and pool reuse and does not change unrelated foreground transactions.

Use deterministic phase handshakes and observed backend state rather than sleeps
as the sole ordering proof. Real PostgreSQL relation/row locks and provider-backed
tests remain necessary for the relevant boundaries. Keep database identity in
pg_locks filters: physical-clone fixtures can share relation OIDs across databases.

## Validation scope when implementation starts

Run affected common repository/integration tests for each slice, then API,
supervisor, executor, worker, and notifier consumers whose contracts changed.
Use the repository's owned disposable database runners and the
[runtime-log verification harness](../deployment/runtime-log-verification.md)
for S3/replica/shared-file cases. Include forward-migration/schema validation and
generated metadata/client checks if those contracts change.

Do not run only tests with lock in their filename: implicit triggers, caller
transactions, broker delivery, and read authorization carry much of the safety
contract. Report actual results separately from this predictive inventory. No
test thresholds, assertions, fixtures, or skips were changed by this audit.

## Storage verifier continuation

The object seal route now calls a storage-only verifier while retaining its old
transaction. Pure adversarial reader tests establish byte validation only; they
must not be counted as proof of pool-size-one progress, admission release, fresh
producer authority, committed close binding or publication/recovery races. The
new database TimeZone retry case uses a nonempty stream, so timestamp rendering
actually participates in the selected-input hash. Historical non-UTC selections
are not rewritten. Selected-close publication remains disabled.

### Staged source helpers continuation

The new source API does not move route barriers: no route uses it yet. Tests of
`log_source::acquire` / `Source::bind` must consume Source through its commit method before opening
the final transaction, then compare a freshly authenticated binding. The source
owns its transaction, so it cannot be reused on a different transaction.
Source lock release must be observed by an independent NOWAIT source writer;
serializing HistoricalBinding must retain only historical identity. Source
expiry checks belong after target waits, including sensor lease clock expiry.
Native terminal completion must remain a positive case; worker terminal completion
must remain denied. No FK or fabricated worker permit may replace these checks.

The completed source prerequisite now has actual integration coverage for process,
delivery and sensor owners through the hidden Rust helper API, plus typed native
and legacy-worker repository projections. See the implementation map for the
PG16/18 35-case inventory and provenance. The approval-expiry fixture warms and
cleans its own migration context before starting the original 30-second approval;
all wait budgets/barriers/assertions are unchanged. This fixes a demonstrated
cold-setup failure and does not reclassify older timing or CommitUnknown failures.
These results still do not certify durable finalization or a pool-free route.
