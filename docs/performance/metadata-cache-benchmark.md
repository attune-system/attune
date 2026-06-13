# Metadata cache benchmark

This benchmark compares Attune with the optional Valkey/Redis-backed `metadata_cache` **enabled** vs **disabled** under the same Docker Compose topology.

Cache-on runs now include explicit cache-engagement checks in the report. The comparison separates process-local L1 counters, Valkey L2 client counters, and Valkey server `INFO stats` deltas. Low Valkey command deltas are not automatically bad: repeated reads may be served by the in-process L1 cache. Cache-covered hot paths include action/rule/trigger/sensor/queue/workflow/policy/permission-set metadata plus runtime/runtime-version metadata, webhook-key trigger lookup, enabled queue definition polling, and selected permission-set assignment reads.

The automation latency workload uses a shorter `--e2e-poll-interval-seconds` (default `0.05`) than the general execution polling interval so p95/p99 are less dominated by benchmark observation delay. The comparison report also includes automation sub-metrics for webhook POST latency, execution discovery latency, terminal wait latency, event-to-execution creation time, and execution internal duration.

## What it measures

The suite is intentionally **blended**. Each mode runs three workloads back-to-back:

1. **API metadata-read workload**
   - repeated list/detail reads for packs, actions, triggers, rules, workflows, queues, and policies
   - outputs request rate plus p50/p95/p99 latency
2. **Execution pipeline workload**
   - submits one or more benchmark workflow parent executions through `/api/v1/executions/execute`
   - each parent workflow expands `with_items` into `core.noop` child executions inside Attune, so the scheduler/worker hot path is measured with less per-child HTTP overhead from the Python harness
   - outputs parent submission latency plus child scheduling latency, child end-to-end execution latency, and child completed executions/sec based on child execution DB timestamps
3. **End-to-end automation latency workload**
   - webhook → event → rule → `core.echo` execution pipelines, including the webhook-key trigger lookup path
   - outputs terminal completion latency percentiles over completed samples plus success rate; missed observations are retained as failed samples instead of aborting the run

Each workload also captures:

- `docker stats` snapshots plus sampled CPU/memory series for the core containers
- in-process metadata cache stats from `/api/v1/diagnostics/metadata-cache`
- Valkey `INFO stats` before/after deltas
- an analytics dashboard snapshot from `/api/v1/analytics/dashboard`
- sanity checks so invalid runs are easy to spot

## Benchmark modes

- **Cache on**: base `docker-compose.yaml` + `config.docker.yaml` (`metadata_cache.enabled: true`)
- **Cache off**: base `docker-compose.yaml` + `docker/compose.metadata-cache-off.yaml` (`ATTUNE__METADATA_CACHE__ENABLED=false` on every Attune service)

The cache-off run still keeps the Valkey container present so the service topology stays as close as possible to the cache-on run. Cache-off means PostgreSQL plus process-local L1 cache; cache-on adds Valkey as an optional L2.

## Quick start

```bash
./scripts/run-metadata-cache-benchmark.sh --mode both
```

Outputs land in:

```text
benchmark-results/metadata-cache-<timestamp>/
├── cache-on.json
├── cache-off.json
├── comparison.json
└── comparison.md
```

## Prerequisites

- Docker Engine with `docker compose`
- Python 3.13 (the runner creates `tests/venvs/benchmark` with `python3.13`; override with `BENCHMARK_PYTHON=/path/to/python3.13` if needed)
- free local ports required by `docker-compose.yaml` (8080, 8081, 3000, 5432, 5672, 6379, 15672)

## Recommended run procedure

1. Close other heavy local workloads on the benchmark host.
2. Run cache-on and cache-off back-to-back on the same machine.
3. Let the script reset Docker volumes between modes.
4. Archive the full result directory so the raw JSON stays available.
5. Record host CPU/RAM and any non-default benchmark arguments in your PR/notes.

## Commands

### Full comparison

```bash
./scripts/run-metadata-cache-benchmark.sh --mode both
```

### Single mode only

```bash
./scripts/run-metadata-cache-benchmark.sh --mode cache-on
./scripts/run-metadata-cache-benchmark.sh --mode cache-off
```

### Reuse existing images without rebuilding

```bash
./scripts/run-metadata-cache-benchmark.sh --mode both --skip-build
```

### Keep the last mode running for manual inspection

