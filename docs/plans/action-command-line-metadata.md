# Action command-line metadata specification

## Status and scope

Status: integrated in PR #83; validation and review are tracked in the PR.

The shared `crates/common/src/action_cli/` module validates CLI annotations,
retains them in immutable schema contracts, derives definition-only previews,
and materializes bounded argv after authorized input restoration. Action
repositories, registration, archive checks, API DTOs, CLI/MCP and web authoring
preserve the metadata. Schema binding uses admitted named-type revisions;
defaults remain an admission concern and are not reapplied by the worker.

The worker and injected agent share native execution with a cleared child
environment, pinned release working directory, explicit stdin policy, and
existing output/cancellation/log handling. Supporting Linux workers advertise
`action_cli_arguments_v1`. Executable path, digest, native artifact and variant
are recorded separately in `execution_program_selection`, keyed by the existing
worker attempt; no argv or input values are stored there. The table is removed
with its execution and does not change execution history or authority.

Installed tools resolve from the selected startup baseline PATH, excluding
empty/relative entries, then are canonicalized and hashed. Packaged binaries
are selected from the pinned release manifest and checked for consumer,
platform, executable intent, confinement and content digest. Both paths are
verified again immediately before launching. Deployments must protect tool
files/directories and release materialization against concurrent replacement;
these checks do not remove that operating-system boundary.

Storage uses the forward migration
`migrations/20261118000001_action_command_launch.sql`. Applied migration bytes
and stored snapshots are preserved; this feature requires no database reset.
All typed native/command sources use the worker capability gate, including
argument-free native artifacts, because older workers lack this source handler.

The current scalar/flag/array domain checks conservatively require an explicit
resolved `type` at the selected schema node (and scalar item nodes when expanding
an array). Domain inference from composition-only, enum-only, or const-only
schemas is not implemented; such bindings are rejected rather than guessed.

The words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are normative.

An action may invoke an installed program or a packaged native executable with
fixed arguments and non-secret parameter values. Parameter definitions and their
argument bindings have one authoring source: the action input schema. Fixed
tokens are defined separately because they do not describe execution inputs.
Execution requests continue supplying the ordinary flat parameter map.

The initial implementation covers action launches of type `command` and
`native`. It does not add sensor arguments, shell command interpolation, a
generic caller-supplied argument list, or new workflow invocation syntax.
Argument bindings on file, Java, intrinsic, or workflow launches MUST be rejected
in this implementation. Existing JVM argument rules remain unchanged.

All format contracts remain v1. This is an in-place extension of the canonical
pre-production contract, not a v2 format or a compatibility adapter.

## Canonical example

```yaml
ref: tools.generate_report
label: Generate report
runner_type: native

launch:
  type: command
  program: report-tool
  arguments:
    - literal: generate
      order: 0
    - literal: "--"
      order: 90

parameter_delivery: none
output_format: text

parameters:
  format:
    type: string
    enum: [json, csv]
    default: json
    cli:
      kind: option
      name: "--format"
      order: 10
  verbose:
    type: boolean
    default: false
    cli:
      kind: flag
      name: "--verbose"
      order: 20
  destination:
    type: string
    required: true
    cli:
      kind: positional
      order: 100
```

For `{"destination":"report folder/output.txt","verbose":true}`, the result is:

```json
{
  "program": "report-tool",
  "argv": [
    "generate", "--format", "json", "--verbose", "--",
    "report folder/output.txt"
  ]
}
```

Here and throughout this document, `argv` means application arguments, excluding
the OS-level `argv[0]`. `report-tool` is an illustrative external program, not an
Attune-provided executable. Defaults are applied by normal execution admission
before argument materialization. A caller may supply `format: csv` instead.

## Action metadata fields

| Field | Contract |
| --- | --- |
| `launch` | Typed executable source. This specification adds `command` and extends action `native` launches with fixed arguments. |
| `launch.arguments` | Ordered-by-key list of fixed literal tokens; defaults to `[]`. It MUST NOT reference parameter values. |
| `parameters.<field>.cli` | Optional action-specific argument binding on a field schema. API metadata uses `param_schema` for the same flat document. |
| `parameter_delivery` | Existing delivery setting, extended with `none`. It controls the input document independently of CLI bindings. |
| `parameter_format` | Existing serialization setting. CLI-enabled invocations use JSON when delivering an input document. |

