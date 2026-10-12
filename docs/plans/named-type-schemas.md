# Named type schemas

## Status

Proposed; not implemented. Based on a static review of the repository on
2026-10-12. This plan introduces a common schema facility and does not claim that
all existing declared schemas are currently enforced.

## Goals

- Let packs define and deploy reusable value schemas with namespaced refs such
  as `example.address` and `example.severity`.
- Allow those types wherever a schema node can declare a primitive, object,
  array, or other supported JSON Schema shape.
- Use the same resolved contract for validation, defaults, secret handling,
  dynamic forms, and schema-derived displays.
- Preserve Attune's flat per-field `param_schema`, `out_schema`, and
  `conf_schema` documents and flat execution parameters.
- Prevent specification drift within a runtime record by materializing its
  schema contract when the record is created.
- Reject recursive schema references in the initial implementation.

## Current implementation

| Area | Relevant files | Current behavior / implication |
|---|---|---|
| Schema storage | `crates/common/src/models.rs` | Schema values are generally untyped JSON. There is no named-type registry. |
| API parameter validation | `crates/api/src/validation/params.rs` | Converts flat schemas to JSON Schema and substitutes placeholders for templates. |
| Inquiry options | `crates/common/src/inquiry_options.rs` | Contains a separate flat-schema converter and validates stored response options. |
| Pack checking | `crates/common/src/pack_check.rs`, `crates/common/src/pack_format/source.rs` | Checks schema shape and string type declarations, but does not resolve named types or validate a type dependency graph. |
| Component loading | `crates/common/src/pack_registry/loader.rs` | Transactional loading with ownership preflight and component reconciliation. Pack configuration metadata also has handling outside this loader. |
| Component lifecycle | `crates/common/src/repositories/component_lifecycle.rs`, `crates/common/src/platform_catalog.rs` | Tracks managed projections, release ownership, and retirement. Types must join this lifecycle. |
| Immutable releases | `crates/common/src/repositories/pack_release.rs`, `crates/common/src/repositories/executable_snapshot.rs` | Captures executable action, runtime, workflow, and sensor metadata. Runtime schema binding must respect the selected executable release. |
| Defaults | `crates/executor/src/scheduler.rs` | Applies top-level parameter defaults; nested semantics are not centralized. |
| Secret handling | `crates/common/src/secret_values.rs` | Traverses schema properties directly, but schema secret-path collection does not traverse array items or composition branches. |
| Dynamic forms | `web/src/components/common/ParamSchemaForm.tsx` | Supports primitives and string enums; objects and arrays currently render as JSON textareas. Client validation is a limited implementation, not full JSON Schema validation. |
| Schema authoring | `web/src/components/common/SchemaBuilder.tsx` | Rebuilds schemas from a limited set of fields, potentially losing unsupported structure during edits. |
| Keys | `crates/common/src/models.rs`, `crates/api/src/dto/key.rs`, `crates/api/src/routes/keys.rs`, `web/src/pages/keys/KeyCreateModal.tsx`, `web/src/pages/keys/KeyEditModal.tsx` | Keys accept JSON values but have no value-schema binding. Their forms use independent format controls. |

Validation coverage is uneven. Rule parameters, queue items, and inquiry
options have validation paths. The execution-create and pack-config update
paths inspected do not perform equivalent schema conformance checks. Action
output schemas participate in secret redaction, but the worker success path
inspected does not enforce output conformance.

Reference support and broader enforcement are separate deliverables. A named
schema must behave like its equivalent inline schema; using a name must not
accidentally introduce a different validation policy.

## Authoring contract

### Pack definitions

Add `types/*.yaml` as a pack component directory:

```yaml
# types/severity.yaml
ref: example.severity
label: Severity
schema:
  type: string
  enum: [info, warning, critical]
```

```yaml
# types/address.yaml
ref: example.address
label: Address
schema:
  type: object
  properties:
    street:
      type: string
    city:
      type: string
    country:
      type: string
      minLength: 2
      maxLength: 2
  required: [street, city, country]
  additionalProperties: false
```

A named type describes one value, not an outer flat field map. Its body uses
JSON Schema node semantics plus supported Attune annotations. Existing
`param_schema`, `out_schema`, and `conf_schema` fields remain flat maps.

### References

