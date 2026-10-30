# Artifact upload grant lookup index

The supervisor's storage cleanup checks all grants for an artifact version. The
existing body and segment uniqueness indexes have mutually exclusive predicates;
neither covers this lookup without a segment predicate. The new nonunique btree
index also supports the grant foreign key's all-state cascade lookup. Production
queries and upload settlement rules are unchanged.

## Local plan comparison

PostgreSQL 18.6, stock `postgres:18-alpine`, private disposable container with
2 CPUs and 1 GiB memory. No shared statistics, extensions, or running services
were changed. This synthetic fixture has 1,000 populated artifacts, 50,000
artifact versions, 49,960 ready bodies, and 40 pending bodies aged two days.
There are 25,000 object-segment streams sharing one terminal execution. This
keeps unrelated execution scans small; it is not a production execution mix.
Odd-numbered versions have one body grant; even-numbered versions have three
segment grants. The 100,000 grants comprise 25,000 each of active issued,
settled issued, completed, and expired grants. Both grant kinds cover every
state. Ten aged versions have active grants, so cleanup returns 30 candidates.

The probe extracted SQL literals from
`crates/common/src/repositories/storage_maintenance.rs`, expanded its existing
format placeholders, and used a one-day cutoff and limit 100. It retained the
complete `abandoned_pending` query, including log/execution checks and
`file_evidence_version_eligible`. The single-row write probes used an aged body
whose issued grant had settled. Each write and its setup ran inside an explicit
transaction followed by `ROLLBACK`, including every `EXPLAIN ANALYZE`.

Three consecutive `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` samples per statement
per stage produced these execution-time medians. Buffers are root-plan shared
hits from the third sample; all reported root shared reads were zero.

| Statement | Before ms | After ms | Shared hits before / after |
| --- | ---: | ---: | ---: |
| `abandoned_pending` | 1219.278 | 22.324 | 172020 / 533 |
| `claim_pending` update | 57.689 | 25.553 | 6208 / 496 |
| `mark_deleting_from_pending` update | 122.157 | 31.385 | 6218 / 506 |
| `delete_cleanup_claimed` | 135.350 | 74.197 | 6151 / 439 |
| `delete_deleting` | 157.924 | 103.717 | 6277 / 565 |
| All-state grant lookup by artifact version | 52.451 | 1.605 | 5715 / 6 |

The candidate read kept its outer bitmap heap scan and sort/limit. Its correlated
grant node changed from `Seq Scan`, 40 loops and 171,567 shared hits, to
`Index Scan` on `idx_artifact_upload_grant_artifact_version`, 40 loops and 120
shared hits. Each single-row write's grant node changed from one sequential
scan with 5,715 shared hits to one index scan with three shared hits.

Candidate rows matched byte-for-byte. Artifact-version and grant counts stayed
50,000 and 100,000. Separate rolled-back writes with `RETURNING av.id` returned
the same single ID for every write before and after the index.

These are warm local plans, not production latency or a cold-cache benchmark.
Concurrent host compilation made timings noisy. For example, the cleanup read
ranged from 956.441 to 1323.174 ms before and 15.655 to 22.650 ms after. The
fixture's grant heap had 5,715 allocated pages following setup updates. Buffer
reductions are stronger evidence than these three-sample timings. SQL-only write
plans include triggers but not the Rust repository's preceding graph admission
and row/advisory-lock statements. Repository correctness tests exercise those
real methods separately. The probe explains literal-bound SQL, not SQLx cached
generic plans. Small or broad workloads may correctly use scans even with this
index.

Raw JSON, exact expanded SQL, and the local reproduction script are retained at
`/tmp/opencode/grant-index-evidence/` and
`/tmp/opencode/grant-index-probe.py`. These contain only synthetic fixture data.

## Reproducing the fixture

Use an exclusively owned, freshly migrated PostgreSQL database with the new
index absent for baseline measurement. Never remove an index from a live system
to reproduce this comparison. Keep the normal search path and planner settings.

Seed 1,000 artifacts, each owning 50 versions. Give version ordinals `n` from 1
through 50,000 unique object keys. Versions 1 through 40 are pending with
`body_updated = clock_timestamp() - interval '2 days'`; all others are ready with
valid provider, size, and digest metadata. Even ordinals have an unsealed
`object_segments` log stream and a terminal execution. Insert one body grant for
each odd ordinal and segment sequences 0, 1, 2 for each even ordinal. Set grant
state by `(n / 2) % 4`, using integer division: 0 is active issued, 1 is settled
issued, 2 is completed with required completion metadata, and 3 is expired.
Grant expiry is two days ago; settlement is seven days ahead for group 0 and one
day ago otherwise. Analyze all four queried tables before measurement.

Measure the unchanged repository SQL with a one-day cutoff and limit 100. For
write plans, use group 1's odd version, set the required body state inside the
same transaction, and always roll back. Add only the proposed index and repeat
the same queries, fixtures, and settings. Compare candidate rows and rolled-back
write IDs as well as timings and buffers. Do not force a particular query plan.

## Deployment

The forward migration uses ordinary transactional `CREATE INDEX`, following
local conventions. It takes a table lock that blocks writers during the build.
For a large deployed grant table, an operational `CREATE INDEX CONCURRENTLY`
build needs separate authorization and execution outside a transaction. This
change does not add or assume migration-runner syntax for that operation.

## Validation record

All commands ran from the feature worktree, or its `web/` directory for npm.
Database commands used only the owned container. URL values are omitted here.
Host `psql` was unavailable, so a temporary wrapper forwarded its calls to
`docker exec` on that exact container. `make db-test-setup` initially failed
while PostgreSQL was starting, then passed after `pg_isready` reported ready.
That startup failure is not counted as a passing validation.

Cargo database invocations set `ATTUNE__ENVIRONMENT=test` and
`ATTUNE__DATABASE__URL` to the owned database. SQLx used the same owned URL in
`DATABASE_URL`. Setup supplied owned URLs through `TEST_DB_ADMIN_URL` and
`TEST_DB_URL`. Builds used `CARGO_BUILD_JOBS=4`; final Rust checks also used the
CI value `RUST_MIN_STACK=67108864`. No developer database defaults were used.

The catalog test first failed with `RowNotFound` on the original migrations:

```sh
ATTUNE_TEST_RUN_ID=gidx-red-1030 cargo test -p attune-common \
  --test artifact_upload_grant_repository_tests grant_artifact_version_index \
  -- --nocapture --test-threads=4
```

After adding the migration, `sqlx migrate run` applied
`20261030000001` in 382.961933 ms to the populated fixture. A second invocation
passed without applying anything. Catalog checks confirmed a nonunique btree
on `artifact_version`, unchanged row counts, and successful migration history.

```sh
ATTUNE_TEST_RUN_ID=gidx-green-1030 cargo test -p attune-common \
  --test artifact_upload_grant_repository_tests \
  --test storage_maintenance_repository_tests -- --nocapture --test-threads=4
```

Both executables passed: 7 upload-grant tests and 12 storage-maintenance tests,
with no ignored tests. The new safety test exercises 40 combinations of grant
kind, settlement/state, and cleanup method. Active grants block selection,
claiming, transition to deleting, and both metadata-deletion methods. Settled
issued and terminal grants permit those operations, and successful metadata
deletion cascades the grants. A post-run check found zero red/green-run clones
before janitor recovery.

Logs and expanded plan SQL use the prefix `/tmp/opencode/grant-index-`.

