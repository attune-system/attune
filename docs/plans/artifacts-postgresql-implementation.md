# Artifact reads on stock PostgreSQL: implementation plan

Date: 2026-10-14
Status: implementation and controlled local acceptance pass. Historical/deployed comparison and rollout remain unverified.

## Implementation checkpoint

### Latest storage cycle, PostgreSQL 18 only

The user limited the remainder of this cycle to PostgreSQL 18 to avoid repeating
the already long database matrix. Earlier PostgreSQL 16 evidence is retained;
that lane was not rerun after this instruction. Supported database versions and CI
defaults were not changed.

`/tmp/opencode/artifact-pg18-storage-fixed-source-20261015/evidence.json` passed
direct S3 upload, all nine storage correctness cases, and the 20-stream load test
on PostgreSQL 18.6. Source hashes stayed unchanged. No fixture databases or sessions
remained, and PostgreSQL/RustFS teardown completed without errors. There were 20
peak readers and zero leaked readers. `sse-helper-evidence.json` also records the
pure helper regression passing against the already-built executable, without
recompilation or another database fixture.

Production sources in this cycle were copied unchanged from the accepted
`artifact-sow-verified-final-20261015` snapshot. The manifest explicitly records
the sole test overlay, `crates/api/tests/runtime_log_replica_tests.rs`. This isolates
artifact verification from concurrent queue/permission work in the live checkout;
it is not a certificate for those later edits.

The storage test exposed a fixture ordering race. It marked the execution complete
before publishing its first segment, so slow publication could exceed the existing
two-second terminal grace. A controlled reproduction with a three-second provider
delay returned `log_stream_incomplete`. The corrected fixture retains that delay
but awaits publication and sealing before completion. A separate real-route test
proves that genuine early completion still reports the bounded incomplete-stream
error and closes. No production grace period or performance target was extended.

SSE test helpers now report error codes and timeout stages instead of discarding
error frames and surfacing only opaque EOF. The helper regression was red before
the correction and green afterward. A subsequent generic timeout did not reproduce
after stage diagnostics were added; its failed run remains at
`/tmp/opencode/artifact-sow-verified-storage-20261015`. Do not erase that result or
claim the passing fixed-source cycle explains every prior host/runtime timeout.
Staging load and recovery soak remain production acceptance work.

The live checkout's latest `cargo check --all-targets --workspace` passed without
warnings, and `git diff --check` passed. A new concurrent formatting-only diff in
`crates/api/tests/recorded_execution_events_tests.rs` prevents the latest full
format check from passing. That file was outside the six files the user authorized
for unrelated formatting cleanup and was left untouched.

### 2026-10-15 continuation

The missing executable latency gate now has an opt-in controlled reference. The
serial test reader calls the same repository visibility and runtime-log disclosure
functions, and the paired optimized path is the production handler mounted under
the same test router. Differential tests cover filtered pages and missing origins.
The comparison alternates reader order and requires identical bodies, 20 complete
warm samples per reader, a red serial statement ceiling, at least 90% fewer
statements, and at least 70% lower warm handler p95. This does not reconstruct a
historical build. Both PostgreSQL majors passed those controlled gates.

The complete fixed-source run at
`/tmp/opencode/artifact-sow-acceptance-20261015/evidence.json` passed on PostgreSQL
16.15 and 18.6, with unchanged source hashes and clean database/session/container
and volume teardown in the same run. Each major passed 29 routine artifact tests,
six key-usage tests, 65 notifier tests, and separately selected partition/churn,
cardinality, paired-reference, and notifier-authorization profiles. No gate was
lowered or test skipped to obtain that result. A prior notifier profile correctly
delivered a public artifact to its nominally denied reader; the corrected fixture
is private with an explicit artifact-ID grant for permitted readers. A prior
ancestry ceiling failure remains in its failed run; the ceiling was not relaxed.
New reports persist numeric diagnostics before that assertion.

| Controlled warm comparison | PostgreSQL 16 | PostgreSQL 18 |
| --- | ---: | ---: |
| Serial-reference handler p95 | 859 ms | 7,191 ms |
| Batched handler p95 | 33 ms | 213 ms |
| p95 reduction | 96.2% | 97.0% |
| Handler statement reduction | 98.6% | 98.5% |

These interleaved treatment/reference measurements passed the declared local
gates on this shared host. They are not a historical-build or deployed speedup.
Raw samples, cold-auth samples, response-equivalence checks, and explicit failure
results are retained. Host variability is visible in the absolute timings.

The fresh storage run at `/tmp/opencode/artifact-sow-storage-20261015` used the same
frozen source and passed direct S3 upload, eight storage correctness tests,
recorded-key stream revocation, and 20 concurrent reconnect scenarios. It recorded
40 puts, 80 gets, 20 peak readers, zero leaked readers, and clean teardown.
Reconnect-through-final-event p95 was 4,189 ms, not page TTFB.

The new notifier profile kept authorization evaluations at 1,300 for 20 events
with 64 permitted identities plus one denied identity, whether each permitted
identity had one tab or four. Four tabs increased deliveries to 5,120 without
increasing identity evaluation count. The denied identity received no frames.
The report preserves real SQL counts and authorization wall time separately.

Non-execution graph coverage now includes distinct event/inquiry chains at fixed
depth, in addition to the source-kind width tests. Events are inserted with their
provenance once; their immutable secret records are not overwritten to build chains.
The optional notifier profile records real visibility SQL, per-identity evaluation
and delivery, and authorization wall time separately from artifact requests. The
churn report records explicit audit emission/drain time and audit-row growth.

