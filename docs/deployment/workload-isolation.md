# Pack workload isolation

**Implementation status: work in progress.** The current Compose/container
entrypoints do not yet satisfy the complete supported isolation contract. The
host Bubblewrap boundary below fails closed where namespace prerequisites are
unavailable; host test success does not make the primary deployment supported.
The Docker launch-controller candidate is being implemented separately and is
not yet an enabled or complete execution backend.

Worker and sensor services use Linux Bubblewrap by default. Unset
`ATTUNE_WORKLOAD_SANDBOX` and `bubblewrap` select isolation.
`ATTUNE_WORKLOAD_SANDBOX=trusted-only` explicitly permits host execution of
trusted packs. `disabled` is no longer accepted. Trusted-only pools cannot
support untrusted packs. Unknown modes, non-Linux systems, and volume artifact
transport fail service construction in isolated mode. Sandbox launch failures
fail the operation, with no fallback. Injected runtime containers must install
operator-owned `/usr/bin/bwrap` supporting `--disable-userns`, and allow
unprivileged user, PID and mount namespaces. No privileged container is needed.
A runtime's seccomp/AppArmor policy can deny these prerequisites; such a pool
cannot advertise isolated support until its actual packaged entrypoint passes
the boundary tests. Do not globally disable security policy to hide a failure.

Final native/interpreted actions, managed sensors, runtime verification and
candidate tests use a fresh user/PID/mount/IPC/UTS namespace, private procfs,
minimal devices and private `/tmp` (including `/tmp/artifacts`). Only system
executables/libraries, public DNS/TLS configuration and selected pack/runtime
inputs are visible. Pack/runtime inputs are read-only. Host home, service
configuration, credentials, custody spool and sibling staging are not mounted.
`HOME` and `TMPDIR` become `/tmp`. Producers must upload artifacts via the API
before exit; direct host artifact staging is unsupported. Stdin JSON and admitted
scoped credentials retain their contracts. Loader/environment values enter only
after isolation through an unlinked descriptor, never the launcher's environment
or token-bearing argv.

Runtime environment creation and dependency hooks get the same boundary plus
one writable unpublished staging directory. Its parent and other staging trees
are absent. Completed staging is inspected before trusted marker writes and
publication: escaping symlinks, marker symlinks, hardlinks and special files are
rejected. Interpreter symlinks into `/usr` remain supported. Final workloads
cannot write published runtimes. Setup processes cannot access an earlier
published environment unless explicitly mounted as read-only input.

The uncompiled legacy `PythonVenvManager` was removed. Legacy `runtime.installers`
execution rejects isolated mode; use canonical `execution_config.environment`
and `dependencies` staged setup. No compatibility adapter is provided.

Network access remains shared for admitted API and dependency downloads. This
boundary does not enforce network egress policy. Keep secrets out of system
binary/certificate and selected pack/runtime trees. A privileged host, hostile
manager, kernel exploit or resource exhaustion is outside this boundary. Parent
protection and environment clearing remain required. Control-plane credential
removal is a separate workstream and is not established by these changes.

## Validation

Explicitly select `workload_sandbox::tests::bubblewrap_` ignored tests on Linux
with Bubblewrap and a C compiler. They fail when prerequisites are missing.
Tests own their dummy credentials and processes and cover procfs visibility,
sibling signaling, loader injection, stdin/token delivery, writable setup
staging and read-only published inputs. Publication tests reject link escapes.
These tests alone do not establish packaged service, complete runtime setup,
candidate distribution, log custody, Compose or Kubernetes readiness. Record
those integration results separately before claiming closure of s-23.

## Container readiness blocker

This change is **not a completed closure of s-23**. Default Docker/Compose and
Kubernetes deployment readiness remains blocked. Installing Bubblewrap and
selecting API artifact transport are necessary, but insufficient. On the owned
Docker 29.9.2 Linux/amd64 probe, the builtin seccomp profile denied Bubblewrap's
namespace `clone`. A pinned default-deny derivative admitted that exact clone,
user-namespace unshare and observed mount operations. Fresh PID-namespace procfs
mounting still failed with `EPERM`; an owned direct probe also rejected
`subset=pid`. An owned rootless runc bundle with its default proc restrictions
failed UID-map setup. These failures are gaps, not positive deployment evidence.
Do not disable parent proc masks or use privileged containers, broad capabilities,
`seccomp=unconfined` or `systempaths=unconfined` as a workaround.

