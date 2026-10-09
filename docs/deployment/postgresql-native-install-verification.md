# PostgreSQL native fresh-install verification

Attune is pre-production and targets fresh databases. Canonical migrations create
`event`, `execution_history`, and `audit_event` as native RANGE parents directly.
`cache_entry` is a LIST parent keyed by generation, without a DEFAULT partition.
This verifier does not install an old schema, convert populated heaps, or certify
an existing deployment. Data-preserving forward migrations become required from
1.0.0. See [PostgreSQL-only deployment](postgresql-only.md) for migration policy
and explicit development-database reset instructions.

## Run after the source freeze

Use cached stock `postgres:16-alpine` and `postgres:18-alpine` images. The runner
requires Python 3.10+, Docker, and a compiled SQLx driver. It never builds Rust
or pulls an image automatically.

```bash
CARGO_BUILD_JOBS=2 SQLX_OFFLINE=true cargo build -p attune-common --example verify_native_migrations

python3 scripts/verify-postgresql-native-install.py --help
python3 scripts/verify-postgresql-native-install.py \
  --dry-run --output /tmp/opencode/native-install-plan

python3 scripts/verify-postgresql-native-install.py \
  --versions 16 18 --logical-restore \
  --output /tmp/opencode/native-install-cache-final
```

Choose an unused output directory under an existing parent. Existing directories,
files, and broken symlinks are rejected. Dry-run prints current migration/helper
hashes and direct-partition declaration readiness without writing files, loading
Docker, or compiling. Declaration readiness is not acceptance or a source freeze.
The main session must authorize database runs after the core source freeze.

The default matrix has four cases: PostgreSQL 16 and 18, each with SQLx and the
Docker filename/SHA-384 history. The driver uses SQLx's real runtime-source
`Migrator`, not synthesized history records. It does not call the embedded
application bootstrap or certify its public runner-selection marker. The Docker
case runs the unchanged `docker/run-migrations.sh`, disabling only unrelated
network index seeding with `STANDARD_INDEX_SEEDER=/bin/true`.

For an explicitly authorized preliminary private-source probe, select only the
needed runner with `--runners sqlx` and pass `--preliminary`. Evidence records both
the runner subset and preliminary status. This does not replace the final default
four-case matrix after the main session's source freeze.

## Checks and evidence

Each case first installs all current migrations into an empty owned database.
There is no committed-baseline count/checksum gate. It verifies:

- Stock PostgreSQL without TimescaleDB; empty native parents, correct UTC RANGE
  keys, DEFAULT leaves, and registered daily leaves at offsets 0 through 7.
- Composite `(id, created)` event/audit primary keys, owned BIGSERIAL sequences,
  and no public history-ID key. Execution, enforcement, worker history, and
  sensor-process history remain ordinary tables.
- `execution.parent`, `execution.enforcement`, `workflow_execution.execution`,
  `inquiry.created_by_execution`, and `cache_generation.created_by_execution`
  remain BIGINT fields without outbound FKs, preserving independent retention.
- Canonical parent indexes and valid attached indexes on every daily/DEFAULT
  leaf; native partition checks run as the non-superuser schema owner.
- Six ordinary analytics views, recorded field definitions, and raw aggregation
  oracles after a small business-write probe.
- Exact real-runner migration checksums, no-op reruns, rejection of altered
  already-applied bytes, transactional DDL/history rollback on an injected
  failing test migration, and retry with canonical files. The injected files
  exist only in the private output snapshot.
- Non-superuser writer and column-only producer inserts, actual `xmin` metadata,
  same-transaction origin/marker reuse, sequence reservations, execution history
  digest fields, lifecycle audit rows, outbound FK rejection, and equal event IDs
  at distinct times without global uniqueness assumptions.
- A separate non-superuser maintainer with schema-owner membership can create a
  daily leaf transactionally. The writer cannot, and has no builder-state SELECT.
- Fresh cache LIST storage has composite `(generation, id)` identity, owned ID
  sequence, both parent indexes, no initial children, and zero deployment usage.
  Every cache lifecycle function uses invoker privileges.
- The real cache repositories create a populated and an empty generation as a
  non-superuser API login inheriting a dedicated cache-entry owner role, without
  switching roles. Idempotent creation reuses
  the same partition. Ingestion charges exact generation, owner, and deployment
  totals. Attached leaf indexes are valid and both children have the shared owner.
- Ordinary source writers cannot INSERT through the cache parent or directly into
  a leaf, update accounting, create/attach partitions, or reclaim generations.
  Transactional create/drop rollback and an accounting-underflow failure preserve
  committed storage, generation metadata, and usage.
