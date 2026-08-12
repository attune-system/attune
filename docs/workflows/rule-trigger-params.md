# Rule Trigger Parameters

## Overview

Rules in Attune can now specify `trigger_params` to configure trigger behavior and filter which events should activate the rule. This complements `action_params` (which configures the action to execute) by providing control over trigger matching and event filtering.

---

## What are Trigger Params?

**Trigger params** are JSON parameters stored in a rule that can be used to:

1. **Filter events** - Only match events with specific payload characteristics
2. **Configure trigger behavior** - Customize how the trigger should match events for this specific rule
3. **Pass metadata** - Provide additional context about how events should be processed

This allows multiple rules to reference the same trigger type but respond to different subsets of events.

---

## Use Cases

### 1. Event Filtering by Severity

**Scenario:** You have a generic error trigger, but different rules should handle different severity levels.

```json
{
  "ref": "alerts.critical_errors",
  "trigger_ref": "core.error_event",
  "action_ref": "pagerduty.create_incident",
  "trigger_params": {
    "severity": "critical",
    "min_priority": 5
  },
  "action_params": {
    "routing_key": "{{ config.pagerduty_key }}",
    "severity": "critical"
  }
}
```

```json
{
  "ref": "alerts.minor_errors",
  "trigger_ref": "core.error_event",
  "action_ref": "slack.post_message",
  "trigger_params": {
    "severity": "warning",
    "max_priority": 2
  },
  "action_params": {
    "channel": "#monitoring"
  }
}
```

Both rules use the same `core.error_event` trigger, but `trigger_params` specifies which events each rule should handle.

### 2. Service-Specific Monitoring

**Scenario:** Monitor multiple services with the same trigger type, but different rules per service.

```json
{
  "ref": "monitoring.api_gateway_health",
  "trigger_ref": "core.health_check",
  "action_ref": "alerts.notify_team",
  "trigger_params": {
    "service": "api-gateway",
    "environment": "production"
  },
  "action_params": {
    "team": "backend"
  }
}
```

```json
{
  "ref": "monitoring.database_health",
  "trigger_ref": "core.health_check",
  "action_ref": "alerts.notify_team",
  "trigger_params": {
    "service": "postgresql",
    "environment": "production"
  },
  "action_params": {
    "team": "database"
  }
}
```

### 3. Threshold-Based Rules

**Scenario:** Different rules for different metric thresholds.

```json
{
  "ref": "metrics.cpu_high_warning",
  "trigger_ref": "monitoring.cpu_usage",
  "action_ref": "slack.post_message",
  "trigger_params": {
    "threshold": 80,
    "comparison": "greater_than",
    "duration_seconds": 300
  },
  "action_params": {
    "channel": "#ops"
  }
}
```

```json
{
  "ref": "metrics.cpu_critical",
  "trigger_ref": "monitoring.cpu_usage",
  "action_ref": "pagerduty.create_incident",
  "trigger_params": {
    "threshold": 95,
    "comparison": "greater_than",
    "duration_seconds": 60
  },
  "action_params": {
    "routing_key": "{{ config.pagerduty_key }}"
  }
}
```

---

## How Trigger Params Work

### Architecture Flow

```
Sensor → Generates Event with Payload
    ↓
Trigger Type Matched
    ↓
Rules Evaluated (foreach rule matching trigger type):
    1. Check trigger_params against event payload
    2. Evaluate conditions
    3. If both pass → create Enforcement
    ↓
Enforcement → Execution (with action_params)
```

### Evaluation Logic

When an event fires:

1. **Find matching rules** - All rules that reference the event's trigger type
2. **Filter by trigger_params** - For each rule, check if the event payload matches the rule's `trigger_params`
3. **Evaluate conditions** - Apply the rule's `conditions` logic
4. **Create enforcement** - If both checks pass, activate the rule

---

## Trigger Params vs Conditions

Both `trigger_params` and `conditions` can filter events, but they serve different purposes:

| Feature | `trigger_params` | `conditions` |
|---------|------------------|--------------|
| **Purpose** | Declare intent about which events this rule handles | Complex conditional logic for rule activation |
| **Format** | Simple JSON key-value pairs | JSON Logic expressions or complex DSL |
| **Evaluation** | Direct comparison/matching | Expression evaluation engine |
| **Use Case** | Event filtering, metadata | Business logic, complex conditions |
| **Performance** | Fast direct matching | May require expression parsing |

### Example: Using Both Together

```json
{
  "ref": "alerts.critical_api_errors",
  "trigger_ref": "core.error_event",
  "trigger_params": {
    "service": "api-gateway",
    "severity": "error"
  },
  "conditions": {
    "and": [
      {"var": "event.payload.status_code", ">=": 500},
      {"var": "event.payload.retry_count", ">": 3},
      {
        "or": [
          {"var": "event.payload.endpoint", "in": ["/auth", "/payment"]},
          {"var": "event.payload.customer_impact", "==": true}
        ]
      }
    ]
  },
  "action_params": {
    "priority": "P1"
  }
}
```

Here:
- `trigger_params` declares: "This rule handles API Gateway errors"
- `conditions` adds: "But only if status >= 500, retries > 3, AND it's a critical endpoint or impacts customers"

---

## Implementation in Different Services

### Executor Service

The executor service uses `trigger_params` when evaluating which rules should fire for an event:

```rust
// Pseudo-code
async fn evaluate_rules_for_event(event: &Event) -> Vec<Enforcement> {
    let rules = find_rules_by_trigger(event.trigger_id);
    let mut enforcements = Vec::new();
    
    for rule in rules {
        // Check trigger_params match
        if !matches_trigger_params(&rule.trigger_params, &event.payload) {
            continue; // Skip this rule
        }
        
        // Check conditions
        if !evaluate_conditions(&rule.conditions, &event.payload) {
            continue;
        }
        
        // Rule matches - create enforcement
        enforcements.push(create_enforcement(&rule, &event));
    }
    
    enforcements
}
```

### API Service

The API allows setting `trigger_params` when creating or updating rules:

**Create Rule Request:**
```json
POST /api/v1/rules
{
  "ref": "mypack.my_rule",
  "pack_ref": "mypack",
  "trigger_ref": "core.webhook",
  "action_ref": "slack.post_message",
  "trigger_params": {
    "webhook_source": "github",
    "event_type": "pull_request"
  },
  "action_params": {
    "channel": "#github-prs"
  },
  "enabled": true
}
```

**Update Rule Request:**
```json
PUT /api/v1/rules/mypack.my_rule
{
  "trigger_params": {
    "webhook_source": "github",
    "event_type": ["pull_request", "push"]
  }
}
```

---

## Best Practices

### 1. Use Trigger Params for Simple Filtering

**Good:**
```json
{
  "trigger_params": {
    "severity": "critical",
    "service": "api"
  }
}
```

**Not Recommended (use conditions instead):**
```json
{
  "trigger_params": {
    "complex_logic": "if severity > 3 and (service == 'api' or service == 'web')"
  }
}
```

### 2. Keep Trigger Params Declarative

Trigger params should describe *what* events to match, not *how* to process them:

**Good:**
```json
{
  "trigger_params": {
    "environment": "production",
    "region": "us-east-1"
  }
}
```

**Bad:**
```json
{
  "trigger_params": {
    "should_page_oncall": true,
    "escalation_policy": "immediate"
  }
}
```

The second example describes processing behavior (belongs in action_params).

### 3. Use Empty Object as Default

If a rule should match all events from a trigger, use an empty object:

```json
{
  "trigger_params": {}
}
```

This explicitly states "no filtering, match all events."

### 4. Document Expected Fields

When creating trigger types, document what `trigger_params` fields are expected:

```yaml
# Trigger: core.error_event
# Expected trigger_params:
#   - severity: string (error|warning|info)
#   - service: string (optional, filter by service name)
#   - min_priority: number (optional, minimum priority level)
```

