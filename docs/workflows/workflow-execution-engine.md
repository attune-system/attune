# Workflow Execution Engine

## Overview

The Workflow Execution Engine is responsible for orchestrating the execution of workflows in Attune. It manages task dependencies, parallel execution, state transitions, context passing, retries, timeouts, and error handling.

## Architecture

The execution engine consists of four main components:

### 1. Task Graph Builder (`workflow/graph.rs`)

**Purpose:** Converts workflow definitions into executable task graphs with dependency information.

**Key Features:**
- Builds directed acyclic graph (DAG) from workflow tasks
- Topological sorting for execution order
- Dependency computation from task transitions
- Cycle detection
- Entry point identification

**Data Structures:**
- `TaskGraph`: Complete executable graph with nodes, dependencies, and execution order
- `TaskNode`: Individual task with configuration, transitions, and dependencies
- `TaskTransitions`: Success/failure/complete/timeout transitions and decision branches
- `RetryConfig`: Retry configuration with backoff strategies

**Example Usage:**
```rust
use attune_executor::workflow::{TaskGraph, parse_workflow_yaml};

let workflow = parse_workflow_yaml(yaml_content)?;
let graph = TaskGraph::from_workflow(&workflow)?;

// Get entry points (tasks with no dependencies)
for entry in &graph.entry_points {
    println!("Entry point: {}", entry);
}

// Get tasks ready to execute
let completed = HashSet::new();
let ready = graph.ready_tasks(&completed);
```

### 2. Context Manager (`workflow/context.rs`)

**Purpose:** Manages workflow execution context, including variables, parameters, and template rendering.

**Key Features:**
- Workflow-level and task-level variable management
- Jinja2-like template rendering with `{{ variable }}` syntax
- Task result storage and retrieval
- With-items iteration support (current item and index)
- Nested value access (e.g., `{{ parameters.config.server.port }}`)
- Context import/export for persistence

**Variable Scopes:**
- `parameters.*` - Input parameters to the workflow
- `vars.*` or `variables.*` - Workflow-scoped variables
- `task.*` or `tasks.*` - Task results
- `item` - Current item in with-items iteration
- `index` - Current index in with-items iteration
- `system.*` - System variables (e.g., workflow start time)

**Example Usage:**
```rust
use attune_executor::workflow::WorkflowContext;
use serde_json::json;

let params = json!({"name": "Alice"});
let mut ctx = WorkflowContext::new(params, HashMap::new());

// Render template
let result = ctx.render_template("Hello {{ parameters.name }}!")?;
// Result: "Hello Alice!"

// Store task result
ctx.set_task_result("task1", json!({"status": "success"}));

// Publish variables from result
let result = json!({"output": "value"});
ctx.publish_from_result(&result, &["my_var".to_string()], None)?;
```

### 3. Task Executor (`workflow/task_executor.rs`)

**Purpose:** Executes individual workflow tasks with support for different task types, retries, and timeouts.

**Key Features:**
- Action task execution (queues actions for workers)
- Parallel task execution (spawns multiple tasks concurrently)
- Workflow task execution (nested workflows - TODO)
- With-items iteration (batch processing with concurrency limits)
- Conditional execution (when clauses)
- Retry logic with configurable backoff strategies
- Timeout handling
- Task result publishing to context

**Task Types:**
- **Action**: Execute a single action
- **Parallel**: Execute multiple sub-tasks concurrently
- **Workflow**: Execute a nested workflow (not yet implemented)

**Retry Strategies:**
- **Constant**: Fixed delay between retries
- **Linear**: Linearly increasing delay
- **Exponential**: Exponentially increasing delay with optional max delay

**Example Task Execution Flow:**
```
1. Check if task should be skipped (when condition)
2. Check if task has with-items iteration
   - If yes, process items in batches with concurrency limits
   - If no, execute single task
3. Render task input with context
4. Execute based on task type (action/parallel/workflow)
5. Apply timeout if configured
6. Handle retries on failure
7. Publish variables from result
8. Update task execution record in database
```

### 4. Workflow Coordinator (`workflow/coordinator.rs`)

**Purpose:** Main orchestration component that manages the complete workflow execution lifecycle.

**Key Features:**
- Workflow lifecycle management (start, pause, resume, cancel)
- State management (completed, failed, skipped tasks)
- Concurrent task execution coordination
- Database state persistence
- Execution result aggregation
- Error handling and recovery

