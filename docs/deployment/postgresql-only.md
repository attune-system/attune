# PostgreSQL-only deployment

Attune requires stock PostgreSQL 16 or newer. Compose and CI use PostgreSQL 18.
The application schema requires no TimescaleDB extension, package, preload,
background worker, or runtime adapter. The optional `pg_stat_statements`
extension remains useful for load reports that explicitly require it.

## Fresh-schema installation

The canonical migrations define three native RANGE-partitioned parents:

| Parent | Partition key | Leaf interval |
| --- | --- | --- |
| `event` | `created` | One UTC day |
| `execution_history` | `time` | One UTC day |
| `audit_event` | `created` | One UTC day |

The table-creation migrations create partitioned parents and DEFAULT partitions
directly. Native-maintenance setup adds daily leaves for today through seven
days ahead, offsets `0..=7`. Bounds are half-open, `[start, end)`.
The DEFAULT partition keeps backdated records and missed future maintenance
queryable through the parent.

`execution`, `enforcement`, `worker_history`, and `sensor_process_history` remain
ordinary tables. History-writing triggers still record field changes, including the
`IS DISTINCT FROM` checks and `_jsonb_digest_summary()` values for large JSONB
fields. Event and audit IDs remain `BIGINT`, with primary keys `(id, created)`.
These keys do not enforce global uniqueness of `id` alone. BIGINT sequences
allocate IDs. History records have no public history-ID contract.

Both migration runners enforce recorded migration checksums. The canonical
pre-production migrations target a fresh stock PostgreSQL database, not a
populated phase-1 or TimescaleDB upgrade. There is no heap-copy conversion step.
The historical filename `20250101000009_timescaledb_history.sql` does not imply
an extension requirement.

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
data, or bypass checksum validation. A checksum mismatch requires an explicit
development reset or a separately designed conversion. Editing recorded
checksums is not a schema migration.

## Database roles for native maintenance

The migration and supervisor roles need ownership of the managed parents or
effective membership in their owner roles, plus `USAGE` and `CREATE` on the
application schema. Maintenance creates, alters, attaches, and drops schema-local
tables. It also reads and updates the partition registry, summary tables,
coverage ledger, builder state, invalidations, and schedule, with the required
sequence privileges. Reconciliation also requires SELECT/UPDATE on
`native_partition_reconcile_state` and `native_partition_reconcile_cursor`, plus
execution of the native partition functions. The SQL functions use invoker privileges, not
`SECURITY DEFINER`. Ordinary DML grants alone cannot authorize partition DDL.

The summary migration grants existing source
writers `INSERT` on `native_summary_invalidation` and `USAGE` on its sequence.
Marker reuse also requires column-level SELECT on `kind`, `bucket`,
`transaction_origin`, and `xmin`. Provision those grants for later writer roles
too. Producers do not need access
to builder state. Native maintenance needs neither superuser privileges nor
database-creation privileges. Test harnesses have separate database-creation needs.

Cache refresh creation also performs DDL. The API role needs schema `USAGE` and
`CREATE` and effective ownership of the `cache_entry` parent. The supervisor
needs the same cache owner role for reclamation and parent/leaf `ANALYZE`. New
leaves are assigned to the catalog parent owner rather than to the creating API
login, allowing distinct service logins to share this limited ownership scope.
Use a cache-storage owner role for this parent and its leaves, and grant that
owner role schema `USAGE` and `CREATE` too. Do not grant API
superuser, database ownership, or ownership of unrelated tables to solve cache
DDL access. Changing a leaf's owner also requires permission to assume the parent
owner role; provision that membership explicitly.

Lifecycle and pin writers need the required cache DML and sequence grants,
including updates to `cache_entry_statistics_state` and
`cache_generation_entry_usage`. These lifecycle functions and triggers remain
`SECURITY INVOKER`. Ordinary entry writers do not need schema `CREATE` or table
ownership. Validate creation under the API login and reclamation/statistics under
the supervisor login, not only under the migration role. See
[Supervisor maintenance](supervisor.md) for budgets and observations.

