# File origin retention and delivery audit

Source audit only. No build, migration, database, transport or behavior test was
run in this worktree. Readiness remains disabled. This document does not claim
complete retention or complete delivery fencing.

## Changes in this follow-up

- Internal GET reaches `protected_file_response` before legacy transfer checks.
- Internal HEAD authenticates a protected body and runs the post-blob delivery
  guard before returning plaintext size with no response body.
- Protected GET ranges select authenticated plaintext. Bodies yield at most
  64 KiB per chunk and consume C's chunk and verified EOF guard. Content-Length
  is removed from protected GET so completion requires the final stream poll.
- The file-specific callback validates the saved registry reference, exact
  Ready version/object/provider/digest/manifest and credential expiry. It does
  not replace C's origin evaluator or add a source grant.
- Detail/history serializers suppress alternate JSON payload fields for files,
  including parent-type-aware suppression for old inline-file detail rows.
- Unique staging paths and a durable path quarantine prevent raw internal
  fallback from exposing orphaned plaintext after proof/version deletion.

The common `DeliveryEffectFence` also checks both retained deadlines at its last
DB-clock sample after invoking A's opaque fence. Neither a diagnostic deadline
nor a cached timestamp replaces opaque fence validation.

## Route coverage and limits

Exact and latest public downloads, recorded downloads, internal GET/HEAD and
finite file SSE converge on the protected reader. Public by-ref allocation and
multipart upload still publish through the existing encrypted proof path.
File detail/latest/history serializers are metadata projections, not byte
readers. Inline `data`/JSON admission and arbitrary admitted file metadata stay
denied by the earlier file hooks. File `as_file` helpers must keep routing bytes
through the existing allocation/PUT/completion path, not serialize the bytes
into an artifact DTO.
No SDK-specific `as_file` implementation was found in this checkout. Its caller
behavior remains an external integration contract, not verified route coverage.

Raw unregistered legacy paths outside the reserved namespace still use legacy
transfer authorization. New origin-bound staging remains inaccessible there
after metadata deletion because its quarantine outlives the version. Direct
native filesystem access is outside these HTTP checks. Log delivery, live log
origin revisions and Execution SSE remain C's implementations and must survive
integration unchanged.

## Retention implementation, not yet validated

Migration `20261015006005_file_evidence_retention.sql` and
`FileEvidenceRetentionRepository` now connect reservation to durable evidence
ownership. The owner retains the original reservation and bounded frozen C
graph, independent of artifact metadata. Its pins cover exact event tuples,
admissions, schema/configuration/sensor/inquiry evidence and code dependencies.
Reservation holds shared `named-type-graph` admission before source/capture/target
locks. Reclamation takes the same gate exclusively before those locks. Delete
statement triggers enter that gate before row triggers; callers with earlier
source or target locks must enter explicitly at transaction start.

Manual artifact deletion, supervisor version cleanup, storage claims, execution
retention, pack/configuration retention and partition expiry participate. An
outside file prevents source-version cleanup. Explicit bounded whole-owner
requests can clean self-dependencies and cycles without absorbing outside files.
Cycle members release together, only after every member's metadata, object and
staging obligations complete. Pinned event leaves cannot expire, but bounded
unpinned row expiry still scans those leaves through the partition parent.

Object provider completion and native staging absence complete separate durable
obligations. Provider failure does not release pins. PUT operations are recorded
before external I/O so cleanup cannot overtake an in-flight upload. Reservations
also create object-ledger candidates before any PUT, allowing never-uploaded
bodies to receive a native absence observation. Unresolved PUTs have no timeout
that releases evidence. New protected PUTs use one native atomic create-only
request and an HTTP-independent owned transport task. After metadata cleanup,
the supervisor deletes payload providers exactly and installs a permanent
non-content provider sentinel. This fences late creates before evidence can be
released. A lost settlement receipt is reconciled against the same saved
key/token/provider identity. Ledger states and DB retirement guards prevent old
maintenance binaries from deleting sentinels or treating cancellation as proof.
Sentinels and their small control rows remain reserved indefinitely; they contain
no file body or authorization graph. Native provider and memory acceptance is
still required. Generic log/pack PUT is unchanged.

Migration 06006 seeds existing reservations as pending audit while retaining
their exact physical obligations. Supervisor readmission runs the same bounded
frozen graph traversal under exclusive graph admission and a savepoint. Missing
or corrupt proofs remain quarantined with a version/node diagnostic and cleanup
ownership intact. It never reconstructs authority from current mutable parents.
Old orphan staging and object-ledger ownership are also preserved. The original
06005 bytes are preserved exactly. A pre-06005 database with reservations first
uses the explicit, writer-drained `scripts/prepare-file-evidence-upgrade.sql`
preflight. It parks the original table with its constraints/triggers intact,
leaves a write-rejecting empty view for the existing 06005 guard, and 06006
restores the original table before seeding quarantine. Do not start applications
between preflight and forward migration. Already-applied 06005 databases and
fresh schemas need no parking. No reset or applied-checksum update is involved.

Old generic remote PUTs did not record their random multipart staging identity.
Migration 06007 marks those operations `legacy_unknown` and withholds automatic
settlement. They still require an original-writer/provider staging audit; neither
process death, cancellation nor HEAD absence can prove that unknown remote
staging was cleaned. That historical recovery limitation remains open.

