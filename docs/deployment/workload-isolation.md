# Pack workload isolation

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

The exact Debian `bubblewrap 0.8.0-2+deb12u1` package supports the required flags:
its owned host trace completed procfs/mount setup and enforced
`--disable-userns` (a further user namespace creation returned `ENOSPC`).
That host result does not establish compatibility with a container's parent
proc restrictions. `Dockerfile.workload-runtime` builds the injected runtime
prerequisites instead of relying on startup package installation in stock
images. Startup continues to reject incompatible container policy.

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
