# Managed component edits

Managed workflow graph/schema edits, rule dispatch edits, and queue dispatch edits
use the deployment runtime-policy compiler. The write and readmission commit in
one transaction under exclusive schema-graph admission. Failure rolls back the
projection and future-policy pointer together.

Workflow edits capture immutable action metadata over the existing code release.
The compiler reads the edited immutable graph and replaces that workflow's task
bindings, including removed tasks. Unrelated invocation bodies and concrete
ownership pins remain retained. Existing executions and retries keep their old
policy, code, schema, graph, and selected grants.

An unchanged exact invocation binding also keeps its admitted target metadata,
release, schema, grant bodies and foreign publisher-policy pin during source
readmission. A publisher's newer current policy is not a replacement for that
retained target. Newly introduced bindings undergo fresh deployment admission.

Rule and queue edits retain dispatch metadata under their immutable policy
revision in `managed_dispatch_metadata_revision`. These records are reclaimed
with the policy. The policy retains the source and target code releases through
the existing dependency mechanism. Labels, descriptions, and tags do not recapture
authority. Rule input-expression edits retain and verify the existing immutable
purpose-specific input ceiling and owner binding. They do not reload the editor's
Key grants or recapture mutable input sets. Runtime API selection does not grant
input access, and widening an input ceiling remains a separate admission change.
Rule source metadata includes verified `managing_pack_id`, the exact rule
ID/ref/pack-ref tuple, and actual trigger/sensor IDs and managing-pack proofs.
Foreign sensor relationships never establish ownership from a matching name.
Sensor placement is captured after its authorized write and structural admission
check. Workflow and rule input schema checks use the pinned executable contract
pair, not the publisher's mutable current schema or ref-only task discovery.

Control-plane Update checks obtain `managing_pack_id` from
`IamOwnershipRepository` for the actual action, rule, or queue and recheck that
proof in the mutation transaction. An ad-hoc component's pack association is not
managing ownership. Repeating the same managed queue owner is a no-op, not a
request to transfer ownership.

Foreign targets must pass current reference visibility against their actual pack
owner. A matching ref prefix is not proof. Foreign permission selections use the
same fixed external-approval checks as deployment. Neither an administrator nor
an approval can bypass a foreign private component's reference gate.

## Dynamic workflow actions

Canonical v1 tasks can declare a finite target ceiling:

```yaml
tasks:
  - name: select_target
    action: '{{ parameters.target }}'
    allowed_action_refs: [example.first, shared.second]
    permission_set_refs: '{{ parameters.selected_permissions }}'
```

`allowed_action_refs` contains 1 to 256 literal, fully qualified action refs.
Templates, wildcards, prefixes, empty lists, and missing bounds are rejected.
Duplicates are normalized. Every target is resolved to immutable executable
metadata and admitted separately under the exact authored task pointer and target
ref. Array and escaped named-map pointers retain their original identity.

A literal action may also declare this field, but must belong to its ceiling.
The field survives YAML, API graph JSON, parser serialization, and immutable
workflow snapshots. Runtime selection must match an exact admitted task/target
binding, not just a name in the graph. A rendered permission selection only
reduces that target's admitted defaults plus literal permission refs authored by
the task. It cannot acquire a different target's grants or inherit parent refs.

The workflow builder exposes an Action selector expression and an Allowed action
targets list in the task inspector. Its task selector includes nested scopes.
Existing targets remain editable even when they are absent from current discovery.
Suggestions come from the server's referenceable-action discovery for the
declaring pack, not the editor's permission-set membership. Structural preview
errors include the authored task pointer; the server still decides private and
restricted foreign referenceability and capability admission.

Load, save, YAML export and save-before-run share the same conversion. It retains
named task-map keys, escaped names, nested/fanout/cache fields, permission
selections and unedited task metadata. A historical dynamic task without a bound
can be repaired in the inspector. CLI workflow upload uses the canonical finite
target validator and preserves opaque task fields; workflow show JSON/YAML keeps
the stored definition. No client regenerates or invents runtime authority.

Current action schemas are authoring previews, not retained task contracts.
Loaded tasks keep explicit inputs even if they match a publisher's newer defaults,
and parameter edits preserve fields absent from that preview. File PUT compares
executable graph content separately from labels, layout and equivalent default
serialization, so an unchanged graph does not acquire a new policy revision.
Target ceilings, permission selections, payloads and schemas remain behavioral
changes that require admission.

## Accepted queue work

Queue template, dispatch target, schema, trace template, config, permission, or
batch-envelope changes require queued, leased, and retry items to drain. Policy
activation also retains the existing pack-wide drain check before changing the
revision ID. No accepted item is rebound, and no batching partition is introduced
based on requester identity or grant differences. Enqueue-only defaults and labels
do not reinterpret accepted payloads.

Managed workflow file PUT records metadata rather than rewriting released code
bytes. Ad-hoc file publication happens after the database commit and reports a
committed-publication failure explicitly.

## Runtime integration

The planner emits one existing `InvocationBinding::WorkflowTask` for each exact
task/target choice. Consumers must use the parent's retained policy and immutable
graph. Resolve a target with
`ManagedExecutableRepository::resolve_invocation(connection, retained_policy, binding)`
and validate it through B's `workflow_child` admission. Do not reload the foreign
or local current action to choose executable bytes, manufacture parent refs, or
use an unbounded rendered ref as a binding.

Foreign nested workflows use A's `WorkflowPolicyRepository` publisher pin and
scoped executable lookup. Source verification uses its canonical
`task_at_authored_path`, including normalized escaped names and numeric-name/index
ambiguity denial. A captures purpose-specific authored input declarations on each
canonical policy capture. This module does not duplicate those declarations or
B/C's materialization and delivery receipts. No worker issuance or registrar
exemption is enabled by these changes.