A CLI-enabled action is a `command` launch, or a `native` launch with at least
one literal argument, at least one CLI binding, or `parameter_delivery: none`.
An empty/default `arguments` list by itself does not opt an existing native
action into new delivery restrictions. The new `none` value is supported only
on these two typed launch forms in this implementation.

`parameters` and `param_schema` remain aliases for one schema field under the
existing authoring rules; declaring both MUST be rejected. The schema is Attune's
flat per-field format, not a raw JSON Schema document with a root `properties`.
Nested object definitions retain their normal `properties` syntax.

There is no action-level dynamic `arguments` list. Execution requests MUST NOT
override `launch`, `cli`, ordering, option names, or fixed tokens. Data properties
whose names happen to be `cli`, `launch`, or `arguments` do not become metadata.

## Executable source

### Installed program

```yaml
launch:
  type: command
  program: report-tool
  arguments: []
```

A `command` launch contains exactly these fields:

| Field | Type | Required/default |
| --- | --- | --- |
| `type` | Literal `command` | Required |
| `program` | Nonempty string without NUL | Required |
| `arguments` | Array of literal entries | Default `[]` |

`program` MUST be either a bare executable name without path separators or an
absolute Linux path. Relative paths such as `./tool` and `bin/tool` MUST be
rejected. A bare name MUST resolve using the worker's selected baseline `PATH`,
excluding empty and relative search entries; caller-controlled environment
values and the working directory MUST NOT participate in that search. Resolution
MUST produce an absolute executable regular-file target before spawning. Preserve
the static invocation spelling as `argv[0]` for installed multicall tools; this
is derived from `program` and is never an execution input. Budgets cover the
longer of that spelling and the canonical executable path.
The worker MUST NOT fall back to a shell, download a missing program, or use
another executable when resolution fails.

Installed programs are dependencies of the worker or injected agent container.
The operator MUST provide them in the intended workers and use placement
constraints as needed. Search directories and executable files MUST be protected
against modification by untrusted action processes. The deployment must provide
that protection; resolving an absolute path alone does not prevent replacement.
The resolved path SHOULD be recorded as safe attempt metadata.

A command launch MUST NOT be used to execute files inside the materialized pack
release tree, including through symlinks. Packaged executables use `native`, so
release confinement and verification cannot be bypassed by this new variant.

### Packaged native executable

```yaml
launch:
  type: native
  artifact: report_tool
  arguments:
    - literal: generate
      order: 0
```

This preserves the existing logical artifact reference and native variant
selection, adding only optional `arguments`. Artifact existence, platform
selection, file integrity, and protected materialization retain their existing
requirements. The artifact path is not supplied as an execution parameter.

### Source and runtime consistency

An action MUST select exactly one execution source: `launch`, `entry_point`
(`entrypoint` in API metadata), or `workflow_file`/its API workflow representation.
No empty entry-point sentinel is required for a typed launch. Both new launch
forms require the native runtime, resolved through the existing normalized
runtime-name and trusted runtime-reference rules. They MUST NOT be routed through
an interpreter-backed runtime as a fallback.

