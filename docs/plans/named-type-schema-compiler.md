# Named type schema compiler contract

## Scope and status

The first implementation slice lives in `crates/common/src/value_schema/`.
It is a pure compiler and contract consumer, not a deployed type registry.
Existing rule-parameter, queue-payload, inquiry-response, and inquiry-option
validators now use it. Pack checking, archive checking, and queue definition
checking use the same compiler. Manual execution and enforcement creation now
materialize and persist immutable action contracts, apply input defaults, and
validate and redact under those contracts. Scheduling no longer reapplies
current defaults to bound action inputs. Workflow context reconstruction
validates restored parameters under the stored input contract. Workflow children
capture static task contracts at parent admission and inherit them at dispatch;
dynamic action selectors bind after rendering. Queue dispatch and completion
callbacks attach child bindings transactionally. Workers validate restored input
and protect parsed output, raw diagnostics, and runtime-log mirroring under the
stored contracts. Output conformance produces sanitized warnings, not failed
execution status. JSONL validates each record separately.
Workflow output maps retain their existing top-level result shape and use the
workflow's stored output contract. Malformed sensitive workflow output is
encrypted as a whole with its warning. Retries copy the previous attempt's
contracts and encrypted input rows atomically without adding retry metadata to
the flat parameter map.
Pack installation recognizes `types/*.yaml` and `types/*.yml` as managed definitions.

The next slice has storage foundations in `schema_type.rs`,
`repositories/schema_type.rs`, and migration `20261012000002_named_schema_types.sql`.
They provide pack-owned projections and immutable effective release views.
Registration checks the effective candidate type graph, external active release
dependencies, version constraints, visibility, defaults, and secret annotations
before projection mutation. Release activation captures the complete effective
type view. Reverse edges prevent removal or policy restriction of types used by
another active pack. Local checking reports unresolved external references as
unverified, rather than inventing schema bodies.

Authenticated schema discovery and rendering previews are available through the
API, CLI, and MCP. No additional schema-read grants are required. Keys support
single-value schema assignment and explicit rebinding, with complete-value
validation before encryption. Ordinary Key updates retain their binding. Queue
items retain immutable admission contracts through payload updates and dispatch,
with encrypted schema-secret payload fields and recorded queue origins.
Inquiries now retain immutable response contracts, apply defaults before strict
option/response acceptance, encrypt secret option and response values, and retain
their origins. Public requests can select an `option_ref` without disclosing its
value. Plaintext inquiry detail requires explicit decrypt rights plus visibility
and recorded-origin access. Workflow inquiry waits restore and validate values
under their original contract and mark template sensitivity before child creation.
Keyed admission fingerprints keep idempotent creation on the original binding
without exposing a plain hash of secret options.

API event and webhook ingress now bind payloads from trigger `out_schema`, and
sensor event configuration from `event_config_schema`. Webhook metadata has a
separate server-owned contract. Original event identity travels with derived
values; decrypting an execution does not bypass original event read/decrypt checks.
Event contracts and encrypted values survive native DEFAULT repair and are
cleaned by exact repository deletion or bounded owner-expiry checks. Event displays
read bounded stored graphs without fetching newer definitions.

Sensor workloads now capture generation-scoped configuration and trigger
contracts before launch. Configuration secrets are stored separately from
immutable executable metadata. Sensor events use the admitted generation's
contracts, and cache grants are captured in the deployed release rather than
rechecked against the installer at runtime. All sensor SDKs and the core timer
read the same bounded stdin `config`/`triggers` bootstrap.

Internal event producers now use transactional contracts. Configuration revisions
encrypt pack/sensor secrets and retain dependency-scoped assignments. Deployed
rule inputs use captured authority independent of the installer. Sensor log
versions and live deliveries retain original provenance; streams recheck
authorization before each frame. Indexed secret-origin edges protect retained
sources during bounded reclamation. Shared forms include bounded object/array
and single-value controls.

Mutable consumer writes now compile within their transaction and retain reference
edges. Registration validates against its candidate view; ordinary writes retain
declaring-pack dependency and visibility policy. Runtime admission still binds
released definitions independently of these authoring checks. Repository and
service-level validation of the newly enabled paths is unfinished. The sensitive
sensor-startup gate has been reproduced in the baseline; its removal still needs
the complete service-level regression to pass.

