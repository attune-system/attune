# Schema driven action output secrecy

## Status and objective

Implementation in progress. Secret input parameters must never, by themselves,
classify an execution's entire output as secret. Pack authors define which output
values are public and which are secret through the action's stored output
contract. Classification is deterministic and does not scan output text for
credentials or infer secrecy from parameter names.

This changes the trust boundary: trusted pack code and its published output
contract can produce public summaries from secret inputs or API reads. Pack
authors are responsible for marking outputs that actually contain secrets.
Recorded origins remain necessary for revealing values classified as secret;
they do not automatically classify unrelated output values as secret.

## Pre production design policy

Design the canonical output model for the required behavior. Existing consumer
assumptions are code to replace, not compatibility requirements. Update producers,
storage, native workflow/queue consumers, and external projections together. Do
not preserve blanket redaction or add adapters merely to satisfy today's result
validators. Keep the canonical format v1 and avoid dual readers or legacy modes.

A storage inconsistency found during review identifies an implementation task,
not a reason to narrow the feature. Correctness of the replacement model and its
consumers remains required; compatibility with the broken model does not.

## Current causes

- `crates/common/src/secret_values.rs::redact_bound_action_result` protects raw
  stdout and error text when any secret parameter or runtime Key origin exists,
  even without secret output fields.
- `protect_runtime_read_result` protects entire data/stdout/stderr/error/queue_ack
  fields whenever recorded runtime reads are present.
- The archived v0.10.3 worker additionally inserts a
  `delegated_execution_output` origin for delegated executions with selected API
  permission refs. This can classify outputs without an actual protected read.
  Reconcile that implementation with the implementation branch before editing;
  this checkout and v0.10.3 differ.
- Text-format actions have no parsed `result.data`; blanket stdout protection
  therefore hides their only output.

## Authoring contract

Keep `out_schema` as Attune's existing flat field map. Reuse nested `secret: true`
annotations, named types, references, arrays, and the compiler in
`crates/common/src/value_schema/`. Do not introduce a parallel path language,
regex masking, raw JSON Schema at the flat root, or a new pack format version.

```yaml
output_format: json
out_schema:
  status:
    type: string
  account:
    type: object
    properties:
      name:
        type: string
      access_token:
        type: string
        secret: true
  credentials:
    type: array
    items:
      type: object
      properties:
        service:
          type: string
        password:
          type: string
          secret: true
```

Here `status`, `account.name`, and each `credentials[*].service` remain public.
Only the token and password values are encrypted and replaced by redaction
markers. An enclosing `secret: true` protects that entire subtree. Preserve the
compiler's monotonic inheritance: `secret: false` cannot override a secret parent
or referenced type. Fields without secret annotations are public, including
allowed extra fields; authors can constrain extra fields using the supported
schema facilities. Missing output schemas do not inherit secrecy from inputs.

### Raw streams and diagnostics

Propose an optional action-level `output_policy` alongside `out_schema`, because
stdout, stderr, and execution error text are not fields of parsed `result.data`.
These names and values are proposed API additions, not existing syntax:

```yaml
output_policy:
  stdout: sanitized  # public | secret | sanitized
  stderr: secret    # public | secret
  error: public     # public | secret
```

- `public` exposes that channel with ordinary resource read authorization.
- `secret` protects the entire channel with encrypted storage and explicit,
  authorized reveal.
- `sanitized` is supported for structured stdout. Render a serialization of the
  parsed result with schema-secret paths replaced by markers. Preserve the
  original privately for authorized reveal; never publish both versions in the
  ordinary response. Whole stdout protection is an explicit author choice, not
  a consequence of secret parameters.

For structured output containing secret fields, reject `stdout: public` at
publication: the raw duplicate would defeat those markings. Authors can expose
public summaries through the structured contract; removing secret markings is an
explicit contract edit. Use `sanitized` for selective release and `secret` for a
wholly private raw channel.

Proposed defaults depend only on the output contract: with secret structured
fields, stdout defaults to `sanitized` and stderr/error to `secret`. Without
secret output declarations, channels default to `public`. Text-format stdout
supports `public` or `secret`; partial text secrecy requires changing to a
structured format. Reject unsupported policy/format combinations at publication.
Input sensitivity and API permission refs never alter these defaults.

A sanitized stdout projection must contain only the parsed value. Leading debug
lines must not be copied through merely because the parser accepts a final JSON
line. For JSONL, classify each complete record separately. If parsing, truncation,
or resource limits prevent safe projection, protect only the affected raw channel
and return a non-sensitive reason. Do not reclassify valid public `result.data`.

## Runtime classification and provenance

Separate two operations in the worker and workflow completion paths:

1. Classify concrete output paths using the immutable output contract and channel
   policy. The presence of secret parameters, selected permissions, or runtime
   reads must not expand this path set.
