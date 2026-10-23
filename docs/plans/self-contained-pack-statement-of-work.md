# Self-contained managed packs: statement of work

Date: 2026-10-13

Status: implementation paused for budget on 2026-10-16. See the [budget checkpoint](self-contained-pack-budget-checkpoint.md) and local issue #112 with children #113–#122 for preserved work and deferred tasks. The full statement of work is not implemented or enabled. Resuming it requires explicit prioritization and budget approval.

The user later authorized stabilizing the current branch for a release without
all SOW features. This authorizes targeted release-blocker fixes and validation,
not completion of the full SOW or activation of its experimental gates.

## Executive scope

Implement pack ownership as the source of admitted local runtime authority. A user authorized to deploy a pack can define and connect its managed components in one deployment, without personally holding each internal runtime grant. Actions retain selected, bounded permissions. Human invocation rights, runtime sponsorship, external approvals, and secret disclosure remain distinct.

This is a coordinated authorization and deployment change across common code, API, executor, worker, sensor admission, notifier, persistence, CLI, web, bootstrap, and generated clients. A registration exception alone is insufficient. The minimum useful delivery includes local deployment admission, recorded executable authority, every production execution-admission path, token consumption, event authorization, and revocation.

The work also makes candidate-graph binding and configuration behavior consistent. It does not promise that arbitrary pack code is sandboxed or that ordinary redeployment preserves every external dependency's behavior.

Related documents:

- [Recommended semantics](self-contained-pack-semantics.md).
- [Earlier implementation inventory](pack-owned-runtime-authority.md).
- [Current event authorization lifecycle](../permissions/event-submission-lifecycle.md).
- [Other deployment/binding findings](../permissions/deployment-binding-idiosyncrasies.md).

This SOW resolves two differences between the earlier plans: local runtime sets are ownership-bounded by default, and deployed external privileges use explicit recorded approvals rather than silently following the installer's current IAM grants. Ordinary ad-hoc human delegation remains a separate contract.

## Recommended baseline decisions

| Topic | Baseline to implement |
| --- | --- |
| Deployment permission | Keep Packs/Install for creation and Packs/Configure for authorized replacement. Resolve candidate pack scope before evaluation. Do not introduce an overlapping Deploy action for this project. |
| Local policy authoring | Pack-managed runtime sets default to a local ownership envelope. Normalize effective grants consistently before any consumer uses them. Explicit external/global scope requires approval. Platform definitions retain their declared platform scope. |
| Runtime selection | Keep `permission_set_refs` opt-in. Empty refs mean no execution API credential. `standard` retains its scoped resource contract and does not authorize events or signing. |
| Sponsorship | A stable, system-managed pack identity sponsors admitted local authority. Requester attribution does not become a local-grant coverage requirement. |
| Overrides | Execution requests can reduce selected authority. Broader local selections require a deployment-approved action/task/invocation binding. |
| External approvals | Record exact approved scope, subject pack, issuer, lifetime, and revocation state. Installer/issuer inactivity or personal IAM edits do not revoke or rewrite an approval. Revocation is a separate explicit operational action. |
| Release lifetime | Ordinary redeployment changes new admission. Existing runs and their admitted retries retain pinned code/policy unless explicitly revoked. Resource deletion, target disablement, or expired external approval can still prevent effects. |
| Effects after completion | Events, signing, child creation, and other runtime effects stop at terminal execution status. Any necessary final artifact/log delivery uses a separately defined cleanup path, not a general token grace period. |
| Configuration | Release defaults plus durable operator overrides, consistently for packs and sensors. Reset is explicit. Policy-bearing fields cannot be changed as ordinary value overrides. |
| Queue batching | Aggregate eligible items across requesters by default when batching is enabled. Provide an explicit, easily accessible requester-separation control in pack metadata, UI, CLI/API and documentation. Do not change batching eligibility based on human activity or personal runtime grants. |
| Managed API edits | Use the same admission planner and create an immutable metadata/authority revision referencing the existing code release. Do not mutate the authority of an already admitted execution. |
| Event target schema | Preserve current-trigger contract and current rule routing initially. Detect changed contracts with live admitted emitters and require a drain or explicit incompatible-rollout acknowledgement. Do not imply that pinned authority guarantees old payload compatibility. |
| Historical secrets | Preserve existing recorded-origin authorization. Missing retained origins fail closed; independent event/execution retention remains valid. |

Human account activity is a control-plane concern. Disabling a maintainer, uploader, requester, or approval issuer prevents their interactive access and new personally authorized requests; it does not suspend admitted automation, queued work, retries, external approvals, or runtime secret delivery. Operational suspension/revocation of a pack's system principal remains an explicit, separate action.

Distinguish the human maintainer who may change the pack, the pack that structurally owns its components, and the system-owned runtime principal that exercises recorded capabilities. Ownership transfer changes management rights, not runtime authorization.

These are the agreed behavioral decisions, not claims that the repository already implements them. The agreement log records changes made during the walkthrough. Engineering design items at the end remain to be resolved without changing these behaviors.

### Agreement log

Updated 2026-10-14. Behavioral agreement is complete for the choices discussed below. The user subsequently requested implementation; progress is tracked separately.

The user agreed to these foundational decisions during the design walkthrough:

1. Pack-defined runtime permissions are pack-local by default. External/global scope requires explicit authorization.
2. Deployment authority implies control over the pack's runtime capabilities and secrets delivered to its code. Separately controlled packs and external approvals provide stronger trust separation.
3. UI/API edits to deployed components remain supported. Authority-affecting edits use the same admission checks and create immutable metadata revisions without changing existing executions.
4. Deployment automatically records external approval when the installer already has authority to grant the requested scope. Otherwise, separate approval from an authorized owner or administrator is required.
5. External approval is recorded for the pack, with fixed scope, explicit revocation and optional expiry. Later changes to the installer's unrelated personal IAM permissions do not silently rewrite it.
6. The earlier agreement to revoke external approvals when their human issuer is frozen is superseded by the user's operating-system continuity requirement. Human inactivity only removes their interactive access; issued approvals and admitted automation continue until explicit operational revocation, expiry, or another configured operational event.
7. Ordinary action disablement stops new invocations; already admitted runs and their retries may finish. Explicit cancellation/revocation stops existing work. Disabled effect targets still reject effects, including events for disabled triggers.
8. Completion/cancellation ends execution-token access. Necessary final artifact/log delivery uses a bounded worker-owned cleanup mechanism, not a general execution-token grace period.
9. Pack ownership primarily governs who changes configuration and behavior. Runtime principals and recorded capabilities are independent of human maintainer/uploader/requester/issuer activity. Human ownership transfer or inactivity must not change deployed behavior.
10. External identity providers may invalidate personal credentials independently of Attune. This is an intentional, accepted limitation; automation should use service credentials where possible. Attune must not itself couple admitted automation to human inactivity.
11. Pack/sensor configuration uses release defaults plus persistent operator overrides. Objects/lists replace a whole schema field; reset is explicit. Preserve existing values as overrides when source history is unknown.
12. Block incompatible trigger-schema rollout while affected executions are active, unless the operator explicitly acknowledges the incompatibility. Draining is the safe default; ordinary deployment confirmation is not acknowledgement.
13. Queues aggregate eligible batches across requesters by default. Requester separation is an explicit opt-in control, easily accessible and documented. This supersedes the proposed requester-separation default. Preserve per-item attribution and secret-origin/visibility protections without automatically splitting a batch merely because it has multiple requesters.
14. Candidate tests may exercise real Attune API operations using isolated candidate resources/events, never active production targets, rules or notifications. Ordinary tests need not opt in.
15. Cover all currently supported pack-owned resource kinds through the common ownership model. Keep explicit gates for unsupported features rather than bypassing them.

No further product-behavior agreement is currently outstanding. Concrete data representations, endpoint/field names, query/lock design and equivalent safety mechanisms remain engineering work. Surface any newly discovered conflict with these agreements rather than silently changing the behavior. Implementation was authorized by the subsequent explicit user request, not by agreement alone.

## Work package 1: policy semantics and resource ownership

**Deliverable:** canonical policy types, supported local scope mappings, normalization, and admission tests.

Affected code:

- `crates/common/src/rbac.rs`: `Grant`, `GrantConstraints`, matching, and containment.
- `crates/common/src/delegation.rs`: preserve human/external delegation proof; do not add a universal pack-install bypass to `covers`.
- `crates/common/src/models.rs`: permission-set and authority models.
- `crates/common/src/pack_registry/permissions.rs`: definition parsing, requested uses, and source locations.
- `crates/common/src/repositories/identity.rs`: every set read and direct/role effective-grant expansion.
- `crates/common/src/platform_catalog.rs`, compiled catalog definitions, and matching core-pack definitions.

Tasks:

1. Represent local and explicitly external policy modes in the canonical v1 authoring contract. Use one typed definition parser for loader, validation, API, and CLI export/import.
2. Compile local grants under the actual managing pack identity/ownership envelope. Preserve authored selectors as narrowing constraints; reject selectors escaping local ownership unless explicitly external and approved.
3. Establish resource-specific scope meaning. Do not simply add `pack_refs` to every grant: some consumers use owner-qualified refs, and current validation rejects combining `refs` with `pack_refs`.
4. Keep normalized effective policies and authored definitions distinguishable for inspection. Raw authored grants must never feed IAM or runtime matching without the ownership envelope.
5. Classify mixed local/external sets per grant and require all external portions to be approved. Unused local definitions should be admitted under local ownership; unused external definitions still need publication authority.
6. Preserve platform-owned immutable policies. Installing `core` must not reinterpret `core.admin` as pack-local or overwrite its catalog ownership.

Minimum ownership mappings:

| Resource/operation | Local proof |
| --- | --- |
| Events/Create | Resolved trigger owner belongs to the sponsoring pack; narrower refs select triggers, not event IDs. |
| Actions/Execute and action metadata | Target action is pack-owned, with existing invocation and reference-visibility rules retained. |
| Rules, triggers, policies, queues and queue items | Actual managing pack or owning queue, including authorization of introduced foreign relationships. |
| Keys | Actual pack/action/sensor owner-qualified scope. Human identity and System keys are not made local by name. |
| Artifacts | Actual owner scope plus existing authoritative execution-linkage visibility and secret disclosure rules. |
| Caches | Actual namespace owner, mode, and generation constraints; retain quotas and iteration rules. |
| Pack configuration and local runtimes | The managing pack and supported operations, not global index or worker administration. |
| Types and workflows | Candidate ownership plus their existing reference/contract rules; unsupported named-schema consumers remain an explicit gate. |
| Identities, IAM assignments, workers and other global administration | No automatic local exemption. |

Concrete declared refs can bind stable component identities. An ownership-scoped grant intentionally applies to eligible resources in that owned namespace, including resources created later. Keep these two target modes distinct; do not accidentally enumerate an open owner scope or authorize ref reuse across owners.

Impact: existing custom unscoped policies become local rather than global under the new default. Accept the narrowing as a new authoring behavior, but require an inventory/diff and explicit external approvals for intended broader policies. Do not silently grandfather global rights based on `installed_by`.

Evidence: `PackPermissionPlan::validate` currently checks registrar coverage for definitions and uses. `IdentityRepository::authorization_state` directly expands `permission_set.grants`; repository readers return the same raw JSON. `GrantConstraints` represents several different scope mechanisms. These are coordinated change locations, not just YAML parsing sites.

## Work package 2: complete candidate graph and scoped deployment admission

**Deliverable:** one deployment admission module returning an immutable, located deployment plan.

Affected code:

- `crates/common/src/pack_registry/loader.rs`: `load_all`, ownership preflight, `load_actions`, `load_workflow_for_action`, and dependency checks.
- `crates/common/src/pack_registry/permissions.rs`, `schema_types.rs`, `dependency.rs`, `diagnostics.rs`.
- `crates/common/src/action_visibility.rs` and managed-component ownership repositories.
- `crates/api/src/routes/packs.rs`: install/upload/register/batch entry checks, `authorize_pack_registry_action`, replacement authorization, staging, and activation.
- `crates/common/src/pack_check.rs`: deterministic offline structural checks and located diagnostics.

Tasks:

