# Workflow Orchestration Design

## Overview

This document describes the architecture and implementation plan for **workflow orchestration** in Attune. Workflows enable the composition of multiple actions into complex, conditional execution graphs with variable passing, iteration, and error handling.

## Design Philosophy

Workflows in Attune follow these core principles:

1. **Workflows are Actions**: A workflow is itself an action that can be invoked by rules, other workflows, or directly via API
2. **YAML-First**: Workflow definitions are declarative YAML files stored in packs
3. **Event-Driven**: Workflows execute asynchronously via the same message queue infrastructure as regular actions
4. **Composable**: Workflows can invoke other workflows recursively
5. **Observable**: Each task in a workflow creates an execution record with full traceability
6. **Recoverable**: Failed workflows can be resumed or retried from specific tasks

## Architecture Components

### 1. Workflow Definition Format

Workflow actions use two files: action metadata under `actions/` and a graph-only
definition under `actions/workflows/`. The metadata file links the graph with
`workflow_file`.

```yaml
# packs/my_pack/actions/deploy_application.yaml
ref: my_pack.deploy_application
label: "Deploy Application Workflow"
description: "Deploys application with health checks and rollback"
enabled: true
workflow_file: workflows/deploy_application.workflow.yaml

parameters:
  app_name:
    type: string
    required: true
    description: "Name of the application to deploy"
  version:
    type: string
    required: true
  environment:
    type: string
    enum: [dev, staging, production]
    default: dev

output:
  deployment_id:
    type: string
  status:
    type: string
  deployed_version:
    type: string
```

```yaml
# packs/my_pack/actions/workflows/deploy_application.workflow.yaml
version: "1.0.0"

vars:
  deployment_id: null
  health_check_url: null
  rollback_required: false

tasks:
  - name: create_deployment
    action: my_pack.create_deployment_record
    input:
      app_name: "{{ parameters.app_name }}"
      version: "{{ parameters.version }}"
      environment: "{{ parameters.environment }}"
    next:
      - when: "{{ succeeded() }}"
        publish:
          - deployment_id: "{{ result().id }}"
          - health_check_url: "{{ result().health_url }}"
        do: [build_image]
      - when: "{{ failed() }}"
        do: [notify_failure]

  - name: build_image
    action: docker.build_and_push
    input:
      app_name: "{{ parameters.app_name }}"
      version: "{{ parameters.version }}"
      registry: "{{ config.docker_registry }}"
    next:
      - when: "{{ succeeded() }}"
        do: [deploy_containers]
      - when: "{{ failed() }}"
        do: [cleanup_deployment]

  - name: deploy_containers
    action: kubernetes.apply_deployment
    input:
      app_name: "{{ parameters.app_name }}"
      image: "{{ task.build_image.data.image_uri }}"
      replicas: 3
    next:
      - when: "{{ succeeded() }}"
        do: [wait_for_ready]
      - when: "{{ failed() }}"
        do: [rollback_deployment]

  - name: wait_for_ready
    action: kubernetes.wait_for_ready
    input:
      deployment: "{{ parameters.app_name }}"
      timeout: 300
    retry:
      count: 3
      delay: 10
    next:
      - when: "{{ succeeded() }}"
        do: [health_check]
      - when: "{{ failed() }}"
        do: [rollback_deployment]

  - name: health_check
    action: http.get
    input:
      url: "{{ workflow.health_check_url }}"
      expected_status: 200
    next:
      - when: "{{ succeeded() }}"
        do: [update_deployment_status]
      - when: "{{ failed() }}"
        do: [rollback_deployment]

  - name: update_deployment_status
    action: my_pack.update_deployment_status
    input:
      deployment_id: "{{ workflow.deployment_id }}"
      status: "success"
    next:
      - when: "{{ succeeded() }}"
        do: [notify_success]

  - name: rollback_deployment
    action: kubernetes.rollback_deployment
    input:
      app_name: "{{ parameters.app_name }}"
    next:
      - publish:
          - rollback_required: true
        do: [cleanup_deployment]

  - name: cleanup_deployment
    action: my_pack.cleanup_resources
    input:
      deployment_id: "{{ workflow.deployment_id }}"
      rollback: "{{ workflow.rollback_required }}"
    next:
      - do: [notify_failure]

  - name: notify_success
    action: slack.post_message
    input:
      channel: "#deployments"
      message: "✅ Deployed {{ parameters.app_name }} v{{ parameters.version }} to {{ parameters.environment }}"

  - name: notify_failure
    action: slack.post_message
    input:
      channel: "#deployments"
      message: "❌ Failed to deploy {{ parameters.app_name }} v{{ parameters.version }}"

output_map:
  deployment_id: "{{ workflow.deployment_id }}"
  status: "{{ task.update_deployment_status.data.status }}"
  deployed_version: "{{ parameters.version }}"
```

