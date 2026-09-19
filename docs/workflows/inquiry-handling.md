# Workflow inquiry handling

An action creates and delivers an inquiry. A later workflow task can wait for that inquiry without creating a child execution first.

## Authoring contract

Add `wait_for.inquiry` to an action task. The value must render to a positive integer inquiry ID.

```yaml
tasks:
  request_approval:
    action: slack.request_approval
    input:
      prompt: "Approve production deployment?"
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
  "assigned_to": 42,
  "timeout_seconds": 3600
}
```

The API derives the creator execution, the workflow execution, and the workflow task from the token. Callers cannot supply those fields.

The action needs an execution permission set that grants `inquiries:create`. The reserved `standard` permission set does not grant inquiry creation.

Creation is idempotent within this scope:

```text
workflow execution + workflow task name + action attempt family + purpose
```

An equivalent retry returns the existing inquiry. A retry that changes `prompt`, `response_schema`, `assigned_to`, or `timeout_seconds` returns `409 Conflict`.

The action owns provider delivery and provider idempotency. If delivery definitely fails, cancel the inquiry before the action returns a failure. Attune prevents duplicate inquiry rows, but the provider adapter must prevent duplicate provider messages.

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

The response update uses a pending-state compare-and-set. A second response, a late response, or a response after cancellation returns `409 Conflict`.

Provider integrations must map the provider actor to an Attune identity before submitting a response. Do not store provider credentials or raw callback bodies in inquiry metadata.

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