1. Read all declarations once into a candidate identity/ownership graph, retaining authored files and field paths.
2. Check scoped installation against the validated candidate ref, including a fresh pack with no database ID. Preserve authentication, archive limits, approved source-host restrictions, and early resource protections while deferring the target-specific check.
3. Resolve same-pack forward references before binding. Reserve IDs or use transaction-private candidate identities; do not expose half-defined active rows.
4. Resolve nested workflow edges independently of filenames. Distinguish invalid compile-time cycles from deliberate event/rule feedback loops; do not require the entire automation graph to be acyclic.
5. Prove actual ownership and preserve private/restricted cross-pack referenceability. A human administrator is not an override for a foreign private component.
6. Recheck existing ownership, replacement authority, configuration revision, and external approvals in activation. Preserve pack/admission locks and protect against concurrent edits.
7. Compile per-executable, per-task, rule-dispatch, and queue-dispatch bindings from the approved graph.
8. Commit active projections, authority revisions, schema contracts, and configuration together. Filesystem/object publication failure after database commit must report the true committed outcome and recover through existing install/projection mechanisms.

Impact: some deployment errors move earlier and become independent of file order. Accept this. Preserve stable component IDs and references across redeployment. Extend existing diagnostic structures rather than introduce another error format.

Evidence: `authorize_pack_registry_action` currently supplies no candidate pack scope. The loader separates ordinary actions from workflow actions, but workflow binding still queries partially loaded projections. Existing `DeploymentDetails` and `PackDiagnostic` already support source-aware failures. API activation already provides the transaction/publication distinction to preserve.

## Work package 3: immutable authority persistence and external approvals

**Deliverable:** repository-owned records for admitted metadata, selected runtime authority, sponsorship, and revocation.

Affected code:

- `crates/common/src/models.rs`.
- `crates/common/src/repositories/pack_release.rs`, `executable_snapshot.rs`, `execution.rs`, `rule_authority.rs`, `identity.rs`, and component lifecycle repositories.
- New narrowly scoped admission/approval repositories, if existing records cannot carry the contract cleanly.
- `migrations/`, SQLx offline query metadata, repository fixture/builders, backup and retention paths.

Persist at least:

- Admitted pack/component metadata revision and source digest, tied to immutable code release ID/digest.
- Runtime sponsor identity and real pack ownership mapping, not editable identity attributes as trust evidence.
- Action/invocation/task binding, selected refs, normalized local grants, target modes and stable target identities.
- External approval IDs and immutable approved grants, with explicit subject, issuer, expiry and revocation state.
- Per-execution selected admission and attempt/fence identity, distinct from requester attribution.
- Purpose-specific input/signing bindings and their provenance, without merging them into unrestricted runtime API grants.

Tasks:

1. Extend the canonical v1 contract in place. Do not introduce v2, dual readers, or mutable replacement of existing release bytes.
2. Preserve immutable executable releases. Managed API edits create a new admitted metadata binding referencing those bytes, not a second snapshot under an existing immutable unique key.
3. Atomically attach admission to executions/enforcements and preserve create-if-absent idempotence. A duplicate creation must return the original binding rather than reauthorize the same ID with different grants.
4. Add current revocation state/fences separate from immutable policy. Routine activation must not masquerade as revocation.
5. Preserve plain retained runtime IDs where existing independent retention requires them. Do not add an execution foreign key to partitioned event attribution.
6. Add cleanup/pinning so admissions, target contracts, configuration origins and required releases survive while referenced, then reclaim safely. Avoid retaining every execution forever.
7. Update `SELECT_COLUMNS`, history triggers/digest summaries for mutable execution fields, and SQLx prepared queries when schema changes are implemented.

Impact: more database records, retention dependencies, and index work. Mitigate through immutable-policy reuse, compact per-run selections, and existing retention/ownership patterns rather than copying the full pack policy into every token.

External approvals are a substantive new behavior. Required control-plane operations are issue, inspect, renew/replace, and revoke. Validate human issuer authority when granting or changing an approval, not as an ongoing runtime dependency. Freezing that human does not revoke issued approvals. A security response can explicitly revoke selected approvals, but must not conflate this with disabling login.

Evidence: `pack_runtime_identity` and immutable input authority already exist in `20261013000014_pack_rule_input_authority.sql`. `component_configuration_revision` has immutable revisions/current pointers. `PackReleaseRepository::activate_projected` captures executable/schema metadata, and action snapshots currently lack resolved runtime authority. Current pack identity assignments accumulate across capture/refresh.

## Work package 4: all production execution-admission paths

**Deliverable:** one runtime authority module and complete conversion of production admission paths.

| Path | Required locations and changes |
| --- | --- |
| Manual API and execution-created child | `crates/api/src/routes/executions.rs::create_execution`: preserve Actions/Execute, use target binding rather than caller coverage for local policy, reject a missing/invalid originating execution rather than fall back to JWT subject. |
| Event to enforcement | `crates/executor/src/event_processor.rs`: replace the early identity-ref check and capture invocation authority with the enforcement. This happens before the enforcement processor. |
| Enforcement to execution | `crates/executor/src/enforcement_processor.rs::create_execution`: inherit the recorded binding, preserving input authority, attributed owner, pinned snapshot and idempotence. |
| Queue dispatch | `crates/executor/src/queue_dispatcher.rs`: separate pack-sponsored API capability from requester homogeneity; preserve item attribution, leases, release grouping and secret-input authorization. |
| Ordinary workflow children | `scheduler.rs::dispatch_workflow_task` and `_with_conn`: admit rendered selections against the child action and task binding. |
| Fanout and cache batches | `scheduler.rs::dispatch_with_items_task`, `_with_conn`, and `refill_cache_iteration_with_conn`: use the same admission contract and transactional parent/iteration protections. Terminal cache sentinels remain credential-free. |
| Executor cache reads | `scheduler.rs::initialize_cache_iteration_with_conn` and `authorize_cache_iteration_read`: consume recorded read authority, not mutable set bodies or accidental parent-ref inheritance. Preserve namespace/generation protections. |
| Both retry paths | `crates/executor/src/retry_manager.rs` and `scheduler.rs::maybe_retry_workflow_task`: preserve originating policy, target bindings, schema and secret origins; give the new execution its own attempt identity and recheck explicit revocation. |
| Completion callback helper | `crates/executor/src/execution_manager.rs::trigger_child_executions`: stop copying parent refs to another action. The discovery hook is currently dormant; harden or prohibit it before enabling it. |
| Persistence variants | `ExecutionRepository` create/pinned/retry/workflow/create-if-absent methods: require a server-owned admission result at the production boundary and persist it transactionally. |

