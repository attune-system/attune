# Notifier Service

The **Notifier Service** provides real-time notifications to clients via WebSocket connections. It listens for PostgreSQL NOTIFY events and broadcasts them to subscribed WebSocket clients based on their subscription filters.

## Overview

The Notifier Service acts as a bridge between the Attune backend services and frontend clients, enabling real-time updates for:

- **Execution status changes** - When executions start, succeed, fail, or timeout
- **Inquiry creation and responses** - Human-in-the-loop approval workflows
- **Enforcement creation** - When rules are triggered
- **Event generation** - When sensors detect events
- **Workflow execution updates** - Workflow state transitions

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    Notifier Service                          │
│                                                              │
│  ┌──────────────────┐         ┌─────────────────────────┐  │
│  │   PostgreSQL     │         │   Subscriber Manager    │  │
│  │    Listener      │────────▶│  (Client Management)    │  │
│  │ (LISTEN/NOTIFY)  │         └─────────────────────────┘  │
│  └──────────────────┘                      │                │
│           │                                │                │
│           │                                ▼                │
│           │                  ┌─────────────────────────┐   │
│           │                  │   WebSocket Server      │   │
│           │                  │  (HTTP + WS Upgrade)    │   │
│           │                  └─────────────────────────┘   │
│           │                                │                │
│           └────────────────────────────────┘                │
└─────────────────────────────────────────────────────────────┘
                                 │
                ┌────────────────┴────────────────┐
                │                                  │
                ▼                                  ▼
        ┌──────────────┐                  ┌──────────────┐
        │  WebSocket   │                  │  WebSocket   │
        │   Client 1   │                  │   Client 2   │
        └──────────────┘                  └──────────────┘
```

## Components

### 1. PostgreSQL Listener

Connects to PostgreSQL and listens on multiple notification channels:

- `execution_status_changed`
- `execution_created`
- `inquiry_created`
- `inquiry_responded`
- `enforcement_created`
- `event_created`
- `workflow_execution_status_changed`

When a NOTIFY event is received, it parses the payload and broadcasts it to the Subscriber Manager.

**Features:**
- Automatic reconnection on connection loss
- Error handling and retry logic
- Multiple channel subscription
- JSON payload parsing

### 2. Subscriber Manager

Manages WebSocket client connections and their subscriptions.

**Features:**
- Client registration/unregistration
- Subscription filter management
- Notification routing based on filters
- Automatic cleanup of disconnected clients

**Subscription Filters:**
- `all` - Receive all notifications
- `entity_type:TYPE` - Filter by entity type (e.g., `entity_type:execution`)
- `entity:TYPE:ID` - Filter by specific entity (e.g., `entity:execution:123`)
- `user:ID` - Filter by user ID (e.g., `user:456`)
- `notification_type:TYPE` - Filter by notification type (e.g., `notification_type:execution_status_changed`)

### 3. WebSocket Server

HTTP server with WebSocket upgrade support.

**Endpoints:**
- `GET /ws` - WebSocket upgrade endpoint
- `GET /health` - Health check endpoint
- `GET /stats` - Service statistics (connected clients, subscriptions)

**Features:**
- CORS support for cross-origin requests
- Automatic ping/pong for connection keep-alive
- JSON message protocol
- Graceful connection handling

## Usage

### Starting the Service

```bash
# Using default configuration
cargo run --bin attune-notifier

# Using custom configuration file
cargo run --bin attune-notifier -- --config /path/to/config.yaml

# With custom log level
cargo run --bin attune-notifier -- --log-level debug
```

### Configuration

Create a `config.notifier.yaml` file:

```yaml
service_name: attune-notifier
environment: development

database:
  url: postgresql://postgres:postgres@localhost:5432/attune
  max_connections: 10

notifier:
  host: 0.0.0.0
  port: 8081
  max_connections: 10000

log:
  level: info
  format: json
  console: true
```

### Environment Variables

Configuration can be overridden with environment variables:

```bash
# Database URL
export ATTUNE__DATABASE__URL="postgresql://user:pass@host:5432/db"

# Notifier service settings
export ATTUNE__NOTIFIER__HOST="0.0.0.0"
export ATTUNE__NOTIFIER__PORT="8081"
export ATTUNE__NOTIFIER__MAX_CONNECTIONS="10000"

