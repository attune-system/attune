# Native PostgreSQL repository workload acceptance

This document records acceptance measurements for the phase-2 partition and hourly
summary implementation. The runner calls the actual Rust repositories on stock
PostgreSQL 16 and 18. Its performance queries do not use substitute SQL.

These are historical measurements from before the final correctness fixes and
the fresh-install migration rewrite. The user deferred reporting-performance work
on 2026-10-07. Preserve the failed gates below; they are not acceptance results
for the current migration baseline.

The latest completed acceptance fails. Eleven of 48 ingestion pairs exceed the
strict 10-percent limit. Three large-profile summary repository p95s exceed 10 ms,
and PG16's large-profile mixed-event p95 exceeds 500 ms. Seven-day read gates now
pass, and both full fixtures reach clean coverage through bounded restart catch-up.
No latency sample or ingestion pair was retried or excluded.

## Final combined-production acceptance with reconciliation

The original run `/tmp/opencode/native-workload-current-final`, owner
`1ccaa2be4352`, completed on October 7, 2026. Its parent PID was `656163`.
Harness restarts interrupted observation, not the detached benchmark. Observation
resumed with Linux `pidfd_open` and `poll` until the original parent exited.
No duplicate benchmark, replacement sample, or source edit occurred during the run.
The terminal `run.json` records `scope_passed=false`, `source_stable=true`, and
`cross_version_count_match=true`. Both Rust workload processes exited zero and
wrote `finished` records, so failed gates are measured results, not missing runs.

This run jointly measures the combined-CTE four-round-trip reader, derived-cache
transactions using local asynchronous commit, and cached static producer SQL with
full-origin indexing and actual-xmin ownership checks. Production bytes were
frozen before compilation. The executable SHA-256 is
`8e930dec27d01f057657db8a4a043908aa79b6810a4239959f6164331268caa0`.
The principal production fingerprints are:

| Source | SHA-256 |
| --- | --- |
| `native_maintenance/read.rs` | `5ec05582eb152370783203040d1002ae1658f95221d0c371c53b08a52f9508a1` |
| `native_maintenance/summaries.rs` | `ec5546c336f15b6439618ad3de78dd26192c29d665edddc30048875f4f0439c4` |
| `20261006000002_hourly_summaries.sql` | `12c8f3a6de13c72d864be7900ac416de79a156a807056bab8d6e673ddc8bf140` |

The full source map in `run.json` matches before build, after build, before and
after each server, and after the run. The first terminal audit also found no
current-source mismatch. Later edits to `summaries.rs` and `retention.rs` have
mtime 15:17:06 UTC, after terminal `run.json` at 15:13:53 UTC. Those later bytes
are not measured by this run. The runner and example stayed unchanged during
measurement.

### Environment, preparation, and count checks

Stock PostgreSQL 16.15 and 18.6 ran sequentially on owned disk-backed volumes,
each limited to four CPUs and four GiB. Shared buffers were 256 MiB, work memory
16 MiB, and JIT was disabled. Server `fsync` and `synchronous_commit` stayed on.
Only production derived-cache transactions set local `synchronous_commit=off`;
ingestion source transactions retained the normal durable-commit setting. This
workload does not replace the separate crash-recovery correctness tests.

The exact seven-day and full-payload 30-day event/history plus 90-day audit fixture
bytes match the preceding optimized run. There was no extra source-query warm-up
beyond the recorded oracles and normal measurement routine. Empty source-day
partitions were prepared through `PartitionRepository::ensure_day` before import.
Setup allows at most three attempts per day and a 600-second profile deadline,
retrying only `DeferredBusy` or `DeferredDeadline`. Every attempt records its
outcome, elapsed time, and rollback status. Exhaustion refuses import.

Each version recorded 180 `Applied`, 288 `AlreadyPresent`, and three `DeferredBusy`
setup outcomes across the complete workload. The three deferrals belong only to
the intentional blocked-parent protocol probe. That probe exhausted all three
attempts and recorded `import_started=false`. Actual fixture and ingestion setup
needed no deferred retry and had no setup errors. No DDL correction was needed to
prepare these fixtures.

| Profile | Events | Execution history | Executions / enforcements | Audit | Worker history | Source partitions event / history / audit |
| --- | ---: | ---: | ---: | ---: | ---: | --- |
| Seven days | 280,003 | 448,000 | 140,000 each | 140,000 | 2,800 | 16 / 16 / 16 |
| 30/90 days | 1,200,003 | 1,920,000 | 600,000 each | 1,800,000 | 12,000 | 39 / 39 / 99 |

Both versions match every declared source count and have zero DEFAULT event,
execution-history, and audit rows. All four summary kinds have 169 clean hours
for seven days and 721 clean hours for 30 days, including the boundary hour and
empty hours. Pending notifications are zero before clean-summary measurement.
All eight grouped summary oracles per version match raw source rows and the
independent fixture formulas. Cross-version fingerprints match, as do all
available count fingerprints from the preceding optimized run. Each mixed tail
contains 1,667 full-payload events and 2,667 history rows.

### Final read gates

Every row below contains 20 retained warm samples at four-query concurrency,
plus a separately retained first-observed sample. All samples return the correct
counts and mode. Summary responses are `SummaryOnly`; mixed responses are
`SummaryPlusRaw`. The clean-summary limit remains strictly below 10 ms, and the
raw/mixed limit remains at most 500 ms. Gate decisions use unrounded values.

| PG | Profile | Path | Query | Server p95 ms | Repository p95 ms | Repository gate |
| --- | --- | --- | --- | ---: | ---: | --- |
| 16 | Seven days | Raw | Events | 63.118 | 78.182 | Pass |
| 16 | Seven days | Raw | Status | 69.163 | 77.908 | Pass |
| 16 | Seven days | Summary | Events | 1.781 | 8.840 | Pass |
| 16 | Seven days | Summary | Status | 3.363 | 9.337 | Pass |
| 16 | Seven days | Mixed | Events | 12.174 | 25.407 | Pass |
| 16 | Seven days | Mixed | Status | 10.754 | 21.030 | Pass |
| 18 | Seven days | Raw | Events | 60.317 | 73.023 | Pass |
| 18 | Seven days | Raw | Status | 67.555 | 74.143 | Pass |
| 18 | Seven days | Summary | Events | 1.722 | 8.219 | Pass |
| 18 | Seven days | Summary | Status | 2.747 | 7.064 | Pass |
| 18 | Seven days | Mixed | Events | 23.300 | 30.601 | Pass |
| 18 | Seven days | Mixed | Status | 14.167 | 23.272 | Pass |
| 16 | 30/90 days | Raw | Events | 63.129 | 77.612 | Pass |
| 16 | 30/90 days | Raw | Status | 71.825 | 77.636 | Pass |
| 16 | 30/90 days | Summary | Events | 4.130 | 11.440 | Fail |
| 16 | 30/90 days | Summary | Status | 4.449 | 10.539 | Fail |
| 16 | 30/90 days | Mixed | Events | 2,729.908 | 2,736.512 | Fail |
| 16 | 30/90 days | Mixed | Status | 10.050 | 21.771 | Pass |
| 18 | 30/90 days | Raw | Events | 49.584 | 61.278 | Pass |
| 18 | 30/90 days | Raw | Status | 74.784 | 82.956 | Pass |
| 18 | 30/90 days | Summary | Events | 2.993 | 6.174 | Pass |
| 18 | 30/90 days | Summary | Status | 5.854 | 12.290 | Fail |
| 18 | 30/90 days | Mixed | Events | 19.932 | 26.197 | Pass |
| 18 | 30/90 days | Mixed | Status | 11.441 | 19.905 | Pass |

