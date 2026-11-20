# Candidate migration 9: blocked, unrecorded and unfrozen

`cache-partition-creation.candidate.sql` is deliberately outside `migrations/`.
It has not been applied as migration 9. Migrations 1–8 retain their exact bytes.
No deployment default, production caller, provisioning exception, or readiness
gate is changed by this candidate. It is **not operational or ready to merge**.

## PostgreSQL ownership blocker

The first separate-login candidate run compiled without warnings, then returned
10 passing relay controls and one failing role-backed boundary. SQLSTATE 42501
occurs on `ALTER TABLE <numeric derived child> OWNER TO <cache_owner>`.
The existing owner-only CREATE schema ACL is intact. Both owner roles are
NOLOGIN, and installation_owner has direct INHERIT+SET/non-ADMIN membership in
cache_owner. That membership alone does not satisfy PostgreSQL: the target owner
must itself have CREATE on the table's schema. See the ownership paragraph of
[PostgreSQL 18 ALTER TABLE](https://www.postgresql.org/docs/18/sql-altertable.html).

Consequently the requested conjunction cannot currently use this ALTER OWNER
implementation: distinct cache_owner leaf ownership, ordinary trusted NOLOGIN
installation-owner definer, and schema CREATE restricted at every instant to
installation_owner. No CREATE was granted to cache_owner and the verifier was
not weakened. Direct catalog writes, privileged fallback and a LOGIN owner are
not proposed workarounds.

A transactionally temporary GRANT/REVOKE to the target owner would be a different
privilege-transition design requiring explicit approval and cancellation/
visibility proof; it is not implemented or tested here. A changed ownership/schema
model would also need approval. Until that decision, the candidate remains WIP.

## Candidate shape and actual evidence

The draft retains the synchronous generation-only repository calls. An
AFTER-INSERT DB trigger records a private one-time intent only for a directly
admitted service principal with existing admission and parent protection, and
canonical namespace locking. Intent binds the database installation UUID,
generation/namespace, parent/schema/owner OIDs, session principal/backend, top-level
xid8 and mode/state. No input claims or xmin inference are used. Creation,
attachment, owner change and consumption share the caller's transaction.

A rollback-only SQL installation was successfully rolled back before the owned
candidate test installation. The candidate reaches partition creation/attachment
under the unprivileged API login and then fails the ownership transition. Owned
cleanup is awaited even on failure. The first concurrent split run exhausted the
owned PostgreSQL lock table during DROP OWNED, leaving five exact manifested
roles. Those roles were recovered with exact-set membership revocation/drop and
verified sessions0/cluster roles0; original failed cleanup manifests are retained.
This test binary now serializes its own teardown cascades. No PostgreSQL restart,
global lock-budget change, shared-role cleanup or peer-resource cleanup occurred. A pre-9 generation/leaf is seeded with the
cache owner before candidate installation. This does **not** yet establish fresh
installation/upgrade acceptance: both isolated cases still fail. Savepoint/replay,
concurrency, live write, maintenance, conflict, drift, cancellation and strict
rollout assertions beyond the failure point are authored or planned but not
passing evidence. The candidate is not permission to freeze proposed bytes.

## Test-posture census before any broad harness change

`cache-test-postures.json` is reproduced by
`python3 scripts/inventory-cache-test-postures.py`. It is a static, per-source
census including ignored tests and helper candidates, not a complete dynamic call
graph or execution evidence. Counts exclude the new isolated candidate module.

| Existing common source/module | Declared tests | Direct admin-DDL test candidates | Maintenance call candidates | Remaining product-path candidates |
| --- | ---: | ---: | ---: | ---: |
| cache_repository_tests | 63 | 7 | 12 | 48 |
| cache_live_repository_tests | 9 | 0 | 1 | 8 |
| cache_cascade_repository_tests | 15 | 0 | 3 | 12 |
| cache_reclamation_admission_tests | 16 | 0 | 0 | 16 |
| cache_generation_partition_tests | 8 | 0 | 6 | 2 |
| partition cleanup_acknowledgement | 5 | 0 | 1 | 4 |
| partition direct_cleanup_graph_ordering | 1 | 0 | 1 | 0 |
| cache_large_generation_integrity_tests | 2 | shared helper DDL | shared maintenance helpers | requires separate review |

These eight common files/modules declare 119 tests. Categories overlap: seven
explicit admin-DDL cases and 24 lexical maintenance cases do not sum to a disjoint
partition. Ninety ordinary-product candidates remain after excluding the two
large-integrity cases and direct-DDL/maintenance unions; fixture DML still needs
an admin pool and their hidden helpers require review.

The seven explicit common admin-DDL cases are statistics acknowledgement race,
statistics cancellation, independent creation/cleanup deadlines, missing-storage
retry, nonterminal workflow pin, component retirement rollback, and effective
nonowner statistics rejection. API's partition-creation rollback case also
creates/drops fault objects. Both million-record integrity cases share fault DDL
helpers. These are not valid tests to redirect wholesale to an API pool.

Additional source candidates: 50 API cases (including child coverage/native-cache
modules), 28 common runtime-authority cache cases, 19 executor ordering cases, and
11 CLI cases: 227 declared cases total across 20 files. CLI cases need no direct
PostgreSQL identity. The API `TestContext` passes its fixture pool into AppState;
common `helpers::create_test_pool` returns the migration-owner TestDatabase. These
currently conflate fixture administration and product DB execution. Metadata-only
SQL does not prove that a suite never invokes DDL through a helper.

An approved future harness change must retain an independently owned admin pool
for migration/seed/fault/tamper, use an unprivileged API pool for product paths,
and a distinct cache-maintenance login for DROP/ANALYZE. Do not replace owner checks
with grants to a LOGIN owner or add test-only runtime bypasses. No broad helper
change was made here. Representative existing regression acceptance, full mandatory
candidate boundary coverage, and explicit development/Compose provisioning approval
remain prerequisites to placing bytes in migration 9. Current default shared-owner
cache calls would be denied by the mandatory definer guard; defaults remain untouched.

## Latest exact candidate run

After teardown serialization the owning target reports **11 passed / 2 failed**
in 7.25s. Both fresh-cache and seeded-pre-9 cases fail at the same ALTER OWNER
permission boundary. All original 11 repository/process/relay controls pass.
Before reaching that failure, direct API CREATE/ALTER/DROP/private-intent reads,
unguarded calls, separate offline login, inherited EXECUTE and SET ROLE denial
assertions execute successfully. The pre-9 case additionally verifies the existing
leaf's cache_owner and performs maintenance DROP/ANALYZE under a distinct real
login with cache ownership but no schema CREATE. These are partial controls,
not end-to-end creation acceptance. The later savepoint/live/race assertions
remain blocked by the first attempted new partition.

All six exact newly created ownership manifests report sessions/roles cleaned,
remaining0/errors[]. The prior failed cleanup manifests and exact recovery proof
remain as local validation artifacts. Focused owning-target Clippy passes with
-D warnings; build, fmt and frozen1–8 hash checks pass. SQLx prepare for canonical
migration9, representative existing regression execution, cancellation and all
remaining mandatory boundaries are unrun/blocked. No production provisioning
membership exception or cache EXECUTE grant was added to shipped provision.sql.