**Workflow Execution States:**
- `Requested` - Workflow execution requested
- `Scheduling` - Being scheduled
- `Scheduled` - Ready to execute
- `Running` - Currently executing
- `Completed` - Successfully completed
- `Failed` - Failed with errors
- `Cancelled` - Cancelled by user
- `Timeout` - Timed out

**Example Usage:**
```rust
use attune_executor::workflow::WorkflowCoordinator;
use serde_json::json;

let coordinator = WorkflowCoordinator::new(db_pool, mq);

// Start workflow execution
let handle = coordinator
    .start_workflow("my_pack.my_workflow", json!({"param": "value"}), None)
    .await?;

// Execute to completion
let result = handle.execute().await?;

println!("Status: {:?}", result.status);
println!("Completed tasks: {}", result.completed_tasks);
println!("Failed tasks: {}", result.failed_tasks);

// Or control execution
handle.pause(Some("User requested pause".to_string())).await?;
handle.resume().await?;
handle.cancel().await?;

// Check status
let status = handle.status().await;
println!("Current: {}/{} tasks", status.completed_tasks, status.total_tasks);
```

## Execution Flow

### High-Level Workflow Execution

```
1. Load workflow definition from database
2. Parse workflow YAML definition
3. Build task graph with dependencies
4. Create parent execution record
5. Initialize workflow context with parameters and variables
6. Create workflow execution record in database
7. Enter execution loop:
   a. Check if workflow is paused -> wait
   b. Check if workflow is complete -> exit
   c. Get ready tasks (dependencies satisfied)
   d. Spawn async execution for each ready task
   e. Wait briefly before checking again
8. Aggregate results and return
```

### Task Execution Flow

```
1. Create task execution record in database
2. Get current workflow context
3. Execute task (action/parallel/workflow/with-items)
4. Update task execution record with result
5. Update workflow state:
   - Add to completed_tasks on success
   - Add to failed_tasks on failure (unless retrying)
   - Add to skipped_tasks if skipped
   - Update context with task result
6. Persist workflow state to database
```

## Database Schema

### workflow_execution Table

Stores workflow execution state:

```sql
CREATE TABLE attune.workflow_execution (
    id BIGSERIAL PRIMARY KEY,
    execution BIGINT NOT NULL REFERENCES attune.execution(id),
    workflow_def BIGINT NOT NULL REFERENCES attune.workflow_definition(id),
    current_tasks TEXT[] NOT NULL DEFAULT '{}',
    completed_tasks TEXT[] NOT NULL DEFAULT '{}',
    failed_tasks TEXT[] NOT NULL DEFAULT '{}',
    skipped_tasks TEXT[] NOT NULL DEFAULT '{}',
    variables JSONB NOT NULL DEFAULT '{}',
    task_graph JSONB NOT NULL,
    status execution_status_enum NOT NULL,
    error_message TEXT,
    paused BOOLEAN NOT NULL DEFAULT false,
    pause_reason TEXT,
    created TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);
```

### workflow_task_execution Table

Stores individual task execution state:

```sql
CREATE TABLE attune.workflow_task_execution (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES attune.workflow_execution(id),
    execution BIGINT NOT NULL REFERENCES attune.execution(id),
    task_name TEXT NOT NULL,
    task_index INTEGER,
    task_batch INTEGER,
    status execution_status_enum NOT NULL,
    started_at TIMESTAMP WITH TIME ZONE,
    completed_at TIMESTAMP WITH TIME ZONE,
    duration_ms BIGINT,
    result JSONB,
    error JSONB,
    retry_count INTEGER NOT NULL DEFAULT 0,
    max_retries INTEGER NOT NULL DEFAULT 0,
    next_retry_at TIMESTAMP WITH TIME ZONE,
    timeout_seconds INTEGER,
    timed_out BOOLEAN NOT NULL DEFAULT false,
    created TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW(),
    updated TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT NOW()
);
```

## Template Rendering

### Syntax

Templates use Jinja2-like syntax with `{{ expression }}`:

```yaml
tasks:
  - name: greet
    action: core.echo
    input:
      message: "Hello {{ parameters.name }}!"
      
  - name: process
    action: core.process
    input:
      data: "{{ task.greet.result.output }}"
      count: "{{ variables.counter }}"
```

