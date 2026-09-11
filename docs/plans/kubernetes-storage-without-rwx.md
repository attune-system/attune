# Kubernetes storage without RWX

Status: Draft

[Open the HTML architecture summary](kubernetes-storage-without-rwx-architecture.html).

## Decision

Attune should not require a `ReadWriteMany` storage class for Kubernetes.

Use these storage contracts instead:

| Data | Durable storage | Pod storage |
| --- | --- | --- |
| Pack releases | Immutable S3 or GCS archives | Extracted per-pod cache |
| Runtime environments | None | Per-worker cache |
| Completed artifacts | Immutable S3 or GCS objects | Bounded upload staging |
| Active logs | Immutable object segments | Small write buffer |
| Pack and artifact metadata | PostgreSQL | None |

PostgreSQL remains the authority for active pack releases, artifact versions,
upload state, retention state, and object locations. Object storage holds bytes.
Pods use `emptyDir` or a pod-specific `ReadWriteOnce` volume for mutable files.

Do not make a mounted bucket Attune's permanent storage contract. GCS FUSE and
Mountpoint for Amazon S3 omit filesystem operations that Attune currently uses.
Amazon S3 Files supports those operations, but it is an EFS-backed NFS service
with another cost and failure model. It is a useful AWS migration option rather
than the cross-cloud design.

## Current shared storage

The chart has three claims that become RWX requirements when their consumers
run on different nodes.

| Claim | Current consumers | Why it is shared |
| --- | --- | --- |
| `packs` | API, executor, action workers, sensor workers, `init-packs` | The API mutates the canonical pack tree. Other services read it. |
| `runtime-envs` | API, action workers, sensor workers, `init-packs` | Workers create and reuse dependency environments in one directory tree. |
| `artifacts` | API, executor, supervisor, action workers, `init-packs` | Workers and the executor write files. The API reads them. The supervisor deletes them. |

The released chart defines these claims in
`../attune-charts/charts/attune/templates/pvc.yaml`. Their values are in
`../attune-charts/charts/attune/values.yaml`. The checked-in deployment setup
requests RWX from Longhorn in `../attune-charts/attune-setup/values.yaml`.

The sharing is broader than the durable data requires:

- Runtime environments are disposable caches. Sharing them also creates a
  cross-pod race because the setup lock in
  `crates/worker/src/runtime/process.rs` is process-local.
- Workers already download pack archives through
  `crates/common/src/pack_transport/` when they do not share the API pack path.
- Workers and sensors already support API-backed artifact transfer through
  `crates/common/src/artifact_transport/`.
- The executor needs the artifact claim for workflow logs, not general
  artifact ownership.
- The supervisor needs the artifact claim only because it constructs a
  filesystem transport for retention deletion.

## Mounted bucket options

The products in this area do not provide the same behavior.

| Product | Interface | Filesystem behavior | Suitable Attune use |
| --- | --- | --- | --- |
| GKE Cloud Storage FUSE CSI | FUSE over GCS | Non-POSIX, no file locks or patching, cache-dependent visibility | Read-only archives and unique write-once files |
| Mountpoint for Amazon S3 CSI | FUSE-style client over S3 | Sequential writes, limited overwrite, no general-purpose bucket rename or append, no locks | Read-only archives and unique write-once files |
| Amazon S3 Files | NFS 4.1 and 4.2 backed by EFS with S3 synchronization | Read/write files, POSIX permissions, advisory locks, close-to-open consistency | AWS-only compatibility bridge for the current application |
| Native S3 or GCS API | Object API | Atomic object publication, conditional writes, explicit versions | Permanent durable storage |

### GKE Cloud Storage FUSE CSI

Cloud Storage FUSE is viable when each file is immutable or has one writer that
replaces the whole object. It is not a general shared filesystem.

Its limits matter to Attune:

- It does not support file locking or file patching.
- Different mounts must not write the same object. A losing writer can receive
  `ESTALE` when it flushes.
- An in-place update can upload the whole object again. Append avoids that only
  for supported files and access patterns.