All server gates pass except PG16's large-profile mixed-event gate. Server time
is the sum of actual logged parse, bind, execute, and simple-protocol durations.
Repository wall time includes the actual client pipeline after connection
acquisition. These are separate clocks; HTTP was not measured. Component p95s
overlap and must not be added.

The PG16 mixed-event failure contains two retained warm samples, not a single
discardable first-observed sample:

| Tag | Repository ms | Server total ms | Raw-tail server bind ms | Raw-tail execute ms |
| --- | ---: | ---: | ---: | ---: |
| `nw_d30_mixed_EventVolume_0` | 2,736.512457 | 2,730.728 | 2,727.241 | 1.293 |
| `nw_d30_mixed_EventVolume_1` | 2,736.613662 | 2,729.908 | 2,726.175 | 1.527 |

Both long bind records belong to the parameterized raw-tail aggregate
`SELECT date_trunc('hour', created, 'UTC') ... FROM event WHERE created >= $1
AND created < $2 ... GROUP BY 1, 2, 3`. PostgreSQL itself records those durations,
so client/network delay alone does not explain the tail. Bind includes server
parameter handling and prepared-plan selection/planning. The logs do not isolate
partition pruning, planning metadata, storage waits, or another server wait as
the root cause. This acceptance run did not enable nested plan instrumentation.
`postgres.log` and `read-samples-with-server-statements.json` preserve the exact
statements and tags. No explanation based only on these timings should be treated
as a proven bad-plan or I/O diagnosis.

### First-pass deadlines and persisted restart catch-up

Materialization keeps the one-second operation deadline, 250-ms lock timeout,
five-second cycle budget, and 128 attempted operations per cycle. The harness
declares one first attempt per kind/hour, then at most 96 actual refresh cycles
within an overall 600-second materialization deadline. It does not retry timed
read or ingestion samples. Each catch-up cycle opens a fresh builder pool, calls
`SummaryRepository::refresh_cycle`, closes that pool, and compares persisted
coverage-ledger publications with the repository's confirmed commit count.
Cycle planning uses the fixed fixture end and retention bounds covering its source
window. No client-local list selects the retry buckets.

| PG | Profile | First attempts | On-budget first successes | First deadline failures | Catch-up commits | Catch-up repository / wall ms | Total materialization ms |
| --- | --- | ---: | ---: | ---: | ---: | --- | ---: |
| 16 | Seven days | 676 | 676 | 0 | 0 | 42 / 53.075 | 5,891.673 |
| 18 | Seven days | 676 | 676 | 0 | 0 | 32 / 43.385 | 5,610.696 |
| 16 | 30/90 days | 2,884 | 2,856 | 28 | 28 | 2,491 / 2,518.534 | 136,902.062 |
| 18 | 30/90 days | 2,884 | 2,868 | 16 | 16 | 378 / 407.530 | 75,933.278 |

Each profile needed only one verification/catch-up cycle. Both large-profile
cycles completed below five seconds with zero deadline or lock failures, zero
serialization retries, and no exhausted cycle budget. The ledger recorded exactly
28 and 16 new committed bucket publications. PG16 acknowledged 28 notifications
and wrote 406 groups; PG18 acknowledged 16 and wrote 254 groups. Every kind then
had complete clean coverage. A separate protocol probe also demonstrates that a
fresh builder pool fills one persisted empty-worker-hour gap on each version.

The first-pass failures remain failures of the unchanged one-second budget.
PG16 had 13 execution-status, 12 event-volume, and three execution-creation
failures. PG18 had nine execution-status and seven event-volume failures.
All 44 returned `Operation timed out: summary transaction deadline`, with
rollback awaited, `commit_acknowledged=false`, and
`commit_outcome_unknown=false`. There were no acknowledged late commits in this
run. Rollback logs put 27 PG16 and 15 PG18 failures in `replace_publish_ack`,
and one per version in `state_capture`. Reported transaction elapsed times were
1,000 to 1,002 ms. The slowest successful first attempts were 966 ms on PG16 and
960 ms on PG18. These phase records identify where deadlines expired, not a
proven underlying cause.

Bounded restart convergence passes for these fixtures. A claim that every
materialization operation met one second is false. The runner's
`all_first_materialization_attempts_on_budget` remains false on both versions;
`all_catchup_attempts_on_budget` is true. Complete coverage permits valid large-
profile read measurements without erasing first-pass budget failures.

### All final paired ingestion results

All 48 pairs and 96 samples completed, retaining all 38,400 transaction intervals.
There were no writer errors, missing sides, or source-count mismatches. Four
writers performed 100 durable transactions each. Each transaction made one or
25 actual `EventRepository::create` or `ExecutionRepository::create` calls with
full JSON, normal indexes, and normal production triggers. OFF disables only
native source INSERT tracking; pair order alternates OFF/ON and ON/OFF.

Every tracked sample produced 400 invalidation markers and consumed exactly
400 sequence values, including the 10,000-row batch samples. OFF produced no
markers and left the sequence uncalled. All marker/sequence checks passed.
This establishes coalescing correctness, not the 10-percent performance gate.

Both unrounded ratios must be at most 1.10 for every pair. Eleven pairs fail,
four of 24 on PG16 and seven of 24 on PG18. Ten fail only transaction p95;
PG18 event-single pair 1 fails both ratios. Execution-batch failures fell from
all 12 in the preceding optimized run to three of 12 here. These separate runs
are not pooled, and the improvement does not turn the remaining failures into
an acceptance pass.

| Source | Calls per transaction | Pair | PG16 elapsed ratio | PG16 p95 ratio | PG16 gate | PG18 elapsed ratio | PG18 p95 ratio | PG18 gate |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| Event | 1 | 0 | 0.985401 | 0.836432 | Pass | 1.004631 | 1.004759 | Pass |
| Event | 1 | 1 | 0.990538 | 1.038696 | Pass | 1.723697 | 6.788955 | Fail |
| Event | 1 | 2 | 0.983757 | 1.005698 | Pass | 0.976545 | 0.992836 | Pass |
| Event | 1 | 3 | 0.996649 | 0.997034 | Pass | 1.003664 | 1.005475 | Pass |
| Event | 1 | 4 | 1.023445 | 0.997980 | Pass | 0.984019 | 0.995703 | Pass |
| Event | 1 | 5 | 1.006344 | 1.002191 | Pass | 1.008777 | 0.993607 | Pass |
| Event | 25 | 0 | 0.959033 | 0.783726 | Pass | 1.012707 | 1.149416 | Fail |
| Event | 25 | 1 | 0.977427 | 0.998025 | Pass | 0.976011 | 0.980869 | Pass |
| Event | 25 | 2 | 1.013635 | 0.953630 | Pass | 1.043933 | 1.192567 | Fail |
| Event | 25 | 3 | 1.017516 | 1.136514 | Fail | 1.008606 | 0.989463 | Pass |
| Event | 25 | 4 | 1.042162 | 1.172454 | Fail | 1.029119 | 1.170732 | Fail |
| Event | 25 | 5 | 1.011163 | 1.164635 | Fail | 1.009018 | 1.068841 | Pass |
| Execution | 1 | 0 | 1.001844 | 0.988597 | Pass | 1.045055 | 1.195002 | Fail |
| Execution | 1 | 1 | 1.048677 | 1.031899 | Pass | 1.010034 | 0.995215 | Pass |
| Execution | 1 | 2 | 1.014225 | 1.006717 | Pass | 0.972689 | 0.991561 | Pass |
| Execution | 1 | 3 | 0.995555 | 1.007872 | Pass | 0.997242 | 0.884411 | Pass |
| Execution | 1 | 4 | 0.967989 | 0.997121 | Pass | 1.019265 | 1.024408 | Pass |
| Execution | 1 | 5 | 0.984741 | 1.014493 | Pass | 0.999173 | 0.993985 | Pass |
| Execution | 25 | 0 | 1.076544 | 1.106116 | Fail | 1.034150 | 1.003435 | Pass |
| Execution | 25 | 1 | 1.032595 | 0.991570 | Pass | 1.031339 | 1.025152 | Pass |
| Execution | 25 | 2 | 1.030875 | 1.082407 | Pass | 1.026438 | 0.995738 | Pass |
| Execution | 25 | 3 | 1.009699 | 0.972057 | Pass | 1.090599 | 1.215584 | Fail |
| Execution | 25 | 4 | 0.960377 | 1.043453 | Pass | 1.075230 | 1.103782 | Fail |
| Execution | 25 | 5 | 1.051741 | 1.007455 | Pass | 1.046117 | 1.081404 | Pass |

