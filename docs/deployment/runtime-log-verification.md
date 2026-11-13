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

## Asynchronous worker log custody

On Unix workers, delegated object logs use a persistent encrypted spool by
default. After EOF and bounded capture, the worker synchronizes an immutable
entry and registers its exact segment hashes and final proof before publishing
the action result. A separately bounded uploader completes the same versions.
Live segment upload remains available while the action runs. Log availability
is independent of execution completion; later delivery failures do not rewrite
the action result.

```yaml
worker:
  name: stable-worker-name
  log_delivery:
    enabled: true
    # Optional private persistent worker-local directory. Default:
    # runtime_envs_dir/.log-delivery/<database-worker-id>
    spool_dir: /var/lib/attune/log-spool
    max_spool_bytes: 1073741824
    max_finalizers: 64
```

Persist the spool volume and keep the worker name and encryption key stable
across process/container restarts. Each database worker has its own locked
subdirectory. Entries are authenticated and encrypted at rest, contain the exact
on-wire bytes and final proof, and contain no bearer credentials or encryption
keys. Entry and total-byte budgets and the concurrent finalizer limit bound
resource use. If custody cannot be accepted, capture is marked incomplete and
the classified action outcome can still settle. This local spool is not the log
artifact backend: `artifacts.transport: api` and the configured object store
continue to own delivery and published logs.

A restarted worker claims only matching custody under its original attempt and
active source authority. The new process instance fences old delivery tokens.
A dedicated incarnation lock fences replacement without blocking heartbeat or
active/busy updates. Background uploads wait for terminal publication to activate
custody, so their source locks cannot hold the execution open.
Bounded spooled segments use the provider's atomic create-only PUT, avoiding
multipart staging/copy while retaining exact size/hash and provider-version
verification. The source proof is reused within each transaction; a new phase
still reacquires authority and checks freshness.

The purpose-specific credential authorizes exact segment append and seal only;
completion closes action API access. General file operations, other log versions,
changed segment bytes, replaced attempts and revoked sources are denied.
Restoring a revoked source does not revive an old handoff.
Changes to an owner's directly or role-assigned covering permission sets also
cancel pending handoffs, even when that set differs from the selected execution
permission set.

Each accepted stream has a 60-second deadline. Reads report `finalizing` only
while custody remains active and before that deadline; verified Ready publication
wins concurrent expiry diagnosis. Process/container restart is recoverable when
the spool volume persists. **Node or volume loss can lose undelivered logs**:
restarting the same worker without the custody marks them incomplete immediately;
otherwise the deadline bounds the finalizing period. No API-owned custody or
node-loss durability guarantee is implied. The action result remains unchanged.

Settlement becomes manager-owned before spool persistence, so canceling its
caller cannot abandon a persisted entry or an uncertain database commit.
Database preparation has a ten-second bound. Failed preparation is reconciled
under exclusive graph admission before absence permits orphan removal.
After the upload deadline, the manager retains its bounded finalizer slot and
retries terminal bookkeeping and custody cleanup when database access recovers;
this does not permit late upload or extend the deadline. Shutdown can leave that
custody for restart recovery after its bounded drain.

Graceful shutdown stops action admission and drains delivery before heartbeat
shutdown/deregistration. A forced stop retains synchronized entries for recovery.
Shared-file and recorded-action logs keep their existing settlement order; this
handoff does not enable recorded credentials or broaden their readiness gate.

## Empty execution output

Execution stdout and stderr allocate independently on their first nonempty write.
A successful or failed action that stays silent creates no log artifact version,
stream or stream-delivery permit. Whitespace still counts as output. Execution
attempt registration remains independent of output.

The worker records positive per-stream capture state only after capture has
settled. A terminal stream known to be empty returns SSE `done` at cursor `0`
without a discovery grace period. A nonzero resume cursor is invalid for that
stream. Capture/allocation failure, cancellation during first allocation, a
crashed worker, or supervisor settlement without positive capture evidence must
not become successful empty output; missing or incomplete logs retain their
existing error behavior. Fresh allocation under an already-canceling recorded
execution is still denied and is reported as incomplete capture.

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