Pure `{{ ... }}` templates preserve arrays, objects, numbers, booleans, and
null. Task output is available canonically as `task.<name>.data`; output fields
are also flattened onto `task.<name>`, so
`task.build_image.image_uri` is an equivalent convenience alias.

### 2. Advanced Workflow Features

#### 2.1 Parallel Execution

Execute multiple tasks concurrently:

```yaml
tasks:
  - name: start_checks
    action: core.noop
    next:
      - when: "{{ succeeded() }}"
        do: [check_database, check_redis, check_rabbitmq]

  - name: check_database
    action: postgres.health_check

  - name: check_redis
    action: redis.ping

  - name: check_rabbitmq
    action: rabbitmq.cluster_status
```

#### 2.2 Iteration (with-items)

Iterate over lists with optional batching:

```yaml
tasks:
  # Without batch_size: process items individually (one execution per item)
  - name: deploy_to_regions
    action: cloud.deploy_instance
    with_items: "{{ parameters.regions }}"
    input:
      # item is the individual region value
      region: "{{ item }}"
      instance_type: "{{ parameters.instance_type }}"
    # Creates one execution per item
    next:
      - when: "{{ succeeded() }}"
        do: [verify_deployments]

  # With batch_size: items split into batches (batch processing)
  - name: process_large_dataset
    action: data.transform
    with_items: "{{ workflow.records }}"
    batch_size: 100  # Process 100 items at a time
    concurrency: 5   # Process up to 5 batches concurrently
    input:
      # item is an array containing up to 100 records
      # Last batch may contain fewer items
      records: "{{ item }}"
    next:
      - when: "{{ succeeded() }}"
        publish:
          - total_processed: "{{ task.process_large_dataset.total_count }}"
```

**Batch Processing Behavior**:
- **Without `batch_size`**: Each item is processed individually (one execution per item)
  - `item` variable contains a single value
  - `index` variable contains the item index (0-based)
- **With `batch_size`**: Items are split into batches and processed as arrays
  - `item` variable contains an array of items (batch)
  - `index` variable contains the batch index (0-based)
  - Last batch may contain fewer items than `batch_size`
  - Use `batch_size` for efficient bulk processing when actions support arrays

#### 2.2.1 Native Data Cache Iteration (`iterate_cache`)

`iterate_cache` is the implemented workflow contract for traversing a Data
Cache. It is not shorthand for an action-authored HTTP loop: the executor pins
one generation, reads bounded internal pages, groups entries into task batches,
and schedules one child action execution per batch.

```yaml
- name: process_cached_users
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
  permission_set_refs:
    - standard
  retry:
    count: 3
    delay: 5
    backoff: exponential
    max_delay: 60
  input:
    users: "{{ item }}"
    batch_index: "{{ index }}"
```

The `iterate_cache` fields and defaults are:

| Field | Required | Default | Behavior |
| --- | --- | --- | --- |
| `owner_type` | No | `pack` | Cache owner type: `system`, `identity`, `pack`, `action`, or `sensor`. |
| `owner_ref` | For `action` and `sensor` | Workflow pack for `pack` | Canonical owner reference. Omit it for `system` and the current-identity scope. |
| `namespace` | Yes | None | Cache namespace in the selected owner scope. |
| `generation` | No | `active` | Selects `active`, an integer generation ID, or a template resolving to either. |
| `require_fresh` | No | `false` | Rejects a stale generation when `true`; stale last-known-good data remains usable by default. |
| `page_size` | No | `100` | Internal scan fetch size from `1` through `1000`; it does not shape child input. |

`concurrency` remains a task field, not an `iterate_cache` field. It limits
in-flight batches and defaults to serial execution (`1`). Task `batch_size` is
supported independently of `page_size`, accepts `1..1000`, and defaults to `1`.
Only `with_items` is mutually exclusive with `iterate_cache`.

At `batch_size: 1`, each child receives one entry object through `item`:

```json
{
  "external_id": "user-001",
  "value": {"display_name": "Example User", "enabled": true},
  "source_updated_at": "2026-08-04T10:00:00Z",
  "source_checksum": "sha256:example",
  "size_bytes": 96
}
```

At `batch_size` greater than `1`, `item` is an array of entry objects and the
final batch may be smaller. `index` is the zero-based batch ordinal. The item
never has `generation_id`, `stale`, or `entries` wrapper fields.

The generation is selected once before the first child is created. Promotion
during iteration cannot mix snapshots. The executor persists the generation,
last external ID, next batch ordinal, counts, and child execution IDs, so
executor restart or workflow resume continues the same generation without
replaying completed batches. It never falls back to the current active
generation.