C's newer event evidence and live-log/metadata read variants
are prerequisites present on primary but absent from this older task checkout.
No compile, migration, positive historical-reader or concurrency test has run.
Readiness remains off.

### Original failure and acceptance contract

`artifact_file_origin.origin_read` is a plain ID. Its referenced row belongs to
an admission whose deletion cascades through `execution_runtime_read_origin`.
`ExecutionRepository::delete` and `RetentionRepository::delete_executions_sql`
can remove the producer/ancestor execution and its schema/secrets while a file
still needs them. The file then denies disclosure. That is not retained evidence.

Keeping only the root registry row is insufficient. The saved graph can need:

- Ancestor registry rows and their owning admissions/capture evidence.
- Execution rows, retained runtime schema and input/result/workflow provenance.
- Artifact/version identity and original structural owner evidence.
- Component configuration revisions, sensor generation/delivery bindings and
  their retained proof material, rather than current policy pointers.
- Inquiry/schema evidence and its original execution association.
- Exact event snapshots plus the actual `(event.id, event.created)` row where
  C's current disclosure resolver still requires that row.

The exact event requirement is visible in C's
`EventOriginRepository::human_scopes`. A snapshot pin alone does not retain its
JOIN target. Row DELETE guards alone also cannot stop partition DETACH/DROP.
Never add an event FK or interpret an event numeric ID as globally unique.

### Required acquisition contract

A future common retention repository must accept the actual file reservation
transaction, version/cleanup identity and immutable root reference. It must
traverse the frozen parents, not reload and union mutable current parents, and
retain the concrete evidence rows above. Use C's bounded graph rules, including
cycle/depth/node/byte limits. Schema/proof dependencies count toward the bound.
Keys and current operational identities must remain revocable, not frozen as
grants. No runtime-ID FK may keep a process credential usable.

Acquisition and deletion need one documented serialization protocol. A predicate
that merely checks for existing pins races a concurrent acquisition. Every
execution/admission/schema/configuration/inquiry/artifact/version deletion and
event partition drop must participate. Do not take source/admission/approval
locks behind file targets to repair the race. C owns the global
source/capture/target order, so integrate this protocol there before adding
post-target retention locks.

Repository caller changes must cover `ExecutionRepository::delete`,
`RetentionRepository`, `ComponentConfigurationRepository` cleanup,
`MaintenanceRepository` artifact version selection/deletion,
`StorageMaintenanceRepository` claims and manual artifact/version DELETE.
Physical body deletion must not happen before dependency eligibility is decided
under the same protocol. Cleanup predicates should skip pinned candidates rather
than repeatedly select them or delete their secrets from a pre-delete list.

### Required release contract

A metadata-owned FK cascade is not sufficient. API object-version deletion
enqueues provider cleanup, then drops metadata; the blob can remain until the
supervisor finishes deletion. A pending staging body can also outlive metadata.
Retained evidence must survive those waits and failed cleanup retries.

Use a durable cleanup identity that outlives artifact metadata, with the exact
object key/provider version and staging obligations. Release evidence only after
all corresponding physical cleanup succeeds. Connect object completion at
`ObjectMaintenanceRepository::complete_delete` and staging completion after the
native transport confirms absence. The staging quarantine added here is not
that evidence owner. Handle self-dependencies and same-artifact dependency
chains without preventing legitimate whole-owner cleanup, while preserving
external dependent files. Resolve dependency cycles explicitly.

The original audit above explains why preserving a registry row alone is not
enough. The new source must pass the positive historical-reader and physical
cleanup tests before this audit can be closed.

## C transport caller contract

This work consumes `recorded_reads::guarded_chunks` from C `78eca202` and A's
opaque expiry APIs from `92165cc0`; it does not copy those prerequisites into
this older tree. Keep primary's newer C graph/capture ordering and its
`EffectTransaction` semantic merge when applying these file-specific hooks.

Chunk polling is not socket readiness. C's HTTP adapter must run a new
source/capture/target transaction after the last actual transport readiness wait,
validate the exact retained file body pin and original consumer, check the
retained approval/context/JWT deadlines after its last query, then enqueue the
header/body/control frame without another await. EOF/Done needs its own checked
completion path. A prebuilt response, successful blob authentication or a guard
before a later socket wait does not establish this boundary.

For HEAD, the same contract applies to headers even though Hyper suppresses body
polling. For finite SSE, preserve protected file/log bodies and emit `content`
once, then `append`; errors must never become `done`. C's existing transport
implementation owns the readiness integration, not another F socket framework.

## Regression sources and unrun acceptance

The existing
`protected_a_to_file_b_retains_sources_encrypts_and_denies_public_bypass` source
now checks positive recorded internal GET, plaintext Range, protected HEAD,
serializer suppression and orphan quarantine after version deletion.
`protected_file_chunks_and_successful_eof_recheck_source_and_ready_pin` checks
bounded chunks and denial after committed source revocation, Ready cleanup, and
revocation immediately before EOF. These sources have not been compiled or run.

Still required centrally: real blob-read barriers with live/expired controls;
ordinary-human identity/grant revocation between file chunks; actual HTTP
socket-readiness and HEAD barriers; full route/SDK `as_file` tests; retention
before cleanup and release after provider/staging cleanup; acquisition/delete
races and partition drops; large binary/UTF-8 file SSE and memory/load checks.
The new staging migration has not been executed. Earlier PG18 migration success
does not cover it or any of this follow-up's behavior.
