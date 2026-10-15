# Pack-owned runtime authority

Date: 2026-10-13

Status: proposed changes only. No implementation, migrations, permission assignments, or deployments performed.

This earlier inventory is superseded where it requires a live human executor/delegator during runtime consumption. The agreed continuity model in [the statement of work](self-contained-pack-statement-of-work.md) separates human maintainer/requester/issuer status from system runtime authority. Human inactivity does not revoke admitted jobs or recorded external approvals. The file below retains the original proposal for context; use the SOW's agreement log as the current decision record.

## Goal

Allow an identity authorized to deploy a pack to define and connect that pack's components, including permission sets that let its actions create events for its triggers. The installer must not need preexisting event-create authority over a pack that does not exist yet.

Keep human authority separate from executable authority. Deploying an event-emitting action does not give the installer a credential that can directly submit arbitrary events. An authorized invocation of that action can use its deployment-approved runtime authority.

This is a change to the authorization model, not just a relaxation of YAML validation. Deployment admission, execution creation, JWT consumers, and revocation must agree on the new model.

## Example that must work in one deployment

An active human has permission to deploy `example`, but no Events/Create grant and no permission-management grant. The pack defines `example.completed`, `example.emit_completed`, and the following set:

```yaml
# permission_sets/emit_completed.yaml
ref: example.emit_completed_events
grants:
  - resource: events
    actions: [create]
    constraints:
      refs: [example.completed]
```

```yaml
# relevant fields in actions/emit_completed.yaml
ref: example.emit_completed
default_execution_permission_set_refs:
  - example.emit_completed_events
```

Deployment admits the set and its attachment because the trigger, set, and action belong to the authorized candidate pack. No separate IAM assignment or second deployment is required. A caller authorized to execute the action receives only its selected runtime authority. It may submit an event for `example.completed`, but not for another trigger unless separately granted.

For Events/Create, this proposal defines `constraints.refs` as target trigger refs and `constraints.pack_refs` as the target trigger's owning pack. That meaning must be implemented explicitly; the current event handler does not evaluate these grants.

## Current constraints that block or weaken this operation

| Current behavior | Required change |
| --- | --- |
| Registration requires the installer to cover definitions unless it has unrestricted Permissions/Manage. Attachments always require grant coverage. | Recognize deployment-approved authority over the pack's own resources without requiring matching human grants. |
| Manual execution requires the caller to cover requested named sets. | Permit action-bound local runtime grants without granting them to the caller identity. |
| Rule, queue, and workflow admission check named sets against their executor/requester. | Use the same action-bound authority resolver in every execution path. |
| API runtime authorization resolves current named grants and requires current subject coverage. | Resolve recorded pack-owned capabilities separately from identity-delegated external grants. |
| Direct event creation accepts any execution JWT without checking grants or execution liveness. | Enforce selected Events/Create authority against the actual trigger and current execution authorization state. |
| Compose's direct loader differs from API registration and authority capture. | Make bootstrap establish the same admitted runtime authority as ordinary deployment. |

Sources: `crates/common/src/pack_registry/permissions.rs:211-318`; `crates/api/src/routes/executions.rs:194-223`; `crates/common/src/delegation.rs:167-203`; `crates/executor/src/enforcement_processor.rs:379-392`; `crates/executor/src/queue_dispatcher.rs:765-778`; `crates/api/src/authz.rs:573-605`; `crates/api/src/routes/events.rs:129-205`; [existing lifecycle investigation](../permissions/event-submission-lifecycle.md).

## Authorization contract

### Deployment authority is not global runtime authority

Interpret unrestricted local composition as freedom to define and connect resources owned by the authorized pack. Do not interpret it as permission to publish global grants or obtain authority over unrelated packs.

For each requested grant, prove one of the following:

1. Its entire effective scope is contained in resources the pack is authorized to manage. Admit it as pack-owned runtime authority.
2. Its scope extends outside that ownership. Require existing delegation authority for that external scope, or reject deployment.

Both permission definitions and their uses receive this treatment. A set containing local and external grants must pass both checks. A local set name does not make its grant targets local.

An unconstrained Events/Create grant is global, not local. The example uses explicit trigger refs. Do not silently reinterpret unscoped existing grants or bypass checks merely because YAML files live inside the pack. Authors can use `pack_refs: [example]` to grant access to all applicable resources owned by that pack, subject to the declared semantics.

### Resolve ownership, not name prefixes

Build a server-owned candidate ownership graph before mutating database projections. Resolve refs against that graph and existing managed ownership records. This permits references to not-yet-created triggers while refusing a ref that names or collides with another owner's resource.

