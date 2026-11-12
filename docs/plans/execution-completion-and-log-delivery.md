# Execution completion and asynchronous log delivery

Status: proposed implementation plan, 2026-11-12. Runtime behavior is unchanged.
This plan extends the existing
[worker lifecycle plan](worker-execution-lifecycle-performance.md), whose current
contract requires log settlement before terminal publication.

## Objective and completion semantics

Report the action's process duration accurately and publish its terminal result
without waiting for remote log uploads, verification, or sealing. Clients must
expect a completed execution to have logs that are still finalizing.

Execution completion means the process outcome and bounded output required for
its result have been captured, validated, classified for secret disclosure, and
persisted. It also means ownership of remaining log work has been recorded.
Log readiness means the selected immutable version has completed authorized
verification and publication. These are independent milestones.

The first asynchronous implementation targets object-segment logs backed by S3,
including RustFS. Shared-file delivery retains its existing synchronous ordering
until its separate ownership and physical-settlement design is validated. Both
backends expose the same client-facing availability contract.

This changes completion semantics and post-completion delivery authority. The
[recovery proposal](runtime-log-worker-loss-recovery.md) supplies related custody
and revocation requirements, but its first slice only covers fully selected
recorded-action closes. It does not already authorize this broader delegated
worker handoff. Complete that protocol review before enabling the new path.

## Current dependencies to replace

- `crates/worker/src/runtime/process_executor.rs`: `capture_output` drains pipes
  and seals a stream in the same task. The executor joins both tasks before
  returning; its reported `duration_ms` includes remote log finalization.
- `crates/worker/src/executor.rs`: output classification and terminal persistence
  run after that return. Completion notifications and workflow/queue progress
  follow the persisted terminal row.
- `crates/common/src/repositories/delegated_execution.rs`: worker log-source
  validation requires the assigned open attempt and `execution.status=running`.
  Completion closes the delegated attempt. Existing worker-delivery permits are
  a useful narrower pattern, not automatic authority for delegated recovery.
- Finite artifact reads distinguish `LOG_FINALIZING` and `LOG_INCOMPLETE`, but
  terminal execution state currently contributes to the incomplete diagnosis.
  Execution log SSE also applies a two-second terminal grace period. Both rules
  must change before earlier terminal publication is enabled.

## Separate timing and state

Stop the monotonic process timer when the child wait resolves, including timeout
or cancellation termination. Track spawn/stdin/wait, pipe capture, durable
handoff, remote finalization, and terminal persistence separately. Preserve wall
clock timestamps for externally visible lifecycle milestones. A process that
never launched has no invented process duration.

Revise the canonical v1 timing contract so `result.duration_ms` for process
runtimes measures process execution rather than remote log settlement. Preserve
`started_at` and `completed_at` as execution lifecycle timestamps; their difference
may include preparation, bounded capture, and result persistence. Document and
update UI, analytics, CLI, and API consumers that currently conflate them. Keep
workflow orchestration elapsed time meaningful; workflows are not OS processes.

Keep existing per-stream `runtime_log_capture` evidence (`empty`, `captured`,
`incomplete`, `unknown`) and distinguish it from delivery availability. Audit all
consumers: captured bytes alone must not imply a readable sealed object.
Expose a typed, authorized per-stream delivery projection rather than rewriting
the terminal execution result JSON whenever background work progresses.

| Delivery projection | Meaning | Client behavior |
| --- | --- | --- |
| `empty` | Positive EOF evidence proves that this stream produced no bytes. | Render empty output immediately; allocate no log artifact. |
| `finalizing` | Log bytes remain under an active, bounded delivery obligation. | Show pending availability and continue waiting or reconnecting. |
| `ready` | Exact version is verified, sealed, and published Ready. | Fetch or finish consuming that immutable version. |
| `incomplete` | Capture or delivery definitively failed, was fenced, or exhausted its deadline. | Explain the log failure independently of the action outcome; preserve any authorized partial preview policy. |
| `unknown` | No sufficient capture or delivery evidence exists. | Refresh discovery; never interpret missing evidence as empty or ready. |

The projection includes available immutable version/stream IDs, a revision,
`finalize_by`, and a bounded retry hint. It is derived from canonical capture,
delivery, and publication records. `finalizing` does not set `logs_incomplete`.
Later delivery failure changes log availability without changing the persisted
action outcome or republishing an execution-completed event.

## Worker completion sequence

