# Self-contained pack implementation progress

> Paused for budget on 2026-10-16. The current handoff is
> [the budget checkpoint](self-contained-pack-budget-checkpoint.md), with deferred
> work in local issue #112 and children #113–#122. The older progress below is
> historical, not current readiness or authorization to resume implementation.

Date: 2026-10-15

The [statement of work](self-contained-pack-statement-of-work.md) is being implemented in stages. This file is a handoff and status record, not a claim that the full feature is available.

Parallel implementation is now authorized. See the
[coordination plan](self-contained-pack-parallel-implementation.md) for worktree
ownership, dependency handoffs, migration reservations and integration status.
Agents may commit in their isolated task worktrees; the primary integration
checkout remains uncommitted. All database validation in this cycle uses
PostgreSQL 18 only.

## Parallel integration in progress

The latest frozen repository lane completed with **94 passing and two failing
tests** on PostgreSQL 18.6. Its 746-file source fingerprint and primary HEAD were
unchanged, and the owned container stopped. Approval-expiry lock barriers, exact
event twins, managed sensor proofs, both worker completion cases and private
contributor input checks passed. A duplicate-task admission regression and an
initial sensor delivery fixture failed. D corrected its fixture without changing
production capture rules; B owns the duplicate-task fix. API/executor cases were
excluded because their compiler fixes were still pending.

After that freeze, coordinated bootstrap, metadata-pinned execution options,
collection selector parity and live stream source were integrated uncommitted.
Those additions are not covered by the 94 passes. The new consumer compile is
running, and all production gates remain off. Docker/Helm packaging was applied
uncommitted to their clean primaries, with 20 offline tests passing and no deploy.

Latest integration update: the second frozen PostgreSQL 18 lane finished with
72 tests passing and 12 failing, with its 715-file source manifest unchanged.
Retention corrections, payload-editor proofs, defaults retention and sensor
rollout passed. Nine completion tests stopped during worker fixture setup before
testing delivery behavior. Crypto-provider setup, a non-hex synthetic digest and
a malformed tagged API patch account for the other entry-point failures. Those
test adapters are corrected but require reruns. Fresh migrations and SQLx
preparation passed; no completion acceptance is inferred from setup failures.

The next source batch is integrated uncommitted: B through `a14f8e62`, C through
`d06c1d9e`, D through `14879f1b`, G through `620f6c7e`, H through `3d719150`, I's
five collection commits and A's approval-expiry helper. The combined web type
check and 80 tests passed. Rust compilation currently fails on scheduler future
lifetimes and API/test interface adapters. Earlier all-target passes do not cover
this batch. C's subsequent large-log/expiry delivery and F's durable file-origin
follow-ups are committed in task worktrees but not yet integrated.

All enablement gates remain off. Remaining work includes live/execution stream
delivery, native input propagation, recorded ad-hoc item admission, final expiry
caller integration, bootstrap parity and the full acceptance matrix.

Reviewed task-worktree commits are being applied as uncommitted changes to the
primary checkout. Integrated source includes the queue-attribution JSON codec,
trigger rollout repository and sensor/named-contract regressions, structural
artifact owner bindings, recorded artifact metadata/mutation consumers, notifier
recorded revalidation, candidate persistence/typed ingress/lifecycle, A's deployed
policy capture and action metadata, D's configuration/reset/UI/CLI hooks, B's
initial production admission and C's worker/input and generic read provenance.
The A/G integration milestone passed the workspace library and all-target checks
without warnings. Generated clients, web type checking and 42 focused web tests
passed. Its fresh PostgreSQL 18.6 migrations and SQLx preparation passed; focused
runtime tests finished with 77 passing and five failing. The immutable source
manifest matched after that run. Queue lifecycle expectations, a generated-column
fixture, foreign-code cleanup and two standard-artifact reads need fixes or
verification. Those results predate the newly integrated F/H completion and
managed readmission source, whose all-target check is now running. Supported protected artifact reads now record
immutable generic origins; unsupported historical/live-log paths remain denied.
Worker issuance and bootstrap admission gates remain off.

