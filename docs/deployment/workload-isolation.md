# Workload process isolation with Bubblewrap

Linux action and sensor managers can opt in with
`ATTUNE_WORKLOAD_SANDBOX=bubblewrap` and API artifact transport. Provision an
operator-owned `/usr/bin/bwrap` that supports `--disable-userns`. The manager runs
a sandbox probe at startup to check whether the binary, kernel and container
security profile allow the required user, PID, mount, IPC and UTS namespaces
and `close_range(..., CLOSE_RANGE_CLOEXEC)` (Linux 5.11 or newer). The sandbox
keeps only the workload's explicitly configured stdio descriptors across exec.
A failed probe stops startup. A failed workload sandbox prevents that workload
from running.

Each sandbox runs as a local child process of the existing worker or sensor
manager. It mounts a private procfs, minimal devices and a private `/tmp`.
System executables and libraries, public DNS/TLS configuration and the selected
pack and runtime inputs are read-only. Inline inputs and declared tool
executables are exposed only as needed. Keep credentials out of these mounted
trees. The private PID namespace limits access to the manager and sibling
processes by PID. The sandbox shares the manager's network namespace, so
application HTTP paths remain available.

## Preparation and execution

Tool probes and dependency preparation run with writable staging owned by
that preparation. The pack release stays read-only. Lifecycle recipes can request
a writable workspace copy during preparation. Final recipe actions, managed
sensors and candidate tests get a read-only view of the prepared files and
independent writable home, scratch and artifact staging. Other sandboxed
workloads use `/tmp` for temporary files and `/tmp/artifacts` for artifact staging.
File producers can upload through the API while those files exist.

The sandbox defaults `PYTHONPYCACHEPREFIX` to `/tmp/pycache`. Python recipes can
also set it to `{scratch_dir}/pycache` to use their private invocation directory.
These settings let imports generate bytecode while pack files and dependencies
remain read-only. Recipes with other launch-time caches should direct that state
to scratch. Resolve dependencies and generate build outputs during `prepare`.

Setup and tool probes do not receive the invocation token or inputs. Final
workloads retain scoped API credentials, stdin and normal argument delivery.
Bubblewrap starts with a trusted PATH. The workload's environment, including
any token, is delivered through an unlinked file and applied inside the sandbox.

## Helm and GKE rollout

Enable the setting on every action and sensor pool eligible for the workloads
that require isolation. The runtime image must include Bubblewrap. The Attune
Charts pool `env` and `containerSecurityContext` fields support the opt-in and
a cluster-approved seccomp profile for the agent container. Chart object-storage
mode selects API artifact transport. The Pod default stays `RuntimeDefault`.
Validate the exact runtime image, non-root identity, node pool and security
profile together. In GKE Standard a cluster operator can supply a
[`Localhost` seccomp profile](https://docs.cloud.google.com/kubernetes-engine/docs/concepts/seccomp-in-gke)
when the default blocks the required namespace operations.

Run a representative action, sensor, dependency build and candidate test on
the target pool before rollout. The startup probe exercises the namespace
policy. The Linux tests exercise the file and process boundaries and prove that
Python writes real bytecode into private scratch:

```bash
ATTUNE_TEST_RUN_ID=bwrap-boundary cargo test -p attune-common --lib \
  workload_sandbox::tests::bubblewrap_ -- --ignored --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=bwrap-recipe cargo test -p attune-common --test runtime_lifecycle_tests \
  bubblewrap_ -- --ignored --nocapture --test-threads=4
```

## Mantis findings s-2 and s-23

The process and filesystem boundary limits a pack workload's access to
manager credentials, sibling processes and published files. Workers and sensor
managers still hold their control-plane credentials. Moving those credentials
out of the managers requires a separately designed, attempt-scoped control
protocol that preserves recorded authority, secret origins and log custody.
Control-plane network policy can restrict direct database reachability after
workers and sensor managers switch to that protocol.

Recipes still prepare on the worker or sensor manager when needed. Durable
install-time preparation requires release-pinned readiness for actions and
sensors, immutable publication gated on preparation success, and placement
that can resolve the prepared generation on the selected pool. Findings s-2
and s-23 remain open pending the remaining remediation and target-pool
validation.
