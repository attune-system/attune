# Execution Management API

This document describes the Execution Management API endpoints for the Attune automation platform.

## Overview

Executions represent the runtime instances of actions being performed. The Execution Management API provides observability into the automation system, allowing you to track action executions, monitor their status, analyze results, and understand system activity.

**Base Path:** `/api/v1/executions`

## Data Model

### Execution

```json
{
  "id": 1,
  "action": 5,
  "action_ref": "slack.send_message",
  "config": {
    "channel": "#alerts",
    "message": "Error detected in production"
  },
  "parent": null,
  "enforcement": 3,
  "executor": 1,
  "status": "completed",
  "result": {
    "message_id": "1234567890.123456",
    "success": true
  },
  "created": "2024-01-13T10:00:00Z",
  "updated": "2024-01-13T10:00:05Z"
}
```

### Execution Summary (List View)

```json
{
  "id": 1,
  "action_ref": "slack.send_message",
  "status": "completed",
  "parent": null,
  "enforcement": 3,
  "created": "2024-01-13T10:00:00Z",
  "updated": "2024-01-13T10:00:05Z"
}
```

### Execution Status Values

- **`requested`** - Execution has been requested but not yet scheduled
- **`scheduling`** - Execution is being scheduled to a worker
- **`scheduled`** - Execution has been scheduled and queued
- **`running`** - Execution is currently in progress
- **`completed`** - Execution finished successfully
- **`failed`** - Execution failed with an error
- **`canceling`** - Execution is being cancelled
- **`cancelled`** - Execution was cancelled
- **`timeout`** - Execution exceeded time limit
- **`abandoned`** - Execution was abandoned (worker died, etc.)

## Endpoints

### List All Executions

Retrieve a paginated list of executions with optional filters.

**Endpoint:** `GET /api/v1/executions`

**Query Parameters:**
- `page` (integer, optional): Page number (default: 1)
- `per_page` (integer, optional): Items per page (default: 20, max: 100)
- `status` (string, optional): Filter by execution status
- `action_ref` (string, optional): Filter by action reference
- `pack_name` (string, optional): Filter by pack name
- `result_contains` (string, optional): Search in result JSON (case-insensitive substring match)
- `enforcement` (integer, optional): Filter by enforcement ID
- `parent` (integer, optional): Filter by parent execution ID

**Response:** `200 OK`

```json
{
  "data": [
    {
      "id": 1,
      "action_ref": "slack.send_message",
      "status": "completed",
      "parent": null,
      "enforcement": 3,
      "created": "2024-01-13T10:00:00Z",
      "updated": "2024-01-13T10:00:05Z"
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

**Examples:**

```bash
# List all executions
curl http://localhost:3000/api/v1/executions

# Filter by status
curl http://localhost:3000/api/v1/executions?status=completed

# Filter by action
curl http://localhost:3000/api/v1/executions?action_ref=slack.send_message

# Filter by pack
curl http://localhost:3000/api/v1/executions?pack_name=core

# Search in execution results
curl http://localhost:3000/api/v1/executions?result_contains=error

# Multiple filters with pagination
curl http://localhost:3000/api/v1/executions?pack_name=monitoring&status=failed&result_contains=timeout&page=2&per_page=50
```

---

### Get Execution by ID

Retrieve detailed information about a specific execution.

**Endpoint:** `GET /api/v1/executions/:id`

**Path Parameters:**
- `id` (integer): Execution ID

**Response:** `200 OK`

```json
{
  "data": {
    "id": 1,
    "action": 5,
    "action_ref": "slack.send_message",
    "config": {
      "channel": "#alerts",
      "message": "Error detected in production"
    },
    "parent": null,
    "enforcement": 3,
    "executor": 1,
    "status": "completed",
    "result": {
      "message_id": "1234567890.123456",
      "success": true
    },
    "created": "2024-01-13T10:00:00Z",
    "updated": "2024-01-13T10:00:05Z"
  }
}
```

**Errors:**
- `404 Not Found`: Execution with the specified ID does not exist

---

### List Workflow Cache Iterations

Retrieve safe operational status for native cache iterations belonging to a
workflow execution. The caller must be authorized to read the execution.

**Endpoint:** `GET /api/v1/executions/:id/workflow-cache-iterations`

**Path Parameters:**
- `id` (integer): Top-level workflow execution ID

**Response:** `200 OK`

```json
{
  "data": [
    {
      "task_name": "process_users",
      "namespace_id": 42,
      "generation_id": 73,
      "state": "scanning",
      "scanned_count": 1200,
      "dispatched_count": 48,
      "page_size": 100,
      "batch_size": 25,
      "concurrency": 4,
      "created": "2026-08-05T10:00:00Z",
      "updated": "2026-08-05T10:00:05Z",
      "completed_at": null,
      "error_summary": null
    }
  ]
}
```

The response deliberately omits the scan cursor, cache entries, external IDs,
and values. `error_summary` is bounded. Valid states are `scanning`,
`completed`, `failed`, and `cancelled`.

**Errors:**
- `401 Unauthorized`: Authentication is missing or invalid
- `403 Forbidden`: The execution is not visible to the caller
- `404 Not Found`: Execution with the specified ID does not exist

---

### List Executions by Status

Retrieve all executions with a specific status.

**Endpoint:** `GET /api/v1/executions/status/:status`

**Path Parameters:**
- `status` (string): Execution status (lowercase)
  - Valid values: `requested`, `scheduling`, `scheduled`, `running`, `completed`, `failed`, `canceling`, `cancelled`, `timeout`, `abandoned`

**Query Parameters:**
- `page` (integer, optional): Page number (default: 1)
- `per_page` (integer, optional): Items per page (default: 20, max: 100)

**Response:** `200 OK`

**Examples:**

```bash
# Get all running executions
curl http://localhost:3000/api/v1/executions/status/running