The durable iteration row pins the generation while its state is `scanning`;
there is no renewable lease. Cache cleanup excludes the pinned generation.
Normal workflow completion, failure, and cancellation make the row terminal,
and workflow terminal-state handling plus supervisor workflow remediation
terminalize abandoned scanning rows so terminal workflows release their pins.

Task `retry` applies independently to each failed child batch and reuses that
batch's input and index. A terminal child failure stops discovering new
batches, allows already-running siblings to settle, and marks the
iterator failed before evaluating its failure transitions. Cache resolution,
freshness, authorization, and scan failures are iterator failures and do not
silently skip records. There are no page-level retry summary fields.

If a durable child remains `requested` because its MQ request was lost,
supervisor's requested-execution recovery republishes it after the configured
grace period. Idempotent workflow task/batch identity avoids duplicate children.

The resolved task permission sets authorize the native reads. `standard`
provides read-only access only to cache namespaces in the executing action/pack
scope and containing workflow action/pack scope; another owner requires a
delegable named permission set with a scoped `caches:read` grant. A child action
may use a named ref only when that ref was already delegated onto the parent
workflow execution. It does not need cache permission merely to consume the
explicitly supplied batch, but it needs its own task token grant for additional
cache API calls.

Cache values are disclosed only in the explicitly configured child input.
Attune does not copy entries into workflow variables, parent results, transition
events, audit details, iterator errors, or iterator log fields. The safe status
endpoint contains only task and namespace/generation IDs, state,
scanned/dispatched counts, configured sizes and concurrency, timestamps, and a
bounded error summary. Authors must not log, publish, or return `item` unless
that disclosure is intentional.

Use `with_items` for an array already present in workflow context and
`iterate_cache` for a potentially high-cardinality, generation-pinned Data
Cache. `with_items` creates item executions from an eagerly rendered array;
`iterate_cache` discovers bounded pages lazily, preserves a cache snapshot and
terminal-state retention pin, and does not materialize the namespace in
workflow context.

#### 2.3 Conditional Execution

Execute tasks based on conditions:

```yaml
tasks:
  - name: check_environment
    action: core.noop
    next:
      - when: "{{ succeeded() and parameters.environment == 'production' }}"
        do: [require_approval]
      - when: "{{ succeeded() and parameters.environment != 'production' }}"
        do: [deploy_app]

  - name: require_approval
    action: core.ask
    input:
      prompt: "Approve production deployment of {{ parameters.app_name }}?"
      response_schema:
        approved:
          type: boolean
          required: true
    next:
      - when: "{{ succeeded() and result().response.approved == true }}"
        do: [deploy_app]
      - when: "{{ succeeded() and result().response.approved != true }}"
        do: [cancel_deployment]
      - when: "{{ timed_out() }}"
        do: [cancel_deployment]
```

#### 2.4 Error Handling and Retry

```yaml
tasks:
  - name: flaky_api_call
    action: http.post
    input:
      url: "{{ workflow.api_endpoint }}"
    retry:
      count: 5
      delay: 10  # seconds
      backoff: exponential  # linear, exponential, constant
      max_delay: 60
      on_error: "{{ task.flaky_api_call.error.type == 'timeout' }}"
    next:
      - when: "{{ succeeded() }}"
        do: [process_response]
      - when: "{{ failed() }}"
        do: [log_error]
```

#### 2.5 Timeout and Cancellation

```yaml
tasks:
  - name: long_running_task
    action: ml.train_model
    input:
      dataset: "{{ workflow.dataset_path }}"
    timeout: 3600  # 1 hour
    next:
      - when: "{{ timed_out() }}"
        do: [cleanup_and_notify]
```

#### 2.6 Subworkflows

Invoke other workflows:

```yaml
tasks:
  - name: provision_infrastructure
    action: infrastructure.provision_stack  # This is also a workflow
    input:
      stack_name: "{{ parameters.app_name }}-{{ parameters.environment }}"
      region: "{{ parameters.region }}"
    next:
      - when: "{{ succeeded() }}"
        do: [deploy_application]
```

## Data Model Changes

### New Tables

#### 1. `workflow_definition` Table

