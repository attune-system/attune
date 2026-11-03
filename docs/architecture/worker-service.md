# Worker Service Architecture

## Overview

The **Worker Service** is responsible for executing automation actions in the Attune platform. It receives execution requests from the Executor service, runs actions in appropriate runtime environments (Python, Shell, Node.js, containers), and reports results back.

## Service Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Worker Service                           │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌─────────────────────┐  ┌──────────────────────┐          │
│  │ Worker              │  │ Heartbeat            │          │
│  │ Registration        │  │ Manager              │          │
│  └─────────────────────┘  └──────────────────────┘          │
│           │                         │                         │
│           v                         v                         │
│  ┌─────────────────────────────────────────────┐             │
│  │         Action Executor                     │             │
│  │  ┌─────────────────────────────────────┐   │             │
│  │  │      Runtime Registry               │   │             │
│  │  │  - Python Runtime                   │   │             │
│  │  │  - Shell Runtime                    │   │             │
│  │  │  - Local Runtime (Facade)           │   │             │
│  │  │  - Container Runtime (Future)       │   │             │
│  │  └─────────────────────────────────────┘   │             │
│  └─────────────────────────────────────────────┘             │
│           │                         │                         │
│           v                         v                         │
│  ┌─────────────────────┐  ┌──────────────────────┐          │
│  │ Artifact            │  │ Message Queue        │          │
│  │ Manager             │  │ Consumer/Publisher   │          │
│  └─────────────────────┘  └──────────────────────┘          │
│                                                               │
└─────────────────────────────────────────────────────────────┘
         │                    │                    │
         v                    v                    v
   PostgreSQL            RabbitMQ           Local Filesystem
```

## Core Components

### 1. Worker Registration

**Purpose**: Register worker in the database and maintain worker metadata.

**Responsibilities**:
- Register worker on startup with name, type, capabilities
- Update existing worker records to active status on restart
- Deregister worker on shutdown (mark as inactive)
- Update worker capabilities dynamically
- Preserve operator cordon metadata separately from observed worker status

**Key Implementation Details**:
- Worker name defaults to hostname if not specified
- Capabilities include supported runtimes (python, shell, node)
- Worker type can be Local, Remote, or Container
- Uses direct SQL queries for registration (no repository pattern needed)
- `worker.status` describes observed health; `worker.cordoned` describes operator intent and makes the worker unschedulable while it may continue heartbeating

**Database Table**: `attune.worker`

### 2. Heartbeat Manager

**Purpose**: Keep worker status fresh in the database with periodic heartbeat updates.

**Responsibilities**:
- Send periodic heartbeat updates (default: every 30 seconds)
- Update `last_heartbeat` timestamp in database
- Run in background task until stopped
- Handle transient database errors gracefully

**Key Implementation Details**:
- Runs as a tokio background task with interval ticker
- Configurable heartbeat interval via worker config
- Logs errors but doesn't fail the worker on heartbeat issues
- Clean shutdown on service stop

### 3. Runtime System

**Purpose**: Abstraction layer for executing actions in different environments.

**Components**:

#### Runtime Trait
```rust
pub trait Runtime: Send + Sync {
    fn name(&self) -> &str;
    fn can_execute(&self, context: &ExecutionContext) -> bool;
    async fn execute(&self, context: ExecutionContext) -> RuntimeResult<ExecutionResult>;
    async fn setup(&self) -> RuntimeResult<()>;
    async fn cleanup(&self) -> RuntimeResult<()>;
    async fn validate(&self) -> RuntimeResult<()>;
}
```

#### Python Runtime
- Executes Python scripts via subprocess
- Generates wrapper script to inject parameters
- Supports timeout, stdout/stderr capture
- Parses JSON results from stdout
- Default entry point: `run()` function

**Example Action**:
```python
def run(x, y):
    return x + y