- The supervisor login also uses the non-superuser effective owner. Its repository
  reclaims an empty generation without releasing another generation's bytes,
  then drops populated storage and returns all accounting to zero. Optional restore
  retains the populated partition in the archive and reclaims it after restore.
- The same restricted supervisor runs the repository's parent ANALYZE refresh,
  acknowledges pending statistics, and gets a no-op on immediate rerun. Metadata
  observations report exact created/dropped partition counters and cleanup backlog.
  A separate DML-only role reaches the repository's ownership guard and is rejected
  without changing statistics state. No superuser or owner-role fallback is used.
- Neither cache service inherits the application-schema owner. Both are denied
  DROP on an unrelated sentinel table. The cache owner role owns only the entry
  parent, its sequence, and leaves. Cache metadata and statistics DML is granted
  explicitly rather than through broad application ownership.

The write fixture adds two events, one execution with two status updates, four
audit records, one worker-history record, and one dangling-event enforcement.
Other source writes are rolled back. This is a correctness fixture, not a row
volume or performance profile. Event payloads are not updated after insertion.
Later-provisioned producer roles receive explicit metadata/sequence grants;
this verifier does not claim to test migration-time discovery of preexisting ACLs.
The cache fixture has two small records in one failed generation and a separate
zero-record failed generation. It does not exercise API authentication, workflow
pins, quotas, or concurrent ingestion. Those remain repository/API suite contracts.

`evidence.json` records before/after SHA-384 migration hashes, helper and driver
SHA-256 hashes, cache repository/config/model source hashes, server image IDs/settings,
per-case checks, source fingerprints, sequence values, catalog definitions, and
cleanup results. Any input change
during the run makes acceptance incomplete. Per-phase stdout/stderr logs include
expected checksum and transaction failures. Installation wall time is diagnostic
only. No query-speed or ingestion-overhead benchmark runs.

## Optional fresh-schema logical restore

```bash
python3 scripts/verify-postgresql-native-install.py \
  --versions 16 18 --logical-restore \
  --output /tmp/opencode/native-install-with-restore
```

This adds one same-major `pg_dump -Fc`/transactional `pg_restore` per case, after
the small fresh-schema business probe. It compares source contents, catalog
definitions and grants, partition keys/bounds, populated cache entries and exact
usage, sequence reservations, and real migration history. After comparison,
the supervisor repository must reclaim restored cache storage and release its
exact bytes, leaving no children, generation usage, or ingest metadata.
Constant VARCHAR-array-to-TEXT CHECK casts can render differently after restore;
the hash normalizes only that lossless literal form. Original constraint text
remains in the evidence. Physical relation OIDs are not restore contracts.

The target starts with an empty owned schema and a narrow CREATE grant to the
cache-storage owner. A saved archive list skips only the expected CREATE SCHEMA
entry, retaining all objects and ACL entries. This avoids `pg_restore`'s table
ownership commands failing before the later schema ACL entry grants CREATE to
the cache owner. Schema ownership and ACLs are included in the compared catalog.

The optional probe starts a target producer transaction before restore and puts
its full origin on existing source dirty markers. The dump/restore gives imported
markers new tuple `xmin` values. A repeatable-read builder captures imported IDs;
two fresh producer writes must create one distinct dirty marker that survives
acknowledgment of only those imported IDs. This exercises restore collisions on
the fresh schema, not an old-schema upgrade. It is not a mandatory install gate.

## Ownership and cleanup

PostgreSQL majors run sequentially, with at most one server active. Each server
has a 1 GiB memory limit, two CPUs, and an ephemeral loopback port. Trust auth is
limited to this disposable cluster; no existing database URL is accepted.
Only the bootstrap connection is superuser. All eight probe roles lack superuser,
BYPASSRLS, CREATEDB and CREATEROLE privileges.

### Deployment-role limits

The probe provisions its own role contract. It is not proof that installed
deployment roles satisfy it. API generation creation now requires schema CREATE
and effective ownership of `cache_entry`; ordinary DML grants are insufficient.
Supervisor reclamation also requires ownership of API-created children.
The helper transfers each new leaf to the parent owner resolved from the catalog.
Without that transfer, inherited parent-owner membership creates children owned
by the API login and supervisor DROP fails. The probe uses distinct API and cache
supervisor logins inheriting only `native_cache_owner`, which owns `cache_entry`
and its leaves, not the application schema or unrelated tables. It grants schema
USAGE/CREATE to that owner and explicit DML/sequence privileges for cache metadata,
usage, statistics, and the workflow/retention reads needed by the probed paths.
It never switches service roles or grants either service arbitrary application DROP.

