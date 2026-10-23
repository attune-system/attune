# Self-contained pack parallel implementation

Date: 2026-10-15

The user authorized parallel implementation, local commits in task worktrees,
and integration into the current branch without commits in the primary checkout.
The [SOW](self-contained-pack-statement-of-work.md) remains the behavioral contract.
The [progress tracker](self-contained-pack-implementation-progress.md) records
delivered behavior and validation, not merely completed agent assignments.

## Integration rules

1. Start task worktrees from an isolated snapshot commit containing the current
   tracked and untracked source changes. The primary checkout remains untouched
   by snapshot commits. Preserve unrelated baseline work in every worktree.
2. Commit only task changes after the snapshot. Report commit IDs, changed files,
   exported interfaces, migrations, tests, and exact remaining acceptance gaps.
   Keep each task worktree clean at handoff. Do not push.
3. Own the files below. Put new shared helpers in narrowly named modules when
   possible. Changes to another stream's owned files require an explicit handoff
   or an integrator-resolved patch. Do not update this coordination document,
   the main progress tracker, or generated clients independently.
4. Use PostgreSQL 18 only. Run focused tests during development. The integrator
   owns the final combined workspace, migrations, generated clients and
   cross-service acceptance pass. Each database test invocation needs a unique
   run ID and at least four test threads. Own and clean up test resources.
5. Preserve execution-input authority and historical secret-origin checks.
   Empty runtime selections stay credential-free. Recorded credentials never
   fall back to human IAM. Ordinary disablement and explicit revocation remain
   distinct. Approval issuer/requester activity is not runtime authority.
6. Deployment exemptions and recorded worker issuance activate only after
   admission and every required consumer agree. No partial workstream may
   independently enable this switch or weaken a failing closed path.

## Workstreams

| ID | Ownership and deliverable | SOW acceptance responsibility |
| --- | --- | --- |
| A, policy admission | Canonical policy normalization in IAM/loader/import/export; candidate graph and concrete bindings; deployment capture; immutable managed executable metadata readmission. Own `runtime_authority.rs`, its repository, `runtime_policy.rs`, pack-registry admission/loading, identity policy expansion, and deployment activation in `routes/packs.rs`. | 1, 2, 3, 8, 14, 17; support 16 and 18 |
| B, execution admission | One server-side execution-admission interface; manual/child, rule/enforcement, queue, workflow/fanout/cache, retry and callback paths; mixed-requester sponsorship/input/privacy. Own executor admission call sites, `routes/executions.rs`, and dedicated new execution-admission modules. | 4, 9, 12; support 7, 8 and 18 |
| C, credential consumers | Remaining recorded-token API consumers, artifact visibility/provenance, notifier revalidation, worker issuance readiness and bounded completion delivery. Own API auth/consumer modules, artifact routes, notifier, worker runtime-token/completion code. Coordinate execution-route changes with B. | 5, 6, 7, 10, 11, 12; support 18 |
| D, configuration and rollout | Release defaults plus durable overrides/reset for packs/sensors; conservative historical preservation; authority-bearing config classification; incompatible trigger-schema rollout controls. Own component-configuration repository, config DTO/routes/UI and sensor configuration loading. Coordinate `routes/packs.rs`, loader and activation hooks with A. | 8, 13, 14; support 18 and 19 |
| E, candidate isolation and bootstrap | Real isolated candidate API-effect context, lifecycle/archive fences, exclusion from production rules/notifications; common bootstrap admission integration and version/readiness parity. Own candidate test context/service code and bootstrap entry points. Coordinate common deployment entry points with A and token/ingress hooks with C. | 15, 16; support 18 |
| F, completion and file publication | Split from C after its first handoff. Operation-specific worker completion permits and final log/artifact delivery; recorded file/internal-file mutation and final publication fences. C retains protected-read provenance; B owns queue/execution API consumer continuation. | 6, 7, 12; support 18 |
| G, structural IAM consumers | Verified structural contexts for normalized human/sensor grants, origin/input authorization and remaining SQL filters. Coordinate execution/queue hooks with B and retained-read hooks with C. | 12, 17, 20 |
| H, managed component readmission | Workflow/rule/queue behavior API edits through immutable readmission and finite declared dynamic action targets. D owns sensor metadata readmission; A owns foreign graph pins and declared purpose inputs. | 4, 8, 14; support 19 |
| I, recorded collection filtering | SQL authorization before pagination/count/order for recorded execution/queue/item lists and previews. B retains mutation/native/input paths. | 12, 20 |
| Integrator | Shared interface decisions, collision resolution, frontend execution-options integration, client regeneration, acceptance inventory, integration and release enablement. | 18, 19, 20; verify every criterion above |

Reserve migration versions by stream: A uses `20261015001xxx`, B
`20261015002xxx`, C `20261015003xxx`, D `20261015004xxx`, E
`20261015005xxx`, F `20261015006xxx`, G `20261015007xxx`, H `20261015008xxx`,
I `20261015010xxx` only if an index migration is necessary;
the integrator owns `20261015009xxx`. Existing migration
`20261015000001` remains unchanged. Do not assume another stream's migration
exists until its contract and commit are handed off.

## Dependency handoffs

- A publishes normalization, policy lookup, metadata revision and activation
  interfaces. B consumes admitted invocation bindings; C consumes persisted
  admissions and selected authority. D and E call the same activation planner.
- B publishes server-owned admission/retry/attribution helpers. C mints worker
  credentials only from these stored records. Native executor operations must
  validate server-owned authority without manufacturing a worker attempt.
- C publishes operation-specific final delivery and candidate ingress hooks.
  A/B cannot treat an unconverted consumer as permission to issue broader tokens.
- D publishes defaults/overrides/reset and rollout preflight/activation checks.
  A connects those checks atomically with metadata and policy activation.
- E publishes isolated candidate context and bootstrap call requirements.
  Production event routing and notifier delivery must exclude candidate effects.

An assignment is complete only when its required production paths and focused
tests are implemented, its dependency contracts are explicit, and its commits
are ready for integration. Report blockers and unimplemented acceptance items
honestly. Do not replace required behavior with inactive scaffolding and call it
complete.

## Integration sequence

1. Review independent commits and interface handoffs against all 20 SOW criteria.
2. Integrate dependency-ready commits as uncommitted changes in the primary
   checkout, checking their patch against the captured baseline and later edits.
   Resolve overlap deliberately; preserve unrelated user changes.
3. Regenerate clients once the combined API contract is stable. Run focused
   cross-stream tests before broader validation, on PostgreSQL 18 only.
4. Activate the coordinated runtime path only after all required ingress,
   execution admission, consumer and completion checks pass. Keep readiness
   failures explicit in both bootstrap modes.
5. Record exact commands/results and unresolved acceptance items in the main
   tracker. The feature is not complete until every acceptance item has evidence.

## Active implementation cycle

The primary checkout is `/mnt/wdc/Codebase/attune-system/attune`, branch
`feature/named-type-schemas`. Its starting HEAD is
`1fa52e22a077fea1a5ee86d259e038023234b76a`. The isolated baseline commit is
`03fd6dfdc4737181357288273bc9fce01d9299d4`, created in the baseline worktree,
not on the primary branch. Snapshot inputs and hashes are recorded in
`/tmp/opencode/sow-f067e2bb/manifest.json`.

Task worktrees are under `/mnt/wdc/attune-worktrees/sow-f067e2bb/`:

| Stream | Directory | State | Integrated commits |
| --- | --- | --- | --- |
| A | `a-policy` | Foreign graph pins and authored-input helpers committed and integrated; B/C caller wiring remains | Own commits through `113048f9`, with reviewed D/E prerequisites, applied uncommitted |
| B | `b-execution` | Initial admission/payload-editor source integrated; immutable-contract adaptation and operation consumers running | Initial own commits through `dfe0abfb` applied uncommitted |
| C | `c-consumers` | Registry/protected-read source integrated; event timestamp, log decoder and notifier race continuation running | All own commits through `69d5755e` applied uncommitted |
| D | `d-configuration` | Sensor metadata/readmission and retained foreign-rule proofs committed and staged; C/G reader hooks pending | Initial/CLI/configuration and all `bf2435a2` tests applied; new sensor delta held for frozen validation |
| E | `e-candidates`, `e-charts`, `e-docker` | Guard and packaging follow-up committed; packaging activation held | `dfdac46f`, `4fb36ece`, `37282791`, `fa33d3b6` applied uncommitted; bootstrap/packaging held |
| F | `f-completion` | Completion/file fences/manifests integrated; generic file-origin propagation continuation running | Own delta through `e7ef4b35` applied uncommitted; redundant fixture-only commit skipped |
| G | `g-iam-contexts` | First structural consumer set integrated; cache/inquiry/redaction/dashboard continuation running | Eight own commits through `7af22593` applied uncommitted |
| H | `h-managed-readmission` | Managed edits/dynamic bounds integrated; workflow authoring UI continuation running | Nine own commits through `d61e77d5` applied uncommitted; prerequisite copies excluded |
| I | `i-recorded-collections` | Implementing typed SQL pagination/count/filtering from B's `a14f8e62` base | None yet |