B is completing queue/execution API consumers; C is completing protected-read
provenance; F owns bounded worker completion and file publication. D continues
managed sensor metadata/readmission. A continues pinned foreign workflow graphs
and authored purpose-input authority. G owns structural human/sensor IAM contexts,
and H owns managed workflow/rule/queue readmission and finite dynamic targets.
E's candidate guard is integrated; its sibling Docker/Helm bootstrap packaging is
committed in task worktrees but held for working coordinated readiness. Exact
applied and held commits, prerequisites and evidence are in the
coordination plan. No agent commits were made in any primary checkout.

## Implemented foundation

- `crates/common/src/runtime_authority.rs` defines strict canonical permission-set/grant scope types, pack-owned envelopes, external approval snapshots, release-pinned invocation policies, reduction-only selection and historical requester/batch attribution.
- Local grants retain the actual managing pack ID. A matching ref prefix is not proof of target ownership. Global administration has no automatic local exemption.
- External grants require a covering approval for the subject pack and live approval validation at consumption. Human-relative conditions are rejected at this foundation interface until a caller can resolve them into explicit stable resource authority; they are not silently transferred to a system principal.
- `standard` selection is explicit and supplies no event/signing grant. Empty selection remains credential-free. Workflow/task/rule/queue bindings remain distinct.
- `repositories/runtime_authority.rs` captures immutable policy/admission records, checks release and runtime-owner matching, keeps idempotent creation bound to the original attempt, and provides transaction-scoped runtime credential fences.
- Human issuer/requester activity is not a consumption dependency. Only the real mapped system sponsor, execution liveness, recorded policy, and explicit approval/admission/policy revocation participate in these new fences.
- Migration `20261014000003_pack_runtime_authority.sql` adds approval/policy/admission persistence, immutable-record guards, irreversible revocation, and execution/enforcement admission cleanup. Historical issuer IDs and runtime owner IDs do not become retention-blocking human/execution foreign keys.
- Migration `20261014000004_runtime_authority_attempt_fences.sql` separates the current worker attempt from immutable admitted policy. Assigned running workers can rotate an attempt idempotently; retired attempt IDs cannot be restored to make old credentials valid again. Execution cleanup removes attempt/fence records through their admission.
- Migration `20261014000005_runtime_authority_release_dependencies.sql` records indexed source/target release dependencies for policy revisions and backfills existing revisions. Deferred release foreign keys prevent dangling executable pins while allowing same-transaction removal of a pack's own policies.
- Policy capture writes dependency rows in the admission transaction. Pack retention checks these pins during both selection and deletion. It prunes dormant revisions in bounded batches only outside the age/count rollback windows, never while a current-policy pointer or retained runtime admission refers to them. Completion or revocation alone does not erase retained authority evidence.
- `current_policy` resolves the pack's recorded active revision and rejects stale pointers after a release change. Replacing the current revision does not rebind admitted executions. Policy activation now rechecks external approvals so expiry/revocation between capture and activation cannot admit new policy.
- `repositories/runtime_target.rs` resolves pack-local target ownership from locked database records. It covers pack components, execution release owners, rule/queue/inquiry parents, canonical key/cache owners, artifact owner rows, runtime versions, and exact partitioned event rows. Event submission resolves a trigger rather than inventing an event. Platform/ad-hoc namespace association and historical ref prefixes do not grant local ownership.
- Validated runtime authority can match these opaque resolved targets directly. New-effect checks also require target lifecycle availability. Cache definition management cannot grant local authority over another pack's data; system/human-owned keys and caches have no automatic pack owner. Secret disclosure still needs separate recorded-origin checks.
- Policy capture now verifies each invocation against the immutable action snapshot in its selected release, including action ref and structural pack ID. A valid digest for an unrelated release cannot establish an executable binding. Direct action policies must bind to the sponsoring pack's executable.
- `InvocationPolicy::from_executable` assembles static invocation capabilities from a pinned executable and previously admitted permission sets. Omitted refs use pinned defaults; explicit empty refs disable API access. It rejects unadmitted names, mismatched bindings and set references, deduplicates refs, and records `standard` separately without manufacturing event/signing grants.
- External approval control endpoints now support issue, paginated inspection, immutable replacement and explicit idempotent revocation. Changes use fresh current issuer authority, preserve fixed historical scope, and emit ID-only audit details. Replacement revokes the predecessor without rebinding existing runs. API access is limited to access-token callers with subject pack read authority plus covering grants or permission-administration authority.
- CLI `pack approval`, five MCP approval tools, and the web External approvals page expose these controls. Destructive CLI operations require `--yes`, MCP operations require explicit confirmation, and the web requires confirmation. CLI JSON/stdin and offline dry-run are supported. Generated OpenAPI/web clients include the endpoints. See [external runtime approvals](../permissions/runtime-external-approvals.md).
- `pack_registry/runtime_policy.rs` reads candidate permission definitions and action/task/rule/queue selections with located diagnostics, validates unused external definitions, and assembles recorded policies from immutable executable maps. A's integrated deployment now calls it in the admission transaction with concrete source/target bindings and metadata revisions. Pinned foreign workflow graphs, finite dynamic action ceilings and purpose-specific sensor/input admission remain active implementation work; registrar exemption stays gated.
- Complete release action enumeration now reads immutable snapshots, including disabled declarations, without taking defaults from mutable action rows. Exact bundled platform permission definitions are excluded from local compilation rather than reinterpreted or republished. Foreign policy references require explicit fixed external approval; approval selection is deterministic by ID.
- The deployment approval helper reuses active fixed approvals without requiring the current installer to personally cover their runtime grants. Missing external grants are recorded automatically only when the installer actually covers them. Permissions/Manage alone requires a separate explicit administrator approval instead of silently becoming runtime delegation. A's integrated deployment calls this helper in the unified admission transaction; the remaining registrar live-IAM exemption is deliberately not enabled yet.