```

#### Shell Runtime
- Executes bash/shell scripts via subprocess
- Injects parameters as environment variables (PARAM_*)
- Supports timeout, output capture
- Executes with `set -e` for error propagation

**Example Action**:
```bash
echo "Hello, $PARAM_NAME!"
```

#### Local Runtime
- Facade that delegates to Python or Shell runtime
- Selects runtime based on action metadata
- Currently supports Python and Shell
- Extensible for additional local runtimes

#### Runtime Registry
- Manages collection of registered runtimes
- Selects appropriate runtime for each action
- Handles runtime setup/cleanup lifecycle

### 4. Action Executor

**Purpose**: Orchestrate the complete execution flow for an action and own execution state after handoff.

**Execution Flow**:
```
1. Receive execution.scheduled message from executor
2. Load execution record from database
3. Update status to Running (owns state after handoff)
4. Load action definition by reference
5. Prepare execution context (parameters, env vars, timeout)
6. Select and execute in appropriate runtime
7. Capture results (stdout, stderr, return value)
8. Store artifacts (logs, results)
9. Update execution status (Completed/Failed) in database
10. Publish status change notifications
11. Publish completion notification for queue management
```

**Ownership Model**:
- **Worker owns execution state** after receiving `execution.scheduled`
- **Authoritative source** for all status updates: Running, Completed, Failed, Cancelled, etc.
- **Updates database directly** for all state changes
- **Publishes notifications** for orchestration and monitoring

**Responsibilities**:
- Coordinate execution lifecycle
- Load action and execution data from database
- **Update execution state in database** (after handoff from executor)
- Prepare execution context with parameters and environment
- Execute action via runtime registry
- Handle success and failure cases
- Store execution artifacts
- Copy locally staged file-backed artifacts to API transport in standalone/API-transport mode
- Publish status change notifications

**Key Implementation Details**:
- Parameters merged: action defaults + execution overrides
- Environment variables include execution metadata
- `ATTUNE_ARTIFACTS_DIR` is always provided; actions that allocate file-backed artifact versions write to `$ATTUNE_ARTIFACTS_DIR/{file_path}`
- When no shared artifact volume is detected, finalization uploads those worker-local files through the API artifact transport before updating artifact version sizes
- Default timeout: 5 minutes (300 seconds)
- Errors captured and stored as execution result

### 5. Artifact Manager

**Purpose**: Store and manage execution artifacts (logs, results, files).

**Artifact Types**:
- **Log**: stdout/stderr from execution
- **Result**: JSON result data from action
- **File**: Custom file outputs from actions
- **Trace**: Debug/trace information (future)

**Storage Structure**:
```
/tmp/attune/artifacts/{worker_name}/
  └── execution_{id}/
      ├── stdout.log
      ├── stderr.log
      └── result.json
```

**Responsibilities**:
- Store logs (stdout/stderr) for each execution
- Store JSON result data
- Support custom file artifacts
- Clean up old artifacts (retention policy)
- Delete artifacts for specific executions

**Key Implementation Details**:
- Creates execution-specific directories
- Stores all IO errors as Internal errors
- Configurable base directory per worker
- Retention policy based on file modification time

### 6. Secret Management

**Purpose**: Securely manage and inject secrets into action execution environments.

**Responsibilities**:
- Fetch secrets from database based on ownership hierarchy
- Decrypt encrypted secrets using AES-256-GCM
- Inject secrets as environment variables
- Clean up secrets after execution

**Secret Ownership Hierarchy**:
1. **System-level secrets** - Available to all actions
2. **Pack-level secrets** - Available to all actions in a pack
3. **Action-level secrets** - Available to specific action only

More specific secrets override less specific ones with the same name.

**Environment Variable Injection**:
- Secret names transformed: `api_key` → `SECRET_API_KEY`
- Prefix: `SECRET_`
- Uppercase with hyphens replaced by underscores

**Encryption**:
- Algorithm: AES-256-GCM (authenticated encryption)
- Key derivation: SHA-256 of the cryptographically random configured server secret
- Format: one Base64 string encoding `nonce || ciphertext || authentication_tag`
- Stored verifier: domain-separated SHA-256 of the AES key, never the AES key itself.
  See [fingerprint remediation](../deployment/encryption-fingerprint-remediation.md)
  before upgrading a populated installation.
- Random nonce per encryption operation

**Key Implementation Details**:
- Encryption key loaded from `security.encryption_key` config
- Key hash validation ensures correct decryption key
- Graceful handling of missing secrets (warning, not failure)
- Secrets never logged or exposed in artifacts
- Automatic injection during execution context preparation

**Configuration**:
```yaml
security:
  encryption_key: "your-secret-encryption-password"
```

**Database Table**: `attune.key`

See `docs/secrets-management.md` for comprehensive documentation.

### 7. Worker Service

**Purpose**: Main service orchestration and message queue integration.

**Responsibilities**:
- Initialize all service components
- Register worker in database
- Start heartbeat manager
- Consume execution messages from worker-specific queue
- **Own execution state** after receiving scheduled executions
- **Update execution status in database** (Running, Completed, Failed, etc.)
- Publish execution status change notifications
- Publish execution completion notifications
- Handle graceful shutdown
- Respect cordon state: cordoned workers are excluded from new scheduling, but existing work is not cancelled solely because of cordon

**Message Flow**:
```
Executor (Scheduler) 
  → Publishes: execution.scheduled 
    → Queue: worker.{worker_id}.executions
      → Worker consumes message
        → Executes action
          → Publishes: execution.status.running
            → Publishes: execution.status.succeeded/failed
