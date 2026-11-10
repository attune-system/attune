# Object-log durable close protocol

Status: implementation checkpoint, work in progress; final review and CI required.

The production `ObjectSegments` seal route now uses durable prepare, storage-only
verification, and a freshly authorized final transaction. Shared-file seal keeps
its transaction, snapshot and flock. Actual-route paused-provider tests passed on
PG16 and PG18 with pool size one: an unrelated query and shared/exclusive graph
work finish before GET resumes. This establishes released database ownership for
that tested path; it does not establish PUT settlement or every authority race.

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
cross-table constraints. Forward guards 007–011 prevent generic publication bypass,
bind recorded final evidence, and preserve PR31 operational `log_state`/`log_failure`
updates without allowing source/proof/body/binding replacement. No session bypass exists. Identical published retries
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
eligibility-error scheduling, broader producers/final waits, full packaged runtime
validation and final CI remain acceptance requirements. See the later successful
COMMIT acceptance checkpoint in the central implementation record for exact source,
initial fixture failures and explicit resource cleanup.

Runtime evidence: API 25/25 and verifier 4/4 on each PG version; close lifecycle
9/9 on each; full storage PG16 39/40, PG18 40/40; artifact 8/8, wire types 1/1,
workflow log 30/30 on each. Four workers, zero ignored. The PG16 bounded pending
claim failed its existing 250 ms budget inside the version guard; it remains
unclassified, without a quiet-rerun baseline claim or timeout change. See the
central implementation record for exact source attribution and residual gaps.
