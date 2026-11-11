# Action Management API

This document describes the Action Management API endpoints for the Attune automation platform.

## Overview

Actions are the executable units in Attune that perform specific tasks. Each action belongs to a pack and can have parameters, output schemas, and runtime requirements.

**Base Path:** `/api/v1/actions`

`param_schema` and `out_schema` use Attune's flat per-field format. Nested
`secret: true` annotations in `out_schema` classify structured output paths.
`output_format` selects `text`, `json`, `yaml`, or `jsonl`; `output_policy`
controls stdout/stderr/error independently of input secrecy. These fields are
accepted by action create/update and returned in action responses. See
[action output secrecy](../action-output-secrecy.md) for authoring examples,
policy defaults, and publication constraints.

## Data Model

### Action

```json
{
  "id": 1,
  "ref": "core.http.get",
  "pack": 1,
  "pack_ref": "core",
  "label": "HTTP GET Request",
  "description": "Performs an HTTP GET request to a specified URL",
  "entrypoint": "/actions/http_get.py",
  "runtime": 1,
  "param_schema": {
    "url": { "type": "string", "required": true },
    "headers": { "type": "object" }
  },
  "output_format": "json",
  "output_policy": {},
  "out_schema": {
    "status_code": { "type": "integer" },
    "body": { "type": "string" }
  },
  "created": "2024-01-13T10:00:00Z",
  "updated": "2024-01-13T10:00:00Z"
}
```

### Action Summary (List View)

```json
{
  "id": 1,
  "ref": "core.http.get",
  "pack_ref": "core",
  "label": "HTTP GET Request",
  "description": "Performs an HTTP GET request to a specified URL",
  "entrypoint": "/actions/http_get.py",
  "runtime": 1,
  "created": "2024-01-13T10:00:00Z",
  "updated": "2024-01-13T10:00:00Z"
}
```

## Endpoints

### List All Actions

Retrieve a paginated list of all actions.

**Endpoint:** `GET /api/v1/actions`

**Query Parameters:**
- `page` (integer, optional): Page number (default: 1)
- `per_page` (integer, optional): Items per page (default: 20, max: 100)

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 1,
      "ref": "core.http.get",
      "pack_ref": "core",
      "label": "HTTP GET Request",
      "description": "Performs an HTTP GET request",
      "entrypoint": "/actions/http_get.py",
      "runtime": 1,
      "created": "2024-01-13T10:00:00Z",
      "updated": "2024-01-13T10:00:00Z"
    }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 1,
    "total_pages": 1
  }
}
```

---

### List Actions by Pack

Retrieve all actions belonging to a specific pack.

**Endpoint:** `GET /api/v1/packs/:pack_ref/actions`

**Path Parameters:**
- `pack_ref` (string): Pack reference identifier

**Query Parameters:**
- `page` (integer, optional): Page number (default: 1)
- `per_page` (integer, optional): Items per page (default: 20, max: 100)

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 1,
      "ref": "core.http.get",
      "pack_ref": "core",
      "label": "HTTP GET Request",
      "description": "Performs an HTTP GET request",
      "entrypoint": "/actions/http_get.py",
      "runtime": 1,
      "created": "2024-01-13T10:00:00Z",
      "updated": "2024-01-13T10:00:00Z"
    }
  ],
  "pagination": {
    "page": 1,
    "per_page": 20,
    "total": 1,
    "total_pages": 1
  }
}
```

**Errors:**
- `404 Not Found`: Pack with the specified ref does not exist

---

### Get Action by Reference

Retrieve a single action by its reference identifier.

**Endpoint:** `GET /api/v1/actions/:ref`

**Path Parameters:**
- `ref` (string): Action reference identifier (e.g., "core.http.get")

**Response:** `200 OK`

```json
{
  "data": {
    "id": 1,
    "ref": "core.http.get",
    "pack": 1,
    "pack_ref": "core",
    "label": "HTTP GET Request",
    "description": "Performs an HTTP GET request to a specified URL",
    "entrypoint": "/actions/http_get.py",
    "runtime": 1,
    "param_schema": { ... },
    "out_schema": { ... },
    "created": "2024-01-13T10:00:00Z",
    "updated": "2024-01-13T10:00:00Z"
  }
}
```

**Errors:**
- `404 Not Found`: Action with the specified ref does not exist

---

### Create Action

Create a new action in the system.

**Endpoint:** `POST /api/v1/actions`

**Request Body:**

```json
{
  "ref": "core.http.get",
  "pack_ref": "core",
  "label": "HTTP GET Request",
  "description": "Performs an HTTP GET request to a specified URL",
  "entrypoint": "/actions/http_get.py",
  "runtime": 1,
  "param_schema": {
    "url": { "type": "string", "required": true },
    "headers": { "type": "object" }
  },
  "out_schema": {
    "type": "object",
    "properties": {
      "status_code": { "type": "integer" },
      "body": { "type": "string" }
    }
  }
}
```

**Required Fields:**
- `ref`: Unique reference identifier (alphanumeric, dots, underscores, hyphens)
- `pack_ref`: Reference to the parent pack
- `label`: Human-readable name (1-255 characters)
- `description`: Action description (min 1 character)
- `entrypoint`: Execution entry point (1-1024 characters)