`scripts/artifact_read_capacity.py` now models retention and backlog with exact
rounding, explicitly supplied storage/reserve assumptions, and failing exits for
insufficient headroom. Five tests cover the math, negative headroom, invalid inputs,
BIGINT-scale rounding, stdin, and no-overwrite output. Deployment forecasts still
require representative operator observations. See
[the acceptance runbook](../deployment/artifact-read-acceptance.md).

The Python and JavaScript SDK trees already contained generated pagination changes
from other work. They were preserved rather than regenerated. New contract tests
exercise progress queries, typed page parsing, history traversal, and invalid-page
errors. The full Python suite passed 111 tests; JavaScript passed 72 tests and its
build. Java's generic-client contract test and migration notice are in place, and
all 45 Java tests passed. SDK package publication remains a release action, not a
source-code blocker.

Read-only discovery through the installed system-wide MCP client reported local
and server SHA `a354b9cf239eefd3a15d8589ac61bf65b8c50747`. The user clarified that
this MCP binary is not the current development build. It was left untouched;
installing a dev MCP server would be a separate task. Treat its report as an
installed-client observation, not an independently confirmed API/web deployment
certificate or an identifier for this checkout's uncommitted sources. No live
artifact bodies, secret values, or database settings were collected or changed.

The user approved formatting-only changes in the six unrelated files identified
by the workspace check. Their behavior and edits were preserved. After that change,
`cargo fmt --all -- --check`, `git diff --check`, and warning-free workspace
compilation all passed. Current web validation passed 392 tests and the build.

The read-path, pagination, browser refresh, and local measurement work is implemented.
This is not full P0-P9 acceptance or live performance certification.

| Unit | Current implementation |
| --- | --- |
| P0 | Fresh PostgreSQL 16/18 verification passed for SQLx and Docker runners. The disposable writer role now has the SELECT/INSERT rights required by the input-authority inheritance trigger. Deployment grants are unchanged. |
| P1 | Real-route SQLx, pool acquisition, Linux poll CPU, actual plans, source fingerprints, partition/batch/graph/cardinality/churn profiles validated on 16/18. The controlled serial-reference comparison passed the 90% query and 70% p95 targets. No historical/deployed speedup claim. |
| P2 | Global/execution/history summary projections exclude large artifact/version payloads. Execution reads use a count-free internal keyset scan and EXISTS linkage. Version pages use bounded summary queries and the same visibility predicate for counts. |
| P3 | Direct execution authorization, combined input/output provenance, and retained schema bindings are batched. Provenance reads exclude ciphertext and encryption-key hashes. |
| P4 | All supported source-kind scope records and public-action fallback are batched. Cross-version route/query tests passed, including fresh inquiry attributes, scope-first binding checks, distinct ancestry, and per-traversal depth/cycle rejection. Recursive decisions remain uncached. |
| P5 | Narrow ID-only event batches retain immutable `created` and reject ambiguous physical IDs. Daily/DEFAULT repair, expiry, prepared-plan, and populated partition-count tests passed on 16/18. Measured fan-out grows with leaf count; no pruning claim. |
| P6 | Exact-total execution pages, visible history pages, checked bounds, filtered progress discovery, CLI traversal, and browser navigation are implemented. Web and Python artifact clients match the generated contract. PostgreSQL 16/18 pagination and mutation-scope regression tests passed. |
| P7 | Shared refresh, cancellation, bounded previews, page resets, and reconciliation are implemented. Browser loss recovery, real notifier gaps/lag, object-stream revocation, storage correctness, and 20-stream load tests passed. |
| P8 | No optimization-specific index/migration, autovacuum, or supervisor cadence change. Local capacity/churn profiles passed with complete expired-cohort deletion. A tested capacity/backlog/reserve calculator is implemented. Deployment forecasts and alert policy still need representative operator inputs. |
| P9 | Full cross-version local selection and storage validation pass on the same frozen source with clean teardown. SDK source contracts/tests and notices are verified. Approved formatting-only cleanup closed the whole-workspace format gate. Package publication, a matching deployment, and live/operator measurements remain release/operational work. |

### Earlier 2026-10-14 validation

`/tmp/opencode/artifact-sow-validation-isolated-v2-20261014` retained a hash-checked
source copy and independent build directory. Both PostgreSQL 16.15 and 18.6 passed
26 artifact tests, six key-usage tests, 65 notifier tests, and both explicit workload
profiles. Both ignored stress tests were selected separately and produced reports.
Source hashes were unchanged, fixture databases were gone, and sessions were zero
before container removal. Docker's 30-second PostgreSQL 18 removal command timed
out, so `evidence.json` correctly remains failed. `late-teardown.json` separately
records verified container/volume absence after the owned removal completed.
The runner now allows 180 seconds for those removal commands. It still reports
cleanup failures rather than hiding or retrying an entire test run.
`/tmp/opencode/artifact-owned-cleanup-budget-proof.json` records a fresh owned
PostgreSQL 18 startup/removal with that budget, zero sessions, and clean teardown.
This check does not relabel the earlier full run as passing.

`/tmp/opencode/artifact-runtime-log-acceptance-final-20261014` passed direct S3
upload, eight storage correctness tests including recorded-key stream revocation,
and the 20-stream load selection against the same frozen source. Its manifest has
no source changes, fixture database/session leaks, or cleanup errors. The numeric
load report records 40 puts, 80 gets, a peak of 20 streams, and zero leaked streams.
Reconnect-through-final-event p95 was 5,357 ms. That is not page TTFB or a claimed
speedup.

