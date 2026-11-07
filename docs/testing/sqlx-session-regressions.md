# SQLx session boundary regressions

Run `python3 scripts/run-sqlx-session-tests.py` from the repository checkout.
The runner creates exclusively owned PostgreSQL 16 and 18 containers, runs the
small standalone crate with the repository's vendored drivers and pinned lockfile,
then awaits and verifies removal. It does not migrate or contact application
services. Dependencies need to be cached or downloadable. Every invocation has a
unique run ID and at least four test threads.

The owned TCP relay forwards PostgreSQL protocol frames and holds a selected
ReadyForQuery response after server acceptance. A readiness signal establishes
the cancellation boundary. Tests observe backend state through an independent
connection, distinguish backend reuse/replacement, and check a later transaction's
committed visibility. Relay tasks are joined. Ordinary transaction ping remains
supported. Raw open/aborted transactions test containment independently of the
startup fix; healthy nested transactions test continued reuse.

The application integration tests `session_lease_tests`,
`audit_writer_resilience_tests`, `event_schema_binding_tests`,
`analytics_repository_tests`, and `storage_maintenance_repository_tests` use the
normal owned `TestDatabase` fixtures and runners. The lease tests exercise the
shared session owner used by scheduler checkpoints, including before advisory
lock acknowledgement and across a post-commit publication suspension.

The missing incident reproducer archive is not required by this harness; these
are new tests and do not certify rerunning its historical characterization cases.

Lifecycle warnings carry process-local cumulative counts: `interrupted_starts`,
`interrupted_nested_completions`, `reuse_rejections`, and
`uncertain_session_disposals`. These are boundary/ownership diagnostics, not
request counts or evidence of lost writes. Audit delivery has separate emitted,
persisted, retry, insert-failure, and unconfirmed-loss counters. None of these
counters uses request/backend IDs or payload data as labels.

The three native scheduler checkpoints use `SessionLease` transaction-boundary
helpers. These keep SQLx depth tracking valid for nested savepoints, while the
lease retains session ownership after commit for publication and unlock. The
healthy lease regression checks nested completion, explicit commit/rollback, and
same-backend reuse. Separate driver and analytics tests reject adopting a caller's
raw transaction and prove its writes remain under the caller's commit authority.

The executor's `checkpoint_cancellation::` cases invoke the actual initial-wait,
registered-wait, and cache checkpoint wrappers. They require the owned RabbitMQ
fixture (`ATTUNE_TEST_CACHE_BROKER_URL`, with the `cache-cancellation` vhost) and
run explicitly with `--ignored --test-threads=4`. A one-connection database pool
and protocol relays gate the real advisory-lock acknowledgement or publisher
confirmation. Independent observation checks the retained lock and transaction
state, and cancellation must terminate that backend before awaited teardown.
The tests do not replace publication with a synthetic pending future.

The cancellation relay enables TCP_NODELAY on both streams so deliberate
per-frame forwarding does not add delayed-ACK latency. The registered-wait
fixture must have a real response before either cancellation gate; the initial
wait fixture stays pending when testing registration itself.

The executor's six `receipt_deadline` cases require their owned broker vhosts
and explicit `--ignored --test-threads=4` invocation. They cover expiry at commit,
first publication and recovered publisher readiness, with a positive control.
Keep the declared consent windows and gate watchdogs unchanged. On Docker
Desktop, run the host-built executable in the owned database/broker network
when published-port latency consumes short consent windows. Record the binary
hash, runtime prerequisites and test run identity; changing the network is not
permission to bypass source or freshness checks.

Same-advance task input preserves opaque receipts for recorded workflow
publications when attaching CONTROL dependencies. These retain prospective
result/wait/empty witnesses from the issuing transaction. The internal path
requires a newly recorded workflow-variable receipt from that transaction and
execution; it cannot reopen an old origin as fresh approval or bind CONTROL as
replacement VALUE.

The cache-generation cleanup acknowledgement suite also distinguishes server
effect from client acknowledgement. When a relay holds BEGIN's response, the
startup guard can queue and flush rollback, making the backend idle. The caller
must still return the exact recovery deadline error if acknowledgement never
arrives. The test retains the held gate, absolute grace and durable quota checks.

## Explicit HTTP publication broker prerequisites

The `recorded_queue_execution_consumers_tests::collections::requested_publication`
cases in `recorded_execution_events_tests` require run-owned PostgreSQL and
RabbitMQ. Set `ATTUNE_REQUESTED_PUBLICATION_MQ_URL` to the run's normal
disposable vhost, and `ATTUNE_REQUESTED_PUBLICATION_FAILURE_MQ_URL` to a
**separate empty** disposable vhost on the owned broker. Do not initialize common
topology in the failure vhost or point either variable at a developer/service
broker. The fixture initializes topology only in the normal vhost.

Envelope publication selects its exchange from the message type, and a closed
connection can reconnect successfully. The failure case therefore uses the
missing execution exchange in the empty vhost to test a real publish error;
a supported delegated target must return exactly HTTP 500 and leave zero durable
reschedule marks. Recorded callers and targets remain unavailable at the route
boundary (HTTP 501, zero marks), independently of broker topology. The recorded
caller-expiry race consumes authentic historical admission at the repository
boundary; it does not certify currently disabled recorded HTTP publication. The test does not disable recovery or enlarge deadlines.

After provisioning these prerequisites, run all four explicitly:

```bash
ATTUNE_TEST_RUN_ID=requestpub1 \
  cargo test --locked -p attune-api --test recorded_execution_events_tests \
  recorded_queue_execution_consumers_tests::collections::requested_publication:: \
  -- --ignored --nocapture --test-threads=4
```

Await fixture teardown and verify removal of both owned vhosts or the owning
disposable broker and database. Default-suite ignored counts do not certify
these four cases; record their explicit invocation separately.