The registrar exemption and worker switch to recorded-token issuance remain disabled. Explicitly recorded pack tokens now have a typed signer/parser, RequireAuth admission/attempt/liveness checks, and converted event-submission, Key CRUD/list/read and signing consumers. Trusted route markers permit those consumers; unconverted consumers reject recorded tokens, and the IAM grant loader refuses to flatten their ownership envelopes. Existing worker-issued human-delegation tokens retain their prior path until the coordinated production conversion. Do not use a naked `AdmittedGrant::grant()` as an effective IAM grant and drop its ownership envelope.

## Recorded-token event ingress

- Recorded JWTs bind the system sponsor, execution, admission, selected refs, action and current attempt. Malformed pack metadata cannot fall back to human delegation. Empty selections cannot obtain this credential.
- RequireAuth checks current recorded authority before allowing a marked consumer. Event ingress repeats validation in the effect transaction, resolves/locks the real trigger owner, and requires selected Events/Create plus an enabled target. Neither current requester IAM nor requester activity is consulted.
- Secret-marked event fields receive bounded conservative execution-input provenance, using the existing secret-origin representation and aggregate-size limits. Sensor input provenance and fencing remain intact.
- Migration `20261014000006_execution_event_emitters.sql` stores immutable, digest-checked emitter attribution keyed by event ID and creation time. Execution/admission/sponsor/release/attempt and historical requester attribution are separate from the sensor `source` fields. There are no retention-blocking execution/identity/policy FKs. Event-schema orphan maintenance also reclaims emitter metadata in bounded batches.
- Recorded-token Key reads now resolve the actual locked Key owner and check the selected ownership envelope. Successful plaintext delivery records a bounded, digest-checked Key origin before returning it. Migration `20261014000008_execution_key_read_origins.sql` retains these records under the admission, with a separate per-admission write fence to avoid upgrading credential-validation shared locks. Repeated reads are idempotent. Secret event fields conservatively inherit these origins as well as execution-input origins.
- Recorded-token signing now validates execution/admission/attempt/sponsor/approval state in the signing transaction, checks selected Key Use or Read against the locked actual Key, and retains operator profile, subject and TTL checks. Key Use alone cannot retrieve plaintext material. Sensor signing also holds the actual Key row lock through signing. The general worker switch remains disabled.
- All Key CRUD/list routes now accept recorded credentials through the same transaction-scoped validation. Listing pushes actual managing-pack and pinned standard owner IDs into SQL before pagination/counting, retaining narrower selectors and private-identity-key restrictions. It never obtains permits from the requester's live IAM.
- Key creation resolves and locks prospective System/Identity/Pack/Action/Sensor owners before checking uniqueness. Foreign or missing owners are denied without disclosing whether a foreign Key already exists. The owner ref must still match the locked identity, so lookup/ref-reuse races cannot reinterpret ownership. Retired owners reject new creation; ordinary action disablement does not revoke an existing execution's data authority. No Key row is written merely to determine its prospective authority. The typed prospective-owner resolver now also supports cache namespace creation.
- Key update/delete hold source credentials and target locks through commit. Preserved schema/encryption transformations still require read and applicable decrypt rights. Update-only capability cannot disclose an existing value; authorized plaintext update responses record the resulting Key origin. Human/sensor grant loading happens before reserving an effect connection, and owner/uniqueness queries use that transaction rather than acquiring another pool connection.
- Authenticated non-sensitive schema discovery/inspection/preview now explicitly accepts recorded credentials, retaining the existing public-metadata contract rather than manufacturing schema or Key grants. RequireAuth still rejects terminal/revoked recorded credentials on those metadata-only routes.
- SDK Key origins now propagate into secret-marked results and raw stdout/error encryption. Before launch, recorded Key-read capability suppresses service-log mirroring. Direct runtime-log disclosure and batched artifact visibility include the retained Key origins. The latter uses a batch query without ciphertext. Schema-less unmarked application data remains the code-trust boundary.
- Migration `20261014000009_runtime_standard_owner_bindings.sql` records concrete action/pack owner IDs from pinned executable metadata for `standard`. Workflow-task bindings also capture their pinned containing workflow owners. The target resolver exposes actual owner IDs for Keys, artifacts and caches; standard matching is limited to its existing operation contract and never grants Events/Create or signing. The converted Key consumer now honors standard access without following a deleted/recreated action's ref.
- Recorded credentials now work across cache namespace listing/read/create/update/delete, generation inspection and refresh upload/seal/promote/abandon, snapshot scans and live lookup/mutation. Effects retain the execution/admission/approval fence and actual owner locks through their transactions. Namespace listing applies typed structural ownership and pinned standard IDs in SQL before pagination, retaining narrower grant constraints. Cloning requires both refresh and read authority. Pack-managed definition edits retain the existing API gate. Native executor cache iteration remains unconverted.
- Key and cache audits include ID-only execution/admission/sponsor/attempt context, without copying token metadata or input material.
- Event detail inspection projects immutable `execution_emitter` metadata only to access-token readers who can see the emitting execution, or hold unrestricted execution read authority. After execution retention, only the unrestricted check can establish visibility; missing ancestry never becomes permission. Requester and queue-item attribution are excluded from this projection. Event creation and sensor source fields remain unchanged. The web event detail shows the authorized historical admission, release, sponsor and attempt separately from the sensor source. The OpenAPI/web client has been regenerated.
- General worker minting and other API/notifier consumers remain outstanding. This is not the production-wide credential switch and does not yet tighten the old worker-issued event path. Concrete named-ref binding and end-to-end standard workflow/cache/artifact consumer coverage remain unfinished.