## Bound finalization and retries

`artifacts.log_finalization_timeout_ms` defaults to 60,000 ms for the total
post-exit drain and log settlement. Segment uploads and seals retry transient
failures using `log_segment_retry_max_attempts`,
`log_segment_retry_attempt_timeout_ms`, and the configured backoff settings.
Sealing preserves the exact final manifest on every retry. Shared-file appends
are not retried because an append acknowledgement can be ambiguous.

Idle pipe reads stop one second after process exit, independently of uploads and
sealing. Already buffered pipe bytes are drained first. A descendant retaining
stdout or stderr without producing output therefore cannot consume the longer
storage budget. Missing EOF is incomplete capture and is never sealed as complete.

The action deadline, cancellation, and total finalization budget still bound the
worker's wait. Increasing this budget can keep a worker slot occupied longer
during storage failures and must fit deployment shutdown budgets. Successful
requests return immediately. There is no durable retry job after worker loss
or deadline exhaustion, and a dropped request may have committed remotely.
Use `logs_incomplete` and finalization warnings to identify affected executions;
do not interpret action success as proof that logs were sealed.

## Interpret delivery outcomes

Finite execution-log downloads return HTTP 409 with `LOG_FINALIZING` while the
producer is active, or `LOG_INCOMPLETE` after terminal capture failed. These
states are returned only after full content authorization. Unauthorized readers
retain the existing denial behavior. A version published concurrently with a
Pending selection returns the retryable finalizing diagnosis rather than falsely
claiming permanent loss; retry the same immutable version ID.

The web UI displays these states explicitly. Execution SSE still uses
`log_stream_incomplete` and never reports successful completion of an unsealed
stream. A successful action and its two stream outcomes are independent.
Positive `runtime_log_capture` evidence that stderr was empty remains valid if
stdout delivery failed; missing capture evidence remains unknown.

`artifact.downloaded` for runtime logs records full authorized EOF. A cancelled
preview, partial range, rejected download, or failed body does not count as a
successful full download. This does not prove a client retained the bytes.

The API exposes `attune_runtime_log_finalizing_responses_total` and
`attune_runtime_log_incomplete_responses_total`. These count responses, not unique
logs. Correlate worker retry operation/attempt and finalization deadline logs
with version IDs; the API diagnostic includes observed `pending_age_ms` without
an expensive background backlog scan. No content or credentials belong in these
logs or metric labels.

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
- Recorded key-decrypt revocation between object-backed artifact SSE frames. The
  reader retains execution/artifact permissions but receives no data frame after
  its source-key decrypt grant is removed.
- Concurrent duplicate uploads through separate S3 clients.
- A delayed successful S3 PUT whose caller receives an injected failure, then
  duplicate retries that recover the object and commit one segment row.
- S3 version IDs and exact pinned reads.
- The 15-second reconciliation path after one replica misses log and terminal
  notifications.
- Slow publication longer than terminal grace, with completion only after sealing
  is acknowledged. A separate early-completion case expects `log_stream_incomplete`
  and EOF rather than silently treating the stream as successfully completed.
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

Fixtures that expose runtime logs create a released action snapshot and attach its
admission schema binding. Tests must not rely on unbound executions being readable.

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

## Compare completion and log delivery on RustFS

The paired collector runs native API/executor/worker/sensor/notifier binaries
against exclusively owned Docker PostgreSQL, RabbitMQ and versioned RustFS. It
uses random host ports and removes only its own project. Build the service
binaries before running it; keep all compilation and other test workloads out
of the measured interval.