**Optional Fields:**
- `runtime`: Runtime ID for execution environment
- `param_schema`: Flat field map defining input parameters
- `out_schema`: Flat field map defining expected outputs and nested secret annotations
- `output_format`: `text`, `json`, `yaml`, or `jsonl`
- `output_policy`: Optional `stdout` (`public`, `secret`, `sanitized`), `stderr`
  (`public`, `secret`), and `error` (`public`, `secret`) choices

**Response:** `201 Created`

```json
{
  "data": {
    "id": 1,
    "ref": "core.http.get",
    "pack": 1,
    "pack_ref": "core",
    "label": "HTTP GET Request",
    "description": "Performs an HTTP GET request to a specified URL",
    "entrypoint": "/actions/http_get.py",
    "runtime": 1,
    "param_schema": { ... },
    "out_schema": { ... },
    "created": "2024-01-13T10:00:00Z",
    "updated": "2024-01-13T10:00:00Z"
  },
  "message": "Action created successfully"
}
```

**Errors:**
- `400 Bad Request`: Invalid request data or validation failure
- `404 Not Found`: Referenced pack does not exist
- `409 Conflict`: Action with the same ref already exists

---

### Update Action

Update an existing action's properties.

**Endpoint:** `PUT /api/v1/actions/:ref`

**Path Parameters:**
- `ref` (string): Action reference identifier

**Request Body:**

All fields are optional. Only provided fields will be updated.

```json
{
  "label": "Updated HTTP GET Request",
  "description": "Updated description",
  "entrypoint": "/actions/http_get_v2.py",
  "runtime": 2,
  "param_schema": { ... },
  "out_schema": { ... }
}
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 1,
    "ref": "core.http.get",
    "pack": 1,
    "pack_ref": "core",
    "label": "Updated HTTP GET Request",
    "description": "Updated description",
    "entrypoint": "/actions/http_get_v2.py",
    "runtime": 2,
    "param_schema": { ... },
    "out_schema": { ... },
    "created": "2024-01-13T10:00:00Z",
    "updated": "2024-01-13T12:00:00Z"
  },
  "message": "Action updated successfully"
}
```

**Errors:**
- `400 Bad Request`: Invalid request data or validation failure
- `404 Not Found`: Action with the specified ref does not exist

---

### Delete Action

Delete an action from the system.

**Endpoint:** `DELETE /api/v1/actions/:ref`

**Path Parameters:**
- `ref` (string): Action reference identifier

**Response:** `200 OK`

```json
{
  "success": true,
  "message": "Action 'core.http.get' deleted successfully"
}
```

**Errors:**
- `404 Not Found`: Action with the specified ref does not exist

---

### Get Queue Statistics

Retrieve real-time queue statistics for an action's execution queue.

**Endpoint:** `GET /api/v1/actions/:ref/queue-stats`

**Path Parameters:**
- `ref` (string): Action reference identifier

**Response:** `200 OK`

```json
{
  "data": {
    "action_id": 1,
    "action_ref": "core.http.get",
    "queue_length": 5,
    "active_count": 2,
    "max_concurrent": 3,
    "oldest_enqueued_at": "2025-01-27T10:30:00Z",
    "total_enqueued": 1250,
    "total_completed": 1245,
    "last_updated": "2025-01-27T12:45:30Z"
  }
}
```

**Response Fields:**
- `action_id`: Numeric action ID
- `action_ref`: Action reference identifier
- `queue_length`: Number of executions waiting in queue
- `active_count`: Number of currently running executions
- `max_concurrent`: Maximum concurrent executions allowed (from policy)
- `oldest_enqueued_at`: Timestamp of oldest queued execution (null if queue empty)
- `total_enqueued`: Lifetime count of executions enqueued
- `total_completed`: Lifetime count of executions completed
- `last_updated`: Last time statistics were updated

**Response When No Queue Stats Available:** `200 OK`

```json
{
  "data": {
    "action_id": 1,
    "action_ref": "core.http.get",
    "queue_length": 0,
    "active_count": 0,
    "max_concurrent": null,
    "oldest_enqueued_at": null,
    "total_enqueued": 0,
    "total_completed": 0,
    "last_updated": null
  }
}
```

**Errors:**
- `404 Not Found`: Action with the specified ref does not exist

**Use Cases:**
- Monitor action execution queue depth
- Detect stuck or growing queues
- Track execution throughput
- Validate policy enforcement
- Operational dashboards

