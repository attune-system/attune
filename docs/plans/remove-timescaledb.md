# Remove the TimescaleDB dependency

Status: implementation and PostgreSQL-only acceptance completed.
Date: 2026-10-06.
Base: `252ce12e` on `main`.
Branch: `feature/remove-timescaledb`.
Worktree: `/mnt/wdc/attune-worktrees/remove-timescaledb`.

## Goal

Install and operate Attune on stock PostgreSQL. Remove TimescaleDB package
requirements, DDL, worker-management calls, and catalog queries. Keep execution,
workflow, event, history, audit, and dashboard behavior working together.

The initial design uses ordinary PostgreSQL tables, raw-data analytics, and
bounded row retention. Native partitioning and scheduled summary storage become
necessary only if representative workload checks reject this design. Neither
case reintroduces an optional Timescale backend.

The initial installation path assumes a fresh database. Preserving an existing
Timescale database is a separate conversion deliverable. Implementation must
not reset a developer database or reuse its data volume without an explicit
conversion or reset decision.

## Verified baseline

Run `python3 docs/research/audit_timescaledb.py` to reproduce the migration
inventory. This script reads declarations, not deployed database state.

| Dependency | Current declaration |
| --- | --- |
| Hypertables | `event`, `execution_history`, `worker_history`, `sensor_process_history`, `audit_event` |
| Compression policies | Five, each with a seven-day chunk-age threshold |
| Continuous aggregates | `execution_status_hourly`, `execution_throughput_hourly`, `event_volume_hourly`, `worker_status_hourly` |
| Aggregate refresh policies | Four |
| Ordinary hourly views | `enforcement_volume_hourly`, `execution_volume_hourly` |
| Timescale retention policies | None; the supervisor directly calls `drop_chunks` |

`execution` and `enforcement` are already ordinary tables. Several project rules
and SQL comments incorrectly describe them as hypertables. The history-writing
triggers and the history-reading repository use standard PostgreSQL features.

The runtime-specific calls are concentrated in:

- `crates/common/src/repositories/retention.rs`
- `crates/api/src/routes/dashboards.rs`
- `crates/common/src/test_database.rs`

Deployment requirements live in the two Compose files, CI, and five Linux
package manifests. Catalog assertions live in the migration and fixture
lifecycle tests. The broader feature is not isolated to those explicit calls:
analytics relations, retention settings, generated clients, and docs must also
change coherently.

## Target contract

### Tables and history

- Keep all five tables as ordinary PostgreSQL tables with their existing
  business columns and history-trigger behavior.
- Use `BIGINT` IDs throughout. Restore single-column primary keys for `event`
  and `audit_event` in the fresh schema. Retain history tables without adding
  a public history-ID contract.
- Preserve deliberately dangling references and independent retention periods.
  Do not add foreign keys merely because the extension no longer prevents them.
- Keep `_jsonb_digest_summary` and every current mutable-field history check.
- Add explicit time indexes where Timescale previously supplied them. Cover
  general history-range cleanup and `audit_event.created`, not only entity lookups
  and partial status indexes.

### Analytics and dashboard freshness

- Replace the four continuous aggregates with ordinary views that retain their
  names, column aliases, count types, and grouping meaning.
- Use UTC-aligned hourly buckets. Preserve creation counts versus status
  transition counts; they are different metrics.
- Preserve the existing endpoint bucket-inclusion rules and filter behavior.
  Verify partial-hour boundaries and non-UTC sessions explicitly.
- Query raw records for the authorable dashboard and report `raw_only` with
  no aggregate watermark. Delete the Timescale watermark lookup and unused
  aggregate-cutover logic after migrating their callers.
- Keep SQL in repositories. Move retained dashboard raw queries into the
  owning analytics repository as the Timescale adapter is removed. Do not
  create a new service-level database-query path.
- Check query plans for the ordinary views. A filter on a computed bucket
  must not accidentally force a full retained-history scan. If necessary, use
  repository queries with source-timestamp bounds while preserving bucket rules.
- Analytics will reflect retained raw records immediately. Separate aggregate
  retention and historical summaries beyond raw retention no longer exist in
  this initial design. Document that behavior change.

### Retention and configuration

- Delete expired rows in bounded, committed batches with a fixed cutoff per
  target run. Preserve terminal-state, waiting-workflow, undelivered-log, and
  other existing operational-row protections.
- Use ID-based selection for keyed tables. For the three ID-less history
  tables, select and delete statement-local `ctid` values in one statement.
  Never persist those row locations or reuse them across transactions. This
  assumes ordinary tables; native partitioning would require a revised identity.
- Add a positive, persisted `max_batches_per_target` setting. A target stops
  when exhausted, when a batch deletes no rows, when cancelled, or when its
  batch budget is consumed. Each batch keeps `batch_size` as a row limit.
