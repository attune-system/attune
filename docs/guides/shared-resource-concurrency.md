# Shared resource concurrency

A shared resource policy limits executions across action definitions. It adds a
constraint without changing action, pack, or global standard-policy precedence.
Each action can belong to only one enabled shared policy. PostgreSQL rejects
conflicting membership, including concurrent policy creation and enablement.

Create a policy in the policy administration form, enable shared resource
concurrency, and select each participating action and its parameter path.
Global scope creates an administrator-owned policy. Pack scope identifies its
owning pack, not the set of actions it can restrict. Shared policies cannot
contain standard concurrency, rate-limit, or quota controls themselves.

Pack policies use the existing `policies/` directory:

```yaml
ref: salesforce_org_concurrency
name: Salesforce organization concurrency
description: Serialize metadata changes for each organization.
enabled: true
resource_concurrency:
  namespace: salesforce_org
  limit: 1
  method: enqueue
  participants:
    - action: salesforce.deploy_metadata
      resource_parameter: org_id
    - action: salesforce.update_configuration
      resource_parameter: organization_id
    - action: salesforce.validate_metadata
      resource_parameter: org_id
```

The policy API accepts the same `resource_concurrency` object on create and
update. API creation also requires the usual `scope`, for example
`{"type":"global"}`. Participants use fully qualified action refs. Enabled
participants must already exist.

## Resource identity

Paths traverse objects in the flat execution parameter map. For example,
`target.org_id` reads `{"target":{"org_id":"example"}}`. There is no
`parameters` wrapper, array indexing, expression evaluation, or composite key.

Identifiers must be nonempty strings, integers, or booleans. Missing values,
nulls, arrays, objects, whitespace-only strings, and fractional numbers cause
policy cancellation with a diagnostic naming the policy and parameter path.
Strings preserve their exact bytes and case. Integer `1` and string `"1"` are
different identifiers. Different participant parameter names do not affect
identity.

A group is identified by the database policy ID and the SHA-256 digest of the
scalar's JSON representation. The namespace describes the pool; two separately
created policies with the same namespace do not share capacity. Logs and stored
reservation keys contain the digest, not the original parameter value. Hashing
is not encryption and does not protect low-entropy values against guessing.

## Admission and lifecycle

The executor uses the existing PostgreSQL admission repository. A short
transaction locks the execution and shared policy row, checks shared capacity
and FIFO position, then checks the existing effective standard concurrency
constraint. It commits capacity for both only when both checks succeed. Shared
occupancy never depends on a process-local counter. Locking is per policy, so
admission transactions for different resources briefly serialize, but their
running executions do not block one another.

Admission resolves participant paths after acquiring the policy lock. If an
edit moves the action to another policy while admission waits, it restarts and
locks the new policy before resolving the resource identifier.

`execution_resource_admission` associates each active reservation or waiting
request with one execution. Its execution primary key makes duplicate messages
idempotent. Queued requests consume no shared capacity. Standard completion and
maintenance promotion cannot independently activate shared waiters.

With `enqueue`, executions remain `requested` and keep their FIFO position.
The supervisor's existing requested-execution recovery republishes eligible
waiters after its configured recovery grace period. It checks a readiness hint
for shared capacity and any queued standard constraint. The executor repeats
the actual admission checks under locks. Eligible shared waiters remain
recoverable even after the ordinary reschedule attempt budget is exhausted.
This also provides recovery after executor or broker-publication interruption.
Recovery requires the supervisor and its MQ publisher to be enabled.

With `cancel`, capacity exhaustion follows the existing policy-violation
cancellation path. The diagnostic includes the shared policy reference and
hashed group. A denied shared admission does not acquire a standard slot, and
a deferred standard admission does not acquire a shared slot.

A database execution-state trigger removes the reservation on `completed`,
`failed`, `cancelled`, `timeout`, or `abandoned`. `canceling` retains capacity.
Repeated terminal updates cannot release a second reservation. Execution
deletion also removes its reservation. Database logs record terminal releases;
executor logs record shared admission decisions.

Workflow child actions acquire their own reservations through normal
scheduling. A workflow parent acquires none unless its action explicitly
participates. A startup error that returns a parent from `scheduling` to
`requested` releases both concurrency reservations in the same transaction.
Once the parent is `running`, startup/dispatch errors retain capacity because
child operations may already be in flight. Its reservation then lasts until
terminal. Avoid assigning a parent and its children the same resource
with a limit that prevents the children from running.

## Policy changes and operational limits

- Disabling or retiring a policy removes enabled participant membership. New
  executions are unrestricted by that shared policy; existing standard policies
  still apply. Waiting requests drop the shared constraint on their next
  scheduling attempt. Already admitted executions retain their original policy
  and reservation until terminal.
- Creating or enabling a policy does not retroactively reserve capacity for
  executions already dispatched without that policy. Drain existing operations
  before enabling a new restriction if the external resource requires it.
- Resource settings cannot change while the policy has active or waiting
  requests. Drain or cancel those executions before editing. Identity fields,
  descriptions, and enabled state can still change.
- Deleting a policy with active or waiting requests fails through a foreign-key
  restriction. Disable it and let outstanding executions drain before deletion.
  Reservations never disappear merely because a policy was disabled.
- A reservation is admission capacity, not an external-operation fencing token.
  Worker loss can leave an operation running outside Attune. Marking an execution
  abandoned releases its capacity but does not prove that Salesforce or another
  external operation stopped. Retrying can repeat side effects. This feature
  does not provide exactly-once execution.

Run the PostgreSQL repository tests with the owned integration runner:

```bash
bash scripts/run-rust-integration-tests.sh --test shared_resource_concurrency_tests
```