```sql
CREATE TABLE attune.workflow_definition (
    id BIGSERIAL PRIMARY KEY,
    ref VARCHAR(255) NOT NULL UNIQUE,
    pack BIGINT NOT NULL REFERENCES attune.pack(id) ON DELETE CASCADE,
    pack_ref VARCHAR(255) NOT NULL,
    label VARCHAR(255) NOT NULL,
    description TEXT,
    version VARCHAR(50) NOT NULL,
    
    -- Workflow specification (parsed YAML)
    param_schema JSONB,
    out_schema JSONB,
    definition JSONB NOT NULL,  -- Full workflow definition
    
    -- Metadata
    tags TEXT[] DEFAULT '{}',
    enabled BOOLEAN DEFAULT true,
    
    created TIMESTAMPTZ DEFAULT NOW(),
    updated TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_workflow_pack ON attune.workflow_definition(pack);
CREATE INDEX idx_workflow_enabled ON attune.workflow_definition(enabled);
```

#### 2. `workflow_execution` Table

Tracks the state of a workflow execution:

```sql
CREATE TABLE attune.workflow_execution (
    id BIGSERIAL PRIMARY KEY,
    execution BIGINT NOT NULL REFERENCES attune.execution(id) ON DELETE CASCADE,
    workflow_def BIGINT NOT NULL REFERENCES attune.workflow_definition(id),
    
    -- Workflow state
    current_tasks TEXT[] DEFAULT '{}',  -- Currently executing task names
    completed_tasks TEXT[] DEFAULT '{}',
    failed_tasks TEXT[] DEFAULT '{}',
    
    -- Variable context (scoped to this workflow)
    variables JSONB DEFAULT '{}',
    
    -- Graph traversal state
    task_graph JSONB NOT NULL,  -- Adjacency list representation
    
    -- Status tracking
    status attune.execution_status_enum NOT NULL DEFAULT 'requested',
    error_message TEXT,
    
    created TIMESTAMPTZ DEFAULT NOW(),
    updated TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_workflow_exec_execution ON attune.workflow_execution(execution);
CREATE INDEX idx_workflow_exec_status ON attune.workflow_execution(status);
```

#### 3. `workflow_task_execution` Table

Tracks individual task executions within a workflow:

```sql
CREATE TABLE attune.workflow_task_execution (
    id BIGSERIAL PRIMARY KEY,
    workflow_execution BIGINT NOT NULL REFERENCES attune.workflow_execution(id) ON DELETE CASCADE,
    execution BIGINT NOT NULL REFERENCES attune.execution(id) ON DELETE CASCADE,
    
    task_name VARCHAR(255) NOT NULL,
    task_index INTEGER,  -- For with-items iterations
    
    -- Status
    status attune.execution_status_enum NOT NULL DEFAULT 'requested',
    started_at TIMESTAMPTZ,
    completed_at TIMESTAMPTZ,
    
    -- Results
    result JSONB,
    error JSONB,
    
    -- Retry tracking
    retry_count INTEGER DEFAULT 0,
    max_retries INTEGER DEFAULT 0,
    
    created TIMESTAMPTZ DEFAULT NOW(),
    updated TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_wf_task_exec_workflow ON attune.workflow_task_execution(workflow_execution);
CREATE INDEX idx_wf_task_exec_execution ON attune.workflow_task_execution(execution);
CREATE INDEX idx_wf_task_exec_status ON attune.workflow_task_execution(status);
```

### Modified Tables

#### Update `action` Table

Add a flag to distinguish workflow actions:

```sql
ALTER TABLE attune.action ADD COLUMN is_workflow BOOLEAN DEFAULT false;
ALTER TABLE attune.action ADD COLUMN workflow_def BIGINT REFERENCES attune.workflow_definition(id);
```

## Variable Scoping and Templating

### Template Engine

Use a Jinja2-like template engine (recommend **tera** crate in Rust) for variable interpolation.

### Variable Scopes

Variables are accessible from multiple scopes in order of precedence:

1. **`task.*`** - Results from completed tasks
   - `{{ task.task_name.data.field }}` (canonical result object)
   - `{{ task.task_name.field }}` (flattened output alias)
   - `{{ task.task_name.status }}`
   - `{{ task.task_name.error }}`

2. **`workflow.*`** - Workflow-scoped variables
   - `{{ workflow.deployment_id }}`
   - `{{ workflow.custom_variable }}`

3. **`parameters.*`** - Input parameters to the workflow
   - `{{ parameters.app_name }}`
   - `{{ parameters.environment }}`

4. **`config.*`** - Pack configuration
   - `{{ config.api_key }}`
   - `{{ config.base_url }}`

5. **`system.*`** - System-level variables
   - `{{ system.execution_id }}`
   - `{{ system.timestamp }}`
   - `{{ system.identity.login }}`

6. **`keystore.*`** - Decrypted key-store values
   - `{{ keystore.service.api_key }}`

The parser retains compatibility aliases: `vars` and `variables` map to
`workflow`, `tasks` maps to `task`, and legacy `task.<name>.result` resolves to
the same value as `task.<name>.data`. Existing definitions may continue to
parse, but canonical examples should use the namespaces above.

