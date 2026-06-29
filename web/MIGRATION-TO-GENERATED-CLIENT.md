# Migration Guide: Using Generated API Client

This guide shows how to migrate from manual Axios API calls to the auto-generated OpenAPI client.

## Why Migrate?

✅ **Type Safety** - Catch API schema mismatches at compile time  
✅ **Auto-completion** - Full IDE support for all API methods  
✅ **Automatic Updates** - Regenerate when backend changes  
✅ **Less Code** - No manual type definitions needed  
✅ **Fewer Bugs** - Schema validation prevents runtime errors

## Quick Start

### 1. Import Services Instead of apiClient

**Before (Manual):**

```typescript
import { apiClient } from "@/lib/api-client";
```

**After (Generated):**

```typescript
import { AuthService, PacksService, ActionsService } from "@/api";
```

### 2. Use Service Methods Instead of HTTP Verbs

**Before (Manual):**

```typescript
const response = await apiClient.get("/api/v1/packs", {
  params: { page: 1, page_size: 50 },
});
```

**After (Generated):**

```typescript
const response = await PacksService.listPacks({
  page: 1,
  pageSize: 50,
});
```

## Real-World Examples

### Authentication (AuthContext)

**Before:**

```typescript
// src/contexts/AuthContext.tsx (OLD)
import { apiClient } from "@/lib/api-client";
import type {
  User,
  LoginRequest,
  LoginResponse,
  ApiResponse,
} from "@/types/api";

const login = async (credentials: LoginRequest) => {
  const response = await apiClient.post<ApiResponse<LoginResponse>>(
    "/auth/login",
    credentials,
  );

  const { access_token, refresh_token } = response.data.data;
  localStorage.setItem("access_token", access_token);
  localStorage.setItem("refresh_token", refresh_token);
};

const loadUser = async () => {
  const response = await apiClient.get<ApiResponse<User>>("/auth/me");
  setUser(response.data.data);
};
```

**After:**

```typescript
// src/contexts/AuthContext.tsx (NEW)
import { AuthService } from "@/api";
import type { UserInfo } from "@/api"; // Types are generated!

const login = async (credentials: { login: string; password: string }) => {
  const response = await AuthService.login({
    requestBody: credentials,
  });

  const { access_token, refresh_token } = response.data;
  localStorage.setItem("access_token", access_token);
  localStorage.setItem("refresh_token", refresh_token);
};

const loadUser = async () => {
  const response = await AuthService.getCurrentUser();
  setUser(response.data);
};
```

### Pack Management

**Before:**

```typescript
// Manual pack API calls
import { apiClient } from "@/lib/api-client";

interface Pack {
  id: number;
  ref: string;
  label: string;
  // ... manual type definition
}

const fetchPacks = async () => {
  const response = await apiClient.get<ApiResponse<PaginatedResponse<Pack>>>(
    "/api/v1/packs",
    { params: { page: 1, page_size: 50 } },
  );
  return response.data.data;
};

const createPack = async (data: any) => {
  const response = await apiClient.post("/api/v1/packs", data);
  return response.data.data;
};

const updatePack = async (ref: string, data: any) => {
  const response = await apiClient.put(`/api/v1/packs/${ref}`, data);
  return response.data.data;
};

const deletePack = async (ref: string) => {
  await apiClient.delete(`/api/v1/packs/${ref}`);
};
```

**After:**

```typescript
// Generated pack service (auto-typed!)
import { PacksService } from "@/api";
import type { CreatePackRequest, UpdatePackRequest } from "@/api";

const fetchPacks = async () => {
  const response = await PacksService.listPacks({
    page: 1,
    pageSize: 50,
  });
  return response.data; // Already typed as PaginatedResponse<PackSummary>
};

const createPack = async (data: CreatePackRequest) => {
  const response = await PacksService.createPack({ requestBody: data });
  return response.data; // Already typed as PackResponse
};

const updatePack = async (ref: string, data: UpdatePackRequest) => {
  const response = await PacksService.updatePack({ ref, requestBody: data });
  return response.data;
};

const deletePack = async (ref: string) => {
  await PacksService.deletePack({ ref });
};
```

### React Query Integration

**Before:**

```typescript
import { useQuery, useMutation } from "@tanstack/react-query";
import { apiClient } from "@/lib/api-client";

const { data } = useQuery({
  queryKey: ["packs"],
  queryFn: async () => {
    const response = await apiClient.get("/api/v1/packs");
    return response.data.data;
  },
});

const mutation = useMutation({
  mutationFn: async (data: any) => {
    const response = await apiClient.post("/api/v1/packs", data);
    return response.data.data;
  },
});
```

**After:**

```typescript
import { useQuery, useMutation } from "@tanstack/react-query";
import { PacksService } from "@/api";
import type { CreatePackRequest } from "@/api";

const { data } = useQuery({
  queryKey: ["packs"],
  queryFn: () => PacksService.listPacks({ page: 1, pageSize: 50 }),
});

const mutation = useMutation({
  mutationFn: (data: CreatePackRequest) =>
    PacksService.createPack({ requestBody: data }),
});
```

### Form Submissions

**Before:**

```typescript
const handleSubmit = async (formData: any) => {
  try {
    const response = await apiClient.post("/api/v1/packs", {
      name: formData.name, // ❌ Wrong field
      system: formData.system, // ❌ Wrong field
    });
    console.log(response.data.data);
  } catch (error) {
    // Runtime error when schema doesn't match!
  }
};
```

