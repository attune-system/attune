# Attune supervisor

`attune-supervisor` is Attune's platform maintenance service. It runs outside the API, executor, worker, sensor, and notifier hot paths and owns cross-cutting cleanup, retention, monitoring, and guarded remediation work.

## Where it fits

The supervisor is a single-purpose operations service. It should normally run as one replica, but every maintenance cycle is protected by a PostgreSQL advisory lock so accidental multiple replicas skip work instead of racing each other.

The executor still owns normal execution scheduling, workflow advancement, worker timeout reconciliation, and queue dispatch. The supervisor acts as the maintenance and safety-net layer for data that can otherwise grow without bound or remain stale after an abnormal shutdown.

The service connects to:

- PostgreSQL, required for all retention and maintenance work.
- RabbitMQ, optional but recommended. When configured, supervisor corrective actions publish normal execution lifecycle messages so workflows and queues wake up after remediation.
- The artifact filesystem, required when artifact cleanup deletes expired file-backed artifact versions.

## What it does

### Runtime database retention

The supervisor purges runtime metadata according to database-backed retention settings. The effective retention settings are stored in `runtime_retention_config` and `runtime_retention_target_config`, loaded at the start of every cycle, and can be changed without restarting the service.

Use the web UI **Runtime Retention** page (`/retention`) or the API:

- `GET /api/v1/retention-config` requires `retention:read`
- `PUT /api/v1/retention-config` requires `retention:update`

Retention changes are audited as maintenance/admin audit events.

The default database seed enables all targets, runs every 3600 seconds, and uses
dry-run mode `false`. Each target can delete up to 100 batches of 1000 rows per
cycle. Both limits must be positive.

Each target run uses one fixed cutoff. Ordinary targets count eligible rows once.
Native targets probe only a bounded boundary/DEFAULT row cohort after selecting
whole expired leaves. Each batch commits separately. A target stops when a batch
deletes no rows, its batch budget is consumed, or cancellation is requested
between batches. Remaining backlog waits for the next cycle, so a busy target
does not prevent later targets and maintenance steps from running. The leader
advisory lock covers the whole cycle.

| Target key | Default max age | Purge behavior |
| --- | ---: | --- |
| `events` | 30 days | Drops fully expired daily `event` leaves; deletes boundary/DEFAULT rows by `created`. |
| `enforcements` | 30 days | Deletes only non-`created` rows older than the cutoff. |
| `executions` | 30 days | Deletes only terminal executions (`completed`, `failed`, `cancelled`, `timeout`, `abandoned`) by `updated`. |
| `execution_history` | 30 days | Drops fully expired daily leaves; deletes boundary/DEFAULT rows by `time`. |
| `worker_history` | 30 days | Deletes history rows by `time`. |
| `sensor_process_history` | 30 days | Deletes history rows by `time`. |
| `audit_events` | 90 days | Drops fully expired daily `audit_event` leaves; deletes boundary/DEFAULT rows by `created`. |
| `notifications` | 30 days | Deletes rows older than the cutoff. |
| `webhook_event_logs` | 30 days | Deletes rows older than the cutoff. |
| `inquiries` | 30 days | Deletes only terminal inquiries (`responded`, `timeout`, `cancelled`) by `updated`. |
| `work_queue_items` | 30 days | Deletes only terminal queue items (`completed`, `failed`, `skipped`, `cancelled`) by `updated`. |
| `work_queue_dispatches` | 30 days | Deletes only terminal dispatches (`completed`, `failed`, `released`, `cancelled`) by `updated`. |
| `pack_test_executions` | 30 days | Deletes old pack test execution rows by `execution_time`. |
| `execution_admission` | 30 days | Removes stale execution admission state/entries. |
| `workers` | 30 days | Deletes only stale `inactive`/`error` workers that are not cordoned and do not own active sensor processes. |
| `sensor_processes` | 30 days | Deletes only `stopped`/`failed` processes with `active_rule_count = 0`. |

Set a target's `max_age_seconds` to `null` to keep it forever while leaving it
visible in configuration. Targets have no separate `enabled` field.

Retention preserves operational-row protections, including waiting workflows
and undelivered runtime logs. Relationship IDs can intentionally dangle after
independent retention removes their source rows. Audit output separates confirmed
row deletions from partition drops. Dry runs report candidates without deleting
rows or dropping leaves. With `native_maintenance.enabled: false`, managed
parents use the bounded row-delete path instead of leaf expiry.

Whole-leaf expiry removes related summary groups, coverage, and invalidations in
the same transaction. Partial expiry deletes through the parent and appends
invalidations for the affected hours. Selection and deletion match locked
`(tableoid, ctid)` pairs in one statement. A `ctid` alone can identify different
rows in different leaves. Summary readers use raw counts for dirty hours, so
retention changes subsequent counts without waiting for rebuilding. There is no
separate summary-retention window or `continuous_aggregates` setting.

### Native partition and summary jobs

The supervisor owns three durable schedule rows in `native_maintenance_schedule`.
They share the existing session advisory leader lock but have independent due
times:

| Job | Default cadence | Work |
| --- | --- | --- |
| `partition` | 3600 seconds | Reconcile daily future leaves, then attempt the oldest DEFAULT day per parent. |
| `retention` | 3600 seconds | Runtime retention and cache cleanup, plus normal housekeeping. |
| `summary` | 300 seconds | Refresh bounded completed-hour summaries. |

The loop reloads persisted settings each cycle, with a polling delay of at most
60 seconds between cycles. A summary tick does not rerun retention, cache expiry,
or artifact cleanup. Startup and leader replacement force partition reconciliation
and run normal remediation. Summary
and retention remain cadence-bound. Claiming an attempt records its retry due
time before work starts. Successful bounded work uses its normal interval;
failed work retries after the smaller of its interval and 60 seconds. Restarts
preserve `next_due` and `last_success` rather than resetting another leader's work.

Each parent uses daily UTC half-open ranges and a DEFAULT partition. Normal
reconciliation covers today plus `partition_lookahead_days`, seven by default.
It verifies registry entries against actual parentage, bounds, ownership, and
attached valid indexes. Incompatible or unregistered objects cause an error.
The service does not adopt objects merely because their names match.

Reconciliation persists its next parent and per-parent day cursors. Reservations
advance before repair attempts, so a busy or oversized day cannot consume every
cycle under a one-operation cap. Future and DEFAULT candidates share this fair
ordering; restarts retain it. A later failure reports committed partial counters
rather than erasing successful earlier operations.

