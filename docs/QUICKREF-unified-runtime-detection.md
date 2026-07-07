# Quick Reference: Unified Runtime Detection System

**Last Updated:** 2026-07-07  
**Status:** Production-ready

---

## Overview

Both worker and sensor services use a **unified runtime detection system** from `attune-common` that:
- Queries a single `runtime` table (no action/sensor distinction)
- Verifies runtime availability using database-stored verification metadata
- Supports three-tier configuration: env var → config file → database detection

---

## Key Changes from Previous System

### What Changed
- ❌ **Removed:** `runtime_type` field (was `'action'` or `'sensor'`)
- ❌ **Removed:** Duplicate runtime records for actions vs sensors
- ✅ **Added:** Unified `RuntimeDetector` in `attune-common`
- ✅ **Added:** Shared verification logic for all services

### Runtime Refs Changed
```
Before: core.action.python, core.sensor.python
After:  core.python (used by both)
```

---

## Quick Start

### Worker Service

```rust
use attune_common::runtime_detection::RuntimeDetector;

let mut registration = WorkerRegistration::new(pool, &config);
registration.detect_capabilities(&config).await?;
registration.register().await?;
```

### Sensor Service

```rust
use attune_common::runtime_detection::RuntimeDetector;

let mut registration = SensorWorkerRegistration::new(pool, &config);
registration.register(&config).await?;  // Calls detection internally
```

---

## Configuration Priority

### 1. Environment Variable (Highest)
```bash
# Worker
export ATTUNE_WORKER_RUNTIMES="python,shell,node"

# Sensor
export ATTUNE_SENSOR_RUNTIMES="python,shell,builtin"
```

### 2. Config File (Medium)
```yaml
worker:
  capabilities:
    runtimes: ["python", "shell", "native"]

sensor:
  capabilities:
    runtimes: ["python", "shell", "builtin"]
```

### 3. Database Detection (Default)
- Queries all runtimes from `runtime` table
- Verifies each using `distributions->verification` metadata
- Reports only available runtimes

---

## Database Structure

### Runtime Table (Unified)

```sql
CREATE TABLE runtime (
    id BIGSERIAL PRIMARY KEY,
    ref TEXT NOT NULL UNIQUE,
    pack BIGINT REFERENCES pack(id),
    pack_ref TEXT,
    description TEXT,                      -- Human-readable description
    name TEXT NOT NULL,                    -- e.g., "Python", "Node.js", "Shell"
    aliases TEXT[] NOT NULL DEFAULT '{}', -- Lowercase alias list used in matching
    distributions JSONB NOT NULL,          -- Verification + runtime distribution metadata
    installation JSONB,
    installers JSONB DEFAULT '[]',         -- Optional environment installer metadata
    execution_config JSONB NOT NULL DEFAULT '{}',
    auto_detected BOOLEAN NOT NULL DEFAULT FALSE,
    detection_config JSONB NOT NULL DEFAULT '{}',
    created TIMESTAMPTZ DEFAULT NOW(),
    updated TIMESTAMPTZ DEFAULT NOW()
);
```

**No `runtime_type` field** - runtimes are shared between actions and sensors.
`runtime_type` is legacy and should not be used.

### Verification Metadata Structure

```json
{
  "verification": {
    "commands": [
      {
        "binary": "python3",
        "args": ["--version"],
        "exit_code": 0,
        "pattern": "Python 3\\.",
        "priority": 1
      }
    ],
    "always_available": false,
    "check_required": true
  }
}
```

---

## Pack Authoring: Runtime YAML (Recommended Path)

For pack authors, the normal path is to define runtime YAML files in:

```text
<pack>/runtimes/*.yaml
```

These files are loaded by the pack loader (`PackComponentLoader::load_runtimes`) and upserted into `runtime` + optional `runtime_version` rows.

### Runtime YAML Fields

| Field | Required | Notes |
|---|---|---|
| `ref` | Yes | Lowercase runtime ref, format `pack.runtime_name` (e.g., `my_pack.python`) |
| `pack_ref` | Yes (for pack-managed runtimes) | Usually your pack ref |
| `name` | Recommended | Falls back to name inferred from `ref` if omitted |
| `aliases` | Recommended | Lowercase aliases used for matching (e.g., `["python", "python3"]`) |
| `description` | No | Human-readable description |
| `distributions` | Recommended | Include `verification` metadata for detection |
| `installation` | No | Installation/support metadata |
| `execution_config` | Depends | Required for interpreter-managed runtime execution; `{}` is valid for native/direct execution runtimes |
| `versions` | No | Optional array of version-specific runtime configs (`runtime_version` rows) |

### Runtime YAML Template

