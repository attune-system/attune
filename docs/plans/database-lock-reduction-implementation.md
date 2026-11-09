> Current object-close checkpoint: the production object seal route now selects
> durable input, awaits prepare commit, verifies outside database ownership, and
> freshly authorizes atomic publication. Migration 006 provides deferred shape
> constraints; bounded object housekeeping abandons expired matching revisions.
> This is work in progress. Earlier statements below describing publication as
> disabled or the route as unsplit are historical checkpoint evidence, superseded
> by [the current protocol](object-log-seal-claims.md). New acceptance tests and
> source-specific validation must pass before lock-removal/certification claims.
> No full PUT settlement or filesystem fence removal is claimed.

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

#### Storage guard checkpoint validation

Status: **work in progress, incomplete foundation**, not ready for route activation
or merge. Functional source checkpoint is
`ec4ed280d0b87051e51a9cfe0ee1c5d14c4c9545`; this validation record changes prose
only. Integration checkpoint is `4ac6ea43b9a52d8c70b345de08357ff8c482874a`.
No source changes occurred during the final validation sequence. The exact patch
is `/tmp/attune-close-validation-nu_13_r5/tested-source.patch`; commands, exit
codes, timings and resource removal evidence are retained in
`/tmp/attune-close-validation-state.json`, with logs alongside the patch.

Builds and tests ran sequentially. Disposable PG16/18 servers were bound to
random loopback ports; repository/API fixtures used run-owned, migration-hashed
physical clones. The final-source clones applied the forward migration.

```bash
cargo test -p attune-common --test storage_maintenance_repository_tests --no-run --message-format=json
# Run the resulting executable, cwd crates/common, on each owned PG16/18 URL:
ATTUNE_TEST_RUN_ID=closefinal16 ATTUNE__DATABASE__URL=<owned-pg16-url> <storage-test-binary> --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=closefinal18 ATTUNE__DATABASE__URL=<owned-pg18-url> <storage-test-binary> --nocapture --test-threads=4
DATABASE_URL=<owned-pg18-url> SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets
cargo clippy -p attune-common -p attune-api --all-targets -- -D warnings
cargo fmt --all -- --check
cargo test -p attune-api --lib --no-run --message-format=json
# Run the resulting API libtest executable, cwd crates/api, on each owned URL:
ATTUNE_TEST_RUN_ID=closeapi16 ATTUNE__DATABASE__URL=<owned-pg16-url> <api-test-binary> internal_files::tests:: --nocapture --test-threads=4
ATTUNE_TEST_RUN_ID=closeapi18 ATTUNE__DATABASE__URL=<owned-pg18-url> <api-test-binary> internal_files::tests:: --nocapture --test-threads=4
git diff --check
```

Results: storage repository **35/35** on PG16 (71.52s) and PG18 (68.18s),
including four new lifecycle tests and existing cleanup/admission/shared-file
regressions. API selection **20/20**, zero ignored, on PG16 (52.42s) and PG18
(32.76s); 479 other API libtests were filtered, not counted as passes. These
exercise the existing concurrent size-one-pool object append/seal, direct upload
completion and shared-file seal paths, not the still-unimplemented split route.
Schema prepare passed (225.91s; no macro queries/offline metadata changes),
common/API all-target Clippy passed with warnings denied (203.45s), and formatting
and whitespace passed. The API test build took 777.94s; it did not overlap tests.
No new public API/generated contract changed in this checkpoint.

Intermediate failure evidence remains: the first local test build had an incorrect
module path; the first PG16 run passed 33/35 and failed two new fixture assertions.
One issued a raw deletion claim without required exclusive admission; the other
mistook a valid identical resume retry for a stale conflicting request. Corrected
fixtures retain the guard and retry behavior. Their final reruns passed. This does
not classify the older timing/CommitUnknown failures.

Cleanup: both exact owned containers (`c52ed304c29b`, `565a6498429b`) and their
anonymous volumes were removed. Each server had zero test-clone databases before
removal. Both temporary psql shims were removed; logs and drivers remain as local
evidence. No external PR or production resource was modified.