# Log level
export ATTUNE__LOG__LEVEL="debug"
```

## WebSocket Protocol

### Client Connection

Connect to the WebSocket endpoint:

```javascript
const ws = new WebSocket('ws://localhost:8081/ws');

ws.onopen = () => {
  console.log('Connected to Attune Notifier');
};

ws.onmessage = (event) => {
  const message = JSON.parse(event.data);
  console.log('Received message:', message);
};

ws.onerror = (error) => {
  console.error('WebSocket error:', error);
};

ws.onclose = () => {
  console.log('Disconnected from Attune Notifier');
};
```

### Welcome Message

Upon connection, the server sends a welcome message:

```json
{
  "type": "welcome",
  "client_id": "client_1",
  "message": "Connected to Attune Notifier"
}
```

### Subscribing to Notifications

Send a subscribe message:

```javascript
// Subscribe to all notifications
ws.send(JSON.stringify({
  "type": "subscribe",
  "filter": "all"
}));

// Subscribe to execution notifications only
ws.send(JSON.stringify({
  "type": "subscribe",
  "filter": "entity_type:execution"
}));

// Subscribe to a specific execution
ws.send(JSON.stringify({
  "type": "subscribe",
  "filter": "entity:execution:123"
}));

// Subscribe to your user's notifications
ws.send(JSON.stringify({
  "type": "subscribe",
  "filter": "user:456"
}));

// Subscribe to specific notification types
ws.send(JSON.stringify({
  "type": "subscribe",
  "filter": "notification_type:execution_status_changed"
}));
```

### Unsubscribing

Send an unsubscribe message:

```javascript
ws.send(JSON.stringify({
  "type": "unsubscribe",
  "filter": "entity_type:execution"
}));
```

### Receiving Notifications

Notifications are sent as JSON messages:

```json
{
  "notification_type": "execution_status_changed",
  "entity_type": "execution",
  "entity_id": 123,
  "user_id": 456,
  "payload": {
    "entity_type": "execution",
    "entity_id": 123,
    "status": "succeeded",
    "action": "core.echo",
    "result": {"output": "hello world"}
  },
  "timestamp": "2024-01-15T10:30:00Z"
}
```

### Ping/Pong

Keep the connection alive by sending ping messages:

```javascript
// Send ping
ws.send(JSON.stringify({"type": "ping"}));

// Pong is handled automatically by the WebSocket protocol
```

## Message Format

### Client → Server Messages

```typescript
// Subscribe to notifications
{
  "type": "subscribe",
  "filter": string  // Subscription filter string
}

// Unsubscribe from notifications
{
  "type": "unsubscribe",
  "filter": string  // Subscription filter string
}

// Ping
{
  "type": "ping"
}
```

### Server → Client Messages

```typescript
// Welcome message
{
  "type": "welcome",
  "client_id": string,
  "message": string
}

// Notification
{
  "notification_type": string,    // Type of notification
  "entity_type": string,          // Entity type (execution, inquiry, etc.)
  "entity_id": number,            // Entity ID
  "user_id": number | null,       // Optional user ID
  "payload": object,              // Notification payload (varies by type)
  "timestamp": string             // ISO 8601 timestamp
}

// Error (future)
{
  "type": "error",
  "message": string
}
```

## Notification Types

### Execution Status Changed

```json
{
  "notification_type": "execution_status_changed",
  "entity_type": "execution",
  "entity_id": 123,
  "user_id": 456,
  "payload": {
    "entity_type": "execution",
    "entity_id": 123,
    "status": "succeeded",
    "action": "slack.post_message",
    "result": {"message_id": "abc123"}
  },
  "timestamp": "2024-01-15T10:30:00Z"
}
```

### Inquiry Created

```json
{
  "notification_type": "inquiry_created",
  "entity_type": "inquiry",
  "entity_id": 789,
  "user_id": 456,
  "payload": {
    "entity_type": "inquiry",
    "entity_id": 789,
    "execution_id": 123,
    "schema": {
      "type": "object",
      "properties": {
        "approve": {"type": "boolean"}
      }
    },
    "ttl": 3600
  },
  "timestamp": "2024-01-15T10:31:00Z"
}
```

### Workflow Execution Status Changed

```json
{
  "notification_type": "workflow_execution_status_changed",
  "entity_type": "workflow_execution",
  "entity_id": 456,
  "user_id": 123,
  "payload": {
    "entity_type": "workflow_execution",
    "entity_id": 456,
    "workflow_ref": "incident.response",
    "status": "running",
    "current_tasks": ["notify_team", "create_ticket"]
  },
  "timestamp": "2024-01-15T10:32:00Z"
}
```

## Example Client Implementations

### JavaScript/Browser

```javascript
class AttuneNotifier {
  constructor(url) {
    this.url = url;
    this.ws = null;
    this.handlers = new Map();
  }

