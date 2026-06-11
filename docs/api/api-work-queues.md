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
