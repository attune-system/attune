# History Page URL Query Parameter Examples

This document provides practical examples of using URL query parameters to deep-link to filtered views in the Attune web UI history pages.

## Executions Page Examples

### Basic Filtering

**Filter by action:**
```
http://localhost:3000/executions?action_ref=core.echo
```
Shows all executions of the `core.echo` action.

**Filter by rule:**
```
http://localhost:3000/executions?rule_ref=core.on_timer
```
Shows all executions triggered by the `core.on_timer` rule.

**Filter by status:**
```
http://localhost:3000/executions?status=failed
```
Shows all failed executions.

**Filter by pack:**
```
http://localhost:3000/executions?pack_name=core
```
Shows all executions from the `core` pack.

### Combined Filters

**Rule + Status:**
```
http://localhost:3000/executions?rule_ref=core.on_timer&status=completed
```
Shows completed executions from a specific rule.

**Action + Pack:**
```
http://localhost:3000/executions?action_ref=core.echo&pack_name=core
```
Shows executions of a specific action in a pack (useful when multiple packs have similarly named actions).

**Multiple Filters:**
```
http://localhost:3000/executions?pack_name=core&status=running&trigger_ref=core.webhook
```
Shows currently running executions from the core pack triggered by webhooks.

### Troubleshooting Scenarios

**Find all failed executions for an action:**
```
http://localhost:3000/executions?action_ref=mypack.problematic_action&status=failed
```

**Check running executions for a specific executor:**
```
http://localhost:3000/executions?executor=1&status=running
```

**View all webhook-triggered executions:**
```
http://localhost:3000/executions?trigger_ref=core.webhook
```

## Events Page Examples

### Basic Filtering

**Filter by trigger:**
```
http://localhost:3000/events?trigger_ref=core.webhook
```
Shows all webhook events.

**Timer events:**
```
http://localhost:3000/events?trigger_ref=core.timer
```
Shows all timer-based events.

**Custom trigger:**
```
http://localhost:3000/events?trigger_ref=mypack.custom_trigger
```
Shows events from a custom trigger.

## Enforcements Page Examples

### Basic Filtering

**Filter by rule:**
```
http://localhost:3000/enforcements?rule_ref=core.on_timer
```
Shows all enforcements (rule activations) for a specific rule.

**Filter by trigger:**
```
http://localhost:3000/enforcements?trigger_ref=core.webhook
```
Shows all enforcements triggered by webhook events.

**Filter by event:**
```
http://localhost:3000/enforcements?event=123
```
Shows the enforcement created by a specific event (useful for tracing event → enforcement → execution flow).

**Filter by status:**
```
http://localhost:3000/enforcements?status=processed
```
Shows processed enforcements.

### Combined Filters

**Rule + Status:**
```
http://localhost:3000/enforcements?rule_ref=core.on_timer&status=processed
```
Shows successfully processed enforcements for a specific rule.

**Trigger + Event:**
```
http://localhost:3000/enforcements?trigger_ref=core.webhook&event=456
```
Shows enforcements from a specific webhook event.

## Practical Use Cases

### Debugging a Rule

1. **Check the event was created:**
   ```
   http://localhost:3000/events?trigger_ref=core.timer
   ```

2. **Check the enforcement was created:**
   ```
   http://localhost:3000/enforcements?rule_ref=core.on_timer
   ```

3. **Check the execution was triggered:**
   ```
   http://localhost:3000/executions?rule_ref=core.on_timer
   ```

### Monitoring Action Performance

**See all executions of an action:**
```
http://localhost:3000/executions?action_ref=example.http_request
```

**See failures:**
```
http://localhost:3000/executions?action_ref=example.http_request&status=failed
```

**See currently running:**
```
http://localhost:3000/executions?action_ref=example.http_request&status=running
```

### Auditing Webhook Activity

1. **View all webhook events:**
   ```
   http://localhost:3000/events?trigger_ref=core.webhook
   ```

2. **View enforcements from webhooks:**
   ```
   http://localhost:3000/enforcements?trigger_ref=core.webhook
   ```

3. **View executions triggered by webhooks:**
   ```
   http://localhost:3000/executions?trigger_ref=core.webhook
   ```

### Sharing Views with Team Members

**Share failed executions for investigation:**
```
http://localhost:3000/executions?action_ref=mypack.critical_action&status=failed
```

**Share rule activity for review:**
```
http://localhost:3000/enforcements?rule_ref=mypack.important_rule&status=processed
```

## Tips and Notes

1. **URL Encoding**: If your pack, action, rule, or trigger names contain special characters, they will be automatically URL-encoded by the browser.

2. **Case Sensitivity**: Parameter names and values are case-sensitive. Use lowercase for status values (e.g., `status=failed`, not `status=Failed`).

3. **Invalid Values**: Invalid parameter values are silently ignored, and the filter will default to empty (showing all results).

4. **Bookmarking**: Save frequently used URLs as browser bookmarks for quick access to common filtered views.

5. **Browser History**: The URL doesn't change as you modify filters in the UI, so the browser's back button won't undo filter changes within a page.

6. **Multiple Status Filters**: While the UI allows selecting multiple statuses, only one status can be specified via URL parameter. Use the UI to select multiple statuses after the page loads.

## Parameter Reference Quick Table

| Page | Parameter | Example Value |
|------|-----------|---------------|
| Executions | `action_ref` | `core.echo` |
| Executions | `rule_ref` | `core.on_timer` |
| Executions | `trigger_ref` | `core.webhook` |
| Executions | `pack_name` | `core` |
| Executions | `executor` | `1` |
| Executions | `status` | `failed`, `running`, `completed` |
| Events | `trigger_ref` | `core.webhook` |
| Enforcements | `rule_ref` | `core.on_timer` |
| Enforcements | `trigger_ref` | `core.webhook` |
| Enforcements | `event` | `123` |
| Enforcements | `status` | `processed`, `created`, `disabled` |