# Recorded worker final delivery

Recorded execution API credentials stop working at terminal status. Worker
delivery uses a separate persisted capability, not execution-token grace.
Coordinated recorded-worker readiness remains disabled pending combined validation
and completion of the other SOW consumers.

Before launching code, the worker registers its assigned worker ID, process
instance, recorded admission and current attempt. Its immutable deadline is the
pinned execution timeout plus 60 seconds. An unrepresentable deadline fails
preparation rather than receiving an unbounded delivery window.

Registration and allocation reload the execution's persisted immutable snapshot
under its admission fence. A stale caller copy or current action projection cannot
replace the owner, code metadata or timeout. Target validation locks the pinned
structural action ID without adopting its current executable metadata. Fresh log
allocation requires the still-running assigned launch context; terminal cleanup
can consume only versions and streams already bound to a permit.

Each permit identifies one preallocated artifact version and, for logs, one
stream. Version allocation and permit capture share a transaction. Log permits
allow append and seal, up to the configured stream byte limit and at most one
sequence per allowed byte. File permits capture the configured maximum upload size. They
cannot delete files, read content, emit events, sign assertions or create children.
The delivery credential never enters the action environment. Empty API selections
still get worker-only delivery context and log permits, without an execution API
credential.

Every reserve, segment commit, seal and final ready-state publication rechecks
`ExecutionAdmissionRepository::require`. Policy, sponsor and approval revocation
therefore remain effective after completion or cancellation. Assignment, instance,
current attempt, deadline, exact version and captured structural artifact owner
must still match. A general Worker JWT cannot mutate a recorded version.

Object publication reserves metadata, uploads outside the database transaction,
then opens a new authorization transaction before marking the body ready. A
revoked upload can leave an unreferenced object for the existing maintenance
ledger, but cannot publish it as ready. Identical retries consume no extra log
budget.

Shared-volume append holds the source fence, artifact advisory lock, version and
stream locks through file I/O. Seal takes those locks before the file lock too.
Generic file completion rejects runtime logs, so it cannot unlink a sealed shared
log as though it were disposable staging. This does not sandbox processes with
direct access to a shared volume.

Recorded process file versions retain their own immutable source closure. Under
C's shared capture fence, reservation collects retained execution input/config
and result origins, legacy SDK Key-read receipts and generic runtime reads. Each
receipt retains its entity, field path, source kind/ref and contributors. Mutable
parents are snapshotted by C's bounded origin evaluator; later values are never
merged into the retained graph. Unsupported/unbound sources still fail closed.

The version's `artifact_file_origin` row binds the admission/attempt, actual
target owner, captured plaintext cap and receipt/closure digest to an immutable
C-format `RuntimeReadOrigin` row. Its `file_sink_version` marker excludes sink
snapshots from the producer's acquired-read set and budget, preventing a sink
from becoming its own source. Normal acquisitions remain counted and bounded.

Both shared staging and API/object uploads publish whole-file ciphertext using
the platform secret-store key. The authenticated envelope binds the exact
artifact-version ID and origin-row ID and preserves arbitrary binary bytes.
`artifact_file_body` retains signed plaintext/ciphertext lengths and SHA-256
digests. Version storage size/digest describe ciphertext; authorized HTTP ranges
describe the authenticated plaintext. Protection does not depend on a mutable
artifact visibility/classification or user-supplied version metadata.

Publication revalidates the source/target fence and exact receipt set before
storage and again before Ready, after external PUT. A new read or changed input
receipt requires a new version; the old body is never relabeled. Pending encrypted
bytes are retained in `artifact_file_upload` for byte-identical retries across API
instances and removed in the Ready transaction. This duplicates bounded ciphertext
temporarily in PostgreSQL; plaintext buffers are limited by the captured upload
cap. Existing upload-grant/object quotas account for the actual ciphertext body.
Direct upload URLs are not issued for origin-bound files: the proxy must encrypt
and authenticate the bytes. Shared plaintext staging is removed on successful
completion and is never served through a Pending protected version.

Recorded readers consume the exact file snapshot through C's generic registry,
not the producer's later execution-wide read set. Human readers check current
Read/Decrypt grants against the retained graph and actual target owners, including
at body delivery after blob I/O. Missing/expired evidence denies; it never converts
the file into an unprotected/public body. Database guards reject proof removal,
body/source rebinding, inline plaintext substitution and mismatched Ready proofs.
Metadata-only file changes retain target mutation checks without acquiring body
origins. Inline artifact data mutations keep C's separate sink guard.

Admitted file APIs reject arbitrary caller `meta` JSON and `created_by` labels:
those are not part of the encrypted body projection and must not become a second
plaintext JSON/SDK-response payload channel. Put origin-bearing content in the
file body. Ordinary artifact metadata-only updates remain separate. A future
typed/version-metadata payload needs its own encrypted, origin-bound projection;
it must not silently reuse the file proof while public DTOs return plaintext.

