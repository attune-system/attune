# Timescale removal workload evidence

Date: 2026-10-06.
Baseline source: `252ce12e`.
Runner: `scripts/measure-timescaledb-removal.py`.

## Scope

This is a declared synthetic workload, not a measurement of an installed Attune
system. It establishes a repeatable database correctness and capacity check for
the ordinary-table design in [the removal plan](../plans/remove-timescaledb.md).
It does not establish production equivalence or a data-preserving conversion.

The runner applies frozen migration files with the existing Docker migration
runner. It skips the standard pack-index seeder with `/bin/true` because the
fixture uses denormalized refs and needs no installed packs. It uses diagnostic
SQL, not a new application database-access path. No Cargo build is required.
Repository and authenticated API integration tests remain separate checks.

## Declared workload and acceptance limits

The default fixture uses these inputs, declared before judging stock PostgreSQL:

| Input | Synthetic envelope |
| --- | --- |
| Retained time | Seven days |
| Execution creation rate | 20,000 per day, about 0.231 per second |
| Retained live executions | 140,000 |
| Execution history rate | 64,000 rows per day, about 0.741 per second |
| Retained execution history | 448,000 rows before the recent-tail probe |
| Event rate | 40,000 per day, about 0.463 per second |
| Retained events | 280,000 plus three explicit boundary records |
| Retained enforcements | 140,000 |
| Retained audit events | 140,000 |
| Worker and sensor history | 100 entities each, four transitions per entity per day, 2,800 rows per table |
| Retention backlog | 250,001 expired rows per target, five targets |
| Query concurrency | Four database queries, 20 warm samples per query |
| Query limit | Warm nearest-rank p95 at most 500 ms of server execution time |
| Retention limit | One target's 100 committed batches complete within one hour |
| Container resources | Four CPU limit, 256 MiB shared buffers, 16 MiB work memory, JIT disabled |

The fixture distributes timestamps deterministically over the retained window.
It does not sustain those rates with concurrent writers. The import time includes
bulk SQL and, on Timescale, a manual aggregate refresh. It is not a service-ingestion
benchmark. Each execution has creation, running, and terminal history. Every tenth
execution has an extra failed transition followed by running, and every twentieth
finishes with timeout. Retry transitions therefore count separately from creation
and live-row status.

JSONB includes structured host, request, attempt, and label values. The event
payload contains 32 deterministic MD5 blocks and the execution result contains
64. These vary by row rather than using one highly compressible repeated string.
The live execution config is a flat parameter map. The trigger probe changes a
real live result and checks its digest summary in history, then rolls back.

Timing uses `EXPLAIN ANALYZE` with buffers and saved query text. The runner records
a first-observed sample and warm samples separately. The first sample follows
seeding and correctness queries, so it is not a cold-cache sample. Samples omit
HTTP, connection establishment, Docker command overhead, and client transfer time.
They establish a database execution limit, not an endpoint latency limit.

Queries cover hourly view filters, source-timestamp-bounded raw counts, completed
live executions, entity history, and recent events with JSONB. The raw queries use
the same inclusive hourly-bucket window as the view queries. Saved plans allow
inspection of source-timestamp index use and computed-bucket full scans. The runner
reports a latency rejection with a nonzero exit after collecting all requested
variants. A rejected query is evidence, not an excuse to omit that query.

## Correctness contract

An independent Python counter derives every expected hourly row from the fixture
formula. The runner compares all six relations against it in UTC. It also checks
filtered refs, missing refs, terminal failure counts, column names, and `BIGINT`
count types. The JSON snapshots retain the actual rows, not just a pass flag.

`execution_throughput_hourly` counts history INSERT records.
`execution_status_hourly` counts status transitions, including repeated terminal
transitions on retry. `execution_volume_hourly.initial_status` is the current live
status grouped by creation time, despite the historical alias. The fixture keeps
that distinction. Event and enforcement views count creation records, and worker
status counts track transitions.