# Get all failed executions
curl http://localhost:3000/api/v1/executions/status/failed

# Get completed executions with pagination
curl http://localhost:3000/api/v1/executions/status/completed?page=1&per_page=50
```

**Errors:**
- `400 Bad Request`: Invalid status value

---

### List Executions by Enforcement

Retrieve all executions that were triggered by a specific rule enforcement.

**Endpoint:** `GET /api/v1/executions/enforcement/:enforcement_id`

**Path Parameters:**
- `enforcement_id` (integer): Enforcement ID

**Query Parameters:**
- `page` (integer, optional): Page number (default: 1)
- `per_page` (integer, optional): Items per page (default: 20, max: 100)

**Response:** `200 OK`

**Example:**

```bash
curl http://localhost:3000/api/v1/executions/enforcement/42
```

---

### Get Execution Statistics

Retrieve aggregate statistics about executions.

**Endpoint:** `GET /api/v1/executions/stats`

**Response:** `200 OK`

```json
{
  "data": {
    "total": 1523,
    "completed": 1420,
    "failed": 45,
    "running": 12,
    "pending": 28,
    "cancelled": 15,
    "timeout": 2,
    "abandoned": 1
  }
}
```

**Description:**
- `total`: Total number of executions (limited to most recent 1000)
- `completed`: Executions that finished successfully
- `failed`: Executions that failed with errors
- `running`: Currently executing
- `pending`: Requested, scheduling, or scheduled
- `cancelled`: Cancelled by user or system
- `timeout`: Exceeded time limit
- `abandoned`: Worker died or lost connection

**Example:**

```bash
curl http://localhost:3000/api/v1/executions/stats
```

---

## Use Cases

### Monitoring Active Executions

Track what's currently running in your automation system:

```bash
# Get all running executions
curl http://localhost:3000/api/v1/executions/status/running

# Get scheduled executions waiting to run
curl http://localhost:3000/api/v1/executions/status/scheduled
```

### Debugging Failed Executions

Investigate failures to understand and fix issues:

```bash
# List all failed executions
curl http://localhost:3000/api/v1/executions/status/failed

# Get details of a specific failed execution
curl http://localhost:3000/api/v1/executions/123

# Filter failures for a specific action
curl http://localhost:3000/api/v1/executions?status=failed&action_ref=aws.ec2.start_instance
```

### Tracking Rule Executions

See what actions were triggered by a specific rule enforcement:

```bash
# Get all executions for an enforcement
curl http://localhost:3000/api/v1/executions/enforcement/42
```

### System Health Monitoring

Monitor overall system health and performance:

```bash
# Get aggregate statistics
curl http://localhost:3000/api/v1/executions/stats

# Check for abandoned executions (potential worker issues)
curl http://localhost:3000/api/v1/executions/status/abandoned

# Monitor timeout rate
curl http://localhost:3000/api/v1/executions/status/timeout
```

### Workflow Tracing

Follow execution chains for complex workflows:

```bash
# Get parent execution
curl http://localhost:3000/api/v1/executions/100

# Find child executions
curl http://localhost:3000/api/v1/executions?parent=100
```

---

## Execution Lifecycle

Understanding the execution lifecycle helps with monitoring and debugging:

```
1. requested   → Action execution requested
2. scheduling  → Finding available worker
3. scheduled   → Assigned to worker, queued [HANDOFF TO WORKER]
4. running     → Currently executing
5. completed   → Finished successfully
   OR
   failed      → Error occurred
   OR
   timeout     → Exceeded time limit
   OR
   cancelled   → User/system cancelled
   OR
   abandoned   → Worker lost
