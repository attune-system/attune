# Attune Supervisor

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

Each target run uses one fixed cutoff and counts eligible rows once. Each batch
commits separately. A target stops when a batch deletes no rows, its batch budget
is consumed, or cancellation is requested between batches. Remaining backlog
waits for the next cycle, so a busy target does not prevent later targets and
maintenance steps from running. The leader advisory lock covers the whole cycle.

| Target key | Default max age | Purge behavior |
| --- | ---: | --- |
| `events` | 30 days | Deletes `event` rows by `created`. |
| `enforcements` | 30 days | Deletes only non-`created` rows older than the cutoff. |
| `executions` | 30 days | Deletes only terminal executions (`completed`, `failed`, `cancelled`, `timeout`, `abandoned`) by `updated`. |
| `execution_history` | 30 days | Deletes history rows by `time`. |
| `worker_history` | 30 days | Deletes history rows by `time`. |
| `sensor_process_history` | 30 days | Deletes history rows by `time`. |
| `audit_events` | 90 days | Deletes `audit_event` rows by `created`. |
| `notifications` | 30 days | Deletes rows older than the cutoff. |
| `webhook_event_logs` | 30 days | Deletes rows older than the cutoff. |
| `inquiries` | 30 days | Deletes only terminal inquiries (`responded`, `timeout`, `cancelled`) by `updated`. |
| `work_queue_items` | 30 days | Deletes only terminal queue items (`completed`, `failed`, `skipped`, `cancelled`) by `updated`. |
| `work_queue_dispatches` | 30 days | Deletes only terminal dispatches (`completed`, `failed`, `released`, `cancelled`) by `updated`. |
| `pack_test_executions` | 30 days | Deletes old pack test execution rows by `execution_time`. |
| `execution_admission` | 30 days | Removes stale execution admission state/entries. |
| `workers` | 30 days | Deletes only stale `inactive`/`error` workers that are not cordoned and do not own active sensor processes. |
| `sensor_processes` | 30 days | Deletes only `stopped`/`failed` processes with `active_rule_count = 0`. |

Set a target's `enabled` field to `false` to skip it. Set `max_age_seconds` to `null` to keep that target forever while still leaving it visible in configuration.

Retention preserves operational-row protections, including waiting workflows
and undelivered runtime logs. Relationship IDs can intentionally dangle after
independent retention removes their source rows. Audit output reports actual
rows deleted, including history and audit targets. Dry runs count candidates
without deleting rows.

Hourly analytics use ordinary views over retained raw records. Counts change as
soon as retention deletes source rows; there is no separately retained summary
or `continuous_aggregates` setting. See [PostgreSQL-only deployment](postgresql-only.md).

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

Alerts include a correlation id and are suppressed for `alert_cooldown_seconds` to avoid alert storms. Each cycle emits at most `alert_limit_per_cycle` monitoring alerts.

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

The supervisor also owns lifecycle maintenance for the owner-scoped external-data cache (`cache_namespace`, `cache_generation`, `cache_entry`, `cache_ingest_chunk` — see `docs/KEY_CACHE.md`). This runs as a distinct step **inside the same retention cycle**, reusing its advisory lock and cadence rather than electing a second leader. All cache data access goes through `CacheNamespaceRepository`, `CacheGenerationRepository`, and `CacheEntryRepository`; the supervisor never issues ad hoc SQL against cache tables.

Each cycle:

