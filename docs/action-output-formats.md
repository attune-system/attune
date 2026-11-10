# Action output formats

An action's `output_format` selects how stdout is parsed. Parsed output is stored
at `execution.result.data`; `execution.result` is the execution envelope. Channel
projections are stored at `execution.result.stdout` and `.stderr`, subject to the
bound output schema and channel policy. Captures are bounded by worker settings.

| Format | Parsing | `result.data` |
| --- | --- | --- |
| `text` (default) | No structured parsing | Absent |
| `json` | Entire stdout, then the final line if whole-stream parsing fails | Parsed JSON value |
| `yaml` | Entire stdout | Parsed YAML value encoded as JSON |
| `jsonl` | Each valid JSON line collected in order | Array of records |

Structured parsing runs for successful processes with nonempty stdout. Failure
to parse leaves `data` absent; parsing itself does not change the process exit
status. JSONL data parsing skips invalid lines. Sanitized stdout uses stricter
rules: malformed nonempty records prevent a public raw-channel projection, even
when valid records are available in `data`.

## Text

```yaml
name: echo
output_format: text
```

A public channel producing `Hello, World!` has an envelope such as:

```json
{
  "exit_code": 0,
  "succeeded": true,
  "stdout": "Hello, World!"
}
```

Text stdout supports `public` or `secret` channel policy. Partial secrecy requires
a structured output format and schema.

## JSON and YAML

Schema definitions use Attune's flat per-field `out_schema` format:

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
      token:
        type: string
        secret: true
```

The action can write a complete JSON document or write debug lines followed by a
final JSON line. With this schema, `result.data.account.token` is encrypted and
replaced by a redaction marker. Its public `result.stdout` defaults to a
serialization of the classified value, excluding debug text. Public siblings
remain visible. A protected original channel is stored separately.

For YAML, set `output_format: yaml` and emit one YAML document. The schema format
is the same. Sanitized YAML stdout is serialized as JSON, which is valid YAML and
avoids copying unclassified text.

## JSON Lines

The flat schema describes **one record**, not an enclosing array:

```yaml
name: list_users
output_format: jsonl
out_schema:
  id:
    type: integer
  username:
    type: string
  token:
    type: string
    secret: true
```

For output such as:

```jsonl
{"id":1,"username":"alice","token":"example-token-a"}
{"id":2,"username":"bob","token":"example-token-b"}
```

`result.data` contains two records, with each `token` individually protected.
Sanitized stdout serializes each classified record on its own line. Resource
limits and malformed or truncated stdout can prevent a safe channel projection;
this does not hide independently classifiable public data.

## Classification and validation

The output contract is pinned at admission. Changing an action does not change
classification for executions already admitted. Output conformance is advisory:
public warnings contain codes, while malformed subtrees containing possible
schema-secret descendants are protected conservatively. Compiler and runtime
resource limits still apply.

Secret input parameters, runtime API reads, Key reads, and selected permissions
do not classify outputs. Authors must mark fields that can contain secrets.
Missing output declarations and unmarked permitted fields are public. Channel
choices are controlled by `output_policy`; see [action output secrecy](action-output-secrecy.md)
for defaults, authoring validation, logs, and explicit disclosure through the web,
API, CLI, and MCP.
