# Core Pack Integration Guide

**Last Updated**: 2026-01-20  
**Status**: Implementation Guide

> **⚠️ Note:** This document was written during early planning. Some code examples reference the now-removed `runtime_type` field and old 3-part runtime ref format (`core.action.shell`). The current architecture uses unified runtimes with 2-part refs (`core.shell`) and determines executability by the presence of `execution_config`. See `docs/QUICKREF-unified-runtime-detection.md` for the current model.

---

## Overview

This document outlines the steps required to integrate the filesystem-based core pack with the Attune platform. The core pack has been implemented in `packs/core/` and needs to be loaded into the system during startup or installation.

---

## Current State

### ✅ Completed

- **Pack Structure**: Complete filesystem-based pack in `packs/core/`
- **Actions**: 4 actions implemented (echo, sleep, noop, http_request)
- **Triggers**: 3 trigger type definitions (intervaltimer, crontimer, datetimetimer)
- **Sensors**: 1 sensor implementation (interval_timer_sensor)
- **Documentation**: Comprehensive README and pack structure docs
- **Testing**: Manual validation of action execution

### ⏳ Pending Integration

- **Pack Loader**: Service to parse and register pack components
- **Database Registration**: Insert pack metadata and components into PostgreSQL
- **Worker Integration**: Execute actions from pack directory
- **Sensor Integration**: Load and run sensors from pack directory
- **Startup Process**: Automatic pack loading on service startup

---

## Integration Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Attune Services                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────────────────────────────────────────────┐  │
│  │            Pack Loader Service                        │  │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐           │  │
│  │  │  Parse   │  │ Validate │  │ Register │           │  │
│  │  │ pack.yaml│→ │ Schemas  │→ │ in DB    │           │  │
│  │  └──────────┘  └──────────┘  └──────────┘           │  │
│  └──────────────────────────────────────────────────────┘  │
│           ↓                                                  │
│  ┌──────────────────────────────────────────────────────┐  │
│  │              PostgreSQL Database                      │  │
│  │  ┌──────┐  ┌────────┐  ┌─────────┐  ┌─────────┐    │  │
│  │  │ Pack │  │ Action │  │ Trigger │  │ Sensor  │    │  │
│  │  │ Meta │  │  Meta  │  │  Meta   │  │  Meta   │    │  │
│  │  └──────┘  └────────┘  └─────────┘  └─────────┘    │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                               │
│  ┌──────────────────┐              ┌──────────────────┐    │
│  │  Worker Service  │              │  Sensor Service  │    │
│  │  ┌────────────┐  │              │  ┌────────────┐  │    │
│  │  │  Execute   │  │              │  │    Run     │  │    │
│  │  │  Actions   │  │              │  │  Sensors   │  │    │
│  │  │  from Pack │  │              │  │  from Pack │  │    │
│  │  └────────────┘  │              │  └────────────┘  │    │
│  └──────────────────┘              └──────────────────┘    │
│           ↑                                  ↑               │
└───────────┼──────────────────────────────────┼──────────────┘
            │                                  │
            └──────────────┬───────────────────┘
                           ↓
                  ┌─────────────────┐
                  │  Filesystem     │
                  │  packs/core/    │
                  │  - actions/     │
                  │  - sensors/     │
                  │  - triggers/    │
                  └─────────────────┘
```

---

## Implementation Steps

### Phase 1: Pack Loader Service

Create a pack loader service in `crates/common/src/pack_loader.rs` (or as a separate crate).

#### 1.1 Pack Parser

```rust
// Parse pack.yaml manifest
pub struct PackLoader {
    pack_dir: PathBuf,
}

pub struct PackManifest {
    pub ref_: String,
    pub label: String,
    pub description: String,
    pub version: String,
    pub author: Option<String>,
    pub email: Option<String>,
    pub system: bool,
    pub enabled: bool,
    pub conf_schema: Option<serde_json::Value>,
    pub config: Option<serde_json::Value>,
    pub meta: Option<serde_json::Value>,
    pub tags: Vec<String>,
    pub runtime_deps: Vec<String>,
}

