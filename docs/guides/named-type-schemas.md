# Named type schemas

Named types describe one JSON value. A pack declares them in `types/*.yaml`:

```yaml
ref: example.address
label: Address
reference_visibility: public
schema:
  type: object
  required: [city]
  properties:
    city:
      type: string
    country:
      type: string
      default: "NO"
  additionalProperties: false
```

Use a named type with `type: example.address`. Component parameter, output, and
configuration documents remain flat field maps. Key schemas describe the entire
value directly and do not wrap it in a parameter document.

Schema documents are non-sensitive metadata. Any authenticated client can inspect
installed definitions, including private types. Reference visibility controls
pack-to-pack use, not discovery or Key assignment. Do not put credentials in
descriptions, defaults, examples, enums, or other schema literals. Compilation
rejects `const`, `enum`, defaults, and examples on secret nodes or below secret
ancestors, including literals inherited through references. Secret values must
be strings or nested lists/objects with string leaves, not numbers, booleans, or
null. Enclosing public literals cannot embed secret descendants either. Schema
inspection grants no access to Key values or executions.

## Discover and preview

```bash
attune schema-type list --pack example --json
attune schema-type list --query address --page-size 20 --json
attune schema-type show example.address --json
attune schema-type inspect example.address --json
printf '%s' '{"type":"example.address"}' |
  attune schema-type preview --context single-value --file - --json
```

`show` returns the authored schema. `inspect` returns a self-contained rendering
graph with its selected revisions. Keep authored references in editors instead
of replacing them with resolved bodies. A preview's semantic digest compares
the accepted schema with the preview; it never selects a historical revision.

The HTTP endpoints are:

- `GET /api/v1/schema-types`, with `q`, `page`, and `page_size`.
- `GET /api/v1/packs/{pack_ref}/schema-types`.
- `GET /api/v1/schema-types/{ref}`.
- `GET /api/v1/schema-types/{ref}/inspect`.
- `POST /api/v1/schema-types/preview`, with `context` and `schema`.

Contexts are `single_value` and `flat_document`. MCP exposes the same operations
as `schema_types_list`, `schema_types_get`, `schema_types_inspect`, and
`schema_types_preview`.

## Assign a Key schema

```bash
attune key create --local-ref address --name Address \
  --value '{"city":"Oslo"}' \
  --value-schema '{"type":"example.address"}' --encrypt
attune key update system.address --value '{"city":"Bergen"}'
attune key update system.address --rebind-schema
attune key update system.address --clear-schema
```

Any Key owner may use an installed type. The server binds the assignment,
fills missing public defaults, and validates the plaintext value before
encryption. Ordinary value updates retain the assigned contract even after
the type changes. A schema change or `--rebind-schema` selects current revisions
and revalidates the full value atomically. Revalidating preserved material
requires Key read and applicable decrypt authority. Replacement values remain
subject to the usual update authorization.

Schema-secret values require encryption and the text-tree restriction above.
Whole-Key encryption can still protect other JSON types when they are not
classified as secret by a schema. Key read and decrypt permissions remain
unchanged. The web Key editor supports authored schemas, installed type search,
contract previews, and explicit rebinding.

## Runtime contracts

Pack and sensor configuration writes retain immutable schema/value revisions.
Secret fields stay encrypted; metadata responses and current rows contain
redaction markers. Pack schema migrations apply missing defaults to preserved
values and roll back deployment if those values no longer conform.

Explicit detail reads use `include_secret_values=true` on the pack or sensor
detail endpoint. Pack plaintext requires `packs:read` and `packs:decrypt`.
Sensor plaintext requires its usual `triggers:update` visibility plus
`triggers:decrypt`. Recorded origins can impose additional restrictions.
Schema changes over preserved secret configuration require the same decrypt
authority. Complete replacement values require write authority, not permission
to disclose the previous values. Omit `config` to preserve the encrypted revision;
do not submit redaction markers as replacement values.

Pack-installed rules use a system-owned runtime identity. Their input grants
are separate from execution API access:

```yaml
ref: example.process
trigger_ref: example.received
action_ref: example.process
input_permission_set_refs: [example.input_reads]
permission_set_refs: []
action_params:
  token: "{{ keystore['system.api_token'] }}"
```

Deployment validates and captures only Key/Pack read/decrypt grants from
`example.input_reads`. The action's `token` field must be marked `secret: true`.
Rules never store literal credentials. Input grants follow admitted work,
workflow children, and retries; they do not give the action API access.
Editing deployed inputs requires authority over the captured grants.

Executions use stored input contracts for worker delivery. Output mismatches
produce sanitized warnings without changing successful status. JSONL validates
each record. Invalid sensitive output remains protected by whole-value encryption
and suppressed mirroring. An absent output schema imposes no output constraints.

Queue items retain their admission contracts for updates, retries, and dispatch.
New items use the current schema. Schema-secret payload fields stay encrypted at
rest; ordinary reads show redaction markers. Explicit item detail reads with
`decrypt=true` require queue read and decrypt authority. Execution values derived
from queue payloads retain the original queue identity for disclosure checks.

Events use separate flat schemas. Trigger `param_schema` configures rule trigger
instances, `out_schema` describes emitted payloads, and `event_config_schema`
describes optional sensor-supplied event configuration. Event admission applies
public defaults, validates strictly, and encrypts schema-secret fields under an
immutable binding. Later trigger updates do not change existing events.

Webhook configuration is server-generated transport metadata, not trigger-instance
configuration. Its stored contract protects the webhook key and all supplied
headers. Webhook intake therefore requires `security.encryption_key`, even when
the payload is public. Ordinary event reads stay redacted; explicit plaintext reads
require event decrypt authority and visibility. Execution values derived from an
event also require access to that original event before plaintext disclosure.

Inquiries bind their response schema at creation, before default insertion and
option validation. Repeated creation requests keep the original contract, even
after referenced types change. Secret option values and accepted responses stay
encrypted; response notifications carry only their public projection.

Respondents can submit `{"option_ref":"approve"}` to
`POST /api/v1/inquiries/{id}/respond` instead of sending the option's protected
response value. Ordinary inquiry authorization and privilege-loop restrictions
still apply. Supplying a literal `response` remains supported. Do not supply both
forms in one request.

`GET /api/v1/inquiries/{id}?include_secret_values=true` additionally requires
explicit `inquiries:decrypt` authority. Participation alone is not enough.
Recorded Key, execution, or queue origins may keep individual fields redacted
even when the caller can decrypt the inquiry. Workflow waits restore inquiry
values under the stored contract and retain their sensitivity in child inputs.

## Rollout status

Type definitions, authenticated discovery, Key assignment, and registry-backed
consumer writes are implemented. Pack deployment validates the candidate graph;
runtime admission captures immutable released contracts. Unowned components may
reference public root types. Keys and generic schema previews may reference any
installed type.

Service-level sensor lifecycle validation and final deployment checks are still
in progress. This implementation is not yet certified complete. An editor preview
never chooses the contract used by runtime admission.

See the [implementation plan](../plans/named-type-schemas.md) and
[compiler contract](../plans/named-type-schema-compiler.md) for supported keywords,
limits, revision selection, and remaining work.
