# Artifacts page-load performance

Date: 2026-10-14

Status: implementation in progress. See the linked implementation checkpoint;
there are no live pre-prod measurements or full-plan performance certification.

Follow-up: [Execution-scoped artifact reads](execution-scoped-artifact-reads.md)
records the 2026-10-30 reproduction of the 257-execution detail failure and the
user-approved metadata/content and cursor-pagination changes. The matrix below
records the original baseline contract, not the replacement policy. See
[Artifact reads](../api/artifact-reads.md) for the new canonical contract.

For implementation, follow the file-level work units and PostgreSQL-specific gates
in [Artifact reads on stock PostgreSQL](artifacts-postgresql-implementation.md).
This document remains the source of truth for endpoint policy and acceptance.

## Goal and scope

Reduce first-load latency and refresh load for artifact lists, execution artifact
panels, and artifact details. Start with the reported slow endpoints:

- `GET /api/v1/executions/{execution_id}/artifacts`
- `GET /api/v1/artifacts/{id}`

Also cover their adjacent requests: the global artifact list, version history,
automatic text previews, and notification-driven refreshes.

The initial investigation had no pre-prod access. On 2026-10-15 the installed
system-wide MCP client reported local and server SHA
`a354b9cf239eefd3a15d8589ac61bf65b8c50747`. That client is not the development build
being tested here. Its local compiled version must not identify this checkout or
substitute for independently confirmed API/web deployment fingerprints. Installing
a development MCP server is a separate task, not part of this change.
The checkout is based on `0954b110d7369c1eb8649155da0facd5adc326ac` with uncommitted
changes. Database/operator measurements remain unavailable; installed-client build
discovery is not a live performance comparison.
Preserve those changes and do not commit agent work.

## Findings to address

1. The execution-artifacts route loads an unpaginated list and authorizes rows
   serially. Each row reloads the caller's identity and performs a readability
   query, even with a warm grant cache.
2. Runtime-log authorization enumerates every retained linked execution, loads
   full execution rows, and loads input/output secret records separately for each
   execution. The current working tree also loads and validates historical schema
   bindings. Secret-origin authorization can add recursive reads.
3. Action-level log refs such as `mypack.deploy.stdout.log` share one artifact
   across runs. A metadata request for one execution can therefore authorize many
   historical executions. Stdout and stderr repeat checks for the same executions.
4. Summary queries select full artifact JSONB data and discard it after decoding.
   The execution query also deduplicates full rows through `SELECT DISTINCT`.
5. Live notifications invalidate list, detail, version, and execution queries
   without coalescing. Running execution views also poll every three seconds.
6. Version history is unbounded at the database/API level and only truncated in
   the browser. Text previews automatically download and render the entire body.
7. The global list applies SQL visibility only; execution lists and detail reads
   also enforce runtime-log disclosure. Consolidating their implementation must
   preserve this endpoint-specific distinction.
8. Recursive secret origins and workflow ancestry add reads beyond directly linked
   executions. Deduplicating shared sources does not batch distinct sources.
9. Browser WebSocket connectivity does not prove complete notification delivery.
   Notifier broadcast lag and PostgreSQL listener reconnects can lose updates while
   browser sockets remain connected; artifact deletion has no creation/update event.

Neither reported endpoint directly reads a file body or calls object storage.
Database reads and authorization CPU are the first investigation targets. Storage
is relevant to the separate download requests. Notification authorization and
asynchronous API audit writes contribute additional database load.

## Security contract and approval gate

The first implementation must preserve current authorization outcomes. In
`crates/api/tests/key_usage_tests.rs`,
`execution_decrypt_does_not_override_recorded_key_disclosure_authority` explicitly
expects an artifact detail GET to return 404 until the caller receives the
recorded key's decrypt permission. Removing disclosure checks from metadata GETs
is therefore a policy change, not just a performance refactor.

Use these documents and existing tests when reviewing the contract:

- [Permission delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md)
- [High-level permissions](../permissions/permissions-high-level.md)

Preserve execution-linkage and owner-path rules, private workflow ancestry,
execution-token authority, and dangling execution-ID behavior. Preserve version
visibility and latest-readable-version selection. Never grant access through
missing origins, expired bindings, unknown provenance, or cycles.

### Endpoint policy matrix

Characterize these current behaviors before replacing the read paths. Visibility
means the existing repository readability predicate, not body disclosure.