Local history-heavy execution-page warm p95 measurements were 392 ms on PG16 and
290 ms on PG18 at 1,000 linked executions. Four-client p95 was 1,037 ms and 990 ms.
These are treatment-only in-process requests. The test also passed bounded pages
at 2/20/100 artifacts with a 10 MiB progress body excluded from summaries. Plans,
per-request samples, CPU/pool counters, capacity, maintenance, and cardinality
summaries remain in the per-major evidence directories.

Latest live-tree workspace compilation passed without warnings. Scoped formatting,
12 benchmark-runner tests, six report tests, 40 native-verifier tests, four Python
client tests, and storage-harness safety checks passed. The web suite passed 380
tests plus build and focused ESLint. Whole-workspace formatting still reports
unrelated changes, including callback formatting, module ordering, workflow tests,
and pack-registry code. No unrelated work was reformatted.

Remaining acceptance gates are explicit:

- No preserved comparable legacy p95 sample set exists. The 70% latency-reduction
  gate is unverified, despite reproduced query-growth improvement.
- Deployed revision, browser/network timings, notifier/audit pressure, production
  retention capacity/headroom, and live root-cause confirmation need sanitized
  operator evidence. No deployment or live setting change was made.
- External typed SDK source contracts and generic-consumer notices are now
  verified. Package publication and coordinated deployment remain release work.
  In-tree API, web, Python test client, and CLI contracts are updated.
- A clean single-run teardown result remains distinct from the completed late
  cleanup proof. Frozen-source results do not certify subsequent live-tree edits.

The first real-route baseline used 15 statements for two logs with one linked
execution and 807 with 100. The endpoint-policy characterization passed. Its
manifest detected an unrelated source change during the run, so its timings are
not a certified baseline. Preserve that evidence and recapture a pinned baseline
before claiming a latency percentage improvement.

Lightweight artifact candidates use batches of 256. Execution disclosure records
use batches of 64 because retained schema bundles can contain large workflow
snapshots. These are initial bounds, not a claim that tuning is complete. The
benchmark exercises one, 64, and 100 linked executions and saves query plans
separately from handler timing. No body/metadata policy change was made.

The treatment run at `/tmp/opencode/artifact-treatment-checked-20261014` passed
on PostgreSQL 16 and 18 with unchanged source fingerprints and awaited fixture
cleanup. Each major passed six artifact-read tests and the six existing key-usage
tests. Both majors used 6 handler statements for 1 or 64 linked executions and 9
for 100. This establishes the query-growth improvement, not a p95 latency SLA or
completion of the remaining recursive, partition, pagination, and capacity work.

This slice also passed all 358 Vitest tests, web typecheck/build, focused changed-file
ESLint checks, the four benchmark-runner unit tests, the 40 native-verifier unit
tests, `cargo fmt --all -- --check`, and `cargo check --all-targets --workspace`.
Object-backed download/stream load tests and operator live verification remain
part of full-plan acceptance rather than implied by these checks.

### Execution-source graph continuation

The fixed-source PostgreSQL 18 baseline at
`/tmp/opencode/artifact-graph-baseline-retry-20261014` reproduced 16 handler
statements for one execution origin and 266 for 64 distinct workflow-child
origins. Its source fingerprints were unchanged and fixture cleanup passed. The
earlier attempt failed during PostgreSQL startup and is not performance evidence.

The continuation adds `crates/api/src/disclosure_records.rs` for request-local
source records. It groups parent provenance by kind and processes batches of 64
IDs for each discovery frontier. Missing sources remain explicit empty/missing
records. Execution scope uses the existing evaluator over narrow execution rows
and batched repository lineages, not a second RBAC implementation.

Lineage queries preserve the last surviving anchor when a parent dangles. Root
visibility still follows the full lineage; ancestor-owner constraints still see
only the first 64 parents. Malformed cyclic read/decrypt ancestry now terminates
and denies access instead of leaving the old recursive root query running without
a result. Mutations keep their own execution target and bounded 64-parent owner
lookup, without resolving a read anchor or adding a cycle-based mutation denial.

Record reuse does not authorize a recursive subtree. The evaluator still rejects
depth 32 and uses a fresh active path for each secret traversal. Regression tests
cover a shared leaf reached at depths 1, 31, and 32 in either origin order, cycles,
missing paths/sources, independent expiry, private workflow roots, self versus
descendant scope, and the 64-parent owner cutoff.

The graph treatment at `/tmp/opencode/artifact-graph-treatment-20261014` passed
on both PostgreSQL majors with unchanged source hashes and no clone/session
leaks. Both 1 and 64 distinct execution origins used 10 handler statements and
zero ciphertext reads. All eleven artifact-read tests and six existing key-usage
tests passed on each major. The workspace check passed without warnings.

### Pagination continuation

Execution and version-history lists now return bounded `items` and exact
`pagination` rather than `data` arrays. Execution page selection and totals share
one count-free candidate scan after disclosure. Version count/page reads share
their SQL visibility predicate after the artifact gate. Page sizes are 1-100,
defaulting to 20; page zero and 32-bit offset overflow are rejected at the request
boundary. A debug event records execution candidate, eligible, and returned row
counts plus total scan duration without logging content.

The browser uses server pages with explicit navigation, and progress discovery
uses the execution endpoint's filtered one-row read. CLI version listing traverses
all pages and preserves its existing JSON/YAML array output. See
[the API contract](../api/artifact-reads.md) for offset mutation semantics and
consumer coordination. Tests cover a fully denied first candidate batch, filtered
totals, hidden newer versions, out-of-range pages, boundary validation, and live
reset/resume behavior.