- Hold the existing leader advisory lock across the cycle. Keep transactions
  limited to individual batches and let later targets and other maintenance
  run after a busy target reaches its budget.
- Calculate the default batch budget from the workload checks. The existing
  1,000-row hourly default permits only 24,000 deletes per target per day;
  the replacement must support multiple batches.
- Count candidates once per target run, not once per batch. Report actual rows
  deleted consistently, including history and audit targets. Keep dry runs
  non-mutating and preserve retention-lag alerts.
- Remove `continuous_aggregates` from the target enum, persisted defaults,
  Rust configuration, API validation, web settings, and generated clients.
  A view has no independently retained materialization to clean up.
- Keep canonical pre-production formats in place. Do not introduce v2 config,
  dual readers, or a legacy Timescale runtime adapter.

### Deployment and migrations

- Use stock PostgreSQL 18 for Compose and CI, matching the current major
  version. Keep the documented PostgreSQL 16 minimum and verify fresh-schema
  compatibility with it before claiming support.
- Remove Timescale package requirements and preload configuration. Preserve
  unrelated `pg_stat_statements` support where existing tests require it.
- Revise canonical migrations in place for fresh databases. Preserve runner
  ownership and checksum enforcement. A rewritten migration is not an upgrade
  for an existing populated schema.
- Remove fixture calls to `_timescaledb_functions.stop_background_workers`.
  Keep run-owned database-template cloning, explicit cleanup, and leak checks.

## Ordered implementation units

Each unit ends with its own verification. Units are work boundaries, not a
request to create commits. Keep API and client changes in the same unit.

### 1. Capture correctness and workload baselines

Dependencies: none.

- [x] Add a reproducible fixture and measurement command for hourly counts,
  recent tails, retention backlogs, and realistic JSONB payloads.
- [x] Declare retained row volumes, ingestion rate, dashboard concurrency, and
  acceptable latency before judging the PostgreSQL replacement's performance.
- [x] Capture the current Timescale counts and response contracts on owned
  disposable infrastructure. Separate performance measurements from correctness.
- [x] Record the batch-budget default supported by the declared workload.

Exit check: the same fixture can be replayed against both database variants,
with expected metric meanings and retention protections written down. Existing
developer data is not part of the fixture.

### 2. Make the fresh schema and fixtures PostgreSQL-only

Dependencies: unit 1's correctness fixture.

Primary files:

- `migrations/20250101000009_timescaledb_history.sql`
- `migrations/20250101000013_audit_log.sql`
- `crates/common/src/test_database.rs`
- `crates/common/tests/migration_tests.rs`
- `crates/common/tests/test_database_lifecycle_tests.rs`
- `docker-compose.yaml`
- `docker/distributable/docker-compose.yaml`
- `.github/workflows/ci.yml`

Tasks:

- [x] Remove extension, hypertable, compression, and policy DDL. Keep the
  existing migration filename initially to avoid conflating removal with
  migration-history renaming.
- [x] Create ordinary hourly views and explicit time indexes. Preserve history,
  audit, and notification triggers.
- [x] Switch owned test and deployment database images to stock PostgreSQL 18.
- [x] Remove CI's Timescale preload and extension initialization in the same unit.
  Preserve its independent `pg_stat_statements` setup. Give fresh Compose
  installations a new PostgreSQL-only data volume name so the image switch does
  not attach an old Timescale data directory automatically.
- [x] Remove fixture worker-stop calls and replace Timescale catalog assertions
  with ordinary-table, view, index, trigger, and no-required-extension checks.
- [x] Verify both supported migration runners on separate fresh databases and
  verify repeated migration is a no-op.
- [x] Verify changed-checksum histories still fail rather than silently adopting
  the new schema. Preserve existing migration-runner exclusivity tests.

Exit check: fresh migration and fixture setup/teardown pass on a server without
TimescaleDB installed. History trigger behavior remains correct.

### 3. Replace retention and update its whole contract

Dependencies: unit 2 and unit 1's cleanup-capacity measurements.

Primary files:

- `crates/common/src/repositories/retention.rs`
- `crates/common/src/config.rs`
- `crates/supervisor/src/main.rs`
- `crates/api/src/routes/retention.rs`
- `migrations/20250101000014_runtime_retention_supervisor.sql`
- `web/src/api/retention.ts`
- `web/src/api/models/RetentionConfig.ts`
- `web/src/api/models/RetentionTargetsConfig.ts`
- `tests/generated_client/`
- `tests/e2e/api/test_supervisor_retention.py`

Tasks:

- [x] Implement bounded history, event, and audit deletes in the repository.
- [x] Implement per-target batch budgets, cancellation boundaries, and row-based
  audit counts without weakening existing operational retention predicates.
- [x] Persist and expose the new setting through the authenticated retention API.
  Remove the obsolete aggregate-retention target everywhere.
- [x] Regenerate web and Python clients from current source and update the
  hand-authored web retention types, target labels, and controls.
- [x] Test duplicate history timestamps, cutoff boundaries, dry runs, unlimited
  retention, zero-progress batches, budget exhaustion, multiple batches, and
  protected execution/work-queue rows on a real owned database.
- [x] Verify retention-lag alerts and that one busy target does not prevent the
  remaining maintenance steps from running.

Exit check: a backlog larger than one batch drains within the configured budget;
retained and protected rows survive; API, web, and Python contracts agree.

### 4. Remove the dashboard Timescale adapter

Dependencies: unit 2.

Primary files:

- `crates/common/src/repositories/analytics.rs`
- `crates/api/src/routes/dashboards.rs`
- `crates/api/src/dashboard_data/watermark.rs`
- `crates/api/src/routes/analytics.rs`
- `crates/api/src/dto/analytics.rs`
- `crates/api/tests/dashboard_acceptance_tests.rs`
- `crates/api/tests/dashboard_acceptance_fixtures.rs`

Tasks:

- [x] Replace aggregate/raw cutover with the existing raw dashboard semantics.
  Move SQL into the analytics repository and remove unused watermark helpers.
- [x] Keep auth scopes, action and trigger filters, time windows, and metric
  definitions intact. Return accurate raw-only freshness metadata.
- [x] Verify all dedicated analytics endpoints, including failure-rate counts,
  worker status counts, empty datasets, and recent records.
- [x] Compare hourly counts against the baseline fixture. Cover retry transitions,
  partial buckets, non-UTC sessions, and completed executions at the tail.
- [x] Inspect representative query plans and measure the declared retained-data
  workload. Address timestamp-filter pushdown if needed.

Exit check: dashboards and analytics return correct current results without a
Timescale function or catalog call, and workload measurements meet the declared
acceptance limits. If they fail, record the result and implement bounded summary
storage or native partitions before adopting the replacement.

### 5. Finish packaging, tooling, and operator documentation

Dependencies: units 3 and 4.

- [x] Remove Timescale dependencies from `packaging/nfpm/attune.yaml`,
  `attune-api.yaml`, `attune-executor.yaml`, `attune-notifier.yaml`, and
  `attune-supervisor.yaml` across their default, RPM, and Arch dependencies.
- [x] Remove Timescale-specific job cleanup from `scripts/ci-test-db-safety.sh`
  while retaining database ownership and leak checks. Update test-policy wording.
- [x] Correct `AGENTS.md`, migration comments, deployment docs, and testing docs.
  Keep historical research distinguishable from current prerequisites.
- [x] Document the fresh-database rollout, loss of columnstore compression,
  raw-data analytics retention, and the separate existing-data conversion path.
- [x] Regenerate SQLx metadata against the revised disposable schema.

Exit check: clean installation and package metadata require stock PostgreSQL
only. Active deployment and application paths contain no Timescale dependency.

### 6. Run final PostgreSQL-only acceptance

Dependencies: unit 5.

- [x] Run the full default Rust database/broker lane and full-stack tier-1 E2E
  with owned resources. Exercise supervisor retention and dashboards explicitly.
- [x] Verify fresh migration on PostgreSQL 16 as well as the PG18 default.
- [x] Check action execution, workflow child execution, inquiry waits, event-to-rule
  enforcement, audit/history reads, and runtime log delivery in the E2E stack.
- [x] Verify fixture cleanup and absence of run-owned database/session leaks.
- [x] Run formatting, TypeScript checks, SQLx preparation, and the zero-warning
  workspace check. Record dataset, test inventory, and workload results.

Exit check: all required checks pass without installing or preloading TimescaleDB.
The source inventory reports zero hypertables, compression policies, continuous
aggregates, and aggregate-refresh policies; the four replacement hourly views
and two existing hourly views remain available.

## Validation commands

Run from this worktree to reproduce the checks. Recorded results appear in the
completion section below.

```bash
python3 docs/research/audit_timescaledb.py
bash scripts/run-rust-integration-tests.sh --test migration_tests
bash scripts/run-rust-integration-tests.sh --test test_database_lifecycle_tests
bash scripts/run-rust-integration-tests.sh --test dashboard_acceptance_tests
bash scripts/run-integration-tests.sh -k supervisor_retention
bash scripts/run-rust-integration-tests.sh
bash scripts/run-integration-tests.sh --tier 1
cargo fmt --all -- --check
cargo check --all-targets --workspace
```

Docker runners generate run identities and use at least four database-test
threads. Set timeouts before starting: at least 40 minutes for the Rust lane
and up to two hours for full-stack validation. Read
`docs/testing/running-tests.md` before provisioning dependencies.