## History and independent retention

Attune intentionally keeps selected runtime references as plain `BIGINT` values
without foreign keys. PostgreSQL partitioned tables can be FK targets when the
referenced key meets PostgreSQL's uniqueness rules. That capability does not
change Attune's independent-retention contract or make a partitioned `id` alone
a unique target.

References such as `execution.parent`, `execution.enforcement`,
`workflow_execution.execution`, and `inquiry.created_by_execution` can dangle
after independent retention removes their source rows. Artifacts, inquiries,
and history can outlive their producing executions. Adding foreign keys would
change that retention contract.

The supervisor first drops fully expired registered daily leaves when native
maintenance is enabled. A leaf qualifies only when its upper bound is at or
before the target's fixed cutoff. The same transaction removes its summary
groups, per-hour coverage, and invalidations. Partition drops do not fire row
DELETE triggers.

The supervisor deletes expired boundary-day and DEFAULT rows in bounded,
separately committed batches through the parent. Those deletes invalidate
affected summary hours in the source transaction. Row selection locks
statement-local `(tableoid, ctid)` pairs because `ctid` can repeat across leaves.
The defaults are `batch_size: 1000` and `max_batches_per_target: 100`, allowing
up to 100,000 deleted rows per target per cycle. Each target uses one fixed
cutoff and stops at zero progress, cancellation, or its batch budget.
Eligible backlog continues in later cycles. Terminal-state,
waiting-workflow, and undelivered-log protections remain in effect. Dry runs
count candidates without deleting rows. See [Supervisor retention](supervisor.md)
for target windows and API configuration.

After row cleanup, bounded source-locked housekeeping removes obsolete summary
groups, coverage, and visible invalidations for fully expired hours. It also runs
when raw candidates are zero or native maintenance is disabled. The partial cutoff
hour remains available for recomputation. Dry runs and unlimited retention do not
purge materializations. Later housekeeping failures preserve confirmed raw-row
deletion and partition-drop counters.

