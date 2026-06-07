# Event & Enforcement Query API

The Event & Enforcement Query API provides read-only endpoints for querying events and enforcements in the Attune automation platform. These endpoints enable monitoring of trigger firings (events) and rule activations (enforcements), which are fundamental to understanding automation workflow execution.

## Table of Contents

- [Overview](#overview)
- [Event Model](#event-model)
- [Enforcement Model](#enforcement-model)
- [Authentication](#authentication)
- [Endpoints](#endpoints)
  - [List Events](#list-events)
  - [Get Event by ID](#get-event-by-id)
  - [List Enforcements](#list-enforcements)
  - [Get Enforcement by ID](#get-enforcement-by-id)
- [Use Cases](#use-cases)
- [Related Resources](#related-resources)

---

## Overview

### Events

**Events** represent trigger firings - instances when a trigger condition is met and an event is generated. Events are the starting point of automation workflows, carrying the data payload that flows through the system.

**Key Characteristics:**
- Immutable records of trigger activations
- Contain payload data from the trigger source
- Link to the trigger that generated them
- Can be queried by trigger, source, or time range

### Enforcements

**Enforcements** represent rule activations - instances when a rule matches an event and schedules actions for execution. Enforcements are the bridge between events and executions.

**Key Characteristics:**
- Created when a rule's conditions match an event
- Track the status of rule execution (pending, scheduled, running, completed, failed)
- Link events to the resulting executions
- Store condition evaluation results

### Event Flow

```
Sensor/Trigger → Event → Rule Evaluation → Enforcement → Execution(s)
```

---

## Event Model

### Event Object

```json
{
  "id": 123,
  "trigger": 456,
  "trigger_ref": "core.webhook_received",
  "config": {
    "endpoint": "/webhooks/github",
    "method": "POST"
  },
  "payload": {
    "repository": "attune/platform",
    "action": "push",
    "commit": "abc123"
  },
  "source": 789,
  "source_ref": "github_webhook_sensor",
  "created": "2024-01-15T10:00:00Z",
  "updated": "2024-01-15T10:00:00Z"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique event identifier |
| `trigger` | integer | Optional trigger ID that generated this event |
| `trigger_ref` | string | Reference to the trigger (e.g., "core.webhook_received") |
| `config` | object | Optional configuration data for the event |
| `payload` | object | Event payload data from the trigger source |
| `source` | integer | Optional ID of the sensor that created the event |
| `source_ref` | string | Optional reference to the sensor |
| `created` | datetime | Timestamp when event was created |
| `updated` | datetime | Timestamp of last update |

---

## Enforcement Model

### Enforcement Object

```json
{
  "id": 234,
  "rule": 567,
  "rule_ref": "deploy_on_push",
  "trigger_ref": "core.webhook_received",
  "config": {
    "branch": "main",
    "environment": "production"
  },
  "event": 123,
  "status": "completed",
  "payload": {
    "repository": "attune/platform",
    "commit": "abc123"
  },
  "condition": "passed",
  "conditions": {
    "branch_check": true,
    "approval_check": true
  },
  "created": "2024-01-15T10:00:01Z",
  "updated": "2024-01-15T10:05:00Z"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique enforcement identifier |
| `rule` | integer | Optional rule ID that created this enforcement |
| `rule_ref` | string | Reference to the rule |
| `trigger_ref` | string | Reference to the trigger that fired |
| `config` | object | Optional configuration data |
| `event` | integer | Optional ID of the event that triggered this enforcement |
| `status` | string | Enforcement status (see below) |
| `payload` | object | Data payload for the enforcement |
| `condition` | string | Overall condition result: `passed`, `failed`, `skipped` |
| `conditions` | object | Detailed condition evaluation results |
| `created` | datetime | Timestamp when enforcement was created |
| `updated` | datetime | Timestamp of last update |

### Enforcement Status

| Status | Description |
|--------|-------------|
| `pending` | Enforcement created, waiting to be processed |
| `scheduled` | Actions scheduled for execution |
| `running` | Enforcement is actively running |
| `completed` | All actions completed successfully |
| `failed` | Enforcement failed due to error |
| `cancelled` | Enforcement was cancelled |

### Enforcement Condition

| Condition | Description |
|-----------|-------------|
| `passed` | All rule conditions matched |
| `failed` | One or more conditions failed |
| `skipped` | Enforcement was skipped (e.g., rule disabled) |

---

## Authentication

All event and enforcement endpoints require authentication. Include a valid JWT access token in the `Authorization` header:

```
Authorization: Bearer <access_token>
```

See the [Authentication Guide](./authentication.md) for details on obtaining tokens.

Operational read permissions (`events:read`, `enforcements:read`) return metadata and non-secret JSON values. Fields marked `secret: true` in trigger or action schemas are stored as encrypted side-table values and appear as an `$attune_secret` redaction marker by default. To reveal redacted values, callers must explicitly pass `include_secret_values=true` and hold the matching decrypt permission (`events:decrypt` or `enforcements:decrypt`). Secret disclosures are audited.

---

## Endpoints

### List Events

Retrieve a paginated list of events with optional filtering.

**Endpoint:** `GET /api/v1/events`

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `trigger` | integer | - | Filter by trigger ID |
| `trigger_ref` | string | - | Filter by trigger reference |
| `source` | integer | - | Filter by source ID |
| `page` | integer | 1 | Page number (1-indexed) |
| `per_page` | integer | 50 | Items per page (max 100) |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/events?trigger_ref=core.webhook_received&page=1&per_page=20" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 123,
      "trigger": 456,
      "trigger_ref": "core.webhook_received",
      "source": 789,
      "source_ref": "github_webhook_sensor",
      "has_payload": true,
      "created": "2024-01-15T10:00:00Z"
    }
  ],
  "pagination": {
    "page": 1,
    "page_size": 20,
    "total_items": 1,
    "total_pages": 1
  }
}
```

---

### Get Event by ID

Retrieve a single event by its ID.

**Endpoint:** `GET /api/v1/events/:id`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Event ID |

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `include_secret_values` | boolean | `false` | Reveal redacted payload/config fields. Requires `events:decrypt`. |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/events/123" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "trigger": 456,
    "trigger_ref": "core.webhook_received",
    "config": {
      "endpoint": "/webhooks/github",
      "method": "POST"
    },
    "payload": {
      "repository": "attune/platform",
      "action": "push",
      "token": {"$attune_secret": true, "redacted": true},
      "commit": "abc123",
      "branch": "main"
    },
    "source": 789,
    "source_ref": "github_webhook_sensor",
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:00:00Z"
  }
}
```

**Error Responses:**

- `404 Not Found`: Event not found
- `403 Forbidden`: Missing `events:read`, or missing `events:decrypt` when `include_secret_values=true`

---

### List Enforcements

Retrieve a paginated list of enforcements with optional filtering.

**Endpoint:** `GET /api/v1/enforcements`

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `rule` | integer | - | Filter by rule ID |
| `event` | integer | - | Filter by event ID |
| `status` | string | - | Filter by status (pending, scheduled, running, completed, failed, cancelled) |
| `trigger_ref` | string | - | Filter by trigger reference |
| `page` | integer | 1 | Page number (1-indexed) |
| `per_page` | integer | 50 | Items per page (max 100) |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/enforcements?status=completed&page=1" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 234,
      "rule": 567,
      "rule_ref": "deploy_on_push",
      "trigger_ref": "core.webhook_received",
      "event": 123,
      "status": "completed",
      "condition": "passed",
      "created": "2024-01-15T10:00:01Z"
    }
  ],
  "pagination": {
    "page": 1,
    "page_size": 50,
    "total_items": 1,
    "total_pages": 1
  }
}
```

---

### Get Enforcement by ID

Retrieve a single enforcement by its ID.

**Endpoint:** `GET /api/v1/enforcements/:id`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Enforcement ID |

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `include_secret_values` | boolean | `false` | Reveal redacted enforcement `config` values. Requires `enforcements:decrypt`. The copied event `payload` remains redacted. |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/enforcements/234" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 234,
    "rule": 567,
    "rule_ref": "deploy_on_push",
    "trigger_ref": "core.webhook_received",
    "config": {
      "branch": "main",
      "environment": "production"
    },
    "event": 123,
    "status": "completed",
    "payload": {
      "repository": "attune/platform",
      "commit": "abc123"
    },
    "condition": "passed",
    "conditions": {
      "branch_check": true,
      "approval_check": true,
      "deployment_ready": true
    },
    "created": "2024-01-15T10:00:01Z",
    "updated": "2024-01-15T10:05:00Z"
  }
}
```

**Error Responses:**

- `404 Not Found`: Enforcement not found

---

## Use Cases

### Monitoring Event Flow

Track events as they flow through the system:

```bash
# Get recent events for a specific trigger
curl -X GET "http://localhost:8080/api/v1/events?trigger_ref=core.webhook_received&per_page=10" \
  -H "Authorization: Bearer <token>"

# Get a specific event's details
curl -X GET "http://localhost:8080/api/v1/events/123" \
  -H "Authorization: Bearer <token>"
```

### Tracking Rule Activations

Monitor which rules are being triggered:

```bash
# Get all enforcements for a specific rule
curl -X GET "http://localhost:8080/api/v1/enforcements?rule=567" \
  -H "Authorization: Bearer <token>"

# Get completed enforcements
curl -X GET "http://localhost:8080/api/v1/enforcements?status=completed" \
  -H "Authorization: Bearer <token>"
```

### Debugging Workflow Issues

Investigate why a workflow didn't execute as expected:

```bash
# 1. Find the event
curl -X GET "http://localhost:8080/api/v1/events?trigger_ref=core.webhook_received" \
  -H "Authorization: Bearer <token>"

# 2. Check if enforcement was created for the event
curl -X GET "http://localhost:8080/api/v1/enforcements?event=123" \
  -H "Authorization: Bearer <token>"

# 3. Examine enforcement details and condition evaluation
curl -X GET "http://localhost:8080/api/v1/enforcements/234" \
  -H "Authorization: Bearer <token>"
```

### Auditing System Activity

Audit automation activity over time:

```bash
# Get all enforcements for a specific trigger type
curl -X GET "http://localhost:8080/api/v1/enforcements?trigger_ref=core.webhook_received" \
  -H "Authorization: Bearer <token>"

# Check failed enforcements
curl -X GET "http://localhost:8080/api/v1/enforcements?status=failed" \
  -H "Authorization: Bearer <token>"
```

---

## Event-to-Execution Tracing

To trace the full flow from event to execution:

1. **Find the Event**: Query events by trigger or time range
2. **Find Enforcements**: Query enforcements by event ID
3. **Find Executions**: Query executions by enforcement ID (see [Execution API](./api-executions.md))

**Example Flow:**

```bash
# 1. Get event
EVENT_ID=123

# 2. Get enforcements for this event
curl -X GET "http://localhost:8080/api/v1/enforcements?event=${EVENT_ID}" \
  -H "Authorization: Bearer <token>"

# 3. Get executions for the enforcement (from Execution API)
ENFORCEMENT_ID=234
curl -X GET "http://localhost:8080/api/v1/executions?enforcement=${ENFORCEMENT_ID}" \
  -H "Authorization: Bearer <token>"
```

---

## Best Practices

### 1. Use Filters to Reduce Data Volume

Always filter by specific criteria when possible:

```bash
# Good: Filter by trigger
curl -X GET "http://localhost:8080/api/v1/events?trigger_ref=core.webhook_received"

# Better: Add pagination
curl -X GET "http://localhost:8080/api/v1/events?trigger_ref=core.webhook_received&per_page=20"
```

### 2. Monitor Enforcement Status

Regularly check for failed or stuck enforcements:

```bash
# Check for failed enforcements
curl -X GET "http://localhost:8080/api/v1/enforcements?status=failed"

# Check for long-running enforcements
curl -X GET "http://localhost:8080/api/v1/enforcements?status=running"
```

### 3. Correlate Events and Enforcements

Always correlate events with their enforcements to understand rule behavior:

```bash
# Get event details
curl -X GET "http://localhost:8080/api/v1/events/123"

# Get related enforcements
curl -X GET "http://localhost:8080/api/v1/enforcements?event=123"
```

### 4. Use Pagination for Large Result Sets

Always use pagination when querying large datasets:

```bash
curl -X GET "http://localhost:8080/api/v1/events?page=1&per_page=50"
```

---

## Error Handling

### Common Error Codes

| Status Code | Description |
|-------------|-------------|
| `400 Bad Request` | Invalid query parameters |
| `401 Unauthorized` | Missing or invalid authentication token |
| `404 Not Found` | Event or enforcement not found |
| `500 Internal Server Error` | Server error |

### Example Error Response

```json
{
  "error": "Event with ID 999 not found",
  "status": 404
}
```

---

## Performance Considerations

### Query Optimization

- **Filter at the database level**: Use query parameters like `trigger`, `rule`, `event`, and `status`
- **Limit result sets**: Use `per_page` to control result size
- **Index-aware queries**: Queries by ID, trigger, rule, and status are optimized with database indexes

### Pagination Best Practices

- Default page size: 50 items
- Maximum page size: 100 items
- Use `page` and `per_page` parameters consistently

---

## Related Resources

- [Trigger & Sensor Management API](./api-triggers-sensors.md) - Manage triggers that create events
- [Rule Management API](./api-rules.md) - Manage rules that create enforcements
- [Execution Management API](./api-executions.md) - View executions created by enforcements
- [Authentication Guide](./authentication.md) - API authentication details

---

## Future Enhancements

### Planned Features

1. **Time Range Filtering**: Filter events and enforcements by creation time range
2. **Advanced Search**: Full-text search in event payloads
3. **Aggregation Queries**: Count events/enforcements by trigger/rule/status
4. **Real-time Streaming**: WebSocket support for live event/enforcement updates
5. **Export Capabilities**: Export events and enforcements to CSV/JSON
6. **Event Replay**: Replay events for testing and debugging
7. **Retention Policies**: Automatic archival of old events and enforcements

---

**Last Updated:** 2024-01-13  
**API Version:** v1