# Workflow Orchestration Implementation Plan

## Executive Summary

This document records the original implementation plan and its current status.
Workflow orchestration is implemented; unchecked items below are remaining
work rather than evidence that the core executor is absent.

The active executor does not use a standalone workflow coordinator. The
`ExecutionScheduler` detects workflow actions, creates durable child
executions, and publishes them through RabbitMQ. `CompletionListener` consumes
worker completions and calls `ExecutionScheduler::advance_workflow`. A child
whose action is itself a workflow recursively follows the same path, and its
terminal result (including `output_map`) is returned to the outer workflow.

## Key Design Decisions

### 1. Workflows as Actions
Workflows are first-class actions that can be:
- Triggered by rules (event-driven)
- Invoked by other workflows (composable)
- Executed directly via API
- Referenced in the same way as regular actions

### 2. YAML-Based Definition
Workflows use action metadata in `actions/<name>.yaml` plus a graph-only file in
`actions/workflows/<name>.workflow.yaml`, linked by `workflow_file`. This makes them:
- Version-controllable
- Human-readable
- Easy to author and maintain
- Portable across environments

### 3. Event-Driven Execution
Workflows leverage the existing message queue infrastructure:
- Each task creates a child execution
- Tasks execute asynchronously via workers
- Progress is tracked via execution status messages
- No blocking or polling required

### 4. Multi-Scope Variable System
Variables are accessible from 6 scopes (in precedence order):
1. `task.*` - Results from completed tasks
2. `workflow.*` - Workflow-scoped variables
3. `parameters.*` - Input parameters
4. `config.*` - Pack configuration
5. `system.*` - System variables (execution_id, timestamp, identity)
6. `keystore.*` - Decrypted key-store values

## Architecture Overview

```
┌────────────────────────────────────────────────────────────┐
│                    Attune Platform                          │
├────────────────────────────────────────────────────────────┤
│                                                              │
│  ┌─────────────┐  ┌────────────┐  ┌──────────────────┐    │
│  │ API Service │  │  Executor  │  │ Worker Service   │    │
│  │             │  │  Service   │  │                  │    │
│  │  Workflow   │  │            │  │  Runtime Engine  │    │
│  │  CRUD       │  │ ┌────────┐ │  │                  │    │
│  │             │  │ │Workflow│ │  │  Execute Actions │    │
│  │             │  │ │Engine  │ │  │                  │    │
│  └─────────────┘  │ │        │ │  └──────────────────┘    │
│                    │ │- Parser│ │                          │
│                    │ │- Graph │ │                          │
│                    │ │- Context│ │                         │
│                    │ │- Sched │ │                          │
│                    │ └────────┘ │                          │
│                    └────────────┘                          │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐  │
│  │           PostgreSQL Database                        │  │
│  │  - workflow_definition                               │  │
│  │  - workflow_execution                                │  │
│  │  - workflow_task_execution                           │  │
│  └──────────────────────────────────────────────────────┘  │
│                                                              │
└────────────────────────────────────────────────────────────┘
```

## Database Schema Changes

### New Tables

1. **`workflow_definition`**
   - Stores parsed workflow YAML as JSON
   - Links to pack
   - Contains parameter/output schemas
   - Full task graph definition

2. **`workflow_execution`**
   - Tracks runtime state of workflow
   - Stores variable context
   - Maintains task completion tracking
   - Links to parent execution

3. **`workflow_task_execution`**
   - Individual task execution tracking
   - Supports iteration (with-items)
   - Retry tracking
   - Result storage

### Modified Tables

- **`action`** table gets two new columns:
  - `is_workflow` (boolean)
  - `workflow_def` (foreign key)

## Core Features

### 1. Sequential Execution
Tasks execute one after another based on transitions:
```yaml
tasks:
  - name: task1
    action: pack.action1
    next:
      - when: "{{ succeeded() }}"
        do: [task2]
  - name: task2
    action: pack.action2
```

### 2. Parallel Execution
Multiple tasks execute concurrently:
```yaml
tasks:
  - name: start_checks
    action: core.noop
    next:
      - when: "{{ succeeded() }}"
        do: [check_db, check_cache]

  - name: check_db
    action: db.health

  - name: check_cache
    action: cache.health
```

### 3. Conditional Branching
Execute tasks based on conditions:
```yaml
tasks:
  - name: check_env
    action: core.noop
    next:
      - when: "{{ parameters.env == 'production' }}"
        do: [require_approval]
      - do: [deploy_directly]
```

### 4. Iteration (with-items)
Process lists with optional batching:
```yaml
tasks:
  - name: deploy_regions
    action: deploy.to_region
    with_items: "{{ parameters.regions }}"
    batch_size: 5  # Process 5 at a time
    input:
      region: "{{ item }}"
```

