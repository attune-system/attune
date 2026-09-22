# Workflow inquiry handling

An action creates and delivers an inquiry. A later workflow task can wait for that inquiry without creating a child execution first.

## Authoring contract

Add `wait_for.inquiry` to an action task. The value must render to a positive integer inquiry ID.

```yaml
tasks:
  request_approval:
    action: slack.request_approval
    input:
      purpose: "production-deploy"
      prompt: "Approve production deployment?"
      channel: "C123"
      assigned_to: "{{ parameters.approver_identity_id }}"
    next:
      - do: deploy

  deploy:
    action: deploy.release
    wait_for:
      inquiry: "{{ task.request_approval.inquiry_id }}"
    input:
      approved: "{{ inquiry.deploy.response.approved }}"
```

`wait_for` is not allowed on parallel containers, `with_items` tasks, or `iterate_cache` tasks. A guarded task cannot belong to a cyclic graph region.

The workflow context exposes inquiries by guarded task name:

```text
inquiry.<task_name>.id
inquiry.<task_name>.status
inquiry.<task_name>.response
inquiry.<task_name>.assigned_to
inquiry.<task_name>.responded_by
inquiry.<task_name>.responded_at
inquiry.<task_name>.timeout_at
```

Pure expressions preserve JSON types. `{{ inquiry.deploy.response.approved }}` returns a boolean when the stored response is a boolean.

## Action contract

The action creates the inquiry before sending a provider message. It calls `POST /api/v1/inquiries` with its execution token:

```json
{
  "purpose": "approval",
  "prompt": "Approve production deployment?",
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
      "response": {
        "approved": true
      }
    },
    {
      "ref": "reject",
      "label": "Reject",
      "style": "destructive",
      "response": {
        "approved": false
      }
    }
  ],
  "assigned_to": 42,
  "timeout_seconds": 3600
}
```

The API derives the creator execution, the workflow execution, and the workflow task from the token. Callers cannot supply those fields.

The creation response contains the internal inquiry and rendering metadata with one opaque handle per response option:

```json
{
  "data": {
    "inquiry": {
      "id": 918,
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
  }
}
```

Use `inquiry.id` for `wait_for.inquiry`. Put each option's handle only in its matching provider control. Attune encrypts the handles, and each equivalent creation retry may return different handles for the same inquiry options. Every issued handle becomes unusable when the inquiry is answered, times out, or is cancelled. Do not log, audit, or return handles as an action output.

The action needs an execution permission set that grants `inquiries:create`. The reserved `standard` permission set does not grant inquiry creation.

Creation is idempotent within this scope:

```text
workflow execution + workflow task name + action attempt family + purpose
```

An equivalent retry returns the existing inquiry. A retry that changes `prompt`, `response_schema`, `response_options`, `assigned_to`, or `timeout_seconds` returns `409 Conflict`.

The action owns provider delivery and provider idempotency. If delivery definitely fails, cancel the inquiry before the action returns a failure. Attune prevents duplicate inquiry rows, but the provider action must prevent duplicate provider messages.

## Response contract

Submit a human response with `POST /api/v1/inquiries/{id}/respond`:

```json
{
  "response": {
    "approved": true
  }
}
```

The API validates the response against the inquiry's flat `response_schema`. Only the assigned identity can answer an assigned inquiry. The API rejects self-approval by the creator execution and its descendants.

Fields marked `secret: true` are redacted from inquiry API responses after submission. Workflow context uses the stored value. Fixed response option payloads are visible to inquiry readers, so options must not contain secrets.

The response update uses a pending-state compare-and-set. A second response, a late response, or a response after cancellation returns `409 Conflict`.

Provider integrations must map the provider actor to an Attune identity before submitting a response. Do not store provider credentials or raw callback bodies in inquiry metadata.

Managed sensors submit provider envelopes through one metadata-selected endpoint:

```http
POST /api/v1/internal/inquiry-callbacks/{adapter_ref}
```

The sensor calls this endpoint with its fenced sensor token and the provider's native envelope. Its release-pinned `config.inquiry_callback_adapters` entry defines the provider, subject kind, JSON Pointer extraction rules, and bounded request constraints. Attune stores the normalized selection as encrypted JSON before acknowledging it. The handle supplies opaque correlation and option integrity, not authorization. Ingress requires the caller's current workload fence and an enabled adapter with the same ref in both live and pinned sensor metadata. The background processor rechecks current sensor availability, the `inquiries:respond` grant for the sensor identity, the exact provider identity mapping, and that the mapped identity matches `assigned_to`.

For example, a Slack Socket Mode sensor can declare `slack.socket_mode` and map `/envelope_id`, `/payload/team/id`, `/payload/user/id`, and `/payload/actions/0/value`. Slack's field names and action IDs remain pack metadata rather than API route code.

```yaml
config:
  inquiry_callback_adapters:
    slack.socket_mode:
      enabled: true
      provider: slack
      subject_kind: user
      request:
        delivery_id_pointer: /envelope_id
        tenant_pointer: /payload/team/id
        external_subject_pointer: /payload/user/id
        response_handle_pointer: /payload/actions/0/value
        required_values:
          /type: interactive
          /payload/type: block_actions
        allowed_values:
          /payload/actions/0/action_id:
            - attune.inquiry.response.v1.approve
            - attune.inquiry.response.v1.reject
        required_array_lengths:
          /payload/actions: 1
```

Attune returns `{"acknowledge":true}` after the encrypted delivery commits, so the sensor can acknowledge its provider within the provider's deadline. Failures before that commit return a non-2xx response and remain unacknowledged. After the commit, the API owns the delivery independently of the sensor process lease. A background monitor retries deliveries left `pending` by API crashes. Terminal authorization, mapping, handle, assignment, and inquiry-state failures mark the delivery `rejected`; successful responses mark it `accepted` in the same transaction as the inquiry response.

## Runtime behavior

The executor stores each guarded task in `workflow_task_wait`. While the inquiry is pending, the executor creates no child execution for the guarded task.

When the inquiry is responded:

1. The executor takes the workflow advisory lock.
2. The executor rebuilds workflow and inquiry context from PostgreSQL.
3. The executor creates the guarded child through the existing `workflow_task_dispatch` claim.
4. The executor marks the wait `released` in the same transaction.
5. The executor publishes `ExecutionRequested` after commit.

Timeout and cancellation create logical task outcomes without fake child execution rows. Workflow transitions can use `timed_out()` and `failed()` for those outcomes.

RabbitMQ only wakes the executor sooner. The reconciliation loop reads terminal inquiries and unresolved waits from PostgreSQL, so a lost message does not strand a workflow.

`core.ask` and the `__inquiry` action-result marker are not supported. Packs must create inquiries through the authenticated API.
