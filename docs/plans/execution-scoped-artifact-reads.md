# Execution-scoped artifact reads and shared-log scalability

Date: 2026-10-30
Status: minimal implementation complete in `bugfix/execution-scoped-artifact-reads`;
local targeted validation recorded below, release/load gates still open.
The user approved implementation of this plan, including its human metadata/content
and canonical cursor-contract decisions. The follow-up request authorizes commits,
GitHub pushes, a PR into `main`, and continued milestone work after required CI
validation. Release/load gates remain unverified until their recorded runs pass;
deployment is not authorized.

## Outcome

Opening one execution's stdout/stderr must not authorize every execution that has
ever contributed to the same action-level artifact. Browsing metadata must not
fail because that container has more than 256 retained executions. Returning a
small page must not require an exact count of the complete history.

Keep shared action-level artifact refs and existing log storage. The fix is in
selection, authorization, pagination, and browser requests, not a new log service
or one artifact container per execution.

This is a follow-up to [the original performance plan](artifacts-page-load-performance.md)
and [its implementation checkpoint](artifacts-postgresql-implementation.md).
Batching, summary projections, server pagination, refresh coalescing, and bounded
detail-page previews already exist. Do not repeat that work or treat batching
alone as the solution to the remaining history-dependent costs.

## 1. Findings and scope

At checkout `0eda2d0cc7a092a0467e79d1ad913d446f0dd261`, two independent real-route
runs on disposable PostgreSQL 16 reproduced:

| Distinct retained executions linked to one runtime-log artifact | Global list | Execution list | Detail | Version history |
| --- | --- | --- | --- | --- |
| 256 | Listed | Listed | 200 | 200 |
| 257 | Listed | Listed | 404 | 200 |

`ArtifactReads::authorize_detail_with_receipts` collects linked execution IDs and
passes them to `HumanReceiptGuard::for_executions`. Its aggregate 256-execution
limit rejects the request, and `mask_detail_denial` turns the denial into 404.
The version-history path does not take the same receipt-preparation path, which
explains the inconsistent result.

The execution-list/history paths still authorize all linked executions. They
batch reads, but their work grows with retained history. Exact eligible totals
also force a complete candidate scan. Separately, the global list performs an
exact SQL visibility count before fetching the requested page.

The production browser retry settings resend a 404 after approximately one
second. They inspect Axios-style `error.response.status`, while generated
`ApiError` exposes `error.status`. Existing artifact-hook tests mostly disable
retries, so they do not catch this production behavior.

The diagnostic source/evidence was retained under
`/tmp/opencode/artifact-bound-diagnosis-20261030/` and
`/tmp/opencode/artifact-diagnosis-20261030/`. These are local investigation
artifacts, not permanent test dependencies or deployed performance evidence.
The standard selection passed 41 backend tests; the web suite passed 537 tests
and typechecking. The separate diagnostic tests were intentionally red.

The global list was not substantially slow in the small local fixture. Its
deployment-specific latency still needs measurement; do not claim that the
257-execution bug explains every slow global-list request.

## 2. Approved security decisions for the structural changes

The existing all-linked-execution rule was documented behavior, not an accidental
implementation detail. The user's implementation request approves this replacement
for human Access-token reads in work packages C–F:

1. **Runtime-log header metadata:** use artifact metadata visibility, without
   authorizing bodies or preparing receipts for all historical executions.
2. **Execution artifact summaries:** require access to the requested execution
   and the metadata being returned. A runtime-log summary describes the version
   selected for that execution, not the artifact's latest version across runs.
3. **Runtime-log version summaries:** expose only versions passing the existing
   version metadata visibility predicate. A listed version is not a promise that
   its body can be disclosed.
4. **Content:** authorize the exact selected version and all of that version's
   required recorded sources. A denied sibling execution/version must neither
   grant access to this version nor prevent access to an independently readable one.
5. **Pagination:** replace mandatory exact totals on artifact browse paths with
   bounded cursor pages. This is an intentional v1 contract change.