The integrator is implementing `GET /actions/{ref}/execution-options` in its own
route module and removing execute-modal personal-IAM picker heuristics. It uses
current recorded action policy when available and effective delegation otherwise.
This is an advisory preview; execution creation still rechecks admission and
revocation. Eight focused modal tests, web type checking and focused lint passed.
The focused PostgreSQL 18 API test passed on the final endpoint/test source,
including approval and policy revocation. Its build reported 29m34s including
shared-target lock contention; the database test itself took 30.50s. This endpoint may need its metadata lookup
adapted to A's immutable metadata-revision contract during integration.

### B handoff and required integration work

B's initial commits are `07cf6224`, `c216655f`, `a7ffec0a`, `ee965858`,
`f25381ac`, `7af750ad`, and `2189510d`. Details are in
`/tmp/opencode/sow-f067e2bb/b-execution-handoff.md`. Migration validation passed
on PostgreSQL 18.6; focused repository test/workspace evidence is not yet a
passing handoff. These production-path changes are held out of the primary
checkout until their policy dependencies and validation are ready.

- A must retain policy revisions referenced by accepted queue items and supply
  immutable queue templates, or enforce a drain before changing templates/policy.
  Mixed admitted queue policy IDs currently fail explicitly rather than dropping
  a revocation dependency or partitioning by requester/grants.
- A must resolve retained policy graphs for foreign nested workflows and align
  deployment's graph/pack/sponsor/policy/owner/approval lock order with B.
- C must connect worker Key-origin restoration to B's per-contributor
  `input_key_origin_allowed` helper. Preserve B's `ExecutionCreate` consumer
  marker, enqueue admission hooks and new-acceptance action-lifecycle fence.
- Deployment-approved authored workflow/task Key/config selectors need distinct
  purpose-specific input bindings. Capturing only an Execute-only invoker's
  personal grants cannot authorize declared pack-local inputs, and runtime API
  grant selection must not become input authority. Verify this with empty API
  refs, recorded origins and human-freeze continuity; caller-supplied selectors
  must not borrow authored-code input authority. A was notified of this contract
  requirement during integration review.
- B is implementing current-editor coverage of immutable accepted input grants
  before pending payload mutation, with separate cross-owned queue API commits.
- Existing executor fixtures need real admitted policies. Rejecting their
  unmanaged snapshots is not passing production-path acceptance. A real recorded
  child-create API test and barrier-based cross-stream lifecycle tests remain.

Dependency notifications were sent to A and C. Task worktrees may incorporate
reviewed prerequisite commits locally, but must report prerequisite commit IDs
separately from their own changes to avoid duplicate integration.

### B follow-up and first primary integration

`bf9f79d2` adds the immutable accepted-input editor proof and repository tests.
`27938f34` adds the three queue API payload-mutation hooks. B's PostgreSQL 18 run
passed four of five tests, including both new editor tests; mixed-requester JSONB
decoding failed. `dfe0abfb` fixes that production defect without changing the v1
wire shape. The final focused rerun and API/executor checks remain pending.

The integrator reviewed and applied the codec's production change to the primary
checkout as uncommitted source. Two additional pure unit tests cover tagged
string/JSONB round-trips, mixed/unknown requesters, maximum i64 IDs and rejection
of noncanonical/nonpositive/overflowed IDs. These new primary tests are pending
combined validation; no passing claim is made for them yet. A and C received the
updated dependency handoff. The larger admission/API changes remain held pending
validation and policy integration.

### D handoff and dependency-ready integration

D's initial commits are `e83f9c90`, `d33f3286`, `e7dd0d12`, and `9ff63deb`;
the detailed handoff is `/tmp/opencode/sow-f067e2bb/d-configuration-handoff.md`.
Seven PostgreSQL 18.6 configuration/rollout repository tests passed. Both pack and
sensor configuration API tests passed; the corrected rollout API fixture remains
unverified because its rerun hit a 40-minute build-lock timeout. Type checking,
focused lint and two form tests passed.

The integrator applied `d33f3286` to the primary checkout without committing. Its
rollout repository and tests are independent of D's configuration-layer migration
and do not enable a new production path alone. Configuration-layer and UI/reset
patches are held until A wires preservation before projection mutation, original
authored defaults, pack partial patch/reset, equivalent-contract migration checks,
and atomic rollout/readmission hooks. An exposed reset control must not silently
send a field that the primary pack route ignores.

A received the exact D activation contracts. D's follow-up covers CLI override/reset
and explicit rollout controls plus sensor-generation/named-contract regressions.
Live-sensor and named-dependency inventory, defaults retention and direct managed
readmission still require combined acceptance evidence.

D's follow-up is now committed: `efa666e6` adds secret-safe shared configuration
and trigger-rollout CLI implementations; `d5dfff06` adds isolated command hooks
and parser tests; `bf2435a2` adds sensor-generation, named-contract and bounded
defaults-retention regressions. The worktree is clean. No new Cargo jobs or
databases were started; these additions are not yet compiled/tested. The two
rollout test paths from `bf2435a2` have been applied to the primary checkout.
The defaults-retention test remains with its unintegrated configuration-layer
dependency. CLI/backend/UI controls will be integrated together, not exposed
before the primary pack route implements reset. A received the new commit IDs.

### Validation scheduling

Parallel worktree compilation against one shared Cargo target has caused 38 to
40-minute lock waits/timeouts. The host has 31 GiB RAM and substantial swap
pressure, so five independent parallel target builds are not a safe alternative.
Workers have been directed to avoid new broad/workspace suites and prioritize
source handoffs with exact existing evidence. The integrator will consolidate
remaining builds and tests on one combined source, PostgreSQL 18 only. A waiting
or timed-out compilation is not recorded as a test failure or a passing check.

### C consumer integration and scope split

C's handoff is `/tmp/opencode/sow-f067e2bb/c-consumers-handoff.md`. Own initial
commits are `75255028`, `4cd9cff4`, `a624d27d`, `437df12f`, `37552892`, and
`c4c91923`. B prerequisite cherry-pick copies are listed separately in that
handoff and must not be integrated twice.

The primary checkout now contains the first three C commits as uncommitted
changes: structural artifact ownership, notifier recorded revalidation, and typed
artifact metadata/mutation consumers. A clean three-way merge preserved the
coordinator's execution-options API regression and already-fixed formatting in
the shared recorded-consumer test file. Protected content remains denied until
generic read origins can be retained and consumed. Worker issuance/readiness and
restoration commits are held for B's admission dependencies and coordinated gates.

C's existing evidence includes 18 recorded-consumer API tests and 29 artifact
tests before B integration, plus three worker-input PostgreSQL 18.6 tests after
B integration. These are not combined-source acceptance. The notifier DB-enabled
rerun, connected revocation races and final common/API/worker checks remain.

To finish the outstanding work without one oversized consumer assignment:

- C continues generic runtime-read origin retention/closure and protected
  artifact/version/download/SSE reads, including supported historical origins.
- B continues operation-specific queue and execution API consumers, preserving
  its accepted input, payload-editor and child-creation fences.
- F owns file/internal-file mutation/final publication and operation-specific
  bounded worker completion. Its base is C's `c4c91923`, not the original seed;
  only F's own commits after that base are new integration changes.

### E candidate and bootstrap integration

E's handoff is `/tmp/opencode/sow-f067e2bb/e-candidates-handoff.md`, with commits
`dfdac46f`, `4fb36ece`, and `2950bfd6`. Six candidate/bootstrap PostgreSQL tests,
six health cases, two legacy-upgrade regressions and 18 Python/shell tests passed.
The primary contains only the independent candidate persistence/TestExecutor
commit so far. Ingress/lifecycle wiring is held for the purpose-auth guard update;
API-only bootstrap packaging is held for coordinated readiness and A's planner.

A must validate closed opt-in candidate context/archive evidence before activation.
The bootstrap protocol gate remains false until ordinary admitted deployment,
input/consumer/worker paths and D's configuration hooks agree. No direct SQL
bootstrap removal is integrated ahead of its working replacement.

E is aligning the candidate capability ingress with shared `RequireAuth` while
keeping it distinct from JWT/human/resource authority and revalidating the proof
in the effect transaction. E also owns task worktrees for the clean sibling
repositories `attune-charts` and `attune-docker`. Their primary main branches
remain untouched. Bases and paths are in
`/tmp/opencode/sow-f067e2bb/auxiliary-worktrees.json`; reviewed packaging patches
will be applied to those primary checkouts without commits.

### A dependency set and combined baseline

A's handoff is `/tmp/opencode/sow-f067e2bb/a-policy-handoff.md`. Its own commits
through `a20421e5` and imported D/E prerequisites have been applied to the primary
checkout as uncommitted changes. The 70-file three-way integration had one manual
conflict. Both C's private-artifact baseline restriction and A's concrete target
binding checks were retained. Existing coordinator/C tests and D rollout
extensions were preserved. The review manifest and patch are under
`/tmp/opencode/sow-f067e2bb/integration-a/`.

This connects deployment capture, canonical authoring/normalization, immutable
action metadata, configuration/reset backend and UI/CLI, queue drain guards and
candidate activation evidence. It does not enable registrar exemptions, worker
issuance or bootstrap readiness. A's eight focused PG18 regressions and API
library checks passed before some follow-ups; new canonical-authoring regression
extensions and the combined source still need validation.