`repositories/schema_contract.rs` provides transactional contract materialization
for selected immutable sources. It keeps same-pack bodies on the declaring
release, selects active external releases, checks current policy and recorded
dependency constraints, and rejects conflicting release selections. Callers
must supply schema bodies from their selected snapshots. Input/output pairs are
stored in `runtime_schema_binding` with digest verification and an immutable
update guard. Attachment verifies the owner's executable snapshot. Bindings
use plain owner IDs without runtime foreign keys; owner deletion removes its
own binding, while an inherited execution binding survives enforcement deletion.
Static task executable snapshots retained in bindings protect external release
content before children exist. Release reclamation takes exclusive schema graph
admission before checking those pins; pack deletion rejects nonterminal work
that still needs such snapshots.

Manual creation and rule enforcement creation attach their bindings in the same
transaction as parameters and encrypted secret rows. Enforcement handoff copies
the binding without re-resolution and rejects missing bindings or executable
snapshots. Contract secret traversal uses concrete array paths and preserves all
recorded descendant origins when an enclosing object is encrypted.

This document specifies the initial common-module contract for
[named type schemas](named-type-schemas.md). Runtime persistence and repository
selection policies remain work for subsequent slices. No pack format version or
compatibility adapter is introduced. The remaining service-level validation is
incomplete; schema discovery does not grant secret disclosure or runtime authority.

## Schema contexts

Callers explicitly select `FlatDocument` or `SingleValue`.

- `FlatDocument` accepts an object mapping field names to schema objects.
  Boolean `required` describes the outer field occurrence and is lifted into
  the generated object's required list. An array `required` constrains the
  children of an object value and is preserved. To require both an object field
  and its children, use a named object type or a local ref to an inline object
  definition, with `required: true` at the flat occurrence. There is no new
  second requiredness keyword in this slice.
- `SingleValue` accepts an ordinary JSON Schema object or boolean schema.
  `required` has JSON Schema array semantics, never occurrence semantics.
- The compiler does not guess context from `type` or `properties` keys, tolerate
  a raw JSON Schema in a flat document, skip malformed fields, or substitute an
  unconstrained schema on error.
- Flat-document extra fields remain allowed. A named type's object constraint
  such as `additionalProperties: false` applies to that value, not the flat map.

The pack authoring syntax remains `types/<name>.yaml` with `ref`, `label`, and a
single-value `schema` body. Registration parses these files and rejects duplicate
definitions, invalid ownership, symlinks, and source-budget violations.

## Reference grammar and selection

`type: pack.type_name` uses the existing two-part component-ref grammar.
Short names are rejected. Primitive names are `string`, `number`, `integer`,
`boolean`, `object`, `array`, and `null`. Arrays of primitive type names use
ordinary JSON Schema union semantics; arrays of named refs are not supported.

The compiler receives a fixed `RegistryView`. Available entries contain an
immutable source identity, revision ID, and authored schema body. Explicit
inaccessible and retired entries fail closed, as do missing entries and invalid
bodies. An entry's source component ref must match its registry key, and its
source must be a `Released` identity, never an `Authoring` preview. A separate
`Candidate` registry entry allows pre-deployment bodies only when the root source
is `Authoring`; its manifest has no fabricated revision ID. A `Released` root
rejects candidate entries.

The repository adapter is responsible for selecting and authorizing this view:

- Select the executable release first. Same-pack types come from that selected
  release, never from newer mutable projections.
- External types should come from active releases satisfying the dependencies
  recorded by the source release and the applicable reference-visibility policy.
- Check transitive reference edges against their declaring sources too.
- Reject incompatible requirements or conflicting revision selections instead
  of silently replacing an entry in the view.
- Schema access does not grant Key read/decrypt or execution authority.

The compiler performs no database operations, network requests, file reads,
version selection, or authorization. Passing a registry map is not proof that
the repository has fulfilled these obligations. These rules must be tested in
repository integration before named types are enabled through APIs.

Local `$ref` supports `#` and plain JSON Pointer fragments such as
`#/$defs/address`. Escaped `~0` and `~1` segments are supported. URI percent
escaping, anchors, external URLs, filesystem refs, `$id`, and dynamic references
are rejected. Local targets must be indexed schema-bearing positions, not
defaults, examples, enum values, or arbitrary data.

Each named type retains its own authored local-reference scope. In a flat
document, local pointers address the authored field map, for example
`#/destination/$defs/country`. `#` cannot refer to the flat map as a value schema.

## Supported vocabulary

The validator dialect is JSON Schema draft 2020-12. An authored `$schema`, if
present, must identify that dialect exactly.

Supported schema-bearing positions are:

- `properties` and `$defs` maps.
- `items`, `additionalProperties`, `not`, and `contains` schema nodes.
- `allOf`, `anyOf`, `oneOf`, and `prefixItems` arrays.