The header allowlist should be explicit: `id`, `ref`, `scope`, `owner`, `type`,
`visibility`, `classification`, `name`, `created`, `updated`, `retention_policy`,
and `retention_limit`. Review this allowlist as policy; names and linkage are not
inherently harmless. Exclude arbitrary `description`, `data`, version `meta`,
storage paths, object keys, provider credentials, and source/provenance records.
Show content type and size from an independently visible selected-version summary,
not unconditionally from the latest version of the shared container.

Limit this first policy change to human runtime-log browsing. Ordinary artifact
`data`/progress reads retain their protected behavior. Recorded execution/sensor
credentials retain their existing typed consumer contracts and fail-closed
restrictions; a selector must never route them through human metadata access.
Do not unmark or enable currently denied recorded content routes as part of this work.

A policy-preserving aggregate reader could batch all history, but could not meet
the approved selected-version scalability goal. Do not disguise that limitation
by raising graph limits or extending ACL cache lifetimes.

## 3. Work packages

### A. Permanent reproductions and baselines — first

Files: `crates/api/tests/artifact_read_performance_tests.rs`, its fixture modules,
`crates/common/tests/repository_artifact_tests.rs`, and web hook/page tests.

- Port the local reproduction into owned repository fixtures. Cover 1, 256, 257,
  and 1,000 distinct execution links. Assert list-to-detail behavior explicitly.
- Characterize selection when one execution emits multiple versions: highest
  version for that exact `(artifact, execution)` pair, then pin its immutable ID.
- Add a readable recent execution plus an unrelated denied/missing old execution;
  under the approved policy, the recent version remains readable while the old
  version remains denied. Include real origin/Key identities and retained bindings.
- Add a web reproduction using the production QueryClient settings and generated
  `ApiError`, rather than a fixture with `retry: false`.
- Record global list, execution list, metadata, version history, preview, download,
  and stream authorization separately. Include query counts, rows, pool waits,
  authorization CPU, response bytes, and endpoint status.
- Preserve old authorization characterization until the replacement policy matrix
  is approved. Add the new expected behavior as a red regression before its fix;
  do not quietly change tests to accept a new disclosure outcome.

Acceptance: the new tests expose both the 256→257 defect and the retry delay;
baseline evidence identifies the exact tested source and owned-resource cleanup.

### B. Correct browser retry and error behavior — independently shippable

Files: `web/src/lib/query-client.ts`, `web/src/api/core/ApiError.ts` (reference,
not a hand edit to generated code), and focused query-client/artifact tests.

- Normalize HTTP status from generated `ApiError.status` and Axios
  `error.response.status` in one application-owned helper.
- Do not automatically retry permanent request/auth/not-found errors. Keep bounded
  retries for transport failures and selected transient server failures. Handle
  429 using the application's deliberate rate-limit policy rather than treating
  it as an unconditional immediate retry.
- Preserve explicit user retry and distinguish metadata failure from preview or
  download failure. Content denial must not blank an already authorized header.
- Assert one request and prompt error rendering for 400/401/403/404, including
  production artifact hooks; test cancellation and transient-retry behavior too.

Acceptance: a metadata 404 causes no delayed duplicate request. This package alone
does not fix backend 404s or history-dependent authorization costs.

### C. Put selection and disclosure behind one read module

Files: `crates/api/src/artifact_reads.rs`, `receipt_disclosure.rs`,
`secret_disclosure.rs`, `routes/artifacts.rs`, `routes/executions.rs`,
`routes/executions/streams.rs`, and the existing recorded-read helpers as needed.

Extend the existing `ArtifactReads` module rather than adding another RBAC engine.
Its small interface should own metadata reads, execution-page selection,
version-page selection, and opening one selected version. Callers must not choose
an authorization mode or assemble guard ordering themselves.

- Split runtime-log metadata visibility from content disclosure. The canonical
  human runtime-log detail response uses the reviewed header projection; it does
  not fetch a full artifact row and accidentally serialize `data` or descriptions.
- Resolve and pin the selected version before preparing its disclosure guard.
  Carry artifact ID, immutable version ID, version number, execution/generation
  identity, and backend/body identity internally through the read operation.