PG18 event-single pair 1 retained a tracking wall interval of 7,997.621 ms versus
4,639.806 ms OFF, and transaction p95 of 378.085 ms versus 55.691 ms. Thirteen
tracking intervals exceeded 500 ms; OFF had none. The largest tracking interval
was 1,154.781 ms. All rows and markers were correct, and no writer returned an
error. This result is neither retried nor labeled an environmental outlier.
Acceptance ingestion was not instrumented with per-statement SPI/function
statistics, so these intervals do not prove whether trigger work, planning,
durable commits, or another server wait caused the miss. The older instrumented
diagnostic below measured different producer bytes and cannot establish the
latest producer's root cause.

### Linked expiry, cleanup, and final disposition

`linked-expiry.json` accepts both versions' original matched 10,000/1,000,000-row
cohorts from `/tmp/opencode/native-workload-acceptance`, owner `82c9081ffe54`.
The runner verified byte-identical `native_partition_check` and
`native_partition_expire` definitions before linking original hashes, counts,
confirmed units, sentinel retention, WAL differences, and lock bounds.
Million-row DROP repository intervals remain 220.726 ms on PG16 and 216.334 ms
on PG18, versus 508.710 and 402.859 seconds for equal-cohort row cleanup.
These are retained prior measurements, not new samples. The long controls
were not rerun.

Both terminal `server.json` records contain zero owned clone sessions, no clone
databases, no cleanup errors, and empty remaining-container/volume inventories.
Only each run-owned migration template remained before server teardown; its
volume was then removed. A fresh Docker inventory filtered by the exact label
`local.attune.native-workload=1ccaa2be4352` is empty for both containers and
volumes. The original runner performed teardown; resumed observation did not
kill processes, recover neighboring resources, or prune Docker.

| Gate | Final result |
| --- | --- |
| Full-payload counts, complete clean coverage, raw and independent summary oracles | Pass on both profiles and versions |
| Seven-day summary repository p95 below 10 ms | Pass on both queries and versions |
| 30/90-day summary repository p95 below 10 ms | Fail for PG16 events/status and PG18 status; PG18 events pass |
| Mixed repository p95 at most 500 ms | Fail for PG16 30/90-day events; all other mixed groups pass |
| Every first materialization attempt within one second | Fail, 28 PG16 and 16 PG18 rolled-back deadlines |
| Persisted restart catch-up under unchanged cycle/operation limits | Pass, one bounded cycle per profile |
| Every ingestion pair within both 1.10 ratios | Fail, 11 of 48 pairs |
| Producer marker and sequence correctness | Pass for all 96 samples |
| Equal-cohort expiry and hold bounds | Pass through unchanged linked evidence |
| Frozen sources and owned teardown | Pass |

Overall acceptance remains blocked. The remaining evidence calls for investigation
of the large-profile server bind tail, complete-repository summary latency,
first-pass materialization deadlines, and actual transaction-p95 ingestion misses.
There is no evidence-based reason to raise a limit, exclude a tail, or substitute
server SQL time for the repository pipeline.

The complete final artifacts are `run.json`, both `summary.json` and `server.json`
files, all JSONL samples, server statement logs, frozen sources/executable, fixture
files and hashes, and linked expiry evidence under the current-final directory.
Only this report was updated after the surviving run completed. Historical
attempts below retain their failures and are not current gate results.

## Historical optimized-production acceptance before reconciliation

The final run is `/tmp/opencode/native-workload-final-optimized`, owner
`8e95fb8ed1bd`, with scope `acceptance`. It measured the final Send reader, batched
summary protocol with custom plans, and restore-safe transaction-origin/xmin
producer coalescing. Source SHA-256 maps before build, before and after each server,
and after the run match. The frozen executable SHA-256 is
`f28bea3dcecaa998c8ca3fbbb265f30ecfc589178d7fed44b0d03df3310664e4`.
This full-payload run preceded the combined-production acceptance above. Its
measurements remain historical evidence and are not pooled with the latest run.

The environment stayed disk-backed with four CPUs, four GiB of memory, 256-MiB
shared buffers, 16-MiB work memory, JIT disabled, fsync enabled, and synchronous
commit enabled. PostgreSQL versions were 16.15 and 18.6. The event payload and
execution result still contain 32 and 64 row-dependent MD5 blocks. No thinner
fixture, larger memory allowance, or relaxed budget supplied these results.

Fixture preparation now records every `ensure_day` outcome and permits import
only after `Applied` or `AlreadyPresent`. A real blocked-parent probe on each
server returned `DeferredBusy`, and the preparation helper rejected it before
import. Both measured profiles had the expected full source counts and zero
DEFAULT event, execution-history, and audit rows. Historical source coverage
was 16 partitions per parent for seven days and 39/39/99 for the 30/90-day profile.

### Current read gates

Every numeric row below has 20 retained warm samples at four-query concurrency.
The seven-day summary rows report clean `SummaryOnly`; all four full-window
summary kinds match raw source rows and the independent fixture counter. Their
count fingerprints match both PostgreSQL versions and the earlier seven-day
baseline. Mixed tails contain 1,667 full-payload events and 2,667 history rows.

