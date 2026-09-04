# Notifier WebSocket reference

The notifier forwards PostgreSQL notifications to authenticated WebSocket clients. This page defines the client protocol, subscription filters, delivery behavior, and every notification payload that the notifier can currently send.

The payload definitions come from the current trigger functions in `migrations/` and the rule lifecycle publisher in the API service. Later `CREATE OR REPLACE FUNCTION` migrations take precedence over earlier definitions.

## Connect and authenticate

Connect to `GET /ws` on the notifier service. The default direct address is `ws://localhost:8081/ws`. Use `wss://` when TLS terminates in front of the notifier.

The upgrade requires a JWT. Query-string tokens are not accepted.

- Non-browser clients send `Authorization: Bearer <jwt>`.
- Browser clients request the subprotocols `attune.v1` and `attune.jwt.<jwt>`. The server selects `attune.v1`.
- Access, execution, and sensor tokens are accepted.
- Refresh and worker tokens are rejected.

The server returns HTTP `401` when the token is missing, invalid, expired, or has a disallowed token type. It returns HTTP `500` if it cannot load the identity's roles or permissions. The notifier checks token expiry every 30 seconds after connection and closes an expired connection with code `4401` and reason `token expired`.

Browser connection example:

```javascript
const socket = new WebSocket("wss://attune.example/ws", [
  "attune.v1",
  `attune.jwt.${accessToken}`,
]);
```

Non-browser handshake example:

```http
GET /ws HTTP/1.1
Host: attune.example
Upgrade: websocket
Connection: Upgrade
Authorization: Bearer <jwt>
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: <key>
```

