# Action output secrecy

Pack authors classify structured outputs through the flat `out_schema` field
map. Secret input parameters, Key reads, runtime API reads, and selected
permissions do not make an action's outputs secret. Trusted action and workflow
authors must mark output values that can contain secrets.

```yaml
name: fetch_account
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
output_policy:
  stdout: sanitized
  stderr: secret
  error: secret
```

Here `result.data.status`, `account.name`, and each credential's `service` remain
public. Only `access_token` and each `password` are encrypted and replaced with
redaction markers. Marking an object or array itself `secret: true` protects its
whole subtree. Nested objects, arrays, named types, and inherited secret markings
use the shared value-schema compiler. A child cannot override inherited secrecy
with `secret: false`.

Unmarked fields, including permitted extra fields, are public. A missing output
schema does not inherit secrecy from inputs. The same rule applies to workflow
output maps using the workflow action's own bound output contract. Existing
pack/action/workflow edit authority controls classification; execution callers
cannot override the admitted contract.

## Channel policy

`output_policy` controls channels outside parsed `result.data`:

| Channel | Choices | Meaning |
| --- | --- | --- |
| `stdout` | `public`, `secret`, `sanitized` | Publish the channel, protect the whole channel, or publish only a classified serialization of structured output. |
| `stderr` | `public`, `secret` | Publish or protect the diagnostic channel. |
| `error` | `public`, `secret` | Publish or protect execution error text. |

With secret output declarations, structured stdout defaults to `sanitized`,
and stderr/error default to `secret`. Text stdout with secret output declarations
defaults to `secret`. Without secret output declarations, all channels default
to `public`. Individual policy choices override these defaults. Input sensitivity
and runtime permissions never affect policy defaults.

`sanitized` requires JSON, YAML, or JSONL output. It emits a serialization with
secret paths replaced by markers and omits leading debug text. YAML projections
use JSON serialization, which is also valid YAML. JSONL is classified record by
record. Publication rejects `stdout: public` with secret output declarations,
because raw stdout would duplicate the declared secrets.

The original protected channel is stored separately from its public projection.
If parsing, truncation, or a resource limit prevents a safe stdout projection,
only that channel is hidden, with a bounded reason in `result.output_channels`.
Valid public structured siblings remain visible. Invalid structured shapes
protect the smallest ambiguous subtree containing declared secrets. Validation
warnings contain codes rather than values or raw validator messages.

Runtime log access and console mirroring also follow the bound channel policy.
Sanitized mirrors wait for classification; private originals require authorized
disclosure. Structured secret slots and original channel bodies retain recorded
input/runtime origins for later authorization. Origins govern disclosure of
already-classified secret values; they do not classify public output fields.

## Explicit disclosure

Ordinary API, web, CLI, and MCP views remain redacted at protected paths. The web
execution detail page can request authorized reveal without hiding public
siblings. The detail API accepts `include_secret_values=true`. It restores
authorized structured slots and returns original channels separately under
`revealed_output_channels`; sanitized `result.stdout` remains unchanged.

```bash
attune execution show 123 --include-secret-values
attune execution result 123 --decrypt
```

MCP `executions_export` requires an explicit `output_file` and writes the full
authorized execution object to a new private file on the MCP server host. The
tool returns only the path, byte count, and status. MCP `executions_get` remains
redacted inline. Private-file export currently requires a Unix host. See
[CLI and MCP](cli/cli.md#file-only-execution-disclosure-through-mcp).

Reveal requires execution read/decrypt permission and access to retained,
recorded origins. Unavailable or revoked origins can leave particular values
redacted even in a successful response. No additional authoring approval or
declassification permission is introduced.

## Pre-production storage boundary

This canonical v1 model requires fresh installations of the revised migrations
and coherent producers/consumers. Existing development data needs an explicit
coordinated reset or a separately scoped conversion before deployment. Stop or
drain old work before switching services; retries cannot cross the binding-format
boundary. Startup does not reset databases, rewrite checksums, or reinterpret
old protected results as public. See the [implementation plan](plans/action-output-secret-classification.md#development-data-and-rollout)
and [database policy](deployment/postgresql-only.md).