```yaml
ref: my_pack.python
pack_ref: my_pack
name: Python
aliases: [python, python3]
description: Python runtime for my_pack

distributions:
  verification:
    always_available: false
    check_required: true
    commands:
      - binary: python3
        args: ["--version"]
        exit_code: 0
        pattern: "Python 3\\."
        priority: 1

installation: {}

execution_config:
  interpreter:
    binary: python3
    args: ["-u"]
    file_extension: ".py"
  inline_execution:
    strategy: direct
  environment:
    env_type: virtualenv
    dir_name: ".venv"
    create_command: ["python3", "-m", "venv", "{env_dir}"]
    interpreter_path: "{env_dir}/bin/python3"
  dependencies:
    manifest_file: requirements.txt
    install_command: ["{interpreter}", "-m", "pip", "install", "-r", "{manifest_path}"]
  env_vars:
    PYTHONPATH:
      operation: prepend
      value: "{pack_dir}/lib"
      separator: ":"

versions:
  - version: "3.12"
    is_default: true
    distributions:
      verification:
        commands:
          - binary: python3.12
            args: ["--version"]
            exit_code: 0
            pattern: "Python 3\\.12\\."
            priority: 1
    execution_config:
      interpreter:
        binary: python3.12
        args: ["-u"]
        file_extension: ".py"
```

### `execution_config` Shape (Typed by `RuntimeExecutionConfig`)

`execution_config` maps to:

- `interpreter` (`binary`, `args`, `file_extension`)
- `inline_execution` (`strategy`: `direct` or `temp_file`, optional `extension`, `inject_shell_helpers`)
- `environment` (`env_type`, `dir_name`, `create_command`, `interpreter_path`)
- `dependencies` (`manifest_file`, `install_command`)
- `env_vars` (string form or object form with `value`, `operation`, `separator`)

Supported template variables in runtime templates:

- `{pack_dir}`
- `{env_dir}`
- `{interpreter}`
- `{action_file}`
- `{manifest_path}`

### Version Entries (`versions`)

Each `versions[]` entry supports:

- `version` (required)
- `execution_config` (optional, defaults to `{}`)
- `distributions` (optional, defaults to `{}`)
- `is_default` (optional, defaults to `false`)
- `meta` (optional, defaults to `{}`)

The loader upserts by `(runtime, version)` and removes stale versions that were removed from YAML.

### Loader Behavior That Matters to Authors

1. Runtime rows are upserted by `ref`.
2. `aliases` are normalized to lowercase on load.
3. `auto_detected` and `detection_config` are managed by the system for dynamic runtime registration; pack-authored runtime YAMLs are loaded with `auto_detected = false` and empty `detection_config`.
4. Detection uses `distributions.verification` (`always_available`, `check_required`, `commands`).

---

## Common Runtime Refs

| Runtime | Ref | Always Available? |
|---------|-----|-------------------|
| Python | `core.python` | No (requires verification) |
| Node.js | `core.nodejs` | No (requires verification) |
| Shell | `core.shell` | Yes |
| Native | `core.native` | Yes |
| Built-in Sensors | `core.sensor.builtin` | Yes (sensor-only) |

---

## RuntimeDetector API

### Methods

```rust
pub struct RuntimeDetector {
    pool: PgPool,
}

impl RuntimeDetector {
    pub fn new(pool: PgPool) -> Self

    pub async fn detect_capabilities(
        &self,
        config: &Config,
        env_var_name: &str,
        config_capabilities: Option<&HashMap<String, serde_json::Value>>,
    ) -> Result<HashMap<String, serde_json::Value>>

    pub async fn detect_from_database(&self) -> Result<Vec<String>>

    pub async fn verify_runtime_available(runtime: &Runtime) -> bool
}
```

### Example Usage

```rust
use attune_common::runtime_detection::RuntimeDetector;

let detector = RuntimeDetector::new(pool.clone());

// For worker service
let capabilities = detector
    .detect_capabilities(
        &config,
        "ATTUNE_WORKER_RUNTIMES",
        config.worker.as_ref().and_then(|w| w.capabilities.as_ref())
    )
    .await?;

// For sensor service
let capabilities = detector
    .detect_capabilities(
        &config,
        "ATTUNE_SENSOR_RUNTIMES",
        config.sensor.as_ref().and_then(|s| s.capabilities.as_ref())
    )
    .await?;
```

---

## Migration

### Apply Migration

```bash
cd attune
sqlx migrate run
```

**Migration:** `20260203000001_unify_runtimes.sql`

**What It Does:**
- Consolidates duplicate runtime records
- Migrates foreign keys in `action` and `sensor` tables
- Drops `runtime_type` column and enum
- Updates indexes

### Verify Migration

