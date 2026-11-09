# Object-log durable close protocol

Status: storage guard checkpoint, **incomplete foundation; not approved for route activation**.
The production seal route still authorizes, verifies storage and publishes in its
existing long transaction. This checkpoint does not remove its I/O lock.

## Scope and state

Use private fields on `log_stream`, never a new public log format or a generic
seal scheduler. `close_phase` is absent for open streams, then `closing`,
`abandoned`, or ultimately `published`. `close_revision` is monotonic. The deadline
fences verification ownership; expiration does not release the pin. Abandonment
increments the revision and releases only the temporary verification pin. Every
selected phase permanently forbids append and standalone stream removal while
the version survives. Allowed version teardown may remove abandoned/published
state; closing vetoes version and parent cleanup, including origin-less logs.

The internal canonical v1 input is PostgreSQL JSONB text, hashed as UTF-8 SHA256.
Its snapshot contains artifact/version/stream/backend/frontier and every segment
row in sequence order, including segment ID, byte range, size, digest, key,
provider fingerprint and creation identity. Producer and authenticated proof
projections and requested truncation are also bound. They are server projections,
never client assertions or manufactured worker permits. Selected identity cannot
change in any phase. Resume requires the same input, an expected revision,
current Pending/unsealed membership, no cleanup, ready exact-provider ledger
entries and no currently settling issued grants, then increments the revision.
The eventual authorized owner must also repeat current producer authorization.

## Implemented guard boundary

Migration `20261109000003` enforces accepted segment immutability, stream-serialized
membership, durable close identity, revision transitions, direct origin-less
cleanup guards, evidence eligibility and cleanup-request exclusions, and exact
provider ledger protection. The ledger retry candidate exclusion is outside both
the new-deletion and already-deleting branches. Selection rejects committed
physical deletion ownership. No execution/admission retention FK is introduced.

The repository provides opaque selected-input projection, revision-qualified
selection/resume and abandon, and an append/retry guard. Direct issuance, proxy
reservation, direct completion (including its completed-grant fast path), and
proxy publication use that guard after stream locking. SQL grant guards provide
additional consistency enforcement. Forward migration `20261109000004` allows
only identity-preserving `issued → expired` retirement after the existing settlement
deadline without acquiring the stream lock. This avoids the maintenance grant-first
versus producer stream-first inversion and permits expiry through closing and
abandonment. It neither reopens admission nor proves physical PUT settlement. Exact accepted identity retries are allowed;
new membership or a changed provider is rejected. Existing proxy reservation
short-circuits an exact retry before PUT. Earlier admitted requests may still
**start a PUT after closure**: neither a second database check nor a grant TTL
proves physical settlement. No database lock is added across PUT.

Selected-close publication is deliberately fail-closed in this checkpoint:
generic seal, Ready/body replacement and final-manifest INSERT cannot publish a
selected close. There is no enabled transition to `published` yet. A Rust private
type alone would not protect these SQL surfaces. The normal production route
continues to handle streams without selected close state.

## Required completion before route activation

The foundation is incomplete until the following are implemented and reviewed:

1. Connect the staged typed producer/proof projection described below to durable
   prepare/final ownership. The low-level selection repository still accepts
   server JSON projections and is not a production authorization boundary.
   No route invokes selected close preparation or the staged source owner.
2. Storage-only verification accepting storage and immutable inputs, with no
   AppState, pool, connection, transaction or old mutation effect. Consume full
   segment bytes through EOF, preserve concatenated SHA256, exact pinned provider
   reads and encrypted frame verification across segments and final manifest.
3. A private verified-result constructor and fresh authorized final owner. Repeat
   actual source authorization, permit/context/admission/attempt/worker-instance,
   credential deadlines, origin/disclosure checks and current delivery caps after
   verification and lock waits. Missing independently retained source authority
   denies publication. An immutable historical binding grants no authority.
4. Revision/deadline/input fencing and atomic final evidence, encrypted metadata,
   seal/truncation, verified size/digest, Ready and published phase. Replace the
   current fail-closed SQL publication guards with atomic consistency guards,
   never a session authorization bypass flag. Reconcile identical concurrent and
   lost-ack retries; unknown prepare/final commit must be reread, never assumed
   rolled back. Add bounded abandon/recovery without reviving old authority.
5. Barrier-based PG16/18 repository/API races, direct/proxy zero-PUT retries,
   membership/provider/proof failures, expired takeover/stale finalization,
   independent execution retention, encrypted EOF/frame failures, unrelated graph
   progress and a size-one pool while verification is paused.

Concrete counterexample preventing activation of this checkpoint: select input
with producer JSON from attempt A, revoke A during GET, then call a hypothetical
finalizer that accepts only that JSON and its digest. It would publish under
revoked authority. No such finalizer is supplied here; selected-close publication
fails instead. Completing the real fresh-source final owner is mandatory before
enabling publication, and route splitting requires a subsequent Astra review.

## Retained boundaries

Shared-file append/seal and PhysicalCleanup/filesystem deletion fences remain
unchanged. Reuse existing graph/cache admission order; no graph-after-target
acquisition, universal graph-before-cache rewrite or lock across provider I/O.
No synthetic file evidence owner/origin is created.