1. Observe child exit and freeze its process duration and exit outcome. Preserve
   ownership of process descendants and capture until their bounded cleanup ends.
2. Drain stdout and stderr into bounded local capture independently of remote
   sealing. Preserve the existing bounded post-exit pipe drain and descendant
   cleanup. Missing EOF remains incomplete capture. Parse structured results and
   prepare the pinned output-schema validation and redaction. Final origin-based
   classification is fenced with terminal publication below.
3. Transfer remaining bytes and final proof material to explicitly owned delivery
   custody. Construct the exact immutable log/version identity, final byte and
   sequence limits, truncation decision, and authenticated manifest. Persist the
   delivery intent and its custody reference before releasing the producer.
4. Under the attempt fence, end the action's general API access, collect final
   secret origins, and finish output classification. Atomically publish the
   terminal execution result and its link to the accepted delivery intents.
   Use the normal assignment/attempt and cancellation checks.
   A cancellation that already won the terminal transition cannot be overwritten
   by a delayed success. Completion notification follows the committed row.
5. Release the action execution slot after the handoff. A separately bounded,
   worker-owned finalizer uploads and seals the selected logs. Background work
   updates only delivery/publication state and emits log-availability changes.

The capture refactor must separate pipe draining, result capture, and remote
writer settlement. Moving the existing combined task into `tokio::spawn` would
still entangle those responsibilities and lose ownership on shutdown. Bound
both queued bytes and finalizer concurrency; never let a slow object store create
unbounded tasks, memory, disk use, or database sessions. Apply admission/backpressure
when capacity is exhausted and report the affected log outcome explicitly. If
the process has already exited and custody cannot be accepted within its budget,
persist the classified action outcome with explicit incomplete-log evidence;
never leave the action Running indefinitely or claim delivery was accepted.

Nonzero exits, timeouts, cancellation, silent actions, stderr-only output, and
partial capture use the same separation. A later log-storage failure cannot turn
a committed successful execution into a failed action. A captured empty sibling
remains empty when the other stream fails.

## Delivery custody and authorization

Add one repository-owned per-stream delivery intent for work that outlives the
execution. Use BIGINT primary IDs and explicit selected columns. Bind it uniquely
to the immutable version/stream and original execution attempt. Keep the existing
close selection/publication protocol responsible for verified object readiness;
a queued intent alone cannot mark a version Ready or bypass its constraints.

An intent records original producer and active accountable owner, selected
admission/policy dependencies, byte/sequence/proof bounds, custody location,
state/revision, finalization deadline, claim owner/epoch, and sticky revocation.
Never persist bearer tokens, presigned URLs, raw keys, or plaintext secret logs in
intent metadata, audit events, or diagnostics. Protect spool contents according
to the admitted output policy; protected channels require encrypted custody.

For bytes not yet accepted remotely, define a durable spool/ownership handoff.
Fsync the owned bytes and journal before recording their database reference;
reconcile orphaned spool entries if the database transaction does not commit.
Do not pretend filesystem and PostgreSQL commits are atomic. Recoverable custody
must meet the declared failure model: a local persistent spool can survive a
worker-process restart, but does not establish survival of node or volume loss.
If custody is lost, converge to `incomplete` rather than fabricate a successful
seal. Admission policy must state which failures the enabled deployment tolerates.

Authenticate finalization through a private operation scoped to the already
admitted versions and original attempt. Apply `RequireAuth` to any protected
HTTP entry point; knowing an intent or stream ID never authorizes publication. Terminal execution state permits that
specific handoff to continue; it grants no new stream allocation, arbitrary
append, result mutation, Key reads, child creation, or general execution API
access. Normal worker/action credentials expire or close as they do today.

Apply the recovery proposal's mutation matrix to the delegated path explicitly:
owner freeze/deletion/rebinding, policy or approval revocation/expiry, execution
retry/reassignment, worker-instance replacement, artifact deletion, and independent
retention of source evidence. Missing authority fails closed. Cancellation is
sticky; restoring an owner or adding a grant cannot revive an invalidated intent.
The first slice conservatively fences an old intent after attempt/assignment
replacement. Worker restart recovery requires an explicit, authenticated claim
transfer policy, not reuse of an old worker-instance identity.

Use short repository transactions to claim and acknowledge work. Bound storage
operations and retries without retaining a database checkout or lock while
waiting. Preserve current graph/source/target lock ordering, exact-frontier seal
verification, acknowledged cleanup, retention pins, SQLx session checks, and
origin-based disclosure. Runtime references retain independent-retention and
dangling-ID semantics; do not add execution/admission retention foreign keys.

