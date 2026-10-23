# Deployment binding idiosyncrasies

Source inspection of the working tree on 2026-10-13. No tests, infrastructure, or deployments were run. This supplements [pack-owned runtime authority](../plans/pack-owned-runtime-authority.md). Findings distinguish intentional contracts from source-level inconsistencies and untested failure implications.

## Summary

Deployment admission, reference visibility, immutable release bindings, captured secret-input authority, and live execution grants are different mechanisms. They sometimes validate different actors and take effect at different times. Successful deployment therefore does not mean every invocation is authorized, and changing current metadata does not necessarily change already captured authority.

Highest-priority follow-ups are scoped install authorization, workflow runtime grant binding, consistency of configuration migration checks, and complete candidate-graph resolution. Queue requester attribution, secret provenance, and private reference visibility need explicit policy rather than a blanket deploy-permission bypass.

## Preserving values does not mean retaining the new schema unchecked

Pack redeployment deliberately sets `config: None`, preserving user values while replacing `conf_schema`. After release activation, the same transaction captures rule authority and binds configuration. An incompatible preserved value can therefore reject the deployment rather than silently adopt its new schema. Sources: `crates/api/src/routes/packs.rs:2415-2424`, `crates/api/src/routes/packs.rs:2587-2593`.

The binder compares semantic digests. An equivalent contract reuses the previous revision and ciphertext without restoring plaintext, even without an encryption key. A changed contract restores preserved values, applies defaults, validates, and creates a new revision. Semantic equality includes named dependency revisions, not only the authored schema. Sources: `crates/common/src/repositories/component_configuration.rs:403-455`, `crates/common/src/value_schema/mod.rs:140-152`.

For preserved encrypted material, deployment requires Pack read/decrypt only when the semantic contract changes. The sensor equivalent checks Trigger **update**/decrypt, despite the error text saying read/decrypt. This is an authorization naming asymmetry, not evidence that Trigger read suffices. Sources: `crates/common/src/repositories/component_configuration.rs:220-280`.

There is a narrower API discrepancy. Ordinary pack PUT requires read/decrypt whenever `conf_schema` is supplied without `config` and the retained revision contains redactions. It does not first compare semantic equality. Resubmitting an unchanged secret schema can fail there while equivalent redeployment succeeds. Explicit replacement values take a different branch. Source: `crates/api/src/routes/packs.rs:854-885`. No inspected test directly compares these two endpoints with an identical schema.

Tests inspected: `crates/common/tests/component_configuration_tests.rs:20-111` proves equivalent binding reuse and migration authorization; `:290-379` checks preserved values, defaults, sanitized incompatibility errors, and rollback. `crates/api/tests/configuration_binding_tests.rs:147-234` checks redaction, explicit decrypt, forbidden preserved-schema migration, and failed-validation rollback.

## Sensor YAML configuration is not preserved like pack configuration

The sensor loader treats absent YAML `config` as `{}` and writes `config: Some(config)` on update. Deployment then treats an unredacted sensor projection as supplied replacement values. Ordinary redeployment therefore replaces sensor-local configuration, whereas pack configuration survives. The binder's preservation branch exists, but this normal loader path does not choose it. Sources: `crates/common/src/pack_registry/loader.rs:3080-3083`, `:3124-3155`; `crates/common/src/repositories/component_configuration.rs:193-215`.

This appears deliberate in the loader's metadata-refresh policy, but should not be described as universal configuration preservation. No end-to-end redeployment test for operator-edited sensor values was inspected.

## Input authority already has a deployment-owned lifetime

Rules declare `input_permission_set_refs` separately from runtime API permission refs. Capture keeps only Key/Pack read/decrypt grants. Runtime uses those recorded grants and checks the active pack runtime identity plus its owning pack, not the installer's current grants or the current permission-set body. Existing runtime input authority cannot be rebound. Sources: `crates/common/src/repositories/rule_authority.rs:36-79`, `:201-245`, `:349-369`.

Consequently, revoking the installer's access or editing a permission set does not revoke previously captured input authority. Freezing the pack runtime identity does. This is intentional and already documented in `docs/permissions/delegation-and-secret-disclosure.md:67-74`; it is not a missing implementation of the proposed action capability model.

Rules/Update alone is also insufficient for deployed input-affecting edits. The caller must cover the entire captured input grant list, even for conditions or trace-tag changes, rather than only the Keys mentioned by the patch. Sources: `crates/api/src/routes/rules.rs:1001-1014`; `crates/common/src/repositories/rule_authority.rs:95-114`.

Test inspected: `crates/common/tests/rule_authority_tests.rs:22-161` checks filtered grants, denial for metadata-only authority, and continued input access after installer freeze and permission-set clearing. `:205-239` checks inherited authority without API permission assignments. It does not test runtime-identity freeze.

## One rule can feed two different input authority owners

