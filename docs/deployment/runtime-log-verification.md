# Operate and verify runtime logs

Runtime logs use one of two byte backends. PostgreSQL stores artifact, stream,
segment, cursor, and retention metadata for both backends.

## Select the backend

The worker selects the backend from `artifacts.transport` when it allocates the
stdout and stderr streams:

- `api` creates immutable object segments. The worker sends each segment to an
  API replica. The API writes it through the configured `storage` provider.
- `volume` creates a shared file. The worker appends directly under
  `artifacts_dir`, and the API reads the same path.
- `auto` is a local compatibility mode. Deployed workers and sensors must use an
  explicit transport.

The API `storage` setting selects `filesystem`, `s3`, or `gcs` for immutable
objects. It does not turn a shared file into an object-backed stream. For a
deployment without a writable shared volume, set `artifacts.transport: api` on
workers and configure every API replica with the same S3 or GCS bucket and
prefix.

S3 buckets must have versioning enabled. Attune records each returned version ID
and pins later reads and deletes to that version. API replicas may use separate
provider clients, but they must use the same bucket, prefix, credentials policy,
and PostgreSQL database.

## Size worker log buffers

Object-backed writers start at `artifacts.log_segment_initial_bytes`, 64 KiB by
default. Sustained output doubles the segment target up to
`artifacts.log_segment_max_bytes`, 1 MiB by default. The writer flushes partial
segments every `artifacts.flush_interval_ms`, 500 ms by default.

Budget twice `log_segment_max_bytes` for each active stdout or stderr stream.
This covers the stream buffer and a peak segment-sized handoff allocation. A
task with active stdout and stderr can therefore use four times the configured
maximum. Multiply that bound by
`worker.max_concurrent_tasks` when sizing a worker. Smaller segments reduce
per-writer memory and reconnect latency but increase S3 requests and PostgreSQL
segment rows.

`worker.max_stdout_bytes` and `worker.max_stderr_bytes` cap total output. They do
not set the in-memory segment size. A cap truncation is recorded on the sealed
stream.

## Set stream admission limits

Each API replica enforces `server.execution_log_stream_global_limit`. The default
is 100 active SSE readers. It also enforces
`server.execution_log_stream_per_identity_limit`, which defaults to 5.
PostgreSQL leases enforce the same limits across replicas.

The default lease lifetime is 45 seconds, from
`server.execution_log_stream_lease_seconds`. A reader renews its lease every 10
seconds, from `server.execution_log_stream_heartbeat_seconds`. Keep the heartbeat
well below the lease lifetime. If renewal fails until the lease expires, the API
closes the stream with a retryable `stream_lease_lost` event.

## Account for reconciliation and proxies

PostgreSQL `LISTEN/NOTIFY` normally wakes a reader immediately. Reconciliation
bounds missed notifications:

- An active object-backed stream checks for new segments every 15 seconds.
- An active shared-file stream checks file size every 1 second.
- Stream discovery checks every 1 second while the worker has not created the
  stream.
- After the API observes a terminal execution with an incomplete stream, it
  allows a 2-second finalization grace before returning
  `log_stream_incomplete`.

These values are fixed in the API. Configure an ingress or reverse proxy with
response buffering disabled and an idle read timeout above 30 seconds. Sixty
seconds gives room for an active reconciliation interval and an SSE keepalive.
Preserve the `Last-Event-ID` request header on reconnects and do not cache the
SSE response.

## Run the correctness suite

Provision a disposable stock PostgreSQL 16 or newer cluster with a role that can
create databases. PostgreSQL 18 is the default for Compose and CI. Host setup
requires `psql` and `sqlx`. Set `TEST_DB_ADMIN_URL` and `TEST_DB_URL` to that
cluster, then run `make db-test-setup`.

Choose a unique owner for the run and an available S3 host port. Keep these exports for setup, tests, and teardown:

```bash
export ATTUNE_TEST_RUN_ID="logs-$(date +%s)"
export RUNTIME_LOG_HARNESS_OWNER="attune-$ATTUNE_TEST_RUN_ID"
export RUNTIME_LOG_S3_PORT=59000
```

Start disposable versioned RustFS:

```bash
make runtime-log-test-storage-up
```

Set `TEST_DB_URL` if PostgreSQL does not use the Makefile default, then run the
API integration target or only the runtime-log file:

```bash
make test-integration-api TEST_DB_URL=postgresql://attune:attune@localhost:55432/attune_test

make test-runtime-log-correctness \
  TEST_DB_URL=postgresql://attune:attune@localhost:55432/attune_test
```

`make test-integration-api` includes `runtime_log_replica_tests`. Each API
replica has its own PostgreSQL pool, S3 client, wakeup registry, and stream
limiter. The suite checks these behaviors against RustFS:

- Cross-replica upload, live tailing, and `Last-Event-ID` reconnects.
- Artifact preview `content`, `append`, and `done` events across API replicas without a local artifact file, using a real identity with scoped artifact-read permission.
- Concurrent duplicate uploads through separate S3 clients.
- A delayed successful S3 PUT whose caller receives an injected failure, then
  duplicate retries that recover the object and commit one segment row.
- S3 version IDs and exact pinned reads.
- The 15-second reconciliation path after one replica misses log and terminal
  notifications.
- Append-before-seal and seal-before-append orderings through production HTTP
  routes.
- Initial PostgreSQL listener failure followed by a readiness signal after the
  listener connects and registers every channel. Production does not wait for
  this signal and continues serving through reconciliation while the listener
  retries.

On Unix, the same suite starts a child copy of the integration-test binary. The
child appends with `VolumeTransport`, acquires the production file's `flock`, and
is killed without cleanup. The parent proves lock contention, API visibility,
seal behavior, truncation, cleanup retry after process loss, and retention
selection and deletion.

This shared-volume test proves local cross-process Linux `flock` behavior on the
test filesystem. It does not prove NFS lock recovery, mount propagation,
close-to-open consistency, or behavior during a node failure. Validate those
properties on the exact production filesystem and mount options.

The test invokes the supervisor's production artifact cleanup operations. It
checks lock-aware abandoned-log retries, expired-version deletion, and artifact
size metadata refresh instead of reproducing repository calls in the test.

## Validate a Kubernetes RWX implementation

Local CI proves cross-process behavior on one Linux filesystem. Run the opt-in
conformance harness for every RWX StorageClass and CSI implementation used in
production. The harness creates separate writer and reader pods and a PVC. It
checks cross-pod byte visibility, cross-pod `flock` exclusion, lock release after
forced writer-pod deletion, truncation visibility, and file cleanup.

The script refuses to use an existing namespace. It also refuses to contact a
cluster unless context, a new disposable namespace, StorageClass, and the exact
confirmation value are supplied:

```bash
scripts/runtime-log-rwx-conformance.sh \
  --context disposable-test-cluster \
  --namespace attune-rwx-conformance-cephfs-20260913 \
  --storage-class cephfs-rwx \
  --confirm delete-attune-rwx-conformance-namespace
```

Every `kubectl` call includes the supplied context. The script emits one JSON
result and deletes only the namespace it created, including the PVC and pods,
on success or failure. Do not point it at a production context. Static CI runs
`make test-runtime-log-rwx-static`; it does not create Kubernetes resources.

Remove disposable RustFS after the run:

```bash
make runtime-log-test-storage-down
```

The Make harness derives Docker container and network names from the current
user and worktree by default, or from `RUNTIME_LOG_HARNESS_OWNER` when set, and labels both resources. Use distinct owners and ports for overlapping runs. Startup rejects a same-name
resource with a different ownership label. Teardown removes only resources with
the expected label.

Both runtime-log test targets delete all object versions under their unique
`ATTUNE_TEST_S3_PREFIX` when they exit. When tests use persistent external S3
storage, pass its endpoint and credentials to the Make target. Run the same scoped
cleanup explicitly if the test process was interrupted before its exit trap:

```bash
make runtime-log-test-storage-clean \
  ATTUNE_TEST_S3_ENDPOINT=https://s3.test.example \
  RUNTIME_LOG_S3_ACCESS_KEY=attune-test \
  RUNTIME_LOG_S3_SECRET_KEY="$S3_TEST_SECRET_KEY"
```

The cleanup script refuses prefixes outside
`runtime-log-tests/$RUNTIME_LOG_HARNESS_OWNER`.

## Capture a concurrent load report

The load report requires `pg_stat_statements`. Its counters cover the entire
database, not only the two test pools or the test schema. Use a dedicated,
otherwise idle PostgreSQL database for a clean report. Concurrent applications,
maintenance, or tests will inflate the statement-call delta.

