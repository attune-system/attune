# Idle database activity: source analysis and implementation opportunities

Date: 2026-11-05. Status: analysis complete; proposals below are not implemented.
Code inspected: `2ad41436`. Evidence: the six accepted primary matrices in the
[measurement report](../testing/benchmarks/database-activity-2026-11-05/README.md),
plus a fresh examination of retained idle-window service logs. No services were
started and no production settings were changed for this analysis.

## Important correction to the earlier idle interpretation

The full-stack runner invokes `tests/e2e/configure_cache_retention.py` through
`scripts/run-integration-tests.sh`. This sets persisted
`retention.check_interval_seconds=2`, cache traversal/staging windows to five
seconds, and cache freshness alerts off. It happens during startup even when the
selected test is the performance collector. The similarly named pytest fixture
is not the cause of this campaign's override; the runner's standalone setup is.

All six primary runs' supervisor logs confirm a two-second retention interval.
The database checkpoint completes 22 shared-file and 24 API maintenance-cycle
starts during its one-minute observations, runs all 16 retention targets per
cycle, and records complete object cleanup with no graph/metadata scan pending.
The high supervisor rate therefore reflects accelerated scheduled maintenance,
not an unexplained busy loop or necessary pending-scan continuation. The normal
configured retention cadence is 3,600 seconds, with settings polling at most a
minute apart when no faster job/continuation is due.

The measured 8,800–9,500 server calls/minute remain valid observations of the
**E2E-configured stack**, but are not production-default idle rates. The
supervisor's apparent dominance is not sufficient to rank production work.
All three checkpoints used the same accelerated setting, preserving the matched
comparison under that background workload; it does not establish normal-cadence
latency or resource savings. Retained evidence is in
[`idle-follow-up.json`](../testing/benchmarks/database-activity-2026-11-05/idle-follow-up.json).

Before changing supervisor scheduling for performance, collect a separate idle
baseline with production-like persisted settings, completed startup recovery,
and no pending cleanup. Keep the accelerated lane separately labelled as a
maintenance-stress comparison. Never fix the benchmark by disabling maintenance
or omitting startup costs without disclosure. Record actual runtime retention,
native-job settings and schedule/backlog state alongside future counters.

## What the idle statements cost

The database checkpoint records about 833.5 ms shared-file / 684.5 ms API of
**aggregate top-level PostgreSQL execution time per minute**. The observer's own
statement snapshot contributes about 68.8 / 18.8 ms. These times include waits,
are not CPU utilization, exclude connection/network/client cost, and overlap
nested server accounting. They do not indicate current idle database saturation.

Empty workflow-log claims account for 1,170 server statements in each minute,
but only about 65–66 ms of server execution. Avoiding them is useful continuous
work reduction and improves replica scaling; it does not by itself predict a
large reduction in individual execution latency. Maintenance locks and connection
churn could affect tail behavior under load, which requires a controlled test.

## Recommended implementation order

| Order | Opportunity | Measured basis | Expected benefit / confidence |
| --- | --- | --- | --- |
| 1 | Workflow-log dispatcher wakeups with bounded idle fallback | 1,170 empty claims/min across two executors | Large reduction in continuous claim traffic; high confidence in source cause. Notification/retry correctness needs careful tests. |
| 2 | Avoid previous-dispatch reads when no delay is needed | 96 reads/min per executor with eight empty queues | Small, low-risk reduction: about 192 calls/min in this setup. Included in, not additive to, the broader queue opportunity below. |
| 3 | Bulk ready-queue hints before empty dispatch preparation | 192 empty queue attempts/min across two executors | Avoid per-queue counts, transactions, graph admission and lease probes. Benefits grow with enabled empty queue count. |
| 4 | Retention initialization out of ordinary configuration reads | 352/384 target seed inserts in the accelerated minute | Clear redundant work; modest normal-cadence benefit. Batch seeding is the smallest contained step. |
| 5 | Deadline-aware inquiry/callback reconciliation | Three timeout pollers, two wait scanners and one callback poller at one second | Moderate continuous query reduction and better replica scaling; ownership/delivery behavior must remain explicit. |
| 6 | Maintenance connection/session reuse on proven clean completion | 242/264 connection setup statements and repeated enum discovery in the accelerated minute | Potential round-trip/cache savings, but higher lifecycle risk and much smaller normal-cadence impact. Measure first. |

These are hypotheses about removable work, not measured treatment speedups.
Counts are from the recorded topology; new replica/queue counts change them.
SQLx client events and PostgreSQL server calls have different coverage and must
not be combined as a precise accounting total.

### 1. Empty workflow-log claims

[`WorkflowLogDispatcher::start`](../../crates/executor/src/workflow/log.rs)
sleeps 100 ms after `claim_next` returns no row. Every executor independently
repeats the SQL claim. This explains almost 600 statements/minute/replica even
without a workflow.

Use committed outbox changes as wakeup hints, drain available work immediately,
and keep a bounded periodic reconciliation path for lost notifications and
reconnects. After a true empty result, use capped idle backoff with jitter.
Prefer an existing supported notification path; if a listener is introduced,
register its channels once with `listen_all`, reconnect and reconcile, and
avoid a read-then-sleep lost-wakeup race. A notification is not durable work.

Respect future `available_at`, expired claim leases, retry backoff, and the
next FIFO head becoming eligible even without a new insert. Wait until the
earliest known deadline or fallback timer, whichever comes first. Retain atomic
claims, owner/lease fences, sequence assignment, idempotent append/seal, and
fairness across workflows. Do not change the effect transaction's scope.