`95d0facaa352974ad5fa32ae64e9d277a1029580` is an isolated combined-baseline
commit, parented on A's `a20421e5`, containing the current primary source snapshot.
It was created in `combined-baseline-1`, not in a primary checkout. G and H start
there; only their later commits are new integration changes. Metadata is in
`/tmp/opencode/sow-f067e2bb/combined-baseline-1.json`.

The remaining critical work is deliberately split:

- A implements retained foreign workflow policy graphs and authored input bindings.
- B adapts production admission to immutable metadata/source proofs and completes
  recorded execution/queue API operations.
- C completes generic read-origin retention and protected-content delivery.
- D implements immutable sensor metadata/purpose authority and direct readmission.
- E completes candidate guard and sibling bootstrap parity; F completes files and
  operation-specific worker cleanup.
- G supplies verified structural IAM contexts and filters, without removing
  `managing_pack_id` when a consumer lacks proof.
- H implements managed workflow/rule/queue readmission and finite dynamic targets.

Normalization without those consumer contexts would deny legitimate local access;
ignoring its structural constraint would broaden access. Neither outcome is
accepted as completed behavior. No combined-source validation or full-SOW
completion is claimed at this integration boundary.

### Combined admission/read-provenance compile milestone

B's initial source through `dfe0abfb` is now integrated. The codec's already-added
primary unit tests were retained. C's remaining worker/input commits `437df12f`,
`37552892`, `c4c91923` and continuation commits `2ec10951`, `69d5755e` were then
integrated. Review manifests are under `integration-b-initial/` and
`integration-c-worker/` in `/tmp/opencode/sow-f067e2bb/`.

E's candidate lifecycle/ingress `4fb36ece`, generic auth guard `37282791` and
candidate proof authentication `fa33d3b6` are integrated. This preserves ordinary
JWT extraction and uses opaque `RequireAuth<AuthenticatedCandidateContext>` for
candidate ingress. Bootstrap replacement `2950bfd6`, Helm `ca57d5f` and Docker
`63211fb` remain held until their coordinated readiness replacement works. No
primary sibling repository has been changed. E's follow-up passed nine chart,
seven Docker and five existing Helm worker tests. New Rust guard tests remain
pending combined execution.

The first combined library check found C integration errors: missing I/O error
conversion, an undeclared streaming dependency, retained-parent move/type errors,
a schema repository pool/connection mismatch and SSE error-type inference. The
integrator fixed them without adding a dependency, using existing futures
unfold streams. C received the fixes for its continuation. The resulting
`CARGO_BUILD_JOBS=2 SQLX_OFFLINE=true cargo check --workspace --lib` passed without
warnings in 35.12 seconds. This is a library compile milestone, not an all-target,
database, lifecycle or complete acceptance pass. Client regeneration subsequently
passed. All current migrations through `20261015005001`, including the earlier
stream-reserved migrations, passed on a fresh owned PostgreSQL 18.6 container.
The migration-only runner stopped its owned container on exit. An all-target
workspace check is running; no result is claimed yet.

C's registry records bounded immutable parent snapshots and closes acquisition
before outputs. Unsupported or unprovenanced sinks remain explicitly denied.
C still owns timestamp-qualified event provenance, encrypted-log decoder delivery
and connected notifier races. B/F received same-transaction sink guards and the
requirement to complete durable downstream provenance rather than permanently
deny supported flows. F must persist authenticated final stream completeness and
budget ciphertext expansion using the common encrypted-log codec.

The coordinator also connected canonical permission authoring to the web editor.
It edits `authored_definition` rather than reconstructing authored scope from
normalized IAM grants. Both the set default and per-grant override preserve the
canonical `pack`/`external` values. Structural pack IDs are displayed but never
written as authored selectors. Events/Create, Keys/Use, queue items and protected
read/decrypt specs and owner-reference selectors round-trip rather than being
silently filtered. Missing/malformed authored metadata disables editing instead
of treating normalized grants as absolute authority. Five focused web files now
pass 42 tests, and web type checking passes after client regeneration. An older
cache test was updated to exercise an actually unsupported owner-ref resource,
since Keys and Artifacts legitimately support that selector.

### Pinned workflow/input and structural-IAM integration

A's continuation `9c815e36`, `113048f9` is integrated. Its exact B/C/H contracts
are in `/tmp/opencode/sow-f067e2bb/a-continuation-interfaces.md`. It adds pinned
publisher policy graphs, retained parent admission revocation dependencies,
authored field/expression/revision permits and exact destination delivery
receipts. B must connect workflow dispatch, source inheritance and authored
materialization; C must connect receipt-scoped worker restoration. These helpers
alone do not establish positive end-to-end Execute-only local input delivery.

G's first eight commits through `7af22593` are integrated. The handoff is
`/tmp/opencode/sow-f067e2bb/g-iam-contexts-handoff.md`. The semantic merge retained
both C's immutable runtime-read fields and G's batched structural proof maps.
Common input restoration retains C/B's per-contributor and recorded authority
branches while using verified Key ownership for ordinary local grants. B's
recorded contributor matcher must also adopt verified ownership. Structural
component configuration context is resolved only after owner survival checks.
C's generic immutable parent maps, budgets and Access-only separation remain.

The first all-target check caught duplicate synthetic policy fields from an
earlier clean text merge, missing new policy fields, a scheduler test's changed
snapshot argument and a sensor fixture passing the wrong lease type. The
integrator fixed those adapters without weakening production checks. The combined
A/G source and fixes passed `CARGO_BUILD_JOBS=2 SQLX_OFFLINE=true cargo check
--all-targets --workspace --keep-going` without warnings in 2m34s.

The frozen-source PostgreSQL 18 milestone is now running under
`/tmp/opencode/sow-f067e2bb/validate-combined-milestone.sh`. It owns one ephemeral
PostgreSQL container, unique invocation IDs and teardown. The sequence includes
fresh migrations, seven common repository binaries, three API binaries, worker
input restoration, and SQLx preparation against all workspace targets. Tests use
four threads. A focused binary failure will be reported rather than preventing
the remaining binaries from yielding evidence. No primary Rust/migration source
will be changed during the run. The new graph/declaration migrations and runtime
acceptance are not claimed passing until the runner returns.

G continues positive cache/inquiry/event redaction/dashboard structural support;
its current explicit dashboard denial is a safety gate, not completed support.
B/C/D/H received the precise source and retained-origin contracts. The full SOW
remains incomplete, with registrar, worker and bootstrap gates still disabled.

### H/F handoffs staged during frozen-source validation

H's managed workflow/rule/queue readmission and finite dynamic target source is
committed through `d61e77d5`, with imported A/G prerequisites identified separately
in `/tmp/opencode/sow-f067e2bb/h-managed-readmission-handoff.md`. Its staged merge
is `integration-h-staged/manifest.json` under the owned temporary directory. The
planner/test conflicts were resolved against the exact common A continuation;
G's later ownership hardening is retained rather than replaced by H's foundation
copy. H is now adding workflow builder controls and serializer round-trip tests
so `allowed_action_refs` is not a parser-only or raw-JSON-only feature.

F's own completion/file delta is committed through `e7ef4b35`. Original and later
own commit IDs, prerequisite copies, exact permit/final-manifest contracts and
unexecuted tests are in `/tmp/opencode/sow-f067e2bb/f-completion-handoff.md`. The
reviewed merge is `integration-f-rebased/manifest.json`. It excludes unrelated
prerequisite-only files, preserves primary generic `RequireAuth`, stream compiler
fixes, newer structural contexts and canonical fixture fields, and combines F's
frame counters with the coordinator's safe log I/O error conversion. A redundant
fixture-only commit is skipped.

Neither staged source has been applied during the PostgreSQL run. Apply only
after checking the source digest and test result, then rebase the second merge
against the first because their module/fixture changes overlap. These additions
require new migrations and another focused integration milestone; prior passing
checks do not validate their source.

F's manifest authenticates exact execution/version, frame count, plaintext and
ciphertext totals and ciphertext digest. Seal proof, immutable metadata and Ready
publication share the permit-fenced transaction. C received the precise producer
contract for decoder consumption and owns the global read/effect capture-lock
ordering audit. B received H's exact per-task schema compiler contract, and D
received H's retained rule/trigger/sensor owner fields for historical delivery.

F's generic process file effects currently use a temporary taint denial. F is
continuing durable file-origin propagation and protected file delivery; that
denial is not accepted as complete support. C continues encrypted-log decoding,
timestamp-qualified event sources, receipt-scoped restoration and notifier race
tests. None of these unexecuted handoffs enables production gates.

### Frozen PostgreSQL milestone result and next integration

The frozen run finished with **77 tests passing and five failing** across common,
API and worker binaries. Fresh migrations, including A's graph/declaration
tables, and SQLx preparation passed on PostgreSQL 18.6. SQLx reported no
compile-time queries. The owned container stopped on exit. The source manifest
matched all 702 Rust/SQL/Cargo files after the run, and the primary HEAD did not
change. Evidence is in `combined-pg18-source-manifest.json` and
`combined-pg18-result.json` under the owned temporary directory.