For separately provisioned deployments, use an explicit NOLOGIN cache-storage
owner without superuser, BYPASSRLS, CREATEDB or CREATEROLE. Transfer the cache parent
and existing leaves to that owner, grant schema USAGE/CREATE, and grant both service
logins effective owner membership with the SET permission needed by PostgreSQL's
ALTER OWNER check. Keep metadata DML explicit and test unrelated-table DROP denial.
Review existing leaves and deployment grants before making operator changes;
this verifier performs them only in its disposable empty databases.

The current `docker/init-roles.sql` grants `attune_api` database privileges but
does not provision this schema/owner contract. Compose's shared `attune` login is
the PostgreSQL bootstrap superuser. Successful Compose runs can therefore mask a
missing non-superuser deployment grant. The verifier neither changes installed
roles nor grants superuser. Deployment provisioning must be checked separately.

The runner reuses the owned server/session protocol from
`scripts/probe-postgresql-native-maintenance.py`. Sessions close before owned
databases are dropped. Container and volume removal checks their exact run label
and resource identity. Successful runs report zero owned database leaks and no
remaining client sessions before server removal. Cleanup failures fail acceptance
and remain in evidence. A final exact-label Docker inventory records remaining
containers and volumes even after a failed case. Failed attempts keep their logs and snapshots; retry
requires a new output path. SIGTERM enters cleanup. SIGKILL cannot guarantee it.

The named mount must match the image-declared VOLUME root exactly:
`/var/lib/postgresql/data` on PG16 and `/var/lib/postgresql` on PG18. Evidence records
the declared roots and actual mounts. Before deleting the exact owned container,
the runner captures every mounted volume name and uses `docker rm --volumes`,
then removes only the label-checked named volume. Every captured volume is checked
for absence afterward. An empty label-filtered inventory alone is not enough to
prove that anonymous volumes were removed.

## Validation status

The earlier matrices passed on PostgreSQL 16.15 and 18.6 before the cache LIST
migrations and lifecycle checks. They are historical evidence, not acceptance
of the revised cache baseline. Do not treat their source freeze as current.

| Evidence | Run ID | PostgreSQL 16 SQLx / Docker | PostgreSQL 18 SQLx / Docker |
| --- | --- | --- | --- |
| `/tmp/opencode/native-install-final/evidence.json` | `install-a8b8c3afef79` | PASS / PASS | PASS / PASS |
| `/tmp/opencode/native-install-final-restore/evidence.json` | `install-cff6151c71b1` | PASS / PASS | PASS / PASS |

Rebuild the driver after the main session freezes migration and repository
sources. Run the new four-case matrix with `--logical-restore` in a new output
directory and retain its input hashes and exact-label cleanup proof. A private
snapshot run before that freeze is preliminary even if every selected case
passes. Main owns final workspace Rust, SQLx preparation, and Tier-1 E2E gates.

Preliminary role evidence is separate from those old matrices:

- `/tmp/opencode/native-install-cache-role-red-pg16/evidence.json` reproduced
  supervisor DROP SQLSTATE `42501` using a private copy with only the leaf-owner
  transfer removed. No installed migration bytes or history were changed.
- `/tmp/opencode/native-install-cache-role-green-restore-pg16/evidence.json`
  passed both real runners, cache create/drop, restricted-role denials, and
  populated restore/reclamation. Both role snapshots retained unchanged input
  hashes and completed exact-label cleanup. They predate the final lineage and
  statistics probes and are not the final source freeze.
- `/tmp/opencode/native-install-cache-role-candidate-pg18/evidence.json` stopped
  at the new installed lineage check because `execution.parent` still had an FK
  in that candidate. Cleanup succeeded. Main subsequently corrected the canonical
  selected-lineage declarations rather than weakening the verifier.
- `/tmp/opencode/native-install-cache-lineage-stats-preliminary/evidence.json`
  passed SQLx-only probes on PG16 and PG18 with `preliminary: true`. Both passed
  the corrected lineage catalog guard, inherited-owner create/drop, real ANALYZE
  and immediate no-op, DML-only statistics rejection, unrelated-table DROP denial,
  and populated restore/reclamation. Input hashes stayed unchanged. Each server
  had one named volume mounted at the image's declared root; all captured mounts
  disappeared during exact-container cleanup, with zero databases, sessions,
  containers, or volume leaks. These probes preceded the final two-integer cache
  admission-lock change to `(7821101, 0)`. They are not validation of those final
  bytes or the final four-case source freeze.

The six previous upgrade-rehearsal artifact directories under
`/tmp/opencode/native-upgrade-final-10k*` remain untouched. Their old conversion
results are historical evidence only. The old rehearsal CLI, tests, example name,
and operator document were replaced rather than retained as compatibility paths.
Reporting performance remains deferred. This tool does not certify production
readiness, prepared-plan behavior, summary-aware repository readers, crash
durability, or the main workspace/E2E gates.