| Read path | Current authorization | Selection and denial behavior to preserve |
| --- | --- | --- |
| Global artifact list, including its execution filter | SQL visibility only | Visibility-filtered rows and totals; no added runtime-log disclosure filter |
| Execution artifact list | Visibility plus all-linked-execution disclosure for runtime logs | Filter genuinely denied artifacts; infrastructure failures become request failures |
| Artifact detail by ID or ref | Visibility plus all-linked-execution disclosure for runtime logs | Denied reads remain 404 |
| Version history | Existing artifact detail gate, then version visibility | Return only visible versions; denied artifact remains 404 |
| Specific/latest version detail | Existing artifact detail gate, then version visibility | Preserve selected-version and latest-readable-version behavior and denial masking |
| Downloads and streams | Existing artifact gate plus route-specific recorded-source checks | Preserve version selection, sensor-generation checks, and live revocation checks |

Add contract tests for each row across supported token types. Explicitly cover a
caller who can see a runtime-log summary in the global list but cannot read its
detail or see it in an execution list. The shared module must expose named,
endpoint-specific read operations; authorization mode is not caller-controlled.
Keep the current difference until a separately approved policy change resolves it.
Characterize content-route checks individually rather than assuming that every
metadata, download, and streaming route has the same authorization sequence.

Recommended sequence:

1. Batch and reuse authorization work without changing who can read an artifact.
2. Separately decide whether metadata visibility should require body disclosure
   authority. Do not ship that change without explicit approval and contract tests.

If metadata-only access is approved, define a safe field allowlist first. Names,
descriptions, JSONB `data`, version metadata, and linkage may reveal information;
do not assume every field is harmless. Keep body/progress disclosure separately
authorized. Downloads, JSON version content, direct execution logs, and SSE must
still authorize the selected version's recorded source. Preserve live revocation
checks before exposing stream frames or guarded download chunks. Do not simply
delete the shared authorization call from all routes.

## Phase 1: establish a reproducible baseline

Build an agent-runnable benchmark through the real Axum routes and repository
layer using disposable, run-owned PostgreSQL fixtures. Reuse the API test context
and existing test runners; do not create another deployment stack.

Fixture dimensions:

- 2, 20, and 100 artifacts attached to an execution.
- Two action-level runtime-log artifacts linked to 1, 100, and 1,000 retained
  executions. Explicitly configure retention so fixture versions survive.
- Non-sensitive executions, repeated shared secret origins, and distinct origins.
- Distinct execution, event, and inquiry origin chains at fixed depths, plus many
  sensitive workflow children with shared and distinct parent chains.
- A shared provenance subtree reached both directly and through a chain near the
  current depth limit; cyclic graphs and traversal-order permutations.
- Small and large progress JSONB bodies, including a 10 MiB fixture.
- Mixed readable/unreadable executions, private workflow children, and expired
  linked executions.
- Cold and warm authorization caches and more than one concurrent browser client.

Record response status, first-byte and completion time, response bytes, database
statement counts, pool-acquisition wait, and authorization-stage time. Distinguish
query execution from JSON decoding and schema/provenance validation CPU. Count
handler queries separately from audit-writer and notifier queries.

Use request-scoped, redacted tracing or a test-owned query observer at the
repository/database instrumentation seam. Do not add ad hoc application SQL or
log secret records, credentials, artifact bodies, or raw parameter values. If
plans are needed, inspect the SQL actually generated by repositories against the
owned fixture, not an approximate handwritten replacement.

Save baseline artifacts under a run-owned directory in `/tmp/opencode`. Make the
benchmark fail on predeclared query-growth and latency thresholds. Require a red
baseline before claiming a fix.

For pre-prod, request the deployed revision and operator-collected, sanitized
timings for the affected IDs. Compare API audit durations with browser TTFB and
download time. Collect retained linkage counts and database query/pool evidence
without copying secret data. Local reproduction can proceed before access exists;
live root-cause confirmation cannot.

## Phase 2: batch authorization without changing access

Create one request-scoped artifact read module with a small interface for detail
and collection reads. Keep visibility and disclosure implementation inside it,
not duplicated across route handlers. Reuse the existing authorization snapshot
and repository predicates rather than introducing a second RBAC evaluator.

Implementation steps:

1. Load caller attributes and effective grants once per request. Preserve the
   fresh delegated-authority checks required for execution tokens.
2. Add repository collection reads that apply the existing readability predicate
   in SQL. Extend `ArtifactRepository::search_readable` and its existing execution
   `EXISTS` filter with narrow projections instead of adding a parallel search
   implementation. SQL visibility replaces per-artifact `is_readable` calls, not
   the execution list's additional disclosure gate.
