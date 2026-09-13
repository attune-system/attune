# Verify runtime logs across API replicas

Use the runtime-log integration tests to check cross-replica reads, reconnects,
notification recovery, upload races, and cleanup behavior. The tests start two
API servers against an isolated PostgreSQL schema and temporary storage.

## Run the integration tests

Set the test database URL, then run the ignored integration-test file:

```bash
export ATTUNE__DATABASE__URL=postgresql://attune:attune@localhost:55432/attune_test
cargo test -p attune-api --test runtime_log_replica_tests -- --ignored --test-threads=1
```

The command runs these checks:

- One API replica uploads object-backed segments while another replica streams
  and reconnects with `Last-Event-ID`.
- Delayed duplicate uploads and a terminal execution update race without
  duplicating or dropping log bytes.
- A replica without a PostgreSQL notification listener finds new bytes through
  periodic reconciliation.
- Two service instances read one shared-volume path, propagate truncation, clean
  up an abandoned writer, and retain a sealed log.

The object-backed tests use the production `BlobStore` interface with a delayed,
counting filesystem implementation. They do not test S3, GCS, network failures,
or provider-specific version semantics. Run provider integration tests separately
before changing a production object-store configuration.

The shared-volume test uses two service instances on one host with one filesystem
path and the production advisory locks. It does not prove NFS behavior, mount
propagation, or cross-node filesystem consistency.

## Capture a bounded load report

The load case is opt-in. It creates 20 streams by default and accepts between 1
and 200 through `ATTUNE_LOG_LOAD_STREAMS`:

```bash
ATTUNE_RUN_LOG_STREAM_LOAD=1 \
ATTUNE_LOG_LOAD_STREAMS=20 \
cargo test -p attune-api --test runtime_log_replica_tests \
  bounded_runtime_log_load_report -- --ignored --exact --nocapture
```

The test prints one JSON object. Save that line with the tested commit, build
profile, host details, PostgreSQL configuration, and stream count. Compare runs
only when those inputs match.

The report fields have these meanings:

| Field | Meaning |
| --- | --- |
| `schema_version` | Version of the JSON report format. |
| `streams` | Number of reconnect scenarios run. |
| `segments_per_stream` | Number of uploaded segments in each scenario. |
| `elapsed_ms` | Wall-clock time for all scenarios. |
| `reconnect_latency_ms_p50` | Median time from reconnect start through the final event. |
| `reconnect_latency_ms_p95` | 95th-percentile time from reconnect start through the final event. |
| `tail_database_queries` | Tail-path database queries reported by the reader replica. |
| `database_queries_per_second` | `tail_database_queries` divided by elapsed time. |
| `object_puts`, `object_gets`, `object_heads` | Calls observed by the counting `BlobStore`. |
| `active_streams_after_run` | Open reader streams after all scenarios finish. This must be `0`. |
| `db_pool_connections` | Connections currently opened by the test pool. |
| `db_pool_active_connections` | Non-idle connections sampled while both replicas and listeners are still running. |

This is a bounded regression measurement, not a production capacity test. The
test runs reconnect scenarios one at a time and does not report peak connection
use. Treat higher latency, more queries per stream, unexpected object operations,
or a nonzero final stream count as reasons to investigate.