Use a namespaced reference in `type` as shorthand:

```yaml
parameters:
  destination:
    type: example.address
    required: true
  severity:
    type: example.severity
    default: info
  alternate_destinations:
    type: array
    items:
      type: example.address
```

The compiler must distinguish occurrence-level `required: true` in a flat field
map from `required: [...]` inside an object value schema. This cannot be handled
by indiscriminately merging JSON objects.

Initial reference rules:

1. Use fully qualified `pack.type_name` refs; do not add contextual short-name
   lookup initially.
2. Resolve references only in schema-bearing positions, including properties,
   items, supported composition branches, and local definitions. Do not traverse
   defaults, examples, or arbitrary data as schemas.
3. Missing, inaccessible, invalid, or retired references fail closed. Never
   substitute an unconstrained schema.
4. Do not fetch arbitrary HTTP or filesystem references during compilation or
   validation. Supported local JSON Schema references must remain local.
5. Initially allow use-site annotations such as description, default, position,
   occurrence requiredness, and additional secret marking. Reject structural
   overrides rather than inventing ambiguous merge semantics.
6. A use site cannot remove inherited secret marking. Annotation inheritance and
   precedence must be explicit, and defaults must satisfy the resolved schema.
7. Cross-pack references require declared pack dependencies and an allowed
   reference policy. Schema access does not grant Key access or execution rights.

Internally, ordinary JSON Schema `$defs` and references may represent shared
subschemas. The author-facing shorthand need not be a custom URI.

## Shared schema architecture

Introduce a dedicated common schema module rather than growing the existing
database-oriented `crates/common/src/schema.rs`.

The common layer should support:

- Flat schema documents and single-value schemas as distinct contexts.
- Definition validation and schema-aware reference discovery.
- Resolution against an explicit registry/release view supplied through the
  repository layer.
- Cycle detection, depth/size limits, and deterministic compilation.
- An annotated resolved contract for forms, defaults, and secret handling.
- A standards-compliant JSON Schema for the validator.
- A dependency manifest recording refs, immutable revision identities, and
  content digests.
- Shared default application and validation utilities.
- Sanitized errors containing field locations without disclosing secret values.

Attune annotations must remain available to runtime consumers even when stripped
from the validator schema. Defaults are an explicit Attune operation; JSON
Schema validation itself does not populate them.

Keep authored references separate from resolved contracts. UI editing must not
replace named references with expanded definitions merely because it fetched a
rendering schema.

## Binding time and drift prevention

### Decision: materialize runtime contracts at record creation

Resolve and snapshot the applicable schemas when ephemeral runtime records are
created, rather than requiring every schema consumer to be permanently bound
to external type revisions at pack deployment.

Deployment still validates the candidate schema graph, references, ownership,
and compatibility with applicable configuration. It preserves authored refs
and immutable type revision bodies. Runtime creation makes the authoritative
selection of the schema contract for that record.

This allows newly created records to use newly available external type
definitions while existing records retain the contract under which they were
accepted. It avoids runtime lookup against mutable registry state.

### Source selection

Runtime binding must not undo immutable executable-release selection:

- For an execution, choose the executable snapshot first. Resolve same-pack
  types from that selected release, not a newer release containing different
  code and type definitions.
- Resolve external types from an explicit permitted revision view at runtime
  creation. The default can be the active external release at that boundary;
  dependency/version constraints, if supported by the existing dependency
  contract, must still apply.
- For an event, choose and record the applicable trigger schema revision at
  ingestion. When ingestion is tied to a pinned sensor release, define that
  source relationship explicitly rather than mixing incompatible revisions.
- Reject a source definition that no longer satisfies the declared dependency
  or schema contract instead of silently selecting another definition.

These rules should be finalized before persistence changes. Binding at runtime
prevents intra-record drift, but does not by itself prove that a newly changed
external contract is compatible with older action code.

### Persistence and atomicity

Store an immutable binding containing:

- The source component/release identity.
- The authored schema or an unambiguous link to its immutable source.
- The resolved annotated schema or a self-contained schema bundle.
- Referenced type revision identities and digests.
- A deterministic binding digest and compiler contract identifier.

Select revisions, validate supplied values, derive secret handling, and persist
the record and its binding under one consistent transaction/view. Concurrent
type updates must not produce a mixed binding. Publish runtime notifications
only through the existing successful record-creation flow.