2. Encrypt those paths and attach the origins required for their later
   disclosure. Initially retain the existing conservative union of applicable
   secret input/runtime origins for declared secret outputs. Narrower dependency
   attribution is separate future work.

Remove blanket classification from `protect_runtime_read_result` and the
v0.10.3 delegated-output hook. Retain runtime read capture, delegation validation,
source access checks, and token fencing. Runtime reads may affect who can reveal
an already-secret value; they do not override the author's public output fields.
An author publishing an unmarked public field deliberately permits its release,
including derived values from protected inputs. No runtime caller gains a bypass
of the secret markings in the stored contract.

The authority to publish or edit executable action/workflow code also includes
the authority to edit its output classification. Use the existing applicable
pack publication and component-edit permissions; do not add a new declassification
permission or approval mechanism. This applies to ad hoc definitions, referenced
types, and channel policy changes through their existing authoring routes.
Admission still validates referenced types, and bindings pin the selected contract.
Ordinary execution callers cannot supply or override the bound output contract.
Source authorization is enforced before code receives any protected value.

Use concrete compiler paths for nested objects, arrays, JSONL records, named
types, and schema composition. Output conformance warnings remain advisory and
must not themselves cause whole-result secrecy. When malformed output makes
secret traversal ambiguous, protect the smallest ambiguous subtree that contains
potential secret descendants, preserving independently classifiable siblings.
Whole-result protection is appropriate only when that subtree is the whole
result, or the author explicitly marks it secret. Keep warning details free of
values and secret-bearing validation text.

Workflow output maps use the workflow's own stored output contract and existing
top-level result shape. Parent publish/output maps must not silently inherit
blanket child secrecy; author-declared secret paths retain child origins.

Queue completion consumes the canonical internal output representation, not the
public API JSON. Extract and validate acknowledgement controls in the authorized
internal consumer even when an enclosing acknowledgement is secret. Classify
public acknowledgement duplicates consistently. Copies of secret summaries or
errors into queue item state and workflow waits must retain protected storage
and origins; never restore them and persist plaintext in ordinary JSON. Update
queue consumers that currently assume the public projection is executable input.

## Storage and consumer consistency

Capture the effective channel policy with the immutable executable/output binding
at admission, include it in integrity digests, and copy it for retries and pinned
workflow children. Never resolve policy from a newly edited action when rendering
or completing an existing execution.

Define a canonical output representation with three explicitly distinct parts:

- A public structured result projection with markers at secret value paths.
- Protected structured-value slots, each bound to its concrete result path,
  encrypted value, stored contract, and recorded origins.
- Channel projections and separate protected original-channel references. A
  sanitized public stdout string and its private original are separate objects;
  the original must not masquerade as an encrypted replacement at `/stdout`.
  Reveal accesses the original channel through its own typed reference and
  authorization, without silently replacing the sanitized public projection.

Use one typed storage/consumer contract for this representation. Change tables,
repository methods, result manifests, digests, API DTOs, and artifact/channel
storage as needed. Reuse existing facilities where they fit the new model;
retaining the old storage shape is not an acceptance criterion. Original secret
channel content requires protected storage and must not leak through storage
transport or duplicate ordinary artifact bodies.

Replace native result archive and snapshot validators that assume every protected
object is a marker replacement in public JSON. Enforce exact marker/slot matching
for structured secret fields, and independently validate protected channel
references and their public projections. Native consumers must use this contract
rather than reconstructing output sensitivity from inputs or from the old result
shape. Worker-to-workflow and worker-to-queue consumption are required end-to-end
acceptance cases, including sanitized stdout and explicit secret channels.

Ordinary API, CLI, MCP, WebSocket, history, and web views must consume the same
public projection. Raw streams, runtime-log artifacts, agent transport, shared
volume storage, and console mirroring must obey channel policy too. For sensitive
structured streams, buffer until safe records can be classified or use the
existing delayed mirror; never emit raw secret bytes before redaction. Keep
private runtime-log ACLs distinct from the public sanitized projection.

Review input-triggered decrypt requirements in runtime-log authorization so they
do not make public classified output inaccessible. Protected originals retain
existing execution visibility, decrypt, source-origin, and freshness checks.

The web reveal control must keep public siblings visible and request plaintext
only on demand. Fields that remain redacted need a non-sensitive explanation
that distinguishes unavailable stored material from denied disclosure, without
exposing source identities or values. Reproduce the reported admin HTTP-200
reveal failure independently; do not assume selective classification fixes it.

### Explicit CLI and MCP disclosure

