# Option A: isolated transactional privilege transition (WIP)

User-approved follow-up to the original ownership failure at `cd1f059a`.
The candidate remains **outside migrations, unrecorded and unfrozen**. Migrations
1–8 are unchanged. No default profile, provisioning asset, service integration or
readiness gate is enabled. No deployment or push is part of this checkpoint.

## Transition and guards

The unchanged repository call remains
`create_cache_generation_partition(generation_id BIGINT)`, synchronous within
snapshot refresh / first live-write transactions. Existing admission → parent →
namespace lock order and normal quota/idempotence logic remain in the repository.
A guarded DB trigger captures the generation's private one-time intent, including
installation, exact parent/schema/owner OIDs, namespace, source session/role,
top-level xid8 and state. Caller claims and xmin are not used. There is no generic
DDL, role or relation-name argument and no new runtime grant/API service.

The NOLOGIN installation owner must be the exact function/schema owner. The
parent must be generation-keyed LIST storage owned by the distinct NOLOGIN cache
owner. Only the direct INHERIT+SET/non-ADMIN owner→cache-owner relationship is
allowed; API has no owner membership or schema CREATE. Function EXECUTE is
explicit/direct; unexpected grants or memberships deny. The original strict
rollout verifier is unchanged and is exercised under the real offline login.

After ordinary validation and physical partition creation/attachment, the function
captures the schema's exact typed `aclitem[]` and owner/OID, then executes:

1. GRANT CREATE on the exact installation schema to the catalog-derived cache owner;
2. ALTER the numeric generation leaf's owner to that same cache owner;
3. REVOKE CREATE and require exact schema OID/owner/ACL restoration and leaf owner.

Names are DB-derived and identifier-quoted. GRANT, ownership change, REVOKE,
accounting and one-time consumption share a statement/transaction. There is no
commit in the function, no exception swallowed, no caller-controlled bypass or
production fault switch. Any failure rolls back the statement; outer SQLx
transactions and savepoints own final rollback/commit. An initial ACL that is
unsafe or cannot be restored exactly is refused, not repaired. The temporary
CREATE is the explicitly approved uncommitted transition, not a persistent grant
to API/cache_owner or a weakening of owner-only committed schema ACLs.

## Isolation experiment

An owned event trigger suspends the creator **after the GRANT is effective in
that backend**, using a run-owned transaction advisory barrier. Exception testing
instead holds after ALTER OWNER and raises before REVOKE. Event triggers exist
only in disposable test databases and are removed/joined; none are in the candidate.

A separate real LOGIN observer SET ROLEs to the exact cache owner. A positive
CREATE control in a separate cache-owner-owned schema proves its ability to
execute DDL. The observer for each installation-schema CREATE attempt connects
fresh *after* the barrier is reached, avoiding reliance on a warmed ACL cache.
It actually attempts CREATE and must get permission denial (42501), not a timeout.
The same fresh attempt is repeated after each transition/fault. The outside ACL
view and post-operation schema OID/owner/raw ACL must equal the original.

The fault matrix runs under both fresh-cache and seeded pre-9 profiles:

| Path | Required outcome |
| --- | --- |
| Success | Exact ACL restored; intent consumed; leaf owner/schema/parent/bound correct; usage/statistics advance once |
| Exception after owner transfer | P0001, rollback-to-savepoint and outer commit; exact ACL; no generation/intent/usage/leaf residue |
| Cancellation | 57014 and awaited rollback; exact ACL/counters and no residue |
| Statement timeout | 57014 and awaited rollback; exact ACL/counters and no residue |
| Lock timeout | 55P03 and awaited rollback; exact ACL/counters and no residue |
| Backend termination | Only PID+backend_start+DB+login proven owned is signalled; connection disposed and backend disappearance awaited; exact ACL/no residue |
| Actual deadlock | Namespace/advisory cycle produces 40P01 in creator; full rollback; exact ACL/no residue |
| Lost acknowledgement | Relay holds durable COMMIT C+idle Z for one exact server-reserved generation; ACL already restored; fresh observer denied before ACK disconnect; client errors but consumed intent persists |

The relay extension recognizes only the exact typed cache-creation statement,
one positive matching generation parameter, and its root COMMIT. It does not
log wire payloads, classify generic entities or authorize an input. Original
relay/parser/savepoint controls remain selected.

Existing isolated snapshot/live first-write, rollback/retry/savepoint,
same/different-generation concurrency, pre-existing ownership, maintenance
DROP/ANALYZE, worker/offline/inherited/SET ROLE/direct-DDL denials and strict
rollout verification remain selected. Read-only replay validation now uses the
same leaf-owner invariant and a fixed trusted search_path; a changed leaf owner
is denied without repair and the exact restored leaf can be retried. Additional
probes deny unexpected initial
schema CREATE, extra function EXECUTE and changed parent ownership, restoring
fixture metadata afterward with exact ACL/OID comparisons.

## Evidence and limits

The original failed candidate SQL remains in git at `cd1f059a`; original failure
logs and checksum-pinned SQL are retained in the owner's local validation lane.
It is historical evidence, not overwritten by the A result. Early build errors
and the corrected relay style lint remain disclosed in the build logs.

Final owned run: **14/14 passed in 7.00s**, with no ignored cases. The owning
test target compiled without warnings in 9.75s; common all-target Clippy with
`-D warnings` passed in 1.23s. Seven offline deployment-asset controls, formatting,
diff checks and frozen1–8 checks pass. All six exact final fixture ownership
manifests report remaining0/errors[]. The exact commit is recorded in the handoff.
Tests
use stock owned PostgreSQL18.1, separately authenticated principals, bounded
barriers and source-fenced termination, explicit pool/connection/relay close/join
and exact cluster-role ownership manifests. Teardown cascades remain serialized
within this test binary to respect the lane's lock budget; no server restart or
shared-role cleanup is used. SQLx slow-query notices are expected for intentional
1.5-second timeouts/deadlock; they are not compiler warnings or secret payloads.

This is isolated evidence, not universal formal proof or production qualification.
Existing broad suites still conflate fixture/admin/product identities as recorded
in `cache-test-postures.json`. No broad harness changes were made. The shipped DBA
provisioning assets/default Compose profile have not been changed to install the
candidate's owner/API posture. Canonical migration9 application/freeze, its SQLx
prepare, representative existing regression execution under split principals,
other PostgreSQL versions, CI and deployment acceptance remain unrun. All seven
recorded-consumer gates remain false. No recorded-authority transport, queue E2E,
publication/admission activation or service rollout integration is claimed.