| PG | Profile | Path | Query | Server p95 ms | Repository p95 ms | Gate |
| --- | --- | --- | --- | ---: | ---: | --- |
| 16 | Seven days | Raw | Events | 73.625 | 96.593 | Pass, 500 ms |
| 16 | Seven days | Raw | Status | 69.771 | 77.832 | Pass, 500 ms |
| 16 | Seven days | Summary | Events | 1.873 | 13.712 | Server pass, repository fail |
| 16 | Seven days | Summary | Status | 2.216 | 13.788 | Server pass, repository fail |
| 16 | Seven days | Mixed | Events | 26.539 | 32.161 | Pass, 500 ms |
| 16 | Seven days | Mixed | Status | 11.475 | 46.736 | Pass, 500 ms |
| 18 | Seven days | Raw | Events | 58.968 | 70.807 | Pass, 500 ms |
| 18 | Seven days | Raw | Status | 75.655 | 87.467 | Pass, 500 ms |
| 18 | Seven days | Summary | Events | 2.039 | 14.933 | Server pass, repository fail |
| 18 | Seven days | Summary | Status | 1.315 | 11.464 | Server pass, repository fail |
| 18 | Seven days | Mixed | Events | 7.199 | 16.301 | Pass, 500 ms |
| 18 | Seven days | Mixed | Status | 7.039 | 17.975 | Pass, 500 ms |
| 16 | 30/90 days | Raw | Events | 49.513 | 55.495 | Pass, 500 ms |
| 16 | 30/90 days | Raw | Status | 73.468 | 77.899 | Pass, 500 ms |
| 18 | 30/90 days | Raw | Events | 73.105 | 88.924 | Pass, 500 ms |
| 18 | 30/90 days | Raw | Status | 64.707 | 70.617 | Pass, 500 ms |

The current clean-summary reader has five round trips, including BEGIN and COMMIT.
The standalone current-time query and savepoint RELEASE are gone. The metadata
query now returns the transaction clock with ordered coverage arrays. Server time
still sums the actual logged parse, bind, execute, and simple-protocol durations.
Repository time includes network and client work after connection acquisition.
HTTP is not measured. The current protocol-component classifier also sees
`CURRENT_TIMESTAMP` inside the coverage query, so its component is not independent
of the coverage component. Component p95s must not be added.

All four full-repository summary p95s exceed the unchanged 10-ms target. The faster
server query and the thinner candidate's timings do not establish an endpoint or
complete-repository pass in this environment.

### Current large-profile budget failures

Both full 30/90-day fixtures failed the unchanged one-second materialization
deadline on an acknowledged COMMIT. These are confirmed committed buckets, not
ambiguous commits or retries. The harness preserves the committed result and its
phase timings, separate from on-budget successful operations.

| PG | Failed kind and bucket UTC | Attempts | On-budget successes | Confirmed late commits | Total ms | COMMIT ms |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 16 | Worker status, September 6 21:00 | 40 | 39 | 1 | 5,730 | 5,725 |
| 18 | Event volume, September 6 13:00 | 7 | 6 | 1 | 1,757 | 1,703 |

PG16's setup, state/capture, delete, and replace/publish/ack phases were
1/1/0/1 ms. The failed hour was empty. PG18's same phases were 3/3/1/44 ms,
and the committed result wrote eight groups and acknowledged one notification.
Both records have `commit_acknowledged=true`, `commit_outcome_unknown=false`,
and zero serialization retries. The remaining coverage is explicitly incomplete.
There are no valid fully covered 30/90-day summary or mixed latency samples.
Those query gates are inconclusive because the materialization prerequisite failed.

### Current producer and ingestion gates

The same predeclared six alternating pairs, four writers, 100 transactions per
writer, and one or 25 actual repository calls per transaction remain in force.
All 96 samples have their expected source-row counts, all 38,400 transaction
intervals are retained, and no writer returned an error.

Transaction coalescing works. Every tracked sample produces 400 markers and
consumes exactly 400 sequence values, including the 10,000-row batch samples.
OFF samples produce none and leave the sequence at its uncalled initial value.
Every sample's `marker_checks` is true. This validates reduced metadata writes;
it does not establish the unchanged 10-percent latency and throughput gate.

The gate fails 12 of 24 PG16 pairs and 13 of 24 PG18 pairs, 25 of 48 overall.
Every execution-batch pair still fails. Both ratios are tracking divided by
baseline; elapsed ratio is equivalently baseline throughput divided by tracking
throughput. Display precision does not determine the gate.

| Source | Calls per transaction | Pair | PG16 elapsed ratio | PG16 p95 ratio | PG16 gate | PG18 elapsed ratio | PG18 p95 ratio | PG18 gate |
| --- | ---: | ---: | ---: | ---: | --- | ---: | ---: | --- |
| Event | 1 | 0 | 0.995109 | 0.995419 | Pass | 0.996955 | 1.003304 | Pass |
| Event | 1 | 1 | 1.000391 | 0.998995 | Pass | 0.988363 | 0.990127 | Pass |
| Event | 1 | 2 | 0.957786 | 0.998721 | Pass | 0.992369 | 0.999985 | Pass |
| Event | 1 | 3 | 0.996146 | 0.999154 | Pass | 0.945540 | 0.981494 | Pass |
| Event | 1 | 4 | 1.029553 | 1.008738 | Pass | 0.996455 | 0.977424 | Pass |
| Event | 1 | 5 | 0.981454 | 1.000343 | Pass | 0.976053 | 1.002173 | Pass |
| Event | 25 | 0 | 1.053901 | 1.006178 | Pass | 1.072183 | 1.197045 | Fail |
| Event | 25 | 1 | 1.092009 | 1.276591 | Fail | 1.066847 | 1.281496 | Fail |
| Event | 25 | 2 | 1.109249 | 1.055543 | Fail | 1.100890 | 1.236946 | Fail |
| Event | 25 | 3 | 1.129940 | 1.259226 | Fail | 1.120566 | 1.338958 | Fail |
| Event | 25 | 4 | 1.104608 | 1.303718 | Fail | 1.175478 | 1.176067 | Fail |
| Event | 25 | 5 | 1.118643 | 1.193037 | Fail | 1.188561 | 1.348870 | Fail |
| Execution | 1 | 0 | 0.997202 | 0.991911 | Pass | 1.003695 | 1.291314 | Fail |
| Execution | 1 | 1 | 1.047115 | 1.193604 | Fail | 0.999550 | 0.998465 | Pass |
| Execution | 1 | 2 | 1.021622 | 1.000558 | Pass | 0.975222 | 0.990998 | Pass |
| Execution | 1 | 3 | 1.044014 | 1.006871 | Pass | 1.006863 | 1.008358 | Pass |
| Execution | 1 | 4 | 1.025297 | 1.017970 | Pass | 1.013453 | 1.009052 | Pass |
| Execution | 1 | 5 | 1.063487 | 1.090952 | Pass | 1.014903 | 1.005419 | Pass |
| Execution | 25 | 0 | 1.350079 | 1.350117 | Fail | 1.412562 | 1.369045 | Fail |
| Execution | 25 | 1 | 1.393392 | 1.248694 | Fail | 1.331480 | 1.198910 | Fail |
| Execution | 25 | 2 | 1.368667 | 1.290408 | Fail | 1.424841 | 1.271702 | Fail |
| Execution | 25 | 3 | 1.336565 | 1.187867 | Fail | 1.422239 | 1.321588 | Fail |
| Execution | 25 | 4 | 1.349629 | 1.214278 | Fail | 1.191887 | 1.206169 | Fail |
| Execution | 25 | 5 | 1.315883 | 1.218127 | Fail | 1.386583 | 1.262436 | Fail |

### Separate server-phase diagnostic

`/tmp/opencode/native-workload-final-ingest-diagnostic`, owner `7c8f4f79dd41`,
uses actual `EventRepository::create` and `ExecutionRepository::create` with the
same full JSON and normal indexes. Each diagnostic transaction contains 32 calls.
PostgreSQL function statistics and nested `auto_explain` plans are enabled only
in this separate scope. Its records have `acceptance_sample=false` and cannot
replace any of the 48 acceptance pairs. Its production source fingerprints match
before and after both servers.

