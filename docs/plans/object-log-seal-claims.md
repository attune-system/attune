# Object-log durable close protocol

Status: implemented and reviewed for fresh-instance rollout; final record-head CI pending.

The production `ObjectSegments` seal route now uses durable prepare, storage-only
verification, and a freshly authorized final transaction. Shared-file seal keeps
its transaction, snapshot and flock. Actual-route paused-provider tests passed on
PG16 and PG18 with pool size one: an unrelated query and shared/exclusive graph
work finish before GET resumes. This establishes released database ownership for
that tested path; it does not establish PUT settlement or every authority race.

## Rollout boundary

This protocol's supported deployment is a fresh instance. It is not an in-place
transition for pre-close Ready/sealed object logs. Provision an empty database
with fresh managed artifact/blob/log storage namespaces, an isolated queue/vhost,
and fresh instance signing credentials; quiesce old producers before cutover.
Reusing old numeric-ID storage or authorization/queue state with a reset database
is not this boundary. Fresh provisioning is a separate explicit deployment
operation; migrations and this PR do not reset existing data.

Before these migrations, a legitimately sealed object log can have Ready body,
close_revision=0 and close_phase=NULL. Reads and get-or-create may still work,
and whole-version cascading deletion may succeed subject to its other fences.
However, seal replay cannot select the already-Ready/sealed input, and deferred
publication consistency rejects retained-row updates on that unselected final
state, including operational log metadata updates or a committed deleting state.
Pending-only initialization fixtures and selected-publication retries do not
establish a data-preserving upgrade for those rows.

An existing-database rollout requires a separate approved, data-preserving
transition with pre-close Ready/sealed fixtures. Do not synthesize historical
producer/proof bindings or verified-close evidence to bypass that requirement.
The fresh-instance scope retains all current protocol guards.

`log_source::Source` acquires graph/source protection before artifact/version/stream
locks. Prepare binds current producer identity, authenticated manifest and caps,
requested truncation, exact contiguous accepted membership and current frontier.
The database requires Pending/unsealed state, no cleanup ownership, ready exact
provider ledger entries and no currently settling issued grant. It stores canonical
JSONB input/hash, monotonic revision, deadline and the closing pin. An acknowledged
commit is required before GET. Historical data contains no credential or authority.

`object_close::Prepared` owns the selected input, revision/hash, immutable stream,
segments and historical source/proof binding. Only its verifier constructs the
private `Verified` publication value, combining those inputs with `VerifiedBytes`.
The verifier accepts storage and immutable crypto inputs only. It consumes pinned
readers through EOF, checks lengths/digests and complete encrypted frame/manifest
identity. It has no AppState, transaction, pool or mutation proof.

Finalization acquires a new Source, repeats current authority after target waits,
and compares history and exact durable selection. A revision/deadline mismatch
cannot publish. Fresh encrypted evidence/metadata, sealed/truncated state, verified
size/digest, Ready and published close state commit atomically. Migration 006
replaces the selected-publication veto with permanent identity guards and deferred
cross-table constraints. Forward guards 007–012 prevent generic publication bypass,
bind recorded final evidence, and preserve PR31 operational `log_state`/`log_failure`
updates without allowing source/proof/body/binding replacement. No session bypass exists. Identical selected-publication retries
repeat current authorization and compare the selected input and verified result.

Each object request runs in an owned task so waiter cancellation cannot drop an
active phase. Each database phase owns a pool-accounted socket marked for disposal, local statement/lock
limits and a 15-second outer budget; uncertain cancellation disposes that socket.
Helper errors await rollback within the phase. Commit acknowledgement errors use
503 and one fresh authorized reconciliation attempt. A successful commit followed
by pure freshness failure is distinct and does not enter that retry path. Remaining
uncertainty returns an error without physical mutation or pin release.

Expired closing rows remain pinned. Object housekeeping discovers an indexed,
bounded page, enters existing bounded exclusive graph admission, and abandons only
matching revision/deadline rows. Abandonment increments revision; authorized resume
uses the same immutable input, rechecks membership/ledger/cleanup and verifies again.
Permanent closure survives abandonment and standalone stream deletion is forbidden
while the version survives. No execution/admission/enforcement/event retention FK,
fabricated evidence owner, universal graph-before-cache rule or generic scheduler
is introduced.

Scope A does not settle PUTs. Delayed pre-admitted PUTs may still start after
closure; accepted membership cannot grow and retained keys keep create-only
protection. Full settlement, forgotten staging, post-retention recreation and
filesystem path retirement remain separate work. Shared-file/flock and
PhysicalCleanup/native filesystem fences remain in place.

Validation and unresolved cases are recorded in the central implementation record
and test assumptions. Prior verifier/source test results do not certify this route.
Real successful COMMIT with lost acknowledgement now has actual recorded
worker-delivery HTTP coverage at both prepare and final publication on PG16/18,
including revocation, forwarded-ACK expiry and two lost prepare ACKs. Mixed
eligibility-error scheduling, full packaged runtime validation and final CI remain
acceptance requirements. The finite seven-case native/sensor/recorded-delivery
producer and final-target-wait matrix now passes on each PG16/18: actual terminal
native and current sensor HTTP publication, sensor release/reacquire and worker
admission/attempt changes during GET, and observed target waits crossing actual
JWT/sensor-lease expiry with403 and retained selection. Same-producer renewed
worker credentials recover without resetting history. This does not certify every
lifecycle transition. The existing ACK harness's three P2 failure-path/relay/drain
review findings remain pending root's follow-up; no general review approval is
claimed. See the later successful
COMMIT acceptance checkpoint in the central implementation record for exact source,
initial fixture failures and explicit resource cleanup.

Runtime evidence: API 25/25 and verifier 4/4 on each PG version; close lifecycle
9/9 on each; full storage PG16 39/40, PG18 40/40; artifact 8/8, wire types 1/1,
workflow log 30/30 on each. Four workers, zero ignored. The PG16 bounded pending
claim failed its existing 250 ms budget inside the version guard; it remains
unclassified, without a quiet-rerun baseline claim or timeout change. See the
central implementation record for exact source attribution and residual gaps.