The `continuous_aggregates` retention setting no longer exists. Retention reports
confirmed row deletions as `deleted` and confirmed leaf drops as
`partitions_dropped`, with separate `partition_candidates`. Native row candidate
probes cover only boundary/DEFAULT rows and stop at the row budget plus one.
`candidates_exact: false` means a lower bound. Leaf contents are not counted to
invent a deleted-row total. See [Supervisor status and counts](supervisor.md#native-status-and-counts).
Row deletion creates dead tuples and WAL; monitor autovacuum, disk headroom,
replica lag, and cleanup backlog. The schema no longer supplies TimescaleDB
columnstore compression, so retained table and index sizes need new capacity
measurements.

## Hourly summaries over retained raw records

Four ordinary summary tables store counts by UTC hour, independent of the
PostgreSQL session timezone:

| Summary table | Source predicate | Additional dimensions |
| --- | --- | --- |
| `execution_status_hourly_summary` | `execution_history` with `status` in `changed_fields` | Nullable action ref and new status |
| `execution_creation_hourly_summary` | `execution_history.operation = 'INSERT'` | Nullable action ref |
| `event_volume_hourly_summary` | `event` creation records | Trigger ref |
| `worker_status_hourly_summary` | `worker_history` with `status` in `changed_fields` | Nullable worker name and new status |

Counts are BIGINT. Summary keys preserve nullable groups with
`UNIQUE NULLS NOT DISTINCT` where needed. The canonical ordinary hourly views
remain available, but the native reader uses these summary tables and
timestamp-bounded raw queries. The metrics retain their distinct meanings:

- Dedicated execution throughput counts `INSERT` records in `execution_history`.
- Execution-volume analytics group live execution rows by their creation hour
  and current status.
- Execution status analytics count history status transitions.
- Dashboard `execution_count` and `execution_timeseries` count terminal history
  transitions, including repeated terminal outcomes from retries.
- Worker status counts `worker_history` rows with `status` in `changed_fields`.

`native_summary_hour` records each completed materialized hour, including empty
hours. `native_summary_state` serializes builders by kind. Statement triggers
on `event`, `execution_history`, and `worker_history` append one invalidation per
distinct affected kind/hour in `native_summary_invalidation`. Repeated writes reuse
only markers with the current producer's full transaction origin and actual
tuple `xmin`. The origin index is nonunique, so restored origin values do not
suppress new notifications. Savepoints may leave extra markers conservatively.
UPDATE covers old and new predicates and hours. The append log has no FK to builder
state and no unique kind/hour marker that would make producers wait on builders.

Each builder locks the source parent before taking a repeatable-read snapshot,
then locks and updates its kind's state row. It captures bounded explicit visible
invalidation IDs, replaces all groups for the hour, records coverage, and deletes
only those captured IDs in one transaction. Sequence order is not commit order.
A lower ID that commits later stays pending. Serialization retries take a new
snapshot within the original deadline.

The derived refresh transaction alone sets `synchronous_commit = off` locally.
Coverage, summary rows, state, and captured-ID acknowledgment still commit
atomically. A crash can lose the whole cache commit, after which durable source
rows and notifications permit rebuilding. Source writes, retention, partition DDL,
and scheduling keep their normal durability. The setting does not survive the
refresh transaction or change the connection's session default.

Readers also lock the source parent before their snapshot. They use summaries
only for clean, completed, fully included ledger hours. Dirty hours, coverage
holes, the current hour, and partial request boundaries use timestamp-bounded raw
queries. Missing maintenance relations or maintenance privileges also permit raw
fallback. Other SQL and decoding errors remain errors. New committed source rows
are therefore queryable without waiting for summary refresh.

Read metadata reports `raw_only`, `summary_only`, or `summary_plus_raw`, with exact
disjoint `summary_ranges` and `raw_ranges`. No global watermark proves coverage.
Dashboard stale-cache fallback reports `cache_rawfallback` and clears coverage.
See [Dashboard analytics metadata](../dashboards.md#current-analytics-data-behavior).

Summary retention follows raw-source retention. A partial deletion makes its
hour dirty, so subsequent reads use raw counts until rebuilding finishes. A
whole-leaf drop removes coverage and summaries atomically. Independent target
windows can produce different available ranges for live-row and history metrics.

## Migration policy and development resets

Attune is pre-production. There are no production databases to convert for this
change. Before 1.0.0, canonical migrations may change in place and development
databases may require an explicit reset. Preserving a populated phase-1 or
TimescaleDB database is not a delivery requirement.

Reset only a specifically identified development database or database volume,
after confirming its data can be discarded. Removal is a separate operator action;
startup never performs it. Keep pack, artifact, blob, and runtime-environment
volumes intact. An old PostgreSQL data directory is not a fresh database.

Starting with 1.0.0, released migration bytes must remain immutable. Schema changes
use new forward migrations that preserve data and internal contracts. Validate
supported upgrade paths on populated databases, including backup/restore,
failure rollback and retry, IDs, sequences, permissions, and history behavior.
Any destructive change requires an explicit migration plan rather than an
automatic reset or checksum rewrite.

## Fresh-install verification

Verify empty-database installation on PostgreSQL 16 and 18 with both the SQLx and
Docker migration runners. Check native parents, DEFAULT and daily leaves,
indexes, BIGINT sequences, history/audit triggers, independent-retention refs,
summary invalidations, migration checksums, and no-op reruns.

See [Fresh native-install verification](postgresql-native-install-verification.md)
and [Running tests](../testing/running-tests.md#native-partition-and-summary-validation).
The separate [native protocol verifier](../../scripts/verify-native-partitions.py)
checks DEFAULT repair, expiry, producer races, and boundary-row cleanup.

Reporting-performance optimization remains deferred. Earlier workload evidence
passed count checks but failed some latency and ingestion-overhead targets; see
the [workload evidence](../research/postgresql-native-maintenance-workload.md).
Those measurements predate the fresh migration baseline and are not performance
certification for it.