### 5. Variable Publishing
Tasks can publish results to workflow scope:
```yaml
tasks:
  - name: create_resource
    action: cloud.create
    next:
      - when: "{{ succeeded() }}"
        publish:
          - resource_id: "{{ result().id }}"
          - resource_url: "{{ result().url }}"
```

### 6. Error Handling & Retry
Built-in retry with backoff:
```yaml
tasks:
  - name: flaky_task
    action: http.request
    retry:
      count: 5
      delay: 10
      backoff: exponential
    next:
      - when: "{{ succeeded() }}"
        do: [next_task]
      - when: "{{ failed() }}"
        do: [cleanup_task]
```

### 7. Human-in-the-Loop
Integrate inquiry (approval) steps:
```yaml
tasks:
  - name: require_approval
    action: core.ask
    input:
      prompt: "Approve deployment?"
      response_schema:
        approved:
          type: boolean
          required: true
    next:
      - when: "{{ succeeded() and result().response.approved }}"
        do: [deploy]
      - do: [cancel]
```

### 8. Nested Workflows
Workflows can invoke other workflows:
```yaml
tasks:
  - name: provision_infra
    action: infrastructure.full_stack  # This is also a workflow
    input:
      environment: "{{ parameters.env }}"
```

## Template System

### Workflow Expression Engine
- Template interpolation with `{{ ... }}`
- Type-preserving pure expressions
- Arithmetic, comparisons, boolean logic, membership, and member access
- Built-in workflow functions such as `result()`, `succeeded()`, and `length()`

### Helper Functions
```yaml
# Comparisons and boolean logic
deploy: "{{ parameters.replicas > 1 and parameters.environment == 'production' }}"

# List length
count: "{{ length(workflow.items) }}"

# Pack configuration and decrypted keys
value: "{{ config.key }}"
secret: "{{ keystore.api.token }}"
```

## Workflow Lifecycle

```
1. Rule/API triggers workflow action
   ↓
2. Executor detects is_workflow=true
   ↓
3. Load workflow_definition from database
   ↓
4. Create workflow_execution record
   ↓
5. Initialize variable context with parameters
   ↓
6. Build task dependency graph
   ↓
7. Schedule initial tasks (entry points)
   ↓
8. For each task:
   a. Template task inputs
   b. Create child execution
   c. Create workflow_task_execution record
   d. Publish execution.scheduled message
   ↓
9. Worker executes task, publishes result
   ↓
10. Workflow Engine receives completion:
    a. Update workflow_task_execution
    b. Publish variables to context
    c. Evaluate transitions
    d. Schedule next tasks
    ↓
11. Repeat until all tasks complete
    ↓
12. Update workflow_execution status
    ↓
13. Publish workflow.completed event
```

## Historical Implementation Phases

These phases are retained as a status record. Paths in the original
deliverables were projections; the active implementation is primarily in
`crates/executor/src/scheduler.rs`,
`crates/executor/src/completion_listener.rs`, and
`crates/executor/src/workflow/`.

### Phase 1: Foundation (2 weeks)
**Goal**: Core data structures and parsing

- [x] Database migration for workflow tables
- [x] Add workflow models to the common crate
- [x] Create workflow repositories
- [x] Implement YAML parser for workflow definitions
- [x] Implement workflow template rendering and typed expressions
- [x] Create variable context manager

**Deliverables**:
- Migration: `migrations/20250101000006_workflow_system.sql`
- Models: `crates/common/src/models.rs`
- Repositories: `crates/common/src/repositories/workflow.rs`
- Parser and shared expressions: `crates/common/src/workflow/`
- Executor context: `crates/executor/src/workflow/context.rs`

### Phase 2: Execution Engine (2 weeks)
**Goal**: Core workflow execution logic

- [x] Implement task graph builder
- [x] Implement graph traversal logic
- [x] Integrate workflow orchestration into `ExecutionScheduler`
- [x] Publish child execution requests through RabbitMQ
- [x] Implement task scheduling
- [x] Handle task completion events in `CompletionListener`
- [x] Execute nested workflows recursively through the scheduler
- [x] Return nested workflow `output_map` results to the outer workflow

**Deliverables**:
- Graph engine: `crates/executor/src/workflow/graph.rs`
- Executor and state advancement: `crates/executor/src/scheduler.rs`
- Completion handling: `crates/executor/src/completion_listener.rs`

### Phase 3: Advanced Features (2 weeks)
**Goal**: Iteration, parallelism, error handling