- For an execution selector, prove that the version belongs to that exact
  execution. Never fall back to another run or to artifact-global latest.
- Prepare receipts only for the selected execution and its actual bounded source
  graph. Keep the 256-node, depth, byte, cycle, freshness, owner-survival, and
  current-IAM protections. A large container is not one enormous receipt graph.
- Reuse the existing guarded execution SSE path
  `/api/v1/executions/{id}/logs/{stream}/stream` for execution stdout/stderr.
  It already resolves `(artifact, execution)`; verify every protected dispatch
  and per-frame reauthorization path stays selected-execution scoped.
- Update specific-version downloads, latest downloads, generic artifact streams,
  and encrypted-log chunk guards. In particular,
  `download_human_encrypted_log` currently calls aggregate artifact receipt
  authorization again for each guarded delivery; fixing only the initial handler
  would leave the same defect in transport.
- For “latest,” select the newest metadata-visible version once, then authorize
  that exact body. If body disclosure is denied, return the masked denial; do not
  scan older history looking for an accessible body. Users can select an older
  visible version explicitly.
- Preserve sensor-generation/delivery provenance for a selected sensor-log
  version. Do not apply the action-execution rule to sensor logs by assumption.
- Preserve denial masking for protected content and propagate repository/internal
  failures as real failures, not 404 or a successful empty list.

Acceptance: authorized headers, selected previews/downloads, and execution log
streams work beyond 257 links without inspecting unrelated execution origins.
Revocation, expiry, missing-source, and stream/chunk checks still deny delivery.

### D. Repository selection must not enumerate shared history

Files: `crates/common/src/repositories/artifact.rs`, relevant execution/schema/
origin repositories, `crates/api/src/dto/artifact.rs`, and `artifact_reads.rs`.

- Add explicit header and selected-version summary projections; authorization
  projections must exclude bodies, ciphertext, and arbitrary JSON metadata.
- Extend the execution artifact query to select its highest version for each
  `(artifact, requested execution)` pair. Return selected-version identity and
  version-specific display fields. Reuse existing predicates and repository
  selection logic; do not add ad hoc queries in route handlers.
- Remove `linked_execution_ids_many` and aggregate `readable_logs` traversal from
  the approved runtime-log metadata/list paths. Keep them only for callers whose
  retained contract genuinely requires aggregate authorization.
- Use existing `(artifact, execution)` and `(artifact, version DESC)` indexes
  first. Capture actual repository EXPLAIN plans at realistic cardinalities before
  proposing indexes or changing their ordering.
- Keep dangling execution IDs meaningful. Missing evidence denies the affected
  content read; never add retention-coupling foreign keys or treat a missing
  execution as proof of access.
- Avoid holding one pool connection for a guard while an unbounded number of
  unrelated history operations acquire more connections.

Acceptance: adding unrelated runs changes neither the selected version nor the
number of execution-root disclosure reads for a fixed requested execution.

### E. Remove exact totals and deep offsets from the browse hot paths

Files: artifact DTOs/repositories/routes, generated clients, CLI artifact commands,
`web/src/hooks/useArtifacts.ts`, and artifact-specific pagination controls.

Adopt one canonical v1 cursor-page response for the affected artifact collections:
`{ items, next_cursor, has_more }`. Global and execution artifact ordering uses
`(created DESC, id DESC)`; history uses `(version DESC, id DESC)`.

- Fetch at most `per_page + 1` SQL-visible metadata rows; enforce page sizes 1–100.
  Do not compute exact totals or traverse body-disclosure graphs to fill a page.
- Bind opaque validated cursors to the endpoint, execution/artifact scope, filters,
  and ordering. Reapply current authorization on every request; a cursor is not a
  permit. Reject cursor/filter mismatches and malformed values.
- Eliminate mandatory `COUNT(*)` from global/history browsing and the exact-total
  scan from execution runtime-log summaries. Do not label candidate counts or
  estimates as exact eligible totals.
- Do not add a silent post-query content filter that scans arbitrarily many denied
  rows. Content eligibility is checked when opening the selected body. Any other
  unavoidable post-filter must have an explicit scan budget and resumable cursor,
  including empty resumable pages, rather than falsely reporting end of results.