Support same-pack existing resources when the caller is authorized to update that pack. Declaring a dependency or referencing another pack is not ownership. Platform and ad-hoc resources require their existing authorization rules. Decide explicitly how retained or omitted components participate in candidate ownership under each deployment policy.

Resource scope must have resource-specific meaning. For example, Events/Create is scoped by the target trigger, not by a nonexistent event ref. Keys and artifacts use actual owner-qualified scope. Identities, IAM assignments, workers, and global administration do not become pack-owned resources through authored refs.

Scoped writes must also validate relationships they introduce. Permission to update a local rule must not allow it to invoke an unrelated privileged action without the required action/reference authority. Scope on the row being written is not enough to authorize every referenced resource.

### Keep invocation and execution authority distinct

- The human installer retains only their existing IAM grants.
- Invokers need action-execution authority under the normal invocation rules, not the action's pack-local Events/Create authority.
- A runtime token receives only grants admitted for that action and invocation binding, not all grants available to the pack.
- `permission_set_refs: []` continues to suppress runtime API credentials.
- A caller cannot attach arbitrary pack-local sets to an unrelated action by naming them in an execution request.
- External delegation remains identity-bound and cannot gain a local-ownership exemption through a pack ref.

An invoker can intentionally cause events by running an authorized event-emitting action. That is the requested capability. Preventing all indirect event creation would contradict the goal. Action input schemas and implementation control which events and payloads the invoker can cause.

## Required implementation changes

### 1. Establish the deployment permission contract

`packs:deploy` is a logical permission in this proposal, not an existing action enum value. Current source uses Packs/Install for registration-related operations and Packs/Configure for replacement; `Action` has no Deploy member.

Recommended first implementation: retain those canonical names and document that authorized deployment includes admission of pack-owned runtime grants. Resolve the candidate pack ref before checking scope so a scoped Install grant can authorize a fresh pack without a database ID. Preserve explicit replacement/ownership checks.

If a distinct `deploy` permission is desired, add it deliberately to the enum, platform policies, OpenAPI/clients, CLI/docs, and deployment routes. Do not quietly treat all pack creation, installation, configuration, index administration, and replacement operations as interchangeable.

Sources: `crates/common/src/rbac.rs:39-52`; `crates/api/src/routes/packs.rs:2671-2765`.

### 2. Add shared candidate admission and grant classification

Extend the pack permission plan to resolve candidate component ownership and classify grants into pack-owned and externally delegated authority. Return an admitted plan with target bindings and per-action/per-task uses, rather than only success or failure.

Use it for preflight and activation. Preserve the existing requirement to capture external installer authority before mutations. Recheck existing resource ownership and external delegation inside activation to prevent replacement races and self-authorizing permission edits.

Reject unavailable targets, ambiguous scopes, unsupported ownership proofs, and cross-owner collisions. Emit diagnostics with the permission-set file, grant index, referencing component, rejected target, and required external authority.

Main files: `crates/common/src/pack_registry/permissions.rs`, `crates/common/src/pack_registry/loader.rs`, `crates/api/src/routes/packs.rs`, `crates/common/src/delegation.rs`, `crates/common/src/rbac.rs`.

### 3. Persist release-owned runtime capability bindings

Record the admitted local authority as server-owned release metadata. Include managing pack ID, release ID/digest, action or invocation binding, source permission-set refs, resolved grants/targets, and runtime authority owner. Use the existing system-owned pack runtime identity as the sponsor where suitable; do not assign its grants to the installer.

Action release snapshots currently include action/runtime/workflow metadata but not resolved action permission grants. Extend the canonical v1 contract or use a release-owned authority relation linked to it. Existing sensor snapshots already capture selected grants, so reuse their established release-pinning approach where it fits instead of inventing an unrelated policy format.

Keep local capabilities immutable for that release. Permission-set edits must not silently broaden previously admitted action capabilities. A release's executable metadata and admitted policy must commit together. If new persistence is required, add repository methods, migrations, snapshot serialization, history treatment where relevant, and SQLx metadata through the normal repository workflow. Do not rewrite migrations or reset data during this documentation task.

Main files: `crates/common/src/repositories/executable_snapshot.rs:18-103`, `crates/common/src/repositories/pack_release.rs`, `crates/common/src/models.rs`, `crates/common/src/repositories/rule_authority.rs`, `migrations/`.

### 4. Centralize runtime authority admission

Introduce one shared runtime authority module whose interface admits an execution using its pinned action, selected refs, invocation binding, and attributed caller/executor. Its output separates release-owned local grants from identity-delegated external grants.

