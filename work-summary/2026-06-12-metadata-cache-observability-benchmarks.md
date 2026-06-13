# Metadata cache observability and benchmark improvements

Implemented production-visible metadata cache statistics and benchmark reporting updates for Valkey/L1 analysis.

## Changes

- Added in-process metadata cache counters for L1 JSON/index hits and misses, Valkey L2 JSON/index hits and misses, local-only fallbacks, writes, evictions, and best-effort errors.
- Exposed cache stats through `GET /api/v1/diagnostics/metadata-cache`, guarded by existing authenticated `retention:read` authorization.
- Extended benchmark workload captures to include metadata cache stats deltas alongside Valkey `INFO stats` and Docker metrics.
- Split comparison reporting into process-local L1, Valkey L2 client counters, and Redis/Valkey server counters.
- Added benchmark profiles (`smoke`, `metadata-heavy`, `automation-hotpath`, `execution-throughput`, `runtime-queue`, `stable`) and report interpretation notes for low-sample smoke runs.
- Added a `soak` profile with a multi-window `steady_state_hot_path` workload for longer-term warm-cache behavior, per-window L1/L2/Valkey counters, and p95 drift tracking.
- Added a benchmark cross-service prewarm phase that exercises API metadata reads, executor workflow fan-out metadata, worker action/runtime metadata, webhook lookup, and queue polling paths before measurement.
- Added executor-local short-TTL scheduling metadata bundles for action/runtime metadata used during scheduling fan-out bursts.

## Notes

Worker candidate snapshot caching was evaluated but not added because the current worker repository row mixes static capabilities with live status, cordon, and heartbeat state. Caching that row would risk stale scheduling decisions; worker availability remains database-authoritative.
