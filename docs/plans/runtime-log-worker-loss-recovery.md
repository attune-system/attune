# Runtime-log recovery after worker loss and physical PUT settlement

Status: proposed design revised after independent source review on 2026-11-10;
no runtime authority, schema, or protocol changes implemented. Recovery remains
disabled. The first-slice policy below is a conservative parent recommendation,
not accepted permission or validated implementation. Audited HEAD:
`820b4192068ccd321167a28dd888276dd7998368` (merged PR32 and PR31 baseline).

## Scope and conclusion

Build two bounded mechanisms: recovery of an already authorized exact completion,
and settlement of every admitted physical write. Neither implies the other.
Recommend implementing API-owned recovery of **newly opted-in, fully selected
object closes** first. Do not start by making all unfinished logs recoverable or
by treating worker disappearance as a completion signal. Worker-local bytes need
an explicit spool durability policy; delayed provider writes need a separate
physical settlement contract. Shared-file/PhysicalCleanup redesign is separate.

This is the recovery proposal referenced by the runtime-log follow-up roadmap. Existing
close history cannot be upgraded into recovery authority by inference. Preserve
the current v1 format and existing source/graph/target lock ordering. No database
reset, deployment, or retroactive invention of producer evidence is part of it.

## What the merged source actually guarantees

Paths below are relative to the repository root; function names identify change
points rather than asserting that the proposed behavior already exists.

| Entry point | Existing guarantee and remaining boundary |
| --- | --- |
| `crates/worker/src/runtime/log_writer.rs`: `BoundedLogFileWriter::seal`, encryption state | Pending plaintext frame, frame totals, encrypted handoff, and final authenticated manifest are process state. A final manifest is generated at seal; there is no durable worker recovery journal here. |
| `crates/common/src/log_stream.rs`: `SegmentedLogWriter`, `flush`, seal retry | Bounded buffers, retries, and a task join handle protect in-process completion. They do not survive worker/process or machine loss. Shared-file ambiguous appends are deliberately not replayed. |
| `crates/worker/src/runtime/process_executor.rs`: capture/finalization; `executor.rs`: capture result | Missing EOF or failed drain/seal means incomplete capture; action exit status remains separate. Positive empty-stream evidence is distinct from missing logs. |
| `crates/api/src/routes/internal_files.rs`: `reserve_proxy_log_upload`, `commit_log_segment`, direct authorize/complete | Proxy admission commits a grant/ledger reservation before `BlobStore::put`. Source authority and stream state are rechecked for DB acceptance afterwards. A paused, already-admitted PUT can outlive those checks. Direct completion validates exact returned provider identity, size and digest, then current authority. |
| `crates/api/src/routes/internal_files/object_close.rs`: `prepare`, `verify`, `finish`, `seal` | Acknowledged durable selection precedes connection-free pinned GET verification. Finish freshly acquires `log_source::Source`, repeats authorization and matches immutable input/revision. Owned HTTP task survives waiter loss, not API process loss. Five-minute close deadline and 15-second DB-phase budgets are not a durable recovery job. |
| `crates/api/src/routes/internal_files/log_source.rs`: `Producer`, `HistoricalBinding`, `publish_evidence` | Historical producer/proof is explicitly data, not authority. Encrypted selection keeps parsed manifest/hash/caps; final authenticated manifest evidence is supplied through the authorized publication path. Recovery must persist exact proof material as well as its hash. |
| `crates/common/src/repositories/log_stream.rs`: `select_close`, `publish_close`, `recover_expired_closes_bounded` | Exact accepted frontier/provider ledger membership is frozen. Expired-close recovery increments revision and abandons, invalidating old verifiers; it does not publish. Closing pins remain until abandonment, rather than vanishing merely because the clock expired. |
| `migrations/20261109000003_object_log_close_foundation.sql` through `20261109000012_object_log_publication_hardening.sql` | Permanent selected identity, publication/evidence consistency, and closing pins prevent generic Ready/metadata bypasses. Initial selection rejects currently settling issued grants. That is a DB admission condition, not proof that all remote PUTs have ended. |
| `crates/common/src/repositories/artifact_upload_grant.rs`: `renew`, `mark_expired`, `purge_terminal` | `settle_until` gates admission/cleanup; renewal can reuse a grant identity. Expiry and purging are not provider acknowledgements. Do not reuse this row as a complete per-physical-attempt journal. |
| `crates/common/src/repositories/object_maintenance.rs`, `storage_maintenance.rs`; `crates/supervisor/src/object_retention.rs` | Bounded discovery/reclamation respects ledger, grants, close/evidence pins. Forgotten uploads can be discovered, but removal of a ledger row cannot stop a request already admitted at the provider. |
| `crates/common/src/blob_store.rs`: `put`, `put_file_atomic`, `settle_atomic_file_put` | Protected file atomic PUT has a permanent create-only sentinel mechanism. Log segment proxy currently calls generic `put`, which can use multipart; do not transfer the protected-file proof to this path. Sentinel permanence and native exact deletion are essential assumptions, not ordinary ledger GC. |
| `crates/common/src/repositories/runtime_log_completion.rs`: `record` | Final recorded encrypted evidence is bound to delivery context/permit and current deadline. It cannot simply be called with fabricated context after expiry. |
| `crates/executor/src/workflow/log.rs`, `repositories/workflow_log_outbox.rs` | Durable ordered append payloads/seal intent, claim ownership, retry and sequence reconciliation already exist. Claims and transport authorization differ; replay still resolves workflow/execution/artifact state. Existing dispatcher resilience is not proof of independent post-retention authority. |
| `crates/sensor/src/sensor_log.rs`: rotation and `seal_active` | Rotated stream sealing uses sensor generation/lease authority. A replacement generation cannot inherit the previous generation's publication right. |