| PG | Tracked source | Native trigger calls | Native trigger total ms | Marker rows | Sequence values consumed |
| --- | --- | ---: | ---: | ---: | ---: |
| 16 | Event | 32 | 21.156 | 1 | 1 |
| 18 | Event | 32 | 23.442 | 1 | 1 |
| 16 | Execution | 32 | 39.915 | 1 | 1 |
| 18 | Execution | 32 | 44.230 | 1 | 1 |

The instrumentation adds overhead, so these totals are diagnostic observations,
not release latency estimates. They show that collapsing marker writes does not
remove per-statement trigger execution. Execution history invokes both status and
creation branches even when the status predicate has no input rows. Nested plans
show an index scan on `idx_native_summary_invalidation_bucket`; origin and actual
xmin ownership checks remain filters rather than all being index conditions.
Repeated duplicate checks still execute SQL. `diagnostic-plans.json` preserves
96 nested invalidation plans per version with duration, scan conditions, filters,
actual rows/loops, and buffer counts. `repository.jsonl` also separates each actual
create call from its durable commit interval and records all trigger functions.

The minimum remaining investigation is the per-call trigger/planning/ownership
lookup cost under actual batches, plus durable-commit stalls on the large fixture.
The current findings do not justify changing producer ownership semantics,
discarding sample tails, disabling fsync, or relaxing the 1.10 or one-second limits.

### Linked expiry, source stability, and final disposition

`linked-expiry.json` links the previously accepted equal 10,000/1,000,000-row
cohorts and preserves their original owner, source hashes, JSONL hashes, actual
before/after counts, WAL differences, and hold upper bounds. The script requires
byte-identical `native_partition_check` and `native_partition_expire` definitions
before linking that evidence. The long row-cleanup experiments were not repeated.

Final optimized gates remain blocked. Seven-day server summaries, raw controls,
normal-density seven-day mixed reads, producer marker/sequence correctness, and
linked expiry pass. Complete repository summary latency and ingestion fail.
Large-profile materialization misses its budget after acknowledged commits, so
its summary/mixed timing gates remain inconclusive. Both acceptance servers left
zero owned clones and sessions before teardown and zero containers/volumes after.
Source maps match before and after. No failed attempt or failed pair was retried.

```bash
python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/a-new-final-run --scope acceptance \
	--expiry-evidence /tmp/opencode/native-workload-acceptance

python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/a-new-diagnostic --scope ingest-diagnostic
```

`--scope ingest-only` runs the declared writer pairs without replaying read fixtures.
The final runner still defaults to both PostgreSQL versions. New output directories
are required, and each server retains the two-hour timeout. The diagnostic scope
does not alter acceptance aggregation or limits.

## Historical first acceptance

The earlier complete ingestion and expiry run is
`/tmp/opencode/native-workload-acceptance`, owner `82c9081ffe54`. The corrected
source-horizon and normal-density read run is
`/tmp/opencode/native-workload-correct-horizons`, owner `4711cbfbea85`.
The missing PG18 larger-profile attempt is
`/tmp/opencode/native-workload-pg18-profile30`, owner `d8f5804ec3fd`.

## Measurement contract

The owning entry points are
[`scripts/measure-native-maintenance.py`](../../scripts/measure-native-maintenance.py)
and
[`crates/common/examples/measure_native_workload.rs`](../../crates/common/examples/measure_native_workload.rs).
The script records the contract before it starts a server or collects a sample.
It freezes migration and reader source bytes, fixture SHA-256 hashes, and the
compiled example's SHA-256 hash in the evidence directory.

Each server has four CPUs, a four-GiB memory limit, disk-backed Docker storage,
256-MiB shared buffers, 16-MiB work memory, and disabled JIT. Images must already
exist locally. Servers run sequentially, with labeled containers and volumes and
ephemeral loopback ports. The runner never prunes Docker resources.

The seven-day fixture reuses the phase-1 generator unchanged. Its fixed end is
2026-10-06 12:00 UTC. It contains 140,000 live executions, 448,000 execution-history
rows, 280,003 events, 140,000 enforcements, 140,000 audit records, and 2,800 rows in
each small history table. Event payloads contain 32 row-dependent MD5 blocks.
Execution results contain 64. Retry transitions remain separate history records.

The extended fixture keeps those daily rates over 30 days for events and execution
history. It extends audit to 90 days at 20,000 rows per day. It has 600,000 live
executions, 1,920,000 execution-history rows, 1,200,003 events, 600,000 enforcements,
1,800,000 audit records, and 12,000 rows in each small history table.

`PartitionRepository::ensure_day` creates covering source partitions before either
import. The runner requires zero DEFAULT rows in all three managed sources.
Historical import suppresses only event and enforcement notification fanout.
It preserves native invalidation tracking. The deterministic live-execution import
uses the phase-1 generator's disabled execution triggers and explicit history.
Those import choices do not apply to the ingestion measurements.

## Reads and correctness

The phase-1 request includes bucket starts from 2026-10-05 12:00 through
2026-10-06 12:00 UTC. Its exact source bounds are therefore 25 hours, ending at
13:00 UTC. Both repository control and treatment use those same bounds.

`AnalyticsRepository::event_volume_hourly` and
`AnalyticsRepository::execution_status_hourly` call the production `read.rs`
planner. The control has no summary coverage. Materialization then calls
`SummaryRepository::refresh_bucket_at` for every complete retained hour and the
boundary hour. It keeps the default 250-ms lock wait, one-second operation budget,
and 10,000-notification capture cap. No budget increases hide a failed refresh.

The runner requires complete clean coverage, including empty hours. Every optimized
response must report `ReadMode::SummaryOnly` and match the raw oracle's ordered
bucket rows. It also compares all four fully grouped summary kinds across the
entire retained window against raw source rows. The Python counter independently
checks those rows against the phase-1 fixture formulas. Both PostgreSQL versions
must have identical count fingerprints.

The final mixed treatment replays the last retained source hour into the uncovered
13:00 hour. It includes at least 1,666 full-payload events and 2,666 history rows,
including creation and retry transitions. It extends the request through 14:00
UTC and requires `SummaryPlusRaw`. This is a deterministic normal-density recent
tail. It is not a measurement of a live HTTP endpoint or the wall-clock current
hour. Earlier one-record tail samples remain separately labeled in the evidence.

Each query has one first-observed sample and 20 warm samples at four-query
concurrency. Five waves release four already-connected readers together. The first
sample follows import and oracle reads, so it is not a cold-cache measurement.
The runner does not discard slow samples or repeat a rejected measurement.

PostgreSQL statement logging captures the actual repository SQL. Server time is
the sum of logged parse, bind, and execution durations for one tagged invocation,
including its transaction and maintenance-metadata statements. Repository wall
time starts after connection acquisition and includes network round trips,
decoding, merging, and commit. HTTP time is not measured. The evidence reports
these clocks separately.

The server acceptance limit is strictly below 10 ms for clean event and status
summaries and at most 500 ms for raw controls and mixed tails. Repository wall
results are also reported against those numbers, without substituting server time
for an endpoint promise. Warm p95 uses nearest rank, the 19th ordered value among
20 samples.

