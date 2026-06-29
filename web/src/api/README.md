# Generated API Client

This directory contains auto-generated TypeScript client code for the Attune API, created from the OpenAPI specification.

> **⚠️ DO NOT EDIT FILES IN THIS DIRECTORY**  
> All files are auto-generated. Manual changes will be overwritten when the API client is regenerated.

## Regenerating the Client

Whenever the backend API changes, regenerate the client:

```bash
npm run generate:api
```

This command:

1. Fetches the latest OpenAPI spec from the running API server
2. Generates TypeScript types and service classes
3. Overwrites all files in `src/api/`

**Prerequisites:** The API server must be running at `http://localhost:8080`

## Usage

### 1. Import and Configure (already done in `src/lib/api-config.ts`)

```typescript
import { OpenAPI } from "./api";

// Set base URL
OpenAPI.BASE = "http://localhost:8080";

// Configure automatic JWT token injection
OpenAPI.TOKEN = async () => {
  return localStorage.getItem("access_token") || undefined;
};
```

### 2. Use Service Classes

Each API endpoint group has a corresponding service class:

```typescript
import { PacksService, AuthService, ActionsService } from "@/api";

// Example: Login
const response = await AuthService.login({
  requestBody: {
    login: "admin",
    password: "password123",
  },
});

const { access_token, user } = response.data;

// Example: List packs
const packs = await PacksService.listPacks({
  page: 1,
  pageSize: 50,
});

console.log(packs.data.items);

// Example: Create an action
const action = await ActionsService.createAction({
  requestBody: {
    ref: "slack.post_message",
    pack: 1,
    label: "Post Message to Slack",
    description: "Posts a message to a Slack channel",
    entrypoint: "/actions/slack/post_message.py",
    param_schema: {/* ... */},
  },
});
```

### 3. TypeScript Types

All request/response types are available:

```typescript
import type {
  PackResponse,
  CreatePackRequest,
  PaginatedResponse_PackSummary,
  ExecutionStatus,
} from "@/api";

const createPack = async (data: CreatePackRequest) => {
  const response = await PacksService.createPack({ requestBody: data });
  return response.data;
};
```

## Available Services

- **AuthService** - Authentication (login, register, refresh, etc.)
- **PacksService** - Pack management
- **ActionsService** - Action CRUD operations
- **RulesService** - Rule management
- **TriggersService** - Trigger management
- **SensorsService** - Sensor management
- **ExecutionsService** - Execution tracking
- **EventsService** - Event monitoring
- **InquiriesService** - Human-in-the-loop workflows
- **WorkflowsService** - Workflow orchestration
- **HealthService** - Health checks

## Error Handling

The generated client throws `ApiError` for HTTP errors:

```typescript
import { ApiError } from "@/api";

try {
  await PacksService.getPack({ ref: "nonexistent" });
} catch (error) {
  if (error instanceof ApiError) {
    console.error(`API Error ${error.status}: ${error.message}`);
    console.error("Response body:", error.body);
  }
}
```

## Integration with React Query

Combine with TanStack Query for optimal data fetching:

```typescript
import { useQuery, useMutation } from "@tanstack/react-query";
import { PacksService } from "@/api";

// Query
const { data, isLoading } = useQuery({
  queryKey: ["packs"],
  queryFn: () => PacksService.listPacks({ page: 1, pageSize: 50 }),
});

// Mutation
const { mutate } = useMutation({
  mutationFn: (data: CreatePackRequest) =>
    PacksService.createPack({ requestBody: data }),
  onSuccess: () => {
    queryClient.invalidateQueries({ queryKey: ["packs"] });
  },
});
```

## Benefits of Using Generated Client

✅ **Type Safety** - Full TypeScript types for all API requests/responses  
✅ **Auto-completion** - IDE support for all API methods and parameters  
✅ **Schema Validation** - Ensures frontend matches backend API contract  
✅ **Automatic Updates** - Regenerate when API changes to stay in sync  
✅ **Reduced Errors** - Catch API mismatches at compile time, not runtime  
✅ **Documentation** - JSDoc comments from OpenAPI spec included

## Comparison: Manual vs Generated

### ❌ Manual Axios Calls (Don't do this)

```typescript
// NO type safety, easy to make mistakes
const response = await apiClient.post("/api/v1/packs", {
  name: "my-pack", // Wrong field! Should be 'ref'
  system: false, // Wrong field! Should be 'is_standard'
});
```

### ✅ Generated Client (Do this)

```typescript
// Compile-time errors if schema doesn't match!
const response = await PacksService.createPack({
  requestBody: {
    ref: "my-pack", // ✅ Correct
    label: "My Pack", // ✅ Correct
    is_standard: false, // ✅ Correct
  },
});
```

## Troubleshooting

### "Cannot find module '@/api'"

Add path alias to `tsconfig.json`:

```json
{
  "compilerOptions": {
    "paths": {
      "@/*": ["./src/*"]
    }
  }
}
```

### "openapi-typescript-codegen: command not found"

### "command not found: openapi-typescript-codegen"

This shouldn't happen since the script uses `npx`, but if it does:

```bash
# Ensure dependencies are installed
npm install

# The script already uses npx, but you can run manually:
npx openapi-typescript-codegen --input ./openapi.json --output ./src/api --client axios --useOptions
```

### API Server Not Running

Make sure the API service is running before generating:

```bash
# In the attune/crates/api directory
cargo run --bin attune-api
```
