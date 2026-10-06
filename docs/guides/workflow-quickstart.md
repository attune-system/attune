# Workflow Orchestration Quick-Start Guide

This guide helps developers get started implementing the workflow orchestration feature in Attune.

## Overview

Workflows are composable YAML-based action graphs that enable complex automation. This implementation adds workflow capabilities to Attune over 5 phases spanning 9 weeks.

## Before You Start

### Required Reading
1. `docs/workflow-orchestration.md` - Full technical design (1,063 lines)
2. `docs/workflow-implementation-plan.md` - Implementation roadmap (562 lines)
3. `docs/workflow-summary.md` - Quick reference (400 lines)

### Required Knowledge
- Rust async programming (tokio)
- PostgreSQL and SQLx
- RabbitMQ message patterns
- Graph algorithms (basic traversal)
- Template engines (Jinja2-style syntax)

### Development Environment
```bash
# Ensure you have:
- Rust 1.70+
- Stock PostgreSQL 16+, no TimescaleDB extension required
- RabbitMQ 3.12+
- Docker (for testing)

# Clone and setup
cd attune
cargo build
```

## Implementation Phases

### Phase 1: Foundation (Weeks 1-2)

**Goal**: Database schema, models, parser, template engine

#### Step 1.1: Database Migration
```bash
# Create migration file
cd migrations
touch 020_workflow_orchestration.sql
```

Copy content from `docs/examples/workflow-migration.sql`:
- 3 new tables: `workflow_definition`, `workflow_execution`, `workflow_task_execution`
- Modify `action` table with `is_workflow` and `workflow_def` columns
- Add indexes, triggers, views

Run migration:
```bash
sqlx migrate run
```

#### Step 1.2: Data Models
Add to `crates/common/src/models.rs`:

```rust
pub mod workflow {
    use super::*;
    
    #[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
    pub struct WorkflowDefinition {
        pub id: Id,
        pub r#ref: String,
        pub pack: Id,
        pub pack_ref: String,
        pub label: String,
        pub description: Option<String>,
        pub version: String,
        pub param_schema: Option<JsonSchema>,
        pub out_schema: Option<JsonSchema>,
        pub definition: JsonValue,  // Full workflow YAML as JSON
        pub tags: Vec<String>,
        pub enabled: bool,
        pub created: DateTime<Utc>,
        pub updated: DateTime<Utc>,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
    pub struct WorkflowExecution {
        pub id: Id,
        pub execution: Id,
        pub workflow_def: Id,
        pub current_tasks: Vec<String>,
        pub completed_tasks: Vec<String>,
        pub failed_tasks: Vec<String>,
        pub skipped_tasks: Vec<String>,
        pub variables: JsonValue,
        pub task_graph: JsonValue,
        pub status: ExecutionStatus,
        pub error_message: Option<String>,
        pub paused: bool,
        pub pause_reason: Option<String>,
        pub created: DateTime<Utc>,
        pub updated: DateTime<Utc>,
    }
    
    #[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
    pub struct WorkflowTaskExecution {
        pub id: Id,
        pub workflow_execution: Id,
        pub execution: Id,
        pub task_name: String,
        pub task_index: Option<i32>,
        pub task_batch: Option<i32>,
        pub status: ExecutionStatus,
        pub started_at: Option<DateTime<Utc>>,
        pub completed_at: Option<DateTime<Utc>>,
        pub duration_ms: Option<i64>,
        pub result: Option<JsonValue>,
        pub error: Option<JsonValue>,
        pub retry_count: i32,
        pub max_retries: i32,
        pub next_retry_at: Option<DateTime<Utc>>,
        pub timeout_seconds: Option<i32>,
        pub timed_out: bool,
        pub created: DateTime<Utc>,
        pub updated: DateTime<Utc>,
    }
}
```

#### Step 1.3: Repositories
Create `crates/common/src/repositories/workflow_definition.rs`:

```rust
use sqlx::PgPool;
use crate::models::workflow::WorkflowDefinition;
use crate::error::Result;

pub struct WorkflowDefinitionRepository;

impl WorkflowDefinitionRepository {
    pub async fn create(pool: &PgPool, def: &WorkflowDefinition) -> Result<WorkflowDefinition> {
        // INSERT implementation
    }
    
    pub async fn find_by_ref(pool: &PgPool, ref_: &str) -> Result<Option<WorkflowDefinition>> {
        // SELECT WHERE ref = ?
    }
    
    pub async fn list_by_pack(pool: &PgPool, pack_id: i64) -> Result<Vec<WorkflowDefinition>> {
        // SELECT WHERE pack = ?
    }
    
    // ... other CRUD methods
}
```

Create similar repositories for `workflow_execution` and `workflow_task_execution`.