**After:**

```typescript
import type { CreatePackRequest } from "@/api";

const handleSubmit = async (formData: CreatePackRequest) => {
  try {
    const response = await PacksService.createPack({
      requestBody: {
        ref: formData.ref, // ✅ TypeScript enforces correct fields
        label: formData.label, // ✅ Compile-time validation
        is_standard: formData.is_standard,
      },
    });
    console.log(response.data);
  } catch (error) {
    // Caught at compile time!
  }
};
```

### Error Handling

**Before:**

```typescript
import { AxiosError } from "axios";

try {
  await apiClient.get("/api/v1/packs/unknown");
} catch (error) {
  if (error instanceof AxiosError) {
    console.error(error.response?.status);
  }
}
```

**After:**

```typescript
import { ApiError } from "@/api";

try {
  await PacksService.getPack({ ref: "unknown" });
} catch (error) {
  if (error instanceof ApiError) {
    console.error(`${error.status}: ${error.message}`);
    console.error(error.body); // Response body
  }
}
```

## Migration Checklist

### Phase 1: Setup ✅ (Already Done)

- [x] Install `openapi-typescript-codegen`
- [x] Add `generate:api` script to `package.json`
- [x] Generate initial API client
- [x] Create `src/lib/api-config.ts` for configuration
- [x] Import config in `src/main.tsx`

### Phase 2: Migrate Core Files

- [ ] Update `src/contexts/AuthContext.tsx` to use `AuthService`
- [ ] Update `src/types/api.ts` to re-export generated types
- [ ] Create custom hooks using generated services

### Phase 3: Migrate Pages

- [ ] Update all pack-related pages (`PacksPage`, `PackCreatePage`, etc.)
- [ ] Update all action-related pages
- [ ] Update all rule-related pages
- [ ] Update all execution-related pages
- [ ] Update all event-related pages

### Phase 4: Cleanup

- [ ] Remove manual API type definitions from `src/types/api.ts`
- [ ] Remove unused manual API calls
- [ ] Update all import statements
- [ ] Run TypeScript type checking: `npm run build`
- [ ] Test all workflows end-to-end

## Common Patterns

### Pattern 1: Create Custom Hooks

```typescript
// src/hooks/usePacks.ts
import { useQuery } from "@tanstack/react-query";
import { PacksService } from "@/api";

export const usePacks = (page = 1, pageSize = 50) => {
  return useQuery({
    queryKey: ["packs", page, pageSize],
    queryFn: () => PacksService.listPacks({ page, pageSize }),
  });
};

export const usePack = (ref: string) => {
  return useQuery({
    queryKey: ["pack", ref],
    queryFn: () => PacksService.getPack({ ref }),
    enabled: !!ref,
  });
};
```

### Pattern 2: Type-Safe Form Handling

```typescript
import { useForm } from 'react-hook-form';
import type { CreatePackRequest } from '@/api';

const PackForm = () => {
  const { register, handleSubmit } = useForm<CreatePackRequest>();

  const onSubmit = async (data: CreatePackRequest) => {
    await PacksService.createPack({ requestBody: data });
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)}>
      <input {...register('ref')} />
      <input {...register('label')} />
      <button type="submit">Create</button>
    </form>
  );
};
```

### Pattern 3: Optimistic Updates

```typescript
const mutation = useMutation({
  mutationFn: (data: UpdatePackRequest) =>
    PacksService.updatePack({ ref: packRef, requestBody: data }),
  onMutate: async (newData) => {
    await queryClient.cancelQueries({ queryKey: ["pack", packRef] });
    const previous = queryClient.getQueryData(["pack", packRef]);
    queryClient.setQueryData(["pack", packRef], newData);
    return { previous };
  },
  onError: (err, variables, context) => {
    queryClient.setQueryData(["pack", packRef], context?.previous);
  },
  onSettled: () => {
    queryClient.invalidateQueries({ queryKey: ["pack", packRef] });
  },
});
```

## Regenerating After API Changes

When the backend API changes:

1. **Start the API server:**

   ```bash
   cd crates/api
   cargo run --bin attune-api
   ```

2. **Regenerate the client:**

   ```bash
   cd web
   npm run generate:api
   ```

3. **Fix TypeScript errors:**

   ```bash
   npm run build
   ```

4. **Test your changes:**
   ```bash
   npm run dev
   ```

## Tips & Best Practices

1. **Always regenerate after backend changes** - Keep frontend in sync
2. **Use generated types** - Don't create duplicate manual types
3. **Leverage TypeScript** - Let the compiler catch schema mismatches
4. **Create custom hooks** - Wrap services in React Query hooks for reusability
5. **Don't edit generated files** - They'll be overwritten on next generation
6. **Use path aliases** - Import as `@/api` instead of `../../../api`

## Troubleshooting

### "Module not found: @/api"

Add to `vite.config.ts`:

```typescript
resolve: {
  alias: {
    '@': '/src'
  }
}
```

### "Property does not exist on type"

The backend schema changed. Regenerate the client:

```bash
npm run generate:api
```

### Token not being sent

Make sure `src/lib/api-config.ts` is imported in `src/main.tsx`:

```typescript
import "./lib/api-config";
```

## Resources

- Generated API docs: `src/api/README.md`
- OpenAPI spec: `http://localhost:8080/docs` (Swagger UI)
- Backend API code: `crates/api/src/routes/`