Use repositories for every database operation. Whether the bundle is embedded
in a runtime record or stored as a deduplicated immutable object is an
implementation decision; it must remain available for the record's lifetime.
Avoid a mandatory live lookup into mutable component rows.

### Record-specific boundaries

| Record | Binding boundary | Subsequent behavior |
|---|---|---|
| Execution | Creation after executable selection, before parameter persistence/redaction | Defaults, rendered input validation, output validation, secret handling, and displays use the binding throughout the execution. |
| Workflow execution | Root workflow execution creation, before input/default processing | Workflow input/output schemas remain fixed across scheduling, retries, waits, and resume. Reuse the root execution binding where possible rather than creating competing bindings. |
| Workflow child execution | Child creation under the parent workflow's applicable pinned contract | For statically identified tasks, capture or pin their schema sources with the workflow contract. Dynamically selected actions need an explicit child-creation binding boundary and must retain that binding afterward. |
| Event | Ingestion before validation, redaction, and insert | Events remain immutable. Rule processing and historical display must not reinterpret an event using a newer trigger schema. |
| Inquiry | Creation before validating stored response options | Responses and option selection use the original response-schema binding. |
| Queue item | Admission, and later payload updates under an explicit queue policy | Record the admission contract. Define whether existing pending items retain it or undergo an explicit rebind; do not silently reinterpret them after queue schema updates. |

### Updates do not imply rebinding

Re-evaluate updated values against the stored binding. Ordinary execution status
changes, output persistence, workflow checkpoints, retries, and resume must not
resolve the latest named types again.

For example, changing an execution from `running` to `completed` must not cause
its result to be checked against a type introduced after the execution started.

If an API permits changing a schema-bearing record's contract, make rebinding
explicit and authorized. Validate the complete effective value, persist the new
binding atomically, and record its digest transition without payload secrets.
Do not rebind in-flight executions or immutable events. A new execution may
select a new contract; a retry within an existing execution retains its contract.

### Persistent definitions and values

Keys and pack configuration are not ephemeral runtime records. Their policy
needs separate handling:

- Add an optional single-value `value_schema` to Keys, supporting inline and
  named types through the same compiler. Bind on assignment or explicit schema
  change; ordinary value updates validate against the existing binding unless
  an explicit rebind is requested.
- Validate Key plaintext before encryption. Schema changes without replacement
  values must respect existing read/decrypt authorization when inspecting stored
  material. Schemas do not grant disclosure authority or change encryption rules.
- Bind pack configuration to its effective configuration schema and validate
  writes. Revalidate preserved config when deployment changes that schema; define
  the treatment of newly required fields explicitly rather than silently
  invalidating installed configuration.
- Forms for a pending submission may use a current preview binding. Backend
  acceptance is authoritative. A preview digest can detect contract changes
  between form loading and submission and require refresh instead of surprising
  the user.

## Recursion and resource limits

Reject all recursive references initially, including optional recursion.
Detect direct and indirect cycles through named types and supported local
references, including cycles hidden under composition and array items.

Diagnostics should identify the chain and source location:

```text
example.person -> example.organization -> example.person
at schema.properties.employer
```

Optional recursion is not safe merely because the form is lazy: compilation,
defaults, schema display, and secret traversal may still eagerly follow it.
Supporting it later requires coordinated bounded graph-aware behavior in every
consumer and should be a separate proposal.

Even acyclic graphs can expand exponentially. Limit dependency depth, source
and compiled sizes, and rendered nodes. Preserve shared definitions internally;
do not blindly copy complete trees at every reference.

Optional objects should remain absent until provided, defaulted according to
the explicit default policy, or added by the user. Do not instantiate an entire
object tree merely to render a form.

## Validation, templates, and defaults

Separate two modes:

1. Authoring-time validation checks literals while deferring symbolic template
   values. Whole-object templates must not fail because a fabricated empty
   placeholder lacks required children.
2. Runtime validation checks fully rendered values against the stored binding
   before delivery or acceptance.

The existing placeholder approach is insufficient for constrained named objects,
patterns, and complex branches. Refactor it rather than adding more sample-value
generation heuristics.