Failures are recorded, not counted as passing acceptance:

1. Mixed-requester admission test tried to replace policy with accepted items
   pending. B must update its lifecycle expectation, not weaken the drain fence.
2. Foreign workflow fixture updated a generated `management_origin` column. H's
   already-reviewed fixture correction is now integrated.
3. Foreign code retention expected two deletions and got one, with an unsafe
   fixture content-path warning. A is diagnosing actual pins/cleanup; lowering the
   assertion without a retention proof is not an accepted fix.
4. Two standard artifact-root read tests got 404 instead of 200. C is diagnosing
   unconditional creator-execution read/decrypt checks without granting `standard`
   unrelated Execution rights or bypassing sensitive version/log origins.

After the run ended and source equality was checked, the integrator applied B's
two payload-editor commits `bf9f79d2`, `27938f34` explicitly. They followed
`dfe0abfb` in history rather than being included by that earlier tip, so their two
regressions were absent from the frozen three-test admission binary. The next
run must include those tests; the earlier three tests are not evidence for them.

F's reviewed own-file delta and H's nine own commits are now applied uncommitted.
H was rebased over F and B's payload hooks, retaining both. D's remaining
defaults-retention test from `bf2435a2` is integrated now that its layer dependency
is present. New F/H migrations and tests were not in the completed milestone.
The next combined all-target check is running. Runtime fixes from B/C/A and the
new completion/readmission coverage need focused PostgreSQL 18 reruns before any
broader acceptance or gate enablement.

A's retention diagnosis is now available in
`/tmp/opencode/sow-f067e2bb/a-retention-diagnosis.md`. A live omitted queue still
pinned the old source release after the fixture activated only the replacement
action. Retention correctly preserved that independent projection pin. Commit
`1e48d94e` keeps the two-deletion assertion, performs explicit Remove reconciliation,
adds a Retain-versus-Remove regression and uses owned canonical content trees.
No restrictive pin or storage guard changed. Its clean merge is staged in
`integration-a-retention/manifest.json`, pending the active all-target check.
The fixture diagnosis is source-derived; the corrected tests need the central
focused PostgreSQL 18 rerun before being counted as passing.

The subsequent fresh PostgreSQL 18.6 migration-only pass succeeded through
`20261015008001`, including F's worker delivery and encrypted-log completion tables
and H's managed dispatch metadata. Its owned container stopped on exit. This
validates fresh schema installation, not completion delivery or readmission
behavior. A's retention fixture correction is now applied uncommitted; the
affected runtime reruns remain pending. The F/H integrated all-target check then
passed without warnings in 38.71 seconds after correcting typed-snapshot test
adapters. No authorization checks were weakened to make those adapters compile.

The next source-frozen lane is running under
`/tmp/opencode/sow-f067e2bb/validate-completion-readmission.sh`. It covers authority
and retention tests, configuration/default-retention and sensor generation tests,
payload-editor regressions, pure completion permit/manifest tests, H's managed
readmission API and F's API/worker completion cases, then SQLx preparation.
It does not repeat the unrelated recorded artifact-root failures or the queue
test awaiting its corrected drain expectation. This is different coverage from
the first 82-test milestone, not a comparable benchmark or a replacement claim.
The 715-file source fingerprint is in
`completion-readmission-source-manifest.json` under the owned temporary directory.

### D sensor metadata and historical proof staging

D committed `a121d740`, `287a9438`, `620c5fde`, and `b2314b85`. The precise
contracts are `/tmp/opencode/sow-f067e2bb/d-sensor-proof-contract.md`. The staged
merge `integration-d-sensor-staged/manifest.json` retains all A graph/input/runtime
retention pins and adds the sensor-rule proof pin. It remains unapplied while the
715-file completion/readmission milestone runs.

The source adds immutable sensor metadata and future-generation readmission,
independent cache/input/signing authorities, operational runtime revocation and
creation-time foreign rule structural proofs with no historical name backfill.
C/G received the exact proof identity, digest, original-generation validation and
actual target ownership checks required for typed historical disclosure. Proof
does not itself grant read/decrypt authority. Their reader hooks remain pending.
New D migrations and regressions are not covered by the preceding passing check.

D also rechecks fixed approval expiry after target lock waits. The integrator
assigned A a common runtime-authority audit so non-sensor opaque permits and
SQL-filter/stream/completion consumers cannot reuse a pre-wait timestamp past
expiry. That audit is active work, not a verified defect closure or permission
to alter lock ordering or weaken issuer-independent authority.

### Next dependency batch and remaining consumer work

The following source is reviewed/staged, not applied to the frozen F/H test tree:

- A `92165cc0` adds an opaque deadline intersection and final DB-clock freshness
  check. B/C/F must retain the source fence through final target/capture/query
  waits. The common helper alone does not close every consumer race.
- B's 15 own operation/dispatch/input commits through `a14f8e62` are staged under
  `integration-b-continuation-rebased/`. The merge preserves F test hooks and
  completion metadata, G ownership and H's readmission, while incorporating B's
  explicit queue hook reconciliation. B continues native input propagation and
  recorded ad-hoc item admission. Temporary taint barriers remain incomplete.
- C's 12 own commits through `d06c1d9e` are staged under
  `integration-c-continuation-staged/`. They include the standard root-read fix,
  capture lock ordering, connected notifier tests, timestamp-qualified event
  sources, finite encrypted decoder and D's generic proof hook. The merge retains
  G's bounded ownership loading, F's manifest counters and safe compiler bridges.
  C continues human/large/live log delivery, execution streams and expiry wiring.
- G's nine follow-up commits through `922b5d71` are staged under
  `integration-g-followup-staged/`. Positive cache/inquiry/event/runtime/dashboard
  source support and the ordinary foreign-proof reader are source only. D/G are
  completing batched proof retrieval to avoid one query per historical origin.

Each stage must be rebased over the preceding applied stages after the frozen
run returns. Shared prerequisites are not new task commits and must not replace
newer source fences, immutable origin maps or actual ownership checks.

I now owns SQL pushdown for B's recorded collections. B's initial bounded scanner
can reject a small owned page when unrelated global candidates exceed 10,000 and
does per-target database work before paging. I must compile the typed envelope,
concrete IDs and all narrower selectors into repository SQL before counts/limits.
It starts at B's `a14f8e62`; only subsequent I commits are new integration changes.
No additional test/build pipeline is authorized for that worktree. All runtime
evidence remains on the integrator's PostgreSQL 18 lane.

D's batching follow-up `14879f1b` is now included in the reviewed, still-held
`integration-d-sensor-batched-staged/` merge. Exact tuple proof lookup and original
generation/metadata preparation each use zero statements for an empty batch and
one for up to 256 raw entries, with shared digest/pin/shape decoding and duplicate
coalescing. Raw input is capped before coalescing. G/C received those APIs and
must batch existing frontiers and cache known absence without treating proof as
authorization. The 1/64/100-statement-count tests are source assertions only;
their budgets have not yet been measured on the combined source.

### Latest integration and validation boundary

The second frozen lane returned **72 passing and 12 failing tests**, with all
715 source fingerprints unchanged and its owned container stopped. The passing
tests include both retained-code cleanup cases, both payload-editor proofs,
configuration/default retention and 24 sensor workload cases. Nine F API/worker
cases failed during worker creation because their fixture supplied NULL status.
They did not test completion behavior. One pure JWT test lacked the explicit
crypto provider installation used by existing JWT unit tests. H's dynamic schema
fixture used a non-hex digest and its API fixture supplied an untagged pack patch.
These setup adapters are corrected, with expected authorization unchanged, but
the failed tests need reruns. Migrations and SQLx preparation passed.

After the freeze ended, the integrator applied the next reviewed batch, including
reconciled B/C/D/G source, I's five SQL collection commits, H's four authoring
commits and A's expiry helper. The combined web type check and **80 tests across
11 files passed**. The new Rust source has not passed its all-target check.

Compiler integration fixes preserve the intended interfaces: queue item search
and selector preview now have concrete-connection methods to keep their count
and page queries inside one effect transaction. Shared search SQL remains one
builder. The notifier's credential-redaction helper moved unchanged from the
binary into its new library, and internal auth fields remain crate-visible.
Tests compute their canonical fixture hashes locally instead of using a private
schema compiler helper. B owns the scheduler's generic future `Send` boundary
and a narrow initial-config persistence method; a nonexistent general mutable
config field is not an acceptable substitute for immutable input admission.

C's later delivery commits `458bc2e9`, `02186b32`, `78eca202`, `82d16c3c` are now
integrated. F's file-origin/expiry follow-up through `cb7e36a7` is also integrated.
The merges preserve both F's whole-file authenticated codec and C's streaming
sealed-log verifier. Metadata-only file mutation uses C's `EffectTransaction`
with the original source authority, avoiding a second source-lock acquisition
after targets. Pure cache reads retain capture ordering but do not use a mutation
taint guard. Runtime file sink snapshots remain excluded from acquisition counts
and execution-output closure so they cannot form self-referential origins.