These are source observations, not newly executed acceptance results. Existing
ACK-loss/provider-pause tests are useful regression fixtures, not evidence that
worker loss, physical settlement, or this proposal has passed.

## Recovery boundaries and honest outcomes

| Crash/loss boundary | Recoverable evidence/bytes | Required outcome |
| --- | --- | --- |
| Pipe bytes or partial plaintext frame only in worker RAM | None after process loss | Incomplete/unknown. Never synthesize EOF, intentional silence, a truncation decision, or a final manifest. |
| Encrypted segment buffered locally, not DB accepted | Only if an owned durable spool survives; a provider object alone proves neither acceptance nor EOF | Retry exact admitted bytes under valid narrow authority, or leave incomplete and settle physical objects. |
| PUT acknowledged remotely but segment commit/ACK lost | Exact immutable provider version may exist; grant has expected digest/size | Reconcile exact request identity and pinned bytes. Unknown remote result is not absence; discovery alone is not publication authority. |
| Accepted contiguous prefix, no durable final completion intent | Accepted prefix is readable only under existing incomplete-stream policies | Cannot claim complete even if all known frames authenticate. Supervisor loss detection cannot sign a manifest for the worker. |
| Complete selected close, worker gone/credentials expired | Frozen segments and parsed final proof exist; original authenticated proof may still need durable storage | Current route can fail fresh authority. Proposed API recovery may publish only with an independently committed completion authorization. |
| API restart after selected intent, during GET or final COMMIT | Selection/intent and provider bytes survive | Claim a new recovery epoch, reverify pinned bytes, reconcile exact committed result; no replay of arbitrary mutation. |
| Published result with lost reply | Exact final evidence/body identity in DB | Return idempotent exact publication status after applicable read/status authorization. No new PUT or new version. |
| Worker/spool host permanently lost before upload | Only already durable remote/DB evidence survives | Cannot restore missing bytes. Report specific incomplete cause; cleanup proceeds separately. |

“Complete” means the producer captured its intended bounded stream, possibly with
an explicitly authenticated cap-truncation decision. It does not mean every byte
a descendant might later have emitted. Missing EOF is not cap truncation. Retain
per-stream outcomes so one incomplete stream does not erase the other's positive
empty or verified-complete evidence.

## Proposed completion intent and publication authority

Introduce a narrow completion intent at the live authorized handoff, not a generic
queue. For the first slice, commit it atomically with successful close selection,
when all segments are already accepted. The request must explicitly opt in to
server recovery; old selections remain non-authorizing history.

Persist an immutable intent identity containing artifact ID, immutable version ID,
stream ID, selected input/hash, original producer identity, exact authenticated
manifest bytes and digest (encrypted proof where applicable), parsed totals,
encoding/key-version reference, byte caps, truncation and capture-end reason.
Plain sensor/workflow logs use an explicit plain proof kind and producer EOF/seal
assertion, not an invented encrypted proof. Retain the original source binding:

- Recorded action: authority-selection/admission, attempt UUID, delivery context
  and permit IDs, worker ID and worker-instance UUID.
- Native/delegated action: selected authority and full `LogProducerBinding` or
  `RuntimeCredentialBinding`, not an inferred pack owner or replacement worker.
- Sensor: sensor ID, workload ID, assignment generation, lease-owner identity,
  metadata revision and schema/log-generation digest.
- Workflow: exact parent admission/attempt and immutable target version, plus the
  ordered outbox seal frontier. An executor claim UUID alone is insufficient.

Include an explicit system completion owner, authorization epoch, hard
`recover_until`, revocation state/epoch and independent retention pin. No raw JWT,
upload token, presigned URL, decrypted key, plaintext log, or bearer capability is
stored in this intent, audit details, or failure diagnostics. A random claim UUID
is an internal ownership fence, not a credential. Use server-authenticated
internal operations; callers cannot authorize publication merely by knowing IDs.

### Conservative first-slice authority matrix

This is a proposed new capability policy; current routes do not grant it. Explicit
opt-in handoff would survive only the live worker credential's expiry or process
loss. It does not survive security approval expiry or authoritative source loss.
One active API recovery owner may verify/publish one exact selected encrypted
recorded-action input; it receives no append, dataflow, secret disclosure, action
completion or general runtime API privilege. Read authorization is independent.

| Mutation or lifecycle event | Proposed unpublished intent outcome |
| --- | --- |
| Original worker process disappears or its JWT expires, with unchanged assignment/attempt and required authority still valid | May continue only under an acknowledged opt-in handoff and active bounded API claim. Neither event creates a handoff. |
| Worker reassignment, new instance replacing the recorded producer, execution retry or admission/attempt replacement | Cancel/fence the old intent in this conservative first slice. Broader survival is separate policy work. |
| Execution becomes terminal after an authenticated complete handoff | Does not alone revoke exact log completion; no execution-result update is authorized. |
| Execution/admission/origin evidence is removed by independent retention | Stop publication and fail closed; do not pin execution lifetime, synthesize evidence or infer no secret origins. Post-retention recovery/read projection is outside the first slice. |
| Sponsor identity frozen/deleted/rebound; sponsor pack deleted or owner identity replaced | Permanently cancel unpublished intents referencing that authority. Unfreezing/restoring cannot revive them. |
| Policy, admission, required inherited dependency or external approval explicitly revoked/deleted | Permanently cancel affected intents. New grants/replacement objects do not repair the old handoff. |
| Required external security approval or other effect fence expires | Stop/fence publication at its actual deadline, including after target waits. This is not ordinary worker JWT expiry. |
| Active release/policy pointer changes without revoking the retained immutable dependency | Apply existing retained-authority semantics; pointer movement alone is not fabricated revocation or a source of new grants. Sponsor and selected dependency checks remain mandatory. |
| Recovery feature disabled, artifact/version deletion requested, hard recovery horizon reached | Fence unpublished recovery. Reenablement or later retries cannot revive the terminal intent. Existing successful publication is reconciled before interpreting deletion/cancellation. |

Cancelling mutation owners must atomically update sticky cancellation/revocation
state and serialize with final publication in the existing graph/source/target
order. An implementation must map every row above to its actual repository and
SQL mutation paths, including implicit cascades; unhandled mutation paths block
enablement. A copied `revoked=false` snapshot is insufficient. Finalization must
revalidate retained sponsor/policy/admission/approval/dependency authority and its
earliest security expiry after waits. Missing evidence fails closed. No broader
survival after reassignment or retention is included by this recommendation.

The independent review found these are material security decisions. No proposed
capability is enabled by this document or by the user's general remediation
request. Existing seals retain current authorization until the new handoff's
policy and actual cancellation matrix are accepted and tested.

Implementation must add a private recovery authorization type and a dedicated
final-evidence repository path. Do not reconstruct an `AuthenticatedUser`, mint a
replacement worker token, fake a `DeliveryContext`, or relax `Source::recheck` for
normal HTTP seals. Publish original producer provenance alongside system recovery
provenance. Reuse final publication constraints only after they bind intent ID,
revocation epoch, exact proof, and selection to the new authorized operation.

