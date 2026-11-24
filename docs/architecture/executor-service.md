# Executor Service Architecture

## Overview

The **Executor Service** is the core orchestration engine of the Attune automation platform. It is responsible for processing rule enforcements, scheduling executions to workers, managing execution lifecycle, and orchestrating complex workflows.

## Service Architecture

The Executor is structured as a distributed microservice with three main processing components:

```
┌─────────────────────────────────────────────────────────────┐
│                     Executor Service                         │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌─────────────────────┐  ┌──────────────────────┐          │
│  │ Enforcement         │  │ Execution            │          │
│  │ Processor           │  │ Scheduler            │          │
│  └─────────────────────┘  └──────────────────────┘          │
│           │                         │                         │
│           │                         │                         │
│           v                         v                         │
│  ┌─────────────────────────────────────────────┐             │
│  │         Execution Manager                   │             │
│  └─────────────────────────────────────────────┘             │
│                                                               │
└─────────────────────────────────────────────────────────────┘
         │                    │                    │
         v                    v                    v
   PostgreSQL            RabbitMQ              Workers
```

## Core Components

### 1. Enforcement Processor

**Purpose**: Processes triggered rules and creates execution requests.

**Responsibilities**:
- Listens for `enforcement.created` messages from triggered rules
- Fetches enforcement, rule, and event data from the database
- Evaluates rule conditions and policies
- Creates execution records in the database
- Publishes `execution.requested` messages to the scheduler

**Message Flow**:
```
Rule Triggered → Enforcement Created → Enforcement Processor → Execution Created
```

**Key Implementation Details**:
- Uses `consume_with_handler` pattern for message consumption
- All processing methods are static to enable shared state across async handlers
- Validates rule is enabled before creating executions
- Links executions to enforcements for audit trail

### 2. Execution Scheduler

**Purpose**: Routes execution requests to available workers.

**Responsibilities**:
- Listens for `execution.requested` messages
- Determines runtime requirements for the action
- Selects appropriate workers based on:
  - Runtime compatibility
  - Worker status (active only)
  - Load balancing (future: capacity, affinity, locality)
- Updates execution status to `Scheduled`
- Publishes `execution.scheduled` messages to worker queues

**Message Flow**:
```
Execution Requested → Scheduler → Worker Selection → Execution Scheduled → Worker
```

**Worker Selection Algorithm**:
1. Fetch all available workers
2. Filter by runtime compatibility (if action specifies runtime)
3. Filter by worker status (only active workers)
4. Apply load balancing strategy (currently: first available)
5. Future: Consider capacity, affinity, geographic locality

**Key Implementation Details**:
- Supports multiple worker types (local, remote, container)
- Handles worker unavailability with error responses
- Plans for intelligent scheduling based on worker capabilities

### 3. Execution Manager

**Purpose**: Orchestrates execution workflows and handles lifecycle events.

**Responsibilities**:
- Listens for `execution.status.*` messages from workers
- **Does NOT update execution state** (worker owns state after scheduling)
- Handles execution completion orchestration (triggering child executions)
- Manages workflow executions (parent-child relationships)
- Coordinates workflow state transitions

**Ownership Model**:
- **Executor owns**: Requested → Scheduling → Scheduled (updates DB)
  - Includes pre-handoff cancellations/failures (before `execution.scheduled` is published)
- **Worker owns**: Running → Completed/Failed/Cancelled (updates DB)
  - Includes post-handoff cancellations/failures (after receiving `execution.scheduled`)
- **Handoff Point**: When `execution.scheduled` message is **published** to worker
  - Before publish: Executor owns and updates state
  - After publish: Worker owns and updates state

**Message Flow**:
```
Worker Status Update → Execution Manager → Orchestration Logic (Read-Only)
                                         → Trigger Child Executions
```

**Status Lifecycle**:
```
Requested → Scheduling → Scheduled → [HANDOFF: execution.scheduled published] → Running → Completed/Failed/Cancelled
    │                       │                                                     │
    └─ Executor Updates ───┘                                                     └─ Worker Updates
    │  (includes pre-handoff                                                     │  (includes post-handoff
    │   Cancelled)                                                               │   Cancelled/Timeout/Abandoned)
                                                                                  │
                                                                                  └→ Child Executions (workflows)
```

**Key Implementation Details**:
- Receives status change notifications for orchestration purposes only
- Does not update execution state after handoff to worker
- Handles workflow orchestration (parent-child execution chaining)
- Only triggers child executions on successful parent completion
- Read-only access to execution records for orchestration logic

## Message Queue Integration

### Message Types

The Executor consumes and produces several message types:

**Consumed**:
- `enforcement.created` - New enforcement from triggered rules
- `execution.requested` - Execution scheduling requests
- `execution.status.changed` - Status change notifications from workers (for orchestration)
- `execution.completed` - Completion notifications from workers (for queue management)