- Directory rename is atomic only with hierarchical namespace buckets.
- Object versioning is not formally supported by Cloud Storage FUSE.
- A writable mount cannot use a bucket retention policy.
- New or modified objects can consume local temporary storage until close or
  sync.
- Small-file and metadata-heavy access has much higher latency than local disk.
- Stat, list, file, and negative caches can hide another client's changes.
- Each mounted pod gets a sidecar. CPU, memory, and cache storage must be part
  of pod sizing.
- Node-restart recovery depends on the GKE and driver version. Older versions
  require pod replacement.

These limits rule out virtual environments and `node_modules`. They also make
active log append, pack-tree replacement, and multi-pod pack authoring risky.
A hierarchical namespace improves pack directory operations, but it does not
add locking or make package-manager workloads suitable.

Cloud Storage FUSE does fit an optional read-only projection of immutable pack
archives or completed artifacts. If Attune uses it:

- Use a hierarchical namespace bucket.
- Mount an Attune-specific prefix rather than the whole bucket.
- Keep mutable caches on `emptyDir`.
- Disable or shorten negative and metadata cache TTLs where another pod creates
  objects. Use long TTLs only for immutable prefixes.
- Set sidecar requests and limits explicitly.
- Use Workload Identity Federation and uniform bucket-level access.
- Pin a minimum GKE version that supports the required restart and namespace
  behavior.
- Monitor request count, sidecar memory, cache pressure, `ESTALE`, and `EIO`.

