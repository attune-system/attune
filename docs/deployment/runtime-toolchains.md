# Pack-defined trusted toolchains

Runtime metadata can declare `execution_config.lifecycle` instead of the
interpreter/environment/dependency fields. The same shared module discovers tools,
prepares a target, and supplies its launch command to actions, sensors, and
candidate test runners. No language name, package manager, source extension, or
invocation flag is hardcoded in this path.

This first implementation supports the **trusted** execution profile on Unix. Preparation
and discovery are not a sandbox for hostile dependency hooks. Keep the limitations
in [workload isolation](workload-isolation.md) and
[child-process environments](child-process-environments.md) in view when admitting
code. It does not implement hostile-pack admission, network egress enforcement,
cross-host output distribution, or reusable prepared-target caching.

Managers configured to require isolated workload effects reject these local recipes,
even when the digest is listed. The same applies to the legacy Bubblewrap profile:
recipe discovery and preparation do not yet use that launcher. They remain
available only in the explicit trusted local profile. Native recipe producers
must be connected and validated before the isolated deployment defaults activate.
The native Runner now supplies a fixed `inspect-runtime-tools` guest effect
using the same recipe discovery engine, explicit guest PATH and read-only tool
identities. This effect is not manager admission or startup integration, and it
does not prepare or launch recipe targets. Private-generation invocation remains
required before lifecycle recipes are supported by the native managers.

## Admission and discovery

Provision the required executables in the worker/sensor image. Publishing a recipe
does not admit it. Obtain the canonical recipe digest with the local helper:

```bash
cargo run -p attune-common --example runtime-recipe-digest -- packs/core/runtimes/bun.yaml
```

Set `ATTUNE_RUNTIME_RECIPE_DIGESTS` on the manager process to a comma-separated list
of those lowercase SHA-256 digests, then restart that process. This is operator
configuration, captured at startup, and is never inherited by workload children.
Changing commands, probes, or recipe environment changes its digest and requires
new operator admission. A name filter may narrow the selected runtimes; it cannot
grant recipe admission. Required tools are actually probed before the instance is
advertised under `worker.capabilities.runtime_toolchains`. Placement checks the
owner-qualified runtime ref and exact recipe digest, rather than an alias alone.
Verified versions are scoped to that same ref and digest. Shared aliases cannot
borrow another recipe's or a legacy interpreter's versions. Registered versions
must retain their parent's lifecycle mode. An admitted alternate version can be
available even when the parent's default executable is absent.

Agents still report their bootstrap interpreter inventory, but no longer turn that
inventory into an automatic name filter. After connecting, they can discover any
registered, admitted recipe, including tools unknown to the agent binary. An
explicit `ATTUNE_WORKER_RUNTIMES`/`ATTUNE_SENSOR_RUNTIMES` remains an operator filter.

## Recipe contract

See [Bun](../../packs/core/runtimes/bun.yaml),
[Python/uv](../../packs/core/runtimes/python_uv.yaml),
[locked uv projects](../../packs/core/runtimes/python_uv_project.yaml),
[Deno](../../packs/core/runtimes/deno.yaml), and
[optional PyPy](../../packs/core/runtimes/pypy.yaml). Gradle and GraalVM templates
are [build examples](../examples/runtime-gradle.yaml) and
[native-output examples](../examples/runtime-native-image.yaml), not installed
toolchain images. Representative fixtures have passed command checks on Linux
AMD64; each workload still needs validation in its provisioned pool.

`tools` declares names, candidate binaries, argument arrays, a regex with a named
`version` capture, and exactly one `primary` tool. Auxiliary versions remain
separate. Constrained action/sensor selection requires registered, verified runtime
versions; invocation rechecks the selected primary version and fails on drift.
`extensions` supplies source hints and an inline source suffix. Explicit
action runtime selection remains authoritative. `workspace: true` provides an
owned writable copy of the workload release, including source-relative imports.
Symlinks/special files in copied inputs fail; copies have bounded file/byte counts.
Ad hoc recipe actions use private inline files; when no pack exists, preparation
creates an empty input root inside the private generation.

`prepare` is an ordered list of commands. `when_file_exists` is a contained
pack-relative regular file condition; missing required files should be enforced
by an unconditional checking command. `launch` names the executable/arguments and
optional working directory of the prepared target. It can invoke a tool with a
source/JAR or execute a produced native binary. Runtime images must provide any
required libraries. Build and execution currently remain on the same host.

Templates are expanded once, preserving argument-array boundaries:

- `{pack_dir}`: pinned workload input root.
- `{workspace_dir}`: owned build working copy (empty unless requested or inline).
- `{env_dir}`: permanent physical path of this private generation.
- `{output_dir}` and `{scratch_dir}`: owned output and scratch directories.
- `{source_file}`: selected source or its workspace/inline mapping.
- `{tool_NAME}`: verified absolute executable for a declared tool.

