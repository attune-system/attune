# Runtime grant and DDL inventory at ceda62b1

This is a source requirement inventory, not a claim about unknown production ACLs.
`inventory.sql` is the exact read-only cluster/installation ACL capture contract;
no production connection/inventory was supplied. Retain its role OIDs, column and
table ACLs, membership options, function owners/search paths/ACLs with IaC state.
Do not generate an ownership manifest from that discovery without external DBA
ownership proof. The assets grant **no application runtime DML**: its complete
per-service least-privilege set remains a deployment integration prerequisite.

| Principal/caller | Exact current operations/requirements | Strict rollout posture |
| --- | --- | --- |
| Development PostgreSQL `attune` shared owner | Compose POSTGRES_USER, migrations and services share owner credentials; docker/init-roles.sql also creates svc_attune/attune_api with database ALL | Denied, never treated as production proof |
| Offline SQLx migrator | Installation schema CREATE/ownership via explicit owner SET; canonical migration top-level table/type/index/trigger/function/constraint DDL; owns `_sqlx_migrations` | Separate offline login/NOLOGIN owner; no ledger conversion |
| API cache snapshot refresh, `CacheGenerationRepository` in common/repositories/cache.rs | `create_cache_generation_partition`: CREATE TABLE LIKE cache_entry INCLUDING DEFAULTS/CONSTRAINTS; ADD generation CHECK; ATTACH LIST partition; ALTER child OWNER TO parent owner | Requires schema CREATE and effective parent ownership today; conflicts with strict runtime no-CREATE/owner-membership |
| API cache live write, common/repositories/cache/live.rs `CacheLiveRepository::apply_conn` | Same create_cache_generation_partition on first live backing generation | Same conflict; not fixed here |
| API/supervisor cache cleanup, `CacheGenerationRepository::drop_if_cleanup_eligible` | `drop_cleanup_cache_generation` canonical replacement in 20261015006013: protected traversal, DROP exact generation child, usage/state DML | Effective child ownership; cache owner assumption conflicts with strict runtime role if added |
| Supervisor cache statistics, supervisor/cache_retention.rs → `CacheStorageRepository::refresh_statistics` | ANALYZE cache_entry(generation,external_id,id,size_bytes); preflight `pg_has_role(current_user,parent_owner,'USAGE')` | Requires effective parent ownership even when idle; ordinary runtime cannot receive owner membership |
| Supervisor native partition maintenance, repositories/native_maintenance/partitions.rs `ensure_day`/`expire_before` | `native_partition_ensure_day`: CREATE leaf LIKE parent INCLUDING ALL, ALTER OWNER, ADD/VALIDATE temporary DEFAULT exclusion and leaf bounds, ATTACH partition, DROP exclusion, move bounded DEFAULT rows. `native_partition_expire`: DROP exact registered leaf after retention evidence guards | Dedicated maintenance ownership requirements remain; not granted to API/worker/operator by these assets |
| Explicit artifact benchmark/operator fixture diagnostics | ArtifactReadDiagnostics::analyze_event_fixtures ANALYZE event; ArtifactRepository::analyze_read_fixtures ANALYZE artifact,artifact_version,execution,execution_secret_value,runtime_schema_binding | Not called by artifact handlers; no diagnostic ownership granted to ordinary runtime |
| Offline encryption conversion | encryption_conversion_apply performs guarded UPDATE, not runtime DDL; trigger creation/replacement and function search_path ALTER are migration-time DO blocks | Distinct existing operator contract; no encryption credentials/grants used by rollout Jobs |
| Offline rollout operator | Schema USAGE, five rollout metadata SELECTs, four receipt/ledger EXECUTEs, exact direct capability membership; no schema/table/sequence DDL/DML | This lane only; accepted-work authority remains independent |

`ddl-source-inventory.json` is reproducible with
`python3 scripts/inventory-rollout-ddl.py`. It retains all canonical dollar-quoted
function definitions containing dynamic DDL and their lexical Rust references,
including replaced definitions for provenance. Current effective definitions must
be read in migration order (not counted as independent live functions). The
canonical runtime DDL functions are create_cache_generation_partition,
drop_cleanup_cache_generation, native_partition_ensure_day and
native_partition_expire. Installation-only top-level/DO DDL is executed by the
offline migrator, not a service. Static Rust ANALYZE diagnostics are listed above;
EXPLAIN ANALYZE is query execution, not DDL authority. Lexical references can
include colocated test candidates; the index is not a call-graph proof or a grant
synthesizer. The read-only live function-definition capture is decisive for an
installation, and absent until an owner supplies it.

API ordinary cache DML currently spans namespace/permission bindings, generation,
clone, ingest chunk, cache entries, iteration pins, usage/quota/statistics metadata
and their trigger effects. Executor cache iteration reads generation/entry/schema
and permission snapshots and retains iteration pins; supervisor reclaims pins,
metadata, partitions and statistics. All use repository transactions and their
existing grants. Neither broad schema DML nor cache-owner membership is safe to
invent from this summary. The exact per-service runtime ACL inventory and future
DDL separation design must be approved before a production profile can satisfy
both cache functionality and rollout verification. Current source posture is
explicitly denied; an isolated no-DDL role fixture is the only passing posture
validated by this slice.