Current manifests include `integration-c-delivery-staged/`,
`integration-f-after-delivery/`, `integration-i-collections/`,
`integration-g-after-c/`, and `integration-g-proof-batches/`. "Staged" in an
artifact directory name describes its original preparation; the recorded patch
and `appliedFiles` identify what was actually integrated. Earlier passing checks
do not validate this later source. The new consumer compile and fresh schema
installation are running. B/C/E/F/I continue their remaining caller, transport,
bootstrap, retention and selector-parity work without independent build jobs.

The fresh schema pass for that batch completed successfully on PostgreSQL 18.6,
including `20261015003003`, D's `04002`/`04003` and F's `06003`. The owned container
stopped on exit. This is fresh-install schema evidence only; runtime delivery,
origin propagation and post-lock expiry behavior still need executable tests.

All targets in common, worker and notifier subsequently compiled without
warnings in 1m38s. API/executor compilation remains blocked on B's initial-config
and scheduler future fixes. The common schema/provenance and worker/notifier
milestone is now running under `validate-provenance-repositories.sh` in the owned
temporary directory. It uses one disposable PostgreSQL 18 container, four test
threads and one Cargo lane. Coverage includes authority/admission, exact events,
sensor metadata/proof batches, dashboard/ownership repositories, pure file/log
completion and collection filters, worker input/completion, and notifier permit
separation. API/executor cases are explicitly excluded until their fixes compile.
No full-workspace or connected socket acceptance is implied by that exclusion.
The 746-file fingerprint is `provenance-repositories-source-manifest.json`.
Primary Rust and migration source remain frozen until this lane returns.

### Bootstrap and execution-options follow-ups

E's application bootstrap commits `805ccb4a`, `d5da5b7e`, together with previously
held `2950bfd6`, are reviewed in `integration-e-bootstrap-staged/manifest.json`.
They remain unapplied during the frozen repository lane. The merged application
uses one coordinated deployment service instead of duplicating inline policy,
rule and configuration capture. It preserves tested candidate archive closure,
rollout acknowledgement, original defaults, ownership/replacement checks and
existing source fences. The loader capability and root/publisher readiness checks
remain disabled by the shared release switch. The new acceptance fixtures have
not compiled or run. E reports 39 offline tests across its three worktrees.

The E Docker/Helm packaging changes through `3ee009b` and `1c07145` were reviewed
and applied uncommitted to their previously clean primary checkouts. Their HEADs
remain `15c69681` and `40394c9c`; no commits or deployments occurred. Reviewed
manifests are `integration-e-docker/` and `integration-e-charts/`. The integrator
reran the offline suites on those primary sources: **9 Docker and 11 chart tests
passed**. These tests render/inspect configuration; they are not fresh service
startup, upgrade/rollback or controlled-skew acceptance. Bootstrap still requires
the coordinated API descriptor and existing deployment integration credential.

H's additional `3f2239de` fixes execution-options selection to use exact admitted
metadata/defaults instead of only the current code release. Its staged merge
uses the same-transaction resolved structural target, preserving G's ownership
proof. Optional approval choices remain filtered without acquiring personal
membership, and missing managed admission is not unmanaged delegation fallback.
The source and regressions remain held in `integration-h-options-staged/` pending
the active frozen lane. The API tests are uncompiled/unrun.

H's `f412cbbe` now includes the options lock-order correction in its own source
and adds a queued graph-writer regression using the real metadata fixture. The
updated stage is `integration-h-options-final-staged/manifest.json`, superseding
the earlier options stage. Human IAM loads before the transaction; shared graph
admission precedes source target locks. This source remains unapplied during the
repository validation freeze, and its barrier test remains unrun.

I's selector-parity follow-up `9d0519df`, `a394051b`, `de9bcd4e` is staged in
`integration-i-parity-staged/`. It adds typed-target differential tests, SQL-level
metadata redaction and accepted-item reads after queue retirement. I also found
bulk mutation selector privacy, selection-drift and source-revalidation ordering
gaps. I now owns narrow cancel/reprioritize selector fixes and their tests; B
retains input admission, item creation/editing, native workflows and compiler
fixes. No passing collection or bulk-mutation behavior is claimed from this
source-only handoff.

### Repository milestone result and newly applied sources

The repository lane finished with **94 tests passing and two failing**, across
12 binary/filter runs. All 746 Rust/SQL/Cargo source hashes and primary HEAD
matched; the owned PostgreSQL 18.6 container stopped. The exact result is
`provenance-repositories-result.json` in the owned temporary directory.

- All 39 authority tests passed, including expired/unexpired/no-expiry lock-wait
  barriers, metadata readmission, retained foreign graphs and cleanup.
- The dashboard and exact partition-twin event tests passed; ownership deletion
  barriers passed. Sensor tests passed 27/28, including new metadata and foreign
  proof/batching paths. Initial rule delivery failed because its fixture changed
  only an in-memory rule after clearing the stored row. D's test-only correction
  `b54676e7` is now integrated; the production stable-source guard is unchanged.
- Admission tests passed 8/9. The remaining failure proposes unadmitted refs to an
  already-created task's `if_absent` call. B must preserve existing child/admission
  idempotence without reinterpreting unused retry input or widening grants.
- All five worker restoration/completion tests passed, including empty-selection
  worker authority and bounded shared-volume terminal delivery. Pure file/log
  proofs and collection filters passed, as did notifier permit separation.
  These are not connected socket or API-backed worker producer acceptance.

After source equality was recorded, the integrator applied the E application
bootstrap stage, H's two options commits and I's three selector-parity commits.
C's live revision/decoder/transport source through `0e0532f6` is also integrated.
The merge preserves F's owner-pinned log naming, G/B private input matching, exact
event budgets, F file sink exclusions and both stats/stream consumer markers.
The B native contributor matcher now handles Metadata with Read only and LiveLog
with source/content, exact owner and stream/version checks instead of an unsafe
catch-all or a permanent unsupported denial. New tests/production source need
their own combined compile and runtime run. C is adding actual F producer route
coverage for live delivery; direct fixture frames alone are not producer evidence.

The committed B continuation through `922e750d` is now integrated from committed
blobs only. Its seven own commits include `434c0f3b`, `1fae591c`, `7b1087e1`,
`ca43f1cf`, `11add0c9`, `fc7ecc93` and `922e750d`; A/C prerequisite copies and dirty
native work were not imported. The review manifest is `integration-b-native-ready/`.
It preserves C's live frontier API and I's SQL collection projections while adding
the concrete pool/connection scheduler split and narrow initial configuration API.
The insertion-transaction proof uses an immutable stored transaction identity,
not `xmin`, which can change on UPDATE. Config history stores digest summaries.
The duplicate child path validates retained lineage/admission before considering
unused proposed refs and never installs the proposed receipts on the old child.
These corrections need the running all-target workspace compile and runtime rerun.

A now owns purpose-typed inherited receipt and accepted ad-hoc batch dependency
APIs requested in `b-required-contracts-after-a14f8e62.md`. Source and destination
entity kinds must distinguish config, variables and results at identical JSON
pointers. B retains renderer-site materialization and canonical input propagation;
I retains only bulk selector mutation fixes. Unsupported ad-hoc cross-source
batches and incomplete renderer receipt sites are still incomplete support.

The next planned consumer runner compiles each selected package's tests once,
records executable identities, then runs the exact selected binaries/filters on
owned PostgreSQL 18 with unique IDs and four threads. It is prepared as
`validate-current-consumers.py` but not launched while compilation is blocked.
It rejects empty filter runs and records immutable source comparison. This avoids
rebuilding common for each pure filter; its expanded coverage differs from prior
milestones and is not presented as an equivalent benchmark.

The fresh PostgreSQL 18.6 schema check for B's committed continuation passed,
including all `20261015002003` through `02007` migrations. The owned container
stopped. This covers qualified enforcement timestamps, retained ad-hoc dispatch,
nested contributors, digest-only config history and immutable creation transaction
identity, but not their runtime behavior. The first compile found missing local
SchemaError conversions and Clone for the private captured input record. The
integrator added sanitized validation mapping and Clone without adding Debug or
changing grants. The next all-target workspace compile is running.

The resulting combined all-target workspace check passed without warnings in
1m13s. This includes E bootstrap, H options, I selector parity, C live delivery,
F route protection and B's committed native/compiler continuation, plus test
adapter fixes. The prior production `Send` failure is resolved without changing
MQ/spawn bounds. Duplicate synthetic fields, Arc fixture mutation, concrete SQLx
reborrows and byte-slice comparisons were corrected; negative cases remain.

The expanded consumer runner is now active. It records source hashes and selected
test executable identities, builds each package's selected tests once, and then
runs the specified binaries/filters against one owned PostgreSQL 18 instance.
Empty filters fail instead of counting as coverage. Its API selections include
connected notifier/live streams, file delivery, managed options/readmission,
bootstrap admission, candidate isolation, configuration and disclosure budgets.
It also repeats common/worker cases with the dedup and sensor fixture fixes.
Primary Rust/SQL/Cargo source is frozen until it returns. B's later six evaluator
and canonical traversal commits through `d6d0f112` remain staged, not included.
All production enablement gates remain false.

F's route-audit delta through `cf1b426e` was integrated before the passing check.
Its path quarantine and GET/HEAD/range/chunk guards do not fix historical evidence
retention. F is implementing the documented reservation/GC/physical-cleanup pin
protocol; source/capture ordering and actual supported HTTP handoff semantics
remain C-owned. Fail-closed disclosure after evidence deletion is not accepted as
retention completion.