3. Collect runtime-log linkage in batches. Deduplicate execution IDs across
   stdout, stderr, and other artifacts in the same request.
4. Add narrow execution authorization projections. Avoid loading config, result,
   environment maps, and full executable snapshots unless a disclosure check
   actually needs them. Determine required fields from the real call sites.
5. Add provenance-only secret projections containing entity kind/ID, JSON path,
   source kind, and source ref. Authorization must not select encrypted values or
   encryption-key hashes that it does not use. Fetch input and output secret
   records together in one repository query per execution batch, keyed by entity
   kind and ID. Batch historical schema-binding reads and validate each binding
   once per request; current action metadata cannot replace a historical binding.
6. Batch recursive origins by traversal frontier. Group newly discovered source
   IDs by entity kind, batch their authorization records, bindings, and provenance,
   and deduplicate before advancing. Cover execution, event, inquiry, component
   configuration, and sensor-generation/delivery sources supported by
   `origin_allows`; batch required owner-survival checks too. Query growth may
   follow entity kinds, batch count, and graph depth, but not one query per source
   within a frontier batch.
7. Batch workflow visibility anchors and ancestor-executor resolution for execution
   sets through repositories. Replace parent-by-parent full-row loads and one
   recursive anchor query per child. Preserve owner/self/descendant constraints,
   ancestry limits, and dangling-parent behavior.
8. Separate request-local record caches from authorization-result caches. Key
   decisions by caller, source identity, entity kind, ID, path, action, and any
   traversal context needed for correctness. Preserve the current provenance
   cutoff at depth 32 even on cache hits. Reuse an allow only when its complete
   subtree fits the remaining depth budget and does not intersect the active
   traversal. Cycle- or depth-dependent denials are not context-free cache entries.
   Keep cycle detection local to each traversal; do not reuse decisions across
   callers, requests, or stream frames.
9. Chunk large ID sets at an explicit repository batch size. Do not substitute
   unbounded parallel queries for batching.
10. Preserve the current all-linked-execution disclosure condition until the
    metadata policy gate is approved. Batch all required records, not merely the
    execution named in the URL.
11. Stop treating infrastructure errors as invisible artifacts. Filter genuine
    access denials; propagate database/internal failures as request failures while
    keeping existing not-found masking for denied detail reads.

Main files:

- `crates/api/src/routes/artifacts.rs`
- `crates/api/src/routes/executions.rs` for existing visibility helper call sites
- `crates/api/src/authz.rs`
- `crates/api/src/secret_disclosure.rs`
- `crates/common/src/repositories/artifact.rs`
- `crates/common/src/repositories/execution.rs`
- `crates/common/src/repositories/execution_secret_value.rs`
- `crates/common/src/repositories/schema_contract.rs`

Extend source-specific repositories for recursive batch reads as needed; keep their
survival and binding checks authoritative rather than reimplementing them in the
artifact module.

Do not extend authorization cache TTLs or persist allow decisions across requests
as a shortcut. All database access stays in repositories.

## Phase 3: make list and history reads bounded

1. Introduce repository summary projections with explicit select-column constants
   that exclude artifact `data`, descriptions, and version content/metadata not
   returned by the response. Apply SQL visibility in the query and complete any
   remaining disclosure checks before returning projected results.
2. Replace the execution-artifact join plus full-row `DISTINCT` with an indexed
   `EXISTS` linkage predicate. Preserve ordering by `created DESC, id DESC`.
3. Add server-side pagination to execution artifacts and artifact versions.
   Update DTOs, OpenAPI, generated clients, and the web consumers together. Use
   the existing v1 page/offset contract; no legacy reader or v2. Define eligibility
   and totals as described below before applying pagination.
4. Make the panel show the first page and offer explicit access to further pages.
   Preserve counts and ensure the execution progress indicator does not depend
   on a progress artifact appearing in the first page. Give it a filtered,
   bounded summary query if necessary.
5. Derive execution-list rows and exact totals from the same disclosure-eligible
   candidate scan. Reuse batched records and decisions for both, without a second
   authorization pass. Global-list counts retain SQL visibility semantics. Add
   version-history counts using the existing version visibility predicate after
   the artifact gate, rather than using unfiltered `count_by_artifact`.
6. Examine existing execution and artifact/execution indexes before adding any.
   Add an index only with query-plan evidence. Never reset pre-prod databases or
   rewrite applied migration checksums. Any approved schema change requires
   migration review and `cargo sqlx prepare`.

### Eligibility and exact totals