Keep ad-hoc human delegation as a separately tagged authority source. Do not change `DelegationAuthority::covers` so that arbitrary metadata or action refs can claim pack sponsorship.

Human-supplied authority for an accepted request, including requester-specific input access, must be captured into its accountable admission before deferred dispatch. Runtime consumption uses that recorded, bounded authority rather than rechecking whether the submitting human remains active. Human authorization still governs whether they can submit a new request or modify an existing approval. Explicit admission/capability revocation remains possible.

Requester and sponsor must be separate data concepts. Manual execution keeps its human attribution. A mixed-requester queue batch needs explicit batch attribution and an authorized pack sponsor, not the first requester or identity 1. Aggregate eligible items across requesters by default when batching is enabled; do not impose an implicit privacy/requester partitioning default. Expose an explicit requester-separation mode for operators who want it. Preserve actual queue/action/release eligibility and per-item approved input/secret-origin checks. Reject genuinely unauthorized input without granting access from another contributor. Full-batch disclosure must not become readable to one requester merely because they supplied one item; runtime aggregation and permission to inspect results/logs are separate.

Impact: callers without personal local runtime grants can invoke approved effects, and queues/workflows no longer depend on accidental shared IAM assignments. Accept these as the requested behavior. Preserve invocation authorization, private references and secret disclosure as safeguards.

## Work package 5: credentials and every runtime consumer

**Deliverable:** execution credentials bound to recorded admission, with consistent live validation.

Affected code:

- `crates/common/src/auth/jwt.rs`: execution token generation/claims.
- `crates/worker/src/executor.rs`: minting, explicit execution identity, standard context and environment delivery.
- `crates/common/src/execution_env.rs`, worker parameter/environment validation.
- `crates/api/src/auth/middleware.rs::RequireAuth`, `crates/api/src/authz.rs::load_grants_for_token` and runtime delegation helpers.
- `crates/notifier/src/websocket_server.rs`: upgrade, context, fingerprinting, delivery authorization and expiration loop.
- Internal executor consumers such as cache iteration, not only HTTP routes.

Tasks:

1. Add admitted-authority and execution-attempt identity to canonical claims or the validated server context. Match execution ID, subject, action, selection and authority against the recorded admission.
2. Resolve local/external-approved runtime grants from recorded policy. Access-token IAM remains separate. Ref strings alone cannot select broader live authority.
3. Validate current execution state, system runtime principal/sponsor, expiry and explicit operational revocation even on protected routes that use `RequireAuth` without loading resource grants. Do not check human maintainer/requester/issuer activity as an execution-liveness dependency. Bind credential subject to the system runtime principal; keep human attribution separately recorded. Existing identity-equality checks must follow that distinction rather than assume requester and runtime subject are identical.
4. Use snapshot-owned pack identity rather than deriving policy-sensitive ownership from action-ref text.
5. Preserve environment clearing, stdin JSON, reserved `ATTUNE_*` protection, and no token for empty refs. Recommended behavior is to fail preparation if an admitted API-dependent execution cannot obtain its required token, rather than knowingly launch without one.
6. Update notifier contexts and memoization keys with execution/admission/attempt identity. Revalidate before protected deliveries and close revoked contexts; checking only at upgrade or JWT expiry is insufficient.
7. Cache immutable policy only. Bound live-validation queries and specify failure-closed behavior under database or revocation-state unavailability. A best-effort MQ invalidation cannot be the only revocation mechanism.

Impact: valid but mismatched, terminal, revoked, or missing-execution JWTs stop working before their cryptographic expiry. Accept this tightening. Any cleanup exception must be operation-specific and bounded. Existing WebSocket connections may close on revocation; document and test reconnect behavior.

Evidence: worker minting currently uses refs and timeout plus 60 seconds. `RequireAuth` fences sensors but does not fence executions. API grant resolution reloads mutable named policies. Notifier stores upgrade-time grants in `WebSocketAuthContext`; its fingerprint lacks execution identity. Worker identity resolution already fails closed despite a stale identity-1 fallback comment.

## Work package 6: scoped events, signing, provenance and transaction order

**Deliverable:** target-specific effect authorization with attributable, secret-safe event records.

Affected code:

- `crates/api/src/routes/events.rs::create_event` and `crates/common/src/repositories/event_schema.rs::materialize_current`.
- `crates/common/src/repositories/event.rs`, event DTO/OpenAPI, audit and visibility/disclosure projections.
- `crates/api/src/routes/keys.rs::sign_key_jwt`, key access and operator signing profiles.
- Shared admission/revocation validation plus cancellation, sponsor freeze, target retirement and capability-revocation writers.

Tasks:

1. Keep Sensor/Execution-only direct event ingress. For executions, require selected Events/Create on the resolved trigger identity and owning pack.
2. Preserve sensor trigger scope, generation, membership and lease fencing. Do not make sensor emission depend on the original installer's current grants.
3. Reuse the event schema transaction's graph selection and trigger locks. Add explicit retired-target, execution-attempt, sponsor and approval checks under a documented lock order shared with their writers.
4. Define revocation by transaction ordering: a revocation committed before the admission check denies the effect; an effect admitted first may commit before revocation. Do not promise retroactive wall-clock cancellation of already admitted writes.
5. Add separate execution-emitter, admission/sponsor/release/attempt attribution. Preserve existing sensor `source` meaning and event immutability; descriptive attribution must remain useful after execution retention.
6. Propagate a bounded conservative closure of execution input secret origins into secret-marked event fields, analogous to sensor origin propagation. Emitter metadata alone does not protect forwarded secrets. Unmarked arbitrary bytes remain a code-trust issue, not something the platform can infer reliably.
7. Apply the same consumption fence to signing. Preserve operator-approved profiles, bounded assertions, and no private-key disclosure. Keep current Key Read compatibility initially; prefer Key Use in new policies. `standard` must not acquire signing authority.

Impact: actions receiving only `standard` or unrelated grants can no longer submit events. Accept this security correction; update packs to select an admitted event policy. Some expired historical origins will keep plaintext events redacted after source retention. Accept fail-closed disclosure rather than inventing origin ownership from current metadata.