The explicit 11:59:59, 12:00:00, and 12:15:00 UTC event records prove bucket
inclusion. The inclusive bucket window 11:30 through 12:30 excludes the 11:00
bucket and includes both records in the 12:00 bucket. A session in
`Asia/Kathmandu` checks UTC alignment, including a timezone with a 45-minute
offset. The replacement must return identical bucket rows in both sessions.

Timescale jobs are disabled only inside the runner's disposable database. The
runner manually refreshes all four continuous aggregates through 13:00 UTC before
checking counts. It then inserts a completed transition at 13:15 UTC. Raw history
must show one record immediately. The materialized baseline must show none until
a second refresh through 14:00; the stock view must show one immediately.
This avoids presenting empty default materializations as the Timescale baseline.

The real execution trigger probe must produce exactly an INSERT and an UPDATE
history row. An identical second UPDATE must add no history row. The stored result
must equal `_jsonb_digest_summary` of the live result.

## Retention capacity and protections

The provisional default is 100 batches of 1,000 rows per target, once per hour.
Its arithmetic ceiling is 100,000 rows per hour or 2.4 million rows per day per
target. This ceiling assumes every hourly cycle runs and reaches its batch budget.
It is not an observed sustained delete rate.

The highest declared history arrival rate is 64,000 rows per day. A 2.4-million-row
daily budget exceeds that rate by 37.5 times. The old one-batch hourly budget of
24,000 rows per day cannot keep up with this declared execution-history rate or
the 40,000 daily events. Measured stock delete timings must still show that all
100 batches fit inside the interval.

Every history, event, and audit target receives the same 250,001-row expired
backlog, including duplicate timestamps, plus a record exactly at the cutoff.
The cutoff stays fixed throughout the target run. Stock cleanup uses the
repository's diagnostic query shape, with a materialized candidate CTE, ordered
ID or statement-local `ctid` selection, `FOR UPDATE SKIP LOCKED`, and a row limit.
The three ID-less history tables never persist a `ctid` between statements.
A single psql session commits each batch separately. The runner counts candidates
once before the run and records each batch's actual deleted count and SQL timing.
An instrumentation-only existence check can stop the session when no candidates
remain. It does not recount the backlog on every batch.

Stock cleanup must delete exactly 100,000 expired rows and leave 150,001 at the
default budget. Counts at or after the cutoff, including the equality sentinel,
must remain unchanged. Timescale instead runs its existing `drop_chunks` operation
and reports rows removed by before-and-after counts. A row count is not a chunk
count. Timescale background compression does not run during this comparison.

This unit checks history, event, and audit retention. Waiting workflows,
undelivered workflow logs, nonterminal executions, and nonterminal queue items
require the retention agent's repository tests. The measurement must not claim
those protections from this diagnostic fixture.

## Run and inspect the comparison

The runner requires Python 3, Git, Docker, and the local images
`timescale/timescaledb:2.30.1-pg18`, `postgres:18-alpine`, and `postgres:16-alpine`.
It records image IDs and source migration SHA-384 checksums. It refuses a missing
image rather than silently downloading a different image under the same tag.

Run from the removal worktree after the stock migration edits are present:

```bash
python3 scripts/measure-timescaledb-removal.py \
  --output /tmp/opencode/timescale-removal-comparison
```

The output path must be new and its parent must already exist. To collect only
the immutable baseline before the candidate migrations are ready:

```bash
python3 scripts/measure-timescaledb-removal.py \
  --output /tmp/opencode/timescale-removal-baseline \
  --variants timescale
```

This host's Docker VM ran out of disk during the first startup. For an explicitly
in-memory comparison, use the same storage option for every variant:

```bash
python3 scripts/measure-timescaledb-removal.py \
  --output /tmp/opencode/timescale-removal-tmpfs \
  --tmpfs --memory 8g
```

`--tmpfs` sets a six-GiB cap on PGDATA. It labels the run arguments and does not
establish disk-backed performance. The ordinary default uses image-owned anonymous
volumes. The runner assigns unique container names and ownership labels, binds
only ephemeral loopback ports, and processes variants sequentially. It uses no
developer database, named volume, Compose project, or service stack.