Keep the existing 60-second finalization budget as the initial proposed hard
bound, measured from process exit, subject to any earlier security deadline.
Publish the effective deadline rather than making clients guess it. An expired
worker JWT is different from expired security approval. Shutdown stops admission,
drains owned finalizers within its budget, then leaves recoverable custody or an
explicit incomplete outcome. A tracked in-memory task alone is not crash recovery.

## API and client behavior while logs are delayed

### Discovery and finite downloads

Authorize the exact requested content before exposing delivery state. A terminal
execution with an active delivery intent returns `LOG_FINALIZING`, not
`LOG_INCOMPLETE`. Reuse the existing HTTP 409 diagnostic, add `Retry-After` and
allowlisted typed timing/state fields, and make pending responses non-cacheable.
A verified Ready publication wins races with a stale pending selection after
fresh authorization. Existing download-success audits still require full EOF.

Execution-scoped discovery must report finalizing even before a public artifact
listing shows the version. Artifact 404, an empty list, and execution completion
alone are insufficient evidence of silence. Retry through authorized execution
log discovery; a real 404/denial is not broadly converted to a delivery success.
Select and retain immutable version IDs once available. Never retry an action's
mutable latest-log selector and accidentally display a later execution's output.

### Live tails and notification loss

Remove the two-second terminal-state cutoff for a stream with an active intent.
Continue SSE through execution completion and emit `done` only after verified
sealed EOF, or positive evidence of an empty stream. Pending delivery can emit a
typed finalizing status/keepalive. Definitive failure emits the existing
`log_stream_incomplete` outcome; a deadline that requires reconciliation causes
a fresh canonical-state read rather than an invented failure or success.

On reconnect, use the same immutable stream/version and `Last-Event-ID` cursor.
Deduplicate replayed bytes. Reauthorize each continuation and preserve disclosure
checks. Availability notifications are hints: periodic bounded reconciliation
must recover after a missed notification, reconnect gap, API replica change, or
out-of-order execution-completed and log-ready messages.

### Web UI

Display the action as completed promptly, with its process duration and result.
Keep an independent per-stream “Logs finalizing” indicator while the log tab is
waiting. Stop execution-status polling when appropriate, but retain the log
subscription or availability retry loop until each selected stream settles.

Start fallback discovery retries around 500 ms, apply jitter and exponential
backoff capped at 2 seconds, and honor a server-provided retry hint within the
client budget. Bound automatic waiting by the advertised finalization deadline
and a small transport allowance. After that bound, refresh canonical state and
show pending/unavailable with a manual retry if reconciliation is unavailable;
client timeout alone is not evidence that logs were lost. Cancel requests and
retry timers on navigation, unmount, logout, or explicit user cancellation.

Render empty streams immediately. Show an incomplete-log notice independently
of the completed/failed action badge. A transient `LOG_FINALIZING` must not become
a permanent error panel, download failure toast, negative cache entry, or blank
output success. Keep prior authorized bytes during reconnect and append once.
Do not display arbitrary API error bodies or disclose existence after a 401/403;
refresh authentication only through the normal flow and stop on definitive denial.

### CLI and MCP clients

Execution wait/run APIs resolve on the persisted terminal execution outcome.
Explicit log-follow or download operations wait independently for log readiness,
with bounded timeout and cancellation controls. Offer separate execution-wait
and log-wait budgets. Preserve the action exit code in run output while exposing
log delivery failure as a distinct structured outcome. An explicitly requested
log download can fail without rerunning the completed action.

MCP execution results expose the process result plus log availability and stable
references; later log retrieval may return a typed retryable finalizing response.
Update supported SDK/example consumers and generated clients to the same v1
contract. Document that execution-completed notifications guarantee result
availability, while log-ready notifications guarantee log availability.

## Implementation phases and completion gates