The attributed pagination run at
`/tmp/opencode/artifact-pagination-attributed-20261014` passed all fourteen
artifact tests and six existing key-usage tests on both PostgreSQL majors, with
unchanged source hashes and awaited cleanup. The one-origin and 64-origin cases
both used six graph-record reads and ten total handler statements. The gate now
attributes graph-record reads separately from other handler queries, requires
constant graph reads within the frontier batch, and retains the predeclared
20-statement handler ceiling. A prior PG16 trial exceeded the relative warm-count
comparison while staying under that ceiling; those results remain in
`/tmp/opencode/artifact-pagination-validated-20261014`. Query counts are not a
latency SLA. The observer also tests SQLx's newline-wrapped statement format so a
ciphertext projection cannot escape its counter.

The shared ancestry helper's mutation branch has a real-route regression test.
Before correction, cancelling an owned execution with cyclic ancestry returned
403 instead of the existing 200 outcome. That red result and owned fixture cleanup
are recorded in `/tmp/opencode/artifact-cancellation-red.json`. The correction
keeps read/decrypt cycle denial separate from bounded mutation scoping.

The final pagination/cancellation run at
`/tmp/opencode/artifact-pagination-final-retry-20261014` passed all fifteen artifact
tests and six key-usage tests on PostgreSQL 16.15 and 18.6. Source hashes were
unchanged. Both servers had no remaining fixture databases, zero sessions before
removal, and no cleanup errors. This result precedes the non-execution source
continuation below; it does not validate those newer changes.

Live resets evict browse-page queries, not independently mounted progress-discovery
queries under the same execution prefix. A cache regression test reproduced the
over-broad eviction before the correction. Loaded paused pages reject automatic
shared invalidations while still fetching a newly selected page.

Current local checks include the full web suite, web build/typecheck, focused ESLint,
seven CLI artifact tests, four offline generated Python-client tests, scoped Rust
formatting, and workspace compilation. Web tests use one worker and a 20-second
per-test budget on this shared host; earlier worker-startup and timeout failures
are not passing evidence. Whole-workspace formatting encountered unrelated
`runtime_approvals` module ordering, which this change has not reformatted.

### Non-execution source and workload continuation

The continuation adds batch scope records for event identity/bindings/visibility,
inquiry identity/bindings/creators, component owner survival, sensor generations,
initial rules and deliveries, and current public-action fallback. Scope evaluation
still uses the endpoint's existing predicates and the shared authorization service.
No recursive authorization decision is reused.

New route tests cover 1/64/65 sources of every supported kind, constrained event
visibility, inquiry participant/decrypt rules, missing sources/bindings, repository
failures, retired rule identity, and distinct public-action fallbacks. Native event
tests cover DEFAULT repair, duplicate physical IDs, and partition expiry. A further
inquiry regression is being run because the original inquiry evaluator loads fresh
identity attributes while its parent consumer can have a cached snapshot. The
batched path must retain that distinction.

That regression reproduced an artifact 200 where the inquiry endpoint returned
404 after an attribute change. The genuine red result, after correcting an invalid
empty-options fixture, is in
`/tmp/opencode/artifact-inquiry-attributes-red-retry.log` and its `.json` manifest.
The fix shares a fresh, narrow identity-attribute read with the inquiry evaluator.
Inquiry binding validation remains after its participant/read/decrypt scope gate.
Both execution input and output origins now share one provenance read per frontier
batch, and sensor initial-rule scope projections exclude configuration values.
Route regressions also cover distinct workflow ancestry, combined input/output
origins, and scope-first handling of corrupt inquiry bindings. The workload profile
captures the same lineage SELECT used by authorization for shared/distinct depth-eight
graphs, not an approximate query.

The CPU observer also needed to handle SQLx reentering its current span. A workload
attempt aborted on the original extension replacement; the observer now tracks
outermost entries per thread and has a passing pure reentrancy regression. Aborted
attempts with fixture clones still present before server removal are failed
evidence, even though owned server/volume teardown completed.

The explicit `--workload-profile` adds populated 1/7/30/90-leaf event profiles,
SQLx-prepared 1/16/64/256-ID probes, complete expired execution cohorts, concurrent
reads and native/audit/summary maintenance, and capacity snapshots. Diagnostics
record pool acquisition and Linux poll CPU without changing live settings. These
are implemented measurements awaiting runtime acceptance, not a completed p95 or
deployment-capacity gate.

The first all-source attempt at
`/tmp/opencode/artifact-all-source-origins-20261014` aborted before database tests
because concurrent schema-consumer changes did not compile. The subsequent focused
compile passed. Later test/profile additions still require a final stable-source
run; the in-progress workload attempt is not the final source certificate.

The initial object-storage attempt at
`/tmp/opencode/artifact-runtime-log-acceptance-20261014` also stopped during
compilation. Its cleanup exposed Docker host-network access to a loopback-published
RustFS endpoint. The harness now uses its labeled private network/container
endpoint for cleanup and removes its anonymous storage volume. The regression was
red before that correction and green afterward. Label-verified fallback teardown
for the failed attempt is recorded in its `fallback-cleanup.json`; it is not a
passing object-stream result.