### Supported Expressions

**Parameters:**
```
{{ parameters.name }}
{{ parameters.config.server.port }}
```

**Variables:**
```
{{ vars.my_variable }}
{{ variables.counter }}
{{ my_var }}  # Direct variable reference
```

**Task Results:**
```
{{ task.task_name.result }}
{{ task.task_name.output.key }}
{{ tasks.previous_task.status }}
```

**With-Items Context:**
```
{{ item }}
{{ item.name }}
{{ index }}
```

**System Variables:**
```
{{ system.workflow_start }}
```

## With-Items Iteration

Execute a task multiple times with different items:

```yaml
tasks:
  - name: process_servers
    action: server.configure
    with_items: "{{ parameters.servers }}"
    batch_size: 5        # Process 5 items at a time
    concurrency: 10      # Max 10 concurrent executions
    input:
      server: "{{ item.hostname }}"
      index: "{{ index }}"
```

**Features:**
- Batch processing: Process items in batches of specified size
- Concurrency control: Limit number of concurrent executions
- Context isolation: Each iteration has its own `item` and `index`
- Result aggregation: All results collected in array

## Native Data Cache Iteration

`iterate_cache` is a native task iteration source. The executor, rather than
the called action, owns generation selection, cursor traversal, the durable
retention pin, and resume checkpoints.

```yaml
- name: process_users
  action: examples.process_user_batch
  iterate_cache:
    owner_type: pack
    owner_ref: examples
    namespace: users
    generation: active
    require_fresh: false
    page_size: 100
  batch_size: 25
  concurrency: 4
  permission_set_refs: [standard]
  input:
    users: "{{ item }}"
    batch_index: "{{ index }}"
```

### Field Contract

| Field | Default | Validation |
| --- | --- | --- |
| `owner_type` | `pack` | `system`, `identity`, `pack`, `action`, or `sensor`. |
| `owner_ref` | Owner-dependent | Pack defaults to the workflow pack; required for `action` and `sensor`; omitted for `system` and current identity. |
| `namespace` | Required | A valid namespace in the selected owner scope. |
| `generation` | `active` | `active`, an integer generation ID, or a template resolving to either. |
| `require_fresh` | `false` | Boolean. `true` fails before dispatch when the selected generation is stale. |
| `page_size` | `100` | Integer from `1` through `1000`; bounds each internal scan fetch only. |
| Task `batch_size` | `1` | Integer from `1` through `1000`; entries delivered to each child. |
| Task `concurrency` | `1` | Positive integer; maximum child batches in flight. |

`iterate_cache` is mutually exclusive with `with_items`. `batch_size` is a
supported task field and is independent of `page_size`.
Conditions, timeout, retry, placement, permissions, input rendering, and `next`
transitions otherwise use normal action-task semantics.

### Batch Context

At `batch_size: 1`, `item` has this exact shape:

```json
{
  "external_id": "opaque-case-sensitive-id",
  "value": {},
  "source_updated_at": null,
  "source_checksum": null,
  "size_bytes": 2
}
```

At `batch_size` greater than `1`, `item` is an array of entry objects and the
last batch may be smaller. `index` is the stable zero-based batch ordinal.
There is no `item.generation_id` or `item.entries` wrapper. Entries retain
bytewise `external_id` order. An authoritative empty generation succeeds
without running the called action; an unpopulated namespace fails.

### Pinning and Resume

The executor resolves `generation` once and uses that immutable generation for
the complete task. A concurrent promotion cannot change the task's data.

The durable iteration row pins the generation while its state is `scanning`;
cache cleanup excludes that generation. There is no renewable lease or lease
metadata. The workflow engine makes the iteration terminal on completion,
failure, or cancellation. Workflow terminal-state handling and supervisor
workflow remediation terminalize abandoned scanning rows so terminal workflows
do not retain cache generations indefinitely.

Iteration state persists the generation ID, last external ID, next batch
ordinal, scanned/dispatched counts, and child lineage. Executor restart and
workflow resume reconcile that state with child executions, continue after the
saved external ID, and do not switch to latest or recreate completed children.

### Retries and Failures

Normal task `retry` is evaluated per child batch. Every retry receives the
same persisted child input and `index`; it does not advance or rewind the scan
cursor. The iterator does not expose page-level retry counters or summaries.

