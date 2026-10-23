# Recommended self-contained pack semantics

Date: 2026-10-13

Status: design recommendation only. No implementation or deployment changes.

Related findings: [deployment binding idiosyncrasies](../permissions/deployment-binding-idiosyncrasies.md). Detailed implementation inventory: [pack-owned runtime authority](pack-owned-runtime-authority.md).

## Recommendation

Make the pack the unit of managed ownership, deployment, and runtime authority. Treat an authorized deployment as admission of a complete internal component graph, not as a collection of individual human permission delegations.

Keep runtime privileges explicit and selected per action, but resolve pack-local privileges from the admitted pack release. The installer and invoker do not need to hold those privileges personally.

Use four rules throughout:

1. Deployment authority permits managing the pack's own components and their internal connections.
2. Runtime authority belongs to the pack and is restricted to the executable's selected, admitted permissions.
3. Access outside the pack requires an explicit dependency and, where privileged access is needed, an approved external capability.
4. Code and runtime policy are release-bound; configuration and explicit revocation have separately defined lifetimes.

## The normal authoring experience

An author defines a trigger, an event-emitting action, a runtime permission set, and a rule in one pack. Someone allowed to deploy that pack uploads it once. Internal refs resolve regardless of file order. No manual identity assignment is required. An authorized invoker can run the action without having Events/Create themselves. The action can emit only events allowed by its selected runtime policy.

The pack runtime identity is not a general administrative identity. It sponsors admitted capabilities and never supplies arbitrary grants to a human or to unrelated actions.

### Make local scope the authoring default

Recommended authoring contract: pack-owned runtime permission sets have a pack-local scope envelope by default. A grant cannot escape that envelope merely by omitting constraints. During admission, normalize effective grants to explicit ownership constraints before storage, IAM expansion, or runtime evaluation.

For example, an Events/Create grant without narrower selectors in a local set means events for triggers owned by this pack, not events everywhere. A narrower `refs` selector can restrict it to one trigger. External or global authority requires an explicit external scope and approval; it must never be inferred from YAML placement or an unrestricted grant object.

This is a proposed change from the current raw grant semantics and from the stricter authoring recommendation in the earlier runtime-authority plan. Do not implement it only in the event route. The same normalized scope must apply everywhere the set can be consumed, including human IAM assignments if those remain supported.

If changing permission-set authoring defaults is out of scope, use explicit `pack_refs` constraints initially. The ownership-based admission and runtime model still works; authors just supply the local scope themselves.

Keep the existing `permission_set_refs` opt-in and selection mechanism. Do not automatically give every action all pack privileges. Keep `standard` limited to its documented scoped resource operations, without implicit event creation. Request overrides may reduce authority, not attach an undeclared broader local set.

## Deployment and component binding

### Check one pack management entitlement

Resolve the candidate pack ref before evaluating scoped installation authority. On replacement, require authority over the existing pack. Do not require the deployer to personally hold every runtime grant on internal candidate resources.

Use the current Install/Configure permission vocabulary initially rather than adding another overlapping permission solely to fix this behavior. Document them as the creation and update operations for pack management. Global index administration and management of other owners remain separate.

### Admit the complete graph

Use one common deployment planner:

```text
read complete candidate metadata
  -> resolve refs and prove ownership
  -> reserve component identities
  -> validate contracts and external access
  -> compile per-executable runtime policies
  -> test candidate in its isolated admission context
  -> atomically activate release and policies
```

Do not bind workflows while only part of the candidate action list exists. Ordinary actions, nested workflows, permission sets, triggers, rules, and queues must have the same forward-reference behavior. Reject invalid cycles deliberately, not accidentally through filename order.

Use the same planner for upload, register, install, bootstrap, and managed metadata edits. A direct edit can produce a new admitted metadata revision, but must not bypass ownership or capability checks. Do not maintain a second Python interpretation of permission admission.

Scope the atomicity promise to active metadata, executable policy, and retained configuration. External effects of candidate code or tests are not rolled back automatically. Keep candidate events and resources isolated from active workloads.

### Internal references need no extra IAM ceremonies

An authorized pack deployment can connect its own actions, triggers, workflows, queues, types, and local secret selectors. Private visibility does not obstruct same-pack composition.

Foreign private components remain private even to an installer who happens to be an administrator. External reference permission and privileged runtime capability are separate requirements, but diagnostics should state both clearly. A declared dependency is not a grant of privileged authority.

Platform metadata, IAM administration, workers, system-owned secrets, and another owner's components never become local because their names resemble the pack's refs. Prove ownership from server records and candidate identities.

## Runtime behavior

Give every pack a stable system-managed runtime sponsor. For each action or invocation binding, capture selected grants and their approved targets in the release authority record.

The human maintainer, structural component-owning pack, and runtime sponsor are different concepts. Human ownership determines management rights, not automation liveness. Inactivity of a maintainer, uploader, requester or approval issuer prevents their interactive access and new human requests but does not change admitted runtime behavior. Capture request-specific authority at admission rather than consulting a human's active status during deferred work.

All execution paths use that binding: manual execution, rules, workflows, queue dispatch, fanout, retries, and execution-created children. The requester remains recorded for accountability and invocation authorization, but their personal grant list does not determine whether admitted pack-local code works.

