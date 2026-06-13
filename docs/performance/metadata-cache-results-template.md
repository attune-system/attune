# Metadata cache benchmark results template

## Environment

- Date:
- Git revision:
- Host machine:
- CPU / RAM:
- Docker version:
- Benchmark command:

## Workload settings

```json
{}
```

## Summary table

| Metric | Cache on | Cache off | Delta | Winner |
| --- | ---: | ---: | ---: | --- |
| metadata_api.requests_per_second |  |  |  |  |
| metadata_api.latency_ms.p95 |  |  |  |  |
| metadata_api.latency_ms.p99 |  |  |  |  |
| execution_throughput.completed_per_second |  |  |  |  |
| execution_throughput.schedule_latency_ms.p95 |  |  |  |  |
| execution_throughput.end_to_end_latency_ms.p95 |  |  |  |  |
| automation_latency.latency_ms.p95 |  |  |  |  |
| automation_latency.latency_ms.p99 |  |  |  |  |

## Cache-on notes

- Sanity checks:
- Valkey hit/miss deltas:
- Docker CPU/memory observations:

## Cache-off notes

- Sanity checks:
- Docker CPU/memory observations:

## Interpretation

- Did the cache materially improve metadata-read latency?
- Did throughput improve, regress, or stay flat?
- Did resource cost change enough to matter?
- Any unexpected failures, warmup artifacts, or noisy-neighbor effects?

## Raw artifacts

- `cache-on.json`:
- `cache-off.json`:
- `comparison.json`:
- `comparison.md`:
