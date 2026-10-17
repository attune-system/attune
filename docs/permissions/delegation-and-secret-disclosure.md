# Permission delegation and secret disclosure

## Pack registration

Pack installation does not grant permission to publish arbitrary authorization policies.
Registration checks permission-bearing metadata before candidate tests and again in the activation transaction.
The loader requires the registering identity's authority before it changes components.

A caller can publish permission-set definitions whose grants it already holds.
An unrestricted `permissions:manage` grant also permits policy definitions.
Attaching permission refs to actions, rules, queues, workflow tasks, or sensor cache access requires separate proof that the caller covers those grants.
An unused broad definition still requires authority to publish it.

Delegation compares resource, action, and scope constraints.
A restricted grant cannot authorize an unrestricted grant by matching a context with missing target fields.
Scope narrowing is allowed when the evaluator can prove containment.
Unsupported containment relationships fail closed.

The registration transaction captures authority before updating the pack's permission sets.
A set assigned to the caller cannot authorize its own replacement with broader grants.
The current caller becomes `pack.installed_by` for deployment attribution.
Included rules run under a separate system-owned pack identity. Deployment
captures their input grants and explicitly assigns their execution permission
sets to that identity. Ownerless rules cannot create executions.

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
Named sensor cache grants are checked against the registrar at deployment and
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