DEFAULT repair moves one whole day and attaches its leaf in a single transaction.
A bounded cap-plus-one probe detects oversized days. Such days stay queryable in
DEFAULT and report `deferred_over_budget`. Lock contention and operation deadlines
report deferred outcomes after rollback. Constraint validation of remaining
DEFAULT data shares the operation deadline. A committed detached staging table
would create a parent-query gap, so the implementation never commits that state.
Persistent backlog needs operator attention or revised budgets, not repeated
partial moves that hide data. DDL-role requirements are in
[PostgreSQL-only deployment](postgresql-only.md#database-roles-for-native-maintenance).

Summary builders use `native_summary_state` to serialize work by kind and
`native_summary_hour` to record each completed hour, including empty hours.
Producers append transactional `native_summary_invalidation` records without an
FK to builder state. Builders lock the source parent before their repeatable-read
snapshot, replace all groups for one hour, record coverage, and acknowledge only
the explicit visible IDs they captured. A maximum ID is not a commit watermark.
The default recent bootstrap covers 24 hours, then backfill expands backward
within raw retention. Dirty, bootstrap, and backfill work rotate across kinds
within bucket and elapsed-time limits. A cap on acknowledged invalidations does
not cap the source aggregation, which instead has a deadline.

Repeated producer writes skip notification INSERT work only when the matching
kind/hour marker has both their full transaction origin and actual tuple `xmin`.
This handles imported origin collisions without a shared unique-key lock.
Savepoints may generate extra safe invalidations. Builders continue acknowledging
only captured IDs.

Refresh transactions use local asynchronous commit for derived cache data only.
Coverage publication, summary replacement, and acknowledgment remain atomic. A
lost cache commit after a crash is rebuilt from durable raw rows and notifications.
Raw writes, expiry, DDL, and job scheduling keep normal commit durability.

Row retention also performs bounded housekeeping for fully expired summary hours.
It removes groups and coverage and acknowledges only visible invalidation IDs
under source-parent and builder-state locks. The partial cutoff hour survives.
This cleanup runs even with zero raw candidates or native jobs disabled, so old
worker-history and DEFAULT metadata cannot accumulate permanently. Failures keep
confirmed retention progress and can resume in a later cycle.

### Native status and counts

`GET /api/v1/retention-config/native-status` uses `RequireAuth` and requires
`retention:read`. It returns `observed_at`, native `enabled`, `partitions`,
`summaries`, and `schedule`. Status remains available when native jobs are
disabled. The inventories are observations, not one atomic cross-table snapshot.

- Partition status includes registered/future leaf counts, missing future
  coverage, `default_rows_at_least`, `default_count_exact`, and
  `oldest_default_day`. The row probe stops at `default_repair_row_limit + 1`.
  When `default_count_exact` is false, display an "at least" count. With the
  default limit, 1001 means at least 1001 rows, not the full backlog size.
- The API summary inventory reports actual `coverage_hours`,
  `dirty_notifications`, and distinct `dirty_hours`, plus oldest dirty/notification
  times and `latest_success`. `covered_since` and `covered_until` are extrema.
  They can enclose holes and do not prove continuous clean coverage.
  `coverage_hours` counts ledger entries, including hours dirtied since refresh.
  Notification counts describe appended records, not changed source rows.
- Supervisor summary-backlog logs use a separate bounded probe per kind, capped
  at `min(max_summary_invalidations_per_bucket, 10000) + 1`. Its
  `notifications_at_least` and `count_exact` describe a lower bound or an exact
  count. `oldest_observed_notification` describes only that probe's sample.
- Schedule status reports each job's `next_due` and `last_success`. Success can
  mean a bounded attempt completed while more backfill remains.
- Retention logs and audits report `deleted` as confirmed committed row deletions,
  `partitions_dropped` as confirmed leaf drops, and `partition_candidates`
  separately. `candidates_exact` applies to the row cohort. Native row probes
  exclude complete old leaves, including leaves deferred by the DDL cap. Never
  label a leaf count as deleted rows or sum these units into one total.

Structured `Native maintenance attempt finished` logs and
`maintenance.native.job_completed` audits report created leaves, moved rows,
deferred work, refreshed buckets, processed notifications, written groups,
serialization retries, lock/deadline failures, budget exhaustion, and duration
where supplied by that job. Confirmed progress survives later failures. Failed
or unconfirmed commits do not earn deletion or drop counts. Native audits and
alerts use fixed counters and SQLSTATEs rather than source record bodies or
arbitrary SQL error text. These are operational observations, not
performance-gate results.

### Artifact cleanup

Artifact version-count retention still happens when artifact versions are inserted. The supervisor handles the complementary cleanup path for artifacts using time-based policies (`days`, `hours`, or `minutes`):

1. Find expired artifact versions.
2. Delete the file-backed bytes when a version has a `file_path`.
3. For object-backed versions, mark the row `deleting`, wait the configured
   safety period, delete the recorded provider version, then delete the row.
4. Refresh artifact metadata or delete empty artifact metadata rows when no versions/data remain.

This is controlled by `maintenance.artifact_cleanup_enabled` and
`maintenance.artifact_cleanup_batch_size`. The same cycle checks abandoned
`pending` uploads and verifies non-log `ready` rows with `HEAD`. A missing or
mismatched ready object moves to `deleting`; it does not fall back to a bucket
scan or an unpinned read.

### Pack release and object retention

Activating a pack release records `inactive_since` on the previous release.
Each cycle retains the active release, every release referenced by durable
execution, enforcement, queue-item, or sensor-workload metadata, releases still
inside `pack_release_rollback_seconds`, and the newest
`pack_release_newest_inactive` inactive releases per pack. The supervisor
deletes at most `pack_release_cleanup_batch_size` release rows per cycle.

Pack, artifact, and log producers reserve an object key in
`object_maintenance_ledger` before upload and record the exact provider version
after upload. Metadata deletion changes that ledger entry to
`deletion_pending`; API handlers do not delete provider objects. The supervisor
waits `object_delete_grace_seconds`, claims a bounded batch with
`FOR UPDATE SKIP LOCKED`, and anti-joins the exact key and provider version
against all live metadata before deletion. Missing exact versions count as an
idempotent success. Failed and interrupted deletes remain in the ledger for a
later cycle. No collector path lists the bucket.

The structured cycle log `Object retention cycle completed` reports
`retained_releases`, `deleted_releases`, `pending_collection`,
`deleted_objects`, `deleted_bytes`, and `failures`.

### Storage migration and rollback snapshots

Stop pack publication and artifact uploads, take a PostgreSQL backup, then run:

```bash
cargo run --bin attune-supervisor -- --config config.development.yaml \
  migrate-storage
```

Use `--rollback-snapshot-seconds` to override the configured period. The
command uploads every legacy pack archive and artifact file under its immutable
key, verifies each target's byte count and SHA-256, verifies aggregate source
and target counts and bytes, and re-reads every source. It switches all selected
metadata in one transaction only after those checks pass. A failed or restarted
run reuses byte-identical objects by key and leaves unswitched metadata on the
filesystem path.

The default rollback snapshot period is seven days. During that period, the
old `archive_path` and `file_path` remain recorded and the source files remain
untouched. The supervisor removes expired snapshots in bounded batches. Do not
shorten the period until an object-backed restore has been tested.

The command and supervisor discover work from PostgreSQL rows. They do not list
the bucket. Provider inventory reports may be used for an offline operator
audit, but never decide request or execution correctness.

### PostgreSQL and object recovery point

Define a recoverable time `T` as the latest PostgreSQL recovery point for which
the object store still retains every exact version referenced by the restored
rows. Configure S3 Versioning or GCS soft delete so its retention window is at
least the PostgreSQL point-in-time recovery window plus
`maintenance.object_delete_grace_seconds`. Keep migration filesystem snapshots
for at least `maintenance.storage_rollback_snapshot_seconds`.

To recover, restore PostgreSQL to `T`, then restore or undelete the exact S3
version IDs or GCS generations referenced at `T`. Run the supervisor ready-row
reconciliation before reopening writes. If any referenced version cannot be
restored, that PostgreSQL point is not recoverable. Move `T` back to a point
whose complete object set is retained. Database backups and bucket retention
must be monitored as one recovery contract.

### Monitoring and alerts

When `maintenance.monitoring_enabled` is true, the supervisor emits deduplicated `core.alert` events for:

- non-terminal executions that have remained stale beyond `stuck_execution_seconds`
- leased queue items and leased/dispatched queue dispatches stale beyond `stuck_queue_seconds`
- retention lag, where eligible rows remain older than a target's max age plus `retention_lag_alert_seconds`
- missing future partitions, DEFAULT backlog, deferred partition operations,
  summary backlog, builder/status failures, and invalid persisted native settings

Alerts include a correlation id and use `alert_cooldown_seconds` for duplicate
suppression. Native status reporting caps alerts at `alert_limit_per_cycle` per
job's status pass. Native job-failure alerts are separate from that status cap.

### Corrective actions

When `maintenance.corrective_actions_enabled` is true, the supervisor applies guarded remediation for stale runtime state:

- stale `canceling` executions become `cancelled`
- stale `requested`, `scheduling`, `scheduled`, and unavailable-worker `running` executions become `abandoned`
- stale work queue dispatches and leased items are released, retried, failed, or cancelled according to queue state and retry limits
- execution admission entries tied to terminal/stale executions are removed, and queued entries may be promoted when capacity opens
- stale workflow rows are synchronized from terminal parent executions, or failed when all children are terminal and at least one child failed/cancelled/timed out/was abandoned
- stale terminal synthetic cache-iteration child completions are republished when their task is not yet terminal in workflow state
- scanning cache iterations owned by terminal workflows are reconciled to completed, failed, or cancelled, releasing their retention pins

Corrective mutations emit `core.alert` events and `maintenance.corrective_action.applied` audit events. If RabbitMQ is configured, the supervisor publishes `ExecutionCompleted` or `ExecutionRequested` messages for corrected/promoted executions so downstream workflow and queue handlers observe the change.

### Cache subsystem retention and freshness

The supervisor also owns lifecycle maintenance for the owner-scoped external-data
cache. See [Data Caches](../KEY_CACHE.md) for its namespace, generation, entry, and
ingest-chunk contracts. Cache maintenance runs inside the same retention cycle,
reusing its advisory lock and cadence rather than electing a second leader. All
cache data access goes through the cache repositories, including
`CacheStorageRepository` for storage observations and planner statistics. The
supervisor never issues ad hoc SQL against cache tables.

Each cycle:

1. **Expires abandoned unpublished generations.** A `staging` or `ready` generation older than `staging_expiry_seconds` is marked `failed` so the normal cleanup path reclaims it. Selection is bounded and state-specific, so newer failed generations cannot hide abandoned unpublished generations; additional expired generations are handled in later cycles.
2. **Reclaims cleanup candidates.** Each generation has one LIST partition. The repository atomically drops an eligible partition, subtracts its exact admitted-entry byte usage, and removes generation/chunk metadata. It rechecks failed or expired-retired state, the minimum traversal window, active pointers, and live workflow pins under locks. Entry rows are never individually deleted. Lock or statement deadline failures roll back and defer the candidate. A rotating cursor prevents a blocked oldest candidate from monopolizing later cycles.
3. **Drains tombstoned namespaces.** A tombstoned namespace already has its in-flight `staging`/`ready` generations moved to `failed` and its active generation retired immediately (see `CacheNamespaceRepository::tombstone`); once all of a tombstoned namespace's generations are gone, the supervisor deletes the namespace row (bounded by `max_namespaces_per_cycle`). Owner rows (identity/pack/action/sensor) stay protected by `cache_namespace`'s `ON DELETE RESTRICT` foreign keys until this drain completes.
4. **Emits freshness and repeated-failure alerts** (when `freshness_alerts_enabled`) as `core.alert` events: once when a namespace's active generation is older than its own nonzero `freshness_target_seconds` plus `freshness_alert_grace_seconds`, and once when its persisted consecutive refresh-failure streak reaches `staging_failure_alert_threshold`. A freshness target of `0` disables freshness classification and alerts for that namespace. Failing a generation increments the streak once, an idempotent repeated failure does not, and successful promotion resets it; supervisor restarts and failed-row cleanup do not erase the streak. Alerts carry only bounded, low-cardinality fields — numeric namespace/generation IDs, owner type, and counts — and are suppressed for `alert_cooldown_seconds` per correlation id. Namespace names, owner refs, external IDs, and cached values are never included.

5. **Refreshes planner statistics when due.** Generation creation, state changes,
   and deletion request parent/leaf `ANALYZE` through a persisted revision. The
   supervisor performs it in a separate bounded transaction after cleanup. It
   acknowledges only the revision observed before sampling. A concurrent request
   with a newer revision remains pending for another pass; a failed or deferred
   attempt acknowledges nothing. Cleanup already committed in other transactions
   remains committed if statistics refresh fails.

Statistics refresh analyzes `generation`, `external_id`, `id`, and `size_bytes`
on the entry parent and its leaves, not the cached JSON payload. The supervisor
requires effective ownership of `cache_entry` and its leaves. A role that can
read or write entries but does not own the parent is insufficient. These are
table-owner operations on stock PostgreSQL 16 or newer, not superuser operations.
Lock waits use `ddl_lock_timeout_milliseconds`; `ANALYZE` has its own
`statistics_statement_timeout_milliseconds` deadline. Dry-run mode skips it.

`statistics_interval_seconds` is a minimum interval between successful refreshes,
not an independent timer or a guarantee that pending work completes within that
interval. The supervisor checks it during retention cycles. With the default
3600-second retention cadence, a 300-second statistics interval still means at
most one attempt per retention cycle. Tune the retention cadence as well if
planner statistics need more frequent refreshes.

Atomic reclamation also removes bounded generation metadata. The upload contract
permits at most 10,000 ingest chunks per generation. At most 10,000 retained
workflow cache-iteration rows may reference one generation, including terminal
rows. Terminalizing an iteration releases its live pin but does not free this
retained-row quota. These caps bound metadata row work separately from entry
partition removal.

Cache ingestion, lifecycle writes, partition DDL, cleanup, and runtime-log writes
retain normal commit durability. Do not set global `synchronous_commit=off` for
cache maintenance. The local asynchronous-commit exception for rebuildable
hourly summaries does not apply to runtime logs or cache generations.

Active generations and retired generations still within their readable window
are never reclaimed. `CacheGenerationRepository::select_cleanup_candidates`
only returns `failed` rows or `retired` rows whose `readable_until` has passed.

Data Cache retention is capacity management for reconstructable Attune-local
snapshots, not business-record retention. Deleting an expired generation must
not destroy the only authoritative copy. Cache payloads are plaintext `JSONB`
and therefore contribute to PostgreSQL data files, WAL, replicas, and backups;
deployment encryption and backup controls apply to them.

## Configuration

### Runtime retention configuration

Retention is database-backed and hot-reloaded each supervisor cycle. The YAML `retention` block documents defaults and provides fallback config shape, but the runtime source of truth is the database once migrations have seeded it.

Example API payload:

```json
{
  "enabled": true,
  "check_interval_seconds": 3600,
  "batch_size": 1000,
  "max_batches_per_target": 100,
  "dry_run": false,
  "advisory_lock_key": 7821001,
  "native_maintenance": {
    "enabled": true,
    "partition_interval_seconds": 3600,
    "summary_interval_seconds": 300,
    "partition_lookahead_days": 7,
    "max_partition_operations_per_cycle": 32,
    "default_repair_row_limit": 1000,
    "lock_timeout_milliseconds": 250,
    "operation_timeout_milliseconds": 1000,
    "max_partition_cycle_milliseconds": 5000,
    "max_summary_buckets_per_cycle": 128,
    "max_summary_invalidations_per_bucket": 10000,
    "summary_bootstrap_hours": 24,
    "max_summary_cycle_milliseconds": 5000
  },
  "targets": {
    "events": { "max_age_seconds": 2592000 },
    "executions": { "max_age_seconds": 2592000 },
    "audit_events": { "max_age_seconds": 7776000 }
  }
}
```

| Field | Default | Description |
| --- | ---: | --- |
| `enabled` | `true` | Master switch for runtime retention. Maintenance jobs still use `maintenance.enabled`. |
| `check_interval_seconds` | `3600` | Persisted retention/housekeeping cadence. Native jobs have their own intervals. Must be greater than zero. |
| `batch_size` | `1000` | Maximum rows deleted per batch for each target. Must be greater than zero. |
| `max_batches_per_target` | `100` | Maximum delete batches per target per cycle. Must be greater than zero. |
| `dry_run` | `false` | Reports candidates and emits audit/log output without deleting rows or dropping leaves. Native reconciliation and building still run. |
| `advisory_lock_key` | `7821001` | PostgreSQL advisory lock key used to make multiple supervisors safe. |
| `targets.<target>.max_age_seconds` | target default | Maximum retained age. Use `null` to keep forever (purging disabled for that target). Must not be `0`. |

### Native maintenance configuration

`NativeMaintenanceConfig` is the `native_maintenance` JSON object on the singleton
`runtime_retention_config` row. GET and PUT of `/api/v1/retention-config` return
and persist it with the rest of `RetentionConfig`. The supervisor reloads it
without a restart. Startup seeds it from `retention.native_maintenance` only while
the database column is the migration default `{}`. After seeding, use the API
for changes. GET the current config, edit it, and PUT the complete object to
preserve other retention settings.

| Field | Default | Meaning |
| --- | ---: | --- |
| `enabled` | `true` | Enables partition reconciliation and summary building independently of `retention.enabled` and `maintenance.enabled`. Also selects leaf expiry for enabled runtime retention. |
| `partition_interval_seconds` | `3600` | Successful partition-job cadence. |
| `summary_interval_seconds` | `300` | Successful summary-job cadence. |
| `partition_lookahead_days` | `7` | Future days after today, with today included separately. |
| `max_partition_operations_per_cycle` | `32` | Reconciliation attempt cap; also limits leaves selected per target expiry call. Verified existing days do not consume the reconciliation cap. |
| `default_repair_row_limit` | `1000` | Whole-day repair cap. One additional probed row detects overflow. |
| `lock_timeout_milliseconds` | `250` | PostgreSQL lock-acquisition timeout. |
| `operation_timeout_milliseconds` | `1000` | Individual operation budget, shortened to the remaining cycle budget. |
| `max_partition_cycle_milliseconds` | `5000` | Elapsed-time budget for reconciliation or a target's leaf-expiry call. |
| `max_summary_buckets_per_cycle` | `128` | Maximum attempted hour refreshes per summary cycle. |
| `max_summary_invalidations_per_bucket` | `10000` | Maximum explicit visible notification IDs acknowledged per refresh. Remaining notifications keep the hour dirty. |
| `summary_bootstrap_hours` | `24` | Recent completed-hour bootstrap window before older missing-hour backfill. |
| `max_summary_cycle_milliseconds` | `5000` | Elapsed-time budget shared by summary planning, operations, and retries. |

The parser supplies these defaults for omitted native fields. Boolean `enabled`
must be a JSON boolean. Integer limits do not accept fractions, strings, or
`null`. All numeric values must be positive even when native work is disabled.
The interval and millisecond fields must fit a positive signed BIGINT,
`1..=9223372036854775807`. The two PostgreSQL timeout fields have a stricter
maximum of `2147483647` milliseconds. Signed count/day/hour fields must fit
positive BIGINT; `default_repair_row_limit` must be below its maximum to leave
room for the overflow probe. The validator imposes no smaller arbitrary
lookahead or bucket cap, and does not require the cycle budget to exceed the
operation budget. Timestamp and duration overflow can still reject an operation.

Invalid native limits produce HTTP 400. Malformed JSON field types can produce
HTTP 422 before validation. The API does not persist rejected values. If stored
values deserialize but fail native validation, the supervisor disables native
work for that cycle, uses valid row-cleanup fallback settings, and reports an
`invalid_config` alert. JSON that cannot deserialize is a config-load error.
Disabling native jobs does not remove partitions, summary data, or reader
coverage. `retention.dry_run` applies to retention, not to native reconciliation
or summary building.

### Maintenance configuration

Maintenance settings are loaded from the normal Attune configuration file and environment variables at supervisor startup. Restart the supervisor after changing these values.

```yaml
maintenance:
  enabled: true
  artifact_cleanup_enabled: true
  artifact_cleanup_batch_size: 100
  pack_release_retention_enabled: true
  pack_release_newest_inactive: 2
  pack_release_rollback_seconds: 604800
  pack_release_cleanup_batch_size: 100
  object_upload_abandon_seconds: 86400
  object_delete_grace_seconds: 86400
  storage_rollback_snapshot_seconds: 604800
  monitoring_enabled: true
  corrective_actions_enabled: true
  stuck_execution_seconds: 3600
  execution_remediation_seconds: 7200
  stuck_queue_seconds: 900
  queue_remediation_seconds: 1800
  admission_remediation_seconds: 1800
  retention_lag_alert_seconds: 86400
  alert_limit_per_cycle: 25
  alert_cooldown_seconds: 3600
```

| Field | Default | Description |
| --- | ---: | --- |
| `enabled` | `true` | Master switch for non-retention maintenance jobs. |
| `artifact_cleanup_enabled` | `true` | Enables cleanup of expired time-policy artifact versions. |
| `artifact_cleanup_batch_size` | `100` | Maximum expired artifact versions cleaned per cycle. |
| `pack_release_retention_enabled` | `true` | Enables bounded cleanup of inactive pack releases. |
| `pack_release_newest_inactive` | `2` | Newest inactive releases retained per pack after the rollback window. |
| `pack_release_rollback_seconds` | `604800` | Minimum age before an inactive, unpinned release can be removed. |
| `pack_release_cleanup_batch_size` | `100` | Maximum inactive releases removed per cycle. |
| `object_upload_abandon_seconds` | `86400` | Age after which a pending upload is reconciled. |
| `object_delete_grace_seconds` | `86400` | Delay before a deleting row's recorded object version is removed. |
| `storage_rollback_snapshot_seconds` | `604800` | Period that migrated filesystem sources remain available for rollback. |
| `monitoring_enabled` | `true` | Enables stuck-state and retention-lag alerting. |
| `corrective_actions_enabled` | `true` | Enables guarded DB remediation for stale executions, queues, workflow rows, and admission entries. |
| `stuck_execution_seconds` | `3600` | Alert threshold for stale non-terminal executions. |
| `execution_remediation_seconds` | `7200` | Remediation threshold for stale executions and workflow state. |
| `stuck_queue_seconds` | `900` | Alert threshold for stale queue leases and dispatches. |
| `queue_remediation_seconds` | `1800` | Remediation threshold for stale queue leases and dispatches. |
| `admission_remediation_seconds` | `1800` | Remediation threshold for stale execution admission entries. |
| `retention_lag_alert_seconds` | `86400` | Grace period beyond a target's retention window before alerting on remaining eligible rows. |
| `alert_limit_per_cycle` | `25` | Maximum monitoring/remediation alerts emitted per cycle. |
| `alert_cooldown_seconds` | `3600` | Duplicate-alert suppression window for the same correlation id. |

### Cache retention configuration

Cache retention settings are stored in the `cache_retention` JSON object on
the singleton `runtime_retention_config` row. They are returned and updated as
`cache_retention` through `GET/PUT /api/v1/retention-config` and reloaded at
the start of every supervisor cycle without a restart. The top-level YAML
block below is only a first-start bootstrap value while the database column is
still the migration default `{}`; after it is seeded, PostgreSQL is the source
of truth.

```yaml
cache_retention:
  enabled: true
  max_cleanup_cycle_milliseconds: 30000
  ddl_lock_timeout_milliseconds: 250
  ddl_creation_statement_timeout_milliseconds: 5000
  ddl_statement_timeout_milliseconds: 1000
  statistics_interval_seconds: 300
  statistics_statement_timeout_milliseconds: 5000
  max_generations_per_cycle: 50
  max_namespaces_per_cycle: 50
  min_traversal_window_seconds: 3600
  staging_expiry_seconds: 86400
  dry_run: false
  freshness_alerts_enabled: true
  freshness_alert_grace_seconds: 900
  staging_failure_alert_threshold: 3
  alert_cooldown_seconds: 3600
  alert_limit_per_cycle: 25
```

| Field | Default | Description |
| --- | ---: | --- |
| `enabled` | `true` | Master switch for the cache cleanup step within the retention cycle. |
| `max_cleanup_cycle_milliseconds` | `30000` | Reclamation budget checked before each generation, with remaining time limiting the next attempt. |
| `ddl_lock_timeout_milliseconds` | `250` | Maximum DDL lock wait before rollback and deferral. Also bounds refresh partition creation. |
| `ddl_creation_statement_timeout_milliseconds` | `5000` | Separate server-side statement deadline for refresh partition creation. Admission coordination precedes the short DDL deadlines. |
| `ddl_statement_timeout_milliseconds` | `1000` | Server-side deadline for one atomic drop operation. Unchanged by the creation budget. |
| `statistics_interval_seconds` | `300` | Minimum interval between successful parent/leaf statistics refreshes, checked during retention cycles. Allowed range `1..86400`. |
| `statistics_statement_timeout_milliseconds` | `5000` | Independent server-side statement deadline for parent/leaf `ANALYZE`. Allowed range `1..3600000` milliseconds. |
| `max_generations_per_cycle` | `50` | Maximum cleanup-candidate generations (`failed`, or `retired` past `readable_until`) processed per cycle. |
| `max_namespaces_per_cycle` | `50` | Maximum namespaces inspected for staging expiry/freshness per cycle, and maximum tombstoned-and-emptied namespaces deleted per cycle. Namespace inspection uses a rotating ID-keyset watermark and wraps safely, so low-ID namespaces cannot starve the rest of the fleet. |
| `min_traversal_window_seconds` | `3600` | Minimum time a retired generation must remain readable after retirement, enforced defensively by the supervisor in addition to the generation's own stored `readable_until`. |
| `staging_expiry_seconds` | `86400` | Age at which an unpublished `staging` or `ready` generation is treated as abandoned and marked `failed`. |
| `dry_run` | `false` | Reports staging-expiry/cleanup candidates and metrics without mutating rows. |
| `freshness_alerts_enabled` | `true` | Enables freshness and repeated-staging-failure `core.alert` emission. |
| `freshness_alert_grace_seconds` | `900` | Extra grace beyond a namespace's own `freshness_target_seconds` before its active generation is alert-worthy. |
| `staging_failure_alert_threshold` | `3` | Consecutive failed generations for one namespace before a repeated-failure alert is emitted. |
| `alert_cooldown_seconds` | `3600` | Duplicate cache-alert suppression window for the same correlation id. |
| `alert_limit_per_cycle` | `25` | Maximum cache alerts of each kind (freshness, repeated-failure) emitted per cycle. |

Cleanup-cycle and DDL timeout fields also accept integers from `1` through
`3600000` milliseconds. These limits remain required when cache maintenance is
disabled. Creation, cleanup, and statistics statement deadlines are independent;
raising one does not raise the others. Read the complete retention object before
editing it, then PUT the complete object to preserve native settings and other
cache policies. The web Runtime Retention page exposes these storage budgets.

Each enabled cache-maintenance cycle emits structured operational metric
events through the shared service-log observability pipeline. The
`cache_maintenance_cycle` event includes active-generation age/freshness,
observed refresh failures, records/storage, cleanup backlog and saturation,
expired staging/snapshot cleanup, and maintenance duration/count fields.
Additional `cache_scope_storage` events aggregate storage and refresh failures
only by the bounded `owner_type` label. Namespace/owner IDs, names, refs,
generation IDs, and external IDs are never metric labels.

The cycle event also reports:

| Fields | Interpretation |
| --- | --- |
| `registered_partitions` | Current attached cache-entry leaf count, including empty and unpublished generations. |
| `storage_observed` | Whether the storage observation succeeded. When false, inventory, backlog, and pending-statistics fields are unavailable, not evidence of zero backlog or clean statistics. |
| `partitions_created_total`, `partitions_dropped_total` | Persisted cumulative committed partition lifecycle counts, not per-cycle counts. |
| `cleanup_backlog_total`, `oldest_cleanup_age_seconds` | Full eligible generation backlog and age of its oldest eligibility timestamp. Unlike `cleanup_backlog_generations`, this is not just the selected bounded cohort. |
| `reclamation_duration_ms` | Wall time spent in reclamation attempts, including waits and deferrals. This is separate from statistics duration. |
| `statistics_duration_ms`, `statistics_refreshed`, `statistics_pending` | Statistics attempt duration, whether a refresh committed, and whether an unacknowledged lifecycle revision remains. |
| `statistics_age_seconds` | Age of the last successful statistics refresh. With `storage_observed: true`, an absent age means no successful refresh has been recorded. With `storage_observed: false`, the age is unavailable. |
| `statistics_lock_deferrals`, `statistics_deadline_deferrals`, `statistics_failures` | Separate contention, deadline, and other-failure counts for statistics work. |

`cleanup_backlog_saturated` describes the bounded selected cohort, not the full
backlog. Partition counts, deleted entry counts, and reclaimed bytes have
different units. Do not add them into one deletion total. A statistics failure
does not erase confirmed cleanup counters.

Audit details report `cache_cleanup_cycle_partial_failure` with a failure outcome
when maintenance, cleanup, or statistics failure counters are nonzero. They
retain the confirmed progress from earlier committed transactions. A successful
audited cycle reports `cache_cleanup_cycle_completed` with a success outcome.
Lock and deadline deferrals have separate counters; a deferred statistics refresh
is not itself a statistics failure. Do not interpret a partial-failure audit as
proof that all cleanup rolled back.

Fleet-wide reporting-query optimization is deferred. Metadata observations and
their duration still need workload measurements; the new counters do not prove a
reporting or maintenance speedup.

Combine these events with PostgreSQL and infrastructure metrics for total
table/index size, disk headroom, WAL generation, replica lag, checkpoints,
dead tuples/autovacuum, backup duration/size, and tested restore duration.
Cache metrics do not replace database-capacity or backup monitoring.

Aggregate admission is separate from supervisor retention. The startup-loaded
`cache_admission` block enforces global/per-owner live namespace and physical
byte limits plus unpublished generations per owner. Physical accounting keeps
all generation states and tombstoned namespaces charged until this cleanup loop
deletes their entries. See `docs/configuration/configuration.md` for defaults.

### Environment overrides

All YAML fields can be overridden with `ATTUNE__` environment variables. Common supervisor-related examples:

```bash
ATTUNE_CONFIG=/etc/attune/attune.yaml
ATTUNE__DATABASE__URL=postgresql://attune:attune@localhost:5432/attune
ATTUNE__RABBITMQ__URL=amqp://attune:attune@localhost:5672/attune
ATTUNE__MAINTENANCE__CORRECTIVE_ACTIONS_ENABLED=false
ATTUNE__MAINTENANCE__ALERT_COOLDOWN_SECONDS=7200
ATTUNE__CACHE_RETENTION__DRY_RUN=true
ATTUNE__CACHE_RETENTION__STAGING_EXPIRY_SECONDS=3600
RUST_LOG=info
```

Use the retention API for runtime, cache, and native retention changes after the
database has been initialized. Cache and native environment overrides affect
only their initial seeding. Native settings use the nested
`ATTUNE__RETENTION__NATIVE_MAINTENANCE__` prefix.

## Running the supervisor

### Docker Compose

`docker-compose.yaml` includes a `supervisor` service using `attune-supervisor`. It mounts the same Docker config, artifact volume, and blob volume as the API:

```bash
docker compose up -d supervisor
docker compose logs -f supervisor
```

### Local development

```bash
make run-supervisor
```

Or directly:

```bash
cargo run --bin attune-supervisor -- --config config.development.yaml
```

### Linux packages

Package installs include a systemd unit for service packages:

```bash
sudo systemctl enable --now attune-supervisor
sudo journalctl -u attune-supervisor -f
```

Set required secrets and service URLs in `/etc/attune/environment` and `/etc/attune/attune.yaml`.

### Kubernetes

The Helm chart exposes:

```yaml
supervisor:
  replicaCount: 1
  resources: {}
```

Keep `replicaCount: 1` unless you intentionally want advisory-lock-protected standby replicas.

## Operational guidance

- Start with `dry_run: true` when lowering retention windows in an existing environment. Review logs/audit entries, then switch to `false`.
- Keep audit-event retention longer than other runtime targets unless compliance requirements say otherwise.
- Set `max_age_seconds: null` rather than a very large value to retain a target indefinitely.
- Do not run aggressive retention windows in test suites unless the tests are isolated from workflow/retry/concurrency assertions.
- If corrective actions are too aggressive for an environment, set `maintenance.corrective_actions_enabled: false`; monitoring alerts can remain enabled.
- If RabbitMQ is unavailable, the supervisor can still mutate database state, but workflow/queue wakeups from corrective actions will not be published until another component observes the state.
- Start with `cache_retention.dry_run: true` when tuning cache cleanup bounds in an existing environment, the same way you would for runtime retention.
- Cache cleanup drops one generation partition at a time and deletes a tombstoned namespace only after its generations are gone. Monitor lock deferrals, deadline deferrals, and the generation/time budgets before raising limits. A drop briefly takes ACCESS EXCLUSIVE on the entry parent and can block unrelated cache traffic. Defaults remain provisional pending the workload gates in `docs/plans/cache-generation-partitioning.md`.
- Record warning/action thresholds for cache capacity, cleanup backlog, WAL and replica lag, cache API SLOs, and backup/restore objectives in the deployment runbook. Persistent threshold breaches require quota/retention tuning or capacity expansion; isolate cache tables on dedicated PostgreSQL before they degrade the Attune control plane.
- Move the data to an independent database/warehouse/search or object-storage system when it is not reconstructable, needs authoritative retention or general querying, primarily serves non-Attune consumers, or remains outside its SLO/capacity envelope after PostgreSQL isolation. A dedicated PostgreSQL cache cluster is an intermediate scaling step, not permission to turn Data Caches into a system of record.
- Namespace and aggregate owner/deployment quotas are hard admission controls. Treat monitoring thresholds as earlier capacity warnings; quota rejection is the final guard and cleanup must physically delete entries before physical-byte capacity becomes available again.
