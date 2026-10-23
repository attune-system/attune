# Permission delegation and secret disclosure

## Pack registration

Pack ownership records the primary owner or installer; it is not an additional
authorization gate. Applicable RBAC grants authorize access regardless of who
owns the pack. Global `packs:read` includes packs installed by other identities.
Explicit owner, pack, ref, and ID constraints still limit their grants. Reading a
pack does not authorize configuration, replacement, deletion, or secret disclosure;
each operation requires its own applicable permissions. Access does not transfer
ownership.

`packs:install` is trusted deployment authority for the matching pack. A global
grant permits deployment to any pack; a ref-scoped grant permits deployment only
to matching pack refs. The same permission authorizes replacing that pack with
an explicit replacement request. Deployment does not additionally require
`packs:configure` or personal possession of every permission declared by the pack.

`core.editor` retains global pack read access. Its namespace-claim grant allows
first installation through `constraints: {pack_state: uninstalled}`. A separate
grant permits install and configure on packs owned by the requesting identity,
through `constraints: {owner: self}`. Ownership includes the primary owner or
current membership in the pack's declared owner roles. Redeploying or
configuring other packs requires an additional matching grant. `core.admin` retains global
install authority. Ownership remains attribution, not an extra deny on other
applicable grants.

`uninstalled` means no persisted pack or namespace entry exists. An existing pack
with NULL `installed_by` does not qualify. The platform namespace `core` remains
reserved even without a pack row. Missing namespace context fails closed.
Preflight resolves the actual pack owner; activation resolves it again under the
pack mutation lock before effects. Concurrent first uploads cannot both claim
the same ref. The create endpoint requires both matching create and install
authority under that lock and cannot use a namespace claim to mark a pack as
standard. Its normal editor grant uses the same `uninstalled` constraint.

Authorized deployers may publish permission sets broader than their own grants
and select permission refs in deployed components. Permission definitions must
remain in the deploying pack's namespace and cannot overwrite another pack's
components or platform-managed policies. Global or cross-pack runtime authority
still uses explicit external scope and recorded approvals when selected for runtime
use. Deployment can issue those approvals for its target pack without granting them
to the human uploader.
Schema validity, reference resolution, immutable releases, concrete target pins,
and secret-disclosure contracts remain enforced.

Publication is separate from runtime admission. Unused human IAM definitions may
contain `owner: self` or `pack_owner: self`; they are stored without runtime grants
or runtime approvals. Every definition still requires valid authoring metadata,
scope and namespace ownership. Selecting a human-relative definition for execution
remains invalid. Only the permission refs needed by each pinned invocation ceiling
are compiled and approved for runtime use. Omission uses pinned action defaults;
`[]` selects none. Dynamic workflow tasks admit their finite targets' defaults plus
literal task refs, not every published definition or the parent's selected API refs.
Retained bindings keep their recorded permission bodies independently of current
definitions. Sensor-purpose and rule-input selections retain their own admission
checks. An unused external definition no longer requires a runtime approval merely
to be published under trusted deployment authority.

Registration checks deployment authority before candidate tests and again in
the activation transaction. Runtime approval issuance rechecks an active
deployer's authority for the specific subject pack. Direct component edits and
explicit approval APIs do not acquire this deployment exception. Publishing a
permission set does not create user or role assignments for it. Updating an
already assigned permission set does change the grants of its existing assignees,
so pack deployers must be trusted to administer the policies in their packs.

Outside trusted pack deployment, delegation compares resource, action, and scope constraints.
A restricted grant cannot authorize an unrestricted grant by matching a context with missing target fields.
Scope narrowing is allowed when the evaluator can prove containment.
Unsupported containment relationships fail closed.

The registration transaction captures authority before updating the pack's permission sets.
A transaction-bound `AuthorizedPackDeployment` retains the original namespace
context and locked IAM snapshot before candidate mutations. After row creation it
binds to the exact pack ID, ref and actor. Component metadata admission and runtime
external approvals validate that binding and the database transaction ID, rather
than treating the newly created row as an uninstalled namespace. The capability
cannot authorize another deployment transaction or another pack. First-install
authority therefore survives row creation without granting later replacement or
personal permission-administration authority.
A candidate permission-set change cannot supply missing deployment authority
for its own activation. An authorized deployer may update an assigned set.
The first registering identity initializes `pack.installed_by`; redeployment
preserves that primary owner. Install records and deployment audits identify
the current deploying identity separately.
Included rules run under a separate system-owned pack identity. Deployment
captures their input grants and explicitly assigns their execution permission
sets to that identity. Ownerless rules cannot create executions.

