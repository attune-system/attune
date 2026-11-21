# Pack-defined trusted toolchains

Runtime metadata can declare `execution_config.lifecycle` instead of the
interpreter/environment/dependency fields. The same shared module discovers tools,
prepares a target, and supplies its launch command to actions, sensors, and
candidate test runners. No language name, package manager, source extension, or
invocation flag is hardcoded in this path.

Runtime recipes are intended for audited pack code. Linux pools can opt in to
Bubblewrap for tool discovery, preparation and execution. See
[workload isolation](workload-isolation.md) and
[child-process environments](child-process-environments.md) for the launch
boundary and the prerequisites for enabling it.

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
owned copy of the workload release, including source-relative imports. With
Bubblewrap, preparation can write the copy and final processes see it read-only.
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
- `{output_dir}`: owned build output, read-only during a sandboxed invocation.
- `{scratch_dir}`: writable scratch. The sandbox gives each final invocation a
  fresh scratch path outside the build generation.
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

By default, preparation creates a new mode-0700 generation under
`runtime_envs_dir/lifecycle-generations/`. The path is stable for its lifetime,
so virtualenv shebangs and compiled resources do not break through relocation.
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

## Install-time preparation

Set `prepare_at_install: true` under `execution_config.lifecycle` to run that
recipe's `prepare` commands before activating a pack release. The supported
targets are declared file-backed actions and managed sensors with a default
toolchain recipe. Use vendored input files or dependencies already in the
operator image. Install preparation runs offline, with a fixed baseline
environment, an immutable copy of the candidate release, and a private writable
generation. It receives no invocation inputs or credentials. Inline actions and
explicit candidate test runners still prepare for each invocation.

- Enable Bubblewrap and API artifact transport on eligible action workers and
  sensor managers. Admit the recipe digest on each pool and set
  `ATTUNE_RUNTIME_PREPARATION_REVISION` to a nonsecret revision for the exact
  image, toolchain and effective operator build configuration. Change that
  revision when those inputs change. Use read-only, revisioned system tooling.
- A builder and its consumers must see the same runtime storage at the same
  absolute path, using compatible images and a common manager UID. A persistent
  storage ID distinguishes separate worker-local volumes even if their paths
  and configured revisions match. Keep that ID unique when provisioning
  independent volumes. Give the managers write access for generation creation
  and invocation scratch. An action can use a worker's local volume. Each
  managed sensor needs explicitly shared storage between its builder and
  eligible sensor managers.
- A release can prepare up to 16 targets. Each target gets its own eligible
  storage scope and a five-minute preparation attempt. Builders honor pack
  placement, and action builders also honor action placement. Sensor and rule
  placement govern the sensor managers that can consume a result. All required
  targets must be ready before the release activates. Install-prepared file
  workloads use the parent recipe without runtime version variants or additional
  action runtime constraints.
- Pack activation checks the worker's complete readiness evidence against the
  release digest, entrypoint, recipe and eligible storage scope. A failed build,
  expired attempt or unavailable scope leaves the previous release active,
  including on forced installs and installs that skip candidate tests. A new
  install request starts fresh attempts at new physical paths. On cancellation,
  the worker stops and joins build input copies before releasing the candidate.
- At launch, the manager verifies its scope, the manager-written completion
  seal, the retained files, and the resolved tools against the recorded
  identities. It mounts the prepared files read-only and allocates independent
  writable home, scratch, artifact staging and Python bytecode paths. Loss of
  the storage or an identity mismatch prevents launch until a matching scope is
  available or the release is prepared again.

Installed generations keep their original physical paths under
`runtime_envs_dir/.installed-runtime-preparations/` across manager restarts.
Invocation teardown removes its own scratch after process teardown. Installed
generation storage is retained for pinned executions and sensors and has no
online reclamation in this slice. Size the volume for retained and quarantined
generations. Reclaim it only after retiring its release references, draining all
workloads and proving that writers have stopped.

The full-service regression is in
[test_install_runtime_preparation.py](../../tests/e2e/api/test_install_runtime_preparation.py).
Set `ATTUNE_E2E_INSTALL_PREPARATION=1` for the test runner against an owned
stack with the [fixture recipe](../../tests/fixtures/runtime-services/install_recipe.json)
admitted on both pools. It exercises upload, activation, action reuse, sensor
startup, failed and ineligible updates, and a changed release. To exercise two
worker-local scopes, also set `ATTUNE_E2E_SPLIT_PREPARATION_POOLS=1` and configure
action workers with the label `fixture=install-preparation` and separate
`pool=first` and `pool=second` labels and runtime volumes. Put a `dedicated`
no-schedule taint on the second pool.

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