Already issued assertions accepted by another service cannot be withdrawn by Attune alone. Mitigate with short TTLs and recipient-side revocation support where available; explicitly accept this limitation.

Evidence: execution ingress currently has no Events/Create check and stores no execution source. Sensor ingress propagates captured input origins; execution ingress does not. `materialize_current` already locks/reloads the trigger and selects its current contract. Signing currently applies profiles and identity/action checks but needs the same execution lifecycle fence.

## Work package 7: one configuration/default/override model

**Deliverable:** consistent pack and sensor effective configuration, migration and reset semantics.

Affected code:

- `ComponentConfigurationRepository::{bind_deployment, authorize_preserved_migration, bind_pack, bind_sensor, bind, pack_template_values}`.
- Pack/sensor loader config parsing and projections in `pack_registry/loader.rs`.
- `crates/api/src/routes/packs.rs::{create_pack, update_pack}` and sensor create/update in `routes/triggers.rs`, corresponding DTOs.
- Configuration revisions, secret-value persistence/retention, template rendering, sensor generation admission and configuration UI/CLI.

Tasks:

1. Persist authored release defaults separately from operator overrides and effective bound revisions. The current effective/redacted value alone cannot establish which fields were overrides.
2. Define top-level schema-field overlay semantics, explicit null, reset paths, list/object replacement, and unknown-field validation. Avoid undocumented recursive merging.
3. Preserve sensor overrides just as pack overrides. A missing YAML config field no longer resets sensor-local values to `{}`.
4. Apply the same semantic-contract comparison and migration authorization in deployment and direct APIs. Equivalent resubmission should reuse the binding; incompatible preserved values block activation with sanitized field diagnostics.
5. Classify authority-bearing sensor config fields, including input/cache/signing permission refs, as admitted metadata. Changing them must readmit affected bindings; they are not ordinary operator value overrides.
6. Require code/schema/secret-consumer edits to receive appropriate readmission. Remove broad unrelated input-authority requirements from truly metadata-only edits, while preventing a patch from changing executable behavior under an old authority record.
7. Support explicit reset without returning or duplicating plaintext secrets. Revision storage, templates and audit must retain the actual origin of effective values.

Impact: YAML default changes can become visible where no operator override exists, and missing sensor config no longer wipes settings. Accept these new semantics. Existing values need an explicit conversion policy: safest is to preserve current effective user-visible values as overrides unless trustworthy source history proves otherwise, then allow operators to reset selected fields to new defaults.

Evidence: current pack redeployment preserves config, while sensor loader supplies replacement config. Binding already supports semantic reuse and immutable encrypted revisions. Pack PUT has a different preserved-secret-schema authorization check. Sensor update lives in `routes/triggers.rs`, not a separate `sensors.rs`. Authority selectors currently coexist with ordinary sensor values under `config`.

## Work package 8: dependency evolution, omission and managed edits

**Deliverable:** explicit behavior for changing contracts/components while runs are pinned.

Affected code: component lifecycle and release repositories, schema contract/reference repositories, action/trigger/rule/queue/workflow update routes, input authority revisions, admission reference tracking, supervisor reclamation and deployment diagnostics.

Tasks:

1. Keep `remove` as normal desired-state omission, with clear errors for active referencers left behind. Preserve explicit `retain`/`disable` choices but display exactly which policy remains active; do not silently change the meaning of an existing override.
2. Stop treating accumulated pack identity assignments as executable authority. Prune deployment-owned obsolete assignments where appropriate without deleting manual or unrelated assignments; track ownership if necessary.
3. Readmit managed component and policy edits into immutable metadata revisions, atomically updating affected future executable bindings. Labels and non-authority configuration overlays can remain lightweight.
4. Keep same-pack/cross-pack reference policy separate from grant admission. Changing a referenced component's owner or identity must invalidate concrete bindings, not silently follow reused names.
5. For changed trigger contracts with live admitted emitters, report affected executions and require draining or an explicit acknowledgement of incompatible rollout. Recheck under activation locks. Keep current event contract/rule routing for the initial delivery.
6. Report named-schema consumer rollout gates honestly. Reuse existing type preflight/compiler machinery; do not disable gates or build a new schema compiler merely to permit local authority.

Impact: pinned runs retain authority but can still fail against a deliberately changed or unavailable live target. Accept this after an explicit rollout choice. Release-coherent event routing and universal schema-compatibility proofs are separate projects, not hidden prerequisites claimed as already solved.

## Work package 9: candidate tests and bootstrap parity

**Deliverable:** the same admission semantics for install testing and fresh installation.

Affected code:

- API pack test/staging/install handlers, `crates/worker/src/service.rs` candidate handling, `crates/common/src/test_executor.rs`, pack test models.
- `scripts/bootstrap_core_pack.py`, `scripts/load_core_pack.py`, `docker/init-packs.sh`, `docker-compose.yaml`, API startup/release-upgrade readiness.
- Sibling `attune-docker/docker-compose.yaml` and `attune-charts/charts/attune/templates/jobs.yaml`, chart values/readiness tests.

Tasks:

1. Candidate tests currently use `TestExecutor`/`pack_test_execution`, not normal executions, and do not receive execution JWTs. Keep archive-download credentials separate from runtime authority.
2. Provide an isolated candidate API-effect context if tests must exercise event emission. Recommended first mechanism is a server-owned candidate ingress context with separately isolated target IDs/events, excluded from active rule matching and notifications. Ordinary tests remain offline unless explicitly opted in.
3. Fence candidate contexts to the install/test lifetime and clean them up. A finished, rejected or expired candidate cannot leave a usable production capability.
4. Route bootstrap through ordinary API admission where possible. Replace or constrain the partial direct-database loader; release freezing alone does not create admitted authority.
5. Resolve startup ordering explicitly: platform readiness permits controlled bootstrap without requiring already deployed content, while application content readiness waits for a fully admitted core pack. No startup credential based on editable identity attributes.
6. Test both shared-volume and object-backed packaging paths, hook behavior and upgrade failure outcomes. A failed core upgrade should keep the prior admitted release unless platform compatibility genuinely requires the new one.