### Special Variables for Iteration

When using `with_items`:

- **`{{ item }}`** - Current item or batch
  - **Without `batch_size`**: Individual item value
  - **With `batch_size`**: Array of items (batch)
- **`{{ index }}`** - Zero-based index
  - **Without `batch_size`**: Item index
  - **With `batch_size`**: Batch index

Example usage:
```yaml
# Without batch_size - individual items
- name: process_one_by_one
  with_items: "{{ parameters.files }}"
  input:
    file: "{{ item }}"        # Single filename
    position: "{{ index }}"   # Item position

# With batch_size - arrays
- name: process_in_batches
  with_items: "{{ parameters.files }}"
  batch_size: 10
  input:
    files: "{{ item }}"       # Array of up to 10 filenames
    batch_num: "{{ index }}"  # Batch number
```

### Template Helper Functions

```yaml
# String manipulation
message: "{{ parameters.app_name | upper }}"
path: "{{ workflow.base_path | trim | append('/logs') }}"

# JSON manipulation
config: "{{ workflow.raw_config | from_json }}"
payload: "{{ workflow.data | to_json }}"

# List operations
regions: "{{ parameters.all_regions | filter(enabled=true) }}"
first_region: "{{ parameters.regions | first }}"
count: "{{ workflow.results | length }}"

# Batching helper
batches: "{{ workflow.large_list | batch(size=100) }}"

# Conditional helpers
status: "{{ task.deploy.status | default('unknown') }}"
url: "{{ workflow.host | default('localhost') | prepend('https://') }}"

# Pack configuration and decrypted keys
flag: "{{ config.feature_flags.enabled }}"
secret: "{{ keystore.api.credentials.token }}"
```

## Workflow Execution Engine

### Architecture

The workflow execution engine is a new component within the **Executor Service**:

```
┌────────────────────────────────────────────────┐
│           Executor Service                      │
├────────────────────────────────────────────────┤
│                                                 │
│  ┌──────────────────┐  ┌───────────────────┐  │
│  │ Enforcement      │  │ Execution         │  │
│  │ Processor        │  │ Scheduler         │  │
│  └──────────────────┘  └───────────────────┘  │
│                                                 │
│  ┌──────────────────────────────────────────┐ │
│  │     Workflow Engine (NEW)                │ │
│  ├──────────────────────────────────────────┤ │
│  │ - Workflow Parser                        │ │
│  │ - Graph Executor                         │ │
│  │ - Variable Context Manager               │ │
│  │ - Task Scheduler                         │ │
│  │ - State Machine                          │ │
│  └──────────────────────────────────────────┘ │
│                                                 │
└────────────────────────────────────────────────┘
```

### Workflow Lifecycle

```
1. Rule triggers workflow action
2. Executor recognizes workflow action (is_workflow = true)
3. Workflow Engine loads workflow definition
4. Create workflow_execution record
5. Initialize variable context with parameters
6. Build task dependency graph
7. Schedule initial tasks (entry points with no dependencies)
8. For each task:
   a. Template task inputs using current variable context
   b. Create child execution for the action
   c. Publish execution.scheduled message
   d. Create workflow_task_execution record
9. Worker executes task and publishes result
10. Workflow Engine receives execution.completed
    a. Update workflow_task_execution
    b. Publish variables to workflow context
    c. Evaluate next tasks based on transitions
    d. Schedule next tasks or complete workflow
11. Repeat until all tasks complete or workflow fails
12. Update parent workflow execution status
13. Publish workflow.completed event
```

### State Machine

```
                   ┌──────────────┐
                   │  Requested   │
                   └──────┬───────┘
                          │
                   ┌──────▼───────┐
                   │  Scheduling  │
                   └──────┬───────┘
                          │
                   ┌──────▼───────┐
                   │   Running    │◄────┐
                   └──┬───┬───┬───┘     │
                      │   │   │         │
         ┌────────────┘   │   └─────┐   │
         │                │         │   │
    ┌────▼────┐    ┌─────▼─────┐   │   │
    │ Paused  │    │  Waiting  │───┘   │
    └────┬────┘    │ (for task)│       │
         │         └───────────┘       │
         └─────────────────────────────┘
                      
         ┌────────────┬────────────┐
         │            │            │
    ┌────▼────┐  ┌───▼─────┐  ┌──▼────────┐
    │Completed│  │ Failed  │  │ Cancelled │
    └─────────┘  └─────────┘  └───────────┘
```

### Core Components (Rust Implementation)

#### 1. Workflow Definition Parser

