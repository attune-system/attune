# Single-writer log streaming simplification

Status (2026-11-10 UTC): partially implemented by merged
[PR32](https://github.com/attune-system/attune/pull/32). Object close verification
now runs outside database transactions with durable selection and fresh final
authorization. The full single-writer simplification, physical PUT settlement,
shared-file split, incremental verification and worker crash recovery remain
proposed. No deployment is certified here.
Initial plan date: 2026-11-09 UTC. See the
[follow-up roadmap](runtime-log-follow-up-roadmap.md) for dependency order,
acceptance gates, CI evidence and rollout prerequisites.

## Objective

Make execution stdout/stderr behave as single-writer, append-only streams.
Remove database transactions and locks from slow storage reads, hashing, upload
waits, and retry backoff wherever the current safety contract permits. Reduce
repeated database work and unnecessary serialization between independent logs.

The desired steady state is:

```text
assigned worker captures bytes
  -> ordered, bounded ingestion
  -> immutable accepted data
  -> short metadata publication

writer finishes
  -> drain accepted writes
  -> close a specific committed frontier
  -> verify outside database transactions when necessary
  -> short final publication
```

Zero explicit application locks in the byte-processing loop is a useful target.
Zero database locking is not a promise: metadata updates take row locks, and
publication still needs ordering against ownership changes and cleanup. Optimize
lock scope and eliminate unnecessary locks rather than weakening those contracts.

This document retains the original design goals. Its object close subset is now
implemented; remaining implementation, release, deployment, and changes to
retention or security semantics require their own authorized work.

## Relationship to existing plans

- [Graph admission contention](graph-admission-contention.md) establishes the
  existing lock order and the need to move storage work outside transactions.
- [Object-log seal claims review](object-log-seal-claims.md) documents missing
  upload-settlement, snapshot, and cleanup guarantees. Those counterexamples
  remain applicable. A writer credential or upload-grant TTL is not already a
  safe seal reservation.
- [Worker lifecycle performance](worker-execution-lifecycle-performance.md)
  covers lazy log allocation and terminal completion ordering.
- [Bounded retention queries](bounded-retention-query-optimization.md) covers
  collector planning and progress, rather than stream ownership.
- [Transaction reuse remediation](database-transaction-reuse-remediation.md)
  defines cancellation, confirmed cleanup, and connection quarantine guarantees.

Prefer one narrow stream lifecycle over adding a generic seal-job or lease
framework. If a durable close-intent record is required, identify exactly which
existing guarantee it supplies and which old checks/state it replaces. It is a
new protocol change even if it only adds fields to an existing table.

Keep existing public log formats, authenticated completion evidence, and v1
contracts. Use forward migrations for necessary durable state. Do not introduce
a second log format, remove a storage backend, or reset data implicitly.

## Evidence and source scope

The incident investigation inspected deployed Attune `0.10.3` and the local
checkout at `880b1b2a138a335eb549c08452f0d3440de68d41`. These are different
revisions; reconcile the current implementation branch before coding.

At that historical revision, the inspected sealing route locked the parent
artifact, version, and stream,
then read and hashed stored content before publishing the seal and Ready state
in the same transaction. This established a long-lock risk. At main
`820b4192068ccd321167a28dd888276dd7998368`, ObjectSegments instead uses the
[durable close protocol](object-log-seal-claims.md). Shared-file verification still
retains its transaction, snapshot and flock. The old description is not a claim
about today's object route or deployed image revisions.

Database-path stalls affected multiple services simultaneously during the
incident. That establishes a shared symptom, not that log sealing initiated it.
This plan addresses a confirmed risk in the code, not a proven root cause of
those particular pauses.

Primary source areas:

- `crates/worker/src/runtime/process_executor.rs` and `executor.rs`.
- `crates/common/src/log_stream.rs` and `runtime_log_completion.rs`.
- `crates/common/src/worker_delivery.rs` and `repositories/worker_delivery.rs`.
- `crates/common/src/repositories/log_stream.rs` and `artifact_upload_grant.rs`.
- `crates/common/src/artifact_transport/{api,volume}.rs`.
- `crates/api/src/routes/internal_files.rs`, `recorded_files.rs`, and
  `live_logs.rs`.
- `crates/executor/src/workflow/log.rs` and sensor log producers/readers.
- Artifact, file-evidence, object-ledger, and storage-maintenance repositories.

## Check the single-writer premise before relying on it

The normal worker-delivery path supports one logical producer per execution
attempt and stream. Its capability binds worker ID, worker instance, execution
attempt/admission, exact artifact version, stream, deadline, and byte/sequence
bounds. Delivery rechecks the current assignment and attempt. Purpose-specific
delivery credentials must not be passed to action code.

Important distinctions:

- The ownership unit is an execution's stdout or stderr stream, not its parent
  artifact. An action-level artifact can contain versions from many executions.
- One logical producer can have overlapping HTTP retries or outstanding uploads.
  A timed-out request is not proof its storage operation stopped.
- Readers, seal/publication handlers, and retention also access stream state.
  Read access is not write authority.
- Workflow logs are produced through executor/outbox paths, and sensor logs use
  workload/generation ownership. Do not force their ownership into worker fields.
- Shared-volume access depends on filesystem permissions and isolation. API
  credential restrictions do not make a shared mount private to one process.
- Token checks establish identity and scope, not universal exclusion of every
  generic artifact mutation path. Audit these paths before declaring the
  single-writer property fully enforced.

Phase 1 must produce an access matrix covering allocation, append, overwrite,
direct/proxy upload, upload completion, seal, generic artifact mutation, read,
delete, recovery, worker replacement, and native/workflow/sensor production.
Use the real router and middleware in negative tests. A route's WorkerDelivery
marker accepts purpose-specific tokens; its presence alone does not prove all
other credential classes are rejected.

The intended contract is one active logical producer for a particular stream
generation. A different worker or attempt must not adopt the same mutable
stream silently. Duplicate delivery by the same producer is an idempotency case,
not permission to append different bytes at the same sequence.

## Required properties

1. Committed bytes have one order and immutable identity. Exact retries succeed
   without appending or charging bytes twice; differing retries conflict.
2. Reads expose only committed content authorized for the exact version and its
   recorded execution/sensor origins. Preserve guarded read freshness.
3. Final publication binds the exact accepted bytes, truncation, producer,
   attempt, and authenticated completion evidence. Never trust a client-supplied
   digest as the sole integrity proof.
4. A closed stream cannot accept new content, including from a delayed request
   that began before closure or replacement.
5. Cleanup cannot remove open content, a verification snapshot, or unresolved
   physical write obligations. Expiry alone does not prove settlement.
6. Cancellation, worker loss, API restart, and uncertain commits have bounded,
   explicit outcomes. They cannot manufacture successful completion.
7. Preserve canonical graph/source/target lock order. Do not carry a target lock
   across storage work and then acquire graph admission behind it.
8. Preserve encryption, origin attribution, fresh revocation checks, independent
   deadlines, retention, and execution terminal-publication ordering. A producer
   lease does not substitute for authorization.

## Proposed design

### 1. Give runtime logs a narrow append-only interface

Separate log operations from generic mutable artifact/file operations in the
owning repositories and handlers, while retaining artifact metadata for discovery
and existing APIs where their behavior remains correct.

The runtime-log interface needs only open/allocate, append accepted data, close,
read committed data, and abandon/recover. Generic replace/delete operations must
not bypass stream ownership or cleanup eligibility. Do not create duplicate
metadata merely to rename the interface.

Serialize each producer locally with one bounded queue and one ordered append
owner. Bound memory, outstanding storage requests, total bytes, and retry time.
Drain stdout and stderr independently so a stalled sink cannot deadlock process
pipe handling. Define truncation/failure explicitly when bounds are exhausted.

Keep empty-stream allocation lazy. Preserve the distinction between intentional
silence and failed capture. Do not combine stdout and stderr into one identity.

### 2. Keep storage work outside database transactions

For object segments, use the existing exact sequence, byte range, digest, and
provider-version identities as the basis for idempotent accepted data. Select
one canonical ingestion path per deployment; normalize direct and proxy paths
to the same ownership and settlement rules.

A short metadata operation may authorize/reserve a write. The upload itself
must occur without a database transaction, graph lock, parent artifact lock, or
checked-out connection. Another short operation freshly authorizes publication
and records only verified, settled content against the expected owner/frontier.
Readers must not discover provisional uploads as accepted stream content.

Audit all existing object and multipart staging obligations before removing a
lock. Preserve native create-only/fencing guarantees and recovery of late PUTs.
Grant expiry, an HTTP timeout, HEAD absence, or a successful duplicate request
does not establish that every outstanding write settled. Reuse settlement
infrastructure only where its ownership contract actually applies to logs.

Avoid parent-artifact serialization on every append where only a version/stream
frontier changes. Separate parent creation/latest-version/retention bookkeeping
from byte acceptance after proving the required lock order and ownership checks.
Independent executions sharing an artifact container should not serialize their
storage I/O on that parent.

### 3. Close the producer, then publish completion

Treat closing as part of the single-writer lifecycle, not a second independent
producer that competes with the worker.

The conceptual states are open, closing a fixed frontier, and published
complete, with an explicit incomplete/abandoned recovery outcome. Map these onto
the existing state model with the minimum reviewed durable extension. Do not
expose `sealed = true` or Ready before verification has completed.

1. The producer stops accepting new bytes and drains its accepted queue.
2. In a short transaction, revalidate the owner and authority, confirm upload
   settlement, and record closure of the exact committed frontier. Any further
   append or upload completion must respect that closure.
3. If verification is needed, verify the fixed input outside database
   transactions. Retain an explicit durable retention protection for the input
   until verification and final publication resolve. Counts alone are not an
   immutable snapshot; bind exact identities or enforce immutable membership.
4. In a short transaction, reacquire current authority and locks in order,
   compare the unchanged owner/frontier/proof, and atomically publish the final
   digest, completion evidence, sealed state, and Ready artifact metadata.

An identical completion retry observes the same result. A different proof,
frontier, owner, or truncation request must not replace it. Recovery reconciles
unknown commits before trying another state transition.

Prefer a stream-specific close intent over a generic distributed seal scheduler.
The object-seal review's missing guarantees still have to be supplied; an
ordinary producer credential is not itself a durable close intent or cleanup pin.
Do not add a second owner token if the existing attempt plus a narrowly scoped
close generation already provides proven stale-finalizer fencing.

### 4. Avoid full-log rereads where trusted ingestion can prove completion

Measure the simplest intermediate improvement first: retain the existing full
verification, but perform it outside database transactions against protected,
fixed inputs. This removes the long lock hold without changing the proof format.

Then evaluate incremental verification on the trusted ingestion path:

- Verify actual accepted bytes, sequence, size, framing, and required encrypted
  identities while processing each chunk; retain sufficient trusted evidence
  to validate the final authenticated manifest.
- Update integrity state only for an acknowledged accepted segment. Duplicate
  retries and uncertain publication must not advance it twice.
- Specify restart and multi-API-instance behavior. In-memory hash state on one
  API pod is not a durable stream protocol.
- Permit bounded reconstruction or a full out-of-transaction verification on
  recovery if needed. Do not make the normal close path reread all bytes solely
  because the state is currently not retained.
- Ordinary SHA-256 digests of segments cannot simply be combined to produce the
  existing SHA-256 of the concatenated stream. Preserve the public/authenticated
  digest contract; do not quietly substitute a Merkle digest or new format.
- Head metadata, ETags, a worker's claimed digest, and a valid header without
  required frame checks are insufficient substitutes for the current proof.

Adopt incremental verification only if it removes work without adding a more
fragile protocol than out-of-transaction verification. The plan does not require
eliminating every recovery reread to count as a successful simplification.

### 5. Handle shared-file transport separately

Direct file append can become visible before a metadata transaction commits.
Moving it outside database locks therefore requires either an enforced committed
read frontier or another reviewed visibility/fencing mechanism.

Establish exclusive producer ownership and account for open descriptors, late
writes, process replacement, and cleanup. A path, file mode, or size check alone
is not a replacement for an immutable snapshot. Do not assume changing a file to
read-only invalidates descriptors that were already open.

Prefer producer-side drain and close followed by a stable snapshot. Keep only
the minimum necessary filesystem synchronization around append/close and short
metadata synchronization around publication. A process mutex alone cannot
coordinate different hosts sharing the volume.

If existing filesystem capabilities cannot establish those guarantees simply,
retain the safe shared-file path as a documented exception while improving the
object backend. Backend removal or changing deployment storage mode is outside
this plan's authorization.

### 6. Keep reader and cleanup responsibilities separate

Readers follow a committed prefix and independently enforce current disclosure
rights; they do not become log producers. Preserve per-frame/source freshness
where required by the existing disclosure contract.

Cleanup skips or defers a stream with live ownership, an unresolved close input,
or a physical write obligation. Failed cleanup should not hold a transaction
while waiting for the producer. Revisit through bounded maintenance scheduling.
Do not implement retention by letting the writer hold a database lock for its
entire lifetime.

Recovery may quarantine or reclaim incomplete data after proving settlement.
It must not publish complete logs using expired producer authority. Preserve
pending-delivery lineage independently of execution-row retention.

## Lock-removal targets and remaining decisions

| Operation | Target | Required condition |
| --- | --- | --- |
| Read stdout/stderr from process pipes | No database connection or lock | Bounded producer buffering and explicit overflow handling |
| Upload/read/hash/retry storage bytes | No database connection or lock | Exact input identity, write settlement, and retention protection |
| Publish an accepted append | Short version/stream metadata transaction | Current owner, expected sequence, fresh authority, retry identity |
| Update parent artifact metadata | Only when parent state actually changes | Reviewed lock order and atomic latest-version/retention semantics |
| Close a frontier | Short transaction | No unsettled append can change the selected content |
| Publish final completion | Short transaction | Current authorization and exact verified frontier/proof |
| Stream disclosure | Bounded current authorization checks | Existing revocation and origin rules preserved |
| Shared-file append/close | Minimize scope; backend-specific decision | Proven cross-process byte visibility and late-writer fencing |

Decide whether explicit stream advisory locks remain necessary once one owner,
unique sequence constraints, and conditional frontier updates are authoritative.
Row locks taken by those updates may be sufficient for some paths. Prove that
through race tests; do not simply delete the advisory calls.

Global graph locks protect more than byte ordering. Remove them from storage
waits by preserving lifetime/authority through the correct state transitions,
not by declaring all log operations exempt from graph admission.

## Implementation sequence and validation

1. **Ownership audit and baseline.** Produce the access matrix and negative
   tests before relying on one writer. Instrument database calls, pool use,
   graph/artifact/version/stream wait and hold times, storage time, and recovery
   outcomes. Test action, executor/workflow, and sensor producers separately.
2. **Simplify the existing producer path.** Ensure one ordered local append
   owner, bounded queues, exact retry identities, and lazy allocation. Remove
   duplicate immutable lookups within existing protected units; preserve mutable
   authority freshness.
3. **Object-backend close/publication.** Implement the minimum closure,
   input-protection, and upload-settlement state required to move verification
   outside transactions. Include all producer and cleanup entry points in the
   same change. Keep final publication atomic.
4. **Reduce repeated proof work.** Evaluate incremental verification and
   batched metadata against the existing proof contract. Do not merge this with
   a new public format or relaxed delivery semantics.
5. **Shared-file decision.** Implement a separately validated narrow path, or
   document the remaining lock requirement and why removing it is not yet safe.
6. **Retire redundant machinery.** Remove superseded branches/checks/state only
   after their replacement and recovery paths are proven. Reconcile earlier
   plans so they do not remain competing implementation instructions.

Required tests include:

- Wrong worker, wrong instance, wrong attempt, other execution, child execution
  token, human Access token, generic artifact mutation, and stale producer
  credentials cannot mutate a worker-owned stream outside their proper scope.
- Empty, stdout-only, stderr-only, binary/UTF-8 boundaries, large, encrypted,
  truncated, and long-running sensor logs retain their semantics.
- Exact duplicate appends and finalization are idempotent; different duplicate
  bytes, sequence gaps, and conflicting final manifests fail.
- API restart or a different API instance does not lose integrity state or
  accept stale publication. Producer replacement cannot reopen the old stream.
- Late PUTs, expired grants, duplicate in-flight uploads, partial/multipart
  writes, and unknown commits preserve cleanup obligations and pins.
- Revocation, permission expiry, dynamic secret-origin changes, and cleanup races
  retain the current fail-closed behavior. No plaintext or credentials enter
  operational traces.
- Cancellation/crash at each lifecycle transition produces correct recovery or
  an explicit incomplete result, never invented successful completion.
- Inject slow/failing object reads, uploads, hashing, and filesystem operations.
  In the redesigned paths, no open database transaction or retained database
  lock spans those waits; unrelated executions continue making progress.
- Test several executions sharing one parent artifact, many independent
  streams, and a very large stream alongside short executions.
- Validate physical deletion and restart recovery on disposable fixtures, not
  only mocks or read-only plan inspection.

Accept changes based on reduced database interactions and lock hold times,
correct deletion/publication, bounded resource use, and simpler ownership—not
only a lower mean execution time. Keep durability settings unchanged. Preserve
the current execution completion contract; making settlement asynchronous after
terminal publication requires a separate explicit durability decision.

## Deliverable and remaining uncertainty

The implementation should leave one understandable producer lifecycle and a
small set of metadata transitions. It must identify which explicit locks were
removed, which remain, and which safety property each remaining lock protects.

The normal worker binding is source-confirmed. Universal single-writer access,
incremental-proof recovery, and shared-file lock elimination remain
design/validation work, not established guarantees. PR32 supplies durable object
close selection, without claiming a globally minimal lifecycle. The object-seal
review's settlement and cleanup counterexamples block claiming the full
simplification or removing the corresponding guards. PR32's narrower
fresh-instance close protocol does not claim those guarantees; this plan does
not expand its rollout contract.