Remaining gates are the protocol completion items in
[object-log durable close protocol](object-log-seal-claims.md): typed actual
producer/proof projection, trusted full-byte verifier, fresh-authority private
finalizer with atomic SQL publication consistency, bounded recovery, and the
corresponding full API/provider-backed race/security/independent-retention tests.
Cross-session timestamp rendering in the current JSONB segment projection also
needs canonicalization review and a cross-TimeZone retry test before activation;
a differently rendered creation timestamp currently conflicts safely rather than
reconciling. No size-one-pool connection-free verification claim is made. The
retained pre-admitted PUT-start/settlement and filesystem-retirement limitations,
Docker relay omission, and 58 base-manifest policy-script gaps remain unresolved.
Astra review has not been requested by this implementation agent.


#### Astra storage-guard review and expiry corrections

Astra's read-only review of `ec4ed280` against dependency merge `4ac6ea43`,
at documentation head `e5c0b37f`, found two actionable P2 defects in the new
grant guard: maintenance expiry acquired grant then stream while API renewal
acquired stream then grant; and an issued grant past settlement could be selected
into a close but its later expiry poisoned the entire maintenance batch. Neither
was inherited from PR31. These source-derived interleavings were absent from
the previously passing focused suites. This review was not merge approval.

Forward migration `20261109000004_object_log_grant_expiry.sql` admits exactly
identity-preserving `issued → expired` transitions with the original settlement
deadline elapsed before any stream lock. Other grant writes retain the close
guard. Expiry retires authority; it does not assert physical settlement or remove
the permanent close state/pin. Migration `003` remains byte-identical.

New regression coverage pauses real `mark_expired` execution on a test-only
advisory barrier after the grant row is locked, holds the stream in the renewal
transaction, observes renewal waiting for the grant, then releases the barrier.
Both owners must finish successfully. Another case checks both closing and
abandoned streams, unrelated grants in the same batch, identity-changing expiry
rejection, continued append/renewal prohibition, and unchanged close identity.
Correction validation uses base `e5c0b37f` plus the saved staged patch at
`/tmp/attune-expiry-validation-8_y4k87z/tested-source.patch` and its hash sidecar.
Source files stayed unchanged throughout the final build and validation.

The final storage target passed **37/37 on PG16** (96.26s). The first PG18 run
passed 36/37, including both new expiry regressions; the existing
`bounded_admission::combined_cleanup_rolls_back_version_triggers_when_parent_cleanup_fails`
failed in its `claimed_pending` setup at line 465 with `DeadlineDeferred`, SQLSTATE
57014, and a 250ms statement timeout reported in `guard_object_log_close_version`.
The cause remains **unclassified**, not baseline-proven. No test, budget or
production timeout was changed to accommodate it.

A fresh owned PG18 run passed that exact case alone (1 passed, 36 filtered;
42.50s including template setup), then the whole target **37/37** with four workers
(33.86s). These passes do not erase the failed run. First-run evidence is in
`/tmp/attune-expiry-validation-state.json`; follow-up commands, timing and logs
are in `/tmp/attune-expiry-recheck-recheck-state.json`, logs
`/tmp/attune-expiry-recheck-recheck-pq4dk7l_`.

Workspace all-target schema preparation on PG18 passed (138.06s, no macro queries
or offline metadata changes). Formatting and whitespace passed. Common all-target
Clippy passed with warnings denied (138.71s). All three exact owned containers
and anonymous volumes were removed; successful PG16 and fresh PG18 runs checked
zero test clones before removal. The failed PG18 run removed its owned server
and volume without a separate post-test zero-clone query. All temporary psql
shims were removed. No production or unowned resource was changed.

This does not complete the split sealing route, broader consumer execution,
required final-head CI, or any previously disclosed validation gaps. Astra has
not yet re-reviewed the expiry correction.

#### Storage-only verifier continuation (incomplete requested milestone)

Starting head: `b130aad5f38631c0a75129276d0e67804d7acdd4`; sole writer used
only the existing task checkout. Checkpoint
`307cd76421251a6de49ea81712c8aa10e6286444` adds the storage-only verifier,
private byte-result construction, adversarial unit cases and forward migration
005 with a nonempty cross-TimeZone close retry case. The subsequent route edit
uses that verifier in the existing ObjectSegments branch. It removes duplicated
frontier logic, checks segment stream identity and size/digest independently of
provider adapters, consumes successful EOF even at the expected length, computes
the concatenated SHA256, and completes encrypted frame/manifest verification.
Read-route metrics and shared-file snapshot/flock behavior are unchanged.