## Queue requester grouping

- `config.dispatch.separate_requesters` defaults to false in the canonical queue definition. Leasing therefore aggregates otherwise eligible requesters unless separation is explicit. Separation groups missing attribution separately without consulting human activity or personal grants. Release ID/digest and the entire pinned executable snapshot remain mandatory boundaries alongside existing payload coalescing. The dispatcher also rejects a mixed executable batch defensively, rather than running every item under its first item's action metadata.
- Migration `20261015000001_queue_requester_grouping_attribution.sql` preserves immutable enqueue-time requester grouping IDs without identity foreign keys. Deleting humans may clear the existing live attribution link but cannot merge their separated batches. Backfill preserves only still-known IDs; unknown history is not reconstructed from metadata or payloads. This descriptive ID never supplies runtime or inspection authority. Real PostgreSQL tests cover deletion of both requesters, unattributed anchors, aggregation/separation and rejected historical-ID mutation.
- The queue form exposes requester grouping outside advanced configuration, and inspection shows the effective choice. CLI `queue update --requester-grouping aggregate|separate` and `queue show` expose the same setting. API create/update/detail round-trip the config field. Pack-managed metadata retains its existing API-edit restrictions. See [work queue API](../api/api-work-queues.md).
- These controls do not transfer batch visibility or input authority. Production queue dispatch still requires homogeneous requester authority for API grants and Key resolution until recorded queue admission and input delivery are connected. Mixed-requester recorded execution sponsorship and full result/log privacy acceptance remain required.

