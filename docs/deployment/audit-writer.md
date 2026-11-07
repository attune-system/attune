# Audit writer delivery and shutdown

Audit emission is nonblocking. The channel holds at most 4096 events; saturation
or a stopped writer rejects new events and increments `AuditEmitter.stats.lost`.
`emitted` counts attempts to an active emitter, `persisted` counts events whose
persistence was confirmed, and `lost` includes rejected or unconfirmed events.
A failed acknowledgement cannot establish that the database has no copy.

Batches contain at most 256 events, including when explicit tuning requests a
larger size. A batch reserves its PostgreSQL sequence ID and timestamp once before
insertion.
Retries reuse the complete `(id, created)` partitioned primary key with
`ON CONFLICT DO NOTHING`. This avoids duplicate records after ambiguous insertion
completion or a UTC day rollover without changing audit retention or its schema.
Allocation and each insertion attempt have two-second deadlines. Three insertion
attempts exhaust the budget; remaining unconfirmed events are explicitly counted
as loss. Flush returns false after any loss in this writer's lifetime, when it
cannot confirm delivery, or after its thirty-second deadline.

Orderly API shutdown retains the writer handle, closes admission, drains accepted
events and joins the task. Draining is bounded to thirty seconds; unfinished work
is cancelled and joined and residual unconfirmed events are counted. Threaded
fixture writers use the same bounded drain loop and explicitly join their thread.
Call `shutdown()` before closing the owning database pool.

The API continues using its existing pool budget. A second production pool would
need an explicit split of that configured budget; adding two connections per
replica without that split could worsen database pressure. The threaded fixture
writer already uses a separately attributed two-connection pool. Pool separation
would provide scheduling isolation, not independent durable storage. SQLx's
shared clean-session check applies to both constructions.
