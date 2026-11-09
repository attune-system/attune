# Object-log durable close protocol

Status: implementation checkpoint, work in progress; final review and CI required.

The production `ObjectSegments` seal route now uses durable prepare, storage-only
verification, and a freshly authorized final transaction. Shared-file seal keeps
its transaction, snapshot and flock. The paused-provider test is the required
proof of released database ownership; until it passes on the checkpoint source,
this document records implementation structure rather than measured lock removal.

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
cross-table constraints. No session bypass exists. Identical published retries
repeat current authorization and compare the selected input and verified result.

Each object request runs in an owned task so waiter cancellation cannot drop an
active phase. Each database phase owns a detached socket, local statement/lock
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
Real successful COMMIT with lost acknowledgement, mixed eligibility-error scheduling,
broader consumers and final CI remain acceptance requirements until exercised.