## Independently safe deployment improvements

- Upload/register/install/batch registration preflight permits staging for an active caller with an Install grant, then checks the actual validated candidate pack ref before tests and again using locked current authority during activation. Scoped Install authority can therefore target a fresh pack without a preexisting database ID. Existing replacement and grant-delegation checks remain enforced.
- Ownership preflight collects all declared same-pack action refs. Workflow loading can resolve a later-declared nested workflow without depending on filename order. Final dependency validation still rechecks every edge after all projections are loaded; foreign private references still use the existing reference-visibility checks.

The standalone staged-download endpoint retains its existing authorization for now. These changes are not a blanket permission bypass or a complete candidate graph planner.

## Execution form selection

- The execute modal preserves omitted selection as action defaults without consulting `core.admin` or personal set membership. Missing personal membership no longer silently sends an empty selection or disables the default option. Explicit empty selection remains a token opt-out.
- Explicit rerun selections and named action defaults remain visible in custom choices instead of being silently filtered away. The backend still validates invocation and delegation; the form does not grant authority or enable pack admission.
- `GET /actions/{ref}/execution-options` now supplies pinned defaults and current admitted action choices without personal set membership. Without recorded policy it uses server-side effective delegation coverage, not a client `core.admin` special case. The modal no longer fetches IAM lists to infer runtime choices, and explicit rerun refs are never silently filtered or converted to an empty selection. The response is advisory; actual execution creation must still enforce admission. Full integration with A's managed metadata revisions and B's production admission remains required.

## Validation

For the remainder of this implementation cycle, the user requested PostgreSQL 18
only. Do not repeat the long database suites against a second PostgreSQL version.
The owned runtime-authority runner already provisions `postgres:18`.