- [x] Implement with-items iteration
- [x] Add batching and concurrency support
- [x] Implement parallel task execution through transition fan-out
- [x] Add retry logic with backoff
- [x] Implement task timeout handling
- [x] Add conditional branching

**Deliverables**:
- Iteration, fan-out, retry, and timeout dispatch: `crates/executor/src/scheduler.rs`
- Durable cache iteration state: `crates/common/src/repositories/workflow_cache_iteration.rs`

### Phase 4: API & Tools (2 weeks)
**Goal**: Management interface and tooling

- [x] Workflow CRUD API endpoints
- [x] Workflow execution monitoring through execution APIs
- [x] Workflow cancellation with child cascading
- [ ] Workflow pause/resume operations
- [ ] Dedicated workflow validation CLI command
- [ ] Dedicated workflow graph visualization API endpoint
- [x] Pack registration workflow scanning and validation

**Deliverables**:
- API routes and handlers: `crates/api/src/routes/workflows.rs`
- CLI commands: `crates/cli/src/commands/workflow.rs` (upload/list/show/delete implemented; validation remains open)
- Documentation updates

### Phase 5: Testing & Documentation (1 week)
**Goal**: Comprehensive testing and docs

- [x] Unit tests for core workflow components
- [ ] Comprehensive DB/MQ integration and end-to-end workflow coverage
- [x] Example workflows
- [x] User documentation
- [x] API documentation
- [ ] Migration guide

**Deliverables**:
- Unit tests colocated with workflow and scheduler modules
- E2E workflows: `tests/e2e/tier*/test_*workflow*.py`
- Examples: `docs/examples/*workflow*.yaml`
- User guide: `docs/guides/workflow-quickstart.md`
- API guide: `docs/api/api-workflows.md`

The original estimate was nine weeks. It is historical and is not a current
delivery forecast.

## Testing Strategy

### Unit Tests
- Template rendering with all scope types
- Graph construction and traversal
- Condition evaluation
- Variable publishing
- Task scheduling logic
- Retry logic
- Timeout handling

### Integration Tests
- Simple sequential workflow
- Parallel execution workflow
- Conditional branching workflow
- Iteration workflow (with batching)
- Error handling and retry
- Nested workflow execution
- Workflow cancellation
- Long-running workflow

### Example Test Workflows
Located in `docs/examples/`:
- `simple-workflow.yaml` - Legacy standalone sequential fixture
- `complete-workflow.yaml` - Legacy standalone feature fixture
- `parallel-workflow.yaml` - Parallel execution
- `conditional-workflow.yaml` - Branching logic
- `iteration-workflow.yaml` - with-items examples

## API Endpoints

### Workflow Management
```
POST   /api/v1/packs/{pack_ref}/workflows          - Create workflow
GET    /api/v1/packs/{pack_ref}/workflows          - List workflows in pack
GET    /api/v1/workflows                           - List all workflows
GET    /api/v1/workflows/{workflow_ref}            - Get workflow definition
PUT    /api/v1/workflows/{workflow_ref}            - Update workflow
DELETE /api/v1/workflows/{workflow_ref}            - Delete workflow
POST   /api/v1/workflows/{workflow_ref}/execute    - Execute workflow directly
POST   /api/v1/workflows/{workflow_ref}/validate   - Validate workflow definition
```

### Workflow Execution Management
```
GET    /api/v1/workflow-executions                 - List workflow executions
GET    /api/v1/workflow-executions/{id}            - Get workflow execution details
GET    /api/v1/workflow-executions/{id}/tasks      - List task executions
GET    /api/v1/workflow-executions/{id}/graph      - Get execution graph (visualization)
GET    /api/v1/workflow-executions/{id}/context    - Get variable context
POST   /api/v1/workflow-executions/{id}/pause      - Pause workflow
POST   /api/v1/workflow-executions/{id}/resume     - Resume paused workflow
POST   /api/v1/workflow-executions/{id}/cancel     - Cancel workflow
POST   /api/v1/workflow-executions/{id}/retry      - Retry failed workflow
```

## Pack Structure with Workflows

```
packs/
└── my_pack/
    ├── pack.yaml               # Pack metadata
    ├── config.yaml             # Pack configuration schema
    ├── actions/
    │   ├── action1.py
    │   ├── action2.py
    │   ├── action.yaml
    │   ├── deploy.yaml         # Workflow action metadata
    │   └── workflows/
    │       └── deploy.workflow.yaml # Graph-only definition
    ├── sensors/
    │   ├── sensor1.py
    │   └── sensor.yaml
    ├── rules/
    │   └── on_push.yaml
    └── tests/
        ├── test_actions.py
        └── test_workflows.yaml  # Workflow test definitions
```

### Pack Registration Process

