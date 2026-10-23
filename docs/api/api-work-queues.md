# Work Queue API

## Definition and item permissions

Queue definition metadata and queued business items use separate RBAC resources:

- `queues:read/create/update/delete` controls queue definitions.
- `queue_items:read/create/update/delete` controls listing, enqueuing, updating, and deleting items inside a queue.

Creating a work queue item requires a `queue_items:create` permission grant for the target queue. Item endpoints authorize against the queue, so grant constraints follow the same resource-scope semantics used elsewhere in RBAC:

| Scope | Grant shape | Meaning |
|-------|-------------|---------|
| Queue-scoped | `{"resource":"queue_items","actions":["create"],"constraints":{"refs":["ops.review"]}}` | Can enqueue items only in queue `ops.review`. |
| Queue ID-scoped | `{"resource":"queue_items","actions":["create"],"constraints":{"ids":[42]}}` | Can enqueue items only in queue id `42`. |
| Pack-scoped | `{"resource":"queue_items","actions":["create"],"constraints":{"pack_refs":["ops"]}}` | Can enqueue items in any queue owned by pack `ops`. |
| System-scoped | `{"resource":"queue_items","actions":["create"]}` | Can enqueue items in public queues. Private/restricted queues need a constrained item grant, execution pack context, or queue-management override. |

Use the most specific grant possible. For pack-owned queues, prefer `refs` when granting access to a single queue and `pack_refs` when granting access to all queues in a pack. Omitting constraints grants access across all queues.

## Queue reference visibility

Work queues have `reference_visibility` and `reference_allowed_pack_refs` fields:

- `public` (default): any pack may target the queue; broad `queue_items:*` grants can operate on items.
- `private`: only the queue's own pack may target it; direct API item operations require a constrained `queue_items:*` grant or queue-management access.
- `restricted`: the queue's own pack and `reference_allowed_pack_refs` may target it; direct API item operations still require a constrained item grant or queue-management access.

List/detail endpoints accept `referencing_pack_ref` for discovery, so UIs can show queues usable from a selected pack. Item write endpoints do not trust caller-supplied pack context; execution-scoped calls use server-derived execution pack context, and direct API callers need RBAC grants.

## Batch requester grouping

With `batch_mode: batch`, eligible items aggregate across requesters by default.
To keep each requester's items in separate batches, set
`config.dispatch.separate_requesters: true`. Omission or `false` selects
aggregation. Items without enqueue-time requester attribution form their own group
only when separation is enabled. The grouping ID is immutable and survives human
identity deletion; it grants no runtime or inspection authority. Existing items
whose requester was already deleted remain unattributed. Release ID, digest and
the pinned executable snapshot remain batch boundaries,
independently of requester grouping, priorities, and payload coalescing.

```yaml
batch_mode: batch
config:
  dispatch:
    separate_requesters: false
    batch_size:
      source: literal
      value: 16
```

For API-managed queues, choose **Requester grouping** in the queue form, or use:

```bash
attune queue update ops.inbox --requester-grouping separate
attune queue update ops.inbox --requester-grouping aggregate
attune queue show ops.inbox
```

Pack-managed queues use the same field in their queue YAML. Existing API
restrictions on editing pack-managed definitions still apply. API create/update
accept the field inside `config`; detail inspection returns the stored value.
The CLI preserves other configuration fields when changing this setting.

Grouping does not transfer input authority or permission to inspect private
results and logs. Each item keeps its own requester attribution. The production
dispatcher still requires homogeneous requester authority for Key resolution and
execution API grants until recorded queue admission is connected. The grouping
control does not bypass those checks or complete the managed-pack runtime rollout.

## Dispatch execution permissions

Work queues can also define `permission_set_refs` to control the execution-scoped API token granted to executions dispatched from that queue. Omit or set `permission_set_refs` to `null` to inherit the dispatch action's defaults, set it to `[]` to force no execution API token, or set one or more permission-set refs to grant that exact execution access. API-created queue overrides must be delegable by the caller.

## Selector-based item maintenance

Administrators can preview and bulk-maintain unprocessed queue items with PostgreSQL SQL/JSONPath selectors:

- `POST /api/v1/queues/{ref}/items/query/preview` returns the total mutable match count plus up to 100 items.
- `POST /api/v1/queues/{ref}/items/query/apply` applies `cancel`, `patch_payload`, or `reprioritize` to matching mutable items.

Selectors evaluate against a JSON document with `payload`, `metadata`, `item_key`, `priority`, `status`, `enqueue_source`, and `attempt_count`. Only mutable pending items (`queued` and `retry`) are selected; leased, completed, failed, cancelled, and dead-lettered items are skipped. `patch_payload` uses a static JSON Merge Patch object and validates every resulting payload against the queue `item_schema` before persisting changes.

The API and CLI accept full PostgreSQL SQL/JSONPath. The web UI defaults to a friendlier condition mode where an administrator enters a predicate such as `@.priority == 50 && @.payload.customer == $customer`; the UI sends it as `$ ? (<predicate>)`. Raw JSONPath mode remains available for advanced path-specific selectors.

Example preview request:

```json
{
  "selector": {
    "path": "$.payload.customer_id ? (@ == $customer_id)",
    "vars": { "customer_id": 123 }
  },
  "limit": 100
}
```
