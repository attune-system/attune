# Core Pack Runtime Metadata

This directory contains runtime metadata YAML files for the core pack. Each file defines a runtime environment that can be used to execute actions and sensors.

## File Structure

Each runtime YAML file contains only the fields that are stored in the database:

- `ref` - Unique runtime reference (format: pack.name)
- `pack_ref` - Pack this runtime belongs to
- `name` - Human-readable runtime name
- `description` - Brief description of the runtime
- `distributions` - Runtime verification and capability metadata (JSONB)
- `installation` - Installation requirements and metadata (JSONB)
- `execution_config` - Interpreter, environment, dependency, and execution-time env var metadata

## `execution_config.env_vars`

Runtime authors can declare execution-time environment variables in a purely declarative way.

String values replace the variable entirely:

```yaml
env_vars:
  NODE_PATH: "{env_dir}/node_modules"
```

Object values support merge semantics against an existing value already present in the execution environment:

```yaml
env_vars:
  PYTHONPATH:
    operation: prepend
    value: "{pack_dir}/lib"
    separator: ":"
```

Supported operations:

- `set` - Replace the variable with the resolved value
- `prepend` - Add the resolved value before the existing value
- `append` - Add the resolved value after the existing value

Supported template variables:

- `{pack_dir}`
- `{env_dir}`
- `{interpreter}`
- `{manifest_path}`

## Available Runtimes

- **python.yaml** - Python 3 runtime for actions and sensors
- **nodejs.yaml** - Node.js runtime for JavaScript-based actions and sensors
- **shell.yaml** - Shell (bash/sh) runtime - always available
- **native.yaml** - Native compiled runtime (Rust, Go, C, etc.) - executes binaries directly without an interpreter

## Loading

Runtime metadata files are loaded by the pack loading system and inserted into the `runtime` table in the database.

## Authoring Checklist (Pack Authors)

When adding a new runtime YAML:

1. Set a lowercase `ref` (`<pack_ref>.<runtime_name>`) and include `pack_ref`.
2. Define `aliases` for expected runtime names (`node`, `nodejs`, etc.).
3. Add `distributions.verification` commands (or `always_available: true` when appropriate).
4. Provide `execution_config` for interpreter-managed runtimes; use `{}` only for native/direct execution runtimes.
5. If you need explicit version targeting, add `versions[]` entries with version-specific `execution_config`.

For full field-level guidance and examples, see:
- `docs/QUICKREF-unified-runtime-detection.md` (`Pack Authoring: Runtime YAML`)