```

### State Ownership Model

Execution state is owned by different services at different lifecycle stages:

**Executor Ownership (Pre-Handoff):**
- `requested` → `scheduling` → `scheduled`
- Executor creates and updates execution records
- Executor selects worker and publishes `execution.scheduled`
- **Handles cancellations/failures BEFORE handoff** (before `execution.scheduled` is published)

**Handoff Point:**
- When `execution.scheduled` message is **published to worker**
- Before handoff: Executor owns and updates state
- After handoff: Worker owns and updates state

**Worker Ownership (Post-Handoff):**
- `running` → `completed` / `failed` / `cancelled` / `timeout` / `abandoned`
- Worker updates execution records directly
- Worker publishes status change notifications
- **Handles cancellations/failures AFTER handoff** (after receiving `execution.scheduled`)
- Worker only owns executions it has received

**Orchestration (Read-Only):**
- Executor receives status change notifications for orchestration
- Triggers workflow children, manages parent-child relationships
- Does NOT update execution state after handoff

### State Transitions

**Normal Flow:**
```
requested → scheduling → scheduled → [HANDOFF] → running → completed
  └─ Executor Updates ─────────┘      └─ Worker Updates ─┘
```

**Failure Flow:**
```
requested → scheduling → scheduled → [HANDOFF] → running → failed
  └─ Executor Updates ─────────┘      └─ Worker Updates ──┘
```

**Cancellation (depends on handoff):**
```
Before handoff:
  requested/scheduling/scheduled → cancelled
  └─ Executor Updates (worker never notified) ──┘

After handoff:
  running → canceling → cancelled
           └─ Worker Updates ──┘
```

**Timeout:**
```
scheduled/running → [HANDOFF] → timeout
                                └─ Worker Updates
```

**Abandonment:**
```
scheduled/running → [HANDOFF] → abandoned
                                └─ Worker Updates
```

**Key Points:**
- Only one service updates each execution stage (no race conditions)
- Handoff occurs when `execution.scheduled` is **published**, not just when status is set to `scheduled`
- If cancelled before handoff: Executor updates (worker never knows execution existed)
- If cancelled after handoff: Worker updates (worker owns execution)
- Worker is authoritative source for execution state after receiving `execution.scheduled`
- Status changes are reflected in real-time via notifications

---

## Data Fields

### Execution Fields

| Field | Type | Description |
|-------|------|-------------|
| `id` | integer | Unique execution identifier |
| `action` | integer | Action ID (null for ad-hoc executions) |
| `action_ref` | string | Action reference identifier |
| `config` | object | Execution configuration/parameters |
| `parent` | integer | Parent execution ID (for nested executions) |
| `enforcement` | integer | Rule enforcement that triggered this execution |
| `executor` | integer | Worker/executor that ran this execution |
| `status` | string | Current execution status |
| `result` | object | Execution result/output |
| `created` | datetime | When execution was created |
| `updated` | datetime | Last update timestamp |

### Config Field

The `config` field contains the parameters passed to the action:

```json
{
  "config": {
    "url": "https://api.example.com",
    "method": "POST",
    "headers": {
      "Authorization": "Bearer token123"
    },
    "body": {
      "message": "Alert!"
    }
  }
}
```

### Result Field

The `result` field contains the output from the action execution:

```json
{
  "result": {
    "status_code": 200,
    "response_body": {
      "success": true,
      "id": "msg_12345"
    },
    "duration_ms": 234
  }
}
```

For failed executions, result typically includes error information:

```json
{
  "result": {
    "error": "Connection timeout",
    "error_code": "ETIMEDOUT",
    "stack_trace": "..."
  }
}
```

---

## Query Patterns

### Time-Based Queries

While not directly supported by the API, you can filter results client-side:

```bash
# Get recent executions (server returns newest first)
curl http://localhost:3000/api/v1/executions?per_page=100

# Then filter by timestamp in your application
```

### Action Performance Analysis

```bash
# Get all executions for a specific action
curl http://localhost:3000/api/v1/executions?action_ref=slack.send_message

# Check success rate
curl http://localhost:3000/api/v1/executions?action_ref=slack.send_message&status=completed
curl http://localhost:3000/api/v1/executions?action_ref=slack.send_message&status=failed
```

### Enforcement Tracing

```bash
# Get the enforcement details first
curl http://localhost:3000/api/v1/enforcements/42