```

**Message Types**:

**Consumed**:
- `execution.scheduled` - New execution assigned to this worker

**Published**:
- `execution.status.running` - Execution started
- `execution.status.succeeded` - Execution completed successfully
- `execution.status.failed` - Execution failed

**Key Implementation Details**:
- Worker-specific queues enable direct routing from scheduler
- Database and MQ connections initialized on startup
- Graceful shutdown deregisters worker
- Message handlers run async and report errors

### Operational visibility

- Worker list API responses include computed heartbeat age, stale-heartbeat flag, and health state.
- Admin/operator users can cordon and uncordon action and sensor workers through `/api/v1/workers/{id}/cordon` and `/api/v1/workers/{id}/uncordon`.
- The executor reconciles `running` executions on unavailable workers to `abandoned` and publishes the normal completion message so downstream workflow, queue, notifier, and history paths observe a terminal state.
- Unexpected non-cordoned worker loss emits a structured `core.alert` event.

## Configuration

Worker service uses the standard Attune configuration system:

```yaml
# config.yaml
database:
  url: postgresql://localhost/attune
  max_connections: 20

message_queue:
  url: amqp://localhost
  exchange: attune.executions

worker:
  name: worker-01                    # Optional, defaults to hostname
  worker_type: Local                 # Local, Remote, Container
  runtime_id: null                   # Optional runtime association
  host: null                         # Optional, defaults to hostname
  port: null                         # Optional
  max_concurrent_tasks: 10           # Max parallel executions
  heartbeat_interval: 30             # Seconds between heartbeats
  task_timeout: 300                  # Default task timeout (seconds)
  execution_log_retention_policy: days      # action stdout/stderr artifact retention policy: versions, days, hours, minutes
  execution_log_retention_limit: 7          # Retention limit interpreted by the selected policy

security:
  encryption_key: "your-encryption-key"  # Required for encrypted secrets
```

Environment variable overrides:
```bash
ATTUNE__WORKER__NAME=my-worker
ATTUNE__WORKER__MAX_CONCURRENT_TASKS=20
ATTUNE__WORKER__HEARTBEAT_INTERVAL=60
```

## Running the Service

### Prerequisites

- PostgreSQL 14+ with Attune schema initialized
- RabbitMQ 3.12+ with exchanges and queues configured
- Python 3.x and/or bash (for local runtimes)
- Environment variables or config file set up

### Startup

```bash
# Using cargo
cd crates/worker
cargo run

# With custom config
cargo run -- --config /path/to/config.yaml

# With custom worker name
cargo run -- --name worker-prod-01

# Or with environment overrides
ATTUNE__WORKER__NAME=worker-01 \
ATTUNE__WORKER__MAX_CONCURRENT_TASKS=20 \
cargo run
```

### Graceful Shutdown

The service supports graceful shutdown via SIGTERM/SIGINT (Ctrl+C):
1. Stop accepting new execution messages
2. Finish processing in-flight executions (future enhancement)
3. Stop heartbeat manager
4. Deregister worker (mark as inactive)
5. Close message queue connections
6. Close database connections
7. Exit cleanly

## Execution Context

The executor prepares a comprehensive execution context for each action:

```rust
pub struct ExecutionContext {
    pub execution_id: i64,
    pub action_ref: String,              // "pack.action"
    pub parameters: HashMap<String, JsonValue>,
    pub env: HashMap<String, String>,    // Environment variables
    pub timeout: Option<u64>,            // Timeout in seconds
    pub working_dir: Option<PathBuf>,    // Working directory
    pub entry_point: String,             // Function/script entry point
    pub code: Option<String>,            // Action code (inline)
    pub code_path: Option<PathBuf>,      // Action code (file path)
}
```

### Environment Variables

The executor injects these environment variables:
- `ATTUNE_EXECUTION_ID` - Execution ID
- `ATTUNE_ACTION` - Action reference (pack.action)
- `ATTUNE_RUNNER` - Runner type (if specified)
- `ATTUNE_CONTEXT_*` - Context data as environment variables

For shell actions, parameters are also injected as:
- `PARAM_{KEY}` - Each parameter as uppercase env var

## Execution Result

Actions return a standardized result:

```rust
pub struct ExecutionResult {
    pub exit_code: i32,              // 0 = success
    pub stdout: String,              // Standard output
    pub stderr: String,              // Standard error
    pub result: Option<JsonValue>,   // Parsed result data
    pub duration_ms: u64,            // Execution duration
    pub error: Option<String>,       // Error message if failed
}
```

## Error Handling

### Error Categories

1. **Setup Errors**: Runtime initialization failures
2. **Execution Errors**: Action execution failures
3. **Timeout Errors**: Execution exceeded timeout
4. **IO Errors**: File/network operations
5. **Database Errors**: Connection, query failures

### Error Propagation

- Runtime errors captured in `ExecutionResult.error`
- **Worker updates** execution status to Failed in database (owns state)
- Error published in status change notification message
- Error published in completion notification message
- Artifacts still stored for failed executions
- Logs preserved for debugging

## Testing

### Unit Tests

Each runtime includes unit tests:
- Simple execution
- Parameter passing
- Timeout handling
- Error handling

### Integration Tests

Integration tests require PostgreSQL and RabbitMQ:
- Worker registration and heartbeat
- End-to-end action execution
- Message queue integration
- Artifact storage

### Running Tests

```bash
# Unit tests only
cargo test -p attune-worker --lib

