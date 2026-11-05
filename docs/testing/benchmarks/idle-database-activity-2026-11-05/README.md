# Normal-cadence idle activity and execution measurements

Date: 2026-11-05. Matched baseline/treatment stacks; this follows the corrected interpretation in [the idle analysis](../../../plans/idle-database-activity-analysis.md).

These runs use persisted deployment defaults: hourly retention, five-minute summary maintenance, normal cache retention, two executors, four workers and eight enabled empty queues. Each fresh stack settles for 60 seconds, then records six minutes of idle activity crossing a summary deadline. The active matrix has six short actions at batch concurrency one and four, with 100 measured jobs/profile. Output, capture completeness, terminal status, counter epochs and warmed-worker assertions are unchanged.

The two transports are shared files and API/object segments backed by a local filesystem. Both use the same DEBUG instrumentation and 10-second capture-finalization budget. The primary comparison is one matrix per revision/transport. An independent shared-file pair reverses the initial run order; both trials are retained. These are not capacity, CPU, production-SLA or S3 measurements. Do not compare these normal-cadence totals directly with the earlier accelerated two-second-retention campaign.

The stable result is a **67% reduction in idle top-level SQL**, from about 3,400 to 1,110 statements/minute in all three matched comparisons. Empty workflow-log claims fall from about 1,170 to 26/minute; empty-queue capacity/history reads disappear and are replaced by 24 readiness snapshots/minute. Retention target seeding falls from 16 statements/minute to one.

Completion latency remains inconclusive. The initial shared-file pair has median paired p50/p95 changes of +0.9%/+8.3%; the independent reversed-order pair has −3.4%/−6.0%. API/object segments have −0.1%/+3.4%. The shared-file tail direction does not repeat. These data support idle-work reduction, with no established general completion-speed or tail-latency improvement. Idle DB time is only about 0.12–0.26 aggregate seconds/minute across these observations; it is not evidence of saturation. Active SQL/job is modestly lower and WAL is essentially unchanged.

## Idle rates

| Transport | Top-level SQL/min, baseline → current | DB execution ms/min, baseline → current | Top-level WAL bytes/min, baseline → current |
| --- | ---: | ---: | ---: |
| Shared file | 3,398.2 → 1,111.5 (-67.3%) | 147.0 → 118.9 (-19.1%) | 85,520.2 → 85,187.0 (-0.4%) |
| API / object segments | 3,377.4 → 1,108.0 (-67.2%) | 245.3 → 131.5 (-46.4%) | 85,282.0 → 86,978.8 (+2.0%) |

| Transport | Counter-query DB ms/min, baseline → current | Remaining top-level DB ms/min, baseline → current |
| --- | ---: | ---: |
| Shared file | 4.2 → 7.0 | 142.7 → 111.9 |
| API / object segments | 5.8 → 8.1 | 239.5 → 123.4 |

The collector tags statements containing `pg_stat_statements` as counter observation. The second table removes only those tagged statements; API/caller activity and other validation reads remain included. Both raw and adjusted totals are retained.

### Recurring query families

| Top-level calls/min | Shared baseline | Shared current | API baseline | API current |
| --- | ---: | ---: | ---: | ---: |
| Workflow-log claims | 1,174.3 | 27.0 | 1,166.3 | 25.7 |
| Per-queue capacity counts | 192.0 | 0.0 | 189.3 | 0.0 |
| Latest terminal dispatch | 192.0 | 0.0 | 189.3 | 0.0 |
| Queue readiness hints | 0.0 | 24.0 | 0.0 | 24.0 |
| Retention target initialization | 16.0 | 1.0 | 16.0 | 1.0 |
| Inquiry timeout updates | 180.0 | 180.0 | 180.1 | 180.1 |
| Workflow-wait reconciliation | 120.0 | 120.0 | 120.1 | 120.1 |
| Inquiry callback claims | 60.0 | 60.0 | 60.0 | 60.0 |
| Connection initialization (proxy) | 1.2 | 0.2 | 1.2 | 0.2 |
| Other top-level work | 1,462.6 | 699.2 | 1,454.9 | 696.7 |

Families use normalized SQL-text patterns; `idle-statements.csv` retains the complete source statements. Other top-level work includes unclassified statements, and the separately shown counter observer is retained in `idle-families.csv`. Connection-initialization statements are a proxy, not a physical connection census. Each updated executor can also hold one additional listener backend: two at this topology. Backend memory and CPU utilization are not measured.


These are aggregate PostgreSQL execution times, including waits and the observation workload. They are not CPU utilization or elapsed application latency. Top-level and nested times overlap and are never added together. Query-family details and observer counts are retained in `idle-statements.csv` and `runs.json`.

## Active workload

