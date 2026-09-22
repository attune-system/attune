# Inquiry API

The inquiry API lets a workflow task pause for a human response. An inquiry has immutable fixed response options. Each option contains the response object that Attune records when a user or provider control selects it.

All endpoints require a bearer token. Inquiry creation and cancellation require an execution token. Human response endpoints accept access and execution tokens.

## Client and caller matrix

The caller's token determines which operations it can perform. The web portal does not expose creation or cancellation because browser access tokens do not carry execution authority.

| Caller | Browse | Respond | Create | Cancel |
|---|---|---|---|---|
| Web portal with an access token | Yes | Yes | No | No |
| CLI with an access token | Yes | Yes | No | No |
| MCP with an access token | Yes | Yes | No | No |
| CLI or MCP with an execution token | Yes | Yes, subject to self-approval checks | Yes, with `inquiries:create` | Yes, for the exact creator execution |
| Managed sensor callback adapter | No | Select one fixed option | No | No |

Browse operations return only inquiries visible to the caller. An identity can read an inquiry when it is the assignee, owns the creator execution, or has a matching `inquiries:read` grant. Execution context has a separate authorization check. If the identity cannot read an execution, the API omits that execution's action, pack, and workflow fields. It also returns `0` for an unreadable `created_by_execution`.

Responding does not require `inquiries:read` or `inquiries:respond`. An assigned inquiry accepts a response only from the assigned identity. An unassigned inquiry accepts a response from any access or execution identity, except that the creator execution and its descendants cannot answer it. The provider callback path has separate integration authorization and requires `inquiries:respond` for the sensor identity.

### Web portal

Open **Inquiries** to filter visible inquiries by status, creator execution, assignee, workflow action, or workflow pack. Workflow filters require permission to read matching executions. The list and detail pages show identity labels and readable creator and workflow context. The detail page renders saved responses against the flat response schema and masks secret fields.

### CLI

Access-token commands browse and answer inquiries:

```bash
attune inquiry list --status pending
attune inquiry show 123
attune inquiry respond 123 --option approve
attune inquiry respond 123 --response-json '{"approved":true}'
```

Execution-token commands create and cancel inquiries:

```bash
attune inquiry execution create --request-file inquiry.json
attune inquiry execution create --request-file - < inquiry.json
attune inquiry execution cancel 123
```

JSON and YAML list output includes both `items` and `pagination`. Use `pagination.has_next` with `--offset` and `--limit` to continue a bounded scan.

### MCP

MCP exposes these access-token tools:

- `inquiries_list`
- `inquiries_get`
- `inquiries_respond`

`inquiries_respond` requires exactly one of `option_ref` or `response`. MCP also exposes `execution_inquiries_create` and `execution_inquiries_cancel` for clients configured with an execution token.

### Managed sensor adapter

The managed sensor callback API is not a general inquiry client. It accepts one fixed-option selection from a fenced sensor workload through a metadata-selected adapter:

```http
POST /api/v1/internal/inquiry-callbacks/{adapter_ref}
```

The sensor's `config.inquiry_callback_adapters` metadata defines the provider, subject kind, JSON Pointer extraction rules, exact required values, allowed values, and array lengths. Attune reads that metadata from the workload's immutable sensor snapshot. The request cannot choose its provider, actor kind, integration identity, or response object.