impl PackLoader {
    pub fn load_manifest(&self) -> Result<PackManifest> {
        // Parse packs/{pack_name}/pack.yaml
    }
}
```

#### 1.2 Component Parsers

```rust
pub struct ActionMetadata {
    pub name: String,
    pub ref_: String,
    pub description: String,
    pub runner_type: String,
    pub entry_point: String,
    pub enabled: bool,
    pub parameters: Option<serde_json::Value>,
    pub output_schema: Option<serde_json::Value>,
    pub tags: Vec<String>,
}

pub struct TriggerMetadata {
    pub name: String,
    pub ref_: String,
    pub description: String,
    pub type_: String,
    pub enabled: bool,
    pub parameters_schema: Option<serde_json::Value>,
    pub payload_schema: Option<serde_json::Value>,
    pub tags: Vec<String>,
}

pub struct SensorMetadata {
    pub name: String,
    pub ref_: String,
    pub description: String,
    pub runner_type: String,
    pub entry_point: String,
    pub trigger_types: Vec<String>,
    pub enabled: bool,
    pub parameters: Option<serde_json::Value>,
    pub poll_interval: Option<i32>,
    pub tags: Vec<String>,
}

impl PackLoader {
    pub fn load_actions(&self) -> Result<Vec<ActionMetadata>> {
        // Parse actions/*.yaml files
    }

    pub fn load_triggers(&self) -> Result<Vec<TriggerMetadata>> {
        // Parse triggers/*.yaml files
    }

    pub fn load_sensors(&self) -> Result<Vec<SensorMetadata>> {
        // Parse sensors/*.yaml files
    }
}
```

#### 1.3 Database Registration

```rust
impl PackLoader {
    pub async fn register_pack(
        &self,
        pool: &PgPool,
        manifest: &PackManifest,
    ) -> Result<i64> {
        // Insert into attune.pack table
        // Returns pack ID
    }

    pub async fn register_actions(
        &self,
        pool: &PgPool,
        pack_id: i64,
        actions: &[ActionMetadata],
    ) -> Result<()> {
        // Insert into attune.action table
    }

    pub async fn register_triggers(
        &self,
        pool: &PgPool,
        pack_id: i64,
        triggers: &[TriggerMetadata],
    ) -> Result<()> {
        // Insert into attune.trigger table
    }

    pub async fn register_sensors(
        &self,
        pool: &PgPool,
        pack_id: i64,
        sensors: &[SensorMetadata],
    ) -> Result<()> {
        // Insert into attune.sensor table
    }
}
```

#### 1.4 Pack Loading Function

```rust
pub async fn load_pack(
    pack_dir: PathBuf,
    pool: &PgPool,
) -> Result<()> {
    let loader = PackLoader::new(pack_dir);

    // Parse pack manifest
    let manifest = loader.load_manifest()?;

    // Register pack
    let pack_id = loader.register_pack(pool, &manifest).await?;

    // Load and register components
    let actions = loader.load_actions()?;
    loader.register_actions(pool, pack_id, &actions).await?;

    let triggers = loader.load_triggers()?;
    loader.register_triggers(pool, pack_id, &triggers).await?;

    let sensors = loader.load_sensors()?;
    loader.register_sensors(pool, pack_id, &sensors).await?;

    info!("Pack '{}' loaded successfully", manifest.ref_);
    Ok(())
}
```

---

### Phase 2: Worker Service Integration

Update the worker service to execute actions from the filesystem.

#### 2.1 Action Execution Path Resolution

```rust
pub struct ActionExecutor {
    packs_dir: PathBuf,
}