Regenerate SQLx metadata with `cargo sqlx prepare --workspace -- --all-targets`
after pointing `DATABASE_URL` at a migrated, disposable PostgreSQL database.
Run `npm run generate:api` in `web/` and use
`scripts/generate-python-client.sh` for the Python client. Give Python generation
an invocation-owned `OPENAPI_SPEC_PATH` rather than its shared default temp path.
Select web checks from the existing package scripts during implementation.

## Existing-data conversion decision

Before deploying against any populated Timescale database, decide whether its
data must survive. No reset or extension drop belongs in automatic startup.

If preservation is required, add a separately verified conversion unit:

1. Inventory logical row volumes, compressed sizes, sequence positions, dangling
   references, and summaries whose raw input has expired.
2. Build the revised PostgreSQL schema in a separate owned database.
3. Transfer rows from one consistent source snapshot or a controlled write pause.
   Disable history and lifecycle-audit triggers on the target during import so
   imported live rows do not duplicate imported history and audit records.
   Restore and verify those triggers before allowing application writes.
4. Preserve IDs, sequence positions, relationship columns, and all required
   summaries. If older summaries must remain queryable, the raw-view design needs
   explicit summary storage before conversion can complete.
5. Verify row counts, representative content, triggers, permissions, and application
   behavior. Measure copy duration, capacity, and permitted downtime before cutover.
6. Keep the old database available for rollback until the target passes acceptance.

Logical conversion is distinct from runtime compatibility. Do not bypass migration
checksums or assume a Timescale backup restores into stock PostgreSQL unchanged.

## Production rollout inputs and original estimate

Two inputs remain before rollout planning is complete:

- Whether current populated installations need data preservation.
- The representative retained-data and ingestion envelope that determines cleanup
  defaults and whether raw analytics meet the required latency.

Fresh-schema implementation is complete. The measurements use a declared
synthetic workload, not an inventory of an installed system. These inputs still
matter before a data-preserving rollout or a production performance claim.

The original estimate was 3 to 5 engineering days for the ordinary-table design,
including contract updates and validation. Native partition lifecycle and scheduled
summaries would bring the total closer to 1 to 2 engineering weeks. Existing-data
conversion needs its own estimate after the inventory and rehearsal.

## Completion evidence

The final source inventory contains no hypertables, compression policies,
continuous aggregates, or aggregate-refresh policies. Six ordinary hourly views
remain, and application analytics use source-timestamp-bounded repository queries.

| Check | Result |
| --- | --- |
| Final full Rust PostgreSQL/broker lane | 3,346 passed, zero failures, 99 executables at four threads; 10 ignored and 11 default-filtered |
| Full-stack tier-1 E2E | 31 passed, zero failures or skips |
| Explicit supervisor-retention E2E | 7 passed, zero failures or skips |
| Inquiry and runtime-log E2E | 7 passed, zero failures or skips after fixing compound transition classification |
| Web suite | 233 passed, one pre-existing skipped test |
| Formatting, TypeScript, workspace check | Passed; no Rust compiler warnings |
| SQLx preparation | Completed against the revised schema; no macro-query metadata required |
| Standards and specification reviews | Findings corrected; follow-up reviews report no unresolved findings |

Review corrections preserve confirmed committed deletion counts when a later
retention batch fails, await new migration fixtures' teardown, and remove the
obsolete single-batch retention wrapper. Inquiry acceptance exposed a pre-existing
classifier bug: compound `succeeded()` conditions had skipped expression evaluation.
Classification now recognizes only standalone status predicates, with pure
regressions and mutually exclusive approval/denial E2E assertions.

The workload comparison passed count, UTC, recent-tail, trigger, and retention
checks on stock PostgreSQL 16 and 18. Timestamp-bounded application query shapes
had approximately 46 to 63 ms warm server p95 for event/status counts. Direct hourly
view queries missed the declared 500 ms limit and remain documented as slow paths.
The application does not use those computed-bucket filter paths. The slowest
100-batch stock deletion loop took 4.578 seconds. These are synthetic database
measurements, not endpoint latency or production equivalence.

See [workload evidence](../research/timescaledb-removal-workload.md) and
[PostgreSQL-only deployment](../deployment/postgresql-only.md). Local raw evidence:

- `/tmp/opencode/timescale-removal-disk-comparison/`
- `/tmp/opencode/ts-removal-rust-integrated-20261006/`
- `/tmp/opencode/ts-removal-e2e-20261006/`

Both final Docker runners explicitly removed their owned containers, volumes,
and networks. The Rust runner recorded zero clone and migration-database leaks
before any janitor recovery. Existing-data conversion remains a separate operator
decision and deliverable.
