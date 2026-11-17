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
