# Child-process environments

Action runtimes and managed sensors start subprocesses with a cleared environment.
Each service captures a selected baseline at startup. Final action and sensor
processes, dependency installers, and registered-runtime verification commands
receive that baseline rather than the service's complete environment.

## Default baseline

The baseline retains available values for `PATH`, `HOME`, `LANG`, `TZ`, and
`TMPDIR`. It also retains these locale variables:

```text
LC_ALL LC_CTYPE LC_NUMERIC LC_TIME LC_COLLATE LC_MONETARY LC_MESSAGES
LC_PAPER LC_NAME LC_ADDRESS LC_TELEPHONE LC_MEASUREMENT LC_IDENTIFICATION
```

The selection uses exact names. Other `LC_` variables do not pass through.
On Windows, the baseline also retains `SystemRoot`, `SystemDrive`, `WINDIR`,
`PATHEXT`, `USERPROFILE`, `TEMP`, and `TMP`.

Proxy settings, custom CA paths, package-manager settings, and cloud credentials
do not pass through by default. Runtime dependency setup uses its own HOME,
XDG configuration/cache directories, and pip configuration/cache settings under
the owned runtime environment directory.

## Operator passthrough

Worker and sensor configuration have independent allowlists:

```yaml
worker:
  passthrough_env: [HTTPS_PROXY, NO_PROXY, SSL_CERT_FILE]
sensor:
  passthrough_env: [HTTPS_PROXY, NO_PROXY, REQUESTS_CA_BUNDLE]
```

Both lists default to empty. Names must be exact environment-variable names,
without wildcards. Duplicate names and the reserved `ATTUNE_` namespace produce
a configuration error. A selected variable that is absent from the service
environment remains absent from child processes.

Environment overrides accept comma-separated names:

```bash
ATTUNE__WORKER__PASSTHROUGH_ENV=HTTPS_PROXY,NO_PROXY,SSL_CERT_FILE
ATTUNE__SENSOR__PASSTHROUGH_ENV=HTTPS_PROXY,NO_PROXY,REQUESTS_CA_BUNDLE
```

To clear an override, remove the environment variable and use an empty YAML list.
Restart the service after changing its allowlist or selected variable values.
The service keeps an immutable snapshot for its lifetime.

Named passthrough deliberately exposes a value to pack-controlled code, including
dependency build hooks. Lowercase proxy names require separate entries when a
tool uses them. A CA variable must point to a file available inside the child
container or host.

## Runtime and execution values

For actions, explicit worker context overrides the baseline. Runtime `env_vars`
then apply their set, prepend, or append operation. Parameter-delivery metadata
and admitted execution-request `env_vars` follow. Runtime and execution-request values
cannot replace the reserved Attune context.

Caller overrides default to denied. Trusted runtime metadata may declare exact
ordinary application names in the canonical v1 execution configuration:

```yaml
execution_config:
  permitted_caller_env: [APP_REGION, LOG_LEVEL]
  env_vars:
    LOG_LEVEL: info
```

Declarations must be arrays of unique portable names (`[A-Za-z_][A-Za-z0-9_]*`),
without wildcards, nulls or case-insensitive duplicates. Interpreter, linker,
shell startup, search path, home/configuration discovery, locale, temporary-directory
and Attune controls are forbidden even when listed. Examples include `BASH_ENV`,
`PYTHONPATH`, `LUA_INIT`, `LUA_PATH`, `LUA_CPATH`, `NODE_OPTIONS`, `LD_PRELOAD`,
`PATH`, `HOME`, and `ATTUNE_API_TOKEN`. The `LUA_` family includes versioned
startup/module-path controls such as `LUA_INIT_5_4`; Lua can execute startup
code before the fixed script ([Lua standalone contract](https://www.lua.org/manual/5.4/manual.html#7)).
Operator `env_vars` and passthrough settings remain separate trusted configuration.
Declare only names whose values the fixed action treats as data.

Admission reads the immutable executable snapshot, requiring the captured parent
runtime and every eligible available captured version to permit each name.
Version eligibility uses the action's captured constraint. Differing declarations
may therefore reject a request before worker placement; policy errors name the
variable, never its value. The worker also checks the exact captured version it
selects after placement. Authoring edits do not change existing selections.
Workflow/callback inheritance and retries are validated at repository creation
against their own target snapshot; a wider parent declaration grants no child
permission. Queue and enforcement producers supply no overrides by default.
Invalid stored overrides fail execution rather than being silently stripped.
No new snapshot format or compatibility reader is introduced.

Prepend and append operations read selected or explicitly supplied values.
They do not retrieve excluded values from the ambient service environment.
Selected non-UTF-8 OS values remain intact unless an explicit string-valued
runtime or execution setting replaces them.

Execution API tokens come only from the execution's permission snapshot.
No-permission actions receive no `ATTUNE_API_TOKEN`. Dependency setup does not
receive an action or sensor API token. Parameters and key-backed inputs retain
their existing stdin delivery contract.

Managed sensors receive their scoped sensor API token and workload context.
Sensors emit events through `POST /api/v1/events`. The manager supplies neither
`ATTUNE_MQ_URL` nor `ATTUNE_MQ_EXCHANGE` to sensor processes. The sensor service
still uses its own broker connection for lifecycle handling and internal alerts.

## Helm worker pools

The canonical chart lives in the sibling `attune-charts` repository. Each
`actionWorkers` or `sensorWorkers` pool accepts `passthroughEnv`:

```yaml
actionWorkers:
  - name: python
    image: python:3.12-slim
    runtimes: [python, shell]
    passthroughEnv: [HTTPS_PROXY]
    env:
      - name: HTTPS_PROXY
        value: http://proxy.example.internal:3128
```

`env` supplies values to the service container. `passthroughEnv` selects which
of those values reach pack processes.

Action-worker containers import the database URL, broker URL, JWT signing secret,
and encryption key through individual Secret references. Sensor-worker containers
import the first three. Schema and broker readiness containers import their own
required keys. Worker pools do not import the entire runtime Secret.

## Isolation boundary

This policy controls environment inheritance. It does not isolate files,
networks, shared mounts, or same-user process access. Pack code still runs with
the operating-system privileges of its configured runtime container or host.

The shared implementation is `crates/common/src/child_process_environment.rs`.
Worker process/native builders and sensor command builders apply it before
adding explicit context. Regression tests use owned subprocesses with dummy
service credentials, without mutating the test runner's environment.
