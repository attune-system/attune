# Parameter Mapping Status

## Quick Reference

This document tracks the implementation status of rule parameter mapping — the system that resolves `{{ }}` template variables in rule `action_params` before passing them to action executions.

---

## ✅ Completed

### Database Schema
- **Migration:** `migrations/20240103000003_add_rule_action_params.sql`
- **Column:** `rule.action_params` (JSONB, default `{}`)
- **Index:** `idx_rule_action_params_gin` (GIN index for efficient querying)

### Data Models
- **File:** `crates/common/src/models.rs`
- **Struct:** `rule::Rule` has `pub action_params: JsonValue` field

### API Layer
- **File:** `crates/api/src/dto/rule.rs`
- **Request DTOs:**
  - `CreateRuleRequest.action_params` (with default `{}`)
  - `UpdateRuleRequest.action_params` (optional)
- **Response DTOs:**
  - `RuleResponse.action_params`
  - `RuleSummary.action_params`

### Repository Layer
- **File:** `crates/common/src/repositories/rule.rs`
- **Operations:** CREATE, UPDATE, and SELECT all handle `action_params`

### Template Resolver Module
- **File:** `crates/common/src/template_resolver.rs`
- **Struct:** `TemplateContext` with `event`, `pack_config`, and `system_vars` fields
- **Function:** `resolve_templates()` — recursively resolves `{{ }}` templates in JSON values
- **Re-exported** from `attune_common::template_resolver` and `attune_common::{TemplateContext, resolve_templates}`
- **Also re-exported** from `attune_sensor::template_resolver` for backward compatibility
- **20 unit tests** covering all template features

### Template Syntax

**Available Sources:**

| Namespace | Example | Description |
|-----------|---------|-------------|
| `event.payload.*` | `{{ event.payload.service }}` | Event payload data |
| `event.id` | `{{ event.id }}` | Event database ID |
| `event.trigger` | `{{ event.trigger }}` | Trigger ref that generated the event |
| `event.created` | `{{ event.created }}` | Event creation timestamp (RFC 3339) |
| `config.*` | `{{ config.api_token }}` | Pack configuration values |
| `system.*` | `{{ system.timestamp }}` | System-provided variables |

### Integration in Executor
- **File:** `crates/executor/src/event_processor.rs`
- **Method:** `resolve_action_params()` builds a `TemplateContext` from the event and rule, then calls `resolve_templates()`
- **Context includes:**
  - `event.id`, `event.trigger`, `event.created`, `event.payload.*` from the `Event` model
  - `system.timestamp` (current time), `system.rule.id`, `system.rule.ref`
- **Called during:** enforcement creation in `create_enforcement()`

### Data Flow
```
Rule.action_params (templates)
  ↓  resolve_templates() in EventProcessor
Enforcement.config (resolved values)
  ↓
Execution.config (passed through)
  ↓
Worker (receives as action parameters)
```

### Template Features
- ✅ Static values pass through unchanged
- ✅ Single-template type preservation (numbers, booleans, objects, arrays)
- ✅ String interpolation with multiple templates
- ✅ Nested object access via dot notation (`event.payload.metadata.host`)
- ✅ Array element access by index (`event.payload.tags.0`)
- ✅ Missing values resolve to `null` with warning logged
- ✅ Empty/null action_params handled gracefully

### Documentation
- ✅ `docs/workflows/rule-parameter-mapping.md` — comprehensive user guide
- ✅ `docs/examples/rule-parameter-examples.md` — real-world examples
- ✅ `docs/api/api-rules.md` — API documentation
- ✅ Inline code documentation in `template_resolver.rs`

---

## Pack Configuration

### Pack Config Loading
- **Current:** Pack configuration is loaded for template resolution
- **Canonical namespace:** Use `{{ config.* }}`

---

## 📋 Remaining Work

### Phase 1: Complete Core (Short-term)

- [x] **Pack config loading** — Load pack config for `{{ config.* }}` resolution
- [ ] **Integration tests** — End-to-end test: create rule with templates → fire event → verify enforcement has resolved params

### Phase 2: Advanced Features (Future)

- [ ] **Default values** — Parse `| default: 'value'` syntax for fallback values
- [ ] **Filters** — `upper`, `lower`, `trim`, `date`, `truncate`, `json`
- [ ] **Conditional templates** — `{% if event.payload.severity == 'critical' %}...{% endif %}`
- [ ] **Performance** — Skip resolution early if no `{{ }}` patterns detected in action_params

---

## 🔍 Template Example

**Input (Rule `action_params`):**
```json
{
  "message": "Error in {{ event.payload.service }}: {{ event.payload.message }}",
  "channel": "{{ config.alert_channel }}",
  "severity": "{{ event.payload.severity }}",
  "event_id": "{{ event.id }}",
  "trigger": "{{ event.trigger }}"
}
```

**Context (built from Event + Rule):**
```json
{
  "event": {
    "id": 456,
    "trigger": "core.error_event",
    "created": "2026-02-05T10:00:00Z",
    "payload": {
      "service": "api-gateway",
      "message": "Connection timeout",
      "severity": "critical"
    }
  },
  "pack": {
    "config": {
      "alert_channel": "#incidents"
    }
  },
  "system": {
    "timestamp": "2026-02-05T10:00:01Z",
    "rule": { "id": 42, "ref": "alerts.error_notification" }
  }
}
```

**Output (Enforcement `config`):**
```json
{
  "message": "Error in api-gateway: Connection timeout",
  "channel": "#incidents",
  "severity": "critical",
  "event_id": 456,
  "trigger": "core.error_event"
}
```

---

## Related Documentation

- [Rule Parameter Mapping Guide](./rule-parameter-mapping.md)
- [Rule Parameter Examples](../examples/rule-parameter-examples.md)
- [Rule Management API](../api/api-rules.md)
- [Executor Service Architecture](../architecture/executor-service.md)