After a child exhausts its retries, the executor stops discovering and
dispatching new batches. Already-running children are allowed to reach a
terminal state, and the iterator transitions once through the normal failed
path. Authorization denial,
staleness with `require_fresh: true`, unpopulated cache, explicit-generation
mismatch, and scan failure are task failures. No failure path skips entries or
mixes generations.

Children are normal durable executions. If a child is committed in `requested`
state but its MQ request is lost, supervisor's requested-execution recovery
republishes the request after the configured grace period. The workflow task
identity and batch ordinal keep creation idempotent.

### Permissions and Disclosure

The executor evaluates the task's resolved `permission_set_refs` before
namespace resolution and on every page. `standard` grants read-only cache
access for the signed executing/containing action and pack scopes. Cross-owner
iteration requires a delegable named permission set with a constrained
`caches:read` grant. Each named ref must already have been delegated onto the
parent workflow execution; child dispatch cannot introduce new named authority.
Native iteration never grants cache writes. The child does not receive an API
token unless its resolved task permissions require one.

Cache entries appear only in the explicitly rendered child input. They are not
copied into workflow variables, iterator results, audits, transition events,
errors, or structured iterator logs. Child action code remains responsible for
not logging or returning cache values.

### Observability

The protected
`GET /api/v1/executions/{id}/workflow-cache-iterations` endpoint returns
`task_name`, `namespace_id`, `generation_id`, `state`, `scanned_count`,
`dispatched_count`, `page_size`, `batch_size`, `concurrency`, `created`,
`updated`, `completed_at`, and a bounded `error_summary`. It omits cursor and
entry data. The workflow execution details panel renders task name, state,
generation ID, scanned/dispatched counts, batch/page sizes, concurrency,
timestamps, and the bounded error summary. It does not invent lease,
freshness, page-retry, or failed-page fields.

## Retry Strategies

### Constant Backoff

Fixed delay between retries:

```yaml
tasks:
  - name: flaky_task
    action: external.api_call
    retry:
      count: 3
      delay: 10        # 10 seconds between each retry
      backoff: constant
```

### Linear Backoff

Linearly increasing delay:

```yaml
retry:
  count: 5
  delay: 5
  backoff: linear
# Delays: 5s, 10s, 15s, 20s, 25s
```

### Exponential Backoff

Exponentially increasing delay:

```yaml
retry:
  count: 5
  delay: 2
  backoff: exponential
  max_delay: 60
# Delays: 2s, 4s, 8s, 16s, 32s (capped at 60s)
```

## Task Transitions

Control workflow flow with transitions:

```yaml
tasks:
  - name: check
    action: core.check_status
    on_success: deploy      # Go to deploy on success
    on_failure: rollback    # Go to rollback on failure
    on_complete: notify     # Always go to notify
    on_timeout: alert       # Go to alert on timeout
    
  - name: decision
    action: core.evaluate
    decision:
      - when: "{{ task.decision.result.action == 'approve' }}"
        next: deploy
      - when: "{{ task.decision.result.action == 'reject' }}"
        next: rollback
      - default: true
        next: manual_review
```

## Error Handling

### Task Execution Errors

Errors are captured with:
- Error message
- Error type
- Optional error details (JSON)

### Workflow Failure Handling

- Individual task failures don't immediately stop the workflow
- Dependent tasks won't execute if prerequisites failed
- Workflow completes when all executable tasks finish
- Final status is `Failed` if any task failed

### Retry on Error

```yaml
retry:
  count: 3
  delay: 5
  backoff: exponential
  on_error: "{{ result.error_code == 'RETRY_ABLE' }}"  # Only retry specific errors
```

## Parallel Execution

Execute multiple tasks concurrently:

```yaml
tasks:
  - name: parallel_checks
    type: parallel
    tasks:
      - name: check_service_a
        action: monitoring.check_health
        input:
          service: "service-a"
      
      - name: check_service_b
        action: monitoring.check_health
        input:
          service: "service-b"
      
      - name: check_database
        action: monitoring.check_db
    
    on_success: deploy
    on_failure: abort
```

**Features:**
- All sub-tasks execute concurrently
- Parent task waits for all sub-tasks to complete
- Success only if all sub-tasks succeed
- Individual sub-task results aggregated

## Conditional Execution

Skip tasks based on conditions:

```yaml
tasks:
  - name: deploy
    action: deployment.deploy
    when: "{{ parameters.environment == 'production' }}"
    input:
      version: "{{ parameters.version }}"
```

**When Clause Evaluation:**
- Template rendered with current context
- Evaluated as boolean (truthy/falsy)
- Task skipped if condition is false

## State Persistence

Workflow state is persisted to the database after every task completion:

- Current executing tasks
- Completed tasks list
- Failed tasks list
- Skipped tasks list
- Workflow variables (entire context)
- Execution status
- Pause state and reason
- Error messages

This enables:
- Workflow resume after service restart
- Pause/resume functionality
- Execution history and auditing
- Progress monitoring

## Integration Points

### Message Queue

Tasks queue action executions via RabbitMQ:

```rust
// Task executor creates execution record
let execution = create_execution_record(...).await?;

// Queues execution for worker (TODO: implement MQ publishing)
self.mq.publish_execution_request(execution.id, action_ref, &input).await?;
```

### Worker Coordination

- Executor creates execution records
- Workers pick up and execute actions
- Workers update execution status
- Coordinator monitors completion (TODO: implement completion listener)

### Event Publishing

Workflow events should be published for:
- Workflow started
- Workflow completed/failed
- Task started/completed/failed
- Workflow paused/resumed/cancelled

## Future Enhancements

### TODO Items

1. **Completion Listener**: Listen for task completion events from workers
2. **Nested Workflows**: Execute workflows as tasks within workflows
3. **MQ Publishing**: Implement actual message queue publishing for action execution
4. **Advanced Expressions**: Support comparisons, logical operators in templates
5. **Error Condition Evaluation**: Evaluate `on_error` expressions for selective retries
6. **Workflow Timeouts**: Global workflow timeout configuration
7. **Task Dependencies**: Explicit `depends_on` task specification
8. **Loop Constructs**: While/until loops in addition to with-items
9. **Manual Steps**: Human-in-the-loop approval tasks
10. **Sub-workflow Output**: Capture and use nested workflow results

## Testing

### Unit Tests

Each module includes unit tests:

```bash
# Run all executor tests
cargo test -p attune-executor

# Run specific module tests
cargo test -p attune-executor --lib workflow::graph
cargo test -p attune-executor --lib workflow::context
```

### Integration Tests

Integration tests require database and message queue:

```bash
# Set up test database
export DATABASE_URL="postgresql://attune_test:attune_test@localhost:5432/attune_test"
sqlx migrate run

# Run integration tests
cargo test -p attune-executor --test '*'
```

## Performance Considerations

### Concurrency

- Parallel tasks execute truly concurrently using `futures::join_all`
- With-items supports configurable concurrency limits
- Task graph execution is optimized with topological sorting

### Database Operations

- Workflow state persisted after each task completion
- Batch operations used where possible
- Connection pooling for database access

### Memory

- Task graphs and contexts can be large for complex workflows
- Consider workflow size limits in production
- Context variables should be reasonably sized

## Troubleshooting

### Workflow Not Progressing

**Symptoms**: Workflow stuck in Running state

**Causes**:
- Circular dependencies (should be caught during parsing)
- All tasks waiting on failed dependencies
- Database connection issues

**Solution**: Check workflow state in database, review task dependencies

### Tasks Not Executing

**Symptoms**: Ready tasks not starting

**Causes**:
- Worker service not running
- Message queue not connected
- Execution records not being created

**Solution**: Check worker logs, verify MQ connection, check database

### Template Rendering Errors

**Symptoms**: Tasks fail with template errors

**Causes**:
- Invalid variable references
- Missing context data
- Malformed expressions

**Solution**: Validate templates, check available context variables

## Examples

See `docs/workflows/` for complete workflow examples demonstrating:
- Sequential workflows
- Parallel execution
- With-items iteration
- Native generation-pinned Data Cache iteration in
  `docs/examples/cache-iteration-workflow-action.yaml` and
  `docs/examples/cache-iteration-workflow.workflow.yaml`
- Conditional execution
- Error handling and retries
- Complex workflows with decisions

## Related Documentation

- [Workflow Definition and Orchestration](workflow-orchestration.md)
- [Pack Integration](../api/api-pack-workflows.md)
- [Execution API](../api/api-executions.md)
- [RabbitMQ Queues](../QUICKREF-rabbitmq-queues.md)
