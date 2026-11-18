# Pack Runtime Environments

Pack dependencies are described by the DB-backed runtime's `execution_config`:
its environment creation, dependency manifest and install commands, and hooks.
The bundled [Python](../packs/core/runtimes/python.yaml) and
[Node.js](../packs/core/runtimes/nodejs.yaml) definitions are concrete examples.
Runtime names and aliases are normalized when matching a pack's requirements.

This is pre-production work. The native Pod/container migration remains
unactivated while the remaining executable consumers are migrated and tested.
See [workload isolation](deployment/workload-isolation.md) for its scope and
current acceptance gaps. Dependency separation alone is not a security boundary.

## Ownership and execution

The trusted worker or sensor retains runtime selection, preparation, retries,
secret-origin checks and publication policy. Pack-controlled commands belong in
isolated ordinary Pods/containers. Selected Pack, Runner and published Runtime
inputs are read-only; setup has a private writable stage. Platform database,
broker, signing and master-encryption credentials stay with the trusted manager.

The fixed guest roots are `/opt/attune/pack`, `/opt/attune/runner` and
`/opt/attune/runtime`. Setup creates a private project at
`/opt/attune/runtime/project` so relative writes and generated project files
survive with the environment. Published interpreted actions use that project;
candidate tests use their private Pack copy with the selected Runtime for
interpreter/dependencies. They must not mutate the source Pack or a published
Runtime.

Runtime reuse must bind the exact image and platform, Pack/configuration and
public Runner evidence. A legacy directory or a `ready` metadata row alone is
not an admission to execute it. Missing or changed proof must fail closed.

## Pack Environment Manager API

`attune_common::pack_environment::PackEnvironmentManager` tracks environment
metadata and coordinated publication. Its unused local installer runner has
been removed. It does not execute `runtime.installers` commands, infer a launch
policy from the process environment, or fall back to a manager-host interpreter.
The retained `installers` metadata is not an execution contract.

```rust
use attune_common::pack_environment::PackEnvironmentManager;

let manager = PackEnvironmentManager::new(pool.clone());
let environments = manager.list_pack_environments(pack_id).await?;
let environment = manager.get_environment(pack_id, runtime_id).await?;
```

The owning setup loop uses `upsert_coordinated_environment`, claims and renews
its exact worker lease, and records ready/failed/outdated state. The
`pack_environment` table retains the version, dependency-manifest checksum,
claim owner/deadline and publication metadata. Read/list methods and retained
unversioned records remain available. `verify_environment` checks directory
presence; it does not verify a native resource, program hash or image binding.

## Publication and recovery

Setup output is eligible for publication only after a successful, non-OOM
payload exit and the backend's required settled ownership evidence. Docker
requires acknowledged removal before reading its retained writable stage;
Kubernetes retains the fixed transfer companion after the payload terminates.
The archive is bounded and validated before a new read-only Runtime is admitted.
Cleanup follows the owning publication receipt. A timeout or provider 404 is
not evidence of successful removal or publication.

The existing worker loops own setup failures and retries. Native effects must
not turn a rejected local command into an optional success or reuse an
unverified old environment. The native setup/probe/candidate/sensor consumers
and their complete restart/publication paths are still migration acceptance
work; the presence of backend effects or metadata APIs does not certify them.

For runtime logs, preserve the separate encrypted persistent custody and
finalization contracts in [runtime log verification](deployment/runtime-log-verification.md).