**The requested functional prepare/verify/finalize milestone is NOT complete.**
The production caller still owns its transaction during GET/hash/frame work;
there is no connection-free route guarantee. No publication transition was
activated, no generic SQL bypass added, and no old fence removed. The result is
byte evidence only, not a committed-close-bound publication capability. Typed
actual producer projection, fresh final owner, atomic SQL publication consistency,
unknown prepare/final COMMIT reconciliation and bounded recovery are unfinished.

Concrete source gaps discovered while tracing fresh authorization:
`authorize_version_mutation` has no sensor-generation branch; sensor current-fence
validation occurs in authentication middleware before the handler. A generation
can therefore change after that check and before a hypothetical split finalizer.
`require_native_log` validates selection and does an unlocked workflow-existence
read; this alone supplies neither a native producer/attempt projection nor the
required source fence after target waits. Reusing old proof JSON or manufacturing
a worker permit would not close either gap. They require actual source locking,
after-wait freshness and negative race coverage before final publication is safe.
These are implementation gaps, not a claim that new retention FKs, formats or
physical PUT settlement are prerequisites. Existing selected-close vetoes remain
coherent and fail closed.

Migration 005 sets only the snapshot SQL function's TimeZone option to UTC.
Applied migration bytes and old immutable selected input/hash values are untouched.
New selections retry identically across session TimeZones. Historical selected
non-UTC timestamps can still conflict safely; automatic conversion/resumption
of those identities is not supplied. Never clear permanent closure to work around
that conflict.

Validation provenance: tested source is checkpoint 307cd764 plus the saved route
and documentation patch at
`/tmp/attune-verifier-validation-cpvfp9sf/tested-source.patch` (SHA256 sidecar and
head file adjacent). Storage binaries were built from that snapshot. During the
storage run, a verifier/API-only correction preserved the old forbidden response
for authenticated final-manifest mismatches and added explicit error-variant
assertions. That later patch is `final-source.patch` with a SHA256 sidecar in the
same directory; schema/lint/API builds consume it. Storage repository tests do
not call this verifier, and their Rust/SQL/fixture source remained unchanged,
so those results are reused for the final verifier-only correction. The focused command `cargo test -p attune-common --lib object_log_verification::tests -- --test-threads=4`
passed **4/4**, 1287 filtered, zero ignored; log
`/tmp/attune-verifier-unit.log`. Initial compilation failed on an unavailable hex
crate, ambiguous test conversion and a test closure lifetime; corrected by sharing
the existing digest codec and using explicit test lifetimes. Those compilation
failures are not quiet reruns or classifications of earlier timing failures.

PG16/18 storage/API, schema and lint validation results and owned-resource cleanup
are recorded below after the sequential runner finishes. Earlier disclosed
CommitUnknown/timing failures, Docker relay packaging omission, base-manifest
58 dependency-policy failures, successful-COMMIT/lost-ACK gap, mixed eligibility
scheduling gap, broader consumers, final-head CI and Astra review remain open.
Full physical PUT settlement, forgotten multipart staging, post-retention key
recreation and native filesystem retirement remain explicitly outside A.

Continuation validation results: owned host-fallback PG16/18 storage target
**38/38 each**, zero ignored/filtered (97.17s / 67.21s). API
`internal_files::tests::` **20/20 each**, zero ignored and 479 filtered per run
(57.38s / 33.09s). The existing API one-connection concurrency test remains an
old-transaction-path test, not proof of a paused GET releasing admission/pool.
The added storage test proves selection/retry under Honolulu and Kathmandu session
TimeZones with an actual accepted segment. No budgets or old expectations changed.

