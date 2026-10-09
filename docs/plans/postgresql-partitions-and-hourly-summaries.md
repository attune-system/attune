# Supervisor-managed PostgreSQL partitions and hourly summaries

Status: fresh-install migration baseline and operational acceptance in progress;
reporting-performance optimization deferred by the user on 2026-10-07.
Date: 2026-10-06.
Historical phase-1 baseline: `59c9a9a0d00fbc431cc01a5c512da243c86b6a6b`,
`timescaledb removal - phase 1`.
Worktree: `/mnt/wdc/attune-worktrees/remove-timescaledb`.

## Goal and scope

Recover cheap time-based expiry and precomputed historical analytics on stock
PostgreSQL. The supervisor owns partition lifecycle and bounded summary updates.
The executor and API do not create partitions or run maintenance on write paths.

This phase follows the completed [TimescaleDB removal](remove-timescaledb.md).
Keep PostgreSQL 16+ support, repository-owned SQL, existing authorization scopes,
and canonical v1 contracts. No TimescaleDB dependency returns.

On 2026-10-07 the user confirmed that there are no production databases. Target
fresh installations and revise the canonical pre-production migrations directly.
Populated phase-1 conversion is superseded, not a remaining acceptance gate.
From 1.0.0, released migration bytes stay immutable and forward migrations must
preserve data. See [Migration policy](../deployment/postgresql-only.md#migration-policy-and-development-resets).

The chosen implementation uses daily native partitions and ordinary hourly
summary tables. PostgreSQL materialized views recompute their full result during
standard refresh, so they do not provide the bounded incremental work required
here. The summary tables are the persisted materialization of the hourly counts.

Summaries describe currently retained raw records. Their retention follows their
source table. Retaining summaries beyond raw-data expiry is a separate product
change that requires different late-data and historical-query semantics.

## Baseline and acceptance targets

Reporting-performance work is deferred for the current delivery. The limits below
remain the recorded benchmark targets; they have not been lowered or reported as
passed. Finish correctness, full-stack validation, SQLx checks, and fresh-install
verification before assessing operational readiness. Invalidation overhead also affects
source writes and remains a documented deployment risk.

The baseline uses ordinary tables, bounded row deletes, and timestamp-bounded
analytics queries. Its [workload evidence](../research/timescaledb-removal-workload.md)
records these disk-backed warm server p95 measurements at four-query concurrency:

| Query | Stock PostgreSQL 18 | Timescale precomputed baseline |
| --- | ---: | ---: |
| Status counts, bounded raw query | 57.029 ms | 0.915 ms for the summary read |
| Event counts, bounded raw query | 45.600 ms | 0.308 ms for the summary read |
| Direct ordinary status hourly view | 474.772 ms | 0.915 ms |
| Direct ordinary event hourly view | 579.021 ms | 0.308 ms |

The slowest stock 100-batch deletion loop took 4.578 seconds for 100,000 rows.
These are synthetic database measurements, not production or endpoint timings.

The next phase must demonstrate:

- The same metric counts, filters, null dimensions, and UTC boundary behavior.
- Warm p95 below 10 ms for fully summarized event/status queries on the existing
  fixture, measured through the actual repository query shapes.
- Bounded raw tails remain within the baseline 500 ms query limit.
- Whole-partition expiry eliminates row-delete/vacuum work for that partition.
  Compare equal expired cohorts rather than one partition drop against a
  100,000-row subset of a larger backlog.
- Partition maintenance does not stall normal writers indefinitely. Start with
  a 250 ms lock-acquisition limit and a one-second parent-lock hold target;
  validate those limits before adopting them as defaults.
- Dirty-bucket tracking costs at most 10 percent additional write time under the
  declared four-writer fixture. Record latency and throughput together.

Extend the fixture with the configured default horizons: 30 days of events and
execution history, and 90 days of audit records. Keep the seven-day profile for
an identical baseline comparison. Treat the acceptance limits as release gates,
not measured promises.

## Partition contract

### Managed tables

| Table | Partition key | Initial interval |
| --- | --- | --- |
| `event` | `created` | One UTC day |
| `execution_history` | `time` | One UTC day |
| `audit_event` | `created` | One UTC day |

Keep `execution` and `enforcement` ordinary because they have mutable lifecycle
predicates and active-work protections. Keep `worker_history` and
`sensor_process_history` ordinary in this phase; their measured volumes are small.
Worker status can still use an hourly summary without partitioning its source.

Use typed internal table identifiers and UTC half-open ranges, `[start, end)`.
Partition names come from those identifiers and UTC dates, not user-provided SQL.
Queries remain unqualified and use the connection's `search_path`.

### Creation and fallback routing

- Migrations create today's partition, seven days ahead, and a DEFAULT partition
  for each managed parent. The DEFAULT partition prevents a missed maintenance
  tick or backdated record from stopping ingress.
- Run partition reconciliation at startup and on an hourly cadence. Create
  missing future partitions idempotently before expiry work.
- Reconcile actual PostgreSQL catalogs against an explicit managed registry.
  Verify parent, bounds, indexes, and ownership before adopting an existing object.
- Backdated records stay queryable. Repair one whole day atomically within row and
  time limits. A day exceeding the budget remains queryable in DEFAULT and returns
  a deferred outcome. Committed moves into detached staging are prohibited because
  they create a query gap. The SQL prototype verified this on both PG versions.
- A bound assertion can require scanning or locking DEFAULT data. Budget that
  work, use validated constraints where possible, and retry a busy operation.
  Never put unbounded DEFAULT draining on the normal write path.
- Alert on missing future coverage, persistent DEFAULT backlog, incompatible
  catalog objects, or insufficient DDL privileges.

The supervisor's database role must own the managed tables or have membership
in their owner role. Document schema-local CREATE/ALTER/DROP requirements. No
superuser or production database-creation privilege should be necessary.

### Expiry and audit accounting

- A partition is eligible only when its upper bound is at or before the fixed
  retention cutoff. No retained row can be dropped because another row is old.
- Use a short transactional `DROP TABLE` operation initially. A DEFAULT partition
  prevents the usual concurrent-detach path, so do not assume
  `DETACH PARTITION CONCURRENTLY` is available.
- Lock the managed parent before dependent objects in a documented order.
  Test lock contention with long reads, concurrent inserts, and another supervisor.
- Expiry updates corresponding summaries and coverage in the same transaction.
  A committed partition drop must not leave its old counts visible to new reads.
- Use bounded row deletion for the partially expired boundary partition and
  expired DEFAULT rows. Those deletes invalidate their affected summary hours
  transactionally.
- Partition drop and row deletion have different units. Record actual
  `partitions_dropped` and confirmed `rows_deleted` separately. Do not perform a
  full row count solely to manufacture a deleted-row total for fast partition DDL.
  Any row estimate is explicitly an estimate.
- Preserve known committed progress on errors, cancellation between operations,
  dry-run behavior, unlimited retention, leader locking, and lag alerts.

The ID-less history cleanup helper must select `(tableoid, ctid)` pairs when it
operates on a partitioned parent. A `ctid` alone is not unique across partitions.
Keep those row locations statement-local and lock selected rows before deletion.

## Hourly-summary contract

### Sources and storage

Create four ordinary summary tables with explicit columns and indexes:

| Summary | Source predicate | Dimensions |
| --- | --- | --- |
| Execution status | `execution_history` rows with `status` in `changed_fields` | UTC hour, action ref, new status |
| Execution creation | `execution_history.operation = 'INSERT'` | UTC hour, action ref |
| Event volume | `event` creation records | UTC hour, trigger ref |
| Worker status | `worker_history` rows with `status` in `changed_fields` | UTC hour, worker name, new status |

Counts are BIGINT. Preserve nullable refs/status values using PostgreSQL's
`UNIQUE NULLS NOT DISTINCT` or an equally explicit key representation. Do not
collapse nulls into user-visible empty strings or an `unknown` value unless the
specific existing metric already does that.

Keep direct live execution/enforcement volume analytics on their current bounded
queries. Their lifecycle and retention semantics differ from append-only history.
Dashboard execution counts continue counting terminal transitions, including
retry outcomes, rather than changing into execution-creation counts.

### Coverage and dirty buckets

Persist per-summary coverage with an inclusive start and exclusive end, last
successful refresh, and current bootstrap/backfill progress. Empty completed hours
must have coverage even when there is no summary row. Absence of a row alone does
not distinguish zero activity from an hour that has never been materialized.

Add an append-only transactional invalidation queue with BIGINT record IDs,
summary kind, and UTC hour:

- Source inserts mark relevant hours dirty in the same transaction as the source
  row. Backdated commits invalidate old hours, not merely the recent window.
- Row deletion or a grouping-field correction invalidates the affected hours.
  Partition drop removes the affected materialization directly because leaf
  deletion does not run ordinary row DELETE triggers.
- Statement triggers append notifications for affected kinds/hours. Repeated writes
  reuse only a marker whose full transaction origin and actual tuple `xmin` belong
  to the current producer. The origin lookup index is nonunique. Imported origin
  values cannot suppress a later writer's notification. Savepoints can retain
  extra markers conservatively. Producers do not lock shared bucket markers or
  reference builder state by FK.
- Refresh under source-parent locks and a repeatable-read snapshot. Serialize
  builders using per-kind state, capture bounded explicit visible notification IDs,
  replace all groups for the hour, publish coverage, and acknowledge only those
  captured IDs atomically. Group removal and empty hours must work as well as inserts.
- Never acknowledge by maximum ID. A lower ID can commit after the snapshot and
  must remain pending. The SQL probes verified both late-commit cases.
- Initial coverage building uses the same snapshot protocol. Persist explicit
  per-hour coverage so no interval envelope conceals an unfinished hour.

Only the derived refresh transaction uses `SET LOCAL synchronous_commit = off`.
Summary replacement, coverage publication, and exact-ID acknowledgment remain one
atomic commit. A crash can discard that cache commit, leaving durable source rows
and invalidations available for rebuilding. Source writes, raw retention, partition
DDL, and scheduling retain normal durability. The crash verifier exercises both
discarded and retained cache commits on PostgreSQL 16 and 18.

Use complete UTC hours for summaries. The current hour and any uncovered or dirty
hour use raw queries. Query planning must account for dirty hours inside an older
coverage range; one global watermark is not sufficient proof of freshness.

Bootstrap the most recent 24 hours first, then expand coverage backward within
retained data. Bound catch-up work and keep refreshing recent completed hours
while historical backfill runs. Startup must not require a full-history rebuild.

### Retention consistency

There is no independent summary-retention setting in this phase:

- Dropping an expired daily source partition removes its associated summaries
  and updates coverage atomically.
- Partial row deletion marks the changed hour dirty in the deletion transaction.
  Readers use raw records for that hour until recomputation finishes.
- DEFAULT draining is a physical row move, not a business deletion. Preserve
  counts and use bounded invalidation rather than double-counting moved records.
- Expiry of ordinary `worker_history` follows the same invalidation rules.
- If summary maintenance is unavailable, raw retention can still progress through
  the repository's atomic invalidation/removal contract. Readers must not serve
  stale pre-deletion totals as current data.

This policy preserves phase-1 analytics semantics. Longer-lived historical totals
would require a separate retained-summary policy and a defined rule for inserts
whose original bucket's raw data has already expired.

## Read planning and API behavior

Keep the read strategy inside `AnalyticsRepository`:

1. Acquire source-parent read locks in a stable order and pin summary metadata,
   dirty markers, summary rows, and raw data to one consistent transaction snapshot.
   Prove correctness when a partition expires during a request.
2. Convert the endpoint's existing bucket rules into exact source-time bounds.
   Dedicated hourly endpoints include complete final hours. Dashboard queries
   still clip their final hour to the exclusive request end.
3. Use summaries only for fully included, covered, clean hours. Use raw queries
   for partial boundaries, dirty hours, and missing coverage.
4. Apply identical authorization/ref filters on both paths before totals are
   combined. Preserve deterministic ordering and BIGINT count types.
5. Merge disjoint ranges without double-counting. Coalesce raw ranges and keep
   the query budget bounded rather than issuing one query per hour.

If coverage metadata is unavailable, use the existing indexed raw path. Do not
interpret unavailable maintenance state as zero activity. Summary refresh failures
must not make otherwise available raw analytics fail.

Return truthful metadata for raw-only, summary-only, and summary-plus-raw reads.
Keep stale response-cache handling separate from database materialization state.
Define coverage/freshness fields in the canonical Rust DTOs and source catalog,
then regenerate web and Python clients. Do not introduce a v2 dashboard contract.
Keep the existing ordinary hourly views as raw SQL relations;
application reads use the repository planner, not computed-bucket view filters.

## Supervisor scheduling and ownership

The current supervisor couples maintenance to the retention interval. Add distinct
due times while keeping one leader and bounded work:

- Partition reconciliation defaults to hourly and runs at startup.
- Summary refresh defaults to every five minutes.
- Retention keeps its existing configured cadence.
- Every job has a batch/time budget, cancellation boundaries, and durable progress.
  Catch-up must not starve cache retention, artifact cleanup, or operational remediation.
- A restarted or replacement leader resumes from persisted coverage, dirty buckets,
  and actual partition catalogs. Work converges after crashes and retries.

Use the existing maintenance lock consistently across supervisor instances. Keep
DDL, refresh, and retention transactions short. Do not hold a transaction open
across the entire maintenance cycle.

Persist validated partition/summary maintenance settings through the existing
runtime-config pattern. Expose operator settings and status through protected
Axum routes with `RequireAuth` and owning repository methods. Metrics/audits need
coverage lag, dirty backlog, DEFAULT backlog, lock retries, refresh cost, and
separate partition/row expiry counts.

## Ordered implementation units

### 1. Prove the SQL protocols and performance fixture

Dependencies: the pinned phase-1 commit.

- [ ] Add rerunnable SQL/repository probes for DEFAULT draining and attachment,
  partition expiry locks, and refresh-versus-writer invalidation.
- [ ] Test both PostgreSQL 16 and 18, including nullable summary dimensions,
  rollback, transaction isolation, and startup without DDL privileges.
- [ ] Extend `scripts/measure-timescaledb-removal.py` or add an owning native
  maintenance runner that exercises real repository methods on identical fixtures.
- [ ] Record partition-drop versus row-delete cost, summary versus raw query cost,
  and the ingestion cost of dirty-marker writes. Preserve rejected samples.

Exit gate: the concurrency protocols are demonstrated with real transactions and
bounded lock waits. The fixture compares identical coverage and data volumes.

### 2. Define the native fresh-install schema

Dependencies: unit 1.

- [x] Create the three RANGE-partitioned parents and DEFAULT children directly in
  their original table-creation migrations. Remove heap-conversion code.
- [x] Keep fresh migration checksums and runner ownership consistent for both
  migration histories; validate no-op reruns and checksum rejection.
- [x] Define BIGINT sequences, indexes, required outbound FKs, views, history/audit
  triggers, grants, and denormalized references coherently in the fresh schema.
- [x] Use `(id, created)` primary keys for partitioned event/audit tables. Audit
  ID-only upsert/lookup assumptions; a sequence remains the normal global ID source,
  but a composite key does not enforce global ID uniqueness by itself.
- [x] Add managed-partition metadata, summary storage, coverage state, and dirty
  tracking. Update fixture seeding helpers for explicit historical partition ranges.
- [x] Verify fresh installation on both PostgreSQL versions with both runners.
  Confirm initial UTC coverage, DEFAULT routing, and transactional failure behavior.

Exit gate: both runners create the same native schema from an empty database;
normal writes produce correct history, audits, and invalidations.

### 3. Implement partition lifecycle and hybrid retention

Dependencies: unit 2.

- [ ] Add owning repositories for partition reconciliation and expiry.
- [ ] Implement startup/future creation, DEFAULT repair, lock-bounded whole-partition
  expiry, and `(tableoid, ctid)` boundary cleanup.
- [ ] Replace row-only progress with explicit partition and row accounting while
  preserving confirmed progress on partial failure.
- [ ] Integrate atomic summary invalidation/removal with raw expiry.
- [ ] Test midnight/UTC boundaries, shortened and unlimited retention, late records,
  catalog drift, concurrent writers/readers, cancellation, crashes, and two leaders.

Exit gate: whole expired partitions disappear while boundary and retained rows
survive; restarts converge without duplicate partitions or false audit totals.

### 4. Implement bounded materialization and independent scheduling

Dependencies: units 2 and 3.

- [ ] Implement recent bootstrap, historical backfill, dirty-bucket recomputation,
  empty-hour coverage, atomic replacement, and bounded retry.
- [ ] Add persisted settings and separate supervisor job cadences with fair budgets.
- [ ] Verify source writes racing with refresh, retention racing with refresh,
  grouping-field changes, old dirty hours, and leader restart at every commit point.
- [ ] Verify that expensive backfill cannot prevent other maintenance from running.

Exit gate: summaries equal a raw-data oracle after catch-up, and pending invalidation
is never lost. A partial or failed refresh does not publish false coverage.

### 5. Integrate summary/raw reads and operator contracts

Dependencies: unit 4.

- [ ] Implement a typed read plan in the analytics repository with bounded range
  coalescing, consistent snapshots, and identical auth/ref filters.
- [ ] Preserve dedicated versus dashboard partial-hour semantics, retries, unknown
  or null status groups, failure rates, empty series, and recent-tail visibility.
- [ ] Update DTOs, source-catalog metadata, web settings/status, and generated clients
  in one unit. Keep existing response-cache semantics.
- [ ] Test raw-only startup, mixed clean/dirty coverage, summary-only historical
  reads, refresh failures, raw expiry, and tenant/ref isolation through the API.

Exit gate: every optimized endpoint matches its raw oracle, with truthful metadata
and no missing or duplicated buckets during concurrent maintenance.

### 6. Run acceptance and document rollout

Dependencies: unit 5.

- [ ] Run the full Rust database/broker lane, focused supervisor/dashboard E2E,
  full tier-1 E2E, web checks, and fresh-install acceptance.
- [ ] Run the unchanged seven-day benchmark plus the 30/90-day profile on PG16/18.
  Compare actual repository reads and equal expiry cohorts against phase 1.
- [ ] Verify partition pruning, summary indexes, bounded parent-lock duration,
  dirty-marker write cost, maintenance fairness, and zero owned resource leaks.
- [ ] Run `cargo sqlx prepare`, formatting, TypeScript checks, and
  `cargo check --all-targets --workspace` with zero compiler warnings.
- [ ] Update `AGENTS.md`, PostgreSQL deployment, supervisor, analytics, and testing
  docs for fresh partitions, migration policy, new settings, and freshness semantics.

Current delivery gate: correctness and operational checks pass without TimescaleDB
or another database extension. Reporting-performance targets remain deferred, with
the failed measurements preserved. Unsupported workload claims remain explicit.

## Owning code and validation entry points

Expected code locations:

- `crates/common/src/repositories/` for partition, materialization, retention,
  and analytics SQL; `crates/common/src/models.rs` for persisted domain types.
- `crates/common/src/config.rs` and new migrations for runtime settings/state.
- `crates/supervisor/src/` for scheduling and bounded maintenance coordination.
- `crates/api/src/routes/retention.rs`, analytics/dashboard routes, and DTOs for
  protected operator settings and accurate read metadata.
- Existing migration/lifecycle, analytics repository, dashboard acceptance, and
  supervisor E2E tests for boundary contracts.

Use the existing Docker-owned runners with unique run identities and at least
four database-test threads. Give the Rust lane at least 40 minutes and full-stack
validation up to two hours. Read [running tests](../testing/running-tests.md)
before provisioning; await teardown and verify leaks before janitor recovery.

Keep the work units independently verifiable. Do not create commits from agent
work. Historical phase-1 results remain separate from the fresh-install acceptance.

## Effort and remaining measurements

Allow roughly two engineering weeks for the scoped implementation and validation.
Concurrency failures can extend that estimate.
The SQL prototypes determine the final lock budgets, partition-repair strategy,
and default refresh budget before the production implementation depends on them.

The synthetic fixture cannot choose an installation's storage limits.
Record those values during deployment planning. This phase does not supply
columnstore compression or long-lived summaries beyond raw retention.

## Verified protocol decisions

The [SQL protocol evidence](../research/postgresql-native-maintenance-protocols.md)
records both PostgreSQL versions, rejected controls, and owned-resource cleanup.
Shared UPSERT markers serialized producers. Append-only notifications preserve
invalidation races without that shared write lock. Transaction-owned marker reuse
reduces repeated producer work without relying on origin uniqueness after logical
restore. Builders still acknowledge explicit visible IDs, never an ID watermark.

Integrated focused migration, partition, retention, summary, read, API, and
supervisor checks have passed on both PostgreSQL versions. Web type checking and
238 web tests passed before the final database optimizations. These checks do not
replace the full Rust database/broker lane or full-stack acceptance.

The [repository workload evidence](../research/postgresql-native-maintenance-workload.md)
preserves failed measurements as well as subsequent runs. Performance acceptance
remains open. Compare repository wall time separately from server SQL time, and
report first-attempt deadline failures even when bounded catch-up later converges.
Do not rerun or exclude individual failed latency or ingestion samples.

The completed combined run reached oracle-matched coverage on both versions and
cleaned up its owned resources. It failed three large-profile summary latency
gates, one mixed-read gate, and 11 of 48 ingestion pairs. These results describe
the frozen measured implementation, not the subsequent correctness fixes.

## Current remaining delivery work

- [x] Complete the review corrections below and their PostgreSQL 16/18 regressions.
- [x] Add real-supervisor E2E coverage for native scheduling, recovery, and expiry;
  preserve and restore the full runtime configuration in existing fixtures.
- [x] Validate the previous baseline: 3,420 Rust tests and all 13 native/retention
  E2E scenarios passed, with unchanged coverage and clean teardown. Preserve the
  initial stale OpenAPI inventory failure and targeted red/green correction.
- [ ] Revalidate full Rust, the 13 native/retention scenarios, and tier-1 E2E
  against the fresh-install baseline. The previous tier-1 stack failed before tests
  because Docker Desktop denied the worktree bind mount; an owned snapshot in a
  shared host path is the verified workaround.
- [x] Run SQLx preparation, generated-contract checks, web validation, formatting,
  and zero-warning workspace compilation against the final implementation.
- [x] Verify fresh PostgreSQL 16/18 installation with both migration runners.
  The completed populated-upgrade rehearsal is historical evidence only and is
  superseded by the user's fresh-database decision.
- [ ] Confirm least-privilege maintenance roles, alert behavior, and owned-resource
  cleanup; update deployment guidance to match the final changes.

Reporting latency and ingestion-overhead optimization are deferred, not part of
the active implementation queue. Preserve the benchmark evidence for later work.

Fresh-baseline checks completed on 2026-10-07, before the subsequent cache
refresh-coordination changes:

- 51 migration tests passed on each PostgreSQL version.
- Both real runner histories passed all four fresh-install cases and a separate
  optional four-case logical-restore matrix. Evidence is recorded in
  [Native-install verification](../deployment/postgresql-native-install-verification.md).
- The current OpenAPI export matches its saved spec, and all 443 generated web
  API files match regeneration. Three offline Python contract tests passed.
- Web type checking, 238 web tests, Rust formatting, diff checks, and workspace
  compilation passed without compiler warnings.
- SQLx preparation passed against an owned migrated PostgreSQL 18 database. No
  compile-time queries were found, so the cache remained empty. Initial invocation
  configuration failures are retained separately from the successful run.

Final-baseline SQL lifecycle and producer checks passed on both PostgreSQL
versions. The full Rust database/broker run completed with 3,421 passing tests
and two failures across 104 executables. Workflow-log shutdown exceeded its
deadline; the multi-worker FIFO simulation hit connection-pool acquisition
timeouts. Evidence is retained in
`/tmp/opencode/native-fresh-full-rust-20261007-36f9b2/`. The runner removed its
owned containers, volumes, and network. A focused shutdown rerun passed both
executor feature variants. The FIFO simulation passed alone, and all eight
selected FIFO tests passed together at four threads. All 36 selected workflow-log
tests also passed across both executor feature variants at four threads, with
zero owned containers, volumes, or networks remaining. These focused results do
not turn the failed full run into a pass or establish the cause of its failures.

Full-stack acceptance also has two failures under diagnosis: the first
native-status request returned HTTP 500 after a statement timeout, and tier-1
automation hit timer/execution failures before a per-test timeout stopped the
run. Neither run is a passing result. Both stacks cleaned up and retained their
failed evidence. Earlier baseline passes do not validate this baseline, and no
performance gate was relaxed.

## Review corrections pending acceptance

The static review reopened parts of partition lifecycle and materialization:

- [x] Remove expired summary groups, coverage, and invalidations through bounded
  source-locked maintenance after row-based expiry, including worker history,
  DEFAULT rows, and native-disabled cleanup. Whole-leaf expiry alone is insufficient.
- [x] Persist fair reconciliation ordering so deferred repair under a small
  operation cap cannot indefinitely starve other parents or candidate days.
- [x] Preserve confirmed reconciliation progress in typed failures and supervisor
  failure audits when a later catalog or DDL operation fails.

The final focused runs passed 57 tests per PostgreSQL version, with one explicit
ignored test per version. They include twelve new regressions for expiry cleanup,
bounded cancellation, late invalidations, fair reconciliation, and committed
failure counters. Failed fixture attempts remain preserved. Test-only setup and
failure-injection budgets do not change production deadlines.

SQLx preparation passed with no compile-time queries found; the empty cache stayed
empty. Full workspace, E2E, and fresh-install acceptance remain separate checks.
The completed performance run predates these fixes and is not evidence for their
performance.

## PostgreSQL references

- [Declarative partitioning, maintenance, DEFAULT constraints, and unique keys](https://www.postgresql.org/docs/18/ddl-partitioning.html)
- [Materialized-view refresh and concurrent-reader requirements](https://www.postgresql.org/docs/18/sql-refreshmaterializedview.html)
- [Transaction isolation and consistent snapshots](https://www.postgresql.org/docs/18/transaction-iso.html)
