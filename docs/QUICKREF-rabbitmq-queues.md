# RabbitMQ queues quick reference

See the [Internal RabbitMQ message queue reference](architecture/internal-message-queues.md) for payload schemas, producers, consumers, delivery behavior, and source links.

## Fixed queues

| Queue | Binding | Consumer |
| --- | --- | --- |
| `attune.executor.events.queue` | `attune.events / event.created` | Executor event processor |
| `attune.enforcements.queue` | `attune.executions / enforcement.#` | Executor enforcement processor |
| `attune.execution.requests.queue` | `attune.executions / execution.requested` | Executor scheduler |
| `attune.execution.status.queue` | `attune.executions / execution.status.changed` | Executor execution manager |
| `attune.execution.completed.queue` | `attune.executions / execution.completed` | Executor completion listener |
| `attune.inquiry.responses.queue` | `attune.executions / inquiry.responded` | Executor inquiry handler |
| `attune.pack.tests.queue` | `attune.executions / pack.test.requested` | Executor pack-test processor |
| `attune.rules.lifecycle.queue` | Rule and pack lifecycle keys on `attune.events` | Sensor lifecycle listener |
| `attune.events.queue` | `attune.events / #` | No consumer |
| `attune.dlx.queue` | `attune.dlx / #` | Executor dead-letter handler; binding is currently ineffective |

## Per-worker queues

For worker ID `42`, setup creates:

| Queue | Binding |
| --- | --- |
| `worker.42.executions` | `execution.dispatch.worker.42` |
| `worker.42.packs` | `pack.registered`, `pack.deleted` |
| `worker.42.cancel` | `execution.cancel.worker.42` |
| `worker.42.packtests` | `pack.test.dispatch.worker.42` |

These queues are durable. Only the execution queue has a message TTL, which defaults to five minutes.

## Ephemeral queues

Each executor, worker, and API replica creates a broker-named `amq.gen-...` queue for its metadata cache invalidations. These queues are non-durable, exclusive, and auto-delete. Every connected replica gets a copy.

## Known gaps

- `attune.events.queue` has no consumer and can grow indefinitely.
- `metadata.trigger.changed` publishes to `attune.metadata`, but its intended sensor binding is on `attune.events`.
- `inquiry.created` has a producer but no queue binding.
- The AMQP notification exchange and queue are dormant. The notifier uses PostgreSQL `LISTEN/NOTIFY`.
- `attune.dlx` is direct, but its queue binds with `#`. Normal dead-letter routing keys do not match.
- The configured dead-letter queue TTL is not applied to `attune.dlx.queue`.

## Inspect RabbitMQ

```bash
docker compose exec rabbitmq rabbitmqctl list_exchanges name type durable auto_delete
docker compose exec rabbitmq rabbitmqctl list_queues name durable auto_delete messages consumers arguments
docker compose exec rabbitmq rabbitmqctl list_bindings source_name destination_name routing_key
docker compose exec rabbitmq rabbitmqctl list_consumers queue_name consumer_tag prefetch_count ack_required
```