Commands and timings are recorded exactly in
`/tmp/attune-verifier-validation-state.json`, driver
`/tmp/attune-verifier-validation.py`, logs
`/tmp/attune-verifier-validation-cpvfp9sf`. Builds/tests ran sequentially. Both
owned databases were bootstrapped with `make db-test-setup`; storage/API executables
ran from their respective crate directories with unique run IDs (`verify16`,
`verify18`, `verifyapi16`, `verifyapi18`) and `--nocapture --test-threads=4`.
`DATABASE_URL=<owned-pg18> SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets`
passed (227.03s, no queries/offline metadata changes).
`cargo clippy -p attune-common -p attune-api --all-targets -- -D warnings`
passed (199.37s). API test build passed (740.80s); storage build passed (290.18s).
The first formatting/diff checks passed before the small verifier-only error
correction; final formatting/whitespace is repeated at the final checkpoint.
These passing runs do not classify or erase any earlier reported failures.

Cleanup is confirmed in the JSON: zero test clones after storage and again after
API on each owned server. Containers `929b9710afda` (PG16) and `cd605d62cd15`
(PG18), their anonymous volumes, and both temporary psql shims were removed.
No production/unowned resource, original dirty checkout, PR, remote branch or
external message was changed. No Astra review was requested by this writer.
Final-head CI and the full functional milestone remain pending.

Final verifier rerun after the manifest-error correction:
`cargo test -p attune-common --lib object_log_verification::tests -- --test-threads=4`
passed **4/4**, zero ignored, 1287 filtered, build 3m15s; log
`/tmp/attune-verifier-final-unit.log`. It includes explicit typed rejection of
manifest size/hash mismatches so the API preserves its prior forbidden response.
Final code diff and SHA256 are `final-code.patch` / `final-code.patch.sha256`,
with storage/API executable SHA256 in `executable-sha256.json`, under the same
validation log directory. Only this documentation changed after the final rerun.

## Source fencing prerequisite continuation (work in progress)

The staged `internal_files::log_source` API acquires shared graph admission before
source/target protection, returns a transaction-owning `Source`, and derives a
serializable `HistoricalBinding` only after fresh source, target/origin, bounds
and authenticated v1 proof checks. Final callers open a new transaction, reacquire
Source, authorize targets, lock/reload version/stream, and call `bind` with the old
binding. Recheck immediately before effects/commit after subsequent waits.
`HistoricalBinding::{producer,proof}` expose server projections to durable selection;
the binding and a successful prepare never authorize publication.

Native projection locks the actual workflow before selection/authority and
execution/attempt, validates selection again after execution locking, and permits
terminal native completion. It does not strengthen the generic require_native_log
used by the old long route. Sensor sources use the existing current-fence workload
lock and recheck the current assignment, lease clock and generation metadata after
waits. Recorded sources preserve existing source/disclosure ordering, then protect
immutable context/permit rows before artifact/version/stream. Durable projection
uses server BIGINT context/permit IDs, not their bearer UUIDs. Process identity
also includes delivery worker/instance. Renewed JWTs do not change identity.
Delegated execution credentials retain their current claim/attempt validation;
legacy workers have no synthetic instance or permit. No runtime FK is added.

The old route's source/target code was extracted without activating the staged
module or adding sensor/native locks across GET. Its module-scoped dead-code
allowance is deliberate staging, as permitted by AGENTS, rather than a fake
production caller. Selected-close SQL publication remains fail-closed.

Tests added: API `staged_log_source_fresh_transactions_and_revocation`, common
`staged_native_log_source_roundtrip_terminal_and_disappearance`,
`staged_log_sensor_fresh_transaction_and_after_wait_expiry`, and
`staged_sensor_source_checks_lease_after_queued_target_lock`. These assert fresh
transactions, credential renewal/expiry, revocation, terminal native identity,
source disappearance, sensor instance/generation changes and lease checks after
an observed pg_blocking_pids target wait, plus independent NOWAIT lock release.
Exact validation results will follow; test existence is not a passing result.

Remaining next-stage work: durable prepare/finalize orchestration, revision/input/
deadline ownership, current selected-state/bounds checks after final waits,
atomic publication SQL with encrypted evidence/metadata/Ready, commit-unknown
reconciliation, abandon/recovery, route split, paused-verification pool/graph
progress and provider races, and independent review. Existing permanent closure,
expiry-retirement exception in 004 and UTC snapshot setting in 005 are retained.
Broader consumers/final CI, earlier unclassified timing/CommitUnknown failures,
Docker relay omission, 58 base-manifest policy failures and real successful-COMMIT
lost-ACK coverage remain open. PUT settlement, delayed preadmitted PUT, forgotten
multipart and post-retention key recreation/path retirement remain outside A.