The next storage attempt reached the real routes and rejected unbound execution
fixtures. Those fixtures now materialize real released action snapshots and attach
their admission schema bindings. No disclosure fallback was added. The corrected
storage teardown passed. Cross-version and storage validation now run against a
hash-checked source copy because the shared checkout is still changing. The first
capture correctly refused missing static release fixtures; the copy logic now
retains those fixtures and has a regression test. Source-copy results are not
certificates for later live-tree changes.

The storage suite additionally checks recorded key-decrypt revocation between real
object-backed artifact SSE frames. That is a correctness test with explicit source
grants, not a change to content authorization or a claim of instant revocation for
bytes already delivered to a client.

The fixed-source run at `/tmp/opencode/artifact-source-frozen-retry-20261014`
passed 65 notifier tests and its partition/maintenance profile on both majors.
The complete run failed because a constrained-event test used a public trigger
and expected it to become unreadable after removing its explicit trigger grant.
That fixture now uses a private trigger. Do not treat the failed run as acceptance.
The profile deleted all 600 expired execution rows, dropped its owned expired event
leaf, and left zero expired execution rows. Its prepared event reads visited more
leaves as partitions were added, confirming that ID-only batching does not prune
native partitions. No new index or maintenance change follows from that result.

`/tmp/opencode/artifact-runtime-log-frozen-20261014` passed direct S3 upload and
seven object/shared-volume correctness tests with unchanged source and clean
teardown. Its separate load selection failed because the disposable server did
not preload `pg_stat_statements`. Load validation requires that extension in
template0 before physical fixture clones are created. Only the owned test server
is configured for it; live PostgreSQL settings are unchanged.

The cardinality profile now covers 2/20/100 attached artifacts, two shared logs at
1/100/1,000 retained executions, four simultaneous Axum clients, and summary
exclusion of a 10 MiB progress body. These measured workload cases do not replace
the missing fixed-source legacy p95 baseline or operator deployment forecast.

The current web suite passed 380 tests in 61 files, including missed updates and
unnotified deletion while the socket context remains connected. The build and
focused ESLint check passed. See
[Artifact-read acceptance](../deployment/artifact-read-acceptance.md) for local
commands, measurement limits, and the operator/rollout gates.

## Scope and sources of truth

Implement the artifact-read improvements from
[Artifacts page-load performance](artifacts-page-load-performance.md) against the
current stock PostgreSQL schema. That document owns endpoint policy, pagination
semantics, browser refresh behavior, and acceptance targets. This document maps
those requirements and the PostgreSQL-specific work into implementation units.

TimescaleDB removal is already represented in the repository. Native partition
and summary operational acceptance remains separate work, described in
[the native-maintenance plan](postgresql-partitions-and-hourly-summaries.md).
Do not repeat database conversion, restore TimescaleDB dependencies, or reopen the
deferred reporting-performance project as part of artifact optimization.

Deployment and migration rules come from
[PostgreSQL-only deployment](../deployment/postgresql-only.md). Preserve unrelated
working-tree changes. Match the deployed revision before attributing live latency
to the current source. No commits, service restarts, resets, or live configuration
changes are authorized by this plan.

## Storage assumptions

| Data | Current storage | Implication for this work |
| --- | --- | --- |
| Artifacts, versions, executions, secret values, runtime schema bindings | Ordinary tables | Optimize projections, joins, batching, and indexes; no time-partition pruning |
| Events | Daily native RANGE partitions on `created`, plus DEFAULT | ID-only provenance reads can search many partitions |
| Execution history and audit events | Daily native RANGE partitions | History/audit writers and retention are competing database workloads |
| Hourly analytics summaries | Ordinary derived tables | Not an authorization oracle or a replacement for per-caller artifact totals |

PostgreSQL 16 is the minimum supported major; Compose and CI currently use 18.
Use the deployed major/configuration for performance comparisons. Correctness
must pass on 16 and 18 without TimescaleDB. Retained sizes must be remeasured;
TimescaleDB columnstore compression is not available in the current schema.

## Delivery order

| Unit | Dependency | Deliverable |
| --- | --- | --- |
| P0 | None | Source/deployment inventory and plain-PostgreSQL readiness evidence |
| P1 | P0 | Reproducible endpoint benchmark and database-work measurements |
| P2 | P1 | Narrow artifact projections and count-free candidate reads |
| P3 | P1 | Batched execution, secret, and historical-binding reads |
| P4 | P2, P3 | Request-local authorization, recursive-origin and ancestry batching |
| P5 | P1, P4 | Partition-aware event-source reads and retention interaction tests |
| P6 | P2, P4 | Execution/history pagination and coherent API, CLI, and browser consumers |
| P7 | P6 | Refresh coordination, reconciliation, live page resets, bounded previews |
| P8 | P5, P6, P7 | Conditional index/statistics changes and retention/capacity evidence |
| P9 | All required units | Cross-version validation and operator-run rollout verification |

P2 and P3 can be implemented independently. Keep P4's direct authorization,
ancestry, and source-kind batch changes separately reviewable. Implement only the
P8 changes justified by measurements; P8 can finish with evidence that no schema
or operational change is needed. Metadata/body policy changes remain outside this
sequence until the security approval gate in the parent plan is satisfied.

## P0: confirm the installed PostgreSQL model

Inspect `docker-compose.yaml`, `.github/workflows/ci.yml`, `migrations/`, and the
existing native-install verifier. No image or schema rewrite is expected here.

1. Record source revision and relevant uncommitted file hashes, migration hashes,
   PostgreSQL major/minor, image identity, pool limits, retention horizons, and
   planner/memory settings in the benchmark manifest. Keep credentials out.
