# Rule Parameter Mapping

## Overview

Rules in Attune can specify parameters to pass to actions when triggered. These parameters can be:

1. **Static values** - Hard-coded values defined in the rule
2. **Dynamic from event payload** - Values extracted from the event that triggered the rule
3. **Dynamic from pack config** - Values from the pack's configuration

This enables flexible parameter passing without hardcoding values or requiring custom code.

---

## Parameter Mapping Format

Rule `action_params` uses a JSON object where each value can be:

- **Static**: A literal value (string, number, boolean, object, array)
- **Dynamic**: A template string using `{{ }}` syntax to reference runtime values

### Template Syntax

```
{{ source.path.to.value }}
```

**Available Sources:**

- `event.payload.*` - Data from the event payload
- `config.*` - Configuration values from the pack
- `system.*` - System-provided values (timestamp, execution context)

---

## Static Parameter Example

The simplest form - just pass fixed values to the action:

```json
{
  "ref": "slack.notify_on_error",
  "pack_ref": "slack",
  "trigger_ref": "core.error_event",
  "action_ref": "slack.post_message",
  "action_params": {
    "channel": "#alerts",
    "message": "An error occurred in the system",
    "color": "danger"
  }
}
```

When this rule triggers, the action receives exactly these parameters.

---

## Dynamic Parameters from Event Payload

Extract values from the event that triggered the rule.

### Example: Alert with Event Data

**Event Payload:**
```json
{
  "severity": "error",
  "service": "api-gateway",
  "message": "Database connection failed",
  "timestamp": "2024-01-15T10:30:00Z",
  "metadata": {
    "host": "api-01.example.com",
    "error_code": "DB_CONN_TIMEOUT"
  }
}
```

**Rule Definition:**
```json
{
  "ref": "alerts.error_notification",
  "pack_ref": "alerts",
  "trigger_ref": "core.error_event",
  "action_ref": "slack.post_message",
  "action_params": {
    "channel": "#incidents",
    "message": "Error in {{ event.payload.service }}: {{ event.payload.message }}",
    "severity": "{{ event.payload.severity }}",
    "host": "{{ event.payload.metadata.host }}",
    "timestamp": "{{ event.payload.timestamp }}"
  }
}
```

**Resulting Action Parameters:**
```json
{
  "channel": "#incidents",
  "message": "Error in api-gateway: Database connection failed",
  "severity": "error",
  "host": "api-01.example.com",
  "timestamp": "2024-01-15T10:30:00Z"
}
```

---

## Dynamic Parameters from Pack Config

Use configuration values stored at the pack level (useful for API keys, URLs, etc.).

### Example: Using Pack Configuration

**Pack Configuration:**
```json
{
  "ref": "slack",
  "config": {
    "api_token": "xoxb-1234567890-abcdefghijk",
    "default_channel": "#general",
    "webhook_url": "https://hooks.slack.com/services/...",
    "bot_name": "Attune Bot"
  }
}
```

**Rule Definition:**
```json
{
  "ref": "slack.auto_notify",
  "pack_ref": "slack",
  "trigger_ref": "core.notification_event",
  "action_ref": "slack.post_message",
  "action_params": {
    "token": "{{ config.api_token }}",
    "channel": "{{ config.default_channel }}",
    "username": "{{ config.bot_name }}",
    "message": "{{ event.payload.message }}"
  }
}
```

**Benefits:**
- Secrets stored in pack config, not in rules
- Easy to update credentials without changing rules
- Reuse configuration across multiple rules

---

## Mixed Parameters (Static + Dynamic)

Combine static and dynamic values in the same rule:

```json
{
  "ref": "github.create_issue",
  "pack_ref": "github",
  "trigger_ref": "core.error_event",
  "action_ref": "github.create_issue",
  "action_params": {
    "repo": "myorg/myrepo",
    "token": "{{ config.github_token }}",
    "title": "Error: {{ event.payload.message }}",
    "body": "Service {{ event.payload.service }} reported an error at {{ event.payload.timestamp }}",
    "labels": ["bug", "automated"],
    "assignees": ["oncall"]
  }
}
```

---

## Nested Object Access

Access nested properties using dot notation:

```json
{
  "action_params": {
    "user_id": "{{ event.payload.user.id }}",
    "user_name": "{{ event.payload.user.profile.name }}",
    "metadata": {
      "ip_address": "{{ event.payload.request.client_ip }}",
      "user_agent": "{{ event.payload.request.headers.user_agent }}"
    }
  }
}
```

---

## Array Access

Access array elements by index:

```json
{
  "action_params": {
    "first_error": "{{ event.payload.errors.0 }}",
    "primary_tag": "{{ event.payload.tags.0 }}"
  }
}
```

---

## Default Values and Fallbacks

Provide default values when the referenced field doesn't exist:

```json
{
  "action_params": {
    "priority": "{{ event.payload.priority | default: 'medium' }}",
    "assignee": "{{ event.payload.assignee | default: 'unassigned' }}"
  }
}
```

---

## Type Preservation

Template values preserve their JSON types:

```json
{
  "action_params": {
    "count": "{{ event.payload.count }}",          // Number: 42
    "enabled": "{{ event.payload.enabled }}",      // Boolean: true
    "tags": "{{ event.payload.tags }}",            // Array: ["a", "b"]
    "metadata": "{{ event.payload.metadata }}"     // Object: {"key": "value"}
  }
}
```

---

## System Variables

Access system-provided values:

```json
{
  "action_params": {
    "execution_time": "{{ system.timestamp }}",
    "rule_id": "{{ system.rule.id }}",
    "rule_ref": "{{ system.rule.ref }}",
    "event_id": "{{ system.event.id }}",
    "enforcement_id": "{{ system.enforcement.id }}"
  }
}
```

---

## String Interpolation

Embed multiple values in a single string:

```json
{
  "action_params": {
    "message": "User {{ event.payload.user_id }} performed {{ event.payload.action }} at {{ system.timestamp }}",
    "subject": "[{{ event.payload.severity | upper }}] {{ event.payload.service }} Alert"
  }
}
```

---

## Filters (Future Enhancement)

Apply transformations to values:

```json
{
  "action_params": {
    "uppercase_name": "{{ event.payload.name | upper }}",
    "lowercase_email": "{{ event.payload.email | lower }}",
    "formatted_date": "{{ event.payload.timestamp | date: '%Y-%m-%d' }}",
    "truncated": "{{ event.payload.message | truncate: 100 }}"
  }
}
```

**Available Filters:**
- `upper` - Convert to uppercase
- `lower` - Convert to lowercase
- `trim` - Remove whitespace
- `default: <value>` - Use default if null/missing
- `date: <format>` - Format timestamp
- `truncate: <length>` - Truncate string
- `json` - Serialize to JSON string
- `base64` - Base64 encode
- `length` - Get length/count

---

## Real-World Examples

### 1. Webhook to Slack Alert

```json
{
  "ref": "monitoring.webhook_to_slack",
  "pack_ref": "monitoring",
  "trigger_ref": "core.webhook",
  "action_ref": "slack.post_message",
  "action_params": {
    "channel": "{{ config.alert_channel }}",
    "token": "{{ config.slack_token }}",
    "message": "⚠️ Alert from {{ event.payload.source }}: {{ event.payload.message }}",
    "attachments": [
      {
        "color": "{{ event.payload.severity | default: 'warning' }}",
        "fields": [
          {
            "title": "Service",
            "value": "{{ event.payload.service }}",
            "short": true
          },
          {
            "title": "Environment",
            "value": "{{ event.payload.environment | default: 'production' }}",
            "short": true
          }
        ],
        "footer": "Attune Automation",
        "ts": "{{ system.timestamp }}"
      }
    ]
  }
}
```

### 2. Error to Ticket System

```json
{
  "ref": "errors.create_ticket",
  "pack_ref": "errors",
  "trigger_ref": "core.error_event",
  "action_ref": "jira.create_issue",
  "action_params": {
    "project": "{{ config.jira_project }}",
    "auth": {
      "username": "{{ config.jira_username }}",
      "token": "{{ config.jira_token }}"
    },
    "issuetype": "Bug",
    "summary": "[{{ event.payload.severity }}] {{ event.payload.service }}: {{ event.payload.message }}",
    "description": {
      "type": "doc",
      "content": [
        {
          "type": "paragraph",
          "content": [
            {
              "type": "text",
              "text": "Error Details:\n\nService: {{ event.payload.service }}\nHost: {{ event.payload.host }}\nTimestamp: {{ event.payload.timestamp }}\n\nStack Trace:\n{{ event.payload.stack_trace }}"
            }
          ]
        }
      ]
    },
    "priority": "{{ event.payload.priority | default: 'Medium' }}",
    "labels": ["automated", "{{ event.payload.service }}"]
  }
}
```

### 3. Metric Threshold to PagerDuty

