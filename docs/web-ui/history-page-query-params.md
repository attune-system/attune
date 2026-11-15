# History Page URL Query Parameters

This document describes the URL query parameters supported by the history pages (Executions, Events, Enforcements) in the Attune web UI.

## Overview

All history pages support deep linking via URL query parameters. When navigating to a history page with query parameters, the page will automatically initialize its filters with the provided values.

## Executions Page

**Path**: `/executions`

### Supported Query Parameters

| Parameter | Description | Example |
|-----------|-------------|---------|
| `action_ref` | Filter by action reference | `?action_ref=core.echo` |
| `rule_ref` | Filter by rule reference | `?rule_ref=core.on_timer` |
| `trigger_ref` | Filter by trigger reference | `?trigger_ref=core.webhook` |
| `pack_name` | Filter by pack name | `?pack_name=core` |
| `executor` | Filter by executor ID | `?executor=1` |
| `status` | Filter by execution status | `?status=running` |

### Valid Status Values

- `requested`
- `scheduling`
- `scheduled`
- `running`
- `completed`
- `failed`
- `canceling`
- `cancelled`
- `timeout`
- `abandoned`

### Examples

```
# Filter by action
http://localhost:3000/executions?action_ref=core.echo

# Filter by rule and status
http://localhost:3000/executions?rule_ref=core.on_timer&status=completed

# Multiple filters
http://localhost:3000/executions?pack_name=core&status=running&action_ref=core.echo
```

## Events Page

**Path**: `/events`

### Supported Query Parameters

| Parameter | Description | Example |
|-----------|-------------|---------|
| `trigger_ref` | Filter by trigger reference | `?trigger_ref=core.webhook` |

### Examples

```
# Filter by trigger
http://localhost:3000/events?trigger_ref=core.webhook

# Filter by timer trigger
http://localhost:3000/events?trigger_ref=core.timer
```

## Enforcements Page

**Path**: `/enforcements`

### Supported Query Parameters

| Parameter | Description | Example |
|-----------|-------------|---------|
| `rule_ref` | Filter by rule reference | `?rule_ref=core.on_timer` |
| `trigger_ref` | Filter by trigger reference | `?trigger_ref=core.webhook` |
| `event` | Filter by event ID | `?event=123` |
| `status` | Filter by enforcement status | `?status=processed` |

### Valid Status Values

- `created`
- `processed`
- `disabled`

### Examples

```
# Filter by rule
http://localhost:3000/enforcements?rule_ref=core.on_timer

# Filter by event
http://localhost:3000/enforcements?event=123

# Multiple filters
http://localhost:3000/enforcements?rule_ref=core.on_timer&status=processed
```

## Usage Patterns

### Deep Linking from Detail Pages

When viewing a specific execution, event, or enforcement detail page, you can click on related entities (actions, rules, triggers) to navigate to the history page with the appropriate filter pre-applied.

### Sharing Filtered Views

You can share URLs with query parameters to help others view specific filtered data sets:

```
# Share a view of all failed executions for a specific action
http://localhost:3000/executions?action_ref=example.http_request&status=failed

# Share enforcements for a specific rule
http://localhost:3000/enforcements?rule_ref=my_pack.important_rule
```

### Bookmarking

Save frequently used filter combinations as browser bookmarks for quick access.

## Implementation Notes

- Query parameters are read on page load and initialize the filter state
- Changing filters in the UI does **not** update the URL (stateless filtering)
- Multiple query parameters can be combined
- Invalid parameter values are ignored (filters default to empty)
- Parameter names match the API field names for consistency