# Internal RabbitMQ message queue reference

Attune uses RabbitMQ for asynchronous communication between the API, executor, worker, sensor, and supervisor services. This page catalogs the current exchanges, queues, bindings, producers, consumers, payloads, and delivery behavior.

This reference describes the broker topology created by the Rust services. It does not describe the database-backed Attune `WorkQueue` feature, except where that feature publishes an execution request to RabbitMQ.

## Topology at a glance

Attune has three kinds of RabbitMQ queues:

- Fixed durable queues have configured names such as `attune.execution.requests.queue`.
- Per-worker durable queues use a worker database ID, such as `worker.42.executions`.
- Broker-named ephemeral queues use names such as `amq.gen-...`. Each service replica creates its own queue for cache invalidation broadcasts.

The active topology contains five exchanges, ten fixed queues, four per-worker queue families, and three ephemeral queue families.

Source: [RabbitMQ configuration](../../crates/common/src/mq/config.rs), [topology setup](../../crates/common/src/mq/connection.rs#L163-L741).

## Exchanges

| Exchange | Type | Durable | Intent |
| --- | --- | --- | --- |
| `attune.events` | topic | yes | Event ingress, rule lifecycle, and pack lifecycle |
| `attune.executions` | topic | yes | Enforcement, scheduling, worker dispatch, completion, inquiry, cancellation, and pack testing |
| `attune.metadata` | topic | yes | Replica-local cache invalidation |
| `attune.notifications` | fanout | yes | Reserved AMQP notification path; no active publisher or consumer |
| `attune.dlx` | direct | yes | Dead-lettered messages from durable application queues |

All exchanges are non-auto-delete. Each service calls the common setup before declaring the queues it consumes.

Source: [exchange defaults](../../crates/common/src/mq/config.rs#L331-L419), [common infrastructure](../../crates/common/src/mq/connection.rs#L437-L483).

## Common wire envelope

Application publishers serialize messages as JSON in `MessageEnvelope<T>`:

```typescript
interface MessageEnvelope<T> {
  message_id: string;
  correlation_id: string;
  message_type: MessageType;
  version: string;
  timestamp: string;
  headers: {
    retry_count: number;
    source_service?: string;
    trace_id?: string;
    [name: string]: unknown;
  };
  payload: T;
}
```

Example:

```json
{
  "message_id": "550e8400-e29b-41d4-a716-446655440000",
  "correlation_id": "550e8400-e29b-41d4-a716-446655440000",
  "message_type": "ExecutionRequested",
  "version": "1.0",
  "timestamp": "2026-09-03T12:34:56.789Z",
  "headers": {
    "retry_count": 0,
    "source_service": "api-service"
  },
  "payload": {
    "execution_id": 12001,
    "action_id": 44,
    "action_ref": "core.echo",
    "parent_id": null,
    "enforcement_id": null,
    "config": {"message": "hello"}
  }
}
```

`message_type` uses the exact PascalCase Rust enum name. Values are case-sensitive. `MessageEnvelope::new()` sets `version` to `1.0` and initially uses `message_id` as `correlation_id`.

Consumers do not enforce `version`. Compatibility currently comes from Serde behavior:

- Missing or null envelope UUIDs get new UUIDs.
- A missing `version` becomes `1.0`.
- Missing `headers` become empty headers with `retry_count: 0`.
- Unknown envelope and payload fields are ignored.
- Missing required payload fields fail deserialization.
- Most optional payload fields serialize as explicit JSON null values.

Publishers set persistent delivery mode, `application/json`, AMQP message and correlation IDs, and a Unix timestamp. Publisher confirms are enabled. `publish_envelope()` chooses the exchange and routing key from `message_type`; targeted worker routes override both explicitly.

Sources: [`MessageType` routing](../../crates/common/src/mq/messages.rs#L43-L175), [`MessageEnvelope`](../../crates/common/src/mq/messages.rs#L177-L325), [publisher properties and routing](../../crates/common/src/mq/publisher.rs#L83-L175).

## Consumer and retry behavior

All active consumers manually acknowledge messages.

- A successful handler causes `basic_ack`.
- Invalid JSON or a typed deserialization failure causes `basic_nack` with `requeue: false`.
- A handler error causes `basic_nack`. Connection, channel, publish, timeout, pool, and Lapin errors requeue immediately. Other error classes do not requeue.
- There is no delayed retry exchange, retry queue, or enforced retry limit.
- The envelope's `headers.retry_count` is not incremented by broker requeue behavior.
- Durable consumers recreate their channel and restore QoS after a recoverable connection failure.

Most fixed executor consumers use prefetch `10`. Pack tests use prefetch `5` on the executor and `1` on a worker. Ephemeral metadata consumers use prefetch `32`.

Consumers generally trust the queue binding and payload shape. Most do not verify that `message_type` agrees with the routing key or concrete payload type.

Sources: [consumer loop](../../crates/common/src/mq/consumer.rs#L182-L291), [retriable error classes](../../crates/common/src/mq/error.rs#L100-L112).

## Fixed queue catalog

| Queue | Exchange and binding | Consumer | Consumer tag | Prefetch | Intent |
| --- | --- | --- | --- | --- | --- |
| `attune.executor.events.queue` | `attune.events` / `event.created` | Executor `EventProcessor` | `executor.event` | 10 | Turn persisted events into enforcements |
| `attune.enforcements.queue` | `attune.executions` / `enforcement.#` | Executor `EnforcementProcessor` | `executor.enforcement` | 10 | Accept enforcements and create execution requests |
| `attune.execution.requests.queue` | `attune.executions` / `execution.requested` | Executor `ExecutionScheduler` | `executor.scheduler` | 10 | Load requested executions and select workers |
| `attune.execution.status.queue` | `attune.executions` / `execution.status.changed` | Executor `ExecutionManager` | `executor.manager` | 10 | Apply worker or API lifecycle updates |
| `attune.execution.completed.queue` | `attune.executions` / `execution.completed` | Executor `CompletionListener` | `executor.completion` | 10 | Advance workflows and release queue accounting |
| `attune.inquiry.responses.queue` | `attune.executions` / `inquiry.responded` | Executor `InquiryHandler` | `executor.inquiry` | 10 | Resume or finish executions waiting for input |
| `attune.pack.tests.queue` | `attune.executions` / `pack.test.requested` | Executor `PackTestProcessor` | `executor.packtest` | 5 | Select a worker for a pack test |
| `attune.rules.lifecycle.queue` | `attune.events` / lifecycle keys | Sensor `RuleLifecycleListener` | `sensor-rule-lifecycle` | 10 | Start, update, or stop managed sensor processes |
| `attune.events.queue` | `attune.events` / `#` | None | none | none | Declared sensor catch-all; currently accumulates messages |
| `attune.dlx.queue` | `attune.dlx` / `#` | Executor `DeadLetterHandler` | `executor.dlq` | 10 | Mark expired scheduled executions failed; current binding is ineffective |

The configured fixed queues are durable, non-exclusive, and non-auto-delete. Executor queues and `attune.events.queue` declare `x-dead-letter-exchange: attune.dlx` when dead lettering is enabled. `attune.rules.lifecycle.queue` is declared directly without a dead-letter exchange.

Sources: [fixed queue defaults](../../crates/common/src/mq/config.rs#L191-L329), [executor bindings](../../crates/common/src/mq/connection.rs#L486-L569), [sensor catch-all](../../crates/common/src/mq/connection.rs#L694-L717), [sensor lifecycle queue](../../crates/sensor/src/rule_lifecycle_listener.rs#L55-L157).

## Event ingress queue

Route:

```text
attune.events / event.created
  -> attune.executor.events.queue
  -> executor EventProcessor
```

Producers include the event API, webhook ingress, system alerts, repeated sensor failure alerts, and the database-backed work queue lifecycle bridge.

Payload:

```typescript
interface EventCreatedPayload {
  event_id: number;
  trigger_id: number | null;
  trigger_ref: string;
  sensor_id: number | null;
  sensor_ref: string | null;
  payload: unknown;
  config: unknown | null;
}
```

The envelope uses `message_type: "EventCreated"`. The consumer treats `event_id` as authoritative and reloads the event from PostgreSQL. It does not use the embedded event data as current state.

Producers: [event API](../../crates/api/src/routes/events.rs#L392-L407), [webhooks](../../crates/api/src/routes/webhooks.rs#L802-L821), [system alerts](../../crates/common/src/system_alert.rs#L73-L85), [work queue lifecycle bridge](../../crates/executor/src/work_queue_events.rs#L192-L219).

Consumer: [`EventProcessor`](../../crates/executor/src/event_processor.rs#L98-L164). Payload: [`EventCreatedPayload`](../../crates/common/src/mq/messages.rs#L331-L348).

## Enforcement queue

Route:

```text
attune.executions / enforcement.created
  -> attune.enforcements.queue through enforcement.#
  -> executor EnforcementProcessor
```

The executor event processor is the producer.

```typescript
interface EnforcementCreatedPayload {
  enforcement_id: number;
  rule_id: number | null;
  rule_ref: string;
  event_id: number | null;
  trigger_ref: string;
  payload: unknown;
}
```

The envelope uses `message_type: "EnforcementCreated"`. The consumer reloads the enforcement by `enforcement_id`; it does not compare the other payload values with the row.

Producer: [event processor](../../crates/executor/src/event_processor.rs#L416-L430). Consumer: [`EnforcementProcessor`](../../crates/executor/src/enforcement_processor.rs#L65-L101). Payload: [`EnforcementCreatedPayload`](../../crates/common/src/mq/messages.rs#L350-L365).

## Execution request queue

Route:

```text
attune.executions / execution.requested
  -> attune.execution.requests.queue
  -> executor ExecutionScheduler
```

This queue is the central scheduling input. Producers are:

- API manual execution and rescheduling
- Rule enforcement processing
- Workflow child creation, retry, iteration, and successor handling
- Admission or policy queue promotion
- Database-backed work queue dispatch
- Supervisor recovery

```typescript
interface ExecutionRequestedPayload {
  execution_id: number;
  action_id: number | null;
  action_ref: string;
  parent_id: number | null;
  enforcement_id: number | null;
  config: unknown | null;
}
```

The envelope uses `message_type: "ExecutionRequested"`. The scheduler trusts `execution_id` and reloads the execution. Persisted status makes stale or duplicate messages safe to ignore. The other fields remain required or nullable according to the type even though the scheduler gets current data from PostgreSQL.

Producer sources: [API execution routes](../../crates/api/src/routes/executions.rs#L320-L345), [enforcement processor](../../crates/executor/src/enforcement_processor.rs#L432-L454), [work queue dispatcher](../../crates/executor/src/queue_dispatcher.rs#L1161-L1178), [workflow scheduler](../../crates/executor/src/scheduler.rs), [completion listener](../../crates/executor/src/completion_listener.rs#L955-L967), [execution manager](../../crates/executor/src/execution_manager.rs#L404-L462), [supervisor](../../crates/supervisor/src/main.rs#L954-L1046).

Consumer: [scheduler input](../../crates/executor/src/scheduler.rs#L790-L879). Payload: [`ExecutionRequestedPayload`](../../crates/common/src/mq/messages.rs#L367-L382).

## Execution status queue

Route:

```text
attune.executions / execution.status.changed
  -> attune.execution.status.queue
  -> executor ExecutionManager
```

Workers produce normal lifecycle changes. The API also publishes changes caused by cancellation requests.

```typescript
interface ExecutionStatusChangedPayload {
  execution_id: number;
  action_ref: string;
  previous_status: string;
  new_status: string;
  changed_at: string;
}
```

The envelope uses `message_type: "ExecutionStatusChanged"`. The consumer parses `new_status` case-insensitively. It accepts `requested`, `scheduling`, `scheduled`, `running`, `completed`, `failed`, `canceling`, `cancelled`, `canceled`, `timeout`, and `abandoned`.

Producers: [worker status publisher](../../crates/worker/src/service.rs#L1762-L1808), [API execution status publisher](../../crates/api/src/routes/executions.rs#L1610-L1645). Consumer: [`ExecutionManager`](../../crates/executor/src/execution_manager.rs#L58-L103). Payload: [`ExecutionStatusChangedPayload`](../../crates/common/src/mq/messages.rs#L384-L397).

## Execution completion queue

Route:

```text
attune.executions / execution.completed
  -> attune.execution.completed.queue
  -> executor CompletionListener
```

Workers produce terminal action results. The executor and supervisor also produce synthetic completions for workflow completion, unschedulable work, policy cancellation, inquiry response or timeout, execution timeout, and stale-state repair.

```typescript
interface ExecutionCompletedPayload {
  execution_id: number;
  action_id: number;
  action_ref: string;
  status: string;
  result: unknown | null;
  completed_at: string;
}
```

The envelope uses `message_type: "ExecutionCompleted"`. Status casing varies by producer. Workers and the supervisor can emit Rust debug names such as `Completed`; scheduler paths can emit lowercase values such as `completed`. The listener reloads the execution and does not parse this field into an enum.

Some synthetic producers emit `action_id: 0` when an execution has no action ID because the wire field is not nullable.

Producers: [worker](../../crates/worker/src/service.rs#L1813-L1857), [timeout monitor](../../crates/executor/src/timeout_monitor.rs#L218-L238), [inquiry handler](../../crates/executor/src/inquiry_handler.rs#L351-L484), [workflow scheduler](../../crates/executor/src/scheduler.rs), [supervisor](../../crates/supervisor/src/main.rs#L980-L1019).

Consumer: [`CompletionListener`](../../crates/executor/src/completion_listener.rs#L101-L166). Payload: [`ExecutionCompletedPayload`](../../crates/common/src/mq/messages.rs#L399-L414).

## Inquiry response queue

Route:

```text
attune.executions / inquiry.responded
  -> attune.inquiry.responses.queue
  -> executor InquiryHandler
```

The API inquiry response route is the producer.

```typescript
interface InquiryRespondedPayload {
  inquiry_id: number;
  execution_id: number;
  response: unknown;
  responded_by: number | null;
  responded_at: string;
}
```

The envelope uses `message_type: "InquiryResponded"`. The handler reloads the inquiry and requires its persisted status to be `responded`, then reloads the execution. It does not compare `responded_by` or `responded_at` with the database.

Producer: [inquiry API](../../crates/api/src/routes/inquiries.rs#L485-L509). Consumer: [`InquiryHandler`](../../crates/executor/src/inquiry_handler.rs#L72-L95). Payload: [`InquiryRespondedPayload`](../../crates/common/src/mq/messages.rs#L433-L446).

## Pack test request queue

Pack tests take two hops. The API requests a test through the fixed queue, then the executor selects a worker and republishes the same payload to a worker-specific queue.

```text
attune.executions / pack.test.requested
  -> attune.pack.tests.queue
  -> executor PackTestProcessor
  -> attune.executions / pack.test.dispatch.worker.{worker_id}
  -> worker.{worker_id}.packtests
  -> selected worker
```

```typescript
interface PackTestRequestedPayload {
  pack_install_id: number;
  pack_ref: string;
  pack_version: string;
  candidate_path: string | null;
  candidate_access_token?: string;
  trigger_reason: string;
  required_runtimes: string[];
  worker_selector: unknown;
  worker_tolerations: unknown;
  worker_affinity: unknown;
}
```

Both hops use `message_type: "PackTestRequested"`. Missing placement fields get empty object or array defaults. `candidate_access_token` is omitted when absent. The API omits it on the first hop. After the executor claims a worker, it adds an attempt-scoped token for staged candidate access on the second hop.

`candidate_access_token` is a secret. Do not log complete pack-test messages or expose them through queue inspection tooling.

Producer and consumer sources: [API request](../../crates/api/src/routes/packs.rs#L1178-L1192), [executor selection and dispatch](../../crates/executor/src/pack_test_processor.rs#L46-L192), [worker consumer](../../crates/worker/src/service.rs#L995-L1075). Payload: [`PackTestRequestedPayload`](../../crates/common/src/mq/messages.rs#L646-L678).

## Sensor rule lifecycle queue

`attune.rules.lifecycle.queue` multiplexes rule and pack lifecycle payloads. The sensor first deserializes `MessageEnvelope<serde_json::Value>`, selects a concrete payload by `message_type`, and updates managed sensor processes.

Active bindings on `attune.events` are:

```text
rule.created
rule.enabled
rule.disabled
rule.deleted
pack.registered
pack.deleted
```

`rule.created` carries:

```typescript
interface RuleCreatedPayload {
  rule_id: number;
  rule_ref: string;
  trigger_id: number | null;
  trigger_ref: string;
  action_id: number | null;
  action_ref: string;
  trigger_params: unknown | null;
  enabled: boolean;
}
```

`rule.enabled` carries:

```typescript
interface RuleEnabledPayload {
  rule_id: number;
  rule_ref: string;
  trigger_ref: string;
  trigger_params: unknown | null;
}
```

`rule.disabled` carries:

```typescript
interface RuleDisabledPayload {
  rule_id: number;
  rule_ref: string;
  trigger_ref: string;
}
```

`rule.deleted` carries:

```typescript
interface RuleDeletedPayload {
  rule_id: number;
  rule_ref: string;
  trigger_id: number | null;
  trigger_ref: string;
}
```

Rule producers are the API rule and trigger routes. The same queue receives `PackRegisteredPayload` and `PackDeletedPayload`, defined in the per-worker pack queue section.

Sources: [rule publishers](../../crates/api/src/routes/rules.rs), [trigger bulk enable and disable publishers](../../crates/api/src/routes/triggers.rs#L83-L110), [payload definitions](../../crates/common/src/mq/messages.rs#L465-L550), [sensor queue and consumer](../../crates/sensor/src/rule_lifecycle_listener.rs#L55-L246).

The queue also declares a `metadata.trigger.changed` binding on `attune.events`. Normal `TriggerChanged` messages publish to `attune.metadata`, so this binding receives none of them. See [non-working routes](#non-working-and-dormant-routes).

## Sensor catch-all queue

The sensor startup path declares durable `attune.events.queue` with an `attune.events / #` binding. No service currently consumes it.

Every event, rule lifecycle, and pack lifecycle publication routed through `attune.events` can therefore accumulate in this queue. Treat its depth as retained, unprocessed traffic rather than evidence of a slow active consumer.

Source: [sensor infrastructure declaration](../../crates/common/src/mq/connection.rs#L694-L717).

## Per-worker queue families

`Connection::setup_worker_infrastructure(worker_id, ...)` creates four durable queues for each worker database ID. They are non-exclusive and non-auto-delete, so a worker restart with the same ID reconnects to the same queues.

| Queue pattern | Binding | Consumer tag | Prefetch | Intent |
| --- | --- | --- | --- | --- |
| `worker.{worker_id}.executions` | `execution.dispatch.worker.{worker_id}` | `worker-{worker_id}` | worker concurrency + 2 | Run actions assigned to one worker |
| `worker.{worker_id}.packs` | `pack.registered`, `pack.deleted` | `worker-{worker_id}-packs` | 5 | Synchronize pack files and runtime environments |
| `worker.{worker_id}.cancel` | `execution.cancel.worker.{worker_id}` | `worker-{worker_id}-cancel` | 10 | Stop an execution owned by one worker |
| `worker.{worker_id}.packtests` | `pack.test.dispatch.worker.{worker_id}` | `worker-{worker_id}-packtests` | 1 | Run a pack test on the selected worker |

Only the execution queue has a message TTL. Its default is 300,000 milliseconds. All four queues point at `attune.dlx` when dead lettering is enabled.

Source: [worker topology generation](../../crates/common/src/mq/connection.rs#L571-L691), [worker queue TTL default](../../crates/common/src/mq/config.rs#L551-L553).

## Per-worker execution dispatch

Route:

```text
attune.executions / execution.dispatch.worker.{worker_id}
  -> worker.{worker_id}.executions
  -> selected worker
```

The scheduler's private producer payload is:

```typescript
interface ExecutionScheduledProducerPayload {
  execution_id: number;
  worker_id: number;
  action_ref: string;
  config: unknown | null;
  scheduled_attempt_updated_at: string;
}
```

The envelope still uses `message_type: "ExecutionRequested"`. The worker deserializes a smaller local type:

```typescript
interface ExecutionScheduledWorkerPayload {
  execution_id: number;
  action_ref: string;
  worker_id: number;
}
```

Serde ignores the producer's `config` and `scheduled_attempt_updated_at` fields. The worker reloads execution details by `execution_id`. It does not verify payload `worker_id`; the targeted routing key provides worker selection.

After obtaining a concurrency permit, the consumer starts the action in a background task and returns success. RabbitMQ acknowledges the dispatch before the action finishes. Later action failure is reported through status and completion messages, not by nacking the dispatch.

Sources: [producer payload and dispatch](../../crates/executor/src/scheduler.rs#L489-L497), [scheduler publication](../../crates/executor/src/scheduler.rs#L6510-L6544), [worker payload and consumer](../../crates/worker/src/service.rs#L81-L87), [worker dispatch handling](../../crates/worker/src/service.rs#L1243-L1411).

## Per-worker pack lifecycle

Route:

```text
attune.events / pack.registered or pack.deleted
  -> worker.{worker_id}.packs for every durable worker ID
  -> each worker
```

`pack.registered` carries:

```typescript
interface PackRegisteredPayload {
  pack_id: number;
  pack_ref: string;
  version: string;
  runtime_names: string[];
}
```

`pack.deleted` carries:

```typescript
interface PackDeletedPayload {
  pack_id: number;
  pack_ref: string;
}
```

The API pack routes produce both messages. Workers validate `pack_ref`, then synchronize files, create runtime environments, or remove local pack state. The sensor lifecycle queue receives the same messages.

Sources: [pack publishers](../../crates/api/src/routes/packs.rs#L908-L916), [pack registration publisher](../../crates/api/src/routes/packs.rs#L2277-L2294), [worker pack consumer](../../crates/worker/src/service.rs#L783-L973), [payloads](../../crates/common/src/mq/messages.rs#L523-L550).

## Per-worker cancellation

Route:

```text
attune.executions / execution.cancel.worker.{worker_id}
  -> worker.{worker_id}.cancel
  -> selected worker
```

```typescript
interface ExecutionCancelRequestedPayload {
  execution_id: number;
  worker_id: number;
}
```

The envelope uses `message_type: "ExecutionCancelRequested"`. The API produces user-requested cancellations. The executor execution manager produces workflow cancellation cascades.

The worker uses `execution_id` and does not verify the payload's worker ID. A cancellation that arrives before dispatch is kept in the worker's pending-cancellation set so the later execution can stop immediately.

Sources: [API producer](../../crates/api/src/routes/executions.rs#L1578-L1594), [executor producer](../../crates/executor/src/execution_manager.rs#L323-L343), [worker consumer](../../crates/worker/src/service.rs#L1593-L1659), [payload](../../crates/common/src/mq/messages.rs#L634-L644).

## Per-worker pack tests

`worker.{worker_id}.packtests` is the second hop described in the [pack test request queue](#pack-test-request-queue). It uses the same `PackTestRequestedPayload`, with an optional `candidate_access_token` added by the executor.

The worker validates the claimed install, assigned worker ID, pack identity, trigger reason, candidate path, and candidate token before running tests.

Sources: [executor second-hop publisher](../../crates/executor/src/pack_test_processor.rs#L163-L192), [worker validation and execution](../../crates/worker/src/service.rs#L1997-L2139).

## Ephemeral metadata queues

Each replica calls `create_ephemeral_topic_consumer()`. RabbitMQ assigns the queue name. These queues are non-durable, exclusive, and auto-delete. Every replica receives a copy of each matching publication, unlike fixed durable queues where replicas compete for messages.

| Owner | Bindings on `attune.metadata` | Consumer tag | Intent |
| --- | --- | --- | --- |
| Executor replica | `metadata.action.changed` | `executor.metadata.invalidation` | Invalidate action metadata cache |
| Worker replica | `metadata.action.changed`, `metadata.runtime.changed`, `metadata.pack.changed` | `worker-{worker_id}-metadata` | Invalidate action, runtime, and pack caches |
| API replica | `metadata.permission_set.changed`, `metadata.identity_authorization.changed` | `api.authz.metadata.invalidation` | Invalidate authorization caches |

Source: [ephemeral queue creation](../../crates/common/src/mq/connection.rs#L163-L229), [executor consumer](../../crates/executor/src/service.rs#L80-L131), [worker consumer](../../crates/worker/src/service.rs#L1681-L1759), [API consumer](../../crates/api/src/main.rs#L133-L184).

Metadata payloads are:

```typescript
interface ActionChangedPayload {
  action_id: number;
  action_ref: string;
  pack_ref: string;
  operation: string;
  updated_at: string;
}

interface TriggerChangedPayload {
  trigger_id: number;
  trigger_ref: string;
  pack_ref: string | null;
  operation: string;
  updated_at: string;
}

interface RuntimeChangedPayload {
  runtime_id: number;
  runtime_ref: string;
  pack_ref: string | null;
  operation: string;
  updated_at: string;
}

interface PackChangedPayload {
  pack_id: number;
  pack_ref: string;
  operation: string;
  updated_at: string;
}

interface PermissionSetChangedPayload {
  permission_set_id: number;
  permission_set_ref: string;
  operation: string;
  updated_at: string;
}

interface IdentityAuthorizationChangedPayload {
  identity_id: number;
  operation: string;
  updated_at: string;
}
```

`operation` is a free string, not an enum. Current producers use create, update, delete, enable, disable, registration, role-assignment, and permission-assignment terms appropriate to each payload.

Producers: [actions](../../crates/api/src/routes/actions.rs#L939-L958), [triggers](../../crates/api/src/routes/triggers.rs#L543-L562), [runtimes](../../crates/api/src/routes/runtimes.rs#L100-L119), [packs](../../crates/api/src/routes/packs.rs#L411-L429), [permission and identity authorization](../../crates/api/src/routes/permissions.rs#L52-L99).

Payload definitions: [metadata payloads](../../crates/common/src/mq/messages.rs#L552-L632).

`metadata.trigger.changed` currently has no correctly bound consumer. The API publishes it to `attune.metadata`, while the sensor lifecycle queue binds the key on `attune.events`.

## Dead-letter queue

Common setup derives `attune.dlx.queue` from the default exchange name `attune.dlx`. The queue is durable, non-exclusive, and non-auto-delete. Application queues set only `x-dead-letter-exchange`, so RabbitMQ preserves the original routing key when dead-lettering.

The executor's `DeadLetterHandler` accepts any JSON envelope but only acts on `message_type: "ExecutionRequested"`. It extracts `payload.execution_id`, checks that the execution is still scheduled, and marks an expired dispatch failed. Other message types are acknowledged and discarded.

The current topology has a routing defect. `attune.dlx` is a direct exchange, but `attune.dlx.queue` is bound with `#`. A direct exchange treats `#` as a literal key, not a wildcard. Dead-lettered messages retain keys such as `execution.dispatch.worker.42`, so they do not match this binding and are normally unroutable.

The configured dead-letter TTL defaults to 86,400,000 milliseconds, but common setup does not apply it to `attune.dlx.queue`. The queue has no configured expiry.

Sources: [dead-letter configuration](../../crates/common/src/mq/config.rs#L396-L427), [DLX declaration and binding](../../crates/common/src/mq/connection.rs#L457-L480), [`DeadLetterHandler`](../../crates/executor/src/dead_letter_handler.rs#L45-L190).

## Non-working and dormant routes

These names exist in configuration or code but do not form an active producer-to-consumer path.

| Route or queue | Current state |
| --- | --- |
| `attune.events.queue` | Declared and bound to all `attune.events` traffic, but has no consumer |
| `attune.executions.queue` | Legacy configured name; never declared, bound, or consumed |
| `attune.notifications.queue` | Setup helper exists but is never called |
| `attune.events.dlq`, `attune.executions.dlq`, `attune.notifications.dlq` | Constants only; no declarations or consumers |
| `attune.executions / inquiry.created` | Active producer and payload type, but no queue binding |
| `attune.notifications / notification.created` | Payload type and routing exist, but no active publisher or consumer |
| `attune.executions / execution.cancel` | Canonical routing key exists, but active producers override it with a worker-specific key |
| `attune.metadata / metadata.trigger.changed` | Active producer, but the intended sensor binding is on the wrong exchange |
| `attune.dlx / #` | Queue binding exists, but direct-exchange matching makes it ineffective for normal dead-letter keys |

`InquiryCreatedPayload` would have this shape:

```typescript
interface InquiryCreatedPayload {
  inquiry_id: number;
  execution_id: number;
  prompt: string;
  response_schema: unknown | null;
  assigned_to: number | null;
  timeout_at: string | null;
}
```

The executor publishes it with `message_type: "InquiryCreated"` and routing key `inquiry.created`. RabbitMQ accepts the non-mandatory publication and drops it when no queue is bound.

Source: [inquiry created producer](../../crates/executor/src/inquiry_handler.rs#L234-L248), [payload](../../crates/common/src/mq/messages.rs#L416-L431).

`NotificationCreatedPayload` is defined as:

```typescript
interface NotificationCreatedPayload {
  notification_id: number;
  channel: string;
  entity_type: string;
  entity: string;
  activity: string;
  content: unknown | null;
}
```

The live notifier service uses PostgreSQL `LISTEN/NOTIFY`, not this RabbitMQ route. See the [Notifier WebSocket reference](../api/notifier-websocket.md).

Sources: [notification payload](../../crates/common/src/mq/messages.rs#L448-L463), [notification routing](../../crates/common/src/mq/messages.rs#L103-L132), [notification queue setup helper](../../crates/common/src/mq/connection.rs#L719-L741).

## Configuration source

The Rust services construct `attune_common::mq::MessageQueueConfig::default()` and use the application configuration only for the RabbitMQ URL. The nested queue, exchange, and dead-letter values in repository YAML files do not currently replace this topology.

As a result, changing a queue name or exchange in YAML does not change the active service declarations. Change the shared MQ configuration construction and all affected producers and consumers together.

Sources: [API startup](../../crates/api/src/main.rs#L58-L61), [executor startup](../../crates/executor/src/service.rs#L154-L168), [worker startup](../../crates/worker/src/service.rs#L195-L197), [sensor startup](../../crates/sensor/src/service.rs#L87-L106), [supervisor startup](../../crates/supervisor/src/main.rs#L131-L140).

## Operating expectations

- Treat fixed and per-worker queues as durable work delivery. Service replicas that consume the same fixed queue compete for messages.
- Treat ephemeral metadata queues as best-effort broadcasts. A disconnected replica misses invalidations and must rebuild state through its normal cache loading path.
- Make message handlers idempotent. Requeue, connection recovery, and duplicate publication can deliver the same logical work more than once.
- Use PostgreSQL rows as authoritative state where consumers already reload by ID.
- Do not place secrets in generic messages. The pack-test candidate token is the narrow exception and must not appear in logs or diagnostics.
- Monitor `attune.events.queue`; it has no consumer and can grow without bound.
- Do not rely on the current dead-letter queue until its direct-exchange binding is corrected.

## How to update this reference

Build the inventory from declarations, bindings, publications, and active consumer startup. A queue name in configuration or a `MessageType` variant does not prove that a working route exists.

### Rebuild the exchange and queue inventory

1. Read the defaults in `crates/common/src/mq/config.rs` for configured exchange and queue names.
2. Inspect `setup_common_infrastructure()` and each service-specific setup function in `crates/common/src/mq/connection.rs`.
3. Search for direct `queue_declare`, `exchange_declare`, and `queue_bind` calls outside the shared connection module. `attune.rules.lifecycle.queue` is declared this way.
4. Find generated queue names built with `format!`. Record which identifier becomes part of each name and routing key.
5. Find calls to `create_ephemeral_topic_consumer()`. Each call creates one broker-named queue per running service replica.
6. Search service startup code for calls to each setup helper. A helper that has no caller describes dormant topology, not an active queue.

Useful searches:

```bash
rg -n "queue_declare|exchange_declare|queue_bind" crates --glob '*.rs'
rg -n "setup_.*infrastructure|create_ephemeral_topic_consumer" crates --glob '*.rs'
rg -n 'format!\("worker\.|amq\.gen' crates --glob '*.rs'
```

### Trace producers and consumers

For each binding, trace both ends:

1. Start with the exchange and routing key in the queue binding.
2. Search for the routing-key string, its constant, and the matching `MessageType` variant.
3. Include explicit `publish_envelope_with_routing()` calls. Worker dispatch, cancellation, and pack-test dispatch override the canonical routing key.
4. Record every producer call site. Do not list only the service named in a payload comment.
5. Find the `ConsumerConfig` and the task that starts it. Record the consumer tag, prefetch, exclusivity, and expected concrete payload type.
6. Follow the handler far enough to state its intent and identify which payload fields it trusts or replaces with database state.

Useful searches:

```bash
rg -n "publish_envelope|publish_envelope_with_routing|basic_publish" crates --glob '*.rs'
rg -n "ConsumerConfig|basic_consume|consume_with_handler|consume_once_with_handler" crates --glob '*.rs'
rg -n "routing_keys::|routing_key\(\)|\.routing_key\(" crates --glob '*.rs'
```

### Rebuild payload definitions

1. Use `MessageEnvelope<T>`, `MessageHeaders`, and `MessageType` in `crates/common/src/mq/messages.rs` for the common envelope.
2. Use the concrete payload struct consumed on each route. Check for private producer or consumer structs outside `attune_common`; per-worker execution dispatch currently has two different Rust structs.
3. Read Serde attributes on every field. Record defaults, omitted optional fields, flattened values, enum representation, and unknown-field behavior.
4. Check queues that first deserialize to `serde_json::Value` and switch on `message_type`. These queues intentionally carry multiple payload shapes.
5. Search for payload construction sites to identify actual string values such as status casing and metadata operations.
6. Treat fields containing tokens, credentials, or arbitrary user data as sensitive and state whether operators may inspect or log them.

Useful searches:

```bash
rg -n "struct .*Payload|enum MessageType|struct MessageEnvelope|struct MessageHeaders" crates --glob '*.rs'
rg -n "MessageEnvelope::new|MessageType::" crates --glob '*.rs'
```

### Recheck delivery behavior

Inspect `crates/common/src/mq/publisher.rs`, `consumer.rs`, and `error.rs` whenever publication or failure handling changes. Record:

- Message persistence and publisher confirms
- Manual or automatic acknowledgements
- Prefetch and concurrency behavior
- Which errors requeue
- Queue TTL and dead-letter arguments
- Whether a worker acknowledges before or after the underlying action finishes

For dead lettering, verify the exchange type, source queue arguments, preserved or overridden routing key, target binding, and target consumer. All five must agree before the dead-letter path can be called active.

### Compare with a live broker

When RabbitMQ is available, compare the documented topology with the broker after all services have started:

```bash
docker compose exec rabbitmq rabbitmqctl list_exchanges name type durable auto_delete
docker compose exec rabbitmq rabbitmqctl list_queues name durable auto_delete messages consumers arguments
docker compose exec rabbitmq rabbitmqctl list_bindings source_name destination_name routing_key
docker compose exec rabbitmq rabbitmqctl list_consumers queue_name consumer_tag prefetch_count ack_required
```

Generated worker queues depend on registered worker IDs. Ephemeral queues exist only while their owning service replica is connected.

### Verify the update

1. Ensure every `MessageType` variant is documented as active, multiplexed, or dormant.
2. Ensure every declared queue appears in the fixed, generated, ephemeral, or dormant inventory.
3. Ensure every active queue has at least one binding and one running consumer.
4. Check local Markdown links and run `git diff --check`.
5. Run the shared MQ tests:

```bash
cargo test -p attune-common mq::
```

Run service-specific tests when a producer, consumer, worker dispatch, or dead-letter path changes. A live broker check is the strongest proof for generated names, queue arguments, bindings, and consumer tags.