`finally` teardown verifies the ownership label, removes the exact container and
its anonymous volumes, and checks that those resources no longer exist. It also
handles failed setup, failed SQL, SIGINT, SIGHUP, and SIGTERM. A cleanup failure is
an error. SIGKILL or a host crash cannot execute Python teardown; the saved owner
and resource IDs identify that invocation's resources for recovery.

The evidence directory contains:

- `run.json`, the measurement runner snapshot, source snapshots, checksums, image and server metadata, resource IDs,
  ephemeral ports, run arguments, and final owned-container count.
- `fixture.sql` and `expected-hourly-counts.json`.
- Per-variant `hourly-counts.json`, `hourly-counts-kathmandu.json`,
  `analytics-columns.json`, and `correctness.json`.
- Per-query SQL, first-observed JSON query plans, every warm timing, and p95 results.
- `sizes.json` with uncompressed relation sizes and actual JSONB byte averages.
- `backlog.sql`, stock batch SQL and logs, and `retention.json` with actual counts.
- Migration logs, PostgreSQL logs, and `cleanup.json`.

Archive the whole directory before discarding local temporary storage. The summary
alone cannot reproduce the source bytes or inspect the query plans.

## Earlier tmpfs baseline

The final full Timescale run at `/tmp/opencode/timescale-evidence-final-baseline` passed on
PostgreSQL 18.6 using tmpfs, a four-CPU limit, and an eight-GiB memory limit.
The immutable source was `252ce12e`. Its ephemeral host port was `33123`.
The owner was `8da274e3c9c146f5`. Teardown removed its exact container and anonymous
volume, and `run.json` reports no remaining owned containers.

All six hourly relations matched the fixture oracle in UTC, including filtered
and missing refs and BIGINT count types. The four Timescale
aggregates also matched in Asia/Kathmandu. The existing
`execution_volume_hourly` and `enforcement_volume_hourly` views did not. Their
session-dependent buckets are an observed baseline defect, not a requirement to
preserve in the replacement. The recent tail contained one raw completed record,
zero aggregate records before refresh, and one after refresh. The real history
probe produced two history rows and an 82-byte result summary with the expected
digest. Average result JSONB was 2,154.11 bytes and event payload JSONB was
1,130.49 bytes.

Terminal transition counts were 133,000 completed, 14,000 failed, and 7,000 timeout.
The 154,000 terminal transitions exceed the 140,000 created executions because
the fixture includes retry failures. Their failure percentage is 13.6364 percent.

The full baseline's warm server p95 values were:

| Query | Timescale tmpfs p95 ms |
| --- | ---: |
| Status hourly view, 24 hours | 4.455 |
| Event hourly view, 24 hours | 1.266 |
| Timestamp-bounded raw status, 24 hours | 91.817 |
| Timestamp-bounded raw events, 24 hours | 42.555 |
| Completed live executions, 24 hours | 6.822 |
| Recent entity history | 19.810 |
| Recent events with JSONB | 5.851 |

`drop_chunks` removed all 250,001 expired rows per target. Retained-row counts
stayed unchanged, including cutoff equality records. Wall times were 0.369 seconds
for execution history, 0.406 for worker history, 0.398 for sensor history,
0.539 for events, and 1.427 for audit events. These are chunk-drop observations,
not measurements of the replacement's row-delete capacity.

The initial disk-backed startup failure and an early full-fixture integer-overflow
failure also completed owner-scoped teardown. The runner now uses BIGINT arithmetic
for the timestamp formula. The small pilot at
`/tmp/opencode/timescale-evidence-pilot2` passed before the full fixture run.
The SIGTERM check at `/tmp/opencode/timescale-evidence-cancel` observed the owned
container running before terminating the runner. Its `cleanup.json` confirms
container removal and no remaining anonymous volumes. A final label-filtered
Docker inventory found no evidence-run containers.