```bash
./scripts/run-metadata-cache-benchmark.sh --mode cache-off --keep-up
```

### Override workload shape

Arguments after `--` are forwarded to `tests/benchmarks/metadata_cache_benchmark.py run`:

```bash
./scripts/run-metadata-cache-benchmark.sh --mode both -- \
  --metadata-duration-seconds 30 \
  --metadata-concurrency 24 \
  --execution-count 200 \
  --execution-concurrency 16 \
  --execution-workflow-parent-count 1 \
  --execution-measurement-rounds 3 \
  --e2e-pipelines 6 \
  --e2e-iterations 10 \
  --e2e-measurement-rounds 3
```

### Select a benchmark profile

The default profile is `smoke`, intended for quick regression checks rather than statistically stable throughput conclusions. Use a larger profile when comparing cache-on/cache-off performance:

```bash
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile stable
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile soak
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile execution-throughput
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile metadata-heavy
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile automation-hotpath
./scripts/run-metadata-cache-benchmark.sh --mode both -- --profile runtime-queue
```

Use `--profile soak` to study longer-term warm-cache behavior. It adds a `steady_state_hot_path` workload with multiple windows separated by pauses, plus per-window L1/L2/Valkey counters and p95 drift. This is the preferred profile when short smoke-run webhook p95/p99 deltas disagree with execution-throughput results.

## Manual Docker Compose toggles

If you want to inspect the two modes manually before running the driver:

### Cache on

```bash
docker compose -f docker-compose.yaml up -d --build
```

### Cache off

```bash
docker compose \
  -f docker-compose.yaml \
  -f docker/compose.metadata-cache-off.yaml \
  up -d --build
```

Reset state between manual runs:

```bash
docker compose -f docker-compose.yaml down -v --remove-orphans
```

## Default workload profile

The default script settings are tuned to be repeatable on a developer workstation while still exercising the cache paths:

- metadata warmup: `3s`
- metadata measured run: `12s`
- metadata concurrency: `12`
- execution warmup submissions: `8`
- execution measured child executions: `60`
- execution workflow parents: `1`
- execution measurement rounds: `3` (comparison uses the median `completed_per_second` round)
- workflow child concurrency: `8`
- end-to-end warmup iterations: `1` per pipeline
- end-to-end measured pipelines: `4`
- end-to-end measured iterations: `6` per pipeline
- end-to-end measurement rounds: `3` (comparison uses the median latency p95 round)
- synthetic metadata seed: `12` ad-hoc triggers + rules inside a benchmark pack

Tune upward for longer stress runs; keep the same settings for both modes when comparing results.

## Result interpretation

Start with `comparison.md`.

Recommended primary metrics:

- `metadata_api.requests_per_second` (**higher is better**)
- `metadata_api.latency_ms.p95` / `p99` (**lower is better**)
- `execution_throughput.completed_per_second` (**higher is better**)
- `execution_throughput.schedule_latency_ms.p95` (**lower is better**)
- `execution_throughput.end_to_end_latency_ms.p95` (**lower is better**)
- `automation_latency.latency_ms.p95` / `p99` (**lower is better**)

Also check:

- sanity checks in both JSON files and in `comparison.md`
- the `Execution throughput rounds` table; if one round is much slower than the others, treat the run as scheduler/worker jitter until repeated with a larger profile or reversed mode order
- the `Automation latency rounds` table; if only one round is slow, do not treat that p95/p99 delta as cache overhead. If success rate falls below the sanity threshold, investigate the retained `failures` entries before comparing latency.
- process-local L1 and Valkey L2 hit/miss deltas to confirm cache engagement
- Valkey server command deltas to identify Redis/Valkey round-trip pressure
- Docker CPU/memory samples if latency improves but resource cost spikes materially

## Raw result structure

Each mode JSON includes:

- environment metadata (git revision, host, Docker version)
- benchmark settings
- seed summary (pack ref, synthetic trigger/rule refs, webhook pipelines)
- one object per workload (`metadata_api`, `execution_throughput`, `automation_latency`)
- workload-local Docker, metadata-cache, and Valkey captures
- analytics dashboard snapshot
- sanity checks

## Updating the report template

Use `docs/performance/metadata-cache-results-template.md` when pasting benchmark outcomes into a PR, issue, or work summary.