Other validation commands and recorded results:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | Passed |
| `cargo fmt --manifest-path crates/core-timer-sensor/Cargo.toml -- --check` | Passed |
| `git diff --check` | Passed |
| `cargo check --all-targets --workspace` | Passed, no compiler warnings |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Passed |
| `cargo sqlx prepare --workspace -- --all-targets --all-features` | Passed; no query macros found, no metadata changes |
| `cargo deny --locked check` | Passed with existing duplicate-dependency warnings |
| `cargo deny --manifest-path crates/core-timer-sensor/Cargo.toml --config deny.toml --locked check` | Passed |
| Gitleaks 8.24.2 `git --log-opts=HEAD --redact --config .gitleaks.toml` | Passed, no leaks; downloaded to an owned temporary directory |
| Gitleaks 8.24.2 `stdin --redact --config .gitleaks.toml` over the diff and both new files | Passed, no leaks in uncommitted changes |
| `bash scripts/test-test-isolation-guards.sh` | Passed, 6 tests |
| `bash scripts/database-test-ignore-policy.sh` | Passed |
| `bash scripts/check-db-test-threads.sh` | Passed |
| `ATTUNE_TEST_RUN_ID=gidx-timer-1030 cargo test --manifest-path crates/core-timer-sensor/Cargo.toml --locked -- --test-threads=4` | Passed, 97 tests |
| `npm ci` | Passed |
| `npm run check:api` | Passed, generated client current |
| `npm run test:api-generation` | Passed |
| `npm run format:check` | Passed |
| `npm run lint` | Passed |
| `npm run typecheck` | Passed |
| `npm run typecheck:contracts` | Passed |
| `npm run build` | Passed |
| `npm run knip` | Passed |
| `npm test` | Failed to start Vitest fork workers; worker-response timeouts, not a passing test run |
| `npm test -- --maxWorkers=4` | Passed, all 78 files and 537 tests; bounded-worker rerun, not a pass of the original command |
| `ATTUNE_TEST_RUN_ID=gidx-wire-1030 cargo test -p attune-api --test web_form_contract_tests -- --test-threads=4` | Passed, 6 tests |
| `npm audit --omit=dev` | Failed, existing high-severity `follow-redirects@1.16.0` advisories `GHSA-q3jh-wrc6-jgq8` and `GHSA-8r9p-f939-6h3c` |

The first broader migration-suite invocation was:

```sh
ATTUNE_TEST_RUN_ID=gidx-mig-1030 cargo test -p attune-common \
  --test migration_tests -- --nocapture --test-threads=4
```

It reported 52 passed and one failed. The fresh SQLx installation test loaded
the new migration, but `Database::migrate()` used an older cached embedded list
and reported `migration 20261030000001 was previously applied but is missing in
the resolved migrations`. Adding a migration file during the session had not
invalidated the already compiled common library. `cargo clean -p attune-common`
removed only this worktree's package build artifacts before rebuilding and
repeating validation. No source or migration-history workaround was added.

The rebuilt migration-suite invocation used run ID `gidx-mig2-1030` and the
same four-thread command. All 53 tests passed with none ignored, in 172.44
seconds after compilation.

The rebuilt upload-grant and storage-maintenance invocation used run ID
`gidx-safe2-1030`, again with four threads. All 7 and 12 tests passed. A final
probe after SQLx applied the migration also matched the original candidate
rows, table counts, and rolled-back write IDs; its JSON is `migrated.json` in
the evidence directory.

After that rebuild, the full Clippy command, workspace all-target check, and
SQLx prepare command in the table all passed again. A final independent
`cargo check --all-targets --workspace` also passed. No compiler warnings or
SQLx metadata changes remained.

The full workspace/service, S3, broker, and E2E test lanes were not run.
Semgrep was unavailable locally and was not run.
These omissions and the failed default Vitest/audit checks do not constitute a
passing full CI run. PostgreSQL 16 was not measured.

## Teardown and change scope

All test and build subprocesses finished before database teardown. The final
ownership check found zero non-template test databases and zero remaining client
sessions. Five run-owned templates remained as expected. Awaited Docker stop and
removal deleted the exact labeled container and its one owned anonymous volume,
including those templates. Follow-up inspection confirmed both resources absent.
The teardown record is `/tmp/opencode/grant-index-teardown.json`.

At the end of this validation session, the feature worktree was on
`feature/artifact-upload-grant-index`, based on `0eda2d0c`, with changes uncommitted.
No push, PR, publication, deployment, live database changes, notifier changes, or
shared-infrastructure cleanup occurred during that session.