# Then get all executions triggered by it
curl http://localhost:3000/api/v1/executions/enforcement/42
```

---

## Best Practices

### 1. Polling for Status Updates

When waiting for execution completion:

```bash
# Poll every few seconds
while true; do
  status=$(curl -s http://localhost:3000/api/v1/executions/123 | jq -r '.data.status')
  echo "Status: $status"
  if [[ "$status" == "completed" || "$status" == "failed" ]]; then
    break
  fi
  sleep 2
done
```

**Better approach:** Use WebSocket notifications (via Notifier service) instead of polling.

### 2. Monitoring Dashboard

Build a real-time dashboard:

```bash
# Get current statistics
curl http://localhost:3000/api/v1/executions/stats

# Get active executions
curl http://localhost:3000/api/v1/executions/status/running

# Get recent failures
curl http://localhost:3000/api/v1/executions/status/failed?per_page=10
```

### 3. Debugging Workflow

When investigating issues:

1. Check statistics for anomalies
2. Filter by failed status
3. Get execution details
4. Check result field for error messages
5. Trace back to enforcement and rule
6. Check action configuration

### 4. Performance Monitoring

Track execution patterns:

- Monitor average execution duration via `created` and `updated` timestamps
- Track failure rates by status counts
- Identify slow actions by filtering and analyzing durations
- Monitor timeout frequency to adjust limits

### 5. Cleanup and Archival

Executions accumulate over time. Plan for:

- Regular archival of old executions
- Retention policies based on status
- Separate storage for failed executions (debugging)
- Aggregated metrics for long-term analysis

---

## Limitations

### Current Limitations

1. **Read-Only API**: Currently, executions cannot be created or modified via API. They are created by the executor service when rules trigger.

2. **No Direct Cancellation**: Cancellation endpoint not yet implemented. Will be added in future release.

3. **Limited History**: List endpoint returns most recent 1000 executions from repository.

4. **Client-Side Filtering**: Some filters (action_ref, parent) are applied client-side rather than in database.

5. **No Time Range Queries**: Cannot directly query by time range. Results are ordered by creation time (newest first).

### Future Enhancements

Planned improvements:

- **Cancellation**: `POST /api/v1/executions/:id/cancel` - Cancel a running execution
- **Retry**: `POST /api/v1/executions/:id/retry` - Retry a failed execution
- **Database-Level Filtering**: Move all filters to SQL queries for better performance
- **Time Range Queries**: Add `created_after` and `created_before` parameters
- **Aggregations**: More detailed statistics and analytics
- **Bulk Operations**: Cancel multiple executions at once
- **Execution Logs**: Stream execution logs via WebSocket
- **Export**: Export execution history to CSV/JSON

---

## Error Responses

### 404 Not Found

Execution does not exist:

```json
{
  "error": "Execution with ID 999 not found",
  "status": 404
}
```

### 400 Bad Request

Invalid status value:

```json
{
  "error": "Invalid execution status: invalid_status",
  "status": 400
}
```

---

## Integration Examples

### JavaScript/Node.js

```javascript
// Get execution statistics
async function getExecutionStats() {
  const response = await fetch('http://localhost:3000/api/v1/executions/stats');
  const data = await response.json();
  console.log(`Total: ${data.data.total}, Failed: ${data.data.failed}`);
}

// Monitor execution completion
async function waitForExecution(executionId) {
  while (true) {
    const response = await fetch(`http://localhost:3000/api/v1/executions/${executionId}`);
    const data = await response.json();
    
    if (data.data.status === 'completed') {
      console.log('Execution completed!', data.data.result);
      return data.data.result;
    } else if (data.data.status === 'failed') {
      throw new Error(`Execution failed: ${JSON.stringify(data.data.result)}`);
    }
    
    await new Promise(resolve => setTimeout(resolve, 2000));
  }
}
```

### Python

```python
import requests
import time

# Get failed executions
def get_failed_executions():
    response = requests.get('http://localhost:3000/api/v1/executions/status/failed')
    data = response.json()
    return data['data']

# Wait for execution
def wait_for_execution(execution_id, timeout=300):
    start_time = time.time()
    while time.time() - start_time < timeout:
        response = requests.get(f'http://localhost:3000/api/v1/executions/{execution_id}')
        data = response.json()
        status = data['data']['status']
        
        if status == 'completed':
            return data['data']['result']
        elif status == 'failed':
            raise Exception(f"Execution failed: {data['data']['result']}")
        
        time.sleep(2)
    
    raise TimeoutError(f"Execution {execution_id} did not complete within {timeout}s")
```

---

## Related Documentation

- [Action Management API](./api-actions.md)
- [Rule Management API](./api-rules.md)
- [Enforcement API](./api-enforcements.md)
- [Worker Service](./worker-service.md)
- [Executor Service](./executor-service.md)

---

**Last Updated:** January 13, 2026