```json
{
  "ref": "monitoring.critical_alert",
  "pack_ref": "monitoring",
  "trigger_ref": "metrics.threshold_exceeded",
  "action_ref": "pagerduty.trigger_incident",
  "action_params": {
    "routing_key": "{{ config.pagerduty_routing_key }}",
    "event_action": "trigger",
    "payload": {
      "summary": "{{ event.payload.metric_name }} exceeded threshold on {{ event.payload.host }}",
      "severity": "critical",
      "source": "{{ event.payload.host }}",
      "custom_details": {
        "metric": "{{ event.payload.metric_name }}",
        "current_value": "{{ event.payload.current_value }}",
        "threshold": "{{ event.payload.threshold }}",
        "duration": "{{ event.payload.duration_seconds }}s"
      }
    },
    "dedup_key": "{{ event.payload.host }}_{{ event.payload.metric_name }}"
  }
}
```

### 4. Timer to HTTP Request

```json
{
  "ref": "healthcheck.periodic_ping",
  "pack_ref": "healthcheck",
  "trigger_ref": "core.interval_timer",
  "action_ref": "http.request",
  "action_params": {
    "method": "POST",
    "url": "{{ config.healthcheck_endpoint }}",
    "headers": {
      "Authorization": "Bearer {{ config.api_token }}",
      "Content-Type": "application/json"
    },
    "body": {
      "source": "attune",
      "timestamp": "{{ system.timestamp }}",
      "rule": "{{ system.rule.ref }}"
    },
    "timeout": 30
  }
}
```

---

## Implementation Details

### Template Processing Flow

1. **Rule Evaluation** - When an event matches a rule
2. **Template Extraction** - Identify `{{ }}` patterns in `action_params`
3. **Context Building** - Assemble available data:
   - `event.id` - Event database ID
   - `event.trigger` - Trigger ref that generated the event
   - `event.created` - Event creation timestamp
   - `event.payload` - Event payload data
   - `config` - Pack configuration
   - `system.*` - System-provided values (timestamp, rule info)
4. **Value Resolution** - Extract values from context using dot notation paths
5. **Type Conversion** - Preserve JSON types (string, number, boolean, object, array)
6. **Parameter Assembly** - Build final parameter object
7. **Enforcement Creation** - Store resolved parameters in enforcement config
8. **Execution Creation** - Pass parameters to action execution

### Error Handling

**Missing Values:**
- If a referenced value doesn't exist and no default is provided, use `null`
- Log warning: `"Template variable not found: event.payload.missing_field"`

**Invalid Syntax:**
- If template syntax is invalid, log error and use the raw string
- Log error: `"Invalid template syntax: {{ incomplete"`

**Type Mismatches:**
- Preserve JSON types when possible
- Convert to string as fallback for complex interpolation

---

## Configuration in Pack

Pack configuration should be stored securely and can include:

```json
{
  "ref": "mypack",
  "config": {
    "api_token": "secret-token-here",
    "api_url": "https://api.example.com",
    "default_timeout": 30,
    "retry_attempts": 3,
    "enable_notifications": true,
    "notification_channels": ["#alerts", "#monitoring"]
  }
}
```

**Security Note:** Sensitive values (API keys, tokens, passwords) should be stored in pack config, not in rule definitions, since:
- Pack configs can be encrypted
- Easier to rotate credentials
- Rules can be version controlled without exposing secrets

---

## Best Practices

### 1. Use Pack Config for Secrets
❌ **Bad:**
```json
{
  "action_params": {
    "api_key": "sk_live_abc123xyz789"  // Hardcoded secret
  }
}
```

✅ **Good:**
```json
{
  "action_params": {
    "api_key": "{{ config.api_key }}"
  }
}
```

### 2. Provide Defaults for Optional Fields
```json
{
  "action_params": {
    "priority": "{{ event.payload.priority | default: 'medium' }}",
    "assignee": "{{ event.payload.assignee | default: 'unassigned' }}"
  }
}
```

### 3. Use Descriptive Template Paths
```json
{
  "action_params": {
    "user_email": "{{ event.payload.user.email }}",
    "user_id": "{{ event.payload.user.id }}"
  }
}
```

### 4. Keep Static Values Where Appropriate
If a value never changes, keep it static:
```json
{
  "action_params": {
    "service_name": "my-service",  // Static - never changes
    "error_code": "{{ event.payload.code }}"  // Dynamic - from event
  }
}
```

### 5. Test Your Templates
Create test events with sample payloads to verify your templates extract the correct values.

---

## Testing Parameter Mapping

### 1. Manual Testing via API

Create a test event with known payload:

```bash
curl -X POST http://localhost:8080/api/v1/events \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "trigger_ref": "core.test_event",
    "payload": {
      "message": "Test message",
      "severity": "info",
      "user": {
        "id": 123,
        "name": "Alice"
      }
    }
  }'
```

