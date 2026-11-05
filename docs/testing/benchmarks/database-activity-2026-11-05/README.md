# Database activity and individual execution completion measurements

This campaign compares the original worker, the worker lifecycle changes, and
the first database activity reductions on fresh disposable full-stack instances.
The comparison separates database work from time to completion: lower query
counts alone do not establish a latency improvement.

> **Idle baseline correction (2026-11-05):** The E2E runner sets persisted
> retention to **2 seconds**, not the normal 3,600 seconds. All six runs confirm
> that accelerated cadence. The reported idle counts describe this test setup;
> they do not establish production-default supervisor load or hotspot ranking.
> See the [idle source analysis](../../../plans/idle-database-activity-analysis.md)
> and `idle-follow-up.json`. Matched execution comparisons remain observations
> under identical accelerated maintenance, rather than normal-cadence evidence.

## Results

**The combined changes reduced database calls and median completion time in all 24 matched profiles.** The first database-specific slice reduced top-level SQL calls in all 24 profiles beyond the worker checkpoint, but its incremental latency/tail and WAL results are mixed. These are measured comparisons on this instrumented host, not a general production speedup guarantee.

| Transport | Checkpoint | Top-level SQL/job | Nested SQL/job | Top-level DB ms/job | Top-level WAL bytes/job | Idle top-level SQL/min |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| Shared file | Original | 1,148.7 | 446.0 | 673.3 | 74,513.3 | 9,549.4 |
| Shared file | Worker | 595.0 | 161.4 | 359.9 | 58,232.4 | 9,274.5 |
| Shared file | Database | 540.6 | 160.8 | 373.4 | 59,686.5 | 8,812.6 |
| API / object segments | Original | 1,157.3 | 309.3 | 133.1 | 77,529.6 | 9,531.0 |
| API / object segments | Worker | 659.5 | 139.1 | 106.7 | 65,197.2 | 9,276.1 |
| API / object segments | Database | 602.2 | 136.1 | 96.3 | 65,011.7 | 9,337.3 |

Each cell averages the same 12 profiles, with 100 samples/profile. Whole-stack SQL/time/WAL are normalized by jobs; shorter profile duration also reduces background work observed in that window. The worker-attributed SQLx event counts in `profiles.csv` provide a narrower, partially covered cross-check.

- Original → database: **52.9% fewer top-level calls/job with shared files; 48.0% fewer with API segments**. Durable-terminal p50 improves in 24/24 profiles; p95 improves in 22/24. Across profiles, the median paired p50 change is −36.9% / −29.5% for shared-file / API transports. This is the median of paired profile changes, not a pooled latency statistic.
- Worker → database: **9.1% / 8.7% fewer top-level calls/job**. Durable-terminal p50 improves in 8/12 shared-file profiles and 12/12 API profiles. Median paired p50 changes are −4.8% / −6.9%. Four shared-file output-heavy profiles are slower, by up to 6.8%.
- Incremental p95 improves in only 5/12 shared-file and 6/12 API profiles. The API serial stderr-failure p95 is **695.7 ms versus 396.1 ms** at the worker checkpoint (+75.7%). The combined original → current comparison also has two worse p95 cells: shared-file both-streams C4 (2,841.9 versus 2,760.0 ms), and API serial stderr failure (695.7 versus 596.3 ms). These are retained observations; independent repeated trials are needed to establish their cause.
- Top-level PostgreSQL execution time beyond the worker checkpoint changes **+3.7% for shared files and −9.7% for API segments**. In the shared-file runs, the normalized two-key advisory-lock family accounts for 259.7 → 275.8 DB ms/job at the worker → database checkpoints. The same normalized family can cover multiple lock namespaces; server totals alone do not identify one lock owner. Fewer SQL statements have not produced a consistent reduction in server time.
- Incremental top-level WAL changes **+2.5% / −0.3%**. The read-focused database slice does not establish a workload WAL improvement. Combined original → current WAL is lower by 19.9% / 16.1%, largely from the worker lifecycle changes.
- Idle top-level SQL remains approximately **8,800–9,500 calls/minute**. The shared-file idle window improves while API idle is essentially unchanged; one window per phase is insufficient to claim an idle-rate improvement. The count-only queue change primarily avoids materializing discarded rows, and eight empty queues do not quantify its benefit for populated dispatches.