The earlier 57.029-ms status and 45.600-ms event measurements used bounded raw SQL
and `EXPLAIN ANALYZE`. The current no-coverage control additionally performs the
native read transaction, source lock, current-time read, savepoint, coverage and
invalidation lookup, and savepoint release. It also uses the current UTC expression
and reader query builder. These are different SQL paths and measurement clocks.
The runner does not claim that the phase-1 SQL remained unchanged.

### Observed read results

The corrected run has 16 registered partitions per source in the seven-day clone.
The 30/90-day clone has 39 event and execution-history partitions and 99 audit
partitions, including migration-created current and future coverage. It asserts
that no source has extra partitions before its declared horizon. Both profiles
have zero DEFAULT rows before measurement.

The seven-day materialization has 169 covered hours for each kind and no pending
notifications. Its fully grouped results match both raw source rows and the
independent fixture counter. All four count fingerprints match across PG16 and
PG18 and across the earlier full run and corrected read run.

Every populated row in this table represents 20 warm samples at four-query
concurrency. Mixed rows use 1,667 events and 2,667 history records in the raw tail.
Summary rows report `SummaryOnly`. Mixed rows report `SummaryPlusRaw`.

| PG | Profile | Path | Query | Server p95 ms | Repository p95 ms | Server gate |
| --- | --- | --- | --- | ---: | ---: | --- |
| 16 | Seven days | Raw control | Events | 64.029 | 78.662 | Pass |
| 16 | Seven days | Raw control | Status | 73.240 | 81.877 | Pass |
| 16 | Seven days | Summary | Events | 1.915 | 14.142 | Pass |
| 16 | Seven days | Summary | Status | 1.717 | 13.677 | Pass |
| 16 | Seven days | Mixed | Events | 6.535 | 16.606 | Pass |
| 16 | Seven days | Mixed | Status | 5.310 | 22.825 | Pass |
| 18 | Seven days | Raw control | Events | 58.896 | 76.852 | Pass |
| 18 | Seven days | Raw control | Status | 74.047 | 83.257 | Pass |
| 18 | Seven days | Summary | Events | 2.024 | 12.794 | Pass |
| 18 | Seven days | Summary | Status | 1.459 | 13.216 | Pass |
| 18 | Seven days | Mixed | Events | 8.197 | 24.822 | Pass |
| 18 | Seven days | Mixed | Status | 12.026 | 22.013 | Pass |
| 16 | 30/90 days | Raw control | Events | 43.340 | 54.080 | Pass |
| 16 | 30/90 days | Raw control | Status | 75.621 | 86.839 | Pass |
| 18 | 30/90 days | Raw control | Events | 59.539 | 74.198 | Pass |
| 18 | 30/90 days | Raw control | Status | 76.159 | 86.028 | Pass |

All four seven-day summary repository-wall p95s exceed 10 ms. The server results
pass the plan's server-time target, but they do not establish a sub-10-ms complete
repository invocation or HTTP endpoint.

The current read protocol adds BEGIN, repeatable-read setup, parent locking,
current-time selection, savepoint setup, coverage lookup with dirty/state checks,
savepoint release, and COMMIT around the row query. The corrected seven-day raw
control has these measured server components:

| PG | Query | Coverage lookup p95 ms | Read protocol p95 ms |
| --- | --- | ---: | ---: |
| 16 | Events | 6.578 | 0.467 |
| 16 | Status | 0.312 | 0.336 |
| 18 | Events | 5.826 | 0.481 |
| 18 | Status | 0.405 | 0.270 |

These include their actual parse, bind, and execution durations. Component p95s
are not additive. They isolate the observed metadata/protocol cost, not a claim
that this cost alone explains the difference from the earlier `EXPLAIN ANALYZE`
numbers. Repository wall time also pays for the separate protocol round trips.

PG16's corrected 30/90-day materialization failed at execution-status bucket
2026-09-17 18:00 UTC under the unchanged one-second operation deadline. It had
already completed 1,080 bucket operations. The server canceled the production
`INSERT ... SELECT ... GROUP BY` source aggregation. The preceding empty worker
hour took 878 ms. The evidence does not identify a single query-planning defect
from this timing alone. The 30/90-day summary and mixed gates remain inconclusive
until complete clean coverage can be built within the declared budgets.

The corrected PG18 read run completed its seven-day samples, then failed clone
cleanup before the larger profile. An autovacuum backend raced its force-drop.
The owner reported the remaining clone rather than calling cleanup successful,
and removed the owned server and volume. The follow-up
`/tmp/opencode/native-workload-pg18-profile30` ran only that missing profile.
Its event-volume refresh failed at 2026-09-08 00:00 UTC under the same one-second
budget after 146 successful bucket operations. It collected both 20-sample raw
controls, then preserved the failed refresh and explicitly cleaned its clone.
Neither version has a valid 30/90-day summary or mixed p95. Both gates are
inconclusive, with a failed materialization prerequisite.

The runner now logs and awaits owner-validated cleanup recovery. This recovery
does not retry a performance sample. The follow-up left zero clone databases and
sessions before server teardown and zero owned containers and volumes afterward.

## Equal expiry cohorts

Each expiry side gets its own migrated clone, a daily event partition containing
exactly 10,000 or 1,000,000 expired records, and one retained sentinel in the next
day. Both sides use identical source payload formulas, indexes, and partition
layout. DEFAULT is empty. Fixture loading suppresses notification fanout, then
restores it before measurement.

`RetentionRepository::run_target_bounded` calls `PartitionRepository` for whole-day
expiry. The row-cleanup control uses the same repository with
`native_maintenance.enabled=false`. Both retain the default 1,000-row batches and
100-batch cycle budget. The million-row control uses multiple normal cycles to
finish the equal cohort. It does not compare a whole partition against a subset
of a larger backlog.

Instrumentation checks actual parent counts before and after and measures the WAL
insert-LSN difference. Confirmed rows and partition counts remain separate in the
repository result. The parent-lock hold upper bound is the complete measured
repository expiry interval, including commit and follow-up cycle checks. It is
conservative, not an exact lock-sampling interval. An upper bound below one second
proves this cohort's hold target without claiming that every deployment fits it.

A separate transaction holds the real event parent read lock before an actual
`PartitionRepository::expire_before` call. Its 250-ms lock timeout must reject the
operation with zero confirmed drops. Repository wall time includes rollback and
transport, so a measured wall interval above 250 ms does not raise the configured
server lock-wait limit.

## Four-writer ingestion

There are six predeclared pairs for each of four profiles, event and execution
creation with either one or 25 repository calls per transaction. Four persistent
writers each commit 100 transactions. Each sample therefore writes 400 or 10,000
source records and preserves all 400 transaction timings. Payloads are prepared
before the timing barrier and are identical within a pair.

Every side starts in a fresh, physically cloned migrated database with normal
indexes, notification triggers, execution history triggers, and production JSON.
Only the source `native_summary_insert` triggers differ between OFF and ON.
Execution creation calls `ExecutionRepository::create` and therefore exercises
real execution history and its source invalidation. Events call
`EventRepository::create`. A 25-call transaction remains 25 actual repository
INSERT statements, not a substitute bulk INSERT.

Even pairs run OFF then ON. Odd pairs reverse that order. No other workload server
runs concurrently. Connection setup, payload generation, and clone creation stay
outside timing. Wall throughput includes all four write loops and durable commits.
Transaction p95 is nearest rank across the 400 retained intervals.

Each pair must satisfy both gates without rounding:

- Tracking elapsed time divided by baseline elapsed time is at most 1.10.
  Equivalently, baseline throughput divided by tracking throughput is at most 1.10.