For execution lists, read SQL-visible candidate authorization projections in
bounded, ordered repository batches. Apply the preserved disclosure gate, count
eligible artifacts, and retain only the requested eligible page's summaries.
Advance the internal scan by the ordering key rather than repeatedly using growing
offsets. Do not apply the public page offset before disclosure filtering. A denied
candidate batch must not hide readable artifacts later in the scan.

Expose a count-free repository scan using the shared search predicates. Calling
the public count-plus-page search for every internal batch would repeat a full
visibility count without producing the required disclosure-filtered total.

Exact filtered totals require evaluating all candidate artifacts, even for the
first page. Each runtime log can also require linked history outside the displayed
page. This bounds response size and individual database batches, not total request
authorization work. Measure candidate counts, linked/source records, memory, and
the exact-total cost separately. Changing to optional totals or `has_more` requires
a separately reviewed API/UI contract decision, not silently counting visibility
alone. Preserve existing concurrent-mutation semantics; do not claim a database
snapshot across separate page requests.

### Live pagination semantics

Initial implementation retains page/offset pagination. Ordering is deterministic
and pages are nonduplicated only while the eligible dataset is unchanged. Inserts
and retention can shift offsets, so this is not stable historical browsing.

On an affected live invalidation or reconciliation refresh, reset the view to page
one and discard older cached page results before presenting refreshed rows. Keep
the pause control and explain that resuming live updates resets pagination. Test
insertion and retention deletion between page requests and the reset behavior.
If stable browsing during mutation becomes a requirement, review a keyset cursor
and browsing-boundary contract instead of promising it from offset pagination.

## Phase 4: control browser refresh and preview work

In `web/src/hooks/useArtifactStream.ts` and `web/src/hooks/useArtifacts.ts`:

- Coalesce invalidations per QueryClient and affected query key into one flush
  per 500 ms window. Use one scheduler shared by mounted subscribers so the
  execution panel and progress bar do not each schedule a refresh.
- Avoid redundant prefix invalidation of version queries already covered by a
  detail-prefix invalidation. Prefer precise invalidation keys.
- Do not repeatedly abort/restart an in-flight refresh on each notification.
  Record a dirty flag and issue one trailing refresh after it settles.
- Make notifications the normal update path, but retain low-frequency
  reconciliation polling while connected. Initially use a 30-second interval for
  active, unpaused artifact views and the existing three-second fallback cadence
  for running views while disconnected. WebSocket connectivity alone cannot
  detect notifier broadcast loss, PostgreSQL listener reconnect gaps, or deletions
  without notifications. Refresh after reconnect and perform a final refresh on
  execution completion. Route polling and notifications through the same in-flight
  coordination; paused views remain paused.
- A future reliable delivery-gap/resynchronization signal may replace connected
  reconciliation polling only with loss-recovery tests. Browser reconnection is
  not a substitute for detecting notifier-side gaps.
- Wire query cancellation to the underlying request on navigation/unmount.

In `web/src/pages/artifacts/ArtifactDetailPage.tsx`:

- Load bounded version pages from the server instead of fetching everything and
  slicing the array afterward.
- Bound automatic text preview to 64 KiB using existing byte-range support.
  Clearly label truncated previews and keep an explicit full-download action.
- Abort previews on unmount. For text ranges, use UTF-8-safe decoding and do not
  interpret a truncated multibyte sequence as a file error.
- If a provider ignores the range, cap browser consumption and stop the stream;
  do not silently buffer the full body.

Coalescing reduces browser HTTP load but not PostgreSQL notifications themselves.
Measure notifier authorization and audit-writer activity separately. Only pursue
notifier-side batching if those measurements justify it; preserve per-identity
delivery authorization and existing audit coverage.

## Phase 5: optional metadata/body policy change

This phase is blocked on explicit security-contract approval, not on a benchmark.

If approved, route safe artifact metadata through visibility authorization only.
Return content-bearing fields through a separately authorized path, and scope
body disclosure to the actual selected version. Update permission documentation,
the existing metadata-denial test, and every affected caller in the same change.

Add tests proving that metadata visibility never unlocks content, version
existence outside the caller's scope, recorded secret values, or private workflow
context. Until that contract is agreed, ship the policy-preserving improvements
from phases 2 through 4 independently.

## Validation and acceptance

### Correctness

- Compare existing and optimized authorization decisions across identity,
  pack/action ownership, visibility, constrained grants, and execution-token cases.
- Cover every endpoint policy-matrix row, including global-list visibility of a
  runtime log denied by execution-list/detail disclosure checks.