Full log-key PUT obligations and permanent provider write retirement remain a
later protocol. Accepted bytes are protected while retained; delayed duplicate
PUTs, forgotten multipart staging and key recreation after later cleanup remain
unresolved physical settlement limitations. Filesystem path retirement also
requires allocator/publication participation and stable root/alias handling;
a database generation alone cannot fence delayed unlink.

PR31 `c067e88573529466f40849a7b58325a8e4aaa704` is a task-branch dependency,
merged normally with authorship preserved. Its bounded writer retries, independent
stdout/stderr draining/outcomes and verified EOF authorization/audit are retained.

## Storage verifier continuation (still incomplete)

`object_log_verification::verify` now supplies a storage-only full-byte primitive
used by the existing object seal branch. Its arguments contain storage, a stream
snapshot, ordered segment snapshots, and optional crypto-only inputs. No AppState,
SQLx owner, pool or FileMutationProof enters that module. Private result fields
are constructed only after exact-provider full reads reach successful EOF,
per-segment size/digest checks, concatenated hashing, and encrypted frame/manifest
completion. The result is byte evidence, **not a publication capability** and is
not yet bound to a committed SelectedLogClose. Read routes and metrics are unchanged.
The caller still retains the old transaction across the verifier: this change
alone does not reduce the production lock scope.

Forward migration `005` pins the existing snapshot function's TimeZone to UTC.
It does not edit applied migrations or rewrite selected input/hash identities.
Selections made before this migration with non-UTC segment timestamp projections
can still conflict safely; no silent conversion or permanent-closure reset is
provided. The cross-TimeZone test covers new selections with a real segment.

Activation remains blocked by unimplemented durable final ownership and atomic
final publication, rather than by provider verification. In particular, sensor
fence validation in authentication middleware is not a final-transaction fence;
`authorize_version_mutation` has no sensor-generation branch. Native workflow
`require_native_log` checks workflow existence without locking that source row.
Simply calling the existing helper again does not establish all requested source
checks after target waits. These must be corrected and tested before introducing
the finalizer or replacing the selected-close SQL publication vetoes. No claim is
made that these gaps require a new authority format or retention FK. The staged continuation below supplies typed source/proof projections and a fresh
transaction owner. Their integration into durable prepare/final ownership,
commit reconciliation, publication consistency, revision/deadline recovery and
the requested route races remains unimplemented.

## Staged source API (2026-11-09 continuation)

`internal_files::log_source::acquire` enters shared capture and owns the
caller's transaction. `Source::authorize_target` preserves the existing origin,
disclosure, artifact and owner checks; the caller then locks and reloads the
version/stream through `Source::transaction`. `Source::bind` authenticates the
existing v1 manifest, checks current delivery seal bounds, rechecks freshness,
and compares the newly derived binding with an optional historical binding.
Only `HistoricalBinding` crosses a commit: it contains typed producer identity,
version/stream and plain/encrypted proof identity, never a mutation effect,
connection, secret key, bearer credential or authorization deadline. Encrypted
identity includes SHA256 of the authenticated manifest and the typed opened
manifest. Renewed credentials do not enter producer identity.

Supported source classes follow current transport: recorded process (admission,
attempt and delivery worker/instance), recorded worker delivery (admission,
attempt and worker/instance), delegated execution/legacy worker (actual delegated
worker attempt), native workflow (actual workflow row and workflow attempt), and
sensor (sensor plus workload/worker/instance/generation). Legacy workers have no
instance credential to manufacture. Native completion can log after terminal
execution/closed attempt; unlike worker completion, no running/open predicate is
added. Missing authority denies, without creating source or evidence rows/FKs.

Lock order is shared graph first, then source participants, then artifact advisory
and owner/origin protection, then version/stream. Native takes workflow before
selection/authority and execution/attempt, consistent with cancellation and
outbox source writers; selection validation is repeated after execution locking.
Sensors reuse schema selection -> assignment NO KEY UPDATE -> generation,
metadata/admission and schema validation. Recheck the current fence/lease and
metadata after target waits. Recorded paths retain their existing source ordering
and approval/delivery expiry checks. Cache PinMutation may precede shared graph;
this contract does not establish universal graph-before-cache ordering.

The new module's narrow dead-code allowance is intentional staging under AGENTS:
there is no production caller until the durable finalizer is implemented. The
old route composes the extracted source/target checks exactly as before and does
not acquire the new native/sensor locks across GETs. Selected publication still
fails closed. Prepare/final commit reconciliation, revision/input/deadline
ownership, atomic final publication SQL, abandon/recovery, route split and a
subsequent review remain required. Historical identity alone never authorizes
publication. Permanent closure, accepted membership and cleanup protections from
003/004/005 remain unchanged; full PUT settlement and path retirement remain
outside this prerequisite.

`Source::commit` consumes the transaction owner, checks source freshness immediately
before commit and preserves `FileMutationProof`'s pure check after commit waits.
An error from commit is not evidence of rollback; the next durable orchestration
must reconcile state in a new freshly authorized transaction. No old Source or
FileMutationProof is a recovery capability.

Sensor historical identity also retains the actual optional metadata revision,
schema binding ID/digest and log-generation ID/digest. Existing schema and log
binding rows are locked in the source stage after assignment/generation checks.
A bound log generation must match the current generation's schema identity.
After waits, recheck these projections and metadata, then sample the lease clock.
Absence stays absence for genuine legacy sources; losing recorded evidence cannot
silently produce the same historical binding. No missing record is recreated.