| Transport | Mean top-level SQL/job, baseline → current | Median paired p50 change | Median paired p95 change | Profiles with lower p50 / lower p95 |
| --- | ---: | ---: | ---: | ---: |
| Shared file | 498.8 → 485.2 | +0.9% | +8.3% | 6/12 / 3/12 |
| API / object segments | 571.7 → 561.2 | -0.1% | +3.4% | 7/12 / 5/12 |

SQL/job includes background activity during the counter window. Shorter profiles observe less background work; this is not a worker-only SQL attribution. Median paired changes are medians of twelve profile changes, not pooled latency estimates. Single matrices and 100 samples/profile do not establish stable tail improvements.

### Shared file

Durable-terminal p50 / p95 in milliseconds; request to observed PostgreSQL completion.

| Action | Batch concurrency | Baseline p50 / p95 | Current p50 / p95 |
| --- | ---: | ---: | ---: |
| echo | 1 | 467.9 / 572.1 | 489.2 / 589.2 |
| echo | 4 | 1288.1 / 1629.1 | 1415.6 / 1790.3 |
| silent_success | 1 | 179.0 / 212.0 | 170.6 / 273.5 |
| silent_success | 4 | 408.3 / 499.1 | 397.3 / 482.9 |
| silent_failure | 1 | 183.6 / 242.1 | 173.8 / 262.3 |
| silent_failure | 4 | 420.0 / 494.7 | 404.2 / 530.3 |
| stderr_failure | 1 | 427.4 / 571.9 | 461.1 / 625.3 |
| stderr_failure | 4 | 1233.4 / 1539.2 | 1465.7 / 1764.8 |
| both | 1 | 593.6 / 796.4 | 549.8 / 793.0 |
| both | 4 | 1948.6 / 2366.8 | 1713.7 / 2269.2 |
| json_stdout | 1 | 481.9 / 590.5 | 519.0 / 645.3 |
| json_stdout | 4 | 1393.8 / 1740.8 | 1458.8 / 1882.3 |

### API / object segments

Durable-terminal p50 / p95 in milliseconds; request to observed PostgreSQL completion.

| Action | Batch concurrency | Baseline p50 / p95 | Current p50 / p95 |
| --- | ---: | ---: | ---: |
| echo | 1 | 313.1 / 396.8 | 306.7 / 422.1 |
| echo | 4 | 657.7 / 928.3 | 656.8 / 840.5 |
| silent_success | 1 | 171.5 / 232.0 | 170.4 / 261.4 |
| silent_success | 4 | 407.2 / 529.7 | 402.3 / 546.8 |
| silent_failure | 1 | 170.3 / 190.4 | 170.2 / 221.4 |
| silent_failure | 4 | 398.5 / 686.2 | 416.9 / 552.6 |
| stderr_failure | 1 | 289.5 / 384.3 | 286.1 / 365.5 |
| stderr_failure | 4 | 636.4 / 748.7 | 635.3 / 814.5 |
| both | 1 | 307.9 / 418.1 | 310.5 / 410.1 |
| both | 4 | 768.3 / 960.9 | 775.1 / 1080.6 |
| json_stdout | 1 | 338.4 / 465.2 | 360.9 / 481.5 |
| json_stdout | 4 | 844.3 / 1133.4 | 851.7 / 1088.1 |

## Provenance and limits

| Run | Runtime revision | Samples |
| --- | --- | ---: |
| baseline / volume / pair 1 | `747832c97e09cbc122f72af1304e66375e3dde0e` | 1,200 |
| current / volume / pair 1 | `0c37e1dd8792de0ec93bf7ed397e674a2cb4def8` | 1,200 |
| baseline / api / pair 1 | `747832c97e09cbc122f72af1304e66375e3dde0e` | 1,200 |
| current / api / pair 1 | `0c37e1dd8792de0ec93bf7ed397e674a2cb4def8` | 1,200 |
| current / volume / pair 2 | `0c37e1dd8792de0ec93bf7ed397e674a2cb4def8` | 1,200 |
| baseline / volume / pair 2 | `747832c97e09cbc122f72af1304e66375e3dde0e` | 1,200 |

Baseline images report `747832c9`; their runtime source and build definitions match merged main `4846e607`. Its actual build identity remains 747832c9; the complete intervening changed-path list contains tests, instrumentation and documentation only. All seven runtime image identities are checked against the archived validated baseline. Treatment images are built from the recorded frozen commit. Each run verifies image identities, API build identity, collector hash, PostgreSQL settings and the running agent binary. Actual retention configuration and maintenance due/success/deferred timestamps are retained in `runs.json`. Every stack has independent verified container, network and volume teardown.

Connection reuse is proven per owner by backend-PID and cancellation tests. These collectors do not directly count physical connection establishments; connection-setup statement families are proxies, not an exact connection census.