```bash
cargo build --locked --release --bin attune-api --bin attune-executor \
  --bin attune-worker --bin attune-sensor --bin attune-notifier
CARGO_TARGET_DIR=target/core-timer \
  cargo build --locked --manifest-path crates/core-timer-sensor/Cargo.toml
python3 scripts/benchmark-async-log-delivery.py \
  --output /tmp/attune-log-delivery-new-run \
  --bin-dir target/release \
  --timer-bin target/core-timer/debug/attune-core-timer-sensor \
  --seconds 120
```

The output directory must be new. Python needs PyYAML, and Docker needs the
images pinned in the collector. Both cohorts use the same optimized binaries,
rule, storage and retention settings; only asynchronous handoff changes. Each
cohort warms for 30 seconds and runs a one-second `core.intervaltimer` rule calling
`core.echo`. The collector records database Running/Completed history timestamps,
observed child-wait timestamps, sealed timestamps, results, immutable object-log
counts and warmed API backend PID continuity. Database transition timestamps
mark their transaction entry rather than a client-visible commit acknowledgement.
The report separately records the worker completion observation after terminal
persistence, monotonic process duration, and post-exit capture/handoff cost. Its
pre-process estimate subtracts process duration from Running-to-child-wait time;
it includes preparation and timing/observation overhead.

After the timed cohorts, it checks persistent-spool process restart, missing-volume
custody, lost seal acknowledgement, deadline expiry, and unchanged action results.
These fault probes are not included in the latency comparison. Source/binary
hashes, raw cohort rows, service logs and a JSON summary remain in the private
output directory. A failed assertion exits nonzero and retains its evidence.

## Capture a concurrent load report

The load report requires `pg_stat_statements`. Its counters cover the entire
database, not only the two test pools or the test schema. Use a dedicated,
otherwise idle PostgreSQL database for a clean report. Concurrent applications,
maintenance, or tests will inflate the statement-call delta.

Start PostgreSQL with `pg_stat_statements` in `shared_preload_libraries`, then
create that extension in the test database. This extension is required for the
load report only; Attune does not require TimescaleDB.

Physical fixtures clone a migration-hashed template built from `template0`, not
the database named in `TEST_DB_URL`. In an exclusively owned disposable cluster,
install the extension in `template0` before creating any fixture templates, then
restore its connection restriction. Otherwise the load test's clone has no
extension even if `TEST_DB_URL` does. Never modify a shared or live template
database for this test.

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

Set `ATTUNE_LOG_LOAD_REPORT` to an unused file path to save that JSON after awaited
teardown. The file is never overwritten. The test waits for active readers to
reach zero and fails on its bounded leak-check deadline.

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

## Measure worker completion latency

The opt-in lifecycle profile uses an exclusively owned full-stack E2E project.
It publishes a small workload pack and runs echo, silent success/failure,
stderr failure, both streams, and structured JSON stdout. Each profile separates
its first invocation, warms workers, then records at least 20 samples at
concurrency 1 and 4. First invocation is not proof of a cold filesystem cache.

Use the same benchmark test and Compose override on both source revisions.
Record their hashes, source revisions, build profile, host and storage mode.
Build the images first, allow compilation to finish, then measure on an idle
host. Set `ATTUNE_BUILD_GIT_SHA` during image builds to the full tested commit;
this also invalidates cached package builds when source mtimes are older.

```bash
export ATTUNE_E2E_RUN_ID="worker-profile-$(date -u +%Y%m%d%H%M%S)-$$"
export ATTUNE_E2E_PROJECT_NAME="attune-$ATTUNE_E2E_RUN_ID"
export ATTUNE_E2E_COMPOSE_OVERRIDE="$PWD/docker-compose.worker-lifecycle-benchmark.yaml"
export ATTUNE_WORKER_BENCHMARK_REVISION="$(git rev-parse HEAD)"
export ATTUNE_BUILD_GIT_SHA="$ATTUNE_WORKER_BENCHMARK_REVISION"
export ATTUNE_WORKER_BENCHMARK_TRANSPORT=volume # use api for object segments
export ATTUNE_WORKER_BENCHMARK_EXPECT_LAZY=1 # use 0 for the eager baseline
bash scripts/run-integration-tests.sh \
  -k test_worker_lifecycle_profile -m performance -s
```