### Source prerequisite: completed helper boundary and validation

The finite source prerequisite is implemented; the overall PR remains work in
progress and selected-close publication remains disabled. The Rust library
exports `internal_files::log_source` with hidden documentation so the existing
integration fixtures exercise the actual helper. `Source` and `HistoricalBinding`
are opaque: no public field/constructor exposes a FileMutationProof or permits a
historical binding to stand in for current authorization. This adds no HTTP route.
The module's narrow dead-code allowance remains staging for production integration,
not a fake caller. The canonical production route retains its old long transaction.

The owner API is `acquire(state, transaction, authenticated_user, version)` ->
`Source`; `authorize_target`; `transaction` for ordered target locks/reloads;
`bind(state, user, locked_version, locked_stream, headers, expected_history)`;
`recheck`; and consuming `commit`. Acquire locks shared graph before sources,
including legacy callers. Cache PinMutation may precede graph admission. Commit
rechecks sources before commit and preserves the existing pure effect check after
commit waits; any commit error must be reconciled, never assumed rolled back.
Only the immutable historical value crosses phases; final phases must call
acquire on a NEW transaction and compare freshly authenticated bindings.

Sensor source protection now also locks existing schema/log-generation rows after
assignment/generation/metadata validation. Identity retains optional actual metadata
revision, schema ID/digest and log-generation ID/digest; a bound log generation
must match the current generation. Recheck projections and metadata after waits,
then check the lease clock. Absence remains absence for legacy evidence; it cannot
silently equal a previously recorded binding. No execution/admission/event FK,
missing source recreation, fabricated evidence owner, or workflow/sensor worker
permit was added. Native source protection uses workflow -> selection/authority
-> execution/attempt, permitting terminal native completion. Recorded paths retain
existing authority/origin order, then context/permit protection before targets.

Validation source: runtime checkpoint `499ce06a5fbafbea58f14abdd09b10642f4103a9`
plus the saved source/test patch under `.scratch/source-validation9/tested-source.patch`
(SHA256 `660299fb3d7ba634ed1ba133df8b0be6eb01d2acfa7eea2d04fb62dccb8dd030`).
That patch adds hidden Rust visibility, real integration coverage and fixture/test
corrections; subsequent edits are this prose record only. Exact executable commands,
exit codes, container IDs and removal results are in
`.scratch/source-validation9/state.json`; earlier evidence is in the corresponding
`source-validation5` and `source-validation8` directories.

All selected executables ran sequentially on owned stock PostgreSQL 16/18, with
four libtest threads and no overlapping compilation. Host bootstrap used explicit
owned `make db-test-setup TEST_DB_ADMIN_URL=... TEST_DB_URL=...` URLs and temporary
psql shims forwarding solely to each exact owned container. No default cluster
was used. Focused commands and their scope are also in the testing guide.

| Selected target/filter | PG16 | PG18 |
| --- | --- | --- |
| API lib `internal_files::tests::` | 21/21 | 21/21 |
| common `sensor_workload_repository_tests`, `staged_` | 2/2 | 2/2 |
| API `sensor_log_tests` (entire target) | 3/3 | 3/3 |
| common `execution_admission_tests`, `staged_` | 6/6 | 6/6 |
| recorded integration `terminal_log_delivery_keeps_caps` | 1/1 | 1/1 |
| recorded integration `encrypted_terminal_logs_require` | 1/1 | 1/1 |
| recorded integration `worker_file_approval_expiry_after_source` | 1/1 | 1/1 |

Total selected coverage is **35/35 per version**, zero ignored. The native filter
includes four existing staged-completion cases as well as two new source cases.
PG16's first 34 final passes are from run8; the corrected expiry case passed in
run9 (96.72s). PG18's complete set passed in run9; expiry took 70.51s. The later
fixture change affects only the expiry function, so the other PG16 results remain
valid. The API unit binary predates only Rust visibility changes; its source
algorithm and tests are unchanged, while the public-helper integration targets
compile and exercise current source. No old test budget/assertion was weakened.