The earlier default-budget shared-file DEBUG stress failure is not waived by the longer measurement budget. Functional E2E at the default budget is recorded separately in the PR validation handoff. These matrices do not measure sustained business-queue throughput or many-replica notification traffic. Repository/service tests cover readiness/admission/recovery and notification loss, retry/lease deadlines, burst disposal, cancellation, ordering and exactly-once append/seal effects.

Files: [interactive comparison](comparison.html), [profile counters](profiles.csv), [individual samples](samples.csv), [idle statements](idle-statements.csv), [run/configuration metadata](runs.json), and [paired comparison statistics](comparison.json). Raw owned-stack logs and complete reports remain in the local `.scratch/idle-readiness/` evidence directory.

## Independent shared-file repeat

The initial shared-file pair has mixed latency and worse p95 in nine of twelve profiles. A second complete pair uses the same twelve profiles, 100 samples each, six-minute idle and 60-second settling windows, capture budget, collector and images, in current → baseline order. It is an independent experiment, not a replacement or hidden retry; all observations are retained. Primary tables above describe the initial pair.

| Measure | Initial pair | Independent repeat |
| --- | ---: | ---: |
| Median paired p50 change | +0.9% | -3.4% |
| Median paired p95 change | +8.3% | -6.0% |
| Idle top-level SQL/min change | -67.3% | -67.2% |
| Idle top-level DB execution time change | -19.1% | -54.5% |

Use the interactive comparison’s run-pair selector to inspect every repeated profile. CSV rows and run metadata include `trial` and retain separate 100-sample groups. Percentiles are not pooled across trials. The repeat decision and order are documented in `repeat-design.json`.

## Reproducing a matrix

Use the owned runner on each frozen revision, with identical instrumentation and settings. For example, after choosing the tested revision and transport:

```bash
ATTUNE_BUILD_GIT_SHA="$(git rev-parse HEAD)" \
ATTUNE_WORKER_BENCHMARK_REVISION="$(git rev-parse HEAD)" \
ATTUNE_E2E_COMPOSE_OVERRIDE="$PWD/docker-compose.worker-lifecycle-benchmark.yaml" \
ATTUNE_DOCKER_CONFIG_PATH="$PWD/docker/distributable/config.docker.yaml" \
ATTUNE_DOCKER_LOG_MAX_SIZE=200m \
ATTUNE_E2E_RETENTION_PROFILE=default \
ATTUNE_WORKER_BENCHMARK_TRANSPORT=volume \
ATTUNE_WORKER_BENCHMARK_EXPECT_LAZY=1 \
ATTUNE_WORKER_BENCHMARK_SAMPLES=100 \
ATTUNE_WORKER_BENCHMARK_IDLE_SECONDS=360 \
ATTUNE_WORKER_BENCHMARK_SETTLE_SECONDS=60 \
ATTUNE_WORKER_BENCHMARK_LOG_FINALIZATION_MS=10000 \
bash scripts/run-integration-tests.sh -k test_worker_lifecycle_profile -m performance -s
```

Use `api` for the API/object-segment transport. Build and startup are outside the timed windows. The runner owns fresh resources and teardown; capture stdout for the emitted complete report. The retained campaign also runs subscription-first reader probes and verifies runtime/base images, dependency versions, running agent digests and cleanup. A collector/image/configuration mismatch, incomplete matrix, invalid counters, failed output checks or startup cleanup overlapping idle invalidates a comparison.

## Validation scope

The runtime source `0c37e1dd` has all fourteen CI checks passing, including the workspace test job and full-stack smoke. Final PR-head CI is a separate merge gate. Local scoped Rust validation has 157 passing cases, workspace/all-target checking and Clippy with warnings denied. See `validation-provenance.json` for source/result reuse and the implementation analysis for commands. Standard deferred repository coverage remains deferred, not counted as passing by these measurements.

The supplemental default-budget 35-case selection returns **32 passed / 3 failed on both baseline and treatment**. The three inquiry creators stop at the identical existing recorded-credential readiness guard; no guard, test assertion or ignore status is changed. `known-baseline-inquiry.json` preserves the exact failures, source identities and cleanup proof. These results do not certify inquiry E2E deployment support. Musl linking repeats an archived baseline deprecation warning, documented in `baseline-linker-warning.json`; native workspace checking and Clippy are warning-free.

`collection-audit.json` retains all six accepted matrices, their report/manifest digests, reader and cleanup results, and the separate correctness failures. No measurement run is discarded and no faster trial is selected. An earlier stopped image build produced no measurement data. The actual baseline build identity stays `747832c9`; `baseline-source-equivalence.json` records why its executable source and build definitions match merged main `4846e607` despite different provenance metadata. Every benchmark stack and the native fixture have verified owned-resource teardown.
