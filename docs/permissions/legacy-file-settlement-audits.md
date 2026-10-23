# Explicit legacy file settlement audits

The retention maintenance API exposes cleanup-only audits for `legacy_unknown`
operations and prospectively registered `controlled_native_v1` operations. The
[original producer adapter](native-original-producer-adapter.md) creates the latter
before any body request; it never labels a new operation as unknown history.
This API does not readmit lost source authority and is not exposed to
execution, worker or sensor SDK tokens. Each request requires an unscoped live
Access credential, current unrestricted `permissions:manage`, and the applicable
`retention:read` or `retention:update` grant. The repository reloads operator IAM;
it does not borrow an execution sponsor's privileges. Native observations come
from the registered storage management API, never operator request JSON, command
stdout, a guessed timestamp or credentials recovered from historical evidence.

## Operator invocation

All paths below are relative to `/api/v1`. Use the normal authenticated HTTP API
with the operator's Access token. No provider credentials are accepted or returned.

- `GET /retention-config/file-audits/capabilities` reports the configured backend
  scope fingerprint and management transport state. `configured_unverified` does
  not claim any writer or provider has been stopped.
- `GET /retention-config/file-audits/candidates` returns at most 128 located
  version/operation members. Resume with `after_version` and `after_operation`
  from the last row. Keep the composite cursor so multiple operations for one
  version are not skipped.
- `POST /retention-config/file-audits` accepts only `{"versions":[123]}` and
  freezes at most 256 operations whose metadata publication is already closed.
  Ordinary artifact deletion/cleanup remains separately authorized. Caller fields
  such as `writer_stopped`, namespace prefixes or a controller URL are rejected.
  An unavailable controller produces a persisted `manual_required` result here.
- `GET /retention-config/file-audits/{id}` reports immutable plan members,
  persisted cursor, state, diagnosis and receipt counts.
- `POST /retention-config/file-audits/{id}/step` performs one bounded catalogue
  or exact-cleanup step. Repeat explicitly until completed or manual-required.
  A transient failure leaves the same audit resumable. Query status after a lost
  response; never infer settlement from the lost HTTP connection.
- `POST /retention-config/file-audits/{id}/quarantine` records the authenticated
  operator's explicit choice to retain quarantine. It certifies no native fact
  and releases no pin. A new audit can later use stronger management evidence;
  the prior audit's history is not rewritten.
- `GET /retention-config/file-audits/{id}/receipts` returns at most four immutable
  proof records. Resume with `after_kind` and `after_identity` from the last row.
  These include the signed management observations for independent verification.
- `POST /retention-config/file-evidence/{version}/readmit` retries the original
  frozen evidence traversal. Missing proof stays quarantined. It cannot reopen
  metadata-gone ownership, settle a writer, or revive runtime credentials.
- `GET /retention-config/file-evidence/{version}` reports the located evidence
  diagnosis, physical cleanup state and unresolved operation count.

Step and readmission tasks share the API's file-transfer slots, have a 128 MiB
metadata allowance and a 120-second deadline, and remain tracked through socket
disconnect. API shutdown drains or cancels and joins them. Cancellation does not
mint a native receipt; operations without confirmed settlement stay pinned.

## Registered management API

For configured filesystem/SharedFile or versioned S3/RustFS topologies, configure
the owning native storage/process controller on the API:

```yaml
artifacts:
  legacy_audit_controller:
    endpoint: https://storage-management.example.internal/attune/
    issuer: storage-management
    audience: attune-legacy-file-audit
    verification_key_file: /run/attune-trust/storage-management-public.pem
    client_identity_file: /run/attune-secrets/storage-management-client.pem
    ca_certificate_file: /run/attune-trust/storage-management-ca.pem
```

The client identity PEM contains the API's management client certificate and
private key. Files are read at API startup, with bounded reads. Invalid HTTPS,
PEM or client configuration fails startup. Endpoints are operator configuration,
not request parameters. Redirects, URL credentials, query strings and fragments
are forbidden. Requests have a three-second connection and ten-second total
deadline. The independent pinned RS256 key verifies observations without changing
Attune's HMAC-only execution/access JWT provider.

The controller must map the scope fingerprint to its own configured physical
backend and SharedFile root. The fingerprint is the canonical value-schema digest
of `{"storage": <the API's storage config>, "artifacts_dir": <configured path>}`.
It must also bind native device/inode, bucket/account or equivalent physical
identities in its proof records. A matching path string alone is insufficient.
Do not select a different bucket, process scope or volume from plan keys.

The adapter makes authenticated POSTs to these management paths beneath `endpoint`:

| Path | Arguments | Signed `data` |
| --- | --- | --- |
| `legacy-file-audits/{id}/stop-and-settle` | `{}` | `NativeWriterFence` |
| `legacy-file-audits/{id}/verify-fence` | original `receipt` | reverified, identical `NativeWriterFence` |
| `legacy-file-audits/{id}/catalogue` | `fence`, persisted `cursor`, `limit` at most 128, `max_bytes` 524288 | `LegacyAuditPage` |
| `legacy-file-audits/{id}/delete-exact` | `fence`, exact `item` | `ExactLegacyAbsence` |

Each JSON request contains `nonce`, the entire frozen `plan`, `operation`, and
`arguments`. The controller returns a compact RS256 JWS as `application/jose`,
at most 1 MiB. Its claims are `iss`, single-string `aud`, `iat`, `exp`, optional
`nbf`, `nonce`, `audit`, `operation`, `request_digest`, `scope`, and typed `data`.
`request_digest` is the canonical value-schema digest of the whole JSON request.
Issuer, audience, fresh challenge, audit, operation, scope and exact request digest
must match. Observation lifetime is at most the client's 60-second freshness
window. Unverified bodies, algorithm substitution and replayed challenges fail
closed. The client persists the verified compact JWS as immutable audit evidence.
It never forwards the operator token or logs controller error bodies.

Management observation expiry is not a native settlement proof or an expiry of
the persistent writer fence. `verify-fence` must freshly revalidate the recorded
native barriers and provider drain; merely signing an echo of the supplied JSON
violates this interface. Exact deletes must be idempotent by physical identity,
not by request nonce, so a deleted object whose DB response was lost remains
provably absent on retry. Response data must contain credential-free native
record IDs and proofs, never provider credentials, file content or arbitrary logs.

## Required native capabilities

An implementation of `NativeLegacyAuditor` must establish and verify a persistent
fence before cataloguing. Its credential-free immutable receipt binds the actual
backend, plan digest, native API instances, worker instances and execution
attempts. It must prove old writers cannot restart and ALL accepted provider
requests have settled. Process exit, joined local tasks, credential expiry and
an empty HEAD alone do not prove remote request settlement. Resuming after a
restart must verify the original controller/provider evidence and fence, not
echo stored JSON or silently substitute a newer observation.

The adapter must inventory all historical object versions, delete markers,
multipart sessions and exact shared staging paths in the fixed plan. The native
catalogue completion receipt binds both plan and writer-fence digests. It is a
positive completeness observation, not an inference from an empty page. A
current-object listing such as ordinary `ObjectStore::list` is insufficient on a
versioned provider. Aborting one multipart upload without accounting for accepted
completion/part requests is also insufficient.

The old generic PUT placed random staging keys at the bucket/root GLOBAL
`.attune-upload/` namespace, even when final objects used a configured tenant
prefix. A tenant prefix is therefore not proof of staging ownership. Shared
buckets need authoritative ownership traces and a fence covering every relevant
writer; a dedicated namespace needs a verified native ownership inventory. Key
shape or a matching content digest never establishes ownership. Adapters must
classify each exact discovered identity as owned, provably unowned or unknown.
Only owned identities reach deletion. Unknown ownership blocks finalization;
provably unowned bodies are recorded and left untouched. No broad prefix delete
is used. Local deletion must also bind the configured transport root and exact
inode/file identity and retain its existing path/symlink protections.

## Persisted protocol

1. Request at most 256 legacy operations whose metadata cleanup already closed
   publication. The graph gate is exclusive before identity/member rows lock.
2. Establish the native writer/request-settlement fence and persist its immutable
   receipt. Missing capabilities produce a located `manual_required` diagnostic.
3. Catalogue at most 128 identities and 1 MiB of proof data per step. Persist the
   exact cursor, page digest, identities, provider versions/upload IDs and native
   ownership proof before advancing. Cursors are bounded and cycles are rejected.
4. After positive catalogue completion, delete at most 16 owned provider/inode/session
   exactly and persist its qualified native absence receipt. Provider failure or
   loss of a DB receipt leaves pins and progress intact for an idempotent retry.
5. Only after all owned identities are confirmed absent and none remain unknown
   may a completed audit retire its exact legacy operations. Other unresolved
   operations and cleanup-group peers still prevent release. Audit records are
   immutable and outlive the metadata, body and evidence pins.

If stronger ownership evidence later becomes available, an explicitly authorized
new audit can supersede a manual-required attempt without rewriting its history.
An in-progress audit cannot be silently superseded. Previously confirmed native
deletions remain idempotent on re-enumeration.

## Unsupported or irrecoverable history

The production adapter is `StorageLegacyController`, constructed from API storage
configuration and reached by the authenticated maintenance routes. It delegates
native management operations to the explicitly trusted owning controller. It does
not treat stock object-data APIs as a writer management service.