- Use next/previous navigation with a client-side cursor stack; remove page numbers
  and totals the server no longer provides. Preserve pause/resume, reconnect,
  completion refresh, and return-to-first-page behavior on relevant live changes.
- CLI full-history output traverses cursors deliberately; it must not truncate to
  the first page. Update typed consumer contract tests.
- Adapt recorded collection consumers to the same canonical cursor response,
  retaining their existing typed authorization and per-artifact graph limits.
  Use bounded candidate scans and resumable empty pages where authorization must
  post-filter candidates. They do not inherit the human selected-version policy
  or its scalability claim; verify this separately rather than keeping a second
  response shape or routing recorded callers through human visibility predicates.
- If operators need exact historical counts later, design a separate explicitly
  expensive/summary-backed reporting operation. It must not run automatically on
  page mount or notification refresh and is not required for this fix.

Cursor pagination avoids growing offsets, not all PostgreSQL work: scoped
visibility predicates may still examine many candidates. Measure rows examined
and actual plans; do not promise constant database time from a bounded response.
Concurrent insertion/retention is not a multi-request snapshot. Define and test
the browse/reset behavior without claiming stable historical snapshots.

Acceptance: first-page and next-page reads issue no exact-count query and perform
no disclosure checks for unrelated execution roots.

### F. Make the web consistently request the selected run/version

Files: `web/src/hooks/useArtifacts.ts`, `useArtifactStream.ts`,
`useArtifactReconciliation.ts`, `web/src/lib/artifact-refresh.ts`,
`web/src/pages/artifacts/ArtifactDetailPage.tsx`, `ArtifactsPage.tsx`,
`artifactHelpers.tsx`, and `ExecutionArtifactsPanel.tsx`.

- Consume generated DTOs, including classification and selected-version fields,
  instead of maintaining drifting hand-written artifact response shapes.
- Include execution/version selection in query keys and navigation. An execution
  page's preview/download must use that execution's selected version, never
  `/artifacts/{id}/download` or generic latest-stream fallback.
- Render the safe header independently of history and body requests. Avoid a
  metadata→history dependency when history only needs the route artifact ID and
  has its own authorization; do not duplicate header authorization in every row.
- Keep the existing 64 KiB detail-page preview. Apply the same bounded range/read
  helper to the execution artifact panel, which currently consumes the whole
  completed text file with `response.text()`. Use the existing execution SSE
  reader for running stdout/stderr; preserve reconnect cursors and cancellation.
- Label “latest,” execution-selected, and explicitly selected version views
  distinctly. On content denial show “Content unavailable or access denied” while
  retaining authorized metadata; do not reveal private denial reasons.
- Keep shared refresh coalescing and loss-recovery reconciliation. For pinned
  execution views, unrelated runs of the same action must not invalidate their
  content/history unnecessarily. Global headers may still need refresh on changes.
- Do not introduce persistent authorization-result caches. Per-frame revocation
  checks remain authoritative even if header/query results are cached.

Acceptance: with executions A and B of the same action, every preview, download,
and stream opened from A displays A, including after B produces a newer version.

### G. Validation, diagnostics, and rollout

Promote a routine regression suite for 256/257 and 1,000 links. Keep the large
profiles explicit: 100,000 and 604,800 retained executions with two shared log
artifacts (up to 1,209,600 log versions). Seed through owning repository/fixture
helpers using real admitted schema/owner evidence; do not fabricate authorization
proofs or wait seven days to build the fixture.

At each scale, measure a fixed page of 20 and a fixed selected execution, with:

- Unrestricted and scoped human grants, readable and denied versions, shared and
  distinct secret origins, workflow lineage, deleted sources, and expired receipts.
- First-after-auth-cache-invalidation samples separately from warm samples, at
  least 30 complete warm samples, and both one and four concurrent readers.
- Continued one-per-second `core.echo` writes during reads, retention/cleanup
  backlog, late stream allocation, concurrent publication, and version deletion.
