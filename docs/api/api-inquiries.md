# Inquiry Management API

The Inquiry Management API provides endpoints for managing human-in-the-loop interactions within Attune workflows. Inquiries allow executions to pause and request input from users before continuing, enabling approval workflows, data collection, and interactive automation.

## Table of Contents

- [Overview](#overview)
- [Inquiry Model](#inquiry-model)
- [Authentication](#authentication)
- [Endpoints](#endpoints)
  - [List Inquiries](#list-inquiries)
  - [Get Inquiry by ID](#get-inquiry-by-id)
  - [List Inquiries by Status](#list-inquiries-by-status)
  - [List Inquiries by Execution](#list-inquiries-by-execution)
  - [Create Inquiry](#create-inquiry)
  - [Update Inquiry](#update-inquiry)
  - [Respond to Inquiry](#respond-to-inquiry)
  - [Delete Inquiry](#delete-inquiry)
- [Use Cases](#use-cases)
- [Related Resources](#related-resources)

---

## Overview

Inquiries represent questions or prompts that require human input during workflow execution. They support:

- **Approval Workflows**: Request approval before proceeding with critical actions
- **Data Collection**: Gather additional information from users during execution
- **Interactive Automation**: Enable dynamic workflows that adapt based on user input
- **Assignment**: Direct inquiries to specific users or teams
- **Timeouts**: Automatically expire inquiries after a specified time
- **Schema Validation**: Define expected response formats using JSON Schema

### Key Features

- **Status Tracking**: Monitor inquiry lifecycle (pending, responded, timeout, canceled)
- **Response Validation**: Optionally validate responses against JSON schemas
- **User Assignment**: Assign inquiries to specific users for accountability
- **Timeout Handling**: Automatically handle expired inquiries
- **Execution Integration**: Link inquiries to specific workflow executions

---

## Inquiry Model

### Inquiry Object

```json
{
  "id": 123,
  "execution": 456,
  "prompt": "Approve deployment to production?",
  "response_schema": {
    "type": "object",
    "properties": {
      "approved": {"type": "boolean"},
      "comment": {"type": "string"}
    },
    "required": ["approved"]
  },
  "assigned_to": 789,
  "status": "pending",
  "response": null,
  "timeout_at": "2024-01-15T12:00:00Z",
  "responded_at": null,
  "created": "2024-01-15T10:00:00Z",
  "updated": "2024-01-15T10:00:00Z"
}
```

### Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique inquiry identifier |
| `execution` | integer | ID of the execution this inquiry belongs to |
| `prompt` | string | Question or prompt text displayed to the user |
| `response_schema` | object | Optional JSON Schema defining expected response format |
| `assigned_to` | integer | Optional user ID this inquiry is assigned to |
| `status` | string | Current status: `pending`, `responded`, `timeout`, `canceled` |
| `response` | object | User's response data (null until responded) |
| `timeout_at` | datetime | Optional timestamp when inquiry expires |
| `responded_at` | datetime | Timestamp when user responded (null until responded) |
| `created` | datetime | Timestamp when inquiry was created |
| `updated` | datetime | Timestamp of last update |

### Inquiry Status

| Status | Description |
|--------|-------------|
| `pending` | Inquiry is waiting for a response |
| `responded` | User has provided a response |
| `timeout` | Inquiry expired without receiving a response |
| `canceled` | Inquiry was canceled before completion |

---

## Authentication

All inquiry endpoints require authentication. Include a valid JWT access token in the `Authorization` header:

```
Authorization: Bearer <access_token>
```

See the [Authentication Guide](./authentication.md) for details on obtaining tokens.

---

## Endpoints

### List Inquiries

Retrieve a paginated list of inquiries with optional filtering.

**Endpoint:** `GET /api/v1/inquiries`

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `status` | string | - | Filter by status (`pending`, `responded`, `timeout`, `canceled`) |
| `execution` | integer | - | Filter by execution ID |
| `assigned_to` | integer | - | Filter by assigned user ID |
| `page` | integer | 1 | Page number (1-indexed) |
| `per_page` | integer | 50 | Items per page (max 100) |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/inquiries?status=pending&page=1&per_page=20" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 123,
      "execution": 456,
      "prompt": "Approve deployment to production?",
      "assigned_to": 789,
      "status": "pending",
      "has_response": false,
      "timeout_at": "2024-01-15T12:00:00Z",
      "created": "2024-01-15T10:00:00Z"
    }
  ],
  "meta": {
    "page": 1,
    "page_size": 20,
    "total": 1,
    "total_pages": 1
  }
}
```

---

### Get Inquiry by ID

Retrieve a single inquiry by its ID.

**Endpoint:** `GET /api/v1/inquiries/:id`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Inquiry ID |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/inquiries/123" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "execution": 456,
    "prompt": "Approve deployment to production?",
    "response_schema": {
      "type": "object",
      "properties": {
        "approved": {"type": "boolean"},
        "comment": {"type": "string"}
      },
      "required": ["approved"]
    },
    "assigned_to": 789,
    "status": "pending",
    "response": null,
    "timeout_at": "2024-01-15T12:00:00Z",
    "responded_at": null,
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:00:00Z"
  }
}
```

**Error Responses:**

- `404 Not Found`: Inquiry not found

---

### List Inquiries by Status

Retrieve inquiries filtered by status.

**Endpoint:** `GET /api/v1/inquiries/status/:status`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `status` | string | Status filter: `pending`, `responded`, `timeout`, `canceled` |

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `page` | integer | 1 | Page number |
| `page_size` | integer | 50 | Items per page |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/inquiries/status/pending" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 123,
      "execution": 456,
      "prompt": "Approve deployment to production?",
      "assigned_to": 789,
      "status": "pending",
      "has_response": false,
      "timeout_at": "2024-01-15T12:00:00Z",
      "created": "2024-01-15T10:00:00Z"
    }
  ],
  "meta": {
    "page": 1,
    "page_size": 50,
    "total": 1,
    "total_pages": 1
  }
}
```

**Error Responses:**

- `400 Bad Request`: Invalid status value

---

### List Inquiries by Execution

Retrieve all inquiries associated with a specific execution.

**Endpoint:** `GET /api/v1/executions/:execution_id/inquiries`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `execution_id` | integer | Execution ID |

**Query Parameters:**

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `page` | integer | 1 | Page number |
| `page_size` | integer | 50 | Items per page |

**Example Request:**

```bash
curl -X GET "http://localhost:8080/api/v1/executions/456/inquiries" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 123,
      "execution": 456,
      "prompt": "Approve deployment to production?",
      "assigned_to": 789,
      "status": "pending",
      "has_response": false,
      "timeout_at": "2024-01-15T12:00:00Z",
      "created": "2024-01-15T10:00:00Z"
    }
  ],
  "meta": {
    "page": 1,
    "page_size": 50,
    "total": 1,
    "total_pages": 1
  }
}
```

**Error Responses:**

- `404 Not Found`: Execution not found

---

### Create inquiry

Workflow task actions create inquiries with an execution token.

**Endpoint:** `POST /api/v1/inquiries`

```json
{
  "purpose": "production-approval",
  "prompt": "Approve deployment to production?",
  "response_schema": {
    "approved": {"type": "boolean", "required": true},
    "comment": {"type": "string"}
  },
  "assigned_to": 789,
  "timeout_seconds": 3600
}
```

Attune derives the execution and workflow scope from the token. The response contains an internal inquiry ID and a provider-neutral response handle:

```json
{
  "data": {
    "inquiry": {
      "id": 123,
      "execution": 456,
      "purpose": "production-approval",
      "prompt": "Approve deployment to production?",
      "assigned_to": 789,
      "status": "pending",
      "response": null,
      "timeout_at": "2026-09-19T12:00:00Z"
    },
    "response_handle": "attune_irh_REDACTED"
  },
  "message": "Inquiry created successfully"
}
```

The response includes `Cache-Control: no-store`. Use the numeric ID for workflow waits. Give the opaque handle only to an external adapter. The handle is correlation data and does not replace integration authentication or actor mapping.

Equivalent retries return the same inquiry and may return a different valid handle. A retry with different immutable fields returns `409 Conflict`.

---

### Update Inquiry

Update an existing inquiry's properties.

**Endpoint:** `PUT /api/v1/inquiries/:id`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Inquiry ID |

**Request Body:**

```json
{
  "status": "canceled",
  "assigned_to": 999
}
```

**Updatable Fields:**

| Field | Type | Description |
|-------|------|-------------|
| `status` | string | Update status (any valid status) |
| `response` | object | Manually set response data |
| `assigned_to` | integer | Change assignment |

**Example Request:**

```bash
curl -X PUT "http://localhost:8080/api/v1/inquiries/123" \
  -H "Authorization: Bearer <access_token>" \
  -H "Content-Type: application/json" \
  -d '{
    "status": "canceled"
  }'
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "execution": 456,
    "prompt": "Approve deployment to production?",
    "response_schema": null,
    "assigned_to": null,
    "status": "canceled",
    "response": null,
    "timeout_at": "2024-01-15T12:00:00Z",
    "responded_at": null,
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:05:00Z"
  },
  "message": "Inquiry updated successfully"
}
```

**Error Responses:**

- `404 Not Found`: Inquiry not found
- `400 Bad Request`: Validation error

---

### Respond to Inquiry

Submit a response to a pending inquiry. This is the primary endpoint for users to answer inquiries.

**Endpoint:** `POST /api/v1/inquiries/:id/respond`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Inquiry ID |

**Request Body:**

```json
{
  "response": {
    "approved": true,
    "comment": "Deployment approved for production release v2.1.0"
  }
}
```

**Behavior:**

- Only `pending` inquiries can be responded to
- If inquiry has `assigned_to`, only that user can respond
- If inquiry has timed out, response is rejected
- Automatically sets `status` to `responded`
- Automatically sets `responded_at` timestamp
- Response should conform to `response_schema` if defined (future validation)

**Example Request:**

```bash
curl -X POST "http://localhost:8080/api/v1/inquiries/123/respond" \
  -H "Authorization: Bearer <access_token>" \
  -H "Content-Type: application/json" \
  -d '{
    "response": {
      "approved": true,
      "comment": "Looks good, proceeding with deployment"
    }
  }'
```

**Response:** `200 OK`

```json
{
  "data": {
    "id": 123,
    "execution": 456,
    "prompt": "Approve deployment to production?",
    "response_schema": {
      "type": "object",
      "properties": {
        "approved": {"type": "boolean"},
        "comment": {"type": "string"}
      }
    },
    "assigned_to": 789,
    "status": "responded",
    "response": {
      "approved": true,
      "comment": "Looks good, proceeding with deployment"
    },
    "timeout_at": "2024-01-15T12:00:00Z",
    "responded_at": "2024-01-15T10:30:00Z",
    "created": "2024-01-15T10:00:00Z",
    "updated": "2024-01-15T10:30:00Z"
  },
  "message": "Response submitted successfully"
}
```

**Error Responses:**

- `404 Not Found`: Inquiry not found
- `400 Bad Request`: Inquiry is not in pending status or has timed out
- `403 Forbidden`: User is not authorized to respond (inquiry assigned to someone else)

---

### Submit an external response

An integration adapter verifies its provider callback, then submits the normalized actor and response to Attune.

**Endpoint:** `POST /api/v1/inquiry-responses`

```json
{
  "response_handle": "attune_irh_REDACTED",
  "external_actor": {
    "provider": "chat-provider",
    "tenant": "workspace-id",
    "external_subject": "user-id"
  },
  "response": {
    "approved": true,
    "comment": "Approved in the provider"
  }
}
```

This endpoint accepts only access JWTs minted from integration tokens. Attune rechecks the integration credential and current RBAC grant on every request. It resolves the external actor through the integration's external identity mappings and requires the mapped identity to match `assigned_to`.

The first valid response returns `200 OK`. A response after another response, timeout, cancellation, or terminal workflow returns `409 Conflict`. Invalid or unknown handles return `404 Not Found` without disclosing an inquiry ID.

Provider-specific signature verification, replay checks, payload parsing, and acknowledgment remain in the adapter. Attune does not parse Slack, Discord, or other provider payloads.

---

### Delete Inquiry

Delete an inquiry.

**Endpoint:** `DELETE /api/v1/inquiries/:id`

**Path Parameters:**

| Parameter | Type | Description |
|-----------|------|-------------|
| `id` | integer | Inquiry ID |

**Example Request:**

```bash
curl -X DELETE "http://localhost:8080/api/v1/inquiries/123" \
  -H "Authorization: Bearer <access_token>"
```

**Response:** `200 OK`

```json
{
  "message": "Inquiry deleted successfully",
  "success": true
}
```

**Error Responses:**

- `404 Not Found`: Inquiry not found

---

## Use Cases

### Approval Workflows

Create inquiries to request approval before executing critical actions:

```bash
# Create an approval inquiry
curl -X POST "http://localhost:8080/api/v1/inquiries" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "execution": 456,
    "prompt": "Approve deletion of production database backup?",
    "response_schema": {
      "type": "object",
      "properties": {
        "approved": {"type": "boolean"},
        "reason": {"type": "string"}
      },
      "required": ["approved", "reason"]
    },
    "assigned_to": 789,
    "timeout_at": "2024-01-15T18:00:00Z"
  }'