The recorded integration tests directly exercise actual process and delivery
Source owners through fresh transactions, numeric context/permit projections,
instance mismatch, admission revocation, encrypted proof roundtrip and rejection
of a different authenticated manifest encoding. Sensor API tests exercise actual
admission/log bindings, renewed credential deadlines, expiry and lifecycle release.
Native repository tests preserve terminal completion and deny missing workflow
state; actual worker attempt replacement yields distinct identity and missing
attempt denies. Sensor repository tests cover generation/instance replacement,
independent NOWAIT source/schema lock release, and a proven queued target wait
followed by lease expiry and specifically PermissionDenied.

`cargo clippy -p attune-common -p attune-api --all-targets -- -D warnings`
passed on the final source. Owned PG18 `DATABASE_URL=... SQLX_OFFLINE=false cargo
sqlx prepare --workspace -- --all-targets` passed, checking every workspace target;
SQLx reported no macro queries found and changed no offline metadata. Formatting
and whitespace are checked again after this prose update. No migration or public
HTTP contract changed, so no forward migration or generated client was needed.

Failures/corrections are retained, not erased by reruns. Initial compile attempts
caught stale dependency metadata and fixture mistakes (lease fence conversion,
fixture destructuring and an unused TTL parameter); corrected checks passed.
Three superseded optimized builds were explicitly cancelled before any DB test.
Run5 API passed 21/21, but its sensor fixture expected a schema row after only
begin_process; run8 admits the schema through the actual producer lifecycle before
asserting the lock, preserving the assertion. Run8's expiry case failed during
setup: the original 30-second deadline began before cold schema replay, which
lasted 37.92s. The fixture now creates/cleans an owned warm-up context before
starting that same 30-second clock. Its 15/40-second observation budgets, real
blocking/DB-clock barrier and all publication assertions remain unchanged. This
classifies that new setup failure, not earlier unclassified timing failures.

All failed and successful runs removed their exact owned containers and anonymous
volumes. Successful pipelines checked zero per-test clones before teardown; cached
owned templates were removed with their servers. All temporary psql shims are
removed, and no owned DB/test/compiler process remains. No push, PR edit, deployment,
production change or change to the original dirty checkout was performed.

Remaining implementation is the next durable prepare/finalize/route-split task:
selected input/revision/deadline ownership, binding byte verification to that input,
atomic final evidence/metadata/seal/Ready/published consistency, commit-unknown
reconciliation and bounded recovery, followed by review and paused-I/O pool/graph
and provider races. Existing source helpers no longer constitute the discovered
sensor/native prerequisite gaps; their production integration still must happen.
Required final CI, broader consumer runtime coverage, real successful-COMMIT/lost-ACK
coverage, older unclassified timing/CommitUnknown failures, Docker relay packaging
and the 58 unchanged base-manifest policy failures remain disclosed. Complete PUT
settlement, delayed preadmitted PUT prevention, forgotten multipart obligations,
post-retention recreation and filesystem retirement remain outside milestone A.

## Object-seal route integration checkpoint (2026-11-09)

Actual object-route integration is implemented, not just verifier/source staging.
Checkpoint `05c4bff913eb9fb3da776262c2749b0cb23f13b0` introduces prepare/verify/fresh
publication and bounded housekeeping recovery; `2f88d22f829d475c58e21c002b40a1fffdc83c4d`
adds publication entry guards, deadline fencing and route/recovery race coverage.
`025e7c11` makes expiry discovery indexable; `b15f537f` binds every recorded final
evidence attribute. Follow-up operational-status guards and synthetic consumer
fixtures preserve workflow delivery/error handling. No source authority is
manufactured by the fixture helper: it explicitly tests SQL shape, not auth or bytes.

The decisive actual-route test pauses provider GET with a size-one pool. An unrelated
query and shared/exclusive graph transactions finish before release. Plain and
encrypted success, authenticated incomplete-manifest rejection, revocation during
GET, changed truncation, refreshed-credential published retry without GET, identical
concurrent seals and shared-file behavior passed. The object route uses an owned
request task, pool-accounted disposable phase sockets, local SQL limits and bounded
phase cleanup. Unknown commit acknowledgements receive one fresh authorized
reconciliation; acknowledged commit followed by pure freshness failure stays distinct.