## Recovery owner, deadlines and retention

Use a dedicated repository record keyed uniquely by immutable version/stream;
BIGINT IDs, explicit SELECT columns, no FK to retained execution/admission/event
rows. Retain intent identity and byte custody independently of execution retention;
a dangling execution ID is not a grant. First-slice publication still requires
the existing retained authority/disclosure evidence to be available. Do not retain
or reconstruct a new post-retention content authorization graph in this slice.

1. Live handoff takes existing graph/source protection, then artifact/version/stream
   locks; validates current producer after waits; selects close and inserts intent
   and pin atomically. Conflicting proof is rejected, identical retries match.
2. A bounded discovery query finds due intents. Short claim transaction follows
   the existing admission/target order and CASes intent state/epoch. Claim contains
   `(intent_id, owner_uuid, recovery_epoch, lease_until, selected_revision/hash)`.
   Every takeover, revoke, terminal transition or expired-close abandonment
   invalidates old epochs. Use DB clock after lock waits.
3. Drop DB ownership before provider GET. Reconstruct immutable selected input and
   verify all exact provider versions through EOF using `object_log_verification`.
   The current verifier uses the configured encryption key; there is no established
   historical-key registry. A key-version label does not supply key custody.
   First-slice recovery must fail explicitly if the original verification key is
   unavailable or changed; key rotation/recovery requires separate design. Never
   persist raw keys or infer a historical registry exists.
4. Final short transaction repeats graph/target protection and checks active system
   owner, current claim/epoch/lease, revocation epoch, hard deadline, selected
   revision/hash/full input and verified bytes. Atomically publish final proof,
   sealed/truncated, size/digest, Ready and recovery outcome. Old owner cannot
   publish, acknowledge or release a successor's pin.
5. Unknown COMMIT acknowledgement gets a bounded fresh-session reconciliation by
   immutable intent and result identity. Unknown GET/PUT/delete remains retryable
   uncertainty, never success or proof of absence. Keep pins while DB outcome is
   unresolved. Cancellation-safe SQLx session ownership remains mandatory.

Operational bounds are not selected yet. Use an effective verification deadline
strictly inside both the current claim lease and immutable close-selection
revision deadline, with a reserved bounded final-publication budget. In the first
slice, do not extend claims during verification: a timed-out attempt cancels and
joins its owned verifier before retry/takeover; expired epochs cannot publish or
release pins. Bound per-instance claim count, total verification bytes in flight,
retry/backoff and fair scan progress. Values must be measured before enabling.
Earlier candidates were a 30-second claim, 15-second DB phase, 60-second verifier
and 15-minute horizon; those mismatched windows were not an executable policy. These are candidate settings,
not measured acceptance or permission to extend existing timeouts. Large streams
may need a different measured bound. A timed-out verifier cannot publish and must stop and join before restart. No lease renewal extends the horizon.

The existing five-minute close revision deadline and abandoned-close sweeper must
be coordinated: reacquire a new selected revision for the **same immutable input**
under system completion authority before verification, never finalize an expired
revision. Abandonment cannot release the intent's independent pin. Deadline
expiry CASes to incomplete with a sanitized cause and initiates settlement. It
releases recovery byte pins only after reconciliation of any unknown DB commit;
physical-write obligations remain separately pinned until settled.

Keep three lifetimes separate: publication eligibility, exact evidence/byte
custody, and physical cleanup uncertainty. Pin selected artifact/version/stream,
segment versions and exact proof while a verifier or unknown DB commit can still
need them. Independent execution deletion remains allowed and ends first-slice
publication if required authority evidence disappears. It never becomes a grant.

Artifact deletion atomically fences unpublished recovery against publication.
If final COMMIT acknowledgement is unknown, reconcile the exact outcome before
releasing custody; no lost reply may produce resurrection or false deletion ACK.
Hard horizon/cancellation ends publication eligibility, not remote write activity.

The first slice has no complete physical-settlement oracle. Preserve existing
cleanup fences and existing physical uncertainty; do not promise deletion waits
until all remote writes settle. Exact recovery of pinned accepted bytes may ship
without new PUTs, but does not strengthen PR32's late-write/recreation guarantees.
If stronger deletion guarantees are required, physical settlement becomes a
prerequisite rather than a later claim. Unresolved custody/cleanup debt must be
visible and bounded operationally, never mislabeled completed reclamation.