impl ActionExecutor {
    pub fn resolve_action_path(
        &self,
        pack_ref: &str,
        entry_point: &str,
    ) -> Result<PathBuf> {
        // packs/{pack_ref}/actions/{entry_point}
        let path = self.packs_dir
            .join(pack_ref)
            .join("actions")
            .join(entry_point);

        if !path.exists() {
            return Err(Error::ActionNotFound(entry_point.to_string()));
        }

        Ok(path)
    }
}
```

#### 2.2 Environment Variable Setup

```rust
pub fn prepare_action_env(
    params: &HashMap<String, serde_json::Value>,
) -> HashMap<String, String> {
    let mut env = HashMap::new();

    for (key, value) in params {
        let env_key = format!("ATTUNE_ACTION_{}", key.to_uppercase());
        let env_value = match value {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::Bool(b) => b.to_string(),
            _ => serde_json::to_string(value).unwrap(),
        };
        env.insert(env_key, env_value);
    }

    env
}
```

#### 2.3 Action Execution

```rust
pub async fn execute_action(
    &self,
    action: &Action,
    params: HashMap<String, serde_json::Value>,
) -> Result<ExecutionResult> {
    // Resolve action script path
    let script_path = self.resolve_action_path(
        &action.pack_ref,
        &action.entrypoint,
    )?;

    // Prepare environment variables
    let env = prepare_action_env(&params);

    // Execute based on runtime name (resolved from runtime.name, lowercased)
    let output = match runtime_name.as_str() {
        "shell" => self.execute_shell_action(script_path, env).await?,
        "python" => self.execute_python_action(script_path, env).await?,
        _ => return Err(Error::UnsupportedRuntime(runtime_name.clone())),
    };

    Ok(output)
}
```

---

### Phase 3: Sensor Service Integration

Update the sensor service to load and run sensors from the filesystem.

#### 3.1 Sensor Path Resolution

```rust
pub struct SensorManager {
    packs_dir: PathBuf,
}

impl SensorManager {
    pub fn resolve_sensor_path(
        &self,
        pack_ref: &str,
        entry_point: &str,
    ) -> Result<PathBuf> {
        // packs/{pack_ref}/sensors/{entry_point}
        let path = self.packs_dir
            .join(pack_ref)
            .join("sensors")
            .join(entry_point);

        if !path.exists() {
            return Err(Error::SensorNotFound(entry_point.to_string()));
        }

        Ok(path)
    }
}
```

#### 3.2 Sensor startup values

The manager writes one bounded stdin JSON object and closes the pipe:

```json
{"config": {"check_interval_seconds": 1}, "triggers": []}
```

`config` is the flat sensor settings map. `triggers` contains initial active rule
bindings with `id`, `ref`, `trigger_ref`, and `config` fields. Both sections may
contain secrets. Configuration environment variables are not supported.
Connection metadata, sensor identity, and trigger-ref lists remain in the
cleared child environment.

#### 3.3 Sensor Execution

`crates/sensor/src/sensor_manager.rs` selects the admitted executable release,
clears inherited environment variables, provisions a workload-fenced token, and
starts the process with piped stdin/stdout/stderr. It starts log readers before
writing the bootstrap to avoid pipe deadlocks. Failed or timed-out delivery
terminates and waits for the child. Stdout is a log stream, not an event protocol.
Sensors emit events through the authenticated API.

---

### Phase 4: Service Startup Integration

Add pack loading to service initialization.

#### 4.1 API Service Startup

```rust
// In crates/api/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    // ... existing initialization ...

    // Load core pack
    let packs_dir = PathBuf::from("packs");
    let core_pack_dir = packs_dir.join("core");

    if core_pack_dir.exists() {
        info!("Loading core pack...");
        pack_loader::load_pack(core_pack_dir, &pool).await?;
        info!("Core pack loaded successfully");
    }

    // ... continue with server startup ...
}
```

#### 4.2 Worker Service Startup

```rust
// In crates/worker/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    // ... existing initialization ...

    // Set packs directory for action execution
    let packs_dir = PathBuf::from("packs");
    let executor = ActionExecutor::new(packs_dir);

    // ... continue with worker initialization ...
}
```

#### 4.3 Sensor Service Startup

```rust
// In crates/sensor/src/main.rs

#[tokio::main]
async fn main() -> Result<()> {
    // ... existing initialization ...

    // Set packs directory for sensor execution
    let packs_dir = PathBuf::from("packs");
    let sensor_manager = SensorManager::new(packs_dir);

    // Load and start sensors
    let sensors = load_enabled_sensors(&pool).await?;
    for sensor in sensors {
        sensor_manager.start_sensor(sensor).await?;
    }

    // ... continue with sensor service ...
}
```

---

## Configuration

Add pack-related configuration to `config.yaml`:

```yaml
packs:
  # Directory containing packs
  directory: "./packs"

  # Auto-load packs on startup
  auto_load:
    - core

  # Pack-specific configuration
  core:
    max_action_timeout: 300
    enable_debug_logging: false