#### Step 1.4: YAML Parser
Create `crates/executor/src/workflow/parser.rs`:

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowSpec {
    pub r#ref: String,
    pub label: String,
    pub description: Option<String>,
    pub version: String,
    pub parameters: Option<serde_json::Value>,
    pub output: Option<serde_json::Value>,
    pub vars: HashMap<String, serde_json::Value>,
    pub tasks: Vec<TaskSpec>,
    pub output_map: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    pub name: String,
    #[serde(rename = "type")]
    pub task_type: Option<TaskType>,
    pub action: Option<String>,
    pub input: HashMap<String, String>,
    pub publish: Option<Vec<String>>,
    pub on_success: Option<String>,
    pub on_failure: Option<String>,
    pub on_complete: Option<String>,
    pub on_timeout: Option<String>,
    pub decision: Option<Vec<DecisionBranch>>,
    pub when: Option<String>,
    pub with_items: Option<String>,
    pub batch_size: Option<usize>,
    pub retry: Option<RetryPolicy>,
    pub timeout: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Action,
    Parallel,
    Workflow,
}

pub fn parse_workflow_yaml(yaml: &str) -> Result<WorkflowSpec> {
    serde_yaml::from_str(yaml)
        .map_err(|e| Error::InvalidWorkflowDefinition(e.to_string()))
}
```

#### Step 1.5: Template Engine
Add to `crates/executor/Cargo.toml`:
```toml
[dependencies]
tera = "1.19"
```

Create `crates/executor/src/workflow/context.rs`:

```rust
use tera::{Tera, Context};
use std::collections::HashMap;
use serde_json::Value as JsonValue;

pub struct WorkflowContext {
    pub execution_id: i64,
    pub parameters: JsonValue,
    pub vars: HashMap<String, JsonValue>,
    pub task_results: HashMap<String, TaskResult>,
    pub pack_config: JsonValue,
    pub system: SystemContext,
}

impl WorkflowContext {
    pub fn new(execution_id: i64, parameters: JsonValue) -> Self {
        Self {
            execution_id,
            parameters,
            vars: HashMap::new(),
            task_results: HashMap::new(),
            pack_config: JsonValue::Null,
            system: SystemContext::default(),
        }
    }
    
    pub fn render_template(&self, template: &str) -> Result<String> {
        let mut tera = Tera::default();
        let context = self.to_tera_context();
        tera.render_str(template, &context)
            .map_err(|e| Error::TemplateError(e.to_string()))
    }
    
    fn to_tera_context(&self) -> Context {
        let mut ctx = Context::new();
        ctx.insert("parameters", &self.parameters);
        ctx.insert("vars", &self.vars);
        ctx.insert("task", &self.task_results);
        ctx.insert("system", &self.system);
        ctx.insert("pack", &json!({"config": self.pack_config}));
        ctx
    }
}
```

**Phase 1 Testing**:
```bash
cargo test -p attune-common workflow
cargo test -p attune-executor workflow::parser
```

---

### Phase 2: Execution Engine (Weeks 3-4)

**Goal**: Graph builder, workflow executor, message handlers

#### Step 2.1: Task Graph
Create `crates/executor/src/workflow/graph.rs`:

```rust
use std::collections::HashMap;

pub struct TaskGraph {
    nodes: HashMap<String, TaskNode>,
    edges: HashMap<String, Vec<Edge>>,
}

impl TaskGraph {
    pub fn from_workflow_spec(spec: &WorkflowSpec) -> Result<Self> {
        // Build graph from task definitions
        // Create nodes for each task
        // Create edges based on transitions (on_success, on_failure, etc.)
    }
    
    pub fn get_entry_tasks(&self) -> Vec<&TaskNode> {
        // Return tasks with no incoming edges
    }
    
    pub fn get_next_tasks(&self, completed_task: &str, result: &TaskResult) -> Vec<&TaskNode> {
        // Follow edges based on result (success/failure)
    }
}
```

#### Step 2.2: Workflow Executor
Create `crates/executor/src/workflow/executor.rs`:

```rust
pub struct WorkflowExecutor {
    pool: PgPool,
    publisher: MessagePublisher,
}

impl WorkflowExecutor {
    pub async fn execute_workflow(
        &self,
        execution_id: i64,
        workflow_ref: &str,
        parameters: JsonValue,
    ) -> Result<()> {
        // 1. Load workflow definition
        // 2. Create workflow_execution record
        // 3. Initialize context
        // 4. Build task graph
        // 5. Schedule initial tasks
    }
    
    pub async fn handle_task_completion(
        &self,
        workflow_execution_id: i64,
        task_name: String,
        result: TaskResult,
    ) -> Result<()> {
        // 1. Update workflow_task_execution
        // 2. Publish variables
        // 3. Determine next tasks
        // 4. Schedule next tasks
        // 5. Check if workflow complete
    }
}
```

#### Step 2.3: Message Handlers
Integrate with existing executor message loops:

```rust
// In executor/src/main.rs
async fn start_workflow_message_handlers(
    pool: PgPool,
    publisher: MessagePublisher,
) -> Result<()> {
    let executor = WorkflowExecutor::new(pool.clone(), publisher.clone());
    
    // Listen for execution.completed on workflow tasks
    let consumer = create_consumer("workflow.task.completions").await?;
    
    consumer.consume_with_handler(move |envelope| {
        let executor = executor.clone();
        async move {
            executor.handle_task_completion(
                envelope.payload.workflow_execution_id,
                envelope.payload.task_name,
                envelope.payload.result,
            ).await
        }
    }).await?;
    
    Ok(())
}
```

**Phase 2 Testing**:
```bash
cargo test -p attune-executor workflow::graph
cargo test -p attune-executor workflow::executor
```

---

### Phase 3: Advanced Features (Weeks 5-6)

**Goal**: Iteration, parallelism, retry, conditionals

#### Step 3.1: Iteration
Create `crates/executor/src/workflow/iterator.rs`:

```rust
pub struct TaskIterator {
    items: Vec<JsonValue>,
    batch_size: Option<usize>,
}

