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

1. Typed producer/proof projection derived through the actual workflow, sensor,
   legacy and recorded-worker authorization helpers. The low-level selection
   repository currently accepts server JSON projections and is not a production
   authorization boundary. No route invokes it.
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

Activation remains blocked by unimplemented fresh-source ownership and atomic
final publication, rather than by provider verification. In particular, sensor
fence validation in authentication middleware is not a final-transaction fence;
`authorize_version_mutation` has no sensor-generation branch. Native workflow
`require_native_log` checks workflow existence without locking that source row.
Simply calling the existing helper again does not establish all requested source
checks after target waits. These must be corrected and tested before introducing
the finalizer or replacing the selected-close SQL publication vetoes. No claim is
made that these gaps require a new authority format or retention FK. Typed actual
producer projections, prepare/final commit reconciliation, publication consistency,
revision/deadline recovery and the requested route races remain unimplemented.
