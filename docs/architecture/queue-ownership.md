# RabbitMQ queue ownership

The service that consumes a queue owns its declaration and bindings. All RabbitMQ users may declare the shared exchanges and dead-letter infrastructure because those declarations are idempotent.

See the [Internal RabbitMQ message queue reference](internal-message-queues.md) for the full topology, payloads, producer call sites, and known routing gaps.

## Shared infrastructure

API, executor, worker, sensor, and supervisor startup paths call `setup_common_infrastructure()`. It declares:

- `attune.events`
- `attune.executions`
- `attune.metadata`
- `attune.notifications`
- `attune.dlx`
- `attune.dlx.queue`

Source: [`Connection::setup_common_infrastructure()`](../../crates/common/src/mq/connection.rs#L437-L483).

## Executor ownership

The executor declares and consumes:

- `attune.executor.events.queue`
- `attune.enforcements.queue`
- `attune.execution.requests.queue`
- `attune.execution.status.queue`
- `attune.execution.completed.queue`
- `attune.inquiry.responses.queue`
- `attune.pack.tests.queue`
- `attune.dlx.queue`

Each executor replica also creates a broker-named ephemeral queue for action metadata invalidation.

Sources: [executor topology](../../crates/common/src/mq/connection.rs#L486-L569), [executor consumers](../../crates/executor/src/service.rs#L246-L550).

## Worker ownership

After registration, each worker creates four queues from its database ID:

- `worker.{worker_id}.executions`
- `worker.{worker_id}.packs`
- `worker.{worker_id}.cancel`
- `worker.{worker_id}.packtests`

Each worker also creates a broker-named ephemeral queue for action, runtime, and pack metadata invalidation.

Sources: [worker topology](../../crates/common/src/mq/connection.rs#L571-L691), [worker consumers](../../crates/worker/src/service.rs).

## Sensor ownership

The sensor declares `attune.events.queue`, but no current service consumes it. The sensor's active lifecycle listener separately declares and consumes `attune.rules.lifecycle.queue`.

Sources: [sensor catch-all declaration](../../crates/common/src/mq/connection.rs#L694-L717), [lifecycle queue](../../crates/sensor/src/rule_lifecycle_listener.rs#L55-L157).

## API ownership

The API is mostly a publisher. Each API replica creates one broker-named ephemeral queue for permission-set and identity-authorization invalidations.

Source: [API metadata consumer](../../crates/api/src/main.rs#L133-L184).

## Notifier ownership

The notifier does not use RabbitMQ. It receives PostgreSQL `LISTEN/NOTIFY` messages. `setup_notifier_infrastructure()` and `attune.notifications.queue` remain dormant.

See the [Notifier service](notifier-service.md) for its active transport.

## Queue properties

Fixed and per-worker queues are durable, non-exclusive, and non-auto-delete. Broker-named metadata queues are non-durable, exclusive, and auto-delete.

Most durable application queues dead-letter to `attune.dlx`. `attune.rules.lifecycle.queue` does not. The current direct-exchange binding on `attune.dlx.queue` does not match normal dead-letter routing keys; do not treat that queue as a working recovery path.