Google documents the limitations in the
[Cloud Storage FUSE overview](https://cloud.google.com/storage/docs/cloud-storage-fuse/overview)
and the
[GKE CSI driver overview](https://docs.cloud.google.com/kubernetes-engine/docs/concepts/cloud-storage-fuse-csi-driver).

### Mountpoint for Amazon S3 CSI

Mountpoint is closer to an object client than an NFS filesystem. It works well
for large reads and new sequential writes.

For S3 general purpose buckets:

- Writes must be sequential and have one writer.
- Overwrite requires truncate mode and an explicit mount option.
- Append and rename are not supported.
- Directory rename, file locks, `chmod`, `chown`, hard links, and symbolic links
  are not supported.
- Separate mounts do not coordinate writes to the same object.
- A new object becomes visible only after upload completion on close or
  `fsync`.
- Cache TTLs can expose stale object data and metadata.
- The EKS driver supports only static provisioning and does not support
  Fargate.

S3 Express One Zone adds append and single-file rename, but it does not provide
the full shared-directory behavior that pack installation and runtime setup
need.

Mountpoint can expose immutable pack archives and completed artifacts. It
should not hold runtime environments, active logs, or the mutable canonical
pack tree.

AWS documents these details in
[Mountpoint filesystem behavior](https://github.com/awslabs/mountpoint-s3/blob/main/doc/SEMANTICS.md)
and the
[EKS CSI considerations](https://docs.aws.amazon.com/eks/latest/userguide/s3-csi.html).

### Amazon S3 Files

S3 Files is distinct from Mountpoint. It uses EFS to provide NFS 4.1 and 4.2
semantics and synchronizes filesystem changes with a general purpose S3 bucket.
It supports shared mutation, POSIX permissions, and advisory locks. This makes
it the only bucket-related option reviewed here that can plausibly run the
current Attune filesystem code without redesign.

The compatibility comes with costs and operating constraints:

- Active data occupies a high-performance EFS-backed tier. AWS charges for that
  storage and its reads and writes in addition to S3 storage, requests, and
  synchronization traffic.
- Changes written through NFS reach S3 asynchronously. Native S3 readers can
  lag behind the filesystem view by seconds or minutes.
- Changes made directly through S3 can take time to appear through NFS.
- Direct S3 and filesystem writers need a conflict policy. They must not own
  the same key prefix during normal operation.
- The linked bucket must have S3 Versioning enabled. Version retention and
  cleanup contribute to storage cost.
- At least one mount target is required. Create one in every Availability Zone
  where Attune runs to avoid cross-zone traffic and a single-zone dependency.
  Network paths, security groups, DNS, and port 2049 become application
  dependencies.
- The service is scoped to one VPC per filesystem.
- Locks are advisory. Applications still need correct coordination.
- S3 ACLs are not preserved after filesystem changes. Use IAM and filesystem
  access points instead.
- Archival S3 classes are unavailable until objects are restored.
- File paths must remain within S3 key and path-component limits.
- S3 Files has separate limits for connections, open files, and locks.

S3 Files is viable as an AWS deployment profile while Attune moves away from
shared mutable paths. It does not solve GKE portability, and it may not solve
the original cost problem. Benchmark it against EFS directly with Attune's
small-file count, active data set, log write rate, and S3 synchronization
requests before adopting it.

AWS describes the architecture and billing in the
[S3 Files overview](https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-files.html)
and its limits in
[Unsupported features, limits, and quotas](https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-files-quotas.html).

## Target storage model

### Store immutable bytes

Object storage should expose object operations, not filesystem operations:

```rust
pub struct ObjectKey(String);

pub struct StoredObject {
    pub key: ObjectKey,
    pub provider_version: Option<String>,
    pub size: u64,
    pub sha256: [u8; 32],
}

pub enum PutCondition {
    CreateOnly,
    MatchVersion(String),
}

#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn put(
        &self,
        key: &ObjectKey,
        body: ByteStream,
        condition: PutCondition,
    ) -> Result<StoredObject, BlobStoreError>;

    async fn get(
        &self,
        key: &ObjectKey,
        provider_version: Option<&str>,
        range: Option<ByteRange>,
    ) -> Result<BlobReader, BlobStoreError>;

    async fn head(&self, key: &ObjectKey)
        -> Result<Option<StoredObject>, BlobStoreError>;

    async fn delete(
        &self,
        key: &ObjectKey,
        provider_version: Option<&str>,
    ) -> Result<(), BlobStoreError>;
}
```

Implement `S3BlobStore` and `GcsBlobStore`. Keep provider ETags, generations,
version IDs, request IDs, and retry rules inside those adapters.

Use immutable keys:

```text
v1/<deployment>/packs/blobs/sha256/<digest>.tar.zst
v1/<deployment>/artifacts/<artifact-id>/versions/<version-id>/body
v1/<deployment>/logs/<stream-id>/segments/<sequence>
```

Use S3 conditional writes and GCS generation preconditions. Do not overwrite a
`latest` object. PostgreSQL stores the pointer to the active pack release or
ready artifact version.

### Publish packs as releases

Replace the mutable canonical pack tree with immutable releases:

1. Extract and validate a candidate in API-local staging storage.
2. Create a deterministic archive and SHA-256 digest.
3. Upload the archive with create-only semantics.
4. Test the exact digest.
5. Record the release and change the active-release pointer in one PostgreSQL
   transaction.
6. Emit the release ID and digest in the pack event.
7. Download and verify that digest in each consuming pod.
8. Extract into a temporary local directory, then rename it into the pod-local
   cache.

Executions and managed sensors should pin a pack release. This requires more
than adding a digest to the pack event:

- Add an immutable pack release reference to the execution snapshot and the
  managed sensor assignment.
- Resolve action and runtime metadata from that release instead of the current
  mutable rows.
- Include the release ID and digest in dispatch and `pack.registered` messages.
- Change `PackFileTransport` to fetch by release ID or digest, not only by
  `pack_ref`.

A later activation must not change metadata or code beneath queued or running
work.

Workflow authoring currently writes into an active pack directory. In the
target model, it writes a draft and publishes a new immutable pack release.

### Keep runtime environments local

Use an `emptyDir` by default. Offer a pod-specific RWO cache for pools where
dependency installation is expensive. A persistent cache requires one claim
per replica. Implement that option with a StatefulSet claim template or a
generic ephemeral volume rather than mounting one RWO claim into a Deployment
with several replicas.

Key each environment by:

- Pack digest.
- Runtime and interpreter version.
- Dependency manifest digest.
- Architecture, operating system, and libc.
- Worker image or installer format version.

Build into a temporary sibling directory. Mark it ready only after validation,
then rename it locally. A restart can cause a cache miss, but it must not affect
correctness.

### Upload completed artifacts

Keep `ATTUNE_ARTIFACTS_DIR` as pod-local staging so existing actions can write
files. On finalization, stream files with S3 multipart upload or GCS resumable
upload. Record the provider version, size, and digest before marking the
artifact version ready.

Workers should authenticate to Attune rather than receive bucket-wide cloud
credentials. The API can proxy the stream or issue a short-lived upload session
for one object after checking execution permissions.

### Segment logs

Do not append to one object. Buffer logs locally and upload ordered immutable
segments. Each segment has a stream ID, sequence, byte range, and digest.
PostgreSQL records committed sequence numbers. A final manifest records the
segment count, total bytes, truncation state, and completion state.

This model supports retries without duplicate bytes and avoids whole-object
rewrites. Execution completion must flush and seal stdout and stderr before the
artifact versions become ready.

## Failure handling

The database and object store cannot share one transaction. Represent that gap:

- Reserve an upload as `pending` before writing bytes.
- Mark it `ready` only after upload completion and verification.
- If an upload result is unknown, use `HEAD` and compare its version, size, and
  digest before retrying.
- Collect unreferenced objects after a safety window.
- Reconcile ready rows with missing or mismatched objects.
- Make deletion idempotent. Record `deleting`, delete the object, then remove or
  tombstone the metadata.
- Pin every range read to the recorded S3 version ID or GCS generation.
- Do not use bucket listing in a correctness path.

## Security and operations

- Use EKS Pod Identity or IRSA and GKE Workload Identity Federation. Do not put
  static cloud keys in Kubernetes Secrets.
- Give bucket access to the API service account by default. Keep cloud
  credentials out of action runtime pods.
- Separate pack, artifact, and log prefixes. Restrict IAM to one deployment
  prefix.
- Use KMS or CMEK when required.
- Keep the bucket in the cluster region. Alert on cross-region transfer.
- Enable S3 Versioning or GCS soft delete for operator recovery, but do not use
  provider versions as Attune's logical version model.
- Align bucket lifecycle rules with database retention. PostgreSQL point-in-time
  recovery must not restore references to already deleted objects.
- Expire abandoned multipart uploads and pending records.
- Record provider request IDs, operation duration, retries, bytes, and failures.
  Never log signed URLs or artifact content.
- Put limits on local pack caches, runtime caches, artifact staging, and log
  buffers. Alert before eviction or disk pressure stops executions.
- Load-test rollout stampedes. Many pods can request the same pack at once.

## Helm direction

Keep one Attune application chart. RWX-free deployment changes storage,
identity, init containers, and configuration, but it does not define a different
application topology. A separate `attune-rwx-free` chart would duplicate every
workload template and let the two variants drift.

Add one release-wide storage mode instead of inferring behavior from sentinel
files:

```yaml
storage:
  mode: object # object | sharedVolume
  object:
    provider: s3 # s3 | gcs
    bucket: ""
    prefix: ""
    region: ""
    endpoint: ""
    kmsKey: ""
  local:
    packs:
      sizeLimit: 2Gi
    runtimeEnvs:
      persistence: emptyDir # emptyDir | statefulSetClaim | genericEphemeral
      sizeLimit: 10Gi
      storageClassName: ""
    artifactStaging:
      sizeLimit: 20Gi

compatibilityMount:
  enabled: false
  type: gcsFuse # gcsFuse | mountpointS3 | s3Files
  purpose: readOnlyArtifacts
  mountOptions: []

serviceAccounts:
  api:
    annotations: {}
```

Do not support a different durable storage mode for each service. Mixed modes,
such as an object-backed API with shared-volume workers, create combinations
that require separate correctness and upgrade testing. `storage.mode` applies
to the whole Helm release.

Worker pools may choose `emptyDir` or a pod-specific RWO volume for runtime
environment caches. That choice changes rebuild time, not the durable storage
contract. It is safe to configure per pool after both options pass the same
cache-correctness tests.

In `object` mode, the chart should:

- Stop creating the three shared claims.
- Mount separate `emptyDir` volumes into each consumer.
- Remove filesystem `wait-for-packs` init containers.
- Start the API without requiring an active core-pack release.
- Make the bootstrap job wait for API health, then publish the built-in pack
  release through an idempotent API call.
- Make the executor and workers wait for the active core-pack release after the
  bootstrap call succeeds.
- Grant object-store identity only to services that need it.
- Keep compatibility mounts opt-in and provider-specific.

The chart should configure bucket names, prefixes, service accounts, and IAM
annotations, but it should not create cloud infrastructure. Terraform or a
provider-specific infrastructure package owns buckets, workload identity
bindings, KMS keys, clusters, networks, databases, and message brokers. This
keeps cloud provisioning separate without creating a second Attune application
chart.

## Delivery plan

The storage backend is not the first change. Attune must pin work to immutable
pack releases before moving pack bytes to object storage. The API transports
also need correctness fixes before the chart sends normal worker traffic
through them.

The repositories contain two versions of the chart. The chart in
`../attune-charts/charts/attune` is newer and has stronger rendered-manifest
checks than `charts/attune`. Choose one chart as the release source before
changing storage values. The milestones below assume `attune-charts` becomes
that source and the application repository stops publishing its embedded copy.

### 0. Establish one Helm chart source

- Make `../attune-charts/charts/attune` the supported chart.
- Remove chart packaging from the application repository after consumers use
  the chart repository.
- Keep this milestone free of storage behavior changes.
- Require strict Helm linting, schema validation, and kubeconform checks for the
  default and external-service profiles.

This milestone is complete when one repository owns chart releases and CI does
not validate or publish a second copy.

### 1. Repair API artifact transport

Fix the current transport before enabling it for Kubernetes workers:

- Replace detached `ApiBufferedWriter` flush tasks with ordered writes.
- Make `poll_flush` and `poll_shutdown` wait for HTTP completion and return
  failures.
- Preserve a short final buffer instead of losing it when the writer drops.
- Map only HTTP 404 to a missing file. Return authentication, authorization,
  server, and network failures.
- Propagate a failed initial delete from `create_writer`.
- Either implement the `open_reader` offset contract in the internal download
  route or remove the unsupported contract.
- Make live execution-log reads follow promotion from `_pending` to the final
  artifact path.

The main files are `crates/common/src/artifact_transport/api.rs`,
`crates/api/src/routes/internal_files.rs`,
`crates/api/src/routes/executions.rs`, and
`crates/worker/src/runtime/log_writer.rs`.

Tests must delay and fail individual HTTP requests. They must prove byte order,
flush completion, final-buffer delivery, and error propagation.

### 2. Make transport selection explicit

- Add an explicit pack transport setting beside the artifact transport setting.
- Require `volume` or `api` in deployed worker and sensor configuration.
- Return a startup error when API mode lacks an API URL or worker token.
- Remove fallback from API mode to `VolumeTransport`.
- Stop using sentinel presence as the production configuration contract.

Keep the sentinels only while local Docker profiles still use automatic
detection. Delete them when those profiles select their transport explicitly.

This milestone is complete when a bad API transport configuration fails at
startup and cannot silently read or write pod-local files as though they were
shared.

### 3. Make sensor reconciliation replica-safe

The sensor lifecycle queue is a competing-consumer queue. A pack event reaches
one sensor replica, which is insufficient once each replica has a local pack
cache.

- Give each sensor-worker instance a lifecycle queue, or use equivalent fanout.
- Keep PostgreSQL desired state and assignment generation as the authority.
- Treat lifecycle messages as prompts to reconcile, not as durable state.
- Synchronize the required pack before a sensor process starts.
- Add bounded retry behavior. Do not rely on `MqError::Other`, which is not
  retriable.

Test with at least two sensor replicas. Drop a lifecycle message and verify that
periodic reconciliation still moves the workload owner to the requested pack.

### 4. Publish immutable pack releases on the filesystem

Add a `pack_release` record and an atomic `pack.active_release` pointer before
adding a cloud backend. A release owns both the pack bytes and the executable
metadata needed by actions, workflows, and sensors.

- Build and test a deterministic archive before activation.
- Store its SHA-256 digest and immutable release manifest.
- Keep current component tables as the active discovery projection.
- Change the active release and its projection in one database transaction.
- Reject the same declared pack version when it has a different digest.
- Retain old releases. Automated release deletion is out of scope here.

The filesystem layout can use a content-addressed directory under the current
pack root. This proves release semantics without mixing them with GCS or S3
SDK work.

### 5. Pin asynchronous work to releases

- Add the release identity to every execution when the execution row is
  created.
- Snapshot the action and resolved runtime metadata that the worker needs.
- Make retries preserve the original release and executable snapshot.
- Pin a workflow root before graph loading. Pin each cross-pack child to the
  target release selected when that child is created.
- Add the desired release to managed sensor workload state.
- Include the release ID and digest in shared MQ payloads for validation and
  prefetching.

Update every execution creation path, including manual runs, enforcements, work
queues, workflow children, item and batch children, and retries. Because
`execution` has history tracking, the migration must also update the history
trigger when the new column can change.

Race tests are the acceptance gate. Queue work on release A, activate release
B before dispatch, and prove that A still runs. Run the same test across a
workflow transition, a retry, and a managed sensor replacement.

### 6. Materialize releases and runtime environments locally

- Change `PackFileTransport` to fetch a release ID and digest instead of only a
  `pack_ref`.
- Verify the archive digest before extraction.
- Extract into a temporary sibling and rename it into a digest-keyed cache.
- Make execution and sensor startup ensure their pinned release is local.
- Use `pack.registered` only to prewarm caches.
- Key runtime environments by the release digest, dependency digest, runtime
  version, platform, and worker image format version.
- Build runtime environments in a temporary directory and publish them only
  after validation.
- Remove the API's best-effort runtime environment creation.

Workers and sensors can now use `emptyDir` for packs and runtime environments.
A pod-specific RWO cache remains an optional performance setting.

### 7. Add the object storage contract

- Add `BlobStore` with create-only writes, version-pinned and ranged reads,
  `head`, and idempotent deletion.
- Implement a filesystem adapter and run the contract suite against it first.
- Implement `GcsBlobStore` and `S3BlobStore` against the same suite.
- Keep generations, version IDs, ETags, request IDs, and retry rules inside the
  provider adapters.
- Reject incomplete provider configuration at startup.

Contract tests must cover write conflicts, exact-version reads, interrupted
writes, unknown upload results, digest mismatch, and repeated deletion.

### 8. Move pack release bytes to object storage

- Store new pack archives through `BlobStore`.
- Record the returned provider version, size, and digest on the release.
- Make every read request the recorded GCS generation or S3 version.
- Keep worker and sensor access behind authenticated Attune API routes.

Do not give bucket credentials to workers. The move is complete when changing
the byte backend does not change release selection or materialization behavior.

### 9. Move completed artifact bodies

- Add `pending`, `ready`, and `deleting` lifecycle states to file artifact
  versions.
- Reserve metadata before upload and mark it ready only after verification.
- Stream pod-local staging files through the API first.
- Record the immutable key, provider version, size, and SHA-256 digest.
- Keep filesystem reads only for rows that have not migrated. Do not dual-write.

The API proxy is the first implementation. Scoped provider upload sessions are
a later optimization if measurements show that the proxy is a bottleneck.

### 10. Replace append-based logs with segments

- Add log stream and segment metadata with a unique stream sequence.
- Upload immutable segments and commit them in order.
- Accept an identical retry and reject the same sequence with different bytes.
- Seal stdout and stderr before marking their artifact versions ready.
- Apply the same protocol to execution, sensor, and workflow logs.
- Record the configured maximum unflushed byte count or time window.

After every caller uses segments, remove artifact append, rename, streaming
writer, and internal PATCH operations that exist only for mutable logs.

### 11. Migrate data and move cleanup to the supervisor

- Add a restartable migration command for existing pack and artifact files.
- Verify source and target counts, bytes, and SHA-256 digests before switching
  metadata.
- Keep the old filesystem as an operator rollback snapshot for a fixed period.
- Reconcile abandoned uploads and ready rows with missing objects.
- Mark objects `deleting`, delete the recorded provider version, then tombstone
  or remove metadata.
- Collect unreferenced objects only after a safety window.
- Document a PostgreSQL and object-retention recovery point.

Do not use bucket listing in a request or execution correctness path.

### 12. Ship object mode and remove shared claims

Add `storage.mode: object | sharedVolume` to the canonical chart. In object
mode, the chart must:

- Create no pack, runtime environment, or artifact PVC.
- Mount bounded `emptyDir` volumes for pack caches, runtime caches, artifact
  staging, and log buffers.
- Remove filesystem `wait-for-packs` init containers.
- Publish the built-in core pack through an idempotent API call after API
  health succeeds.
- Remove the executor pack mount, the API runtime mount, and the supervisor
  artifact mount.
- Assign GCP or AWS object identity only to the API and any supervisor operation
  that cannot use an API-mediated path.
- Leave bucket, KMS key, and IAM creation outside the chart.

Run the acceptance tests on at least three Kubernetes nodes. Delete pods during
pack extraction, runtime setup, artifact upload, and log flush. The 24-hour run
must find no mixed releases, corrupt artifacts, log sequence gaps, or unbounded
pending uploads.

Amazon S3 Files can preserve the current filesystem behavior during these
phases on EKS. RWO claims with same-node placement are a short-term fallback,
but the chart must add required same-node affinity for every claim consumer and
the bootstrap job. CSI attachment behavior still depends on the provider.
Neither option is the target architecture.

## Implemented safeguards and remaining limits

Issues #40 through #50 closed the implementation defects found during the
initial Kubernetes proof:

- Explicit API pack and artifact transports fail startup when their URL or
  token configuration is incomplete. They do not fall back to pod-local volume
  storage.
- Pack, artifact, log, and migration object I/O streams with bounded memory.
  Range requests reach the storage provider instead of reading the full object.
- Startup verifies the configured provider supports stable object versions. S3
  requires enabled bucket versioning, and GCS operations require generations.
- Zero-byte artifacts publish as ready immutable objects. Cleanup deletes the
  recorded S3 version or GCS generation and cannot delete a replacement object.
- Pack activation exposes the API-local projection only after the database
  transaction commits.
- Supervisor retention preserves active and pinned pack releases. A
  safety-windowed metadata ledger collects unreferenced pack, artifact, and log
  object versions without relying on bucket listing.
- Execution, sensor, and workflow logs use the configured byte and time limits.
  Their effective limits and failures are durable, and the chart accounts for
  the bounded in-memory loss window instead of mounting an unused log volume.
- Optional runtime persistence gives each worker or sensor pod its own RWO
  claim. `emptyDir` remains the default, and a lost cache rebuilds without
  changing execution correctness.

Two limits remain:

- The local three-node proof passed five disruption cycles over 130 seconds,
  but the required 24-hour soak is still pending in issue #39. Do not treat the
  short run as the soak acceptance result.
- The regenerated Helm chart package `attune-0.7.0.tgz` and repository
  `index.yaml` are working-tree artifacts in `attune-charts`; no published
  package release contains this implementation yet.

## Acceptance criteria

- API, executor, supervisor, action-worker, and sensor-worker pods can run on at
  least three nodes without an RWX claim.
- Losing a pod during pack extraction or runtime setup exposes no partial cache.
- Pack activation is immutable, digest-verified, and atomic to readers.
- Queued and running work remains pinned to its selected pack release.
- Artifact upload retries do not overwrite another version.
- Log retries do not omit, duplicate, or reorder committed segments.
- Workers have no bucket-wide cloud credentials.
- A database restore and object-retention runbook defines a recoverable point.
- S3 and GCS pass the same storage contract tests.
- A 24-hour multi-node test finds no missing packs, corrupt artifacts, mixed
  releases, or unbounded pending uploads.

## Open questions

- At what measured size or throughput should large artifact uploads move from
  the initial API proxy to scoped provider upload sessions?
- What is the acceptable log-loss window when a pod dies between segment
  flushes?
- Should expensive runtime pools use `emptyDir`, pod-specific RWO claims, or
  prebuilt runtime images?
- Must workflow drafts survive independently of a published pack release?
- What pack and artifact migration rollback window is required?
- What are the measured file counts, object sizes, log rates, and cache rebuild
  times in representative installations?