- A real browser full-stack soak crossing 257 executions at one execution/second;
  test both the global artifact page and an execution's artifact panel/detail.
- Object-backed and shared-file delivery, protected log decoding, sensor-log
  selection, and revocation between chunks/frames. Retain recorded-token denials.

Predeclare these release gates before treatment measurements:

1. No unexpected 404 at 257, 1,000, 100,000, or 604,800 links for an authorized
   runtime-log header or a readable selected version.
2. Zero unrelated-execution provenance/binding loads for runtime-log metadata,
   history summaries, or one selected body; security work follows only that body's
   bounded source graph. Growing retained sibling history adds no such queries.
3. No mandatory count query, whole-history materialization, body/ciphertext
   projection, or increasing application-memory retention for fixed metadata pages.
4. On fixed hardware/data/permissions, warm selected-read p95 at 100,000 siblings
   stays within 2× its 1,000-sibling baseline, excluding neither failed requests
   nor pending work. Record absolute latency; a passing ratio alone is insufficient.
   Set an operator-approved absolute latency SLO before rollout, using browser and
   API measurements rather than inferring it from local in-process timing.
5. No wrong-execution content, weaker disclosure decision, stale permitted stream
   frame after revocation, leaked reader, or unowned fixture cleanup.
6. Continued one-per-second writing has no growing queue/maintenance backlog
   attributable to these reads; record throughput, storage growth, waits, and WAL.
   Read-path success is not certification of arbitrary write/storage capacity.

Add low-cardinality internal diagnostics for selection, candidate/returned rows,
source-root count, SQL count, pool wait, and stage latency. Distinguish graph-limit,
missing-source, visibility, and infrastructure failures internally. Never expose
these private reasons to unauthorized callers or log raw origins, SQL secret
binds, tokens, artifact bodies, or decrypted material.

Run targeted repository/API/web regressions, the owned benchmark runner, and the
runtime-log correctness suite according to `docs/testing/running-tests.md` and
`docs/deployment/runtime-log-verification.md`. Extend the benchmark runner for the
new contract; do not treat its old response/count assertions as the new acceptance.
Run applicable `.github/workflows/` validations on the exact implementation state,
including formatting, warning-free workspace check/Clippy, required Rust/service
tests, web tests/typecheck/build/lint, and generated-client checks. A plan-only
documentation change does not claim those implementation gates have run.

Update `docs/api/artifact-reads.md`, permission documentation, the original policy
matrix, OpenAPI, in-tree generated clients, CLI, and SDK migration notices together.
Run `npm run generate:api` and `npm run check:api` from `web/`; verify generated
version fields match the workspace version. Remain on canonical v1: no v2 format,
dual readers, or compatibility adapters. If evidence justifies schema/index
changes, review the migration policy and run `cargo sqlx prepare`; no database
reset, volume replacement, or checksum rewrite is part of this plan.

Deploy matching backend/web/CLI consumers together. Verify the actual deployed
revision, a formerly failing high-history artifact, browser first usable render,
and API/database pressure under the operator's workload. Keep a matching previous
release set for rollback; a coordinated rollback also restores the old limits.
The user's follow-up authorizes commits, pushes, and a PR after required validation;
this plan does not authorize deployment or waive validation gates.

### Implementation validation checkpoint (2026-10-30)

The minimal source implementation covers metadata/header projection, selected
version authorization and immutable-ID assertions, confidential count-free
cursors, web/CLI/MCP consumers, generated clients, and credential-specific guarded
delivery. Independent static review identified and then re-reviewed fixes for
guarded-pool reacquisition, legacy credential dispatch, recorded acquisition
ordering, and owner-bound live-log refs; it found no further concrete blocker.
The final live-seal reconciliation was also reviewed: it refreshes by immutable
ID and retains source/IAM guards rather than substituting a sibling/latest body.
Static review is not dynamic test evidence.

