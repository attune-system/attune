# Policy API

Policies are first-class execution controls for concurrency, rate limits, and quotas. Attune resolves one effective policy for an execution: action-scoped policies override pack-scoped policies, pack-scoped policies override global policies, and higher `priority` wins within the same scope.

## Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/api/v1/policies` | List policies with optional `scope`, `pack_ref`, `action_ref`, `enabled`, and `tag` filters |
| `POST` | `/api/v1/policies` | Create a policy |
| `GET` | `/api/v1/policies/{ref}` | Get a policy |
| `PUT` | `/api/v1/policies/{ref}` | Update a policy |
| `DELETE` | `/api/v1/policies/{ref}` | Delete a policy |
| `GET` | `/api/v1/packs/{pack_ref}/policies` | List policies for a pack |
| `GET` | `/api/v1/actions/{action_ref}/policies` | List policies for an action |

## Request shape

```json
{
  "ref": "core.limit_echo",
  "name": "Limit echo",
  "description": "Limit concurrent echo executions",
  "enabled": true,
  "priority": 10,
  "scope": { "type": "action", "action_ref": "core.echo" },
  "concurrency": {
    "limit": 5,
    "method": "enqueue",
    "parameters": ["customer_id"]
  },
  "rate_limit": {
    "max_executions": 100,
    "window_seconds": 3600
  },
  "quotas": [
    { "quota_type": "running_executions", "limit": 10 },
    { "quota_type": "executions_total", "limit": 1000 }
  ],
  "tags": ["production"]
}
```

Supported scopes are `global`, `pack`, and `action`. Supported concurrency methods are `enqueue` and `cancel`. Supported quota types are `running_executions` and `executions_total`.

## Pack YAML

Pack-managed policies live in `policies/*.yaml` and are loaded after actions/queues and before rules/sensors:

```yaml
ref: my_pack.limit_deploy
name: Limit deploys
enabled: true
priority: 20
action_ref: my_pack.deploy
concurrency:
  limit: 2
  method: enqueue
  parameters:
    - environment
rate_limit:
  max_executions: 20
  window_seconds: 3600
quotas:
  - quota_type: running_executions
    limit: 5
tags:
  - production
```