```rust
// crates/executor/src/workflow/parser.rs

pub struct WorkflowDefinition {
    pub ref_: String,
    pub label: String,
    pub parameters: JsonSchema,
    pub output: JsonSchema,
    pub vars: HashMap<String, JsonValue>,
    pub tasks: Vec<TaskDefinition>,
    pub output_map: HashMap<String, String>,
}

pub struct TaskDefinition {
    pub name: String,
    pub task_type: TaskType,
    pub action: Option<String>,
    pub input: HashMap<String, String>,  // Template strings
    pub publish: Vec<PublishMapping>,
    pub transitions: TaskTransitions,
    pub retry: Option<RetryPolicy>,
    pub timeout: Option<u64>,
    pub when: Option<String>,  // Condition template
    pub with_items: Option<String>,  // List template
    pub batch_size: Option<usize>,
}

pub enum TaskType {
    Action,
    Parallel,
    Workflow,  // Subworkflow
}

pub struct Transition {
    pub when: Option<String>,
    pub publish: Vec<PublishMapping>,
    pub do_tasks: Vec<String>,
}
```

The parser still accepts `on_success`, `on_failure`, `on_complete`,
`on_timeout`, and `decision`, normalizing them into `next` transitions. They
are compatibility syntax, not the canonical authoring model.

#### 2. Variable Context Manager

```rust
// crates/executor/src/workflow/context.rs

pub struct WorkflowContext {
    pub execution_id: i64,
    pub parameters: JsonValue,
    pub vars: HashMap<String, JsonValue>,
    pub task_results: HashMap<String, TaskResult>,
    pub config: JsonValue,
    pub system: SystemContext,
}

impl WorkflowContext {
    pub fn render_template(&self, template: &str) -> Result<String> {
        // Use tera template engine
        let mut tera = Tera::default();
        let context = self.to_tera_context();
        tera.render_str(template, &context)
    }
    
    pub fn publish_variables(&mut self, mappings: &[PublishMapping]) -> Result<()> {
        for mapping in mappings {
            let value = self.render_template(&mapping.template)?;
            self.vars.insert(mapping.var_name.clone(), value);
        }
        Ok(())
    }
}
```

#### 3. Graph Executor

```rust
// crates/executor/src/workflow/graph.rs

pub struct TaskGraph {
    nodes: HashMap<String, TaskNode>,
    edges: HashMap<String, Vec<Edge>>,
}

pub struct TaskNode {
    pub name: String,
    pub definition: TaskDefinition,
    pub status: TaskStatus,
}

pub struct Edge {
    pub from: String,
    pub to: String,
    pub condition: EdgeCondition,
}

pub enum EdgeCondition {
    OnSuccess,
    OnFailure,
    OnComplete,
    OnTimeout,
    Decision(String),  // Template condition
}

impl TaskGraph {
    pub fn find_next_tasks(&self, completed_task: &str, result: &TaskResult) -> Vec<String> {
        // Traverse graph based on task result and transitions
    }
    
    pub fn get_ready_tasks(&self) -> Vec<&TaskNode> {
        // Find tasks with all dependencies satisfied
    }
}
```

#### 4. Workflow Executor

