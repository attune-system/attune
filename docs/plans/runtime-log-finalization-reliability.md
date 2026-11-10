# Runtime log finalization reliability plan

Status (2026-11-10 UTC): phases 1 and 2 integrated through merged
[PR #31](https://github.com/attune-system/attune/pull/31) and main's PR32 integration.
Object close verification outside transactions is also integrated through
[PR #32](https://github.com/attune-system/attune/pull/32), for the fresh-instance
ObjectSegments scope. Worker crash recovery and physical PUT settlement remain
incomplete. See the [follow-up roadmap](runtime-log-follow-up-roadmap.md) for
current CI evidence, deployment observations, dependencies and rollout gates.
Initial plan date: 2026-11-08; dated evidence below is preserved.

PR scope: phases 1 and 2, including their applicable validation. The user
confirmed durable recovery belongs in a follow-up PR. PR32 subsequently delivered
the object verification split using durable close selection. Measurement, physical
PUT settlement and the shared-file split remain separate work.

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
recovery remained follow-up work at the PR31 checkpoint. PR32 subsequently moved
object verification outside transactions; shared-file verification still retains
its existing transaction and flock.

### Validation of the integrated implementation

The following is historical PR31 checkpoint evidence, including its then-pending
CI/security statements. On 2026-11-10 GitHub reported PR31 merged with all 16
reported checks successful; the roadmap records exact heads and CI links. Those
results do not certify the current deployed images or deferred recovery work.

This is checkpoint evidence; the PR carries the final validation handoff and
readiness status. Initial integrated Rust checks target
`668c6edad1904ff2f492e1617594a4b116a9b121` after integrating main at
`8af23fc4d77ce84d661474cb6711d0db3be906d3`. Subsequent changes at `7174ff16`
add a web capture-failure notice and generated client responses. `eec204e0` adds
concurrent seal-replay coverage and excludes HTTP error-body EOF from download
success auditing. The affected API tests are rerun for those final changes;
common/worker and web results remain reusable because their sources did not change.

All checks run in the fix worktree. Rust artifacts use its separate target
cache. PostgreSQL 18 and versioned RustFS storage are disposable, task-owned
containers; no cluster configuration or deployed data was changed.

- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed,
  including all-target workspace compilation.
- Common and worker library tests filtered by `runtime`: 272 passed.
- Common log-writer tests filtered by `log_stream`: 17 passed.
- API artifact headers/content, single-connection, execution-log admission, and
  sensor-log integration suites: 27 passed on PostgreSQL. These cover authorized
  Pending diagnostics, every terminal execution status, hidden-content rejection,
  per-stream empty results, selected-version fences, and full-EOF audit semantics.
- Full web suite: 612 tests passed. `npm run build` and
  `npm run typecheck:contracts` passed. TypeScript, ESLint, and Prettier passed.
- API generation and `npm run check:api` passed, including the two new HTTP 409
  response entries. Workspace, exported OpenAPI, and generated client versions
  all remain `0.10.3`.
- The direct S3 upload test passed. All nine replica/storage cases passed with
  versioned RustFS and PostgreSQL, including reconnect, missed notifications,
  terminal-before-publication, source revocation, ambiguous upload retries,
  pinned reads, and shared-file cross-process locking/retention. The first
  integrated run had three SSE timeouts while concurrent compiler I/O produced
  3–4-second database commits. The full rerun passed with task-owned compilers
  paused, the same four test threads, timeouts, and coverage. Compilation resumed
  afterward, and owned object-prefix cleanup completed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

An initial full web run timed out during concurrent compilation. The clean base
passed all 600 tests, and both subsequent complete fix-suite runs passed without
changing timeout settings or excluding tests. On Node 26 the local command uses
`NODE_OPTIONS=--no-experimental-webstorage` so jsdom owns localStorage; CI uses
Node 22. The earlier CI client-generation failure is addressed by including the
regenerated `ArtifactsService.ts` rather than changing the contract check.

The PR records the final API/encrypted-reader reruns, targeted Clippy, required
CI, and requested GPT-6 Astra review. A security CI attempt at `eec204e0` failed
before scanning when GitHub returned HTTP 504 for the Gitleaks download; a
successful security check is still required for readiness. These checks validate
fault handling and authorization contracts, not production throughput or NFS/RWX
conformance. Durable crash recovery and comparative deployment benchmarking
remain follow-up work; this PR makes no measured performance-improvement claim.

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

- [x] Add structured capture, segment-flush, retry, and seal timing diagnostics
  correlated through execution/stream spans and version/attempt fields. Distinguish
  deadline exhaustion, retry exhaustion, permanent denial, and unknown outcomes.
  Fine-grained server verification profiling remains part of phase 4.
- [x] Define separate process-runtime, pipe-drain, and delivery-finalization
  semantics. Preserve bounded behavior for descendants holding pipes open.
  Decide explicitly whether delivery may continue after the process deadline;
  any continuation must fit valid delivery authority and shutdown budgets.
- [x] Replace the 1-second default with a bounded 60-second finalization budget.
  Raising the existing setting is a deployment mitigation, not durable recovery.
  Document the chosen value and its effect on occupied worker slots and shutdown.
- [x] Give remaining uploads and seal retries one explicit deadline. Bound each
  attempt and backoff by the remaining budget; avoid nested policies whose
  advertised retry window cannot fit. Validate configuration relationships.
- [x] Retry transient seal transport failures with bounded backoff and jitter.
  Reuse the same version, truncation, and authenticated-manifest identity.
  Authentication, proof mismatches, and permanent conflicts must fail closed;
  retry only explicitly classified transient conditions.
- [x] Reconcile lost acknowledgements through freshly authorized, identical
  seal retries. Test server commit followed by a dropped response and concurrent
  identical requests. Different proof or truncation inputs must conflict.
- [x] Keep settlement before terminal publication for this phase. Do not add
  detached work or extend execution-token permissions as a shortcut.

## Phase 2 Expose log delivery state

- [x] Define per-stream finalizing, complete, and incomplete or failed outcomes,
  with bounded reason codes. Reuse artifact lifecycle and existing capture
  metadata where sufficient; add persisted fields only where recovery or
  accurate reader behavior requires them.
- [x] Preserve action success when its process succeeds, while making log
  delivery failure visible on execution details. Review whether additional
  operational alerting is needed for repeated failures.
- [x] Return a stable incomplete-log response distinguishable from authorization
  failure without disclosing metadata to unauthorized callers. Update finite
  downloads, execution SSE, artifact previews, CLI consumers, and web UI together.
- [x] Align SSE terminal grace and reconnect behavior with the finalization
  contract. Never emit successful EOF for an unsealed stream.
- [x] Correct premature download-success auditing and define its meaning for
  streamed responses; do not claim verified delivery before it occurs.
- [x] Expose API counters for finalizing/incomplete responses and structured worker
  retry/deadline timing diagnostics; log observed pending age. Avoid unbounded ID
  labels. Dedicated worker metrics and operational alert thresholds can follow
  measured deployment rates; this PR does not add an alerting policy.

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

Current disposition (2026-11-10): **partial**. PR32 implements durable object
prepare, storage-only verification and freshly authorized atomic publication.
It does not settle outstanding physical PUTs, remove shared-file locks, or prove
deployment performance. The following design requirements remain the basis for
those follow-ups; do not treat this whole phase as unimplemented or complete.

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

- Use controlled transports and readiness barriers to test upload and seal
  delays longer than the old 1-second deadline that succeed within the new
  budget. Use deterministic clocks where possible.
- Cover transient failures, retry exhaustion, lost acknowledgements,
  concurrent identical seals, conflicting manifests, and unknown commit outcomes.
- Preserve process cancellation, runtime timeout, post-exit cancellation,
  bounded pipe drain, and joined background task teardown.
- Exercise silent output, stdout only, stderr only, both streams, truncation,
  encrypted frame boundaries, and independent failure of either stream.
- Run repository/API tests with real disposable PostgreSQL and versioned
  object storage for commit, settlement, and reader contracts. Cover shared-file
  transport separately. Follow [runtime log verification](../deployment/runtime-log-verification.md).
- Verify revocation, expired authority, stale attempts, worker reassignment,
  changed owner bindings, retention races, and invalid or incomplete proofs still
  deny publication and plaintext delivery.
- Follow-up phase 3: for durable recovery, inject crashes before and after intent persistence,
  final upload, seal commit, acknowledgement, takeover, and cleanup. Require
  stale owners to be fenced and outstanding obligations to remain owned.
- Add UI and API contract cases distinguishing incomplete logs from denied
  access, preserving exact version selection and successful empty-stream behavior.
- Follow-up phase 4 deployment benchmark: repeat identical serial and concurrent workloads with failures included
  in the denominator. Report incomplete rates, process latency, finalization
  latency, worker occupancy, memory, retries, pool waits, and lock hold times.
  Require no incomplete logs for injected transient faults that recover within
  the configured budget, and explicit bounded failure beyond it.
- Follow [running tests](../testing/running-tests.md) for affected crate,
  formatting, Clippy, web, generated-client, and SQLx checks as applicable.
  Required CI must pass before merge readiness.

## Delivery sequence

1. Instrumentation, coherent deadlines, seal retries, and focused failure tests.
2. Delivery state, reader/UI errors, audit semantics, and contract validation.
3. Reviewed durable recovery design followed by schema, repository, worker,
   supervisor, retention, and crash-test changes as one coherent protocol.
4. PR32's object verification split is integrated for fresh instances with its
   documented physical-settlement exclusions. Complete settlement/cleanup and
   shared-file protocols before removing their remaining fences; measure each
   subsequent optimization against equal workloads.

Implement and validate on disposable infrastructure first. Production configuration
changes and application rollout require separate deployment authorization.