## Worker encrypted spool decision (later slice)

Recommend an opt-in encrypted durable spool if process-restart recovery of
uncommitted bytes is required. It cannot promise machine-loss durability on an
ephemeral worker disk. API-owned selected-close recovery needs no spool and should
ship independently. Remote durable spooling increases latency/storage and needs
its own admission/settlement proof; do not imply it exists today.

For local spool mode, encrypt before disk; journal owned stream/attempt/instance,
frame sequence, ciphertext bytes/digest, immutable segment mapping, exact manifest,
capture outcome and handoff receipt. Atomic rename plus file/directory fsync marks
journal steps. Persist each encrypted segment before sending; delete it only after
reconciled exact DB acceptance and a durable receipt. Persist completion intent
before requesting handoff. Never serialize the encryption key or raw runtime
credentials. Short plaintext frame fragments still in RAM remain unrecoverable;
only assert durable capture for fsynced encrypted frames. Partial/corrupt journal
or missing EOF cannot be repaired by recomputing “complete” from remaining bytes.

After restart authenticate the worker afresh, prove access to the original journal
identity, and obtain narrow server recovery ownership for the original attempt.
Do not treat the restarted instance UUID as the old live worker. A new attempt
cannot append to or close the old stream. Recovery admission must have been
explicitly reserved while live if authority is to survive expiry; otherwise replay
requires still-valid original authority and stops on revocation. Enforce per-stream,
worker and global spool byte quotas and oldest-deadline backpressure; exhaustion
marks incomplete, with no plaintext fallback. Journal GC uses owned identities
and durable receipts/deadlines, never directory age alone.

## Physical PUT settlement and forgotten staging (separate slice)

The close state freezes membership, not all effects of previously issued writes.
A pre-admitted proxy future or presigned direct request can start/finish after
`settle_until`, after close, or after retention deleted its original key. Rejected
DB completion prevents disclosure but does not prevent physical recreation.
HEAD absence, grant expiry, request timeout, task cancellation, or deletion of a
known version is not a proof that no later version can arrive.

Add a per-physical-attempt journal **before** provider admission, separate from
renewable logical grants: immutable attempt ID, key/namespace incarnation,
version/stream/segment, owner fence, expected size/digest, transport/provider kind,
authority expiry, admission generation, and remote phase/outcome. Retain unknown,
active, accepted, abandoned, settling and settled dispositions separately from
close state. Store exact provider versions/settlement receipts when observed;
no presigned URL or grant token in the journal. A logical retry must reconcile
or allocate a distinct attempt with explicit mapping, never overwrite uncertainty.

Prefer bounded atomic create-only PUT for log segments, after proving S3, GCS and
object-filesystem native semantics. With an irreversible provider fence at each
retired key, a delayed atomic writer either wins first and its exact version is
deleted, or loses to the fence. Borrow `settle_atomic_file_put` only after adapting
its proof/ownership and proving log direct/proxy paths cannot bypass atomic
create-only writes. Preserve fences permanently, outside normal payload retention
and ledger deletion. A fence is control storage, never a log segment. Provider
admin/lifecycle policies must not remove it; if they do, settlement is unsupported.

Direct presigned expiry bounds new authorization, not necessarily a request that
already started. Require signed conditional-create semantics, immutable digest/
size constraints, exact version receipts and the same permanent fence. Unsupported
providers/transports must disable direct log upload or advertise only incomplete
settlement, not substitute a grace sleep. Atomic policy must cover retries too.
Use instance-incarnated keys to avoid reuse after a fresh database installation.

Generic `BlobStore::put` can have multipart staging. A sentinel for the final key
alone does not retire multipart uploads or random staging keys. Either eliminate
multipart for bounded segments, or durably journal every upload/staging ID before
it can escape, abort/reconcile each exact remote operation, enumerate only the
owned namespace with a durable bounded cursor, and retain unknown obligations.
Provider lifecycle cleanup is a fallback, not proof. Provider operations whose
identities cannot be recovered require an explicit capability limitation.