Worker final delivery remains a separate, exact permit authority. It may validate
the captured file closure after acquisition has closed, but cannot reopen the
registry, ignore new reads, allocate a fresh version or strip origins. Runtime
logs retain their separate encrypted frame/manifest protocol; generic file
reservation rejects runtime-log classification and log-stream versions.

Approval expiry is retained separately from the stored permit. At source
validation, `WorkerDeliveryRepository::require` captures A's opaque
`ApprovalEffectFence` and returns a `DeliveryEffectFence` alongside context and
permit. The latter also retains the existing context deadline. It neither adds
grants nor relaxes worker, instance, attempt, version, stream or operation caps.
Launch/context registration and binding carry the source fence across allocation
waits. Process file effects reuse their source-stage validated authority; they
do not validate approval/source rows again behind target locks.

API reserve/commit/seal/Ready transactions check the retained fence with the DB
wall clock after target, capture, hash and manifest waits, before committing.
External PUT is followed by a fresh source transaction. Shared append acquires
the real file lock before its final clock check; the check and byte write run in
the same blocking task, after its scheduling wait. Shared seal inspection does
not freeze a file until the final manifest and liveness checks pass, so rejecting
a valid prefix does not prevent appending the remaining frames. Context deadlines
still apply even when A's entire retained approval/dependency set has no expiry.

This source requires C's globally ordered source/capture/target entry fence. The
file helpers do not replace C's lock-order work or authorize direct access by an
untrusted process to other processes' shared filesystem staging. Runtime,
migration, race, memory and combined-reader acceptance remain centrally gated.

Encrypted logs reserve ciphertext limits with
`runtime_read_log::maximum_encoded_bytes`, including truncation-notice allowance,
and retain a separate plaintext allowance. The encrypted writer sends a final
authenticated manifest through `seal_log_stream_with_manifest` in the
`x-attune-log-final-manifest` header. Its seven serialized fields match the C/F
contract: encoding, execution, version, frame_count, plaintext_bytes,
ciphertext_bytes and ciphertext_sha256. Counts and digest cover the concatenated
ciphertext frames including their newlines, not transport segment boundaries.

Seal authenticates this manifest, verifies every complete frame with the common
codec and checks the final counts/digest against the exact stored stream. Missing,
spliced, extra, unterminated or valid-prefix-only streams cannot become Ready.
The authenticated string is written to
`artifact_version.meta.encrypted_log_manifest` and an immutable attributed proof
row in the same seal/Ready transaction. Database guards prevent later replacement
or adding a proof to a Ready version. Exact replay remains idempotent. C owns the
separate decoder and disclosure checks; the proof itself grants no read authority.

Recorded log roots include the pinned action ID. Ref deletion/reuse cannot assign
old logs to a replacement action. API discovery uses the same root calculation;
existing disclosure checks remain responsible for reads.

Unbound legacy execution completion is not converted by this capability. The
coordinated rollout must drain or deny that path rather than infer authority from
old execution metadata. Candidate credentials are not delivery credentials.

## File route audit follow-up

Internal GET resolves the registered version and tries the protected reader
before the legacy file-transfer gate. Internal HEAD uses the same reader,
authenticates the full body, polls its delivery guard, then returns an empty body
with the plaintext length. It does not advertise ciphertext length or accept a
plaintext range as a shortcut around whole-file authentication.

Protected responses recheck source disclosure and the exact Ready/body binding
after blob loading/authentication, then after each bounded plaintext chunk and
at successful EOF. The chunk/EOF loop consumes C's `guarded_chunks` contract.
GET omits Content-Length so transport completion cannot bypass the EOF check.
Ranges still refer to the fully authenticated plaintext. These checks do not
prove socket readiness or prevent a transport from queuing an authorized frame.
C owns that transport contract.

File-backed detail projections omit inline JSON, arbitrary metadata and
`created_by`. File history omits arbitrary `created_by`; recorded history omits
that payload regardless of storage shape. Parent-type-aware detail projection
also covers old inline-file rows with no storage marker. Metadata projections
must not become alternate file-body channels. They do not acquire new body
origin receipts merely to serialize these omitted fields.

New file staging paths include the globally allocated version ID, not only a
reusable display ref/version number. `artifact_file_staging_guard` quarantines
origin-bound paths after version deletion. Internal routes deny orphaned
quarantined paths and unregistered paths in the reserved `files/versions/`
namespace. The native supervisor releases a quarantine only after physical
staging deletion and an absence check. This is path protection, not evidence
retention, and it does not sandbox direct volume access.

Historical evidence retention is still incomplete. Execution/admission cleanup
can remove registry evidence while dependent files survive; failing closed on
those files does not satisfy retention. See
[the retention and delivery audit](file-origin-retention-audit.md) for the
remaining caller contracts. These changes have source/static checks only.