# Integration tests (requires services)
cargo test -p attune-worker --test '*'

# Specific runtime tests
cargo test -p attune-worker python_runtime
cargo test -p attune-worker shell_runtime
```

## Implementation Status

### Phase 5.1: Worker Foundation ✅ COMPLETE
- [x] Worker registration module
- [x] Heartbeat manager
- [x] Service initialization
- [x] Configuration loading

### Phase 5.2: Runtime System ✅ COMPLETE
- [x] Runtime trait abstraction
- [x] Python runtime implementation
- [x] Shell runtime implementation
- [x] Local runtime facade
- [x] Runtime registry

### Phase 5.3: Execution Logic ⏳ IN PROGRESS
- [x] Action executor module
- [x] Execution context preparation
- [ ] Fix data model mismatches
- [ ] Complete message queue integration
- [ ] Test end-to-end flow

### Phase 5.4: Artifact Management ✅ COMPLETE
- [x] Artifact manager module
- [x] Log storage (stdout/stderr)
- [x] Result storage (JSON)
- [x] File artifact storage
- [x] Cleanup/retention policies

### Phase 5.5: Testing 📋 TODO
- [x] Runtime unit tests (basic)
- [ ] Integration tests with database
- [ ] End-to-end execution tests
- [ ] Error handling tests

### Phase 5.6: Advanced Features 📋 TODO
- [ ] Container runtime (Docker)
- [ ] Remote worker support
- [ ] Concurrent execution limits
- [ ] Worker capacity management
- [ ] Execution queuing

## Known Issues

### Data Model Mismatches

The current implementation has several mismatches with the actual database schema:

1. **Execution.action**: Expected String, actual is `Option<i64>`
2. **Execution fields**: Missing `parameters`, `context`, `runner` fields
3. **Action fields**: `entry_point` → `entrypoint`, missing `timeout`
4. **Repository pattern**: Repositories don't have `::new()` constructors
5. **Error types**: `Error::BadRequest` and `Error::NotFound` have different signatures

### Required Fixes

1. Update executor to use `action_ref` field instead of `action`
2. Fix action loading to query by ID from execution
3. Update execution context preparation for actual schema
4. Fix repository usage patterns
5. Update error construction calls
6. Implement From<MqError> for Error

## Future Enhancements

### Phase 1: Core Improvements
- Concurrent execution management (max_concurrent_tasks)
- Worker capacity tracking and reporting
- Execution queuing when at capacity
- Retry logic for transient failures

### Phase 2: Advanced Runtimes
- Container runtime with Docker
- Container image management and caching
- Volume mounting for code injection
- Network isolation for security

### Phase 3: Remote Workers
- Remote worker registration
- Worker-to-worker communication
- Geographic distribution
- Load balancing strategies

### Phase 4: Monitoring & Observability
- Execution metrics (duration, success rate)
- Worker health metrics
- Runtime-specific metrics
- OpenTelemetry integration

### Phase 5: Security
- Execution sandboxing
- Resource limits (CPU, memory)
- Secret injection from key store
- Encrypted artifact storage

## Related Documentation

- [Executor Service](./executor-service.md)
- [API - Executions](./api-executions.md)
- [API - Actions](./api-actions.md)
- [Configuration](./configuration.md)
- [Quick Start](./quick-start.md)