Validation source attribution and commands:

- API source `2f88d22f` (common build metadata also `2f88d22f`):
  `cargo test -p attune-api --lib --no-run --message-format=json`, then executable
  filter `routes::internal_files::tests:: --test-threads=4`: **25/25 each PG16/18**.
  Later common changes are expiry discovery/evidence shape, not the GET connection
  ownership path. The subsequent Source lint fix removes compiler-inserted redundant
  borrows only. These results do not certify recorded-delivery constraint 009 or
  every authority producer through the route.
- Common source `b15f537f`: build storage integration and common lib test targets
  before execution. Verifier filter `object_log_verification::tests::`: **4/4 each**.
  Full storage target from `crates/common`, four workers: **PG16 39/40; PG18 40/40**.
  All nine `object_close::` cases passed on each, including explicit expiry recovery,
  stale owner fencing and empty verified atomic publication.
- The first storage invocation used workspace-root cwd and failed all nine filtered
  cases before setup (`./../../config.test.yaml` absent). Corrected cwd runs are
  separate evidence; no test/source/timeout was weakened to correct invocation.
- PG16 full-storage failure:
  `bounded_admission::bounded_pending_claim_and_delete_defer_without_releasing_evidence`
  expected Progress, got DeadlineDeferred/57014 at its existing 250 ms budget, inside
  `guard_object_log_close_version() line 2 at statement block`. **Unclassified**;
  no baseline equivalence or quiet rerun is claimed. Earlier timing/CommitUnknown
  failures remain unclassified too.
- Consumer fixture follow-up on `b15f537f` plus its saved source diff: common artifact
  filter `log` **8/8**, artifact wire types **1/1**, executor
  `workflow::log::tests::` **30/30**, each PG16/18, four workers, zero ignored.
  Workflow cases cover lost seal response, append/seal races, failures, shutdown,
  terminal completion and retained root lineage. They use an explicit synthetic
  transport, not the actual Source authorization path. Operational-status SQL
  probes on both servers also accepted status updates and rejected identity changes.
- `cargo clippy -p attune-common -p attune-api -p attune-executor -p attune-supervisor
  --all-targets -- -D warnings` passed after removing needless borrows. Formatting
  and diff checks passed. Workspace SQLx validation is recorded below when complete.

Evidence logs: `/tmp/attune-split-validation-fc_9jdgm`; commands/results are in
`integration-results.json`, `storage-results.json`, and `consumer-results.json`.
The owned server inspection found zero remaining per-test database clones on each
server before any janitor/container removal. Exact owned servers/volumes/shims are
removed at handoff; logs remain. No original dirty checkout, deployment, push,
external message, PR edit or merge is part of this checkpoint.

Status remains **work in progress**. Unknown prepare/final commit fault injection,
real successful COMMIT/lost acknowledgement, full finalizer-wait authority matrix,
actual-route native/sensor/recorded producer coverage, mixed eligibility-error
scheduling and broader final CI remain gaps. Earlier verifier/source evidence is
historical, not certification of these new paths. Docker relay-fixture and 58 base
manifest dependency-policy failures remain separate gaps. Scope A still permits
delayed pre-admitted PUT starts; full settlement, forgotten staging, post-retention
recreation and filesystem path retirement are separate. Root Astra review and CI
are required before merge readiness.

Final follow-up code source: base `b15f537fbe57fed8cbcca52a01533fd1d4482cfb` plus `final-source.diff` in the evidence directory, SHA256 `14ff15130bd475d8027d684157673d1ddd556e3501dbdb9266477a0fed46c757`. This captures Source borrow-only lint fixes, synthetic consumer fixtures and forward operational-status guards 010/011. Documentation-only changes after checks preserve code evidence.

Final schema validation: `DATABASE_URL=<owned PG18> SQLX_OFFLINE=false cargo sqlx prepare --workspace -- --all-targets` and the corresponding `--check` command both passed, with no `.sqlx` changes and no workspace all-target warnings. Final `cargo fmt --all -- --check` and `git diff --check` passed. All runtime/build checks were sequential with timing-sensitive tests. The two exact owned PostgreSQL containers and their anonymous volumes were removed; both invocation-owned psql shims were deleted. `cleanup.json` in the evidence directory records their exact identities. Zero per-test clones existed before removal. No production infrastructure was used.