The CLI exposes `--include-secret-values` (`--decrypt`) on execution `show` and
`result`, opting into the existing authorized API disclosure path. Keep original
channels under `revealed_output_channels`, separate from sanitized public output.
MCP inline `executions_get` remains redacted. MCP `executions_export` requires an
agent-specified output filepath and writes the complete authorized execution
object only to a new private file. Return only absolute path, byte count, and
status metadata to the caller. No plaintext MCP content, structured payload,
resource attachment, preview, log, or error reflection is permitted. Treat this
as a local-write tool under existing access policies, with no new decrypt grants
or approval mechanism. Validate the path before disclosure where possible and
create no file when API authorization fails.

### Development data and rollout

Target fresh installations of the revised canonical v1 model. There is no
requirement to read old result manifests, infer policy for missing bindings, or
run old and new producers concurrently. Do not introduce historical-policy
fallbacks, parallel representations, or compatibility readers.

An existing development deployment needs an explicit coordinated reset, or a
separately scoped conversion if retaining its data is desired. Plan the reset
boundary to include affected runtime rows, source snapshots, raw logs/artifact
bodies, and admission bindings. Stop or drain old services and queued/running work
before switching all producers and consumers together; retries cannot cross the
boundary with an uninterpretable old binding. This plan does not authorize
performing a reset, terminating work, or deploying changes.

Never make old protected logs or results public by interpreting missing metadata
as the new public default. Do not automatically reset databases, rewrite recorded
checksums, or convert historical data during startup. Follow
[PostgreSQL-only deployment](../deployment/postgresql-only.md) for the explicit
pre-production reset policy. A data-preserving conversion is separate work and
must not add runtime backward compatibility to this implementation.

## Implementation sequence

1. **Define the canonical model and bind the contract.** Specify typed public
   projections, protected structured slots, separate original-channel references,
   and native consumption. Add policy parsing, defaults, publication validation,
   storage/repository changes, immutable snapshots, and digest coverage. Update
   action/workflow authoring, DTO/OpenAPI contracts, and generated clients together.
   Document the coordinated development reset boundary before rollout.
2. **Build one output classifier.** Reuse the value-schema compiler for structured
   paths, minimal malformed-subtree protection, channel projections, and concrete
   provenance assignment. Keep it independent of storage and services.
3. **Integrate worker and agent completion.** Replace blanket input/read/delegation
   classification across success, failure, timeout, cancellation, and preparation
   failures. Apply channel policy before logs or other sinks publish content.
4. **Replace consumer assumptions and integrate sinks.** Update native result
   archives/snapshots, workflow output maps, queue callbacks and copied queue
   state, logs/artifacts, mirroring, notifications, history, and external views.
   Retain release pins and independent retention semantics. Delete superseded
   validators and blanket-classification paths rather than adapting around them.
5. **Complete disclosure diagnostics and authoring UX.** Show schema annotations,
   channel choices/defaults, partial reveal states, and safe explanations. Add pack
   author documentation and examples; update the secrecy policy documentation and
   relevant AGENTS.md invariant if the new contract requires it.

## Acceptance and validation

Required behavioral coverage includes:

- Secret input plus entirely public output: stdout and structured fields remain
  visible; the input remains encrypted and redacted.
- Public and secret siblings, nested arrays, inherited named types, JSONL, and
  workflow output maps: only declared secret paths are protected.
- Selected API refs without reads, actual Key/runtime reads, and delegated
  executions: public output stays public; secret fields retain correct origins.
- Each explicit channel policy, structured stdout duplication, leading debug
  lines, malformed/truncated output, unknown fields, and traversal limits.
- Failure/timeout/cancellation paths and diagnostic text; no unclassified bytes
  leak through live logs, console mirrors, artifacts, history, or notifications.
- Public projections require ordinary read access. Secret reveal succeeds for an
  authorized admin with retained valid sources, remains denied for insufficient
  rights, and fails closed for missing origins. Report partial disclosure safely.
- Action edits do not alter admitted bindings; retries preserve policy within
  the new model. Coordinated rollout cannot consume old bindings accidentally or
  expose retained old log bodies through a missing-policy default.
- Native workflow/queue consumers successfully process the canonical output,
  including secret acknowledgement containers and nested summaries/errors, while
  public copies retain redactions. Secret outputs with no secret inputs have
  valid local origins and remain revealable to authorized users.
- Existing executable/schema authoring authority controls classification changes;
  ordinary execution callers cannot override the stored contract. Source access
  remains required before protected input delivery.

Use pure compiler/classifier tests plus real worker/API/repository and full-stack
boundary tests on owned disposable infrastructure. Exercise both worker and
injected agent transports. Before merge readiness, run applicable Rust formatting,
checks/Clippy and scoped suites, generated API regeneration/checks, web tests,
format/lint/typecheck, and SQLx preparation if persistence changes. Follow
[Running Tests](../testing/running-tests.md) for infrastructure and CI requirements.

## Related contracts

- [Named type schema compiler](named-type-schema-compiler.md)
- [Delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md)
- [Execution API](../api/api-executions.md)