- Initial `cargo check --all-targets --workspace` passed before changes.
- Updated-source `cargo check --all-targets --workspace` passed without warnings.
- 27 filtered pure runtime-authority tests passed on the updated source. They cover envelope confinement, exclusion of system/human-owned resources, explicit external approval, strict authoring, invalid human-relative scopes, reduction-only selection, distinct invocation bindings, standard/empty opt-in, attempt identity, tampering and mixed-requester attribution.
- Added real PostgreSQL repository tests for human-freeze/deletion independence, operational revocation, credential mismatch, completion, system-principal suspension, idempotence, retention cleanup, worker attempt rotation, and immutable/revocation database guards.
- Added API regressions for fresh scoped installation/denial outside scope and fresh nested-workflow forward references.
- All 11 PostgreSQL repository tests and both targeted API regressions passed against an owned disposable PostgreSQL 18 instance. Fresh-install migrations through `20261014000004` passed.
- The clean validation rerun passed all 11 repository tests, both API regressions, fresh-install migrations, and `cargo sqlx prepare --workspace -- --all-targets`. SQLx reported `no queries found`; these repositories use runtime queries, so no query-cache entries were generated. The owned PostgreSQL container was stopped.
- The preceding runner failure came from a redundant second migration invocation using the default search path after migration history had moved to `attune`. The corrected runner omits that invocation and sets the application search path for preparation. No application migration change was needed.
- Earlier validation exposed a test-only execution-status typo, a connection reborrow needed after adding fence persistence, and PostgreSQL readiness/TLS negotiation. These were corrected. The earlier Docker build captured the status typo before its correction and did not run tests.
- The release-dependency/current-policy stage passed all 15 authority repository tests, all 14 existing async release-pin tests, fresh migrations through `20261014000005`, the workspace check, and SQLx preparation. SQLx found no compile-time queries to cache.
- Target-ownership resolution and invocation assembly passed all 19 authority repository tests, six target-resolution tests, and 14 existing async release-pin tests against owned PostgreSQL. Fresh migrations through `20261014000005`, the workspace check, and SQLx preparation passed. SQLx found no compile-time queries to cache.
- Coverage includes structural ownership, system/human key exclusion, disabled targets, exact event identity, ad-hoc association, missing rows, resolved local/foreign event effects, executable-release binding, and pinned defaults after live edits. Validation caught and corrected a test expectation: reassembling identical capabilities correctly reuses the immutable revision ID.
- A stable-source validation pass passed the workspace check without warnings, 30 policy unit tests, four candidate-planner tests, two CLI tests, two MCP tests, 24 authority repository tests, six target-resolution tests, 14 existing release-pin tests, and all four approval API tests. Fresh PostgreSQL migrations and SQLx preparation passed. SQLx found no compile-time queries to cache. The owned database container was stopped.
- All five approval UI tests and web type checking passed. OpenAPI and the generated web client were regenerated. Earlier intermediate builds caught the API test harness's boxed-error return type and its default-token behavior; both test setup errors were corrected before the stable pass. Browser-level UI verification and full-suite validation remain outstanding.
- The recorded-token event stage passed a stable 111-test Rust validation sequence, including 21 JWT tests, three real API event tests, and the sensor emission/signing regression, plus fresh migrations through `20261014000006` and SQLx preparation. Tests proved requester-freeze independence, selected/local scope, standard denial, terminal/revoked credentials, stored emitter identity and bounded input origins.
- Additional stale-attempt/selection and emitter-retention/cleanup assertions were added after that pass. Latest validation was interrupted by concurrent changes elsewhere in the shared checkout: new `schema_consumer.rs` wrappers passed optional trigger/sensor pack IDs to functions requiring `i64`. The build also reported unrelated newly unused imports and an inquiry helper warning. These changes were not reverted or overwritten. The extra assertions and latest combined checkout still require validation after the concurrent work stabilizes. A preceding sensor binding Debug-derive mismatch was fixed by removing Debug from its disclosure wrapper, preserving the non-Debug sensitive binding.
- Subsequent validation supersedes that interruption: 112 targeted Rust tests, workspace checks with no warnings, fresh migrations through `20261014000008`, and SQLx preparation passed. The structural-ownership fixture now asserts the newer schema-consumer check rejects a forged namespace before testing a valid persisted owner. The duplicate migration number from parallel work was resolved by assigning the Key-origin migration number 8. One artifact-performance assertion was updated to the metric's current `payload_projection_reads` name.
- Signing conversion then passed all five recorded-consumer API tests and all six existing Key-usage tests on owned PostgreSQL. Tests cover external approval plus operator profile, requester/issuer freeze, no plaintext signing-key retrieval, terminal denial, Key-origin propagation, stale attempts/selections, emitter retention and orphan cleanup. Initial signing-fixture failures were corrected by explicitly assigning its issuer the exact Key Use grant before issuing the approval. Full workspace checking passed for the signing source before that test-only fixture correction.
- SDK output/log protection passed 11 secret-value unit tests, five recorded-consumer API tests, six Key-usage tests and all 26 non-stress artifact-read/performance tests. Direct and batched log tests prove execution decrypt alone cannot expose SDK-fetched Key material. Signing tests now also explicitly revoke their approval while the execution is running and reject the still-unexpired token.
- After concrete standard binding, all six recorded-consumer tests and six Key-usage tests passed, including sibling-action denial and deleted/recreated-action ref reuse. Workspace checking passed without warnings. The latest artifact suite passed 25 tests but failed its warm history statement budget, observing 7/7/19 statements for 1/64/100 executions, while graph reads were 3/3/6 and no ciphertext/payload was projected. The same suite previously passed twice after the SDK batch query was added. Do not treat the latest combined validation as entirely green; investigate variable authorization/cache queries before changing budgets. Migration 9 was exercised by API fixture migration, but the final full migration/SQLx sequence still needs rerunning.
- The final stable-source sequence subsequently passed all 156 targeted Rust tests, workspace checking without warnings, fresh migrations through `20261014000009`, and SQLx preparation. Two explicit artifact stress profiles remain ignored, not claimed as passing. This supersedes earlier interrupted compile/migration runs, but does not establish that the intermittent history-budget failure is fixed. Aggregate statement categories now distinguish graph, SDK-origin, identity, advisory and other operations without recording SQL or values. An instrumented failing run showed exactly two SDK-origin queries for two batches and eight additional other statements; isolated cache invalidation before the measured request did not reproduce the failure. Temporary invalidation probes were removed and the existing budgets were not relaxed. All validation containers were stopped by their owning runners.
- Later diagnosis reproduced that budget failure deterministically by replacing the owned pool's connections before the 100-execution sample. SQLx performed four `pg_catalog.pg_type` and four `pg_catalog.pg_enum` reads to discover connection-local enum metadata. The observer now reports those separately while retaining total statements and end-to-end timings. The unchanged numerical scaling budget applies to application statements, not driver catalog discovery. A regression keeps the cold-connection case, and a pure observer test proves application queries remain counted. The pinned standard-owner test now covers both the executing action and containing workflow, including ref reuse after deletion.
- Final validation after that diagnosis passed 157 Rust tests, the zero-warning workspace check, fresh migrations through `20261014000009`, and SQLx preparation. Two explicit stress profiles were excluded. This stable run supersedes the earlier intermittent-budget results. No production-wide admission/credential switch or deployment exemption was enabled. General production admission, remaining consumers, canonical IAM normalization, managed edits, configuration, queue aggregation, candidate isolation and bootstrap parity still prevent full-SOW completion.
- The subsequent execution-form correction passed all six modal tests and web type checking. New cases use a non-admin with no personal set assignments and verify omitted defaults, explicit empty selection, and explicit rerun refs reach the request unchanged. The server-owned execution-options endpoint is not yet implemented.
- The Key-consumer conversion passed the zero-warning workspace check, 164 targeted Rust tests, fresh migrations through `20261014000009`, and SQLx preparation. This includes eight recorded API tests, seven runtime target repository tests and two typed Key-filter tests. Three explicit artifact stress/reference profiles were excluded. Additional artifact tests from the shared checkout were preserved and passed. Creation tests cover owner scope, uniqueness ordering, requester freeze and terminal denial; mutation tests cover foreign denial, protected preserved material and write-only response redaction; pagination tests cover exact scoped totals and pinned workflow/action owner ref reuse. A compiler-only intermediate failure from generic borrowed SQLx Acquire futures was fixed by adding a concrete connection search entry point; the temporary diagnostic function was removed. No shared database resets, commits or deployment-exemption activation occurred.
- The subsequent schema-metadata consumer passed nine recorded API tests, all six Key-usage tests and 29 artifact correctness/performance tests on owned PostgreSQL. Three explicit artifact profiles remain excluded. Schema tests verify successful metadata listing/preview without extra runtime grants and terminal-token denial. Runtime caches, artifacts, execution/child creation, queues and notifier consumers remain unconverted; the full SOW is still incomplete.
- No shared database was reset, no production deployment was performed, and no commits were made. The existing uncommitted workspace changes were retained.
- The cache conversion passed 14 recorded-consumer API tests and all 36 cache API tests on owned PostgreSQL, plus workspace checking without warnings. Subsequent emitter inspection passed 15 recorded-consumer tests and the same 36 cache API tests. Its initial registration fixture incorrectly expected only HTTP 201; it now accepts the API's HTTP 200 response without logging returned credentials on assertion failure.
- Queue grouping passed 15 definition tests, three CLI tests, nine queue repository tests, all 14 release-pin tests and ten queue API tests on owned PostgreSQL. The release-boundary fixture now activates the action projection for release B, rather than only moving the pack pointer. Fifteen web tests passed across queue form/detail/edit and emitter-card tests, along with web type checking. The full updated validation sequence subsequently passed 259 targeted Rust tests, the zero-warning workspace check, fresh migrations through `20261015000001`, and SQLx preparation. SQLx found no compile-time queries to cache. Three explicit artifact profiles remain excluded. An earlier background run stopped because editing the running Bash script shifted its interpreter offset; the unchanged-runner foreground pass supersedes that incomplete run. All owned validation containers were stopped by their runners. This does not complete production admission, recorded worker issuance or the full SOW.
- The final executable-boundary tightening passed all 51 queue-definition/CLI/repository/API/release-pin tests on PostgreSQL 18, fresh migrations and SQLx preparation with all workspace targets checked. The release test now proves two different actions pinned to the same release are dispatched in separate batches, as well as separating old and new releases.
- The parallel-cycle coordinator's execution-options change passed eight modal tests, focused web lint and type checking, and its focused PostgreSQL 18 API test on the final source. It proves admitted choices are visible to an Execute-only invoker without internal set membership after issuer/requester freeze, while denied callers and runtime tokens cannot inspect them. Approval revocation removes the affected named choice; policy revocation denies the preview. Compilation including shared-target lock contention took 29m34s; the test took 30.50s. This does not prove the unconverted production execution-admission path accepts that choice yet.