1. **Expires abandoned unpublished generations.** A `staging` or `ready` generation older than `staging_expiry_seconds` is marked `failed` so the normal cleanup path reclaims it. Selection is bounded and state-specific, so newer failed generations cannot hide abandoned unpublished generations; additional expired generations are handled in later cycles.
2. **Drains cleanup candidates.** Generations that are `failed`, or `retired` past their `readable_until` (and past the supervisor's own defensive `min_traversal_window_seconds` check), have their entries deleted in indexed bounded batches (`batch_size` per call, up to `max_batches_per_generation` batches per generation per cycle) before the emptied generation row itself is deleted. A generation with more entries than one cycle's bound allows is simply picked up again next cycle — the foreign-key cascade is a safety net, not the routine deletion path.
3. **Drains tombstoned namespaces.** A tombstoned namespace already has its in-flight `staging`/`ready` generations moved to `failed` and its active generation retired immediately (see `CacheNamespaceRepository::tombstone`); once all of a tombstoned namespace's generations are gone, the supervisor deletes the namespace row (bounded by `max_namespaces_per_cycle`). Owner rows (identity/pack/action/sensor) stay protected by `cache_namespace`'s `ON DELETE RESTRICT` foreign keys until this drain completes.
4. **Emits freshness and repeated-failure alerts** (when `freshness_alerts_enabled`) as `core.alert` events: once when a namespace's active generation is older than its own nonzero `freshness_target_seconds` plus `freshness_alert_grace_seconds`, and once when its persisted consecutive refresh-failure streak reaches `staging_failure_alert_threshold`. A freshness target of `0` disables freshness classification and alerts for that namespace. Failing a generation increments the streak once, an idempotent repeated failure does not, and successful promotion resets it; supervisor restarts and failed-row cleanup do not erase the streak. Alerts carry only bounded, low-cardinality fields — numeric namespace/generation IDs, owner type, and counts — and are suppressed for `alert_cooldown_seconds` per correlation id. Namespace names, owner refs, external IDs, and cached values are never included.

Active generations, and retired generations still within their readable window, are never touched — `CacheGenerationRepository::select_cleanup_candidates` only returns `failed` rows or `retired` rows whose `readable_until` has already passed.

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
| `check_interval_seconds` | `3600` | Delay between supervisor cycles. Must be greater than zero. |
| `batch_size` | `1000` | Maximum rows deleted per batch for each target. Must be greater than zero. |
| `max_batches_per_target` | `100` | Maximum delete batches per target per cycle. Must be greater than zero. |
| `dry_run` | `false` | Counts candidates and emits audit/log output without deleting rows. |
| `advisory_lock_key` | `7821001` | PostgreSQL advisory lock key used to make multiple supervisors safe. |
| `targets.<target>.max_age_seconds` | target default | Maximum retained age. Use `null` to keep forever (purging disabled for that target). Must not be `0`. |

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
  batch_size: 1000
  max_batches_per_generation: 20
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
| `batch_size` | `1000` | Maximum `cache_entry` rows deleted per bounded batch call. |
| `max_batches_per_generation` | `20` | Maximum entry-deletion batches performed for one cleanup-candidate generation per cycle. A generation with more entries remaining is picked up again next cycle. |
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

Each enabled cache-maintenance cycle emits structured operational metric
events through the shared service-log observability pipeline. The
`cache_maintenance_cycle` event includes active-generation age/freshness,
observed refresh failures, records/storage, cleanup backlog and saturation,
expired staging/snapshot cleanup, and maintenance duration/count fields.
Additional `cache_scope_storage` events aggregate storage and refresh failures
only by the bounded `owner_type` label. Namespace/owner IDs, names, refs,
generation IDs, and external IDs are never metric labels.

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

Use the retention API for runtime and cache retention changes after the
database has been initialized. Environment overrides affect only initial cache
retention seeding.

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
- Prefer disabling a target or setting `max_age_seconds: null` over setting a very large value when you want to retain data indefinitely.
- Do not run aggressive retention windows in test suites unless the tests are isolated from workflow/retry/concurrency assertions.
- If corrective actions are too aggressive for an environment, set `maintenance.corrective_actions_enabled: false`; monitoring alerts can remain enabled.
- If RabbitMQ is unavailable, the supervisor can still mutate database state, but workflow/queue wakeups from corrective actions will not be published until another component observes the state.
- Start with `cache_retention.dry_run: true` when tuning cache cleanup bounds in an existing environment, the same way you would for runtime retention.
- Cache cleanup deletes entries in bounded batches before deleting a generation, and only deletes a tombstoned namespace once its generations are gone — a namespace that keeps accumulating cleanup-candidate generations faster than `max_generations_per_cycle`/`max_batches_per_generation` allow needs those bounds raised rather than a one-off manual purge.
- Record warning/action thresholds for cache capacity, cleanup backlog, WAL and replica lag, cache API SLOs, and backup/restore objectives in the deployment runbook. Persistent threshold breaches require quota/retention tuning or capacity expansion; isolate cache tables on dedicated PostgreSQL before they degrade the Attune control plane.
- Move the data to an independent database/warehouse/search or object-storage system when it is not reconstructable, needs authoritative retention or general querying, primarily serves non-Attune consumers, or remains outside its SLO/capacity envelope after PostgreSQL isolation. A dedicated PostgreSQL cache cluster is an intermediate scaling step, not permission to turn Data Caches into a system of record.
- Namespace and aggregate owner/deployment quotas are hard admission controls. Treat monitoring thresholds as earlier capacity warnings; quota rejection is the final guard and cleanup must physically delete entries before physical-byte capacity becomes available again.