- Tracking transaction p95 divided by baseline transaction p95 is at most 1.10.

Every pair must pass. Medians, negative paired overhead, or a passing single-row
profile cannot erase a failed batch pair. The older prototype failures remain in
[the protocol evidence](postgresql-native-maintenance-protocols.md).

### All paired ingestion results

The full run retains 96 samples and 38,400 transaction intervals across both
versions. Every sample has its expected source-row count and no writer error.
The failed-pair totals are 12 of 24 on PG16 and 11 of 24 on PG18.

Both ratios below are tracking divided by baseline. The elapsed ratio is also
baseline throughput divided by tracking throughput. Values displayed to six
decimal places do not determine the gate. `repository.jsonl` retains unrounded
elapsed time, rows per second, p95, and every transaction sample.

| PG | Source | Calls per transaction | Pair | Elapsed ratio | Transaction p95 ratio | Gate |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 16 | Event | 1 | 0 | 1.026638 | 1.009969 | Pass |
| 16 | Event | 1 | 1 | 0.990265 | 0.997876 | Pass |
| 16 | Event | 1 | 2 | 0.884693 | 0.834121 | Pass |
| 16 | Event | 1 | 3 | 1.221779 | 1.580880 | Fail |
| 16 | Event | 1 | 4 | 0.876850 | 0.841487 | Pass |
| 16 | Event | 1 | 5 | 0.936452 | 0.992085 | Pass |
| 16 | Event | 25 | 0 | 1.038101 | 1.272254 | Fail |
| 16 | Event | 25 | 1 | 0.993210 | 1.011114 | Pass |
| 16 | Event | 25 | 2 | 0.969300 | 0.865429 | Pass |
| 16 | Event | 25 | 3 | 1.024077 | 1.153016 | Fail |
| 16 | Event | 25 | 4 | 1.152852 | 1.289350 | Fail |
| 16 | Event | 25 | 5 | 1.000344 | 0.976947 | Pass |
| 16 | Execution | 1 | 0 | 0.999865 | 0.999975 | Pass |
| 16 | Execution | 1 | 1 | 1.063202 | 1.345570 | Fail |
| 16 | Execution | 1 | 2 | 0.991186 | 0.988775 | Pass |
| 16 | Execution | 1 | 3 | 1.027582 | 1.180652 | Fail |
| 16 | Execution | 1 | 4 | 0.992028 | 0.998203 | Pass |
| 16 | Execution | 1 | 5 | 1.019651 | 1.010428 | Pass |
| 16 | Execution | 25 | 0 | 1.514757 | 2.163445 | Fail |
| 16 | Execution | 25 | 1 | 1.476983 | 2.039567 | Fail |
| 16 | Execution | 25 | 2 | 1.196941 | 1.265805 | Fail |
| 16 | Execution | 25 | 3 | 1.165733 | 1.165328 | Fail |
| 16 | Execution | 25 | 4 | 1.314481 | 1.169074 | Fail |
| 16 | Execution | 25 | 5 | 1.434762 | 1.294721 | Fail |
| 18 | Event | 1 | 0 | 0.960999 | 0.996024 | Pass |
| 18 | Event | 1 | 1 | 0.991337 | 0.999123 | Pass |
| 18 | Event | 1 | 2 | 1.018972 | 1.005087 | Pass |
| 18 | Event | 1 | 3 | 1.091742 | 1.388137 | Fail |
| 18 | Event | 1 | 4 | 0.995397 | 0.972196 | Pass |
| 18 | Event | 1 | 5 | 0.977643 | 0.999637 | Pass |
| 18 | Event | 25 | 0 | 1.115995 | 1.362034 | Fail |
| 18 | Event | 25 | 1 | 1.144237 | 1.276846 | Fail |
| 18 | Event | 25 | 2 | 1.097341 | 1.171035 | Fail |
| 18 | Event | 25 | 3 | 1.049152 | 1.241598 | Fail |
| 18 | Event | 25 | 4 | 1.003916 | 1.004462 | Pass |
| 18 | Event | 25 | 5 | 1.019781 | 0.972365 | Pass |
| 18 | Execution | 1 | 0 | 0.990836 | 0.999040 | Pass |
| 18 | Execution | 1 | 1 | 0.985796 | 0.986320 | Pass |
| 18 | Execution | 1 | 2 | 1.009136 | 1.006723 | Pass |
| 18 | Execution | 1 | 3 | 1.000161 | 1.026442 | Pass |
| 18 | Execution | 1 | 4 | 0.995135 | 1.002957 | Pass |
| 18 | Execution | 1 | 5 | 0.986653 | 0.993492 | Pass |
| 18 | Execution | 25 | 0 | 1.211381 | 1.236437 | Fail |
| 18 | Execution | 25 | 1 | 1.233140 | 1.250473 | Fail |
| 18 | Execution | 25 | 2 | 1.173619 | 0.996002 | Fail |
| 18 | Execution | 25 | 3 | 1.153142 | 1.099951 | Fail |
| 18 | Execution | 25 | 4 | 1.225469 | 1.178735 | Fail |
| 18 | Execution | 25 | 5 | 1.203002 | 1.150367 | Fail |

PG16 execution-batch pair 0 fell from 1,577.710 to 1,041.559 rows per second.
Transaction p95 rose from 82.920 to 179.394 ms. On PG18, the batch elapsed ratios
range from 1.153142 to 1.233140. These are substantial misses, not a rounded
10-percent boundary. PG18 pair 3's p95 ratio is below 1.10, but its elapsed ratio
still fails. Negative paired overhead remains in the table and is not a speedup
claim.

The source INSERT triggers emit 400 or 10,000 event-volume notifications for an
ON event sample and the same number of execution-creation notifications for an ON
execution sample. Normal execution INSERT history has empty `changed_fields`, so
it does not emit a status notification. OFF samples emit none. This is the actual
production trigger path, unlike a 25-row bulk statement with one notification.

### Observed equal-cohort expiry

These are single cohort measurements, not expiry p95s. Each side leaves exactly
one retained row. The drop path confirms one dropped partition and zero row
deletes. The row path confirms 10,000 or 1,000,000 row deletes and zero drops.
The WAL column is the cluster insert-LSN difference over the repository interval.
It includes ordinary server background WAL generated in that interval.

| PG | Expired rows | Method | Repository ms | WAL bytes | Actual rows after | Parent hold upper bound ms |
| --- | ---: | --- | ---: | ---: | ---: | ---: |
| 16 | 10,000 | Partition drop | 121.891 | 253,728 | 1 | 121.891 |
| 16 | 10,000 | Row cleanup | 447.146 | 12,704,000 | 1 | Not measured |
| 16 | 1,000,000 | Partition drop | 220.726 | 253,744 | 1 | 220.726 |
| 16 | 1,000,000 | Row cleanup | 508,709.595 | 1,616,588,144 | 1 | Not measured |
| 18 | 10,000 | Partition drop | 79.737 | 425,288 | 1 | 79.737 |
| 18 | 10,000 | Row cleanup | 382.233 | 12,773,664 | 1 | Not measured |
| 18 | 1,000,000 | Partition drop | 216.334 | 431,376 | 1 | 216.334 |
| 18 | 1,000,000 | Row cleanup | 402,859.169 | 2,678,233,912 | 1 | Not measured |