## Remaining required work

1. Complete candidate ownership/resource mapping and hook the canonical policy parser/normalization into every set read, IAM expansion, loader, CLI export/import and inspection response. Database-backed runtime target resolution is implemented and validated; production consumers still need conversion.
2. Capture approved action/task/rule/queue policies and external approvals at deployment and managed metadata readmission; complete revision/current-pointer and resource-binding semantics.
   The new static assembler and released-action binding check are prerequisites, not live deployment integration. Ad-hoc/UI-edited executables still need immutable metadata-revision snapshots before this capture path can admit them; it currently rejects targets without a release action snapshot rather than silently trusting mutable metadata.
3. Port every production admission path, including early event-to-enforcement checks, both retry implementations, fanout/cache readers, queue dispatch and dormant callback helpers.
4. Connect the recorded-token signer and validators to all production admission/worker paths, including persisted standard scopes and bounded worker-owned completion delivery; convert remaining API/notifier consumers and native executor cache iteration. Typed recorded credentials and converted event/Key/signing/cache consumers are implemented, but the worker switch remains disabled.
5. Roll out the implemented recorded-token Events/Create, Key read/provenance, SDK output/log protection, signing and cache paths after production admission conversion. Historical emitter inspection is filtered in event detail; requester/item-level inspection and broader lifecycle acceptance remain. The old worker-issued token path is not yet converted.
6. Finish retention integration for future sensor/queue/candidate admission records and explicit pack-deletion controls. Source/target policy dependency pins and bounded revision cleanup are implemented and validated.
7. Implement consistent configuration defaults/overrides/reset, authority-bearing metadata readmission, and explicit incompatible schema rollout controls.
8. Connect the implemented aggregation-default/requester-separation controls to recorded queue sponsorship with origin-aware input and result/log protections. The grouping setting and leasing behavior alone do not complete queue runtime admission.
9. Add isolated real candidate API-effect context and unify bootstrap admission in both deployment modes.
10. Update frontend execution selection, CLI/API/MCP inspection and controls, regenerate clients, and complete owned migration/upgrade/concurrency/retention coverage.

Do not enable the pack-owned registrar exemption while these consumers still resolve live human IAM policy. That would expose incomplete semantics and leave the existing broad token-type event gate intact.