Undefined variables fail. Cross-provider helper paths are unavailable: embed
commands in the admitted recipe or use workload-owned files. Loading helpers from
a mutable provider pack ref is unsupported until separate retained provider
artifact binding/distribution is implemented.

`env` applies explicit ordinary variables to preparation and invocation. Attune
context is reserved. Exact ordinary caller override names may be declared in
`lifecycle.permitted_caller_env`; this policy is part of the recipe digest. A
top-level caller declaration cannot broaden an admitted lifecycle recipe. Existing
startup-variable restrictions remain in force. Preparation receives no action/sensor API token, action
parameters, sensor secret configuration, or execution secrets. Final invocation
retains admitted API context, caller override restrictions, stdin JSON, output
classification, and runtime-log custody. Tool-specific acquisition/config flags
are defense in depth; trusted profile networking is not restricted by Attune.

## Generations and cleanup

Every preparation creates a new mode-0700 generation under
`runtime_envs_dir/lifecycle-generations/`. The path is stable for its lifetime,
so virtualenv shebangs and compiled resources do not break through relocation.
There is no reuse or shared ready marker. Changed operator config therefore cannot
accidentally select an older prepared result. Future reuse requires revisioned
effective config, tool/platform/policy identity, and trusted publication fencing.
Each generation contains a private diagnostic ownership record with the recipe
digest, workload root, manager PID, and creation time. It contains no credentials.
Its contents are not proof of writer termination or authority to reuse/delete it.

Successful preparation produces a launch target only after its commands exit.
Commands have bounded duration and captured output; setup/probe output is not
copied into error diagnostics. Unix commands own process groups, including
package-manager/compiler descendants. This is trusted-code supervision, not
containment of processes that deliberately escape their group.
Action and candidate-runner timeouts cover discovery, preparation, and invocation
as one budget. Preparation expiry and cancellation preserve their terminal status
and retain uncertain private files.

Normal completed workloads explicitly remove their owned generation after process
and output handling and confirmed process-group exit. Surviving or unreaped groups,
or a missing recorded leader, do not authorize deletion. Failed/dropped preparation or uncertain workload supervision
leaves a quarantined generation. It is never published or reused. Cleanup of those
directories requires positively establishing that their writers have stopped;
there is no age-based or automatic shared-root deletion. Private registry
credentials and invocation-dependent compilation remain gated by the plan.
Failed or cancelled probes similarly retain their private
`attune-runtime-probe-*` home under the manager's temporary directory. Successful
discovery removes it only when every attempted probe confirmed teardown.

## Candidate tests and local deployment

A runner can use `type: runtime` and a `lifecycle` recipe with its existing
`entry_point`, timeout, and result format. Candidate preparation uses the same
operator admission; the candidate cannot admit its own recipe. Candidate API-effect
credentials are delivered only to the final test process, never preparation.
Local CLI pack tests use the same recipe contract and admission settings.
Candidate dispatch requires every runner recipe digest in the selected worker's
startup admission capability. Workers still verify admission and required tools
after downloading and verifying the immutable candidate archive.

Repeatable command-level compatibility checks require Docker, Python, and PyYAML:

```bash
python3 scripts/test-runtime-toolchains.py deno --image denoland/deno:2.5.6
python3 scripts/test-runtime-toolchains.py gradle --image gradle:8.14.3-jdk21
python3 scripts/test-runtime-toolchains.py native_image --image ghcr.io/graalvm/native-image-community:25.0.1 --memory 6g
```

The runner uses an unprivileged UID, an owned writable directory, a read-only
container root, and disabled networking. These test actual recipe commands and
local imports/resources; they do not replace Attune admission and service tests.
The fixtures also cover Bun, both uv modes, and PyPy with uv-provisioned images.
GraalVM directs JVM temporary compiler helpers into owned scratch, where executing
them is permitted; an unrelated system `/tmp` mount need not allow execution.

The full-stack runner's `--runtime-lifecycle` flag admits fixed test-only recipes
and adds components to an owned copy of core before bootstrap. It refuses
`--no-startup`, retains staged inputs until teardown, and never changes the shipped
core directory. CI uses it to test released/inline actions, preparation timeout,
sensor events, and admitted/rejected candidate dispatch through the HTTP API.

Compose's optional `toolchains` profile adds `worker-toolchains`, injecting the
normal agent into an image with Python, Node, Bun, and uv. It admits no recipe by
default. Set digests before starting it. PyPy, Deno, Gradle, and GraalVM need their
own provisioned images and validation; no existing service is restarted or deployed
by changing these files.

The full design, implementation gates, and deferred security profiles are in the
[runtime lifecycle plan](../plans/language-neutral-runtime-lifecycle.md).