The latency details below are durable-terminal **p50 / p95 in milliseconds**. C1/C4 indicate submission batch concurrency. Individual observations remain in `samples.csv`; MQ and p99 measurements remain in `profiles.csv`.

### Shared file

| Action | Concurrency | Original | Worker | Database |
| --- | ---: | ---: | ---: | ---: |
| echo | 1 | 753.5 / 958.5 | 497.0 / 624.8 | 473.9 / 635.6 |
| echo | 4 | 2,107.3 / 2,486.0 | 1,401.1 / 1,712.0 | 1,332.7 / 1,756.1 |
| silent_success | 1 | 652.5 / 929.8 | 191.1 / 250.6 | 173.0 / 211.1 |
| silent_success | 4 | 1,796.6 / 2,198.5 | 445.8 / 600.5 | 419.3 / 524.4 |
| silent_failure | 1 | 680.5 / 782.7 | 193.1 / 260.3 | 174.0 / 268.3 |
| silent_failure | 4 | 1,856.5 / 2,292.8 | 438.7 / 657.5 | 408.7 / 531.8 |
| stderr_failure | 1 | 793.1 / 993.4 | 474.0 / 617.3 | 490.1 / 749.3 |
| stderr_failure | 4 | 2,265.8 / 2,719.4 | 1,382.2 / 1,662.2 | 1,442.6 / 1,782.2 |
| both | 1 | 773.0 / 934.3 | 679.4 / 852.7 | 703.9 / 918.3 |
| both | 4 | 2,363.3 / 2,760.0 | 2,187.0 / 2,806.2 | 2,334.6 / 2,841.9 |
| json_stdout | 1 | 798.3 / 970.3 | 596.8 / 736.9 | 549.8 / 724.6 |
| json_stdout | 4 | 2,236.9 / 2,929.6 | 1,678.6 / 2,081.6 | 1,603.3 / 2,058.2 |

### API / object segments

| Action | Concurrency | Original | Worker | Database |
| --- | ---: | ---: | ---: | ---: |
| echo | 1 | 495.2 / 626.8 | 335.0 / 425.9 | 329.0 / 433.4 |
| echo | 4 | 928.1 / 1,150.5 | 698.6 / 817.4 | 696.7 / 995.6 |
| silent_success | 1 | 369.9 / 505.9 | 200.6 / 281.5 | 173.9 / 253.3 |
| silent_success | 4 | 717.8 / 915.1 | 451.5 / 540.7 | 421.0 / 574.2 |
| silent_failure | 1 | 360.6 / 846.0 | 196.1 / 262.8 | 173.5 / 314.4 |
| silent_failure | 4 | 717.2 / 841.1 | 463.9 / 726.2 | 431.3 / 625.2 |
| stderr_failure | 1 | 425.3 / 596.3 | 314.3 / 396.1 | 294.9 / 695.7 |
| stderr_failure | 4 | 854.9 / 1,439.7 | 732.1 / 1,497.0 | 660.1 / 824.5 |
| both | 1 | 445.7 / 585.6 | 338.6 / 454.0 | 319.5 / 416.9 |
| both | 4 | 940.4 / 1,255.4 | 822.5 / 997.1 | 800.9 / 1,055.1 |
| json_stdout | 1 | 473.8 / 653.9 | 383.3 / 516.9 | 348.4 / 458.5 |
| json_stdout | 4 | 1,054.3 / 1,222.6 | 966.4 / 1,347.2 | 853.5 / 1,161.4 |

## What remains expensive

All retained service logs start before the idle window. The database checkpoint's
SQLx idle events are attributed as follows; these client-event counts have
different coverage from PostgreSQL top-level counters and must not be summed as
an exact reconciliation of server calls.

| Service | Shared-file events/min | API-segment events/min |
| --- | ---: | ---: |
| Supervisor | 4,859.6 | 5,300.8 |
| Both executors combined | 2,468.8 | 2,469.5 |
| API | 125.9 | 127.9 |
| Four workers combined | 24.0 | 24.0 |
| Notifier, with no subscribed clients | 0 observed | 0 observed |
| Sensor | SQLx attribution unavailable | SQLx attribution unavailable |

