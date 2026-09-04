# Notifier service

The notifier service turns PostgreSQL `LISTEN/NOTIFY` messages into authenticated WebSocket notifications. Clients use the stream to invalidate cached API data and react to live state changes.

For the connection protocol, subscription filters, and exact payload for every stream, see the [Notifier WebSocket reference](../api/notifier-websocket.md).

## Data flow

```text
Database trigger or API publisher
  -> PostgreSQL NOTIFY channel
  -> PostgresListener
  -> Tokio broadcast channel
  -> subscription filter
  -> per-identity authorization check
  -> WebSocket text frame
```

`PostgresListener` opens one dedicated PostgreSQL connection and calls `listen_all()` once with the complete channel list. It parses each channel payload as JSON. A message must contain a string `entity_type` and a numeric `entity_id`; malformed messages are logged and discarded.

The subscriber manager first selects connections whose filters match the message. The WebSocket server then checks whether each identity may read the referenced entity. It shares one authorization result among connections with the same authorization snapshot.

Sources: [`PostgresListener`](../../crates/notifier/src/postgres_listener.rs), [`SubscriberManager`](../../crates/notifier/src/subscriber_manager.rs), [WebSocket dispatch](../../crates/notifier/src/websocket_server.rs#L612-L730).

## HTTP endpoints

| Endpoint | Purpose |
| --- | --- |
| `GET /ws` | Authenticated WebSocket upgrade |
| `GET /health` | Returns `{"status":"ok"}` |
| `GET /stats` | Returns current connection and subscription counts |

The WebSocket endpoint accepts access, execution, and sensor JWTs. Browser clients send the JWT through the `attune.jwt.<jwt>` WebSocket subprotocol because the browser WebSocket API cannot set an `Authorization` header. Other clients can use `Authorization: Bearer <jwt>`. The service does not accept tokens in the query string.

Source: [WebSocket routing and authentication](../../crates/notifier/src/websocket_server.rs#L102-L155).

## Delivery model

The service provides best-effort live delivery. It does not persist WebSocket messages, acknowledge notifications, or replay messages after a disconnect. PostgreSQL also does not retain `NOTIFY` messages for disconnected listeners.

Clients should reconnect, restore their subscriptions, and fetch current state through the API. Payloads with `auth_mode: "deferred"` are compact forms of messages that exceeded the publisher's safe payload threshold. Clients must fetch the entity to recover omitted fields.

The notifier's in-process broadcast channel has a capacity of 1,000 messages. If its receiver lags, Tokio drops messages and the service logs the number dropped.

Source: [service orchestration](../../crates/notifier/src/service.rs#L48-L165).

## Authorization model

The notifier captures roles, permission grants, identity attributes, token type, and token expiry at connection time. It checks token expiry every 30 seconds, but permission changes do not alter an existing connection's snapshot. Reconnect after changing an identity's roles or permission sets.

Filter authorization limits what a connection may request. Delivery authorization checks each matching notification against the connected identity. Sensor tokens are narrower: they may subscribe only to `trigger_ref:<ref>` filters listed in the token and receive only matching `rule_lifecycle_changed` messages.

Source: [authorization snapshot and filter checks](../../crates/notifier/src/websocket_server.rs#L168-L252).

## PostgreSQL channels

The notifier listens on the following fixed channels:

```text
attune_notifications
execution_status_changed
execution_created
inquiry_created
inquiry_responded
inquiry_timeout
enforcement_created
enforcement_status_changed
event_created
workflow_execution_status_changed
artifact_created
artifact_updated
work_queue_created
work_queue_updated
work_queue_item_created
work_queue_item_updated
rule_lifecycle_changed
```

The [Notifier WebSocket reference](../api/notifier-websocket.md#stream-catalog) explains when each usable stream fires and gives its payload shape. `attune_notifications` is currently not usable by WebSocket clients because its generic publisher omits the required `entity_id`.

Source: [`NOTIFICATION_CHANNELS`](../../crates/notifier/src/postgres_listener.rs#L11-L30).

## Run and inspect the service

Start the notifier with the repository configuration:

```bash
make run-notifier
```

Check the HTTP endpoints:

```bash
curl http://localhost:8081/health
curl http://localhost:8081/stats
```

Run its unit tests:

```bash
cargo test -p attune-notifier
```

Set the service log level to `debug` to inspect PostgreSQL channel receipt, subscription changes, authorization denials, and WebSocket delivery failures.

## Operational constraints

- Keep the PostgreSQL listener on one `listen_all()` call. Calling `listen()` repeatedly in a loop can leave the listener without all intended subscriptions.
- Keep notification payloads below PostgreSQL's 8,000-byte limit. Guarded publishers switch to a compact payload at 7,000 bytes.
- Treat payloads as invalidation hints. The API remains the source of current state.
- Use TLS at the ingress or reverse proxy so that JWT-bearing WebSocket handshakes use `wss://`.