Sensor releases capture their own `config.input_permission_set_refs`, filtering to Key/Pack read/decrypt. Signing grants capture Key use separately and cannot supply plaintext input authority. Deployment requires registrar coverage of the filtered input grants. Sources: `crates/common/src/repositories/executable_snapshot.rs:74-91`; `crates/common/src/pack_registry/permissions.rs:182-201`, `:354-379`.

The sensor renders a rule's `trigger_params` with the **sensor generation's** captured grants, not the rule's captured input grants. The latter govern the rule execution-input path. Granting a rule access to a Key does not by itself allow its managed sensor to receive that Key in trigger configuration. Sources: `crates/common/src/repositories/sensor_schema.rs:104-135`, `:138-173`.

Sensor input rendering checks a sensor identity's frozen state, with no live installer lookup. Release authority and current values have different lifetimes: generation admission takes the released sensor schema but restores current sensor configuration; pack selectors read current pack configuration and record its revision. It then renders and validates under the generation contract. Sources: `crates/common/src/repositories/sensor_schema.rs:49-75`, `:113-135`, `:454-464`, `:587-629`.

Tests inspected: `crates/common/tests/sensor_workload_repository_tests.rs:553-641` and `:644-729` clear mutable permission-set grants before workload acquisition, then verify plaintext delivery through captured Key/Pack authority and recorded origins. The Key is created after executable setup in the first test. Deployment approval is not proof that every future selector can resolve.

## Secret rule inputs are selectors, not arbitrary encrypted literals

Secret-marked rule action and trigger inputs require whole Key templates, including nested values and effective defaults. Literal secrets, mixed text, and direct event templates are rejected even if the caller could decrypt the source. API writes, pack loading, and runtime admission use this policy. Sources: `crates/api/src/validation/params.rs:64`, `:139`; `crates/common/src/pack_registry/loader.rs:2643`; `crates/executor/src/event_processor.rs:401`; `crates/common/src/repositories/sensor_schema.rs:155-167`.

This is intentional provenance policy, not a missing permission. Pack configuration can store supplied secrets through its encrypted revision mechanism; rule authoring instead stores Key selectors. Tests inspected: `crates/api/tests/rule_update_authz_tests.rs:76-184`; `crates/common/src/value_schema/runtime/authoring/tests.rs:15-81`.

## Named-schema publication, assignment, and deployment are distinct

The current rollout gate still rejects named schemas in pack consumers, even though type publication, discovery, generic assignment, and runtime materialization machinery exist. Both pack preflight and direct repository guards enforce this. Sources: `crates/common/src/pack_registry/schema_types.rs:345-347`, `:488-502`; `crates/common/src/schema_type.rs:34-48`; `crates/common/src/repositories/pack.rs:235`, `:286`. This is an explicit temporary gate, not an IAM failure or proof that the schema cannot compile.

Where materialization is enabled, same-pack named bodies come from the declaring release. External bodies select the active release and must satisfy the declaring release's dependency constraint; the resolver does not search older compatible releases. Conflicting release selections fail closed. Current retirement/ownership/reference policy still governs new references. Once attached, a runtime contract is immutable and matches its executable snapshot. Sources: `crates/common/src/repositories/schema_contract.rs:599-658`, `:684-704`, `:277-322`.

Generic assignments and previews have no declaring pack. They can select installed private types without pack dependencies; pack-owned configuration assignments enforce pack reference policy. "Private" therefore controls pack references, not schema secrecy or generic Key assignment. This distinction is intentional. Sources: `crates/common/src/repositories/schema_contract.rs:409-428`, `:431-494`.

Tests inspected: `crates/common/tests/schema_type_repository_tests.rs:110-218` accepts a generic private-type assignment and rejects an unrelated pack-owned assignment. `crates/common/tests/runtime_schema_binding_tests.rs:217-337` checks inheritance and rejection of rebinding. The broader dependency resolver behavior above is source evidence, not a claim that these inspected tests cover every release-selection case.

## Scoped install permission is checked without the candidate pack scope

Upload checks Packs/Install at entry. The shared check constructs only `AuthorizationContext::new(identity_id)`, without a target ref or pack ref. RBAC denies a `pack_refs` constraint when the context has no pack ref. Scoped Install authority alone therefore cannot satisfy this entry check, even if the archive names an allowed pack. This is distinct from lacking runtime grants and also affects fresh-pack deployments.

Classification: authorization limitation. Authenticate first, resolve and validate candidate identity, then check installation scope before activation or privileged work. Retain early size/resource protections.

Sources: `crates/api/src/routes/packs.rs:1648-1653,2777-2797`; `crates/common/src/rbac.rs:217-224`.

## Workflow task permission approval is not runtime-owner assignment

The pack permission plan checks workflow task refs against installer authority. Deployed rule authority capture assigns its pack runtime identity only the rule's selected refs or the workflow action's defaults, not every task override/default. Child scheduling then checks child refs against `parent_execution.executor`.

Example: a rule starts a workflow with empty or `standard` defaults, but a task requests named set S. An installer covering S can pass deployment. If the shared pack runtime identity has not acquired S through another rule or assignment, that task fails runtime delegation. An unrelated rule in the same pack can change whether it succeeds by adding S to that shared identity.

