# Runtime log finalization reliability plan

Status: phases 1 and 2 implemented on `fix/runtime-log-finalization`; integration, final validation, and review remain in progress.
Date: 2026-11-08.

PR scope: phases 1 and 2, including their applicable validation. The user
confirmed durable recovery belongs in a follow-up PR. Phase 4 remains gated by
measurement and the separate seal-claim protocol.

Make stdout and stderr delivery tolerate transient storage and API delays without
leaving completed executions with unreadable logs. First align finalization
deadlines and retries and expose delivery failures accurately. Treat recovery
after worker loss and verification outside database transactions as separate
protocol changes with explicit durability and authorization requirements.

## Incident and evidence

The default Kubernetes cluster was running API version `0.10.3`. Investigation
observations came from a moving one-hour window on 2026-11-09 UTC
(2026-11-08 America/Chicago), rather than a controlled benchmark.

Execution `7668` completed normally. Its stdout artifact was
`core.echo.stdout.log`, artifact ID `1`, version number `7668`, version ID
`7669`. The stream was sealed 214 ms after creation. Its encrypted completion
manifest decrypted with the configured key, and its byte count and checksum
matched the stored metadata. Subsequent downloads returned HTTP 200.

The three earlier HTTP 403 downloads selected version number `7670`, version ID
`7671`, belonging to execution `7670`. That execution completed, but its log
remained Pending and unsealed, with uploaded bytes and no final manifest. The
worker warned: `Runtime logs were not fully drained and sealed before finalization`.
The UI's generic access-denied message concealed the incomplete delivery state.
The requesting identity had `core.admin`; these failures were not evidence of
missing read permissions. The reason the user initially associated them with
`7668` was not established; preserve exact version selection in any UI changes.

Successful seal HTTP requests sampled during the investigation had a 98 ms
median, 217 ms p95, and 762 ms maximum across 1,505 requests. A separate database
snapshot counted 49 unsealed streams among 3,580 streams attached to completed
executions in its preceding hour. Successful-request timings exclude failures
and do not include the entire final upload and output-draining path. The precise
slow stage for execution `7670` remains unconfirmed.

Source was rechecked in the release worktree at
`d9da94122ff6fab725c212ac81a6a965a3bd2dbd` when writing this plan.

## Failure mechanisms at the incident baseline

- `artifacts.log_finalization_timeout_ms` defaults to 1,000 ms. The process
  executor shares that budget across draining both streams, flushing the final
  encrypted frames, uploading remaining segments, and sealing. It also takes
  the earlier execution deadline, so little execution time remaining can shorten
  finalization further.
- Segment commits allow five attempts, each with a 10,000 ms timeout plus
  backoff. The outer finalization deadline can cancel the first attempt long
  before this retry policy has a chance to work.
- Segment commits retry transient errors. The seal transport classifies some
  errors as retryable, but the seal caller has no corresponding transient retry
  loop; authentication refresh is separate.
- Cancellation drops the seal future, and dropping `SegmentedLogWriter` aborts
  its upload task. The authenticated manifest and any buffered bytes are not
  retained for recovery. A remote request may nevertheless have committed;
  cancellation is not proof of rollback or stopped provider activity.
- The worker can report a successful action with `result.logs_incomplete`.
  That distinction is useful, but the readers and UI do not consistently explain
  incomplete logs. The download audit can record success before delivery fails.
- API sealing reads and verifies stored log bytes while retaining database
  transactions and locks. Storage latency can therefore contribute to contention.

## Implementation progress

The current implementation keeps a bounded 60-second default delivery budget,
with the action deadline as an earlier bound and cancellation still active.
Idle pipe reads have a separate one-second post-exit limit; already buffered
bytes drain first. Exhausting delivery time after process exit marks logs
incomplete without retroactively timing out a successful process. A storage
write failure disables storage for that stream while bounded pipe capture
continues, preventing a logging failure from causing a broken pipe in the action.

Both storage backends retry transient seals with identical version, truncation,
and authenticated-manifest inputs. Requests and backoff remain inside bounded
attempt and total deadlines. Stream tasks are explicitly aborted and joined on
capture completion or cancellation; Drop remains fallback cleanup.

Capture outcomes are independent for stdout and stderr. The existing v1
`runtime_log_capture` markers are authoritative per stream; aggregate
`logs_incomplete` does not erase positive evidence that its sibling reached empty
EOF. Permanent seal failure leaves the process exit outcome intact.

After full selected-version content authorization, finite downloads distinguish
HTTP 409 `LOG_FINALIZING` from `LOG_INCOMPLETE`. The UI displays only allowlisted
messages, bounds error-body parsing, and preserves exact immutable selection.
Execution SSE keeps its incomplete-stream code and two-second terminal grace:
normal worker settlement still precedes terminal publication, so the longer
worker budget does not require a longer terminal reader grace.

Runtime-log download success audits require full authorized EOF. Cancelled
previews, partial ranges, rejected downloads, and failed body reads do not emit
successful full-download events. Instance-local API counters distinguish
finalizing and incomplete responses; request logs record observed pending age,
and worker traces distinguish retry operations, seal timing, and deadline loss.
The CLI already renders structured API error bodies and needs no fallback reader.

