# Database lock reduction: code-level implementation map

Status: milestones 1–3 implemented for review; later slices remain proposals.
Date: 2026-11-09 UTC.

This expands the [lock audit](database-lock-scope-audit.md), with a separate
[test assumption audit](database-lock-test-assumptions.md). It identifies concrete
change boundaries; it does not claim that the new concurrency protocols have
already been implemented or proven.

## Baseline and integration

The analysis checkout is 880b1b2a138a335eb549c08452f0d3440de68d41. The locally
available origin/main is 8af23fc4d77ce84d661474cb6711d0db3be906d3.
[PR #31](https://github.com/attune-system/attune/pull/31) is still open at
c067e88573529466f40849a7b58325a8e4aaa704. The first independent checkpoint is based on main 8af23fc4 and excludes PR #31:
its JIT and cache-statistics changes do not touch logging reliability. Later
logging milestones must integrate that reliability change after merge or with an
explicit dependent branch. Do not implement against the older analysis checkout
without reconciliation.

The maintenance admission, maintenance queries, bounded release collector,
pack cleanup, sensor admission, cache admission/statistics, partition repository,
log-stream repository, and graph tracing files examined here are identical to
their PR #31 versions. The API seal path was inspected directly at that PR head.
That PR supplies bounded retries, per-stream outcomes, and accurate delivery
errors; it does not move seal verification outside the database transaction.

### Preserve existing work

- Release discovery already reads raw bounded ID/xmin pages outside graph
  admission. Empty pages already skip exclusive admission. Retain its high-water,
  revision, retry/backoff, partial-progress, and unknown-commit machinery.
- Proxy object-log PUT already happens between reservation and publication
  transactions. Do not introduce a new upload worker merely to move that PUT.
- Shared graph tracing already records owner completion, first acquisition,
  reentry, and observed hold duration. Extend coverage rather than replace it.
- Session cleanup/disposal fixes exist on the newer baseline. Preserve them.
- A queued cache cleanup must release graph admission before waiting on cache
  admission. Workflow entry can hold cache PinMutation before shared graph
  admission; imposing a universal graph-before-cache order is incorrect.

## First functional checkpoint (2026-11-09)

Status: work in progress for early review; not ready to merge.

Implemented only transaction-local JIT policy and cache-statistics no-op avoidance.
Every owned transaction in `maintenance_admission.rs` now disables JIT on entry:
standalone discovery, `run` discovery and protected work, and `PhysicalCleanup`.
The physical cleanup owner keeps that local policy throughout its metadata phases
and physical work until its actual commit/rollback; a helper return does not end
its transaction. Later deadline refreshes do not repeat the JIT setter.
`pack_retention/bounded.rs` covers the initial progress read, raw-page discovery,
protected page work (including empty-page progress), and separately owned deferral
bookkeeping. The existing budget round trip sets JIT; existing deadlines,
admission ordering, acknowledged cleanup, disposal, and unknown-commit accounting
are retained.

Cache statistics validates configuration and effective parent ownership before a
read-only due preflight. Idle and interval-throttled work returns `NotDue` without
`SHARE UPDATE EXCLUSIVE`. Due work still acquires the bounded parent lock and
revalidates ownership and rereads revision/interval eligibility before analyzing. Acknowledgement still uses
only the captured analyzed revision, so concurrent lifecycle requests stay pending.
The preflight never updates statistics state.

Added focused real-PostgreSQL regressions for local JIT restoration on success,
SQL failure, server timeout, and reuse of the same backend; actual maintenance
owner callbacks also assert JIT is off. Statistics tests cover no pending revision
and an interval-throttled pending revision under a conflicting parent maintenance
lock, plus eligibility becoming stale while waiting for that lock. Existing due
lock deferral, concurrent lifecycle acknowledgement, cancellation, graph/storage,
and uncertain-commit assertions are unchanged.

Not implemented: retention SQL rewrites (next checkpoint), durable cursors, log
lifecycle changes, physical-cleanup splitting, or removal of any admission locks.
The five task analysis documents retain their historical/proposed evidence; their
measurements do not establish a new implementation performance result.

### Validation provenance and gaps

Source baseline: `8af23fc4d77ce84d661474cb6711d0db3be906d3`, with the
uncommitted first-checkpoint source diff saved at `/tmp/lock-checkpoint-source.diff`.
SHA-256 of that diff: `fb26d5f613ccfa4470a27bf608c420061af0a51c69a6df843d39bb5a5d87df28`.
The diff covers the five changed Rust files only; subsequent documentation edits
do not invalidate code-test results. Source-file hashes were checked after build
and remain unchanged. This records uncommitted-source evidence, not tests of the
base commit alone.

- `cargo fmt --all -- --check` and `git diff --check`: passed.
- `cargo check --all-targets --workspace`: passed without warnings on the initial
  implementation snapshot (15m56s). This predates the final private ownership
  recheck and additional tests; do not treat it as a final-head workspace result.
  A full final-source rerun was stopped while waiting for Cargo's build lock to
  keep this checkpoint scoped; final-head workspace/consumer CI remains pending.
- `cargo clippy -p attune-common --all-targets -- -D warnings`: passed on the final
  source (23m30s including the earlier build-lock wait). This compiles all changed
  repository code and new tests with zero warnings.
- `bash scripts/check-db-test-threads.sh 4`: passed.
- `bash scripts/check-workspace-deps.sh`: failed with 58 violations. Running the
  same script against an exported snapshot of the base crate manifests produced
  byte-identical output and exit 1. No manifests or dependency policy were changed.
- `bash scripts/run-rust-integration-tests.sh --test cache_repository_tests`:
  failed during image compilation, before tests or dependency-stack startup:
  `session_lease_tests.rs` cannot read `tests/sqlx-session/src/relay.rs`. The
  Dockerfile copies `tests/fixtures/tls.rs` but omits that relay fixture. The
  Dockerfile, referencing test, and fixture are unchanged from the base; no full
  base Docker rebuild was performed. The queued final-source `--crate common
  --filter jit` attempt was cancelled rather than repeat the same packaging
  problem. Both runners reported that no stack was started or needed teardown.
- Host fallback uses the documented `make db-test-setup`, with both database URLs
  explicitly targeting a fresh owned `postgres:18-alpine` container on ephemeral
  loopback port 38363. Because the host lacks `psql`, a temporary host shim
  forwards psql invocations through `docker exec` to that exact owned container;
  SQLx migration uses its mapped host port. Setup passed, with no default cluster
  access. Direct Cargo runs each use their own `ATTUNE_TEST_RUN_ID` and four test
  threads.
- `ATTUNE_TEST_RUN_ID=lock-jit-host ATTUNE__DATABASE__URL=<owned URL> cargo test
  -p attune-common --lib jit -- --nocapture --test-threads=4`: passed, 3 tests
  (the two new JIT regressions and the existing pure recovery-jitter test), zero
  failures/ignored; execution 50.20s after 6m09s build/lock wait.
- `ATTUNE_TEST_RUN_ID=lock-stats-host ATTUNE__DATABASE__URL=<owned URL> cargo test
  -p attune-common --test cache_repository_tests statistics -- --nocapture
  --test-threads=4`: passed, 6 tests, zero failures/ignored; execution 48.75s after
  9m12s build/lock wait. Includes both new tests and existing dry-run, due-lock
  deferral, concurrent revision, and server cancellation cases. PostgreSQL emitted
  slow-query warnings for template/clone setup and clone removal (some removals
  took about 40s); these were completed teardown operations, not test failures.
- `ATTUNE_TEST_RUN_ID=lock-collector-host ATTUNE__DATABASE__URL=<owned URL> cargo
  test -p attune-common --lib repositories::pack_retention::bounded::tests:: --
  --nocapture --test-threads=4`: passed, 28 tests, zero failures/ignored, 82.24s.
  Includes fresh pin/identity revalidation, raw-page wrapping, discovery timeout,
  queued deadline isolation, empty sweeps, and lost commit acknowledgements.
- `ATTUNE_TEST_RUN_ID=lock-storage-host ATTUNE__DATABASE__URL=<owned URL> cargo test
  -p attune-common --test storage_maintenance_repository_tests bounded_admission::
  -- --nocapture --test-threads=4`: passed, 16 tests, zero failures/ignored, 41.99s.
  PhysicalCleanup, rollback, stale identity, physical-authority withholding and
  unknown-commit assertions were retained.
- `ATTUNE_TEST_RUN_ID=lock-evidence-host ATTUNE__DATABASE__URL=<owned URL> cargo test
  -p attune-common --test file_evidence_retention_tests bounded_admission:: --
  --nocapture --test-threads=4`: passed, 15 tests, zero failures/ignored, 65.14s.
- After all invocations exited, the owned server had zero per-test or migration
  database clones before janitor recovery. Its exact container ID and anonymous
  volumes were then removed. No owned test infrastructure remains running.

Remaining review/CI gaps: final-source workspace-wide checks, full cache/graph and
consumer suites, PostgreSQL 16 coverage, Docker runner fixture packaging, and
required CI. The dependency-policy failures above are reproducible on the base.
This checkpoint is saved progress for Astra review, not merge readiness.

No migrations, SQLx query macros, or public API contracts changed; schema-prepare
and generated-client updates are not applicable. No latency, production workload,
PostgreSQL 16, or full service/broker acceptance result is claimed.

## Milestone 2 checkpoint (review pending)

The early Astra review of `3f3b8465078a7ebad0b7d8da852a8d9c8a26ccf6`
completed with **no actionable runtime bugs**. It confirmed transaction-local
setting coverage, cleanup, and statistics revision handling. Its nonblocking
coverage request was effective ownership rejection and ownership loss while
waiting for the parent lock. Final-source workspace/consumer CI and PostgreSQL
16 were outstanding at that review; the milestone-1 results above are scoped to
that checkpoint, not silently carried forward as final-source passes.

Query checkpoints `c54813ab` and `b6866375` materialize current ID/xmin and cheap release
retention eligibility, then filter pins in a second materialized stage reading
only those survivors, before rejoining base identities for row locking. This
avoids base-table predicate pushdown ahead of the coarse join and leaves the
non-retention phases on their original query shape. Every release pin source
is retained, with OFFSET 0 barriers preventing anti-join pull-up, including both
nested static-action paths. Existing raw discovery, durable cursors, graph/table
lock modes, SKIP LOCKED, deletion allowance, retries and unknown-commit machinery
are unchanged. The materialized boundary fences execution; it does not avoid
planning the pin subqueries when coarse eligibility is empty. Application-level
avoidance of that planning is not claimed.

Artifact count, find and budgeted discovery share a materialized expiration/body
prefilter before evidence eligibility. Claim retains current parent locking and
rechecks the same retention predicate plus evidence under graph admission. The
version threshold uses OFFSET/LIMIT existence only with a positive configured
limit; bigint subtraction avoids overflow on extreme negative integer limits.
The existing active-object/shared-file distinction, version gaps, body states,
strict comparison and created/ID ordering remain. No durable artifact raw cursor
or bounded raw scan is claimed; that is the next milestone.

New positive fixtures cover all JSON release pin shapes, an independent expanded
pin-set reference, pin insertion after raw identity discovery, exact delete
allowance, and EXPLAIN ANALYZE inside a rolled-back savepoint (no plan-tree
assertion). Artifact fixtures cross all four policies, negative/zero/positive
limits, version gaps, NULL/ready/pending bodies and open object/shared-file logs;
selected identities/counts/budgeted prefixes are compared with the original
count-all reference. Claim checks cover unchanged candidates, changed policy and
a newly committed evidence pin. Statistics uses real roles and inherited
ownership, plus independent sessions with an observed pg_locks waiter before
changing the actual relation owner; both the idle preflight and post-lock ownership checks are necessary.

Validation below distinguishes intermediate attempts from final-source results. The intermediate
PG16 collector run (`c54813ab` query shape plus new tests) passed 28/29, including
the positive JSON fixture, but failed the unchanged production-default
continuation-delay assertion at bounded/tests.rs:973 (expected 1–2 seconds).
It ran alongside compilation and other template-building suites. No baseline
reproduction or passing-suite claim is made for that attempt. `b6866375` also
restores the original query shape for non-retention phases; the final collector
reruns below retain this failure evidence. No deadlines or assertions were relaxed.
The run owns container `attune-m2-pg16` (ID
`6714caec742bc6832d3b4b50effe5004f27a078a9ac1daf7aac20e3d87c0d49e`),
ephemeral loopback port 39257. `make db-test-setup` passed using both explicit
owned URLs and a temporary psql shim forwarding only to that container; no
cluster defaults or Kubernetes contexts were used. The first no-run compile
caught a SQLx raw_sql/after_connect lifetime error in the new role fixture; the
fixture now uses a parameterized set_config query. That failed attempt is not
passing evidence. The first storage run passed 30 existing tests but the new
matrix fixture failed at its first policy UPDATE with PostgreSQL 42804 (text
assigned to artifact_retention_enum). Explicit enum casts were added for policy,
body state and backend before the final runs; that attempt is not a suite pass.

Milestone-2 validation commands (four libtest threads throughout):

```bash
# Owned PG16: final source plus positive and existing bounded regressions.
ATTUNE_TEST_RUN_ID=m2-final16 ATTUNE__DATABASE__URL=<owned-pg16-url> \
  cargo test -p attune-common --lib --test cache_repository_tests \
  --test storage_maintenance_repository_tests --test file_evidence_retention_tests -- \
  repositories::pack_retention::bounded::tests:: statistics bounded_admission:: \
  expiration_prefilter_matches --nocapture --test-threads=4
# Separate owned PG18: all three new positive/ownership cases.
ATTUNE_TEST_RUN_ID=m2-positive18 ATTUNE__DATABASE__URL=<owned-pg18-url> \
  cargo test -p attune-common --lib --test cache_repository_tests \
  --test storage_maintenance_repository_tests -- release_json_pins_match_reference \
  expiration_prefilter_matches cache_statistics_rejects_effective_nonowner \
  --nocapture --test-threads=4
cargo clippy -p attune-common --all-targets -- -D warnings
cargo clippy -p attune-common --test storage_maintenance_repository_tests -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Final-source evidence is based on `fe886366` (query checkpoints plus committed
coverage; only this prose record was uncommitted). PostgreSQL 18.6 passed all
three new cases: release JSON/reference/deletion (1 test, 48.59s including template
setup), statistics ownership (1, 1.84s), and artifact reference/policy/pin claims
(1, 4.09s). No failures or ignored tests. The release fixture's rolled-back
EXPLAIN ANALYZE observed 25.764 ms planning and 30.005 ms execution. This is a
small positive owned fixture, not representative production throughput or a
latency acceptance result. PG18 container ID
`6b305d35e29f768e3c8daa9d83d85dbd26a8276944a4de66bc2ff8f1071a8d1e`
used ephemeral loopback port 40179; only its owned `attune_test` bootstrap DB was
created before TestDatabase built the canonical migration template.

All-targets common Clippy passed with `-D warnings` on `519581fb` (10m11s including
the shared Cargo lock wait). The subsequent change only adds the preexisting-pin
fixture to the storage test; a final scoped Clippy rerun covers that target.
Other all-targets lint results are reusable because no other source changed.
Formatting and diff checks passed on the final source.

The first final-source PG16 combined command stopped after the library binary:
28/29 collector tests passed, including the positive JSON/reference case, but
the unchanged continuation-delay assertion at bounded/tests.rs:973 failed again
(expected 1–2 seconds). Runtime was 151.35s. This is **not** a passing collector
suite and has not been reproduced on the base, so it is not classified as a
baseline failure. The final positive release EXPLAIN observed 30.664 ms planning
and 24.768 ms execution; again, this is a small owned fixture only.

With no source changes, a follow-up invocation reused the same run-owned PG16
template after the failed command had fully exited:

```bash
ATTUNE_TEST_RUN_ID=m2-final16 ATTUNE__DATABASE__URL=<owned-pg16-url> \
  cargo test -p attune-common --lib --test cache_repository_tests \
  --test storage_maintenance_repository_tests --test file_evidence_retention_tests -- \
  production_defaults_cross_ten_thousand_pins_and_persist_fast_continuation \
  statistics bounded_admission:: expiration_prefilter_matches --nocapture --test-threads=4
```

It passed the continuation test alone (1, 11.91s), statistics (7, 4.76s), evidence
bounded admission (15, 25.00s), and storage bounded admission plus the new matrix
(17, 14.35s), with no failed or ignored tests. This does not erase the broad
collector failures. The final scoped storage-test Clippy check also passed
`-D warnings` on `fe886366` (12m26s including build-lock wait).

The same run identity is intentionally retained for sequential commands within
this owned validation session to reuse its template; commands do not overlap.
A final quiet four-thread collector rerun uses that same template and source,
without concurrent compilation or another suite. Its result is recorded below.

The quiet four-thread PG16 collector rerun passed **29/29**, zero failed/ignored,
in 47.92s (same command as the earlier collector run, `ATTUNE_TEST_RUN_ID=m2-final16`).
Its small positive release EXPLAIN observed 31.561 ms planning and 23.693 ms
execution. The isolated and quiet passes do not establish that the earlier
continuation-delay failures were baseline issues; load sensitivity remains a
review/CI concern. No test assertions, budgets, or defaults were changed.

Both owned servers were inspected after all their invocations exited and before
any janitor: zero per-test clones and zero statistics fixture roles remained.
Their exact container IDs and anonymous volumes were then removed, along with
the temporary psql shim. No owned database/test process remains running. Logs
remain at `/tmp/m2-{final16,isolated16,quiet-collector16,positive18,clippy,clippy-final}.log`
for the parent checkpoint review.

Status: **work in progress for parent Astra review**, not merge readiness.
Implemented scope stops at query shape and regression coverage. Durable artifact
paging, logging/PR31, seal claims, scoped lock protocols and migrations remain
out of scope. No pushes or PR updates were made by this implementation agent.

Remaining gaps: required final-head CI, workspace/consumer checks, full affected
consumer/service suites, and production-scale planning/execution under load.
The milestone-1 PG18 suites remain historical checkpoint evidence; final-source
PG18 coverage here is the three new positive tests, while PG16 covers the selected
existing bounded suites too. The unchanged Docker relay-fixture packaging blocker
and 58 dependency-policy violations retain the exact provenance recorded above;
neither is labeled passing. No migration, SQLx macro metadata, or public contract
changed, so schema-prepare and generated-client changes are not applicable.

## Milestone 2 Astra follow-up

Astra reviewed `0a495c55e53c73889056fb9c9d1e13abe076eee0` read-only and found
no concrete runtime defect. Its P3 coverage finding was that the artifact matrix's
evidence pin masked the only expiring shared-file version, and the selected
limits did not distinguish whether NULL-state newer versions were counted.

The follow-up adds an unpinned versions/limit=2 case and explicitly requires
shared-file version 3 to expire because NULL-state version 12 and Ready version
20 both count. Versions-policy limits now include both integer extremes; these
extremes are not applied to interval policies. A separate positive policy-revision
test exercises both optional-queue-pin modes and preserves the newest inactive
release, closing coverage of that changed materialized path.

The original continuation-delay assertion and production budgets remain unchanged.
Its failure message now includes the observed delay, durable progress, and phase
outcomes. A passing retry does not classify the earlier PG16 failures as baseline.
Follow-up source evidence is `0a495c55` plus the two changed test files, saved
as followup-test-source.diff in the root's owned temporary validation directory.
SHA-256: `4af7cc0a0beeb12e0eb010b2673601075d8854b5bf194a99c7dcabf25dc090b9`.
Production Rust source is unchanged by this follow-up.

An owned PostgreSQL 16 container on an ephemeral loopback port was initialized
with `make db-test-setup`, using explicit owned admin/test URLs and a temporary
psql forwarding shim. An initial bootstrap raced temporary-server readiness and
created split migration history; only that task-owned bootstrap was recreated
before the successful clean setup. Those setup attempts are not passing tests.

```bash
ATTUNE_TEST_RUN_ID=lock-review-fix16 ATTUNE__DATABASE__URL=<owned-pg16-url> \
  cargo test -p attune-common --lib --test storage_maintenance_repository_tests -- \
  policy_revision_survivors expiration_prefilter_matches --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=lock-review-fix16 ATTUNE__DATABASE__URL=<owned-pg16-url> \
  cargo test -p attune-common --lib -- \
  production_defaults_cross_ten_thousand_pins_and_persist_fast_continuation \
  --nocapture --test-threads=4
```

Both targeted tests passed (policy-revision case 71.72s including template setup,
artifact matrix 3.41s). The separate quiet continuation run passed in 11.52s;
the prior failures remain unclassified. Each invocation used four libtest threads
and reused only its own run-owned template after the previous command exited.
Final `cargo clippy -p attune-common --all-targets -- -D warnings` passed in
1m30s. Formatting, local links, and diff whitespace passed. After all commands
exited, zero per-test database clones remained; the exact owned container and
anonymous volumes were removed, along with the temporary forwarding shim.
Validation logs and the source diff remain in /tmp/attune-lock-review-eade_5to.
These focused results do not resolve the earlier timing failures or replace
final-head CI and affected-consumer validation.

## Milestone 3: durable bounded artifact discovery (initial checkpoint)

Implemented in this worktree after `9c832ca9` (plus isolated test-guard correction
`d78b720f`): a forward migration adds independent artifact discovery state and
`artifact_version(created,id)`. Existing indexes were examined: `created DESC`
lacks the deterministic ID tie-break, while `(artifact,version DESC)` supports
version-frontier checks but not global oldest-first traversal.

Discovery serializes progress-row updates, freezes a greatest `(created,id)` per
pass, and visits at most `clamp(batch_size,1,256)` raw rows before retention or
evidence predicates. All eligible rows in the raw page are returned, including
when fewer than the requested limit survive filtering. Progress includes empty
filtered pages; completion wraps on the next call. Reporting count/find and their
global oldest-first contracts are unchanged. Raw reservations commit before a
separate bounded eligibility read, using one
absolute page deadline and detached-session ownership. A repeatedly failing
eligibility page therefore cannot trap later raw pages. Discovery results are
released only after an acknowledged reservation and successful eligibility read;
a failed or cancelled scan cannot expose a candidate page. Progress is advisory,
so rows
skipped by cancellation after a commit, late commits, changed policy/pins or
metadata are revisited on later passes. This adds no deletion authority.

Expired-artifact cleanup now uses the existing object-retention job's durable
cadence, independently of hourly runtime retention. Successful pending raw pages
request a one-second continuation; errors retain existing completion-time retry
backoff. One raw page is processed per attempt. Physical deletion remains in the
owned task and retains its current `PhysicalCleanup` graph/parent fence, current
policy/evidence checks, and freshly locked physical identity. No seal, logging,
sensor, quota, cache protocol, production, branch, push or PR changes are included.

Tests retain the positive shared-file/NULL-newer/extreme-limit and queue-policy
predicates from `9c832ca9`. The former bounded/global-prefix comparison now checks
the complete frozen pass against the same independent global oracle. New cases
cover prefilter bounds, empty pages, 550 pinned rows, competing progress writers,
cancellation, disconnect during state commit, wrap/unpin, late older commits,
new arrivals beyond the high-water mark, changed evidence and physical identity,
failed eligibility after a committed reservation, and supervisor pending-page
scheduling without hourly retention or hot loops.
The separate test-guard checkpoint corrects the SQL string inspected by the
existing reporting-count failure fixture; it does not weaken its assertions.

The lower cursor is an indexable tuple comparison even for PostgreSQL generic
prepared plans; an optional `OR cursor IS NULL` would filter old prefixes after
a generic plan switches away from parameter-specific planning.

Validation results and resource cleanup are appended after final runs below.
Prior milestone results remain historical evidence only; the two earlier PG16
continuation-delay failures remain unclassified. Docker runner relay omission,
58 base-manifest policy violations, PR31 and final-head CI remain disclosed gaps.

### Milestone 3 validation provenance

Final runtime/test source: `4f1465cd115cdfe3065b7f925754c2ac1fa7bbe2`, clean
when final runs began. A later fixture-only checkpoint enables panic cleanup for the new supervisor
fixture; runtime source is unchanged. No earlier
milestone performance measurement is claimed for this implementation.

Owned disposable PostgreSQL containers (ephemeral loopback ports):

- PG16 `attune-durable-ret16`, ID `6ef4fe392ae03936481fd9ba40d209acca2ec762630c7270da918fdf4415d2be`, port 43105.
- PG18 `attune-durable-ret18`, ID `1d1164448b703061494596a65462c44b97b3d20e30d59532a5007167c9bcc26e`, port 45007.

Each used `POSTGRES_USER=attune`, a test-only password, and `attune_test`; schema
and database search path were initialized explicitly inside that owned server.
No default cluster was used. With `DATABASE_URL` set to each owned URL,
`cargo sqlx migrate run` passed fresh migration replay and a second no-op replay
on both versions (logs `/tmp/attune-durable-{migrate,rerun}{16,18}.log`). The new
migration has not changed since those runs; no applied migration was rewritten.

```bash
# Owned PG18 URL; includes workspace-wide all-target check.
DATABASE_URL=<owned18> cargo sqlx prepare --workspace -- --all-targets
cargo clippy -p attune-common -p attune-supervisor --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check

# Final PG16 source; owned URL and ATTUNE_TEST_RUN_ID=durable16.
cargo test -p attune-common -p attune-supervisor --bins \
  --test maintenance_repository_tests --no-fail-fast -- \
  bounded_metadata:: artifact_empty_raw_pages housekeeping_defers \
  physical_cleanup_keeps --test-threads=4

# Quiet final storage run, after ALL compiler invocations exited; cwd crates/common.
../../target/debug/deps/storage_maintenance_repository_tests-1c806b184497e4a7 \
  bounded_admission:: expiration_prefilter_matches --test-threads=4
```

Schema prepare passed (12m07s including build-lock wait); SQLx reported `no queries
found`, so no offline metadata changed. All repository queries here are dynamic.
Later changes to the private reservation helper/SQL have no query macros or
schema/API changes; final common/supervisor all-target Clippy checks their types
and affected consumers. Final Clippy passed with `-D warnings` (10m32s including
wait), as did formatting and whitespace. Logs:
`/tmp/attune-durable-{prepare,clippy-final,fmt-final}.log`.

Final PG16 repository tests passed **11/11** (7.67s), including the deliberately
failed eligibility-page progress/revisit case. Supervisor tests passed **3/3**
(3.29s), including cancellation through the slow physical fence and independent
scheduling. Log: `/tmp/attune-durable-reserved-final16.log`. The quiet storage run
passed **17/17** (15.57s), retaining the shared-file/NULL-newer/extreme-limit
positive predicates and pin/identity/unknown-commit cases. Log:
`/tmp/attune-durable-storage-quiet16.log`.

PG18 uses those exact final executables, sequentially from `crates/common`, with
`ATTUNE_TEST_RUN_ID=durable18` and the owned PG18 URL: maintenance executable with
`bounded_metadata::`, supervisor executable with `artifact_empty_raw_pages
housekeeping_defers physical_cleanup_keeps`, then storage executable with
`bounded_admission:: expiration_prefilter_matches`, all with `--test-threads=4`.
PG18 passed **11/11** repository (8.05s), **3/3** supervisor (3.28s), and
**17/17** storage (13.80s), with zero failed/ignored tests. Logs:
`/tmp/attune-durable-{reserved-final18,supervisor-final18,storage-quiet18}.log`.
Final executable SHA-256 values before the panic-cleanup-only fixture follow-up:

- maintenance: `712caec5a8e2bf70a96514d61963b0822a88507cf6a85ff9a7d72e28f625bfa9`
- supervisor: `083d457c2459547bbbc24771c248521ea26990b7059256a8bb50f5a6518cee22`
- storage: `1a345869fea140dae5d5b571fe6de7fd0a48b6aae8ca04e9fdb8a24119f2ad40`

A PG18 generic-plan probe copied the table/index layout into a session-local
temporary fixture and inserted 10,000 equal-created rows. With
`plan_cache_mode=force_generic_plan`, the production raw predicate selected two
rows beyond cursor 9,000 using the composite index (two heap fetches, three local
buffer hits). Log: `/tmp/attune-durable-keyset-populated18-corrected.log`. This
supports the index/query shape, not production-scale latency or throughput claims.
An empty-table plan could choose the old created-only index; index choice depends
on statistics. The initial populated probe omitted required `body_updated` and
failed setup; its log is retained and is not passing evidence.

Earlier runs and corrections remain disclosed:

- `/tmp/attune-durable-test16.log`: 6/8; `/tmp/attune-durable-test18-corrected.log`:
  8/10. Two new fixtures tried direct pin deletion, correctly rejected by the
  cleanup guard. `dd3b1b7f` fixes them using completed owner cleanup and exclusive
  evidence admission, preserving pin protection.
- `/tmp/attune-durable-test18.log`: direct-binary invocation from repository root
  failed before setup (helpers require crate cwd); corrected runs use crate cwd.
- `/tmp/attune-durable-supervisor16.log`: 1/3; `f777d7b1` corrects timestamp
  precision and advances both independently persisted retry deadlines. The
  physical fence passed even in this run. No production budget was weakened.
- Initial Clippy reported tuple type complexity; `05d9e136` adds a type alias.
- `/tmp/attune-durable-combined16.log`: 44 common tests and 8 repository tests
  passed on the earlier single-transaction variant; storage 16/17, with
  `combined_cleanup_rolls_back_version_triggers_when_parent_cleanup_fails`
  reporting `CommitUnknown` at its deadline. `/tmp/attune-durable-storage-final16.log`
  passed that case but was 16/17 due to an observed-lock timeout in
  `combined_cleanup_rolls_back_when_artifact_lock_defers_and_retries_the_whole_unit`.
  Both invocations overlapped compilation after Cargo released its build lock.
  The quiet 17/17 retry does not establish either failure as baseline. They remain
  unclassified, alongside the two earlier milestone-2 continuation-delay failures.

The pre-teardown leak audit found zero clients on both servers, zero PG18 test
clones, and one PG16 clone from the earlier failed new scheduler assertion:
`attune_db_durable16_f1bdf66a53c844cab2f814ac3d2642a2`. Its artifact ref was
`owned_housekeeping_followup_raw_prefix`, identifying that failed fixture.
The fixture lacked `with_cleanup_on_drop`; the follow-up adds the standard panic
recovery guard while preserving explicit awaited cleanup on success. The exact
failed-run clone was then dropped on the owned PG16 server. This is disclosed
failure recovery, not a claim that every initial run cleaned itself normally.

The panic-recovery-only follow-up is
`a4366fd5a858adad32ae70170ee2e724a6439e1a`. On that source, the same supervisor
Cargo command passed **3/3** on PG16 (13.72s) and the rebuilt executable passed
**3/3** on PG18 (3.47s). Supervisor all-target Clippy with `-D warnings` passed
again (43.69s including wait). Logs:
`/tmp/attune-durable-supervisor-cleanup{16,18}.log` and
`/tmp/attune-durable-supervisor-cleanup-clippy.log`. The rebuilt supervisor
SHA-256 is `e6ccd8e1187314b9773d5b8e353abd6ec11ee07f786c75a1ff3879ef526f321a`.
Formatting and whitespace passed again (`/tmp/attune-durable-fmt-complete.log`).
Common runtime and repository-test sources are identical to the final 4f1465cd
runs, so their PG16/18 and common Clippy results remain valid for this fixture-only
follow-up. Subsequent source changes are this documentation report only.

After those final runs exited, both owned servers reported **zero test clones
and zero clients**, before server teardown. Their one run-owned template each
was expected cached infrastructure. The exact two container IDs above were then
removed with `docker rm -f -v`, including their anonymous volumes/templates.
No task-owned server, test process, or psql shim remains; no default cluster or
neighbor resource was altered. Logs remain under `/tmp/attune-durable-*` for parent
review. The shared worktree retains only committed task source and build cache.

Status remains **work in progress for parent Astra review**, not merge readiness.
Final-head CI, production-scale performance and the previously disclosed Docker
relay fixture omission / 58 unchanged base-manifest dependency-policy violations
remain gaps. PR31 and later seal/cleanup/sensor/cache protocols remain separate.

## Milestone 3 P2 correction: bound membership by insertion ID

Astra found that the timestamp tuple at `ea422e3f` did not freeze membership:
a future-created high row lets arbitrarily many higher-ID backdated arrivals
enter between the cursor and high tuple, indefinitely preventing wrap. That
checkpoint is not accepted as complete despite its scoped passing runs above.

The correction freezes the maximum visible insertion ID and visits primary-key
pages by ID. Separate initial and continuation SQL shapes keep both bounds
indexable under generic plans, without filtering an arbitrarily large timestamp
prefix or excluding the minimum signed ID through a sentinel. Eligible rows are
still oldest-first within each raw page; global reporting count/find ordering is
unchanged. Whole-pass tests compare exact candidate sets rather than assuming
private pages concatenate into globally timestamp-ordered output.

A new forward migration restarts advisory progress, removes the timestamp
columns and unused maintenance composite index, and adds the ID-state invariants.
The original migration bytes remain intact. A populated old-state migration
fixture verifies artifact versions survive and discovery safely restarts.

The deterministic P2 regression reserves and skips A/B after eligibility failure,
then inserts backdated pairs before a future-created high row on every call. The
original pass must finish, and the next pass must revisit A/B. The late-commit
fixture now allocates its uncommitted ID before the visible rows so it really
falls behind the ID cursor. The scheduler fixture is extended through its final
page, normal cadence, and the next pass. Earlier timing failures remain
unclassified; the results below do not erase them.

Astra re-reviewed `7be4dd397dba217e15f08286f38fc2024f508319` against `ea422e3f`
and confirmed the P2 is fixed, with no new must-fix runtime defect in that delta.
Its review was read-only; signed-ID extremes and deleted-frontier cases were
supported by source reasoning, not newly executed boundary tests. This is a
focused review conclusion rather than approval to merge the full transformation.

### ID-frontier correction validation

The complete pipeline tested clean source `7be4dd397dba217e15f08286f38fc2024f508319`.
This later validation record changes prose only. Exact commands, exit statuses,
timings, executable paths and owned-server removal evidence are retained in
`/tmp/attune-frontier-validation-state.json`; logs are under
`/tmp/attune-frontier-validation-cmqgg7rj`. The driver remains at
`/tmp/attune-frontier-validation.py`. It builds before testing and executes every
suite sequentially, with four libtest threads, so no test suite overlaps compilation.

```bash
cargo test -p attune-common --test maintenance_repository_tests \
  --test storage_maintenance_repository_tests --no-run --message-format=json
cargo test -p attune-supervisor --bin attune-supervisor --no-run --message-format=json
# Explicit owned PG16/18 URLs; temporary psql shims target their exact containers.
make db-test-setup TEST_DB_ADMIN_URL=<owned-admin-url> TEST_DB_URL=<owned-test-url>
DATABASE_URL=<owned-pg18-url> SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets
cargo clippy -p attune-common -p attune-supervisor --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Both owned servers applied the canonical migrations successfully. The populated
old-state fixture independently replayed the actual 001/002 SQL against timestamp-
valid progress whose IDs were reversed, verified both versions survived, and
restarted discovery. Schema prepare passed (149.69s); SQLx reported no macro
queries found and changed no offline metadata. Common/supervisor all-targets
Clippy passed with warnings denied (79.01s); formatting and whitespace passed.

Using the same compiled executables on each server, with run identities
`frontier16` and `frontier18`, the direct runs were:

- maintenance executable, cwd crates/common, filter `bounded_metadata::`: PG16
  **13/13** (79.99s including template setup), PG18 **13/13** (44.69s).
- supervisor executable, cwd crates/supervisor, filters `artifact_empty_raw_pages`,
  `housekeeping_defers`, `physical_cleanup_keeps`: PG16 **3/3** (3.49s),
  PG18 **3/3** (4.23s).
- storage executable, cwd crates/common, filters `bounded_admission::`,
  `expiration_prefilter_matches`: PG16 **17/17** (13.53s), PG18 **17/17** (20.30s).

All used `--nocapture --test-threads=4`, with zero failures or ignored selected
tests. Executable SHA-256 values:

- maintenance: `b677284cbe491446240189045667c375b4f8a14b36b95fbfd5fe7deaba1f37d9`
- supervisor: `fb7a4473e0e3bfb5478015e77776eaf26c24a73a56673989ad662a1a6839f6c0`
- storage: `a4687ffa55fca35afb705e84dab958220da67ed4ff96b246fd45cde97d5a8dcb`

A PG18 temporary 10,000-row ID/created fixture, including a future-created row,
used `plan_cache_mode=force_generic_plan`. The continuation query selected two
rows past ID 9000 using its primary-key range with two heap fetches and three
local buffer hits. This verifies the chosen query shape on that owned fixture,
not a production latency target or a universal plan-tree guarantee.

After all commands exited, each server reported zero per-test clones. Their
run-owned templates were removed with the exact owned containers and anonymous
volumes; both temporary psql shims were removed. No owned server remains.

Status: scoped correction validation and focused Astra re-review passed. Final-head
CI, broader affected-consumer execution, real successful-COMMIT/lost-ack coverage,
and eligibility-error scheduling alongside other object work remain gaps. Prior
timing/cleanup failures remain unclassified. The existing Docker fixture-packaging
and base-manifest dependency-policy failures are not relabeled as passes.

## Slice 1: cheaper bounded maintenance, with the same locking contract

This is the strongest first implementation candidate: it reduces existing
exclusive occupancy and database work without changing ownership semantics.

| Source and entry points | Specific change |
| --- | --- |
| [maintenance_admission.rs](../../crates/common/src/repositories/maintenance_admission.rs): configure, probe, run, PhysicalCleanup metadata | Set transaction-local jit off at owned transaction entry. Keep remaining server statement budgets, acknowledged cleanup, and socket quarantine. Avoid adding another setting query before every SQL statement if one transaction-entry setting suffices. |
| [pack_retention/bounded.rs](../../crates/common/src/repositories/pack_retention/bounded.rs): page, budget_with_lock_timeout, defer | Cover discovery, protected work, and independently owned bookkeeping transactions. Preserve the existing raw-page protocol and do not count JIT disablement as a replacement for query rewriting. |
| [pack_retention/queries.rs](../../crates/common/src/repositories/pack_retention/queries.rs): delete_sql, RELEASE_PINS | Separate cheap age/active/newest protection from expensive nested JSON dependency checks. Use the correlated existence-query shape validated in the retention investigation where a planner barrier is needed. Preserve every pin source, candidate ID/xmin guard, ID tie-break, and delete limit. SQL predicate order alone does not force evaluation order. |
| [maintenance.rs](../../crates/common/src/repositories/maintenance.rs): expired_artifact_version_predicate, expired_artifact_candidates_bounded, claim_expired_artifact_version | Separate retention eligibility from evidence eligibility. Use an explicit materialized prefilter before file_evidence_version_eligible. Replace count-all-newer with a bounded existence threshold, preserving positive-limit and ready-version semantics. Reuse the exact eligibility logic in authoritative claim revalidation. |

The first artifact rewrite can preserve the existing candidate interface and
ordering. It still is not a bounded raw scan merely because its output has LIMIT.
For a subsequent bounded implementation, add an internal raw-page result with a
cursor, frozen pass upper bound, scanned-row count, and eligible rows. Store
durable progress per artifact-cleanup scan, independently of the release phases.
Advance on raw scanned rows, including pages containing no eligible candidates;
wrap so formerly pinned or changed rows are revisited. Revalidate identity and
current policy under reclamation admission before deletion.

Use the existing [retention query plan](bounded-retention-query-optimization.md)
for prototype SQL and comparison fixtures. Do not adopt its measured savings as
new performance evidence without rerunning against the implementation baseline.

Migration: none for the initial query/setting rewrite; a durable artifact cursor
may require a forward migration. No retention policy or authorization change.

## Slice 2: separate object-log verification from publication

### Refactor the route before changing its transaction lifetime

The PR #31 seal route in
[internal_files.rs](../../crates/api/src/routes/internal_files.rs), seal_log_stream,
does authorization, artifact/version/stream locking, grant checks, segment
GET/hash/frame verification, final evidence insertion, seal, and Ready publication
inside one transaction. Its publication code explicitly relies on rows having
remained locked since the first validation.

First extract the byte-verification loop into an internal helper that accepts
only exact segment metadata, immutable proof inputs, and a storage reader. Return
size, concatenated SHA-256, and authenticated frame-completion results. Its type
must not accept a PgConnection, Transaction, or authorization effect object.
Keep the current outer transaction while validating this behavior-preserving
extraction. This step alone removes no locks.

Next introduce private lifecycle operations, with names such as:

- prepare_close: fresh authorization, exact owner/frontier selection, and durable
  input protection in a short transaction.
- verify_close: full pinned-object reads and frame verification without any
  checked-out database connection.
- finish_close: fresh authorization and comparison followed by atomic final
  evidence, seal, Ready, and close-resolution publication in another transaction.

An internal PreparedLogClose value should bind version, stream, close revision,
producer/attempt where applicable, requested truncation, next sequence, byte
count, proof identity, and exact ordered segment identities. A VerifiedLogClose
must be constructible only by successful full verification and bound to that
same input identity. These are proposed internal types, not new public formats.

Do not carry authorize_version_mutation's effect across transactions. Call the
current source/worker/origin authorization helpers again in finish_close; compare
current attempt, assignment, body state, frontier, proof, and deadlines. Existing
ensure_fresh_on calls alone do not establish all those relationships anew.

### Durable changes required before releasing the prepare transaction

The reviewed design uses private close state on the existing stream: monotonic
revision, permanent immutable input/frontier/producer/proof identity and a phase.
Closing supplies a temporary verification pin. Abandoned and published remain
permanently closed to append. Retain closed streams until allowed version teardown;
standalone deletion/recreation cannot reset this identity. Do not add a generic
distributed job scheduler or duplicate worker authority.

Keep public sealed false and the body Pending during verification. Do not insert
runtime_log_final_manifest until verification and final publication succeed.
Preserve the v1 encrypted-log format and concatenated-stream SHA-256 contract.

Required participants are broader than the seal handler:

| Participant | Required change |
| --- | --- |
| [log_stream.rs repository](../../crates/common/src/repositories/log_stream.rs): commit_segment, seal, seal_shared_file, lock | Add internal lifecycle reads/transitions. A fresh append cannot cross a closed frontier. Exact already-accepted retries remain idempotent. Initially retain current metadata locks to isolate the lifetime change from lock removal. |
| internal_files.rs: authorize_direct_log_segment_upload, complete_direct_log_segment_upload, reserve_proxy_log_upload, commit_log_segment | All upload admission and publication paths must consult the same close state under stream coordination. A request admitted earlier must settle before prepare or be prevented from changing the selected frontier. |
| [artifact_upload_grant.rs](../../crates/common/src/repositories/artifact_upload_grant.rs) and [blob_store.rs](../../crates/common/src/blob_store.rs) | Later milestone B supplies durable ownership/settlement for every physical PUT, including duplicates, staging/multipart work, and late completion. Milestone A preserves the existing active-grant publication exclusion without treating TTL as settlement; earlier admitted PUTs may start after closure. |
| Forward SQL migration for log_segment | Enforce immutable accepted segment identity/membership during unresolved close. The current create-only table comment is not an UPDATE guard. Retain exact provider-version reads. |
| [models.rs](../../crates/common/src/models.rs), internal repository projections | Map new internal state without accidentally exposing lifecycle tokens or proof material through the serialized public LogStream model. Update relevant column constants and SQLx metadata if required. |
| [storage_maintenance.rs](../../crates/common/src/repositories/storage_maintenance.rs), [maintenance.rs](../../crates/common/src/repositories/maintenance.rs), [file_evidence_retention.rs](../../crates/common/src/repositories/file_evidence_retention.rs), object deletion predicates and SQL guards | An unresolved close must veto metadata deletion, cleanup-state transition, segment deletion/replacement, and physical deletion, including parent-artifact cascades. Existing file-evidence owners cannot be fabricated for logs lacking authenticated origins. |
| Worker [log writer](../../crates/worker/src/runtime/log_writer.rs), workflow [outbox](../../crates/common/src/repositories/workflow_log_outbox.rs), sensor producers | Preserve each producer's existing identity and bounded ordered delivery. Honor the same server close protocol; do not assume every producer has worker-delivery fields. |

Upload settlement is a blocking design prerequisite for claiming the complete
physical-cleanup guarantee. A final manifest comparison alone detects changed
metadata but does not settle an old PUT. Reuse protected-file atomic/fencing
infrastructure only after its owner/lifetime requirements are satisfied for logs.
Preserve the counterexamples in the [seal claims review](object-log-seal-claims.md).

On cancellation, the unresolved close input remains protected. A bounded recovery
path may retry the same immutable input with fresh allowed authority, or explicitly
abandon only the temporary verification pin with a revision-qualified transition.
Abandonment does not prove physical settlement or erase other upload obligations.
Deadline expiry alone cannot remove its cleanup protection. An identical concurrent close may wait for the
existing result outside any database transaction; do not return a new permanent
conflict for the identical-retry contract supplied by PR #31.

Migration: required for durable lifecycle and guards. This is a protocol change,
not a mechanical relocation of the GET loop.

### Remove redundant metadata locking only afterward

LogStreamRepository::lock currently takes the per-version advisory key and then
the stream row FOR UPDATE. Audit all users, including workflow_log_outbox,
storage_maintenance, and create_with_backend, before selecting one coordination
primitive. Existing-row writes can potentially use the row lock alone; stream
creation needs unique insert/conflict handling for the missing-row case.

Parent artifact locks cannot simply be dropped: find_by_id_for_update participates
in version allocation, final metadata projection, and deletion ordering. Separate
stream-frontier writes from parent projection, then prove two independent versions
of one parent can publish safely. The final retention trigger still updates the
parent implicitly even after an explicit advisory lock is removed.

Keep shared_file append/seal on its existing safe path until an immutable
snapshot, open-descriptor fencing, committed-prefix visibility, and cross-process
flock protocol are established. A backend-specific exception is preferable to
shipping a partial proof under a shared abstraction.

## Slice 3: remove physical deletion from graph transactions

Two distinct mechanisms need changes; they must not be conflated.

### Artifact and legacy snapshot deletion

[PhysicalCleanup](../../crates/common/src/repositories/maintenance_admission.rs)
owns an exclusive graph transaction through physical deletion.
[artifact_cleanup.rs](../../crates/supervisor/src/artifact_cleanup.rs) calls
claim_expired_artifact_version, awaits artifact_transport.delete_file, then deletes
metadata and refreshes the parent. The claim helper currently selects/locks the
candidate; its name does not mean it commits a durable deletion reservation.
[Supervisor legacy snapshot cleanup](../../crates/supervisor/src/main.rs),
delete_expired_legacy_snapshots, explicitly notes that no durable filesystem
claim exists and waits for deletion without a timeout while retaining the gate.

Replace this only after a durable cleanup obligation can commit before I/O.
Bind exact version/path/provider identity and a generation, deny new references
and conflicting publication, retain required evidence, and record completion or
retry after restart. Paths must be fenced against reuse: a cancelled unlink that
later resumes must never delete replacement content. Existing cleanup_claimed,
deleting, staging guards, evidence obligations, and ledger state should be reused
where they actually cover that identity; do not introduce duplicate state blindly.

The target sequence is claim and commit, delete without a connection, then fresh
identity-checked finalization and parent refresh. After a committed claim, a
failed finalization does not roll back the claim or resurrect deleted bytes.
Recover unknown outcomes explicitly. Remove PhysicalCleanup only when every
production caller has moved to the new protocol; until then keep its cancellation
and confirmed-rollback protections intact.

### Pack release tree deletion

[PackRetentionRepository::cleanup_unreferenced](../../crates/common/src/repositories/pack_retention.rs)
holds SHARE on the whole pack_release table during remove_release_tree.
Add per-content publication/cleanup ownership, shared by this method and
[PackReleaseRepository::create_or_get_with_policy](../../crates/common/src/repositories/pack_release.rs).
The content identity includes validated storage location, not just a row ID.

[PackStorage::publish_release](../../crates/common/src/pack_registry/storage.rs)
can reuse an existing content-addressed directory before the DB transaction.
Therefore moving only the final INSERT behind a cleanup check is insufficient.
Keep immutable staging available until publication is confirmed; a publisher
must detect cleanup and retry installation safely rather than refer to a removed
directory. Audit worker-side pack materialization/removal separately because its
storage roots and readers may have a different ownership scope.

A proposed per-content lifecycle must cover publishing, referenced, deleting,
and absent/retry states with durable recovery, or provide an equally strong
filesystem quarantine protocol. Recursive deletion belongs outside transactions.
Preserve existing digest/path validation and shared-digest reference checks.

Migration: likely required for durable filesystem/content ownership. The exact
representation is a design gate; no safe one-line lock removal was identified.

## Slice 4: avoid strong locks on maintenance no-ops

### Cache statistics

In [CacheStorageRepository::refresh_statistics](../../crates/common/src/repositories/cache/storage.rs),
check whether a revision is due before requesting SHARE UPDATE EXCLUSIVE.
Preserve existing ownership/configuration validation. If due, acquire the bounded
parent lock and reread requested/completed revision and interval eligibility;
only acknowledge the revision actually analyzed. An idle preflight must not
consume a concurrent request. No new durable state is needed.

### Already-present native partitions

[PartitionRepository::ensure_day](../../crates/common/src/repositories/native_maintenance/partitions.rs)
acquires exclusive graph admission before native_partition_ensure_day. Moving
only the SQL function's already-present check would still retain that broad gate.
Introduce a separate validated read/preflight path before graph admission.
For an existing day, protect required relations with compatible read locks and
validate registry, physical bounds, ownership, and default-partition layout using
the existing SQL validation helpers. Do not return success from registry presence
alone. End the read transaction before any repair path requests exclusive graph
and stronger table locks; avoid lock upgrades behind earlier relation locks.

If missing or repair is required, use the existing bounded authoritative repair
transaction and recheck under its locks. Actual row movement/DDL remains atomic
and retains its necessary locks. The existing reconciler already discovers work;
measure how often an already-present fast path is exercised before ranking its
benefit above other changes.

Migration: a forward function change/helper may be needed; never edit applied
migration bytes as a side effect of this optimization.

## Slice 5: narrow sensor admission incrementally

Add explicit scoped operations to
[SensorAdmissionRepository](../../crates/common/src/repositories/sensor_admission.rs).
A viable candidate is hierarchical coordination:

1. Keep the existing global key exclusive for topology/runtime/bulk mutations
   whose complete sensor footprint cannot be fixed safely in advance.
2. A local mutation takes that key shared, then exclusive per-sensor admission
   for its complete affected set in stable order, before source/target row locks.
3. A sensor event/workload check takes global shared admission and shared admission
   for its exact sensor, then rechecks the current workload fence and membership.

This allows a change to sensor A to coexist with an event from sensor B while
retaining a safe fallback for unknown or changing footprints. It still requires
proof that every relevant mutation participates; a local writer must never
discover it needs global exclusive admission after taking shared/row locks.

Call sites include events, rules, triggers, runtimes, packs, cache request paths,
auth, inquiry responses/callbacks, and sensor_workload_admission. Some generic
request paths cannot name one sensor and must retain their broader fence. For
rule/trigger retargeting, include old and new sensors and revalidate the mapping
under admission. Cross-pack references and newly inserted dependencies are part
of footprint correctness. Do not blindly replace lock_mutations with hash(id).

Preparse immutable deployment/configuration input before admission, but preserve
fresh authorization and atomic activation. Introduce narrower operations first;
keep the old broad method for unsupported mutation categories until coverage
proves they can move.

Migration: may be unnecessary for advisory keys, but revision-based alternatives
would require durable revisions and conditional publication. The hierarchical
protocol is a candidate, not yet an approved complete access matrix.

## Slice 6: reduce repeated authority work; defer broad cache gate removal

In [runtime_authority.rs](../../crates/common/src/repositories/runtime_authority.rs),
lock_policy_with_deadline loads/validates a dependency before the visited-set
check on the next iteration. Deduplicate records and their validation within
that traversal. Verify every incoming pin against the memoized record and merge
all required deadline/approval fences; skipping a repeated edge's verification
would be incorrect. Start with local traversal state rather than a global cache
or a reusable authority object spanning requests/transactions. Retain ordered
initial reads and locked rereads of mutable authority.

For cache, first separate the purposes currently bundled into protect_transaction:
quota admission, pin lifecycle, namespace/generation mutation, and relation
lifetime. Do not remove its non-Read global gate in the first optimization.
COUNT-based namespace/partition caps, global/owner byte counters, refresh policy,
and retained-iteration caps need equivalent atomic accounting. A short reservation
design must account for committed plus outstanding usage, cancellation, rollback,
recovery, and trigger updates without double charging. Existing singleton byte
UPDATEs otherwise remain a bottleneck after removing the advisory lock.

Workflow session locks across broker publication are another independent later
slice: persist delivery ownership/checkpoints before releasing the lock, then
publish and reconcile idempotently outside it. Keep current claims and session
disposal until that delivery protocol replaces them. Neither cache quota redesign
nor workflow publication redesign is a prerequisite for slices 1–4.

## Suggested delivery order

1. Reconcile baseline with PR #31; add only missing phase measurements and the
   tests identified in the companion audit.
2. Ship transaction-local JIT/query-shape fixes and cache-statistics no-op
   avoidance as independently reviewed changes. Preserve existing lock protocols.
3. Specify and test close-input immutability, upload settlement, and cleanup veto;
   then split object-log seal verification. Remove redundant locks afterward.
4. Introduce durable physical-cleanup ownership and migrate each caller before
   removing its transaction fence. Partition fast-path work can proceed separately.
5. Narrow sensor admission with a complete participant matrix; optimize authority
   traversal locally. Reconsider cache/workflow gate redesign using measured waits.

The acceptance evidence is successful recovery and correct outcomes under forced
interleavings, plus lower lock/transaction tails under the same workload. At the original document-only analysis stage, no application tests were run.
Source/link and whitespace checks were its validation; first-checkpoint results
are recorded above. The companion test audit remains a predictive inventory for
later milestones, rather than evidence those milestones have passed.

### Object-log close storage guard checkpoint

Dependency integration: normal task-branch merge `4ac6ea43` integrates PR31
`c067e88573529466f40849a7b58325a8e4aaa704`; PR31 itself remains open and unchanged.
Only the existing `fix/reduce-database-lock-scopes` worktree is modified. No push,
PR update, production operation or checkout creation is part of this checkpoint.

The coherent current protocol and explicit incomplete boundary are in
[object-log durable close protocol](object-log-seal-claims.md). This supersedes the
older dedicated claim-table/token candidate: temporary pin and permanent closure
are separate, abandonment never reopens, and stream identity survives until
allowed version teardown. No production prepare/split is enabled. Selected-close
publication fails closed pending the typed verified-result/fresh-authority owner.
The existing production seal transaction still holds locks during storage I/O.

Prior corrected milestone3 PG16/18 repository 13 / supervisor 3 / storage 17,
schema prepare and Clippy results remain historical evidence, as does Astra's P2
fix confirmation. Earlier timing/CommitUnknown failures remain unclassified.
The Docker relay fixture omission and 58 base-manifest policy-script failures
remain gaps; this storage checkpoint cannot turn them into passes.