Verified locally so far: 568 web tests; TypeScript and generated-contract checks;
ESLint, Prettier, production web build, 23 API-generator tests; four Python client
contracts; 197 CLI-library/MCP tests (including 16 artifact tests); benchmark/report
helper tests and isolation guards. OpenAPI/client drift checking passes and
generated-client versions match workspace version `0.10.1`. Rustfmt, warning-free
`cargo check --all-targets --workspace`, and all-target/all-feature Clippy with
`-D warnings` pass.

Real, disposable PostgreSQL validation on the final production sources:

| Suite | PostgreSQL 18 | PostgreSQL 16 |
| --- | ---: | ---: |
| Header/content, including live sealing and freeze | 10 passed | 10 passed |
| Cursor API and effective signing key | 2 passed | 2 passed |
| Single-connection Access/legacy/sensor delivery | 2 passed | 2 passed |
| Read/source-frontier regressions | 42 passed | 42 passed |
| Key/sensor disclosure | 7 passed | 7 passed |
| Recorded execution/event consumers | 156 passed | not rerun |
| Execution-token permissions | 4 passed | not rerun |
| Web form wire contracts | 6 passed | not rerun |

Additionally: two repository cursor tests, three cursor unit tests, 11 API
versioned-RustFS/shared-file correctness tests, and the S3 direct-upload proof
pass. Storage runs exclude the explicit load report and subprocess-only lock
helper, not correctness cases. The read suite's two explicit workload profiles
and inequivalent paired reference remain ignored; four recorded RabbitMQ
publication cases require their dedicated broker lane and were not run. None
of those omissions counts as passing release evidence. Owned infrastructure and
fixture databases were torn down without cleanup errors.

At 1/256/257/1,000 retained siblings, the routine PostgreSQL profile returns 200
with constant 10 application statements for the execution metadata page and 25
for its fixed selected body, and zero linked-history/count queries. Selected
source-frontier tests retain constant within-batch application-query slopes and
explicit additional-frontier bounds; their absolute ceilings now include current
IAM, receipt/owner locks, and immutable-version validation rather than comparing
different security work. These are query/correctness results, not latency SLOs.
Existing summary EXPLAIN probes cover the global-with-execution-filter builder,
not a large-scale certification of the execution-specific lateral selection.
The owned benchmark runner also passes its PostgreSQL 18 treatment run with
Key-usage tests: both executables exit zero, the source fingerprint stays
unchanged, and owned infrastructure cleanup succeeds. Evidence is retained at
`/tmp/opencode/artifact-selected-owned-benchmark-final-pg18-20261030/evidence.json`.
This routine run does not enable the blocked paired reference or certify the
explicit workload profiles, browser latency, or large-scale release gates.

The initial API/storage runs failed and some detected concurrent source changes;
they are retained as diagnosis, not passing evidence. Stable-source reruns covered
the fixes. The broad runs' last failure was one stale artifact-denial expectation
after sensor revocation; the test-only 403-to-masked-404 correction was rerun in
the complete Key suite on both PostgreSQL versions. No production source changed
after the passing broad selected-read/recorded-consumer/storage runs.

Still unverified: 100,000/604,800 retained-execution profiles, 30-sample latency
ratios/absolute SLOs, one-Hz full-stack browser soak, sustained-write backlog,
full workspace/service CI lanes, external SDK publication, and actual deployment.
The old `--paired-reference` benchmark is deliberately blocked because its
aggregate-disclosure policy is not equivalent to selected-content authorization.
No schema/index migration, database reset, commit, push, or deployment was made.

## 4. Recommended delivery order

1. **A + B:** permanent reproductions, measured baseline, and independent retry fix.
2. **Policy approval:** header allowlist, version-scoped disclosure, latest-denial
   behavior, credential scope, and cursor/no-total contract.
3. **C + D:** one selected-version read module and repository-backed selection;
   preserve transport revocation and all selected-source checks.
4. **E + F:** publish the canonical browse contract and update every consumer in
   the same coordinated change. Do not ship mismatched backend and web contracts.
5. **G:** scale/security/soak gates, matched deployment evidence, then rollout.

Do not substitute shorter retention, suppressed logs, a larger 256 limit, larger
pools, cached allow decisions, or more parallel per-execution queries for these
fixes. Those can hide symptoms without removing the coupling to shared history.
