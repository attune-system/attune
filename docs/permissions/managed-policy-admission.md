# Managed policy authoring and admission

Pack permission sets default to structural pack ownership. The canonical v1
definition supports a set default and per-grant overrides:

```yaml
ref: example.effects
scope: pack
grants:
  - resource: events
    actions: [create]
  - resource: keys
    actions: [use]
    scope: external
    constraints:
      refs: [system.signer]
```

Local ownership comes from the managing pack ID, not the first segment of a
resource ref. Authored selectors narrow that ownership. System and human-owned
data and global administration do not become local. Platform catalog policies
keep their platform scope.

Permission inspection returns `authored_grants` separately from effective
`grants`. CLI export/import uses the shared definition parser and preserves
scope. Effective IAM grants carry a server-owned `managing_pack_id` constraint.
Authors cannot set that field. Direct and role assignments use the same
normalization. Consumers must establish structural target ownership before
matching a local grant; an otherwise matching display ref is insufficient.

Deployment captures immutable action, workflow-task, rule and queue bindings in
the activation transaction. Concrete selectors pin resource and owner identities.
Ownership scopes remain open to eligible future children. Rendered task
permission selections are bounded by the target action's defaults plus literal
refs declared by that task. A rendered ref outside that maximum is denied.
Dynamic action targets without a declared target set remain an explicit gate.

Managed action API edits create immutable metadata revisions referencing the
existing code release. They do not replace immutable release snapshots. A policy
edit readmits future local consumers and leaves existing admissions unchanged.
Changing an admitted pack policy needs pack configuration authority as well as
the endpoint's permission-administration authority. Unchanged grants on a
label-only edit do not require unrelated runtime delegation. Permission update
dry-run executes the same readmission transaction and rolls it back.

Accepted queue items must drain before dispatch templates or the current pack
policy revision change. Stop accepting new items while allowing existing items
to finish, then readmit the change. This avoids mixing distinct live revocation
dependencies in one batch. It does not partition batches by requester or grants.

Trigger deployment uses `acknowledge_incompatible_rollout` as separate consent
for a contract change affecting live admitted emitters. API install/register,
multipart upload and the corresponding CLI commands expose the flag. `--force`
does not supply this acknowledgement. Named dependency changes are compared by
resolved semantic contract, without removing existing named-consumer gates.

Mutation lock order starts with exclusive schema-graph admission, before cache
admission, pack locks, sponsorship and policy records. Runtime creation takes
shared graph admission before pack, sponsor, policy, owner and approval locks.
Bootstrap and custom deployment entry points must acquire the outer graph lock
before taking pack or cache locks; acquiring it inside a nested helper is too late.

These changes do not enable the coordinated registrar exemption or worker
credential switch by themselves. Execution admission must consume the recorded
binding and immutable metadata, and all runtime and IAM consumers must retain
the ownership envelope. Missing structural context fails closed. Existing
unbound policies, approvals and executions require controlled readmission rather
than inferred trust from names or installation attribution.

## Retained foreign workflow graphs

Foreign workflow invocations capture `foreign_workflow_policy`, containing the
publisher's immutable revision ID and digest. Capture verifies the publisher's
own Action admission against the exact action ID, graph metadata, metadata
revision, code release and digest. Source-pack policy contains the outer
invocation only. It does not copy the publisher's inner task refs or grants.

At child creation, require the parent's retained admission, then call
`WorkflowPolicyRepository::for_invocation` with its retained source policy,
invocation binding and pinned parent snapshot. Select the child from the returned
policy and resolve its immutable target metadata by task binding. The returned
policy has the publisher's sponsor for a foreign workflow. Runtime dispatch never
reads the publisher's current-policy pointer. Ordinary publisher replacement or
parent completion does not revoke admitted work. Explicit policy/admission
revocation, approval expiry and sponsor suspension still do.

`ParentExecution` attribution retains the parent admission's revocation fence.
Retry attachment must additionally call `retain_inherited_admission` with the
source and new admission IDs. Historical execution rows can expire independently;
referenced admission fences remain until descendants and input receipts release
them. Policy pruning retains foreign-policy targets and their code/metadata pins.
Dependency graphs are bounded to 256 retained revisions/admissions.

## Authored input purpose bindings

Canonical capture records template sites in pinned action defaults and workflow
graphs. A declaration binds action/pack/release/metadata identities, the digest of
the complete immutable executable metadata, an escaped source JSON pointer and
the exact authored expression. Metadata readmission captures new declarations for
future work. Existing admissions retain their original declarations.

`DeclaredInputRepository::permit` validates these identities and the live source
admission before issuing an opaque input-purpose permit. It allows only explicit
Key lookups and pack configuration lookups inside the declaration author's
structural pack. It does not expose grants, mint a credential or authorize an SDK
read. Foreign/system input selectors need separately admitted input-purpose
authority and are not covered by this local permit. Empty API refs remain
credential-free.

Renderers must walk fields in the pinned declaration. They must not reinterpret
caller-supplied selector strings or recursively evaluate strings produced by a
template with authored authority. A parameter reference alone cannot authorize a
Key lookup. Supplied values retain their separate accepted contributor authority.

Materialization records each actual Key or immutable configuration origin with
the destination execution and field path in the same transaction as encrypted
input storage. Worker restoration checks `delivery_allowed` for that exact field
and origin. This proof is not a general Key permission and does not replace
secret-schema checks, original provenance or later disclosure authorization.
Identical-input retries can copy receipts only after retaining their source
admission dependency. Workflow children record their actual new destination
paths rather than copying the parent's receipts wholesale.

The B/C consumers must use these graph and materialization interfaces before the
coordinated activation gate can be enabled. Source tests are not a substitute for
combined dispatch, restoration, revocation and retention acceptance tests.

## Approval expiry at effect time

Row locks prevent approval revocation from racing a transaction, but do not stop
time passing while it waits for a target. `ValidatedRuntimeAuthority` retains an
opaque `ApprovalEffectFence` with the earliest expiry from its selected approvals,
pinned foreign graph dependencies and retained parent/retry admissions. Dependency
approvals constrain liveness; they do not become additional API grants.

Target matching and new SQL-filter construction check the current clock. A filter
or boolean built before another wait is not a final effect permit. Keep the
authority or its effect fence through target, version and origin-capture locks,
then call `ensure_fresh_on(connection)` immediately before the effect or returned
disclosure. It uses PostgreSQL `clock_timestamp()`, not transaction-start `now()`,
and acquires no row locks. Recheck each listing response and streamed chunk after
its actual query/capture waits. Do not send bytes and check afterwards.

Native selection callers use `selected_effect_fence`; bounded worker completion
uses `retained_admission_fence` alongside its separate worker/attempt/version,
operation, byte and deadline bounds. These methods neither mint credentials nor
grant terminal execution API access. A new transaction must validate its source
again. Cached approval IDs, an old authorization boolean or a token TTL cannot
replace the final fresh-clock fence.
