# PostgreSQL-only deployment

Attune requires stock PostgreSQL 16 or newer. Compose and CI use PostgreSQL 18.
The application schema requires no TimescaleDB extension, package, preload,
background worker, or runtime adapter. The optional `pg_stat_statements`
extension remains useful for load reports that explicitly require it.

## Fresh-schema installation

The canonical migrations define ordinary PostgreSQL runtime, history, and audit
tables. History-writing triggers still record field changes, including the
`IS DISTINCT FROM` checks and `_jsonb_digest_summary()` values for large JSONB
fields. Event and audit IDs remain `BIGINT` primary keys. History records do not
introduce a public history-ID contract.

Both migration runners enforce recorded migration checksums. The PostgreSQL-only
change rewrites canonical migration contents in place for fresh schemas. It is
not an upgrade script for a populated TimescaleDB schema. The historical filename
`20250101000009_timescaledb_history.sql` does not imply an extension requirement.

For a fresh Compose installation, follow [Docker deployment](../docker-deployment.md).
Both Compose definitions use the new `postgres_data_plain_pg18` named volume.
Compose prefixes its actual volume name with the project name. The new name
keeps the previous `postgres_data_pg18` TimescaleDB data directory separate, even
though both images use PostgreSQL 18. It also leaves older PostgreSQL volumes
untouched. Volume separation does not convert or copy data.

For a fresh host installation, create an empty database and run either the
embedded migration runner or the standalone SQLx runner. Preserve their
migration-history ownership rules. See [Running tests](../testing/running-tests.md)
for disposable test database setup.

Automatic startup does not reset databases, drop extensions, convert stored
data, or bypass checksum validation. A checksum mismatch requires a deliberate
existing-data conversion or reset decision. Editing migration-history rows to
accept changed contents does not convert the schema.

## History and independent retention

`event`, `enforcement`, `execution`, `execution_history`, `worker_history`,
`sensor_process_history`, and `audit_event` are ordinary tables. PostgreSQL can
enforce foreign keys on ordinary tables, but Attune intentionally keeps selected
runtime references as plain `BIGINT` values without foreign keys.

References such as `execution.parent`, `execution.enforcement`,
`workflow_execution.execution`, and `inquiry.created_by_execution` can dangle
after independent retention removes their source rows. Artifacts, inquiries,
and history can outlive their producing executions. Adding foreign keys would
change that retention contract.

The supervisor deletes expired rows in bounded, separately committed batches.
The defaults are `batch_size: 1000` and `max_batches_per_target: 100`, allowing
up to 100,000 deleted rows per target per cycle. Each target uses one fixed
cutoff, counts candidates once, and stops at zero progress, cancellation, or its
batch budget. Eligible backlog continues in later cycles. Terminal-state,
waiting-workflow, and undelivered-log protections remain in effect. Dry runs
count candidates without deleting rows. See [Supervisor retention](supervisor.md)
for target windows and API configuration.

The `continuous_aggregates` retention setting no longer exists. History and
audit deletions report actual row counts rather than dropped chunk counts.
Row deletion creates dead tuples and WAL; monitor autovacuum, disk headroom,
replica lag, and cleanup backlog. The schema no longer supplies TimescaleDB
columnstore compression, so retained table and index sizes need new capacity
measurements.

## Analytics over retained raw records

Hourly analytics use ordinary views and repository queries over raw records.
Hourly buckets align to UTC, independent of the PostgreSQL session timezone.
The metrics retain their distinct meanings:

- Dedicated execution throughput counts `INSERT` records in `execution_history`.
- Execution-volume analytics group live execution rows by their creation hour
  and current status.
- Execution status analytics count history status transitions.
- Dashboard `execution_count` and `execution_timeseries` count terminal history
  transitions, including repeated terminal outcomes from retries.

Newly committed source rows are queryable without an aggregate refresh delay.
Retention removes those rows from subsequent counts immediately. There is no
separate summary-retention window, so older aggregate totals do not survive
the deletion of their raw input. Independent target windows can produce
different available time ranges for live-row volume and history-based metrics.

Authorable dashboard queries report `meta.freshness_mode: raw_only` and no
aggregate watermark. Existing response caching and stale-result handling still
apply. A cached result can precede the latest source change until its next query;
raw-only metadata is not a promise that a previously rendered card has refreshed.
See [Dashboard authoring](../dashboards.md) for preview metadata.

## Existing-data conversion or reset

A populated TimescaleDB installation needs an explicit operator decision.
Data-preserving conversion is a separate deliverable that requires rehearsal
and verification before cutover. A TimescaleDB backup does not restore into
stock PostgreSQL unchanged, and a new image must not automatically attach the
old data directory.

If data must survive, use a separately owned PostgreSQL database and a verified
conversion procedure:

1. Inventory row volumes, compressed sizes, IDs, sequence positions, dangling
   references, and aggregate summaries whose raw input has expired.
2. Create the revised schema in the target database using its own fresh migration
   history. Keep the source database intact for rollback.
3. Transfer data from one consistent snapshot or a controlled write pause.
   During import, disable target history and lifecycle-audit triggers so imported
   rows do not duplicate imported history and audit records.
4. Preserve IDs, sequence positions, and relationship columns. Restore and verify
   triggers before application writes resume.
5. Verify row counts, representative content, permissions, history/audit behavior,
   retention protections, and dashboard counts. Measure storage requirements,
   copy duration, and permitted downtime before cutover.
6. Keep the source database available for rollback until the target passes the
   agreed acceptance checks.

If summaries whose raw input has expired must remain queryable, the raw-view
design needs explicit summary storage before conversion can complete. A logical
row copy alone cannot reconstruct those totals.

If data can be discarded, record the reset decision and identify the exact
database and volume it applies to. Initialize a fresh owned schema using the
normal installation path. Removal of the old database or volume is a separate,
explicit operator action. Broad volume deletion can also remove pack, artifact,
and log storage.

Conversion and reset do not add new runtime formats, dual readers, or compatibility
adapters. For implementation decisions and acceptance checks, see the
[TimescaleDB removal plan](../plans/remove-timescaledb.md).