When a pack is registered:
1. Scan action metadata and resolve `workflow_file` relative to `actions/`
2. Parse and validate each referenced graph-only workflow definition
3. Create `workflow_definition` record in database
4. Create synthetic `action` record with `is_workflow=true`
5. Link action to workflow via `workflow_def` foreign key
6. Workflow is now invokable like any other action

## Performance Considerations

### Optimizations
1. **Graph Caching**: Cache parsed task graphs per workflow definition
2. **Template Compilation**: Compile templates once, reuse for iterations
3. **Parallel Scheduling**: Schedule independent tasks concurrently
4. **Database Batching**: Batch task creation/updates when using with-items
5. **Context Serialization**: Use efficient JSON serialization for variable context

### Resource Limits
- Max workflow depth: 10 levels (prevent infinite recursion)
- Max tasks per workflow: 1000 (prevent resource exhaustion)
- Max iterations per with-items: 10,000 (configurable)
- Max parallel tasks: 100 (configurable)
- Variable context size: 10MB (prevent memory issues)

## Security Considerations

1. **Template Injection**: Sanitize all template inputs, no arbitrary code execution
2. **Variable Scoping**: Strict isolation between workflow executions
3. **Secret Access**: Expose `keystore.*` only to authorized workflow executions
4. **Resource Limits**: Enforce max task count, depth, iterations
5. **Audit Trail**: Log all workflow decisions, transitions, variable changes
6. **RBAC**: Workflow execution requires action execution permissions
7. **Input Validation**: Validate parameters against param_schema

## Monitoring & Observability

### Metrics to Track
- Workflow executions per second
- Average workflow duration
- Task execution duration (p50, p95, p99)
- Workflow success/failure rates
- Task retry counts
- Queue depth for workflow tasks
- Variable context size distribution

### Logging Standards
```
INFO  [workflow.start] execution=123 workflow=deploy_app version=1.0.0
INFO  [workflow.task.schedule] execution=123 task=build_image
INFO  [workflow.task.complete] execution=123 task=build_image duration=45s
INFO  [workflow.vars.publish] execution=123 vars=["image_uri"]
INFO  [workflow.task.schedule] execution=123 tasks=["deploy","health_check"]
WARN  [workflow.task.retry] execution=123 task=flaky_api attempt=2
ERROR [workflow.task.failed] execution=123 task=deploy_db error="connection_timeout"
INFO  [workflow.complete] execution=123 status=success duration=2m30s
```

### Distributed Tracing
- Propagate `trace_id` through entire workflow
- Link all task executions to parent workflow
- Enable end-to-end request tracing
- Integration with OpenTelemetry (future)

## Dependencies

### New Rust Crates
- **tera** (^1.19) - Template engine
- **petgraph** (^0.6) - Graph data structures and algorithms

### Existing Dependencies
- sqlx - Database access
- serde/serde_json - Serialization
- tokio - Async runtime
- lapin - RabbitMQ client

## Future Enhancements

### Short Term (3-6 months)
- Workflow versioning (multiple versions of same workflow)
- Workflow pausing/resuming with state persistence
- Advanced retry strategies (circuit breaker, adaptive)
- Workflow templates (reusable patterns)

### Medium Term (6-12 months)
- Dynamic workflows (generate graph at runtime)
- Workflow debugging tools (step-through execution)
- Performance analytics and optimization suggestions
- Workflow marketplace (share workflows)

### Long Term (12+ months)
- Visual workflow editor (drag-and-drop UI)
- AI-powered workflow generation
- Workflow optimization recommendations
- Multi-cloud orchestration patterns

## Success Criteria Status

- [x] Workflows can be defined in YAML and registered via packs
- [x] Scheduler/MQ/listener orchestration executes workflow task graphs
- [x] Variables are scoped and rendered across workflow contexts
- [x] Parallel fan-out and joins are supported
- [x] Iteration supports lists, batching, and concurrency limits
- [x] Task error handling, retry, and timeout behavior are supported
- [x] Human-in-the-loop inquiry actions are supported
- [x] Nested workflows execute and return mapped outputs
- [x] Workflow CRUD and execution cancellation APIs are available
- [ ] Workflow pause/resume controls are available
- [ ] Comprehensive DB/MQ integration and full-service E2E coverage is complete

## References

- Full design: `docs/workflow-orchestration.md`
- Canonical action metadata: `docs/examples/cache-iteration-workflow-action.yaml`
- Canonical graph: `docs/examples/cache-iteration-workflow.workflow.yaml`
- Migration SQL: `docs/examples/workflow-migration.sql`

## Remaining Work

Prioritize the unchecked items in the historical phases and success criteria;
do not use the completed phase checklist as the current product backlog.