For accepted objects, normal retention first retires every write attempt, then
settles the key while deleting payload versions exactly; finally reclaims DB
metadata while retaining the permanent fence/control receipt. For abandoned
unaccepted objects, do the same without adding them to accepted membership.
Forgotten ledger/uploading rows are discovery inputs, not authorization to erase
an attempt journal. Discovery, exact deletion, fence creation and receipt commit
are restartable CAS steps. Failure/unknown outcome retains ownership and reports
cleanup debt. A hard recovery deadline cannot manufacture physical settlement.

Shared object filesystem storage can use an atomic create-only object contract
only after native path/version/replacement tests. Shared **append files** use
source locks/flock and mutable paths: no provider generation or create-only segment
fence. Their late append, path retirement, inode reuse and PhysicalCleanup require
separate design. Preserve current fences and never claim this object proposal
settles shared-file appends or makes ambiguous append retry safe.

## Producer-specific application

Action stdout/stderr use separate intents and outcomes; do not couple recovery to
worker's terminal execution-result transaction. Workflow child action streams use
this same action path; the workflow orchestration log uses its existing ordered
outbox. Keep that outbox rather than copying action spool logic into it. Bind its
seal to the drained ordered frontier and exact version before handing off, retain
pending payload/claim metadata independently of parent execution retention, and
prove old dispatcher claims cannot mutate after takeover. Current outbox joins
are not sufficient for a new post-retention provenance guarantee; that guarantee
is excluded from the first slice.

Sensors rotate indefinitely: completion intent belongs to one rotated stream and
one workload generation, never to the whole sensor lifetime. Normal rotation can
handoff a complete plain stream while live. Crash without rotation/EOF intent
leaves the old stream incomplete. Release/reacquire or schema/metadata replacement
cannot authorize old-generation writes; only a previously admitted exact system
completion survives ordinary lease loss under the proposed policy. Direct/proxy
transport changes neither this authority requirement nor settlement obligations.

## Smallest coherent milestones and acceptance gates

1. **Selected-close recovery only (recommended first implementation slice).** Add
   intent/revocation/claim/pin schema and repository methods beside `log_stream`;
   explicit live API handoff in `object_close::prepare`; private system-authorized
   finalizer sharing storage-only verifier; one API-owned bounded discovery/claim/verification/publication loop (no fake
   JWT). Supervisor retains reclamation, not a second recovery scheduler. A durable
   scan/cursor and claim protocol handles API restart and missed wakeups. Multiple
   API replicas share the same bounded CAS claim ownership.
   Worker seal request opts in and retains exact retry input. Coordinate abandoned
   close sweeper and artifact/evidence retention predicates. Start with recorded
   encrypted action object logs; fail closed for other producer kinds. No recovery
   of unaccepted bytes, no new PUTs. Update API client only if public contracts
   change; retain current auth gates. Extend SQLx metadata for schema changes.
2. **Producer coverage.** Add native/delegated action, workflow outbox handoff with
   retained bindings, and sensor rotation handoff, each with exact authority and
   lease/revocation tests. No universal source-type bypass. Shared files excluded.
3. **Physical settlement.** Add attempt journal and atomic bounded log PUT path,
   capability checks, exact version/fence receipts, retention ordering and staging
   cleanup. Close API metrics must still distinguish published from settled.
   Existing grants/ledger remain admission/discovery mechanisms with strengthened
   pin predicates, not the settlement oracle. Direct upload enabled per provider
   only after equivalent proof.
4. **Optional encrypted spool.** Add fsynced encrypted journal and original-attempt
   recovery admission, then restart replay/quota/GC. Do this only after deciding
   required host-loss durability and acceptable disk/latency costs.

Before enabling any slice, use deterministic barriers/signals and owned disposable
resources, at least four test threads, PG16 and PG18 for repository/API contracts.
Extend existing `storage_maintenance_repository_tests/object_close.rs`,
`api/tests/support/object_seal_*`, `runtime_log_replica_tests`,
`recorded_execution_events_tests`, `sensor_log_tests`, worker writer/capture tests,
and executor workflow log tests. Real transport/provider tests complement mocks.

Required concrete scenarios:

- Kill worker before encrypt, after spool fsync, after PUT, after DB accept and
  after live handoff; kill API after prepare COMMIT, during paused GET, during
  publication and after real COMMIT with lost ACK. Verify exact recoverability
  table outcomes, no synthetic empty/complete, and unchanged action result.