The executor workflow-log outbox candidate statement alone executes **1,170 times
in each one-minute database-checkpoint idle window**. Supervisor configuration
initialization and maintenance admission also recur with no submitted work.
The outbox pattern supports idle dispatch work. The supervisor ranking must
be reassessed at normal retention cadence; accelerated maintenance inflates its
share. See the follow-up correction above. They do not justify weakening delivery
fences, retention independence, or admission ownership.

Worker → database execution-scoped SQLx events decrease by approximately **18
per job** in both transports, across all 24 profiles. Every measured job at both
checkpoints has attributed events. The average changes are 166.7 → 148.7 events/job
for shared files and 114.2 → 96.2 for API segments. This narrower cross-check
supports a real execution-path reduction beyond changes in background work.
The original worker lacks the required execution spans: its correlated client
counts are **unavailable**, represented by empty CSV cells, rather than zero
queries. SQLx event counts still are not protocol exchange counts.

Observed API creation graph holders improve at p50 from 94.5 → 83.7 ms with
shared files and 92.2 → 81.3 ms with API segments. Shared-file log-seal holders
move the other way: p50 163.8 → 174.2 ms, p95 551.8 → 619.3 ms. API-segment seal
holders are much shorter (database checkpoint p50 27.8 ms, p95 75.1 ms).
The number of observed seal transactions falls from 2,400 per original matrix
to 1,000 at both later checkpoints, matching lazy stream allocation. These
owner traces cover API creation/sealing only, not a census of every database
lock holder. Recorded unknown drops are zero within this observed coverage.
Pool-acquisition p95 is below 0.4 ms for each instrumented service with events;
it does not appear to dominate these runs. Other advisory-lock queries include
longer waits; their elapsed client/server time must not be described as pool wait.

## Default budget and collection audit

The separate **1,000 ms** database-checkpoint probes are not included in the
primary tables:

- Shared files: serial echo passes its 20 samples; concurrency-four echo reaches
  20 completions, then fails `measured output must be sealed and ready`. One
  worker capture-settlement phase lasts 1,001.2 ms. Its failed-profile latencies
  were not emitted before the assertion; only the preceding validated profile
  is retained as a partial report. This is an unresolved capture-completeness
  failure under DEBUG instrumentation. No matched worker-checkpoint default
  run was performed in this campaign, so this does not establish which change
  introduced it.
- API segments: all 12 profiles / 240 measured executions pass, followed by both
  reader probes. This small run does not establish default-budget reliability
  for other loads, transports or hosts.

An initial original/shared-file pilot completed its matrix but was rejected
because the post-run provenance validator required a Git SHA that the original
API reports as `unknown`. That report is preserved and excluded. The validator
was corrected to require the archived exact identities of all seven runtime
images, then the original/shared-file matrix was rerun on fresh infrastructure.
Acceptance did not depend on the observed performance. All nine campaign stacks
(the rejected pilot, six primary runs and two default probes) have verified
owned-resource teardown. See `collection-audit.json` and `default-budget.json`.

## Checkpoints and method

| Label | Runtime revision | Meaning |
| --- | --- | --- |
| Original (`legacy`) | `6906b96846eb83299cfb9b8a740ccf8854b25e8e` | Before worker lifecycle optimizations |
| Worker (`before`) | `ece3c859dc92709c38aa0fabf1040d77c168f07b` | Validated worker treatment; runtime source/build inputs identical to worker PR head `6dc75148` |
| Database (`after`) | `747832c97e09cbc122f72af1304e66375e3dde0e` | Worker treatment plus count-only queue capacity and single-validation input authority reads |

The primary matrix has six short actions (`core.echo`, silent success, silent
failure, stderr failure, both streams, and JSON stdout), each at submission
concurrency one and four, with 100 measured executions per profile. The same
matrix runs for shared-file transport and API/object-segment transport. The
object backend is a local filesystem; these are not S3 measurements. A profile
submits a batch and waits for all its completions before submitting the next
batch. Concurrency four therefore describes batch concurrency, not a fixed
arrival rate or a saturated throughput test.

Each action has one first invocation and 16 warmup executions excluded from the
measured samples. Warm-profile worker assignments must be covered by warmup.
First invocations remain in the evidence and are not described as globally cold:
stack startup and other actions can already have warmed shared caches.