The runner tears down only its own project. To collect service traces, use
`--no-teardown`, capture `docker compose` logs with the same project and all three
Compose files, then explicitly tear down that owned project with `down --volumes`.
Use a fresh project for every repeat and storage mode. Keep workload sample counts
and debug logging identical. Ordinary E2E invocations exclude performance tests.

The `WORKER_LIFECYCLE_REPORT` JSON contains request-to-terminal PostgreSQL
notification latency, request-to-observed MQ completion latency, raw sample IDs,
artifact versions, stream counts, database-wide statement-call/SQL-time deltas,
and p50/p95/p99 completion observations. MQ observation polls a dedicated owned
management queue every 10 ms, so it includes polling and management overhead.
The observer is an additional expiring durable classic queue, explicitly deleted
after the run; both revisions incur its overhead.
It does not measure exact broker arrival. SQL totals include orchestration and
background jobs. With `pg_stat_statements.track=all`, nested SQL contributes
to the totals, so summed SQL time is not elapsed wall-clock time. Worker debug
logs provide lifecycle phases and SQL/pool traces;
API traces and version IDs correlate seal requests. PostgreSQL logs lock waits
longer than 50 ms. No arbitrary latency target is asserted by this test.

The benchmark override now defaults explicitly to the product's 60,000 ms
capture-finalization budget. `ATTUNE_WORKER_BENCHMARK_LOG_FINALIZATION_MS` can
select a different diagnostic budget; the report records it. Earlier lifecycle
measurements used a 10,000 ms override against a then-1,000 ms product default.
Those results do not establish capacity at the current default and must not be
compared with current runs unless both revisions use identical workload,
finalization budget, storage durability, and observation settings.

The lazy expectation additionally checks independent per-stream allocation,
zero log versions for silence, terminal capture markers, backend selection and
persisted/MQ status agreement. Recorded/encrypted authority cases remain in the
repository/API and versioned-storage correctness suites; a default rollout-mode
benchmark does not replace those security checks.

Attach the companion functional checks to the owned stack left running with
`--no-teardown`:

```bash
bash scripts/run-integration-tests.sh --no-build --no-startup --no-teardown \
  -k 'test_worker_log_stream_discovered_after_subscription or test_worker_managed_retry_completion or test_worker_managed_queue_completion'
```

Collect the traces and tear down that same owned project afterward. For a fresh
functional run, choose a new run/project identity and omit `--no-startup`.

The subscription checks hold output behind an owned HTTP gate, subscribe before
allocation, then verify delivery, EOF, reconnects and empty stderr. Published
release fixtures verify native retry lineage and queue acknowledgement. Action
API selections stay empty; these tests do not enable coordinated credential
readiness or repair absent ad hoc admissions. The repository's lock-controlled
reader case separately covers sealing between pending discovery and authorization
through both execution and artifact stream entry points.

## Bounded post-PR32 acceptance

Use the same owned runner and lifecycle override for a smaller correctness run:

```bash
export ATTUNE_E2E_RUN_ID="log-accept-$(date -u +%Y%m%d%H%M%S)-$$"
export ATTUNE_E2E_PROJECT_NAME="attune-$ATTUNE_E2E_RUN_ID"
export ATTUNE_E2E_COMPOSE_OVERRIDE="$PWD/docker-compose.worker-lifecycle-benchmark.yaml"
export ATTUNE_WORKER_BENCHMARK_REVISION="$(git rev-parse HEAD)"
export ATTUNE_BUILD_GIT_SHA="$ATTUNE_WORKER_BENCHMARK_REVISION"
export ATTUNE_WORKER_BENCHMARK_TRANSPORT=api # repeat in a fresh project with volume
export ATTUNE_WORKER_BENCHMARK_LOG_FINALIZATION_MS=60000
bash scripts/run-integration-tests.sh -k test_worker_log_acceptance -m performance -s
```