The 60-second ceiling accommodates a single default segment retry window of up
to 51.5 seconds with headroom. It is a bounded starting policy, not a measured
optimal value or a guarantee that every upload and seal attempt fits. Durable
recovery and moving verification outside transactions remain follow-up work.

An initial full web run hit timeouts during concurrent compilation. The clean
base checkout passed its full 600-test suite; the full fix-suite rerun and final
integration results will be recorded below. No timeout settings or exclusions
were changed to obtain passing results.

### Validation of the first slice

Status: work in progress, not ready to merge. Tested base:
`d9da94122ff6fab725c212ac81a6a965a3bd2dbd`, with the uncommitted first-slice code
and documentation changes in `/mnt/wdc/attune-worktrees/runtime-log-finalization`.
All commands ran in that worktree. Cargo artifacts used the isolated target
`/mnt/wdc/attune-worktrees/runtime-log-finalization-target`.

- `ATTUNE_TEST_RUN_ID=logfix-common2 cargo test --locked -p attune-common --lib log_stream::tests -- --test-threads=4`: 16 passed.
- `ATTUNE_TEST_RUN_ID=logfix-worker1 cargo test --locked -p attune-worker --lib runtime::process_executor::tests -- --test-threads=4`: 18 passed.
- `ATTUNE_TEST_RUN_ID=logfix-writer1 cargo test --locked -p attune-worker --lib runtime::log_writer::tests -- --test-threads=4`: 15 passed.
- `cargo clippy --locked -p attune-common -p attune-worker --lib -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, and plan relative-link checks: passed.

The selected tests exercise the changed retry mechanism, process boundary, lazy
allocation, encryption, and truncation without external services. The HTTP test
uses an owned local server; it does not prove production API seal idempotency or
object storage settlement. Workspace-wide checks, PostgreSQL/object-storage
integration, sensor consumer validation, fault benchmarks, and required CI remain
unrun. The final plan update is prose only and preserves the code-test results.
The first slice was subsequently committed as `7ff2fcc4` and published in draft
PR #31. Those initial results are historical evidence, not final-head readiness.
No deployment changes were made.

## Required invariants

- Keep process exit status distinct from log delivery outcome. A completed action
  does not prove its logs are complete; missing output is not intentional silence.
- Publish Ready, sealed state, and authenticated final evidence atomically.
  Preserve full frame, sequence, byte-count, checksum, and exact provider-version
  verification. Never mark uploaded prefixes complete to remove a warning.
- Preserve lazy allocation, independent stdout/stderr outcomes, bounded memory,
  truncation rules, and exact execution/version selection. No fallback to latest.
- Preserve source and target authorization, active ownership, admission and
  attempt fencing, worker-instance identity, expiry, revocation, and final
  freshness checks. Recovery evidence must not become new publication authority.
- Preserve repository access, graph lock order, and independent execution
  retention. No new execution foreign keys or automatic database resets.
- Bound shutdown and cancellation and await owned task teardown. Retain unknown
  commit and upload outcomes explicitly; do not label them confirmed failures.
- Logs remain private. Diagnostics must not contain plaintext logs, bearer
  credentials, encryption keys, or decrypted secret material.

## Phase 1 Align deadlines and retry sealing

- [ ] Instrument drain, final frame flush, upload attempts, seal request,
  verification, and acknowledgement separately, correlated by execution, stream,
  version, and attempt. Record deadline exhaustion, retry exhaustion, permanent
  denial, and unknown outcome distinctly.
- [ ] Define separate process-runtime, pipe-drain, and delivery-finalization
  semantics. Preserve bounded behavior for descendants holding pipes open.
  Decide explicitly whether delivery may continue after the process deadline;
  any continuation must fit valid delivery authority and shutdown budgets.
- [ ] Replace the 1-second default with a measured, bounded finalization budget.
  Raising the existing setting is a deployment mitigation, not durable recovery.
  Document the chosen value and its effect on occupied worker slots and shutdown.
- [ ] Give remaining uploads and seal retries one explicit deadline. Bound each
  attempt and backoff by the remaining budget; avoid nested policies whose
  advertised retry window cannot fit. Validate configuration relationships.
- [ ] Retry transient seal transport failures with bounded backoff and jitter.
  Reuse the same version, truncation, and authenticated-manifest identity.
  Authentication, proof mismatches, and permanent conflicts must fail closed;
  retry only explicitly classified transient conditions.
- [ ] Reconcile lost acknowledgements through freshly authorized, identical
  seal retries. Test server commit followed by a dropped response and concurrent
  identical requests. Different proof or truncation inputs must conflict.
- [ ] Keep settlement before terminal publication for this phase. Do not add
  detached work or extend execution-token permissions as a shortcut.

## Phase 2 Expose log delivery state

- [ ] Define per-stream finalizing, complete, and incomplete or failed outcomes,
  with bounded reason codes. Reuse artifact lifecycle and existing capture
  metadata where sufficient; add persisted fields only where recovery or
  accurate reader behavior requires them.
- [ ] Preserve action success when its process succeeds, while making log
  delivery failure visible on execution details. Review whether additional
  operational alerting is needed for repeated failures.
- [ ] Return a stable incomplete-log response distinguishable from authorization
  failure without disclosing metadata to unauthorized callers. Update finite
  downloads, execution SSE, artifact previews, CLI consumers, and web UI together.
- [ ] Align SSE terminal grace and reconnect behavior with the finalization
  contract. Never emit successful EOF for an unsealed stream.
- [ ] Correct premature download-success auditing and define its meaning for
  streamed responses; do not claim verified delivery before it occurs.
- [ ] Add metrics for incomplete streams, retry outcomes, finalization deadline
  exhaustion, and pending age. Avoid unbounded ID labels in metrics.

## Phase 3 Design recovery after worker loss

This phase is required for recovery across worker restarts, not for the first
deadline and retry fix. Specify and review the protocol before implementation.

- [ ] Persist finalization intent containing the exact stream/version, accepted
  ciphertext frontier, authenticated completion manifest, truncation, and
  admission/attempt/worker identities before handing work to recovery.
- [ ] Decide how uncommitted encrypted buffers survive process or node loss.
  A durable job cannot recover bytes that existed only in worker memory. Bound
  any spool, encrypt it, assign ownership, and specify retention and cleanup.
- [ ] Define fenced ownership, leases, takeover, bounded retries, terminal
  failure, and shutdown behavior. Persist intent before reporting any recoverable
  handoff; define the execution terminal-publication boundary explicitly.
- [ ] Specify publication authority after worker loss or credential expiry.
  Revalidate through an explicit bounded recovery capability; do not retain raw
  tokens or infer authority from historical permission snapshots alone.
- [ ] Pin all evidence and exact object references required by pending recovery.
  Coordinate supervisor scheduling and retention so cleanup cannot erase live
  work, while expired or revoked publication still permits safe cleanup.
- [ ] Preserve unresolved remote PUT obligations and unknown commit outcomes.
  Recovery must not claim old incomplete streams can be repaired without a
  trusted completion proof and all required bytes.

## Phase 4 Reduce seal contention when measurements justify it

Profile API pool waits, graph admission, source/target locks, object reads,
hashing, frame verification, and final publication separately. Reuse the
[worker lifecycle performance work](worker-execution-lifecycle-performance.md)
instead of duplicating its query reductions and lazy allocation changes.

If storage verification must leave the transaction, follow the
[object log seal claims design](object-log-seal-claims.md) and its upload
settlement prerequisites. Freeze and pin exact inputs under a fenced claim,
verify outside database transactions, then freshly authorize and atomically
publish in a short transaction. Existing upload grants and reader leases are
not substitutes for this claim. Keep shared-file locking and recovery semantics
explicit rather than assuming the object protocol applies unchanged.

## Validation and acceptance

- [ ] Use controlled transports and readiness barriers to test upload and seal
  delays longer than the old 1-second deadline that succeed within the new
  budget. Use deterministic clocks where possible.
- [ ] Cover transient failures, retry exhaustion, lost acknowledgements,
  concurrent identical seals, conflicting manifests, and unknown commit outcomes.
- [ ] Preserve process cancellation, runtime timeout, post-exit cancellation,
  bounded pipe drain, and joined background task teardown.
- [ ] Exercise silent output, stdout only, stderr only, both streams, truncation,
  encrypted frame boundaries, and independent failure of either stream.
- [ ] Run repository/API tests with real disposable PostgreSQL and versioned
  object storage for commit, settlement, and reader contracts. Cover shared-file
  transport separately. Follow [runtime log verification](../deployment/runtime-log-verification.md).
- [ ] Verify revocation, expired authority, stale attempts, worker reassignment,
  changed owner bindings, retention races, and invalid or incomplete proofs still
  deny publication and plaintext delivery.
- [ ] For durable recovery, inject crashes before and after intent persistence,
  final upload, seal commit, acknowledgement, takeover, and cleanup. Require
  stale owners to be fenced and outstanding obligations to remain owned.
- [ ] Add UI and API contract cases distinguishing incomplete logs from denied
  access, preserving exact version selection and successful empty-stream behavior.
- [ ] Repeat identical serial and concurrent workloads with failures included
  in the denominator. Report incomplete rates, process latency, finalization
  latency, worker occupancy, memory, retries, pool waits, and lock hold times.
  Require no incomplete logs for injected transient faults that recover within
  the configured budget, and explicit bounded failure beyond it.
- [ ] Follow [running tests](../testing/running-tests.md) for affected crate,
  formatting, Clippy, web, generated-client, and SQLx checks as applicable.
  Required CI must pass before merge readiness.

## Delivery sequence

1. Instrumentation, coherent deadlines, seal retries, and focused failure tests.
2. Delivery state, reader/UI errors, audit semantics, and contract validation.
3. Reviewed durable recovery design followed by schema, repository, worker,
   supervisor, retention, and crash-test changes as one coherent protocol.
4. Measurement-driven verification split, gated by seal claims and upload
   settlement correctness.

Implement and validate on disposable infrastructure first. Production configuration
changes and application rollout require separate deployment authorization.