impl TaskIterator {
    pub fn from_template(
        template: &str,
        context: &WorkflowContext,
        batch_size: Option<usize>,
    ) -> Result<Self> {
        let rendered = context.render_template(template)?;
        let items: Vec<JsonValue> = serde_json::from_str(&rendered)?;
        Ok(Self { items, batch_size })
    }
    
    pub fn batches(&self) -> Vec<Vec<&JsonValue>> {
        if let Some(size) = self.batch_size {
            self.items.chunks(size).map(|c| c.iter().collect()).collect()
        } else {
            vec![self.items.iter().collect()]
        }
    }
}
```

#### Step 3.2: Parallel Execution
Create `crates/executor/src/workflow/parallel.rs`:

```rust
pub struct ParallelExecutor {
    // Execute multiple tasks simultaneously
    // Wait for all to complete
    // Aggregate results
}
```

#### Step 3.3: Retry Logic
Create `crates/executor/src/workflow/retry.rs`:

```rust
pub struct RetryHandler {
    // Exponential/linear/constant backoff
    // Max retries
    // Condition evaluation
}
```

---

### Phase 4: API & Tools (Weeks 7-8)

**Goal**: REST endpoints, validation, pack integration

#### Step 4.1: API Routes
Create `crates/api/src/routes/workflows.rs`:

```rust
pub fn workflow_routes() -> Router {
    Router::new()
        .route("/packs/:pack_ref/workflows", post(create_workflow))
        .route("/packs/:pack_ref/workflows", get(list_workflows))
        .route("/workflows/:workflow_ref", get(get_workflow))
        .route("/workflows/:workflow_ref", put(update_workflow))
        .route("/workflows/:workflow_ref", delete(delete_workflow))
        .route("/workflows/:workflow_ref/execute", post(execute_workflow))
}
```

#### Step 4.2: Pack Integration
Update pack registration to scan `workflows/` directory:

```rust
// In pack registration logic
async fn register_workflows_in_pack(pool: &PgPool, pack_id: i64, pack_path: &Path) -> Result<()> {
    let workflows_dir = pack_path.join("workflows");
    if !workflows_dir.exists() {
        return Ok(());
    }
    
    for entry in std::fs::read_dir(workflows_dir)? {
        let path = entry?.path();
        if path.extension() == Some("yaml".as_ref()) {
            let yaml = std::fs::read_to_string(&path)?;
            let spec = parse_workflow_yaml(&yaml)?;
            
            // Create workflow_definition
            // Create synthetic action with is_workflow=true
        }
    }
    
    Ok(())
}
```

---

### Phase 5: Testing & Documentation (Week 9)

**Goal**: Comprehensive tests and documentation

#### Integration Tests
Create `crates/executor/tests/workflow_integration.rs`:

```rust
#[tokio::test]
async fn test_simple_sequential_workflow() {
    // Test basic workflow execution
}

#[tokio::test]
async fn test_parallel_execution() {
    // Test parallel tasks
}

#[tokio::test]
async fn test_conditional_branching() {
    // Test decision trees
}

#[tokio::test]
async fn test_iteration_with_batching() {
    // Test with-items
}
```

---

## Development Tips

### Debugging Workflows
```bash
# Enable debug logging
RUST_LOG=attune_executor::workflow=debug cargo run

# Watch workflow execution
psql -d attune -c "SELECT * FROM attune.workflow_execution_summary;"

# Check task status
psql -d attune -c "SELECT * FROM attune.workflow_task_detail WHERE workflow_execution = ?;"
```

### Testing YAML Parsing
```bash
# Validate workflow YAML
cargo run --bin attune-cli -- workflow validate path/to/workflow.yaml
```

### Common Pitfalls
1. **Circular Dependencies**: Validate graph for cycles
2. **Template Errors**: Always handle template rendering failures
3. **Variable Scope**: Test all 6 scopes independently
4. **Message Ordering**: Ensure task completions processed in order
5. **Resource Limits**: Enforce max tasks/depth/iterations

---

## Resources

- **Design Docs**: `docs/workflow-*.md`
- **Examples**: `docs/examples/simple-workflow.yaml`, `complete-workflow.yaml`
- **Migration**: `docs/examples/workflow-migration.sql`
- **TODO Tasks**: `work-summary/TODO.md` Phase 8.1

---

## Getting Help

- Review full design: `docs/workflow-orchestration.md`
- Check implementation plan: `docs/workflow-implementation-plan.md`
- See examples: `docs/examples/`
- Ask questions in project discussions

---

**Ready to start? Begin with Phase 1, Step 1.1: Database Migration**