### 5. Combine with Conditions for Complex Logic

Use `trigger_params` for simple key-value filtering, and `conditions` for complex expressions:

```json
{
  "trigger_params": {
    "event_type": "metric_alert"
  },
  "conditions": {
    "and": [
      {"var": "metric_value", ">": 100},
      {"var": "duration_minutes", ">=": 5}
    ]
  }
}
```

---

## Schema and Validation

### Database Schema

```sql
-- In attune.rule table
trigger_params JSONB DEFAULT '{}'::jsonb
```

### API Schema

**OpenAPI Definition:**
```yaml
trigger_params:
  type: object
  description: Parameters for trigger configuration and event filtering
  default: {}
  example:
    severity: high
    service: api-gateway
```

### Runtime Validation

Trigger params are stored as JSON and validated at runtime:
- Must be valid JSON object
- Keys should match trigger's param_schema (if defined)
- Values are compared against event payload during evaluation

---

## Migration Guide

If you have existing rules without `trigger_params`, they will default to `{}` (empty object), which means "match all events from this trigger."

No action is required for existing rules unless you want to add filtering.

### Adding Trigger Params to Existing Rules

**Before:**
```json
{
  "ref": "alerts.notify_errors",
  "trigger_ref": "core.error_event",
  "action_ref": "slack.post_message",
  "conditions": {
    "var": "severity",
    "==": "critical"
  }
}
```

**After (moving simple filtering to trigger_params):**
```json
{
  "ref": "alerts.notify_errors",
  "trigger_ref": "core.error_event",
  "action_ref": "slack.post_message",
  "trigger_params": {
    "severity": "critical"
  },
  "conditions": {}
}
```

This improves performance by filtering earlier in the evaluation pipeline.

---

## Examples

### Example 1: Webhook Source Filtering

```json
{
  "ref": "webhooks.github_pr_opened",
  "trigger_ref": "core.webhook_received",
  "trigger_params": {
    "source": "github",
    "event": "pull_request",
    "action": "opened"
  },
  "action_ref": "slack.post_message",
  "action_params": {
    "channel": "#pull-requests",
    "message": "New PR: {{ event.payload.title }} by {{ event.payload.user }}"
  }
}
```

### Example 2: Multi-Environment Monitoring

```json
{
  "ref": "monitoring.prod_cpu_alert",
  "trigger_ref": "monitoring.cpu_threshold",
  "trigger_params": {
    "environment": "production",
    "threshold_type": "critical"
  },
  "action_ref": "pagerduty.create_incident",
  "action_params": {
    "severity": "critical"
  }
}
```

```json
{
  "ref": "monitoring.staging_cpu_alert",
  "trigger_ref": "monitoring.cpu_threshold",
  "trigger_params": {
    "environment": "staging",
    "threshold_type": "warning"
  },
  "action_ref": "slack.post_message",
  "action_params": {
    "channel": "#staging-alerts"
  }
}
```

### Example 3: Timer with Context

```json
{
  "ref": "backups.hourly_db_backup",
  "trigger_ref": "core.intervaltimer",
  "trigger_params": {
    "interval_minutes": 60,
    "context": "database_backup"
  },
  "action_ref": "backups.run_backup",
  "action_params": {
    "backup_type": "incremental",
    "retention_days": 7
  }
}
```

---

## Related Documentation

- [Rule Parameter Mapping](./rule-parameter-mapping.md) - Dynamic parameters from event payload
- [Trigger and Sensor Architecture](./trigger-sensor-architecture.md) - How triggers and sensors work
- [API Rules Endpoint](./api-rules.md) - Creating and managing rules via API

---

## Summary

- **`trigger_params`** provides a way to filter which events activate a rule
- Use for simple key-value filtering and event categorization
- Complements `conditions` for complex business logic
- Improves rule organization when multiple rules share the same trigger type
- Defaults to `{}` (match all events) if not specified
- Stored as JSONB in the database for flexible querying