```

### Data Collection

Gather additional information during workflow execution:

```bash
# Request deployment details
curl -X POST "http://localhost:8080/api/v1/inquiries" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "execution": 456,
    "prompt": "Enter deployment configuration",
    "response_schema": {
      "type": "object",
      "properties": {
        "environment": {"type": "string", "enum": ["staging", "production"]},
        "replicas": {"type": "integer", "minimum": 1, "maximum": 10},
        "rollback_enabled": {"type": "boolean"}
      },
      "required": ["environment", "replicas"]
    }
  }'
```

### Monitoring Pending Inquiries

List all pending inquiries requiring attention:

```bash
# Get all pending inquiries
curl -X GET "http://localhost:8080/api/v1/inquiries?status=pending" \
  -H "Authorization: Bearer <token>"

# Get inquiries assigned to a specific user
curl -X GET "http://localhost:8080/api/v1/inquiries?status=pending&assigned_to=789" \
  -H "Authorization: Bearer <token>"
```

### Responding to Inquiries

Users can respond to assigned inquiries:

```bash
curl -X POST "http://localhost:8080/api/v1/inquiries/123/respond" \
  -H "Authorization: Bearer <token>" \
  -H "Content-Type: application/json" \
  -d '{
    "response": {
      "environment": "production",
      "replicas": 5,
      "rollback_enabled": true
    }
  }'