**Published**:
- `execution.requested` - To scheduler (from enforcement processor)
- `execution.scheduled` - To workers (from scheduler) **← OWNERSHIP HANDOFF**

**Note**: The executor does NOT publish `execution.completed` messages. This is the worker's responsibility as the authoritative source of execution state after scheduling.

### Message Envelope Structure

All messages use the standardized `MessageEnvelope<T>` structure:

```rust
MessageEnvelope {
    message_id: Uuid,
    message_type: MessageType,
    source: String,
    timestamp: DateTime<Utc>,
    correlation_id: Option<Uuid>,
    trace_id: Option<String>,
    payload: T,
    retry_count: u32,
}
```

### Consumer Handler Pattern

All processors use the `consume_with_handler` pattern for robust message consumption:

```rust
consumer.consume_with_handler(move |envelope: MessageEnvelope<PayloadType>| {
    // Clone shared state
    let pool = pool.clone();
    let publisher = publisher.clone();
    
    async move {
        // Process message
        Self::process_message(&pool, &publisher, &envelope).await
            .map_err(|e| format!("Error: {}", e).into())
    }
}).await?;
```

**Benefits**:
- Automatic message acknowledgment on success
- Automatic nack with requeue on retriable errors
- Automatic dead letter queue routing on non-retriable errors
- Built-in error handling and logging

## Database Integration

### Repository Pattern

All database access uses the repository layer:

```rust
use attune_common::repositories::{
    enforcement::EnforcementRepository,
    execution::ExecutionRepository,
    rule::RuleRepository,
    Create, FindById, Update, List,
};
```

### Database Update Ownership

**Executor updates execution state** from creation through handoff:
- Creates execution records (`Requested` status)
- Updates status during scheduling (`Scheduling` → `Scheduled`)
- Publishes `execution.scheduled` message to worker **← HANDOFF POINT**
- **Handles cancellations/failures BEFORE handoff** (before message is published)
  - Example: User cancels execution while queued by concurrency policy
  - Executor updates to `Cancelled`, worker never receives message

**Worker updates execution state** after receiving handoff:
- Receives `execution.scheduled` message (takes ownership)
- Updates status when execution starts (`Running`)
- Updates status when execution completes (`Completed`, `Failed`, etc.)
- **Handles cancellations/failures AFTER handoff** (after receiving message)
- Updates result data and artifacts
- Worker only owns executions it has received

**Executor reads execution state** for orchestration after handoff:
- Receives status change notifications from workers
- Reads execution records to trigger workflow children
- Does NOT update execution state after publishing `execution.scheduled`

### Transaction Support

Future implementations will use database transactions for multi-step operations:
- Creating execution + publishing message (atomic)
- Enforcement processing + execution creation (atomic)

## Configuration

The Executor service uses the standard Attune configuration system:

```yaml
# config.yaml
database:
  url: postgresql://localhost/attune
  max_connections: 20
  
message_queue:
  url: amqp://localhost
  exchange: attune.executions
  prefetch_count: 10
```

Environment variable overrides:
```bash
ATTUNE__DATABASE__URL=postgresql://prod-db/attune
ATTUNE__MESSAGE_QUEUE__URL=amqp://prod-mq
```

Completion processing defaults to one handler per executor. Set
`executor.completion_concurrency` or `ATTUNE__EXECUTOR__COMPLETION_CONCURRENCY`
to a positive integer to overlap independent workflow completions. The executor
caps the setting at four and budgets two database connections per handler plus
two for routing and other consumers. A small pool uses a single handler.
Deliveries for a workflow stay in order within each consumer. A transient
classification failure drains earlier work before processing that delivery on
its own. Durable receipts and workflow locks coordinate processing across
replicas.

Measure broker backlog, completion latency, and database lock waits together.
Concurrent handlers can spend the broker time they save waiting for shared
cache admission in PostgreSQL.

## Error Handling

### Error Types

The Executor handles several error categories:

1. **Database Errors**: Connection issues, query failures
2. **Message Queue Errors**: Connection drops, serialization failures
3. **Business Logic Errors**: Missing entities, invalid states
4. **Worker Errors**: No workers available, incompatible runtimes

### Retry Strategy

- **Retriable Errors**: Requeued for retry (connection issues, timeouts)
- **Non-Retriable Errors**: Sent to dead letter queue (invalid data, missing entities)
- **Retry Limits**: Configured per queue (future implementation)

### Dead Letter Queues

Failed messages are automatically routed to dead letter queues for investigation:
- `executor.enforcement.created.dlq`
- `executor.execution.requested.dlq`
- `executor.execution.status.dlq`

## Workflow Orchestration

### Parent-Child Executions