Classification: deployment/runtime authority mismatch with a hidden shared-identity dependency. Bind approved authority to the actual workflow/action invocation rather than an accidental union of pack identity assignments.

Sources: `crates/common/src/pack_registry/permissions.rs:207-255,293-315`; `crates/common/src/repositories/rule_authority.rs:378-400`; `crates/executor/src/scheduler.rs:2321-2338,2928-2935`. This combined scenario was not reproduced.

## Queue authority follows requesters rather than the installer

Queue refs can pass registrar checks at deployment, but dispatch uses item requester identities. One executor exists only if every batched item has the same non-null requester. Runtime API refs and explicit Key access require that attribution; named API refs must be covered by that requester's current grants.

The same queue can dispatch for one user and fail for another. A mixed-requester batch can fail where individual batches would succeed when API access is requested. This is a runtime-principal constraint, not a guarantee established by deploy-time admission.

Classification: intentional accountable delegation with a composition constraint. The proposed local capability sponsor should remain separate from requester attribution, external grants, and secret provenance.

Sources: `crates/executor/src/queue_dispatcher.rs:681-685,717-722,765-779`.

## Workflow permission inheritance and deferred validation differ

Absent a task override, a child uses its action's default refs, not the parent's selected refs. An empty override suppresses child API credentials. Permission expressions are stripped from deployment's literal-ref plan and checked after rendering at child creation.

Classification: intentional per-action selection and deferred validation. Parent token opt-in does not enable every child, and deployment success cannot certify every possible rendered ref.

Sources: `crates/executor/src/scheduler.rs:2321-2380`; `crates/common/src/pack_registry/permissions.rs:409-440`.

## Private reference visibility is not overridden by installer administration

Private actions/triggers are referenceable only from their owner pack. Restricted refs additionally permit named referencing packs. The reference helper does not consult installer IAM authority. A human administrator able to read or execute a private foreign action can still be unable to deploy a workflow referencing it from another pack.

Classification: intentional pack encapsulation, separate from human IAM. Keep this visible in diagnostics and do not equate runtime grant coverage with referenceability.

Sources: `crates/common/src/action_visibility.rs:10-44,55-98`; `crates/common/src/pack_registry/loader.rs:2850-2863,3734-3748`.

## Nested workflow forward references can still depend on load order

The current loader loads ordinary actions before workflow actions, explicitly fixing ordinary action filename ordering. Workflow actions remain sequential. Loading each workflow immediately looks up its targets from persisted projections before the invoking workflow action is upserted.

A fresh workflow A referencing a not-yet-loaded workflow B can therefore fail depending on workflow YAML ordering, although both appear in the candidate pack. Reinstallation with B already present can behave differently. This is a source-level implication, not an executed reproduction; ordinary action forward refs are already handled by current code.

Classification: candidate-graph/load-order limitation. Resolve edges against the complete candidate graph, reserving identities before binding, and explicitly handle cycles.

Sources: `crates/common/src/pack_registry/loader.rs:1421-1444,1473-1500,2850-2857,3816-3838`.

## Removing authored metadata is not necessarily revocation

Permission sets are non-toggleable. Omission policy `disable` leaves omitted sets active, as does `retain`. Deployment capture and rule-ref refresh add missing pack identity assignments without pruning older assignments. Removing a requested ref can therefore leave that set assigned to the shared runtime identity.

Selected refs still constrain ordinary grant-checked runtime calls, so this does not automatically grant every token the old authority. It does leave authority available for later delegation and makes YAML omission an unreliable revocation mechanism.

Classification: explicit omission semantics plus additive-assignment debt. Release-owned capabilities should not depend on accumulated identity assignments. Define explicit revocation separately from omission and routine redeployment.

Sources: `crates/common/src/repositories/component_lifecycle.rs:253-260,282-293`; `crates/common/src/repositories/rule_authority.rs:117-169,378-400`; `crates/api/src/authz.rs:580-604`.

## Unused permission definitions still require publication authority

Admission validates every set definition, not only definitions referenced by executable metadata. Without unrestricted Permissions/Manage, an unused definition with undelegable grants rejects the whole pack.

Classification: intentional policy-publication control. The proposed local-authority exception must apply to definition admission as well as attachment admission, or packs still fail before component binding.

Source: `crates/common/src/pack_registry/permissions.rs:257-274`.

## Design implications

The runtime-authority proposal should distinguish four proofs instead of treating deployment as one universal authorization check:

1. Authority to manage the candidate pack and its component graph.
2. Permission for one pack to reference another pack's components.
3. Authority approved for a particular executable or invocation binding.
4. Authority to deliver or disclose a particular secret value, with retained origins.

Use complete candidate ownership for the first, reference-visibility policy for the second, immutable local capability plus explicit external delegation for the third, and captured input/provenance contracts for the fourth. Unifying their plumbing should not erase their different security meanings.
