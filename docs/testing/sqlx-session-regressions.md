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