```

---

## Database Schema Updates

The existing database schema already supports packs. Ensure these tables are used:

- `attune.pack` - Pack metadata
- `attune.action` - Action definitions
- `attune.trigger` - Trigger type definitions
- `attune.sensor` - Sensor definitions
- `attune.runtime` - Runtime definitions

**Note**: The current `scripts/seed_core_pack.sql` inserts data directly. This should be replaced or complemented by the filesystem-based loader.

---

## Migration Strategy

### Option 1: Replace SQL Seed Script

Remove `scripts/seed_core_pack.sql` and load from filesystem exclusively.

**Pros**: Single source of truth (filesystem)  
**Cons**: Requires pack loader to be implemented first

### Option 2: Dual Approach (Recommended)

Keep SQL seed script for initial setup, add filesystem loader for development/updates.

**Pros**: Works immediately, smooth migration path  
**Cons**: Need to maintain both during transition

**Implementation**:
1. Keep existing SQL seed script for now
2. Implement pack loader service
3. Add CLI command: `attune pack reload core`
4. Eventually replace SQL seed with filesystem loading

---

## Testing Plan

### Unit Tests

- Pack manifest parsing
- Component metadata parsing
- Path resolution
- Environment variable preparation

### Integration Tests

1. **Pack Loading**
   - Load core pack from filesystem
   - Verify database registration
   - Validate component metadata

2. **Action Execution**
   - Execute `core.echo` with parameters
   - Execute `core.http_request` with mock server
   - Verify environment variable passing
   - Capture stdout/stderr correctly

3. **Sensor Execution**
   - Run `core.interval_timer_sensor`
   - Verify event emission
   - Check trigger firing logic

### End-to-End Tests

- Create rule with `core.intervaltimer` trigger
- Verify rule fires and executes `core.echo` action
- Check execution logs and results

---

## Dependencies

### Rust Crates

```toml
[dependencies]
serde_yaml = "0.9"      # Parse YAML files
walkdir = "2.4"         # Traverse pack directories
tokio = { version = "1", features = ["process"] }  # Async process execution
```

### System Dependencies

- Shell (bash/sh) for shell actions
- Python 3.8+ for Python actions
- Python packages: `requests>=2.28.0`, `croniter>=1.4.0`

---

## Rollout Plan

### Week 1: Pack Loader Implementation
- [ ] Create `pack_loader` module in `attune_common`
- [ ] Implement manifest and component parsers
- [ ] Add database registration functions
- [ ] Write unit tests

### Week 2: Worker Integration
- [ ] Add action path resolution
- [ ] Implement environment variable preparation
- [ ] Update action execution to use filesystem
- [ ] Add integration tests

### Week 3: Sensor Integration
- [ ] Add sensor path resolution
- [ ] Implement sensor process management
- [ ] Update event creation from sensor output
- [ ] Add integration tests

### Week 4: Testing & Documentation
- [ ] End-to-end testing
- [ ] CLI commands for pack management
- [ ] Update deployment documentation
- [ ] Performance testing

---

## Success Criteria

- ✅ Core pack loaded from filesystem on startup
- ✅ Actions execute successfully from pack directory
- ✅ Sensors run and emit events correctly
- ✅ Environment variables passed properly to actions/sensors
- ✅ Database contains correct metadata for all components
- ✅ No regression in existing functionality
- ✅ Integration tests pass
- ✅ Documentation updated

---

## Related Documentation

- `packs/core/README.md` - Core pack usage guide
- `docs/pack-structure.md` - Pack structure reference
- `docs/pack-management-architecture.md` - Architecture overview
- `docs/worker-service.md` - Worker service documentation
- `docs/sensor-service.md` - Sensor service documentation

---

## Open Questions

1. **Runtime Registration**: Should we create runtime entries in the database for each runner type (shell, python)?
2. **Pack Versioning**: How to handle pack updates? Replace existing entries or keep version history?
3. **Pack Dependencies**: How to handle dependencies between packs?
4. **Pack Registry**: Future external pack registry integration?
5. **Hot Reload**: Should packs be hot-reloadable without service restart?

---

## Conclusion

Integrating the filesystem-based core pack requires:
1. Pack loader service to parse and register components
2. Worker service updates to execute actions from filesystem
3. Sensor service updates to run sensors from filesystem
4. Startup integration to load packs automatically

The implementation can be phased, starting with the pack loader, then worker integration, then sensor integration. The existing SQL seed script can remain as a fallback during the transition.