Attune commits the encrypted normalized delivery before returning `{"acknowledge":true}`. Background processing then checks the sensor grant, exact external identity mapping, inquiry assignment, response handle, and current inquiry state. See [Workflow inquiry handling](../workflows/inquiry-handling.md#response-contract) for the delivery sequence.

## Inquiry object

```json
{
  "id": 123,
  "created_by_execution": 456,
  "created_by_action_ref": "deploy.request_approval",
  "created_by_pack_ref": "deploy",
  "workflow_execution": 321,
  "workflow_root_execution": 455,
  "workflow_action_ref": "deploy.production_release",
  "workflow_pack_ref": "deploy",
  "workflow_task_name": "request_approval",
  "purpose": "production-approval",
  "prompt": "Approve deployment to production?",
  "response_schema": {
    "approved": {
      "type": "boolean",
      "required": true
    }
  },
  "response_options": [
    {
      "ref": "approve",
      "label": "Approve",
      "style": "positive",
      "response": {"approved": true}
    },
    {
      "ref": "reject",
      "label": "Reject",
      "style": "destructive",
      "response": {"approved": false}
    }
  ],
  "assigned_to": 789,
  "assigned_to_login": "release-reviewer",
  "assigned_to_display_name": "Release reviewer",
  "status": "pending",
  "response": null,
  "timeout_at": "2026-09-20T12:00:00Z",
  "responded_by": null,
  "responded_by_login": null,
  "responded_by_display_name": null,
  "responded_at": null,
  "created": "2026-09-20T11:00:00Z",
  "updated": "2026-09-20T11:00:00Z"
}
```

`response_schema` uses Attune's flat per-field format. Do not send a raw JSON Schema object with top-level `type` and `properties` fields.

`workflow_execution` is the internal workflow state row ID. Use `workflow_root_execution` for execution API requests and web links.

The API redacts submitted fields marked `secret: true` when it returns an inquiry. Workflow evaluation still uses the stored value. Fixed response option payloads remain visible to callers who can read the inquiry because clients need those payloads to submit an option. Do not put secrets in fixed response options.

Each response option has these fields:

| Field | Rules |
|---|---|
| `ref` | Unique lowercase token, up to 64 ASCII characters. It may contain digits, `_`, and `-`. |
| `label` | Nonempty display text, up to 255 bytes. |
| `style` | `default`, `positive`, or `destructive`. |
| `response` | JSON object that conforms to `response_schema`. |

An inquiry must have 1 to 25 options. The serialized options array must not exceed 64 KiB.

The status is one of `pending`, `responded`, `timeout`, or `cancelled`.

## List inquiries

`GET /api/v1/inquiries`

The endpoint accepts these optional query parameters:

| Parameter | Description |
|---|---|
| `status` | Filter by inquiry status. |
| `created_by_execution` | Filter by creator execution ID. |
| `assigned_to` | Filter by assigned identity ID. |
| `workflow_action_ref` | Exact containing workflow action reference. Requires matching `executions:read` permission. |
| `workflow_pack_ref` | Exact containing workflow pack reference. Requires matching `executions:read` permission. |
| `offset` | Number of visible rows to skip. The default is `0`. |
| `limit` | Number of rows to return. The default is `50`; the maximum is `500`. |

The API returns only inquiries visible to the authenticated identity. It applies execution authorization separately to the creator execution and the workflow root execution. A `403 Forbidden` response to a workflow filter means that the caller cannot use the filter to inspect execution references.

## Get an inquiry

`GET /api/v1/inquiries/{id}`

The API returns `404 Not Found` when the inquiry does not exist or the authenticated identity cannot read it.

## List inquiries by status

`GET /api/v1/inquiries/status/{status}`

This endpoint accepts the standard `page` and `page_size` pagination parameters. Valid status path values are `pending`, `responded`, `timeout`, and `cancelled`.

## List inquiries created by an execution

`GET /api/v1/executions/{execution_id}/inquiries`

This endpoint accepts the standard `page` and `page_size` pagination parameters. It returns inquiries created by the path execution and returns `404 Not Found` when that execution does not exist.

## Create an inquiry

`POST /api/v1/inquiries`

Use an execution token with `inquiries:create`. The token must belong to a workflow task execution. Attune derives the execution, workflow, task, and attempt from the token.

```json
{
  "purpose": "production-approval",
  "prompt": "Approve deployment to production?",
  "response_schema": {
    "approved": {"type": "boolean", "required": true}
  },
  "response_options": [
    {
      "ref": "approve",
      "label": "Approve",
      "style": "positive",
      "response": {"approved": true}
    },
    {
      "ref": "reject",
      "label": "Reject",
      "style": "destructive",
      "response": {"approved": false}
    }
  ],
  "assigned_to": 789,
  "timeout_seconds": 3600
}
```

`purpose` makes creation idempotent within one workflow task attempt. An equivalent retry returns the existing inquiry. A retry that changes the prompt, schema, options, assignment, or timeout returns `409 Conflict`.

The response contains the inquiry and one opaque handle per response option:

```json
{
  "data": {
    "inquiry": {
      "id": 123,
      "created_by_execution": 456,
      "purpose": "production-approval",
      "prompt": "Approve deployment to production?",
      "response_options": [
        {
          "ref": "approve",
          "label": "Approve",
          "style": "positive",
          "response": {"approved": true}
        },
        {
          "ref": "reject",
          "label": "Reject",
          "style": "destructive",
          "response": {"approved": false}
        }
      ],
      "assigned_to": 789,
      "status": "pending"
    },
    "response_options": [
      {
        "ref": "approve",
        "label": "Approve",
        "style": "positive",
        "response_handle": "attune_irh_REDACTED"
      },
      {
        "ref": "reject",
        "label": "Reject",
        "style": "destructive",
        "response_handle": "attune_irh_REDACTED"
      }
    ]
  },
  "message": "Inquiry created successfully"
}
```

The response includes `Cache-Control: no-store`. A handle is at most 96 ASCII characters and selects exactly one stored option. Do not log, persist outside the provider control, or expose a handle in audit data. A later callback route must still authenticate its provider request and resolve the external identity. The handle alone grants no access.

Equivalent retries may return new handles for the same options. Use the numeric inquiry ID for workflow waits.

## Respond to an inquiry

`POST /api/v1/inquiries/{id}/respond`

```json
{
  "response": {"approved": true}
}
```

Attune validates the response against the stored flat schema. Only a pending, unexpired inquiry can accept a response. If `assigned_to` is set, only that identity can respond.

This endpoint does not require an `inquiries:read` or `inquiries:respond` grant. A caller can submit a custom response without first reading the inquiry. Clients that select a fixed option by `ref` must read the inquiry first to resolve that option's stored response object.

An execution cannot respond to an inquiry that it created. A descendant execution also cannot respond to an ancestor's inquiry. These checks prevent an action or its child from approving its own request.

The first valid response changes the status to `responded`. Concurrent or repeated responses return `409 Conflict`.

## Cancel an inquiry

`POST /api/v1/inquiries/{id}/cancel`

Only the execution token that created the inquiry can cancel it. The inquiry must still be pending. A successful request changes the status to `cancelled`.

## Errors

| Status | Meaning |
|---|---|
| `400 Bad Request` | The request body is malformed. |
| `401 Unauthorized` | Authentication is missing or invalid. |
| `403 Forbidden` | The identity, assignment, token type, or execution relationship does not permit the operation. |
| `404 Not Found` | The resource does not exist or is not visible. |
| `409 Conflict` | Idempotent creation fields differ, or the inquiry can no longer change state. |
| `422 Unprocessable Entity` | The request, schema, option, or response fails validation. |

## Related documentation

- [Workflow orchestration](../workflows/workflow-orchestration.md)
- [Execution API](./api-executions.md)
- [Notifier WebSocket API](./notifier-websocket.md)