C's next batch through `0ea8c019` is staged in
`integration-c-worker-http-staged/`, not applied during the expanded run. It uses
F's actual completion register/allocation/transport/append/seal methods on both
backends, then exercises artifact/execution readers, later SDK acquisitions,
foreign approvals, revocation, reciprocal capture ordering and corrupt manifests.
It adds independent HEAD/header checks and real TCP backpressure/cancellation
sources. The documented HTTP boundary is Body handoff to Hyper after the final
application wait, not an unavailable database callback at physical kernel write.
Already handed-off bytes are not retroactively revoked. Tests remain source only;
dev-only worker dependencies and fixture hooks need a future compile.

F is implementing evidence pins and cleanup release rather than leaving its
retention audit as a future proposal. A is implementing purpose-specific inherited
receipt sessions and accepted multi-source queue dependencies; B is waiting for
those signatures for native caller wiring. Both are still required before the
SOW is complete, even if the current expanded tests pass.

The first expanded direct-executable run used the workspace root as its working
directory. Most fixture-backed tests therefore failed before behavior because
`../../config.test.yaml` was missing. Its source comparison was unchanged and
its owned container stopped. SQLx preparation completed, but those setup failures
are not authorization acceptance. The runner now uses each package directory,
matching Cargo's test execution convention, and repeats the same coverage on a
new owned PostgreSQL 18 instance without changing production source.

The A inherited-input, F evidence-retention, I bulk-mutation and E core-bootstrap
continuations hit subagent usage limits. Their unfinished isolated worktrees are
preserved. No uncommitted continuation has been imported or declared complete.

The corrected package-directory consumer run completed with **188 passing and
68 failing tests** across 23 selected binary/filter runs. Its source fingerprints
were unchanged, SQLx preparation completed, and its owned PostgreSQL 18 container
stopped. Result artifacts are in `ct0052593f39/` under the owned temporary root.
All 40 authority tests, 28 sensor workload tests, six candidate-isolation tests,
four configuration API tests and five worker restoration/completion tests passed.
The remaining failures include admission dedup/default fixtures, file producer
authorization/sealing, disclosure assertions, live-reader timeouts, shared test
sponsor setup and readmission/bootstrap fixtures. These are assigned to owners;
the failing workload tests do not establish their query budgets.

After the freeze ended, C's actual-worker HTTP batch through `0ea8c019` was
reviewed and applied using `integration-c-worker-http-current/manifest.json`.
The merge retains B/F fixture modules, the pinned test digest dependency and
owner-pinned log naming while adding C's independent header checks. A new
all-target workspace compile is running. The next B evaluator batch remains
staged. Restarted agents may create local task-branch commits; primary integration
commits, pushes and deployments remain prohibited. H and G now own their observed
readmission/options and disclosure-workload setup failures respectively.

The C actual-worker HTTP batch passed the all-target workspace check without
warnings after adding the missing optional foreign-workflow-policy field to its
action-only fixture. The integrator then applied B's six evaluator/canonical
traversal commits through `d6d0f112` and E's `9ee1eb2c`/`6810280a` bootstrap test
follow-up. No dirty scheduler continuation was imported. These additions require
the next combined check and PostgreSQL rerun.

E's chart `2509bf9` and Docker `a66203b` test-only deltas are also integrated
uncommitted. The integrator reran their publisher suites on primary sources:
12 chart tests and 10 Docker tests passed. The unrelated Compose working-tree
edit remains outside the integration. Actual shipped core workflow bindings
remain a known gap; E is implementing the canonical two-file declarations rather
than treating supplementary test metadata as shipped component coverage.

The B/E combined check compiled but reported unused native receipt/control
helpers in executor workflow context. B must connect the intended callers;
this is not a zero-warning pass and no warning suppression was added.

The integrator subsequently applied the reviewed acceptance-repair batch:
C through `06b08e40`, I `e7cdc8a8`/`29a4de7d`/`817ab017`, H `097acc0d` and G
`c9f33c30`. I's superseded `0120312e` and H's duplicate code-pin bridge
`af9d5bc8` were excluded. The manifests are `integration-c-failure-fixes/`,
`integration-i-bulk/`, `integration-h-acceptance/` and `integration-g-workload/`.
No prerequisite or dirty worktree files were copied wholesale.

C omits invalid empty Key grants rather than relaxing grant validation; its
encrypted-log decoder now precedes the plaintext-only sensor check while F's
protected-file dispatch stays intact. I's bounded bulk selector locks exact IDs,
rechecks visible selection after waits and carries source authority without
per-item source relocking. H and G use canonical coordinated deployment fixtures
for metadata/options and workload/sensor proofs. All negative assertions and
query-budget tests remain. This batch needs the running combined compile and a
new PostgreSQL 18 run; no behavior or query-budget pass is inferred from source.

A's original accepted-batch dependency commit `4f4c73fc`, B's eight own fixes
through `ad0f153e` and F's connected retention batch through `e49e649b` are now
integrated uncommitted. The B shared-fixture duplicate and imported A copy were
excluded; dirty native scheduler work remains outside primary. F was regenerated
against B's changed execution repository before application. Reviewed manifests
are `integration-a-batch-dependencies/`, `integration-b-native-followup-current/`
and `integration-f-retention-current/`.

Fresh installation of this schema passed on PostgreSQL 18.6, including accepted
batch dependencies `01010`, file staging quarantine `06004` and evidence retention
`06005`. The owned container stopped. This is not existing-reservation migration,
historical-reader, concurrent deletion or cleanup-recovery acceptance. The next
consumer runner includes the new file-evidence repository target alongside the
existing authority target's batch dependency tests. Compilation remains running;
new production source is not yet covered by an all-target or behavior pass.

The combined A/B/F/C/I/H/G source subsequently passed all-target workspace
checking without warnings in 51.46s. New test adapters use the public retained
admission fence and concrete connection search; private schema digest helpers
remain private. B's later mirror `6fd6cc61` was skipped as already integrated.

E's canonical shipped workflow migration `0f7e6fb5` was then applied through
`integration-e-core-workflow/`. Core is now `1.0.4`, with action-linked graph
metadata and ordinary JSON transport to existing protected pack APIs. The
integrator reran its two offline primary suites: 12 workflow tests and four
composition tests passed. Packaging version/digest alignment is still assigned
to E; no archive digest was invented or release published.

A new expanded consumer lane is running, including file-evidence retention,
batch dependencies, bulk mutations, actual-worker HTTP, corrected workload and
bootstrap/readmission tests. Rust/SQL/Cargo and shipped core-pack sources remain
frozen until the runner returns; the source fingerprint now covers core files
used by composition tests as well. Its results must not be inferred from the
earlier 188-pass/68-failure run. No gate, push, deployment or primary commit.

The expanded lane finished with **254 passing and 38 failing tests** across 24
selected binary/filter runs. Rust/SQL/Cargo/core fingerprints were unchanged;
SQLx preparation completed and the owned PostgreSQL container stopped. Exact
artifacts are `ct015406fb72/`. Coverage differs from the earlier run. All ordinary
sensor, configuration, RBAC and worker targets passed; the disclosure workload
passed 31 tests with two foreign-proof failures and three explicitly ignored
profiles. The graph-writer options barrier passed. Remaining failures include
file producers/seals, admission/default fixtures, raw file-data fixture paths,
readmission/options, actual core secret defaults and retained graph lock ordering.
New diagnostics identify PostgreSQL NUL-in-text path authorization during sealing;
F owns that root fix. File allocation still denies in secret-origin disclosure.

After source comparison, A's typed receipts/approval/async evaluator through
`3196c929`, B's default callers `eb9b0c2a`/`29faee35`, C's six consumer commits
through `aadc59ee`, F's four recovery/retention follow-ups through `25076190`,
and E's application packaging pins `f776ca23` were integrated. The C merge retains
F file-origin dispatch and prepares receipt frontiers before consumer capture;
B's canonical accepted-input traversal remains shared. These changes are NOT
covered by the 254 passes. The connected compile and fresh migration check run
centrally; A freshness/classification/snapshot APIs and B/F caller completion
remain active requirements.

G `e87ee0de` and H `626ddc60` fixture repairs are staged outside that compile.
G also found a fresh-deployment production gap: dispatch source IDs/digests are
captured without the managed dispatch metadata row required by D's foreign sensor
proof producer. H owns the coordinated deployment writer fix. Manually readmitting
rules in a fixture validates readmission, not a fresh deployment, and is not
counted as completion of the missing production path.

Fresh migrations for the connected typed-receipt/recovery batch passed on
PostgreSQL 18.6, including purpose receipts `01011`, native dependencies `01012`,
file readmission `06006`, PUT settlement `06007` and receipt retention `06008`.
The owned schema-runner container stopped. This verifies fresh installation only;
the explicit pre-06005 upgrade preflight, existing-reservation quarantine,
concurrent settlement and historical reader behavior remain unverified.