```sql
-- Check unified runtimes
SELECT ref, name FROM runtime ORDER BY ref;

-- Expected:
-- core.native
-- core.nodejs
-- core.python
-- core.sensor.builtin
-- core.shell

-- Check worker capabilities
SELECT name, capabilities->'runtimes' FROM worker;
```

---

## Adding New Runtimes

### 1. Add to Database

```sql
INSERT INTO runtime (ref, pack, pack_ref, name, distributions)
VALUES (
    'core.ruby',
    (SELECT id FROM pack WHERE ref = 'core'),
    'core',
    'Ruby',
    jsonb_build_object(
        'verification', jsonb_build_object(
            'commands', jsonb_build_array(
                jsonb_build_object(
                    'binary', 'ruby',
                    'args', jsonb_build_array('--version'),
                    'exit_code', 0,
                    'pattern', 'ruby \d+\.\d+',
                    'priority', 1
                )
            )
        )
    )
);
```

### 2. Restart Services

Services will automatically detect the new runtime on next startup.

### 3. Verify

```sql
SELECT name, capabilities->'runtimes' FROM worker WHERE name = 'worker-hostname';
-- Should include 'ruby' if installed
```

---

## Troubleshooting

### Runtime Not Detected

**Check verification command:**
```bash
python3 --version  # Does this work?
node --version     # Does this work?
```

**Check database metadata:**
```sql
SELECT ref, distributions->'verification' FROM runtime WHERE ref = 'core.python';
```

**Force detection:**
```bash
unset ATTUNE_WORKER_RUNTIMES  # Remove env override
# Restart service - will query database
```

### Wrong Runtimes Reported

**Priority order:**
1. Env var overrides everything
2. Config file if no env var
3. Database detection if neither

**Check env:**
```bash
env | grep ATTUNE.*RUNTIMES
```

**Check config:**
```bash
cat config.development.yaml | grep -A5 capabilities
```

### Update Runtime Verification

```sql
UPDATE runtime
SET distributions = jsonb_set(
    distributions,
    '{verification,commands,0,binary}',
    '"python3.11"'
)
WHERE ref = 'core.python';
```

Restart services to pick up changes.

---

## Code Locations

### Core Module
- `crates/common/src/runtime_detection.rs` - RuntimeDetector implementation
- `crates/common/src/models.rs` - Runtime model (no runtime_type)
- `crates/common/src/repositories/runtime.rs` - Database operations

### Service Integration
- `crates/worker/src/registration.rs` - Worker uses RuntimeDetector
- `crates/sensor/src/sensor_worker_registration.rs` - Sensor uses RuntimeDetector

### Migration
- `migrations/20260203000001_unify_runtimes.sql` - Schema changes

---

## Testing

### Unit Tests
```bash
cargo test -p attune-common runtime_detection
```

### Integration Tests
```bash
cargo test --test repository_runtime_tests
```

### Manual Verification
```bash
# Start worker with debug logging
RUST_LOG=debug cargo run -p attune-worker

# Check logs for:
# - "Detecting worker capabilities..."
# - "✓ Runtime available: Python (core.python)"
# - "Detected available runtimes: ["python", "shell", "native"]"
```

---

## Performance

### Query Optimization
- Runtime detection happens **once at startup**
- Results cached in worker registration
- No runtime queries during action/sensor execution

### Indexing
```sql
CREATE INDEX idx_runtime_name ON runtime(name);
CREATE INDEX idx_runtime_verification ON runtime USING gin ((distributions->'verification'));
```

---

## Security Considerations

### Command Execution
- Verification commands run **at startup only**
- Commands from database (trusted source)
- Output parsed with regex, not eval'd
- Non-zero exit codes handled safely

### Environment Overrides
- Env vars allow operators to restrict runtimes
- Useful for security-sensitive environments
- Can disable verification entirely with explicit list

---

## Future Enhancements

### Planned Features
1. **Version Constraints:** Require Python >=3.9
2. **Capability Matching:** Route work to compatible workers
3. **Health Checks:** Re-verify runtimes periodically
4. **API Endpoints:** GET /api/workers/{id}/capabilities

### Contribution Guide
- Add verification metadata to new runtime records
- Update `RuntimeDetector` for new verification types
- Keep worker and sensor services using shared detector

---

## Summary

✅ **One Runtime Table** - No action/sensor distinction  
✅ **Shared Detection Logic** - In `attune-common`  
✅ **Three-Tier Config** - Env → Config → Database  
✅ **Database-Driven** - Verification metadata in JSONB  
✅ **Extensible** - Add runtimes via SQL inserts  

**Migration Required:** Yes (`20260203000001_unify_runtimes.sql`)  
**Breaking Changes:** Yes (pre-production only)  
**Production Ready:** ✅ Yes