2. Confirm ordinary versus partitioned tables, attached indexes, DEFAULT leaves,
   native-maintenance schedules, and service-role privileges from actual catalogs.
   An old migration filename containing `timescaledb` is not an extension dependency.
3. Run the existing fresh-install verification on owned PostgreSQL 16/18 servers
   after the source freeze. Follow
   [Native fresh-install verification](../deployment/postgresql-native-install-verification.md);
   do not turn a declaration check or historical result into a passing runtime gate.
4. For pre-prod, request operator-collected inventory. If its schema or revision
   differs, report that mismatch before planning a migration against it.

Done when the storage/role inventory and versioned source manifest exist and the
applicable fresh-install checks pass. A populated TimescaleDB deployment would
need a separately approved data-preservation/cutover plan, not this implementation.

## P1: measure statements and actual PostgreSQL work

Primary files:

- `crates/api/tests/helpers.rs` and `crates/common/tests/helpers.rs`
- Proposed `crates/api/tests/artifact_read_performance_tests.rs`
- `crates/common/src/db.rs` and the repository query instrumentation seam
- `scripts/run-rust-integration-tests.sh`, reused rather than replaced
- Proposed repository diagnostics helpers only where existing instrumentation
  cannot expose the required measurements

1. Add the parent plan's fixtures and real Axum-route benchmark. Separate ordinary
   integration query-growth checks from an explicitly selected timing/load profile.
   Keep timing runs isolated from other executables while respecting the existing
   database-test thread policy.
2. Add event partition-count profiles with 1, 7, 30, and 90 retained daily leaves,
   plus late data in DEFAULT. Keep requested origin count and graph depth constant
   while varying unrelated partitions. The 90-day event profile is an owned stress
   fixture, not a change to deployment retention.
3. Capture handler statement count, returned rows/bytes, authorization CPU, pool
   wait, TTFB, completion time, and memory. Attribute audit/notifier activity
   separately; do not infer per-request attribution from cluster-wide counter deltas
   while concurrent writers are active.
4. Capture the exact repository-generated SELECT statements and fixture-only binds
   at a test seam. Inspect them with `EXPLAIN (ANALYZE, BUFFERS, FORMAT JSON)` against
   owned data. Record planning/execution time, scanned/removed rows, loop counts,
   buffer hits/reads, heap fetches, partition fan-out, and temporary spills.
   EXPLAIN runs are separate from endpoint timing samples.
5. Analyze the seeded ordinary tables and partition parents/leaves before the
   clean baseline. Add a separate churn profile with inserts/deletes and naturally
   maintained statistics. Keep settings and data equal between baseline and
   treatment; do not hide poor plans by disabling sequential scans or forcing JIT.
6. Test small and near-limit ID batches through SQLx's real prepared-query path.
   Repeated execution can expose different plan choices than one literal SQL run.
   Start with a proposed batch limit of 256 IDs, compare smaller/larger candidates,
   and pin the selected limit in the manifest before treatment measurements.

Use repository-owned database access for diagnostic SELECTs and fixture maintenance.
`pg_stat_statements` is optional supplementary evidence, not a required extension
or a reason to change a live server. Use unused run-owned output directories under
`/tmp/opencode`; await writer shutdown and teardown.

Done when baseline results reproduce the declared regression, generated query
plans are recorded, and per-query/per-batch growth budgets are predeclared. A
reduction in round trips alone does not satisfy the PostgreSQL-work comparison.

## P2: narrow artifact queries and remove redundant counts

Primary files:

- `crates/common/src/repositories/artifact.rs`
- `crates/common/src/models.rs`, only if a shared projection belongs there
- `crates/common/tests/repository_artifact_tests.rs`
- `crates/api/src/dto/artifact.rs`

1. Add typed artifact-summary, version-summary, and authorization projections with
   explicit select-column constants. Exclude `data`, descriptions, JSON version
   content, and metadata not needed by the response or authorization.
2. Reuse artifact search filters and SQL visibility predicates. Add a count-free,
   keyset-advanced internal candidate scan instead of calling count-plus-page
   search once per batch. Public pagination remains the parent plan's page contract.
3. Replace execution linkage's full-row join/`DISTINCT` with `EXISTS`. Add batched
   artifact-to-execution linkage reads that preserve missing execution IDs and
   deduplicate without fetching full version bodies.
4. Add bounded version-summary reads and visibility-filtered version counts using
   the same predicate. Detail reads retain content where the current contract
   requires it; do not silently turn detail responses into summaries.

Done when global/execution/history summary reads never transfer or decode the
10 MiB payload fixture, deterministic ordering is retained, and internal candidate
batches issue no repeated count queries. Compare plans with the existing indexes
before proposing any migration.

## P3: batch direct authorization records

Primary files:

- `crates/common/src/repositories/execution.rs`
- `crates/common/src/repositories/execution_secret_value.rs`
- `crates/common/src/repositories/schema_contract.rs`
- Corresponding repository integration tests

1. Add narrow execution authorization rows selected by bounded ID arrays. Derive
   fields from existing visibility/disclosure call sites instead of reconstructing
   partial full `Execution` models with dummy values.
2. Add one combined input/output provenance query per execution batch. Return
   entity kind/ID, path, source kind, and source ref; exclude ciphertext and key
   hashes. Keep plaintext/decryption reads on their existing separate path.
3. Batch retained schema-binding reads while preserving digest, source, and schema
   validation. Cache validated bindings within the request, not current action
   definitions. Record missing requested IDs explicitly rather than omitting them
   through an inner join that could weaken disclosure checks.