E's `e34f2c13` corrects the actual shipped secret-default failure without relaxing
the compiler: optional credentials have no literal default, omitted task inputs
stay absent, and supplied credentials retain parameter origins. It is integrated
through `integration-e-secret-default/`. Primary offline suites passed 14 workflow
and six composition tests. The new executor `core_pack_workflow_contract_tests`
then passed all six tests with no warnings, using the real flat compiler, parser
and renderer. This is pure contract evidence, not protected bootstrap or worker
lifecycle acceptance. The next consumer runner includes this executor target.

I's read-only audit in `remaining-acceptance-audit.md` confirms actual 22/22
collection API passes in the 254/38 run, including all 13 bulk tests and measured
three-statement repository pages at 1/64/100 rows. Those results do not establish
root cancellation, full-request latency or lifecycle acceptance. Newly assigned
owners are D for ordinary HTTP fresh scoped all-component deployment, I for root
execution-tree cancellation/race/query coverage, G for CLI/MCP input approvals
and SDK contract inventory, and E for an owned lifecycle acceptance runner.
Final generation/browser/SDK interoperability and combined validation stay central.

B's `622e606a`, `5a81efba`, `06ab4081` and `f6ecc205` are now integrated via
`integration-b-latest-fixtures/`: lineage-preserving fixture corrections, deletion
graph ordering and exact native ciphertext/receipt restoration. Unfinished native
producer scheduler edits remain outside primary. A new all-target check is running;
these changes have no fresh PostgreSQL behavior result yet.

That batch passed all-target workspace checking without warnings in 40.45s after
adding the missing Create trait import to its new test fixture. H's fresh producer
series `84a7e52f`/`c4bc19a1`/`af185f7a`/`584594dc` was then integrated through
`integration-h-fresh-metadata/`. Fresh deployment now prepares immutable action
metadata and dispatch bodies before activation, binding exact source releases
across redeployment. The merge retains A's final dependency/approval deadline
intersection AFTER capture waits. No historical missing evidence is regenerated
from current projections at read time.

D owns the older synthetic sensor fixtures that hand-built shortened dispatch
digests and inserted metadata after policy capture. They must use real source
capture under the new producer, not bypass it. The new H fresh foreign-sensor
deployment target and full options/readmission targets remain unrun. Another
all-target compile is active for this new production source.

The fresh-metadata batch passed all-target workspace checking without warnings
in 39.89s after removing one unused fixture import. A focused PostgreSQL 18 lane
is now running the full managed deployment metadata, managed component readmission,
managed execution-options and two bootstrap API targets. The runner records an
explicit coverage manifest. It excludes transport and broad repository suites;
it cannot substitute for their acceptance or an equivalent broad-run comparison.
Rust/SQL/Cargo/core source remains frozen during this lane. F's independently
identified allocation/seal repairs and other owner work continue in task worktrees.

### Focused deployment result and file validation follow-up

The frozen `ct0308310271` PostgreSQL 18.6 run finished with nine passes and two
failures. Source fingerprints were unchanged, SQLx preparation completed, and the
owned container stopped. Fresh foreign-sensor deployment passed without repair
PUTs. Both execution-options tests passed. Managed readmission still failed its
unchanged-policy assertion, and the actual-core bootstrap fixture failed metadata
validation for its supplementary queue. These results do not replace broad
repository, transport, or lifecycle acceptance.

Integrated subsequent owner commits remain uncommitted in primary:

- D `7decb275` and `f90d05d7` correct creation-time sensor fixtures and add eleven
  ordinary HTTP deployment tests. Positive install-only cases still depend on
  coordinated enablement and are not claimed as passing.
- F `c79c8088`, `4ca7ba4b`, `26925eff`, `1ab0d577`, `b94b0ce2`, and `ab533085`
  repair seal argument types and fixture provenance, carry prepared file receipt
  budgets, and add the legacy audit protocol and diagnostics. Prerequisite
  `589a717a` supplies bounded streaming and owned publication/shutdown work.
  Production legacy-auditor wiring remains incomplete.
- H `fa64ab19` normalizes parser defaults for semantic workflow no-ops and adds
  real input/target edit and historical-admission preservation assertions.

Central compile corrections use the installed tokio-util token import, a local
canonical test digest, and the actual repository model return types. Final file
allocation checks use the complete source proof after reservation. The resulting
all-target workspace check passed without warnings in 68 seconds, shell
`sh_142c94aaf001mK3pA9r3yucGP5`.

SDK primary checkouts retain their preexisting changes and have no new commits.
Isolated baseline snapshots were excluded from integration. Java `4c3787f` passed
49 offline tests centrally; JavaScript `a4212bc`/`6048184` passed type checking and
14 focused adapter tests; Python `0f3232cb` passed 198 tests with two explicit
generation-dependent skips. These are local transport contracts, not real-server
authorization evidence. Final public schema generation and mandatory generated
operation checks remain open. G's CLI/MCP commits `ac5b741a`/`dba4c078` are staged
outside primary pending A's API prerequisite and public-contract completion.

The next frozen PostgreSQL 18 run, shell `sh_142ca91fd001XYkWuGJFuWz9Kn`, covers
`file_evidence_retention_tests`, `legacy_file_audit_tests`,
`recorded_execution_events_tests`, and `managed_component_readmission_tests`.
Its outcome is pending. No production gates, pushes, or deployments were enabled.

### Receipt entry, public consent and remaining positive-path failures

The frozen file-focused run `ct0338541078` completed on PostgreSQL 18.6 with
94 passes and 24 failures. Source comparison was unchanged, SQLx preparation
completed, and the owned container was removed. Both controlled legacy-audit
tests passed. File evidence retention passed five of seven cases; two upgrade
fixtures could not locate their migration-history table. Recorded API consumers
passed 85 of 106 cases. The former seal NUL error was replaced by a concrete
graph-ordering error on publication: reclamation was entered after source/target
locks. F owns that correction. Worker seal/HEAD tests that stopped there still
do not establish their intended barrier behavior. The readmission target passed
two of three cases and reached a located planner-validation response mismatch.
H's subsequent `d2e2a2cc` checks the exact 422 diagnostic and public DTO decoding;
the new four-test target remains unrun.

After teardown, A's `224a9fa5` through `d36b937e` were integrated, adding accepted
input seals, receipt freshness/classification, collector predicates and system/item
proofs. I's `4560c656`/`b335ec27` add bounded cancellation and nine regression
sources. Its 18-statement assertion is not a measured result. F's
`f2e96f1a`/`f97889dc` add authenticated operator calls and signed management
transport, not an owning native provider controller. G's `ac5b741a`/`dba4c078`
and `3370e8c4`/`6a8c7df6`/`eac74ae0` supply CLI/MCP consent operations,
strict typed public schemas, visible retained target summaries and discovery.
Local compiler adapters preserve exact dependency edges and redacted errors.
This combined source passed all-target workspace checking without warnings in
251 seconds, shell `sh_142e758d4001nXMUYqJPe70HI8`.

E's queue correction and concrete lifecycle/provisioning/skew fixtures, commits
`2f7839be`/`f8f2bc13`/`d0c66508`/`a59a6033`, are integrated. All 44 offline
checks passed centrally. The actual live storage probes, compatible immutable
images and deployment-bound first operator remain prerequisites, not PASS stubs.
Web component/unit tests passed 443 tests across 72 files. Neither result is
browser or full lifecycle acceptance.

Initial OpenAPI export contained both new operation IDs and matched its source
fingerprint. Client generation is staged outside primary and existing generated
work is backed up. Python generation exposed three missing archive response
schemas. G's `440b4748` supplies truthful inline string/binary gzip bodies without
changing handlers. Its real `ApiDoc::openapi()` regression passed without warnings,
shell `sh_142f6e3d2001W8mfRLZy6dsG4H`, one test with two filtered out. Python's
durable generator configuration maps gzip to raw-byte interpretation; it does not
change the server media type. Generated-client acceptance still awaits regeneration.

B's six owned commits `f6373cae`/`3f148585`/`12747a43`/`c1b189c1`/`0d33ccad`/
`9ad108fa` and C's `7f25f3f2`/`c19dfe1a`/`555ace84` are now integrated. B's
creation fence supplies I's missing parent-lock prerequisite, and its native
initial-variable producer and initial seals are actual callers. Transition,
publication, output, cache and wait conversion remain incomplete. C's receipt
consumers retain an exposing regression for A's missing authenticated original-site
descriptor on accepted first-receipt source leaves; no current-row inference or
parent-evidence exemption was introduced. The next workspace check and corrected
OpenAPI export run in shell `sh_142ff9b3f001EAEURxxcjWDxdv`. All primary integration
remains uncommitted and every production enablement gate remains off.

### Cancellation, public consent and connected disclosure milestones

Frozen run `ct045236606f` completed 206 passes and 29 failures across seventeen
selected suites/filters. Frozen run `ct0559327335` then completed 138 passes and
14 failures across six targets. Both used PostgreSQL 18.6, retained unchanged
source fingerprints, completed SQLx preparation and removed their owned containers.
These coverage sets differ; their totals are not an equivalent broad comparison.