These earlier timings use tmpfs and an eight-GiB memory limit. The disk-backed
comparison below uses fresh anonymous volumes and a four-GiB memory limit.
The Docker VM was reset by the user between these runs. Do not attribute timing
differences between them solely to storage, or combine their samples into one p95.

## Disk-backed three-variant comparison

The complete comparison is at
`/tmp/opencode/timescale-removal-disk-comparison`. It uses the default runner
storage, fresh disk-backed anonymous volumes, four CPUs, and a four-GiB container
memory limit for every variant. Variants ran sequentially. The runner froze the
current candidate migrations before starting the first database and replayed the
same fixture SQL against all three. The immutable Timescale source remains
`252ce12e96106d0249a0369aad86e5f4b39daf81`.
The recorded fixture SHA-256 is
`d815bd17f79b586a7662b3e1a7987b4148d9176048b1936510d41cf8595ecf28`.

The user explicitly purged the Docker VM before this comparison. The three exact
image tags were pulled again. No additional pruning occurred. The images resolved
to these recorded IDs:

| Variant | Server | Image ID |
| --- | --- | --- |
| Timescale 2.30.1 | PostgreSQL 18.6 | `sha256:9dede0e3ccc071cf71935b17f76bf243331df0b1575338c8ac294640fcf12a36` |
| Stock PG18 | PostgreSQL 18.6 | `sha256:77f585114c32fbca283dc835b0596f4e52b51b4c6662d7810b2f4084f60a1873` |
| Stock PG16 | PostgreSQL 16.15 | `sha256:721873c34ceb9f8d8fc265984940dc982404c105f19ad51be9fdc5970a6080ea` |

### Correctness passed

All six hourly relations matched the independent fixture oracle in UTC on all
three variants. Filtered refs, missing refs, BIGINT count types, the two-record
partial-hour boundary, and the live history trigger probe passed. Terminal
transition counts remained 133,000 completed, 14,000 failed, and 7,000 timeout.
The 13.6364-percent transition failure rate matched the oracle.

All six stock views returned identical bucket rows in UTC and Asia/Kathmandu.
The historical Timescale baseline still has the two session-dependent ordinary
views described above. Both stock variants exposed the new completed tail record
immediately, without refresh. The baseline exposed it after its second manual
refresh. The aggregate counts therefore compare populated, explicitly refreshed
Timescale data against current raw-data views.

The JSONB byte averages were identical across variants, with 2,154.11 bytes per
live execution result and 1,130.49 bytes per event payload. Both stock servers
migrated successfully without a Timescale extension. Their recorded extensions
are `plpgsql`, `pgcrypto`, and `uuid-ossp`.

### Direct hourly-view latency failed

The declared limit remains 500 ms of warm server p95 at four-query concurrency.
The runner collected every query and completed retention checks on every variant
despite the latency rejection. The failed queries remain in the evidence.

| Query | Timescale disk p95 ms | PG18 disk p95 ms | PG16 disk p95 ms |
| --- | ---: | ---: | ---: |
| Status hourly view, 24 hours | 0.915 | 474.772 | 582.078 |
| Event hourly view, 24 hours | 0.308 | 579.021 | 500.563 |
| Timestamp-bounded raw status, 24 hours | 72.118 | 57.029 | 63.215 |
| Timestamp-bounded raw events, 24 hours | 38.173 | 45.600 | 49.475 |
| Completed live executions, 24 hours | 8.922 | 5.144 | 5.885 |
| Recent entity history | 23.717 | 0.415 | 0.278 |
| Recent events with JSONB | 6.688 | 0.620 | 0.662 |

PG18 rejected the direct event-view query. PG16 rejected both direct event-view
and status-view queries. PG16's event result exceeds the limit by 0.563 ms and
still counts as a rejection. The limit was not raised or rounded to hide it.
The timestamp-bounded raw queries and recent-record queries passed on both stock
versions. These are separate query results, not a passing replacement sample
substituted for the failed view query.

