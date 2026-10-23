# Protected HTTP disclosure boundary

## What the API controls

For the standard Axum/Hyper server, the body disclosure boundary is the guarded
body returning `Poll::Ready(Some(frame))` to Hyper. The header boundary is the
route returning its prepared response to Hyper. These are application handoffs,
not physical socket writes.

`recorded_reads::guarded_chunks` first awaits the next decoder/blob/channel item.
It then awaits a fresh authorization transaction. That transaction follows
source validation, capture admission and actual target/pin checks in that order.
The retained approval fence is checked after the transaction's final target and
origin queries, before commit. A synchronous retained-deadline and JWT check
also follows the commit wait on recorded log/download paths. There is no further
application await before yielding the item. A stream resumed by Hyper after
backpressure performs this sequence again for its next item. Successful EOF has
its own authorization call. The finite/live SSE adapters turn only checked EOF
into Done, with no additional await between the checked result and the event.

The authorization decision serializes with source and target writers through
the checked transaction. This does not make a database transaction and an HTTP
write atomic. Ordinary-human paths rebuild their current authorization and
origin projection after the decoder wait, then check identity/JWT liveness
before returning. They do not substitute the original actor's mutable IAM for a
recorded credential or contributor's retained input-purpose authority.

HEAD has a separate final route check after response preparation. It never
depends on the guarded body's first poll: Hyper may never poll that body. The
recorded download guard and human sealed-log guard invoke the same authorization
closure for headers and subsequent body/EOF polls. Existing log versions on the
execution SSE route are pinned and their complete frontier authorized before
headers are returned. A missing log allocation returns only the authorized
execution's waiting response, not content from an unchecked future allocation.
Sensor log downloads likewise check headers and successful EOF through the
shared body guard.

## What the API does not control

Hyper owns HTTP framing, connection flow control and socket readiness after a
frame has been handed off. TLS, kernel queues, intermediaries and clients may
buffer those bytes too. This Body API has no callback that can atomically run a
database check at the later kernel write. It provides no retroactive revocation
of previously authorized, handed-off bytes.

This is not a token grace period. New unyielded items must pass fresh source,
target, origin and expiry checks after their last application read/queue wait.
An item waiting in the decoder when revocation or expiry takes effect is denied
when that wait completes. The decoder has bounded frames and a one-slot channel;
F's separate shared-delivery follow-up supplies the 64 KiB file chunk limit and
must be preserved during integration. Those application bounds do not claim a universal byte or time bound for queues
owned by Hyper, the kernel, proxies or clients.

The notifier has a different interface. Its unsplit WebSocket checks actual
`Sink::poll_ready`, then validates authority and calls `start_send` without a
further await. Keep that stronger socket-facing check. Do not replace it with
HTTP Body polling or split-sink intermediary readiness.

## Regression sources, not passing acceptance

`crates/api/tests/recorded_execution_events_tests/worker_live_streams.rs` uses
F's actual Worker completion allocator, registered context, bound permit and
transport. Shared-file appends use F's guarded native append path; object appends
and both final seals use the actual HTTP routes. Tests check both live readers,
immutable frontier revisions, accepted input-purpose receipts, later SDK Key and
generic reads, exact foreign approvals, revocation, closure, final proof and
expiry behind real blob-reader waits, including empty-plaintext EOF.

`http_handoff.rs` drives the stock router over actual TCP with small owned socket
buffers and a paused reader. A synchronous test-only Body observer counts
handoffs. Tests allow earlier authorized queued prefix bytes after revocation,
deny a later frame created after revocation, and check cancellation releases the
stream lease. HEAD controls hold an actual version lock and verify positive and
expired responses with zero Hyper body polls. These observations do not claim
database checks at kernel write time or erase bytes already handed off.

These additions have not been compiled or run. Only source/static checks are
authorized in C's isolated worktree. Combined PG18, backend, timing and memory
acceptance remains required; readiness and issuance gates remain false.

F owns evidence retention and cleanup. This document does not implement that
protocol or close the retention findings in `file-origin-retention-audit.md`.
C must consume F's published retention protocol when it is available and retain
F's protected-file codec, chunk/EOF/HEAD hooks and quarantine behavior.