In the six-target run, all nine root-cancellation tests passed, including actual
creation/spawn races, independent descendant permissions, late approval expiry,
cycle/overflow rejection and an asserted eighteen helper application statements
at 1/64/100 nodes with 10,001 unrelated rows. That budget excludes API preflight,
source validation, root reload/precommit and message publication. No whole-request
latency claim follows. Public input approval API tests passed 3/3; actual-core
bootstrap transaction tests passed 6/6; file evidence retention passed 8/8.
The core result does not establish a matching native-inclusive release image or
live Compose/Helm acceptance.

All twelve worker live-stream cases and the protected HEAD barrier passed. Ten
file completion cases instead stopped at allocation or disclosure. New response
diagnostics expose `Runtime read evidence is unavailable or exceeds its bound`.
F identified a structural missing typed receipt parent in the frozen closure and
added a witness test; C owns the production repair. The separate File-B PUT denial
still needs an exact trace. Native initial variables also reached a purpose-receipt
mismatch after successful restoration. These supported positive paths remain open.

Generated clients were updated from measured schema
`3b49deb7912ea2211ab71d9ae6642b9b0910316a880d819abbba357cf8f28b7a`, preserving
existing generated work in an explicit snapshot and leaving handwritten web API
wrappers intact. Central JavaScript tests passed 97/97, Python 264/264 with zero
skips against the final spec, and Java regeneration compiled 693 sources with all
54 tests passing. Seven actual CLI approval integration tests and two MCP tests
passed using local transports. A mistaken CLI library filter matched zero tests
and is explicitly excluded from acceptance. These client results do not substitute
for server authorization or live lifecycle tests.

The web generator now preserves structurally tagged unions, including every
ApprovalPurpose and InvocationBinding variant. All 22 generation tests pass.
The input-consent management UI uses authorized retained-target discovery, exact
policy-bound issuance and confirmed purpose-preserving replacement. Central web
type checking and all 478 tests across 75 files passed. Real browser/server
acceptance remains unrun.

A's cache/wait snapshot, one-pass condition and staged-VALUE API batch is now
integrated, retaining standalone `e4149b8b` and excluding its duplicate equivalent.
B's independent/unconditional transition and empty-workflow output callers,
F's human final guard/private custody and repository-backed storage inspector,
and E's concrete storage-probe binding are also integrated. The combined all-target
workspace check passed without warnings in 249 seconds, shell
`sh_1436dd1b00015lwGYq1S5vQk5E`.

The new native-source PostgreSQL run is shell `sh_14372360d001F2sPpkJO0MdIND`.
Rust/SQL/Cargo/core sources are frozen during it. Native storage-parent projection
and retention wiring, complete B/C native consumption, immutable configuration
traversal, physical probe authentication compatibility and deployment-native
legacy settlement remain assigned unfinished work. No production gate, push,
primary commit or deployment has been enabled.

### Connected receipt delivery and remaining native composition

The native-source run `ct064200ac0c` completed 78 passes and seven failures.
Fresh native snapshot, wait, staged-publication and custody migrations applied
successfully on PostgreSQL 18.6. File evidence passed 9/9, including native archive
custody; operator audit API tests passed 2/2. Cache, wait and staged-value positive
cases still stopped on fixture or proof failures. These are not passing native
caller acceptance results.

After integrating C's original immutable-configuration traversal, authenticated
typed-parent freezing and keyed content guard, B connected the actual GET caller
and repaired its admissionless completion fixture. The workspace passed without
warnings in 60 seconds, shell `sh_1438e5c24001fVY5MC4vkmRnrL`.

The subsequent frozen run `ct071453f85a` completed 131 passes and seven failures.
Execution admission/completion passed 20/20. Recorded API tests passed 111/118,
including all three keyed GET regressions, the existing capture-before-plaintext
case, all nine cancellation tests and the recovered worker-file expiry/revocation
controls. Six file disclosure/publication cases still fail with recorded-origin
disclosure denial, and one queue payload-edit negative fixture returns 200 rather
than its expected 403. C/F own the exact file denial trace; B owns the original
queue acceptance fixture. Both runs retained unchanged source fingerprints,
completed SQLx preparation and removed their owned containers.

A's exact keyless native storage projection and E/F's canonical auth routes and
separate unscoped maintenance login are integrated. All 69 selected offline
lifecycle/probe checks pass. Live storage/provider acceptance remains unverified.
F's positive typed native-custody and File-B original-acceptance corrections are
staged, with shared-file conflict resolution still pending.

Actual B caller conversion exposed three missing proof compositions. A owns
prospective initial/linked cache frontiers and authenticated wait outcomes.
The atomic activation/refill model is retained; callers must not fabricate an
iteration or authorize sources after target locks. A dedicated J worktree owns
staged Condition/CONTROL composition, so a condition reading a pending VALUE can
control another publication without relabeling the condition as a Value or
bypassing unpublished-ancestry checks. No cache/wait/dependent-publication caller
completion is claimed. Primary integration remains uncommitted and gates remain off.

### Original-source file publication and prospective proof integration

Run `ct0749257d18` completed 159 passes and thirteen failures. The corrected queue
negative acceptance fixture passed, as did the original immutable configuration
bridge. Three staged-value/Condition tests stopped at schema setup, so neither
their proof composition nor expiry behavior was established. Six file cases still
failed before the subsequent C/F publication changes were integrated.

C's `a68a2d27` and F's `bbdfb3dc`/`1f7ecdfc`/`10a1f935`/`819a657b` were composed
as eighteen hash-checked paths. Publication now authorizes frozen original sources;
only an exact admitted producer can replay an authenticated sink capture without
mistaking its Pending destination for an SDK read. Ordinary reader permissions
and Ready checks remain. J's `ccecb5de` adds actual Boolean decision composition
and standalone freshness checks, retaining the true-only constructor and normal
unpublished-ancestry rejection. Its isolated fixture correction `03c3e53b` is also
integrated. No proof behavior is inferred from compilation alone.

The resulting file-focused PostgreSQL 18.6 run `ct082115eaf5` completed 133 passes
and three failures. Recorded API tests passed 119/122. Create-only multipart,
key-only SDK upload, recorded worker allocation, both exact-producer regressions
and the actual generic-read-between-external-PUT-and-Ready barrier now pass.
File evidence passed 9/9, graph-order retention 3/3 and operator API tests 2/2.
Two protected GETs still return 403 after publication, and File-B's fixture stops
on a missing value. C/F own those remaining traces. Both frozen runs retained
unchanged source fingerprints, completed SQLx preparation and removed their
owned containers.

A's `580f3000`/`aa42b806`/`0a8e51e3` and F's `6091f956` were then integrated as
one eleven-path batch. It supplies prospective initial/linked/EOF cache proofs,
authenticated native wait outcomes and forward frontier/epoch custody migrations
01017/06014. One child-first collector preserves pre-LIMIT pin exclusions and
counts each stored native row once. Compilation passed without warnings in
228 seconds, shell `sh_143e5f50f001dDv5uvJ1pdnINk`. Sixteen Python source checks
passed; these do not establish Rust or physical-provider behavior.

The native proof/custody validation is now shell `sh_143e9d364001BCs0f0fnOrhJTk`,
with Rust/SQL/Cargo/core sources frozen. Actual native caller conversion, native
reader adapters, live browser/SDK authorization and deployment-native settlement
remain incomplete. No production gate, primary commit, push or deployment is enabled.

### Recorded file routing and dependent completion acceptance

Run `ct0852399ffb` completed 88 passes and seven failures; `ct091023632f`
completed 119 passes and three failures. The latter identified the recorded
internal file GET/HEAD registration defect: their converted reader handlers
lacked the trusted recorded-consumer marker, so RequireAuth rejected admitted
tokens before handler authorization. The narrow fix adds ArtifactVersionRead
markers to GET/HEAD only. Selected Read/Decrypt, Ready, origin, body and lifecycle
checks remain; worker completion scopes and unconverted operations gain no access.

After that fix, A's cache freshness/terminal observation changes, B's actual
dependent completion callers, J's schema-coherent fixtures and F's full schema
initialization fixture were validated in `ct0937226e8e`. PostgreSQL 18.6 results
were 216 passes and three failures across four targets. Source fingerprints stayed
unchanged, SQLx preparation completed and the owned container was removed.

Recorded API tests passed 121/122, including protected chunk and successful EOF
barriers, internal recorded GET/HEAD, producer-only denial and scoped-reader
positive cases. File-B now reaches its remaining human-reader positive, which
returns 403. Execution admission passed 24/24, including all four real dependent
publication/overwrite/true-false decision/output caller cases. Runtime authority
passed 62/64, with native wait observation, cache freshness, J staged composition
and independent expiry cases passing. Its two failures are physical-custody tests
whose shared fixture first seals a source already marked Completed. F owns the
original-acceptance lifecycle correction. File evidence passed 9/9. No live
provider settlement or complete native caller coverage follows from these results.

A's `e1bd7cb6` is now integrated as six paths. It adds genuine non-Boolean selector
Control composition, an opaque cache-constructor adapter and forward 01020 binding
publication to the actual frontier. Existing J helpers and strict public replay
guards remain. The combined workspace check is shell `sh_1442ed23f001ahnK60XVqW4ith`,
pending. B still owns actual cache/wait caller conversion; C owns native reader
adapters. All primary changes remain uncommitted and every production gate stays off.
