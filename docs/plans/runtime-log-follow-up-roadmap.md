# Runtime-log follow-up roadmap

Status: first-wave planning reconciliation, 2026-11-10 UTC; local documentation
only. This records integrated source and read-only observations, not rollout
approval or certification of deployed behavior.

## Integrated scope and evidence

Source inspected: main `820b4192068ccd321167a28dd888276dd7998368`, fetched and
fast-forwarded from `d9da94122ff6fab725c212ac81a6a965a3bd2dbd` before edits.
GitHub `gh pr view 31/32 --json state,mergedAt,mergeCommit,headRefOid,statusCheckRollup`
reported both merged and all 16 reported head checks successful on each:

| Scope | Merge and tested PR head | CI evidence |
| --- | --- | --- |
| [PR31](https://github.com/attune-system/attune/pull/31): finalization phases 1/2 | Merged 2026-11-10 14:09:12Z as `4ac6ea43b9a52d8c70b345de08357ff8c482874a`; head `c067e88573529466f40849a7b58325a8e4aaa704` | [CI 42587428610](https://github.com/attune-system/attune/actions/runs/42587428610): Tests, Clippy, Rustfmt, smoke, security and web checks successful |
| [PR32](https://github.com/attune-system/attune/pull/32): lock-scope/maintenance work, including PR31 integration and object close | Merged 2026-11-10 14:09:10Z as `820b4192068ccd321167a28dd888276dd7998368`; head `2275b04d2be5f7e2b52cf93f8b0b0c312684498d` | [CI 42807858819](https://github.com/attune-system/attune/actions/runs/42807858819): Tests, Clippy, Rustfmt, smoke, security and web checks successful |

These are PR-head CI results, not a fresh run against this prose or deployed
binaries. Historical failures and pending statements remain dated in the
[implementation record](database-lock-reduction-implementation.md),
[finalization plan](runtime-log-finalization-reliability.md), and
[close protocol](object-log-seal-claims.md). Later successful CI does not establish
the cause of earlier incidents or unclassified timing failures.

Completed source scope: bounded finalization/retry policy, per-stream delivery
outcomes, authorized finalizing/incomplete errors and UI/audit handling; durable
object close selection, out-of-transaction verification, freshly authorized
atomic publication and bounded abandoned-selection handling. Partial scope:
[single-writer simplification](single-writer-log-streaming-simplification.md).
Worker crash recovery, all physical PUT settlement, shared-file transaction/lock
removal, incremental verification and comprehensive alerts are **not complete**.

## Dependency order and acceptance gates

Each row is a separately reviewable follow-up; retain existing guards until its
gate passes on owned disposable infrastructure. PG16/18 and versioned object
storage coverage follow [running tests](../testing/running-tests.md) and
[runtime-log verification](../deployment/runtime-log-verification.md).

| Order | Work | Acceptance gate |
| --- | --- | --- |
| 1 | Review the [worker-loss recovery proposal](runtime-log-worker-loss-recovery.md) and encrypted durable spool/intent. Reuse exact version/frontier/proof and active-owner binding; define terminal publication and credential-expiry behavior. | Crash matrix before/after intent, final upload, acknowledged/unknown commit, takeover and cleanup. No lost claimed-durable bytes, fabricated completion, stale-owner publication or raw-token persistence. In-memory-only bytes are explicitly unrecoverable. Implementation follows protocol review. |
| 2 | Inventory every outstanding write/staging obligation across direct/proxy/multipart paths; implement physical settlement and cleanup ownership. Feeds recovery from row 1. | Delayed pre-admitted PUT, cancellation, duplicate success, expired grants, missing HEAD, forgotten staging and restart tests retain every obligation until settled. Test post-retention recreation; timeout/TTL/closure alone never proves settlement. Accepted membership remains immutable. |
| 3 | Design shared-file close and durable path retirement using the proven ownership/settlement contract. | Separate cross-process and supported RWX/filesystem tests cover writer/flock, rename/unlink, stale open descriptors, restart, revocation and retention races. Prove byte/proof identity and no late recreation before removing transaction/flock protection. Object tests do not certify this backend. |
| 4 | Narrow PhysicalCleanup only after rows 2/3 establish replacement physical authority for each backend. | Physical deletion, unknown commit, late PUT/file writes, restarted cleanup, lease/owner changes and sibling streams preserve exclusion and eventual owned cleanup. Keep current graph/parent/native filesystem fences until replacement is demonstrated. Do not infer safety from metadata closure. |
| 5 | Measure and then consider incremental verification on trusted ingestion. Depends on fixed accepted membership, replay/settlement and durable recovery. | Duplicate/unknown acknowledgements cannot advance hash/frontier twice. Multi-API/restart tests preserve exact frame/manifest/digest verification; bounded reconstruction/full verification remains available. Equal-workload p95, throughput, memory and DB/lock measurements justify complexity. Keep v1 evidence/format. |
| Parallel prerequisite; complete before operational acceptance | Metrics and alerts, starting with provenance/config verification. | Scrape every API replica; verify finalizing/incomplete counters, worker retry/deadline/finalization and shutdown outcomes, pending age, recovery backlog, outstanding-write/cleanup age, pool waits/lock duration. Use bounded labels without log content/secrets. Establish workload denominators and measured thresholds; exercise alert and resolution on disposable faults. Counters alone are not alert coverage. |
| Separate lane | Broader DB/maintenance work in the [DB implementation record](database-lock-reduction-implementation.md), [transaction reuse plan](database-transaction-reuse-remediation.md), and [bounded-retention plan](bounded-retention-query-optimization.md). | Per-plan gates for cancellation/socket quarantine, admission/retention ordering, fairness and production-scale workloads. Preserve unclassified failures and deferred optimization targets; do not count merged runtime-log close tests as universal DB/performance acceptance. |

## Execution completion and delayed logs

The proposed [execution completion and asynchronous log delivery plan](execution-completion-and-log-delivery.md)
separates terminal results from log availability and specifies client retry/SSE
behavior for completed executions whose logs are still finalizing. It extends
the recovery/custody and authority work above to delegated worker delivery; the
existing recovery first slice does not already authorize that handoff. Enable
earlier completion only after the scoped delivery protocol and all consumers
are validated together.

## First implementation and review wave

The immediate implementation wave adds isolated post-merge log acceptance
coverage and audits/fixes bounded worker shutdown. Current service shutdown
previously marked the worker inactive and stopped heartbeats before waiting for
in-flight work. Source audit found deregistration does not directly revoke
recorded delivery permits; owner/policy/approval, assignment/attempt, instance and
expiry checks govern delivery. Local changes stop new admission and keep heartbeat
and cancellation handling alive through drain and owned capture cleanup. They
also fence registration maintenance to a process-incarnation UUID so an old drain
cannot deactivate a replacement worker. Focused shutdown and PG18 replacement
regressions passed; pack-test subprocess ownership and combined validation remain
open. A larger grace period alone does not establish valid completion.

The [recovery proposal](runtime-log-worker-loss-recovery.md) is under independent
protocol review. Its first candidate handles only fully selected encrypted action
logs, with no new uploads and no recovery of memory-only bytes. Publication after
ordinary producer expiry, durable revocation retention, spool durability and
permanent provider fences remain proposed decisions, not accepted permissions or
implemented guarantees. Complete the authority review before enabling recovery.

## Read-only deployment snapshot

Observed 2026-11-10 approximately 14:14–14:17Z via `kubectl` context `default`,
namespace `attune`. No resources, data or executions were changed. Deployment
specs, pod statuses, ConfigMap allowlisted fields and `/metrics` through the
Kubernetes read-only service/pod proxy were inspected; secret objects, runtime
log bodies and production SQL were not accessed.

All Attune service and agent-init tags were `0.10.3`. Worker runtime base is
`nikolaik/python-nodejs:python3.12-nodejs22-slim`, so the injected agent digest
also matters. Selected observed imageIDs (SHA-256, not source attestations):

| Container/pod cohort | Digest |
| --- | --- |
| Non-deleting API `5cb7fb888f` (two Ready pods) | `dd09b3dbf4bccd3c8d2804f36e6dac24516aa88ade855418dd0a87ae3ab408d6` |
| Terminating API `d48477b97` | `0e8086f3bfc69259dc0139aeb348b6adcc8a35db40905fca7ff5c71c9f044693` |
| Non-deleting action worker `5b5ff6d88f` and sensor worker agent-init | `88a698dae73f92610a564815b359c418a816fb9cd323e322a8f9ca85a7548bc0` |
| Terminating action worker `78fc9c48f` agent-init | `bdd171f1697bf8678deb97113421375ce189e28d86c587a723193d6cd61b2f6a` |
| Executor | `7216558bba41ace3d62335f65d2d52c49324e70a4649cdcd9278d1dcf87f24e6` |
| Notifier | `fc29be75511eafa364eb6e0acf5244b45099be6ae3ec38e63a458e1a6c4fe76c` |
| Supervisor | `9f4bdf9823aeedf986284e3407c3163c2448ec674252ff42b66aa5d200d9980e` |
| Web | `61dd7b7ba9f4ad4446cb10016ffc011ee61006ed2da5d37290692f739e7eeeb9` |

Differing digests coexist with terminating pods; no tag-to-source mapping was
established. Do not conclude these binaries include PR31/32. No rollout was
initiated here.

Mounted `attune-attune-config` specifies `artifacts.transport=api`, segment cap
65,536 bytes and `storage.provider=s3`, endpoint host `attune-rustfs`. No bucket,
key, credential or connection-string values are reproduced. Versioning, actual
provider durability and storage contents were not checked.

No `log_finalization_timeout_ms`, worker shutdown timeout or sensor shutdown
timeout appears in the shared config or inspected deployment environment.
Thus the compiled defaults apply absent other runtime inputs; deployed defaults
cannot be certified from tags. Current source defaults are finalization 60s,
worker in-flight shutdown wait 30s and sensor shutdown 30s. Action/sensor pod
termination grace is 45s; API grace is 30s with explicit server drain override
25s. No inspected deployment has a preStop hook. The worker's 30s wait and 45s
pod budget cannot accommodate a fresh full 60s finalization window. API 25s drain
fits a bounded 15s close phase nominally, not an arbitrarily slow storage read
or the whole worker delivery window. Require measured end-to-end drain/cancel
and joined-teardown budgets; simply increasing one timeout is insufficient.

Service `/metrics` and each of the two non-deleting API pod endpoints responded.
They exposed execution-log stream activity/read/retry/wakeup counters but neither
`attune_runtime_log_finalizing_responses_total` nor
`attune_runtime_log_incomplete_responses_total` was present in the sampled output.
Current source exports both in `crates/api/src/execution_log_streams.rs`; absence
is an evidence gap, not proof of zero incomplete logs. No ServiceMonitor,
PodMonitor or PrometheusRule resources were listed in `attune`; external scrape
or alert configuration was not assessed. No controlled workload, production
metric baseline or shutdown experiment was run.

## Fresh-instance rollout checklist (not executed)

1. Obtain separate deployment authorization. Choose an immutable candidate source
   revision and image digests for every service plus injected agent. Retain build
   provenance, passing required CI and applicable packaged PG16/18/backend evidence.
   Resolve remaining gates relevant to claimed scope; record unsupported recovery
   and physical-settlement behavior explicitly if delivering the narrower scope.
2. Inventory old-instance data/retention requirements and backup/restore access.
   Provision a new empty database, new managed artifact/blob/log namespaces,
   isolated broker vhost/queues, fresh signing context and securely supplied
   credentials. Never pair reset numeric IDs with old storage or queue authority.
   Follow [PostgreSQL deployment policy](../deployment/postgresql-only.md).
3. Apply the canonical baseline only on that new instance. Verify schema, storage
   versioning/create-only semantics, access isolation and intended backend.
   Demonstrate that old producers cannot publish into the new instance.
4. Set coherent process/drain/delivery/request/server/shutdown/pod budgets. Allow
   the worker delivery window plus measured registration/teardown overhead, or
   explicitly accept earlier bounded incomplete delivery. Prove graceful shutdown
   and forced termination behavior on the disposable candidate for each producer.
5. Run authorized isolated acceptance: silent/stdout/stderr/both, transient and
   permanent faults, lost ACKs, cancellation, authority expiry/revocation, exact
   version selection, bounded incomplete outcomes, retention and restart cases.
   Verify no unauthorized content or false Ready/EOF; do not create synthetic
   production executions as a substitute for these fixtures.
6. Verify per-replica metrics/scrapes, alert routing and a bounded observation
   plan with failure denominators, not successful-seal latency alone. Quiesce old
   producers before cutover under separately approved operations; ensure old
   queues cannot deliver into new authority. Retain the old environment isolated
   for its retention/access policy. Define rollback before new writes: rollback
   must not repoint an old binary at the new incompatible database/state.

## Alternative: data-preserving in-place transition (separate scope)

A tag change on the existing database is not the supported PR32 rollout.
First construct owned fixtures with genuine pre-close Ready/sealed object logs
(`close_revision=0`, NULL phase), Pending/unsealed streams, outstanding grants,
operational metadata updates, reads/replays and retained/deleting lifecycle cases.
Prove the current consistency-guard rejection and design an explicit transition
that preserves truthful historical provenance without manufacturing producer,
proof or verification evidence. Specify how legacy final rows remain immutable
and readable while supported metadata/cleanup operations work.

Review forward migration/guard changes, quiescence of incompatible producers,
rollback/version-skew limits, resumability, authority and all physical cleanup
obligations. Exercise PG16/18 and applicable storage backends with revocation,
retention, replay, failure and restart matrices. Obtain separate approval for the
transition and rollout after those gates pass. Do not rewrite recorded migration
checksums, automatically reset a database, or treat pre-production policy as
permission to destroy an existing deployment.

## Current local validation checkpoint (2026-11-10)

Changes remain local and uncommitted, based on `820b4192`. The first API-backed
acceptance attempt built E2E, pack-binary and agent images but was interrupted
while compiling shared services, before workload startup: **0 of 21 executions**,
no runner exit status and no API/volume acceptance pass. Owned containers,
volumes and networks were confirmed absent without fallback cleanup. Private
source/diff/build evidence is retained under `.scratch/log-accept-validation/`;
these images do not certify the subsequently integrated shutdown source.

The incarnation-fenced shutdown source passed five focused shutdown tests and
two PG18 incarnation regressions twice with four test threads. Earlier workspace,
API authority and Clippy results apply only to their recorded source checkpoints;
final combined workspace/API/Clippy and SIGTERM/MQ/storage checks remain open.
Source and binary hashes are retained in `/tmp/attune-fence-0e0934cc/`. No database
migration or delivery authority change was introduced by the incarnation marker.

Shipped Docker configuration explicitly retains `worker.shutdown_timeout: 30` and
45-second Compose grace; development workers inherit the proposed new90-second
source default. Absence/null activates90 and requires matching container grace.
The read-only cluster snapshot above is historical, not updated by these edits.

### Combined ownership integration

The pack-test cancellation correction is integrated through narrow service hunks,
retaining the incarnation-fenced shutdown sequence. Its tracker is closed/waited
after the pack-test consumer abort/join barrier and before shared resource close.
Subprocess supervision owns Unix group termination, direct-child reap and inline
reader completion/closure; cancellation is not accepted as a passing warn-policy
result. Windows still owns only the direct child and escaped process groups remain
outside containment. Candidate setup/repository work can extend total stop time.
The separate source passed14 common runner tests, six ordering repetitions, one
worker ownership regression, workspace checks and common/worker Clippy. Those are
component results; combined review, checks and API/full-stack validation must be
reported against the integrated source separately.

### Combined-source validation and final descendant correction

Frozen pre-correction diff `017622e7f125f2f701e2bf4e65582e75a5264c7b2d091e6b2e7e5f341c306956`
passed workspace/all-target checking, common/worker and selected API Clippy with
warnings denied, formatting and Python syntax. Its 69 unique focused tests plus
seven repeats passed on owned PG18.6 with four threads. Five initial repository/
API launches failed before setup from the wrong working directory; corrected
launches passed on a fresh owned server. Both runs had zero clones before janitor
cleanup and no remaining owned containers/volumes. Evidence is retained under
`/tmp/attune-combined-751ca9d1-20261110/`; it is not proof of full-stack acceptance.

Combined review found one remaining action descendant boundary: the group guard
was disarmed after the leader exited but before log finalization. The parent
retained it through incomplete/cancelled/aborted settlement and preserved normal
fully captured completion. The new Linux regression covers token cancellation
and forced abort after leader reap with a blocked upload and live same-group
child. It passed, followed by all 25 process-executor tests on the component
source. Tested binary and source hashes are in `/tmp/attune-leader-exit-proof/`.
That correction is now integrated; affected final checks and owned full-stack
acceptance remain required on this final source. No claimed SIGTERM/MQ/S3 proof
or required CI pass is inferred from component tests.