Supported assertions are `type`, `enum`, `const`, `required`, `minimum`,
`maximum`, `exclusiveMinimum`, `exclusiveMaximum`, `multipleOf`, `minLength`,
`maxLength`, `pattern`, `minItems`, `maxItems`, `uniqueItems`, `minContains`,
`maxContains`, `minProperties`, and `maxProperties`.

`format` is retained as a string annotation for forms and displays. It is not
asserted by the validator, matching the existing draft 2020-12 validation policy.
Reference occurrences cannot override it. Adding format assertion is a separate
enforcement decision.

Unknown keywords are rejected. In particular, `patternProperties`,
conditional schemas, dependent schemas, and unevaluated keywords are not yet
supported. This is deliberate: a keyword must not gain validator-only support
while defaults or secret traversal ignore its schema children. Expanding the
vocabulary requires coordinated compiler and consumer tests.

## Annotations and overrides

Annotations are `title`, `description`, `default`, `examples`, `position`, and
`secret`. The annotated graph retains them; the validator schema strips them
only in schema-bearing positions. Data containing similarly named keys is
unchanged.

Named and local reference occurrences may override these annotations only.
Structural siblings are rejected, including attempts to replace `properties`,
`items`, enums, or bounds. Local ref nodes may contain `$defs` declarations;
those declarations establish local targets and do not override the target.
Boolean flat occurrence requiredness is handled before reference overrides.

Use-site annotations take precedence over inherited annotations. `secret` is
monotonic: true marking is inherited through aliases, and explicit false cannot
remove an inherited root marking. Structural overrides cannot remove nested
secret marking either.

Every effective default must validate without further default insertion.
Defaults and examples containing values classified as secret are rejected,
including an inherited default made secret by use-site marking. Examples are
checked for secret content but are not otherwise required to conform.

Secret values are strings or nested arrays/objects whose leaves are strings.
Number, integer, boolean, and null types are rejected beneath secret markers,
including unions and resolved named/local references. `const`, `enum`, `default`,
and `examples` are prohibited beneath a secret marker, even for public domain
values or empty examples. Enclosing public literals cannot contain secret
descendants either. Unconstrained children and composition-only schemas do not
bypass the text-tree restriction: strict and authoring validation check the
concrete secret tree. Redaction still classifies malformed values so output
protection can fail closed without exposing them.

## Shared graphs and diagnostics

The compiler emits self-contained annotated and validator bundles with shared
`$defs` and local `$ref`s. It does not copy referenced trees into every use site.
Source-location metadata maps generated node IDs back to authored paths.

Direct and indirect cycles are rejected, including optional properties, arrays,
composition, local references, and unused definitions. Lexical containment in
`$defs` is not a dependency edge. An unused definition pointing at a root that
does not reference it is not recursive.

Diagnostics contain stable error codes, escaped JSON Pointer paths, source refs
where available, and cycle chains. They never format validator display strings
or attach submitted values. Callers must not add plaintext to logs or audit
details while converting these errors.

Default limits are 64 levels, 10,000 schema nodes, 1 MiB of combined authored
source JSON, 4 MiB of serialized contract JSON including both bundles and
metadata, 100,000 data/traversal
nodes, 4 MiB of value JSON, and 256 effective default/example/const/enum checks or auxiliary
authoring validation checks. Limits are
explicit inputs and become part of the contract. Limits cover arbitrary data in
annotations as well as schema nodes, transitive sources, and memoized dependency
depth. Runtime traversal also has a work budget, including a conservative
graph/value preflight before strict validation. A small scalar cannot bypass the
budget through exponential composition evaluation. Default insertion is bounded
before cloning repeated default values.

## Immutable contract

A contract records the compiler identifier `attune.value-schema/1`, source
identity, schema context, authored digest, annotated and validator bundles,
node locations, sorted dependency manifest, limits, and binding digest.

Digests use SHA-256 over JCS canonical JSON. The binding digest covers the entire
contract with an empty `binding_digest` field. Dependency digests cover their
authored schema bodies. Preparing a contract checks its digest and compiler
identifier. A digest detects corruption; it is not an authorization signature.

`SourceIdentity` distinguishes selected immutable `Released` sources from
current-definition `Authoring` checks and previews. `compile_inline` supplies
the latter explicitly, without inventing a release ID or claiming a runtime
pin. It uses an empty named-type registry and therefore fails closed on named
refs until repository-backed selection is implemented. Such previews must not
be persisted as authoritative runtime bindings.

The authored document remains separate. Neither bundle should replace authored
named refs in an editor. A serialized contract can validate values without any
registry lookup. Action input/output bindings now use self-contained JSONB in
`runtime_schema_binding`; dependent type changes cannot rewrite an existing row.
Record creation takes shared type-graph admission before rule/owner locks.
Registration takes pack-first exclusive admission, so selection does not reverse
the registration pack/rule lock order.