The configured 250-ms blocked-reader limit rejected every actual expiry call with
zero confirmed drops. Repository wall intervals were 341.048 and 325.274 ms on
PG16 and 338.947 and 327.523 ms on PG18 for the two cohort sizes. These include
transaction setup, transport, and awaited rollback. Every successful drop's
conservative parent-hold upper bound is below one second.

## Actual append protocol

The example inspects the migrated source trigger catalog, transition-table names,
statement-level flags, and enabled state. It checks that the invalidation log has
no foreign key. A three-row history statement spanning creation and status emits
one notification per affected kind and hour. A three-row event statement spanning
two hours emits two notifications. Refresh and actual reads preserve distinct
NULL and empty-string dimensions and a NULL status.

The final logged probe also inserts three worker transitions in one hour and
requires one worker-status notification. Its five notifications cover all four
summary kinds. The statement log and catalog show nine installed, enabled
statement triggers with the expected transition tables and zero invalidation
foreign keys. A real uncommitted `EventRepository::create` holds the parent write
lock while actual expiry rejects at the configured 250-ms limit. The final PG18
follow-up measured 319.927 ms of repository wall time for that rejection,
including transport and rollback, with zero confirmed partition drops.

This checks the installed production migration rather than rebuilding the SQL
prototype. The separate concurrency protocol and focused repository tests remain
the evidence for late commits, builder races, and expiry snapshots.

The final driver check is `/tmp/opencode/native-workload-final-protocol`, owner
`7ef72ecd8d45`. Both PostgreSQL versions passed the protocol scope with exit code
zero, no remaining clone sessions or databases, and no remaining containers or
volumes. This check does not replace any failed performance gate.

## Reproduction and artifacts

The full run requires Python 3, Cargo, Docker, and cached `postgres:16-alpine` and
`postgres:18-alpine` images. It refuses an existing output directory.

```bash
python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/native-workload-acceptance
```

Each server can run for two hours. Use an outer timeout large enough for both
versions and compilation, or use `--background` to receive an owned PID and log
path. SIGTERM to that PID waits for its child and tears down only its labeled
resources. A rejected performance gate exits nonzero after collecting the run.

```bash
python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/native-workload-acceptance --summarize

python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/native-workload-acceptance --report

python3 scripts/measure-native-maintenance.py \
	--output /tmp/opencode/native-workload-acceptance \
	--compare /tmp/opencode/another-native-workload-run
```

`--reads-only` collects read profiles without repeating ingestion or expiry.
`--profiles 30 --versions 18` selects the missing larger profile explicitly.
`--protocol-only` checks migrated triggers and parent-writer locking with no
performance samples. Scope flags and profiles appear in `run.json` and the
example's first JSONL record. An incomplete count comparison reports matching
available fingerprints separately from complete oracle coverage and exits nonzero.

`--recover` reads an existing evidence owner's identity and removes only its exact,
stopped partial-setup container and labeled volume. It refuses a running or foreign
container. Recovery writes a separate evidence file and does not overwrite samples.

`repository.jsonl` preserves every sample, response metadata, raw oracle,
materialization outcome, expiry result, and writer error. `postgres.log` preserves
actual read statements. `read-samples-with-server-statements.json` joins both
clocks. `summary.json` derives p95s, paired gates, and count fingerprints.
`run.json`, source snapshots, fixture files, build logs, server settings, image
digests, and cleanup inventories identify the measured implementation.

The example awaits every writer and explicitly cleans every clone. The Python
owner checks remaining clone databases and sessions before server teardown.
It verifies resource labels before it stops the exact container and removes the
exact named volume, then records remaining owned resources. A migration template
belongs to that server and disappears with its owned volume.

## Historical first-acceptance gates and remaining work at that point

| Gate | Result | Evidence |
| --- | --- | --- |
| Seven-day counts and clean `SummaryOnly` | Pass | All four summaries match raw and independent fixture oracles on both versions |
| Seven-day warm summary server p95 below 10 ms | Pass | Events 1.915/2.024 ms and status 1.717/1.459 ms on PG16/18 |
| Seven-day complete repository p95 below 10 ms | Fail | Summary p95 ranges from 12.794 to 14.142 ms |
| Normal-density seven-day mixed tail at most 500 ms | Pass | All 20 samples match raw counts on each query and version |
| 30/90-day fully covered materialization | Fail | Default one-second refresh deadline cancels an actual source aggregation on both versions |
| 30/90-day summary and mixed query p95 | Inconclusive | Clean complete coverage prerequisite failed, so no valid samples were fabricated |
| Equal-cohort expiry and one-second parent hold | Pass for these cohorts | Actual counts agree, WAL differs by method, and maximum hold upper bound is 220.726 ms |
| 250-ms lock acquisition setting | Pass | Actual reader/writer blockers reject expiry with zero confirmed drops |
| At most 10-percent ingestion overhead | Fail | 23 of 48 pairs fail, including every execution-batch pair |
| Final owned resources | Removed | All recorded containers and volumes are gone; one earlier clone force-drop race remains a recorded teardown failure |

The next production work must reduce tracking cost on real repository batch
transactions enough to pass both unchanged 1.10 ratio limits. It must also make
bounded historical refresh finish a complete hour on the valid 30/90-day fixture.
The timed-out source statements are recorded in the server logs. Profiling those
statements and their I/O is needed before selecting a query or index correction.

If the intended target includes the complete repository invocation, its separate
protocol round trips also need attention. Combining setup and metadata requests
must preserve the parent-lock-before-snapshot order and maintenance-unavailable
fallback. A fast server aggregate does not fix client round-trip latency.

The owned harness now creates source-specific horizons, tests a normal-density
tail, freezes its executable for both server versions, and logs bounded,
owner-validated cleanup recovery. None of these changes raises a performance
limit or repairs production features. Production core, schema, API, configuration,
and the plan document were outside this task's edit scope.

## Earlier attempts

`/tmp/opencode/native-workload-first` preserves the initial full-seven-day request
experiment. PG16 counts matched, but that request was wider than the phase-1
24-hour benchmark. Its event and status server p95s were 2.694 and 5.694 ms for
summaries and 337.374 and 338.285 ms for raw reads. Repository summary p95s were
21.718 and 32.389 ms. The outer two-hour timeout interrupted the subsequent import.
Those results do not become samples of the corrected window.

`/tmp/opencode/native-workload-window-corrected` preserves a deliberate cancellation
during setup, before any performance sample, with zero remaining owned resources.
Neither initial attempt supplies an ingestion gate result.

The final stopped-resource audit found a never-started PG18 container from the
first timed-out attempt. Its state was `created`, PID was zero, and both resource
labels matched owner `cf84d7a7fa63`. The exact container and named volume were
removed with the new owner-scoped recovery command. Evidence is
`/tmp/opencode/native-workload-first/partial-setup-recovery-ac12a123.json`.
The runner now claims partial-creation recovery before `docker run`, and stops
processing later versions after cancellation. Final label-filtered inventories
contain zero containers and volumes. This was a startup cleanup failure, not a
passing leak test for the initial attempt.

The complete run's first larger profile created 90 days of leaves under all three
parents. Its PG16 execution-status refresh timed out at September 12 00:00 UTC,
and PG18 at September 8 01:00 UTC. Those overprovisioned-profile results are
retained as diagnostics. The corrected source horizons did not eliminate the
subsequent deadline failures. The first full run's one-record mixed tails are not
pooled with the final normal-density tail samples.