- Workflow tasks use their own declared action/invocation policy. Parent grants are not an accidental inheritance mechanism.
- An unrelated rule cannot expand another workflow's authority through shared identity assignments.
- Queue capability does not depend on whether a batch contains one or several requesters. Batch eligible items across requesters by default; provide an explicit requester-separation control through pack metadata, UI, CLI/API and documentation. Preserve per-item attribution/input authorization and origin-aware result/log visibility. Do not select a privileged requester or silently split work solely because requesters differ or become inactive.
- Cross-pack children receive the target action's admitted capability under authorized invocation rules, not all rights held by their parent pack.
- Token consumers resolve recorded authority and validate the current execution, sponsor, and revocation state. They do not trust refs alone or reconstruct local policy from the installer's current grants.
- Events/Create is checked against the actual trigger and owner. Token type alone is never permission to emit.

An action invocation intentionally permits its declared behavior. A user without direct Events/Create can cause an event through an action they may execute. Control that behavior through action visibility, execution authorization, schemas, and implementation rather than requiring the user to own the action's internal API permissions.

## Configuration and secrets

### Separate release defaults from operator values

Use the same configuration model for packs and sensors:

```text
effective configuration = release defaults + persisted operator overrides
```

Redeployment updates defaults and validates existing overrides. It does not erase overrides because a YAML field is absent. Provide an explicit reset operation. Define merge behavior through the compiled flat field contract rather than an undocumented recursive JSON merge.

If new schema/defaults make preserved values invalid, reject activation with a sanitized, field-specific migration diagnostic and leave the active release unchanged. Use the same semantic contract comparison for API edits and deployment. Resubmitting an equivalent contract should not require extra decrypt authority through one path but not another.

### Preserve secret provenance without installer-dependent runtime permissions

Captured input authority, runtime API authority, and human plaintext disclosure are separate concerns. Use the same pack sponsor and approved bindings for local rule/sensor input delivery, while retaining their purpose-specific grants and provenance records.

For external secrets, approve the actual source owner and operation explicitly. A rule's execution-input selector and the sensor's trigger-input selector must be admitted for their respective consuming code. Prefer validating both through one graph so authors do not discover a missing sensor grant only after activation.

Metadata-only changes such as labels or trace correlation should not require coverage of an unrelated broad input grant list. Changes to secret selectors, consumers, schemas, or disclosure behavior must trigger targeted readmission. Preserve Key-selector authoring and recorded-origin checks rather than replacing them with general plaintext access.

There is an unavoidable trust implication: permission to deploy executable code into a pack must be treated as control over that pack's runtime capabilities, including secrets delivered to its code. If someone must not influence code that can read an existing secret, they must not have unrestricted deployment authority over that secret's consuming pack. Use a separate pack or separately approved external capability; do not claim that denying human decrypt alone prevents extraction through deployable code.

## Predictable lifecycle rules

| Operation | Recommended behavior |
| --- | --- |
| Normal redeployment | New admissions use new code and policy. Existing runs retain pinned bindings unless explicitly revoked. |
| Omit a managed component | Retire it from new admission by default. A referencer left active causes a clear dependency error. |
| Omit a local permission set | It cannot be selected by new admissions. No lingering identity assignment reintroduces it. |
| Disable a component | Prevent new invocations. Cancellation/revocation of existing runs is an explicit separate operation. |
| Cancel/finish an execution | Its event-emission authority stops immediately, regardless of JWT expiry. |
| Revoke a pack or external capability | Live token consumption fails for affected authority, including already-issued tokens. |
| Freeze a human maintainer/uploader/requester/issuer | Prevent their interactive access and new human requests. Already admitted automation and recorded approvals continue. |
| Transfer human ownership | Change who may manage configuration/behavior without changing runtime capabilities or execution liveness. |
| Explicitly suspend the system runtime principal | Stop its runtime authority through a deliberate operational control, separate from human inactivity. |
| Change a mutable set definition | Does not rewrite already admitted local executable policy. Require readmission for future use. |
| Delete/recreate a target | Do not transfer old rights to a new resource identity or owner merely because a ref is reused. |

Existing omission policies should remain explicit overrides with inspectable consequences. The normal default should favor a complete desired-state pack, rather than leaving absent policy active under a misleading disable operation.

External approvals belong to recorded, revocable capabilities with explicit lifetime and scope. Do not make them silently depend on the original installer/issuer's current permissions or active status. Human issuer freeze does not revoke an approval. Explicit operational revocation, expiry and approved resource lifecycle events can. Security response may deliberately revoke approvals separately from disabling human login.

## Keep the implementation small at its interfaces

Two shared modules should own the complexity:

- A deployment admission module that takes the candidate graph and pack management authority, then returns an immutable admitted release plan or located diagnostics.
- An execution authority module that takes a pinned executable/invocation and requester context, then admits and validates its runtime capability.

Keep resource-specific scope evaluation and provenance checks inside these modules where possible. Repositories own persistence and locking. API, worker, scheduler, queue, and bootstrap callers should not each recreate deployment or delegation policy.

Do not add a universal deploy-permission bypass to IAM grant matching. The ownership proof and approved executable binding are what permit local operations.

## Delivery scope

Start with Events/Create and the existing local input/resource contracts, but establish the common ownership and binding model first. Add further resource/action mappings only when their local scope is precisely defined. Ship installation admission, runtime resolution, event enforcement, and revocation together so deployment success actually yields the promised behavior.

Configuration consistency and complete candidate-graph binding should follow the same architecture. Temporary named-schema rollout gates can remain explicit until consumer integration is complete; changing authorization does not make an unported consumer safe.

The user-facing result should be simple: deploy a complete pack, configure it, invoke its actions, and get the same behavior whether invocation comes from a user, rule, workflow, or queue. Only genuine external ownership, unsupported contracts, invalid configuration, and explicit lifecycle controls should interrupt that flow.