| Phase | Deliverable | Gate before proceeding |
| --- | --- | --- |
| 1 | Separate process timing from capture/settlement; add typed phase metrics. | Fast exit, stdin broken pipe, nonzero exit, timeout, cancellation, and descendant-pipe tests establish timer boundaries and preserve outcomes. |
| 2 | Specify and implement capture split, durable custody, scoped handoff, claim/revocation, and per-stream delivery projection. | Review the delegated authority and crash matrices; prove bounded resources, encrypted custody, atomic terminal/intent linkage, and idempotent recovery/publication. Keep earlier terminal publication disabled. |
| 3 | Update API readers, SSE, discovery, notifications, UI, CLI, MCP, and v1 generated contracts. | Delayed logs remain finalizing after terminal state, survive reconnect/lost notifications, and never appear empty or permanently failed merely because the action completed. |
| 4 | Enable earlier terminal publication for object logs under an explicit rollout setting. | Fault tests block storage beyond the old two-second grace and demonstrate completed results, valid scoped finalization, eventual authorized log availability, and bounded worker/finalizer capacity. Shared-file behavior stays protected. |
| 5 | Run paired release-build acceptance and prepare coordinated rollout. | Required CI and affected security/client contracts pass. Record enabled backends, effective deadlines, custody failure model, and rollback/drain procedure. |

Update Rust DTOs, OpenAPI, generated web API code, exports, and relevant
CLI/MCP schemas together. Keep initial formats v1 and revise their canonical
contract in place. Apply reviewed schema changes through the deployment's
migration policy; no automatic database reset, checksum rewrite, legacy adapter,
or relaxed authorization is part of enablement. Update AGENTS.md if the new
completion/log-delivery invariant becomes established runtime behavior.

## Validation and operational acceptance

Use owned disposable PostgreSQL 16 and 18 plus versioned RustFS and the normal
runners from [Running tests](../testing/running-tests.md). Keep compilation out
of timing-sensitive runs. Extend the existing worker lifecycle, staged-close,
artifact header/content, runtime-log replica, CLI/MCP disclosure, and web log tests.

- Gate process exit, pipe EOF, handoff acknowledgement, terminal commit, upload,
  selected-close verification, and Ready publication independently. Freeze the
  seal for 5–10 seconds: the action must become terminal, return a classified
  result, release its action slot, and keep logs finalizing throughout the wait.
- Exercise silence without artifacts, stderr-only and two-stream output,
  structured/secret output, truncation, slow readers, and pipe-holding descendants.
  A dependent workflow may use the classified result immediately; an explicit
  artifact/log dependency waits for availability rather than reexecuting the task.
- Test crash/restart before and after spool fsync, handoff, ambiguous terminal
  commit, final upload, unknown seal acknowledgement, and final publication.
  Reconcile the exact intent/version without duplicate result completion, lost
  claimed-durable bytes, or invented log evidence. Cover node/volume loss against
  the declared custody guarantee.
- Test stale attempts, reassignment, owner/policy/approval revocation and expiry,
  deleted source evidence, sibling streams, retention and cleanup races, queue
  exhaustion, disk-full conditions, storage outage, and bounded graceful shutdown.
- Test immediate client reads after terminal notification, finalizing longer than
  two seconds, late publication at the deadline, missed/out-of-order notifications,
  cross-replica reconnect, cursor replay, stale selection, auth expiry/denial, and
  user navigation. Assert no duplicate bytes, premature SSE EOF, empty success,
  retry storm, or action reexecution.
- Repeat the one-second `core.echo` rule with RustFS after warm-up. Measure exact
  PostgreSQL Running/Completed transitions, monotonic process duration, handoff
  cost, log-ready delay, slot occupancy, pool waits, and API backend turnover.
  Retain the complete per-execution dataset rather than inferring latency from
  throughput. Use identical release builds, workloads, storage and observation
  overhead for the baseline and candidate. The earlier debug run motivates this
  work; it is not a paired performance acceptance baseline.

Proposed warm-echo completion target: p95 Running-to-Completed at or below 250 ms
on the declared local acceptance environment, alongside correct process timing
and prompt result availability. If preparation or metadata persistence prevents
that target, profile and report the remaining gap. Never omit capture/security
work or reduce the test workload to claim a speedup. Independently prove that a
blocked seal does not delay execution completion or cause API connection churn.

Export bounded-label metrics for process duration, pipe drain, handoff latency,
execution terminal latency, delivery queue bytes/depth/age, active finalizers,
ready delay, retries, incomplete/revoked intents, custody loss, and pool rejection.
Alert on stuck finalizing logs, recovery backlog, repeated loss, and exhausted
custody capacity. Exclude request IDs, SQL, log bodies, bearer data, and raw secrets
from metric labels and audit details.

Enable consumers before enabling earlier terminal publication. On rollback,
stop new asynchronous handoffs and drain or explicitly settle existing intents;
do not strand them by reverting to the old two-second reader rule or revoking
finalizer custody while work remains outstanding.
