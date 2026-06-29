# API Client Quick Reference

> **TL;DR:** Use the auto-generated TypeScript client instead of manual API calls for type safety and schema validation.

## 🚀 Quick Start

### 1. Generate/Update Client

```bash
# Ensure API server is running first!
cd web
npm run generate:api
```

### 2. Import and Use

```typescript
import { PacksService, AuthService, ActionsService } from "@/api";
import type { CreatePackRequest, PackResponse } from "@/api";

// All API calls are now type-safe! ✅
const packs = await PacksService.listPacks({ page: 1, pageSize: 50 });
```

## 📚 Common Operations

### Authentication

```typescript
import { AuthService } from "@/api";

// Login
const response = await AuthService.login({
  requestBody: { login: "admin", password: "secret123" },
});
const { access_token, refresh_token, user } = response.data;

// Get current user
const currentUser = await AuthService.getCurrentUser();

// Refresh token
const refreshed = await AuthService.refreshToken({
  requestBody: { refresh_token },
});

// Register new user
await AuthService.register({
  requestBody: {
    login: "newuser",
    password: "strongpass",
    display_name: "New User",
  },
});
```

### Packs

```typescript
import { PacksService } from "@/api";
import type { CreatePackRequest, UpdatePackRequest } from "@/api";

// List packs
const packs = await PacksService.listPacks({ page: 1, pageSize: 50 });

// Get single pack
const pack = await PacksService.getPack({ ref: "core" });

// Create pack
const newPack = await PacksService.createPack({
  requestBody: {
    ref: "my-pack",
    label: "My Custom Pack",
    description: "A pack for custom automations",
    version: "1.0.0",
  },
});

// Update pack
const updated = await PacksService.updatePack({
  ref: "my-pack",
  requestBody: {
    label: "Updated Pack Name",
    description: "New description",
  },
});

// Delete pack
await PacksService.deletePack({ ref: "my-pack" });
```

### Actions

```typescript
import { ActionsService } from "@/api";

// List actions
const actions = await ActionsService.listActions({ page: 1, pageSize: 50 });

// Get action
const action = await ActionsService.getAction({ ref: "slack.post_message" });

// Create action
const newAction = await ActionsService.createAction({
  requestBody: {
    ref: "slack.post_message",
    pack: 1,
    label: "Post Message to Slack",
    description: "Posts a message to a Slack channel",
    entrypoint: "/actions/slack/post_message.py",
    param_schema: {/* JSON Schema */},
  },
});
```

### Rules

```typescript
import { RulesService } from "@/api";

// List rules
const rules = await RulesService.listRules({ page: 1, pageSize: 50 });

// Create rule
const rule = await RulesService.createRule({
  requestBody: {
    ref: "webhook-to-slack",
    label: "Webhook to Slack",
    pack: 1,
    trigger: "webhook.received",
    action: "slack.post_message",
    criteria: {/* Rule conditions */},
    parameters: {/* Action parameters */},
  },
});
```

### Executions

```typescript
import { ExecutionsService } from "@/api";

// List executions
const executions = await ExecutionsService.listExecutions({
  page: 1,
  perPage: 50,
  status: "Running", // Optional filter
});

// Get execution details
const execution = await ExecutionsService.getExecution({ id: 123 });

// Cancel execution
await ExecutionsService.cancelExecution({ id: 123 });

// Re-run execution
const rerun = await ExecutionsService.rerunExecution({ id: 123 });
```

### Events

```typescript
import { EventsService } from "@/api";

// List events
const events = await EventsService.listEvents({
  page: 1,
  perPage: 50,
  triggerRef: "webhook.received", // Optional filter
});

// Get event details
const event = await EventsService.getEvent({ id: 456 });
```

## 🔄 React Query Integration

### Query Hooks

```typescript
import { useQuery } from '@tanstack/react-query';
import { PacksService } from '@/api';

function PacksList() {
  const { data, isLoading, error } = useQuery({
    queryKey: ['packs'],
    queryFn: () => PacksService.listPacks({ page: 1, pageSize: 50 })
  });

  if (isLoading) return <div>Loading...</div>;
  if (error) return <div>Error: {error.message}</div>;

  return (
    <ul>
      {data?.data.items.map(pack => (
        <li key={pack.id}>{pack.label}</li>
      ))}
    </ul>
  );
}
```

### Mutation Hooks