  connect() {
    this.ws = new WebSocket(this.url);

    this.ws.onopen = () => {
      console.log('Connected to Attune Notifier');
    };

    this.ws.onmessage = (event) => {
      const message = JSON.parse(event.data);
      
      if (message.type === 'welcome') {
        console.log('Welcome:', message.message);
        return;
      }

      // Route notification to handlers
      const type = message.notification_type;
      if (this.handlers.has(type)) {
        this.handlers.get(type)(message);
      }
    };

    this.ws.onerror = (error) => {
      console.error('WebSocket error:', error);
    };

    this.ws.onclose = () => {
      console.log('Disconnected from Attune Notifier');
      // Implement reconnection logic here
    };
  }

  subscribe(filter) {
    this.ws.send(JSON.stringify({
      type: 'subscribe',
      filter: filter
    }));
  }

  unsubscribe(filter) {
    this.ws.send(JSON.stringify({
      type: 'unsubscribe',
      filter: filter
    }));
  }

  on(notificationType, handler) {
    this.handlers.set(notificationType, handler);
  }

  disconnect() {
    if (this.ws) {
      this.ws.close();
    }
  }
}

// Usage
const notifier = new AttuneNotifier('ws://localhost:8081/ws');
notifier.connect();

// Subscribe to execution updates
notifier.subscribe('entity_type:execution');

// Handle execution status changes
notifier.on('execution_status_changed', (notification) => {
  console.log('Execution updated:', notification.payload);
  // Update UI with new execution status
});
```

### Python

```python
import asyncio
import json
import websockets

async def notifier_client():
    uri = "ws://localhost:8081/ws"
    
    async with websockets.connect(uri) as websocket:
        # Wait for welcome message
        welcome = await websocket.recv()
        print(f"Connected: {welcome}")
        
        # Subscribe to execution notifications
        await websocket.send(json.dumps({
            "type": "subscribe",
            "filter": "entity_type:execution"
        }))
        
        # Listen for notifications
        async for message in websocket:
            notification = json.loads(message)
            print(f"Received: {notification['notification_type']}")
            print(f"Payload: {notification['payload']}")

# Run the client
asyncio.run(notifier_client())
```

## Monitoring and Statistics

### Health Check

```bash
curl http://localhost:8081/health
```

Response:
```json
{
  "status": "ok"
}
```

### Service Statistics

```bash
curl http://localhost:8081/stats
```

Response:
```json
{
  "connected_clients": 42,
  "total_subscriptions": 156
}
```

## Testing

### Unit Tests

Run the unit tests:

```bash
cargo test -p attune-notifier
```

All components have comprehensive unit tests:
- PostgreSQL listener notification parsing (4 tests)
- Subscription filter matching (4 tests)
- Subscriber management (6 tests)
- WebSocket message parsing (7 tests)

### Integration Testing

To test the notifier service:

1. **Start PostgreSQL** with the Attune database
2. **Run the notifier service**:
   ```bash
   cargo run --bin attune-notifier -- --log-level debug
   ```
3. **Connect a WebSocket client** (using browser console or tool like `websocat`)
4. **Trigger a notification** from PostgreSQL:
   ```sql
   NOTIFY execution_status_changed, '{"entity_type":"execution","entity_id":123,"status":"succeeded"}';
   ```
5. **Verify the client receives the notification**

### WebSocket Testing Tools

- **websocat**: `websocat ws://localhost:8081/ws`
- **wscat**: `wscat -c ws://localhost:8081/ws`
- **Browser DevTools**: Use the Console to test WebSocket connections

## Production Deployment

