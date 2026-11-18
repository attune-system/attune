# Pack dependency environments

**Status: migration incomplete; not release acceptance.** Dependency environments
separate packages and versions. A virtualenv or dependency directory does not
isolate a process from manager credentials, sibling processes or writable files.
The ordinary Pod/container boundary and its remaining deployment requirements
are documented in [workload isolation](../deployment/workload-isolation.md).

## Canonical dependency configuration

Actions and sensors select a DB-backed runtime and, when requested, a matching
runtime version. That runtime's `execution_config` defines the interpreter,
environment creation, dependency manifest/install command and environment
variables. Runtime names and aliases are matched case-insensitively; version
selection uses the available versions satisfying the declared constraint.

For example, a pack using the bundled Python runtime places `requirements.txt`
at its release root. The relevant configuration in
[core.python](../../packs/core/runtimes/python.yaml) is:

```yaml
execution_config:
  interpreter:
    binary: python3
    args: ["-u"]
    file_extension: ".py"
  environment:
    env_type: virtualenv
    dir_name: ".venv"
    create_command: [python3, "-m", venv, "--copies", "{env_dir}"]
    interpreter_path: "{env_dir}/bin/python3"
  dependencies:
    manifest_file: requirements.txt
    install_command: ["{interpreter}", "-m", pip, install, "-r", "{manifest_path}"]
```

Use the runtime definition as the authority for supported fields and defaults.
[core.nodejs](../../packs/core/runtimes/nodejs.yaml) provides the corresponding
Node dependency example. Pin dependency versions and retain lockfiles for
reproducibility; dependency downloads still require network access. No network
isolation is claimed.

The previous `PythonVenvManager` and `PythonRuntime` implementations have been
removed. Their Rust constructors, `meta.python_dependencies` examples,
`ATTUNE__WORKER__VENV_BASE_DIR` and `ATTUNE__WORKER__PYTHON_PATH` examples are not
supported alternate configuration paths. The generic dependency traits that
remain in source do not provide an installed Python manager. There is no
compatibility reader or automatic conversion for those examples.

## Setup and published inputs

The trusted owning loop selects immutable Pack, image/platform and public
Runner inputs, performs admission and owns retries/publication. Pack-controlled
creation, dependency commands and hooks must execute inside the isolated
workload. They receive cleared environments and explicit selected context;
setup does not receive the action API token or manager control credentials.

The fixed setup helper copies regular project files into the private
`/opt/attune/runtime/project` and runs steps there. Generated project and lock
files remain in the runtime archive. The admitted `/opt/attune/pack` stays
read-only. Runtime installation uses the stable `/opt/attune/runtime` path so
console-script shebangs and safe internal absolute links survive publication.
Python venv creation uses `--copies`; external interpreter symlinks are rejected
by archive validation rather than silently rewritten.

A failed creation/install/hook step prevents publication. Successful output is
bounded and validated, and the owning workflow must record its publication
receipt before reclaiming staging. Published Runtime inputs are read-only;
interpreted execution uses their project and dependencies. Candidate tests keep
their own writable Pack copy and Pack working directory, including when they
use a published Runtime. Per-attempt scratch and artifacts remain private.

Runtime reuse must verify the admitted release/configuration, image/platform and
helper/build evidence. An existing directory or metadata `ready` state is not
that proof. The retained-tree/archive bounds do not establish a universal live
filesystem quota. See [pack runtime environments](../pack-runtime-environments.md)
for coordinated metadata and publication ownership.

## Current validation limits

`ProcessRuntime` remains the active legacy environment/dependency implementation
while the native consumers migrate. Configured native managers select a
captured deny-local policy; missed local setup, install, execution and version
probe wrappers fail instead of running on the manager. The unused common
`PackEnvironmentManager` installer runner is removed; its retained metadata APIs
do not execute installer commands.

Real disposable-Pod tests cover Python venv/pip console scripts, internal links,
validated archive replay and read-only reuse after builder removal on amd64 and
arm64. These provider effects do not prove the complete owning setup workflow,
all language toolchains or default deployment readiness. The corresponding
Docker and candidate effects, all remaining consumers, restart/publication
behavior and packaged Compose/chart acceptance must pass before activation.
Missing prerequisites and guard denials are gaps, not passing evidence. Do not
manually install packages on the trusted manager to bypass a failed isolated
setup.
