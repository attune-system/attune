# Workflow inquiry prerequisites

## Status

Implemented. This document records the canonical contract.

## Decision

A pack action owns inquiry creation and external communication. The workflow coordinator does not know how Slack, PagerDuty, Teams, Discord, or another provider works.

A later action task can declare the inquiry as a scheduling prerequisite. When that task is otherwise ready, the coordinator resolves the inquiry ID and waits. It creates no child execution until the inquiry reaches the required state.

This split gives each part one job:

- The pack action creates the Attune inquiry, contacts the external system, and returns the inquiry ID.
- The workflow coordinator records a durable task wait and releases the task after the inquiry is fulfilled.
- The provider integration submits the external user's response to the Attune inquiry API.
- The downstream action receives selected inquiry fields through normal workflow templates.

## Declarative contract

### Create and deliver the inquiry

The first task is an ordinary pack action:

```yaml
tasks:
  - name: request_approval
    action: slack.request_approval
    permission_set_refs:
      - standard
      - slack.inquiry_creator
    input:
      purpose: "production-deploy"
      channel: "#production-approvals"
      prompt: "Approve deployment of {{ parameters.version }} to production?"
      assigned_to: "{{ parameters.approver_identity_id }}"
      timeout_seconds: 3600
    next:
      - when: "{{ succeeded() }}"
        do:
          - deploy
      - when: "{{ failed() }}"
        do:
          - report_delivery_failure
```

`slack.request_approval` performs this work:

1. Create or find an inquiry through the Attune API.
2. Post a Slack message that identifies that inquiry.
3. Return the inquiry ID after Slack accepts the message.

Its result must contain:

```json
{
  "inquiry_id": 918,
  "provider_message_id": "1726671001.12345"
}
```

The same pattern works for `pagerduty.request_approval`, `teams.request_approval`, and other pack actions.

### Wait before scheduling a downstream action

The downstream action declares `wait_for.inquiry`:

```yaml
  - name: deploy
    action: deployments.deploy
    wait_for:
      inquiry: "{{ task.request_approval.inquiry_id }}"
    input:
      version: "{{ parameters.version }}"
      approval_reason: "{{ inquiry.deploy.response.reason }}"
      approved_by: "{{ inquiry.deploy.responded_by }}"
    next:
      - when: "{{ succeeded() }}"
        do:
          - verify
      - when: "{{ timed_out() }}"
        do:
          - report_approval_timeout
```

The coordinator handles this task as follows:

1. The graph and join rules make `deploy` ready.
2. The coordinator renders `wait_for.inquiry` from the completed upstream action result.
3. The coordinator verifies that inquiry `918` belongs to this workflow execution.
4. If the inquiry is pending, the coordinator records a wait and stops.
5. If the inquiry is responded, the coordinator renders the action input and creates the `deploy` child execution.
6. If the inquiry times out, the coordinator records a timed-out logical task outcome without creating the child execution.

`wait_for.inquiry` accepts exactly one value: an inquiry ID. It is always templatable. The coordinator renders it only after all graph and join prerequisites have completed. A pure expression preserves the upstream action's numeric `i64` result:

```yaml
wait_for:
  inquiry: "{{ task.request_approval.inquiry_id }}"
```

The upstream action should declare `inquiry_id` as an integer in its flat output schema. A missing, null, non-integer, or non-positive value fails the guarded logical task with `inquiry_reference_invalid`. It does not create a child execution.

```yaml
- name: process_response
  action: approvals.record_response
  wait_for:
    inquiry: "{{ task.send_question.inquiry_id }}"
  input:
    response: "{{ inquiry.process_response.response }}"
```

### Inquiry template namespace

The workflow context exposes awaited inquiries by guarded task name:

```text
inquiry.<task_name>.id
inquiry.<task_name>.status
inquiry.<task_name>.response
inquiry.<task_name>.assigned_to
inquiry.<task_name>.responded_by
inquiry.<task_name>.responded_at
inquiry.<task_name>.timeout_at
```