## Integrated lifecycle Astra corrections (work in progress)

Astra reviewed pinned `43e22481..1f2e12ee` read-only and found four actionable
issues: P1 NULL-state plaintext digest replacement, P2 stream/evidence INSERT
bypasses, P2 standalone published membership removal, and P2 transient target
contention being mapped to terminal authorization failure. The review did not
execute DB tests and did not give merge approval. The decisive pool-one split
remains present; those findings concern its safety and failure contracts.

Forward migration `20261109000012_object_log_publication_hardening.sql` makes
selected lifecycle/body predicates NULL-safe, requires the result-change exception
to be explicitly true, initializes object streams only open on pending versions,
and validates unselected final state/evidence at deferred checks. Standalone
published segment/final-evidence removal is rejected while its version survives;
whole-version teardown cascades remain permitted. No earlier migration is edited.

API file-path error mapping retains ApiError status rather than forcing403.
Typed transient SQL errors (including statement/lock timeout, deadlock, connection
loss and server unavailability) become503 through source acquisition, target
checks and freshness; genuine Forbidden remains403. Commit uncertainty handling
and postcommit freshness rejection are not combined into one error class.

New tests exercise nonempty publication, NULL and two-update replacement attempts,
reverse Ready/stream creation, presealed INSERT, evidence-before-selection,
required evidence deletion and legitimate version cascades. The HTTP seal route deliberately denies process credentials before the handler;
that policy must remain403. The revised fixture separately blocks the process
Source target helper, then the real route with its permitted worker-delivery
credential. Both database contention paths must remain503 without publishing.
The object route's initial discovery now performs only reads/scope/path checks;
source/target authorization occurs inside the existing bounded phase. Shared-file
authorization remains on its existing path.

Checkpoint evidence: PG16 close lifecycle **12/12** and selected API/error cases
**31/31**, four workers, zero ignored (31 and473 other cases filtered). This
includes the nonempty NULL/deletion, INSERT and evidence-cascade regressions.
PG18, revised blocked-target test, schema/lint and final cleanup remain pending.
All earlier unclassified failures and acceptance gaps remain open.


Correction validation provenance: base `1f2e12ee` plus saved patches/hashes in
`/tmp/attune-hardening-validation-eeraujr0`,
`/tmp/attune-hardening-recheck-5746kv94`, and current run
`/tmp/attune-hardening-final-h2yet2de`. Commands/results are respectively in
`/tmp/attune-hardening-validation-state.json`,
`/tmp/attune-hardening-recheck-state.json`, and
`/tmp/attune-hardening-final-state.json`.

Intermediate failures remain recorded: initial test compilation used an unsupported
LowerHex implementation and was corrected with byte-wise encoding. First PG16
close run passed11/12; the reverse-order fixture constructed an invalid generic
Ready row without provider metadata, before reaching the new initializer. Its
fixture now supplies existing required provider metadata; no guard or assertion
was removed. Next close run passed12/12 and API/error31/31. Its recorded-route test
failed because a process credential is intentionally not a declared consumer of
that HTTP endpoint; observation of a target wait timed out after the route's403.
The revised test preserves that403 policy, exercises the process Source directly,
and uses the real worker-delivery token for the blocked HTTP route. This test
correction is currently compiling, not yet reported as passing.

The parent-version initialization check uses a per-version FOR SHARE to serialize
with concurrent generic Ready updates, confined to short stream initialization,
never provider I/O. Its final SQL predates all regression test processes. Selected
TestDatabase::create fixtures load migrations from disk; no migration-history
macro fixture is selected. API binaries were reused only while API Rust and their
fixtures remained unchanged; the revised recorded fixture is being rebuilt.
Final schema/lint checks compile the final source. No API runtime result is reused
for the revised fixture. Earlier generic lost-ACK and authority/finalizer wait gaps
remain unverified; these corrections do not supply that missing evidence.

The two failed runs' owned containers/volumes/shims were removed. The current
run's two disposable servers remain owned and active, with unconditional driver
cleanup; do not report them removed until the driver records it. No production,
unowned checkout, deployed service or external data was changed.