```typescript
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { PacksService } from '@/api';
import type { CreatePackRequest } from '@/api';

function CreatePackForm() {
  const queryClient = useQueryClient();

  const mutation = useMutation({
    mutationFn: (data: CreatePackRequest) =>
      PacksService.createPack({ requestBody: data }),
    onSuccess: () => {
      // Invalidate and refetch
      queryClient.invalidateQueries({ queryKey: ['packs'] });
    }
  });

  const handleSubmit = (formData: CreatePackRequest) => {
    mutation.mutate(formData);
  };

  return (
    <form onSubmit={e => {
      e.preventDefault();
      handleSubmit({
        ref: 'my-pack',
        label: 'My Pack',
        description: 'Description'
      });
    }}>
      {/* form fields */}
      <button type="submit" disabled={mutation.isPending}>
        {mutation.isPending ? 'Creating...' : 'Create Pack'}
      </button>
    </form>
  );
}
```

## 🎯 Error Handling

```typescript
import { ApiError } from "@/api";

try {
  await PacksService.getPack({ ref: "nonexistent" });
} catch (error) {
  if (error instanceof ApiError) {
    console.error(`Status: ${error.status}`);
    console.error(`Message: ${error.message}`);
    console.error(`Body:`, error.body);

    switch (error.status) {
      case 401:
        // Unauthorized - redirect to login
        break;
      case 404:
        // Not found
        break;
      case 422:
        // Validation error
        console.error("Validation errors:", error.body);
        break;
      default:
        // Other errors
        break;
    }
  }
}
```

## 📦 Available Services

| Service               | Description            | Common Methods                                             |
| --------------------- | ---------------------- | ---------------------------------------------------------- |
| `AuthService`         | Authentication         | `login`, `register`, `getCurrentUser`, `refreshToken`      |
| `PacksService`        | Pack management        | `listPacks`, `getPack`, `createPack`, `updatePack`         |
| `ActionsService`      | Action CRUD            | `listActions`, `getAction`, `createAction`, `updateAction` |
| `RulesService`        | Rule management        | `listRules`, `getRule`, `createRule`, `updateRule`         |
| `TriggersService`     | Trigger config         | `listTriggers`, `getTrigger`, `createTrigger`              |
| `SensorsService`      | Sensor monitoring      | `listSensors`, `getSensor`, `createSensor`                 |
| `ExecutionsService`   | Execution tracking     | `listExecutions`, `getExecution`, `cancelExecution`        |
| `EventsService`       | Event history          | `listEvents`, `getEvent`                                   |
| `InquiriesService`    | Human-in-the-loop      | `listInquiries`, `getInquiry`, `respondToInquiry`          |
| `WorkflowsService`    | Workflow orchestration | `listWorkflows`, `getWorkflow`, `createWorkflow`           |
| `HealthService`       | Health checks          | `health`, `healthDetailed`, `healthReady`                  |
| `SecretsService`      | Secret management      | `listKeys`, `getKey`, `createKey`                          |
| `EnforcementsService` | Rule enforcements      | `listEnforcements`, `getEnforcement`                       |

## 🚨 Common Mistakes

### ❌ Don't Do This

```typescript
// Manual axios calls - NO TYPE SAFETY!
import { apiClient } from "@/lib/api-client";

const response = await apiClient.post("/api/v1/packs", {
  name: "my-pack", // ❌ Wrong field name
  system: false, // ❌ Wrong field name
});
```

### ✅ Do This Instead

```typescript
// Generated client - FULL TYPE SAFETY!
import { PacksService } from "@/api";
import type { CreatePackRequest } from "@/api";

const response = await PacksService.createPack({
  requestBody: {
    ref: "my-pack", // ✅ Correct field (enforced by TypeScript)
    label: "My Pack", // ✅ Correct field
    is_standard: false, // ✅ Correct field
  },
});
```

## 🔧 Troubleshooting

### Client out of sync with backend?

```bash
npm run generate:api
```

### Token not being sent?

Make sure `src/lib/api-config.ts` is imported in `src/main.tsx`:

```typescript
import "./lib/api-config"; // ← This line must be present
```

### TypeScript errors after generation?

The backend schema changed. Update your code to match the new schema:

1. Read the error messages
2. Check what changed in the OpenAPI spec at http://localhost:8080/docs
3. Update your code accordingly

## 📖 Full Documentation

- **Detailed Guide:** `src/api/README.md`
- **Migration Guide:** `MIGRATION-TO-GENERATED-CLIENT.md`
- **Architecture:** `../docs/openapi-client-generation.md`
- **Interactive Docs:** http://localhost:8080/docs

## 💡 Pro Tips

1. **Always regenerate after backend changes** to stay in sync
2. **Use TypeScript** - let the compiler catch errors
3. **Create custom hooks** - wrap services in React Query hooks
4. **Don't edit generated files** - they'll be overwritten
5. **Use generated types** - import from `@/api`, not manual definitions
6. **Leverage auto-completion** - your IDE knows the full API schema

---

**Remember:** The generated client is your single source of truth for API interactions. Use it exclusively for type safety and automatic schema validation! 🎉