Done when increasing executions within a batch leaves statement count constant,
large ciphertext does not affect authorization transfer bytes, and missing or
invalid bindings retain the characterized denial/error behavior.

## P4: centralize request-local authorization and batch graph reads

Primary files:

- Proposed `crates/api/src/artifact_reads.rs`, registered in `crates/api/src/lib.rs`
- `crates/api/src/authz.rs` and `crates/api/src/secret_disclosure.rs`
- `crates/api/src/routes/artifacts.rs` and `crates/api/src/routes/executions.rs`
- Execution, event, inquiry, component-configuration, and sensor repositories

1. Expose named detail, global-list, execution-list, and version read operations
   through a small module interface. Hide batching, visibility/disclosure staging,
   and caches inside it. Reuse existing authorization snapshots and evaluators;
   supported endpoint policies come from the parent plan's matrix.
2. Share direct authorization reads across stdout/stderr and across list counting
   and page selection. Preserve fresh execution-token delegated-authority checks.
3. Add repository batch resolution for workflow anchors and ancestor executor IDs,
   keyed by requested execution. Preserve self/descendant rules, private ancestry,
   traversal limits, and missing-parent behavior. Replace route-owned recursive SQL
   in the affected path with repository reads rather than adding another evaluator.
4. Discover provenance sources by frontier, group by entity kind, and batch source
   rows, bindings, owner-survival checks, and parent provenance. Cover every source
   kind actually supported by `origin_allows`.
5. Start with record caching and per-traversal evaluation. Add decision caching only
   with proof that depth budget and active traversal do not change the result.
   Preserve depth 32 and traversal-order independence; infrastructure failures stay
   distinct from access denials.

Done when endpoint characterization tests pass and distinct-origin/child-execution
growth follows batch/frontier count rather than one round trip per entity. Run
existing key-origin, execution-token, and stream-revocation tests unchanged.

## P5: handle native event partitions and source expiry

Primary files:

- `crates/common/src/repositories/event.rs`
- `crates/common/src/repositories/event_schema.rs`
- `crates/api/src/secret_disclosure.rs`
- `crates/common/tests/native_partition_repository_tests.rs`
- Proposed artifact-read API/performance tests

1. Add narrow batch event-source reads through the partitioned parent. Existing
   `SecretOrigin::Entity` records provide an ID/path, not an event timestamp.
   Initially retain ID-only lookup semantics and measure partition fan-out;
   batching is not a claim of partition pruning.
2. Include `created` in returned source identity and preserve binding-owner checks.
   Native `(id, created)` keys do not enforce global ID uniqueness. Do not silently
   overwrite duplicate IDs when building a batch map. Characterize ambiguous-source
   behavior; unresolved ambiguity must not authorize disclosure and requires an
   explicit source-contract decision before changing selection semantics.
3. Test parent-visible sources in both daily and DEFAULT leaves, atomic DEFAULT
   repair, and independently expired sources. Dropping an event partition must not
   leave a cached cross-request allow decision or depend on row DELETE triggers.
4. If partition fan-out remains material, propose a separate timestamp-qualified
   provenance contract. Determine where the immutable timestamp is captured, update
   all producers/readers and contract tests together, and review retention semantics.
   Do not derive time from an ID, use the destination execution's timestamp, or add
   an arbitrary recent-window filter. This optional contract change is not P5's
   initial implementation or a hidden requirement to pass its correctness gate.

Done when batched reads preserve retained/expired-source outcomes on both majors,
DEFAULT is covered, and plans quantify ID-only partition work. Any residual cost
and optional timestamp-contract proposal are documented rather than concealed.

## P6: paginate eligible results and update every in-tree consumer

Primary files:

- `crates/api/src/routes/artifacts.rs`, `dto/artifact.rs`, and `openapi.rs`
- `crates/common/src/repositories/artifact.rs`
- `crates/cli/src/commands/artifact.rs`
- `web/src/hooks/useArtifacts.ts` and generated `web/src/api/` files
- Artifact list/detail pages, `ExecutionArtifactsPanel.tsx`, and
  `ExecutionProgressBar.tsx`

1. Implement the parent plan's exact-total eligible scan for execution lists.
   One scan supplies both count and selected summaries; SQL visibility counts
   cannot substitute for disclosure-filtered totals. Instrument total-scan cost.
2. Add execution-artifact and version-history pagination using the existing v1
   response conventions. Validate page sizes and offset arithmetic at the request
   boundary. Use visible-version counts after the artifact gate.
3. Update OpenAPI and regenerate clients. Replace browser fetch-all/slice logic with
   server pages and explicit navigation. Give progress discovery a filtered bounded
   read that does not require finding its artifact on page one.
4. Update CLI version listing's array-response assumption and audit all in-tree
   readers of these endpoints. Preserve the CLI's documented listing behavior by
   using explicit pagination or deliberate page traversal; never silently truncate.
   Record external SDK consumers that need coordinated contract updates.
5. Retain exact totals initially. If their measured full-set cost prevents the
   latency gate, request review of optional totals using the existing
   `PaginatedResponse::without_totals` capability rather than returning an estimate
   as exact or using analytics summaries.

Consumer audit for the remaining pagination change:

- Update both execution views, `ExecutionArtifactsPanel.tsx` and
  `ExecutionProgressBar.tsx`. The latter currently discovers progress by searching
  the entire execution list, so merely switching that list to page one loses
  progress outside the first page.