- Cover private workflows, mixed-access histories, denied latest versions,
  missing linked executions, missing bindings, multi-origin secrets, and cycles.
- Retain key-origin and sensor-generation revocation tests, including streaming
  revocation between frames/chunks.
- Verify denied detail reads retain not-found masking; repository failures do not
  become successful empty lists.
- Verify provenance results are independent of traversal order. Exercise shared
  subtrees reached at different depths, the depth-32 cutoff, and cycle-dependent
  denials without reusing context-dependent decisions.
- Verify unchanged datasets paginate deterministically without duplicates and
  return endpoint-appropriate exact totals. Cover denied leading candidates with
  readable artifacts later in the scan, and progress beyond the first page.
- Verify inserts and retention between page requests follow the documented live
  reset behavior, including paused browsing and resume.
- Verify summaries neither select nor return large `data` or content fields.
- Verify authorization-only secret queries exclude ciphertext and key hashes.
- Test notification bursts with fake timers, duplicate mounted subscribers,
  in-flight requests, reconnect, pause, navigation, and execution completion.
- Simulate notifier message loss while the browser socket stays connected and
  deletion without an artifact event. Reconciliation must restore current data;
  overlapping polling and notification refreshes must not duplicate requests.
- Verify large text previews are bounded and full downloads remain available.

Extend `crates/common/tests/repository_artifact_tests.rs`, the API authorization
suites, and focused web hook/page tests. Use real repository/API tests for query
growth, not mocked database calls. Reuse owned fixtures and await teardown.

### Performance

Predeclare these targets before the treatment benchmark:

- For two non-sensitive log artifacts with 100 linked executions each, reduce
  handler database statement count by at least 90% from the measured baseline.
- Within a repository batch, increasing linked executions must not add one query
  per execution. Expect growth in rows and validation CPU, not round trips.
- At fixed graph depth and within each entity-kind frontier batch, increasing
  distinct secret origins must not add one query per origin. Record per-kind batch
  sizes and expected query-count bounds before running the treatment benchmark.
- Increasing sensitive workflow children within a repository batch must not add
  one anchor query per child or full-row reads per ancestor. Include shared and
  distinct ancestry fixtures and query-growth assertions beyond non-sensitive logs.
- Input/output provenance requires one combined read per execution batch. Large
  encrypted values must not increase authorization-projection transfer bytes or
  require ciphertext JSON decoding.
- Increasing unrelated progress JSONB from small to 10 MiB must not increase
  summary transfer bytes or require its application-side decoding.
- After settlement, a 100-notification burst within one coalescing window causes
  at most one refresh per affected active query. Notifications during an in-flight
  refresh cause at most one trailing refresh for that window.
- Automatic text preview consumes at most 64 KiB of body data.
- On the pinned local fixture, target at least a 70% reduction in p95 latency for
  the history-heavy execution-artifacts case. This is a proposed target, not a
  measured result or a pre-prod SLA.

Compare identical data, permissions, cache states, concurrency, and completed
request counts. Include cold/warm results, returned rows, errors, CPU, and pool
wait. Do not count cancelled requests or omitted authorization as speedups.

Run focused owned Rust integration suites, relevant Vitest tests, web type-check
and build, `cargo fmt --all -- --check`, and
`cargo check --all-targets --workspace`. Follow
[Running tests](../testing/running-tests.md) and the existing runtime-log storage
harness when exercising object-backed downloads and streams.

## Delivery and live verification

Implement in separately reviewable units:

1. Baseline benchmark, tracing, and security characterization tests.
2. Request-scoped authorization with direct, recursive-origin, and ancestry
   batching, preserving endpoint policies and traversal limits.
3. Narrow projections, disclosure-eligible execution pagination and exact totals,
   version-history pagination, and coherent web clients.
4. Refresh coalescing, reconciliation/fallback polling, live page resets, and
   bounded previews.
5. Optional approved metadata/body authorization contract.

Build backend and web assets from the same revision for contract changes. Deploy
only after local correctness and performance gates pass. Have the operator
repeat the sanitized pre-prod measurements with the same affected IDs, workload,
permissions, and retention. Compare endpoint p50/p95, first usable page render,
request rate, database/pool load, notifier activity, and audit-writer pressure.

Do not disable audit, retention, or disclosure checks, restart services, or change
pre-prod configuration during investigation without operator approval. Retain a
compatible backend/web rollback pair. If a migration is needed, document its
rollback constraints before deployment and do not undo it by deleting data.