The saved PG18 event-view plan uses a parallel sequential scan with
`date_trunc('hour', created, 'UTC')` comparisons in its filter. Each of its three
scan loops removes about 80,000 records outside the selected buckets. It scans
the full 280,003-row retained event table rather than using a timestamp range
index to read the 40,003 selected records. Its serial first-observed query takes
127.818 ms; that is not the four-query warm p95 of 579.021 ms.

The saved PG16 status-view plan also scans the full retained history table with
computed-bucket comparisons. Its serial first-observed time is 89.952 ms, while
its four-query warm p95 is 582.078 ms. This is why one fast serial sample cannot
establish the concurrency acceptance check.

The PG18 timestamp-bounded event plan uses `idx_event_created` with an index
condition on `created >= ... AND created < ...`. It reads 40,003 selected records.
The query text, plans, and all samples are in each variant's `query-*.sql`,
`plan-*-first-observed.json`, and `latency.json` files.

The minimum application fix is source-timestamp bounds in the owning analytics
repository, with UTC bucket-bound conversion that preserves the endpoint's
inclusive bucket rules. This strategy already exists in the current
`AnalyticsRepository::execution_status_hourly` and `event_volume_hourly` methods,
which use `hourly_source_bounds` and query the source tables. Verify that all
application callers use those bounded methods. The measured diagnostic bounded
queries use equivalent UTC bucketing but do not invoke Rust or authenticated API
handlers, so API integration acceptance remains separate.

This run rejects a performance-equivalence claim for direct ordinary-view bucket
filters. It supports the bounded raw-query strategy at the declared synthetic
envelope. It does not justify adding summary storage or partitioning before
checking the existing bounded application paths. If direct view queries themselves
must meet the latency contract, they still need an indexed bucket-access design
and another comparison that preserves these failing samples.

### Stock batch capacity passed

Both stock versions executed exactly 100 autocommitted batches of 1,000 rows for
each of the five targets. Each target began with 250,001 expired candidates,
deleted exactly 100,000, and left 150,001 expired records. The budget therefore
stopped cleanup before exhausting the backlog, as intended. Retained counts and
the exact-cutoff sentinels remained unchanged.

The wall times below include the batch loop, its instrumentation-only existence
checks, and psql invocation overhead. They exclude fixture import and the initial
candidate and retained-row counts. The evidence also records server SQL timings
and every batch count.

| Target | PG18 100-batch wall seconds | PG16 100-batch wall seconds |
| --- | ---: | ---: |
| Execution history | 2.253 | 2.350 |
| Worker history | 4.346 | 2.602 |
| Sensor process history | 2.338 | 2.412 |
| Events | 2.642 | 4.578 |
| Audit events | 3.555 | 3.003 |

The slowest observed loop took 4.578 seconds, below the declared one-hour limit.
This supports the provisional 100-batch default for this synthetic envelope on
this disk-backed host. Its 2.4-million-row daily budget remains an arithmetic
ceiling, not a measured 24-hour sustained delete rate. No larger default is needed
to match the declared maximum arrival rate of 64,000 history rows per day.

Timescale dropped all 250,001 expired rows per target in one `drop_chunks`
operation. Its wall times were 2.454 seconds for execution history, 0.125 for
worker history, 0.109 for sensor history, 1.178 for events, and 1.137 for audit.
This operation removes a different number of rows than the stock budgeted loop,
so these wall times are not an equal-row deletion speed comparison.

### Resources and cleanup passed

The disk-backed run owner is `313a56d5d54a402b`. Its ephemeral loopback ports were
`34803` for Timescale, `35845` for PG18, and `43993` for PG16. Each variant's
`cleanup.json` records container removal and no remaining anonymous volumes.
The final owner-label container inventory is empty, and the final Docker volume
inventory is also empty. The pulled images remain available for reruns.

The complete three-variant run is not an all-green latency result. Remaining
acceptance work is application-call-path verification for bounded analytics.
Compression savings, sustained ingestion, endpoint latency, production durability
under backlog, and installed-workload measurements remain outside this evidence.