This specification explicitly extends the self-contained release guarantee in
[Pack release artifacts](../packs/pack-release-artifacts-spec.md#component-launch-model):
`command` declares a worker-provided dependency, while `native` remains a verified
pack artifact. A release pins the command declaration, not installed program
bytes. Replaying or retrying a command action does not guarantee identical
program bytes unless the deployment also pins its worker image or installation.

## Fixed literal arguments

Each `launch.arguments` entry MUST contain exactly:

```yaml
literal: "--format"
order: 10
```

`literal` MUST be a string without NUL; the empty string is valid and emits an
empty argument. `order` is a nonnegative integer no greater than `2147483647`.
Unknown fields and parameter references in this structure MUST be rejected.

Literals are emitted exactly as written. There is no templating, environment
expansion, wildcard expansion, quote removal, or splitting on whitespace.
For example, the literal `$HOME` remains the five-character string `$HOME`.

Subcommands, fixed flags, fixed option values, and a program-supported `--`
separator all use this structure. A fixed option/value pair uses two entries:

```yaml
arguments:
  - literal: "--format"
    order: 10
  - literal: json
    order: 11
```

These values are immutable action metadata. An overridable default instead uses
the parameter's existing `default` plus a `cli` binding. Literal arguments and
schema defaults MUST NOT be used to store credentials or other secret values.

## Parameter CLI bindings

`cli` MUST be one object, not a list of references to separately defined
parameters. One schema occurrence has at most one binding. Its value is selected
implicitly from that occurrence's location in the action input document.

### Binding fields

| Field | Type/default | Allowed kinds | Meaning |
| --- | --- | --- | --- |
| `kind` | Required: `positional`, `option`, or `flag` | All | Emission behavior. |
| `order` | Required nonnegative integer, at most `2147483647` | All | Position of the binding's entire emitted group. |
| `name` | Required nonempty literal string without NUL | `option`, `flag` | Fixed option or flag token. |
| `false_name` | Optional nonempty literal string without NUL | `flag` | Token emitted for an explicitly false value. |
| `style` | `separate` (default), `equals`, or `attached` | `option` | How the name and value are combined. |
| `encoding` | `scalar` (default) or `json` | `positional`, `option` | Serialization of each selected value. |
| `expand` | Boolean, default `false` | `positional`, `option` | Expand an array into consecutive values or repeated options. |
| `missing` | `omit` (default) or `error` | `option`, `flag` | Behavior when the field is absent after defaults. |

Fields not listed for a kind, unknown fields, null annotations, and unknown enum
values MUST be rejected. `name` and `false_name` are single tokens, not command
fragments. They are literals and are not rendered. Different programs have
different keyword syntax, so names need not start with `-`. Separator tokens
`-` and `--` MUST be expressed as literal arguments instead of option names.

### Positional

```yaml
source:
  type: string
  required: true
  cli: {kind: positional, order: 100}
```

Emit the selected value, without a flag or keyword. A missing positional value
MUST fail materialization even if its schema field is optional. Authors must
require the field or supply a valid default to make it reliably available.
Implicitly skipping optional positional values would shift later operands.
Optional positional omission is not part of this initial contract.

### Option

For `name: --format` and value `json`:

| Style | Emitted argv elements |
| --- | --- |
| `separate` | `["--format", "json"]` |
| `equals` | `["--format=json"]` |
| `attached` | `["--formatjson"]` |

An attached short option can use `name: -I`, producing `-I/path/to/include`.
An absent optional value emits neither name nor value when `missing: omit`.
`missing: error` rejects absence even when the schema permits it.

### Boolean flag

```yaml
color:
  type: boolean
  default: true
  cli:
    kind: flag
    name: "--color"
    false_name: "--no-color"
    order: 20
```

The effective field schema MUST guarantee a boolean value whenever present.
True emits `name`; false emits `false_name` when configured, otherwise nothing.
Missing remains distinct from false: absence follows `missing` and never emits
`false_name`. String values such as `"false"` MUST NOT be coerced to booleans.

### Arrays and repetition

```yaml
include:
  type: array
  items: {type: string}
  maxItems: 20
  cli:
    kind: option
    name: "--include"
    order: 30
    expand: true
```

For `["a", "b"]`, emit `["--include", "a", "--include", "b"]`. Array order is
preserved. `equals` and `attached` apply independently to each element.
An expanded positional array emits each element as a separate operand.

`expand: true` requires an array-compatible effective schema; an empty array
emits no elements. If at least one operand is required, authors MUST express that
through `minItems`. Expansion MUST NOT flatten nested arrays. Each item must be
serializable using the selected encoding. `expand: false` with `encoding: json`
instead emits the entire array as one JSON argument.

### Value conversion

| Value | `scalar` encoding |
| --- | --- |
| String | Unchanged UTF-8 string, including spaces and empty strings. |
| Integer/number | JSON numeric representation, without locale formatting. |
| Boolean used as a value | Literal `true` or `false`; flag bindings have their own behavior. |
| Object/array | Reject; select `json` or explicit array expansion. |
| Explicit null | Reject; it is not absence or the empty string. |

`json` emits compact JSON for the selected non-null value, including JSON quotes
when the selected value is a string. Object keys MUST be serialized in a stable
recursive order. Array order is preserved. Null array elements require rejection
when expanded; nulls inside a non-null JSON-encoded container follow the ordinary
schema contract. No conversion recursively evaluates supplied strings.

Registration MUST reject bindings whose resolved schema cannot guarantee a
supported value domain. Nullable schemas may be used, but a selected null fails
materialization. Runtime validation and conversion MUST fail closed if stored
metadata or restored values violate these rules.

## Nested fields and dot-based paths

Bindings may appear on top-level field schemas and nested `properties`
occurrences in the action-authored schema:

```yaml
parameters:
  paths:
    type: object
    properties:
      source:
        type: string
        cli: {kind: positional, order: 100}
      destination:
        type: string
        cli: {kind: positional, order: 110}
    required: [source, destination]
```

These bindings select `paths.source` and `paths.destination`. There is no
redundant `parameter` or `path` member inside `cli`. Missing ancestor objects make
their descendant fields absent; normal schema validation and binding missing
rules then apply.

Author-facing binding identities and previews MUST use dot/bracket notation:

- `paths.source` for ordinary names.
- `metadata["file.name"]` for a key containing a literal dot.
- `["top.level"]` for a top-level key containing a literal dot.
- `files[0].path` for a concrete array element in value-level diagnostics.

An unquoted segment matches `[A-Za-z_][A-Za-z0-9_]*`. Other property names use
brackets containing a JSON double-quoted string with normal JSON escapes. Array
indices use nonnegative decimal integers without leading zeroes except `0`.
Wildcards, function calls, expressions, and negative indices are unsupported.
Paths MUST be represented as typed segments internally rather than split naively
on dots. Existing secret/provenance records may retain escaped JSON Pointers;
their persisted format is not changed by author-facing notation.

CLI annotations inside `items`, `prefixItems`, `additionalProperties`, `$defs`,
or composition/conditional schema branches MUST be rejected. Bind array values
at the array occurrence with explicit expansion instead. This keeps binding
identity and global ordering statically known; the array-index diagnostic syntax
does not authorize per-element dynamic bindings.

## Global ordering

All literal entries and all explicit parameter bindings share one order space.
Orders MUST be unique across the entire action, including nested bindings and
bindings that might emit nothing. Duplicate order values are definition errors,
not ties resolved by YAML order or field name. Gaps are allowed.

The materializer MUST sort groups by ascending `order`. A literal is a one-token
group. An option/value pair or expanded array is one indivisible group; another
binding cannot interleave with its emitted elements. Omitted flags/options and
empty expansions emit no elements but do not change remaining group order.

The worker MUST NOT automatically insert or move `--`: not all programs support
the same option grammar. Authors place a literal separator where appropriate.
Neither ordinary schema `position` annotations nor object insertion order affect
argv ordering. Repeated option names at different orders are allowed; their
meaning depends on the target program.

## Input document delivery

CLI bindings are independent of the existing input-document delivery setting:

| Setting | Behavior for CLI-enabled actions |
| --- | --- |
| `parameter_delivery: none` | No input document; child stdin is closed/null immediately, never inherited from the worker. |
| `parameter_delivery: stdin` | Deliver the complete validated flat input map as JSON, including public values also used in argv. |

For a `command` launch, omitted `parameter_delivery` defaults to `none`. Native
launches retain the existing default of `stdin`. These defaults MUST be resolved
and persisted at action creation/registration, not reinterpreted differently by
API clients, repositories, and workers. Explicit delivery is recommended in docs.

For a CLI-enabled action, `parameter_format` MUST be omitted or `json`.
Omission canonicalizes
to `json`, including with `none`. `file` delivery is not supported for these new
CLI-enabled invocations. Existing non-CLI actions retain their current delivery
and format behavior.

Parameters without `cli` never become command-line arguments. With `stdin`, they
remain part of the input document. With `none`, unbound public parameters are
accepted but not delivered; authoring tools SHOULD warn about such unused fields.
A concrete secret/protected input with `none` MUST fail before launch, even if it
has no CLI binding. Optional secret fields that are absent do not prevent launch.
This initial feature does not add credential files or automatic stdin adaptation
for arbitrary third-party programs.

## Schema compilation and immutable admission

`cli` is action-input authoring metadata, not a value assertion. Incorporation
into [the shared value-schema compiler](named-type-schema-compiler.md) MUST:

1. Recognize and validate `cli` only at permitted action input occurrences.
   Unknown metadata still fails; it is not silently dropped.
2. Retain the authored annotation in immutable schema/executable metadata and
   exclude it from the generated validator schema in schema-bearing positions.
   Object data with a property named `cli` is unaffected.
3. Allow an action-authored named/local reference occurrence to attach `cli`
   without allowing structural overrides of the referenced type.
4. Reject `cli` in reusable type definitions and all non-action-input contexts.
   Bindings MUST NOT be inherited merely because an alias references another
   occurrence that carries a binding. Extraction follows explicit action-authored
   occurrences, not every node reachable in the resolved reference graph.
5. Resolve types, requiredness, defaults, and all secret annotations through the
   existing graph-aware compiler. A standalone annotation parser is not proof
   that the selected value is public or serializable.

The existing annotation `position` remains presentation metadata. No new
top-level raw JSON Schema dialect is introduced.

Action admission MUST capture the launch, literals, CLI annotations, bound input
contract, and applicable executable identity in one coherent immutable snapshot.
Changes to any executable metadata MUST participate in the relevant snapshot,
release, and approval digests. A compiled argument plan may be stored as derived
metadata, but it MUST be checked against those captured sources and MUST NOT
become a second editable definition.

Manual execution, enforcements, workflows, queue dispatch, callback children, and
retries MUST use the same admission/materialization contracts. Workers MUST NOT
re-read mutable action schemas to interpret pending executions. Retries retain
the original launch and parameter contract; normal revocation and source-delivery
checks still apply. Invalid or unavailable bindings fail closed.

## Secret handling and execution authority

For every binding, registration MUST reject a schema that can classify the
selected value as secret, considering inherited ancestors, resolved named types,
and all conservatively applicable composition branches. Serializing an object or
array also requires all selected descendants to be public. A flag's emission or
an option's presence must not disclose a protected value through control flow.

Immediately before spawning, the worker MUST check the restored input's concrete
secret paths and protected provenance against every selected argument subtree.
An overlapping ancestor, selected path, or descendant MUST reject the invocation.
Checks include expanded items and the `none` delivery restriction. Redacted or
unavailable inputs MUST NOT be substituted into argv. Admission checks alone are
insufficient, and adding `secret: false` cannot downgrade inherited protection.

Secret delivery and restoration remain governed by
[Delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md).
Secret values are never placed in argv or parameter-derived environment variables.
There is no `allow_secret`, forced conversion, or redaction override for a CLI
binding. Complete argv and restored values MUST NOT be logged, audited, or echoed
in rejection messages. Publication metadata itself is not a secret store.

Execution API access remains opt-in through `permission_set_refs`. This feature
does not grant new scopes, bypass owner/attempt fencing, or change `standard`.
Output classification continues to follow the admitted output schema/channel
policy, independently of input sensitivity and CLI delivery.

## Process construction, limits, and security responsibilities

The worker MUST launch using an executable plus an argv vector, not a command
string. It MUST preserve empty strings, Unicode, whitespace, and argument group
boundaries. NUL in any program, literal, name, or emitted value is rejected.
No shell escaping step is required or permitted as a substitute for argv APIs.

Use the existing process executor for timeout, cancellation, process-tree
termination, output parsing, log custody, and exit reporting. Apply
`ChildProcessEnvironment` before explicit context, as required by
[Child-process environments](../deployment/child-process-environments.md).
The working directory is the protected pinned pack release root; the program's
installation directory does not become the working directory. Missing release
materialization is an error, not a fallback to the service working directory.

Initial argument budgets are:

| Budget | Ceiling |
| --- | --- |
| Literal entries plus CLI binding groups per definition | 1,024 |
| Emitted argv elements, excluding `argv[0]` | 1,024 |
| UTF-8 bytes in one emitted argument, excluding its terminator | 16,384 |
| Total program/argv UTF-8 bytes, including one NUL terminator per element and program path | 65,536 |

Operators MAY lower these ceilings. Schema/value traversal also retains the
shared compiler's existing budgets. The materializer MUST enforce cumulative
budgets during expansion, before constructing an unbounded argv allocation.
The OS may impose a lower effective limit including environment bytes; a spawn
failure such as `E2BIG` MUST become a safe execution failure without values.

Direct argv construction prevents shell splitting, not target-program behavior.
Authors MUST constrain inputs that select executable code, config files, plugins,
filesystem destinations, remote hosts, or argument/response files. For example,
curl's `--data` interprets an `@filename` value as a file read; it is not made safe
by being one argument. See [curl's option contract](https://curl.se/docs/manpage.html#--data)
and [OWASP argument injection guidance](https://cheatsheetseries.owasp.org/cheatsheets/OS_Command_Injection_Defense_Cheat_Sheet.html).

Action publication and CLI-binding edits require existing executable-definition
authorization. Generic caller-controlled `extra_args` expansion MUST NOT be
provided. Authors can deliberately publish powerful actions, including
interpreters, but executable-plus-argv is not a sandbox or a proof that inputs
are treated as data. OS privileges, restricted mounts, protected tool/config
locations, worker credentials, and network egress remain deployment concerns.
Do not claim the current environment protections provide per-execution isolation.

## API, storage, and tooling integration

Pack YAML and API requests use the same launch object. API create/update/detail
DTOs expose `launch`; the input schema remains `param_schema`, containing `cli`.
The API also accepts/returns the resolved `parameter_delivery` and
`parameter_format`. This is a metadata change, not a new execute endpoint.

An API create payload may therefore contain:

```json
{
  "ref": "tools.generate_report",
  "pack_ref": "tools",
  "label": "Generate report",
  "runtime_ref": "core.native",
  "launch": {
    "type": "command",
    "program": "report-tool",
    "arguments": [{"literal": "generate", "order": 0}]
  },
  "parameter_delivery": "none",
  "parameter_format": "json",
  "param_schema": {
    "format": {
      "type": "string",
      "enum": ["json", "csv"],
      "default": "json",
      "cli": {"kind": "option", "name": "--format", "order": 10}
    }
  }
}
```

An ordinary execution still supplies `{"format":"csv"}` as its flat config.
It does not submit an alternate launch, argument list, or schema annotation.

Action update requests MUST validate the complete resulting action atomically.
Omitted fields remain unchanged. Setting `launch` replaces that object rather
than merging its internal fields; explicit `null` clears it. Schema updates keep
the existing whole-schema replacement semantics. Switching execution source
requires clearing the old source in the same request; clearing a source without
a valid replacement MUST fail. Delivery defaults apply on creation, not on
patches: an existing incompatible setting must be changed explicitly. API
source fields MUST distinguish omission, explicit null, and a replacement value.

Add a nullable typed `launch` JSONB action projection through the repository
layer, retaining `cli` in `param_schema`. Update action column lists, write paths,
constraints, DTOs, pack loaders/checkers, archive validation, snapshots, and
generated clients. Entry-point nullability/validation must support the exclusive
source contract without empty strings. Existing workflow storage remains the
two-file representation. Any added mutable execution metadata requires the
corresponding history-trigger treatment.

The web authoring UI and CLI/MCP MUST preserve these fields losslessly and expose
safe binding previews. Preview order is derived, not separately edited. Forms
continue collecting ordinary schema-defined inputs; they MUST NOT offer a raw
argv override. Previews cannot establish runtime release bindings or bypass
secret checks. Tool versions that cannot preserve these fields MUST reject
unsupported edits rather than silently erase them.

## Runtime materialization sequence

1. Load and verify the admitted immutable executable and input contract.
2. Restore inputs through the existing authorized secret/provenance path and
   validate the resulting flat map. Do not reapply current action defaults.
3. Verify that launch metadata and the derived binding plan agree with the
   captured contract. Resolve the installed program or verified native artifact.
4. Evaluate missing-value and concrete secret/provenance rules. For `none`, also
   reject any protected input that would lack a secure delivery route.
5. Sort literal/binding groups, convert values, and expand arrays within budgets.
   Reject invalid values without echoing them. Do not spawn on partial success.
6. Build the process with its argv vector, cleared environment, fixed working
   directory, and explicit stdin policy; run through the shared process executor.

Static compilation and concrete materialization SHOULD live behind one shared
command-planning module. Its derived results MUST NOT be duplicated across API,
pack validation, executor, and worker implementations with differing semantics.

## Errors and diagnostics

Definition errors are rejected before publication or ordinary API create/update
success. Runtime errors fail the execution before spawn whenever detectable.
Diagnostics contain a stable code and safe metadata location, never argument
values, serialized input, credentials, or a raw validator error containing data.
Author-facing binding identities use dot/bracket notation; existing internal
schema diagnostic pointers may remain unchanged.

| Code | Condition |
| --- | --- |
| `action.launch.invalid` | Invalid source shape, source conflict, runtime mismatch, or unsupported launch kind. |
| `action.cli.invalid_binding` | Unknown/illegal binding fields, placement, conversion, or kind. |
| `action.cli.duplicate_order` | Two groups declare the same order. |
| `action.cli.missing_value` | Required binding input is absent after admitted defaults. |
| `action.cli.invalid_value` | Null, NUL, incompatible concrete type, or failed conversion. |
| `action.cli.secret_value` | Schema secrecy or concrete protected provenance overlaps an argv value. |
| `action.cli.no_secure_delivery` | Protected concrete input exists with `none` delivery. |
| `action.cli.limit_exceeded` | Definition, expansion, or emitted-byte budget is exceeded. |
| `action.launch.program_unavailable` | Installed program cannot be resolved/executed or the native artifact cannot be verified. |

Existing authorization, schema validation, and process-result codes remain
applicable; they need not be recoded when they fail before argument planning.

## Incorporation and rollout requirements

The integration extends typed archive launches through action registration and
worker execution. It requires end-to-end launch plumbing, including the schema
annotation and every immutable admission path.

Implementation MUST update:

1. Shared action/launch metadata types and the action-aware schema compiler.
2. Pack checking, release/archive building and validation, registration/loading,
   and executable/approval capture, using one validation contract.
3. Action repository projections, creation/patch rules, migrations and SQLx
   metadata, without service-layer ad hoc database queries.
4. API DTOs/OpenAPI, CLI/MCP serialization, web generated clients and authoring.
5. All execution producers, immutable snapshots, worker/agent context preparation,
   native command planning, and explicit stdin handling.
6. Documentation: parameter delivery, action metadata/API, the launch contract in
   the pack-release specification, named-type annotation rules, and AGENTS.md
   guardrail 10 to explicitly permit declared public argv bindings.

Workers/agents MUST advertise `action_cli_arguments_v1` only when they implement
the complete launch, validation, secret-checking, and materialization contract.
Scheduling MUST require that capability for command launches and native actions
using CLI bindings, literal arguments, or `none` delivery. Unsupported workers
must not receive these executions. Missing program dependencies still produce
readiness/placement or safe execution failures; native support alone is not proof
that an arbitrary installed tool exists.

The implementation MUST document its fresh-install/migration choice under
[PostgreSQL policy](../deployment/postgresql-only.md), including any explicitly
required development reset. No database, applied checksum, stored snapshot, or
recorded authority may be rewritten automatically. Existing formats stay v1;
coordinated readiness is required instead of a dual reader or lossy fallback.

## Acceptance and validation

Documentation-only acceptance for this proposal is link, syntax/example, and
consistency review plus `git diff --check`. It does not establish implementation
readiness. Before declaring an implementation ready to merge, follow
[Running tests](../testing/running-tests.md) and cover:

- Flat-schema/CLI parsing, unknown fields, annotation contexts, named/local
  references, independent presentation ordering, and duplicate global orders.
- Every binding kind/style, defaults versus fixed literals, false and absent
  flags, nested missing fields, expanded/JSON arrays, null, empty strings,
  numeric formatting, Unicode, quoting, and stable object serialization.
- Exact argv preservation for values containing spaces, shell syntax, leading
  dashes, literal dots, and brackets; no shell invocation or recursive evaluation.
- Secret parents/children, named types/composition, protected propagated values,
  absent secret fields, no-secure-delivery rejection, and safe diagnostics.
- Definition and incremental expansion limits, NUL rejection, OS spawn failure,
  controlled program resolution, unavailable tools, and release path confinement.
- Pack/API round trips and source-switch patches, immutable snapshots after
  action edits, retries, workflows, rules/enforcements, queues, and callback children.
- A small owned test executable reporting exact argv and stdin, exercised through
  both worker and agent paths, with timeout/cancellation/log/output handling.
- Worker capability rejection and deployment-provided tool dependencies; existing
  stdin-native actions and non-CLI launch behavior must retain their contracts.
- Repository/migration checks on owned PostgreSQL, `cargo sqlx prepare`, generated
  API client generation/checking, and affected UI/CLI/MCP contract tests. Run Rust
  formatting/checks/Clippy appropriate to the shared changes, with zero new warnings.

## Related contracts

- [Action API](../api/api-actions.md)
- [Action parameter delivery](../QUICKREF-action-parameters.md)
- [Named-type schema compiler](named-type-schema-compiler.md)
- [Pack release artifacts and launches](../packs/pack-release-artifacts-spec.md)
- [Child-process environments](../deployment/child-process-environments.md)
- [Delegation and secret disclosure](../permissions/delegation-and-secret-disclosure.md)
- [Action output secrecy](../action-output-secrecy.md)