```

---

## Best Practices

### 1. Use Response Schemas

Define clear response schemas to validate user input:

```json
{
  "response_schema": {
    "type": "object",
    "properties": {
      "approved": {"type": "boolean"},
      "justification": {"type": "string", "minLength": 10}
    },
    "required": ["approved", "justification"]
  }
}
```

### 2. Set Reasonable Timeouts

Always set timeouts to prevent inquiries from blocking workflows indefinitely:

```json
{
  "timeout_at": "2024-01-15T23:59:59Z"
}
```

### 3. Assign to Specific Users

For accountability, assign inquiries to responsible users:

```json
{
  "assigned_to": 789,
  "prompt": "Review and approve security patch deployment"
}
```

### 4. Handle Timeouts Gracefully

Monitor inquiries and handle timeout status appropriately in your workflows.

### 5. Provide Clear Prompts

Write descriptive prompts that clearly explain what information is needed:

```json
{
  "prompt": "The deployment to production requires approval. Review the changes at https://github.com/org/repo/pull/123 and approve or reject."
}
```

---

## Error Handling

### Common Error Codes

| Status Code | Description |
|-------------|-------------|
| `400 Bad Request` | Invalid input or inquiry not in correct state |
| `401 Unauthorized` | Missing or invalid authentication token |
| `403 Forbidden` | User not authorized to respond to inquiry |
| `404 Not Found` | Inquiry or execution not found |
| `422 Unprocessable Entity` | Validation errors |
| `500 Internal Server Error` | Server error |

### Example Error Response

```json
{
  "error": "Cannot respond to inquiry with status 'responded'. Only pending inquiries can be responded to.",
  "status": 400
}
```

---

## Related Resources

- [Execution Management API](./api-executions.md) - Manage workflow executions
- [Action Management API](./api-actions.md) - Define executable actions
- [Rule Management API](./api-rules.md) - Create automation rules
- [Authentication Guide](./authentication.md) - API authentication details

---

## Future Enhancements

### Planned Features

1. **Response Schema Validation**: Automatic validation of responses against JSON Schema
2. **Inquiry Templates**: Reusable inquiry templates for common patterns
3. **Batch Operations**: Respond to multiple inquiries at once
4. **Notification Integration**: Automatic notifications when inquiries are created
5. **Audit Trail**: Detailed logging of inquiry lifecycle events
6. **Custom Actions**: Trigger actions on inquiry state changes
7. **WebSocket Updates**: Real-time inquiry status updates

---

**Last Updated:** 2024-01-15  
**API Version:** v1