`docker/seccomp/workload-bwrap-amd64.json` is a **diagnostic profile**, not an
approved production deployment configuration. It preserves the upstream Moby
allowlist and default deny from commit
[`f862455628f78f84c937e967e4a03902fb1dee11`](https://github.com/moby/profiles/blob/f862455628f78f84c937e967e4a03902fb1dee11/seccomp/default.json),
and adds only the traced amd64 namespace/mount operations. Regenerate it with
`python3 scripts/generate-workload-seccomp.py` and check its immutable baseline
and allowance invariants with `python3 scripts/test-workload-seccomp.py`.
It deliberately grants neither network/cgroup namespace creation nor arbitrary
`setns`. It is not enabled automatically in Compose.

The earlier exact Debian `bubblewrap 0.8.0-2+deb12u1` host trace completed
procfs/mount setup and enforced `--disable-userns` (a further user namespace
creation returned `ENOSPC`). It does **not** support `--argv0`, required by the
current declared CLI invocation contract. The host boundary now requires both
`--disable-userns` and `--argv0` (upstream Bubblewrap 0.9.0 or newer), and startup
probes the actual named launch. The pinned 0.8.0 runtime images are therefore
insufficient for that contract, in addition to the parent-proc incompatibility.
`Dockerfile.workload-runtime` remains a reproducible diagnostic legacy image
build, not evidence of supported isolated deployment. The candidate Docker
Engine backend supplies ordinary private procfs and executes the immutable
Runner directly; it does not depend on nested Bubblewrap in workload images.
Startup continues to reject incompatible policy without an unisolated fallback.

For background, see [Docker seccomp](https://docs.docker.com/engine/security/seccomp/),
[Linux procfs](https://docs.kernel.org/filesystems/proc.html), and
[Kubernetes security contexts](https://kubernetes.io/docs/tasks/configure-pod-container/security-context/).
Kubernetes proc policy, node profile distribution, admission and required cluster
version need their own tested contract. No chart image pins, operator checkbox,
or host-only test result can establish that contract. The current immutable
chart images do not implement this boundary; chart support remains a dependency.

Candidate tests receive a private writable copy of their input tree at the same
logical working path. Their published input tree and selected runtime remain
read-only. The subprocess owner removes the private copy after process teardown.
Runtime setup mounts its private host staging at the final logical install path,
so console-script shebangs remain valid after publication. Internal absolute
links are converted to equivalent relative links before the trusted rename.

The unused legacy `pack_registry::DependencyValidator` no longer executes local
Python/Node/shell binaries to validate runtime requirements. It returns an
unsupported result for local runtime probing; use registered worker capability
validation. Its pure pack-version dependency checks remain available. This
removes another inherited-environment subprocess path without requiring the API
host to execute pack runtime code.

## Docker launch-controller candidate

`crates/launch-controller` currently contains the typed control-plane client,
fixed-policy Docker Engine API transport, bounded ephemeral raw-stream capture,
and controller-owned input/stage cache, lifecycle and scoped frontend code.
End-to-end lifecycle validation, runtime adapters and deployment integration
remain incomplete. This draft implementation
are not an authorization path for callers to submit commands or Docker config.
Only control-plane admitted launch IDs and manifest digests may select execution;
the private invocation comes from the authenticated control plane.

The candidate requires a colocated Unix-socket Engine and a fixed named volume.
Workloads receive only selected subpaths: immutable inputs read-only and a unique
attempt-private stage or candidate copy writable. They receive neither the
Engine socket nor control-plane/manager credentials. The Engine supplies normal
private procfs; the rejected empty-proc prototype is not a supported ABI.
Startup must prove exact controller/Engine volume binding before accepting work.
It also fetches the CP-registered immutable engine ID, daemon identity and Linux
platform, verifies Docker Info against them and inspects/pulls only that platform.
Replacing the daemon/platform requires a new CP engine identity. Runtime reuse
requires the CP-recorded successful build's same engine, platform and image;
co-admitting two index digests is not runtime compatibility evidence.
The control plane and secret-bearing managers belong outside the workload host;
the full placement and enrollment contract remains part of integration work.

All Pack, Runtime, Runner and StageArchive bodies use canonical `application/zip`.
`common::workload_archive` supplies one pure validator for both the controller and
API publication. It checks compressed/decompressed/file-count bounds, full CRC
reads, canonical POSIX paths, implicit directories and the complete internal
symlink graph. Absolute links within the selected fixed root retain their exact
bytes; external links, link cycles, file/link parents and escaping compositions
are rejected. Runtime creation uses a stable `/opt/attune/runtime` install path;
Python venvs require `--copies` because image-external interpreter links are not
portable archive inputs. Filesystem staging additionally rejects hardlinks and
special files without following them. Publication requires successful setup and
acknowledged container Wait **and Remove**, with fresh control-plane authority.

Archive/output bounds, free-space admission, concurrency limits and owned cleanup
are availability controls, **not live filesystem byte/inode quotas**. A writable
named-volume stage can exhaust host disk before validation. This residual risk
must be assessed explicitly during security review; no hard-quota guarantee is
claimed. Network isolation is also not provided; admitted API access remains
necessary. Current chart/image defaults do not implement this candidate.

The controller entrypoint loads only its strict JSON `ATTUNE_LAUNCH_CONFIG` and
an operator-supplied CP-issued `WorkloadSession` file, never the general service
configuration or DB/MQ/master/signing keys. The session file must be a bounded,
regular, single-link file owned by the controller user with no group/other access.
It renews through `POST /api/v1/workload-launches/session` using its current
dedicated controller bearer, updates the shared credential slot and respects the
original deadline when renewal fails. Expiry stops admission and joins owned
lifecycle tasks. Restart after a long outage still requires explicit CP bootstrap;
there is no unlimited-token or local-signing fallback.

The HTTP backend binds literal loopback; a colocated TLS proxy must supply the
CP-registered external HTTPS origin. Frontend calls carry only target dispatch or
channel grants. An HTTP launch response acknowledges scheduling, so execution
hosts poll authenticated `status()` for Running or terminal state, refresh their
assignment/channel grants and deliver stdin once. A lost write acknowledgement
never authorizes replay. Historical stage `None` means authenticated not-ready;
CP denial and transport failures remain errors. Retained raw output is bounded
ephemeral capture, not an acknowledgement of encrypted durable manager custody.

The trusted packaged controller runs as root to enforce stage UID/GID 1000 and
read/validate owned output after teardown. Its required container capabilities
are CHOWN, DAC_OVERRIDE and FOWNER; workload containers still drop **all**
capabilities and receive no controller filesystem or credential. The Engine
binding probe and Python archive/publication fixture exercise actual ownership
and writable-stage behavior in this configuration. Filesystem-only host tests use
an internal test owner matching the CI runner; that seam is absent in production.
These focused fixtures do not certify the unfinished complete service stack.


The public `attune-workload-runner` helper is packaged with the agent binaries.
Its `setup` mode accepts a bounded private v1 plan once on stdin, inside the
admitted Setup container. The immutable Runner ZIP contains executable helper
bytes only. Runtime create/dependency/hook children clear their environment and
receive only the CP-selected plan environment; the helper never logs the plan
or accepts host mounts, controller credentials or an action API token. Fixed
read-only Pack/Runner roots and the writable stable Runtime stage are required;
direct-host use fails before executing a plan. This guard is a packaging check,
not an independent attestation of OCI policy. The actual packaged Python/pip
console-script and internal-link fixture must pass on each supported architecture.

The helper's `exec` mode preserves declared CLI semantics through the private
envelope `[runner(platform), exec, expected_sha256, canonical_program,
static_invocation_name, ...application_args]`. It verifies the regular executable,
canonical path, read-only filesystem and admitted byte hash immediately before
`exec`, preserves the static name as OS argv0, and forwards selected stdin without
reading it. Argument limits match the shared CLI materializer; no shell or PATH
selection occurs in this adapter. The Runner pin contains helper bytes only;
argv, environment and stdin stay in the encrypted frozen context. The CP producer
must bind the selected executable to the admitted immutable image/platform or
pinned native input and selected Pack working root. Installed-command selection
still requires an authoritative image probe/inventory producer; the CP must not
substitute its own host filesystem, PATH or architecture. The owned packaged
fixture measures image executable bytes in that same image and tests argv0,
literal arguments, unchanged stdin and hash mismatch rejection. It is not the
missing CP producer, and its real-container results remain required on both
architectures before support can be claimed.

File artifacts are a separate unfinished integration boundary. The canonical
`workload_artifact` protocol seals CP-selected file-version paths when Run closes,
with a fixed deadline and independent publication grants/receipts. Filesystem
primitives now retain a private per-attempt tree for Action, Sensor and
CandidateTest, reject artifact links/hardlinks/special files, and enforce shared
retained-tree defaults of 256MiB and 4096 directory/file entries. Zero-byte regular
files are valid. Deleting retained staging requires an authenticated settled
cleanup receipt, never process exit, timeout or a 404. These primitives are not
yet activated in the Engine mount builder or connected to actual CP upload
operations; the current tmpfs artifact mount remains a known gap until that
integration and restart/lost-acknowledgement tests pass. Setup and RuntimeProbe
have no action file-publication authority.


The controller now durably indexes each authenticated **public** manifest by
launch ID and admitted digest before Engine creation. Recovery verifies private
file ownership/mode, nofollow/single-link constraints and the exact digest; it
never stores the invocation or uses a fresh private-context resolve fallback.
The finite artifact HTTP client implements exact inventory/grant/receipt checks,
zero-byte uploads, empty-body ReceiptOnly replay and explicit whole-tree failure
reporting. A failure remains not-ready until CP native-operation fencing returns
an authenticated incomplete cleanup receipt. These transport/FS tests do not
prove actual CP publication/discovery effects, which remain incomplete.

The inactive artifact reconciler persists a canonical public cleanup record
before its first artifact claim, including a claim that can immediately settle
an empty inventory. This root-owned, single-link, nofollow `0600` record contains
the exact immutable manifest and authenticated Stopped status, never invocation,
input or tokens. The queue is bounded to 256 records of at most 4MiB each.
Restart recovery uses that historical removal evidence and a freshly
authenticated dedicated-controller claim to replay the native-settlement
receipt, without dispatch reconstruction or private resolve. Stage deletion and
the retained receipt are fsynced before queue deletion. Lost/malformed evidence
is incomplete recovery, not permission to guess a directory or remove it.
Whole-tree loss/invalidity uses explicit failure dispositions; it is never
reported as a selected missing file. This reconciler remains unscheduled until
actual CP effects and the artifact mount are integrated and validated.

Count-only `authorize-io` is rejected for stdin. The controller now sends the
exact immutable bounded bytes to dedicated `authorize-stdin` over HTTPS, with
explicit development-only plaintext policy for owned fixtures. It consumes the
local input slot before that request and retains the same payload through the
one Engine write, so a lost ACK never permits retry. The stdin-only frontend
and HTTP handle share the canonical 8MiB bound; other frontend bodies stay 1MiB.
The actual CP frozen-origin/atomic-consumption effects remain incomplete, so
missing/denied authorization still fails closed without a generic fallback. No public plaintext input hash or host raw
plan submission substitutes for that authorization. Packaged Engine fixtures
exercise actual stdin delivery directly as boundary tests; they do not attest
this unfinished CP authority/host-consumer contract.

Finite raw-output authorization is bounded by the current lease deadline, so a
stalled CP request cannot retain the session lock beyond that lease and postpone
local renewal failure/stop. Authorization failure never returns buffered output.

The separate external Engine/controller Compose project and its public config
generator are described in [the draft deployment contract](docker-launch-controller.md).
Config/model tests are not packaged service or production topology evidence.