## Pack-owner component constraints

`constraints: {pack_owner: self}` requires the requesting identity to be the
persisted primary owner or a current declared owner-role member of the target's
structurally managing pack. It does not
grant permissions by itself. Another applicable grant can still authorize the
operation. Only `self` is accepted for `pack_owner`.

This constraint supports pack metadata and managed actions, rules, policies,
queues, triggers, sensors, runtimes, and dashboards. Workflow metadata uses the
Actions resource with its actual managing ownership. Platform components,
standard packs, packs without a matching primary owner or declared role membership,
missing parents, and ad hoc components do not
qualify. A matching ref prefix, `pack_ref`, or association-only pack FK is not
ownership. Repositories resolve the actual structural FK and
`management_origin = 'pack'` before evaluating the pack's persisted ownership.

`owner: self` remains target-own authorization. It is not reinterpreted as pack
ownership for a rule's runtime identity, an execution's executor, a dashboard's
author, or identity-owned data. `pack_owner` does not apply to execution lineage,
queue items, secrets, caches, or permission administration. Unsupported resource
declarations are rejected; unsupported projections fail closed.

Request-local IAM matching and SQL visibility can narrow `pack_owner` into
explicit managing-pack IDs after querying current primary owners and, for access
credentials, current declared-role membership. These copies
retain all other constraints. Assignment, policy serialization, and delegation
use the canonical relative grants, not those narrowed copies. Captured runtime
policies continue to reject human-relative grants, including `pack_owner`.

API-created actions, rules, queues, and dashboards are ad hoc. Associating one
with an owned pack does not make it eligible for `pack_owner` create authority.
Policy and runtime creation can qualify when their real structural destination
pack is owned by the caller. Managed queue ownership cannot be changed by a
component edit. Existing component editing interfaces do not gain new ownership
transfer powers. No bundled editor component grants are changed by this feature.

## Declared pack owner roles

Pack manifests may declare exact identity-role names:

```yaml
owner_roles: [platform-team, automation-maintainers]
```

The typed `pack.owner_roles` column is separate from arbitrary `meta`. Declaring
roles does not create memberships, assign permission sets, or grant actions.
The first human owner remains recorded in `installed_by` across deployments.
Access-token identities with current persisted membership in a declared role
can satisfy `pack_owner: self` and `owner: self` on pack targets. Non-pack
`owner: self` still refers only to the target's own identity. Execution and sensor
credentials do not borrow a human's owner-role membership. Registered pack
runtime identities cannot use owner-role membership either.

Membership is queried from active identity and role-assignment rows for each
ownership authorization, not JWT metadata or role-name caches. Removing a role
therefore removes its ownership eligibility on the next request. Existing
independent global/scoped grants can still authorize the operation.

Names remain case-sensitive exact strings, with no trimming or case folding.
Lists accept at most 64 entries; names require 1–255 characters, must not be
whitespace-only, and cannot contain control characters. Exact duplicate names
are collapsed in first-occurrence order. Null, scalars and non-string entries
are rejected. Missing declarations default to [] on new packs; replacement
omission preserves the stored list, and an explicit [] clears it.

Deployment requires Install against the previous owner/role declaration before
candidate effects and again under the pack mutation lock. The activation
transaction holds current membership rows and keeps the previous declaration
through runtime approval rechecks, then writes the new list before commit.
Candidates cannot add a caller's role to authorize their own replacement. An
authorized member may deliberately remove its own role.

The create API already requires Create and Install. Updating `owner_roles`
through the pack metadata API requires an access token plus matching Configure
and Install against the previous declaration, with a fresh transaction check.
Changes to `is_standard` also require Install because that flag affects ownership
eligibility. Configure alone cannot rewrite ownership. Scoped or global trusted
installers may publish role declarations without joining those roles themselves.

Migration `20261021000001_pack_owner_roles.sql` adds the column and list-integrity
constraint. It must be applied with the compatible services during a coordinated
rollout. Catalog revision remains 2; the editor's component grants are unchanged.