Illustrative bound: a five-second *fallback* would approach 24 empty probes/min
across two long-idle replicas instead of roughly 1,170, about 98% fewer for this
one family. That cap is a proposal, not an accepted latency contract: a missed
notification could delay work by five seconds. Normal notification-driven wakeup
and worst-case fallback latency both require explicit acceptance thresholds.

### 2–3. Empty work queues

[`poll_once`](../../crates/executor/src/queue_dispatcher.rs) runs every five
seconds. It searches enabled queues and attempts dispatch separately for each.
For each empty queue, the normal path still loads active capacity and the latest
terminal dispatch, starts a transaction, takes shared graph admission, probes
for a leaseable item, and commits without dispatching anything.

First, guard `latest_terminal_for_queue` behind the conditions which actually
need it: concurrency one and a positive inter-execution delay. Its current
caller fetches the row before `sequential_delay_until` can reject unnecessary
work. The empty queues in this campaign have zero delay, so those 192
reads/minute are directly avoidable without increasing poll latency.

Next, add one repository query for a bounded set of potentially ready queue IDs
per poll. Only resolve tunables and enter protected dispatch preparation for
those queues. Readiness is a **hint**: recheck item eligibility, batch/coalescing
rules, capacity and admission inside the existing transaction. A concurrent
insert may miss one scan, so retain the bounded scan and add enqueue/completion
wakeup hints if lower latency is needed. Scheduled retries and coalescing windows
need deadline wakeups even without new items. Keep orphan/leased-dispatch
recovery independent; skipping empty queue work must not disable recovery.

For these single-item, zero-delay queues, an empty attempt has roughly six
server statements (capacity, latest terminal, BEGIN, graph admission, item
selection, COMMIT). At 192 attempts/minute that is approximately 1,152 statements,
including the previous-read opportunity. A batched readiness hint would replace
most of that work when all queues are empty. This is a source-derived estimate;
measure the replacement and its actual query footprint before claiming savings.

### 4. Supervisor configuration and scheduling

[`RetentionRepository::load_config`](../../crates/common/src/repositories/retention.rs)
first calls `ensure_config`, executing one singleton insert and 16 target inserts
with `ON CONFLICT DO NOTHING`, then reads the settings. Thus a read path performs
17 seed writes even after migrations/startup have initialized everything.

Batch target seeding into one statement first. A later step can separate explicit
initialization/repair from steady-state reads, retaining a defined missing-row
repair path. Continue loading current persisted settings at the documented
cadence; this does not justify caching configuration forever. A simple batch
would remove 15 statements per load: about 330–360 per accelerated minute,
roughly 15 per minute at one normal settings poll/minute. These estimates exclude
API readers and other callers.

Schedule initialization also repeats on every leader cycle, schedule status is
read more than once, and each job attempts a conditional due update. Consider
one coherent schedule observation per cycle and conditional claims only for
apparently due jobs. Durable claims must still recheck due state atomically.
Keep startup/leader replacement recovery, database-clock completion deadlines,
and the independent retention/summary/object continuation cadences. Do not solve
this by stretching every maintenance interval or treating deferral as success.

### 5. Inquiry and callback work

The API and both executors each invoke `timeout_expired_pending` once a second:
about 180 attempts/minute across this topology. Both executors additionally scan
resolvable workflow waits at one second (about 120/minute). The API claims pending
callbacks at one second (about 60/minute). These rates are unaffected by the
retention override.

Use deadline-aware wakeups and bounded reconciliation. If consolidating timeout
ownership, first specify API-only availability and how executor lifecycle
messages, database notifications and callback enqueueing are preserved. Atomic
updates currently prevent multiple owners from timing out the same row; simply
removing a monitor can still change recovery/delivery guarantees. Callback
`SKIP LOCKED` claims distribute work, but every idle API replica still polls.
Preserve callback retry due times, lease recovery and independent attempts.

### 6. Maintenance connection churn

The supervisor intentionally calls `close_on_drop` and closes its advisory-lock
session after a cycle. Physical cleanup likewise disposes an owned connection
to prevent cancelled/in-flight work from returning to the pool. Replacements
repeat connection initialization and PostgreSQL enum/catalog discovery. The
recorded 11 connection setup events and 20 catalog events per accelerated cycle
are consistent with these ownership paths, but do not prove one exact owner for
every connection without backend-level correlation.

Do not remove disposal guards as a query-count shortcut. Investigate reuse only
on a path with acknowledged commit/rollback, acknowledged advisory unlock,
known reset session settings, no continuing physical I/O, and no pending protocol
responses. Unknown/cancelled completion must still destroy the socket. This is
higher risk than batching reads or suppressing empty claims and should follow a
production-like idle/connection baseline.

## Acceptance measurements and tests

Collect three distinct states: (1) genuinely quiescent production-like settings,
(2) empty enabled queues with normal scheduling, and (3) the existing accelerated
maintenance lane. Capture actual persisted settings, pending work, due times,
replicas and queues. Use equal observation lengths and independent repeated runs;
include at least one normal maintenance deadline crossing. Separate startup,
empty steady state, backlog draining and active workload windows.

For each proposed slice, report whole-database top-level/nested calls and time,
WAL, transaction and connection establishment counts, scoped client events, and
work-detection p50/p95/max. Exercise multiple executors/API replicas, listener
loss/reconnect, insert-before-sleep races, retry/lease expiry, shutdown during
ownership, and changed settings/leader takeover. Retain the current output and
admission assertions. A lower count must not come from delayed work, omitted
maintenance, skipped recovery, hidden retries or weakened checks.

Validation: read-only source/log analysis; all six primary logs confirm the
retention override. Documentation links and diff whitespace checks pass. No
runtime change was made, so application suites were not rerun. The earlier
capture-completeness failure and required CI remain separate merge-readiness gaps.