Impact: bootstrap jobs/images/chart ordering may change, and candidate effects need new isolation/persistence. Mitigate with blocking fresh-install tests, owned upgrade tests and expiry/cleanup assertions. Candidate code's external non-Attune side effects are not transactionally rolled back; document and sandbox test dependencies where possible.

## Work package 10: API, CLI, web and SDK contract completion

**Deliverable:** tooling that exposes and uses admitted authority rather than inferring it from human IAM assignments.

Affected code:

- API permission/action/execution/pack/config DTOs and `crates/api/src/openapi.rs`.
- `crates/cli/src/commands/permission.rs`, pack/action/execution/config commands, MCP adapters where exposed.
- `web/src/components/common/ExecuteActionModal.tsx`, action editors, workflow task inspector, rule/queue forms, permission-set detail, pack/sensor configuration views and permission hooks.
- Generated web client, repository Python integration client, sibling Python/JS/Java SDK generated clients and generation scripts.

Tasks:

1. Add narrow action execution-options/preview data for approved runtime selections, not a requirement that ordinary invokers browse global IAM policies.
2. Remove `core.admin` and assigned-set-list heuristics from default selection. The web execution modal must not quietly choose `permission_set_refs: []` because a user lacks personal membership in the action's local set.
3. Show authored scope, normalized effective scope, sponsor, admitted revision, external approvals and revocation separately from direct human assignments. Respect existing read/redaction permissions on sensitive inspection data.
4. Round-trip the canonical scope/admission metadata through permission export/import. The current CLI definition parser rejects unknown fields, so adding authoring fields without porting it breaks exports and updates.
5. Represent managed edits, configuration reset, incompatible rollout acknowledgement, approval issue/revoke and dry-run diagnostics consistently. Distinguish deployment authorization from runtime invocation/secret disclosure failures.
6. Regenerate clients from one OpenAPI contract and add SDK contract tests. Runtime HTTP payloads need no invented special event transport; SDKs continue using supplied credentials and report authorization failures clearly.
7. Add the agreed aggregate/separate-requesters batching control to the canonical queue definition and DTOs, queue editor, CLI/API configuration and generated clients. Default to aggregation, label both choices plainly, show the effective setting, and document attribution and result/log visibility independently of batching. Do not require editing raw JSON to find the control.

Impact: API/CLI/generated type changes and some frontend eligibility behavior change. Accept these pre-production contract changes; update first-party clients together rather than keeping conflicting old permission heuristics. Avoid hand-editing generated code.

Evidence: `ExecuteActionModal.tsx:87-107` selects default/none using assigned refs and a `core.admin` exception. `PermissionSetDefinition` in CLI is strict and currently only accepts ref/label/description/grants. API permission summaries expose raw grants/ownership but no admitted scope/binding contract.

## Work package 11: lifecycle control, retention and observability

**Deliverable:** inspectable revocation and useful denial/audit evidence without secret leakage.

Tasks and locations:

- Add pack/release/approval revoke operations through protected API/CLI interfaces, repository state and live validation. Distinguish disable, retire, revoke and cancel.
- Coordinate cancellation, explicit system-principal/pack suspension, approval revoke and target retirement with the effect-consumption lock order. Human account freeze only governs interactive access and new human requests; it must not mutate runtime capability state. Preserve existing cache/schema/workload transaction protection.
- Update authorization-change notifications, but make repository/fence state authoritative for already-issued tokens and established streams.
- Extend audit types/builders with actor, sponsor, admission, authority origin, target and decision/reason. Never log tokens, plaintext configuration, secret values or unredacted policy content unnecessarily.
- Extend release/admission/configuration retention pins and supervisor cleanup. Keep immutable event attribution useful after execution deletion and fail closed for unavailable secret origins.
- Add bounded metrics for admission failure stage, runtime deny reason, stale/missing binding, revocation latency, candidate cleanup and query/lock costs.

Impact: revocation becomes operationally explicit rather than an accidental side effect of editing a permission set or freezing a human. Accept this. Provide first-class revoke/suspend commands and inspectable approval state. Ownership transfer and human deletion must preserve service operation and historical attribution without requiring a live human row or active human account.

## Work package 12: existing-install conversion and coordinated rollout

**Deliverable:** an operator-controlled transition, not automatic trust of old metadata.

1. Inventory existing packs, custom unscoped sets, IAM assignments, managed API edits, config values, active executions, runtime sponsors and bootstrap paths. Produce an authorization/configuration change report before mutation.
2. Use a fresh, owned database for development acceptance if appropriate. For existing environments, require deliberate maintenance/drain, redeployment/readmission and explicit external approval; do not authorize old executions solely from names, `installed_by`, or old token refs.
3. Convert configuration values conservatively to overrides, preserving ciphertext/origins. Do not guess which fields were operator-edited when history cannot prove it.
4. Add migrations through the approved pre-production database policy. Do not reset shared databases, rewrite applied checksums, or create compatibility formats without explicit authorization.
5. Coordinate API, executor, worker/agent, sensor/notifier, bootstrap and client versions. Block incompatible service combinations through readiness/catalog/schema capability checks. A mixed version that bypasses new consumption checks is not acceptable.
6. Preserve canonical v1 authoring/runtime contracts. No legacy broad-token fallback. Old unbound tokens are denied or drained under an explicit operator transition.
7. Define recovery after failed activation and schema rollback. Use compatible binaries and a tested backup/restore or explicit forward repair. Do not promise safe binary downgrade after irreversible database or policy conversion.

Impact: this is a breaking pre-production model change with a maintenance/readmission requirement for existing installations. Accept it rather than adding long-lived dual authorization behavior. Document the scope and operator steps clearly.

## Impact and disposition register