Local grants require a matching deployment-approved binding, not human grant coverage. External grants retain current coverage checks. Preserve requester attribution and input-secret provenance; do not solve this by replacing every manual execution's `executor` with a privileged pack identity. Record the pack sponsor separately.

Use this module for manual API execution, rule enforcement, workflow child creation, ordinary and item/batch/cache fanout, queue dispatch, retries, and execution-token-created children. Selection remains opt-in. Overrides may reduce approved local authority; broader local overrides require an admitted binding, and external overrides require delegation proof.

Workflow children receive their own action's admitted authority. A foreign child does not inherit the parent's pack-owned rights just because it runs inside that workflow. Rendered task refs must be checked after rendering. Rules and queues need explicit admitted bindings to the actions they dispatch, including cases where actions and invocation metadata have different owners.

Retries preserve the originating admitted policy unless an explicit new admission occurs. Queue batches still need accountable requester/input provenance under their existing rules. Do not use the local capability exemption as an ownerless-execution or secret-delivery exemption.

Main files: `crates/api/src/routes/executions.rs`, `crates/api/src/authz.rs`, `crates/common/src/delegation.rs`, `crates/common/src/repositories/execution.rs`, `crates/executor/src/enforcement_processor.rs`, `crates/executor/src/scheduler.rs`, `crates/executor/src/queue_dispatcher.rs`, `crates/executor/src/retry_manager.rs`.

### 5. Resolve execution-token authority from recorded bindings

The JWT should identify the execution and its admitted authority binding; do not let signed permission-set refs alone select broader live pack-owned policy. Keep resolved policy server-owned. Verify token execution/action/authority metadata against stored admission and include an attempt or authorization generation if needed for revocation and reruns.

At token consumption, require an available attributed execution identity and an active runtime sponsor where the capability uses one. Resolve local grants from the recorded release binding without requiring the human to cover them. Continue checking external grants against the recorded delegating identity's current authority. Append `standard` only with its existing scoped contract.

Separate checks that apply to every execution token from route-specific grant checks. Keep this logic shared by HTTP and notifier consumers. Do not put the fix only in worker minting, since already-issued JWTs still reach API consumers.

Main files: `crates/common/src/auth/jwt.rs`, `crates/worker/src/executor.rs`, `crates/api/src/auth/middleware.rs`, `crates/api/src/authz.rs`, notifier execution-token authorization.

### 6. Enforce event creation permissions

Keep direct event ingress restricted to Sensor and Execution token types. A human Events/Create assignment must not bypass that ingress rule unless a separate policy change explicitly permits it.

For Execution tokens:

1. Validate recorded execution admission and liveness.
2. Resolve the actual trigger and its owning pack from repository records.
3. Construct an Events/Create authorization context with that trigger ref and pack ref.
4. Require a matching selected grant.
5. Validate schema and any rule target, then insert the event with provenance.

Perform authorization-sensitive ownership/liveness checks consistently with the insertion transaction so cancellation, trigger retirement, or authority revocation cannot race an unchecked write. Use repository methods and the established lock order.

`standard` must not implicitly authorize emission. A set containing only artifact or key grants must not authorize it either. Capture the emitting execution and admitted authority origin using a provenance representation that preserves existing sensor-source and retention contracts. A trace tag is correlation, not authority.

Preserve the sensor branch's exact trigger, ownership, workload, generation, and lease checks. Sensor emission is already a separate release-owned contract and must not accidentally become dependent on the human installer's current grants.

Main files: `crates/api/src/routes/events.rs`, `crates/common/src/repositories/event.rs`, event provenance models/persistence, shared execution-token authority validation.

### 7. Define update, revocation, and administrative edit behavior

Recommended contract:

| Change | Effect |
| --- | --- |
| Ordinary redeployment | New executions use the new admitted policy; already admitted runs retain pinned local policy unless explicitly revoked. |
| Edit mutable permission-set grants | Does not change admitted local release capabilities. External delegated policy retains its explicit live-grant contract. |
| Remove/disable runtime sponsor or revoke pack authority | Local capability authorization fails immediately through live checks. |
| Cancel or finish execution | Further event submission fails even while the JWT has not expired. |
| Freeze an installer after deployment | Does not automatically revoke pack-owned local authority for separately owned rule executions. |
| Freeze a manually attributed executor | Its runtime token fails active-executor checks; no switch to a system identity. |
| Delete/recreate a target or reuse its ref | Does not silently transfer the old admitted capability to a different owner/resource identity. |

Treat post-completion artifact cleanup separately if existing callbacks need it. It must not reopen event creation. Define explicit pack/release revocation support and distinguish it from routine redeployment rather than treating every release switch as revocation.