Centralize defaults and define behavior for missing values, explicit `null`,
empty strings, `false`, and `0`. Nested defaults must not create absent optional
object trees automatically unless that behavior is explicitly part of the
contract. Referenced defaults and use-site defaults must be validated.

Define validation targets before enforcing action outputs: parsed action payloads
are distinct from Attune's execution-result envelope. Define JSONL record versus
aggregate validation and workflow `output_map` validation explicitly.

## Secret handling

Schema resolution must happen before secret-path derivation and before any
runtime persistence or output mirror that could expose plaintext.

Extend secret handling to objects, array elements, and supported composition
branches. For ambiguous branches, conservative redaction is preferable to
under-redaction. Arrays require value-aware paths or an explicit path-pattern
contract; simply collecting static object-property pointers is insufficient.

The runtime binding governs secret classification for that record. Later type
updates cannot weaken historical classification or replace recorded secret
origins. Preserve the authority rules in
`docs/permissions/delegation-and-secret-disclosure.md`.

Never include secret submitted values in validator errors, audit details,
definition defaults, examples, or schema previews. Type reuse must not expand
Key read/decrypt rights or execution-token permission scope.

## Dynamic forms and authoring

Upgrade the shared form machinery, not each page independently:

- Named-type selection, inspection, and dependency diagnostics in schema editors.
- Lossless round-tripping of references, nested schemas, and unsupported keywords.
- Server-resolved annotated schemas for all schema-derived forms and displays.
- Structured object properties and arrays of primitives/objects.
- Optional object add/remove controls and bounded/lazy rendering.
- Nested defaults and field errors using consistent paths.
- Secret-aware controls and existing disclosure restrictions.
- Template-aware editing for both whole values and nested fields.
- Key creation/editing using the shared single-value form renderer.

Unsupported JSON Schema constructs can retain a raw editor with an explicit
explanation. Never silently remove their constraints or render unresolved named
types as unconstrained strings. Form/client validation is advisory; backend
validation remains authoritative.

## Coverage boundary

Initial coverage must include:

- Action input/output schemas and execution forms.
- Workflow input/output schemas, task action forms, and output maps.
- Trigger schemas and event ingestion/processing.
- Sensor parameter/configuration schemas.
- Pack configuration schemas and values.
- Queue item schemas and dispatch parameter forms.
- Inquiry response schemas and stored response options.
- Key value schemas.
- Schema-derived displays and curated result views.

Not every structure named `schema` is JSON Schema today. For example,
`crates/api/src/dashboard_data/contracts.rs` defines a dashboard source parameter
contract as required/optional name lists, with custom route validation. Named
types there require a separate contract refactor, not merely a resolver call.

Do not automatically replace native Rust platform configuration types, the pack
archive format's schema-version marker, database schema identifiers, or add
schemas to currently schema-less storage. Track any additional genuine
user-authored value-schema surfaces discovered during implementation.

## Implementation sequence

### Phase 1: contract and common compiler

- Finalize the node/document distinction, reference grammar, annotation rules,
  supported JSON Schema vocabulary, and runtime binding policy.
- Implement deterministic resolution, dependency manifests, cycle/resource
  checks, annotated contracts, and JSON Schema compilation.
- Centralize defaults and validation helpers.
- Replace duplicated flat converters and align pack checking/format checking.
- Cover canonical metadata and authoring field names consistently across entry
  points; do not introduce a second format version or compatibility adapter.

Gate: named and equivalent inline schemas have identical behavior, with
deterministic errors for missing refs, cycles, and unsupported overrides.

### Phase 2: registered type component

- Add models, repositories, migrations, and immutable type revision bodies.
- Integrate `types/*.yaml` into ownership preflight, component loading,
  projections, release stamping, retirement, and removal policy.
- Validate the complete candidate graph before mutating consuming components,
  including pack configuration outside the component loader.
- Add reverse-dependency inspection and safe removal rules. Preserve revisions
  needed by live runtime bindings even after definitions retire.
- Provide authenticated list/get/inspect API and CLI/MCP discovery. Initially
  keep definition mutations pack-managed.

Gate: malformed type definitions roll back registration, and type lifecycle
cannot invalidate an already materialized runtime binding.

### Phase 3: runtime binding and persistence