| Impact or risk | Disposition | Required mitigation or accepted behavior |
| --- | --- | --- |
| Deployers can install effects they cannot perform through their user token | Accept | This is the objective. Retain Actions/Execute, invocation schemas and local ownership bounds. |
| Deployers can influence code consuming pack secrets | Accept and disclose | Treat deployment as control over pack capabilities. Separate protected consumers into other packs/approved external capabilities when this trust is inappropriate. |
| Unscoped custom policies change meaning | Mitigate, then accept narrowing | Scope inventory/diff, normalized inspection, explicit external approval. Platform policies remain platform-owned. |
| Runtime rights no longer depend on human activity or personal IAM edits | Accept | Human suspension controls login/new human requests only. Provide separate operational revoke/suspend controls and retain requester/issuer audit. |
| Local mutable set edits no longer rewrite in-flight rights | Accept | Readmit future bindings and expose pinned revision. Emergency revocation remains immediate under transactional ordering. |
| Some actions previously emitted with `standard` | Accept tightening | Add explicit admitted event policies to affected packs; no compatibility bypass. |
| Completion/cancellation invalidates effects before JWT expiry | Accept | Narrow cleanup mechanism if needed; no generic grace that permits events/signing/children. |
| Mixed-requester queue execution/result confidentiality | Mitigate without changing the agreed default | Aggregate eligible items by default; preserve sponsor/per-item attribution, input authorization and origin-aware disclosure. Provide explicit requester separation. Do not broaden whole-batch visibility to one contributor or silently partition solely because requester identities differ. |
| Web currently suppresses local credentials based on IAM membership | Must fix | Consume server execution options and preserve default selection rather than selecting empty refs. |
| Mutable policy and IAM expansion disagree on local scope | Must prevent | One typed normalization contract applied before every grant consumer, including direct/role assignments. |
| More live queries/locks and notifier revalidation | Mitigate | Share immutable policy, bounded repository reads, execution-specific cache keys, measured lock order and concurrency tests. Never use stale cached permits as sole revocation proof. |
| Trigger schema changes break old code | Mitigate and explicitly accept on override | Report live dependent admissions; drain or acknowledge incompatible rollout. Full generation-specific routing is excluded initially. |
| Existing config loses default/override provenance | Mitigate | Preserve effective values as overrides where origin cannot be reconstructed; explicit reset recovers new defaults. |
| Secret origins disappear under retention | Accept fail-closed disclosure | Keep non-secret attribution independently; optionally design retained provenance records, not blanket execution pinning. |
| Candidate tests affect active automation | Must prevent | Isolated ingress context/targets/events, expiry, cleanup, and active-consumer exclusion. |
| A worker with signing material can forge claims | Existing trust limitation | Verify recorded admission on the server. This is not a worker sandbox or per-worker signing isolation project. |
| A signer-issued assertion is already used externally | Accept limitation | Short TTLs and recipient revocation where supported; Attune revocation stops future issuance, not past external acceptance. |
| Account freeze accidentally changes deployed behavior | Must prevent | Maintainer/uploader/requester/issuer freeze must not revoke approvals, reject admitted jobs, interrupt secret delivery, or close runtime service connections. Human-session revocation remains separate. |
| Direct managed edits drift from archive metadata | Mitigate | Record immutable metadata revisions and expose effective versus authored state; do not silently overwrite a release snapshot. |
| Service/client version skew bypasses validation | Must prevent | Coordinated release/readiness checks, regenerated clients and owned upgrade tests. |

## Delivery sequence and completion gates

| Phase | Work packages | Completion gate |
| --- | --- | --- |
| A. Contract and data foundation | 1, 3, baseline decisions | Local/external scope and revision/fence types accepted; normalized grants round-trip; repository invariants and conversion report tested. |
| B. Deterministic deployment | 2, deployment parts of 8 | Fresh scoped installation works; nested forward refs independent of order; unauthorized external references rejected; failed plan leaves active state unchanged. |
| C. Complete runtime authority | 4, 5, 6, lifecycle parts of 11 | Every admission and consumer agrees; explicit Events/Create and revocation work through real API and already-connected notifier tests. |
| D. User-facing consistency | 7, remaining 8, 9, 10 | Sensor/pack overrides agree; managed edits readmit; web/CLI/SDK do not require personal local set assignment; bootstrap/candidates have parity and isolation. |
| E. Conversion and operational acceptance | 11, 12, full regression/performance | Controlled existing-install conversion, retention/concurrency/upgrade tests, diagnostics and documentation complete. |

Phase A/B scaffolding can be merged inactive, but do not expose the new deployment exemption in a released environment before Phase C's runtime checks are complete. This is implementation sequencing, not permission to maintain two live semantic formats.

The work spans several services and persistent contracts. Treat it as a multi-phase feature, not a small permission-check patch. Calendar estimates should follow the contract/data design and a complete test inventory; this source review does not establish measured implementation durations.

## Acceptance matrix

1. A non-IAM-admin identity with only scoped management authority deploys a fresh pack containing a trigger, emitter action, local permission set, rule, workflow and queue in one operation. No preexisting resource IDs or manual permission assignment are needed.
2. Same-pack ordinary/nested workflow refs work in arbitrary filename order. Invalid references/cycles get located errors; valid event feedback topology is not rejected solely for containing a cycle.
3. Local policy definition/attachment succeeds while unapproved external/global scope, ownership collisions and private foreign references fail for the correct distinct reasons.
4. Manual, rule-to-enforcement, enforcement-to-execution, queue, ordinary/dynamic workflow, item/batch/cache fanout, retries and execution-created-child paths use the same target admission. Empty refs remain credential-free.
5. Approved emitter actions work without caller Events/Create. User direct ingress remains forbidden; standard-only/unrelated runtime policy cannot emit; undeclared overrides and copied foreign parent refs fail.
6. Wrong/missing execution, action, subject, admission or attempt fails on every consumer, including protected routes without resource grant loading and notifier delivery. Test two executions belonging to the same identity with different revocation state.
7. Barrier-based cancellation, sponsor freeze, approval revoke and trigger disable/retirement races produce the documented transaction-ordered result, without unchecked events/signing or deadlocks.
8. Ordinary redeployment and mutable policy edits cannot broaden old admissions. Existing retries preserve admitted code/policy; explicit revocation overrides them. Incompatible live target-contract changes require the selected rollout procedure.
9. A batching-enabled queue aggregates otherwise eligible items from different requesters by default and executes using admitted pack authority. Explicit requester-separation mode partitions as configured. Both retain per-item attribution, deny genuinely unauthorized inputs and prevent one contributor from automatically reading the whole batch's private results/logs. Human inactivity does not alter either mode or invalidate accepted work. Pack YAML, UI and CLI/API round-trip the setting with documented defaults.
10. Events retain emitter attribution after execution purge, preserve sensor source semantics, propagate bounded secret origins, and enforce historical disclosure after source retention.
11. Signing retains profiles, Key Use/Read compatibility as chosen, bounded assertions and no private-key return. Revocation denies future issuance with still-unexpired JWTs.
12. Freeze or remove human maintainer/uploader/requester/issuer access after admission: local/external automation, queued jobs, retries, runtime service connections and recorded input delivery continue. Transfer ownership without changing these behaviors. Their human sessions and new requests are denied; explicit operational revoke/suspend still stops the affected runtime authority.
13. Pack and sensor overrides survive redeployment; reset, null, default changes and schema migration behave identically through API/CLI/deployment. No secret plaintext leaks through previews or incompatibility errors.
14. Changing authority-bearing config fields or managed executable metadata requires readmission. Harmless label-only edits do not need unrelated Key grant coverage.
15. Candidate emission reaches only isolated test targets/events. Active rules/notifications do not see it; finished candidate contexts, wrong archive credentials and failed installs leave no reusable authority.
16. API and volume/object bootstrap converge on the same admitted authority; fresh install, failed core upgrade and controlled version-skew cases preserve readiness and safety.
17. Local scope normalizes identically for runtime, human direct/role assignments, exported YAML, and regenerated API clients. Platform catalog metadata remains unchanged in ownership/scope.
18. Configuration/admission/release pins survive while needed and clean up afterward. Event/execution independent retention and create-if-absent identity semantics remain intact.
19. The web execute form uses approved defaults for a non-admin invoker without assigning them the local policy. CLI/SDK round-trips and denial messages expose the correct actor, target, scope and revision without secrets.
20. Full workspace type/check/lint gates and targeted repository/API/executor/worker/sensor/notifier/CLI/UI tests pass; measure equivalent pre/post coverage and runtime query/lock behavior rather than claiming improvements from fewer tests.