| Actual configured storage | What the application has | Legacy settlement path |
| --- | --- | --- |
| Filesystem blobs and SharedFile artifact paths | Root-checked paths and local object identities; no proof about unknown historical processes, open descriptors or other hosts | Registered controller must establish durable process/volume exclusion, enumerate exact local identities without following symlinks, confirm inode-qualified deletion and prove no restart. With those observations the production adapter can complete an audit. Without them it returns manual quarantine. |
| Versioned S3, including configured RustFS endpoints | API-owned credentials, versioning preflight and exact version-ID deletion; no standard S3 data API that proves all old PUT/copy/multipart requests settled | Registered controller must supply native writer exclusion and provider settlement, enumerate versions, delete markers and multipart upload IDs with persisted cursors, establish global staging ownership, and confirm exact version/upload cleanup. With those observations the adapter can complete an audit. Without them it returns manual quarantine. |
| GCS or an injected/custom BlobStore | No registered management integration in this path | Explicit manual quarantine. A custom store cannot inherit the configured native backend's proof authority by copying config. |

The controller also needs `ListBucketVersions`, version-qualified object access,
multipart listing and abort capabilities for S3/RustFS, plus native control-plane
rights to prove stopped writers and accepted-request settlement. Merely granting
those data permissions is not settlement. RustFS compatibility with S3 data
operations does not imply a provider drain or external process-management API.
The application does not grant itself power to stop arbitrary deployment
processes. A deployment-owned local filesystem controller and actual native
writer entrypoint are now shipped for explicitly initialized, journaled,
exclusive generations. See [native legacy filesystem controller](native-legacy-filesystem-controller.md)
for its kernel freeze/join, provider drain, crash recovery and exact-delete
implementation. It is disabled by default and does not adopt untracked history.
Deployments must register an owning controller with the required native scope;
absent those capabilities the safe result is manual quarantine. No cloud
provider drain implementation is implied by the local adapter.

### Native interfaces required by the Compose deployment

The repository's Compose topology mounts `artifacts_data` at
`/opt/attune/artifacts` and `blobs_data` at `/opt/attune/blobs`. API, supervisor,
workers, migration helpers and other mounted-volume clients can participate in
historical writing. The effective roots and provider come from the mounted
configuration. A current execution row, worker heartbeat, container label, or the
API's current native-instance UUID is not a complete historical writer inventory.

A production owning controller still needs deployment-specific implementations:

- An authenticated host/orchestrator management interface, such as the owning
  Docker Engine/Compose controller, that inventories every writer and its volume
  attachments, disables old-generation restart/retry, stops and joins owned work,
  and persists verifiable instance/generation records. Execution cancellation,
  `docker stop` exit status alone, and health/readiness endpoints are insufficient.
  The API service has no Docker socket mount and cannot supply this authority by
  calling its ordinary worker or file routes.
- For filesystem/SharedFile, authoritative host and mount-namespace ownership,
  open-handle/process settlement, persistent exclusion of old writers, and
  root/inode-qualified enumeration and deletion without following symlinks.
  An empty directory or a renamed path is not proof that an old descriptor can
  no longer write. Remote mounted clients must be covered too.
- For S3/RustFS, the actual provider's management or independently verifiable
  request-settlement facility covering accepted PUT/copy/multipart work, in
  addition to version/delete-marker and multipart-upload enumeration and exact
  version/upload-ID cleanup. The configured S3 data endpoint is not such a
  management service. Credential expiry, elapsed time, empty HEAD, current-object
  listing, or one successful multipart abort cannot substitute for this facility.

The [native filesystem controller](native-legacy-filesystem-controller.md) supplies
these barriers for its bounded, exclusive local generation and pinned native
writer. E owns its host/cgroup/storage isolation and Compose/Helm packaging. It
does not supply a stop/drain facility for arbitrary existing Compose containers,
shared mounts or S3/RustFS. Until those native interfaces are available for the
actual historical scope, the operator API retains located manual quarantine with
immutable decision records. A signature does not make an echo implementation a
valid auditor. Neither source/mock checks, controlled filesystem tests nor the
management-client code establish positive production-provider acceptance.

Where native/provider history cannot prove missing staging identities or scope
ownership, keep the operation quarantined. The diagnostic names the audit and
its exact version/operation members and offers explicit choices: supply a
scope-capable verified native auditor, retain quarantine, or design a separately
authorized export/isolation migration with independent physical ownership. No
database reset, checksum rewrite, blanket prefix cleanup or age-only pin release
is performed. Unrelated orphan objects already retained in their dedicated ledger
remain separately owned; this selected-version audit does not silently adopt or
collect them.

Tests use a controlled joined writer, owned filesystem objects, authenticated
maintenance routes, signed observation verification and tracked shutdown work.
They are not proof that a cloud deployment has been quiesced. No provider process
is stopped merely by configuring this client or reading capabilities.