Start PostgreSQL with `pg_stat_statements` in `shared_preload_libraries`, then
create that extension in the test database. This extension is required for the
load report only; Attune does not require TimescaleDB.

```sql
CREATE EXTENSION IF NOT EXISTS pg_stat_statements WITH SCHEMA public;
```

Run 20 concurrent reconnect scenarios, or set a value from 1 through 200:

```bash
make test-runtime-log-load \
  TEST_DB_URL=postgresql://attune:attune@localhost:55432/attune_test \
  ATTUNE_LOG_LOAD_STREAMS=20
```

The test prints one JSON object. Save it with the commit, build profile, host,
PostgreSQL settings, S3 implementation and version, and stream count. Compare runs only when
those inputs match.

| Field | Meaning |
| --- | --- |
| `schema_version` | Version of the JSON report format. |
| `streams` | Concurrent reconnect scenarios. |
| `segments_per_stream` | Segments uploaded in each scenario. |
| `elapsed_ms` | Wall-clock time for the concurrent phase. |
| `reconnect_latency_ms_p50`, `reconnect_latency_ms_p95` | Time from reconnect through the final SSE event. |
| `postgres_statement_calls_delta` | Calls added to `pg_stat_statements` during the concurrent phase. |
| `postgres_statement_calls_per_second` | Statement-call delta divided by elapsed time. |
| `s3_puts`, `s3_gets`, `s3_heads` | `BlobStore` operations sent through real S3-backed clients. |
| `peak_aggregate_pool_connections` | Highest sum of opened connections in both API pools. |
| `peak_aggregate_active_pool_connections` | Highest sampled sum of non-idle connections in both API pools. |
| `peak_active_streams` | Highest sampled sum of active SSE readers on both replicas. |
| `leaked_active_streams` | Readers left after all scenarios finish. This must be zero. |

The S3 counters are Attune `BlobStore` calls, not the storage server's internal HTTP request
count. Multipart implementation details can produce more wire requests. Pool and
stream peaks use 5 ms sampling, so a shorter spike can fall between samples.
This is a bounded regression measurement, not a production capacity test.

## Recover failed streams

Use the metrics endpoint first:

```bash
curl -fsS http://127.0.0.1:8080/metrics | grep attune_execution_log_stream
```

Use the following symptoms and actions:

| Symptom | Check | Recovery |
| --- | --- | --- |
| Repeated `waiting` events for more than 15 seconds | Check the worker, `log_stream` row, and API PostgreSQL listener logs. | Restart a disconnected API listener with `docker compose restart api`. The reader reconciliation path remains available during the restart. |
| `stream_lease_lost` | Check PostgreSQL reachability and pool exhaustion. | Restore PostgreSQL, then reconnect with the last SSE event ID. |
| `log_stream_incomplete` | Check worker finalization logs and the artifact version's `meta.log_failure`. | Fix API or storage access, then rerun the execution. Do not mark a partial stream ready by hand. |
| S3 version or digest errors | Check bucket versioning, the configured prefix, and provider credentials on every API replica. | Restore the recorded object version or restore PostgreSQL and object storage to the same point in time. |
| HTTP 429 when opening a stream | Check active-stream metrics and admission lease rows. | Close abandoned clients. Wait up to one 45-second lease lifetime if the owning API process died. |
| A shared pending log remains after worker loss | Check that the supervisor can acquire the file lock and that maintenance is enabled. | Stop the stale writer process and restart the supervisor with `docker compose restart supervisor`. |

To reconnect manually, send the last byte cursor:

```bash
curl -N \
  -H "Authorization: Bearer $ATTUNE_API_TOKEN" \
  -H "Last-Event-ID: $LAST_LOG_BYTE" \
  "$ATTUNE_API_URL/api/v1/executions/$EXECUTION_ID/logs/stdout/stream"
```

Workflow log dispatch uses a durable outbox. If executor logs report a
permanently failed outbox row, fix the storage or API failure and requeue that
exact row:

```bash
attune artifact retry-workflow-log "$OUTBOX_ID"
```

`OUTBOX_ID` is a required positional integer. A CLI test checks this syntax and
the output of `attune artifact retry-workflow-log --help`. The command does not
repair missing bytes. Inspect the failed row and the target artifact before
retrying it.