### Docker

Create a `Dockerfile`:

```dockerfile
FROM rust:1.75 as builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin attune-notifier

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/attune-notifier /usr/local/bin/
COPY config.notifier.yaml /etc/attune/config.yaml
CMD ["attune-notifier", "--config", "/etc/attune/config.yaml"]
```

### Docker Compose

Add to `docker-compose.yml`:

```yaml
services:
  notifier:
    build:
      context: .
      dockerfile: Dockerfile.notifier
    ports:
      - "8081:8081"
    environment:
      - ATTUNE__DATABASE__URL=postgresql://postgres:postgres@db:5432/attune
      - ATTUNE__NOTIFIER__PORT=8081
      - ATTUNE__LOG__LEVEL=info
    depends_on:
      - db
    restart: unless-stopped
```

### Systemd Service

Create `/etc/systemd/system/attune-notifier.service`:

```ini
[Unit]
Description=Attune Notifier Service
After=network.target postgresql.service

[Service]
Type=simple
User=attune
Group=attune
WorkingDirectory=/opt/attune
ExecStart=/opt/attune/bin/attune-notifier --config /etc/attune/config.notifier.yaml
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable attune-notifier
sudo systemctl start attune-notifier
sudo systemctl status attune-notifier
```

## Scaling Considerations

### Horizontal Scaling (Future Enhancement)

For high-availability deployments with multiple notifier instances:

1. **Use Redis Pub/Sub** for distributed notification broadcasting
2. **Load balance WebSocket connections** using a reverse proxy (nginx, HAProxy)
3. **Sticky sessions** to maintain client connections to the same instance

### Performance Tuning

- **max_connections**: Adjust based on expected concurrent clients
- **PostgreSQL connection pool**: Keep small (10-20 connections)
- **Message buffer sizes**: Tune broadcast channel capacity for high-throughput scenarios

## Troubleshooting

### Clients Not Receiving Notifications

1. **Check client subscriptions**: Ensure filters are correct
2. **Verify PostgreSQL NOTIFY**: Test with `psql` and manual NOTIFY
3. **Check logs**: Set log level to `debug` for detailed information
4. **Network/Firewall**: Ensure WebSocket port (8081) is accessible

### Connection Drops

1. **Implement reconnection logic** in clients
2. **Check network stability**
3. **Monitor PostgreSQL connection** health
4. **Increase ping/pong frequency** for keep-alive

### High Memory Usage

1. **Check number of connected clients**: Use `/stats` endpoint
2. **Limit max_connections** in configuration
3. **Monitor subscription counts**: Too many filters per client
4. **Check for memory leaks**: Monitor over time

## Security Considerations

### WebSocket Authentication

WebSocket upgrades require a valid JWT supplied through the Authorization header or the supported WebSocket subprotocol. The notifier validates the token before accepting subscriptions and applies identity- and token-type-aware filter authorization. Production deployments should additionally use TLS termination, restricted network reachability, and rate limiting.

### TLS/SSL

Use a reverse proxy (nginx, Caddy) for TLS termination:

```nginx
server {
    listen 443 ssl;
    server_name notifier.example.com;

    ssl_certificate /path/to/cert.pem;
    ssl_certificate_key /path/to/key.pem;

    location /ws {
        proxy_pass http://localhost:8081/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## Future Enhancements

- [ ] **Redis Pub/Sub support** for distributed deployments
- [x] **WebSocket authentication** with JWT validation
- [x] **Permission-based filtering** for secure multi-tenancy
- [ ] **Message persistence** for offline clients
- [ ] **Metrics and monitoring** (Prometheus, Grafana)
- [ ] **Admin API** for managing connections and subscriptions
- [ ] **Message acknowledgment** for guaranteed delivery
- [ ] **Binary protocol** for improved performance

## References

- [PostgreSQL LISTEN/NOTIFY Documentation](https://www.postgresql.org/docs/current/sql-notify.html)
- [WebSocket Protocol RFC 6455](https://datatracker.ietf.org/doc/html/rfc6455)
- [Axum WebSocket Guide](https://docs.rs/axum/latest/axum/extract/ws/index.html)
- [Tokio Broadcast Channels](https://docs.rs/tokio/latest/tokio/sync/broadcast/index.html)