This selects one test with a 300-second test deadline, in addition to the
runner's bounded startup. It publishes the existing lifecycle workload pack,
starts an eight-second paced stdout producer, waits for positive stream allocation
while it is running, then submits four concurrent instances of each short action:
silent success, silent failure, stderr failure, both streams, and structured JSON
stdout. There are 21 accepted executions in a complete run, without warmup. The
producer pacing is workload, not a simulated slow storage provider. Submission
alongside an active producer does not prove simultaneous worker-slot occupancy.

`WORKER_LOG_ACCEPTANCE_REPORT` contains IDs and metadata only. It checks action
status independently of capture, structured result preservation, exact per-stream
allocation (including no versions for silence), sealed/ready output, requested
backend, and shared action-level parent artifacts across the concurrent versions.
It does not download or print plaintext logs. It is not a byte-integrity test or
a latency target. The existing runtime-log correctness suite retains those
transport and fault boundaries.

The report records incomplete executions over **all accepted submissions**,
including silence and expected action failures. Missing terminal observations or
missing positive capture markers are unknown; the rate is `null` when any are
unknown. It emits the report on assertion failure after submission as well as on
success; a partial report never passes acceptance. A failed/ambiguous submission
is a test failure, not evidence of zero incomplete captures. Keep the runner exit
status with the report. Failed setup before any submission is not workload evidence.

The configured finalization timeout and each execution's snapshotted action
timeout are recorded separately. The effective post-exit wait is the minimum of
that configured budget and the remaining action deadline; cancellation can shorten
it. Exact effective per-execution budgets and finalization duration are not
exported here. Request-to-observed database/MQ completion times are not
finalization times. Finalization duration, slot occupancy, pool wait, and lock wait
are explicitly unavailable in the JSON; worker IDs are observations, not occupancy.
Existing structured worker `Runtime log finalization finished` events provide
`elapsed_ms` and `budget_ms`; SQLx traces and PostgreSQL waits above 50 ms can
support separate trace review. Do not interpret missing traces as zero waits or
publish raw service logs that may include content.

Save the metadata report, command, exit status, full source SHA, uncommitted diff,
harness and override SHA-256 hashes, host/build/storage versions, and effective
Compose settings. This bounded run does not compare revisions. For a comparison,
use identical harness hashes, all 21 executions, concurrency, budgets, durability,
workers and observer overhead on both revisions; never discard incomplete runs
from the denominator to report a speedup.

Fault coverage is reused, not injected again into this mode:

- `crates/common/src/log_stream.rs`: `retries_the_same_sequence_and_bytes_before_continuing_in_order`,
  `seal_retries_http_unavailability_with_identical_headers`,
  `seal_retries_identical_proof_after_lost_acknowledgement`, and bounded
  attempt/exhaustion tests cover deterministic transient upload/seal and lost ACK.
- `crates/worker/src/runtime/process_executor.rs`:
  `default_finalization_budget_allows_slow_final_upload_and_seal`,
  `seal_failure_preserves_process_success_and_the_other_stream_capture`, and
  `post_exit_log_finalization_deadline_is_bounded` cover process/capture separation.
- `crates/api/tests/support/object_seal_ack_loss.rs`, compiled by
  `recorded_execution_events_tests`, covers prepare/final commit ACK loss, exact selection
  recovery, admission revocation and repeated ambiguity through the PR32 route.
  Run that subset through the existing owned PostgreSQL runner:
  `bash scripts/run-rust-integration-tests.sh --test recorded_execution_events_tests --filter object_seal_ack_loss`.
  The separate versioned-storage suite above still covers real S3 transport.
  Worker acceptance does not certify durable-close recovery after worker loss.

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
