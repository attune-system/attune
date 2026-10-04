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
The current caller becomes `pack.installed_by` and the owner of included rules.
Ownerless rules cannot create executions.

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
Named sensor cache grants require the current pack installer's authority.

## Key values and approved operations

`keys:read` grants metadata access to encrypted keys.
Returning their plaintext requires `keys:read` and `keys:decrypt` on the key's actual owner-qualified ref.
Workers do not enumerate system, pack, or action keys into action input.

Explicit rule, workflow-task, and queue templates resolve only referenced keys.
Resolution checks the active rule owner, workflow executor, or attributed queue requester.
An encrypted key requires read and decrypt authority before arbitrary action code receives its value.
Key-derived destination parameters must be marked `secret: true`.

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
Current pack metadata never substitutes for a missing historical binding.

Origin JSON uses the existing `execution_secret_value.source_kind` and `source_ref` columns.
The canonical pre-production contract remains v1.

## Trusted initialization

Operator-owned bootstrap creates a real registrar identity and attributes pack and rule metadata to it.
HTTP bootstrap revokes its temporary integration credential after registration.
The identity remains while packs or rules reference it.
Editable pack metadata, identity attributes, and NULL owners do not establish privileged runtime trust.