- Select schema sources consistently with executable and sensor release pins.
- Materialize execution/workflow/event/inquiry contracts at their creation
  boundaries using repository-owned transactional operations.
- Define workflow child and queue-item update boundaries explicitly.
- Use stored bindings on updates, retries, resume, output persistence, and
  historical display; never resolve current registry state in those paths.
- Add explicit rebinding only to appropriate mutable schema-bearing records.
- Ensure binding availability and cleanup follow runtime retention. Event
  storage must remain immutable and retain existing partition/ID semantics.
- If mutable execution columns are added, update history-trigger change checks
  and use digest summaries for large JSONB bindings.

Gate: concurrent type updates cannot produce mixed contracts, and new records
may select newer external types without changing existing records.

### Phase 4: backend consumers and security

- Route validation, defaults, redaction, secret destinations, and output handling
  through the common layer and applicable runtime bindings.
- Add Key schema storage and validated encrypted/plaintext write paths.
- Validate pack configuration writes and preserved config on schema changes.
- Close validation gaps deliberately, including rendered execution inputs,
  trigger payload contracts, and action/workflow output validation.
- Define failure behavior for each boundary. Do not change conformance policy
  accidentally based on whether a type was named or inline.

Gate: nested and array-contained secrets cannot leak through parameters,
results, runtime logs, events, validator errors, or API responses.

### Phase 5: shared UI integration

- Add named-type inspection/selection and lossless schema editing.
- Separate authored schemas from resolved rendering contracts.
- Add structured object/array rendering and optional-field controls.
- Integrate Key forms and every existing dynamic-form/display caller.
- Surface preview/acceptance binding mismatches clearly.

Gate: editing preserves refs and constraints; all form callers use the same
resolved semantics with bounded rendering and safe secret controls.

### Phase 6: verification and documentation

- Add the tests below using repository-owned disposable fixtures and the
  guidance in `docs/testing/running-tests.md`.
- Regenerate API clients, including web and other affected generated clients.
- Document type authoring, dependencies, override restrictions, runtime binding,
  updates, deletion, forms, and recursion policy.
- Update `AGENTS.md` with concise invariants/pointers if the new concept changes
  project guardrails.
- Run targeted tests, `cargo sqlx prepare` after database changes, and
  `cargo check --all-targets --workspace` with no introduced warnings.

## Acceptance tests

- Inline/named equivalence for enums, objects, arrays, nested types, and defaults.
- Missing, inaccessible, retired, duplicate, and invalid references.
- Direct, indirect, optional, array-contained, composition-contained, and
  supported local-reference cycles.
- Large acyclic graphs, depth/size limits, and shared-definition reuse.
- Registration rollback, ownership checks, type retirement, and dependencies.
- Concurrent binding/type updates and preview/submission mismatches.
- Executable-release alignment when same-pack types change with action code.
- New runtime records selecting updated external types while old records keep
  their bindings, including queued executions and long-running workflows.
- No rebinding on status updates, completion, retry, checkpoint, or resume.
- Immutable events retaining ingestion schemas after trigger/type updates.
- Workflow child, dynamic-action, inquiry, and queue payload-update boundaries.
- Literal versus template validation and strict validation after rendering.
- Defaults for absent optional objects, missing fields, `null`, `false`, and `0`.
- Sanitized validation errors and secret handling across objects, arrays,
  composition branches, results, mirrors, and logs.
- Key create/update/rebind, encryption changes, and read/decrypt authorization.
- Pack configuration preservation and newly required fields on deployment.
- Lossless UI round-tripping and every existing schema-derived form/display.

## Decisions to finalize before implementation

1. Confirm `type: pack.type_name` and `types/*.yaml` as canonical syntax.
2. Confirm no recursion and annotation-only use-site overrides initially.
3. Finalize the external revision-selection policy at runtime creation and any
   dependency/version constraint behavior without bypassing executable pins.
4. Choose binding persistence/deduplication and lifetime management.
5. Finalize workflow child/dynamic-action and pending queue-item policies.
6. Define enforcement failure behavior and output validation targets.
7. Define configuration migration/revalidation behavior for schema changes.

The recommended initial scope is pack-managed named types, a shared compiler,
runtime-record materialization, secret-safe bindings, and structured object/array
forms. Recursive types and sophisticated structural overrides are separate
follow-up proposals.