Each phase/transport uses a fresh owned database, broker, pack/artifact volumes
and service stack. There are four workers, two executors, an API, notifier,
supervisor and sensor. Eight enabled empty ad hoc queues exercise queue capacity
observations. A 60-second settling interval precedes a 60-second idle window.
Services continue background activity during execution profiles.

The host has an Intel Core i7-4790K, eight logical CPUs, and Docker Desktop
29.9.2 with eight vCPUs and 16,765,710,336 bytes of memory. PostgreSQL is 18.6,
with `fsync=on`, `synchronous_commit=on`, `shared_buffers=16384` blocks, and
`max_connections=100`. Runtime compilation finishes before collection. Stacks
run sequentially; this is one instrumented run per checkpoint/transport, not
independent repeated trials or an isolated production host.

Identical SQLx DEBUG/graph tracing and collector instrumentation are used across
phases. `pg_stat_statements.track=all` distinguishes top-level from nested SQL;
its capacity is 50,000 entries. Each idle/profile window checks that the
statement counter reset/eviction epoch is unchanged. `runs.json` records exact
collector hashes, runtime image identities, running agent binary digests,
observer dependencies, PostgreSQL settings, source revisions and verified
cleanup. Original API builds report Git SHA `unknown`; their seven runtime image
identities are instead checked against the archived validated baseline. The
other checkpoints require exact API Git SHA and image identities.
`runtime-source-equivalence.json` records matching Git objects for the worker
checkpoint and final worker PR runtime/build inputs, with absent paths explicit.

All primary runs use the same **10,000 ms log-finalization budget**. Separate
current-checkpoint probes use the **1,000 ms default** with 20 samples/profile;
they are not pooled into primary comparisons. No production timeout was changed.

## What the measurements mean

- Durable terminal latency measures the monotonic interval from the HTTP
  execution request until the observer receives the PostgreSQL terminal-state
  notification. It includes scheduling, execution, log finalization and delivery
  to the observer; it is not just subprocess runtime. Each submission uses a
  fresh HTTP session, consistently across checkpoints, so client connection
  setup/authentication are included.
- MQ completion latency uses a continuously running observer with 10 ms
  RabbitMQ management polling. It is observed delivery time, not exact publish
  time. The original worker emits incorrect success statuses for failed action
  completion messages. That original defect is retained and counted separately;
  durable database statuses must still match the expected action outcomes.
- SQL counters and PostgreSQL execution time are whole-stack window deltas,
  normalized by completed jobs. They include background work and identified
  observer statements. They are not attributed worker round trips. PostgreSQL
  execution time includes waits and excludes untracked planning time. Nested
  time and WAL can overlap parent accounting, so they must not be added as
  independent elapsed cost.
- Execution-scoped SQLx events provide narrower worker attribution where spans
  are present. Driver catalog discovery and nested client timing events can
  appear in these counts. They are not network exchange counts. Sensor SQLx
  DEBUG is not enabled, so sensor attribution is absent; its database work is
  still in whole-stack counters.
- Graph holder timings cover the transaction owner's first shared admission
  through acknowledged completion. They include SQL, I/O and scheduling gaps.
  Concurrent holders overlap. Their summed hold time is not elapsed wall time,
  and only completed/observed owners are covered. Unknown owner drops remain
  explicit in `graph-transactions.csv`.

The worker/database checkpoints also assert exact lazy stream allocations,
sealed/ready output, complete captures and JSON stdout materialization.
The original checkpoint records 400 incorrect MQ failure statuses per transport
(800 total); the worker/database checkpoints record zero. Durable statuses match
expected outcomes throughout. Subscription-before-first-output reader probes run
after each measured matrix,
so they do not enter the profile windows.

## Reproduction and evidence

The collector is
[`tests/e2e/api/test_worker_lifecycle_performance.py`](../../../../tests/e2e/api/test_worker_lifecycle_performance.py)
with
[`docker-compose.worker-lifecycle-benchmark.yaml`](../../../../docker-compose.worker-lifecycle-benchmark.yaml).
Use the owned integration runner on a frozen checkout for each revision, copying
identical test instrumentation/Compose overlay into the frozen baseline checkouts
without changing their runtime sources. Record the overlay separately from the
runtime revision. A single current-checkpoint collection uses:

```bash
ATTUNE_E2E_RUN_ID=<unique-owned-id> \
ATTUNE_E2E_PROJECT_NAME=attune-<unique-owned-id> \
ATTUNE_E2E_COMPOSE_OVERRIDE="$PWD/docker-compose.worker-lifecycle-benchmark.yaml" \
ATTUNE_WORKER_BENCHMARK_REVISION="$(git rev-parse HEAD)" \
ATTUNE_WORKER_BENCHMARK_SAMPLES=100 \
ATTUNE_WORKER_BENCHMARK_TRANSPORT=volume \
ATTUNE_WORKER_BENCHMARK_EXPECT_LAZY=1 \
ATTUNE_WORKER_BENCHMARK_LOG_FINALIZATION_MS=10000 \
bash scripts/run-integration-tests.sh -k test_worker_lifecycle_profile -m performance -s
```

Use `api` for the second transport. `EXPECT_LAZY=0` is reserved for the original
baseline's eager allocations and known MQ status defect. Each complete report
is emitted as `WORKER_LIFECYCLE_REPORT=<JSON>`; partial reports are separately
marked. Do not accept partial reports as a complete comparison. The campaign
additionally captures service logs and provenance before verifying removal of
all owned containers, volumes and networks.

Raw reports, service logs, source snapshots, manifests and campaign drivers are
retained locally under `.scratch/database-measurements/`. They are intentionally
not committed as multi-gigabyte build/log artifacts. The compact committed
exports retain all individual measured samples and per-profile counters, along
with provenance and the leading query fingerprints. To regenerate them from
completed campaign folders:

```bash
python3 scripts/analyze-worker-lifecycle-benchmark.py \
  .scratch/database-measurements/reports/<accepted-run-directory> ... \
  --output <export-directory>
```

The exporter rejects collector mismatches, incomplete matrices, duplicate
execution IDs, inconsistent counters and unverified cleanup. Service-log parsing
runs after all timing collection, to avoid analysis load contaminating later
measurements. The CSV files use nearest-rank percentiles. `samples.csv` preserves
individual latencies; `profiles.csv` preserves per-profile counters and
percentiles; `phases.csv` contains available worker phase distributions (some phase timers
overlap and must not be summed as disjoint work);
`graph-transactions.csv` contains observed holder timing by service/operation;
`runs.json` contains provenance, idle measurements, pool-acquisition/advisory
query client timings and leading query families.

This does not measure workflow parent/child graphs, queued item throughput,
accepted human inputs, permission-heavy actions, long output streams, real S3,
lock-holder concurrency over time, or production workload ranking. One minute of
idle observations and 100 warm samples per profile establish an exploratory
comparison, not a capacity/SLA claim. p95 differences have limited tail evidence;
run-order and host effects remain possible.


## Validation handoff

Status: **work in progress** for merge readiness and the broader reduction plan;
the requested measurement campaign is complete.

Validation: six complete primary E2E matrices at the runtime revisions above
with identical collector hash `c47a10e5cc46b9b6cfeaa6ac4f8e4d69f0934b739e97230aa00cb068a08da7af`,
72 profiles / 7,200 samples, and 12 post-matrix reader cases pass. The separate
API default matrix (240 samples) and its two reader cases pass. Image/collector
parity, counter consistency, recorded counter-epoch checks and cleanup are
verified. The exporter passes Python syntax checks, synthetic percentile/span/
missing-coverage/graph-timing checks, manifest-rejection checks, and complete
six-run data export. HTML preview reports no console errors. `git diff --check`
passes. The exported raw-sample percentiles and counter normalization are
independently reconciled against the accepted reports.

The implementation's earlier 330 scoped Rust cases, workspace all-target check,
Clippy and formatting evidence at code `94aa4b6d` remain applicable: subsequent
runtime edits through `747832c9` only clarify a documentation comment, and this
measurement work changes instrumentation, export tooling and documentation.
See the [plan's validation record](../../../plans/database-activity-reduction.md#validation-record)
for exact commands and reuse rationale. No schema or public API change is made
by the measurement work.

Gaps: default shared-file capture fails as recorded above; its introduction is
unresolved. Required CI and unfiltered workspace/full-stack suites have not run
for this local branch. Independent repeated trials, real S3, workflow/permission-
heavy workloads and full lock-holder occupancy remain unmeasured. The measured
reductions do not close the remaining implementation opportunities in the plan.