Check the resulting enforcement and execution to verify parameters were resolved correctly:

```bash
# Check enforcement
curl -X GET http://localhost:8080/api/v1/enforcements/1 \
  -H "Authorization: Bearer $TOKEN"

# Check execution
curl -X GET http://localhost:8080/api/v1/executions/1 \
  -H "Authorization: Bearer $TOKEN"
```

### 2. Validate Parameter Resolution

Look for the resolved parameters in the execution's `config` field:

```json
{
  "id": 1,
  "config": {
    "message": "Test message",       // Resolved from event.payload.message
    "severity": "info",              // Resolved from event.payload.severity
    "user_id": 123,                  // Resolved from event.payload.user.id
    "user_name": "Alice"             // Resolved from event.payload.user.name
    "event_id": 456,                 // Resolved from event.id
    "trigger": "core.test_event"     // Resolved from event.trigger
  }
}
```

---

## Migration Guide

### From Static to Dynamic Parameters

**Before (Static):**
```json
{
  "action_params": {
    "message": "An error occurred"
  }
}
```

**After (Dynamic):**
```json
{
  "action_params": {
    "message": "Error: {{ event.payload.message }}"
  }
}
```

### From Hardcoded Secrets to Pack Config

**Before (Hardcoded):**
```json
{
  "action_params": {
    "api_key": "sk_live_abc123"
  }
}
```

**Steps:**
1. Add secret to pack config
2. Update rule to reference pack config
3. Remove hardcoded value

**After (Secure):**
```json
{
  "action_params": {
    "api_key": "{{ config.api_key }}"
  }
}
```

---

## Troubleshooting

### Templates Not Resolving

**Problem:** Parameters contain literal `{{ ... }}` strings instead of resolved values.

**Solutions:**
1. Check template syntax is correct
2. Verify the referenced path exists in the event payload
3. Check sensor service logs for template resolution errors
4. Use default values for optional fields

### Incorrect Values

**Problem:** Parameters have wrong values.

**Solutions:**
1. Inspect event payload structure: `SELECT payload FROM attune.event WHERE id = X;`
2. Verify the dot notation path matches the payload structure
3. Check for typos in template paths
4. Use system logs to see template resolution details

### Type Conversion Issues

**Problem:** Numbers or booleans become strings.

**Solutions:**
1. Ensure the source value is the correct type in the payload
2. Check if string interpolation is converting types
3. Use direct references without string interpolation for non-string types

---

## Future Enhancements

### 1. Conditional Parameters
```json
{
  "action_params": {
    "channel": "{% if event.payload.severity == 'critical' %}#incidents{% else %}#monitoring{% endif %}"
  }
}
```

### 2. Advanced Filters
- Mathematical operations: `{{ event.payload.value | multiply: 100 }}`
- String manipulation: `{{ event.payload.text | replace: 'old', 'new' }}`
- Array operations: `{{ event.payload.items | join: ', ' }}`

### 3. Custom Functions
```json
{
  "action_params": {
    "timestamp": "{{ now() }}",
    "uuid": "{{ uuid() }}",
    "hash": "{{ hash(event.payload.data) }}"
  }
}
```

### 4. Multi-Source Merging
```json
{
  "action_params": {
    "user": "{{ event.payload.user | merge: config.default_user }}"
  }
}
```

---

## Related Documentation

- [Rule Management API](./api-rules.md)
- [Event Management API](./api-events-enforcements.md)
- [Pack Management API](./api-packs.md)
- [Sensor Service Architecture](./sensor-service.md)
- [Security Best Practices](./security-review-2024-01-02.md)
- [Secrets Management](./secrets-management.md)

---

## Summary

Rule parameter mapping provides a powerful way to:

1. **Decouple rules from data** - Rules reference data locations, not specific values
2. **Reuse pack configuration** - Share credentials and settings across rules
3. **Dynamic automation** - Respond to events with context-aware actions
4. **Secure secrets** - Store sensitive data in pack config, not rule definitions
5. **Flexible workflows** - Build complex automations without custom code

**Key Concepts:**
- Static values for constants
- `{{ event.payload.* }}` for event payload data
- `{{ event.id }}`, `{{ event.trigger }}`, `{{ event.created }}` for event metadata
- `{{ config.* }}` for pack configuration
- `{{ system.* }}` for system-provided values
- Filters and defaults for robust templates

This feature enables Attune to match the flexibility of platforms like StackStorm while maintaining a clean, declarative approach to automation.