Existing test homes include permission/pack registry/configuration binding tests, execution-token permissions and key-usage integration tests, rule authority and async release-pin tests, scheduler permission/fanout tests, worker runtime-access/candidate tests, notifier visibility tests, CLI permission tests and web execute/configuration tests. Update synthetic fixtures to create real admitted executions rather than signing arbitrary refs for unrelated rows. Keep mismatched claims as negative cases.

Use the owned PostgreSQL/service runners described in `docs/testing/running-tests.md`; preserve minimum database concurrency, deterministic barriers, explicit external prerequisites and cleanup evidence. No tests were executed for this SOW.

## Scope exclusions and engineering design items

Excluded from the initial project:

- A new pack format version or stable backward-compatibility promise.
- Per-worker signing-key isolation, sandboxing arbitrary action code or generic secret-flow inference from unmarked bytes.
- Release-generation-specific event/rule routing or universal schema-compatibility analysis.
- Removing existing secret-selector/provenance protections or opening direct event ingress to human access tokens.
- Automatic database reset or silent approval of historical broad grants.

Behavioral choices above are agreed. Resolve these engineering items before producing firm estimates or beginning implementation:

1. Choose canonical local/external envelope fields and normalized policy representation, preserving identical effective scope in runtime, IAM and export/import.
2. Define approval issue/inspect/revoke interfaces and resource-owner/administrator authorization checks. Automatic approval when the installer already has authority and issuer-inactivity independence are settled behaviors.
3. Design aggregate-batch attribution, origin-aware result/log visibility and the explicit requester-separation field/UI. Aggregation is the default; inactivity or differing personal runtime grants cannot be hidden partitioning rules.
4. Define retry admission/fence representation that preserves the agreed disable-versus-revoke behavior.
5. Specify required worker-owned cleanup operations and bounds. Completed execution tokens do not regain general access.
6. Implement isolated candidate API targets/events and exclusion from active consumers. Simulation alone does not fulfill the agreed real-API test option; external non-Attune side effects remain separately documented.
7. Specify default/override persistence, null/reset payloads and metadata-revision/export UX under the agreed preservation rules.
8. Inventory and implement precise ownership mappings for every currently supported local resource/action. Keep unsupported named-schema consumer gates explicit. Add no global ownership exemption.

Do not reopen agreed product behavior merely for implementation convenience. If source constraints make a guarantee infeasible, document the conflict and seek a targeted decision.

## Source anchors

The inventory follows the inspected working tree; symbols are more durable than line numbers while parallel work changes files.

- `pack_registry/permissions.rs::PackPermissionPlan::{read,validate}` and `pack_registry/loader.rs::{load_all,load_actions,load_workflow_for_action}` own current registrar checks and staged component binding.
- `routes/packs.rs::{authorize_pack_registry_action,authorize_pack_delegation,register_pack_internal,update_pack}` own entry scope, repeated authority checks, activation and configuration edits.
- `repositories/identity.rs::authorization_state`, `PermissionSetRepository` readers and `rbac.rs::Grant` own raw grant expansion and matching.
- `repositories/pack_release.rs::activate_projected`, `repositories/executable_snapshot.rs::capture_present`, and migrations `20261013000012_component_configuration_bindings.sql`, `20261013000014_pack_rule_input_authority.sql` provide current immutable/revision precedents.
- `event_processor.rs:395-423`, `enforcement_processor.rs:325-438`, `queue_dispatcher.rs:665-855`, and scheduler permission/cache/fanout/retry helpers provide the production admission inventory.
- `auth/jwt.rs:576-618`, worker token generation, API `RequireAuth`/`load_grants_for_token`, and notifier `WebSocketAuthContext`/delivery helpers provide credential and consumer inventory.
- `routes/events.rs:129-474`, `repositories/event_schema.rs:73-95`, and `routes/keys.rs::sign_key_jwt` provide effect authorization/locking/provenance context.
- `ComponentConfigurationRepository::bind`, pack/sensor update routes and the sensor loader provide current default/value/semantic-migration behavior.
- `worker/src/service.rs` candidate test handling and `common/src/test_executor.rs` show tests are not normal executions.
- `ExecuteActionModal.tsx:87-107`, API permission DTOs, and CLI `PermissionSetDefinition` show tooling's current identity-based/default/export assumptions.
- Compose/bootstrap scripts and sibling chart `templates/jobs.yaml`/packaging Compose show both API and direct-loader installation paths must be covered.

This inventory identifies implementation responsibility and acceptance evidence; it is not authorization to begin implementation or a claim that all existing tests already establish the proposed semantics.