## Execution authority

Execution API access remains opt-in through `permission_set_refs`.
Named refs resolve to active permission sets, and their grants must remain covered by the current active executor identity.
The API and notifier enforce this bound independently.
Freezing or removing that identity prevents named-token authorization.

Workflow child creation checks rendered permission refs, including ordinary tasks, `with_items`, batches, and cache iteration.
Queue batches need one attributed requester to obtain execution API access or resolve explicit key values.
Execution-token enqueue preserves its requesting identity.
Missing ownership never implies identity `1` or another privileged actor.

The reserved `standard` ref retains its scoped key and artifact contract.
It does not grant shared system-key access.
Sensor standard cache access remains read-only within the sensor and its pack.
Named sensor cache grants are admitted under trusted pack deployment and
captured in the immutable executable release. An admitted workload generation
uses those grants without a live dependency on the installer's identity or
current access. Tokens remain fenced to a current, eligible workload assignment.
Sensors declare explicit named `config.signing_permission_set_refs` for approved
JWT signing. Deployment checks and captures only their Key `use` grants. Runtime
requires the admitted grant and the operator signing profile. The reserved
`standard` ref does not grant signing, and signing grants do not authorize
plaintext Key reads or decryption.

Sensors declare `config.input_permission_set_refs` for server-rendered Key input
templates. Deployment validates and captures only Key and Pack read/decrypt grants from
these explicitly named sets. Rendering resolves only referenced Keys with that
release-owned authority. Signing grants cannot authorize input resolution.
Secret pack-configuration templates require the captured Pack read/decrypt grants.
Public configuration selectors remain usable without decrypt authority; a
redacted source cannot be substituted into a rendered input.

Sensor SDK Key reads separately retain implicit read/decrypt access to Keys owned
by the sensor's pack. Signing and input permission sets do not remove that access.
Keep a signing Key outside that pack-owned scope when the sensor must have only
`use` access to its private material, and grant the approved signing profile
explicitly.

Successful SDK plaintext reads record immutable Key ownership origins under the
current fenced generation, without storing credential values. Pack-scoped sensor
logs require sensor visibility and decrypt authority even when no schema declares
secrets. Their plaintext also requires read access to every recorded Key, plus
decrypt access for encrypted Keys. Streams recheck these origins before each
frame. Service mirroring and status/alert stderr excerpts remain disabled for
these potentially sensitive generations.
The bundled `core.admin` role grants trigger management, not `triggers:decrypt`.
Assign an explicit sensor-scoped decrypt grant when an operator needs plaintext
logs from an SDK-capable sensor.

## Key values and approved operations

`keys:read` grants metadata access to encrypted keys.
Returning their plaintext requires `keys:read` and `keys:decrypt` on the key's actual owner-qualified ref.
Workers do not enumerate system, pack, or action keys into action input.

Explicit rule, workflow-task, and queue templates resolve only referenced keys.
Resolution checks the active rule owner, workflow executor, or attributed queue requester.
Pack-installed rules declare `input_permission_set_refs` separately from execution
`permission_set_refs`. Deployment captures only Key and Pack read/decrypt grants
from those input sets after validating the registrar's delegation authority.
Enforcements, executions, workflow children, and retries retain that input
authority. Installer freeze or permission-set edits do not replace captured
input grants. Freezing the pack runtime identity stops input delivery.
Input grants do not become execution API permissions. Changing deployed rule
inputs requires the caller to cover its captured input grants.
Retained workflow/default declarations use purpose-qualified input receipts.
Execution config, execution results and workflow variables are separate entities,
even at the same JSON pointer. A render session evaluates the exact admitted
declaration once and retains the evaluator's selected source fields. Supplied
strings remain literal values. Condition proofs remain control dependencies;
they do not claim that a condition's boolean rendered a published value.

An external declared Key input requires independent input-purpose consent.
`POST /api/v1/packs/{pack_ref}/external-input-approvals` takes either an existing
`renderer_execution`, or `policy_revision` with `invocation_binding` before any
renderer exists. Both forms require declaration revision, source path, exact expression, read/decrypt
grants and optional expiry. The issuer must currently cover those grants or
manage permissions. The fixed approval applies only to that declaration, not
selected SDK refs. Its revocation and expiry remain dependencies of propagated
receipts after the original Key read. Human inactivity does not revoke it.
Inspect, replace and revoke these approvals through the existing external
approval controls. Replacement affects future reads, not receipts that depend
on the explicitly revoked predecessor. Input-only approvals cannot be reused
to admit runtime API grants.

