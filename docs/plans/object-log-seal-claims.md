# Object-log seal claims design review

Status: design only, deferred from the minimal contention patch. No implementation, migrations, live
access, or tests performed. Date: 2026-10-30.

## Decision

**No-go for an immediate reuse-only PR5 implementation.** Neither an upload grant
nor a log-reader lease supplies the durable seal ownership, immutable manifest,
retention pin, or recovery contract required by
[PR5](graph-admission-contention.md#pr-5-move-object-log-verification-outside-graph-transactions).
Moving the read loop outside the existing transaction is not sufficient.

A small dedicated, read-only seal claim is feasible. It can reuse the graph gate,
artifact/stream lock ordering, pinned object reader, worker authority checks,
frame verifier, and atomic final publication. It does not require a new public
endpoint or a new artifact body-state enum. It is still a new protocol, not an
existing claim with a different caller.

The full requested contract also includes unresolved uploads and physical cleanup
after claim expiry. Existing log upload recovery does not establish that contract.
Do not describe a one-table seal change as solving it. Keep PR5 deferred unless
both the new seal contract and its upload-settlement scope are reviewed.
Continue contention work in PRs 1 through 4 independently.

## What the owning code actually provides

| Code | Reusable guarantee | Missing guarantee |
| --- | --- | --- |
| `crates/api/src/routes/internal_files.rs:2047` | Current seal authorizes, locks version/stream, verifies, and publishes in one transaction | The segment GET/hash/frame loop at lines 2154 through 2170 holds its graph/source/target locks throughout storage waits |
| `crates/common/src/repositories/log_stream.rs:214` | Transaction advisory lock plus stream row lock serializes producers and sealers | Neither lock survives a prepare commit; no sealing owner or generation exists |
| `migrations/20260909000004_immutable_log_segments.sql` | Segment IDs, unique sequence/key, exact provider version, offsets, size, digest | The create-only comment is not an UPDATE guard; there is no seal pin |
| `crates/common/src/repositories/artifact_upload_grant.rs` | Exact key/sequence/size/digest reservation; completed provider identity | One grant is not a manifest or an exclusive sealer. Expiry/renewal is time-based; terminal rows can be purged. Its artifact-version FK cascades |
| `crates/common/src/repositories/execution_log_stream_lease.rs` | Global/per-identity SSE-reader capacity with expiry | No stream/version/segment binding, upload exclusion, or retention pin |
| `crates/common/src/repositories/worker_delivery.rs` | Immutable permit/context binding, attempt/worker/owner checks, fresh retained authority and independent deadlines | A permit bounds append/seal; it does not reserve one seal or freeze its bytes |
| `crates/common/src/repositories/runtime_log_completion.rs` | Exact immutable encrypted completion attribution, inserted with seal/Ready | This is final evidence, not an in-progress claim. Inserting it at prepare would publish unverified completion evidence |
| `crates/common/src/repositories/file_evidence_retention.rs:1178` | Durable transitive file-origin pins with physical-obligation lifetime | Requires an actual authenticated `artifact_file_origin` and root read. Logs without that origin cannot create a synthetic owner |
| `crates/common/src/repositories/object_maintenance.rs` | Segment references protect their exact provider objects from ordinary collection | A grant/reader by itself does not pin segment metadata or veto artifact cleanup |

Important migration boundaries:

- `20261015006005_file_evidence_retention.sql` protects metadata deletion through
  exclusive admission, pin predicates, and secondary-row guards. It does not
  synthesize pins for every log stream.
- `20261015006001_worker_delivery.sql` makes delivery context/permit immutable.
  Contexts and permits can still disappear through their existing cascades.
  Preserve independent execution-retention semantics; do not add execution FKs.
- `20261015006002_encrypted_log_completion.sql` requires final manifest publication
  with Ready and protects its metadata from replacement.
- `20261015006007_file_put_settlement.sql` explicitly rejects age, cancellation,
  and HEAD absence as settlement proof. Its durable PUTs and permanent write
  fences belong to protected-file owners, not arbitrary log segment uploads.

## Why upload grants cannot be relabeled as seal claims

`segment_sequence = NULL` already means an artifact-body upload. A synthetic
sequence would conflict with nonnegative sequence semantics and collector logic.
A synthetic object key would reserve an object that does not describe the stream.
All variants still lack the segment vector, claim generation, producer exclusion,
and cleanup veto. Changing grant states/meaning also changes direct upload,
proxy upload, renewal, expiry, purge, and every maintenance predicate.

Using a grant row's UUID is not fencing. A stale sealer must lose the right to
finalize when a newer owner takes over. Grant renewal keeps the same token and
has no immutable seal input identity. The reader lease has even less relevant
state. Extending either table until it supplies these properties is more obscure
than a dedicated claim.

Do not set `log_stream.sealed = true` during prepare. Existing readers use sealed
state as completion information; Pending plus sealed is not a private reservation.
Likewise, do not retain a transaction or session advisory lock across storage I/O
and then reacquire graph admission behind target locks. That reverses the graph
order and can deadlock a queued collector.

## Smallest useful seal design, subject to upload prerequisites

Keep `shared_file` on its current path. Add an internal repository, for example
`crates/common/src/repositories/log_seal_claim.rs`, with `prepare`, `finalize`,
`abandon`, and bounded recovery operations. The route owns the two short
transactions and the intervening verification. No authority object or transaction
is carried across the storage phase.

One dedicated claim table is enough for the read-only portion:

- BIGINT primary ID and a unique stream/version association.
- Random internal owner token, replaced on takeover. The token is not a runtime
  credential and is never returned as authorization or logged.
- Immutable artifact/version/stream/backend binding, sequence count, byte count,
  requested truncation, and canonical manifest identity.
- Exact ordered manifest entries containing segment ID, sequence, offsets, size,
  SHA-256, object key, and provider version. Store a bounded manifest or pin
  create-only rows and store its digest/frontier. Counts alone are insufficient.
- Proof mode, encoding, exact authenticated-manifest identity, and relevant
  permit/context/admission/attempt/worker-instance identities when present. Store
  no raw bearer token or plaintext log bytes.
- Database-clock owner deadline and explicit phase. Owner expiry removes finalize
  authority, not automatically the durable pin or an upload obligation.

The table's FK/cascade behavior must not allow deleting a version to erase its
pin. Explicit eligibility guards must cover parent artifact deletion as well as
version, stream, segment, and cleanup-state transitions. A restrictive FK alone
does not stop `body_state = deleting` followed by physical cleanup.

Use existing segment rows as provider-reference pins where possible. This avoids
duplicating one pin row per segment. It is valid only if the claim prevents their
deletion/replacement and prepare rejects already-claimed physical deletion.
Require the ledger's exact provider identity and usable state before claiming.
No new positive authority is granted by retaining any historical evidence.

### State machine

1. **Open to prepared.** Enter shared graph admission first, including for legacy
   workers. Authorize with the existing source/target helpers, then lock artifact,
   version, stream, and claim in that order. Require object backend, Pending,
   unsealed, valid contiguous snapshot, valid seal bounds, and no unresolved
   upload publication. Freeze the manifest and create its durable pin/owner in
   the same transaction. Check fresh authority/deadlines and await commit.
2. **Prepared to verifying.** Read only the prepared immutable manifest with
   `stream_log_segments` and its `get_pinned` calls. Hold no DB transaction or pool
   connection during GET/hash/frame verification. Consume every reader through
   EOF so length/digest failures are observed. Return an internal verification
   result bound to owner token and manifest/proof identity. No durable verified
   phase is necessary; a crash can repeat reads.
3. **Verifying to complete.** Start a fresh READ COMMITTED transaction. Repeat
   authorization, not just `ensure_fresh_on` on the old proof. Repeat artifact
   owner binding and permit/context/attempt checks. After locks, compare current
   exact manifest, claim token/deadline/phase, proof input, body state, stream
   identity, delivery bounds, and upload settlement. Only then insert encrypted
   final evidence, set encrypted metadata, seal, mark Ready, and release the
   read-only seal claim atomically. Preserve the existing final freshness checks
   before commit and handoff. There is no provider I/O here.
4. **Failure or owner loss.** Explicit abandon may release a read-only claim only
   with its matching token and no unresolved obligations owned by that claim.
   Expiry fences the old token. Recovery under ordered admission either assigns
   a new token to the same immutable inputs or abandons verification. Never
   silently adopt another owner's successful hash result.
5. **Outstanding physical obligation.** Keep the relevant durable upload owner,
   provider/key evidence, and required pins until native settlement. Recover or
   quarantine it even if the caller's authority expired or was revoked. Recovery
   is cleanup permission, never permission to publish Ready.

An identical concurrent seal should observe/wait for the first outcome outside
admission and retry fresh authorization, not steal a live token. Preserve the
existing test that both identical seal calls succeed. A lost final response is
reconciled against exact Ready size/digest/truncation and encrypted proof where
present. Do not report an unknown commit as a rollback or start a new upload.

All producer entry points must honor the claim under the stream lock, including
direct grant issuance/renewal, proxy reservation, direct completion, and proxy
segment publication. A prepared claim rejects a new append. Exact existing
segment retries may succeed without any PUT or manifest change. If append is
allowed instead, finalize must compare the full manifest and fail; that is a
different optimistic design, not the freeze contract above.

Prepare must reject preexisting unresolved uploads rather than trying to absorb
them into the read-only claim. An upload admitted before prepare must either
finish before the manifest is selected or make prepare fail. The absence of an
active TTL is not proof that this upload finished.

## Upload settlement is the blocking prerequisite

Current proxy log publication at `internal_files.rs:1685` calls `BlobStore::put`.
The object-store implementation at `crates/common/src/blob_store.rs:703` uploads
multipart data to random `.attune-upload/...` staging, then copies create-only to
the final key. That staging/upload identity is not in the log grant. By contrast,
protected files use `put_file_atomic` and durable `file_evidence_put` ownership.

`active_log_segment_exists` ignores issued grants once `settle_until` passes.
`mark_expired`, `purge_terminal`, version cascades, stale-upload cleanup, and
object deletion predicates do not prove that late log writes have stopped.
Even a completed sequence grant does not prove that all concurrent duplicate
PUTs have settled. The current object while retained prevents a create-only
duplicate from changing it; removing that object later removes this protection.

A read-only seal split creates no new PUTs, so this gap must not be exaggerated
into a newly introduced overwrite bug. Exact-version reads plus a full final
manifest comparison protect byte identity. But the existing gap means the full
physical-obligation requirement cannot honestly be claimed from current grants.
Rejecting only active grants, or rejecting all currently visible expired grants,
does not fix it because terminal grants can already have been purged.

If the full contract is approved, the smallest credible upload work is:

1. Switch bounded proxy log segments to the existing atomic create-only transport
   operation. Direct S3 uploads already bind checksum, length, and create-only
   semantics. Do not use a permanent final-key sentinel to claim random remote
   multipart staging was also removed.
2. Give each log key a durable physical obligation before sending a PUT or
   returning a direct-upload URL. Preserve it through grant expiry, purge, and
   metadata cleanup. A successful key observation is enough to bind the exact
   segment, but not to forget possible duplicate writers before later deletion.
3. Retire an abandoned atomic key only after native permanent fencing. Reuse
   `settle_atomic_file_put` and ledger fence behavior where their exact contract
   applies. Extend repository ownership deliberately; do not forge a
   `file_evidence_owner` root. A log key may have many attempts but one permanent
   fence can close all atomic create-only writers to that key.
4. Update cleanup claim, exact-reference, deletion completion, grant purge, and
   pin-release checks together. New publication must reject keys already claimed
   for deletion/fencing. Preserve unresolved legacy staging as a diagnostic or
   require an explicitly approved fresh-install boundary, never a silent reset.

This needs changes to the upload ownership schema as well as the seal claim.
Whether it extends grants or adds a narrow log-key owner table needs a separate
design review. The current cascading FK and time-based purge cannot remain the only
owner. It is no longer a minimal route refactor.

## Specific implementation boundaries after approval

- `internal_files.rs`: split only the object backend; extract storage-only
  verification; call fresh authorization in prepare and finalize; keep exact
  retry checks and awaited completion on every transaction exit.
- `repositories/log_stream.rs` and the dedicated claim repository: ordered claim
  capture/CAS, manifest comparison, producer checks, explicit abandon/recovery.
- One seal migration: claim state, constraints/indexes, immutable manifest/segment
  identity, deletion and body-cleanup guards. Integrate with
  `file_evidence_version_eligible`, artifact eligibility, cleanup requests, and
  secondary deletion guards rather than a second graph lock.
- Upload prerequisite: `artifact_upload_grant.rs`, `object_maintenance.rs`, the
  proxy call site, and explicit log physical-owner/fence migration. Keep existing
  protected-file recovery semantics intact.
- `runtime_log_completion.rs`: reuse `FinalLogManifest::open`, `FrameVerifier`,
  and `RuntimeLogCompletionRepository::record`. Record only with final Ready.
- Existing maintenance/recovery owners: bounded stale-claim reconciliation and
  diagnostics. Do not teach a collector to auto-complete a log with old authority.
  Claim presence must neither produce SSE done nor extend terminal grace forever.

There is also a source-visible final-publication hazard to resolve before calling
this implementable. `mark_body_ready_in_transaction` fires
`trg_enforce_artifact_retention_ready`. For version-count retention,
`enforce_artifact_retention` issues a DELETE, even when it will delete zero rows.
The statement's `aaa_file_evidence_entry` requests exclusive graph admission and
rejects a transaction already holding shared admission. This review did not run
the path, so it is a static finding, not a reproduced failure. Do not solve it by
holding exclusive admission during verification or by suppressing the guard.
An approved fix must defer old-version reclamation to an independently ordered
cleanup transaction while preserving artifact metadata refresh. Coordinate with
the retention owner before editing these shared migration functions.

For encrypted logs, persist/bind the exact submitted authenticated proof, not
only its decoded counts. Outside the transaction, verify AEAD frame identity,
sequence, total frames, plaintext bytes, ciphertext bytes, and ciphertext digest.
Finalize must match that same proof and newly checked delivery context/permit.
An authentic prefix, an ETag, or a valid final header without frame verification
is not completion. Empty streams and frames spanning segments remain supported.

## Correctness counterexamples and required tests

| Ordered race or failure | Required result |
| --- | --- |
| Prepare commits, then append reserves next sequence | Claim blocks reservation, or an explicitly chosen optimistic design rejects finalization on full manifest mismatch |
| Two sealers prepare; old owner pauses past takeover | New token fences the old finalizer; differing truncation/proof inputs conflict |
| Verify segment A; replace metadata with same size/count but different key/version | Immutable guard rejects replacement or exact comparison rejects finalize |
| Collector finds candidate before prepare, then obtains exclusive admission after it | Fresh eligibility check preserves claimed version/stream/segment and prevents physical deletion |
| Version/whole-artifact delete cascades while verifying | Pin survives; no partial parent/child deletion and no loss of provider references |
| Set body Deleting without deleting metadata | Claim guard blocks cleanup before external DELETE, not merely the later SQL DELETE |
| Grant expires while PUT continues; HEAD reports absent; purge runs | Obligation remains, Ready is not manufactured, cleanup cannot forget the writer |
| Duplicate PUT loses initial create race, then resumes after retention deletes object | Permanent atomic-key fence prevents payload recreation; unresolved staging is not declared settled |
| Revoke sponsor, policy, target owner permission, approval, or worker assignment during GET | Fresh final authorization denies; no seal/Ready/final evidence is published |
| Credential, approval, permit, or claim expires while finalizer waits for row locks | Fresh database-clock checks deny; retained pins do not extend authority |
| Storage returns wrong provider version, truncated data, or different bytes | Verification fails, including errors delivered only at EOF; nothing becomes Ready |
| Authentic encrypted prefix, wrong execution/version, altered final proof, or partial frame | Full frame verifier and exact final-proof binding reject completion |
| Crash before prepare commit, after commit, mid-GET, before final commit, or lost final response | No false Ready; bounded recovery/takeover; stale token denied; exact completed retry reconciles outcome |
| Slow GET paused by a barrier | No sealer graph holder or checked-out connection during pause; unrelated exclusive graph work can finish |
| New Ready publication fires version-retention trigger | Existing graph ordering remains valid; no shared-to-exclusive upgrade introduced |

Extend `internal_files.rs` tests, `runtime_log_replica_tests`,
`tests/support/recorded_file_completion_cases.rs`, `file_evidence_retention_tests`,
and artifact deletion/retention tests. Retain the one-connection seal/commit test
and test both race orderings with independent API replicas. Observe barriers and
PostgreSQL waiters, not fixed sleeps. Include crash recovery with a new API owner,
not only task cancellation in one process.

After implementation, use the owned PostgreSQL and versioned-storage runners in
`docs/testing/running-tests.md` and `docs/deployment/runtime-log-verification.md`.
Run targeted tests, `cargo sqlx prepare` after schema changes, workspace checks,
and applicable CI validations. This review ran none of those tests and supplies
no measured latency improvement.