Ad-hoc action/rule/task edits must produce a newly admitted, immutable binding or fail. A direct metadata edit must not reuse a release's authority record for different refs or targets. Decide whether local authority edits require pack redeployment in the first implementation; requiring redeployment is the simpler rule.

Changing ordinary identity assignments must not change the stored local capability. Conversely, runtime admission does not authorize creating human IAM assignments. Review the existing additive pack-identity assignments so they cannot accumulate broader capability than the selected recorded binding.

### 8. Unify all deployment and testing paths

Apply the same ownership admission and capture through upload, server-side register, remote/index install, replacement, batch deployment, and bootstrap. Prefer ordinary API registration for bootstrap where feasible; otherwise the database initializer must call equivalent trusted admission/capture rather than maintaining a second partial implementation.

Candidate pack tests run before active installation. Admit candidate-owned grants into a candidate-specific execution authority, with target bindings isolated from active releases. Specify how event emission tests create or simulate candidate trigger targets; candidate permission must never authorize unrelated active triggers or leak into active runtime assignments. Do not skip the problem by granting test executions unrestricted Events/Create.

Update CLI/API diagnostics, permission inspection, audit records, and docs to distinguish local release capability from human assignments and external delegated grants. Document that deployers can install code able to affect their own pack resources, including intentionally emitting events.

## Acceptance tests

1. An installer with only authorized pack deployment rights deploys a fresh pack containing a local event-create set and action attachment in one request.
2. No permission-management grant, manual assignment, or preexisting pack row is required for that local case.
3. A scoped installation grant works with the candidate pack ref before any component IDs exist.
4. The installer's direct event POST remains forbidden, and deployment does not add event-create grants to their IAM identity.
5. An authorized invocation of the declared action emits its local trigger event without caller Events/Create authority.
6. Global or unrelated-pack grants fail without external delegation. Mixed local/external sets fail if any external part is uncovered.
7. Forged prefixes, ownership collisions, missing targets, and dependency references cannot establish local ownership.
8. Empty refs deliver no token; `standard` and unrelated named grants cannot emit events.
9. Request overrides cannot attach another action's broader local policy.
10. Manual, rule, queue, workflow, dynamic task, item/batch/cache fanout, retry, and token-created-child paths follow the same admission rules.
11. Cross-pack workflow invocation does not inherit the parent's local rights, and local rule mutation cannot install a privileged foreign action reference unchecked.
12. Permission-set edits and later redeployment do not broaden previously admitted local tokens. New runs use the appropriate new admitted binding.
13. Cancellation, completion, executor freeze, sponsor freeze, and explicit revocation deny event submission from an otherwise unexpired JWT.
14. Trigger retirement and ref reuse fail closed; target ownership/liveness races cannot produce unauthorized insertion.
15. Installer freeze does not revoke separately sponsored local rule capability, while external grants follow their documented identity-bound revocation contract.
16. Existing secret-origin checks still protect plaintext input delivery and historical disclosure.
17. Sensor workload fencing and admitted sensor authority retain their current behavior.
18. Bootstrap and candidate tests produce the same authority semantics without exposing unrelated active triggers.
19. Events retain attributable emitter/authority provenance without introducing unwanted retention foreign keys.
20. Failed admission rolls back metadata/authority activation together and reports the offending file, grant, and target.

Tests should cross the shared admission and token-consumer interfaces, with real repository/API cases for ownership, transaction races, and revocation. No tests were run while writing this proposal.

## Implementation order

1. Agree on local ownership semantics, deployment permission naming, and lifecycle/revocation policy.
2. Build candidate grant classification and immutable release authority capture with tests.
3. Add shared execution admission and token-consumer resolution across every execution path.
4. Enforce Events/Create with live execution validation and emitter provenance.
5. Align bootstrap, candidate tests, ad-hoc edits, diagnostics, and documentation.

Ship these as one coherent authorization change. Relaxing registration alone leaves runtime failures; relaxing registration and execution checks without fixing event authorization leaves broad token-type-based emission. Do not add a blanket Packs/Install bypass to `DelegationAuthority::covers` or globally grant Events/Create to deployers.

## Decisions to confirm before implementation

- Keep Install/Configure terminology or introduce an explicit Deploy action?
- Permit local authority changes only through deployment initially, or also create admitted revisions for ad-hoc edits?
- Which resource/action scope mappings are supported beyond Events/Create, and how do sensitive existing values retain their provenance protections?
- Do older in-flight runs survive ordinary redeployment as recommended, and what operator action explicitly revokes them?
- What limited post-completion callback access is required, without allowing more events?

The recommended design resolves the fresh-pack problem without granting the human unscoped Events/Create and without requiring a two-stage policy deployment.
