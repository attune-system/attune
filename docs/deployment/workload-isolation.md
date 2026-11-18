# Pack workload isolation

**Status: work in progress; s-23 remains open.** Kubernetes is the primary target;
Docker Compose is also required. The native adapters are not enabled defaults.
Current legacy Compose worker/sensor entrypoints retain their fail-closed
Bubblewrap prerequisite check and still fail under the tested ordinary Docker
procfs policy. A successful image build or offline chart render does not resolve
that deployment blocker.

## Execution boundary

The existing trusted worker and sensor loops retain scheduling, preparation,
DB/MQ access, encryption, custody and completion policy. They must delegate every
pack-controlled executable effect to an ordinary per-attempt/generation Pod or
container before the new path is activated. Physical separation of the manager
onto a remote host is not required. Manager, node/kernel, administrator and
launcher compromise are outside this threat model.

`common::isolated_process` is an in-process effects interface. The trusted loop
persists its intent and first-create decision before calling `prepare`. Only the
first durable submission permits `CreateOnce`; recovery uses `ReconcileOnly`.
A missing provider resource after an unknown create never permits another
create. Resource identity is the exact Docker daemon/container or Kubernetes
cluster/namespace/Pod UID. There is no caller-selected host path, mount policy,
container selector, HTTP dispatch credential or independent scheduler.

`crates/workload-backend` holds the two concrete adapters, safe ZIP/cache code
and public `attune-workload-runner`. The former standalone controller service,
frontend, session store and TLS proxy are retired. Their HTTP protocol is not a
requirement for the in-process path.

A fixed Runner gate starts before pack code. Separate bounded stdout/stderr
attach streams are established first. The trusted caller commits the existing
input one-shot fence, then writes a bounded private invocation header and the
original selected stdin bytes exactly once. The header is not Pod metadata, a
public object or a plaintext parameter digest. The helper consumes only the
header, clears its child environment and preserves the application's remaining
stdin, including empty EOF, without adding a newline. Uncertain writes are not
retried. Existing CLI executable verification and static invocation-name semantics
remain part of selected invocation planning.

## Kubernetes effects

The trusted worker uses a namespace-scoped Pod lifecycle/attach/exec identity in
a dedicated workload namespace. It creates ordinary Pods with
`restartPolicy: Never`; the existing worker owns retries. There is no Job
controller, operator, CRD, external Docker dependency or privileged DinD.

Each Pod has fixed `payload` and `transfer` containers in separate PID
namespaces. Service-account automount and service-link environment injection are
disabled. Containers run as UID/GID 1000 with a read-only root filesystem,
`no_new_privs`, all capabilities dropped and RuntimeDefault seccomp. Host
PID/IPC/network namespaces, host paths, control sockets, Secret/projected-token
volumes and manager/custody volumes are absent. Admission-injected authority,
mounts or containers cause policy verification to fail.

A fixed init helper copies the exact public Runner into a read-only bootstrap
mount. The payload command contains only this gate, not private argv/env. The
credential-free transfer helper accepts fixed admitted ZIP/file effects through
trusted Kubernetes exec; it has no arbitrary command listener or platform
credential. Selected pack/runtime/Runner roots are per-Pod volumes mounted
read-only in the payload; candidate tests receive their own writable pack copy.
Setup alone receives a private writable runtime stage at the stable
`/opt/attune/runtime` destination. Scratch and artifact staging are private to
the attempt. The Pod/stage must be retained until publication receipts permit
owned cleanup.

The attach transport requires Kubernetes stream protocol v5: stdin EOF must not
close stdout/stderr. Missing terminal transport status is incomplete capture,
not successful EOF. Actual supported cluster/runtime versions are established
by the provider acceptance matrix, not by the Rust client's schema version.

## Docker effects

The Docker adapter runs inside the trusted worker, using its operator-configured
local Engine socket and fixed named cache volume. The socket and cache parent
never enter a workload. An owned sentinel/container startup probe must prove
that the manager and daemon see the same named volume/subpath. Daemon identity
and immutable native platform are Docker-specific evidence, not Kubernetes
fields.

Each workload has a private PID/IPC/mount namespace, normal private procfs,
read-only root filesystem, all capabilities dropped and no-new-privileges.
Only selected immutable input subpaths and the attempt's own stage/artifact
subpaths are mounted. Container inspection checks exact ownership and fixed
policy before effects. Exit does not imply custody, file publication or removal.

## Storage, capture and readiness

Guest inputs and runtime publication use canonical v1 ZIP only. Source pack
releases retain the existing `PackStorage::publish_release` runtime-v1 manifest
and exact pinned archive size/digest. The trusted manager verifies that primary
GNU TAR inventory against the retained manifest before streaming it into a ZIP;
there is no TAR reader in the payload/transfer helper and no offline CLI-format
fallback or publication rewrite. Compressed length,
digest, decompressed bytes, entry counts/implicit parents, CRCs, path traversal,
link graphs, hardlinks and special files are validated. Safe links remain
confined to the selected fixed root; image-external venv interpreter links need
`--copies`. Setup must create, archive, publish and execute real console scripts
and internal links successfully in the same admitted immutable image/platform.

The concrete adapters' `archive_stopped_stage` effect accepts Setup only and
requires exact resource ownership plus successful payload termination, including
no OOM. Kubernetes reads a fixed retained ZIP from the credential-free transfer
container; Docker reads only the owned private stage after successful Wait.
Both validate the bounded ZIP locally and preserve its exact bytes on retry.
This file is transfer evidence, not a runtime-publication receipt. The owning
runtime workflow must commit validation/publication before removing the retained
Pod/container or stage. The real Python venv/pip console-script and absolute-link
Pod regression is part of the required external provider lane; compiling it does
not establish that it passed.

Archive/output bounds, free-space/admission/concurrency checks and owned cleanup
are not a universal live filesystem byte/inode quota. Named-volume exhaustion
remains a residual availability risk unless a tested filesystem quota
prerequisite is added. Network egress is not isolated: scoped API access and
dependency downloads remain reachable.

The existing trusted worker performs bounded separate capture, output
classification, encryption and persistent log custody. Its independent spool key,
source/attempt fences, tombstones and completion idempotency remain required.
Provider exit alone cannot mark logs complete. Restart, lost stdin/completion
ACK, spool failure and missing custody must preserve finalizing/incomplete
outcomes rather than replay effects or invent EOF.

Default activation requires all action/CLI, setup/dependency hooks, runtime
verification/probes, candidate tests, managed sensors and artifact/secret paths
to use the same boundary. No legacy fallback or feature disabling closes this
requirement. Native provider effects tests do not substitute for the full
existing-worker capture/custody/fenced-completion proof.

The disposable Kubernetes effects fixture is
`cargo test --locked -p attune-workload-backend --test kubernetes_provider -- --include-ignored --nocapture --test-threads=4`.
It requires explicit owned kubeconfig/context/namespace, immutable candidate
payload/helper images and platform via `ATTUNE_ISOLATION_TEST_*`; it never
selects a developer/default cluster. Missing prerequisites fail. CI provisions
only its own disposable cluster. Chart rollout/storage/RBAC, packaged-image
acceptance, the full worker vertical and final independent security review
remain required before readiness.