- Update `tests/e2e/api/test_standalone_transport.py` and regenerate the test
  Python client under `tests/generated_client/` with the new OpenAPI response.
- The sibling `python-attune-sdk` has generated `list_artifacts_by_execution.py`
  and `list_versions.py` consumers. `js-attune-sdk/src/api_client/sdk.gen.ts` and
  its generated types expose both endpoints. Regenerate those separately as a
  coordinated API-contract update; this change has not modified sibling repos.
- `java-attune-sdk` currently exposes generic request methods. Its caller-authored
  deserialization of these endpoint responses still needs the contract notice.

Done when API, generated client, CLI, and browser tests agree on one contract,
denied leading candidates do not hide later readable rows, and summary/page work
is distinguished from full-total authorization cost.

## P7: recover missed updates and bound browser work

Primary files:

- `web/src/hooks/useArtifactStream.ts` and `useArtifacts.ts`
- Artifact pages and execution artifact/progress views
- Focused hook/page tests; notifier integration tests for loss reproduction
- `crates/notifier/src/service.rs` and `postgres_listener.rs` only if a measured
  defect requires a backend fix, not to replace PostgreSQL LISTEN/NOTIFY

Implement the parent plan's shared coalescing/in-flight scheduler, connected
reconciliation and disconnected fallback, reconnect/completion refreshes, pause
behavior, and live page resets. Make all refresh sources use the same coordination
so polling cannot reintroduce duplicate work. Exercise deletion without an artifact
event, broadcast lag, and listener reconnect while the browser socket stays open.

Bound previews using ranges, streaming consumption limits, cancellation, and safe
UTF-8 decoding. Keep full download explicit. Distinguish the retained preview-byte
limit from transport read-ahead when testing a response that ignores ranges.

Done when missed delivery converges through reconciliation, burst/refetch counts
meet the parent plan's gates, paused views remain paused, and preview tests cover
large bodies, empty bodies, truncated text, and ignored ranges.

## P8: review indexes, statistics, retention, and capacity

Primary files and existing paths:

- `crates/common/src/repositories/artifact.rs` and `execution.rs`
- `crates/common/src/repositories/retention.rs` and `native_maintenance/partitions.rs`
- `crates/supervisor/src/`, inspected for workload/cadence interactions
- New `migrations/` entries only when query-plan evidence justifies them
- `docs/deployment/postgresql-only.md` and `supervisor.md` for approved runbook changes

1. Compare actual batch/EXISTS plans with existing execution, parent, artifact
   linkage, and version-order indexes. Review any additional index's write, WAL,
   storage, and deletion costs. Avoid covering indexes containing large JSONB or
   assuming an index-only scan eliminates heap work after churn.
2. Record ordinary-table dead tuples, vacuum/analyze cadence, last analyze times,
   table/index/TOAST sizes, retained-row counts, WAL, retention backlog, and disk
   headroom. Statistics counters are estimates where PostgreSQL reports estimates;
   do not label them exact bloat measurements.
3. Exercise realistic owned reads concurrently with bounded ordinary-row deletion,
   event partition repair/drop, audit writes, and summary maintenance. Compare
   complete expired cohorts and distinguish row cleanup from partition expiry.
   Record lock/pool waits and post-churn read plans, not just deletion wall time.
4. Check parent statistics freshness for native partitions as well as ordinary-table
   autovacuum. If stale parent statistics measurably affect the new query paths,
   propose bounded supervisor-owned ANALYZE with role, timeout, cancellation, and
   cadence tests. Keep ANALYZE/DDL off artifact request paths. Do not alter global
   autovacuum or pool settings without operator approval.
5. Document measured retained bytes and growth at configured horizons, with and
   without maintenance backlog. No compression-ratio assumption carries over from
   TimescaleDB. Define deployment headroom/alert thresholds from this evidence.

Done when the churn/capacity report distinguishes query defects from maintenance
pressure, and each proposed schema or operational change has a measured benefit,
write-cost comparison, role check, and migration/rollback review. If a schema change
is approved, run `cargo sqlx prepare`; never rewrite deployed migration checksums.

## P9: validate and roll out one coherent revision

Follow [Running tests](../testing/running-tests.md) and the parent plan's validation
commands. Extend the existing runner/CI only as needed to select the new tests;
do not create an alternative deployment stack. Keep routine query-growth tests
blocking and noisy timing profiles explicitly selected on a pinned environment.

Required evidence:

- PostgreSQL 16/18 repository/API correctness and applicable native-install checks.
- All parent-plan security, pagination, browser, and performance gates.
- Actual-plan comparisons, partition fan-out, exact-total cost, and maintenance
  concurrency results, with unchanged authorization and completed request counts.
- Relevant CLI tests, generated-client checks, Vitest, web typecheck/build,
  `cargo fmt --all -- --check`, and `cargo check --all-targets --workspace`.
- Owned runtime-log storage tests for range downloads and stream revocation.
- Awaited teardown with no run-owned database, listener, writer, or container leaks.

Operator live verification uses sanitized IDs, identical permissions/retention,
and matched workload on the deployed PostgreSQL major. Compare endpoint p50/p95,
first usable render, HTTP rate, pool waits, database reads/spills, retention backlog,
notifier load, and audit pressure. Document any local target not met; no historical
TimescaleDB or native protocol result certifies artifact-page performance.

Deploy backend/web and affected CLI consumers coherently. Keep a compatible
rollback set; document any index-build locks or migration constraints before
deployment. A populated pre-prod database must not be reset or reinitialized as
part of performance rollout.