Pure expressions preserve JSON types. For example, this remains a boolean:

```yaml
approved: "{{ inquiry.deploy.response.approved }}"
```

The coordinator loads the namespace from persisted inquiry and wait records whenever it rebuilds workflow context. It does not depend on an in-memory RabbitMQ message.

The namespace remains available while rendering the guarded action and evaluating that task's transitions. Later tasks can also reference `inquiry.deploy` directly.

Task names that are valid workflow names but not expression identifiers use bracket access:

```yaml
approval: "{{ inquiry['deploy-prod'].response }}"
```

## Pack action contract

### The action owns both side effects

The pack action owns:

- provider credentials and API calls;
- message or incident formatting;
- Attune inquiry creation;
- provider callback correlation;
- provider-specific idempotency;
- cleanup when provider delivery definitely fails.

The coordinator owns none of those provider details. Attune's API-owned callback adapter authenticates and normalizes the provider response; the pack action does not run a listener.

### Create the inquiry before sending

The action normally creates the inquiry before it sends the provider message. The provider message needs an inquiry identifier or opaque response token so the callback can identify the response target.

If provider delivery definitely fails, the action should cancel the inquiry before returning a failed action result. If delivery may have succeeded, the action must not create another message blindly.

### Idempotent creation

An action can crash after inquiry creation or after provider delivery. The API must support idempotent action-owned inquiry creation.

For workflow actions, Attune derives the idempotency scope from:

```text
workflow execution + workflow task name + action attempt family + purpose
```

The first contract allows one named inquiry purpose per task:

```json
{
  "purpose": "approval",
  "prompt": "Approve deployment?",
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

Repeating the request with the same scope and equivalent immutable fields returns the existing inquiry. Reusing the scope with different immutable fields returns `409 Conflict`.

The action must also use the provider's idempotency mechanism when one exists. Attune can prevent duplicate inquiry rows, but only the provider adapter can prevent duplicate Slack messages or PagerDuty incidents.

### Execution-scoped API access

The action creates the inquiry with its execution token. The API derives the creator execution, workflow execution, and workflow task identity from that token. The action cannot claim an arbitrary execution or workflow.

The reserved `standard` permission does not grant inquiry creation. The Slack action also uses `slack.inquiry_creator`.

The inquiry response schema remains Attune's flat per-field format. The API validates submitted responses against that schema.

## External response path

The first released external-response contract uses metadata-selected adapters on fenced managed sensors and the internal durable callback inbox. Slack Socket Mode is the first configured adapter, not a platform route. The API acknowledges a provider delivery after the inbox commit, then a background monitor applies the response with fresh authorization, mapping, assignment, and inquiry-state checks. The [provider-neutral inquiry callback ingress](provider-neutral-inquiry-callback-ingress.md) defines the public HTTP contract for providers that sign direct callbacks. Unreleased listener and normalized-response scaffolding is not a compatibility contract and must be removed before release.

Examples include:

- a Slack app that receives interactive component callbacks;
- a PagerDuty webhook receiver;
- a Teams bot callback;
- a Discord interaction endpoint.

The callback adapter verifies the provider request, resolves an option-bound inquiry handle, maps the external actor to an Attune identity, and invokes the shared inquiry response service.

The response operation must:

- enforce assignment and integration permissions;
- validate the response against the stored flat schema;
- acquire the owning workflow advisory lock before changing a workflow-scoped inquiry;
- reject the response if the workflow is cancelling or terminal;
- persist `responded_by` and provider actor metadata;
- change `pending` to `responded` with a compare-and-set update;
- reject a response after timeout, cancellation, or an earlier response;
- avoid storing provider credentials or raw callback bodies in audit details.

An integration cannot submit an arbitrary `responded_by` value through the human response endpoint. Each callback adapter binds a distinct integration identity with an explicit `inquiries:respond` grant. Direct callback ingress extracts provider actor evidence, resolves it through a configured external-identity mapping, and records both the mapped Attune identity and a non-secret provider actor reference. If no trusted mapping exists, the response is rejected.

RabbitMQ publication remains a low-latency wake-up. A database reconciler must release waits when the response is committed but the wake-up message is lost.

## Runtime model

### Parsed task shape

Add the prerequisite to action tasks:

```rust
pub struct Task {
    pub wait_for: Option<TaskWaitFor>,
    // Existing fields remain unchanged.
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskWaitFor {
    pub inquiry: serde_json::Value,
}
```

The executable `TaskNode` snapshot must contain the same prerequisite. Running workflows use their stored graph, not a newly edited workflow definition.

The expression validator must inspect `wait_for.inquiry`. It must add `inquiry` to the canonical template namespaces used by the guarded action's input and later workflow templates.

The first contract permits one inquiry prerequisite per task. It rejects `wait_for` on parallel containers, nested workflow nodes, `with_items`, and `iterate_cache`. Iterated inquiry prerequisites need a separate per-item activation and correlation design.

### Durable task wait

The coordinator needs durable state before child execution creation:

```text
workflow_task_wait
  id BIGINT
  workflow_execution BIGINT
  task_name TEXT
  kind inquiry
  state waiting | failed | timed_out | cancelled | released
  inquiry BIGINT
  result JSONB NULL
  created TIMESTAMPTZ
  resolved_at TIMESTAMPTZ NULL
  released_at TIMESTAMPTZ NULL
  updated TIMESTAMPTZ
```

The current runtime dispatches a task name at most once per workflow execution. The first unique key is therefore `(workflow_execution, task_name)`. The parser currently accepts cycles even though `workflow_task_dispatch` prevents repeated task dispatch. Validation must reject guarded tasks in cyclic graph regions until action dispatch and waits both gain activation ordinals.

The wait record is the idempotency boundary:

- Reprocessing task readiness finds the same wait.
- Duplicate response messages resolve the same wait.
- Releasing the wait uses the existing `workflow_task_dispatch` claim.
- A restart reconstructs waiting work without creating a child execution.

### Inquiry ownership

The inquiry remains owned by the upstream action execution that created and delivered it. The wait record only references the inquiry.

The API stores workflow scope inferred from the creator execution. A guarded task may wait only for an inquiry created within the same workflow execution. Cross-workflow and arbitrary inquiry waits are outside the first contract.

The unreleased `InquiryHandler` scaffolding assumes that a response completes `inquiry.created_by_execution`. Remove it before action-created waitable inquiries are enabled. A response changes inquiry state; workflow wait resolution controls downstream scheduling. Synthetic `core.ask` execution completion and the `__inquiry` result marker must not ship.

## Coordinator flow

### Reach a guarded task

Both entry-task and successor-task paths must call one prerequisite-aware activation operation.

1. Acquire the workflow advisory lock.
2. Lock the workflow execution.
3. Confirm that graph and join rules make the task ready.
4. Rebuild workflow context from persisted state.
5. Render `wait_for.inquiry` as a positive `i64`.
6. Load the inquiry and verify its workflow scope.
7. Create or lock the `workflow_task_wait` row.
8. Resolve the current inquiry state.

If the inquiry is pending, commit the wait and schedule nothing.

If the inquiry is already terminal, apply its outcome immediately. This handles a fast external response that arrives before the upstream action result advances the graph.

If the expression is invalid, the inquiry is missing, or the inquiry belongs to another workflow, fail the logical task without creating its child execution. The task's failed transitions receive a stable error result.

### Release a satisfied wait

The response MQ handler and the periodic reconciler call the same operation:

1. Acquire the workflow advisory lock.
2. Lock the workflow execution, wait, and inquiry in a fixed order.
3. Exit if the wait is already terminal or released.
4. Stop if the workflow is cancelling or terminal.
5. Build `inquiry.<task_name>` from the persisted inquiry.
6. Render the guarded action's input and dispatch settings.
7. Claim `workflow_task_dispatch` and create the child execution.
8. Mark the wait `released` in the same transaction.
9. Commit, then publish `ExecutionRequested`.

A crash cannot leave two children because wait release and `workflow_task_dispatch` use durable unique claims.

### Advance without a child execution

Prerequisite failure and timeout need a workflow advancement entry point that does not require an `Execution`:

```rust
pub struct LogicalTaskOutcome {
    pub workflow_execution_id: i64,
    pub task_name: String,
    pub outcome: TaskOutcome,
    pub result: serde_json::Value,
}

async fn advance_from_logical_task_outcome(
    tx: &mut sqlx::PgConnection,
    outcome: LogicalTaskOutcome,
    pending_messages: &mut Vec<PendingExecutionRequested>,
) -> Result<WorkflowAdvanceOutcome>;
```

Extract transition evaluation, publish directives, join accounting, successor activation, and workflow completion from the current execution-specific advancement path. Child completion adapts an `Execution` into `LogicalTaskOutcome`. Invalid references and timeout call the same operation directly. Do not create synthetic failed or timed-out child executions.

### Handle timeout

The inquiry's persisted `timeout_at` is authoritative. Waiting time does not consume the guarded action's execution timeout.

When the inquiry reaches `timeout`, mark the wait timed out and apply a timed-out task outcome without creating a child execution. The task's `timed_out()` transitions run with:

```json
{
  "code": "inquiry_timeout",
  "inquiry_id": 918,
  "status": "timeout"
}
```

If the inquiry is cancelled independently, mark the wait failed and apply a failed task outcome with `inquiry_cancelled`. Do not create the guarded child execution.

### Handle workflow cancellation

Workflow cancellation marks unreleased waits cancelled under the workflow advisory lock. It does not evaluate their outgoing transitions.

Because the inquiry was created for this workflow, cancellation should also change a still-pending inquiry to `cancelled`. A late provider callback then receives a conflict instead of reviving stopped work.

API-owned and executor-owned cancellation paths must use one repository operation and lock order.

### Account for waiting work

Workflow completion must include non-terminal `workflow_task_wait` rows. A parallel branch must not complete the workflow while another branch waits for an inquiry.

Join accounting treats failed and timed-out waits as terminal logical task outcomes. A released wait is not complete until its child execution reaches a terminal state.

## Feature tree

```text
Action-created inquiry prerequisites
|
+-- Pack action ownership
|   +-- create inquiry through scoped API
|   +-- deliver provider message
|   +-- return inquiry_id
|   +-- idempotent retry
|   +-- option-bound provider controls
|
+-- Workflow authoring
|   +-- templated wait_for.inquiry ID
|   +-- inquiry.<task_name> templates
|   +-- normal next transitions
|
+-- Workflow coordination
|   +-- gate before child creation
|   +-- durable workflow_task_wait
|   +-- immediate handling of early responses
|   +-- advisory-locked release
|   +-- no polling action
|
+-- Inquiry lifecycle
|   +-- action-scoped idempotent creation
|   +-- response schema validation
|   +-- persisted responder identity
|   +-- timeout and cancellation
|
+-- Recovery
|   +-- RabbitMQ wake-up hint
|   +-- terminal-inquiry reconciliation
|   +-- duplicate response handling
|   +-- requested-child republish
|
+-- Security
|   +-- explicit inquiry-create permission
|   +-- workflow-scope validation
|   +-- external identity mapping
|   +-- response-content redaction
|
+-- Product UI
|   +-- guarded-task badge
|   +-- waiting state in timeline
|   +-- inquiry link and status
|   +-- no synthetic child execution
|
+-- Pre-release cleanup
    +-- update bundled core.ask definitions
    +-- remove execution-resume inquiry handling
    +-- remove legacy __inquiry marker
```

## Feature work tree

```text
1. Freeze the workflow contract and remove conflicting scaffolding
   |
   +-- Add wait_for parser fixtures
   +-- Define inquiry namespace and result shapes
   +-- Reject iteration and cyclic guarded tasks
   +-- Remove core.ask interception, __inquiry creation, and execution-resume handling
   |
   v
2. Add safe action-owned inquiry creation
   |
   +-- Derive creator and workflow scope from execution tokens
   +-- Add purpose-based idempotent create-or-get
   +-- Add explicit inquiry-create permission
   +-- Validate flat response schemas
   +-- Persist responded_by and provider actor metadata
   +-- Reject arbitrary execution IDs from execution tokens
   +-- Restrict generic inquiry update and delete
   +-- Add compare-and-set response and timeout transitions
   |
   v
3. Add durable task waits
   |
   +-- Add workflow_task_wait schema and repository
   +-- Snapshot wait_for into TaskNode
   +-- Add create-or-get and terminal-state queries
   +-- Regenerate SQLx metadata
   |
   v
4. Gate task dispatch
   |
   +-- Unify entry and successor activation
   +-- Resolve inquiry IDs from workflow context
   +-- Verify same-workflow ownership
   +-- Stop before child execution creation
   +-- Release through workflow_task_dispatch
   |
   v
5. Apply prerequisite outcomes
   |
   +-- Add inquiry namespace to context rebuild
   +-- Add LogicalTaskOutcome advancement without child executions
   +-- Reuse transition, publish, join, and terminal logic
   +-- Count waits as active workflow work
   |
   v
6. Make wake-up and recovery reliable
   |
   +-- Treat InquiryResponded MQ as a hint
   +-- Reconcile terminal inquiries with unresolved waits
   +-- Prove response-before-wait registration
   +-- Prove restart and duplicate delivery behavior
   +-- Convert core.ask definitions and tests
   |
   v
7. Integrate cancellation and API lifecycle
   |
   +-- Use the workflow advisory lock in API and executor cancellation
   +-- Cancel waits and pending inquiries in the shared transaction
   |
   v
8. Add authoring and runtime UI
   |
   +-- Round-trip wait_for in the workflow builder
   +-- Show a waiting prerequisite on the guarded task
   +-- Link to inquiry details
   +-- Add response, rejection, timeout, and cancellation coverage
   |
   v
9. Finish documentation
   |
   +-- Update workflow and inquiry documentation
```

## First implementation slice

The first slice should avoid an external provider and prove the scheduling contract:

1. Add `wait_for.inquiry` to parsing, validation, and graph snapshots.
2. Add scoped, idempotent inquiry creation and compare-and-set response transitions.
3. Restrict generic inquiry mutation.
4. Add logical task advancement that does not require a child execution.
5. Add an API test action that creates an inquiry and returns its ID.
6. Persist a task wait without creating the guarded child execution.
7. Release the wait and create the guarded child exactly once.
8. Integrate cancellation under the same workflow lock.
9. Convert bundled `core.ask` definitions and remove its scheduler interception, generic `__inquiry` creation, and execution-resume response handler before release.
10. Prove response-before-registration, timeout, rejection, cancellation, duplicate wake-up, and executor restart.
11. Count the pending wait during parallel workflow completion.

After that works, update one provider pack. Slack is a useful reference because it exercises outbound delivery, interactive callbacks, provider idempotency, and external identity mapping.

## Rejected shapes

### A separate inquiry wait node

A separate node makes the graph more explicit, but it forces authors to add a structural task that performs no business action. `wait_for` belongs at the exact scheduling boundary it guards.

### The coordinator creates or delivers the inquiry

Provider integration belongs in packs. The coordinator should not know provider credentials, message formats, callback protocols, or delivery retries.

### A worker action polls the inquiry

A polling action occupies worker capacity for a human-scale wait and moves restart recovery into pack code. The coordinator can persist the prerequisite without running a process.

### The upstream action remains running

The delivery action should finish after the provider accepts the message. Keeping it running couples worker lifetime and token lifetime to a human response.

### RabbitMQ is the wait record

Messages can be lost after a response commits. The database wait and inquiry states are authoritative. RabbitMQ only reduces response latency.