**Related Documentation:**
- [Queue Architecture](./queue-architecture.md)
- [Policy Enforcement](./executor-service.md#policy-enforcement)

---

## Examples

### Creating a Simple Action

```bash
curl -X POST http://localhost:3000/api/v1/actions \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "mypack.hello_world",
    "pack_ref": "mypack",
    "label": "Hello World",
    "description": "Prints hello world",
    "entrypoint": "/actions/hello.py"
  }'
```

### Creating an Action with Parameter Schema

```bash
curl -X POST http://localhost:3000/api/v1/actions \
  -H "Content-Type: application/json" \
  -d '{
    "ref": "mypack.send_email",
    "pack_ref": "mypack",
    "label": "Send Email",
    "description": "Sends an email message",
    "entrypoint": "/actions/send_email.py",
    "param_schema": {
      "type": "object",
      "properties": {
        "to": { "type": "string", "format": "email" },
        "subject": { "type": "string" },
        "body": { "type": "string" }
      },
      "required": ["to", "subject", "body"]
    }
  }'
```

### Listing Actions for a Pack

```bash
curl http://localhost:3000/api/v1/packs/core/actions
```

### Updating an Action

```bash
curl -X PUT http://localhost:3000/api/v1/actions/mypack.hello_world \
  -H "Content-Type: application/json" \
  -d '{
    "label": "Hello World v2",
    "description": "Updated hello world action"
  }'
```

### Deleting an Action

```bash
curl -X DELETE http://localhost:3000/api/v1/actions/mypack.hello_world
```

### Getting Queue Statistics

```bash
curl http://localhost:3000/api/v1/actions/core.http.get/queue-stats
```

**Example Response:**
```json
{
  "data": {
    "action_id": 1,
    "action_ref": "core.http.get",
    "queue_length": 12,
    "active_count": 5,
    "max_concurrent": 5,
    "oldest_enqueued_at": "2025-01-27T12:40:00Z",
    "total_enqueued": 523,
    "total_completed": 511,
    "last_updated": "2025-01-27T12:45:30Z"
  }
}
```

---

## Validation Rules

### Action Reference (`ref`)
- Must be unique across all actions
- Can contain alphanumeric characters, dots (.), underscores (_), and hyphens (-)
- Typically follows the pattern: `pack_name.action_name`
- Example: `core.http.get`, `aws.ec2.start_instance`

### Pack Reference (`pack_ref`)
- Must reference an existing pack
- The pack must exist before creating actions for it

### Entry Point (`entrypoint`)
- Path or identifier for the executable code
- Can be a file path, module name, function name, etc.
- Format depends on the runtime environment

### Schemas (`param_schema`, `out_schema`)
- Must use the flat per-field value-schema format; raw JSON Schema roots are rejected
- Compiled and pinned at admission; output annotations classify secret result paths
- Helps with auto-generating documentation and UI

---

## Best Practices

1. **Naming Conventions**
   - Use descriptive, hierarchical names: `pack.category.action`
   - Keep names concise but meaningful
   - Use lowercase with dots as separators

2. **Schema Definitions**
   - Always provide `param_schema` for clarity
   - Mark required fields with `required: true` in each field definition
   - Use supported field types and nested schema definitions
   - Document schema fields with descriptions

3. **Entry Points**
   - Use consistent paths relative to the pack root
   - Keep entry points simple and maintainable
   - Consider versioning: `/actions/v1/http_get.py`

4. **Runtime Association**
   - Specify runtime when actions have specific dependencies
   - Null runtime means use default/generic runtime
   - Ensure runtime exists before creating action

5. **Error Handling**
   - Design actions to handle errors gracefully
   - Use output schemas to define error structures
   - Log execution details for debugging

6. **Queue Monitoring**
   - Use `/queue-stats` endpoint to monitor execution queues
   - Alert on high `queue_length` (> 100)
   - Investigate when `oldest_enqueued_at` is old (> 30 minutes)
   - Track completion rate: `total_completed / total_enqueued`

---

## Queue Statistics

The `/queue-stats` endpoint provides real-time visibility into action execution queues.

### Understanding Queue Metrics

- **queue_length**: Executions waiting to run (0 = healthy)
- **active_count**: Executions currently running
- **max_concurrent**: Policy-enforced concurrency limit
- **oldest_enqueued_at**: How long the oldest execution has been waiting
- **total_enqueued/completed**: Lifetime throughput metrics

### Healthy vs Unhealthy Queues

**Healthy:**
- ✅ `queue_length` is 0 or low (< 10)
- ✅ `active_count` ≈ `max_concurrent` during load
- ✅ `oldest_enqueued_at` is recent (< 5 minutes)
- ✅ `total_completed` increases steadily

**Unhealthy:**
- ⚠️ `queue_length` consistently high (> 50)
- ⚠️ `oldest_enqueued_at` is old (> 30 minutes)
- 🚨 Queue not progressing (stats not updating)
- 🚨 `active_count` < `max_concurrent` (workers stuck)

### Monitoring Recommendations

1. **Set up alerts** for high queue depths
2. **Track trends** in `total_enqueued` vs `total_completed`
3. **Investigate spikes** in `queue_length`
4. **Scale workers** when queues consistently fill
5. **Adjust policies** if concurrency limits are too restrictive

For detailed queue architecture and troubleshooting, see [Queue Architecture Documentation](./queue-architecture.md).

---

## Related Documentation

- [Pack Management API](./api-packs.md)
- [Runtime Management API](./api-runtimes.md)
- [Rule Management API](./api-rules.md)
- [Execution API](./api-executions.md)
- [Queue Architecture](./queue-architecture.md)
- [Executor Service](./executor-service.md)

---

**Last Updated:** January 27, 2025