## Defaults, validation, and secrets

`CompiledContract` prepares the validator once and provides:

- `validate`: strict validation of fully rendered values. Templates are strings,
  not fabricated placeholder values. Defaults are not inserted by validation.
- `validate_authoring`: checks known values and returns sorted deferred template
  paths. Whole-value templates are unknown values, not sample objects or scalars.
  Literal siblings, required fields, fixed object/array shape, array length,
  known duplicates, and incompatible literal enum/const fragments still fail.
  Composition uses conservative possible/certain bounds. In particular, a
  relaxed `oneOf` cannot reject templates merely because branches now overlap,
  and `not` cannot negate a relaxed possible match as if it were certain.
  `contains` counts certain and possible matches separately. Projection work,
  size, and auxiliary validator construction are bounded.
- `apply_defaults`: returns a new value and fills missing object properties.
  Explicit null, false, zero, and empty strings remain unchanged. Existing
  object/array children are visited. Optional absent object trees remain absent
  unless their occurrence has an explicit object default. Array elements are
  never appended to satisfy defaults.
- `secret_paths`: concrete pointers for values present in objects and arrays.
  All supported composition branches are unioned conservatively, even branches
  that do not validate. `contains` marking applies conservatively to every array
  element. Secret parent paths suppress redundant child paths.

Defaults inside composition branches, `not`, and `contains` are rejected,
including referenced defaults. An explicit default at the enclosing node is
allowed if it validates. This avoids selecting a branch arbitrarily or applying
defaults under a negative constraint. Defaults in properties, items, and
`prefixItems` are supported.

These helpers do not encrypt values, rewrite secret origins, or grant disclosure.
Their callers must preserve `docs/permissions/delegation-and-secret-disclosure.md`.
Authoring acceptance is not proof that any joint assignment of symbolic values
will satisfy every constraint. It is a conservative literal check, not a general
satisfiability solver. Fully rendered values still require strict validation at
the delivery/acceptance boundary.

## Existing consumer changes

The API no longer substitutes template placeholders or tolerates malformed/raw
JSON Schemas in flat documents. Invalid declared definitions fail closed; value
errors report sanitized field paths rather than validator display strings.
Schema checks accept supported union types and composition-only nodes instead
of requiring a string `type` at every flat occurrence.

Rule action/trigger parameters use authoring mode. Queue admission/payload
updates and inquiry responses use strict mode at their existing validation
boundaries. Template-looking strings in those submitted values no longer bypass
type, enum, or pattern constraints. This is an intentional correction to the
previous shared template-aware helper. Literal strings containing braces can
still pass a string schema if they satisfy its ordinary constraints.

Stored inquiry options use strict validation through the same compiler. Queue
definitions now validate their schema bodies, not just the outer object shape.
Pack and archive checks share schema definition validation for authored fields
and the metadata field names already checked by the archive builder. Workflow
schema fields are checked too. These checks do not change which fields the
component loader reads or convert authored refs into expanded schemas.

No new execution-create, event-ingestion, output, Key, or pack-config value
enforcement is enabled by this slice. Existing consumers still use current inline
definitions, not persisted runtime bindings. Scheduling and workflow rendering
still need stored-contract strict validation before delivery.

## Next integration gates

1. Add type registration, immutable revisions, graph preflight, reverse
   dependencies, reference policy, retirement, and retention tests.
2. Materialize contracts and encrypted values atomically after source selection.
   Verify concurrent updates and historical records before enabling named types.
3. Route scheduling/defaults, rendered input validation, redaction, and output
   handling through those stored bindings. Define output validation targets
   before adding output conformance checks.

Workflow child selection, sensor-trigger relationships, queue payload updates,
output validation targets, and configuration revalidation still need their
record-specific policies before persistence changes.

### Confirmed policy: enforcement handoff and manual creation

`crates/executor/src/event_processor.rs` renders action parameters, derives
secret handling, and stores encrypted values on an enforcement before an
execution exists. `enforcement_processor.rs` subsequently copies those values
and the selected executable snapshot into an execution. Enforcements are a
schema-bearing persistence boundary missing from the proposal's record table.

Confirmed policy: materialize action input/output
contracts at enforcement creation, after executable selection and before
rendering/secret persistence. The resulting execution inherits that binding.
Direct/manual executions select the latest eligible contract on the server at
their own creation. There is no client-facing contract, type revision, binding,
or executable-release selector. Same-pack types remain aligned with selected
executable code, and the latest external release must satisfy its dependency
constraints; an incompatible latest source fails rather than falling back to
an older schema. The manual request DTO rejects unknown selection fields.
