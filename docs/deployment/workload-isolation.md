# Final workload process isolation (experimental opt-in)

Linux action and sensor managers can set `ATTUNE_WORKLOAD_SANDBOX=bubblewrap`.
The default is disabled; existing deployments retain their execution behavior.
An unknown mode, non-Linux platform, or shared-volume artifact transport rejects
startup. API artifact transport is required. Install operator-owned Bubblewrap
at `/usr/bin/bwrap` with support for `--disable-userns` and enable unprivileged
user and PID namespaces. A failed sandbox launch fails the workload; it never
falls back to an ordinary process. No privileged container mode or host change
is performed by Attune.

Each final interpreted/native action and managed sensor receives a new user,
PID, mount, IPC and UTS namespace, a fresh procfs, minimal devices, and private
`/tmp`. Only system executables/libraries, DNS/TLS public configuration, and the
selected pack and runtime are mounted. Pack/runtime mounts are read-only.
Inline action files are individually mounted read-only. Native actions receive
the selected executable file. Path aliases and broad/system input mounts are
rejected. Pack/runtime paths must be absolute canonical paths. No host home,
service configuration, database/broker credentials, custody spool or sibling
scratch/artifact tree is mounted. The operator must keep credentials out of
system binary/certificate trees and selected pack/runtime trees.

The existing selected environment, scoped API credential and stdin JSON are
preserved. `HOME` and `TMPDIR` become `/tmp`; `ATTUNE_ARTIFACTS_DIR` becomes the
private `/tmp/artifacts` directory. File producers must upload through the API
before exit. File writes under the pack, runtime, or any arbitrary host working
directory fail. Dependencies needing writable state must use private scratch.
Network access remains shared so admitted execution/sensor/control HTTP paths
continue to work; this feature does not enforce network egress policy.

The process namespace prevents a final workload from reading another workload's
initial environment through host procfs or signaling a host sibling by PID.
Read-only mounts prevent a final workload from poisoning the published runtime
used by a later execution. This does not protect against a hostile host service
or a privileged tracer. Operator pack publication and runtime setup must remain
trusted and must not run concurrently with untrusted workloads on the same host.

## Finding mapping and remaining stages

Findings s-2/s-23 and issue 137 remain open. `env_clear` controls inheritance;
`PR_SET_DUMPABLE(0)` protects the manager after startup. Neither mechanism alone
isolates siblings or writable shared runtimes. This stage adds an independent
OS/filesystem boundary around **final** workload processes and removes their
access to credential-bearing host trees. It does not remove master credentials
from managers: worker JWT signing supports execution and worker transport tokens,
encryption supports admitted inputs and encrypted log custody, and DB/MQ support
control operations. Sensors still use DB/MQ/JWT and optional encryption for
recorded configurations.

Dependency setup/build hooks and runtime verification still execute outside this
new sandbox. They can inspect or modify the manager's host filesystem and attack
same-UID launcher processes. Do not treat this opt-in stage as safe admission of
untrusted dependency hooks or a complete solution for mutually hostile packs.
Further implementation must isolate setup/verification and introduce explicitly
attempt-bound control APIs before removing worker/sensor master credentials.
Recorded authority, input origins, output secrecy and log custody must survive
that transition. No runtime credential or snapshot format is changed here.

Compose and Helm do not enable this mode automatically. Chart images, namespace
support, API artifact transport and writable-state expectations need coordinated
validation in the canonical chart repository before enabling a worker pool.

The common `workload_sandbox` boundary tests include an explicitly selected Linux
Bubblewrap case with owned dummy credentials, a readable same-UID sibling,
stdin/scoped-token delivery and a read-only published runtime. Missing prerequisites
fail that selected case. It is not evidence that the full manager/control API
or the setup stage has been isolated.