```rust
// crates/executor/src/workflow/executor.rs

pub struct WorkflowExecutor {
    pool: PgPool,
    publisher: MessagePublisher,
    template_engine: Tera,
}

impl WorkflowExecutor {
    pub async fn execute_workflow(
        &self,
        execution_id: i64,
        workflow_def: WorkflowDefinition,
        parameters: JsonValue,
    ) -> Result<()> {
        // 1. Create workflow_execution record
        let wf_exec = self.create_workflow_execution(execution_id, &workflow_def).await?;
        
        // 2. Initialize variable context
        let mut context = WorkflowContext::new(execution_id, parameters, &workflow_def);
        
        // 3. Build task graph
        let graph = TaskGraph::from_definition(&workflow_def)?;
        
        // 4. Schedule initial tasks
        let initial_tasks = graph.get_ready_tasks();
        for task in initial_tasks {
            self.schedule_task(&wf_exec, task, &context).await?;
        }
        
        Ok(())
    }
    
    pub async fn handle_task_completion(
        &self,
        wf_execution_id: i64,
        task_name: String,
        result: TaskResult,
    ) -> Result<()> {
        // 1. Load workflow execution and context
        let wf_exec = self.load_workflow_execution(wf_execution_id).await?;
        let mut context = self.load_context(&wf_exec).await?;
        
        // 2. Update task result in context
        context.task_results.insert(task_name.clone(), result.clone());
        
        // 3. Publish variables from task
        let task_def = wf_exec.definition.get_task(&task_name)?;
        context.publish_variables(&task_def.publish)?;
        
        // 4. Save updated context
        self.save_context(wf_execution_id, &context).await?;
        
        // 5. Determine next tasks
        let next_tasks = wf_exec.graph.find_next_tasks(&task_name, &result);
        
        // 6. Schedule next tasks
        for next_task in next_tasks {
            let task_def = wf_exec.definition.get_task(&next_task)?;
            
            // Evaluate condition if present
            if let Some(when) = &task_def.when {
                let should_run = context.evaluate_condition(when)?;
                if !should_run {
                    continue;
                }
            }
            
            self.schedule_task(&wf_exec, task_def, &context).await?;
        }
        
        // 7. Check if workflow is complete
        if self.is_workflow_complete(&wf_exec).await? {
            self.complete_workflow(wf_execution_id, &context).await?;
        }
        
        Ok(())
    }
    
    async fn schedule_task(
        &self,
        wf_exec: &WorkflowExecution,
        task: &TaskDefinition,
        context: &WorkflowContext,
    ) -> Result<()> {
        // Handle with_items (iteration)
        let items = if let Some(with_items_template) = &task.with_items {
            let items_json = context.render_template(with_items_template)?;
            serde_json::from_str::<Vec<JsonValue>>(&items_json)?
        } else {
            vec![JsonValue::Null]  // Single execution
        };
        
        // Batch items if batch_size specified
        let batches = if let Some(batch_size) = task.batch_size {
            items.chunks(batch_size).collect::<Vec<_>>()
        } else {
            items.iter().map(|i| vec![i]).collect::<Vec<_>>()
        };
        
        for (batch_idx, batch) in batches.iter().enumerate() {
            for (item_idx, item) in batch.iter().enumerate() {
                // Create item-specific context
                let mut item_context = context.clone();
                item_context.add_item_vars(item, item_idx, batch, batch_idx);
                
                // Render task inputs
                let rendered_input = self.render_task_input(task, &item_context)?;
                
                // Create child execution
                let execution = self.create_task_execution(
                    wf_exec.id,
                    &task.action.unwrap(),
                    rendered_input,
                    wf_exec.execution,
                ).await?;
                
                // Create workflow_task_execution record
                self.create_workflow_task_execution(
                    wf_exec.id,
                    execution.id,
                    &task.name,
                    item_idx,
                ).await?;
                
                // Publish execution.scheduled message
                self.publisher.publish_execution_scheduled(execution.id).await?;
            }
        }
        
        Ok(())
    }
}
```

### Message Flow

#### New Message Types

```rust
// Workflow-specific messages
pub enum WorkflowMessage {
    WorkflowStarted { execution_id: i64, workflow_id: i64 },
    WorkflowCompleted { execution_id: i64, result: JsonValue },
    WorkflowFailed { execution_id: i64, error: String },
    TaskScheduled { workflow_execution_id: i64, task_name: String },
    TaskCompleted { workflow_execution_id: i64, task_name: String, result: TaskResult },
    TaskFailed { workflow_execution_id: i64, task_name: String, error: String },
}
```

## API Endpoints

### Workflow Management

```
POST   /api/v1/packs/{pack_ref}/workflows          - Create workflow
GET    /api/v1/packs/{pack_ref}/workflows          - List workflows
GET    /api/v1/workflows/{workflow_ref}            - Get workflow
PUT    /api/v1/workflows/{workflow_ref}            - Update workflow
DELETE /api/v1/workflows/{workflow_ref}            - Delete workflow

POST   /api/v1/workflows/{workflow_ref}/execute    - Execute workflow directly
GET    /api/v1/workflows/{workflow_ref}/executions - List executions
```

### Workflow Execution Monitoring

```
GET    /api/v1/workflow-executions/{id}            - Get workflow execution
GET    /api/v1/workflow-executions/{id}/tasks      - List task executions
GET    /api/v1/workflow-executions/{id}/graph      - Get execution graph
POST   /api/v1/workflow-executions/{id}/pause      - Pause workflow
POST   /api/v1/workflow-executions/{id}/resume     - Resume workflow
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
    │   ├── action.yaml         # Action metadata
    │   ├── deploy.yaml         # Workflow action metadata
    │   └── workflows/
    │       └── deploy.workflow.yaml # Graph-only definition
    ├── sensors/
    │   ├── sensor1.py
    │   └── sensor.yaml
    ├── rules/
    │   └── on_push.yaml
    └── tests/
        └── test_workflows.yaml
```

### Workflow Pack Registration

When a pack is registered, the system:

1. Scans action metadata and resolves each `workflow_file` relative to `actions/`
2. Parses and validates each graph-only workflow definition
3. Creates `workflow_definition` record
4. Creates synthetic `action` record with `is_workflow=true`
5. Makes workflow invokable like any other action

## Implementation Plan

### Phase 1: Foundation (Week 1-2)