The Executor supports complex workflows through parent-child execution relationships:

```
Parent Execution (Completed)
  ├── Child Execution 1 (action_ref: "pack.action1")
  ├── Child Execution 2 (action_ref: "pack.action2")
  └── Child Execution 3 (action_ref: "pack.action3")
```

**Implementation**:
- Parent execution stores child action references
- On parent completion, Execution Manager creates child executions
- Child executions inherit parent's configuration
- Each child is independently scheduled and executed

### Future Enhancements

- **Conditional Workflows**: Execute children based on parent result
- **Parallel vs Sequential**: Control execution order
- **Workflow DAGs**: Complex dependency graphs
- **Workflow Templates**: Reusable workflow definitions

## Policy Enforcement

### Planned Features

1. **Rate Limiting**: Limit executions per time window
2. **Concurrency Control**: Maximum concurrent executions per action/pack
3. **Priority Queuing**: High-priority executions jump the queue
4. **Resource Quotas**: Limit resource consumption per tenant
5. **Execution Windows**: Only execute during specified time periods

### Implementation Location

Policy enforcement will be implemented in:
- Enforcement Processor (pre-execution validation)
- Scheduler (runtime constraint checking)
- New `PolicyEnforcer` module (future)

## Monitoring & Observability

### Metrics (Future)

- Executions per second (throughput)
- Average execution duration
- Queue depth and processing lag
- Worker utilization
- Error rates by type

### Logging

Structured logging at multiple levels:
- `INFO`: Successful operations, state transitions
- `WARN`: Degraded states, retry attempts
- `ERROR`: Failures requiring attention
- `DEBUG`: Detailed flow for troubleshooting

Example:
```
INFO Processing enforcement: 123
INFO Selected worker 45 for execution 789
INFO Execution 789 scheduled to worker 45
```

### Tracing

Message correlation and distributed tracing:
- `correlation_id`: Links related messages
- `trace_id`: End-to-end request tracing (future integration with OpenTelemetry)

## Running the Service

### Prerequisites

- PostgreSQL 14+ with schema initialized
- RabbitMQ 3.12+ with exchanges and queues configured
- Environment variables or config file set up

### Startup

```bash
# Using cargo
cd crates/executor
cargo run

# Or with environment overrides
ATTUNE__DATABASE__URL=postgresql://localhost/attune \
ATTUNE__MESSAGE_QUEUE__URL=amqp://localhost \
cargo run
```

### Graceful Shutdown

On SIGTERM or SIGINT, the executor stops admitting completions and gives
active completion handlers up to ten seconds with the broker and database
available. It then stops the remaining processors, lets the workflow log
dispatcher shut down, and closes the broker and database connections. RabbitMQ
can redeliver work whose acknowledgement was interrupted.

## Testing

### Unit Tests

Each module includes unit tests for business logic:
- Rule evaluation
- Worker selection algorithms
- Status parsing
- Workflow creation

### Integration Tests

Integration tests require PostgreSQL and RabbitMQ:
- End-to-end enforcement → execution flow
- Message queue reliability
- Database consistency

### Running Tests

```bash
# Unit tests only
cargo test -p attune-executor --lib

# Integration tests (requires services)
cargo test -p attune-executor --test '*'
```

## Future Enhancements

### Phase 1: Core Functionality (Current)
- ✅ Enforcement processing
- ✅ Execution scheduling
- ✅ Lifecycle management
- ✅ Message queue integration

### Phase 2: Advanced Features (Next)
- Policy enforcement (rate limiting, concurrency)
- Advanced workflow orchestration
- Inquiry handling (human-in-the-loop)
- Retry and failure handling improvements

### Phase 3: Production Readiness
- Comprehensive monitoring and metrics
- Performance optimization
- High availability setup
- Load testing and tuning

### Phase 4: Enterprise Features
- Multi-tenancy isolation
- Advanced scheduling algorithms
- Resource quotas and limits
- Audit logging and compliance

## Troubleshooting

### Common Issues

**Problem**: Executions stuck in "Requested" status
- **Cause**: Scheduler not running or no workers available
- **Solution**: Verify scheduler is running, check worker status

**Problem**: Messages not being consumed
- **Cause**: RabbitMQ connection issues or queue misconfiguration
- **Solution**: Check MQ connection, verify queue bindings

**Problem**: Database connection errors
- **Cause**: Connection pool exhausted or database down
- **Solution**: Increase pool size, check database health

### Debug Mode

Enable detailed logging:
```bash
RUST_LOG=attune_executor=debug,attune_common=debug cargo run
```

## Related Documentation

- [API - Executions](./api-executions.md)
- [API - Events & Enforcements](./api-events-enforcements.md)
- [API - Rules](./api-rules.md)
- [Configuration](./configuration.md)
- [Quick Start](./quick-start.md)