- Pause old recovery owner; expire/reclaim to a new epoch; resume old owner at
  every publication/pin-release step. Old CAS fails. Same input retries are
  idempotent; changed manifest, truncation, version, digest or provider identity
  fails. Race sweeper abandonment with recovery and observed target-lock waits.
- Cross actual worker JWT/delivery deadline and sensor lease expiry during GET;
  ordinary worker credential expiry succeeds only for opted-in handoff; required
  external approval expiry still denies. Explicit revocation before
  handoff, during GET, during final target wait or concurrently with commit must
  deny according to transaction serialization. Revoked/missing authority cannot
  be recreated by a new worker, sensor generation, parent retry or API restart.
- Remove execution/admission under owned retention while intent is pinned;
  the first slice denies publication when required retained authority evidence is
  missing; it does not synthesize a readable provenance projection.
  Artifact deletion cancels and fences recovery. Hard deadline gives incomplete,
  never releases unknown-COMMIT state prematurely; physical pins persist until
  settlement. Verify bounded scan progress and independent job cadences.
- Delay an already-admitted PUT before send and inside provider across grant
  expiry, close, retention, payload deletion, fence commit and DB receipt GC.
  Resume it: no payload recreation and no new accepted segment. Exercise direct
  presigned requests and proxy writes separately on versioned S3/RustFS and GCS;
  RustFS alone is not GCS or production S3 proof.
- Lose PUT/delete/fence ACKs and crash after remote success before journal commit;
  reconcile exact versions without deleting a replacement. Inject wrong version,
  missing pinned object, corrupt/truncated frame, valid prefix, proof mismatch,
  no historical key, and unknown provider response. None becomes Ready/settled.
- Crash during multipart/staging creation and abort, orphan discovery pagination,
  provider outage and repeated cleanup. Prove all owned identities discoverable,
  no unowned-prefix deletion, and no grant/ledger purge erasing unknown debt.
- Spool tests: short frame lost in RAM, torn journal, disk full, quota pressure,
  ACK loss, same-host restart with new instance, lost host volume, revoked source
  and stale attempt replay. No plaintext on disk or in diagnostics. Teardown joins
  every writer before deleting owned roots/resources.

Measure recovery scan/claim latency, pin age/bytes, retry/deadline reasons and
unsettled attempts/staging debt using sanitized IDs/codes only. A test passes only
with explicit cleanup acknowledgements; do not print plaintext runtime content.

## Decisions for the user/parent before implementation

- Accept or reject the conservative first-slice mutation matrix. It permits
  original worker credential expiry/process loss only, with unchanged producer
  assignment/attempt and fresh retained security authority. Existing behavior
  remains the alternative; broader reassignment/retention survival is excluded.
- Choose local restart durability versus host-loss durability for buffered bytes;
  recommended first slice promises neither and needs no spool.
- Approve permanent control-fence storage and atomic-only bounded log PUTs, or
  accept that post-retention recreation cannot yet be ruled out. A longer grace
  period alone is not an alternative proof.
- Confirm recovery horizon, quotas and artifact-deletion policy after measurement;
  the proposed timings are starting points. Unknown physical outcomes require
  retained cleanup debt even after publication recovery has expired.

## Validation record

Documentation-only source audit at the HEAD above. No application/database/provider
acceptance tests executed and no historical test counts adopted as current proof.
Validate whitespace, local links/entry-point existence and single-file scope.
Implementation acceptance, schema/API compatibility within v1, authority review,
provider settlement proof and required CI remain subsequent work.

## Independent-review disposition (2026-11-10)

Source review identified three P1 design gaps: undefined security revocation,
unproved post-retention disclosure closure/key custody, and deletion guarantees
that depended on deferred physical settlement. This revision adds an explicit
proposed mutation matrix, excludes post-retention recovery/read expansion, keeps
configured-key limitations explicit, and separates eligibility/custody/physical
uncertainty. Two P2 findings remove dual discovery ownership and mismatched lease
and verification windows. These are design corrections, not passed runtime tests
or permission to enable the capability. Repository mutation-path coverage,
authority acceptance and the full deterministic crash/revocation matrix remain
implementation gates. Shutdown and ordinary in-process retry fixes do not depend
on approving this new capability.