Sources: [`extract_ws_token()` and token validation](../../crates/notifier/src/websocket_server.rs#L280-L358), [`websocket_handler()`](../../crates/notifier/src/websocket_server.rs#L1061-L1176).

## Client messages

New connections have no subscriptions. Send at least one `subscribe` message before expecting notifications.

```json
{"type":"subscribe","filter":"entity_type:execution"}
```

Remove the same filter with `unsubscribe`:

```json
{"type":"unsubscribe","filter":"entity_type:execution"}
```

The application-level ping message is valid, but the server does not send an application-level pong:

```json
{"type":"ping"}
```

WebSocket protocol ping and pong frames are handled by the WebSocket stack. Binary client messages are ignored. Malformed JSON, unknown message types, invalid filters, and denied subscriptions produce an error message without closing the connection:

```json
{"type":"error","message":"Invalid filter 'bad': Invalid filter format: bad"}
```

Subscribe and unsubscribe operations do not produce acknowledgements. Adding the same filter twice or removing a filter that is not present has no effect.

Sources: [client message handling](../../crates/notifier/src/websocket_server.rs#L1382-L1452), [wire message enums](../../crates/notifier/src/websocket_server.rs#L1488-L1522).

## Server messages

The first WebSocket text frame is a welcome message:

```json
{
  "type": "welcome",
  "client_id": "client_1",
  "message": "Connected to Attune Notifier"
}
```

Each delivered notification has this outer envelope:

```typescript
interface Notification<P> {
  type: "notification";
  notification_type: string;
  entity_type: string;
  entity_id: number;
  user_id: number | null;
  payload: P;
  timestamp: string;
}
```

`notification_type` is the PostgreSQL channel name. `entity_type`, `entity_id`, and `user_id` are copied from the PostgreSQL payload. `payload` is the complete PostgreSQL payload. `timestamp` is the UTC time when the notifier parsed the PostgreSQL message, not the database row's creation or update time.

Attune IDs are signed 64-bit integers. The wire format uses JSON numbers, so JavaScript clients must account for values above `Number.MAX_SAFE_INTEGER` if the database can reach that range.

All current production publishers omit `user_id`, so the outer `user_id` is currently `null` for every stream in this catalog.

Sources: [`Notification`](../../crates/notifier/src/service.rs#L16-L36), [PostgreSQL payload parsing](../../crates/notifier/src/postgres_listener.rs#L165-L193), [tagged server messages](../../crates/notifier/src/websocket_server.rs#L1488-L1501).

## Subscription filters

Filters on one connection are ORed. A notification that matches any filter becomes a delivery candidate. Authorization checks still run after filter matching.

| Filter | Matches | Recommended use |
| --- | --- | --- |
| `all` | Every notification | Administrative clients that need every stream |
| `entity_type:<type>` | One entity family | Lists and dashboards, such as `entity_type:execution` |
| `entity:<type>:<id>` | One entity ID | Detail views, such as `entity:execution:42` |
| `notification_type:<type>` | One exact channel | State-specific handlers, such as `notification_type:inquiry_created` |
| `trigger_ref:<ref>` | Rule lifecycle messages for one trigger ref | Managed sensors, such as `trigger_ref:core.intervaltimer` |
| `user:<id>` | Messages whose top-level payload has that `user_id` | Reserved. No current publisher emits `user_id` |

Filter names and values are case-sensitive. Entity IDs and user IDs parse as signed 64-bit integers.

Sensor tokens may subscribe only with `trigger_ref:<ref>`, and the ref must appear in the token's `trigger_types` claim. Non-sensor clients need permission for mapped resource types. An `all` subscription requires the `admin` role or read access to events, enforcements, executions, and rules. The notifier applies a second entity visibility check before delivery, so acceptance of a subscription does not guarantee delivery of every matching message.

Delivery checks vary by entity family:

| Entity family | Delivery check |
| --- | --- |
| `execution` | Execution read grants, ownership constraints, workflow lineage, or a public top-level action for an access token |
| `event` | Event grants and the related rule or trigger visibility |
| `enforcement` | Enforcement grants and the related rule visibility |
| `artifact` | Artifact scope, owner, visibility, and grants; only access and execution tokens qualify |
| `rule_lifecycle` | Rule read grants after the notifier refetches the rule |
| `inquiry`, `work_queue`, `work_queue_item` | No entity-specific delivery check for authenticated non-sensor clients |

The notifier captures roles and grants when the connection opens. Permission changes take effect on a new connection, not an existing one.

Sources: [`SubscriptionFilter::matches()`](../../crates/notifier/src/subscriber_manager.rs#L17-L61), [filter authorization](../../crates/notifier/src/websocket_server.rs#L405-L467), [delivery authorization](../../crates/notifier/src/websocket_server.rs#L701-L730), [filter parser](../../crates/notifier/src/websocket_server.rs#L1454-L1486).

## Stream catalog

| `notification_type` | `entity_type` | Emitted when | Best broad filter |
| --- | --- | --- | --- |
| `execution_created` | `execution` | An execution row is inserted | `entity_type:execution` |
| `execution_status_changed` | `execution` | An execution's status changes | `entity_type:execution` |
| `inquiry_created` | `inquiry` | An inquiry row is inserted | `entity_type:inquiry` |
| `inquiry_responded` | `inquiry` | An inquiry enters `responded` | `entity_type:inquiry` |
| `inquiry_timeout` | `inquiry` | An inquiry enters `timeout` | `entity_type:inquiry` |
| `event_created` | `event` | An event row is inserted | `entity_type:event` |
| `enforcement_created` | `enforcement` | An enforcement row is inserted | `entity_type:enforcement` |
| `enforcement_status_changed` | `enforcement` | An enforcement's status changes | `entity_type:enforcement` |
| `workflow_execution_status_changed` | `execution` | A workflow execution's status changes | `notification_type:workflow_execution_status_changed` |
| `artifact_created` | `artifact` | An artifact row is inserted | `entity_type:artifact` |
| `artifact_updated` | `artifact` | An artifact or artifact version is inserted or updated | `entity_type:artifact` |
| `work_queue_created` | `work_queue` | A work queue row is inserted | `entity_type:work_queue` |
| `work_queue_updated` | `work_queue` | A work queue row is updated | `entity_type:work_queue` |
| `work_queue_item_created` | `work_queue_item` | A work queue item row is inserted | `entity_type:work_queue_item` |
| `work_queue_item_updated` | `work_queue_item` | A work queue item row is updated | `entity_type:work_queue_item` |
| `rule_lifecycle_changed` | `rule_lifecycle` | A rule is created, enabled, disabled, or deleted | `notification_type:rule_lifecycle_changed` |

The next sections define `payload`, not the outer notification envelope. Every key shown in a payload type is present. A `null` union means that PostgreSQL emits the key with a JSON null value.

Database enums serialize as strings. Clients should preserve unknown enum values so that adding a database enum value does not break message parsing. PostgreSQL `TIMESTAMPTZ` values and Rust timestamps serialize as timestamp strings.

## Execution streams

Subscribe with `entity_type:execution` for both general execution streams. Subscribe with `entity:execution:<id>` for one execution, or use an exact `notification_type` filter.

`execution_created` has this full payload:

```typescript
interface ExecutionCreatedPayload {
  entity_type: "execution";
  entity_id: number;
  id: number;
  action_id: number | null;
  action_ref: string;
  status: string;
  trace_tag: string | null;
  enforcement: number | null;
  rule_ref: string | null;
  trigger_ref: string | null;
  parent: number | null;
  started_at: string | null;
  workflow_task: unknown | null;
  created: string;
  updated: string;
  auth_mode: "full";
}
```

`execution_status_changed` adds `old_status: string` to the full payload.

If a full payload exceeds 7,000 bytes, the trigger emits this compact form:

```typescript
interface CompactExecutionPayload {
  entity_type: "execution";
  entity_id: number;
  id: number;
  status: string;
  old_status?: string;
  trace_tag: string | null;
  auth_mode: "deferred";
}
```

`old_status` is present only for `execution_status_changed`. When `auth_mode` is `deferred`, fetch the execution through the API before using fields omitted from the compact payload.

If the execution has an enforcement, the trigger looks up `rule_ref` and `trigger_ref` from that enforcement. Either field can still be null if the referenced enforcement does not exist.

Sources: [current execution payload functions](../../migrations/20260902000001_execution_notification_trace_tag.sql#L6-L106), [trigger attachments](../../migrations/20250101000005_execution_and_operations.sql#L304-L312).

## Inquiry streams

Subscribe with `entity_type:inquiry` for all inquiry lifecycle messages.

```typescript
interface InquiryCreatedPayload {
  entity_type: "inquiry";
  entity_id: number;
  id: number;
  execution: number;
  status: string;
  timeout_at: string | null;
  created: string;
}

interface InquiryRespondedPayload {
  entity_type: "inquiry";
  entity_id: number;
  id: number;
  execution: number;
  status: "responded";
  updated: string;
}

interface InquiryTimeoutPayload {
  entity_type: "inquiry";
  entity_id: number;
  id: number;
  execution: number;
  status: "timeout";
  timeout_at: string | null;
  updated: string;
}
```

The payloads do not contain the inquiry schema, response, assignee, or a recipient `user_id`. Fetch the inquiry through the API for those fields. These streams have no compact fallback.

Source: [inquiry trigger functions and attachments](../../migrations/20250101000005_execution_and_operations.sql#L317-L400).

## Event stream

Subscribe with `entity_type:event`, `entity:event:<id>`, or `notification_type:event_created`.

```typescript
interface EventCreatedPayload {
  entity_type: "event";
  entity_id: number;
  id: number;
  trigger: number | null;
  trigger_ref: string;
  source: number | null;
  source_ref: string | null;
  rule: number | null;
  rule_ref: string | null;
  trace_tag: string | null;
  has_payload: boolean;
  created: string;
  auth_mode: "full";
}
```

The notification never includes the event's payload. `has_payload` reports whether the PostgreSQL column is SQL null. Fetch the event through the API to read its payload.

The compact form is:

```typescript
interface CompactEventCreatedPayload {
  entity_type: "event";
  entity_id: number;
  id: number;
  trigger_ref: string;
  rule_ref: string | null;
  trace_tag: string | null;
  auth_mode: "deferred";
}
```

Sources: [current event payload function](../../migrations/20260902000002_related_notification_trace_tags.sql#L5-L41), [trigger attachment](../../migrations/20250101000004_trigger_sensor_event_rule.sql#L430-L433).

## Enforcement streams

Subscribe with `entity_type:enforcement` for both enforcement streams.

```typescript
interface EnforcementCreatedPayload {
  entity_type: "enforcement";
  entity_id: number;
  id: number;
  rule: number | null;
  rule_ref: string;
  trigger_ref: string;
  event: number | null;
  status: string;
  trace_tag: string | null;
  condition: string;
  created: string;
  resolved_at: string | null;
  auth_mode: "full";
}
```

`enforcement_status_changed` adds `old_status: string` to the full payload.

The compact form keeps `entity_type`, `entity_id`, `id`, `rule`, `rule_ref`, `trigger_ref`, `event`, `status`, `trace_tag`, and `auth_mode: "deferred"`. It also keeps `old_status` for `enforcement_status_changed`.

The trigger takes `trace_tag` from the first related execution that has one. If none exists, it uses the related event's trace tag. An enforcement creation notification usually arrives before its execution exists, so the event is normally the source.

Sources: [current enforcement payload functions](../../migrations/20260902000002_related_notification_trace_tags.sql#L43-L159), [trigger attachments](../../migrations/20250101000004_trigger_sensor_event_rule.sql#L462-L500).

## Workflow execution stream

Use `notification_type:workflow_execution_status_changed` when a client needs to distinguish workflow status changes from ordinary execution status changes.

```typescript
interface WorkflowExecutionStatusChangedPayload {
  entity_type: "execution";
  entity_id: number;
  id: number;
  action_ref: string;
  status: string;
  old_status: string;
  workflow_def: number;
  parent: number | null;
  created: string;
  updated: string;
}
```

Despite the channel name, `entity_type` is `execution`, and all three ID fields identify the execution row. The trigger runs on `execution`, not `workflow_execution`.

One workflow status transition emits both `execution_status_changed` and `workflow_execution_status_changed`. A client subscribed with `entity_type:execution` receives both and must deduplicate if one refresh is enough. This stream has no compact fallback.

Source: [workflow execution trigger function and attachment](../../migrations/20250101000006_workflow_system.sql#L203-L231).

## Artifact streams

Subscribe with `entity_type:artifact` for artifact creation, metadata changes, content version changes, and progress updates.

`artifact_created` has this full payload:

```typescript
interface ArtifactCreatedPayload {
  entity_type: "artifact";
  entity_id: number;
  id: number;
  ref: string;
  type: string;
  visibility: string;
  classification: string;
  name: string | null;
  scope: string;
  owner: string;
  content_type: string | null;
  size_bytes: number | null;
  created: string;
  auth_mode: "full";
}
```

Its compact form keeps `entity_type`, `entity_id`, `id`, `ref`, `type`, `visibility`, `scope`, `owner`, and `auth_mode: "deferred"`.

`artifact_updated` has two producer shapes. An update to the artifact row emits:

```typescript
interface ArtifactUpdatedPayload extends Omit<ArtifactCreatedPayload, "auth_mode"> {
  execution: number | null;
  progress_percent: number | null;
  progress_message: string | null;
  progress_entries: number | null;
  updated: string;
  auth_mode: "full";
}
```

An insert or selected update to an artifact version emits the same fields plus:

```typescript
interface ArtifactVersionUpdatedPayload extends ArtifactUpdatedPayload {
  artifact_version_id: number;
  version: number;
}
```

For the artifact-row producer, `execution` comes from the highest-numbered artifact version. For the version producer, it comes from that version. The outer `entity_id` and payload `id` always identify the parent artifact, not the artifact version.

For progress artifacts backed by a JSON array, `progress_entries` is the array length. The percent and message come from the final array element. The three progress fields are null for other artifact types or data shapes.

Compact artifact update payloads omit `classification`, `name`, `content_type`, `size_bytes`, `progress_message`, and timestamps. They retain the routing fields from compact creation payloads, plus `execution`, `progress_percent`, and `progress_entries`. Version-produced compact payloads also retain `artifact_version_id` and `version`.

An artifact version insert can produce two `artifact_updated` messages: one for the version and one caused by the retention trigger updating parent metadata. Use `artifact_version_id` to recognize the version-produced shape. Consumers should tolerate duplicate refresh signals.

Sources: [current artifact payload functions](../../migrations/20250101000020_notification_payload_envelope.sql#L273-L461), [artifact trigger attachments](../../migrations/20250101000007_supporting_systems.sql#L323-L449).

## Work queue streams

Subscribe with `entity_type:work_queue` for queue definitions and `entity_type:work_queue_item` for queued work.

Both `work_queue_created` and `work_queue_updated` have this payload:

```typescript
interface WorkQueuePayload {
  entity_type: "work_queue";
  entity_id: number;
  id: number;
  ref: string;
  pack_ref: string | null;
  is_adhoc: boolean;
  label: string;
  enabled: boolean;
  dispatch_action_ref: string;
  default_priority: number;
  allow_pending_update: boolean;
  update_strategy: string;
  batch_mode: string;
  created: string;
  updated: string;
}
```

Every queue row update emits `work_queue_updated`, even if the update does not change a value. The payload has no old values.

`work_queue_item_created` has this payload:

```typescript
interface WorkQueueItemCreatedPayload {
  entity_type: "work_queue_item";
  entity_id: number;
  id: number;
  queue: number;
  queue_ref: string;
  item_key: string | null;
  priority: number;
  status: string;
  trace_tag: string | null;
  enqueue_source: string;
  requested_by_identity: number | null;
  requested_by_execution: number | null;
  requested_by_enforcement: number | null;
  leased_execution: number | null;
  lease_expires_at: string | null;
  attempt_count: number;
  created: string;
  updated: string;
}
```

`work_queue_item_updated` adds `old_status: string`. It emits on every item row update, and `old_status` is present even when the status did not change.

`requested_by_identity` is provenance, not a notifier recipient. It does not populate the outer `user_id` and does not make a `user:<id>` filter match.

These four streams have no compact fallback.

Sources: [work queue payload functions and trigger attachments](../../migrations/20250101000012_work_queues.sql#L284-L434), [current work queue item payload functions](../../migrations/20260902000002_related_notification_trace_tags.sql#L161-L226).

## Rule lifecycle stream

Use `trigger_ref:<ref>` for a managed sensor that needs lifecycle messages for one trigger type. Other clients can use `notification_type:rule_lifecycle_changed` or `entity_type:rule_lifecycle` if their rule permissions allow it.

```typescript
interface RuleLifecycleChangedPayload {
  entity_type: "rule_lifecycle";
  entity_id: number;
  event_type: "rule.created" | "rule.enabled" | "rule.disabled" | "rule.deleted";
  active: boolean;
  rule_id: number;
  rule_ref: string;
  trigger_ref: string;
  trigger_params: unknown | null;
  timestamp: string;
  auth_mode: "full";
}
```

The compact form omits `trigger_params` and sets `auth_mode` to `deferred`. The API service publishes this stream directly with `pg_notify`; it does not use a PostgreSQL trigger.

For regular access and execution tokens, the notifier refetches the rule before delivery. A `rule.deleted` message can therefore fail the delivery check after the row has been deleted. Sensor-token delivery checks the payload's `trigger_ref` against the token instead and does not require the rule row.

Sources: [rule lifecycle publisher and payload builder](../../crates/api/src/routes/rule_lifecycle_notifier.rs#L11-L99), [current event type call sites](../../crates/api/src/routes/rules.rs), [sensor delivery authorization](../../crates/notifier/src/websocket_server.rs#L701-L717).

## Compact payloads

PostgreSQL limits a `NOTIFY` payload to 8,000 bytes. Execution, event, enforcement, artifact, and rule lifecycle publishers use a 7,000-byte threshold and replace oversized full payloads with compact payloads.

Treat `payload.auth_mode === "deferred"` as an instruction to fetch the entity through the authenticated API. Do not assume that a field exists merely because it exists in the full type.

Inquiry, workflow execution, work queue, and work queue item publishers do not have a size guard. An oversized payload on one of those channels can fail the database transaction that tries to publish it.

Source: [`_notify_payload_guard()`](../../migrations/20250101000020_notification_payload_envelope.sql#L15-L34).

## Delivery and recovery

The notifier is a live invalidation stream, not a durable event log.

- PostgreSQL delivers `NOTIFY` messages only to listeners connected at commit time.
- A client receives nothing until it subscribes.
- Disconnects have no replay cursor or catch-up operation.
- The in-process broadcast buffer holds 1,000 messages. A lagging broadcaster drops messages and logs the count.
- Subscriptions disappear with the WebSocket connection. Reconnect and resubscribe after a close.
- Several database operations can emit more than one relevant message. Handlers should be idempotent.

Use the REST API as the source of current entity state. A robust client treats notifications as prompts to refetch, especially after reconnecting or receiving `auth_mode: "deferred"`.

Sources: [broadcast channel and lag handling](../../crates/notifier/src/service.rs#L53-L58), [broadcast receive loop](../../crates/notifier/src/service.rs#L122-L155), [empty initial subscription set](../../crates/notifier/src/subscriber_manager.rs#L139-L153).

## The generic notification channel is not client-usable

The notifier listens on `attune_notifications`, but no fixed trigger publishes a compatible payload to that channel. The generic `notification` table trigger publishes to the row's dynamic `channel` value with this shape:

```json
{
  "id": 1,
  "entity_type": "example",
  "entity": "example-ref",
  "activity": "updated"
}
```

That payload lacks the required numeric `entity_id`. The notifier rejects it before WebSocket routing. Dynamic channel values outside the notifier's fixed channel list are not heard at all. Do not build a client subscription around `attune_notifications` until the publisher and parser contracts are aligned.

Sources: [generic notification trigger](../../migrations/20250101000005_execution_and_operations.sql#L646-L671), [required common fields](../../crates/notifier/src/postgres_listener.rs#L171-L183), [listened channels](../../crates/notifier/src/postgres_listener.rs#L11-L30).

## How to update this reference

Use the runtime path as the source of truth. A PostgreSQL function in an early migration may have been replaced by a later migration, and a published channel does not reach clients unless the notifier listens to it and accepts its payload.

### Rebuild the stream inventory

1. Read `NOTIFICATION_CHANNELS` in `crates/notifier/src/postgres_listener.rs`. This is the complete set of PostgreSQL channels the notifier receives.
2. Search migrations and Rust publishers for every `pg_notify` call.
3. Match each published channel to the listener list. Record published but unlisted channels and listed channels without a compatible publisher.
4. Find the trigger attached to each publisher function. The trigger operation and `WHEN` clause define when the stream emits.
5. For functions replaced with `CREATE OR REPLACE FUNCTION`, compare migrations in filename order. The last applied definition is the effective payload builder.

Useful searches:

```bash
rg -n "pg_notify|NOTIFY" migrations crates --glob '*.sql' --glob '*.rs'
rg -n "CREATE (OR REPLACE )?FUNCTION notify_|CREATE TRIGGER .*notify" migrations
rg -n "NOTIFICATION_CHANNELS|listen_all" crates/notifier/src
```

### Rebuild each payload definition

1. Copy field names from the effective `json_build_object` call or Rust JSON builder. Do not infer fields from database models or API responses.
2. Check the underlying column or Rust type for number, string, boolean, JSON, timestamp, and nullability.
3. Remember that `json_build_object` keeps keys whose values are SQL null. Represent those fields as `T | null`, not optional fields.
4. Find `_notify_payload_guard` calls and compare the full and compact builders. A compact payload omits fields rather than setting them to null.
5. Check direct Rust publishers separately. `rule_lifecycle_changed` does not come from a database trigger.
6. Confirm whether more than one producer writes the same channel. `artifact_updated` is the current example.

### Recheck the WebSocket contract

Inspect these symbols when authentication, filters, or envelopes change:

- `extract_ws_token()` and `verify_ws_token()` for handshake authentication
- `ServerMessage` for client-to-server messages
- `ClientMessage` and `Notification` for server-to-client messages
- `parse_subscription_filter()` and `SubscriptionFilter::matches()` for filter syntax and matching
- `filter_allowed_for_identity()` for subscription authorization
- `notification_allowed_for_identity()` for delivery authorization

Useful searches:

```bash
rg -n "enum ClientMessage|enum ServerMessage|parse_subscription_filter" crates/notifier/src
rg -n "filter_allowed_for_identity|notification_allowed_for_identity" crates/notifier/src
rg -n "struct Notification" crates/notifier/src
```

### Verify the update

1. Ensure every channel in `NOTIFICATION_CHANNELS` appears in either the usable stream catalog or the incompatible-channel section.
2. Ensure each catalog row links to its effective publisher and trigger attachment.
3. Check local Markdown links and run `git diff --check`.
4. Run the notifier tests:

```bash
cargo test -p attune-notifier
```

If a database-backed integration test environment is available, cause one real transition for each changed trigger and inspect the WebSocket frame. That catches differences between inferred SQL serialization and the actual JSON sent by PostgreSQL.