An encrypted key requires read and decrypt authority before arbitrary action code receives its value.
Key-derived destination parameters must be marked `secret: true`.

Rules never configure plaintext secret values. Secret-marked `trigger_params`
and `action_params` must contain whole Key templates such as
`"{{ keystore['system.api_token'] }}"`. Secret objects and lists may contain Key
templates at their text leaves, or use one Key template for the entire value.
Literal credentials, mixed text, and non-Key templates in secret destinations
are rejected. Schema compilation prohibits defaults beneath secret markers,
including defaults containing Key templates. API writes and
pack registration use the same compiled-schema check; admission rechecks stored
definitions before rendering. Resolved runtime copies remain encrypted and
retain their original Key provenance. The authoring row stores selectors only.

Approved operations have a different contract.
The JWT signer accepts `keys:use` or `keys:read`, checks an operator profile, and returns a constrained assertion.
The signer keeps the private key inside the API process.
Read permission does not authorize raw private-key retrieval.
The [shared JWT signing guide](../guides/shared-jwt-signing.md) describes profile configuration and action use.

## Recorded secret origins

Execution visibility and entity-level decrypt authorization precede field disclosure.
Each encrypted field records the resolved origin at materialization time.
Key origins include the key ID, canonical ref, owner type, owner identity or ref, and encryption state.
Template records identify the component, input path, and expression that produced the value.

Disclosure checks every recorded origin.
A value that combines two keys requires read authority on both keys and decrypt authority on each encrypted key.
Changing `config.signer_ref` from key A to key B does not change the origins of historical values.
Decrypt authority on B alone cannot disclose a value resolved from A.

Workflow parameter and result references retain their source execution and path.
Source execution checks use the same visibility, ancestry, and decrypt rules as direct execution reads.
Transforms, published variables, and item or batch expansion preserve those references.
Sensitive workflow variables and output-map values stay encrypted at rest.
Secret action outputs conservatively inherit all secret input origins because arbitrary code cannot prove a narrower dependency.
Sensitive actions also encrypt raw stdout and error text, so those fields cannot duplicate protected structured output.
Runtime-log reads require execution read and decrypt authority plus disclosure rights on every recorded secret origin when inputs or declared outputs are sensitive.
This check covers direct execution streams and runtime-log artifacts.

Workers recheck recorded key origins before restoring secret action parameters.
Local caller-supplied values can pass through unattributed batches without granting key or API authority.
Unavailable source records, unknown provenance, missing bindings, and cycles fail closed.
Older rows without recorded provenance remain redacted and cannot establish authority for secret delivery.

Event-derived values record the original event ID and secret path. Execution
plaintext disclosure requires read/decrypt access to that event and its retained
binding, not merely permission to decrypt the destination execution. Missing or
expired event sources fail closed. Webhook transport keys and supplied headers
use a separate server-owned encrypted metadata contract; trigger-instance
parameters do not classify emitted payloads or transport metadata.
Current pack metadata never substitutes for a missing historical binding.

Queue-derived execution values record the original queue ID, ref, pack ref, item
ID, and path. Plaintext disclosure requires read and decrypt rights on that
recorded queue in addition to execution visibility and decrypt authority. Payload
updates do not change historical origins. Locally supplied queue values may still
flow through unattributed batches without granting Key or execution API rights.

Schema definitions are non-sensitive metadata available to any authenticated
caller. Type visibility governs pack references, not schema inspection or Key
assignment. Key value updates validate under the retained schema binding.
Changing or rebinding a schema over preserved material requires Key read and
applicable decrypt authority; a caller cannot use schema validation to inspect
encrypted material using update authority alone.

Origin JSON uses the existing `execution_secret_value.source_kind` and `source_ref` columns.
The canonical pre-production contract remains v1.

## Trusted initialization

Operator-owned bootstrap creates a real registrar identity and attributes pack deployment metadata to it.
HTTP bootstrap revokes its temporary integration credential after registration.
The identity remains while packs or rules reference it.
Editable pack metadata, identity attributes, and NULL owners do not establish privileged runtime trust.