1. **Database Schema**
   - Create migration for workflow tables
   - Add workflow columns to action table
   - Create indexes

2. **Data Models**
   - Add workflow models to `common/src/models.rs`
   - Create workflow definition parser
   - Create repositories

3. **Template Engine**
   - Integrate `tera` crate
   - Implement variable context
   - Create helper functions

### Phase 2: Core Engine (Week 3-4)

4. **Graph Engine**
   - Implement task graph builder
   - Implement graph traversal logic
   - Handle conditional branching

5. **Workflow Executor**
   - Implement workflow execution logic
   - Handle task scheduling
   - Implement state management

6. **Message Integration**
   - Add workflow message handlers
   - Integrate with executor service
   - Handle task completion events

### Phase 3: Advanced Features (Week 5-6)

7. **Iteration Support**
   - Implement `with_items` logic
   - Add batching support
   - Handle concurrent iterations

8. **Parallel Execution**
   - Implement parallel task type
   - Handle synchronization
   - Aggregate results

9. **Error Handling**
   - Implement retry logic
   - Handle timeouts
   - Implement failure paths

### Phase 4: API & Tools (Week 7-8)

10. **API Endpoints**
    - Workflow CRUD operations
    - Execution monitoring
    - Control operations (pause/resume/cancel)

11. **CLI Support**
    - Workflow validation command
    - Workflow execution command
    - Workflow visualization

12. **Testing & Documentation**
    - Unit tests for all components
    - Integration tests
    - Example workflows
    - User documentation

## Testing Strategy

### Unit Tests

- Template rendering with all scope types
- Graph traversal algorithms
- Condition evaluation
- Variable publishing
- Task scheduling logic

### Integration Tests

- End-to-end workflow execution
- Parallel task execution
- Error handling and retry
- Workflow cancellation
- Nested workflow execution

### Example Test Workflows

```yaml
# tests/workflows/simple_sequence.workflow.yaml
version: "1.0.0"
tasks:
  - name: task1
    action: core.echo
    input:
      message: "Task 1"
    next:
      - when: "{{ succeeded() }}"
        do: [task2]
  - name: task2
    action: core.echo
    input:
      message: "Task 2"
```

## Performance Considerations

1. **Graph Optimization**: Cache parsed workflow graphs
2. **Template Caching**: Cache compiled templates per workflow
3. **Parallel Scheduling**: Schedule independent tasks concurrently
4. **Database Batching**: Batch task creation/updates
5. **Context Serialization**: Use efficient JSON serialization

## Security Considerations

1. **Template Injection**: Sanitize template inputs
2. **Variable Scoping**: Strict scope isolation between workflows
3. **Secret Access**: Expose `keystore.*` only to authorized workflow executions
4. **Resource Limits**: Enforce max task count, max depth, max iterations
5. **Audit Trail**: Log all workflow decisions and transitions

## Monitoring & Observability

### Metrics

- Workflow execution duration
- Task execution duration
- Workflow success/failure rate
- Task retry count
- Queue depth for workflow tasks

### Logging

```
INFO  Workflow execution started: id=123, workflow=deploy_app
INFO  Task scheduled: workflow=123, task=build_image
INFO  Task completed: workflow=123, task=build_image, duration=45s
INFO  Publishing variables: deployment_id=456
INFO  Scheduling next tasks: [deploy_containers, health_check]
INFO  Workflow execution completed: id=123, duration=2m30s
```

### Tracing

- Link all tasks to parent workflow execution
- Propagate trace_id through entire workflow
- Enable distributed tracing visualization

## Future Enhancements

1. **Workflow Versioning**: Support multiple versions of same workflow
2. **Workflow Templates**: Reusable workflow patterns
3. **Dynamic Workflows**: Generate workflow graph at runtime
4. **Workflow Marketplace**: Share workflows across organizations
5. **Workflow Testing Framework**: Built-in testing tools
6. **Workflow Debugger**: Step-through execution debugging
7. **Workflow Visualization**: Real-time execution graph UI
8. **Workflow Analytics**: Performance analysis and optimization suggestions

## References

- [StackStorm Orquesta](https://docs.stackstorm.com/orquesta/index.html)
- [Argo Workflows](https://argoproj.github.io/argo-workflows/)
- [AWS Step Functions](https://aws.amazon.com/step-functions/)
- [Temporal Workflows](https://temporal.io/)

## Appendix: Complete Workflow Example

`docs/examples/complete-workflow.yaml` is retained as a legacy standalone
example and should not be copied as the current authoring format.

For the canonical two-file native cache iteration example, see
`docs/examples/cache-iteration-workflow-action.yaml` and
`docs/examples/cache-iteration-workflow.workflow.yaml`.
